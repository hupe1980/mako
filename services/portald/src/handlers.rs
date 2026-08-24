//! HTTP handlers for `portald` — customer portal read-model gateway.
//!
//! # Shape of every handler
//!
//! ```text
//! (cfg, clients, headers, Path(malo_id)) -> authorize(..)? -> proxy upstream
//! ```
//!
//! [`crate::auth::authorize`] is the only way to obtain a
//! [`PortalAuthCtx`], and every customer-scoped handler takes one. A handler
//! that skips the check has no context to carry, which is what keeps the gate
//! from being forgotten on the next route.
//!
//! # Ownership is checked on the object, not only the path
//!
//! Authorising `malo_id` is not enough for routes that also take an object id.
//! `GET …/invoices/{record_id}/download` re-reads the billing record and
//! compares its `malo_id` to the authorised one before rendering: otherwise any
//! authenticated customer could stream any other customer's XRechnung by id.

#![allow(clippy::result_large_err)] // the error *is* the HTTP response

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    auth::{PortalAuthCtx, authorize},
    clients::{PortalClients, UpstreamClient},
    config::PortaldConfig,
};

// ── Shared plumbing ───────────────────────────────────────────────────────────

/// Config + clients, as every handler receives them.
type Cfg = Extension<Arc<PortaldConfig>>;
type Clients = Extension<Arc<PortalClients>>;

/// Proxy a GET to an upstream, mapping absence to 404 and outage to 502.
async fn proxy(client: &UpstreamClient, path: &str) -> Response {
    match client.get_json(path).await {
        Ok(Some(body)) => (StatusCode::OK, Json(body)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::warn!(path, error = %e, "portald: upstream GET failed");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

/// [`proxy`], for an upstream that may not be configured at all.
async fn proxy_opt(client: Option<&Arc<UpstreamClient>>, path: &str, service: &str) -> Response {
    match client {
        Some(c) => proxy(c, path).await,
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{service} not configured"),
        )
            .into_response(),
    }
}

/// Require an upstream, or produce the 503 to return.
fn require<'a>(
    client: Option<&'a Arc<UpstreamClient>>,
    service: &str,
) -> Result<&'a UpstreamClient, Response> {
    client.map(AsRef::as_ref).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{service} not configured"),
        )
            .into_response()
    })
}

/// Relay an upstream write verdict unchanged.
///
/// A 422 from `vertragd` carries the rule it applied (a notice period, a
/// contract state); rewriting it into a generic error would strip exactly the
/// part the customer needs. Only the success code is this service's own — it
/// reports `202` because the market message the write triggers is asynchronous.
fn relay_write(status: u16, body: serde_json::Value, on_success: serde_json::Value) -> Response {
    if (200..300).contains(&status) {
        return (StatusCode::ACCEPTED, Json(on_success)).into_response();
    }
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
        Json(body),
    )
        .into_response()
}

// ── Dashboard ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/dashboard`
///
/// Supply status, account balance, latest invoice, current billing period and
/// advance-payment schedule in one call, fetched concurrently.
///
/// A field is `null` when its upstream is not configured or has no data — one
/// unreachable backend degrades a tile rather than the whole screen.
pub async fn get_dashboard(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    async fn opt(client: Option<&Arc<UpstreamClient>>, path: String) -> Option<serde_json::Value> {
        client?.get_json(&path).await.ok().flatten()
    }

    let (versorgung, balance, last_invoice, meter_summary, vorauszahlung) = tokio::join!(
        opt(
            clients.marktd.as_ref(),
            format!("/api/v1/versorgung/{malo_id}")
        ),
        opt(
            clients.accountingd.as_ref(),
            format!("/api/v1/accounts/{malo_id}/balance")
        ),
        opt(
            clients.billingd.as_ref(),
            format!("/api/v1/billing?malo_id={malo_id}&limit=1")
        ),
        opt(
            clients.edmd.as_ref(),
            format!("/api/v1/billing-period/{malo_id}")
        ),
        opt(
            clients.accountingd.as_ref(),
            format!("/api/v1/accounts/{malo_id}/vorauszahlung")
        ),
    );

    Json(serde_json::json!({
        "malo_id":       malo_id,
        "tenant":        cfg.tenant,
        "kundentyp":     ctx.kundentyp,
        "versorgung":    versorgung,
        "balance":       balance,
        "last_invoice":  last_invoice,
        "meter_summary": meter_summary,
        "vorauszahlung": vorauszahlung,
    }))
    .into_response()
}

// ── Consumption ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LastgangQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// `GET /api/v1/portal/{malo_id}/lastgang?from=&to=`
///
/// Proxies `edmd GET /api/v1/lastgang/{malo_id}`. Both bounds are passed only
/// when both are present — `edmd` reads them as a pair.
pub async fn get_lastgang(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Query(q): Query<LastgangQuery>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    let path = match (q.from.as_deref(), q.to.as_deref()) {
        (Some(from), Some(to)) => format!("/api/v1/lastgang/{malo_id}?from={from}&to={to}"),
        _ => format!("/api/v1/lastgang/{malo_id}"),
    };
    proxy_opt(clients.edmd.as_ref(), &path, "edmd").await
}

// ── Invoices ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct InvoicesQuery {
    pub limit: Option<u32>,
    pub outcome: Option<String>,
}

/// `GET /api/v1/portal/{malo_id}/invoices?limit=&outcome=`
///
/// Proxies `billingd GET /api/v1/billing?malo_id=…`. The page size is clamped
/// here: an unbounded `limit` reaches `billingd` as a full-table read on a
/// customer-facing route.
pub async fn get_invoices(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Query(q): Query<InvoicesQuery>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    let limit = q.limit.unwrap_or(24).clamp(1, MAX_INVOICE_PAGE);
    let mut path = format!("/api/v1/billing?malo_id={malo_id}&limit={limit}");
    if let Some(outcome) = q.outcome.as_deref() {
        path.push_str(&format!("&outcome={outcome}"));
    }
    proxy_opt(clients.billingd.as_ref(), &path, "billingd").await
}

/// Largest invoice page a portal caller may request.
pub const MAX_INVOICE_PAGE: u32 = 100;

/// `GET /api/v1/portal/{malo_id}/invoices/{record_id}/download`
///
/// Stream a billing document as XRechnung 3.0 CII XML (EN 16931).
///
/// The record is read back and its `malo_id` compared to the authorised one
/// before anything is rendered. Authorising only the path's `malo_id` would let
/// any authenticated customer download any invoice in the tenant by id — the id
/// is a UUID, but that is obscurity, not authorization.
pub async fn get_portal_invoice_download(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path((malo_id, record_id)): Path<(String, String)>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let billingd = match require(clients.billingd.as_ref(), "billingd") {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    match billingd
        .get_json(&format!("/api/v1/billing/{record_id}"))
        .await
    {
        Ok(Some(record)) => {
            let owner = record["malo_id"].as_str().unwrap_or_default();
            if owner != ctx.malo_id {
                tracing::warn!(
                    record_id,
                    requested_malo = %ctx.malo_id,
                    "portald: invoice download refused — record belongs to another Marktlokation"
                );
                // 404, not 403: confirming the record exists would turn the id
                // space into an oracle for which invoices the tenant holds.
                return StatusCode::NOT_FOUND.into_response();
            }
        }
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::warn!(record_id, error = %e, "portald: billingd record lookup failed");
            return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
        }
    }

    match billingd
        .get_text(&format!("/api/v1/billing/{record_id}/xrechnung"))
        .await
    {
        Ok(Some(xml)) => (
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/xml".to_owned(),
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"rechnung-{record_id}.xml\""),
                ),
            ],
            xml,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

// ── Account ledger ────────────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/balance` — open-items balance.
pub async fn get_balance(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    proxy_opt(
        clients.accountingd.as_ref(),
        &format!("/api/v1/accounts/{malo_id}/balance"),
        "accountingd",
    )
    .await
}

/// `GET /api/v1/portal/{malo_id}/kontoauszug` — full account statement (§ 666 BGB).
pub async fn get_kontoauszug(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    proxy_opt(
        clients.accountingd.as_ref(),
        &format!("/api/v1/accounts/{malo_id}/kontoauszug"),
        "accountingd",
    )
    .await
}

/// `GET /api/v1/portal/{malo_id}/vorauszahlung` — Abschlag schedule (§ 40 Abs. 1 EnWG).
pub async fn get_portal_vorauszahlung(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    proxy_opt(
        clients.accountingd.as_ref(),
        &format!("/api/v1/accounts/{malo_id}/vorauszahlung"),
        "accountingd",
    )
    .await
}

// ── EEG + supply status ───────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/eeg` — EEG/KWKG plants and settlements.
pub async fn get_eeg_status(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    proxy_opt(
        clients.einsd.as_ref(),
        &format!("/api/v1/anlagen?malo_id={malo_id}"),
        "einsd",
    )
    .await
}

/// `GET /api/v1/portal/{malo_id}/versorgung` — supply status.
pub async fn get_versorgung(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    proxy_opt(
        clients.marktd.as_ref(),
        &format!("/api/v1/versorgung/{malo_id}"),
        "marktd",
    )
    .await
}

// ── Contract lookup ───────────────────────────────────────────────────────────

/// Resolve `(vertrag_id, komponente_id)` for the authorised customer's MaLo.
///
/// `vertragd` keys contracts by customer, and the portal speaks MaLo-IDs, so
/// this walks the customer's contracts to the component that carries the MaLo.
/// The detail fetches run concurrently — the sequential version issued one
/// round-trip per contract before it could answer, on the critical path of
/// every write route.
///
/// A component without a parseable `id` is skipped rather than aborting the
/// search — one malformed row must not make the lookup answer "no contract"
/// and reject a lawful Kündigung.
async fn resolve_vertrag_for_malo(
    vertragd: &UpstreamClient,
    ctx: &PortalAuthCtx,
) -> Option<(uuid::Uuid, uuid::Uuid)> {
    let list = vertragd
        .get_json(&format!("/api/v1/kunden/{}/vertraege", ctx.kunden_id))
        .await
        .ok()
        .flatten()?;
    let ids: Vec<uuid::Uuid> = list
        .as_array()?
        .iter()
        .filter_map(|v| v["id"].as_str()?.parse().ok())
        .collect();

    let details = futures::future::join_all(ids.iter().map(|vtid| async move {
        let detail = vertragd
            .get_json(&format!("/api/v1/vertraege/{vtid}"))
            .await
            .ok()
            .flatten();
        (*vtid, detail)
    }))
    .await;

    for (vtid, detail) in details {
        let Some(detail) = detail else { continue };
        let Some(komps) = detail["komponenten"].as_array() else {
            continue;
        };
        for komp in komps {
            if komp["malo_id"].as_str() == Some(ctx.malo_id.as_str())
                && let Some(komp_id) = komp["id"].as_str().and_then(|s| s.parse().ok())
            {
                return Some((vtid, komp_id));
            }
        }
    }
    None
}

/// Resolve the contract, or produce the 404/503 to return.
async fn require_vertrag<'a>(
    clients: &'a PortalClients,
    ctx: &PortalAuthCtx,
) -> Result<(&'a UpstreamClient, uuid::Uuid, uuid::Uuid), Response> {
    let vertragd = require(clients.vertragd.as_ref(), "vertragd")?;
    match resolve_vertrag_for_malo(vertragd, ctx).await {
        Some((vtid, komp_id)) => Ok((vertragd, vtid, komp_id)),
        None => Err((
            StatusCode::NOT_FOUND,
            "no active supply contract for this delivery point",
        )
            .into_response()),
    }
}

/// `GET /api/v1/portal/{malo_id}/vertrag`
///
/// The active supply contract: product and tariff, delivery dates, notice
/// periods, and the Vertragskomponenten with their OBIS codes. A portal calls
/// this before offering Tarifwechsel or Kündigung.
pub async fn get_portal_vertrag(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let (vertragd, vtid, _) = match require_vertrag(&clients, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    proxy(vertragd, &format!("/api/v1/vertraege/{vtid}")).await
}

// ── Self-service writes (§ 41 EnWG customer rights) ──────────────────────────

/// Request body for `POST /api/v1/portal/{malo_id}/tarifwechsel`.
#[derive(Debug, Deserialize)]
pub struct PortalTarifwechselRequest {
    /// New product code in `productd`.
    pub new_product_code: String,
    /// When the new tariff takes effect (`YYYY-MM-DD`).
    ///
    /// Whether that date is reachable is `vertragd`'s decision: it depends on
    /// the Vertragsart, the running Preisgarantie and the billing cycle.
    pub wirksamkeit: String,
    /// Optional customer reason, kept in the contract audit trail.
    pub grund: Option<String>,
}

/// `POST /api/v1/portal/{malo_id}/tarifwechsel`
///
/// Customer-initiated tariff switch.
///
/// **The notice period is not checked here.** § 41 EnWG states none for a
/// tariff switch, and the facts that decide the real one — Vertragsart,
/// running Preisgarantie, billing cycle — live in `vertragd`. A second, simpler
/// rule here could only disagree with the one that decides. Only the date
/// *format* is this service's business; `vertragd` answers 422 with the rule it
/// applied, relayed unchanged.
pub async fn post_portal_tarifwechsel(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalTarifwechselRequest>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if !is_iso_date(&req.wirksamkeit) {
        return (StatusCode::BAD_REQUEST, "wirksamkeit must be YYYY-MM-DD").into_response();
    }
    let (vertragd, vtid, komp_id) = match require_vertrag(&clients, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let body = serde_json::json!({
        "komp_id":          komp_id,
        "new_product_code": req.new_product_code,
        "wirksamkeit":      req.wirksamkeit,
        "grund":            req.grund,
    });
    match vertragd
        .post_json(&format!("/api/v1/vertraege/{vtid}/tarifwechsel"), &body)
        .await
    {
        Ok((status, resp_body)) => relay_write(
            status,
            resp_body.clone(),
            serde_json::json!({
                "message":          "Tarifwechsel registered",
                "wirksamkeit":      req.wirksamkeit,
                "new_product_code": req.new_product_code,
                "detail":           resp_body,
            }),
        ),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/portal/{malo_id}/kuendigen`.
#[derive(Debug, Deserialize)]
pub struct PortalKuendigungRequest {
    /// Last day of supply (`YYYY-MM-DD`).
    ///
    /// `GET /api/v1/portal/{malo_id}/kuendigungsfrist` answers which dates are
    /// reachable before this is sent.
    pub lieferende: String,
    /// Why. Decides the notice period: `ORDENTLICH` (default),
    /// `PREISANPASSUNG` (§ 41 Abs. 5 Satz 4 EnWG), `UMZUG` (§ 41b Abs. 5 EnWG)
    /// or `LIEFERANTENWECHSEL`.
    pub grund: Option<String>,
    /// Free-text note kept with the contract.
    pub bemerkung: Option<String>,
}

/// `GET /api/v1/portal/{malo_id}/kuendigungsfrist`
///
/// The earliest date this contract can end, per termination reason, with the
/// rule that produced it. A portal offering self-service termination has to
/// show the customer the date *before* they pick one, and the statutes behind
/// it (§ 20 Abs. 1 StromGVV/GasGVV, § 41b Abs. 5 EnWG, § 41 Abs. 5 Satz 4 EnWG,
/// § 309 Nr. 9 lit. c BGB) live in `vertragd`, not in a second implementation.
pub async fn get_portal_kuendigungsfrist(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let (vertragd, vtid, _) = match require_vertrag(&clients, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    proxy(
        vertragd,
        &format!("/api/v1/vertraege/{vtid}/kuendigungsfrist"),
    )
    .await
}

/// `POST /api/v1/portal/{malo_id}/kuendigen`
///
/// Customer-initiated termination. Format-checked here, decided by `vertragd`
/// — see [`get_portal_kuendigungsfrist`] for why the period is not duplicated.
pub async fn post_portal_kuendigen(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalKuendigungRequest>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if !is_iso_date(&req.lieferende) {
        return (StatusCode::BAD_REQUEST, "lieferende must be YYYY-MM-DD").into_response();
    }
    let (vertragd, vtid, _) = match require_vertrag(&clients, &ctx).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let body = serde_json::json!({
        "lieferende": req.lieferende,
        "grund":      req.grund.as_deref().unwrap_or("ORDENTLICH"),
        "bemerkung":  req.bemerkung.as_deref().unwrap_or("Kündigung über das Kundenportal"),
    });
    match vertragd
        .post_json(&format!("/api/v1/vertraege/{vtid}/kuendigen"), &body)
        .await
    {
        Ok((status, resp_body)) => relay_write(
            status,
            resp_body.clone(),
            serde_json::json!({
                "message":    "Kündigung registered — UTILMD Lieferendemeldung will be dispatched",
                "lieferende": req.lieferende,
                "detail":     resp_body,
            }),
        ),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// Request body for `PUT /api/v1/portal/{malo_id}/kontakt`.
#[derive(Debug, Deserialize)]
pub struct PortalKontaktRequest {
    /// Updated BO4E `Geschaeftspartner` (name, address, contact). Partial —
    /// fields absent in the request are preserved by `vertragd`.
    pub geschaeftspartner: Option<serde_json::Value>,
    /// Updated SEPA consent flag.
    pub sepa_erlaubt: Option<bool>,
}

/// `PUT /api/v1/portal/{malo_id}/kontakt`
///
/// Update contact details (GDPR Art. 16 right to rectification). Proxies to
/// `PUT /api/v1/kunden/{kunden_id}` on `vertragd` — the customer id comes from
/// the authorization result, never from the request body.
pub async fn put_portal_kontakt(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalKontaktRequest>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if req.geschaeftspartner.is_none() && req.sepa_erlaubt.is_none() {
        return (StatusCode::BAD_REQUEST, "nothing to update").into_response();
    }
    let vertragd = match require(clients.vertragd.as_ref(), "vertragd") {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let body = serde_json::json!({
        "geschaeftspartner": req.geschaeftspartner,
        "sepa_erlaubt":      req.sepa_erlaubt,
    });
    match vertragd
        .put_json(&format!("/api/v1/kunden/{}", ctx.kunden_id), &body)
        .await
    {
        Ok((200..=299, _)) => StatusCode::NO_CONTENT.into_response(),
        Ok((status, body)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(body),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

// ── SEPA mandate self-service ─────────────────────────────────────────────────

/// Request body for `PUT /api/v1/portal/{malo_id}/sepa`.
#[derive(Debug, Deserialize)]
pub struct PortalSepaRequest {
    /// IBAN in any whitespace-separated form — `accountingd` validates mod-97.
    pub iban: String,
    /// BIC/SWIFT — optional within the SEPA zone.
    pub bic: Option<String>,
    /// Account holder. Defaults to the customer name held by `accountingd`.
    pub kontoinhaber: Option<String>,
    /// The debtor's own postal address (`Dbtr/PstlAdr`), forwarded verbatim to
    /// `accountingd`, which validates it.
    ///
    /// Optional until **15 November 2026**, when version 1.1 of the 2025 SEPA
    /// rulebooks ends the unstructured address and the schemes begin requiring
    /// `town` + `country` on every collection. Shape: `{ "town", "country",
    /// "street", "building_number", "post_code", "country_subdivision" }`.
    pub debtor_address: Option<serde_json::Value>,
}

/// `PUT /api/v1/portal/{malo_id}/sepa`
///
/// Register a SEPA direct-debit mandate for the customer's account.
///
/// Two things the caller does not supply:
///
/// - **`sequence_type`** — the scheme requires a `FRST` collection before any
///   `RCUR`, and the sequence is `accountingd`'s to track across the mandate's
///   life rather than a portal input.
/// - **`mandatsref`** — derived here with a random suffix, so a customer
///   correcting a mistyped IBAN the same day gets a new mandate rather than
///   reusing the reference of the one being replaced.
pub async fn put_portal_sepa(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Json(req): Json<PortalSepaRequest>,
) -> Response {
    if let Err(resp) = authorize(&cfg, &clients, &headers, &malo_id).await {
        return resp;
    }
    let accountingd = match require(clients.accountingd.as_ref(), "accountingd") {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let body = serde_json::json!({
        "malo_id":        &malo_id,
        "lf_mp_id":       cfg.lf_mp_id(),
        "iban":           &req.iban,
        "bic":            req.bic,
        "kontoinhaber":   req.kontoinhaber,
        "mandatsref":     mandatsref(&malo_id),
        "signed_at":      time::OffsetDateTime::now_utc().date().to_string(),
        "debtor_address": req.debtor_address,
    });

    match accountingd.post_json("/api/v1/sepa/mandates", &body).await {
        Ok((200..=299, body)) => (StatusCode::CREATED, Json(body)).into_response(),
        Ok((status, body)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(body),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// A mandate reference unique to one mandate.
///
/// Kept under the SEPA scheme's 35-character limit for `MndtId`: the MaLo is 11
/// digits, so `PORTAL-` + 11 + `-` + 16 hex = 35 exactly.
fn mandatsref(malo_id: &str) -> String {
    let unique = uuid::Uuid::new_v4().simple().to_string();
    format!("PORTAL-{malo_id}-{}", &unique[..16])
}

/// `YYYY-MM-DD`, and a date that exists.
fn is_iso_date(s: &str) -> bool {
    time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SEPA scheme caps `MndtId` at 35 characters, and a reference that is
    /// silently truncated by the bank stops matching the collections booked
    /// against it.
    #[test]
    fn a_mandate_reference_fits_the_sepa_field() {
        let r = mandatsref("51238696012");
        assert_eq!(r.len(), 35, "MndtId is capped at 35 characters: {r}");
        assert!(r.starts_with("PORTAL-51238696012-"));
    }

    /// Two mandates for the same MaLo on the same day are different mandates —
    /// a customer correcting a mistyped IBAN must not reuse the reference of
    /// the one being replaced.
    #[test]
    fn mandate_references_are_unique_per_mandate() {
        assert_ne!(mandatsref("51238696012"), mandatsref("51238696012"));
    }

    /// A present-but-malformed date is a 400. Both write routes accept the date
    /// the customer typed and pass it to `vertragd`, so the format check is the
    /// only thing standing between a typo and a rejected market message.
    #[test]
    fn date_validation_accepts_iso_and_rejects_the_rest() {
        assert!(is_iso_date("2026-06-01"));
        assert!(!is_iso_date("01.06.2026"), "German format is not ISO 8601");
        assert!(!is_iso_date("2026-6-1"), "unpadded components are not ISO");
        assert!(!is_iso_date("2026-02-30"), "a date that does not exist");
        assert!(!is_iso_date(""));
    }

    /// An unbounded `limit` on a customer-facing route reaches `billingd` as a
    /// full-table read.
    #[test]
    fn the_invoice_page_size_is_clamped_at_both_ends() {
        let clamp = |l: Option<u32>| l.unwrap_or(24).clamp(1, MAX_INVOICE_PAGE);
        assert_eq!(clamp(None), 24);
        assert_eq!(clamp(Some(0)), 1, "zero would return nothing");
        assert_eq!(clamp(Some(10_000)), MAX_INVOICE_PAGE);
    }
}

// ── Document inbox ────────────────────────────────────────────────────────────

/// `GET /api/v1/portal/{malo_id}/dokumente`
///
/// The customer's document inbox: what was actually issued to them — invoices,
/// Mahnungen, price-change notices — as `outputd` recorded it.
///
/// **Not the same list as `/invoices`.** That one is billing *records* —
/// what was calculated, drafts included. This is what the customer received.
///
/// Scoped by the authorised MaLo; `outputd` refuses an unscoped document query
/// outright, so the scope is enforced on both sides of the hop.
pub async fn get_dokumente(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Query(q): Query<DokumenteQuery>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let mut path = format!("/api/v1/documents?malo_id={}&limit={limit}", ctx.malo_id);
    if let Some(kind) = q.kind.as_deref().filter(|k| {
        // An allowlist, not a passthrough — the value reaches outputd.
        matches!(*k, "INVOICE" | "MAHNUNG" | "PREISANPASSUNG")
    }) {
        path.push_str(&format!("&kind={kind}"));
    }
    proxy_opt(clients.outputd.as_ref(), &path, "outputd").await
}

/// Filters for the document inbox.
#[derive(Debug, serde::Deserialize)]
pub struct DokumenteQuery {
    /// `INVOICE`, `MAHNUNG` or `PREISANPASSUNG`. Anything else is ignored.
    pub kind: Option<String>,
    pub limit: Option<u32>,
}

/// `GET /api/v1/portal/{malo_id}/dokumente/{document_id}`
///
/// The document itself — the bytes that were issued, not a re-render.
///
/// The document is read back and its `malo_id` compared to the authorised one
/// before anything is streamed, as the invoice download does: a UUID is
/// obscurity, not authorization.
///
/// Fetching it also marks the portal delivery **read** — the evidence a
/// § 41f EnWG dispute asks about.
pub async fn get_dokument(
    Extension(cfg): Cfg,
    Extension(clients): Clients,
    headers: HeaderMap,
    Path((malo_id, document_id)): Path<(String, String)>,
) -> Response {
    let ctx = match authorize(&cfg, &clients, &headers, &malo_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let outputd = match require(clients.outputd.as_ref(), "outputd") {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let document = match outputd
        .get_json(&format!("/api/v1/documents/{document_id}"))
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::warn!(document_id, error = %e, "portald: outputd document lookup failed");
            return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
        }
    };
    if document["malo_id"].as_str().unwrap_or_default() != ctx.malo_id {
        tracing::warn!(
            document_id,
            requested_malo = %ctx.malo_id,
            "portald: document refused — it belongs to another Marktlokation"
        );
        // 404, not 403: confirming the document exists would turn the id space
        // into an oracle for what the tenant has issued.
        return StatusCode::NOT_FOUND.into_response();
    }

    // Best-effort, before the bytes: a failed receipt must not withhold the
    // customer's own document.
    if let Some(delivery_id) = document["deliveries"]
        .as_array()
        .and_then(|ds| ds.iter().find(|d| d["channel"] == "PORTAL"))
        .and_then(|d| d["delivery_id"].as_str())
        && let Err(e) = outputd
            .post_json(
                &format!("/api/v1/deliveries/{delivery_id}/read"),
                &serde_json::Value::Null,
            )
            .await
    {
        tracing::warn!(document_id, error = %e, "portald: could not record the portal read receipt");
    }

    match outputd
        .get_bytes(&format!("/api/v1/documents/{document_id}/content"))
        .await
    {
        Ok(Some((bytes, media_type))) => {
            let kind = document["kind"].as_str().unwrap_or("dokument");
            let name: String = document["subject_ref"]
                .as_str()
                .unwrap_or(&document_id)
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, media_type),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        format!(
                            "inline; filename=\"{}-{name}.pdf\"",
                            kind.to_ascii_lowercase()
                        ),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}
