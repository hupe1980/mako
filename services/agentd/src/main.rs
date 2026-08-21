//! `agentd` — the multi-agent plane for mako.
//!
//! 28 declarative specialists run on the [agentplane] durable runtime. What
//! agentd owns is the bridge from mako to that runtime: routing a CloudEvent to
//! the specialists that subscribe to it, labelling the payload at the trust
//! boundary, wiring the stores, the policy set, the key ring and the calendar,
//! and mounting the surface a human approves things on.
//!
//! ```text
//! CloudEvent  →  Router          one run per subscribing specialist
//!             →  plane::label    re-validated identifiers trusted, the rest not
//!             →  admission key   (source, id, specialist) — at most once, in the store
//!             →  Runtime         journaled effects, strict replay, Cedar gate
//!                  ├─ tools      MCP calls to mako's own services
//!                  ├─ tasks      a mutating call waits for a named approver
//!                  └─ deadlines  Werktage, from mako's BDEW holiday table
//!             →  de.agent.decision.made → marktd audit log
//! ```
//!
//! | Route | What it is for |
//! |---|---|
//! | `POST /webhook` | CloudEvent ingest (Standard Webhooks-verified) |
//! | `POST /api/v1/run` | Run a specialist by hand (OIDC; honours `Idempotency-Key`) |
//! | `GET /api/v1/decisions` | The last 100 decisions |
//! | `GET /api/v1/agents` | What this deployment activated |
//! | `GET /api/v1/agents/catalog` | Everything compiled in |
//! | `GET /.well-known/agents/{name}` | A2A Agent Card, derived from the manifest |
//! | `/api/v1/oversight/*` | agentplane's operator surface: worklist, runs, cases, breached obligations |
//!
//! Port: 9580
//!
//! [agentplane]: https://hupe1980.github.io/agentplane/

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use mako_service::{
    oidc::OidcConfig,
    service::{Daemon, ServiceContext},
};
use tracing::{info, warn};

use agentd::{
    config::{AgentdConfig, JournalConfig},
    handlers::{self, AppState, DecisionLog},
    plane::{Activation, Plane, PlaneConfig, Stores},
};

/// The `agentd` daemon. `mako_service::run` owns the lifecycle (tracing, config,
/// health, metrics, graceful shutdown); this supplies the plane and the routes.
struct Agentd;

impl Daemon for Agentd {
    type Config = AgentdConfig;
    const NAME: &'static str = "agentd";

    async fn build(cfg: Arc<AgentdConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // Credentials first: an `env:VAR` placeholder that reached a driver
        // would be sent literally as a bearer token and come back as a 401 per
        // model call. The config itself stays as the runner handed it over —
        // it holds the placeholders, `Secrets` holds what they resolved to.
        let secrets = cfg
            .resolve_secrets()
            .context("resolve env: indirection in secrets")?;

        info!(
            port = cfg.port,
            tenant = %cfg.tenant,
            providers = cfg.providers.len(),
            enable_all = cfg.bundled_agents.enable_all,
            enabled = cfg.bundled_agents.enable.len(),
            "agentd starting"
        );

        // The tenant is part of every store key, so it is parsed once and
        // bound to both the stores and the runtime. agentplane refuses to build
        // when the two disagree.
        let tenant = agentplane::core::TenantId::new(&cfg.tenant).map_err(|e| {
            anyhow::anyhow!("tenant `{}` is not a usable key scope: {e}", cfg.tenant)
        })?;

        // ── Durable state ────────────────────────────────────────────────
        //
        // The journal is the § 147 AO / GoBD record for the agent layer, and the
        // case layer beside it holds the matters, the obligations and the human
        // tasks. One backend supplies both.
        let stores = match &cfg.journal {
            JournalConfig::Redb { path } => Stores::redb(
                agentplane::store::RedbStore::open(path)
                    .with_context(|| format!("open the agent journal at {path}"))?,
                &tenant,
            ),
            // The DSN comes from `secrets`, where its `env:VAR` indirection was
            // resolved: connecting with the literal placeholder would fail as a
            // hostname nobody can find.
            JournalConfig::Postgres { .. } => {
                let url = secrets
                    .journal_url
                    .as_deref()
                    .context("journal.url is required for the postgres backend")?;
                Stores::postgres(
                    agentplane::store::PostgresStore::connect(url)
                        .await
                        .context("connect the agent journal to Postgres")?,
                    &tenant,
                )
            }
        };

        // ── Sealing ──────────────────────────────────────────────────────
        let keyring =
            agentd::plane::keys::build(cfg.keyring.as_ref(), secrets.vault_token.as_ref())
                .map_err(|e| anyhow::anyhow!(e))?;
        if keyring.is_none() {
            warn!("agentd: {}", agentd::plane::keys::UNSEALED_WARNING);
        }

        // ── Authorization ────────────────────────────────────────────────
        let policy_source = agentd::plane::policy::source(cfg.policy.path.as_deref())
            .map_err(|e| anyhow::anyhow!(e))?;
        let policy =
            agentd::plane::policy::engine(&policy_source).map_err(|e| anyhow::anyhow!(e))?;
        info!(
            policy = cfg.policy.path.as_deref().unwrap_or("<embedded>"),
            "agent policy set compiled"
        );

        // ── Model drivers, under the names the manifests use ──────────────
        let providers = agentd::plane::providers::build_all(&secrets.providers)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        // One transport per MCP server the manifests grant. Connecting at
        // startup rather than lazily is deliberate: an agent whose tools are
        // unreachable still answers, from the model alone and with no evidence
        // behind it, which is worse than a daemon that declines to start.
        let tool_servers =
            agentd::plane::tools::connect(&cfg.mcp_servers, &secrets.mcp_api_key, &ctx.http)
                .await
                .context("connect MCP tool transports")?;

        // Only specialists the operator activated are registered and routed. An
        // `enable` name that matches nothing compiled in refuses to boot rather
        // than presenting as an agent that never fires.
        let activated = Activation::from_config(&cfg.bundled_agents);

        // Durable delivery of a completed run's decision, on the journal's own
        // cursor: registrations are made at admission so no run exists
        // unwatched, and the cursor advances only on 2xx, so a receiver that is
        // down for a deploy is caught up rather than having missed everything.
        let journal_for_delivery = std::sync::Arc::clone(&stores.journal);
        let journal_for_retention = std::sync::Arc::clone(&stores.journal);
        let push_store_for_delivery = std::sync::Arc::clone(&stores.push);
        // Built once and handed to BOTH the outbox (which registers them at
        // admission) and the sender (which signs deliveries). The sender takes
        // the list rather than offering a setter, and that is the control: a
        // sender built without the destinations would deliver unsigned, and
        // only the receiver's refusal would ever show it.
        let destinations: Vec<agentplane::push::Destination> = match cfg.audit_webhook_url.as_ref()
        {
            None => Vec::new(),
            Some(url) => {
                let secret = |s: &secrecy::SecretString| {
                    agentplane::core::Secret::new(secrecy::ExposeSecret::expose_secret(s))
                };
                let mut destination = agentplane::push::Destination::new("erp-audit", url);
                // Standard Webhooks, the same scheme `mako_service::webhook`
                // signs every other mako outbound with — so `de.agent.decision.made`
                // verifies through the shared verifier like everything else, and
                // gains the replay protection a body-only HMAC could not give it.
                //
                // `try_signed_with`, not `signed_with`: the latter panics on a
                // key under 24 bytes or a malformed `whsec_` secret, and this
                // call is inside `Daemon::build` — so a mistyped secret would
                // abort the process before the exit code and the log line that
                // name which destination was wrong. A deployment's own
                // configuration is refused the way every other configuration
                // error here is refused.
                if let Some(current) = secrets.audit_hmac_secret.as_ref() {
                    destination = destination
                        .try_signed_with(&secret(current))
                        .context("audit_hmac_secret is not a usable signing key")?;
                    // The retiring key, presented beside the current one so a
                    // receiver holding either verifies. Rotation is then the
                    // receiver's pace rather than a flag day where every
                    // delivery fails until both sides are restarted together.
                    //
                    // This one has no `try_` form yet, so it still aborts on a
                    // malformed key — reported upstream. What we *can* keep
                    // unreachable is its other panic, "also with no primary
                    // key", which the branch below refuses properly.
                    if let Some(previous) = secrets.audit_hmac_secret_previous.as_ref() {
                        destination = destination.also_signed_with(&secret(previous));
                        info!(
                            "agent decisions are signed with both audit_hmac_secret and \
                             audit_hmac_secret_previous — remove the latter once every \
                             receiver has the new key"
                        );
                    }
                } else if secrets.audit_hmac_secret_previous.is_some() {
                    anyhow::bail!(
                        "audit_hmac_secret_previous is set without audit_hmac_secret — \
                         \"also\" with no primary key signs nothing, and the deliveries \
                         would go out unsigned"
                    );
                }
                vec![destination]
            }
        };
        let outbox = (!destinations.is_empty()).then(|| {
            std::sync::Arc::new(agentplane::push::Outbox::new(
                std::sync::Arc::clone(&stores.push),
                destinations.clone(),
            ))
        });

        let plane = Plane::new(
            stores,
            PlaneConfig {
                owner: &owner_id(),
                tenant: &tenant,
                activated: &activated,
                providers,
                tool_servers,
                policy,
                keyring,
                outbox,
            },
        )
        .map_err(|e| anyhow::anyhow!("build agent plane: {e}"))?;
        info!(
            specialists = plane.router().routes().len(),
            tenant = %cfg.tenant,
            "agent plane ready"
        );

        // ── The tick that makes a deadline mean something ────────────────
        agentd::plane::sweep::spawn(
            plane.runtime(),
            std::time::Duration::from_secs(cfg.sweep_interval_secs),
            ctx.shutdown.clone(),
        );

        // ── The tick that bounds the admission index ─────────────────────
        //
        // No default window: retiring a key reopens the door it closed, so
        // absent an operator's choice the keys are kept and the index's size is
        // a fact their database monitoring already reports.
        match cfg.admission_retention_days {
            None => info!(
                "agentd: admission keys are kept indefinitely — set \
                 admission_retention_days (30 is the recommended figure) to retire them"
            ),
            Some(0) => anyhow::bail!(
                "admission_retention_days = 0 retires a key the moment it is claimed, which \
                 is the duplicate fan-out the key exists to prevent, on a schedule. Omit the \
                 field to keep keys, or choose a window longer than every emitter's retry \
                 horizon (30 days is recommended — a dead-lettered mako delivery can be \
                 replayed by hand days later, and that replay is the same message)"
            ),
            Some(days) => {
                let retention = std::time::Duration::from_secs(u64::from(days) * 24 * 60 * 60);
                agentd::plane::sweep::spawn_admission_retention(
                    journal_for_retention,
                    retention,
                    ctx.shutdown.clone(),
                );
                info!(days, "agent admission keys are retired on a daily tick");
            }
        }

        // ── The tick that makes a registration mean something ────────────
        //
        // `Outbox` puts a run in front of the receiver at admission; this loop
        // is what carries the record there, advancing its cursor only on 2xx.
        if let Some(url) = cfg.audit_webhook_url.as_deref() {
            agentd::plane::sweep::spawn_delivery(
                std::sync::Arc::new(agentplane::push::DeliveryWorker::new(
                    journal_for_delivery,
                    push_store_for_delivery,
                    std::sync::Arc::new(agentplane::push::PushSender::for_operator_destinations(
                        &destinations,
                    )),
                    std::sync::Arc::new(
                        agentplane::push::RunCompleted::new(mako_service::source(
                            "agentd",
                            &cfg.tenant,
                        ))
                        .event_type(mako_events::agent::DECISION_MADE)
                        // The `tenantid` extension every mako receiver binds on
                        // — including this service's own `POST /webhook`. Without
                        // it two operators' completions are indistinguishable
                        // envelopes on one ERP bus.
                        .for_tenant(cfg.tenant.clone()),
                    ),
                )),
                std::time::Duration::from_secs(cfg.sweep_interval_secs),
                ctx.shutdown.clone(),
            );
            info!(
                url,
                "agent decisions deliver through the journal-backed outbox"
            );
        }

        if secrets.inbound_hmac_secret.is_none() {
            warn!(
                "agentd: inbound_hmac_secret not configured — POST /webhook accepts all \
                 inbound events (dev mode)"
            );
        }

        // ── Identity ─────────────────────────────────────────────────────
        let oidc = OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.tenant,
            ctx.shutdown.clone(),
        )
        .await
        .context("OIDC verifier init")?;

        // The oversight surface exists only where callers have names. Every
        // other dev-mode relaxation here accepts an unauthenticated request and
        // warns; an approval is the one place where that is not a relaxation but
        // a forged signature on a regulated dispatch.
        let oversight = if oidc.is_disabled() {
            warn!(
                "agentd: OIDC disabled — POST /api/v1/run accepts all requests and the \
                 oversight surface ({}) is NOT mounted. Every tool grant that requires \
                 approval will suspend its run with nobody able to answer it.",
                agentd::plane::oversight::MOUNT
            );
            None
        } else {
            Some(
                agentd::plane::oversight::router(plane.runtime(), Arc::new(oidc.clone()))
                    .map_err(|e| anyhow::anyhow!(e))?,
            )
        };

        let max_sessions = cfg.max_sessions;
        let state = Arc::new(AppState {
            cfg: Arc::clone(&cfg),
            secrets,
            plane,
            decisions: DecisionLog::new(100),
            session_sem: Arc::new(tokio::sync::Semaphore::new(max_sessions as usize)),
        });

        let mut app = Router::new()
            // CloudEvent ingest
            .route("/webhook", post(handlers::webhook))
            // Manual trigger (OIDC-protected)
            .route("/api/v1/run", post(handlers::manual_run))
            // What just happened
            .route("/api/v1/decisions", get(handlers::get_decisions))
            // What this deployment runs, and what it could run
            .route("/api/v1/agents", get(handlers::list_agents))
            .route("/api/v1/agents/catalog", get(handlers::agents_catalog))
            // A2A Agent Cards, derived from each manifest
            .route("/.well-known/agents/{name}", get(handlers::agent_card))
            // OIDC verifier extension for the Claims Axum extractor
            .layer(Extension(oidc))
            .with_state(state);

        if let Some(surface) = oversight {
            info!(
                mount = agentd::plane::oversight::MOUNT,
                "oversight surface mounted"
            );
            app = app.nest(agentd::plane::oversight::MOUNT, surface);
        }

        Ok(app)
    }
}

/// Identifies this **process** for lease fencing.
///
/// Not the agent and not the service: two agentd instances sharing a Postgres
/// journal must not believe they are the same writer, or a resumed run could be
/// executed twice. `HOSTNAME` is what distinguishes them in every container
/// runtime mako deploys on.
///
/// **The fallback is per-process, not a constant.** A shared owner string
/// defeats fencing entirely — every instance would consider every other
/// instance's lease its own and resume a run another was executing. A random
/// suffix cannot collide, and losing a lease across a restart is the safe
/// direction: it lapses and another owner takes over, which is the designed
/// path.
fn owner_id() -> String {
    std::env::var("HOSTNAME").map_or_else(
        |_| {
            let unique = uuid::Uuid::new_v4();
            warn!(
                owner = %unique,
                "agentd: HOSTNAME is unset — using a random per-process owner id for lease \
                 fencing. Set HOSTNAME to the pod name so an owner survives a reconnect."
            );
            format!("agentd@{unique}")
        },
        |h| format!("agentd@{h}"),
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::service::run::<Agentd>().await
}
