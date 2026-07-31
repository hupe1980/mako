//! Axum router and startup logic for `processd`.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    routing::{get, post},
};
use mako_markt::makod_client::MakodClient;
use secrecy::SecretString;
use tracing::info;

use mako_service::{ServiceContext, cedar::CedarEnforcer, oidc::OidcVerifier};

#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
use crate::pg::PgAnmeldungRepository;
#[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
use crate::pg::PgApprovalQueue;
use crate::{handler::handle_webhook, mcp_server::ProcessdMcpState};

// ── Module state bundles ───────────────────────────────────────────────────────

/// State bundle for the NB module.
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
pub struct NbState {
    pub config: crate::nb_module::NbModuleConfig,
    pub reader: mako_markt::marktd_client::MarktdClient,
    pub makod: MakodClient,
    pub repo: PgAnmeldungRepository,
}

/// State bundle for the LF module.
#[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
pub struct LfState {
    pub config: crate::lf_module::LfModuleConfig,
    pub reader: mako_markt::marktd_client::MarktdClient,
    pub makod: MakodClient,
    pub queue: PgApprovalQueue,
}

// ── Shared application state ──────────────────────────────────────────────────

#[derive(Clone)]
pub struct ProcessdState {
    pub inbound_secret: Arc<Option<SecretString>>,
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    pub nb: Option<Arc<NbState>>,
    #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
    pub lf: Option<Arc<LfState>>,
    /// Shared `MakodClient` for REST handlers that dispatch commands.
    /// Approve/reject approval-queue entries require dispatching to makod.
    pub makod: MakodClient,
    pub tenant: String,
    /// Operator's own Marktpartner-ID (used for LFA command routing).
    pub own_mp_id: String,
    /// `marktd` client — used by the §14a Steuerungsauftrag auto-ORDRSP module (N5).
    pub marktd: Arc<mako_markt::marktd_client::MarktdClient>,
    /// M3: When `true`, auto-dispatch QUOTES from PreisblattMessung on REQOTE arrival.
    pub msb_auto_preisanfrage: bool,
    /// Shared PG pool — used by the EoG module and REST case queries.
    pub pool: sqlx::PgPool,
    /// EoG gap-closure config (§36/§38 EnWG).
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    pub eog: Arc<crate::eog_module::EogModuleConfig>,
}

// ── RunConfig ─────────────────────────────────────────────────────────────────

pub struct RunConfig {
    pub inbound_secret: Option<SecretString>,
    pub makod_url: String,
    pub makod_api_key: SecretString,
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    pub nb_auto_accept: bool,
    pub nb_gas_bearbeitungsfrist_wt: u32,
    pub lf_auto_respond: bool,
    pub lf_queue_ttl_secs: u64,
    /// M3: When `true`, auto-dispatch QUOTES from `PreisblattMessung` on REQOTE (PID 35001–35005) arrival.
    pub msb_auto_preisanfrage: bool,
    /// EoG gap-closure automation (§36/§38 EnWG) — see `[eog]` in TOML.
    pub eog_auto_activate: bool,
    pub eog_default_transaktionsgrund: String,
    pub eog_warn_days_before_expiry: u32,
    pub eog_notify_webhook_url: Option<String>,
    pub eog_notify_webhook_secret: Option<String>,
    /// Webhook URL to register with `marktd` on startup (self-registration).
    /// `None` → skip self-registration (useful in tests / standalone mode).
    pub self_register_webhook_url: Option<String>,
    /// Subscriber ID for the `marktd` subscription upsert.
    pub subscriber_id: String,
    /// Comma-separated event types to subscribe to.
    pub subscriber_event_types: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
    /// MCP server auth config (API-key fallback + optional per-named-key identity).
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

// ── Router assembly ─────────────────────────────────────────────────────────────

/// Build the `processd` domain router and spawn its background workers.
///
/// The [`mako_service::run`] lifecycle owns the pool, migrations, infra routes
/// (health / metrics / trace), bind and graceful serve; this only assembles the
/// domain [`Router`], self-registers with `marktd`, and spawns the approval-queue
/// expiry + §38 EnWG Ersatzversorgung timer workers on `ctx.shutdown`.
pub async fn build_router(cfg: RunConfig, ctx: ServiceContext) -> anyhow::Result<Router> {
    // ── Startup validation ────────────────────────────────────────────────────
    // §20 EnWG parity: validate own_mp_id prefix matches the expected coding authority.
    // BDEW-Codenummern start with "99" (NAD DE3055 = 293), DVGW with "98" (332).
    // A mismatch silently breaks `initiator_is_affiliate` comparisons for gas roles.
    {
        use mako_markt::domain::nad_agency_code;
        // Parse own_mp_id as MarktpartnerId; if malformed, warn but continue.
        match cfg.own_mp_id.parse::<mako_markt::domain::MarktpartnerId>() {
            Ok(id) => {
                let agency = nad_agency_code(&id);
                tracing::info!(
                    own_mp_id = %cfg.own_mp_id,
                    coding_authority = agency,
                    "processd: operator identity validated (293=BDEW, 332=DVGW, 9=GS1)"
                );
                if agency == "9" {
                    tracing::warn!(
                        own_mp_id = %cfg.own_mp_id,
                        "processd: own_mp_id appears to be a GS1 GLN (non-99/98 prefix). \
                         §20 EnWG parity reporting may be incorrect for BDEW/DVGW market participants. \
                         Expected: BDEW-Codenummer (99…) for Strom, DVGW-Codenummer (98…) for Gas."
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    own_mp_id = %cfg.own_mp_id,
                    error = %e,
                    "processd: own_mp_id is not a valid 13-digit MarktpartnerId — \
                     §20 EnWG parity comparisons will fail silently"
                );
            }
        }
    }

    let pool = ctx.pool().clone();

    info!("processd: running");

    let http = ctx.http.clone();

    // ── Self-register subscription with marktd ────────────────────────────────
    // Driven entirely by config (env var / Helm values.yaml). No imperative
    // bootstrap scripts needed. Idempotent: PUT is an upsert.
    // Retries for up to 30 s to tolerate marktd startup ordering in compose/K8s.
    if let Some(ref self_webhook_url) = cfg.self_register_webhook_url {
        use secrecy::ExposeSecret;
        let sub_url = format!(
            "{}/api/v1/subscriptions/{}",
            cfg.marktd_url.trim_end_matches('/'),
            cfg.subscriber_id
        );
        let event_types: Vec<&str> = cfg
            .subscriber_event_types
            .split(',')
            .map(str::trim)
            .collect();
        let body = serde_json::json!({
            "webhook_url":    self_webhook_url,
            "webhook_secret": cfg.inbound_secret.as_ref().map(|s| s.expose_secret()),
            "event_types":    event_types,
            "active":         true
        });
        info!(
            subscriber_id = %cfg.subscriber_id,
            webhook_url   = %self_webhook_url,
            marktd_url    = %cfg.marktd_url,
            "processd: self-registering subscription with marktd"
        );
        let mut remaining = 15u32;
        loop {
            match http.put(&sub_url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    info!(
                        subscriber_id = %cfg.subscriber_id,
                        status        = %resp.status(),
                        "processd: subscription registered with marktd"
                    );
                    break;
                }
                Ok(resp) => {
                    let status = resp.status();
                    remaining -= 1;
                    if remaining == 0 {
                        return Err(anyhow::anyhow!(
                            "processd: self-registration failed: marktd returned HTTP {status}"
                        ));
                    }
                    tracing::warn!(
                        %status, remaining,
                        "processd: self-registration failed, retrying in 2 s"
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => {
                    remaining -= 1;
                    if remaining == 0 {
                        return Err(anyhow::anyhow!("processd: self-registration failed: {e}"));
                    }
                    tracing::warn!(
                        error = %e, remaining,
                        "processd: self-registration failed, retrying in 2 s"
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    let makod = MakodClient::new(&cfg.makod_url, cfg.makod_api_key.clone());

    // ── NB module state ───────────────────────────────────────────────────
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    let nb_state: Option<Arc<NbState>> = {
        let nb_config = crate::nb_module::NbModuleConfig {
            marktd_url: cfg.marktd_url.clone(),
            marktd_api_key: cfg.marktd_api_key.clone(),
            own_mp_id: cfg.own_mp_id.clone(),
            tenant: cfg.tenant.clone(),
            auto_accept: cfg.nb_auto_accept,
            gas_bearbeitungsfrist_wt: cfg.nb_gas_bearbeitungsfrist_wt,
        };
        Some(Arc::new(NbState {
            config: nb_config,
            reader: mako_markt::marktd_client::MarktdClient::new(
                &cfg.marktd_url,
                cfg.marktd_api_key.clone(),
                http.clone(),
            ),
            makod: makod.clone(),
            repo: PgAnmeldungRepository::new(pool.clone()),
        }))
    };

    // ── LF module state ───────────────────────────────────────────────────
    #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
    let lf_state: Option<Arc<LfState>> = {
        let lf_config = crate::lf_module::LfModuleConfig {
            marktd_url: cfg.marktd_url.clone(),
            marktd_api_key: cfg.marktd_api_key.clone(),
            own_mp_id: cfg.own_mp_id.clone(),
            tenant: cfg.tenant.clone(),
            auto_respond: cfg.lf_auto_respond,
            queue_ttl_secs: cfg.lf_queue_ttl_secs,
        };
        Some(Arc::new(LfState {
            config: lf_config,
            reader: mako_markt::marktd_client::MarktdClient::new(
                &cfg.marktd_url,
                cfg.marktd_api_key.clone(),
                http.clone(),
            ),
            makod: makod.clone(),
            queue: PgApprovalQueue::new(pool.clone()),
        }))
    };

    // ── Background: expire stale approval queue entries ───────────────────
    #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
    {
        let expiry_pool = pool.clone();
        let expiry_shutdown = ctx.shutdown.clone();
        tokio::spawn(async move {
            let queue = PgApprovalQueue::new(expiry_pool);
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match queue.expire_stale().await {
                            Ok(n) if n > 0 => {
                                    // REGULATORY WARNING: Expired entries mean the LFA did not
                                    // respond within the 45-minute deadline (BK6-22-024 §5).
                                    // Operator must reconcile via GET /api/v1/queue?status=Expired.
                                    tracing::warn!(
                                        expired = n,
                                        "processd: {n} approval queue entries expired past \
                                         LFA E_0624 45-min deadline — operator must reconcile"
                                    );
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "processd: approval queue expiry failed");
                            }
                            _ => {}
                        }
                    }
                    _ = expiry_shutdown.cancelled() => break,
                }
            }
        });
    }

    // ── Assemble shared handler state ──────────────────────────────────────
    let marktd_for_state = Arc::new(mako_markt::marktd_client::MarktdClient::new(
        &cfg.marktd_url,
        cfg.marktd_api_key.clone(),
        mako_service::http::default_client(),
    ));
    let state = ProcessdState {
        inbound_secret: Arc::new(cfg.inbound_secret),
        #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
        nb: nb_state,
        #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
        lf: lf_state,
        makod: makod.clone(),
        tenant: cfg.tenant.clone(),
        own_mp_id: cfg.own_mp_id.clone(),
        marktd: marktd_for_state,
        msb_auto_preisanfrage: cfg.msb_auto_preisanfrage,
        pool: pool.clone(),
        #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
        eog: Arc::new(crate::eog_module::EogModuleConfig {
            auto_activate: cfg.eog_auto_activate,
            default_transaktionsgrund: cfg.eog_default_transaktionsgrund.clone(),
            warn_days_before_expiry: cfg.eog_warn_days_before_expiry,
            notify_webhook_url: cfg.eog_notify_webhook_url.clone(),
            notify_webhook_secret: cfg.eog_notify_webhook_secret.clone(),
        }),
    };

    // ── Background: §38 EnWG Ersatzversorgung 3-month timer (daily) ────────
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    {
        let timer_pool = pool.clone();
        let timer_cfg = crate::eog_module::EogModuleConfig {
            auto_activate: cfg.eog_auto_activate,
            default_transaktionsgrund: cfg.eog_default_transaktionsgrund.clone(),
            warn_days_before_expiry: cfg.eog_warn_days_before_expiry,
            notify_webhook_url: cfg.eog_notify_webhook_url.clone(),
            notify_webhook_secret: cfg.eog_notify_webhook_secret.clone(),
        };
        let timer_tenant = cfg.tenant.clone();
        let timer_shutdown = ctx.shutdown.clone();
        tokio::spawn(async move {
            let client = mako_service::http::default_client();
            // First sweep shortly after startup, then daily.
            let mut interval = tokio::time::interval(Duration::from_secs(86_400));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match crate::eog_module::sweep_ersatzversorgung_timer(
                            &timer_pool, &timer_cfg, &timer_tenant, &client,
                        ).await {
                            Ok((warned, expired)) if warned + expired > 0 => {
                                tracing::warn!(
                                    warned, expired,
                                    "processd EoG: §38 timer sweep — operator review via \
                                     GET /api/v1/eog?status=expiring|expired"
                                );
                            }
                            Err(e) => tracing::warn!(error = %e, "processd EoG: timer sweep failed"),
                            _ => {}
                        }
                    }
                    _ = timer_shutdown.cancelled() => break,
                }
            }
        });
    }

    // ── MCP state ──────────────────────────────────────────────────────────
    let mcp_state = Arc::new(ProcessdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            cfg.oidc.clone(),
            Some(cfg.cedar.clone()),
            &cfg.tenant,
        ),
        makod_url: cfg.makod_url.clone(),
        makod_api_key: cfg.makod_api_key.clone(),
    });

    // ── Router ─────────────────────────────────────────────────────────────
    // Infra routes (`/health/live`, `/health/ready`, `/metrics`, trace) are
    // owned by `mako_service::run`; the domain Prometheus endpoint therefore
    // lives at `/processd/metrics` to avoid colliding with the generic
    // request-counter `/metrics` the runner mounts.
    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/api/v1/decisions", get(rest::list_decisions))
        .route("/api/v1/queue", get(rest::list_queue))
        .route(
            "/api/v1/queue/{id}/approve",
            post(rest::approve_queue_entry),
        )
        .route("/api/v1/queue/{id}/reject", post(rest::reject_queue_entry))
        .route("/api/v1/start-supply", post(rest::start_supply))
        .route("/api/v1/start-supply-gas", post(rest::start_supply_gas))
        .route("/api/v1/end-supply", post(rest::end_supply))
        .route("/api/v1/end-supply-gas", post(rest::end_supply_gas))
        .route("/api/v1/eog", get(rest::list_eog_cases))
        .route("/processd/metrics", get(rest::metrics))
        .with_state(state)
        .layer(axum::Extension(cfg.oidc.clone()))
        .layer(axum::Extension(cfg.cedar.clone()))
        .layer(axum::Extension(pool.clone()))
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
        .merge(crate::mcp_server::router(mcp_state, ctx.shutdown.clone()));

    Ok(app)
}

// ── REST handlers ──────────────────────────────────────────────────────────────

mod rest {
    use axum::{
        Extension, Json,
        extract::{Path, State},
        http::StatusCode,
        response::IntoResponse,
    };

    use sqlx::PgPool;

    use crate::{
        pg::{PgAnmeldungRepository, PgApprovalQueue},
        server::ProcessdState,
    };

    pub async fn list_decisions(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
    ) -> impl IntoResponse {
        let repo = PgAnmeldungRepository::new(pool);
        match repo.list(&state.tenant, 100).await {
            Ok(records) => Json(serde_json::to_value(records).unwrap_or_default()).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    pub async fn list_queue(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
    ) -> impl IntoResponse {
        let queue = PgApprovalQueue::new(pool);
        match queue.list(&state.tenant, None, 100).await {
            Ok(entries) => Json(serde_json::to_value(entries).unwrap_or_default()).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    /// `GET /api/v1/eog?status=…` — EoG gap-closure case log (§36/§38 EnWG).
    pub async fn list_eog_cases(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    ) -> impl IntoResponse {
        #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
        {
            match crate::eog_module::list_cases(
                &pool,
                &state.tenant,
                q.get("status").map(String::as_str),
            )
            .await
            {
                Ok(rows) => Json(serde_json::to_value(rows).unwrap_or_default()).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        #[cfg(not(any(feature = "role-nb-strom", feature = "role-nb-gas")))]
        {
            let _ = (pool, state, q);
            (StatusCode::NOT_IMPLEMENTED, "NB role not compiled in").into_response()
        }
    }

    /// Approve a LFA E_0624 approval queue entry.
    ///
    /// This dispatches `gpke.nb-lieferende.bestaetigen` (PID 55008 Strom) to `makod`
    /// AND marks the entry as `Approved` in the database.
    ///
    /// **Regulatory note:** The 45-min deadline applies from the original
    /// process.initiated event.  Operators must act before `expires_at`.
    pub async fn approve_queue_entry(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        Path(id_str): Path<String>,
    ) -> impl IntoResponse {
        let Ok(id) = id_str.parse::<uuid::Uuid>() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let queue = PgApprovalQueue::new(pool.clone());

        // Fetch entry first to get process_id, pid, malo_id for dispatch.
        let entry = match queue.find_by_id(id, &state.tenant).await {
            Ok(Some(e)) => e,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

        // Determine the command name from PID.
        // Strom: gpke.nb-lieferende.bestaetigen → PID 55008 (LF Zustimmung Lieferende)
        // Gas stornierung 44022: LF initiates UTILMD G 44022 to GNB via geli.gas.stornierung.initiieren
        // Gas stornierung 44023: GNB confirmed 44023 — this is an inbound response, not an ERP approval;
        //   approval queue entries for 44023 indicate operator review of an automated accept.
        let command = match entry.pid as u32 {
            55008 => mako_markt::commands::GPKE_NB_LIEFERENDE_BESTAETIGEN,
            44022 | 44023 => mako_markt::commands::GELI_GAS_STORNIERUNG_INITIIEREN,
            _ => mako_markt::commands::GPKE_NB_LIEFERENDE_BESTAETIGEN,
        };

        // Dispatch einwilligung to makod — BEFORE marking as Approved.
        // If dispatch fails, the entry stays Pending so the operator can retry.
        let idempotency_key = format!("processd-lf-approve-{id}");
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: None,
            command: command.to_owned(),
            malo_id: entry.malo_id.clone(),
            melo_id: None,
            payload: serde_json::json!({
                "process_id": entry.process_id,
                "approved_by": "operator",
            }),
        };
        if let Err(e) = state.makod.post_command(&idempotency_key, &cmd).await {
            tracing::warn!(
                %id,
                process_id = %entry.process_id,
                error = %e,
                "processd: failed to dispatch einwilligung for approved queue entry"
            );
            return (
                StatusCode::BAD_GATEWAY,
                format!("makod dispatch failed: {e}"),
            )
                .into_response();
        }

        // Mark as Approved in DB.
        match queue.approve(id, &state.tenant).await {
            Ok(true) => {
                tracing::info!(%id, process_id = %entry.process_id, "processd: E_0624 approved — einwilligung dispatched");
                StatusCode::NO_CONTENT.into_response()
            }
            Ok(false) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    /// Reject a LFA E_0624 approval queue entry.
    ///
    /// This dispatches `gpke.nb-lieferende.ablehnen` (PID 55009 Strom) to `makod`
    /// AND marks the entry as `Rejected` in the database.
    pub async fn reject_queue_entry(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        Path(id_str): Path<String>,
    ) -> impl IntoResponse {
        let Ok(id) = id_str.parse::<uuid::Uuid>() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let queue = PgApprovalQueue::new(pool.clone());

        // Fetch entry first to get process_id, pid, malo_id for dispatch.
        let entry = match queue.find_by_id(id, &state.tenant).await {
            Ok(Some(e)) => e,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

        // Determine the command name from PID.
        // Strom: gpke.nb-lieferende.ablehnen → PID 55009 (LF Ablehnung Lieferende)
        // Gas stornierung 44022/44023: reject means the operator declines to initiate stornierung.
        //   Mark as Rejected in the queue without dispatching to makod.
        let command = match entry.pid as u32 {
            55008 => mako_markt::commands::GPKE_NB_LIEFERENDE_ABLEHNEN,
            44022 | 44023 => {
                // For Gas stornierung rejections, there is no inbound command to dispatch
                // (the GNB was not queried). Mark as Rejected immediately.
                return match queue.reject(id, &state.tenant).await {
                    Ok(true) => {
                        tracing::info!(%id, "processd: Gas stornierung approval rejected by operator");
                        StatusCode::NO_CONTENT.into_response()
                    }
                    Ok(false) => StatusCode::NOT_FOUND.into_response(),
                    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
                };
            }
            _ => mako_markt::commands::GPKE_NB_LIEFERENDE_ABLEHNEN,
        };

        // Dispatch ablehnen to makod — BEFORE marking as Rejected.
        let idempotency_key = format!("processd-lf-reject-{id}");
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: None,
            command: command.to_owned(),
            malo_id: entry.malo_id.clone(),
            melo_id: None,
            payload: serde_json::json!({
                "process_id": entry.process_id,
                "reason": entry.reason,
                "rejected_by": "operator",
            }),
        };
        if let Err(e) = state.makod.post_command(&idempotency_key, &cmd).await {
            tracing::warn!(
                %id,
                process_id = %entry.process_id,
                error = %e,
                "processd: failed to dispatch ablehnen for rejected queue entry"
            );
            return (
                StatusCode::BAD_GATEWAY,
                format!("makod dispatch failed: {e}"),
            )
                .into_response();
        }

        // Mark as Rejected in DB.
        match queue.reject(id, &state.tenant).await {
            Ok(true) => {
                tracing::info!(%id, process_id = %entry.process_id, "processd: E_0624 rejected — ablehnen dispatched");
                StatusCode::NO_CONTENT.into_response()
            }
            Ok(false) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    pub async fn metrics(
        axum::Extension(pool): axum::Extension<sqlx::PgPool>,
    ) -> impl IntoResponse {
        let mut out = String::with_capacity(1024);

        // ── NB STP decision counters ──────────────────────────────────────────
        // processd_decisions_total{decision, pid} — sourced from anmeldung_decisions table.
        let decisions: Vec<(String, i32, i64)> = sqlx::query_as(
            r"SELECT decision::text, pid, COUNT(*)::bigint
              FROM anmeldung_decisions
              GROUP BY decision, pid
              ORDER BY pid, decision",
        )
        .fetch_all(&pool)
        .await
        .unwrap_or_default();

        out.push_str("# HELP processd_decisions_total NB STP Anmeldung decisions (Accept/Reject/Escalate) by PID.\n");
        out.push_str("# TYPE processd_decisions_total counter\n");
        for (decision, pid, count) in &decisions {
            out.push_str(&format!(
                "processd_decisions_total{{decision=\"{decision}\",pid=\"{pid}\"}} {count}\n"
            ));
        }

        // ── LF approval queue depth ───────────────────────────────────────────
        let queue_depth: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM approval_queue WHERE resolved_at IS NULL")
                .fetch_one(&pool)
                .await
                .unwrap_or(0);

        out.push_str(
            "# HELP processd_approval_queue_depth Pending LF E_0624 approval-queue entries.\n",
        );
        out.push_str("# TYPE processd_approval_queue_depth gauge\n");
        out.push_str(&format!("processd_approval_queue_depth {queue_depth}\n"));

        // ── DB pool health ────────────────────────────────────────────────────
        let pool_size = pool.size();
        let pool_idle = pool.num_idle();

        out.push_str("# HELP processd_db_pool_size Current PostgreSQL connection pool size.\n");
        out.push_str("# TYPE processd_db_pool_size gauge\n");
        out.push_str(&format!("processd_db_pool_size {pool_size}\n"));
        out.push_str("# HELP processd_db_pool_idle Idle PostgreSQL connections.\n");
        out.push_str("# TYPE processd_db_pool_idle gauge\n");
        out.push_str(&format!("processd_db_pool_idle {pool_idle}\n"));

        (
            axum::http::StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4",
            )],
            out,
        )
    }

    /// `POST /api/v1/start-supply` — ERP initiates a GPKE Lieferbeginn (Strom SLP).
    ///
    /// Validates the LFW24 Mindestvorlauffrist and dispatches
    /// `gpke.lieferbeginn.anmelden` to `makod`.
    ///
    /// ## Request body (JSON)
    ///
    /// | Field               | Type   | Required | Notes |
    /// |---------------------|--------|----------|-------|
    /// | `malo_id`           | string | ✓        | 11-digit Strom Marktlokations-ID |
    /// | `lieferbeginn_datum` | string | ✓        | ISO-8601 date (YYYY-MM-DD) |
    /// | `transaktionsgrund` | string | —        | SG4 STS DE9013 (`E01` Ein-/Auszug, `E03` Wechsel); forwarded onto the outbound UTILMD. The Strom date rule is identical for both (LFW24) |
    ///
    /// ## LFW24 Vorlauffrist (BK6-22-024, consolidated in BK6-24-174 GPKE Teil 2)
    ///
    /// SD "Lieferbeginn" Prozessschritt 1: "Unverzüglich nach Vorliegen des
    /// Anmeldegrundes, jedoch **spätester ÜT ist der Tag vor dem letzten WT vor
    /// dem Zuordnungsbeginn**." The Frist is day-granular (ÜT = calendar day of
    /// the AS4 receipt) — there is **no time-of-day cutoff**.
    ///
    /// - Earliest Lieferbeginn for a submission today: the calendar day **after
    ///   the next Werktag** after today (Berlin date).
    /// - Retroactive dates (`lieferbeginn_datum` < today Berlin) are always rejected.
    pub async fn start_supply(
        State(state): State<ProcessdState>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        use mako_engine::fristen::{self, HolidayCalendar};
        use time_tz::{OffsetDateTimeExt as _, timezones};

        let malo_id = match body
            .get("malo_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(id) => id.to_owned(),
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({
                        "error": "MISSING_MALO_ID",
                        "message": "\"malo_id\" is required (11-digit Strom Marktlokations-ID)"
                    })),
                )
                    .into_response();
            }
        };

        let lieferbeginn_str = match body
            .get("lieferbeginn_datum")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(d) => d.to_owned(),
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({
                        "error": "MISSING_LIEFERBEGINN",
                        "message": "\"lieferbeginn_datum\" is required (ISO-8601 date, e.g. \"2026-10-01\")"
                    })),
                )
                    .into_response();
            }
        };

        // Optional SG4 STS Transaktionsgrund — forwarded onto the outbound
        // UTILMD. Under LFW24 the Strom date rule is Transaktionsgrund-
        // independent, so it does not alter the validation below.
        let transaktionsgrund = body
            .get("transaktionsgrund")
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        // Parse the requested Lieferbeginn date.
        let lieferbeginn = match time::Date::parse(
            &lieferbeginn_str,
            time::macros::format_description!("[year]-[month]-[day]"),
        ) {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({
                        "error": "INVALID_DATE",
                        "message": format!("\"lieferbeginn_datum\" is not a valid ISO-8601 date: {lieferbeginn_str:?}")
                    })),
                )
                    .into_response();
            }
        };

        // ── LFW24 Vorlauffrist validation ─────────────────────────────────────
        //
        // Source: BK6-24-174 GPKE Teil 2 (Lesefassung), SD "Lieferbeginn"
        // Prozessschritt 1 (the LFW24 rules of BK6-22-024, in force since
        // 2025-06-06): "Unverzüglich nach Vorliegen des Anmeldegrundes, jedoch
        // spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn."
        // The Frist is day-granular — there is no time-of-day cutoff. Inverted:
        // the earliest Zuordnungsbeginn is the calendar day after the next
        // Werktag after the submission day (German local date).
        let berlin = timezones::db::europe::BERLIN;
        let now_utc = time::OffsetDateTime::now_utc();
        let now_berlin = now_utc.to_timezone(berlin);
        let today_berlin = now_berlin.date();

        if lieferbeginn < today_berlin {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "RETROACTIVE_DATE",
                    "message": format!(
                        "Retroactive Lieferbeginn is forbidden. Requested: {lieferbeginn}, today (Berlin): {today_berlin}"
                    )
                })),
            )
                .into_response();
        }

        // Earliest allowed Lieferbeginn: the calendar day after the next
        // Werktag after today (the Werktag in between is the NB's processing
        // day — its answer is due 11:00 Uhr des 1. WT nach dem ÜT).
        let base = fristen::add_werktage(today_berlin, 1, HolidayCalendar::BdewMaKo)
            .next_day()
            .expect("date overflow");

        if lieferbeginn < base {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "VORLAUFFRIST_VIOLATION",
                    "message": format!(
                        "LFW24 Mindestvorlauffrist not met. \
                         Earliest allowed Lieferbeginn: {base}. \
                         Requested: {lieferbeginn}. \
                         (Spätester ÜT ist der Tag vor dem letzten WT vor dem \
                         Zuordnungsbeginn — BK6-24-174 GPKE Teil 2, SD Lieferbeginn)"
                    ),
                    "earliest_lieferbeginn": base.to_string(),
                    "berlin_date": today_berlin.to_string(),
                    "cutoff_rule": "spätester ÜT = Tag vor dem letzten WT vor dem Zuordnungsbeginn (day-granular, no time-of-day cutoff)"
                })),
            )
                .into_response();
        }

        // ── Dispatch to makod ─────────────────────────────────────────────────
        let idempotency_key = format!("processd-start-supply-{malo_id}-{lieferbeginn}");
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: Some("LF".to_owned()),
            command: mako_markt::commands::GPKE_LIEFERBEGINN_ANMELDEN.to_owned(),
            malo_id: Some(malo_id.clone()),
            melo_id: None,
            payload: serde_json::json!({
                "malo_id": malo_id,
                "lieferbeginn_datum": lieferbeginn.to_string(),
                // Optional SG4 STS Transaktionsgrund (E01 Ein-/Auszug,
                // E03 Wechsel) — forwarded onto the outbound UTILMD.
                "transaktionsgrund": transaktionsgrund,
            }),
        };
        match state.makod.post_command(&idempotency_key, &cmd).await {
            Ok(accepted) => (
                StatusCode::ACCEPTED,
                axum::Json(serde_json::json!({
                    "process_id": accepted.process_id,
                    "command": "gpke.lieferbeginn.anmelden",
                    "malo_id": malo_id,
                    "lieferbeginn_datum": lieferbeginn.to_string(),
                    "status": "initiated",
                    "vorlauffrist": {
                        "earliest_allowed": base.to_string(),
                        "berlin_date_at_submission": today_berlin.to_string(),
                    }
                })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "error": "MAKOD_DISPATCH_FAILED",
                    "message": e.to_string()
                })),
            )
                .into_response(),
        }
    }

    /// `POST /api/v1/start-supply-gas` — ERP initiates a GeLi Gas Lieferbeginn (Gas 44001).
    ///
    /// Dispatches `geli.lieferbeginn.anmelden` to `makod`.
    ///
    /// ## Request body (JSON)
    ///
    /// | Field          | Type   | Required | Notes |
    /// |----------------|--------|----------|-------|
    /// | `malo_id`      | string | ✓        | 11-digit Gas-MaLo-ID (IDE+Z19, EIC) |
    /// | `zaehlpunkt`   | string | ✓        | Zählpunktbezeichnung (RFF+Z13) — **mandatory** per AHB |
    /// | `process_date` | string | ✓        | Lieferbeginn date (YYYYMMDD in CET/CEST) |
    ///
    /// **Both `malo_id` and `zaehlpunkt` are mandatory** (BK7-24-01-009 AHB rules
    /// `AHB-44001-IDE-M` and `AHB-44001-RFF-M`). There is no Gas equivalent of
    /// API-Webdienste Strom — the ERP must supply the Gas-MaLo-ID upfront.
    pub async fn start_supply_gas(
        State(state): State<ProcessdState>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        // Validate mandatory Gas fields before forwarding.
        let malo_id = match body.get("malo_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_owned(),
            _ => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({
                        "error": "MISSING_MALO_ID",
                        "message": "\"malo_id\" is required (11-digit Gas-MaLo-ID, IDE+Z19)"
                    })),
                )
                    .into_response();
            }
        };
        if body
            .get("zaehlpunkt")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .is_none()
        {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "MISSING_ZAEHLPUNKT",
                    "message": "\"zaehlpunkt\" is required (Zählpunktbezeichnung, RFF+Z13) — mandatory per BK7-24-01-009 AHB"
                })),
            )
                .into_response();
        }
        if body
            .get("process_date")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .is_none()
        {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "MISSING_PROCESS_DATE",
                    "message": "\"process_date\" is required (Lieferbeginn date, YYYYMMDD in CET/CEST)"
                })),
            )
                .into_response();
        }

        // Forward to makod `geli.lieferbeginn.anmelden`.
        let idempotency_key = format!("processd-start-supply-gas-{malo_id}");
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: Some("LF".to_owned()),
            command: mako_markt::commands::GELI_LIEFERBEGINN_ANMELDEN.to_owned(),
            malo_id: Some(malo_id.clone()),
            melo_id: None,
            payload: body,
        };
        match state.makod.post_command(&idempotency_key, &cmd).await {
            Ok(accepted) => (
                StatusCode::ACCEPTED,
                axum::Json(serde_json::json!({
                    "process_id": accepted.process_id,
                    "command": "geli.lieferbeginn.anmelden",
                    "malo_id": malo_id,
                    "status": "initiated",
                    "message": "GeLi Gas Lieferbeginn (PID 44001) initiated — awaiting GNB confirmation (10 Werktage)"
                })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "error": "MAKOD_DISPATCH_FAILED",
                    "message": e.to_string()
                })),
            )
                .into_response(),
        }
    }

    /// `POST /api/v1/end-supply` — ERP initiates a GPKE Lieferende (Strom).
    ///
    /// Forwards to makod `gpke.lieferende.anmelden` (PID 55002). The notice
    /// period that governs *when* a Lieferende is valid is enforced upstream
    /// in `vertragd` (§14 StromGVV); this endpoint validates only that the
    /// mandatory fields are present.
    pub async fn end_supply(
        State(state): State<ProcessdState>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        end_supply_inner(
            state,
            &body,
            mako_markt::commands::GPKE_LIEFERENDE_ANMELDEN,
            "processd-end-supply",
        )
        .await
    }

    /// `POST /api/v1/end-supply-gas` — ERP initiates a GeLi Gas Lieferende (44002).
    pub async fn end_supply_gas(
        State(state): State<ProcessdState>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        end_supply_inner(
            state,
            &body,
            mako_markt::commands::GELI_LIEFERENDE_ANMELDEN,
            "processd-end-supply-gas",
        )
        .await
    }

    /// Shared Lieferende dispatch for Strom and Gas.
    async fn end_supply_inner(
        state: ProcessdState,
        body: &serde_json::Value,
        command: &str,
        key_prefix: &str,
    ) -> axum::response::Response {
        let malo_id = match body
            .get("malo_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(id) => id.to_owned(),
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({
                        "error": "MISSING_MALO_ID",
                        "message": "\"malo_id\" is required"
                    })),
                )
                    .into_response();
            }
        };
        let lieferende_str = match body
            .get("lieferende_datum")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(d) => d.to_owned(),
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(serde_json::json!({
                        "error": "MISSING_LIEFERENDE",
                        "message": "\"lieferende_datum\" is required (ISO-8601 date)"
                    })),
                )
                    .into_response();
            }
        };

        // Idempotent on (malo, date): a redelivered Kündigung must not open a
        // second Lieferende process at makod.
        let idempotency_key = format!("{key_prefix}-{malo_id}-{lieferende_str}");
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: Some("LF".to_owned()),
            command: command.to_owned(),
            malo_id: Some(malo_id.clone()),
            melo_id: None,
            payload: serde_json::json!({
                "malo_id": malo_id,
                "lieferende_datum": lieferende_str,
            }),
        };
        match state.makod.post_command(&idempotency_key, &cmd).await {
            Ok(accepted) => (
                StatusCode::ACCEPTED,
                axum::Json(serde_json::json!({
                    "process_id": accepted.process_id,
                    "command": command,
                    "malo_id": malo_id,
                    "lieferende_datum": lieferende_str,
                    "status": "initiated",
                })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "error": "MAKOD_DISPATCH_FAILED",
                    "message": e.to_string()
                })),
            )
                .into_response(),
        }
    }
}
