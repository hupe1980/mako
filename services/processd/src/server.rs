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
    pub queue: PgApprovalQueue,
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
    /// When `true`, an `Accept` MSB-Wechsel verdict dispatches the Bestätigung
    /// itself; when `false` it goes to the approval queue.
    pub msb_auto_accept: bool,
    /// When `true`, auto-dispatch QUOTES from `PreisblattMessung` on REQOTE arrival.
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
    /// See [`ProcessdState::msb_auto_accept`].
    pub msb_auto_accept: bool,
    /// See [`ProcessdState::msb_auto_preisanfrage`].
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
            // Authenticated: `marktd` requires a bearer on every write, and
            // subscription management is an operator-level capability there.
            // Without this the PUT is a 401 against any marktd that has OIDC
            // configured, the retry loop exhausts, and `build_router` returns
            // an error — processd refuses to start. It only ever worked because
            // the demo runs marktd with `allow_insecure_no_auth`.
            match http
                .put(&sub_url)
                .bearer_auth(cfg.marktd_api_key.expose_secret())
                .json(&body)
                .send()
                .await
            {
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
                    // 401/403 will not fix itself by waiting: the key or the
                    // principal's role is wrong. Fail immediately with the
                    // reason rather than after 30 s of identical retries.
                    if status == reqwest::StatusCode::UNAUTHORIZED
                        || status == reqwest::StatusCode::FORBIDDEN
                    {
                        return Err(anyhow::anyhow!(
                            "processd: self-registration rejected by marktd with HTTP {status} — \
                             check `[marktd] api_key` and that its principal may manage \
                             subscriptions (marktd Cedar action `manage-subscription`, ADMIN role)"
                        ));
                    }
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
            queue: PgApprovalQueue::new(pool.clone()),
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
    //
    // Deliberately not role-gated: the NB, LF and MSB modules all enqueue, so
    // every role build needs its entries to reach `Expired` — that status is the
    // operator's reconciliation surface.
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
                                    // REGULATORY WARNING: an expired entry is a market
                                    // message whose *business* answer Frist has run out —
                                    // 24 h GPKE, 3/5/7/1 WT WiM, 10 WT GeLi Gas. (The
                                    // 45-minute APERAK clock is makod's.) Reconcile via
                                    // GET /api/v1/queue?status=Expired.
                                    tracing::warn!(
                                        expired = n,
                                        "processd: {n} approval queue entries expired past their \
                                         business answer Frist — operator must reconcile"
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
        msb_auto_accept: cfg.msb_auto_accept,
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

    // ── Background: Prometheus gauge sampling ──────────────────────────────
    crate::metrics::spawn_sampler(pool.clone(), cfg.tenant.clone(), ctx.shutdown.clone());

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
    // owned by `mako_service::run`. processd's own metrics register on the same
    // Prometheus registry (see `crate::metrics`), so they are served from that
    // one `/metrics` rather than a second endpoint of its own.
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
    use std::sync::Arc;

    use axum::{
        Extension, Json,
        extract::{Path, State},
        http::StatusCode,
        response::IntoResponse,
    };

    use mako_service::{cedar::CedarEnforcer, oidc::Claims};
    use sqlx::PgPool;

    use crate::{
        pg::{PgAnmeldungRepository, PgApprovalQueue},
        server::ProcessdState,
    };

    /// Authorize `action` for the caller, or produce the `403`.
    ///
    /// processd does not merely report decisions, it makes them: approving a
    /// queue entry dispatches the market answer, and `start-supply` /
    /// `end-supply` commit the operator to a market position. Every route below
    /// therefore takes a `Claims` extractor (which is what authenticates the
    /// request — there is no global auth middleware) and passes through here.
    // The `Err` is a fully-formed HTTP response the caller returns as-is, the
    // same shape `mako_service::mcp_auth::authorize` uses; boxing it would add
    // an allocation on every denial for no benefit.
    #[allow(clippy::result_large_err)]
    fn authorize(
        enforcer: &CedarEnforcer,
        claims: &Claims,
        action: &'static str,
        tenant: &str,
    ) -> Result<(), axum::response::Response> {
        enforcer
            .check(&claims.principal(), action, tenant)
            .map_err(|_| {
                (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error":   "FORBIDDEN",
                        "message": format!("{action} denied for this principal"),
                    })),
                )
                    .into_response()
            })
    }

    /// The principal to record as the deciding operator.
    ///
    /// § 20 Abs. 1 EnWG parity evidence and the GoBD trail both need to say
    /// *who* decided, so this is the principal's `sub`, never a fixed label.
    fn decided_by(claims: &Claims) -> String {
        claims.principal().sub
    }

    /// Turn a failed `makod` command dispatch into a response.
    ///
    /// The status is taken from the error itself rather than hard-coded to 502.
    /// A `MakodConflict` (makod's `invalid_state` — the command is not legal in
    /// the process's current state) is a caller error at 409: repeating it will
    /// fail identically, and callers such as `vertragd` retry on 5xx, so
    /// reporting it as a gateway failure produced three pointless retries and a
    /// component parked in `ANGELEGT` with a misleading "processd unreachable"
    /// trail. Transport failures still surface as 502.
    fn makod_dispatch_error(e: &mako_markt::error::MdmError) -> axum::response::Response {
        let status =
            StatusCode::from_u16(e.status_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let status = if status == StatusCode::INTERNAL_SERVER_ERROR {
            StatusCode::BAD_GATEWAY
        } else {
            status
        };
        (
            status,
            Json(serde_json::json!({
                "error":   if status == StatusCode::CONFLICT {
                    "MAKOD_COMMAND_CONFLICT"
                } else {
                    "MAKOD_DISPATCH_FAILED"
                },
                "message": e.to_string(),
                "retryable": status.is_server_error(),
            })),
        )
            .into_response()
    }

    pub async fn list_decisions(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "read-decisions", &state.tenant) {
            return deny;
        }
        let repo = PgAnmeldungRepository::new(pool);
        match repo.list(&state.tenant, 100).await {
            Ok(records) => Json(serde_json::to_value(records).unwrap_or_default()).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    pub async fn list_queue(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "read-queue", &state.tenant) {
            return deny;
        }
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
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "read-eog", &state.tenant) {
            return deny;
        }
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

    /// Claim a pending entry, dispatch its market command, release the claim on
    /// failure.
    ///
    /// The claim comes **first**. Dispatching before the `status = 'Pending'`
    /// guard let a terminal entry re-send its market command, and let operator A
    /// approving while operator B rejected send both an einwilligung and an
    /// ablehnen (different idempotency keys) while the DB recorded one decision.
    async fn decide_queue_entry(
        state: &ProcessdState,
        pool: PgPool,
        id_str: &str,
        approve: bool,
        decided_by: &str,
    ) -> axum::response::Response {
        let Ok(id) = id_str.parse::<uuid::Uuid>() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let queue = PgApprovalQueue::new(pool);
        let target = if approve {
            crate::pg::approval::QueueStatus::Approved
        } else {
            crate::pg::approval::QueueStatus::Rejected
        };

        let entry = match queue.claim(id, &state.tenant, target, decided_by).await {
            Ok(Some(e)) => e,
            // Absent or already decided — nothing to dispatch.
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

        // Every enqueuing module resolves both commands from the trigger PID at
        // enqueue time, so a stored `None` means "this decision has no market
        // message" — never "derive one from the PID here". A guess would answer
        // the wrong process, to the wrong counterparty, with a valid message.
        let stored = if approve {
            entry.approve_command.as_deref()
        } else {
            entry.reject_command.as_deref()
        };
        let Some(command) = stored else {
            tracing::info!(%id, pid = entry.pid, approve, "processd: queue entry decided without a market message");
            return StatusCode::NO_CONTENT.into_response();
        };

        // The key names the queue entry, not a role — the NB, LF and MSB all
        // enqueue here. The UUID makes it unique; the prefix is what an
        // operator reads in makod's idempotency log.
        let verb = if approve { "approve" } else { "reject" };
        let cmd = mako_markt::makod_client::ForwardCommand {
            marktrolle: entry.marktrolle.clone(),
            command: command.to_owned(),
            malo_id: entry.malo_id.clone(),
            melo_id: None,
            payload: if approve {
                serde_json::json!({
                    "process_id": entry.process_id,
                    "approved_by": decided_by,
                })
            } else {
                serde_json::json!({
                    "process_id": entry.process_id,
                    "reason": entry.reason,
                    "rejected_by": decided_by,
                })
            },
        };
        if let Err(e) = state
            .makod
            .post_command(&format!("processd-queue-{verb}-{id}"), &cmd)
            .await
        {
            tracing::warn!(
                %id,
                process_id = %entry.process_id,
                error = %e,
                "processd: makod dispatch failed for decided queue entry — releasing the claim"
            );
            // Back to Pending so the operator can retry; leaving it decided
            // would record a decision the market never saw.
            if let Err(e) = queue.unclaim(id, &state.tenant).await {
                tracing::error!(%id, error = %e, "processd: failed to release the queue claim");
            }
            return (
                StatusCode::BAD_GATEWAY,
                format!("makod dispatch failed: {e}"),
            )
                .into_response();
        }

        tracing::info!(%id, process_id = %entry.process_id, command, "processd: queue entry {verb}d — command dispatched");
        StatusCode::NO_CONTENT.into_response()
    }

    /// Approve an approval-queue entry: claim it, then dispatch its command.
    ///
    /// **Regulatory note:** `expires_at` carries the *business* answer Frist of
    /// the queued process — 24 h (GPKE), 3/5/7/1 WT (WiM), 10 WT (GeLi Gas) —
    /// less an hour of headroom. Operators must act before it.
    pub async fn approve_queue_entry(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        Path(id_str): Path<String>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "decide-queue", &state.tenant) {
            return deny;
        }
        decide_queue_entry(&state, pool, &id_str, true, &decided_by(&claims)).await
    }

    /// Reject an approval-queue entry: claim it, then dispatch its command.
    pub async fn reject_queue_entry(
        State(state): State<ProcessdState>,
        Extension(pool): Extension<PgPool>,
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        Path(id_str): Path<String>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "decide-queue", &state.tenant) {
            return deny;
        }
        decide_queue_entry(&state, pool, &id_str, false, &decided_by(&claims)).await
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
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "initiate-supply", &state.tenant) {
            return deny;
        }
        use mako_fristen::{self as fristen, HolidayCalendar};
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
            Err(e) => makod_dispatch_error(&e),
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
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "initiate-supply", &state.tenant) {
            return deny;
        }
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
        //
        // The key carries the Lieferbeginn as well as the MaLo, matching the
        // Strom sibling above and the Lieferende key below. A MaLo can legitimately
        // have more than one Lieferbeginn over its life (move-out then move-back-in,
        // or a corrected date after a GNB rejection), so a MaLo-only key would name
        // two genuinely different commands identically.
        let idempotency_key = format!(
            "processd-start-supply-gas-{malo_id}-{}",
            body.get("process_date")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
        );
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
            Err(e) => makod_dispatch_error(&e),
        }
    }

    /// `POST /api/v1/end-supply` — ERP initiates a GPKE Lieferende (Strom).
    ///
    /// Forwards to makod `gpke.lieferende.anmelden` (PID 55004 Abmeldung). The notice
    /// period that governs *when* a Lieferende is valid is enforced upstream
    /// in `vertragd` (§ 20 Abs. 1 StromGVV/GasGVV in der Grundversorgung, sonst
    /// vertraglich); this endpoint validates only that the
    /// mandatory fields are present.
    pub async fn end_supply(
        State(state): State<ProcessdState>,
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "initiate-supply", &state.tenant) {
            return deny;
        }
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
        Extension(enforcer): Extension<Arc<CedarEnforcer>>,
        claims: Claims,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        if let Err(deny) = authorize(&enforcer, &claims, "initiate-supply", &state.tenant) {
            return deny;
        }
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
            Err(e) => makod_dispatch_error(&e),
        }
    }
}
