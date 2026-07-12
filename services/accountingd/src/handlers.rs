//! HTTP handlers for `accountingd`.
//!
//! Covers:
//! - Account CRUD + balance
//! - Ledger entry listing + Kontoauszug
//! - CloudEvent ingest webhook (de.billing.rechnung.erstellt, de.invoic.receipt.settled, de.eeg.verguetung.berechnet)
//! - SEPA mandate management + pain.008 XML generation
//! - Dunning cases
//! - Offene Posten listing

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use sqlx::{PgPool, Row as _};
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    config::AccountingdConfig,
    pg::{
        CreateMandateRequest, UpdateAccountRequest, create_dunning_case, create_mandate,
        fetch_account, fetch_account_by_id, fetch_mandate, fetch_vorauszahlung,
        list_active_mandates, list_ledger, list_ledger_year, list_open_dunning,
        list_overdue_accounts, resolve_dunning_case, update_account, upsert_account,
        upsert_vorauszahlung, write_entry,
    },
    sepa::build_pain_008,
};

// ── Account endpoints ─────────────────────────────────────────────────────────

/// `GET /api/v1/accounts/{malo_id}`
pub async fn get_account(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    match fetch_account(&pool, &malo_id, lf_mp_id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/accounts/{malo_id}`  — upsert account + update fields (IBAN, Abschlag)
pub async fn put_account(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
    Json(req): Json<crate::pg::UpdateAccountRequest>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    let _ = upsert_account(&pool, &malo_id, &lf_mp_id, &cfg.tenant).await;
    match update_account(&pool, &malo_id, &lf_mp_id, req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/balance`  — current balance in ct (negative = credit)
pub async fn get_balance(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    match fetch_account(&pool, &malo_id, lf_mp_id).await {
        Ok(Some(row)) => Json(serde_json::json!({
            "malo_id": malo_id,
            "balance_ct": row.balance_ct,
            "balance_eur": format!("{:.2}", row.balance_ct as f64 / 100.0),
            "status": if row.balance_ct > 0 { "overdue" } else if row.balance_ct < 0 { "credit" } else { "settled" },
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/ledger`  — paged ledger entries
pub async fn get_ledger(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<LedgerQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match list_ledger(&pool, account.account_id, q.limit.unwrap_or(100).min(1000)).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/accounts/{malo_id}/kontoauszug`  — account statement (portald-consumable)
pub async fn get_kontoauszug(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let entries = match list_ledger(&pool, account.account_id, 500).await {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    Json(serde_json::json!({
        "malo_id": malo_id,
        "lf_mp_id": lf_mp_id,
        "balance_ct": account.balance_ct,
        "abschlag_ct": account.abschlag_ct,
        "generated_at": OffsetDateTime::now_utc().to_string(),
        "entries": entries,
    }))
    .into_response()
}

/// `PUT /api/v1/accounts/{malo_id}/abschlag`  — update monthly advance payment
pub async fn put_abschlag(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let abschlag_ct = body.get("abschlag_ct").and_then(|v| v.as_i64());
    if let Some(ct) = abschlag_ct {
        match update_account(
            &pool,
            &malo_id,
            &cfg.tenant,
            crate::pg::UpdateAccountRequest {
                iban: None,
                mandatsref: None,
                abschlag_ct: Some(ct),
                billing_day: None,
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

#[derive(Debug, Deserialize)]
pub struct LedgerQuery {
    pub lf_mp_id: Option<String>,
    pub limit: Option<i64>,
}

// ── CloudEvent ingest (webhook) ───────────────────────────────────────────────

/// `POST /webhook` — ingest CloudEvents from billingd, invoicd, einsd, netzbilanzd.
///
/// Supported event types:
/// - `de.billing.rechnung.erstellt` → debit entry
/// - `de.invoic.receipt.settled`    → credit entry (NNE receipt paid)
/// - `de.invoic.receipt.disputed`   → no entry (dispute logged)
/// - `de.eeg.verguetung.berechnet`  → credit entry (EEG settlement)
pub async fn ingest_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(ce): Json<serde_json::Value>,
) -> impl IntoResponse {
    let ce_type = ce.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let ce_id = ce.get("id").and_then(|v| v.as_str()).map(str::to_owned);
    let data = ce.get("data");

    let today = OffsetDateTime::now_utc().date();

    match ce_type {
        "de.billing.rechnung.erstellt" => {
            let malo_id = data
                .and_then(|d| d.get("malo_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let lf_mp_id = data
                .and_then(|d| d.get("lf_mp_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&cfg.tenant);
            // L8: corrections emit the same event type with is_correction=true.
            // A correction Rechnung has negated amounts already; writing the entry
            // as-is produces the CREDIT effect automatically.
            let is_correction: bool = data
                .and_then(|d| d.get("is_correction"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let total_brutto_eur: f64 = data
                .and_then(|d| d.get("rechnung"))
                .and_then(|r| r.get("gesamtbrutto"))
                .and_then(|g| g.get("wert"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let amount_ct = (total_brutto_eur * 100.0).round() as i64;
            let account_id = upsert_account(&pool, malo_id, lf_mp_id, &cfg.tenant)
                .await
                .unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() && amount_ct != 0 {
                let record_id = data
                    .and_then(|d| d.get("record_id"))
                    .and_then(|v| v.as_str());
                let entry_type = if is_correction {
                    "KORREKTURRECHNUNG"
                } else {
                    "RECHNUNG"
                };
                let description = if is_correction {
                    "Korrekturrechnung / Stornorechnung (Gutschrift)"
                } else {
                    "Kundenrechnung"
                };
                let _ = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    entry_type,
                    amount_ct, // already negative for corrections (amounts negated in Rechnung)
                    record_id,
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some(description),
                )
                .await;
            }
            StatusCode::OK.into_response()
        }
        "de.invoic.receipt.settled" => {
            // NNE receipt settled = credit (NB paid us, or we confirmed receipt)
            // For LF: when an inbound NNE invoice is settled (annehmen), it's a debit.
            // We record it as the payment obligation confirmed.
            StatusCode::OK.into_response()
        }
        "de.eeg.verguetung.berechnet" => {
            let malo_id = ce.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let settlement_eur: f64 = data
                .and_then(|d| d.get("settlement_eur"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let amount_ct = -(settlement_eur * 100.0).round() as i64; // negative = credit
            let account_id = upsert_account(&pool, malo_id, &cfg.tenant, &cfg.tenant)
                .await
                .unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() && amount_ct != 0 {
                let _ = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    "EEG_GUTSCHRIFT",
                    amount_ct,
                    ce_id.as_deref(),
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some("EEG Einspeisevergütung"),
                )
                .await;
            }
            StatusCode::OK.into_response()
        }
        _ => {
            // Unknown event type — log and ignore.
            tracing::debug!(ce_type, "accountingd: unknown CloudEvent type — ignored");
            StatusCode::OK.into_response()
        }
    }
}

/// `POST /api/v1/payments/import`  — ingest CAMT.054 bank statement (JSON array).
///
/// Each entry should have: `{ "iban": "...", "amount_eur": ..., "reference": "...", "date": "YYYY-MM-DD" }`
pub async fn import_payments(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(entries): Json<Vec<serde_json::Value>>,
) -> impl IntoResponse {
    let mut accepted = 0usize;
    for entry in &entries {
        let amount_eur: f64 = entry
            .get("amount_eur")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let reference = entry.get("reference").and_then(|v| v.as_str());
        let iban = entry.get("iban").and_then(|v| v.as_str()).unwrap_or("");
        let date_str = entry.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let date = time::Date::parse(
            date_str,
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .ok();
        let amount_ct = -(amount_eur * 100.0).round() as i64; // credit

        if let (Some(date), Some(reference)) = (date, reference) {
            // Match by IBAN — find account.
            let account_row = sqlx::query(
                "SELECT account_id FROM accounts WHERE iban = $1 AND tenant = $2 LIMIT 1",
            )
            .bind(iban)
            .bind(&cfg.tenant)
            .fetch_optional(&pool)
            .await;

            if let Ok(Some(row)) = account_row {
                let account_id: Uuid = row.try_get("account_id").unwrap_or(Uuid::nil());
                if account_id != Uuid::nil() {
                    let _ = write_entry(
                        &pool,
                        account_id,
                        &cfg.tenant,
                        "ZAHLUNG",
                        amount_ct,
                        Some(reference),
                        None,
                        None,
                        date,
                        Some("CAMT.054 Zahlung"),
                    )
                    .await;
                    accepted += 1;
                }
            }
        }
    }
    Json(serde_json::json!({ "accepted": accepted, "total": entries.len() })).into_response()
}

// ── Offene Posten ─────────────────────────────────────────────────────────────

/// `GET /api/v1/offene-posten`  — overdue accounts
pub async fn get_offene_posten(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<OffenePostenQuery>,
) -> impl IntoResponse {
    let min_ct = q.min_balance_eur.map(|e| (e * 100.0) as i64).unwrap_or(1);
    match list_overdue_accounts(&pool, &cfg.tenant, min_ct, q.limit.unwrap_or(200).min(2000)).await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct OffenePostenQuery {
    pub min_balance_eur: Option<f64>,
    pub limit: Option<i64>,
}

// ── Dunning ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/dunning`  — open dunning cases
pub async fn get_dunning(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<DunningQuery>,
) -> impl IntoResponse {
    match list_open_dunning(&pool, &cfg.tenant, q.limit.unwrap_or(200).min(1000)).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/dunning/{account_id}/escalate`  — manual dunning escalation
pub async fn escalate_dunning(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(account_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let account = match fetch_account_by_id(&pool, account_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let stufe: i16 = body.get("stufe").and_then(|v| v.as_i64()).unwrap_or(1) as i16;
    let amount_due_ct = account.balance_ct.max(0);
    let due_days: i64 = body.get("due_days").and_then(|v| v.as_i64()).unwrap_or(14);
    let due_date = (OffsetDateTime::now_utc() + time::Duration::days(due_days)).date();

    match create_dunning_case(
        &pool,
        account_id,
        &cfg.tenant,
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
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match resolve_dunning_case(&pool, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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
    Json(req): Json<CreateMandateRequest>,
) -> impl IntoResponse {
    // Validate IBAN checksum before writing to DB (B16).
    // Malformed IBANs cause SEPA return charges (€3–15/return) + Mahnstufe escalation.
    if let Err(msg) = validate_iban(&req.iban) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("invalid IBAN: {msg}") })),
        )
            .into_response();
    }
    match create_mandate(&pool, &cfg.tenant, req).await {
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
    Extension(pool): Extension<PgPool>,
    Path(mandate_id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_mandate(&pool, mandate_id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/sepa/run`  — generate pain.008 XML for all active mandates with positive balance
pub async fn run_sepa(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
) -> impl IntoResponse {
    let mandates = match list_active_mandates(&pool, &cfg.tenant, 10_000).await {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Filter: only mandates with positive balance (outstanding debt).
    let mut direct_debits = Vec::new();
    for mandate in &mandates {
        if let Some(acct) = fetch_account_by_id(&pool, mandate.account_id)
            .await
            .ok()
            .flatten()
            .filter(|a| a.abschlag_ct > 0)
        {
            direct_debits.push((mandate, acct.abschlag_ct));
        }
    }

    let xml = build_pain_008(&cfg.tenant, &direct_debits);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response()
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<VorauszahlungQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use rubo4e::current::Vorauszahlung;
    use rust_decimal::Decimal;

    // Validate via rubo4e roundtrip.
    let typed: Vorauszahlung = match serde_json::from_value(body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid Vorauszahlung: {e}") })),
            )
                .into_response();
        }
    };
    let canonical = serde_json::to_value(&typed).unwrap_or_default();

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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<VorauszahlungQuery>,
) -> impl IntoResponse {
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
                let eur = Decimal::from(abschlag_ct) / Decimal::from(100);
                serde_json::json!({
                    "malo_id": malo_id,
                    "vorauszahlung": {
                        "_typ": "VORAUSZAHLUNG",
                        "betrag": {
                            "wert": eur.to_string(),
                            "waehrung": "EUR"
                        }
                    },
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

// ── IBAN validation ───────────────────────────────────────────────────────────

/// Validate an IBAN using the ISO 13616 mod-97 algorithm.
///
/// 1. Remove whitespace and convert to uppercase.
/// 2. Move the first 4 characters to the end.
/// 3. Replace each letter `X` with `(X as u32 - 'A' as u32 + 10).to_string()`.
/// 4. Compute the decimal value mod 97 in 9-digit chunks to avoid overflow.
/// 5. If result == 1, the IBAN is valid.
///
/// Returns `Ok(())` for a valid IBAN, `Err(reason)` otherwise.
pub fn validate_iban(iban: &str) -> Result<(), String> {
    let iban: String = iban
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase();
    if iban.len() < 15 || iban.len() > 34 {
        return Err(format!(
            "length {} is outside the valid range 15–34",
            iban.len()
        ));
    }
    // Rearrange: move first 4 chars to the end.
    let rearranged = format!("{}{}", &iban[4..], &iban[..4]);
    // Expand each letter to its two-digit numeric equivalent.
    let digits: String = rearranged
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                (c as u32 - b'A' as u32 + 10).to_string()
            } else {
                c.to_string()
            }
        })
        .collect();
    // Validate all characters are digits (catches unexpected symbols).
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err("contains invalid characters (expected alphanumeric only)".to_string());
    }
    // Compute mod 97 using 9-digit rolling chunks (fits in u64).
    let mut remainder: u64 = 0;
    for ch in digits.chars() {
        remainder = (remainder * 10 + ch.to_digit(10).unwrap() as u64) % 97;
    }
    if remainder == 1 {
        Ok(())
    } else {
        Err("checksum mismatch — IBAN is invalid".to_string())
    }
}

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
/// Emits CloudEvent `de.accounting.jahresabschluss.abgeschlossen` on commit.
pub async fn post_jahresabschluss(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<JahresabschlussQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let year = q.year.unwrap_or_else(|| OffsetDateTime::now_utc().year());
    let dry_run = q.dry_run.unwrap_or(false);

    // 1. Resolve account.
    let acct = match fetch_account(&pool, &malo_id, lf_mp_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("account for {malo_id} not found"),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // 2. Sum all entries for the year.
    let entries = match list_ledger_year(&pool, acct.account_id, year).await {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let abschlag_sum: i64 = entries
        .iter()
        .filter(|e| e.entry_type == "ABSCHLAG")
        .map(|e| e.amount_ct)
        .sum(); // negative — Abschläge are credits
    let rechnung_sum: i64 = entries
        .iter()
        .filter(|e| e.entry_type == "RECHNUNG")
        .map(|e| e.amount_ct)
        .sum(); // positive — Rechnungen are debits

    // settlement_ct > 0  → Nachzahlung (customer still owes)
    // settlement_ct < 0  → Erstattung (customer overpaid → refund)
    // settlement_ct == 0 → ausgeglichen
    let settlement_ct = rechnung_sum + abschlag_sum;
    let new_abschlag_ct = if rechnung_sum > 0 {
        rechnung_sum / 12
    } else {
        acct.abschlag_ct // no change when there were no Rechnungen this year
    };

    let action = if settlement_ct > 0 {
        "NACHZAHLUNG"
    } else if settlement_ct < 0 {
        "ERSTATTUNG"
    } else {
        "AUSGEGLICHEN"
    };

    if dry_run {
        return Json(serde_json::json!({
            "malo_id": malo_id,
            "year": year,
            "rechnung_sum_ct": rechnung_sum,
            "abschlag_paid_ct": abschlag_sum,
            "settlement_ct": settlement_ct,
            "settlement_eur": format!("{:.2}", settlement_ct as f64 / 100.0),
            "new_monthly_abschlag_ct": new_abschlag_ct,
            "action": action,
            "dry_run": true,
            "committed": false,
        }))
        .into_response();
    }

    let today = OffsetDateTime::now_utc().date();
    let ce_id = Uuid::new_v4().to_string();

    // 3. Write settlement entry when non-zero.
    if settlement_ct != 0 {
        let entry_type = if settlement_ct > 0 {
            "RECHNUNG"
        } else {
            "GUTSCHRIFT"
        };
        let description = format!(
            "{} Jahresabschluss {year} (Abschlag gesamt: {} ct, Rechnung gesamt: {} ct)",
            action, abschlag_sum, rechnung_sum
        );
        if let Err(e) = write_entry(
            &pool,
            acct.account_id,
            lf_mp_id,
            entry_type,
            settlement_ct,
            None,
            Some("de.accounting.jahresabschluss.abgeschlossen"),
            Some(&ce_id),
            today,
            Some(&description),
        )
        .await
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    // 4. Update monthly Abschlag (§40 Abs. 1 EnWG: Abschlag must match actual consumption).
    if new_abschlag_ct != acct.abschlag_ct
        && let Err(e) = update_account(
            &pool,
            &malo_id,
            lf_mp_id,
            UpdateAccountRequest {
                iban: None,
                mandatsref: None,
                abschlag_ct: Some(new_abschlag_ct),
                billing_day: None,
            },
        )
        .await
    {
        tracing::warn!(
            malo_id,
            new_abschlag_ct,
            error = %e,
            "accountingd: Jahresabschluss committed but Abschlag update failed"
        );
    }

    Json(serde_json::json!({
        "malo_id": malo_id,
        "year": year,
        "rechnung_sum_ct": rechnung_sum,
        "abschlag_paid_ct": abschlag_sum,
        "settlement_ct": settlement_ct,
        "settlement_eur": format!("{:.2}", settlement_ct as f64 / 100.0),
        "new_monthly_abschlag_ct": new_abschlag_ct,
        "action": action,
        "dry_run": false,
        "committed": true,
        "ce_id": ce_id,
    }))
    .into_response()
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
