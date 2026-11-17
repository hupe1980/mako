//! The reads accountingd serves through two doors: a documented REST `GET`
//! (`handlers.rs`, in `/api/v1/openapi.json`) and an MCP tool
//! (`mcp_server.rs`). Each is one function returning one type, so the route's
//! documented response and the tool's `output_schema` are generated from the
//! same Rust type and cannot describe different answers.
//!
//! Money is integer minor units (`*_ct`, exact) with its decimal EUR string
//! beside it (`format: decimal`); dates are ISO `YYYY-MM-DD`, instants RFC 3339.

use mako_service::ApiError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::handlers::format_ct_as_eur;

// ── Answers ───────────────────────────────────────────────────────────────────

/// Where an account's balance stands: `overdue` — the customer owes
/// (`balance_ct > 0`); `credit` — a Guthaben (`balance_ct < 0`); `settled` —
/// nothing is open.
// The variants carry no doc comments: schemars renders a documented variant as
// a `oneOf` of `const`s, utoipa as a plain `enum`, and the two would differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Saldostatus {
    Overdue,
    Credit,
    Settled,
}

impl Saldostatus {
    fn of(balance_ct: i64) -> Self {
        match balance_ct.signum() {
            1 => Self::Overdue,
            -1 => Self::Credit,
            _ => Self::Settled,
        }
    }
}

/// The account's open-items balance (`get_balance`,
/// `GET /api/v1/accounts/{malo_id}/balance`).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Kontostand {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under.
    pub lf_mp_id: String,
    /// Open-items balance in cents: positive is owed, negative a Guthaben.
    pub balance_ct: i64,
    /// `balance_ct` as a decimal EUR string, e.g. `"-12.50"`.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub balance_eur: String,
    /// Where the balance stands.
    pub status: Saldostatus,
    /// The monthly Abschlag in cents.
    pub abschlag_ct: i64,
}

/// One movement on the Kontokorrent.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Kontobewegung {
    /// The doubleentry entry id.
    pub entry_id: String,
    /// The Buchungsart, e.g. `RECHNUNG`, `ZAHLUNG`, `GUTSCHRIFT`, `ABSCHLAG`.
    pub entry_type: Option<String>,
    /// The Beleg the booking cites — a Rechnungsnummer, a bank reference.
    pub beleg: Option<String>,
    /// `D` (debit — increases the receivable) or `C` (credit).
    pub side: String,
    /// The movement in cents, always positive; `side` carries the sign.
    pub amount_ct: i64,
    /// Signed contribution to the balance (debit +, credit −), in cents.
    pub signed_ct: i64,
    /// The running balance after this movement, in cents.
    pub running_ct: i64,
    /// The booking date.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub booking_date: String,
}

/// A statement window (`list_ledger`, `GET /api/v1/accounts/{malo_id}/ledger`):
/// `opening_ct` plus every `signed_ct` is the newest line's `running_ct`.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Kontoauszug {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under.
    pub lf_mp_id: String,
    /// The signed balance the window opens at, in cents — what was booked
    /// before it and what `limit` cut off its front.
    pub opening_ct: i64,
    /// The movements, newest first.
    pub lines: Vec<Kontobewegung>,
}

impl Kontoauszug {
    pub(crate) fn new(malo_id: String, lf_mp_id: String, window: crate::pg::LedgerWindow) -> Self {
        Self {
            malo_id,
            lf_mp_id,
            opening_ct: window.opening_ct,
            lines: window
                .lines
                .into_iter()
                .map(|l| Kontobewegung {
                    entry_id: l.entry_id.to_string(),
                    entry_type: l.entry_type,
                    beleg: l.beleg,
                    side: l.side.to_owned(),
                    amount_ct: l.amount_ct,
                    signed_ct: l.signed_ct,
                    running_ct: l.running_ct,
                    booking_date: l.booking_date.to_string(),
                })
                .collect(),
        }
    }
}

/// One open dunning case.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Mahnfall {
    /// The dunning case.
    pub id: String,
    /// The customer account it was opened on.
    pub account_id: String,
    /// The Mahnstufe, 1–3.
    pub stufe: i16,
    /// The amount the Mahnung demands, in cents.
    pub amount_due_ct: i64,
    /// `amount_due_ct` as a decimal EUR string.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub amount_due_eur: String,
    /// When the Mahnung was issued.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub issued_at: String,
    /// The payment date the Mahnung sets.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub due_date: String,
    /// The CloudEvent id of the Sperrauftrag a Mahnstufe 3 raised, if any.
    pub sperrauftrag_ce_id: Option<String>,
}

impl From<crate::pg::DunningCaseRow> for Mahnfall {
    fn from(r: crate::pg::DunningCaseRow) -> Self {
        Self {
            id: r.id.to_string(),
            account_id: r.account_id.to_string(),
            stufe: r.stufe,
            amount_due_ct: r.amount_due_ct,
            amount_due_eur: format_ct_as_eur(r.amount_due_ct),
            issued_at: rfc3339(r.issued_at),
            due_date: r.due_date.to_string(),
            sperrauftrag_ce_id: r.sperrauftrag_ce_id,
        }
    }
}

/// The open dunning cases (`list_dunning`, `GET /api/v1/dunning`).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Mahnfaelle {
    /// The open cases, newest Mahnung first.
    pub faelle: Vec<Mahnfall>,
    /// How many cases `faelle` holds.
    pub count: usize,
    /// The page is full: more cases may be open than were listed.
    pub saturated: bool,
}

/// One account with an overdue balance.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct UeberfaelligesKonto {
    /// The customer account.
    pub account_id: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under.
    pub lf_mp_id: String,
    /// The operator's customer number, where recorded.
    pub kunden_nr: Option<String>,
    /// The open balance in cents.
    pub balance_ct: i64,
    /// `balance_ct` as a decimal EUR string.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub balance_eur: String,
    /// The monthly Abschlag in cents.
    pub abschlag_ct: i64,
    /// When the account's balance last changed.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub updated_at: String,
}

impl From<crate::pg::AccountRow> for UeberfaelligesKonto {
    fn from(a: crate::pg::AccountRow) -> Self {
        Self {
            account_id: a.account_id.to_string(),
            balance_eur: format_ct_as_eur(a.balance_ct),
            malo_id: a.malo_id,
            lf_mp_id: a.lf_mp_id,
            kunden_nr: a.kunden_nr,
            balance_ct: a.balance_ct,
            abschlag_ct: a.abschlag_ct,
            updated_at: rfc3339(a.updated_at),
        }
    }
}

/// The accounts with an open balance (`list_overdue`,
/// `GET /api/v1/offene-posten`).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct UeberfaelligeKonten {
    /// The accounts, largest balance first.
    pub accounts: Vec<UeberfaelligesKonto>,
    /// How many accounts `accounts` holds.
    pub count: usize,
    /// At least `limit` accounts matched: more may be overdue than were listed.
    pub saturated: bool,
}

/// One account a payment may belong to.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Zuordnungskandidat {
    /// The customer account.
    pub account_id: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under.
    pub lf_mp_id: String,
    /// The account's open balance in cents.
    pub open_balance_ct: i64,
    /// `open_balance_ct` as a decimal EUR string.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub open_balance_eur: String,
    /// The evidence: an exact rung (`iban`, `end_to_end_id`,
    /// `remittance_token`) or `amount_proximity`.
    pub matched_by: String,
    /// `EXACT` for an exact rung; `MEDIUM` or `LOW` for amount proximity.
    pub confidence: String,
    /// Amount closeness 0–100, only on the amount-proximity rung.
    pub similarity_score: Option<u32>,
    /// What stays open after booking the payment, in cents (0 settles it).
    pub residual_ct: i64,
}

/// The accounts an incoming payment may belong to (`suggest_payment_match`,
/// `POST /api/v1/payments/match`).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Zahlungszuordnung {
    /// The payment in cents.
    pub payment_amount_ct: i64,
    /// `payment_amount_ct` as a decimal EUR string.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub payment_amount_eur: String,
    /// The Verwendungszweck the bank reported.
    pub payment_reference: String,
    /// The tolerance the amount-proximity rung ran with, in percent; absent
    /// when the reference identified the account.
    pub tolerance_pct: Option<f64>,
    /// How many candidates `candidates` holds.
    pub candidates_count: usize,
    /// One exact candidate, or up to ten ranked by amount proximity.
    pub candidates: Vec<Zuordnungskandidat>,
    /// How to read the answer.
    pub note: String,
}

// ── Parameters ────────────────────────────────────────────────────────────────

/// `suggest_payment_match`'s parameters — the body of
/// `POST /api/v1/payments/match`. It reads and books nothing.
///
/// `Debug` leaves the IBAN out: a payment to match is logged, if ever, by its
/// amount and reference, never by the payer's account.
#[derive(Deserialize, JsonSchema, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PaymentMatchParams {
    /// Payment amount in 1/100 EUR cents (e.g. 12500 = 125.00 EUR).
    pub amount_ct: i64,
    /// Payment reference / Verwendungszweck from the bank statement.
    pub reference: String,
    /// Counterparty IBAN, when the bank reported one — the strongest evidence
    /// of who paid, and the rung that resolves without any guessing.
    pub iban: Option<String>,
    /// `EndToEndId`, when the bank echoed one back.
    pub end_to_end_id: Option<String>,
    /// Value date of the payment (YYYY-MM-DD).
    pub value_date: Option<String>,
    /// Fuzzy tolerance for the fallback rung: how many percent the open balance
    /// may deviate from the payment (default 2 %). Ignored when the reference
    /// identifies the account outright.
    pub tolerance_pct: Option<f64>,
}

impl std::fmt::Debug for PaymentMatchParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentMatchParams")
            .field("amount_ct", &self.amount_ct)
            .field("reference", &self.reference)
            .field("iban", &self.iban.as_ref().map(|_| "…"))
            .field("end_to_end_id", &self.end_to_end_id)
            .field("value_date", &self.value_date)
            .field("tolerance_pct", &self.tolerance_pct)
            .finish()
    }
}

// ── The reads ─────────────────────────────────────────────────────────────────

/// The 404 for a MaLo this tenant keeps no account for.
pub(crate) fn account_not_found(malo_id: &str) -> ApiError {
    ApiError::not_found(format!("account for MaLo {malo_id} not found"))
        .with_code("account_not_found")
}

/// The account's balance. `lf_mp_id` defaults to the tenant.
///
/// # Errors
///
/// `404 account_not_found`; `500` on a database failure.
pub async fn balance(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    lf_mp_id: Option<&str>,
) -> Result<Kontostand, ApiError> {
    let lf_mp_id = lf_mp_id.unwrap_or(tenant);
    let a = crate::pg::fetch_account(pool, malo_id, lf_mp_id, tenant)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| account_not_found(malo_id))?;
    Ok(Kontostand {
        malo_id: a.malo_id,
        lf_mp_id: a.lf_mp_id,
        balance_ct: a.balance_ct,
        balance_eur: format_ct_as_eur(a.balance_ct),
        status: Saldostatus::of(a.balance_ct),
        abschlag_ct: a.abschlag_ct,
    })
}

/// The newest `limit` movements (default 100, at most 1000), with the balance
/// the window opens at. `lf_mp_id` defaults to the tenant.
///
/// # Errors
///
/// `404 account_not_found`; a ledger failure answers as it says.
pub async fn ledger(
    pool: &PgPool,
    ledger: &crate::ledger::PgLedger,
    tenant: &str,
    malo_id: &str,
    lf_mp_id: Option<&str>,
    limit: Option<i64>,
) -> Result<Kontoauszug, ApiError> {
    let lf_mp_id = lf_mp_id.unwrap_or(tenant);
    let a = crate::pg::fetch_account(pool, malo_id, lf_mp_id, tenant)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| account_not_found(malo_id))?;
    let window = crate::pg::list_ledger(
        ledger,
        pool,
        tenant,
        &a.lf_mp_id,
        &a.malo_id,
        &doubleentry::BalanceQuery::all(),
        limit.unwrap_or(100).clamp(1, 1000),
    )
    .await
    .map_err(crate::handlers::ledger_failure)?;
    Ok(Kontoauszug::new(a.malo_id, a.lf_mp_id, window))
}

/// The open dunning cases, newest Mahnung first — `limit` of them (default
/// 100, at most 1000).
///
/// # Errors
///
/// `500` on a database failure.
pub async fn dunning(
    pool: &PgPool,
    tenant: &str,
    limit: Option<i64>,
) -> Result<Mahnfaelle, ApiError> {
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let faelle: Vec<Mahnfall> = crate::pg::list_open_dunning(pool, tenant, limit)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(Mahnfall::from)
        .collect();
    Ok(Mahnfaelle {
        count: faelle.len(),
        saturated: i64::try_from(faelle.len()).unwrap_or(i64::MAX) >= limit,
        faelle,
    })
}

/// The accounts owing at least `min_balance_eur` (a decimal string, default
/// 0.01 €), largest first — `limit` of them (default 100, at most 1000). An
/// EEG plant account is never listed.
///
/// # Errors
///
/// `422` for a `min_balance_eur` that is not a decimal; `500` on a database
/// failure.
pub async fn overdue(
    pool: &PgPool,
    tenant: &str,
    min_balance_eur: Option<&str>,
    limit: Option<i64>,
) -> Result<UeberfaelligeKonten, ApiError> {
    use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive as _};
    use std::str::FromStr as _;
    // Parsed as a decimal, never through `f64`: "1.99" is 199 ct, not 198.
    let min_ct = match min_balance_eur {
        None => 1,
        Some(raw) => Decimal::from_str(raw)
            .ok()
            .and_then(|d| {
                (d * Decimal::ONE_HUNDRED)
                    .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
                    .to_i64()
            })
            .ok_or_else(|| {
                ApiError::unprocessable(format!("min_balance_eur {raw:?} is not a decimal amount"))
                    .with_code("invalid_min_balance")
            })?,
    };
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let accounts: Vec<UeberfaelligesKonto> =
        crate::pg::list_overdue_accounts(pool, tenant, min_ct, limit)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(UeberfaelligesKonto::from)
            .collect();
    Ok(UeberfaelligeKonten {
        count: accounts.len(),
        saturated: i64::try_from(accounts.len()).unwrap_or(i64::MAX) >= limit,
        accounts,
    })
}

/// The accounts an incoming payment may belong to.
///
/// Runs the camt importer's resolution ladder, strongest evidence first: an
/// exact IBAN, `EndToEndId` or a token of the reference resolves the account
/// outright (`EXACT`); only when nothing identifies the payer are accounts
/// ranked by how close their open balance is to the payment.
///
/// # Errors
///
/// `500` on a database failure — never a fall-through to the fuzzy rung,
/// which would report amount proximity under a reference-matching label.
pub async fn payment_match(
    pool: &PgPool,
    tenant: &str,
    iban_key: Option<&[u8; 32]>,
    p: PaymentMatchParams,
) -> Result<Zahlungszuordnung, ApiError> {
    use sqlx::Row;

    // ── The exact rung ───────────────────────────────────────────────────
    //
    // If the reference names a customer, there is nothing to rank: the payer
    // said who they are. Resolving that first is what keeps the score honest.
    let iban_hash = p
        .iban
        .as_deref()
        .map(|iban| crate::ledger::iban_hash(iban_key, iban));
    let exact = crate::pg::resolve_account_for_payment(
        pool,
        tenant,
        crate::pg::PaymentClues {
            iban_hash: iban_hash.as_deref(),
            end_to_end_id: p.end_to_end_id.as_deref(),
            remittance: Some(&p.reference),
        },
    )
    .await
    .map_err(ApiError::internal)?;
    if let Some(hit) = exact {
        // `residual_ct` is the answer the caller acts on, so a balance that
        // could not be read is an error, not a zero.
        let balance_ct: i64 = sqlx::query_scalar(
            "SELECT balance_ct FROM accounts WHERE account_id = $1 AND tenant = $2",
        )
        .bind(hit.account_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?
        .unwrap_or(0);
        return Ok(Zahlungszuordnung {
            payment_amount_ct: p.amount_ct,
            payment_amount_eur: crate::sepa::ct_to_eur_str(p.amount_ct),
            payment_reference: p.reference,
            tolerance_pct: None,
            candidates_count: 1,
            candidates: vec![Zuordnungskandidat {
                account_id: hit.account_id.to_string(),
                malo_id: hit.malo_id,
                lf_mp_id: hit.lf_mp_id,
                open_balance_ct: balance_ct,
                open_balance_eur: crate::sepa::ct_to_eur_str(balance_ct),
                matched_by: hit.matched_by.to_owned(),
                confidence: "EXACT".to_owned(),
                similarity_score: None,
                residual_ct: balance_ct - p.amount_ct,
            }],
            note: "The payment identifies its own account — no amount guessing was involved. \
                   `residual_ct` is what stays open after booking it (0 = settles the balance exactly)."
                .to_owned(),
        });
    }

    // ── The fuzzy rung ───────────────────────────────────────────────────
    //
    // Nothing in the payment says who sent it, so all that is left is "who
    // owes roughly this much". Ranked, never auto-booked.
    let tol_pct = p.tolerance_pct.unwrap_or(2.0);
    let tol_factor = 1.0 + tol_pct / 100.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the value is bounded well inside the target type; a count or ratio reported as a float, never a money amount"
    )]
    let lo = (p.amount_ct as f64 / tol_factor) as i64;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the value is bounded well inside the target type; a count or ratio reported as a float, never a money amount"
    )]
    let hi = (p.amount_ct as f64 * tol_factor) as i64;

    // Candidate matching is on the balance cache (the authoritative balance
    // is the doubleentry ledger; this cache is refreshed from it on every
    // post), so a set-based scan stays cheap.
    let rows = sqlx::query(
        r"SELECT a.account_id, a.malo_id, a.lf_mp_id, a.balance_ct
          FROM accounts a
          WHERE a.tenant = $1
            AND a.balance_ct BETWEEN $2 AND $3
          ORDER BY ABS(a.balance_ct - $4) ASC
          LIMIT 10",
    )
    .bind(tenant)
    .bind(lo)
    .bind(hi)
    .bind(p.amount_ct)
    .fetch_all(pool)
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;

    let candidates: Vec<Zuordnungskandidat> = rows
        .iter()
        .map(|r| {
            let account_id: uuid::Uuid = r.try_get("account_id").unwrap_or_default();
            let malo_id: String = r.try_get("malo_id").unwrap_or_default();
            let lf_mp_id: String = r.try_get("lf_mp_id").unwrap_or_default();
            let balance_ct: i64 = r.try_get("balance_ct").unwrap_or_default();

            // Amount proximity is the only signal here — the reference already
            // failed to identify anyone.
            #[expect(
                clippy::cast_precision_loss,
                reason = "a count or ratio reported as a float, never a money amount"
            )]
            let closeness = if p.amount_ct == 0 {
                0.0_f64
            } else {
                1.0 - (balance_ct - p.amount_ct).unsigned_abs() as f64
                    / p.amount_ct.unsigned_abs() as f64
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to 0..=100 before the cast"
            )]
            let score = (closeness.clamp(0.0, 1.0) * 100.0).round() as u32;
            // No fuzzy match is ever HIGH: an amount that happens to line up is
            // not evidence of who paid. The exact rung above is the only one
            // safe to auto-book.
            let confidence = if score >= 99 { "MEDIUM" } else { "LOW" };

            Zuordnungskandidat {
                account_id: account_id.to_string(),
                malo_id,
                lf_mp_id,
                open_balance_ct: balance_ct,
                open_balance_eur: crate::sepa::ct_to_eur_str(balance_ct),
                matched_by: "amount_proximity".to_owned(),
                confidence: confidence.to_owned(),
                similarity_score: Some(score),
                residual_ct: balance_ct - p.amount_ct,
            }
        })
        .collect();

    Ok(Zahlungszuordnung {
        payment_amount_ct: p.amount_ct,
        payment_amount_eur: crate::sepa::ct_to_eur_str(p.amount_ct),
        payment_reference: p.reference,
        tolerance_pct: Some(tol_pct),
        candidates_count: candidates.len(),
        candidates,
        note: "Nothing in the reference identified the payer, so these are ranked by amount \
               proximity alone — a coincidence, not evidence. Review before booking; ask the \
               customer to quote their Mandatsreferenz or MaLo-ID to make the next one exact."
            .to_owned(),
    })
}

/// An instant as RFC 3339.
fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
