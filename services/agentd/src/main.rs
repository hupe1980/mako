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

use agentd::{dlq::DlqStore, handlers, llm};
use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use mako_service::{health::health_routes, http::default_client, load_config, oidc::OidcConfig};
use tracing::info;

use agentd::{
    agent::{AgentRegistry, OrchestratorAgent},
    config::AgentdConfig,
    handlers::{AppState, SessionStore},
    mcp::McpPool,
    rag::RagEngine,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("agentd");

    let cfg: AgentdConfig = load_config("agentd").context("load config")?;
    let port = cfg.port;

    info!(
        port,
        tenant = %cfg.tenant,
        providers = cfg.providers.len(),
        custom_agents = cfg.agents.len(),
        bundled_enable_all = cfg.bundled_agents.enable_all,
        bundled_enabled = cfg.bundled_agents.enable.len(),
        "agentd starting"
    );

    // Build LLM provider registry + agent registry (builtins + custom merged)
    let registry = AgentRegistry::build(&cfg).context("build agent registry")?;
    let orchestrator = OrchestratorAgent::new(&cfg).context("build orchestrator")?;

    let builtin_count = registry
        .agent_names
        .iter()
        .filter_map(|n| registry.get(n))
        .filter(|a| a.is_builtin)
        .count();
    info!(
        orchestrator_model = %cfg.orchestrator.model,
        total_agents = registry.agent_names.len(),
        builtin_agents = builtin_count,
        dispatch_mode = ?cfg.orchestrator.dispatch_mode,
        "orchestrator ready"
    );
    for name in &registry.agent_names {
        if let Some(a) = registry.get(name) {
            info!(
                agent = %name,
                model = %a.completion_cfg.model,
                triggers = a.trigger_patterns.len(),
                is_builtin = a.is_builtin,
                "specialist ready"
            );
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
        let engine = RagEngine::new(&cfg.rag, embed_provider, &cfg.tenant)
            .await
            .context("RAG engine init")?;
        info!(uri = %cfg.rag.storage_uri, "RAG: ready");
        Some(engine)
    } else {
        info!("RAG: disabled");
        None
    };

    let max_sessions = cfg.max_sessions;
    if cfg.inbound_hmac_secret.is_none() {
        tracing::warn!(
            "agentd: inbound_hmac_secret not configured -- webhook accepts all inbound events (dev mode)"
        );
    }

    // Build OIDC verifier
    let ct = mako_service::shutdown::token();
    let http = default_client();
    let oidc = OidcConfig::build_verifier(cfg.oidc.as_ref(), &http, &cfg.tenant, ct.clone())
        .await
        .context("OIDC verifier init")?;
    if oidc.is_disabled() {
        tracing::warn!("[WARN] OIDC disabled -- POST /api/v1/run accepts all requests (dev mode)");
    }

    // Build DLQ store
    let dlq = DlqStore::new(
        cfg.dlq.capacity,
        cfg.dlq.max_retries,
        cfg.dlq.base_backoff_secs,
    );

    let state = Arc::new(AppState {
        cfg,
        orchestrator,
        registry,
        mcp,
        rag,
        sessions: SessionStore::new(100),
        oidc: Some(Arc::new(oidc.clone())),
        session_sem: Arc::new(tokio::sync::Semaphore::new(max_sessions as usize)),
        dlq: dlq.clone(),
    });

    let health = health_routes(|| async { true });
    let app = Router::new()
        // CloudEvent ingest
        .route("/webhook", post(handlers::webhook))
        // Manual trigger (OIDC-protected)
        .route("/api/v1/run", post(handlers::manual_run))
        // Session history
        .route("/api/v1/sessions", get(handlers::get_sessions))
        // Dead-letter queue status
        .route("/api/v1/dlq", get(handlers::get_dlq))
        // Agent discovery — list active agents
        .route("/api/v1/agents", get(handlers::list_agents))
        // Agent catalog — all 29 built-in definitions (even if not enabled)
        .route("/api/v1/agents/catalog", get(handlers::agents_catalog))
        // A2A Agent Cards for each specialist
        .route("/.well-known/agents/:name", get(handlers::agent_card))
        // RAG: Live ingestion and search
        .route("/api/v1/rag/ingest", post(handlers::rag_ingest))
        .route("/api/v1/rag/search", post(handlers::rag_search))
        // OIDC verifier extension for the Claims Axum extractor
        .layer(Extension(oidc))
        .with_state(Arc::clone(&state))
        .merge(health);

    // Spawn DLQ background retry worker (checks every 10 s)
    {
        let dlq_state = Arc::clone(&state);
        let ct2 = ct.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                        agentd::dlq::run_retry_pass(&dlq_state).await;
                    }
                    _ = ct2.cancelled() => break,
                }
            }
            tracing::debug!("DLQ retry worker shutdown");
        });
    }

    let addr = format!("0.0.0.0:{port}");
    info!(%addr, agents = state.registry.agent_names.len(), "agentd listening");
    let listener = tokio::net::TcpListener::bind(&addr).await.context("bind")?;
    mako_service::shutdown::serve(listener, app, ct).await
}
