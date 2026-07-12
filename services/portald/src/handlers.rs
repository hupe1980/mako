//! HTTP handlers for `portald` — customer portal read-model gateway.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response, Sse},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{clients::UpstreamClient, config::PortaldConfig};

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared upstream clients injected via `Extension`.
#[derive(Clone)]
pub struct PortalClients {
    pub edmd: Option<Arc<UpstreamClient>>,
    pub billingd: Option<Arc<UpstreamClient>>,
    pub accountingd: Option<Arc<UpstreamClient>>,
    pub einsd: Option<Arc<UpstreamClient>>,
    pub marktd: Option<Arc<UpstreamClient>>,
    /// Write-capable client for `vertragd` — used by portal self-service write API (L3).
    pub vertragd: Option<Arc<UpstreamClient>>,
}

// ── Authorization ─────────────────────────────────────────────────────────────

/// Verify the authenticated customer may access `malo_id`.
///
/// Calls `vertragd GET /kunden/authenticate?malo_id={malo_id}`, passing the
/// inbound `Authorization: Bearer` header unchanged.  `vertragd` decodes the
/// JWT sub and checks whether this customer owns the MaLo.
///
/// Returns `Ok(())` when authorized or when `vertragd_url` is absent (dev mode).
/// Returns `Err(Response)` with 401/403 when denied.
async fn authorize_malo_access(
    cfg: &PortaldConfig,
    headers: &axum::http::HeaderMap,
    malo_id: &str,
) -> Result<(), axum::response::Response> {
    let vertragd_url = match &cfg.vertragd_url {
        Some(url) => url.trim_end_matches('/').to_owned(),
        None => {
            tracing::debug!(
                malo_id,
                "portald: no vertragd_url — authorization skipped (dev mode)"
            );
            return Ok(());
        }
    };
    let auth_header = match headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        Some(h) => h.to_owned(),
        None => {
            return Err(
                (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response()
            );
        }
    };

    let client = reqwest::Client::new();
    let mut req = client
        .get(format!("{vertragd_url}/api/v1/kunden/authenticate"))
        .query(&[("malo_id", malo_id)])
        .header("Authorization", &auth_header);
    if let Some(ref key) = cfg.vertragd_api_key {
        req = req.bearer_auth(key);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
            Err((StatusCode::UNAUTHORIZED, "not authenticated").into_response())
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::FORBIDDEN => {
            tracing::warn!(malo_id, "portald: customer not authorized for this MaLo");
            Err((
                StatusCode::FORBIDDEN,
                "not authorized to access this delivery point",
            )
                .into_response())
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => Err((
            StatusCode::UNAUTHORIZED,
            "no customer profile found for this identity",
        )
            .into_response()),
        Ok(_) | Err(_) => {
            tracing::warn!(malo_id, "portald: vertragd auth check failed");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "authorization service unavailable",
            )
                .into_response())
        }
    }
}

// ── Dashboard ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/dashboard`
///
/// Aggregates supply status, account balance, current meter read, latest invoice,
/// and EEG settlement status into a single JSON object.  Each field is `null`
/// when the upstream service is not configured or returned 404.
pub async fn get_dashboard(
    headers: axum::http::HeaderMap,
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    // Authorize: verify the JWT sub owns this MaLo via vertragd.
    if let Err(resp) = authorize_malo_access(&cfg, &headers, &malo_id).await {
        return resp;
    }
    // Fetch in parallel.
    let (versorgung, balance, last_invoice, lastgang_summary) = tokio::join!(
        async {
            match &clients.marktd {
                Some(c) => c
                    .get_json(&format!("/api/v1/versorgung/{malo_id}"))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
        async {
            match &clients.accountingd {
                Some(c) => c
                    .get_json(&format!("/api/v1/accounts/{malo_id}/balance"))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
        async {
            match &clients.billingd {
                Some(c) => c
                    .get_json(&format!("/api/v1/billing?malo_id={malo_id}&limit=1"))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
        async {
            match &clients.edmd {
                Some(c) => c
                    .get_json(&format!("/api/v1/billing-period/{malo_id}"))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        },
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id": malo_id,
            "tenant": cfg.tenant,
            "versorgung": versorgung,
            "balance": balance,
            "last_invoice": last_invoice,
            "meter_summary": lastgang_summary,
        })),
    )
        .into_response()
}

// ── Lastgang ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LastgangQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// `GET /api/v1/portal/{malo_id}/lastgang`
///
/// Proxies `GET edmd /api/v1/lastgang/{malo_id}?from=…&to=…`.
pub async fn get_lastgang(
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
    Query(q): Query<LastgangQuery>,
) -> impl IntoResponse {
    let Some(edmd) = &clients.edmd else {
        return (StatusCode::SERVICE_UNAVAILABLE, "edmd not configured").into_response();
    };
    let mut path = format!("/api/v1/lastgang/{malo_id}");
    if let (Some(from), Some(to)) = (q.from, q.to) {
        path = format!("{path}?from={from}&to={to}");
    }
    proxy_json(edmd, &path).await
}

// ── Invoices ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct InvoicesQuery {
    pub limit: Option<i64>,
    pub outcome: Option<String>,
}

/// `GET /api/v1/portal/{malo_id}/invoices`
///
/// Proxies `GET billingd /api/v1/billing?malo_id=…`.
pub async fn get_invoices(
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
    Query(q): Query<InvoicesQuery>,
) -> impl IntoResponse {
    let Some(billingd) = &clients.billingd else {
        return (StatusCode::SERVICE_UNAVAILABLE, "billingd not configured").into_response();
    };
    let limit = q.limit.unwrap_or(24);
    let mut path = format!("/api/v1/billing?malo_id={malo_id}&limit={limit}");
    if let Some(outcome) = q.outcome {
        path = format!("{path}&outcome={outcome}");
    }
    proxy_json(billingd, &path).await
}

// ── Account balance + ledger ──────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/balance`
pub async fn get_balance(
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    proxy_or_unavailable(
        &clients.accountingd,
        &format!("/api/v1/accounts/{malo_id}/balance"),
        "accountingd",
    )
    .await
}

/// `GET /api/v1/portal/{malo_id}/kontoauszug`
pub async fn get_kontoauszug(
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    proxy_or_unavailable(
        &clients.accountingd,
        &format!("/api/v1/accounts/{malo_id}/kontoauszug"),
        "accountingd",
    )
    .await
}

// ── EEG status ────────────────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/eeg`
///
/// Returns EEG plant list and latest settlements for a given MaLo.
/// Proxies `GET einsd /api/v1/anlagen?malo_id={malo_id}`.
pub async fn get_eeg_status(
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    proxy_or_unavailable(
        &clients.einsd,
        &format!("/api/v1/anlagen?malo_id={malo_id}"),
        "einsd",
    )
    .await
}

// ── VersorgungsStatus ─────────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/versorgung`
pub async fn get_versorgung(
    Extension(clients): Extension<Arc<PortalClients>>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    proxy_or_unavailable(
        &clients.marktd,
        &format!("/api/v1/versorgung/{malo_id}"),
        "marktd",
    )
    .await
}

// ── Server-Sent Events stream ─────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/events`
///
/// Real-time SSE stream.  Currently emits a keepalive heartbeat every 30 s.
/// In production, wire this to an internal notification channel populated by
/// `accountingd` / `billingd` / `einsd` CloudEvents.
pub async fn sse_events(Path(malo_id): Path<String>) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive};
    use tokio::time::interval;
    use tokio_stream::StreamExt as _;
    use tokio_stream::wrappers::IntervalStream;

    let malo = malo_id.clone();
    let stream = IntervalStream::new(interval(std::time::Duration::from_secs(30))).map(move |_| {
        Ok::<_, std::convert::Infallible>(
            Event::default()
                .event("heartbeat")
                .data(serde_json::json!({ "malo_id": malo }).to_string()),
        )
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn proxy_json(client: &UpstreamClient, path: &str) -> Response {
    match client.get_json(path).await {
        Ok(Some(body)) => (StatusCode::OK, Json(body)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

async fn proxy_or_unavailable(
    client: &Option<Arc<UpstreamClient>>,
    path: &str,
    service: &'static str,
) -> Response {
    match client {
        Some(c) => proxy_json(c, path).await,
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{service} not configured"),
        )
            .into_response(),
    }
}

// ── Self-service write API (L3 — §41 EnWG customer rights) ───────────────────

/// Resolved portal authentication context.
///
/// Returned by `authenticate_and_resolve()` after verifying the customer's
/// OIDC JWT against `vertragd /kunden/authenticate`.
struct PortalAuthCtx {
    pub kunden_id: uuid::Uuid,
    #[allow(dead_code)]
    pub kundentyp: String,
}

/// Authenticate the customer and return identity context.
///
/// Calls `vertragd GET /kunden/authenticate?malo_id={malo_id}` with the
/// inbound Authorization header.  Returns the resolved `kunden_id` on success
/// or an HTTP error response on failure.
async fn authenticate_and_resolve(
    cfg: &PortaldConfig,
    headers: &axum::http::HeaderMap,
    malo_id: &str,
) -> Result<PortalAuthCtx, Response> {
    let vertragd_url = match &cfg.vertragd_url {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => {
            // Dev mode: skip auth, return dummy context.
            return Ok(PortalAuthCtx {
                kunden_id: uuid::Uuid::nil(),
                kundentyp: "B2C".into(),
            });
        }
    };
    let auth = match headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        Some(h) => h.to_owned(),
        None => {
            return Err(
                (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response()
            );
        }
    };

    let client = reqwest::Client::new();
    let mut req = client
        .get(format!("{vertragd_url}/api/v1/kunden/authenticate"))
        .query(&[("malo_id", malo_id)])
        .header("Authorization", &auth);
    if let Some(ref key) = cfg.vertragd_api_key {
        req = req.header("X-Api-Key", key.as_str());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Err((StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response()),
    };
    match resp.status() {
        s if s.is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let kunden_id = body
                .get("kunden_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<uuid::Uuid>().ok())
                .unwrap_or(uuid::Uuid::nil());
            Ok(PortalAuthCtx {
                kunden_id,
                kundentyp: body
                    .get("kundentyp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("B2C")
                    .to_owned(),
            })
        }
        reqwest::StatusCode::UNAUTHORIZED => {
            Err((StatusCode::UNAUTHORIZED, "not authenticated").into_response())
        }
        reqwest::StatusCode::FORBIDDEN => Err((
            StatusCode::FORBIDDEN,
            "not authorized for this delivery point",
        )
            .into_response()),
        reqwest::StatusCode::NOT_FOUND => {
            Err((StatusCode::UNAUTHORIZED, "no customer profile found").into_response())
        }
        s => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("vertragd auth check failed: {s}"),
        )
            .into_response()),
    }
}

/// Resolve the active Versorgungsvertrag ID for a MaLo via vertragd.
///
/// Calls `GET /api/v1/vertraege?kunden_id={id}` and finds the vertrag whose
/// Vertragskomponente matches `malo_id`.  Returns the vertrag UUID or None.
async fn resolve_vertrag_for_malo(
    vertragd: &UpstreamClient,
    kunden_id: uuid::Uuid,
    malo_id: &str,
) -> Option<(uuid::Uuid, uuid::Uuid)> {
    // GET /api/v1/kunden/{kunden_id}/vertraege
    let vertraege = vertragd
        .get_json(&format!("/api/v1/kunden/{kunden_id}/vertraege"))
        .await
        .ok()??;
    let arr = vertraege.as_array()?;

    for v in arr {
        let vtid: uuid::Uuid = v.get("id")?.as_str()?.parse().ok()?;
        // GET /api/v1/vertraege/{vtid} — returns { vertrag, komponenten }
        if let Some(detail) = vertragd
            .get_json(&format!("/api/v1/vertraege/{vtid}"))
            .await
            .ok()
            .flatten()
            && let Some(komps) = detail.get("komponenten").and_then(|k| k.as_array())
        {
            for komp in komps {
                if komp.get("malo_id").and_then(|v| v.as_str()) == Some(malo_id) {
                    let komp_id: uuid::Uuid = komp.get("id")?.as_str()?.parse().ok()?;
                    return Some((vtid, komp_id));
                }
            }
        }
    }
    None
}

/// `GET /api/v1/portal/{malo_id}/vertrag`
///
/// Returns the active supply contract for the authenticated customer.
///
/// Response includes:
/// - Current product + tariff (from `tarifbd` via `vertragd`)
/// - Contract dates (Lieferbeginn, Lieferende)
/// - Notice periods (Kündigungsfrist)
/// - Vertragskomponenten with OBIS codes
///
/// **Prerequisite for all portal write operations** — clients should call this
/// before presenting Tarifwechsel / Kündigung options to the user.
pub async fn get_portal_vertrag(
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    headers: axum::http::HeaderMap,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    let auth_ctx = match authenticate_and_resolve(&cfg, &headers, &malo_id).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };
    let vertragd = match &clients.vertragd {
        Some(c) => c.as_ref(),
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "vertragd not configured").into_response();
        }
    };

    match resolve_vertrag_for_malo(vertragd, auth_ctx.kunden_id, &malo_id).await {
        Some((vtid, _komp_id)) => {
            match vertragd
                .get_json(&format!("/api/v1/vertraege/{vtid}"))
                .await
            {
                Ok(Some(v)) => (StatusCode::OK, Json(v)).into_response(),
                Ok(None) => StatusCode::NOT_FOUND.into_response(),
                Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            "no active supply contract found for this delivery point",
        )
            .into_response(),
    }
}

/// Request body for `POST /api/v1/portal/{malo_id}/tarifwechsel`.
#[derive(Debug, serde::Deserialize)]
pub struct PortalTarifwechselRequest {
    /// New product code in `tarifbd`.
    pub new_product_code: String,
    /// Effective date of the tariff switch (YYYY-MM-DD).
    ///
    /// **§41 EnWG** — must be at least 14 days from today.  Typically the
    /// 1st of the following month (billing cycle alignment).
    pub wirksamkeit: String,
    /// Optional customer reason (stored in audit trail).
    pub grund: Option<String>,
}

/// `POST /api/v1/portal/{malo_id}/tarifwechsel`
///
/// Customer-initiated tariff switch (§41 Abs. 1 EnWG).
///
/// Validates:
/// - JWT authentication + MaLo ownership
/// - `wirksamkeit >= today + 14 days` (§41 EnWG minimum notice)
/// - Resolves `vertrag_id` + `komp_id` from `malo_id` — customers need not know internal UUIDs
///
/// Proxies to `POST /api/v1/vertraege/{id}/tarifwechsel` on `vertragd`.
pub async fn post_portal_tarifwechsel(
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    headers: axum::http::HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalTarifwechselRequest>,
) -> impl IntoResponse {
    let auth_ctx = match authenticate_and_resolve(&cfg, &headers, &malo_id).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    // Validate wirksamkeit date.
    let wirksamkeit = match time::Date::parse(
        &req.wirksamkeit,
        &time::format_description::well_known::Iso8601::DATE,
    ) {
        Ok(d) => d,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "wirksamkeit must be YYYY-MM-DD").into_response();
        }
    };
    let today = time::OffsetDateTime::now_utc().date();
    if wirksamkeit < today + time::Duration::days(14) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "wirksamkeit must be at least 14 days from today (§41 EnWG minimum notice)",
        )
            .into_response();
    }

    let vertragd = match &clients.vertragd {
        Some(c) => c.as_ref(),
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "vertragd not configured").into_response();
        }
    };

    let (vtid, komp_id) =
        match resolve_vertrag_for_malo(vertragd, auth_ctx.kunden_id, &malo_id).await {
            Some(ids) => ids,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    "no active supply contract for this delivery point",
                )
                    .into_response();
            }
        };

    let body = serde_json::json!({
        "komp_id": komp_id,
        "new_product_code": req.new_product_code,
        "wirksamkeit": req.wirksamkeit,
        "grund": req.grund,
    });

    match vertragd
        .post_json(&format!("/api/v1/vertraege/{vtid}/tarifwechsel"), &body)
        .await
    {
        Ok((200..=299, resp_body)) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "message": "Tarifwechsel registered",
                "wirksamkeit": req.wirksamkeit,
                "new_product_code": req.new_product_code,
                "detail": resp_body,
            })),
        )
            .into_response(),
        Ok((status, body)) => (
            axum::http::StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(body),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/portal/{malo_id}/kuendigen`.
#[derive(Debug, serde::Deserialize)]
pub struct PortalKuendigungRequest {
    /// Last day of supply (YYYY-MM-DD).
    ///
    /// **§41 Abs. 3 EnWG** — for rolling (unbefristet) B2C contracts:
    /// must be at least 14 days from today and must fall on the last day of
    /// a calendar month (end-of-billing-period).
    pub lieferende: String,
    /// Cancellation reason (stored in audit trail, required for self-service).
    pub grund: Option<String>,
}

/// `POST /api/v1/portal/{malo_id}/kuendigen`
///
/// Customer-initiated contract cancellation (§41 Abs. 3 EnWG).
///
/// Validates:
/// - JWT authentication + MaLo ownership
/// - `lieferende >= today + 14 days` (§41 minimum notice for rolling contracts)
/// - `lieferende` falls on the last calendar day of a month (end of billing cycle)
///
/// Proxies to `POST /api/v1/vertraege/{id}/kuendigen` on `vertragd`, which
/// triggers UTILMD Lieferendemeldung via `processd`.
pub async fn post_portal_kuendigen(
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    headers: axum::http::HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalKuendigungRequest>,
) -> impl IntoResponse {
    let auth_ctx = match authenticate_and_resolve(&cfg, &headers, &malo_id).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    // Validate lieferende.
    let lieferende = match time::Date::parse(
        &req.lieferende,
        &time::format_description::well_known::Iso8601::DATE,
    ) {
        Ok(d) => d,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "lieferende must be YYYY-MM-DD").into_response();
        }
    };
    let today = time::OffsetDateTime::now_utc().date();
    if lieferende < today + time::Duration::days(14) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "lieferende must be at least 14 days from today (§41 EnWG minimum notice)",
        )
            .into_response();
    }
    // Must be last day of calendar month (billing cycle boundary).
    let next_day = lieferende.next_day().unwrap_or(lieferende);
    if next_day.day() != 1 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "lieferende must be the last day of a calendar month (end of billing cycle)",
        )
            .into_response();
    }

    let vertragd = match &clients.vertragd {
        Some(c) => c.as_ref(),
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "vertragd not configured").into_response();
        }
    };

    let (vtid, _) = match resolve_vertrag_for_malo(vertragd, auth_ctx.kunden_id, &malo_id).await {
        Some(ids) => ids,
        None => {
            return (
                StatusCode::NOT_FOUND,
                "no active supply contract for this delivery point",
            )
                .into_response();
        }
    };

    let body = serde_json::json!({
        "lieferende": req.lieferende,
        "grund": req.grund.as_deref().unwrap_or("Kundenkündigung über Kundenportal"),
    });

    match vertragd
        .post_json(&format!("/api/v1/vertraege/{vtid}/kuendigen"), &body)
        .await
    {
        Ok((200..=299, resp_body)) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "message": "Kündigung registered — UTILMD Lieferendemeldung will be dispatched",
                "lieferende": req.lieferende,
                "detail": resp_body,
            })),
        )
            .into_response(),
        Ok((409, body)) => (StatusCode::CONFLICT, Json(body)).into_response(),
        Ok((status, body)) => (
            axum::http::StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(body),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// Request body for `PUT /api/v1/portal/{malo_id}/kontakt`.
#[derive(Debug, serde::Deserialize)]
pub struct PortalKontaktRequest {
    /// Updated `Geschaeftspartner` BO4E COM JSON (name, address, contact).
    /// Partial update — fields absent in the request are preserved.
    pub geschaeftspartner: Option<serde_json::Value>,
    /// Updated SEPA consent flag.
    pub sepa_erlaubt: Option<bool>,
}

/// `PUT /api/v1/portal/{malo_id}/kontakt`
///
/// Update customer contact details (GDPR Art. 16 right to rectification).
///
/// Accepts `geschaeftspartner` (BO4E Geschaeftspartner COM — name, address,
/// email) and `sepa_erlaubt`.  Proxies to `PUT /api/v1/kunden/{id}` on
/// `vertragd` after resolving the kunden_id from the authenticated sub.
pub async fn put_portal_kontakt(
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    headers: axum::http::HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalKontaktRequest>,
) -> impl IntoResponse {
    let auth_ctx = match authenticate_and_resolve(&cfg, &headers, &malo_id).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    let vertragd = match &clients.vertragd {
        Some(c) => c.as_ref(),
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "vertragd not configured").into_response();
        }
    };

    let body = serde_json::json!({
        "geschaeftspartner": req.geschaeftspartner,
        "sepa_erlaubt": req.sepa_erlaubt,
    });

    match vertragd
        .put_json(&format!("/api/v1/kunden/{}", auth_ctx.kunden_id), &body)
        .await
    {
        Ok((200..=299, _)) => StatusCode::NO_CONTENT.into_response(),
        Ok((status, body)) => (
            axum::http::StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(body),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/portal/{malo_id}/invoices/{record_id}/download`
///
/// Download a billing document as ZUGFeRD 2.3 / XRechnung 3.0 CII XML.
///
/// Authenticates the customer and verifies they own `malo_id` before
/// proxying to `billingd GET /api/v1/billing/{record_id}/xrechnung`.
///
/// The `Content-Type: application/xml` response can be opened directly in
/// ERP systems that support EN16931 e-invoicing (SAP, DATEV, etc.).
pub async fn get_portal_invoice_download(
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    headers: axum::http::HeaderMap,
    Path((malo_id, record_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(resp) = authenticate_and_resolve(&cfg, &headers, &malo_id).await {
        return resp;
    }
    let billingd = match &clients.billingd {
        Some(c) => c.as_ref(),
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "billingd not configured").into_response();
        }
    };
    // Proxy XRechnung XML from billingd.
    let url = format!(
        "{}/api/v1/billing/{}/xrechnung",
        billingd.base_url(),
        &record_id
    );
    let mut req = billingd.client().get(&url);
    if let Some(key) = billingd.api_key() {
        req = req.bearer_auth(key);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let xml = resp.text().await.unwrap_or_default();
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, "application/xml"),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"rechnung-{record_id}.xml\""),
                    ),
                ],
                xml,
            )
                .into_response()
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            StatusCode::NOT_FOUND.into_response()
        }
        Ok(resp) => (
            axum::http::StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY),
            resp.text().await.unwrap_or_default(),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}
