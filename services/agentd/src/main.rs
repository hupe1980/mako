//! `agentd` — Multi-agent LLM orchestration daemon for mako.
//!
//! ## Architecture
//!
//! ```text
//! CloudEvent → OrchestratorAgent
//!   ├── trigger_patterns → SpecialistAgent (direct)
//!   └── LLM triage → SpecialistAgent (via handoff tool)
//!         ↓ ReAct loop (MCP tools + peer handoffs)
//!         ↓ RAG context from LanceDB (S3/GCS/local)
//!         ↓ de.agent.decision.made → marktd audit log
//! ```
//!
//! ## LLM Providers
//!
//! | Provider | Config `backend` | Env vars |
//! |---|---|---|
//! | OpenAI / Azure / Ollama | `"openai"` | (api_key in config) |
//! | Anthropic Claude | `"anthropic"` | (api_key in config) |
//! | AWS Bedrock | `"bedrock"` | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` |
//!
//! ## Port: 9580

use agentd::{handlers, llm};
use std::sync::Arc;

use anyhow::Context as _;
use axum::{Router, routing::post};
use mako_service::{health::health_routes, load_config};
use tracing::info;

use agentd::{
    agent::{AgentRegistry, OrchestratorAgent},
    config::AgentdConfig,
    handlers::AppState,
    mcp::McpPool,
    rag::RagEngine,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentd=info".into()),
        )
        .init();

    let cfg: AgentdConfig = load_config("agentd").context("load config")?;
    let port = cfg.port;

    info!(port, tenant = %cfg.tenant, providers = cfg.providers.len(), agents = cfg.agents.len(), "agentd starting");

    // Build LLM provider registry + agent registry
    let registry = AgentRegistry::build(&cfg).context("build agent registry")?;
    let orchestrator = OrchestratorAgent::new(&cfg).context("build orchestrator")?;

    info!(orchestrator_model = %cfg.orchestrator.model, "orchestrator ready");
    for name in &registry.agent_names {
        if let Some(a) = registry.get(name) {
            info!(agent = %name, model = %a.completion_cfg.model, triggers = a.trigger_patterns.len(), "specialist ready");
        }
    }

    // Build MCP pool (connects to all configured MCP servers)
    let mcp = McpPool::connect(&cfg.mcp_servers, &cfg.mcp_api_key).await;

    // Build RAG engine (if enabled)
    let rag = if cfg.rag.enabled {
        // Use orchestrator provider for embeddings unless overridden
        let embed_provider_name = cfg
            .rag
            .embedding_provider
            .as_deref()
            .unwrap_or(&cfg.orchestrator.provider);
        let embed_provider_cfg = cfg.providers.get(embed_provider_name).ok_or_else(|| {
            anyhow::anyhow!("RAG embedding_provider '{}' not found", embed_provider_name)
        })?;
        let embed_provider = llm::build_provider(embed_provider_name, embed_provider_cfg);
        let engine = RagEngine::new(&cfg.rag, embed_provider)
            .await
            .context("RAG engine init")?;
        info!(uri = %cfg.rag.storage_uri, "RAG: ready");
        Some(engine)
    } else {
        info!("RAG: disabled");
        None
    };

    let state = Arc::new(AppState {
        cfg,
        orchestrator,
        registry,
        mcp,
        rag,
    });

    let health = health_routes(|| async { true });
    let app = Router::new()
        .route("/webhook", post(handlers::webhook))
        .route("/api/v1/run", post(handlers::manual_run))
        // M9: Live RAG ingestion for MSB device history
        .route("/api/v1/rag/ingest", post(handlers::rag_ingest))
        .with_state(Arc::clone(&state))
        .merge(health);

    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "agentd listening");
    axum::serve(
        tokio::net::TcpListener::bind(&addr).await.context("bind")?,
        app,
    )
    .await
    .context("serve")
}
