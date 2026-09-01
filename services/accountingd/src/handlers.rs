//! HTTP handlers for `accountingd`.
//!
//! Axum extractors are function parameters, so a handler that needs the pool,
//! the config, the ledger, the Cedar enforcer, a path, a query and a body is
//! already at seven before it has a line of body. The lint measures coupling,
//! which is the framework's here rather than this module's.
#![allow(clippy::too_many_arguments)]
//!
//! ## Security model
//!
//! - **Inbound webhook** (`POST /webhook`): HMAC-SHA256 verified when `erp_hmac_secret`
//!   is set. Uses `mako_service::webhook::hmac_hex` with `sha256=` prefix.
//!   Dev mode (no secret): accepts all but emits `WARN`.
//! - **REST endpoints**: OIDC JWT via the `Claims` extractor, then a Cedar
//!   check. Dev mode: synthetic claims.
//! - **MCP tools**: protected by `McpAuth` (API-key bearer or OIDC).

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use serde::Deserialize;
use sqlx::{PgPool, Row as _};
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    config::AccountingdConfig,
    pg::{
        CreateMandateRequest, UpdateAccountRequest, create_dunning_case_announced, create_mandate,
        fetch_account, fetch_account_by_id, fetch_mandate, fetch_vorauszahlung,
        jahresabschluss_already_settled, list_active_mandates, list_ledger, list_open_dunning,
        list_overdue_accounts, record_jahresabschluss, resolve_dunning_case,
        update_account_tenanted, upsert_account, upsert_vorauszahlung,
    },
    sepa::build_pain_008,
};
// Re-export sepa crate's validate_iban so test code can import from this module.
pub use sepa::validate_iban;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The 403 body every Cedar denial returns.
///
/// A denial names the principal, the action and the tenant it was refused
/// against — enough for an operator to see whether the caller's `mako_roles` are
/// wrong or the policy is, without leaking anything about the resource.
fn forbidden(e: &mako_service::cedar::CedarError) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
        .into_response()
}

/// The 500 body. The detail is returned because every caller of this service is
/// an operator or an internal system, never an end customer.
fn internal(e: &anyhow::Error) -> axum::response::Response {
    tracing::error!(error = %e, "accountingd: request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
        .into_response()
}

/// Convert an amount in ct (i64, × 10⁻² EUR) to a `"1234.56"` EUR string.
/// Uses pure integer arithmetic — no f64.
pub fn format_ct_as_eur(ct: i64) -> String {
    let sign = if ct < 0 { "-" } else { "" };
    let abs = ct.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

// ── Account endpoints ─────────────────────────────────────────────────────────

/// `GET /api/v1/accounts/{malo_id}`
pub async fn get_account(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/accounts/{malo_id}`  — upsert account + update fields (IBAN, Abschlag)
pub async fn put_account(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(iban_key): Extension<Option<[u8; 32]>>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
    Json(req): Json<crate::pg::UpdateAccountRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    let _ = upsert_account(&pool, &malo_id, &lf_mp_id, &cfg.tenant).await;
    match update_account_tenanted(
        &pool,
        &malo_id,
        &lf_mp_id,
        &cfg.tenant,
        iban_key.as_ref(),
        req,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/balance`  — current balance in ct (negative = credit)
pub async fn get_balance(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(row)) => Json(serde_json::json!({
            "malo_id": malo_id,
            "balance_ct": row.balance_ct,
            "balance_eur": format_ct_as_eur(row.balance_ct),
            "status": if row.balance_ct > 0 { "overdue" } else if row.balance_ct < 0 { "credit" } else { "settled" },
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/ledger`  — paged ledger entries
pub async fn get_ledger(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<LedgerQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    if let Ok(None) = fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    match list_ledger(
        &ledger,
        lf_mp_id,
        &malo_id,
        doubleentry::BalanceQuery::all(),
        q.limit.unwrap_or(100).min(1000),
    )
    .await
    {
        Ok(window) => Json(window).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/kontoauszug`  — account statement (portald-consumable)
pub async fn get_kontoauszug(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<KontoauszugQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    // `from`/`to` narrow the statement to a period. Both are needed for a
    // period: a bare `to` is the cumulative position through that date, which is
    // a balance rather than a Kontoauszug, and a bare `from` has no closing
    // date to open the *next* one at.
    let unprocessable = |msg: String| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response()
    };
    let (from, to) = match (
        parse_iso_date("from", q.from.as_deref()),
        parse_iso_date("to", q.to.as_deref()),
    ) {
        (Ok(from), Ok(to)) => (from, to),
        (Err(e), _) | (_, Err(e)) => return unprocessable(e),
    };
    let period = match (from, to) {
        (None, None) => None,
        (Some(from), Some(to)) if from > to => {
            return unprocessable("from must not be after to".to_owned());
        }
        (Some(from), Some(to)) => Some((from, to)),
        _ => return unprocessable("from and to must be given together".to_owned()),
    };
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let query = period.map_or_else(doubleentry::BalanceQuery::all, |(from, to)| {
        doubleentry::BalanceQuery::between(from, to)
    });
    let window = match list_ledger(&ledger, lf_mp_id, &malo_id, query, 500).await {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    // Eröffnungs- und Schlusssaldo: the two figures that make the page add up.
    // The closing figure is the opening plus everything on it, not the account's
    // current balance — a statement for last March must not close at today's.
    let bewegung_ct: i64 = window.lines.iter().map(|l| l.signed_ct).sum();
    Json(serde_json::json!({
        "malo_id": malo_id,
        "lf_mp_id": lf_mp_id,
        "from": period.map(|(from, _)| from.to_string()),
        "to": period.map(|(_, to)| to.to_string()),
        "eroeffnungssaldo_ct": window.opening_ct,
        "bewegung_ct": bewegung_ct,
        "schlusssaldo_ct": window.opening_ct + bewegung_ct,
        "balance_ct": account.balance_ct,
        "abschlag_ct": account.abschlag_ct,
        "generated_at": OffsetDateTime::now_utc().to_string(),
        "entries": window.lines,
    }))
    .into_response()
}

/// Parse an optional ISO 8601 date, naming the field on failure.
fn parse_iso_date(field: &str, raw: Option<&str>) -> Result<Option<time::Date>, String> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    time::Date::parse(raw, &time::format_description::well_known::Iso8601::DATE)
        .map(Some)
        .map_err(|_| format!("{field} must be an ISO 8601 date (YYYY-MM-DD)"))
}

#[derive(Debug, Deserialize)]
pub struct AbschlaegeQuery {
    pub lf_mp_id: Option<String>,
    /// Inclusive lower bound on the advance's `periode` (ISO 8601 date).
    pub from: Option<String>,
    /// Inclusive upper bound on the advance's `periode` (ISO 8601 date).
    pub to: Option<String>,
    /// Include advances that were demanded but not received, and ones a
    /// previous settling invoice already absorbed. Default `false`.
    ///
    /// Off by default because the caller is a settling invoice, and
    /// § 14 Abs. 5 Satz 2 UStG lets one deduct „die **vereinnahmten**
    /// Teilentgelte" — the advances actually received. Deducting a demanded but
    /// unpaid advance would hand the customer a settlement that credits money
    /// nobody paid.
    pub include_open: Option<bool>,
}

/// `GET /api/v1/accounts/{malo_id}/abschlaege`
///
/// The advances of one Marktlokation in a period, in exactly the shape a
/// settling invoice deducts them: date, gross amount, and **the VAT rate the
/// advance was raised at** (§ 14 Abs. 5 Satz 2 UStG — an Endrechnung deducts
/// the part-payments *and the tax attributable to them*, and a gross figure
/// alone cannot express that when a rate changed mid-year).
///
/// This is the read `billingd`'s § 40b sweep needs to issue a lawful
/// Jahresrechnung unattended. Without it the sweep refused every annual
/// settlement, because it could not itemise or deduct the advances § 40 Abs. 1
/// EnWG requires it to show — the correct refusal, but it left automated annual
/// billing switched off.
///
/// **On the service graph.** `accountingd` sits downstream of `billingd` for
/// *events* (`de.billing.rechnung.erstellt` → the receivable). This is a read
/// in the other direction, at billing time, and it does not invert anything:
/// the advance register is customer-account state that only the ledger holds,
/// exactly as SAP IS-U billing reads FI-CA advances. What would invert the
/// graph is `accountingd` computing an invoice, and it does not.
///
/// Response: `[{ "datum", "betrag_eur", "ust_satz", "beschreibung" }]` —
/// `energy_billing::AbschlagDeduction` verbatim, oldest first, plus
/// `reference`, `offen_ct` and `verrechnet_mit` for reconciliation.
pub async fn get_abschlaege(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AbschlaegeQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let parse = |label: &str, raw: Option<&str>, fallback: time::Date| match raw {
        None => Ok(fallback),
        Some(s) => time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
            .map_err(|e| format!("{label}: {e} (expected ISO 8601, e.g. 2026-01-01)")),
    };
    let epoch = time::Date::from_calendar_date(1970, time::Month::January, 1)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH.date());
    let from = match parse("from", q.from.as_deref(), epoch) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let to = match parse("to", q.to.as_deref(), mako_fristen::heute()) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    if from > to {
        return (StatusCode::BAD_REQUEST, "from must not be after to").into_response();
    }
    if let Ok(None) = fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    let all = match crate::pg::list_abschlag_forderungen(
        &ledger,
        &pool,
        &cfg.tenant,
        &malo_id,
        lf_mp_id,
        from,
        to,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let include_open = q.include_open.unwrap_or(false);
    let out: Vec<serde_json::Value> = all
        .into_iter()
        .filter(|a| include_open || a.deductible())
        .map(|a| {
            serde_json::json!({
                // `AbschlagDeduction` field names, so the caller deserialises
                // straight into the engine's type.
                "datum":          a.faellig_am.to_string(),
                "betrag_eur":     format_ct_as_eur(a.betrag_ct),
                "ust_satz":       a.ust_satz.to_string(),
                "beschreibung":   format!(
                    "Abschlag {:04}-{:02}",
                    a.periode.year(),
                    a.periode.month() as u8
                ),
                // Reconciliation, ignored by the engine.
                "reference":      a.reference,
                "offen_ct":       a.offen_ct,
                "verrechnet_mit": a.verrechnet_mit,
            })
        })
        .collect();
    Json(out).into_response()
}

/// `PUT /api/v1/accounts/{malo_id}/abschlag`  — update monthly advance payment
pub async fn put_abschlag(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let abschlag_ct = body.get("abschlag_ct").and_then(|v| v.as_i64());
    if let Some(ct) = abschlag_ct {
        match update_account_tenanted(
            &pool,
            &malo_id,
            &cfg.tenant, // lf_mp_id defaults to tenant when not specified
            &cfg.tenant,
            None,
            crate::pg::UpdateAccountRequest {
                iban: None,
                mandatsref: None,
                abschlag_ct: Some(ct),
                billing_day: None,
                address: Default::default(),
            },
        )
        .await
        {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
        }
    } else {
        (StatusCode::BAD_REQUEST, "abschlag_ct required").into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct AccountQuery {
    pub lf_mp_id: Option<String>,
}

/// Query for `GET /api/v1/accounts/{malo_id}/kontoauszug`.
#[derive(Debug, Deserialize)]
pub struct KontoauszugQuery {
    pub lf_mp_id: Option<String>,
    /// Inclusive start of the statement period (ISO 8601 date).
    pub from: Option<String>,
    /// Inclusive end of the statement period (ISO 8601 date).
    pub to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LedgerQuery {
    pub lf_mp_id: Option<String>,
    pub limit: Option<i64>,
}

// ── CloudEvent ingest (webhook) ───────────────────────────────────────────────

/// `POST /webhook` — ingest CloudEvents from billingd, invoicd, einsd, netzbilanzd.
///
/// ## Security
///
/// When `erp_hmac_secret` is configured, the Standard Webhooks headers
/// is verified before any processing. Requests without a valid signature are rejected
/// with HTTP 403 to prevent fake invoice injection.
///
/// Supported event types:
/// - `de.billing.rechnung.erstellt` → debit entry
/// - `de.invoic.receipt.settled`    → credit entry (NNE receipt paid)
/// - `de.invoic.receipt.disputed`   → no entry (dispute logged)
/// - `de.eeg.verguetung.berechnet`  → credit entry (EEG settlement)
pub async fn ingest_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── Inbound Standard Webhooks verification ────────────────────────
    use secrecy::ExposeSecret;
    let secret = cfg
        .erp_hmac_secret
        .as_ref()
        .map(|s| s.expose_secret().as_bytes().to_vec());
    if secret.is_some() {
        // The shared verifier: constant-time signature compare *and* the
        // timestamp check, so a captured ERP POST cannot be replayed.
        if let Err(err) = mako_service::webhook::verify_request(secret.as_deref(), &headers, &body)
        {
            tracing::warn!(%err, "accountingd: inbound webhook refused");
            return StatusCode::from(err).into_response();
        }
    } else {
        tracing::warn!(
            "accountingd: erp_hmac_secret not set — accepting webhook without HMAC verification (dev mode)"
        );
    }

    // ── Parse CloudEvent from raw body ───────────────────────────────────────
    let ce: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "accountingd: malformed CloudEvent body");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let ce_type = ce.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // A CloudEvent id is mandatory (CE spec) and is the ledger idempotency key —
    // without it a redelivery could not be deduplicated, so reject it outright.
    let Some(ce_id) = ce.get("id").and_then(|v| v.as_str()).map(str::to_owned) else {
        tracing::warn!("accountingd: CloudEvent without an id — cannot deduplicate, rejected");
        return StatusCode::BAD_REQUEST.into_response();
    };
    let data = ce.get("data");
    let today = mako_fristen::heute();

    match ce_type {
        // ── Billing invoice (billingd) ────────────────────────────────────────
        // de.billing.rechnung.erstellt:
        //   is_correction=false → RECHNUNG debit  (customer owes money)
        //   is_correction=true  → STORNO debit/credit (negated amount; billing reversal)
        mako_events::billing::RECHNUNG_ERSTELLT => {
            let malo_id = data
                .and_then(|d| d.get("malo_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let lf_mp_id = data
                .and_then(|d| d.get("lf_mp_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&cfg.tenant);
            let is_correction: bool = data
                .and_then(|d| d.get("is_correction"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Parse as Decimal to avoid f64 rounding errors on money amounts.
            let betrag_ct = |field: &str| -> Option<i64> {
                use rust_decimal::Decimal;
                use std::str::FromStr;
                data.and_then(|d| d.get("rechnung"))
                    .and_then(|r| r.get(field))
                    .and_then(|g| g.get("wert"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .map(|d| {
                        (d * Decimal::from(100))
                            .round()
                            .to_string()
                            .parse::<i64>()
                            .unwrap_or(0)
                    })
            };
            // The receivable is the invoice's **gross**: that is what it charges
            // for the supply. Advances already demanded were debited when they
            // were raised, so booking the gross alone would have the customer
            // owe the period twice — the deduction the document itself states
            // (`gesamtbrutto − zuZahlen`, § 14 Abs. 5 Satz 2 UStG) is booked
            // separately as an `ABSCHLAG_VERRECHNUNG` credit below.
            let amount_ct: i64 = betrag_ct("gesamtbrutto").unwrap_or(0);
            let verrechnete_abschlaege_ct: i64 = betrag_ct("zuZahlen")
                .map_or(0, |zu_zahlen| amount_ct - zu_zahlen)
                .max(0);
            let rechnungsnummer = data
                .and_then(|d| d.get("rechnung"))
                .and_then(|r| r.get("rechnungsnummer"))
                .and_then(|v| v.as_str())
                .unwrap_or(&ce_id)
                .to_owned();
            let account_id = upsert_account(&pool, malo_id, lf_mp_id, &cfg.tenant)
                .await
                .ok();

            // Learn the commodity from the invoice. It drives the ISO 20022
            // `Purp/Cd` on the next direct debit (`ELEC` / `GASB` / `WTER`) —
            // what the debtor's statement and their accounting software read to
            // categorise the collection. Best-effort: a failure here must never
            // hold up the receivable.
            if let Some(account_id) = account_id
                && let Some(sparte) = data
                    .and_then(|d| d.get("rechnung"))
                    .and_then(|r| r.get("sparte"))
                    .and_then(|v| v.as_str())
                    .filter(|s| {
                        matches!(
                            *s,
                            "STROM" | "GAS" | "FERNWAERME" | "NAHWAERME" | "WASSER" | "ABWASSER"
                        )
                    })
                && let Err(e) =
                    crate::pg::set_account_sparte(&pool, account_id, &cfg.tenant, sparte).await
            {
                tracing::warn!(error = %e, malo_id, "accountingd: could not record account Sparte");
            }

            if account_id.is_some() && amount_ct != 0 {
                let record_id = data
                    .and_then(|d| d.get("record_id"))
                    .and_then(|v| v.as_str());
                // STORNO: billing reversal (Stornorechnung). Amount already negated by billingd.
                // RECHNUNG: normal invoice debit.
                let (entry_type, description) = if is_correction {
                    ("STORNO", "Stornorechnung / Korrekturrechnung")
                } else {
                    ("RECHNUNG", "Kundenrechnung")
                };
                if let Err(e) = crate::pg::post_entry(
                    &ledger,
                    &pool,
                    &cfg.tenant,
                    malo_id,
                    lf_mp_id,
                    entry_type,
                    amount_ct,
                    &ce_id,
                    Some(&ce_id),
                    record_id,
                    today,
                    today,
                    Some(description),
                    None,
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — returning 500 so the sender redelivers"
                    );
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                // The advances this invoice settled stop being advances. A
                // Storno reverses its original's gross, and the original's
                // Verrechnung is reversed by re-billing the released period —
                // so only an ordinary invoice books one.
                if !is_correction
                    && verrechnete_abschlaege_ct > 0
                    && let Err(e) = crate::pg::verrechne_abschlaege(
                        &ledger,
                        &pool,
                        &cfg.tenant,
                        malo_id,
                        lf_mp_id,
                        verrechnete_abschlaege_ct,
                        &rechnungsnummer,
                        &format!("{ce_id}:abschlag-verrechnung"),
                        today,
                    )
                    .await
                {
                    tracing::error!(
                        error = %e,
                        malo_id,
                        "accountingd: Abschlag-Verrechnung FAILED — returning 500 so the sender \
                         redelivers; without it the customer owes the period twice"
                    );
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
            StatusCode::OK.into_response()
        }

        // ── NNE / INVOIC receipt settled (invoicd) ────────────────────────────
        // de.invoic.receipt.settled: the LF confirmed an inbound NNE invoice from the NB.
        // For the customer ledger this is not directly relevant (it's an NB↔LF settlement),
        // but if the LF passes the NNE cost through to the customer (MSB pass-through billing),
        // a corresponding RECHNUNG should have been created by billingd already.
        // We log the settlement as a ZAHLUNG credit if `settlement_eur` is present,
        // meaning the NB confirmed receiving payment from the LF.
        mako_events::invoic::RECEIPT_SETTLED => {
            let malo_id = data
                .and_then(|d| d.get("malo_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let settlement_ct: i64 = data
                .and_then(|d| d.get("settlement_eur"))
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    use rust_decimal::Decimal;
                    use std::str::FromStr;
                    // Positive settlement_eur = NB received payment → ZAHLUNG credit for customer.
                    Decimal::from_str(s).ok().map(|d| {
                        -(d * Decimal::from(100))
                            .round()
                            .to_string()
                            .parse::<i64>()
                            .unwrap_or(0)
                    })
                })
                .unwrap_or(0);
            if !malo_id.is_empty()
                && settlement_ct != 0
                && upsert_account(&pool, malo_id, &cfg.tenant, &cfg.tenant)
                    .await
                    .is_ok()
            {
                #[allow(clippy::collapsible_if)]
                if let Err(e) = crate::pg::post_entry(
                    &ledger,
                    &pool,
                    &cfg.tenant,
                    malo_id,
                    &cfg.tenant,
                    "ZAHLUNG",
                    settlement_ct,
                    &ce_id,
                    Some(&ce_id),
                    None,
                    today,
                    today,
                    Some("NNE-Zahlung bestätigt (INVOIC settled)"),
                    None,
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — returning 500 so the sender redelivers"
                    );
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
            StatusCode::OK.into_response()
        }

        // ── EEG Einspeisevergütung (einsd) ────────────────────────────────────
        // de.eeg.verguetung.berechnet: fixed-rate EEG settlement → EEG_GUTSCHRIFT credit.
        // When cfg.eeg.auto_payout = true: also auto-generates pain.001 SEPA Credit Transfer
        // (SCT Inst or SCT CORE per cfg.eeg.sepa_instant) for immediate payout to plant operator.
        mako_events::eeg::VERGUETUNG_BERECHNET => {
            let malo_id = ce.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let tr_id = data
                .and_then(|d| d.get("tr_id"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let billing_year: i16 = data
                .and_then(|d| d.get("billing_year"))
                .and_then(|v| v.as_i64())
                .map(|y| y as i16)
                .unwrap_or_else(|| today.year() as i16);
            let billing_month: i16 = data
                .and_then(|d| d.get("billing_month"))
                .and_then(|v| v.as_i64())
                .map(|m| m as i16)
                .unwrap_or_else(|| today.month() as i16);
            // Bank fields forwarded by einsd (added in NBA #8 hard cut)
            let bank_iban = data
                .and_then(|d| d.get("bank_iban"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let bank_bic = data
                .and_then(|d| d.get("bank_bic"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let zahlungsempfaenger = data
                .and_then(|d| d.get("zahlungsempfaenger"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            let amount_ct: i64 = data
                .and_then(|d| d.get("settlement_eur"))
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    use rust_decimal::Decimal;
                    use std::str::FromStr;
                    Decimal::from_str(s).ok().map(|d| {
                        -(d * Decimal::from(100))
                            .round()
                            .to_string()
                            .parse::<i64>()
                            .unwrap_or(0)
                    })
                })
                .unwrap_or(0);
            let account_id = upsert_account(&pool, malo_id, &cfg.tenant, &cfg.tenant)
                .await
                .unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() && amount_ct != 0 {
                if let Err(e) = crate::pg::post_entry(
                    &ledger,
                    &pool,
                    &cfg.tenant,
                    malo_id,
                    &cfg.tenant,
                    "EEG_GUTSCHRIFT",
                    amount_ct,
                    &ce_id,
                    Some(&ce_id),
                    tr_id.as_deref(),
                    today,
                    today,
                    Some("EEG Einspeisevergütung §21 EEG"),
                    None,
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — returning 500 so the sender redelivers"
                    );
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                // ── SCT Inst / SCT CORE auto-payout ─────────────────────────────
                // Created INLINE, not in a detached task: a spawn would ACK the CE
                // 200 and swallow any payout failure, leaving a booked credit with
                // no payout order and no redelivery. Inline, the order creation is
                // part of the request; both the credit and the order are idempotent
                // (by CE id / EndToEndId), so a failure is
                // logged and safely retried on redelivery or via POST /eeg/payouts/run.
                if cfg.eeg.auto_payout {
                    // Creditor IBAN: CE-supplied bank_iban first, else the account's
                    // stored zahlungsinformation.
                    let creditor = match bank_iban.clone() {
                        Some(iban) => Some((
                            iban,
                            zahlungsempfaenger
                                .clone()
                                .unwrap_or_else(|| "EEG Einspeiser".to_owned()),
                        )),
                        None => {
                            let zi: Option<serde_json::Value> = sqlx::query(
                                "SELECT zahlungsinformation FROM accounts \
                                 WHERE malo_id = $1 AND tenant = $2",
                            )
                            .bind(malo_id)
                            .bind(&cfg.tenant)
                            .fetch_optional(&pool)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|r| {
                                use sqlx::Row;
                                r.try_get("zahlungsinformation").unwrap_or(None)
                            });
                            zi.as_ref()
                                .and_then(|z| z.get("bankverbindung"))
                                .and_then(|b| b.get("iban"))
                                .and_then(|v| v.as_str())
                                .map(str::to_owned)
                                .map(|iban| {
                                    let name = zi
                                        .as_ref()
                                        .and_then(|z| z.get("kontoinhaber"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("EEG Einspeiser")
                                        .to_owned();
                                    (iban, name)
                                })
                        }
                    };
                    let _ = bank_bic; // carried in pain.001 only when present in the CE

                    if let Some((creditor_iban, creditor_name)) = creditor {
                        // `Cdtr/PstlAdr` — the plant operator's own address.
                        // BO4E's Zahlungsinformation COM carries no address, so
                        // it comes from the account's master data.
                        let creditor_address =
                            crate::pg::fetch_account_by_id(&pool, account_id, &cfg.tenant)
                                .await
                                .ok()
                                .flatten()
                                .map(|a| a.postal_address())
                                .unwrap_or_default();
                        create_eeg_payout_order(
                            &cfg,
                            &pool,
                            EegPayoutParams {
                                malo_id,
                                account_id,
                                amount_ct: amount_ct.unsigned_abs() as i64,
                                creditor_iban: &creditor_iban,
                                creditor_name: &creditor_name,
                                creditor_address,
                                tr_id: tr_id.as_deref(),
                                billing_year,
                                billing_month,
                                source_ce_id: Some(&ce_id),
                            },
                        )
                        .await;
                    } else {
                        tracing::info!(
                            malo_id,
                            "accountingd: auto_payout=true but no creditor IBAN available — \
                             set bank_iban in the EEG plant record or PUT zahlungsinformation"
                        );
                    }
                }
            }
            StatusCode::OK.into_response()
        }

        // ── EEG Direktvermarktung Marktprämie (einsd) ─────────────────────────
        // de.eeg.marktpraemie.berechnet: Direktvermarktung / Ausschreibung settlement.
        // Gleitende Marktprämie (§20 EEG) + Managementprämie → EEG_MARKTPRAEMIE credit.
        mako_events::eeg::MARKTPRAEMIE_BERECHNET => {
            let malo_id = ce.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let amount_ct: i64 = data
                .and_then(|d| d.get("settlement_eur"))
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    use rust_decimal::Decimal;
                    use std::str::FromStr;
                    Decimal::from_str(s).ok().map(|d| {
                        -(d * Decimal::from(100))
                            .round()
                            .to_string()
                            .parse::<i64>()
                            .unwrap_or(0)
                    })
                })
                .unwrap_or(0);
            if !malo_id.is_empty()
                && amount_ct != 0
                && upsert_account(&pool, malo_id, &cfg.tenant, &cfg.tenant)
                    .await
                    .is_ok()
            {
                #[allow(clippy::collapsible_if)]
                if let Err(e) = crate::pg::post_entry(
                    &ledger,
                    &pool,
                    &cfg.tenant,
                    malo_id,
                    &cfg.tenant,
                    "EEG_MARKTPRAEMIE",
                    amount_ct,
                    &ce_id,
                    Some(&ce_id),
                    None,
                    today,
                    today,
                    Some("EEG Direktvermarktung Marktprämie §20 EEG"),
                    None,
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — returning 500 so the sender redelivers"
                    );
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
            StatusCode::OK.into_response()
        }

        _ => {
            tracing::debug!(ce_type, "accountingd: unknown CloudEvent type — ignored");
            StatusCode::OK.into_response()
        }
    }
}

/// `POST /api/v1/payments/import`  — ingest a **flat bank export** (JSON array).
///
/// Each row: `{ "iban": "...", "amount_eur": "155.42", "reference": "...", "date": "YYYY-MM-DD",
///             "bank_transaction_id": "...", "end_to_end_id": "...", "return_reason_code": "..." }`
///
/// Parsed by [`crate::sepa::BankStatementEntry`] — amounts through
/// `sepa::ct_from_eur_str` (integer ct, **no f64**), dates through
/// `sepa::IsoDate`. The shape is accountingd's own import contract, not an ISO
/// 20022 message: prefer `POST /api/v1/payments/import/camt054` wherever the
/// bank offers real camt, because `EndToEndId`, the `Btch` block and return
/// reason codes do not survive a flattening.
///
/// A row gives money back when it carries a `return_reason_code` **or** a
/// negative `amount_eur` → `BANKRUECKLAST` debit; anything else is a `ZAHLUNG`
/// credit. The sign is flipped into the ledger's open-items convention by
/// `sepa::bank_to_ledger_ct`, the same single conversion the camt paths use.
///
/// ## Deduplication
///
/// Each row is checked against `bank_import_log` before processing.
/// If `bank_transaction_id` is present and already imported, the row is skipped
/// and counted as `deduplicated` — no duplicate ledger entries are created.
/// When `bank_transaction_id` is absent, a stable hash of (iban+amount+date+reference)
/// is used as the deduplication key.
pub async fn import_payments(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(iban_key): Extension<Option<[u8; 32]>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(entries): Json<Vec<serde_json::Value>>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "import-payments", &cfg.tenant) {
        return forbidden(&e);
    }
    let mut accepted = 0usize;
    let mut deduplicated = 0usize;
    let mut skipped = 0usize;

    for raw in &entries {
        // Every rejection names the field and the reason, so a skipped bank row
        // is diagnosable rather than silently dropped.
        let entry = match crate::sepa::BankStatementEntry::parse(raw) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(error = %e, "accountingd: bank export row rejected — skipping");
                skipped += 1;
                continue;
            }
        };
        let date = entry.date;

        // ── Deduplication ────────────────────────────────────────────────────
        // Derive a stable bank_transaction_id: use the one in the row if present,
        // otherwise compute a deterministic hash from the row's identifying fields.
        let bank_txn_id = entry.bank_transaction_id.clone().unwrap_or_else(|| {
            // Fallback: hash (iban + amount + date + reference) for stability
            let key = format!(
                "{}|{}|{}|{}",
                entry.iban.as_str(),
                entry.ledger_ct(),
                date,
                &entry.reference
            );
            // Simple deterministic key (not cryptographic — only for dedup)
            format!(
                "{:016x}",
                key.bytes().fold(0u64, |acc, b| {
                    acc.wrapping_mul(1099511628211).wrapping_add(u64::from(b))
                })
            )
        });

        // Check deduplication log
        match crate::pg::bank_import_already_processed(&pool, &cfg.tenant, &bank_txn_id).await {
            Ok(true) => {
                tracing::debug!(
                    bank_txn_id = %bank_txn_id,
                    "accountingd: bank export row already imported — skipping (dedup)"
                );
                deduplicated += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %e, "accountingd: dedup check failed — processing entry anyway");
            }
            Ok(false) => {}
        }

        // Resolve the customer. The counterparty IBAN (keyed BLAKE3 hash) is the
        // strongest evidence, but it is not the only one — see
        // `resolve_account_for_payment` for the ladder and why matching on the
        // IBAN alone loses every payment made from a second account.
        let iban_h = crate::ledger::iban_hash(iban_key.as_ref(), entry.iban.as_str());
        let matched = crate::pg::resolve_account_for_payment(
            &pool,
            &cfg.tenant,
            crate::pg::PaymentClues {
                iban_hash: Some(&iban_h),
                end_to_end_id: entry.end_to_end_id.as_deref(),
                remittance: Some(entry.reference.as_str()),
            },
        )
        .await;

        if let Ok(Some(matched)) = matched {
            let malo_id = matched.malo_id.clone();
            let lf_mp_id = matched.lf_mp_id.clone();
            {
                let is_return = entry.is_return();
                let entry_type = if is_return {
                    "BANKRUECKLAST"
                } else {
                    "ZAHLUNG"
                };
                // Idempotency keyed on the bank transaction id — a re-import of the
                // same row replays as a ledger no-op rather than double-booking.
                let ledger_result = crate::pg::post_entry(
                    &ledger,
                    &pool,
                    &cfg.tenant,
                    &malo_id,
                    &lf_mp_id,
                    entry_type,
                    entry.ledger_ct(),
                    &format!("bank:{bank_txn_id}"),
                    None,
                    Some(entry.reference.as_str()),
                    date,
                    date,
                    Some(entry.description().as_str()),
                    None,
                )
                .await;

                match ledger_result {
                    Ok(ledger_id) => {
                        // Secondary audit log (the ledger key is the real dedup now).
                        if let Err(e) = crate::pg::record_bank_import(
                            &pool,
                            &cfg.tenant,
                            &bank_txn_id,
                            entry.ledger_ct().abs(),
                            Some(entry.iban.as_str()),
                            date,
                            Some(ledger_id),
                            None,
                            entry.end_to_end_id.as_deref(),
                        )
                        .await
                        {
                            tracing::warn!(error = %e, "accountingd: bank_import_log insert failed");
                        }

                        // Announce the booking. `de.accounting.bankruecklast`
                        // drives agentd's payment-reconciliation agent, which
                        // never ran because nothing emitted it — a returned
                        // direct debit is precisely what it exists for.
                        let ce_type = if is_return {
                            mako_events::accounting::BANKRUECKLAST
                        } else {
                            mako_events::accounting::PAYMENT_IMPORTED
                        };
                        let amount_ct = entry.ledger_ct().abs();
                        let ce = mako_service::CloudEvent::new(
                            mako_service::source("accountingd", &cfg.tenant),
                            ce_type,
                            &malo_id,
                            serde_json::json!({
                                "malo_id":      malo_id,
                                "lf_mp_id":     lf_mp_id,
                                "amount_ct":    amount_ct,
                                "amount_eur":   format!("{:.2}", amount_ct as f64 / 100.0),
                                "is_return":    is_return,
                                "reference":    entry.reference.as_str(),
                                "bank_txn_id":  bank_txn_id,
                                "booking_date": date.to_string(),
                                "ledger_id":    ledger_id.to_string(),
                                // Which rung of the resolution ladder matched:
                                // "iban" is the bank's own assertion, the others
                                // are inferences a reconciliation agent may want
                                // to review.
                                "matched_by":   matched.matched_by,
                            }),
                        );
                        if let Err(e) = enqueue_ce(&pool, &ce).await {
                            tracing::warn!(error = %e, "accountingd: bank import CE enqueue failed");
                        }
                        accepted += 1;
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                        );
                        skipped += 1;
                    }
                }
            }
        } else {
            skipped += 1;
        }
    }
    Json(serde_json::json!({
        "accepted": accepted,
        "deduplicated": deduplicated,
        "skipped": skipped,
        "total": entries.len(),
    }))
    .into_response()
}

/// Enqueue `ce` on the transactional outbox in its own transaction.
///
/// The ledger write already committed by the time this runs, so the event is
/// announced after the fact rather than atomically with it. That is the weaker
/// guarantee, but the alternative — threading the ledger's own transaction out
/// of `post_entry` — would change the ledger API for every caller. A lost
/// enqueue here re-appears on the next import of the same bank transaction,
/// which is idempotent at the ledger level.
async fn enqueue_ce(pool: &sqlx::PgPool, ce: &mako_service::CloudEvent) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    mako_service::outbox::enqueue(&mut tx, ce).await?;
    tx.commit().await?;
    Ok(())
}

/// Everything the shared camt importer needs, so camt.053 and camt.054 differ
/// only in how the entries were parsed out of the document.
struct CamtImportCtx {
    pool: PgPool,
    ledger: Arc<crate::ledger::PgLedger>,
    iban_key: Option<[u8; 32]>,
    cfg: Arc<AccountingdConfig>,
}

/// Counters returned by [`import_cash_entries`].
#[derive(Default, serde::Serialize)]
struct CamtImportResult {
    accepted: usize,
    deduplicated: usize,
    skipped: usize,
    total: usize,
    /// Collections attributed back to a `sepa_collection_runs` group via the
    /// bank's own `NtryDtls/Btch/PmtInfId`.
    batches_matched: usize,
    /// Transactions no rung of the resolution ladder could attribute to an
    /// account. A persistently non-zero count is money sitting in the bank
    /// account against a receivable that stays open — worth an alert, which is
    /// why it is counted separately from the other `skipped` reasons.
    unresolved: usize,
    /// Batch bookings whose itemised details do not sum to the entry total.
    /// Non-zero means the bank booked more (or less) than it itemised, and the
    /// difference reaches no customer account.
    unreconciled_batches: usize,
    /// Entries the bank has not booked (`PDNG`, `INFO`, `FUTR`). Reported, not
    /// posted — the normal and expected majority of an intraday camt.052.
    not_booked: usize,
}

/// Book every transaction in a set of camt cash entries.
///
/// Shared by `camt.054` (Debit/Credit Notification) and `camt.053` (end-of-day
/// statement): both carry the same `Ntry` structure, and the difference — a
/// notification is intraday and a statement is the closing record — is about
/// *when* the file arrives, not what booking it implies. Splitting the loop in
/// two would leave two copies of the sign convention, the return detection and
/// the deduplication key.
///
/// Batch-booked entries are expanded per `TxDtls`, so a batched SEPA collection
/// books every underlying transaction. Returns (Rückläufer) become
/// `BANKRUECKLAST` debits; ordinary credits become `ZAHLUNG`. Deduplication is
/// keyed on `AcctSvcrRef` (disambiguated per detail by `EndToEndId`), falling
/// back to a stable hash of the transaction fields.
async fn import_cash_entries(
    ctx: &CamtImportCtx,
    entries: &[crate::sepa::CashEntry],
) -> CamtImportResult {
    let CamtImportCtx {
        pool,
        ledger,
        iban_key,
        cfg,
    } = ctx;
    let mut out = CamtImportResult::default();

    for entry in entries {
        // `Btch/PmtInfId` is the bank's own assertion of which submitted
        // `PmtInf` group this booking aggregates — the element that matches a
        // booked collection back to what was sent, without guessing from
        // amounts and dates.
        let batch_pmt_inf_id = entry
            .batch
            .as_ref()
            .and_then(|b| b.payment_info_id.as_deref());

        // ── Only a *booked* entry is a money movement ────────────────────────
        //
        // `Ntry/Sts` is not decoration. `INFO` is explicitly informational — the
        // bank is telling you something, not moving money. `PDNG` has not
        // settled and may still be amended or dropped; `FUTR` has not happened
        // yet. Posting any of them into an append-only ledger books a payment
        // that does not exist and cannot be un-booked — and the camt.053 that
        // later carries the real entry has a different `AcctSvcrRef`, so the
        // deduplication key does not save you.
        //
        // This is what makes the intraday camt.052 door safe: its entries are
        // provisional by design, and the booked ones are exactly the subset that
        // is not.
        // An absent `Sts` parses as `Booked`, which is the right default — a
        // bank that omits the element has booked the entry. So this skips only
        // what the bank explicitly said is not settled.
        if entry.status != crate::sepa::EntryStatus::Booked {
            // `PDNG`, `INFO` and `FUTR` are the expected majority of an intraday
            // file and are not worth a warning. A code outside the enumeration
            // is a bank doing something this service has never seen, and
            // declining to post it is a decision an operator should know about.
            if matches!(entry.status, crate::sepa::EntryStatus::Other(_)) {
                tracing::warn!(
                    status = ?entry.status,
                    account_servicer_ref = ?entry.account_servicer_ref,
                    amount_ct = entry.signed_ct(),
                    "accountingd: camt entry carries an unrecognised booking status — \
                     not posted. If this bank uses the code for a settled booking, the \
                     money is in the account and no ledger entry exists for it."
                );
            } else {
                tracing::debug!(
                    status = ?entry.status,
                    account_servicer_ref = ?entry.account_servicer_ref,
                    "accountingd: camt entry is not booked — reported, not posted"
                );
            }
            out.total += 1;
            out.not_booked += 1;
            continue;
        }

        // A batch booking asserts that its details add up to the entry total.
        // When they do not, the bank has itemised only part of what it booked —
        // the rest is real money that will never reach a customer account, and
        // it shows up nowhere unless someone says so. `details_reconcile()` is
        // the crate's own check; the import continues (the itemised part is
        // still correct) but the discrepancy is logged and counted.
        if entry.batch_booked && !entry.details.is_empty() && !entry.details_reconcile() {
            let itemised = entry.details_signed_sum_ct();
            tracing::warn!(
                account_servicer_ref = ?entry.account_servicer_ref,
                payment_info_id = ?batch_pmt_inf_id,
                entry_total_ct = entry.signed_ct(),
                itemised_sum_ct = ?itemised,
                detail_count = entry.details.len(),
                batch_count = ?entry.batch.as_ref().and_then(|b| b.transaction_count),
                "accountingd: camt batch booking does not reconcile with its itemised details \
                 — the difference is money booked at the bank that reaches no customer account"
            );
            out.unreconciled_batches += 1;
        }

        // One import per transaction detail; entry-level fallback when the
        // bank reported no TxDtls (single unbatched booking).
        let details: Vec<Option<&crate::sepa::EntryDetail>> = if entry.details.is_empty() {
            vec![None]
        } else {
            entry.details.iter().map(Some).collect()
        };

        for detail in details {
            out.total += 1;
            // `EntryDetail::signed_ct()` resolves the per-detail amount
            // (TxDtls/Amt → AmtDtls → entry total only for a single-detail
            // entry), signed by CdtDbtInd, and is `None` when the statement
            // doesn't determine it — precisely when the obvious "reuse the entry
            // total per detail" fallback would multiply a batch by its
            // transaction count.
            let signed_ct = match detail {
                Some(d) => d.signed_ct(),
                None => Some(entry.signed_ct()), // single unbatched booking
            };
            let Some(signed_ct) = signed_ct else {
                tracing::warn!(
                    "accountingd: camt batch detail has no determinable amount — skipping"
                );
                out.skipped += 1;
                continue;
            };
            // A bank does not always report the counterparty IBAN — a cash
            // deposit, a foreign transfer, a booking whose `RltdPties` block the
            // bank omits. Not a dead end: the resolution ladder can still
            // identify the payer from the reference.
            let counterparty_iban = detail
                .and_then(|d| d.counterparty_iban.as_deref())
                .unwrap_or("");
            let end_to_end_id = detail.and_then(|d| d.end_to_end_id.as_deref());
            // `AddtlTxInf` / `AddtlNtryInf` carry the bank's own statement text;
            // for an entry with no `NtryDtls` the latter is often the only
            // remittance information in the file.
            let reference = detail
                .and_then(|d| {
                    d.reference
                        .as_deref()
                        .or(d.end_to_end_id.as_deref())
                        .or(d.additional_info.as_deref())
                })
                .or(entry.additional_info.as_deref())
                .unwrap_or("camt import");
            let Some(date) = entry
                .value_date()
                .or_else(|| entry.booking_date())
                .and_then(|iso| time::Date::try_from(iso).ok())
            else {
                out.skipped += 1;
                continue;
            };

            let bank_txn_id = entry
                .account_servicer_ref
                .clone()
                .map(|r| {
                    // A batched entry shares one AcctSvcrRef — disambiguate
                    // per detail with the EndToEndId when present.
                    match end_to_end_id {
                        Some(e2e) => format!("{r}#{e2e}"),
                        None => r,
                    }
                })
                .unwrap_or_else(|| {
                    let key = format!("{counterparty_iban}|{signed_ct}|{date}|{reference}");
                    format!(
                        "{:016x}",
                        key.bytes().fold(0u64, |acc, b| {
                            acc.wrapping_mul(1099511628211).wrapping_add(u64::from(b))
                        })
                    )
                });

            match crate::pg::bank_import_already_processed(pool, &cfg.tenant, &bank_txn_id).await {
                Ok(true) => {
                    out.deduplicated += 1;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accountingd: dedup check failed — processing entry anyway");
                }
                Ok(false) => {}
            }

            // The counterparty IBAN is the strongest evidence but not the only
            // one — see `resolve_account_for_payment`.
            let iban_h = (!counterparty_iban.is_empty())
                .then(|| crate::ledger::iban_hash(iban_key.as_ref(), counterparty_iban));
            let matched = crate::pg::resolve_account_for_payment(
                pool,
                &cfg.tenant,
                crate::pg::PaymentClues {
                    iban_hash: iban_h.as_deref(),
                    end_to_end_id,
                    remittance: Some(reference),
                },
            )
            .await;
            let Ok(Some(matched)) = matched else {
                out.unresolved += 1;
                out.skipped += 1;
                continue;
            };
            let malo_id = matched.malo_id.clone();
            let lf_mp_id = matched.lf_mp_id.clone();

            // Per **detail**, not per entry: a batch booking mixes settled
            // collections with returns, and `CashEntry::is_return` answers for
            // the aggregate. Reading the aggregate here mislabelled the event
            // for every transaction in a mixed batch.
            let return_reason = detail.and_then(|d| d.return_reason_code.as_deref());
            let is_return = return_reason.is_some() || signed_ct < 0;
            let entry_type = if is_return {
                "BANKRUECKLAST"
            } else {
                "ZAHLUNG"
            };
            // Bank-statement sign → ledger sign, through the one conversion
            // every bank path shares: the ledger convention is positive =
            // Forderung (debit), so an incoming payment (bank credit) REDUCES
            // the receivable and a returned direct debit (bank debit) RE-OPENS
            // it.
            let ledger_ct = crate::sepa::bank_to_ledger_ct(signed_ct);
            let description = match return_reason {
                Some(code) => format!("camt Rückläufer ({code})"),
                None => "camt Zahlungseingang".to_owned(),
            };

            match crate::pg::post_entry(
                ledger,
                pool,
                &cfg.tenant,
                &malo_id,
                &lf_mp_id,
                entry_type,
                ledger_ct,
                &format!("bank:{bank_txn_id}"),
                None,
                Some(reference),
                date,
                date,
                Some(&description),
                None,
            )
            .await
            {
                Ok(ledger_id) => {
                    if let Err(e) = crate::pg::record_bank_import(
                        pool,
                        &cfg.tenant,
                        &bank_txn_id,
                        signed_ct.abs(),
                        Some(counterparty_iban),
                        date,
                        Some(ledger_id),
                        batch_pmt_inf_id,
                        end_to_end_id,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "accountingd: bank_import_log insert failed");
                    }

                    // Close the loop on the collection this booking settles or
                    // returns. The EndToEndId is accountingd's Mandatsreferenz,
                    // so a booked collection stops being an open SUBMITTED row
                    // and a Rückläufer is recorded as the R-transaction it is.
                    if let Some(e2e) = end_to_end_id {
                        match crate::pg::find_collection_entry_by_e2e(pool, &cfg.tenant, e2e).await
                        {
                            Ok(Some(collected)) => {
                                let status = if is_return { "RETURNED" } else { "SETTLED" };
                                if let Err(e) = crate::pg::set_collection_entry_status(
                                    pool,
                                    collected.entry_id,
                                    status,
                                    return_reason,
                                )
                                .await
                                {
                                    tracing::warn!(error = %e, "accountingd: collection entry status update failed");
                                }
                                if batch_pmt_inf_id.is_some_and(|p| p == collected.payment_info_id)
                                {
                                    out.batches_matched += 1;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "accountingd: collection entry lookup failed");
                            }
                        }
                    }

                    let ce_type = if is_return {
                        mako_events::accounting::BANKRUECKLAST
                    } else {
                        mako_events::accounting::PAYMENT_IMPORTED
                    };
                    let amount_ct = ledger_ct.abs();
                    let ce = mako_service::CloudEvent::new(
                        mako_service::source("accountingd", &cfg.tenant),
                        ce_type,
                        &malo_id,
                        serde_json::json!({
                            "malo_id":         malo_id,
                            "lf_mp_id":        lf_mp_id,
                            "amount_ct":       amount_ct,
                            "amount_eur":      crate::sepa::ct_to_eur_str(amount_ct),
                            "is_return":       is_return,
                            "return_reason":   return_reason,
                            "reference":       reference,
                            "bank_txn_id":     bank_txn_id,
                            "payment_info_id": batch_pmt_inf_id,
                            "end_to_end_id":   end_to_end_id,
                            "booking_date":    date.to_string(),
                            "ledger_id":       ledger_id.to_string(),
                            // Which rung of the resolution ladder matched:
                            // "iban" is the bank's own assertion, the others are
                            // inferences an agent may want to review.
                            "matched_by":      matched.matched_by,
                        }),
                    );
                    if let Err(e) = enqueue_ce(pool, &ce).await {
                        tracing::warn!(error = %e, "accountingd: bank import CE enqueue failed");
                    }
                    out.accepted += 1;
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                    );
                    out.skipped += 1;
                }
            }
        }
    }
    out
}

/// `POST /api/v1/payments/import/camt054` — ingest a **camt.054 XML document**
/// exactly as the bank delivers it (Bank-to-Customer Debit/Credit Notification).
///
/// The booking rules are shared with the camt.053 statement import: batched
/// entries are expanded per `TxDtls`, returns (Rückläufer) become
/// `BANKRUECKLAST` debits and ordinary credits `ZAHLUNG`, and deduplication is
/// keyed on `AcctSvcrRef` (disambiguated per detail by `EndToEndId`) with a
/// stable hash of the transaction fields as the fallback.
pub async fn import_payments_camt054(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(iban_key): Extension<Option<[u8; 32]>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    body: String,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "import-payments", &cfg.tenant) {
        return forbidden(&e);
    }
    let doc = match crate::sepa::parse_camt054(&body) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("camt.054 parse failed: {e}") })),
            )
                .into_response();
        }
    };

    let ctx = CamtImportCtx {
        pool,
        ledger,
        iban_key,
        cfg,
    };
    let mut result = CamtImportResult::default();
    let mut accounts = Vec::new();
    for notification in &doc.notifications {
        let r = import_cash_entries(&ctx, &notification.entries).await;
        result.accepted += r.accepted;
        result.deduplicated += r.deduplicated;
        result.skipped += r.skipped;
        result.total += r.total;
        result.batches_matched += r.batches_matched;
        result.unresolved += r.unresolved;
        result.unreconciled_batches += r.unreconciled_batches;
        result.not_booked += r.not_booked;
        accounts.push(account_ref_json(&notification.account));
    }

    Json(serde_json::json!({
        "msg_id": doc.msg_id,
        "accounts": accounts,
        "accepted": result.accepted,
        "deduplicated": result.deduplicated,
        "skipped": result.skipped,
        "unresolved": result.unresolved,
        "not_booked": result.not_booked,
        "unreconciled_batches": result.unreconciled_batches,
        "batches_matched": result.batches_matched,
        "total": result.total,
    }))
    .into_response()
}

/// `POST /api/v1/payments/import/camt052` — ingest a **camt.052 XML report**
/// (Bank-to-Customer Account Report), the bank's intraday view.
///
/// The door for a bank that offers intraday reporting as camt.052 rather than
/// camt.054 notifications — a common pairing with an end-of-day camt.053, and
/// one mako had no way to read at all.
///
/// ## Provisional by design
///
/// camt.052 reports movements *during* the business day, and its entries are
/// provisional: an entry that is `PDNG` now may be booked, amended or dropped by
/// the time the statement arrives. An append-only ledger cannot un-book a
/// posting, so only entries the bank has marked `BOOK` are posted here. The rest
/// are counted as `not_booked` and reported back — normally the majority of an
/// intraday file, and not an error.
///
/// The booked ones are safe to take early: the same transaction arriving again
/// in the evening's camt.053 carries the same `AcctSvcrRef`, so the ledger
/// idempotency key makes the second import a no-op.
///
/// Interim balances (`ITBD`) are returned rather than posted — a receivables
/// ledger has no place to put a treasury figure.
pub async fn import_payments_camt052(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(iban_key): Extension<Option<[u8; 32]>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    body: String,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "import-payments", &cfg.tenant) {
        return forbidden(&e);
    }
    let doc = match crate::sepa::parse_camt052(&body) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("camt.052 parse failed: {e}") })),
            )
                .into_response();
        }
    };

    let ctx = CamtImportCtx {
        pool,
        ledger,
        iban_key,
        cfg,
    };
    let mut result = CamtImportResult::default();
    let mut reports = Vec::new();
    for report in &doc.reports {
        let r = import_cash_entries(&ctx, &report.entries).await;
        result.accepted += r.accepted;
        result.deduplicated += r.deduplicated;
        result.skipped += r.skipped;
        result.total += r.total;
        result.batches_matched += r.batches_matched;
        result.unresolved += r.unresolved;
        result.unreconciled_batches += r.unreconciled_batches;
        result.not_booked += r.not_booked;
        reports.push(serde_json::json!({
            "report_id":        report.report_id,
            "account":          account_ref_json(&report.account),
            "from_date":        report.from_date,
            "to_date":          report.to_date,
            "net_movement_ct":  report.net_movement_ct(),
            "balances":         balances_json(&report.balances),
        }));
    }

    Json(serde_json::json!({
        "msg_id": doc.msg_id,
        "reports": reports,
        "accepted": result.accepted,
        "deduplicated": result.deduplicated,
        "skipped": result.skipped,
        "unresolved": result.unresolved,
        "not_booked": result.not_booked,
        "unreconciled_batches": result.unreconciled_batches,
        "batches_matched": result.batches_matched,
        "total": result.total,
    }))
    .into_response()
}

/// `POST /api/v1/payments/import/camt053` — ingest a **camt.053 XML statement**
/// (Bank-to-Customer Statement), the end-of-day record of the operator's own
/// account.
///
/// Bookings follow the same rules as the camt.054 import; what a statement adds
/// is the balance set. The reported closing balance is
/// returned alongside the import counters so an operator — or the reconciliation
/// agent — can compare the bank's own closing figure against the ledger without
/// a second request.
///
/// Running both imports is safe: `bank_import_log` and the ledger idempotency
/// key are keyed on the bank's transaction reference, so a transaction notified
/// intraday by camt.054 and again in the evening's camt.053 books once.
pub async fn import_payments_camt053(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(iban_key): Extension<Option<[u8; 32]>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    body: String,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "import-payments", &cfg.tenant) {
        return forbidden(&e);
    }
    let doc = match crate::sepa::parse_camt053(&body) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("camt.053 parse failed: {e}") })),
            )
                .into_response();
        }
    };

    let ctx = CamtImportCtx {
        pool,
        ledger,
        iban_key,
        cfg,
    };
    let mut result = CamtImportResult::default();
    let mut statements = Vec::new();
    for statement in &doc.statements {
        let r = import_cash_entries(&ctx, &statement.entries).await;
        result.accepted += r.accepted;
        result.deduplicated += r.deduplicated;
        result.skipped += r.skipped;
        result.total += r.total;
        result.batches_matched += r.batches_matched;
        result.unresolved += r.unresolved;
        result.unreconciled_batches += r.unreconciled_batches;
        result.not_booked += r.not_booked;
        statements.push(serde_json::json!({
            "stmt_id":  statement.stmt_id,
            "account":  account_ref_json(&statement.account),
            "balances": balances_json(&statement.balances),
        }));
    }

    Json(serde_json::json!({
        "msg_id": doc.msg_id,
        "statements": statements,
        "accepted": result.accepted,
        "deduplicated": result.deduplicated,
        "skipped": result.skipped,
        "unresolved": result.unresolved,
        "not_booked": result.not_booked,
        "unreconciled_batches": result.unreconciled_batches,
        "batches_matched": result.batches_matched,
        "total": result.total,
    }))
    .into_response()
}

/// Render a camt balance set for the response body.
fn balances_json(balances: &[sepa::camt::StatementBalance]) -> Vec<serde_json::Value> {
    balances
        .iter()
        .map(|b| {
            serde_json::json!({
                "type":      b.balance_type.as_code(),
                "signed_ct": b.signed_ct(),
                "currency":  b.currency,
                "date":      b.date().map(|d| d.to_string()),
            })
        })
        .collect()
}

/// Render an [`AccountRef`](crate::sepa::AccountRef) for the response body.
///
/// `Acct/Id` is a choice between an IBAN and a proprietary `Othr/Id`, so an
/// account that is not IBAN-addressable stays distinguishable from one with no
/// identifier at all.
fn account_ref_json(account: &crate::sepa::AccountRef) -> serde_json::Value {
    serde_json::json!({
        "iban":         account.iban,
        "other_id":     account.other_id,
        "currency":     account.currency,
        "servicer_bic": account.servicer_bic,
    })
}

// ── Prometheus metrics ────────────────────────────────────────────────────────

/// `GET /metrics` — Prometheus exposition of live financial + operational gauges.
///
/// Queried on scrape (no in-memory counters to drift). Exposes the money-path
/// signals an SRE alerts on: open receivables, credit balances awaiting refund,
/// dunning progression, and stuck SEPA runs.
pub async fn metrics(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
) -> impl IntoResponse {
    let m = match crate::pg::financial_metrics(&pool, &cfg.tenant).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("# metrics query failed: {e}\n"),
            )
                .into_response();
        }
    };
    let t = &cfg.tenant;
    let body = format!(
        "# HELP accountingd_accounts_total Number of customer accounts.\n\
         # TYPE accountingd_accounts_total gauge\n\
         accountingd_accounts_total{{tenant=\"{t}\"}} {}\n\
         # HELP accountingd_open_receivables_ct Sum of positive balances (ct).\n\
         # TYPE accountingd_open_receivables_ct gauge\n\
         accountingd_open_receivables_ct{{tenant=\"{t}\"}} {}\n\
         # HELP accountingd_credit_balances_ct Sum of credit balances owed to customers (ct).\n\
         # TYPE accountingd_credit_balances_ct gauge\n\
         accountingd_credit_balances_ct{{tenant=\"{t}\"}} {}\n\
         # HELP accountingd_dunning_open Open dunning cases by Mahnstufe.\n\
         # TYPE accountingd_dunning_open gauge\n\
         accountingd_dunning_open{{tenant=\"{t}\",stufe=\"1\"}} {}\n\
         accountingd_dunning_open{{tenant=\"{t}\",stufe=\"2\"}} {}\n\
         accountingd_dunning_open{{tenant=\"{t}\",stufe=\"3\"}} {}\n\
         # HELP accountingd_sepa_runs_pending SEPA collection runs not yet dispatched.\n\
         # TYPE accountingd_sepa_runs_pending gauge\n\
         accountingd_sepa_runs_pending{{tenant=\"{t}\"}} {}\n\
         # HELP accountingd_sperrung_pending Mahnstufe-3 cases handed to sperrd.\n\
         # TYPE accountingd_sperrung_pending gauge\n\
         accountingd_sperrung_pending{{tenant=\"{t}\"}} {}\n\
         # HELP accountingd_sepa_collections Direct-debit collections by lifecycle state.\n\
         # TYPE accountingd_sepa_collections gauge\n\
         accountingd_sepa_collections{{tenant=\"{t}\",status=\"submitted\"}} {}\n\
         accountingd_sepa_collections{{tenant=\"{t}\",status=\"rejected\"}} {}\n\
         accountingd_sepa_collections{{tenant=\"{t}\",status=\"returned\"}} {}\n\
         # HELP accountingd_sepa_collections_open_ct Amount in ct collected but not yet confirmed by the bank.\n\
         # TYPE accountingd_sepa_collections_open_ct gauge\n\
         accountingd_sepa_collections_open_ct{{tenant=\"{t}\"}} {}\n",
        m.accounts_total,
        m.open_receivables_ct,
        m.credit_balances_ct,
        m.dunning_stufe1,
        m.dunning_stufe2,
        m.dunning_stufe3,
        m.sepa_runs_pending,
        m.sperrung_pending,
        m.sepa_collections_open,
        m.sepa_collections_rejected,
        m.sepa_collections_returned,
        m.sepa_collections_open_ct,
    );
    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

// ── Business partner (Geschäftspartner) aggregation ───────────────────────────

/// `PUT /api/v1/accounts/{malo_id}/business-partner` — link an account to a
/// vertragd `kunden_nr` so its balance/dunning aggregate with the customer's
/// other market locations (FI-CA contract-account model).
pub async fn put_account_business_partner(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let Some(kunden_nr) = body.get("kunden_nr").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "kunden_nr required" })),
        )
            .into_response();
    };
    match crate::pg::set_account_kunden_nr(&pool, &malo_id, lf_mp_id, &cfg.tenant, kunden_nr).await
    {
        Ok(0) => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/business-partners/{kunden_nr}/accounts` — all accounts.
pub async fn get_bp_accounts(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(kunden_nr): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    match crate::pg::list_accounts_by_bp(&pool, &cfg.tenant, &kunden_nr).await {
        Ok(rows) => Json(serde_json::json!({
            "kunden_nr": kunden_nr,
            "account_count": rows.len(),
            "accounts": rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/business-partners/{kunden_nr}/balance` — consolidated balance.
pub async fn get_bp_balance(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(kunden_nr): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    match crate::pg::bp_consolidated_balance(&pool, &cfg.tenant, &kunden_nr).await {
        Ok(total_ct) => Json(serde_json::json!({
            "kunden_nr": kunden_nr,
            "balance_ct": total_ct,
            "balance_eur": format_ct_as_eur(total_ct),
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Offene Posten ─────────────────────────────────────────────────────────────

/// `GET /api/v1/offene-posten`  — overdue accounts
pub async fn get_offene_posten(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<OffenePostenQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    // parse min_balance_eur as a decimal string to avoid f64 rounding errors.
    // e.g. "1.99" must produce 199 ct, not 198 ct from (1.99 * 100.0) as i64.
    let min_ct: i64 = q
        .min_balance_eur
        .as_deref()
        .and_then(|s| {
            use rust_decimal::Decimal;
            use std::str::FromStr;
            Decimal::from_str(s).ok().map(|d| {
                use rust_decimal::prelude::ToPrimitive as _;
                (d * Decimal::from(100)).round().to_i64().unwrap_or(1)
            })
        })
        .unwrap_or(1);
    match list_overdue_accounts(&pool, &cfg.tenant, min_ct, q.limit.unwrap_or(200).min(2000)).await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Query parameters for `GET /api/v1/offene-posten`.
///
/// `min_balance_eur` is a **decimal string** (e.g. `"1.99"`) to avoid f64 rounding errors.
/// Float query parameters in financial APIs can silently lose precision.
#[derive(Debug, Deserialize)]
pub struct OffenePostenQuery {
    /// Minimum balance in EUR, as a decimal string (e.g. `"1.99"`). Default: 1 ct minimum.
    pub min_balance_eur: Option<String>,
    pub limit: Option<i64>,
}

// ── Dunning ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/dunning`  — open dunning cases
pub async fn get_dunning(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<DunningQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    match list_open_dunning(&pool, &cfg.tenant, q.limit.unwrap_or(200).min(1000)).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/dunning/{account_id}/escalate`  — manual dunning escalation
pub async fn escalate_dunning(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(account_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    let account = match fetch_account_by_id(&pool, account_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let stufe: i16 = body.get("stufe").and_then(|v| v.as_i64()).unwrap_or(1) as i16;
    let amount_due_ct = account.balance_ct.max(0);
    let due_days: i64 = body.get("due_days").and_then(|v| v.as_i64()).unwrap_or(14);
    let due_date = (OffsetDateTime::now_utc() + time::Duration::days(due_days)).date();

    // Manual escalation announces the Mahnstufe exactly like the auto-dunning
    // worker — a case opened by an operator is no less material to the ERP or
    // to agentd's payment-reconciliation agent.
    match create_dunning_case_announced(
        &pool,
        &cfg.tenant,
        account_id,
        &account.malo_id,
        &account.lf_mp_id,
        stufe,
        amount_due_ct,
        due_date,
    )
    .await
    {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/dunning/{id}/resolve`
pub async fn resolve_dunning(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    match resolve_dunning_case(&pool, id, &cfg.tenant).await {
        Ok(0) => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
/// `POST /api/v1/dunning/{id}/abwendung/angebot` — § 41g Abs. 1 S. 2 EnWG.
///
/// Records that the Grundversorger's **offer** went out, and emits
/// `de.accounting.abwendung.angeboten` with the interest-free instalment terms.
///
/// Owed within **one week** of the customer demanding it after the Androhung, and
/// no later than the Ankündigung. Recorded separately from acceptance because it
/// is the only evidence the obligation was met — a supplier that never offered
/// and one that offered and was refused are otherwise indistinguishable.
///
/// The instalment period follows § 41g Abs. 1 S. 7–9: **6 to 18 months** in
/// general, and **12 to 24 months** once the arrears exceed 300 EUR.
pub async fn abwendung_angebot(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    let Ok(Some((_, malo_id, lf_mp_id))) =
        crate::pg::dunning_case_account(&pool, id, &cfg.tenant).await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Re-derived, not read: the instalment band depends on the arrears, and an
    // offer quoting the wrong band is a §41g Abs. 1 S. 7-9 defect.
    let verzug_ct =
        match crate::pg::refresh_verzug(&ledger, &pool, &cfg.tenant, &malo_id, &lf_mp_id).await {
            Ok(v) => v,
            Err(e) => return internal(&e),
        };
    // § 41g Abs. 1 S. 7–9: 6–18 Monate in der Regel, 12–24 Monate über 300 EUR.
    let (min_monate, max_monate) = if verzug_ct > 30_000 {
        (12, 24)
    } else {
        (6, 18)
    };

    let ce = mako_service::CloudEvent::new(
        mako_service::source("accountingd", &cfg.tenant),
        mako_events::accounting::ABWENDUNG_ANGEBOTEN,
        &malo_id,
        serde_json::json!({
            "malo_id":         malo_id,
            "lf_mp_id":        lf_mp_id,
            "case_id":         id.to_string(),
            "amount_due_ct":   verzug_ct,
            "rechtsgrundlage": "§41g Abs. 1 EnWG",
            "ratenzahlung": {
                "zinsfrei":            true,
                "min_laufzeit_monate": min_monate,
                "max_laufzeit_monate": max_monate,
                "weiterversorgung":    true,
            },
        }),
    );
    let done = async {
        let mut tx = pool.begin().await?;
        let marked = crate::pg::mark_abwendung_angeboten(&mut *tx, id, &cfg.tenant).await?;
        if marked {
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
        }
        tx.commit().await?;
        anyhow::Ok(marked)
    }
    .await;
    match done {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        // Already offered, or the case is closed — either way there is nothing
        // to do and nothing went wrong.
        Ok(false) => StatusCode::CONFLICT.into_response(),
        Err(e) => internal(&e),
    }
}

/// `GET /api/v1/sepa/mandates/dormant?within_days=30` — the EPC 36-month sweep.
///
/// A mandate not presented for **36 consecutive months** must be cancelled by
/// the creditor. The clock resets on every presentation, *including collections
/// later rejected or refunded*, so it counts from `last_presented_at` — stamped
/// when the collection is written into a run, not when the bank confirms it.
///
/// The debtor banks do not enforce this, so `within_days` looks ahead: a mandate
/// going dormant next month is worth knowing about while the customer can still
/// be asked for a new one.
pub async fn get_dormant_mandates(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<ReviewQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-banking", &cfg.tenant) {
        return forbidden(&e);
    }
    let within = q.older_than_days.unwrap_or(30).clamp(0, 365);
    match crate::pg::list_dormant_mandates(&pool, &cfg.tenant, within).await {
        Ok(rows) => Json(serde_json::json!({
            "dormancy_months": crate::pg::MANDATE_DORMANCY_MONTHS,
            "within_days": within,
            "count": rows.len(),
            "mandates": rows,
            "note": "a mandate past the 36-month limit is uncollectable and must be \
                     cancelled; the debtor banks do not enforce this, so the first \
                     symptom of ignoring it is a rejected batch",
        }))
        .into_response(),
        Err(e) => internal(&e),
    }
}

// ── Mahnsperren (§§41f/41g halts) ─────────────────────────────────────────────

/// Body of `POST /api/v1/dunning/{id}/locks`.
#[derive(Debug, Deserialize)]
pub struct PlaceLockRequest {
    pub grund: crate::pg::LockGrund,
    /// The citation this rests on. Defaults to the ground's own.
    pub rechtsgrundlage: Option<String>,
    pub note: Option<String>,
    /// When the lock takes effect. Absent = today. Settable because the evidence
    /// a lock rests on has its own dates — a certificate covering January to
    /// March is recorded as it reads, not as of the day it was typed in.
    pub valid_from: Option<time::Date>,
    /// When the lock lapses. Absent = open-ended, and open-ended locks are
    /// listed by `GET /api/v1/dunning/locks/review`.
    pub valid_to: Option<time::Date>,
}

/// `POST /api/v1/dunning/{id}/locks` — stop dunning this account, and say why.
///
/// One endpoint for every §§41f/41g halt. A lock carries its ground, its
/// citation, a validity period and the operator who set it, and is lifted by
/// `DELETE .../locks/{lock_id}` with a reason — because § 41g Abs. 1 S. 11 lets
/// the supplier resume after a broken agreement, and § 41f Abs. 2 makes the
/// Gefahr *auf Verlangen glaubhaft zu machen*, hence reviewable.
pub async fn place_lock(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PlaceLockRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    if matches!(req.grund, crate::pg::LockGrund::Operator) && req.note.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "grund = \"operator\" requires a note — a halt with no \
                          statutory ground and no reason is not auditable",
            })),
        )
            .into_response();
    }
    let placed = crate::pg::place_dunning_lock(
        &pool,
        id,
        &cfg.tenant,
        req.grund,
        req.rechtsgrundlage.as_deref(),
        req.note.as_deref(),
        req.valid_from,
        req.valid_to,
        Some(claims.sub()),
    )
    .await;
    match placed {
        Ok(Some(lock_id)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "lock_id": lock_id })),
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal(&e),
    }
}

/// `GET /api/v1/dunning/{id}/locks` — every lock this account has carried.
pub async fn get_locks(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    match crate::pg::list_dunning_locks(&pool, id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => internal(&e),
    }
}

/// `GET /api/v1/dunning/locks/review?older_than_days=90` — open-ended locks.
///
/// § 41f Abs. 2 contemplates circumstances with no foreseeable end and equally
/// makes them reviewable, so an unbounded lock is allowed but surfaced here.
pub async fn get_locks_due_review(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<ReviewQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    let days = q.older_than_days.unwrap_or(90).clamp(0, 3650);
    match crate::pg::list_locks_due_review(&pool, &cfg.tenant, days).await {
        Ok(rows) => Json(serde_json::json!({
            "older_than_days": days,
            "count": rows.len(),
            "locks": rows,
        }))
        .into_response(),
        Err(e) => internal(&e),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReviewQuery {
    pub older_than_days: Option<i64>,
}

/// Body of `DELETE /api/v1/dunning/locks/{lock_id}`.
#[derive(Debug, Deserialize)]
pub struct LiftLockRequest {
    /// Why the lock no longer applies. `vereinbarung_gebrochen` carries the
    /// § 41g Abs. 1 S. 11 side effect described below.
    pub grund: String,
}

/// `DELETE /api/v1/dunning/locks/{lock_id}` — lift a lock, with a reason.
///
/// Lifting for **`vereinbarung_gebrochen`** is § 41g Abs. 1 S. 11: the supplier
/// may resume, but must re-observe § 41f Abs. 1 S. 2 **and Abs. 5**, so the
/// Ankündigung state is cleared and the case returns to a *fresh* 8-Werktage
/// announcement. Any other reason leaves the announcement standing.
pub async fn lift_lock(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(lock_id): Path<Uuid>,
    Json(req): Json<LiftLockRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    if req.grund.trim().is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "grund must not be empty" })),
        )
            .into_response();
    }
    let broken = req.grund.trim() == "vereinbarung_gebrochen";
    let done = async {
        let mut tx = pool.begin().await?;
        let Some(account_id) =
            crate::pg::lift_dunning_lock(&mut *tx, lock_id, &cfg.tenant, req.grund.trim()).await?
        else {
            return anyhow::Ok(None);
        };
        if broken {
            crate::pg::clear_ankuendigung(&mut *tx, account_id, &cfg.tenant).await?;
            // In the same transaction as the lift: a broken agreement re-opens
            // the path to a disconnection and § 41f Abs. 5 needs a fresh letter,
            // so resuming without telling the ERP is the failure mode.
            let ids: Option<(String, String)> = sqlx::query_as(
                "SELECT malo_id, lf_mp_id FROM accounts WHERE account_id = $1 AND tenant = $2",
            )
            .bind(account_id)
            .bind(&cfg.tenant)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((malo_id, lf_mp_id)) = ids {
                let ce = mako_service::CloudEvent::new(
                    mako_service::source("accountingd", &cfg.tenant),
                    mako_events::accounting::ABWENDUNG_GEBROCHEN,
                    &malo_id,
                    serde_json::json!({
                        "malo_id":         malo_id,
                        "lf_mp_id":        lf_mp_id,
                        "lock_id":         lock_id.to_string(),
                        "rechtsgrundlage": "\u{a7}41g Abs. 1 S. 11 EnWG",
                        "folge": "Unterbrechung wieder zul\u{e4}ssig; \u{a7}41f Abs. 1 S. 2 und                                   Abs. 5 sind erneut zu beachten (neue Ank\u{fc}ndigung, 8 Werktage)",
                    }),
                );
                mako_service::outbox::enqueue(&mut tx, &ce).await?;
            }
        }
        tx.commit().await?;
        anyhow::Ok(Some(account_id))
    }
    .await;

    match done {
        Ok(Some(_)) if broken => (
            StatusCode::OK,
            Json(serde_json::json!({
                "lifted": true,
                "rechtsgrundlage": "\u{a7}41g Abs. 1 S. 11 EnWG",
                "note": "the Ankündigung was cleared — §41f Abs. 5 requires a fresh \
                         8-Werktage announcement before any Sperrauftrag",
            })),
        )
            .into_response(),
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal(&e),
    }
}

// ── Forderungseinwände (§ 41f Abs. 3 S. 3–5) ──────────────────────────────────

/// Body of `POST /api/v1/dunning/{id}/einwaende`.
#[derive(Debug, Deserialize)]
pub struct EinwandRequest {
    pub art: crate::pg::EinwandArt,
    pub betrag_ct: i64,
    /// The disputed booking, where one can be pointed at.
    pub ledger_entry_id: Option<Uuid>,
    pub note: Option<String>,
}

/// `POST /api/v1/dunning/{id}/einwaende` — § 41f Abs. 3 S. 3–5 EnWG.
///
/// Record an amount that must stay **out of the Verzug calculation**: a claim
/// disputed form- und fristgerecht and schlüssig, a disputed price increase, a
/// claim before a § 111b Schlichtungsverfahren, or instalments not yet due.
///
/// Not a lock: it does not halt the sequence, it reduces the number the § 41f
/// Abs. 3 gates are measured against, and the sequence stops by itself when what
/// remains falls below them.
pub async fn place_einwand(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(id): Path<Uuid>,
    Json(req): Json<EinwandRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    if req.betrag_ct <= 0 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "betrag_ct must be positive" })),
        )
            .into_response();
    }
    let Ok(Some((account_id, malo_id, lf_mp_id))) =
        crate::pg::dunning_case_account(&pool, id, &cfg.tenant).await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let done = async {
        let mut tx = pool.begin().await?;
        let einwand_id = crate::pg::record_einwand(
            &mut *tx,
            &cfg.tenant,
            account_id,
            req.art,
            req.betrag_ct,
            req.ledger_entry_id,
            req.note.as_deref(),
            Some(claims.sub()),
        )
        .await?;
        tx.commit().await?;
        // The objection changes the Verzug with no posting behind it, so the
        // cache has to be told; leaving it stale would let the sequence run on
        // arrears the statute says do not count.
        let verzug =
            crate::pg::refresh_verzug(&ledger, &pool, &cfg.tenant, &malo_id, &lf_mp_id).await?;
        anyhow::Ok((einwand_id, verzug))
    }
    .await;

    match done {
        Ok((einwand_id, verzug)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "einwand_id": einwand_id,
                "verzug_ct": verzug,
                "verzug_eur": format_ct_as_eur(verzug),
                "rechtsgrundlage": "\u{a7}41f Abs. 3 S. 3-5 EnWG",
            })),
        )
            .into_response(),
        Err(e) => internal(&e),
    }
}

/// `GET /api/v1/dunning/{id}/einwaende` — every objection on this account.
pub async fn get_einwaende(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    let Ok(Some((account_id, _, _))) =
        crate::pg::dunning_case_account(&pool, id, &cfg.tenant).await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match crate::pg::list_einwaende(&pool, account_id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => internal(&e),
    }
}

/// Body of `POST /api/v1/einwaende/{einwand_id}/erledigen`.
#[derive(Debug, Deserialize)]
pub struct CloseEinwandRequest {
    /// `stattgegeben` | `zurueckgenommen` | `zurueckgewiesen`.
    pub erledigung: String,
}

/// `POST /api/v1/einwaende/{einwand_id}/erledigen` — close an objection.
///
/// The amount re-enters the § 41f Abs. 3 Verzug from this point, whichever way it
/// was decided: upheld means the claim was reduced or written off (and the
/// posting that does so is what removes it from the arrears), withdrawn or
/// rejected means it was owed all along.
pub async fn close_einwand(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(einwand_id): Path<Uuid>,
    Json(req): Json<CloseEinwandRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-dunning", &cfg.tenant) {
        return forbidden(&e);
    }
    if !matches!(
        req.erledigung.as_str(),
        "stattgegeben" | "zurueckgenommen" | "zurueckgewiesen"
    ) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "erledigung must be stattgegeben, zurueckgenommen or zurueckgewiesen",
            })),
        )
            .into_response();
    }
    let done = async {
        let mut tx = pool.begin().await?;
        let account =
            crate::pg::close_einwand(&mut *tx, einwand_id, &cfg.tenant, &req.erledigung).await?;
        tx.commit().await?;
        let Some(account_id) = account else {
            return anyhow::Ok(None);
        };
        let ids: Option<(String, String)> = sqlx::query_as(
            "SELECT malo_id, lf_mp_id FROM accounts WHERE account_id = $1 AND tenant = $2",
        )
        .bind(account_id)
        .bind(&cfg.tenant)
        .fetch_optional(&pool)
        .await?;
        if let Some((malo_id, lf_mp_id)) = ids {
            crate::pg::refresh_verzug(&ledger, &pool, &cfg.tenant, &malo_id, &lf_mp_id).await?;
        }
        anyhow::Ok(Some(account_id))
    }
    .await;

    match done {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal(&e),
    }
}

#[derive(Debug, Deserialize)]
pub struct DunningQuery {
    pub limit: Option<i64>,
}

// ── SEPA mandates ─────────────────────────────────────────────────────────────

/// `POST /api/v1/sepa/mandates`  — register SEPA mandate
/// `POST /api/v1/sepa/mandates`
pub async fn post_mandate(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(iban_key): Extension<Option<[u8; 32]>>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Json(req): Json<CreateMandateRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-sepa", &cfg.tenant) {
        return forbidden(&e);
    }
    // Validate IBAN checksum before writing to DB (B16).
    // Malformed IBANs cause SEPA return charges (€3–15/return) + Mahnstufe escalation.
    if let Err(msg) = validate_iban(&req.iban) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("invalid IBAN: {msg}") })),
        )
            .into_response();
    }
    // Mandatsreferenz is Max35Text (SEPA AT-01) and doubles as the pain.008
    // EndToEndId — an over-long value would make every future collection file
    // schema-invalid, so reject it at the boundary.
    let ref_len = req.mandatsref.chars().count();
    if ref_len == 0 || ref_len > 35 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "mandatsref must be 1–35 characters (SEPA Max35Text), got {ref_len}"
                )
            })),
        )
            .into_response();
    }
    match create_mandate(&pool, &cfg.tenant, iban_key.as_ref(), req).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "mandate_id": id })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/sepa/mandates/{mandate_id}`
pub async fn get_mandate(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(mandate_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-banking", &cfg.tenant) {
        return forbidden(&e);
    }
    match fetch_mandate(&pool, mandate_id, &cfg.tenant).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/sepa/mandates/{mandate_id}`  — revoke SEPA mandate.
///
/// Sets `revoked_at = today` on the mandate. Revoked mandates are excluded from
/// future pain.008 generation (§58 ZAG: debtor may revoke at any time until
/// the cut-off time of the collection date).
///
/// Does NOT affect existing `accounts.iban` or `mandatsref` columns —
/// update those separately if needed via `PUT /api/v1/accounts/{malo_id}`.
pub async fn delete_mandate(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(mandate_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-sepa", &cfg.tenant) {
        return forbidden(&e);
    }
    let today = mako_fristen::heute();
    let rows = sqlx::query(
        "UPDATE sepa_mandates SET revoked_at = $1, updated_at = now() \
         WHERE mandate_id = $2 AND tenant = $3 AND revoked_at IS NULL",
    )
    .bind(today)
    .bind(mandate_id)
    .bind(&cfg.tenant)
    .execute(&pool)
    .await;

    match rows {
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
    }
}

/// `POST /api/v1/sepa/run`  — generate pain.008 XML for all active mandates with positive balance
pub async fn run_sepa(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-sepa", &cfg.tenant) {
        return forbidden(&e);
    }
    let mandates = match list_active_mandates(&pool, &cfg.tenant, 10_000).await {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Filter: only mandates with a scheduled Abschlag (§40 Abs. 1 EnWG monthly collection).
    // Note: Abschlag is collected regardless of credit balance — the Jahresabschluss reconciles.
    let mut direct_debits = Vec::new();
    for mandate in &mandates {
        if let Some(acct) = fetch_account_by_id(&pool, mandate.account_id, &cfg.tenant)
            .await
            .ok()
            .flatten()
            .filter(|a| a.abschlag_ct > 0)
        {
            direct_debits.push((mandate, acct.abschlag_ct));
        }
    }

    // validate creditor_iban before generating pain.008.
    // A missing or invalid creditor IBAN causes hard rejection at the bank with return fees.
    let creditor_iban = match cfg.creditor_iban.as_deref().filter(|s| !s.is_empty()) {
        Some(iban) => iban,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "creditor_iban not configured — set a valid SEPA IBAN in accountingd.toml"
                })),
            )
                .into_response();
        }
    };

    let creditor_name = cfg.creditor_name.as_deref().unwrap_or(&cfg.tenant);
    let Some(creditor_id) = cfg.creditor_id.as_deref().filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "creditor_id not configured — the EPC rulebook mandates CdtrSchmeId; \
                          set the Gläubiger-ID (Bundesbank registry) in accountingd.toml"
            })),
        )
            .into_response();
    };

    // Ad-hoc runs collect at the SDD CORE minimum lead time (D-1, submit today).
    let collection_date = (time::OffsetDateTime::now_utc() + time::Duration::days(2)).date();

    let dd_schema = match crate::sepa::resolve_pain008_schema(cfg.pain008_schema.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let creditor = crate::sepa::CreditorIdentity {
        iban: creditor_iban,
        name: creditor_name,
        creditor_id,
        address: Some(&cfg.creditor_address),
    };

    match build_pain_008(&creditor, collection_date, &direct_debits, dd_schema) {
        Ok(run) => {
            // Archive the ad-hoc run exactly like the N-5 scheduler's. A
            // pain.008 handed to a bank without an audit row is a collection
            // nothing can later attribute, settle or reverse.
            let run_id = match crate::pg::persist_sepa_collection(
                &pool,
                &cfg.tenant,
                collection_date,
                &run,
            )
            .await
            {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!(error = %e, "accountingd: /sepa/run — failed to persist sepa_collection_run");
                    None
                }
            };
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    // One pain.008 message; FRST/RCUR/… are separate PmtInf groups inside it.
                    "run_id": run_id.map(|id| id.to_string()),
                    "msg_id": run.msg_id,
                    "collection_date": collection_date.to_string(),
                    "entry_count": run.entry_count,
                    "total_ct": run.total_ct,
                    "groups": run.groups,
                    "xml": &run.xml,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Vorauszahlung (BO4E typed advance-payment, L12 — §40 Abs. 1 EnWG) ────────

/// `PUT /api/v1/accounts/{malo_id}/vorauszahlung`
///
/// Store or replace the BO4E `Vorauszahlung` COM for an account.
///
/// Body: `rubo4e::current::Vorauszahlung` JSON (camelCase).
///
/// Validation:
/// - Deserialized via `rubo4e::current::Vorauszahlung` to validate all fields.
/// - Re-serialised to canonical camelCase before storage.
/// - `abschlag_ct` is updated atomically from `betrag.wert` (EUR → ct × 100)
///   so the existing Abschlagslauf scheduler continues to work.
///
/// Query parameter: `?lf_mp_id=<mp_id>` (defaults to tenant config).
///
/// §40 Abs. 1 EnWG: Abschlag must match estimated consumption.
/// Typed `Vorauszahlung` enables `portald` Jahresabschluss preview and
/// auto-adjustment when deviation exceeds 10 %.
pub async fn put_vorauszahlung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<VorauszahlungQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-account", &cfg.tenant) {
        return forbidden(&e);
    }
    use rubo4e::current::Vorauszahlung;
    use rust_decimal::Decimal;

    // The BO4E gate — the same four stages every BO4E endpoint in mako runs.
    let typed: Vorauszahlung = match mako_markt::bo4e::decode(body) {
        Ok(v) => v,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(e.to_json())).into_response(),
    };
    let canonical = match serde_json::to_value(&typed) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "validated Vorauszahlung is not serialisable");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "could not serialise Vorauszahlung" })),
            )
                .into_response();
        }
    };

    // Derive abschlag_ct from betrag.wert (EUR → ct).
    let abschlag_ct: Option<i64> =
        typed
            .betrag
            .as_ref()
            .and_then(|b| b.wert)
            .map(|eur: Decimal| {
                use rust_decimal::prelude::ToPrimitive as _;
                let ct = eur * Decimal::from(100);
                ct.round().to_i64().unwrap_or(0)
            });

    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);

    match upsert_vorauszahlung(
        &pool,
        &malo_id,
        lf_mp_id,
        &cfg.tenant,
        canonical,
        abschlag_ct,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/vorauszahlung`
///
/// Retrieve the stored BO4E `Vorauszahlung` for an account.
///
/// Falls back to synthesising a `Vorauszahlung` from `abschlag_ct` when no
/// typed record has been stored yet (backward-compatible bootstrapping).
pub async fn get_vorauszahlung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<VorauszahlungQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    use rust_decimal::Decimal;

    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);

    match fetch_vorauszahlung(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "account not found" })),
        )
            .into_response(),
        Ok(Some((vzahlung, abschlag_ct))) => {
            let body = if !vzahlung.is_null() {
                // Stored typed Vorauszahlung.
                serde_json::json!({
                    "malo_id": malo_id,
                    "vorauszahlung": vzahlung,
                    "abschlag_ct": abschlag_ct,
                    "source": "stored",
                })
            } else {
                // Synthesise from abschlag_ct — bootstrapping fallback.
                //
                // Built typed rather than as a JSON literal: this is a BO4E COM
                // served to a caller, so `_typ` is stamped by rubo4e on the
                // `Vorauszahlung` and the nested `Betrag`, and `waehrung` goes
                // through `Waehrungscode` rather than a bare string.
                use rubo4e::current::{Betrag, Vorauszahlung, Waehrungscode};
                let eur = Decimal::from(abschlag_ct) / Decimal::from(100);
                let vorauszahlung = Vorauszahlung {
                    betrag: Some(Betrag {
                        wert: Some(eur),
                        waehrung: Some(Waehrungscode::Eur),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let vorauszahlung = match mako_markt::bo4e::to_canonical_json(&vorauszahlung) {
                    Ok(v) => v,
                    Err(e) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                    }
                };
                serde_json::json!({
                    "malo_id": malo_id,
                    "vorauszahlung": vorauszahlung,
                    "abschlag_ct": abschlag_ct,
                    "source": "derived_from_abschlag_ct",
                })
            };
            Json(body).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct VorauszahlungQuery {
    pub lf_mp_id: Option<String>,
}

// ── Manual booking ────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/accounts/{malo_id}/buchen`.
#[derive(Debug, serde::Deserialize)]
pub struct BuchenRequest {
    /// Buchungsart. Must be a valid `entry_type` value.
    /// Allowed: `RECHNUNG`, `ZAHLUNG`, `GUTSCHRIFT`, `EEG_GUTSCHRIFT`,
    /// `EEG_MARKTPRAEMIE`, `BANKRUECKLAST`, `MAHNGEBUEHR`, `VERZUGSZINSEN`,
    /// `ABSCHLAG`, `ABSCHLAG_VERRECHNUNG`, `SEPA_STORNO`, `JAHRESABSCHLUSS`,
    /// `KORREKTUR`, `STORNO`.
    ///
    /// The same list [`crate::ledger::ENTRY_TYPES`] holds, so an automated path
    /// and the operator interface accept exactly the same kinds.
    pub entry_type: String,
    /// Amount in ct (× 10⁻² EUR). Positive = debit; negative = credit.
    pub amount_ct: i64,
    /// External reference (invoice number, payment reference, etc.).
    pub reference_id: Option<String>,
    /// Human-readable description for the Kontoauszug.
    pub description: Option<String>,
    /// ISO 8601 booking date. Defaults to today when absent.
    pub booking_date: Option<String>,
    /// ISO 8601 value date. Defaults to `booking_date` when absent.
    pub value_date: Option<String>,
    pub lf_mp_id: Option<String>,
}

/// `POST /api/v1/accounts/{malo_id}/buchen`
///
/// Post a manual ledger entry to a customer account (operator interface).
///
/// Use cases:
/// - Manual ZAHLUNG credit when a customer pays by bank transfer outside SEPA mandate
/// - BANKRUECKLAST debit when a SEPA direct debit is returned by the bank
/// - KORREKTUR for operator-authorised adjustments
/// - GUTSCHRIFT for one-off credits (e.g. goodwill, §40 EnWG compensation)
///
/// ## Idempotency
/// Supply `reference_id` to make the post idempotent — re-posting with the same
/// `reference_id` replays as a ledger no-op. Omit it and each call books a new
/// entry under a fresh random key.
pub async fn post_buchen(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Json(req): Json<BuchenRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }

    // Validate entry_type against the allowed set.
    const ALLOWED: &[&str] = crate::ledger::ENTRY_TYPES;
    if !ALLOWED.contains(&req.entry_type.as_str()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "unknown entry_type '{}'; allowed: {}",
                req.entry_type,
                ALLOWED.join(", ")
            ),
        )
            .into_response();
    }
    if req.amount_ct == 0 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "amount_ct must be non-zero",
        )
            .into_response();
    }

    let lf_mp_id = req.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    if let Err(e) = upsert_account(&pool, &malo_id, &lf_mp_id, &cfg.tenant).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    let today = mako_fristen::heute();
    let booking_date = req
        .booking_date
        .as_deref()
        .and_then(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).ok()
        })
        .unwrap_or(today);
    let value_date = req
        .value_date
        .as_deref()
        .and_then(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).ok()
        })
        .unwrap_or(booking_date);

    // Idempotency: the operator's reference_id when given (repost = no-op), else a
    // fresh key so each call books a new entry.
    let idempotency = req
        .reference_id
        .clone()
        .unwrap_or_else(|| format!("manual:{}", Uuid::new_v4()));

    match crate::pg::post_entry(
        &ledger,
        &pool,
        &cfg.tenant,
        &malo_id,
        &lf_mp_id,
        &req.entry_type,
        req.amount_ct,
        &idempotency,
        None,
        req.reference_id.as_deref(),
        booking_date,
        value_date,
        req.description.as_deref(),
        Some(claims.sub()),
    )
    .await
    {
        Ok(entry_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "entry_id": entry_id,
                "malo_id": malo_id,
                "entry_type": req.entry_type,
                "amount_ct": req.amount_ct,
                "amount_eur": format_ct_as_eur(req.amount_ct),
                "booking_date": booking_date.to_string(),
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── IBAN validation ───────────────────────────────────────────────────────────

/// Validate an IBAN using the ISO 13616 mod-97 algorithm.
///
/// 1. Remove whitespace and convert to uppercase.
/// 2. Move the first 4 characters to the end.
// ── IBAN validation ───────────────────────────────────────────────────────────
//
// validate_iban is re-exported from the `sepa` workspace crate (see imports above).
// The sepa crate implements ISO 13616 mod-97 and is shared with vertragd.

// ── Jahresabschluss REST API ──────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct JahresabschlussQuery {
    pub lf_mp_id: Option<String>,
    pub year: Option<i32>,
    /// When `true`, returns the computed settlement without committing any entries.
    pub dry_run: Option<bool>,
}

/// `POST /api/v1/jahresabschluss/{malo_id}`
///
/// Compute and commit the annual Jahresabschluss settlement for one MaLo.
///
/// Atomically:
/// 1. Sums all `RECHNUNG` debits and `ABSCHLAG` credits for `year`.
/// 2. If settlement_ct ≠ 0: writes a `RECHNUNG` (Nachzahlung) or `GUTSCHRIFT` (Erstattung)
///    ledger entry to `accountingd`.
/// 3. Updates the monthly Abschlag to `actual_annual ÷ 12` (§40 Abs. 1 EnWG).
///
/// Returns `{ settlement_ct, settlement_eur, new_monthly_abschlag_ct, committed }`.
/// Use `?dry_run=true` for a preview without committing.
///
/// Announces the settlement on
/// [`mako_events::accounting::JAHRESABSCHLUSS_ABGESCHLOSSEN`]
/// (`de.accounting.jahresabschluss.abgeschlossen`) — **every** time, whatever
/// the year came to — in the same transaction as the settlement it reports.
/// [`mako_events::accounting::ERSTATTUNG_FAELLIG`] rides alongside it when the
/// year produced a refund *and* an ERP webhook is configured, because that one
/// carries a pain.001 for a bank to execute rather than a fact to react to.
/// The year's Kontokorrent movements folded into the Jahresabschluss figures.
///
/// Every field is a signed Kontokorrent net (debit +, credit −) as produced by
/// [`crate::ledger::PgLedger::year_kind_sums`], so the parts simply add up —
/// and [`JahresabschlussSums::settlement_ct`] is the total of **every** kind
/// booked in the year, not of an enumerated subset. The buckets below only
/// decide how the total is *presented*; a Buchungsart nobody thought of lands
/// in [`JahresabschlussSums::sonstige_sum`] and still moves the settlement.
///
/// That is deliberate. While the total was assembled from a hand-written list
/// of kinds, adding one meant remembering to add it here too, and forgetting
/// meant a settlement that quietly disagreed with the balance it settles — the
/// failure mode that pays a refund out by pain.001.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JahresabschlussSums {
    /// What was billed for the supply: `RECHNUNG` + `STORNO` + `KORREKTUR` +
    /// `GUTSCHRIFT`. § 40 Abs. 1 EnWG — the Jahresabrechnung reflects the
    /// actual billed amounts, corrections included. Verzugsschaden is **not**
    /// here: a Mahngebühr is not consumption and must not recalibrate an
    /// Abschlag.
    pub rechnung_sum: i64,
    /// The Abschlag pair: `ABSCHLAG` demands (debits, +) net of
    /// `ABSCHLAG_VERRECHNUNG` (credits, −). In a year that both raised twelve
    /// advances and settled them on a Jahresrechnung this nets to zero — the
    /// advances were demanded and then absorbed — and what remains is whatever
    /// was demanded but not yet billed for.
    pub abschlag_sum: i64,
    /// Cash: `ZAHLUNG` (credit) net of `BANKRUECKLAST` and `SEPA_STORNO`
    /// chargebacks (debits).
    pub zahlung_sum: i64,
    /// Verzugsschaden: `MAHNGEBUEHR` + `VERZUGSZINSEN`. Owed by the customer
    /// and part of the settlement, but never part of the consumption figure
    /// that resets the Abschlag.
    pub verzugsschaden_sum: i64,
    /// Everything else booked in the year — a prior `JAHRESABSCHLUSS`
    /// Erstattung, an `EEG_GUTSCHRIFT`, a Buchungsart added later. Present so
    /// that the buckets always re-sum to `settlement_ct`.
    pub sonstige_sum: i64,
    /// `> 0` Nachzahlung, `< 0` Erstattung, `0` ausgeglichen. The signed net of
    /// the year's whole Kontokorrent movement.
    pub settlement_ct: i64,
}

impl JahresabschlussSums {
    /// The kinds that make up [`Self::rechnung_sum`] — the supply billing that
    /// also recalibrates the Abschlag (§ 40 Abs. 1 EnWG).
    const BILLED: &'static [&'static str] = &["RECHNUNG", "STORNO", "KORREKTUR", "GUTSCHRIFT"];
    /// The kinds that make up [`Self::abschlag_sum`].
    const ADVANCE: &'static [&'static str] = &["ABSCHLAG", "ABSCHLAG_VERRECHNUNG"];
    /// The kinds that make up [`Self::zahlung_sum`].
    const CASH: &'static [&'static str] = &["ZAHLUNG", "BANKRUECKLAST", "SEPA_STORNO"];

    #[must_use]
    pub fn from_kind_sums(sums: &std::collections::HashMap<String, i64>) -> Self {
        let bucket = |kinds: &[&str]| -> i64 {
            kinds
                .iter()
                .map(|k| sums.get(*k).copied().unwrap_or(0))
                .sum()
        };
        let rechnung_sum = bucket(Self::BILLED);
        let abschlag_sum = bucket(Self::ADVANCE);
        let zahlung_sum = bucket(Self::CASH);
        let verzugsschaden_sum = bucket(crate::pg::VERZUGSSCHADEN_KINDS);
        // The settlement is the whole year, so the residue is derived rather
        // than enumerated: whatever the four named buckets did not claim.
        let settlement_ct: i64 = sums.values().sum();
        Self {
            rechnung_sum,
            abschlag_sum,
            zahlung_sum,
            verzugsschaden_sum,
            sonstige_sum: settlement_ct
                - rechnung_sum
                - abschlag_sum
                - zahlung_sum
                - verzugsschaden_sum,
            settlement_ct,
        }
    }
}

/// A refusal from [`settle_jahresabschluss`], with the status it answers.
///
/// The settlement is driven by two callers — an operator's POST and the annual
/// worker — so it cannot answer in `axum` terms. This is the thin thing that
/// lets it keep saying *what* went wrong: a missing account is a `404`, an
/// unbuildable refund a `422`, an unconfigured creditor IBAN a `503`, and the
/// worker logs each with the same distinction rather than collapsing them into
/// "settlement failed".
#[derive(Debug)]
pub struct SettleError {
    pub status: StatusCode,
    pub body: serde_json::Value,
}

impl SettleError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: serde_json::json!({ "error": message.into() }),
        }
    }
    fn json(status: StatusCode, body: serde_json::Value) -> Self {
        Self { status, body }
    }
    /// Whether retrying later could succeed.
    ///
    /// A `5xx` is a database or a bank adapter having a bad moment; a `4xx` is
    /// this account's own state and will look the same tomorrow. The worker
    /// uses the distinction to decide between a warning and an error.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        self.status.is_server_error()
    }
}

impl std::fmt::Display for SettleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}",
            self.status,
            self.body
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("settlement refused")
        )
    }
}

/// `POST /api/v1/jahresabschluss/{malo_id}` — the operator-driven settlement.
///
/// A thin shell over [`settle_jahresabschluss`], which the annual worker also
/// drives. One implementation, because a settlement an operator triggers and
/// one a schedule triggers must produce the same postings, the same refund and
/// the same event — and a second copy of two hundred lines of money handling is
/// how they stop doing that.
pub async fn post_jahresabschluss(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<JahresabschlussQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "close-period", &cfg.tenant) {
        return forbidden(&e);
    }
    match settle_jahresabschluss(&pool, &ledger, &cfg, &malo_id, &q).await {
        Ok(body) => Json(body).into_response(),
        Err(e) => (e.status, Json(e.body)).into_response(),
    }
}

pub async fn settle_jahresabschluss(
    pool: &PgPool,
    ledger: &Arc<crate::ledger::PgLedger>,
    cfg: &Arc<AccountingdConfig>,
    malo_id: &str,
    q: &JahresabschlussQuery,
) -> Result<serde_json::Value, SettleError> {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    // The Abrechnungsjahr defaults to the current German calendar year.
    let year = q.year.unwrap_or_else(|| mako_fristen::heute().year());
    let dry_run = q.dry_run.unwrap_or(false);

    // 1. Resolve account.
    let acct = match fetch_account(pool, malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Err(SettleError::new(
                StatusCode::NOT_FOUND,
                format!("account for {malo_id} not found"),
            ));
        }
        Err(e) => {
            return Err(SettleError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
            ));
        }
    };

    // 2. Sum the year's movements by kind from the doubleentry ledger.
    // Values are signed Kontokorrent contributions (debit +, credit −).
    let sums = match ledger.year_kind_sums(lf_mp_id, malo_id, year).await {
        Ok(s) => s,
        Err(e) => {
            return Err(SettleError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
            ));
        }
    };
    let JahresabschlussSums {
        rechnung_sum,
        abschlag_sum,
        zahlung_sum,
        verzugsschaden_sum,
        sonstige_sum,
        settlement_ct,
    } = JahresabschlussSums::from_kind_sums(&sums);
    // New monthly Abschlag = actual annual billed ÷ 12 (§40 Abs. 1 EnWG).
    // Only update when there were actual Rechnungen this year; keep unchanged
    // for years with no billed amounts to avoid zeroing the Abschlag on empty years.
    // Mahngebühren and Verzugszinsen stay out of the base: § 40 Abs. 1 ties the
    // Abschlag to the expected *consumption*, and raising it because a customer
    // was dunned would make the next year's advances a second penalty.
    let new_abschlag_ct = if rechnung_sum.abs() > 0 {
        rechnung_sum.abs() / 12
    } else {
        acct.abschlag_ct
    };

    let action = if settlement_ct > 0 {
        "NACHZAHLUNG"
    } else if settlement_ct < 0 {
        "ERSTATTUNG"
    } else {
        "AUSGEGLICHEN"
    };

    if dry_run {
        return Ok(serde_json::json!({
            "malo_id": malo_id,
            "year": year,
            "rechnung_sum_ct": rechnung_sum,
            "abschlag_net_ct": abschlag_sum,
            "zahlung_net_ct": zahlung_sum,
            "verzugsschaden_ct": verzugsschaden_sum,
            "sonstige_ct": sonstige_sum,
            "settlement_ct": settlement_ct,
            "settlement_eur": format_ct_as_eur(settlement_ct),
            "new_monthly_abschlag_ct": new_abschlag_ct,
            "action": action,
            "dry_run": true,
            "committed": false,
        }));
    }

    // Idempotency: a Jahresabschluss for (tenant, malo, year) runs exactly once.
    // Re-invocation (retry, double-click, concurrent) returns the prior result
    // instead of writing a second settlement entry and re-recalibrating Abschlag.
    let billing_year_i16 = i16::try_from(year).unwrap_or(0);
    match jahresabschluss_already_settled(pool, &cfg.tenant, malo_id, billing_year_i16).await {
        Ok(Some(prior_ct)) => {
            return Ok(serde_json::json!({
                "malo_id": malo_id,
                "year": year,
                "settlement_ct": prior_ct,
                "settlement_eur": format_ct_as_eur(prior_ct),
                "committed": true,
                "already_settled": true,
            }));
        }
        Ok(None) => {}
        Err(e) => {
            return Err(SettleError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
            ));
        }
    }

    let today = mako_fristen::heute();
    // Deterministic idempotency key — the ledger post is keyed by it, so a
    // redelivery replays as a no-op returning the original settlement entry.
    let ce_id = format!("jahresabschluss:{malo_id}:{year}");

    // 3. Realise the settlement.
    //
    // The year's Kontokorrent movement already equals `settlement_ct` — the
    // gross RECHNUNG debits, less the ABSCHLAG_VERRECHNUNG the settling invoice
    // booked, plus the advances demanded, less the cash received — so a
    // **Nachzahlung** needs NO ledger entry: it is the open receivable,
    // collected by the SEPA/dunning path. An **Erstattung** (customer overpaid) is realised here: we book a
    // debit that clears the credit balance to zero and pay the money out via
    // pain.001 — but only when the account carries an IBAN. Without one the
    // credit is carried forward and offset against the next Rechnung
    // (§40 EnWG "Verrechnung mit der nächsten Abrechnung").
    let mut settlement_entry_id: Option<Uuid> = None;
    let mut refund_pain001: Option<String> = None;
    if settlement_ct < 0 {
        let refund_ct = -settlement_ct; // positive: amount owed to the customer
        match acct.iban.as_deref().filter(|i| !i.is_empty()) {
            Some(customer_iban) => {
                let creditor_name = cfg.creditor_name.as_deref().unwrap_or(&cfg.tenant);
                let creditor_iban = match cfg.creditor_iban.as_deref().filter(|s| !s.is_empty()) {
                    Some(i) => i,
                    None => {
                        return Err(SettleError::json(
                            StatusCode::SERVICE_UNAVAILABLE,
                            serde_json::json!({
                                "error": "Erstattung due but creditor_iban (payer account) not configured"
                            }),
                        ));
                    }
                };
                let e2e = format!("REFUND-{malo_id}-{year}");
                let customer_name = format!("Kunde {malo_id}");
                // `Cdtr/PstlAdr` — on a refund the customer is the creditor.
                let customer_address = acct.postal_address();
                let ct_schema =
                    match crate::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref()) {
                        Ok(s) => s,
                        Err(e) => {
                            return Err(SettleError::json(
                                StatusCode::SERVICE_UNAVAILABLE,
                                serde_json::json!({ "error": e.to_string() }),
                            ));
                        }
                    };
                // A refund leaves today: §40 EnWG gives no grace period for
                // paying back what the customer overpaid.
                match crate::sepa::build_pain_001(
                    &crate::sepa::DebtorIdentity {
                        iban: creditor_iban,
                        name: creditor_name,
                        address: Some(&cfg.creditor_address),
                    },
                    &[crate::sepa::CreditTransferItem {
                        iban: customer_iban,
                        name: &customer_name,
                        amount_ct: refund_ct,
                        end_to_end_ref: &e2e,
                        address: Some(&customer_address),
                    }],
                    today,
                    false,
                    ct_schema,
                ) {
                    Ok(xml) => refund_pain001 = Some(xml),
                    Err(e) => {
                        return Err(SettleError::json(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            serde_json::json!({ "error": format!("refund pain.001 build failed: {e}") }),
                        ));
                    }
                }
                // Clearing debit: zeroes the credit balance the refund pays out.
                // Idempotent by `ce_id` — if the satellite tx below fails, an
                // operator retry replays this as a no-op and completes the run.
                let desc = format!("Erstattung Jahresabschluss {year} (Auszahlung an Kunde)");
                match crate::pg::post_entry(
                    ledger,
                    pool,
                    &cfg.tenant,
                    malo_id,
                    lf_mp_id,
                    "JAHRESABSCHLUSS",
                    refund_ct,
                    &ce_id,
                    Some(&ce_id),
                    None,
                    today,
                    today,
                    Some(&desc),
                    None,
                )
                .await
                {
                    Ok(id) => settlement_entry_id = Some(id),
                    Err(e) => {
                        return Err(SettleError::new(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            e.to_string(),
                        ));
                    }
                }
            }
            None => {
                tracing::info!(
                    malo_id,
                    refund_ct,
                    "accountingd: Erstattung carried forward — account has no IBAN for refund"
                );
            }
        }
    }

    // Steps 3–5 commit atomically: the Jahresabschluss idempotency row, the
    // Abschlag update, and the refund CloudEvent (persist-before-dispatch) all
    // land in ONE transaction, so a delivery failure can never orphan the event
    // from the state it represents.
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            return Err(SettleError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
            ));
        }
    };

    // Record the run so a re-call is a no-op (audit + idempotency guard).
    if let Err(e) = record_jahresabschluss(
        &mut *tx,
        &cfg.tenant,
        malo_id,
        billing_year_i16,
        rechnung_sum,
        // What the customer contributed against the year's billing: the advances
        // still standing plus the cash actually received, plus whatever else
        // moved. `annual_bill + sum_abschlage == zahlbetrag` then holds in the
        // persisted run by construction rather than by hoping the buckets were
        // exhaustive.
        settlement_ct - rechnung_sum,
        settlement_ct,
        settlement_entry_id,
    )
    .await
    {
        tracing::error!(malo_id, error = %e, "accountingd: record_jahresabschluss failed");
        return Err(SettleError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }

    // 4. Update monthly Abschlag (§40 Abs. 1 EnWG: Abschlag must match actual consumption).
    if new_abschlag_ct != acct.abschlag_ct
        && let Err(e) = update_account_tenanted(
            &mut *tx,
            malo_id,
            lf_mp_id,
            &cfg.tenant,
            None,
            UpdateAccountRequest {
                iban: None,
                mandatsref: None,
                abschlag_ct: Some(new_abschlag_ct),
                billing_day: None,
                address: Default::default(),
            },
        )
        .await
    {
        tracing::warn!(
            malo_id,
            new_abschlag_ct,
            error = %e,
            "accountingd: Jahresabschluss Abschlag update failed"
        );
        return Err(SettleError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }

    // 5. Enqueue the refund CloudEvent for the ERP/bank (persist-before-dispatch);
    //    the outbox worker signs and delivers it after commit.
    if let Some(ref xml) = refund_pain001
        && cfg.erp_webhook_url.is_some()
    {
        let refund_ct = -settlement_ct;
        let ce = mako_service::CloudEvent::new(
            mako_service::source("accountingd", &cfg.tenant),
            mako_events::accounting::ERSTATTUNG_FAELLIG,
            "",
            serde_json::json!({
                "malo_id": malo_id,
                "year": year,
                "refund_ct": refund_ct,
                "pain001_xml": xml,
            }),
        )
        .with_id(format!("{ce_id}:refund"))
        .without_subject();
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            tracing::error!(malo_id, error = %e, "accountingd: outbox enqueue (erstattung) failed");
            return Err(SettleError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
            ));
        }
    }

    // 6. Announce the settlement — **unconditionally**, on every outcome.
    //
    // The refund event above fires only when the year produced a refund *and*
    // an ERP webhook is configured, so a Nachzahlung, a balanced year, and
    // every deployment without a webhook announced nothing at all: anything
    // downstream of the annual cycle had to poll `jahresabschluss_runs` to
    // notice it had happened. This is the completion signal, in the same
    // transaction as the settlement it reports, keyed on (MaLo, year) so the
    // idempotent re-run drops at the outbox.
    let done = mako_service::CloudEvent::new(
        mako_service::source("accountingd", &cfg.tenant),
        mako_events::accounting::JAHRESABSCHLUSS_ABGESCHLOSSEN,
        malo_id,
        serde_json::json!({
            "malo_id":                 malo_id,
            "lf_mp_id":                lf_mp_id,
            "year":                    year,
            "action":                  action,
            "settlement_ct":           settlement_ct,
            "settlement_eur":          format_ct_as_eur(settlement_ct),
            "rechnung_sum_ct":         rechnung_sum,
            "abschlag_net_ct":         abschlag_sum,
            "zahlung_net_ct":          zahlung_sum,
            "verzugsschaden_ct":       verzugsschaden_sum,
            "sonstige_ct":             sonstige_sum,
            "new_monthly_abschlag_ct": new_abschlag_ct,
            "refund_issued":           refund_pain001.is_some(),
        }),
    )
    .with_id(ce_id.clone());
    if let Err(e) = mako_service::outbox::enqueue(&mut tx, &done).await {
        tracing::error!(malo_id, error = %e, "accountingd: outbox enqueue (jahresabschluss) failed");
        return Err(SettleError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }

    if let Err(e) = tx.commit().await {
        return Err(SettleError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }

    Ok(serde_json::json!({
        "malo_id": malo_id,
        "year": year,
        "rechnung_sum_ct": rechnung_sum,
        "abschlag_net_ct": abschlag_sum,
        "zahlung_net_ct": zahlung_sum,
        "verzugsschaden_ct": verzugsschaden_sum,
        "sonstige_ct": sonstige_sum,
        "settlement_ct": settlement_ct,
        "settlement_eur": format_ct_as_eur(settlement_ct),
        "new_monthly_abschlag_ct": new_abschlag_ct,
        "action": action,
        "refund_issued": refund_pain001.is_some(),
        "dry_run": false,
        "committed": true,
        "ce_id": ce_id,
    }))
}

// ── Zahlungsinformation (BO4E typed payment info — IBAN + BIC + SEPA) ────────

/// Query param helper for Zahlungsinformation endpoints.
#[derive(Debug, serde::Deserialize)]
pub struct ZahlungsQuery {
    pub lf_mp_id: Option<String>,
}

/// `PUT /api/v1/accounts/{malo_id}/zahlungsinformation`
///
/// Store or replace the BO4E `Zahlungsinformation` COM for an account.
///
/// Body: `rubo4e::current::Zahlungsinformation` JSON (camelCase).
/// Accepts: `iban`, `bic`, `kontoinhaber`, `sepaReferenz`, `zahlungsart`.
///
/// Side effects:
/// - Validates IBAN via mod-97 before storing.
/// - Atomically syncs `accounts.iban` column from `typed.iban` so that
///   `import_payments` (CAMT.054) matching continues to work.
pub async fn put_zahlungsinformation(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ZahlungsQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-account", &cfg.tenant) {
        return forbidden(&e);
    }
    use rubo4e::current::Zahlungsinformation;
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();

    // The BO4E gate. Its strict-enum stage is what keeps `zahlungsart` — which
    // drives the SEPA collection path — from degrading to `Unknown` and being
    // stored as a mandate instruction nobody can act on.
    let typed: Zahlungsinformation = match mako_markt::bo4e::decode(body) {
        Ok(z) => z,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(e.to_json())).into_response(),
    };

    // Validate IBAN when present.
    if let Some(ref iban) = typed.iban
        && let Err(msg) = validate_iban(iban)
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("invalid IBAN: {msg}") })),
        )
            .into_response();
    }

    let canonical = match serde_json::to_value(&typed) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "validated Zahlungsinformation is not serialisable");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "could not serialise Zahlungsinformation" })),
            )
                .into_response();
        }
    };

    // Ensure account row exists.
    let account_id = match upsert_account(&pool, &malo_id, &lf_mp_id, &cfg.tenant).await {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Store typed Zahlungsinformation JSON + sync iban column for payment matching.
    let iban_to_sync = typed.iban.clone();
    let bic_to_sync = typed.bic.clone();
    let res = sqlx::query(
        r"UPDATE accounts
          SET zahlungsinformation = $1,
              iban = COALESCE($2, iban),
              updated_at = now()
          WHERE account_id = $3",
    )
    .bind(&canonical)
    .bind(&iban_to_sync)
    .bind(account_id)
    .execute(&pool)
    .await;

    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id": malo_id,
                "iban": iban_to_sync,
                "bic": bic_to_sync,
                "zahlungsinformation": canonical,
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/zahlungsinformation`
///
/// Retrieve the stored `Zahlungsinformation` for an account.
/// Falls back to a minimal object from `accounts.iban` when no typed payload has
/// been PUT yet (backward-compatible with legacy IBAN-only mandates).
pub async fn get_zahlungsinformation(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ZahlungsQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-banking", &cfg.tenant) {
        return forbidden(&e);
    }
    use rubo4e::current::Zahlungsinformation;
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let row = sqlx::query(
        "SELECT iban, zahlungsinformation FROM accounts \
         WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3 LIMIT 1",
    )
    .bind(&malo_id)
    .bind(lf_mp_id)
    .bind(&cfg.tenant)
    .fetch_optional(&pool)
    .await;

    match row {
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(Some(row)) => {
            let typed_json: Option<serde_json::Value> =
                row.try_get("zahlungsinformation").ok().flatten();
            let iban: Option<String> = row.try_get("iban").ok().flatten();
            let payload = if let Some(json) = typed_json {
                json
            } else if let Some(ref iban_str) = iban {
                // Synthesise minimal Zahlungsinformation from legacy iban column.
                let z = Zahlungsinformation {
                    iban: Some(iban_str.clone()),
                    ..Default::default()
                };
                serde_json::to_value(&z).unwrap_or_default()
            } else {
                return StatusCode::NOT_FOUND.into_response();
            };
            Json(payload).into_response()
        }
    }
}

#[cfg(test)]
mod iban_tests {
    use super::validate_iban;

    #[test]
    fn valid_de_iban() {
        assert!(validate_iban("DE89 3704 0044 0532 0130 00").is_ok());
        assert!(validate_iban("DE89370400440532013000").is_ok());
    }

    #[test]
    fn valid_gb_iban() {
        assert!(validate_iban("GB29 NWBK 6016 1331 9268 19").is_ok());
    }

    #[test]
    fn wrong_checksum() {
        assert!(validate_iban("DE89 3704 0044 0532 0130 01").is_err());
    }

    #[test]
    fn too_short() {
        assert!(validate_iban("DE89").is_err());
    }
}

// ── Open-item management ────────────────────────────────────────────────

/// `GET /api/v1/accounts/{malo_id}/open-items`
///
/// Returns all unpaid or partially-paid debit entries for this account,
/// computed via **FIFO clearing** of available credits against oldest debits.
///
/// ## What is an "open item"?
///
/// An open item (Offener Posten) is an individual RECHNUNG, STORNO, MAHNGEBUEHR,
/// or ABSCHLAG debit that has not been fully covered by ZAHLUNG/GUTSCHRIFT credits.
///
/// ## Why not just use `balance_ct`?
///
/// `balance_ct` tells you the total outstanding amount but not *which* invoices
/// are unpaid. Open-item management answers: "Invoice R2026-01 is unpaid;
/// Invoice R2025-12 is partially paid (€42 remaining)."
///
/// ## FIFO clearing
///
/// Payments are applied to the oldest debits first. This matches:
/// - Standard European utility billing practice (§252 HGB Vorsichtsprinzip)
/// - SAP FI-CA default (oldest-first clearing)
///
/// ## Response
///
/// ```json
/// [
///   { "id": "...", "entry_type": "RECHNUNG", "amount_ct": 15000,
///     "outstanding_ct": 7500, "reference_id": "R2026-01",
///     "booking_date": "2026-01-15", "description": "Kundenrechnung" }
/// ]
/// ```
pub async fn get_open_items(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match crate::pg::list_open_items(&ledger, lf_mp_id, &malo_id).await {
        Ok(items) => Json(serde_json::json!({
            "malo_id": malo_id,
            "balance_ct": account.balance_ct,
            "balance_eur": format_ct_as_eur(account.balance_ct),
            "open_item_count": items.len(),
            "open_items": items,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GDPR Art. 17 anonymization ─────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct AnonymizeRequest {
    /// Operator identity for the GDPR Art. 5(2) audit log.
    pub requested_by: String,
    /// Legal basis for erasure (e.g. `"GDPR Art. 17 - customer request #42"`).
    pub legal_basis: String,
}

/// `POST /api/v1/accounts/{malo_id}/anonymize`
///
/// Pseudonymize all PII for an account while preserving financial records.
///
/// ## What is anonymized
///
/// - `accounts.iban` → `"ANONYMIZED"`
/// - `accounts.mandatsref` → `NULL`
/// - `accounts.zahlungsinformation` → `NULL`
/// - `accounts.vorauszahlung` → `NULL`
/// - `sepa_mandates.iban` → `"ANONYMIZED"`
/// - `sepa_mandates.kontoinhaber` → `"ANONYMIZED"`
/// - `sepa_mandates.bic` → `NULL`
///
/// ## What is preserved
///
/// All `ledger_entries` (amounts, dates, types, references) are kept intact.
/// `malo_id` is retained (location pseudonym, not personal data per BDEW).
/// Financial records are exempt from GDPR Art. 17 erasure under Art. 17(3)(b)
/// and §238 HGB / §147 AO retention requirements.
///
/// ## Audit trail
///
/// An immutable record is written to `anonymization_log` for GDPR Art. 5(2)
/// accountability.
///
/// ## Error responses
///
/// - `404` — account not found
/// - `409` — already anonymized
/// - `422` — missing `requested_by` or `legal_basis`
pub async fn post_anonymize(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
    Json(req): Json<AnonymizeRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "erase-pii", &cfg.tenant) {
        return forbidden(&e);
    }
    if req.requested_by.is_empty() || req.legal_basis.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "requested_by and legal_basis are required for GDPR audit trail"
            })),
        )
            .into_response();
    }

    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    match crate::pg::anonymize_account(
        &pool,
        account.account_id,
        &cfg.tenant,
        &req.requested_by,
        &req.legal_basis,
    )
    .await
    {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) if e.to_string().contains("already anonymized") => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "account already anonymized" })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Balance reconciliation ────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ReconcileQuery {
    pub lf_mp_id: Option<String>,
    /// When `true`, resets `balance_ct` to the recomputed value (safe, transactional).
    pub repair: Option<bool>,
}

/// `POST /api/v1/accounts/{malo_id}/reconcile`
///
/// Detect (and optionally repair) a `balance_ct` cache drift.
///
/// `balance_ct` is a denormalized cache maintained by the `write_entry` transaction.
/// A crash between `INSERT ledger_entries` and `UPDATE accounts SET balance_ct` could
/// leave the cache stale. This endpoint detects the drift and, with `?repair=true`,
/// atomically resets `balance_ct` to `SUM(ledger_entries.amount_ct)`.
///
/// ## Response
///
/// ```json
/// {
///   "is_consistent": true,
///   "cached_balance_ct": 5000,
///   "recomputed_balance_ct": 5000,
///   "drift_ct": 0
/// }
/// ```
///
/// A non-zero `drift_ct` indicates a bug and must be investigated before repair.
/// This endpoint is idempotent: running it multiple times with `repair=true` is safe.
pub async fn post_reconcile(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ReconcileQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    if let Ok(None) = fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    let repair = q.repair.unwrap_or(false);
    match crate::pg::reconcile_balance(&ledger, &pool, &malo_id, lf_mp_id, &cfg.tenant, repair)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Festschreibung (period seals) + audit proofs — GoBD / § 146 AO / § 239 HGB ──

/// Body for `POST /api/v1/periods/{period_id}/seal`.
#[derive(Debug, serde::Deserialize)]
pub struct SealPeriodRequest {
    /// First day of the period (inclusive, ISO 8601).
    pub start: String,
    /// Last day of the period (inclusive, ISO 8601).
    pub end: String,
}

fn seal_json(seal: &doubleentry::Seal) -> serde_json::Value {
    serde_json::json!({
        "ledger": seal.ledger.as_str(),
        "period": seal.period.as_str(),
        "seal_hash": seal.seal_hash.to_string(),
        "tree_root": seal.tree_head.root.to_string(),
        "tree_size": seal.tree_head.size,
        // Roots travel with their sizes. A Merkle root on its own does not fix
        // which tree it is the root of, so a proof checked against a bare root
        // can be replayed against a different tree; every commitment a seal
        // publishes is therefore published as a (root, size) pair.
        "trial_balance_root": seal.trial_balance.root.to_string(),
        "trial_balance_size": seal.trial_balance.size,
        // What the trial balance's account handles meant at sealing time.
        // Without it the handles float and every balance in the seal could
        // silently refer to a different account.
        "accounts_root": seal.accounts.root.to_string(),
        "accounts_size": seal.accounts.size,
        "entry_count": seal.entry_count,
        "first_index": seal.first_index,
        "last_index": seal.last_index,
        "prev_seal": seal.prev_seal.map(|h| h.to_string()),
    })
}

/// `POST /api/v1/periods/{period_id}/seal` — Festschreibung.
///
/// Closes and seals a period, committing to its entries and closing balances as
/// chained Merkle roots. After sealing, a backdated booking into the period is
/// rejected — corrections book into a later open period (§ 146 Abs. 4 AO).
pub async fn post_seal_period(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Path(period_id): Path<String>,
    Json(req): Json<SealPeriodRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "close-period", &cfg.tenant) {
        return forbidden(&e);
    }
    use time::format_description::well_known::Iso8601;
    let (Ok(start), Ok(end)) = (
        time::Date::parse(&req.start, &Iso8601::DEFAULT),
        time::Date::parse(&req.end, &Iso8601::DEFAULT),
    ) else {
        return (StatusCode::BAD_REQUEST, "invalid start/end date (ISO 8601)").into_response();
    };
    match ledger.seal_period(&period_id, start, end).await {
        Ok(seal) => (StatusCode::CREATED, Json(seal_json(&seal))).into_response(),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/periods/seals` — the Festschreibung history, with chain verification.
pub async fn get_seals(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    let seals = match ledger.seals().await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let verified = ledger.verify_seals().await;
    Json(serde_json::json!({
        "count": seals.len(),
        "chain_valid": verified.is_ok(),
        "verify_error": verified.err().map(|e| e.to_string()),
        // The watermark, not the period list, decides what may still be booked:
        // every date at or before it is sealed whether or not a period covers
        // it. An operator asking "is February still open" needs this.
        "sealed_through": ledger.sealed_through().map(|d| d.to_string()),
        "seals": seals.iter().map(seal_json).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// `GET /api/v1/entries/{entry_id}/proof` — a Merkle inclusion proof that the
/// entry is committed to by the current head (tamper-evidence for an auditor).
pub async fn get_entry_proof(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Path(entry_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    let Ok(uuid) = Uuid::parse_str(&entry_id) else {
        return (StatusCode::BAD_REQUEST, "invalid entry_id").into_response();
    };
    match ledger
        .prove_entry(doubleentry::EntryId::from_uuid(uuid))
        .await
    {
        Ok((content_hash, proof, head)) => {
            // Against the whole head, never the bare root: a proof is only
            // meaningful for a stated tree size. Checked against a root alone,
            // a genuine proof for one leaf verifies unchanged as a proof for a
            // different leaf of a differently sized log.
            let verified = proof.verify(&content_hash, &head);
            Json(serde_json::json!({
                "entry_id": entry_id,
                "content_hash": content_hash.to_string(),
                "tree_size": head.size,
                "tree_root": head.root.to_string(),
                "verified": verified,
                "proof": serde_json::to_value(&proof).unwrap_or(serde_json::Value::Null),
            }))
            .into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// Query for `GET /api/v1/periods/{period_id}/balance-proof`.
#[derive(Debug, serde::Deserialize)]
pub struct BalanceProofQuery {
    /// Marktlokation whose Kontokorrent balance is being proven.
    pub malo_id: String,
    /// The Lieferant the Kontokorrent belongs to.
    pub lf_mp_id: String,
}

/// `GET /api/v1/periods/{period_id}/balance-proof` — proves what one customer's
/// account closed at, for a sealed period.
///
/// The Betriebsprüfung question (§ 147 AO / GoBD) is not "is this booking in the
/// books" but "what did this customer owe at the balance-sheet date". A figure
/// read out of a table is not evidence for that; this returns the two chained
/// Merkle proofs that are.
pub async fn get_period_balance_proof(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Path(period_id): Path<String>,
    Query(q): Query<BalanceProofQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    let outcome = match ledger
        .prove_period_balance(&period_id, &q.lf_mp_id, &q.malo_id)
        .await
    {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // Two ways to have nothing to prove, and they are different sentences. The
    // books are intact in both, so neither is an error — but "had no movement
    // that period" and "was not a customer yet" answer an auditor differently,
    // and collapsing them would invite reading the second as the first.
    let proof = match outcome {
        doubleentry::SealedBalanceOutcome::Proven(proof) => *proof,
        doubleentry::SealedBalanceOutcome::NoRow => {
            return Json(serde_json::json!({
                "period": period_id,
                "malo_id": q.malo_id,
                "lf_mp_id": q.lf_mp_id,
                "absent": true,
                "reason": "no_row",
                // Absent is not zero. A seal's closing balances are cumulative
                // as of the period's last day, so this means nothing was booked
                // on or before that date — not merely that the period itself was
                // quiet. There is nothing the seal committed to, and no proof
                // may be manufactured.
                "detail": "the account was nameable when the period closed but has \
                           nothing booked on or before its last day, so the seal \
                           committed to no balance for it — this is not a proven zero",
            }))
            .into_response();
        }
        doubleentry::SealedBalanceOutcome::NotYetRegistered => {
            return Json(serde_json::json!({
                "period": period_id,
                "malo_id": q.malo_id,
                "lf_mp_id": q.lf_mp_id,
                "absent": true,
                "reason": "not_yet_registered",
                "detail": "the account did not exist when the period sealed — \
                           a seal names the handles the registry had issued by then, \
                           so it cannot speak about this account at all",
            }))
            .into_response();
        }
    };

    // Verified here before it is published: handing out a proof that does not
    // check is worse than returning nothing, because it looks like evidence.
    if !proof.verify() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "constructed balance proof does not verify against its seal"
            })),
        )
            .into_response();
    }

    let balance = &proof.balance.balance;
    Json(serde_json::json!({
        "period": period_id,
        "malo_id": q.malo_id,
        "lf_mp_id": q.lf_mp_id,
        "account": proof.path().to_string(),
        "debits_ct": balance.debits.to_minor(),
        "credits_ct": balance.credits.to_minor(),
        // Positive = the customer owes, negative = the customer is owed.
        "balance_ct": balance.debits.to_minor() - balance.credits.to_minor(),
        "verified": true,
        "seal": seal_json(&proof.seal),
        // The whole bundle, verbatim. A recipient deserialises this back into a
        // `doubleentry::SealedBalance` and calls `verify()` themselves — the
        // flattened fields above are for reading, this is the evidence. An
        // edited seal fails to deserialise at all, so someone who never calls
        // `verify` is not fooled either.
        "sealed_balance": serde_json::to_value(&proof).unwrap_or(serde_json::Value::Null),
    }))
    .into_response()
}

/// Query for `GET /api/v1/entries/consistency-proof`.
#[derive(Debug, serde::Deserialize)]
pub struct ConsistencyQuery {
    /// The tree size the auditor archived on an earlier visit.
    pub since: u64,
}

/// `GET /api/v1/entries/consistency-proof?since=N` — proves the journal has only
/// been appended to since it held `N` entries.
///
/// An inclusion proof on its own says the ledger is internally consistent *now*,
/// which a rebuilt ledger would satisfy just as well. This is the half that makes
/// the log append-only in the eyes of someone who was here before: they check it
/// against the head they archived and the head returned here.
pub async fn get_consistency_proof(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Query(q): Query<ConsistencyQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    match ledger.prove_append_only(q.since).await {
        Ok((proof, then, now)) => Json(serde_json::json!({
            "archived_size": then.size,
            "archived_root": then.root.to_string(),
            "current_size": now.size,
            "current_root": now.root.to_string(),
            "verified": proof.verify(&then, &now),
            "proof": serde_json::to_value(&proof).unwrap_or(serde_json::Value::Null),
        }))
        .into_response(),
        // `since=0` lands here. Every log extends the empty tree, so a proof
        // from it verifies against any root of the right size — correct
        // mathematics and a trap, because `verified: true` from a check that
        // examined nothing is indistinguishable from a real verification. The
        // ledger refuses to build one; an auditor gets the refusal, not
        // reassurance.
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Trial balance + open-item clearing (Zahlungszuordnung) ────────────────────

/// `GET /api/v1/trial-balance` — Summen- und Saldenliste (§ 238 HGB): gross debit
/// and credit turnover and the balance per GL account. Debits must equal credits.
pub async fn get_trial_balance(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    match ledger.trial_balance().await {
        Ok(lines) => {
            let total_debits: i64 = lines.iter().map(|l| l.debits_ct).sum();
            let total_credits: i64 = lines.iter().map(|l| l.credits_ct).sum();
            Json(serde_json::json!({
                "lines": lines,
                "total_debits_ct": total_debits,
                "total_credits_ct": total_credits,
                "balanced": total_debits == total_credits,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/accounts/{malo_id}/clear` — record a FIFO **Zahlungszuordnung**
/// (open credits matched against the oldest open debits). Idempotent: matches
/// nothing when everything is already assigned.
pub async fn post_clear(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    if let Ok(None) = fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    let today = mako_fristen::heute();
    match ledger.apply_fifo_clearing(lf_mp_id, &malo_id, today).await {
        Ok(Some(id)) => Json(serde_json::json!({
            "cleared": true,
            "clearing_id": id.to_string(),
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({
            "cleared": false,
            "message": "nothing left to match",
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/clearings/{clearing_id}/reset` — release a Zahlungszuordnung; the
/// applied amounts return to the postings' residuals (the original record stays).
pub async fn post_reset_clearing(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Path(clearing_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }
    let Ok(uuid) = Uuid::parse_str(&clearing_id) else {
        return (StatusCode::BAD_REQUEST, "invalid clearing_id").into_response();
    };
    let today = mako_fristen::heute();
    match ledger
        .reset_clearing(doubleentry::clearing::ClearingId::from_uuid(uuid), today)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "reset": true })).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── §25 EEG 2023 — SEPA Credit Transfer payout pipeline ──────────────────────
//
// When `de.eeg.verguetung.berechnet` is ingested by the webhook handler:
//  1. EEG_GUTSCHRIFT ledger entry is created (credit, negative amount_ct)
//  2. If cfg.eeg.auto_payout = true: pain.001 is generated, inserted into
//     eeg_payout_orders, and optionally submitted to the bank adapter.
//
// Operators can also trigger a batch run via POST /api/v1/eeg/payouts/run,
// list payout orders, and process pain.002 status reports from the bank.
//
// Regulatory basis:
// - §25 Abs. 1 EEG 2023: Vergütung credited "unverzüglich nach Ende des Monats"
// - EU Regulation 2024/886: SCT Inst mandatory for all PSPs from Oct 2025
// - ISO 20022 pain.001.001.09 (SCT Inst) / pain.001.003.03 (SCT CORE)

/// Query parameters for `GET /api/v1/eeg/payouts`.
#[derive(Debug, serde::Deserialize)]
pub struct EegPayoutQuery {
    pub malo_id: Option<String>,
    pub year: Option<i16>,
    pub month: Option<i16>,
    /// Filter by pain002_status: PDNG | ACCP | RJCT | CANC | NULL (not yet submitted)
    pub status: Option<String>,
    pub payment_type: Option<String>,
}

/// `GET /api/v1/eeg/payouts` — list EEG payout orders with optional filters.
///
/// Returns all `eeg_payout_orders` rows for the tenant, newest first.
/// Use `?status=PDNG` to find orders awaiting pain.002 confirmation, or
/// `?status=RJCT` to audit rejected payments (EPC rejection codes).
pub async fn get_eeg_payouts(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<EegPayoutQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-banking", &cfg.tenant) {
        return forbidden(&e);
    }
    // Dynamic WHERE clause built from optional filters.
    let mut conditions = vec!["tenant = $1".to_owned()];
    let mut params: Vec<String> = vec![cfg.tenant.clone()];
    let mut idx = 2usize;

    if let Some(ref malo) = q.malo_id {
        conditions.push(format!("malo_id = ${idx}"));
        params.push(malo.clone());
        idx += 1;
    }
    if let Some(y) = q.year {
        conditions.push(format!("billing_year = ${idx}"));
        params.push(y.to_string());
        idx += 1;
    }
    if let Some(m) = q.month {
        conditions.push(format!("billing_month = ${idx}"));
        params.push(m.to_string());
        idx += 1;
    }
    if let Some(ref pt) = q.payment_type {
        conditions.push(format!("payment_type = ${idx}"));
        params.push(pt.clone());
        idx += 1;
    }
    if let Some(ref s) = q.status {
        if s == "NULL" || s == "NOTSUBMITTED" {
            conditions.push("pain002_status IS NULL".to_owned());
        } else {
            conditions.push(format!("pain002_status = ${idx}"));
            params.push(s.clone());
            // idx += 1; (not used further)
        }
    }

    let sql = format!(
        "SELECT payout_id, malo_id, tr_id, billing_year, billing_month, \
                amount_ct, creditor_iban, creditor_name, payment_type, \
                end_to_end_ref, pain002_status, pain002_reason, \
                submitted_at, settled_at, source_ce_id, created_at \
         FROM eeg_payout_orders \
         WHERE {} \
         ORDER BY created_at DESC LIMIT 200",
        conditions.join(" AND ")
    );

    // Build dynamic query — sqlx doesn't support $n-parameterised queries with
    // dynamic bind count via the macro path; use the builder API.
    let mut q_builder = sqlx::query(&sql);
    for p in &params {
        q_builder = q_builder.bind(p);
    }

    match q_builder.fetch_all(&pool).await {
        Ok(rows) => {
            let result: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    use sqlx::Row;
                    let submitted_at: Option<time::OffsetDateTime> =
                        r.try_get("submitted_at").unwrap_or(None);
                    let settled_at: Option<time::OffsetDateTime> =
                        r.try_get("settled_at").unwrap_or(None);
                    let created_at: time::OffsetDateTime = r.get("created_at");
                    serde_json::json!({
                        "payout_id":     r.get::<uuid::Uuid, _>("payout_id").to_string(),
                        "malo_id":       r.get::<String, _>("malo_id"),
                        "tr_id":         r.try_get::<String, _>("tr_id").ok(),
                        "billing_year":  r.get::<i16, _>("billing_year"),
                        "billing_month": r.get::<i16, _>("billing_month"),
                        "amount_ct":     r.get::<i64, _>("amount_ct"),
                        "creditor_iban": r.get::<String, _>("creditor_iban"),
                        "creditor_name": r.get::<String, _>("creditor_name"),
                        "payment_type":  r.get::<String, _>("payment_type"),
                        "end_to_end_ref": r.get::<String, _>("end_to_end_ref"),
                        "pain002_status": r.try_get::<String, _>("pain002_status").ok(),
                        "pain002_reason": r.try_get::<String, _>("pain002_reason").ok(),
                        "submitted_at":  submitted_at.map(|t| t.to_string()),
                        "settled_at":    settled_at.map(|t| t.to_string()),
                        "source_ce_id":  r.try_get::<String, _>("source_ce_id").ok(),
                        "created_at":    created_at.to_string(),
                    })
                })
                .collect();
            Json(serde_json::json!({ "payouts": result, "count": result.len() })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/eeg/payouts/{payout_id}` — get a single payout order with pain.001 XML.
pub async fn get_eeg_payout(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(payout_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-banking", &cfg.tenant) {
        return forbidden(&e);
    }
    use sqlx::Row;
    let row =
        match sqlx::query("SELECT * FROM eeg_payout_orders WHERE payout_id = $1 AND tenant = $2")
            .bind(payout_id)
            .bind(&cfg.tenant)
            .fetch_optional(&pool)
            .await
        {
            Ok(Some(r)) => r,
            Ok(None) => return (StatusCode::NOT_FOUND, "payout not found").into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    let submitted_at: Option<time::OffsetDateTime> = row.try_get("submitted_at").unwrap_or(None);
    let settled_at: Option<time::OffsetDateTime> = row.try_get("settled_at").unwrap_or(None);
    let created_at: time::OffsetDateTime = row.get("created_at");

    Json(serde_json::json!({
        "payout_id":     row.get::<uuid::Uuid, _>("payout_id").to_string(),
        "malo_id":       row.get::<String, _>("malo_id"),
        "tr_id":         row.try_get::<String, _>("tr_id").ok(),
        "billing_year":  row.get::<i16, _>("billing_year"),
        "billing_month": row.get::<i16, _>("billing_month"),
        "amount_ct":     row.get::<i64, _>("amount_ct"),
        "creditor_iban": row.get::<String, _>("creditor_iban"),
        "creditor_name": row.get::<String, _>("creditor_name"),
        "payment_type":  row.get::<String, _>("payment_type"),
        "end_to_end_ref": row.get::<String, _>("end_to_end_ref"),
        "pain001_xml":   row.try_get::<String, _>("pain001_xml").ok(),
        "pain002_status": row.try_get::<String, _>("pain002_status").ok(),
        "pain002_reason": row.try_get::<String, _>("pain002_reason").ok(),
        "submitted_at":  submitted_at.map(|t| t.to_string()),
        "settled_at":    settled_at.map(|t| t.to_string()),
        "source_ce_id":  row.try_get::<String, _>("source_ce_id").ok(),
        "created_at":    created_at.to_string(),
    }))
    .into_response()
}

/// Request body for `POST /api/v1/eeg/payouts/run`.
#[derive(Debug, serde::Deserialize)]
pub struct RunEegPayoutsRequest {
    /// When `true`, force SCT Inst regardless of `[eeg].sepa_instant` config.
    /// When `false` (default), use the config flag.
    pub instant_override: Option<bool>,
    /// Only generate payouts for this specific MaLo (for targeted re-run).
    pub malo_id: Option<String>,
    /// Only generate payouts for this year (defaults to current month's year).
    pub billing_year: Option<i16>,
    /// Only generate payouts for this month (defaults to current month).
    pub billing_month: Option<i16>,
}

/// `POST /api/v1/eeg/payouts/run`
///
/// Batch-generate SEPA pain.001 XML for all `EEG_GUTSCHRIFT` ledger entries
/// that do not yet have a corresponding `eeg_payout_orders` row.
///
/// This is the operator-triggered batch path. The auto-path runs per-CE when
/// `[eeg].auto_payout = true`.
///
/// Returns a summary JSON with `generated`, `skipped_no_iban`, `errors`.
pub async fn post_run_eeg_payouts(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(req): Json<RunEegPayoutsRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "run-payout", &cfg.tenant) {
        return forbidden(&e);
    }
    use crate::sepa::build_pain_001;

    let debtor_iban = match cfg.eeg.debtor_iban.as_deref() {
        Some(iban) => iban.to_owned(),
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "EEG payout requires [eeg].debtor_iban in config"
                })),
            )
                .into_response();
        }
    };

    let use_instant = req.instant_override.unwrap_or(cfg.eeg.sepa_instant);
    let today = mako_fristen::heute();
    let year = req.billing_year.unwrap_or(today.year() as i16);
    let month = req.billing_month.unwrap_or(today.month() as i16);

    // EEG_GUTSCHRIFT entries booked in the period, from the ledger. Re-runs are
    // idempotent via ON CONFLICT (end_to_end_ref) below, so no exclusion query is
    // needed — an already-paid entry simply produces no new order.
    let candidates = match ledger
        .entries_of_kind_in_month("EEG_GUTSCHRIFT", year as i32, month as u8)
        .await
    {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let mut generated = 0usize;
    let mut skipped_no_iban = 0usize;
    let mut errors = 0usize;

    for cand in &candidates {
        let malo_id = cand.malo_id.clone();
        if let Some(ref only) = req.malo_id
            && *only != malo_id
        {
            continue;
        }
        let amount_ct = cand.amount_ct.abs();
        let ce_id = cand.correlation.clone();

        // Fetch the operator account (account_id for the order row + payout IBAN).
        let account = match fetch_account(&pool, &malo_id, &cand.lf_mp_id, &cfg.tenant).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                errors += 1;
                continue;
            }
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let account_id = account.account_id;
        let zahlungsinformation: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT zahlungsinformation FROM accounts WHERE account_id = $1")
                .bind(account_id)
                .fetch_one(&pool)
                .await
                .unwrap_or(None);

        // Extract creditor IBAN from account's zahlungsinformation.
        let creditor_iban = zahlungsinformation
            .as_ref()
            .and_then(|z| z.get("bankverbindung"))
            .and_then(|b| b.get("iban"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let creditor_name = zahlungsinformation
            .as_ref()
            .and_then(|z| z.get("kontoinhaber"))
            .and_then(|v| v.as_str())
            .unwrap_or("EEG Einspeiser")
            .to_owned();

        let Some(creditor_iban) = creditor_iban else {
            skipped_no_iban += 1;
            continue;
        };

        // Build unique EndToEndId (max 35 chars, ISO 20022)
        let e2e_ref = format!(
            "EEG-{}-{year:04}-{month:02}-{}",
            &malo_id[..malo_id.len().min(10)],
            ce_id
                .as_deref()
                .and_then(|s| s.get(..8))
                .unwrap_or("MANUAL")
        );

        let payment_type = if use_instant { "SCT_INST" } else { "SCT_CORE" };

        let debtor_name = cfg.creditor_name.as_deref().unwrap_or(&cfg.tenant);
        let ct_schema = match crate::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(malo_id, error = %e, "accountingd: pain.001 schema config invalid");
                errors += 1;
                continue;
            }
        };
        // §25 Abs. 1 EEG 2023 — "unverzüglich nach Ende des Monats": the payout
        // leaves today, as the German banking calendar counts it. The crate's
        // default execution date is not something a payment date should be
        // inherited from.
        let execution_date = today;
        let creditor_address = account.postal_address();
        let pain_xml = match build_pain_001(
            &crate::sepa::DebtorIdentity {
                iban: &debtor_iban,
                name: debtor_name,
                address: Some(&cfg.creditor_address),
            },
            &[crate::sepa::CreditTransferItem {
                iban: &creditor_iban,
                name: &creditor_name,
                amount_ct,
                end_to_end_ref: &e2e_ref,
                address: Some(&creditor_address),
            }],
            execution_date,
            use_instant,
            ct_schema,
        ) {
            Ok(xml) => xml,
            Err(e) => {
                tracing::warn!(malo_id, error = %e, "accountingd: pain.001 build failed");
                errors += 1;
                continue;
            }
        };

        // Insert payout order (idempotent via unique source_ce_id). The creditor
        // address is snapshotted alongside the IBAN and name: what was sent has
        // to stay readable even after the account's master data moves on.
        let insert_result = sqlx::query(
            r"INSERT INTO eeg_payout_orders
                  (malo_id, account_id, billing_year, billing_month, amount_ct,
                   creditor_iban, creditor_name, payment_type, end_to_end_ref,
                   pain001_xml, source_ce_id, tenant,
                   creditor_town, creditor_country, creditor_street,
                   creditor_building_number, creditor_post_code,
                   creditor_country_subdivision)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
              ON CONFLICT (end_to_end_ref) DO NOTHING",
        )
        .bind(&malo_id)
        .bind(account_id)
        .bind(year)
        .bind(month)
        .bind(amount_ct)
        .bind(&creditor_iban)
        .bind(&creditor_name)
        .bind(payment_type)
        .bind(&e2e_ref)
        .bind(&pain_xml)
        .bind(ce_id.as_deref())
        .bind(&cfg.tenant)
        .bind(&creditor_address.town)
        .bind(&creditor_address.country)
        .bind(&creditor_address.street)
        .bind(&creditor_address.building_number)
        .bind(&creditor_address.post_code)
        .bind(&creditor_address.country_subdivision)
        .execute(&pool)
        .await;

        match insert_result {
            Ok(r) if r.rows_affected() > 0 => {
                // If bank_submit_url configured, submit immediately.
                if let Some(ref url) = cfg.eeg.bank_submit_url {
                    submit_pain001_to_bank(
                        url,
                        cfg.eeg.bank_api_key.as_deref(),
                        &pain_xml,
                        &e2e_ref,
                        &pool,
                        &cfg.tenant,
                    )
                    .await;
                }
                generated += 1;
            }
            Ok(_) => { /* already exists — skip */ }
            Err(e) => {
                tracing::warn!(malo_id, error = %e, "accountingd: eeg_payout_orders insert failed");
                errors += 1;
            }
        }
    }

    Json(serde_json::json!({
        "billing_year":  year,
        "billing_month": month,
        "payment_type":  if use_instant { "SCT_INST" } else { "SCT_CORE" },
        "generated":     generated,
        "skipped_no_iban": skipped_no_iban,
        "errors":        errors,
    }))
    .into_response()
}

/// Request body for `PUT /api/v1/eeg/payouts/{payout_id}/status`
///
/// Process a pain.002 Payment Status Report from the bank.
/// Updates the `pain002_status` and `settled_at` / `pain002_reason` columns.
///
/// EPC reason codes (ISO 20022):
/// - `AC01` — incorrect account number (IBAN)
/// - `AM04` — insufficient funds
/// - `AC06` — account blocked
/// - `MD01` — no mandate (direct debit only — not applicable here)
/// - `RJCT` + empty reason — generic rejection
#[derive(Debug, serde::Deserialize)]
pub struct Pain002StatusUpdate {
    /// `ACCP` | `RJCT` | `CANC`
    pub status: String,
    /// EPC/ISO 20022 reason code (e.g. `"AC01"`). Absent for ACCP.
    pub reason_code: Option<String>,
}

/// `PUT /api/v1/eeg/payouts/{payout_id}/status`
///
/// Record a pain.002 status report for a payout order.
/// `ACCP` → sets `settled_at = now()`.
/// `RJCT` / `CANC` → sets `pain002_reason` for audit; emits
/// `de.accounting.eeg.payout.rejected` CloudEvent if ERP webhook is configured.
pub async fn put_eeg_payout_status(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(payout_id): Path<uuid::Uuid>,
    Json(req): Json<Pain002StatusUpdate>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "run-payout", &cfg.tenant) {
        return forbidden(&e);
    }
    if !["ACCP", "RJCT", "CANC"].contains(&req.status.as_str()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "status must be ACCP, RJCT, or CANC" })),
        )
            .into_response();
    }

    let settled_at: Option<time::OffsetDateTime> = if req.status == "ACCP" {
        Some(time::OffsetDateTime::now_utc())
    } else {
        None
    };

    // The status write and the RJCT/CANC CloudEvent commit atomically: the CE is
    // enqueued in the SAME transaction as the eeg_payout_orders update
    // (persist-before-dispatch), then delivered by the outbox worker.
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let updated = match sqlx::query(
        r"UPDATE eeg_payout_orders
          SET pain002_status = $1,
              pain002_reason = $2,
              settled_at     = COALESCE($3, settled_at)
          WHERE payout_id = $4 AND tenant = $5
          RETURNING payout_id, malo_id, end_to_end_ref, amount_ct, payment_type",
    )
    .bind(&req.status)
    .bind(req.reason_code.as_deref())
    .bind(settled_at)
    .bind(payout_id)
    .bind(&cfg.tenant)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "payout not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    use sqlx::Row;
    let malo_id: String = updated.get("malo_id");
    let e2e_ref: String = updated.get("end_to_end_ref");
    let amount_ct: i64 = updated.get("amount_ct");
    let payment_type: String = updated.get("payment_type");

    // Enqueue CloudEvent for RJCT / CANC so ERP can alert the operator.
    if req.status != "ACCP" && cfg.erp_webhook_url.is_some() {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("accountingd", &cfg.tenant),
            mako_events::accounting::EEG_PAYOUT_REJECTED,
            &malo_id,
            serde_json::json!({
                "payout_id":     payout_id.to_string(),
                "malo_id":       malo_id,
                "end_to_end_ref": e2e_ref,
                "amount_ct":     amount_ct,
                "payment_type":  payment_type,
                "pain002_status": req.status,
                "pain002_reason": req.reason_code,
            }),
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            tracing::error!(malo_id, error = %e, "accountingd: outbox enqueue (eeg_payout_rejected) failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// ── pain.002 Payment Status Report ingestion ─────────────────────────────────

/// `POST /api/v1/sepa/pain002` — ingest a **pain.002 XML** exactly as the bank
/// delivers it (Customer Payment Status Report).
///
/// One document answers a whole submission, so it is applied to whatever it
/// refers to, keyed by the reference the bank echoes back:
///
/// | The report is about | Matched on | Effect |
/// |---|---|---|
/// | a pain.001 EEG payout | `eeg_payout_orders.end_to_end_ref` | status, reason, `settled_at`, VoP outcome |
/// | a pain.008 collection | `sepa_collection_entries.end_to_end_id` | `SETTLED` / `REJECTED` + a rejection event |
///
/// ## A rejected collection is not a Bankrücklastschrift
///
/// `RJCT` on a direct debit means the collection **never happened**: no money
/// moved, so nothing is reversed. accountingd books a `ZAHLUNG` only when a camt
/// booking confirms the money arrived, so posting a compensating
/// `BANKRUECKLAST` here would credit a payment that was never received and then
/// debit it back. The receivable simply stays open, the entry is marked
/// `REJECTED`, and `de.accounting.sepa.collection-rejected` tells the ERP the
/// mandate needs attention. A collection that settled and was *then* returned
/// arrives as a camt.054 R-transaction and is the other event.
///
/// ## Verification of Payee
///
/// VoP has been mandatory for euro credit transfers since 9 October 2025 and its
/// result arrives inside this same message. It is a **different axis** from
/// acceptance: `RCVC` says a payee name matched, which is not a statement about
/// whether the payment was taken. The outcome is stored in `vop_outcome` and
/// leaves `pain002_status` alone; anything other than a match emits
/// `de.accounting.payee.verification-mismatch`, because executing after a
/// no-match shifts liability to the payer and that is an operator's decision.
pub async fn import_pain002(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    body: String,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-sepa", &cfg.tenant) {
        return forbidden(&e);
    }
    let doc = match crate::sepa::parse_pain002(&body) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("pain.002 parse failed: {e}") })),
            )
                .into_response();
        }
    };

    let mut payouts_updated = 0usize;
    let mut collections_updated = 0usize;
    let mut unmatched = 0usize;
    let mut verifications = 0usize;
    let mut total = 0usize;

    for pmt_inf in &doc.payment_info_statuses {
        for tx in &pmt_inf.transactions {
            total += 1;
            // Both references are `0..1`; a bank may echo either. A report that
            // names neither is unattributable and must not be guessed at.
            let Some(reference) = tx
                .original_end_to_end_id
                .as_deref()
                .or(tx.original_instruction_id.as_deref())
            else {
                unmatched += 1;
                continue;
            };
            // A missing `TxSts` is not a status: the group status governs, and
            // "no status at all" is not an acceptance.
            let status = tx.status.as_ref().or(doc.group_status.as_ref());
            let Some(status) = status else {
                unmatched += 1;
                continue;
            };
            let reason = tx.reason_codes.first().map(|r| r.as_code().to_owned());
            // `StsRsnInf/AddtlInf` is unbounded and banks use it: a legal notice
            // spans lines, and a VoP close-match name over 105 characters
            // arrives split in two. Join before storing.
            let additional_info = if tx.additional_info.is_empty() {
                None
            } else {
                Some(tx.additional_info.join(" "))
            };

            match apply_pain002_status(
                &pool,
                &cfg,
                reference,
                status,
                reason.as_deref(),
                additional_info.as_deref(),
            )
            .await
            {
                Ok(Pain002Match::Payout) => payouts_updated += 1,
                Ok(Pain002Match::Collection) => collections_updated += 1,
                Ok(Pain002Match::None) => unmatched += 1,
                Err(e) => {
                    tracing::warn!(reference, error = %e, "accountingd: pain.002 status apply failed");
                    unmatched += 1;
                }
            }
            if status.is_verification() {
                verifications += 1;
            }
        }
    }

    // ── Rejections the bank did not itemise ──────────────────────────────────
    //
    // A whole file or a whole `PmtInf` group can bounce with no per-transaction
    // detail at all — a schema fault, a creditor identity the bank refuses, a
    // collection date it will not accept. Reading only `TxInfAndSts` leaves
    // every one of those collections sitting at `SUBMITTED` forever, waiting for
    // money that is never coming.
    //
    // Only rejections are broadcast this way. A group-level *acceptance* is not
    // settlement — `ACTC` says the file parsed — and the camt booking is what
    // confirms a collection actually moved.
    let mut bulk_rejected = 0usize;
    if doc
        .group_status
        .as_ref()
        .is_some_and(crate::sepa::PaymentStatus::is_rejected)
    {
        match crate::pg::reject_submitted_entries_of_run(
            &pool,
            &cfg.tenant,
            &doc.original_msg_id,
            "pain.002 GrpSts=RJCT",
        )
        .await
        {
            Ok(n) => bulk_rejected += n,
            Err(e) => {
                tracing::warn!(error = %e, "accountingd: group-level pain.002 rejection failed to apply");
            }
        }
    } else {
        for pmt_inf in &doc.payment_info_statuses {
            // An itemised group has already been handled per transaction.
            if !pmt_inf.transactions.is_empty() {
                continue;
            }
            let (Some(status), Some(pmt_inf_id)) =
                (&pmt_inf.status, &pmt_inf.original_payment_info_id)
            else {
                continue;
            };
            if !status.is_rejected() {
                continue;
            }
            let reason = pmt_inf.rejection_reasons().first().map_or_else(
                || "pain.002 PmtInfSts=RJCT".to_owned(),
                |r| r.as_code().to_owned(),
            );
            match crate::pg::reject_submitted_entries_of_group(
                &pool,
                &cfg.tenant,
                pmt_inf_id,
                &reason,
            )
            .await
            {
                Ok(n) => bulk_rejected += n,
                Err(e) => {
                    tracing::warn!(error = %e, "accountingd: group-level pain.002 rejection failed to apply");
                }
            }
        }
    }

    // `NbOfTxsPerSts` states counts per outcome for a whole file and itemises
    // only the transactions needing attention — a VoP report on hundreds of
    // payments may carry nothing else. Surface it rather than discard it.
    let status_counts: Vec<serde_json::Value> = doc
        .group_status_counts
        .iter()
        .chain(
            doc.payment_info_statuses
                .iter()
                .flat_map(|p| &p.status_counts),
        )
        .map(|c| {
            serde_json::json!({
                "status":    c.status.as_code(),
                "count":     c.count,
                "total_ct":  c.total_ct,
                "is_verification": c.status.is_verification(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "msg_id":              doc.msg_id,
        "original_msg_id":     doc.original_msg_id,
        "original_msg_type":   doc.original_msg_type.as_ref().map(ToString::to_string),
        "group_status":        doc.group_status.as_ref().map(|s| s.as_code().to_owned()),
        "fully_accepted":      doc.is_fully_accepted(),
        "has_rejections":      doc.has_rejections(),
        "status_counts":       status_counts,
        "payouts_updated":     payouts_updated,
        "collections_updated": collections_updated,
        "bulk_rejected":       bulk_rejected,
        "verifications":       verifications,
        "unmatched":           unmatched,
        "total":               total,
    }))
    .into_response()
}

/// What a pain.002 transaction status was attributed to.
enum Pain002Match {
    Payout,
    Collection,
    None,
}

/// Apply one pain.002 transaction status to whatever it refers to.
async fn apply_pain002_status(
    pool: &PgPool,
    cfg: &AccountingdConfig,
    reference: &str,
    status: &crate::sepa::PaymentStatus,
    reason: Option<&str>,
    additional_info: Option<&str>,
) -> anyhow::Result<Pain002Match> {
    use crate::sepa::VerificationOutcome;

    let verification = status.verification();
    let vop_outcome = verification.map(|v| match v {
        VerificationOutcome::Match => "MATCH",
        VerificationOutcome::CloseMatch => "CLOSE_MATCH",
        VerificationOutcome::NoMatch => "NO_MATCH",
        VerificationOutcome::NotApplicable => "NOT_APPLICABLE",
    });
    // A verification status is not a payment status: writing `RCVC` into
    // `pain002_status` would make a name check look like an acceptance.
    let payment_status = if status.is_verification() {
        None
    } else {
        Some(status.as_code())
    };
    // `ACSC` is the only status that means the money moved; the others that
    // pass `is_accepted` are milestones on the way there. `settled_at` records
    // the first of them, as it always has.
    let settled_at = status.is_accepted().then(time::OffsetDateTime::now_utc);

    let mut tx = pool.begin().await?;

    // ── A pain.001 payout order ──────────────────────────────────────────────
    let payout = sqlx::query(
        r"UPDATE eeg_payout_orders
          SET pain002_status  = COALESCE($1, pain002_status),
              pain002_reason  = COALESCE($2, pain002_reason),
              settled_at      = COALESCE($3, settled_at),
              vop_outcome     = COALESCE($4, vop_outcome),
              vop_name        = COALESCE($5, vop_name),
              vop_reported_at = CASE WHEN $4 IS NULL THEN vop_reported_at ELSE now() END
          WHERE end_to_end_ref = $6 AND tenant = $7
          RETURNING payout_id, malo_id, amount_ct, payment_type",
    )
    .bind(payment_status)
    .bind(reason)
    .bind(settled_at)
    .bind(vop_outcome)
    .bind(additional_info.filter(|_| vop_outcome.is_some()))
    .bind(reference)
    .bind(&cfg.tenant)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(row) = payout {
        let payout_id: uuid::Uuid = row.try_get("payout_id")?;
        let malo_id: String = row.try_get("malo_id")?;
        let amount_ct: i64 = row.try_get("amount_ct")?;
        let payment_type: String = row.try_get("payment_type")?;

        if status.is_rejected() {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("accountingd", &cfg.tenant),
                mako_events::accounting::EEG_PAYOUT_REJECTED,
                &malo_id,
                serde_json::json!({
                    "payout_id":      payout_id.to_string(),
                    "malo_id":        malo_id,
                    "end_to_end_ref": reference,
                    "amount_ct":      amount_ct,
                    "payment_type":   payment_type,
                    "pain002_status": status.as_code(),
                    "pain002_reason": reason,
                    "additional_info": additional_info,
                }),
            );
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
        }
        // Anything but a clean match is an operator decision: after `RVNM`,
        // executing the transfer moves liability to the payer.
        if verification.is_some_and(|v| v != VerificationOutcome::Match) {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("accountingd", &cfg.tenant),
                mako_events::accounting::PAYEE_VERIFICATION_MISMATCH,
                &malo_id,
                serde_json::json!({
                    "payout_id":      payout_id.to_string(),
                    "malo_id":        malo_id,
                    "end_to_end_ref": reference,
                    "amount_ct":      amount_ct,
                    "vop_outcome":    vop_outcome,
                    // On a close match this is the name the payee's PSP holds.
                    "vop_name":       additional_info,
                    "reason_code":    reason,
                }),
            );
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
        }
        tx.commit().await?;
        return Ok(Pain002Match::Payout);
    }

    // ── A pain.008 collection ────────────────────────────────────────────────
    let Some(collected) =
        crate::pg::find_collection_entry_by_e2e(pool, &cfg.tenant, reference).await?
    else {
        tx.rollback().await?;
        return Ok(Pain002Match::None);
    };

    // A verification status says nothing about a collection's lifecycle.
    if status.is_verification() {
        tx.rollback().await?;
        return Ok(Pain002Match::Collection);
    }

    let new_status = if status.is_rejected() {
        "REJECTED"
    } else if status.is_accepted() {
        "SETTLED"
    } else {
        // PDNG and friends: still in flight, nothing to record yet.
        tx.rollback().await?;
        return Ok(Pain002Match::Collection);
    };
    crate::pg::set_collection_entry_status(&mut *tx, collected.entry_id, new_status, reason)
        .await?;

    if status.is_rejected()
        && let Some(malo_id) = collected.malo_id.as_deref()
    {
        // No compensating ledger entry: the collection never settled, so the
        // receivable was never reduced. See the handler's doc comment.
        let ce = mako_service::CloudEvent::new(
            mako_service::source("accountingd", &cfg.tenant),
            mako_events::accounting::SEPA_COLLECTION_REJECTED,
            malo_id,
            serde_json::json!({
                "malo_id":         malo_id,
                "lf_mp_id":        collected.lf_mp_id,
                "mandate_id":      collected.mandate_id.map(|id| id.to_string()),
                "mandatsref":      collected.mandatsref,
                "end_to_end_id":   collected.end_to_end_id,
                "payment_info_id": collected.payment_info_id,
                "amount_ct":       collected.amount_ct,
                "collection_date": collected.collection_date.to_string(),
                "reason_code":     reason,
                "additional_info": additional_info,
            }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
    }

    tx.commit().await?;
    Ok(Pain002Match::Collection)
}

// ── pain.007 SEPA Direct Debit reversal ──────────────────────────────────────

/// Request body for `POST /api/v1/sepa/reversals`.
#[derive(Debug, serde::Deserialize)]
pub struct CreateReversalRequest {
    /// The collected entry to give back (`sepa_collection_entries.entry_id`).
    pub collection_entry_id: uuid::Uuid,
    /// ISO 20022 `ExternalReversalReason1Code`. Defaults to `MS02` — "no reason
    /// specified by the customer", the code the DK's own reversal example
    /// carries and what a creditor uses when it simply collected in error.
    pub reason_code: Option<String>,
    /// Partial reversal amount in ct. Absent reverses the whole collection;
    /// more than was collected is refused.
    pub reversed_amount_ct: Option<i64>,
}

/// `POST /api/v1/sepa/reversals` — build a **pain.007** giving a settled
/// direct-debit collection back.
///
/// A reversal is the creditor's own correction — the Abschlag collected twice,
/// or collected after the customer had already paid by transfer. It is the
/// counterpart to a debtor-initiated refund (which arrives as camt.054) and to a
/// reject (which arrives as pain.002 and never moved money at all).
///
/// Every field of `OrgnlTxRef` is restated from `sepa_collection_entries` and
/// `sepa_mandates` rather than from the request body, so the reversal cannot
/// disagree with what was collected. The DK technical validation subset makes
/// that block — and the mandate inside it — mandatory, so the references-only
/// form plain ISO permits is not one a German bank accepts.
///
/// The compensating `SEPA_STORNO` ledger entry re-opens the receivable: the
/// money leaves the bank account again, so what the collection discharged is
/// owed once more.
pub async fn post_sepa_reversal(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(req): Json<CreateReversalRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "manage-sepa", &cfg.tenant) {
        return forbidden(&e);
    }
    let entry = match crate::pg::fetch_collection_entry(&pool, req.collection_entry_id, &cfg.tenant)
        .await
    {
        Ok(Some(e)) => e,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "collection entry not found" })),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Only a *settled* collection can be reversed. A rejected one never moved
    // money, and an already-returned or already-reversed one would refund twice.
    if entry.status != "SETTLED" {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!(
                    "collection is {} — only a SETTLED collection can be reversed \
                     (a REJECTED one never moved money; a RETURNED or REVERSED one \
                     has already been given back)",
                    entry.status
                ),
                "status": entry.status,
            })),
        )
            .into_response();
    }

    // The debtor identity lives on the mandate, so an erased mandate correctly
    // makes the reversal impossible rather than built from a stale copy.
    let (Some(debtor_iban), Some(signed_at)) =
        (entry.debtor_iban.as_deref(), entry.mandate_signed_at)
    else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "the mandate behind this collection is gone (revoked and deleted, \
                          or anonymised under GDPR Art. 17) — pain.007 needs its IBAN and \
                          signature date in OrgnlTxRef"
            })),
        )
            .into_response();
    };

    let (Some(creditor_iban), Some(creditor_id)) = (
        cfg.creditor_iban.as_deref().filter(|s| !s.is_empty()),
        cfg.creditor_id.as_deref().filter(|s| !s.is_empty()),
    ) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "creditor_iban and creditor_id (Gläubiger-ID) must both be \
                          configured — a reversal restates the creditor identity the \
                          collection carried"
            })),
        )
            .into_response();
    };
    let creditor_name = cfg.creditor_name.as_deref().unwrap_or(&cfg.tenant);
    let creditor = crate::sepa::CreditorIdentity {
        iban: creditor_iban,
        name: creditor_name,
        creditor_id,
        address: Some(&cfg.creditor_address),
    };

    let reason: crate::sepa::ReversalReason = req
        .reason_code
        .as_deref()
        .unwrap_or("MS02")
        .parse()
        .unwrap_or(crate::sepa::ReversalReason::Ms02);
    let reversed_amount_ct = req.reversed_amount_ct.unwrap_or(entry.amount_ct);
    if reversed_amount_ct <= 0 || reversed_amount_ct > entry.amount_ct {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "reversed_amount_ct must be between 1 and the collected {} ct",
                    entry.amount_ct
                )
            })),
        )
            .into_response();
    }

    let dd_schema = match crate::sepa::resolve_pain008_schema(cfg.pain008_schema.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let reversal_request = crate::sepa::ReversalRequest {
        original_msg_id: &entry.msg_id,
        original_payment_info_id: &entry.payment_info_id,
        original_end_to_end_id: &entry.end_to_end_id,
        original_amount_ct: entry.amount_ct,
        reversed_amount_ct: (reversed_amount_ct != entry.amount_ct).then_some(reversed_amount_ct),
        reason: reason.clone(),
        mandate_ref: &entry.mandatsref,
        mandate_signed_at: signed_at,
        collection_date: entry.collection_date,
        sequence_type: &entry.sequence_type,
        scheme: &entry.scheme,
        debtor_name: entry.debtor_name.as_deref().unwrap_or("Kunde"),
        debtor_iban,
        debtor_bic: entry.debtor_bic.as_deref(),
    };

    let reversal = match crate::sepa::build_pain_007(&creditor, &[reversal_request], dd_schema) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // The compensating ledger entry re-opens the receivable. Positive = Forderung.
    let ledger_entry_id = match (entry.malo_id.as_deref(), entry.lf_mp_id.as_deref()) {
        (Some(malo_id), Some(lf_mp_id)) => {
            let today = mako_fristen::heute();
            match crate::pg::post_entry(
                &ledger,
                &pool,
                &cfg.tenant,
                malo_id,
                lf_mp_id,
                "SEPA_STORNO",
                reversed_amount_ct,
                &format!("sepa-reversal:{}", entry.entry_id),
                None,
                Some(&entry.end_to_end_id),
                today,
                today,
                Some(&format!(
                    "pain.007 Storno der Einzugs vom {} ({})",
                    entry.collection_date,
                    reason.as_code()
                )),
                None,
            )
            .await
            {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::error!(error = %e, "accountingd: SEPA_STORNO ledger post failed");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("reversal ledger post failed: {e}")
                        })),
                    )
                        .into_response();
                }
            }
        }
        _ => None,
    };

    // Record the reversal, close the collected entry and announce it in one
    // transaction: the unique index on `collection_entry_id` is what stops a
    // second request refunding the same collection twice.
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let reversal_id = match crate::pg::record_sepa_reversal(
        &mut *tx,
        &cfg.tenant,
        &entry,
        &reversal,
        reversed_amount_ct,
        reason.as_code(),
        ledger_entry_id,
        Some(claims.sub()),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!("reversal already recorded for this collection: {e}")
                })),
            )
                .into_response();
        }
    };
    if let Err(e) = crate::pg::set_collection_entry_status(
        &mut *tx,
        entry.entry_id,
        "REVERSED",
        Some(reason.as_code()),
    )
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Some(malo_id) = entry.malo_id.as_deref() {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("accountingd", &cfg.tenant),
            mako_events::accounting::SEPA_REVERSAL_ISSUED,
            malo_id,
            serde_json::json!({
                "reversal_id":          reversal_id.to_string(),
                "malo_id":              malo_id,
                "lf_mp_id":             entry.lf_mp_id,
                "mandatsref":           entry.mandatsref,
                "original_msg_id":      entry.msg_id,
                "original_end_to_end_id": entry.end_to_end_id,
                "original_amount_ct":   entry.amount_ct,
                "reversed_amount_ct":   reversed_amount_ct,
                "reason_code":          reason.as_code(),
                "ledger_id":            ledger_entry_id.map(|id| id.to_string()),
            }),
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "reversal_id":        reversal_id.to_string(),
            "msg_id":             reversal.msg_id,
            "payment_info_id":    reversal.payment_info_id,
            "original_msg_id":    entry.msg_id,
            "reversed_amount_ct": reversed_amount_ct,
            "reason_code":        reason.as_code(),
            "ledger_id":          ledger_entry_id.map(|id| id.to_string()),
            "xml":                reversal.xml,
        })),
    )
        .into_response()
}

/// `GET /api/v1/sepa/collections/{run_id}/entries` — what a collection run
/// collected, and where each entry stands.
///
/// The list a reversal is chosen from: `entry_id` is what
/// `POST /api/v1/sepa/reversals` takes, and `status` says whether the collection
/// is still in flight (`SUBMITTED`), confirmed (`SETTLED`), refused before it
/// moved (`REJECTED`), returned by the debtor (`RETURNED`) or already given back
/// (`REVERSED`).
pub async fn get_collection_entries(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(run_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-banking", &cfg.tenant) {
        return forbidden(&e);
    }
    let rows = sqlx::query(
        r"SELECT ce.entry_id, ce.mandatsref, ce.end_to_end_id, ce.payment_info_id,
                 ce.sequence_type, ce.amount_ct, ce.status, ce.status_reason, ce.status_at,
                 a.malo_id
          FROM sepa_collection_entries ce
          LEFT JOIN accounts a ON a.account_id = ce.account_id
          WHERE ce.run_id = $1 AND ce.tenant = $2
          ORDER BY ce.payment_info_id, ce.mandatsref",
    )
    .bind(run_id)
    .bind(&cfg.tenant)
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => Json(serde_json::json!({
            "run_id": run_id.to_string(),
            "entries": rows.iter().map(|r| serde_json::json!({
                "entry_id":        r.try_get::<uuid::Uuid, _>("entry_id").ok().map(|i| i.to_string()),
                "malo_id":         r.try_get::<Option<String>, _>("malo_id").ok().flatten(),
                "mandatsref":      r.try_get::<String, _>("mandatsref").ok(),
                "end_to_end_id":   r.try_get::<String, _>("end_to_end_id").ok(),
                "payment_info_id": r.try_get::<String, _>("payment_info_id").ok(),
                "sequence_type":   r.try_get::<String, _>("sequence_type").ok(),
                "amount_ct":       r.try_get::<i64, _>("amount_ct").ok(),
                "status":          r.try_get::<String, _>("status").ok(),
                "status_reason":   r.try_get::<Option<String>, _>("status_reason").ok().flatten(),
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Submit a pain.001 XML to the configured bank adapter and update `submitted_at`.
///
/// Best-effort: failures are logged but do not roll back the payout order.
pub(crate) async fn submit_pain001_to_bank(
    bank_url: &str,
    api_key: Option<&str>,
    pain_xml: &str,
    end_to_end_ref: &str,
    pool: &PgPool,
    tenant: &str,
) {
    // `bank_url` is the full submission endpoint, so the upstream is addressed
    // at it with an empty path — the point here is uniform credential handling,
    // not path composition.
    let bank = mako_service::http::Upstream::new(
        "bank adapter",
        bank_url,
        api_key.map(|k| secrecy::SecretString::from(k.to_owned())),
        mako_service::http::default_client(),
    );
    let req = bank
        .post("")
        .header("Content-Type", "application/xml")
        .body(pain_xml.to_owned());

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let now = time::OffsetDateTime::now_utc();
            let _ = sqlx::query(
                "UPDATE eeg_payout_orders SET submitted_at = $1, pain002_status = 'PDNG' \
                 WHERE end_to_end_ref = $2 AND tenant = $3",
            )
            .bind(now)
            .bind(end_to_end_ref)
            .bind(tenant)
            .execute(pool)
            .await;
            tracing::info!(end_to_end_ref, "accountingd: pain.001 submitted to bank");
        }
        Ok(resp) => {
            tracing::warn!(
                status = %resp.status(),
                end_to_end_ref,
                "accountingd: bank adapter rejected pain.001"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, end_to_end_ref, "accountingd: bank submit failed");
        }
    }
}

/// Bundles all parameters for [`create_eeg_payout_order`] into a single struct
/// to stay within the 7-argument clippy limit.
pub(crate) struct EegPayoutParams<'a> {
    pub malo_id: &'a str,
    pub account_id: uuid::Uuid,
    pub amount_ct: i64,
    pub creditor_iban: &'a str,
    pub creditor_name: &'a str,
    /// The plant operator's own `Cdtr/PstlAdr`, from `accounts.addr_*`.
    pub creditor_address: crate::sepa::AddressParts,
    pub tr_id: Option<&'a str>,
    pub billing_year: i16,
    pub billing_month: i16,
    pub source_ce_id: Option<&'a str>,
}

/// Generate and optionally submit a pain.001 payout for a single EEG settlement CE.
///
/// Called from the `de.eeg.verguetung.berechnet` webhook handler when
/// `cfg.eeg.auto_payout = true`.  Idempotent via `source_ce_id` unique index.
pub(crate) async fn create_eeg_payout_order(
    cfg: &AccountingdConfig,
    pool: &PgPool,
    params: EegPayoutParams<'_>,
) {
    use crate::sepa::build_pain_001;

    let debtor_iban = match cfg.eeg.debtor_iban.as_deref() {
        Some(iban) => iban,
        None => {
            tracing::warn!(
                malo_id = params.malo_id,
                "accountingd: auto_payout=true but [eeg].debtor_iban not set — skip payout"
            );
            return;
        }
    };

    let use_instant = cfg.eeg.sepa_instant;
    let payment_type = if use_instant { "SCT_INST" } else { "SCT_CORE" };

    // Build deterministic EndToEndId (max 35 chars, ISO 20022)
    let e2e_ref = format!(
        "EEG-{}-{:04}-{:02}-{}",
        &params.malo_id[..params.malo_id.len().min(10)],
        params.billing_year,
        params.billing_month,
        params
            .source_ce_id
            .and_then(|s| s.get(..8))
            .unwrap_or("AUTO")
    );

    let debtor_name = cfg.creditor_name.as_deref().unwrap_or(&cfg.tenant);
    let ct_schema = match crate::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(malo_id = params.malo_id, error = %e, "accountingd: pain.001 schema config invalid");
            return;
        }
    };
    // §25 Abs. 1 EEG 2023 — "unverzüglich nach Ende des Monats": the payout
    // leaves today rather than on a library default.
    let execution_date = mako_fristen::heute();
    let pain_xml = match build_pain_001(
        &crate::sepa::DebtorIdentity {
            iban: debtor_iban,
            name: debtor_name,
            address: Some(&cfg.creditor_address),
        },
        &[crate::sepa::CreditTransferItem {
            iban: params.creditor_iban,
            name: params.creditor_name,
            amount_ct: params.amount_ct,
            end_to_end_ref: &e2e_ref,
            address: Some(&params.creditor_address),
        }],
        execution_date,
        use_instant,
        ct_schema,
    ) {
        Ok(xml) => xml,
        Err(e) => {
            tracing::warn!(malo_id = params.malo_id, error = %e, "accountingd: auto pain.001 build failed");
            return;
        }
    };

    let insert = sqlx::query(
        r"INSERT INTO eeg_payout_orders
              (malo_id, account_id, tr_id, billing_year, billing_month, amount_ct,
               creditor_iban, creditor_name, payment_type, end_to_end_ref,
               pain001_xml, source_ce_id, tenant,
               creditor_town, creditor_country, creditor_street,
               creditor_building_number, creditor_post_code,
               creditor_country_subdivision)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
          ON CONFLICT (end_to_end_ref) DO NOTHING",
    )
    .bind(params.malo_id)
    .bind(params.account_id)
    .bind(params.tr_id)
    .bind(params.billing_year)
    .bind(params.billing_month)
    .bind(params.amount_ct)
    .bind(params.creditor_iban)
    .bind(params.creditor_name)
    .bind(payment_type)
    .bind(&e2e_ref)
    .bind(&pain_xml)
    .bind(params.source_ce_id)
    .bind(&cfg.tenant)
    .bind(&params.creditor_address.town)
    .bind(&params.creditor_address.country)
    .bind(&params.creditor_address.street)
    .bind(&params.creditor_address.building_number)
    .bind(&params.creditor_address.post_code)
    .bind(&params.creditor_address.country_subdivision)
    .execute(pool)
    .await;

    match insert {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(
                malo_id = params.malo_id,
                payment_type,
                e2e_ref,
                amount_ct = params.amount_ct,
                "accountingd: EEG payout order created"
            );
            // Auto-submit to bank adapter if configured.
            if let Some(ref url) = cfg.eeg.bank_submit_url {
                submit_pain001_to_bank(
                    url,
                    cfg.eeg.bank_api_key.as_deref(),
                    &pain_xml,
                    &e2e_ref,
                    pool,
                    &cfg.tenant,
                )
                .await;
            }
        }
        Ok(_) => {} // idempotent — already exists
        Err(e) => {
            tracing::warn!(malo_id = params.malo_id, error = %e, "accountingd: eeg_payout_orders insert error");
        }
    }
}

// ── Aging analysis ────────────────────────────────────────────────────────────

/// `GET /api/v1/aging` — open-receivables aging report.
///
/// Groups overdue account balances into four buckets:
/// `0-30d` · `31-60d` · `61-90d` · `>90d`
///
/// Uses the oldest unresolved dunning case issued_at as the "overdue since" date
/// when present; falls back to `accounts.updated_at` otherwise.
pub async fn get_aging(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-books", &cfg.tenant) {
        return forbidden(&e);
    }
    match crate::pg::list_aging_buckets(&pool, &cfg.tenant).await {
        Ok(buckets) => {
            let total_ct: i64 = buckets.iter().map(|b| b.total_ct).sum();
            let total_accounts: i64 = buckets.iter().map(|b| b.account_count).sum();
            Json(serde_json::json!({
                "tenant": cfg.tenant,
                "total_overdue_ct": total_ct,
                "total_overdue_eur": format_ct_as_eur(total_ct),
                "total_overdue_accounts": total_accounts,
                "buckets": buckets,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Interest charges (Verzugszinsen §288 BGB) ─────────────────────────────────

/// `GET /api/v1/accounts/{malo_id}/interest-charges` — list interest charges.
pub async fn get_interest_charges(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match crate::pg::list_interest_charges(&pool, account.account_id, &cfg.tenant, 200).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateInterestChargeRequest {
    pub lf_mp_id: Option<String>,
    pub invoice_reference: Option<String>,
    pub principal_ct: i64,
    pub is_b2b: Option<bool>,
    pub period_from: String,
    pub period_to: String,
}

/// `POST /api/v1/accounts/{malo_id}/interest-charges` — calculate and book Verzugszinsen.
///
/// Calculates interest per §288 BGB using the current ECB Basiszinssatz from
/// `ecb_base_rates` table.  Creates a `MAHNGEBUEHR` ledger entry and records
/// the charge in `interest_charges` for audit.
pub async fn post_interest_charge(
    Extension(pool): Extension<PgPool>,
    Extension(ledger): Extension<Arc<crate::ledger::PgLedger>>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Json(req): Json<CreateInterestChargeRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }
    use time::format_description::well_known::Iso8601;

    let lf_mp_id = req.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let period_from = match time::Date::parse(&req.period_from, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid period_from").into_response(),
    };
    let period_to = match time::Date::parse(&req.period_to, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid period_to").into_response(),
    };
    if req.principal_ct <= 0 {
        return (StatusCode::BAD_REQUEST, "principal_ct must be > 0").into_response();
    }

    match crate::pg::create_interest_charge(
        &ledger,
        &pool,
        account.account_id,
        &cfg.tenant,
        &malo_id,
        lf_mp_id,
        req.invoice_reference.as_deref(),
        req.principal_ct,
        req.is_b2b.unwrap_or(false),
        period_from,
        period_to,
    )
    .await
    {
        Ok(charge) => (StatusCode::CREATED, Json(charge)).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── Payment plans (Zahlungsvereinbarung) ──────────────────────────────────────

/// `GET /api/v1/accounts/{malo_id}/payment-plans` — list payment plans for a MaLo.
pub async fn get_payment_plans(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match crate::pg::list_payment_plans(&pool, account.account_id, &cfg.tenant).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/accounts/{malo_id}/payment-plans` — create a Zahlungsvereinbarung.
///
/// Creates a structured payment plan with an auto-generated installment schedule.
/// An ACTIVE plan suppresses automatic Sperrung escalation (Mahnstufe 3).
pub async fn post_payment_plan(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Json(mut req): Json<crate::pg::CreatePaymentPlanRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }
    req.malo_id = malo_id;
    match crate::pg::create_payment_plan(&pool, &cfg.tenant, req).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "plan_id": id })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/payment-plans/{plan_id}` — get a payment plan with installments.
pub async fn get_payment_plan(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(plan_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-account", &cfg.tenant) {
        return forbidden(&e);
    }
    match crate::pg::get_payment_plan_with_installments(&pool, plan_id, &cfg.tenant).await {
        Ok(Some((plan, installments))) => Json(serde_json::json!({
            "plan": plan,
            "installments": installments,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/payment-plans/{plan_id}` — cancel a payment plan.
pub async fn delete_payment_plan(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Path(plan_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "post-entry", &cfg.tenant) {
        return forbidden(&e);
    }
    match crate::pg::cancel_payment_plan(&pool, plan_id, &cfg.tenant, Some(claims.sub())).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}
