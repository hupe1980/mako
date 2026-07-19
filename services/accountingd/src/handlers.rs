//! HTTP handlers for `accountingd`.
//!
//! ## Security model
//!
//! - **Inbound webhook** (`POST /webhook`): HMAC-SHA256 verified when `erp_hmac_secret`
//!   is set. Uses `mako_service::webhook::hmac_hex` with `sha256=` prefix.
//!   Dev mode (no secret): accepts all but emits `WARN`.
//! - **REST write endpoints**: OIDC JWT required via `Claims` extractor when
//!   `OidcVerifier` is injected via `Extension`. Dev mode: synthetic claims.
//! - **MCP tools**: protected by `McpAuth` (API-key bearer or OIDC).

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_service::oidc::Claims;
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
        list_overdue_accounts, resolve_dunning_case, update_account_tenanted, upsert_account,
        upsert_vorauszahlung, write_entry,
    },
    sepa::build_pain_008,
};
// Re-export sepa crate's validate_iban so test code can import from this module.
pub use sepa::validate_iban;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert an amount in ct (i64, × 10⁻² EUR) to a `"1234.56"` EUR string.
/// Uses pure integer arithmetic — no f64.
pub fn format_ct_as_eur(ct: i64) -> String {
    let sign = if ct < 0 { "-" } else { "" };
    let abs = ct.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// Constant-time byte comparison (timing-safe) for HMAC verification.
///
/// Avoids early-exit that would leak timing information about the HMAC prefix.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// ── Account endpoints ─────────────────────────────────────────────────────────

/// `GET /api/v1/accounts/{malo_id}`
pub async fn get_account(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
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
    _claims: Claims,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
    Json(req): Json<crate::pg::UpdateAccountRequest>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    let _ = upsert_account(&pool, &malo_id, &lf_mp_id, &cfg.tenant).await;
    match update_account_tenanted(&pool, &malo_id, &lf_mp_id, &cfg.tenant, req).await {
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<LedgerQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
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
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
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
        match update_account_tenanted(
            &pool,
            &malo_id,
            &cfg.tenant, // lf_mp_id defaults to tenant when not specified
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
/// ## Security
///
/// When `erp_hmac_secret` is configured, the `X-Mako-Signature: sha256=...` header
/// is verified before any processing. Requests without a valid signature are rejected
/// with HTTP 403 to prevent fake invoice injection (P0-2 fix).
///
/// Supported event types:
/// - `de.billing.rechnung.erstellt` → debit entry
/// - `de.invoic.receipt.settled`    → credit entry (NNE receipt paid)
/// - `de.invoic.receipt.disputed`   → no entry (dispute logged)
/// - `de.eeg.verguetung.berechnet`  → credit entry (EEG settlement)
pub async fn ingest_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── P0-2: Inbound HMAC verification ─────────────────────────────────────
    if let Some(ref secret) = cfg.erp_hmac_secret {
        let expected = format!(
            "sha256={}",
            mako_service::webhook::hmac_hex(
                {
                    use secrecy::ExposeSecret;
                    secret.expose_secret().as_bytes()
                },
                &body
            )
        );
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // Constant-time comparison to prevent timing attacks
        if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            tracing::warn!("accountingd: inbound webhook HMAC mismatch — rejected");
            return StatusCode::FORBIDDEN.into_response();
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
    let ce_id = ce.get("id").and_then(|v| v.as_str()).map(str::to_owned);
    let data = ce.get("data");
    let today = OffsetDateTime::now_utc().date();

    match ce_type {
        // ── Billing invoice (billingd) ────────────────────────────────────────
        // de.billing.rechnung.erstellt:
        //   is_correction=false → RECHNUNG debit  (customer owes money)
        //   is_correction=true  → STORNO debit/credit (negated amount; billing reversal)
        "de.billing.rechnung.erstellt" => {
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
            let amount_ct: i64 = data
                .and_then(|d| d.get("rechnung"))
                .and_then(|r| r.get("gesamtbrutto"))
                .and_then(|g| g.get("wert"))
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    use rust_decimal::Decimal;
                    use std::str::FromStr;
                    Decimal::from_str(s).ok().map(|d| {
                        (d * Decimal::from(100))
                            .round()
                            .to_string()
                            .parse::<i64>()
                            .unwrap_or(0)
                    })
                })
                .unwrap_or(0);
            let account_id = upsert_account(&pool, malo_id, lf_mp_id, &cfg.tenant)
                .await
                .unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() && amount_ct != 0 {
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
                if let Err(e) = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    entry_type,
                    amount_ct,
                    record_id,
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some(description),
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                    );
                }
            }
            StatusCode::OK.into_response()
        }

        // ── Credit note (billingd) ─────────────────────────────────────────────
        // de.billing.gutschrift.erstellt: credit note, negative amount (credit to customer).
        "de.billing.gutschrift.erstellt" => {
            let malo_id = data
                .and_then(|d| d.get("malo_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let lf_mp_id = data
                .and_then(|d| d.get("lf_mp_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&cfg.tenant);
            let gutschrift_ct: i64 = data
                .and_then(|d| d.get("betrag_eur"))
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
            let account_id = upsert_account(&pool, malo_id, lf_mp_id, &cfg.tenant)
                .await
                .unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() && gutschrift_ct != 0 {
                let record_id = data
                    .and_then(|d| d.get("record_id"))
                    .and_then(|v| v.as_str());
                if let Err(e) = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    "GUTSCHRIFT",
                    gutschrift_ct,
                    record_id,
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some("Gutschrift / Rechnungskorrektur"),
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                    );
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
        "de.invoic.receipt.settled" => {
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
                && let Ok(account_id) =
                    upsert_account(&pool, malo_id, &cfg.tenant, &cfg.tenant).await
                && account_id != Uuid::nil()
            {
                #[allow(clippy::collapsible_if)]
                if let Err(e) = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    "ZAHLUNG",
                    settlement_ct,
                    ce_id.as_deref(),
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some("NNE-Zahlung bestätigt (INVOIC settled)"),
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                    );
                }
            }
            StatusCode::OK.into_response()
        }

        // ── EEG Einspeisevergütung (einsd) ────────────────────────────────────
        // de.eeg.verguetung.berechnet: fixed-rate EEG settlement → EEG_GUTSCHRIFT credit.
        // When cfg.eeg.auto_payout = true: also auto-generates pain.001 SEPA Credit Transfer
        // (SCT Inst or SCT CORE per cfg.eeg.sepa_instant) for immediate payout to plant operator.
        "de.eeg.verguetung.berechnet" => {
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
                #[allow(clippy::collapsible_if)]
                if let Err(e) = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    "EEG_GUTSCHRIFT",
                    amount_ct,
                    ce_id.as_deref(),
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some("EEG Einspeisevergütung §21 EEG"),
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                    );
                }

                // ── SCT Inst / SCT CORE auto-payout ─────────────────────────────
                // If [eeg].auto_payout = true: generate pain.001 immediately.
                // Creditor IBAN from CE (bank_iban forwarded by einsd) takes
                // priority; falls back to account zahlungsinformation.iban.
                if cfg.eeg.auto_payout {
                    // Try CE-supplied bank_iban first; fall back to zahlungsinformation
                    let creditor_iban_opt = bank_iban.clone();

                    if let Some(creditor_iban) = creditor_iban_opt {
                        let creditor_name = zahlungsempfaenger
                            .as_deref()
                            .unwrap_or("EEG Einspeiser")
                            .to_owned();
                        // Spawn detached so we don't hold the webhook response while
                        // the bank adapter call happens.
                        let cfg_clone = Arc::clone(&cfg);
                        let pool_clone = pool.clone();
                        let malo_owned = malo_id.to_owned();
                        let ce_id_owned = ce_id.clone();
                        let tr_id_owned = tr_id.clone();
                        tokio::spawn(async move {
                            create_eeg_payout_order(
                                &cfg_clone,
                                &pool_clone,
                                EegPayoutParams {
                                    malo_id: &malo_owned,
                                    account_id,
                                    amount_ct: amount_ct.unsigned_abs() as i64,
                                    creditor_iban: &creditor_iban,
                                    creditor_name: &creditor_name,
                                    tr_id: tr_id_owned.as_deref(),
                                    billing_year,
                                    billing_month,
                                    source_ce_id: ce_id_owned.as_deref(),
                                },
                            )
                            .await;
                        });
                    } else {
                        // No IBAN in CE — look up from zahlungsinformation (async).
                        let cfg_clone = Arc::clone(&cfg);
                        let pool_clone = pool.clone();
                        let malo_owned = malo_id.to_owned();
                        let ce_id_owned = ce_id.clone();
                        let tr_id_owned = tr_id.clone();
                        let bic_owned = bank_bic.clone();
                        tokio::spawn(async move {
                            // Fetch zahlungsinformation IBAN from DB
                            let zi: Option<serde_json::Value> = sqlx::query(
                                "SELECT zahlungsinformation FROM accounts \
                                 WHERE malo_id = $1 AND tenant = $2",
                            )
                            .bind(&malo_owned)
                            .bind(&cfg_clone.tenant)
                            .fetch_optional(&pool_clone)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|r| {
                                use sqlx::Row;
                                r.try_get("zahlungsinformation").unwrap_or(None)
                            });

                            let fallback_iban = zi
                                .as_ref()
                                .and_then(|z| z.get("bankverbindung"))
                                .and_then(|b| b.get("iban"))
                                .and_then(|v| v.as_str())
                                .map(str::to_owned);
                            let fallback_name = zi
                                .as_ref()
                                .and_then(|z| z.get("kontoinhaber"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("EEG Einspeiser")
                                .to_owned();
                            let _ = bic_owned; // included in pain.001 only if bank_bic in CE
                            if let Some(iban) = fallback_iban {
                                create_eeg_payout_order(
                                    &cfg_clone,
                                    &pool_clone,
                                    EegPayoutParams {
                                        malo_id: &malo_owned,
                                        account_id,
                                        amount_ct: amount_ct.unsigned_abs() as i64,
                                        creditor_iban: &iban,
                                        creditor_name: &fallback_name,
                                        tr_id: tr_id_owned.as_deref(),
                                        billing_year,
                                        billing_month,
                                        source_ce_id: ce_id_owned.as_deref(),
                                    },
                                )
                                .await;
                            } else {
                                tracing::info!(
                                    malo_id = %malo_owned,
                                    "accountingd: auto_payout=true but no creditor IBAN available — \
                                     set bank_iban in EEG plant record or PUT zahlungsinformation"
                                );
                            }
                        });
                    }
                }
            }
            StatusCode::OK.into_response()
        }

        // ── EEG Direktvermarktung Marktprämie (einsd) ─────────────────────────
        // de.eeg.marktpraemie.berechnet: Direktvermarktung / Ausschreibung settlement.
        // Gleitende Marktprämie (§20 EEG) + Managementprämie → EEG_MARKTPRAEMIE credit.
        "de.eeg.marktpraemie.berechnet" => {
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
            let account_id = upsert_account(&pool, malo_id, &cfg.tenant, &cfg.tenant)
                .await
                .unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() && amount_ct != 0 {
                #[allow(clippy::collapsible_if)]
                if let Err(e) = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    "EEG_MARKTPRAEMIE",
                    amount_ct,
                    ce_id.as_deref(),
                    Some(ce_type),
                    ce_id.as_deref(),
                    today,
                    Some("EEG Direktvermarktung Marktprämie §20 EEG"),
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "accountingd: ledger write FAILED — entry discarded; investigate DB health"
                    );
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

/// `POST /api/v1/payments/import`  — ingest CAMT.054 bank statement (JSON array).
///
/// Each entry: `{ "iban": "...", "amount_eur": "155.42", "reference": "...", "date": "YYYY-MM-DD",
///               "bank_transaction_id": "..." }`
///
/// Uses `sepa::camt054::parse_simple_json` — **no f64 rounding errors**.
/// Positive `amount_eur` → ZAHLUNG credit. Negative → BANKRUECKLAST debit.
///
/// ## CAMT.054 deduplication
///
/// Each entry is checked against `bank_import_log` before processing.
/// If `bank_transaction_id` is present and already imported, the entry is skipped
/// and counted as `deduplicated` — no duplicate ledger entries are created.
/// When `bank_transaction_id` is absent, a stable hash of (iban+amount+date+reference)
/// is used as the deduplication key.
pub async fn import_payments(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(entries): Json<Vec<serde_json::Value>>,
) -> impl IntoResponse {
    let mut accepted = 0usize;
    let mut deduplicated = 0usize;
    let mut skipped = 0usize;

    for raw in &entries {
        let Some(entry) = sepa::camt054::parse_simple_json(raw) else {
            skipped += 1;
            continue;
        };
        let Ok(date) = time::Date::parse(
            &entry.value_date,
            &time::format_description::well_known::Iso8601::DEFAULT,
        ) else {
            skipped += 1;
            continue;
        };

        // ── CAMT.054 deduplication ───────────────────────────────────────────
        // Derive a stable bank_transaction_id: use the one in the JSON if present,
        // otherwise compute a deterministic hash from the entry's identifying fields.
        let bank_txn_id = raw
            .get("bank_transaction_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned())
            .unwrap_or_else(|| {
                // Fallback: hash (iban + amount + date + reference) for stability
                let key = format!(
                    "{}|{}|{}|{}",
                    &entry.iban,
                    entry.to_ledger_ct(),
                    &entry.value_date,
                    &entry.reference
                );
                // Simple deterministic key (not cryptographic — only for dedup)
                format!(
                    "{:016x}",
                    key.bytes().fold(0u64, |acc, b| {
                        acc.wrapping_mul(1099511628211).wrapping_add(b as u64)
                    })
                )
            });

        // Check deduplication log
        match crate::pg::bank_import_already_processed(&pool, &cfg.tenant, &bank_txn_id).await {
            Ok(true) => {
                tracing::debug!(
                    bank_txn_id = %bank_txn_id,
                    iban = %entry.iban,
                    "accountingd: CAMT.054 entry already imported — skipping (dedup)"
                );
                deduplicated += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %e, "accountingd: dedup check failed — processing entry anyway");
            }
            Ok(false) => {}
        }

        let account_row =
            sqlx::query("SELECT account_id FROM accounts WHERE iban_hash = encode(digest(upper(replace($1,' ','')), 'sha256'), 'hex') AND tenant = $2 LIMIT 1")
                .bind(&entry.iban)
                .bind(&cfg.tenant)
                .fetch_optional(&pool)
                .await;

        if let Ok(Some(row)) = account_row {
            let account_id: Uuid = row.try_get("account_id").unwrap_or(Uuid::nil());
            if account_id != Uuid::nil() {
                let entry_type = if entry.is_return() {
                    "BANKRUECKLAST"
                } else {
                    "ZAHLUNG"
                };
                let ledger_result = write_entry(
                    &pool,
                    account_id,
                    &cfg.tenant,
                    entry_type,
                    entry.to_ledger_ct(),
                    Some(entry.reference.as_str()),
                    None,
                    None,
                    date,
                    Some(entry.description().as_str()),
                )
                .await;

                match ledger_result {
                    Ok(ledger_id) => {
                        // Record in dedup log to prevent re-import
                        if let Err(e) = crate::pg::record_bank_import(
                            &pool,
                            &cfg.tenant,
                            &bank_txn_id,
                            entry.to_ledger_ct().abs(),
                            Some(&entry.iban),
                            date,
                            ledger_id,
                        )
                        .await
                        {
                            tracing::warn!(error = %e, "accountingd: bank_import_log insert failed");
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
            } else {
                skipped += 1;
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

// ── Offene Posten ─────────────────────────────────────────────────────────────

/// `GET /api/v1/offene-posten`  — overdue accounts
pub async fn get_offene_posten(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<OffenePostenQuery>,
) -> impl IntoResponse {
    // P0-1 fix: parse min_balance_eur as a decimal string to avoid f64 rounding errors.
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
    let account = match fetch_account_by_id(&pool, account_id, &cfg.tenant).await {
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
    _claims: Claims,
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

/// `DELETE /api/v1/sepa/mandates/{mandate_id}`  — revoke SEPA mandate.
///
/// Sets `revoked_at = today` on the mandate. Revoked mandates are excluded from
/// future pain.008 generation (§58 ZAG: debtor may revoke at any time until
/// the cut-off time of the collection date).
///
/// Does NOT affect existing `accounts.iban` or `mandatsref` columns —
/// update those separately if needed via `PUT /api/v1/accounts/{malo_id}`.
pub async fn delete_mandate(
    Extension(pool): Extension<PgPool>,
    Path(mandate_id): Path<Uuid>,
) -> impl IntoResponse {
    let today = time::OffsetDateTime::now_utc().date();
    let rows = sqlx::query(
        "UPDATE sepa_mandates SET revoked_at = $1, updated_at = now() \
         WHERE mandate_id = $2 AND revoked_at IS NULL",
    )
    .bind(today)
    .bind(mandate_id)
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
) -> impl IntoResponse {
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

    // P1-2 fix: validate creditor_iban before generating pain.008.
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
    let creditor_id = cfg.creditor_id.as_deref();

    match build_pain_008(creditor_iban, creditor_name, creditor_id, &direct_debits) {
        Ok(batches) => {
            // Return all batches as a JSON array (FRST, RCUR, etc. separate per Rulebook §3.8)
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "batch_count": batches.len(),
                    "batches": batches.iter().map(|b| serde_json::json!({
                        "sequence_type": format!("{:?}", b.sequence_type),
                        "entry_count": b.entry_count,
                        "total_ct": b.total_ct,
                        "xml": &b.xml,
                    })).collect::<Vec<_>>(),
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

// ── Manual booking ────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/accounts/{malo_id}/buchen`.
#[derive(Debug, serde::Deserialize)]
pub struct BuchenRequest {
    /// Buchungsart. Must be a valid `entry_type` value.
    /// Allowed: `RECHNUNG`, `ZAHLUNG`, `GUTSCHRIFT`, `BANKRUECKLAST`,
    /// `MAHNGEBUEHR`, `ABSCHLAG`, `KORREKTUR`, `STORNO`.
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
/// Supply `reference_id` — re-posting with the same `reference_id` will create
/// a new entry (no idempotency guard on this endpoint; use with care).
pub async fn post_buchen(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Json(req): Json<BuchenRequest>,
) -> impl IntoResponse {
    use time::format_description::well_known::Iso8601;

    // Validate entry_type against the allowed set (DB constraint mirrors this).
    const ALLOWED: &[&str] = &[
        "RECHNUNG",
        "ZAHLUNG",
        "GUTSCHRIFT",
        "EEG_GUTSCHRIFT",
        "EEG_MARKTPRAEMIE",
        "BANKRUECKLAST",
        "MAHNGEBUEHR",
        "ABSCHLAG",
        "JAHRESABSCHLUSS",
        "KORREKTUR",
        "STORNO",
    ];
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
    let account_id = match upsert_account(&pool, &malo_id, &lf_mp_id, &cfg.tenant).await {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let today = OffsetDateTime::now_utc().date();
    let booking_date = req
        .booking_date
        .as_deref()
        .and_then(|s| time::Date::parse(s, &Iso8601::DEFAULT).ok())
        .unwrap_or(today);
    let value_date = req
        .value_date
        .as_deref()
        .and_then(|s| time::Date::parse(s, &Iso8601::DEFAULT).ok())
        .unwrap_or(booking_date);

    match crate::pg::write_entry_with_value_date(
        &pool,
        account_id,
        &cfg.tenant,
        &req.entry_type,
        req.amount_ct,
        req.reference_id.as_deref(),
        None,
        None,
        booking_date,
        value_date,
        req.description.as_deref(),
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
    let acct = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
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

    // Sum ALL debit entries for the year (§40 Abs. 1 EnWG: Jahresabrechnung must
    // reflect actual billed amounts, including corrections/stornos).
    // RECHNUNG   = regular invoices (positive/debit)
    // STORNO     = billing reversals (may be negative/credit)
    // MAHNGEBUEHR = dunning fees (positive/debit)
    let rechnung_sum: i64 = entries
        .iter()
        .filter(|e| matches!(e.entry_type.as_str(), "RECHNUNG" | "STORNO" | "MAHNGEBUEHR"))
        .map(|e| e.amount_ct)
        .sum();

    // settlement_ct > 0  → Nachzahlung (customer still owes)
    // settlement_ct < 0  → Erstattung (customer overpaid → refund)
    // settlement_ct == 0 → ausgeglichen
    let settlement_ct = rechnung_sum + abschlag_sum;
    // New monthly Abschlag = actual annual billed ÷ 12 (§40 Abs. 1 EnWG).
    // Only update when there were actual Rechnungen this year; keep unchanged
    // for years with no billed amounts to avoid zeroing the Abschlag on empty years.
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
        return Json(serde_json::json!({
            "malo_id": malo_id,
            "year": year,
            "rechnung_sum_ct": rechnung_sum,
            "abschlag_paid_ct": abschlag_sum,
            "settlement_ct": settlement_ct,
            "settlement_eur": format_ct_as_eur(settlement_ct),
            "new_monthly_abschlag_ct": new_abschlag_ct,
            "action": action,
            "dry_run": true,
            "committed": false,
        }))
        .into_response();
    }

    let today = OffsetDateTime::now_utc().date();
    let ce_id = Uuid::new_v4().to_string();

    // 3. Write settlement entry when non-zero using JAHRESABSCHLUSS entry type
    // for clear auditability separate from regular RECHNUNG/GUTSCHRIFT entries.
    if settlement_ct != 0 {
        let description = format!(
            "{} Jahresabschluss {year} (Abschlag gesamt: {} ct, Rechnung gesamt: {} ct)",
            action, abschlag_sum, rechnung_sum
        );
        if let Err(e) = write_entry(
            &pool,
            acct.account_id,
            lf_mp_id,
            "JAHRESABSCHLUSS",
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
        && let Err(e) = update_account_tenanted(
            &pool,
            &malo_id,
            lf_mp_id,
            &cfg.tenant,
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
        "settlement_eur": format_ct_as_eur(settlement_ct),
        "new_monthly_abschlag_ct": new_abschlag_ct,
        "action": action,
        "dry_run": false,
        "committed": true,
        "ce_id": ce_id,
    }))
    .into_response()
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ZahlungsQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use rubo4e::current::Zahlungsinformation;
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();

    let typed: Zahlungsinformation = match serde_json::from_value(body) {
        Ok(z) => z,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid Zahlungsinformation: {e}") })),
            )
                .into_response();
        }
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

    let canonical = serde_json::to_value(&typed).unwrap_or_default();

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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ZahlungsQuery>,
) -> impl IntoResponse {
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

// ── P1-3: Open-item management ────────────────────────────────────────────────

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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match crate::pg::list_open_items(&pool, account.account_id, &cfg.tenant).await {
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

// ── P1-4: GDPR Art. 17 anonymization ─────────────────────────────────────────

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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
    Json(req): Json<AnonymizeRequest>,
) -> impl IntoResponse {
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ReconcileQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    let account = match fetch_account(&pool, &malo_id, lf_mp_id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let repair = q.repair.unwrap_or(false);
    match crate::pg::reconcile_balance(&pool, account.account_id, &cfg.tenant, repair).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Query(q): Query<EegPayoutQuery>,
) -> impl IntoResponse {
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(payout_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Json(req): Json<RunEegPayoutsRequest>,
) -> impl IntoResponse {
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
    let today = time::OffsetDateTime::now_utc();
    let year = req.billing_year.unwrap_or(today.year() as i16);
    let month = req.billing_month.unwrap_or(today.month() as i16);

    // Fetch all EEG_GUTSCHRIFT ledger entries for the given period that do not
    // yet have a payout order.
    let ungrouped_sql = if req.malo_id.is_some() {
        r"SELECT le.id, le.account_id, le.amount_ct, le.reference_id, le.description, le.ce_id,
                 a.malo_id, a.zahlungsinformation
          FROM ledger_entries le
          JOIN accounts a ON a.account_id = le.account_id
          WHERE le.entry_type = 'EEG_GUTSCHRIFT'
            AND a.tenant = $1
            AND a.malo_id = $2
            AND EXTRACT(YEAR  FROM le.booking_date) = $3
            AND EXTRACT(MONTH FROM le.booking_date) = $4
            AND le.id NOT IN (SELECT source_ce_id FROM eeg_payout_orders WHERE tenant = $1)"
    } else {
        r"SELECT le.id, le.account_id, le.amount_ct, le.reference_id, le.description, le.ce_id,
                 a.malo_id, a.zahlungsinformation
          FROM ledger_entries le
          JOIN accounts a ON a.account_id = le.account_id
          WHERE le.entry_type = 'EEG_GUTSCHRIFT'
            AND a.tenant = $1
            AND EXTRACT(YEAR  FROM le.booking_date) = $3
            AND EXTRACT(MONTH FROM le.booking_date) = $4
            AND le.id::text NOT IN (SELECT source_ce_id FROM eeg_payout_orders WHERE tenant = $1)"
    };

    let rows = if let Some(ref malo) = req.malo_id {
        sqlx::query(ungrouped_sql)
            .bind(&cfg.tenant)
            .bind(malo)
            .bind(year as f64)
            .bind(month as f64)
            .fetch_all(&pool)
            .await
    } else {
        sqlx::query(ungrouped_sql)
            .bind(&cfg.tenant)
            .bind(&cfg.tenant) // placeholder for $2 slot not used in this branch
            .bind(year as f64)
            .bind(month as f64)
            .fetch_all(&pool)
            .await
    };

    let rows = match rows {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let mut generated = 0usize;
    let mut skipped_no_iban = 0usize;
    let mut errors = 0usize;

    for row in &rows {
        use sqlx::Row;
        let malo_id: String = row.get("malo_id");
        let account_id: uuid::Uuid = row.get("account_id");
        let amount_ct: i64 = row.get::<i64, _>("amount_ct").abs();
        let ce_id: Option<String> = row.try_get("ce_id").unwrap_or(None);
        let zahlungsinformation: Option<serde_json::Value> =
            row.try_get("zahlungsinformation").unwrap_or(None);

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

        let pain_xml = match build_pain_001(
            &debtor_iban,
            &[(&creditor_iban, &creditor_name, amount_ct, &e2e_ref)],
            use_instant,
        ) {
            Ok(xml) => xml,
            Err(e) => {
                tracing::warn!(malo_id, error = %e, "accountingd: pain.001 build failed");
                errors += 1;
                continue;
            }
        };

        // Insert payout order (idempotent via unique source_ce_id).
        let insert_result = sqlx::query(
            r"INSERT INTO eeg_payout_orders
                  (malo_id, account_id, billing_year, billing_month, amount_ct,
                   creditor_iban, creditor_name, payment_type, end_to_end_ref,
                   pain001_xml, source_ce_id, tenant)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(payout_id): Path<uuid::Uuid>,
    Json(req): Json<Pain002StatusUpdate>,
) -> impl IntoResponse {
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
    .fetch_optional(&pool)
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

    // Emit CloudEvent for RJCT / CANC so ERP can alert the operator.
    if req.status != "ACCP"
        && let Some(ref webhook_url) = cfg.erp_webhook_url
    {
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.accounting.eeg.payout.rejected",
            "source": format!("urn:accountingd:tenant:{}", cfg.tenant),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": time::OffsetDateTime::now_utc().to_string(),
            "subject": malo_id,
            "datacontenttype": "application/json",
            "data": {
                "payout_id":     payout_id.to_string(),
                "malo_id":       malo_id,
                "end_to_end_ref": e2e_ref,
                "amount_ct":     amount_ct,
                "payment_type":  payment_type,
                "pain002_status": req.status,
                "pain002_reason": req.reason_code,
            }
        });
        let client = mako_service::http::default_client();
        let _ = client
            .post(webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .json(&ce)
            .send()
            .await;
    }

    StatusCode::NO_CONTENT.into_response()
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
    let client = mako_service::http::default_client();
    let mut req = client
        .post(bank_url)
        .header("Content-Type", "application/xml")
        .body(pain_xml.to_owned());
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

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

    let pain_xml = match build_pain_001(
        debtor_iban,
        &[(
            params.creditor_iban,
            params.creditor_name,
            params.amount_ct,
            &e2e_ref,
        )],
        use_instant,
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
               pain001_xml, source_ce_id, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
) -> impl IntoResponse {
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
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
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    _claims: Claims,
    Path(malo_id): Path<String>,
    Json(req): Json<CreateInterestChargeRequest>,
) -> impl IntoResponse {
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
        &pool,
        account.account_id,
        &cfg.tenant,
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<AccountQuery>,
) -> impl IntoResponse {
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
    _claims: Claims,
    Path(malo_id): Path<String>,
    Json(mut req): Json<crate::pg::CreatePaymentPlanRequest>,
) -> impl IntoResponse {
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
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<AccountingdConfig>>,
    Path(plan_id): Path<Uuid>,
) -> impl IntoResponse {
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
    Path(plan_id): Path<Uuid>,
) -> impl IntoResponse {
    match crate::pg::cancel_payment_plan(&pool, plan_id, &cfg.tenant, Some(claims.sub())).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}
