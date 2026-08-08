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

use agentd::handlers;
use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use mako_service::{health::health_routes, http::default_client, load_config, oidc::OidcConfig};
use tracing::info;

use agentd::{
    config::AgentdConfig,
    handlers::{AppState, SessionStore},
    plane::Plane,
};

/// Build an agentplane model driver for a configured provider.
///
/// The name is the key a manifest's `spec.models` refers to, so `[providers.anthropic]`
/// is what makes `provider: anthropic` resolvable. An unknown name is skipped
/// with a warning rather than failing the boot — a deployment may configure
/// providers it has no manifest for.
fn build_model_driver(
    name: &str,
    cfg: &agentd::config::ProviderConfig,
) -> Option<Arc<dyn agentplane::model::ModelProvider>> {
    use secrecy::ExposeSecret as _;
    let key = cfg.api_key.expose_secret().to_owned();
    if key.is_empty() {
        tracing::warn!(provider = %name, "no api_key configured — driver not registered");
        return None;
    }
    let built: Result<Arc<dyn agentplane::model::ModelProvider>, _> = match name {
        "anthropic" => agentplane::model::anthropic::Anthropic::new(key)
            .map(|d| Arc::new(d) as Arc<dyn agentplane::model::ModelProvider>),
        "openai" => agentplane::model::openai::OpenAi::new(key)
            .map(|d| Arc::new(d) as Arc<dyn agentplane::model::ModelProvider>),
        _ => return None,
    };
    match built {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!(provider = %name, error = %e, "model driver construction failed");
            None
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("agentd");

    let mut cfg: AgentdConfig = load_config("agentd").context("load config")?;
    // `env:VAR` indirection: resolve before any provider/pool clones the keys —
    // unresolved placeholders would be sent literally as bearer tokens.
    cfg.resolve_env_indirection()
        .context("resolve env: indirection in secrets")?;
    let cfg = cfg;
    let port = cfg.port;

    info!(
        port,
        tenant = %cfg.tenant,
        providers = cfg.providers.len(),
        enable_all = cfg.bundled_agents.enable_all,
        enabled = cfg.bundled_agents.enable.len(),
        "agentd starting"
    );

    // ── The plane ────────────────────────────────────────────────────────
    //
    // The journal is the § 147 AO / GoBD record for the agent layer, so it must
    // live on durable storage. Every model and tool call is written here before
    // it happens.
    let store: Arc<dyn agentplane::journal::JournalStore> = Arc::new(
        agentplane::store::RedbStore::open(&cfg.journal_path)
            .with_context(|| format!("open agent journal at {}", cfg.journal_path))?,
    );

    // Model drivers, registered under the names the manifests use.
    let mut providers: Vec<(String, Arc<dyn agentplane::model::ModelProvider>)> = Vec::new();
    for (name, pcfg) in &cfg.providers {
        if let Some(driver) = build_model_driver(name, pcfg) {
            providers.push((name.clone(), driver));
        } else {
            tracing::warn!(provider = %name, "unknown model provider — skipping");
        }
    }
    if providers.is_empty() {
        anyhow::bail!(
            "no model provider configured. agentd cannot run an agent without one — \
             declare at least one [providers.<name>] matching a manifest's `spec.models`."
        );
    }

    // Only specialists the operator activated are registered and routed. An
    // `enable` name that matches nothing compiled in refuses to boot rather than
    // presenting as an agent that never fires.
    let activated = agentd::plane::Activation::from_config(&cfg.bundled_agents);
    let plane = Plane::new(store, "agentd", &activated, providers, None, None)
        .map_err(|e| anyhow::anyhow!("build agent plane: {e}"))?;
    info!(
        specialists = plane.router().routes().len(),
        journal = %cfg.journal_path,
        "agent plane ready"
    );

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

    let state = Arc::new(AppState {
        cfg,
        plane,
        sessions: SessionStore::new(100),
        oidc: Some(Arc::new(oidc.clone())),
        session_sem: Arc::new(tokio::sync::Semaphore::new(max_sessions as usize)),
        // 1h dedup window, 10k ids — comfortably beyond any legitimate
        // emitter's retry horizon.
        seen_events: handlers::SeenEvents::new(std::time::Duration::from_secs(3600), 10_000),
    });

    let health = health_routes(|| async { true });
    let app = Router::new()
        // CloudEvent ingest
        .route("/webhook", post(handlers::webhook))
        // Manual trigger (OIDC-protected)
        .route("/api/v1/run", post(handlers::manual_run))
        // Session history
        .route("/api/v1/sessions", get(handlers::get_sessions))
        // Agent discovery — list active agents
        .route("/api/v1/agents", get(handlers::list_agents))
        // Agent catalog — all 28 built-in definitions (even if not enabled)
        .route("/api/v1/agents/catalog", get(handlers::agents_catalog))
        // A2A Agent Cards for each specialist
        .route("/.well-known/agents/:name", get(handlers::agent_card))
        // OIDC verifier extension for the Claims Axum extractor
        .layer(Extension(oidc))
        .with_state(Arc::clone(&state))
        .merge(health);

    // Spawn DLQ background retry worker (checks every 10 s)

    let addr = format!("0.0.0.0:{port}");
    info!(%addr, specialists = state.plane.router().routes().len(), "agentd listening");
    let listener = tokio::net::TcpListener::bind(&addr).await.context("bind")?;
    mako_service::shutdown::serve(listener, app, ct).await
}
