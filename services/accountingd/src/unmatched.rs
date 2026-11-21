//! Bank lines no account claimed — kept until an operator assigns them.
//!
//! Every import door (the flat import and camt.052/053/054) records a booked
//! line the resolution ladder cannot attribute ([`record`]); the Desk lists
//! them ([`list`], `GET /api/v1/payments/unmatched`, MCP
//! `list_unmatched_payments`), proposes an account with
//! `POST /api/v1/payments/match` and books the operator's choice with
//! [`assign`] (`POST /api/v1/payments/{payment_id}/assign`): the booking draft,
//! its `bank_import_log` row, its announcement and the closed row commit in
//! one transaction.

use anyhow::Context as _;
use mako_service::ApiError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// The import door a line arrived through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Quelle {
    Flat,
    Camt052,
    Camt053,
    Camt054,
}

impl Quelle {
    /// Every door, in the order the `source` `CHECK` lists them.
    pub const ALL: [Self; 4] = [Self::Flat, Self::Camt052, Self::Camt053, Self::Camt054];

    /// The `unmatched_payments.source` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Camt052 => "camt052",
            Self::Camt053 => "camt053",
            Self::Camt054 => "camt054",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.as_str() == s)
    }
}

/// A line to keep, as the importer resolved it.
pub struct NewUnmatched<'a> {
    /// `AcctSvcrRef` (per detail), or the fallback hash.
    pub bank_transaction_id: &'a str,
    /// The Kontokorrent sign the booking will take.
    pub ledger_ct: i64,
    /// The value date.
    pub value_date: Date,
    /// The Verwendungszweck.
    pub reference: &'a str,
    /// The `EndToEndId`, when itemised.
    pub end_to_end_id: Option<&'a str>,
    /// The counterparty IBAN as reported; only its masked form is stored.
    pub iban: Option<&'a str>,
    /// The door.
    pub source: Quelle,
}

/// `DE…3000`: the country code and the last four characters.
#[must_use]
pub fn mask_iban(iban: &str) -> Option<String> {
    let compact: String = iban.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() < 8 || !compact.is_ascii() {
        return None;
    }
    Some(format!(
        "{}…{}",
        &compact[..2],
        &compact[compact.len() - 4..]
    ))
}

/// Keep a line no account claimed. A line already kept stays as it is.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn record(pool: &PgPool, tenant: &str, line: &NewUnmatched<'_>) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO unmatched_payments \
         (tenant, bank_transaction_id, ledger_ct, value_date, reference, end_to_end_id, \
          iban_masked, source) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
         ON CONFLICT (tenant, bank_transaction_id) DO NOTHING",
    )
    .bind(tenant)
    .bind(line.bank_transaction_id)
    .bind(line.ledger_ct)
    .bind(line.value_date)
    .bind(line.reference)
    .bind(line.end_to_end_id)
    .bind(line.iban.and_then(mask_iban))
    .bind(line.source.as_str())
    .execute(pool)
    .await
    .context("record unmatched payment")?;
    Ok(())
}

/// Close a kept line that a later import booked — in that import's
/// transaction, so the line is never both booked and still listed.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn close_by_import(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    bank_transaction_id: &str,
    malo_id: &str,
    lf_mp_id: &str,
    ledger_entry_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE unmatched_payments \
            SET assigned_at = now(), malo_id = $3, lf_mp_id = $4, ledger_entry_id = $5, \
                assigned_by = 'import' \
          WHERE tenant = $1 AND bank_transaction_id = $2 AND assigned_at IS NULL",
    )
    .bind(tenant)
    .bind(bank_transaction_id)
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(ledger_entry_id)
    .execute(conn)
    .await
    .context("close unmatched payment")?;
    Ok(())
}

// ── The read ──────────────────────────────────────────────────────────────────

/// One bank line no account claimed.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct UnzugeordneteZahlung {
    /// The line, as `POST /api/v1/payments/{payment_id}/assign` names it.
    pub payment_id: String,
    /// The bank's transaction reference.
    pub bank_transaction_id: String,
    /// The amount in cents, always positive; `is_return` carries the direction.
    pub amount_ct: i64,
    /// `amount_ct` as a decimal EUR string.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub amount_eur: String,
    /// A Rücklastschrift (money leaving the account) rather than a payment in.
    pub is_return: bool,
    /// The value date.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub value_date: String,
    /// The Verwendungszweck.
    pub reference: String,
    /// The `EndToEndId`, when the bank itemised one.
    pub end_to_end_id: Option<String>,
    /// The payer's IBAN as `DE…3000`, when the bank reported one.
    pub iban_masked: Option<String>,
    /// The import door it arrived through.
    pub source: Quelle,
    /// When it was imported.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub received_at: String,
}

/// The bank lines waiting for an account (`list_unmatched_payments`,
/// `GET /api/v1/payments/unmatched`).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct UnzugeordneteZahlungen {
    /// The lines, oldest value date first.
    pub payments: Vec<UnzugeordneteZahlung>,
    /// How many lines `payments` holds.
    pub count: usize,
    /// The page is full: more may be waiting than were listed.
    pub saturated: bool,
}

/// The open lines, oldest value date first — `limit` of them (default 100, at
/// most 1000).
///
/// # Errors
///
/// `500` on a database failure.
pub async fn list(
    pool: &PgPool,
    tenant: &str,
    limit: Option<i64>,
) -> Result<UnzugeordneteZahlungen, ApiError> {
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            i64,
            Date,
            String,
            Option<String>,
            Option<String>,
            String,
            time::OffsetDateTime,
        ),
    >(
        "SELECT payment_id, bank_transaction_id, ledger_ct, value_date, reference, \
                end_to_end_id, iban_masked, source, received_at \
           FROM unmatched_payments \
          WHERE tenant = $1 AND assigned_at IS NULL \
          ORDER BY value_date, received_at \
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    let payments = rows
        .into_iter()
        .map(|r| {
            let source = Quelle::parse(&r.7).ok_or_else(|| {
                ApiError::internal(anyhow::anyhow!("unmatched_payments.source {:?}", r.7))
            })?;
            Ok(UnzugeordneteZahlung {
                payment_id: r.0.to_string(),
                bank_transaction_id: r.1,
                amount_ct: r.2.abs(),
                amount_eur: crate::handlers::format_ct_as_eur(r.2.abs()),
                // A positive Kontokorrent amount re-opens the receivable: the
                // money left the account.
                is_return: r.2 > 0,
                value_date: r.3.to_string(),
                reference: r.4,
                end_to_end_id: r.5,
                iban_masked: r.6,
                source,
                received_at: crate::reads::rfc3339(r.8),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(UnzugeordneteZahlungen {
        count: payments.len(),
        saturated: i64::try_from(payments.len()).unwrap_or(i64::MAX) >= limit,
        payments,
    })
}

// ── The assignment ────────────────────────────────────────────────────────────

/// Body of `POST /api/v1/payments/{payment_id}/assign`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignRequest {
    /// 11-digit Marktlokations-ID of the account the payment belongs to.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under; defaults to the
    /// configured `lf_mp_id`.
    pub lf_mp_id: Option<String>,
}

/// What an assignment booked.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct ZahlungZugeordnet {
    /// The line that was assigned.
    pub payment_id: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under.
    pub lf_mp_id: String,
    /// The ledger entry the booking is (or will be, once the ledger answers).
    pub ledger_entry_id: String,
    /// `ZAHLUNG` or `BANKRUECKLAST`.
    pub entry_type: String,
    /// The amount in cents, always positive.
    pub amount_ct: i64,
}

/// Book line `payment_id` to the account (`malo_id`, `lf_mp_id`) and close it.
///
/// The booking draft, the `bank_import_log` row, the
/// `de.mako.accounting.payment.imported` / `.bankruecklast` event and the closed
/// line commit in one transaction; the draft is then booked (a ledger that is
/// down leaves it `offen` for the recovery worker).
///
/// # Errors
///
/// `404 payment_not_found` / `account_not_found`; `409 payment_already_assigned`
/// for a line that is closed; `500` on a storage failure.
pub async fn assign(
    pool: &PgPool,
    ledger: &crate::ledger::PgLedger,
    tenant: &str,
    default_lf_mp_id: &str,
    payment_id: Uuid,
    req: &AssignRequest,
    actor: &str,
) -> Result<ZahlungZugeordnet, ApiError> {
    let lf_mp_id = req.lf_mp_id.as_deref().unwrap_or(default_lf_mp_id);
    let account = crate::pg::fetch_account(pool, &req.malo_id, lf_mp_id, tenant)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| crate::reads::account_not_found(&req.malo_id))?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    let row = sqlx::query_as::<
        _,
        (
            String,
            i64,
            Date,
            String,
            Option<String>,
            Option<time::OffsetDateTime>,
        ),
    >(
        "SELECT bank_transaction_id, ledger_ct, value_date, reference, end_to_end_id, assigned_at \
           FROM unmatched_payments WHERE tenant = $1 AND payment_id = $2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(payment_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?
    .ok_or_else(|| {
        ApiError::not_found(format!("no unmatched payment {payment_id}"))
            .with_code("payment_not_found")
    })?;
    let (bank_txn_id, ledger_ct, value_date, reference, end_to_end_id, assigned_at) = row;
    if assigned_at.is_some() {
        return Err(
            ApiError::conflict(format!("payment {payment_id} is already assigned"))
                .with_code("payment_already_assigned"),
        );
    }

    let is_return = ledger_ct > 0;
    let entry_type = if is_return {
        "BANKRUECKLAST"
    } else {
        "ZAHLUNG"
    };
    // The key every import door books this line under, so a later re-import
    // of the same line replays rather than books it twice.
    let key = format!("bank:{bank_txn_id}");
    let today = mako_fristen::heute();
    let draft = crate::pg::draft(
        &account.malo_id,
        &account.lf_mp_id,
        entry_type,
        ledger_ct,
        &key,
        None,
        Some(&reference),
        today,
        value_date,
        Some("Zahlung manuell zugeordnet"),
        Some(actor),
    );
    let ledger_id = crate::booking::stage(&mut tx, ledger, tenant, &draft)
        .await
        .map_err(|e| e.to_api())?;
    crate::pg::record_bank_import(
        &mut *tx,
        tenant,
        &crate::pg::BankImport {
            bank_transaction_id: &bank_txn_id,
            amount_ct: ledger_ct.abs(),
            iban: None,
            value_date,
            ledger_entry_id: ledger_id,
            payment_info_id: None,
            end_to_end_id: end_to_end_id.as_deref(),
        },
    )
    .await
    .map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE unmatched_payments \
            SET assigned_at = now(), malo_id = $3, lf_mp_id = $4, ledger_entry_id = $5, \
                assigned_by = $6 \
          WHERE tenant = $1 AND payment_id = $2",
    )
    .bind(tenant)
    .bind(payment_id)
    .bind(&account.malo_id)
    .bind(&account.lf_mp_id)
    .bind(ledger_id)
    .bind(actor)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    let amount_ct = ledger_ct.abs();
    let ce = mako_service::CloudEvent::new(
        mako_service::source("accountingd", tenant),
        if is_return {
            mako_events::accounting::BANKRUECKLAST
        } else {
            mako_events::accounting::PAYMENT_IMPORTED
        },
        &account.malo_id,
        serde_json::json!({
            "malo_id":      account.malo_id,
            "lf_mp_id":     account.lf_mp_id,
            "amount_ct":    amount_ct,
            "amount_eur":   crate::handlers::format_ct_as_eur(amount_ct),
            "is_return":    is_return,
            "reference":    reference,
            "bank_txn_id":  bank_txn_id,
            "booking_date": today.to_string(),
            "ledger_id":    ledger_id.to_string(),
            // An operator chose the account; no rung of the ladder did.
            "matched_by":   "manual",
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(ApiError::internal)?;
    tx.commit()
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    if let Err(e) = crate::booking::book(ledger, pool, tenant, &key).await {
        tracing::warn!(key = %key, error = %e, "accountingd: assigned payment booking deferred");
    }
    Ok(ZahlungZugeordnet {
        payment_id: payment_id.to_string(),
        malo_id: account.malo_id,
        lf_mp_id: account.lf_mp_id,
        ledger_entry_id: ledger_id.to_string(),
        entry_type: entry_type.to_owned(),
        amount_ct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `source` `CHECK` and the doors that write it list the same values.
    #[test]
    fn the_source_check_is_the_doors() {
        let doors: Vec<&str> = Quelle::ALL.iter().map(|q| q.as_str()).collect();
        mako_service::schema_check::assert_agrees_in(
            include_str!("../migrations/0001_schema.sql"),
            "unmatched_payments",
            "source",
            &doors,
        );
    }

    #[test]
    fn an_iban_is_masked_to_its_country_and_last_four() {
        assert_eq!(
            mask_iban("DE89 3704 0044 0532 0130 00").as_deref(),
            Some("DE…3000")
        );
        assert_eq!(mask_iban("short"), None);
    }
}
