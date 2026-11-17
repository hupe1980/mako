//! The Guthaben a Schlussrechnung owes back (§ 40c Abs. 3 EnWG).
//!
//! A `de.billing.rechnung.erstellt` with `schlussrechnung: true` whose
//! `zuZahlen` is negative raises one open payout item, keyed on (tenant,
//! Rechnungsnummer) and due by `mako_fristen::vorlauf::guthaben_faellig` of
//! its Rechnungsdatum ([`aus_rechnung`], [`raise`]). The invoice's Storno
//! cancels it ([`cancel`]); the payout books it and closes it in one
//! transaction ([`pay`]). Past its date an open item is overdue, and
//! [`list`] (`GET /api/v1/credit-payouts`, MCP `list_credit_payouts`) says so
//! for the Desk's Heute.

use anyhow::Context as _;
use mako_service::ApiError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// Where a payout item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Paid,
    Cancelled,
}

impl Status {
    /// Every state, as the `status` `CHECK` lists them.
    pub const ALL: [Self; 3] = [Self::Open, Self::Paid, Self::Cancelled];

    /// The `credit_payouts.status` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Paid => "paid",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// A payout item to raise, read off an invoice event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCredit {
    /// The Schlussrechnung's number — the item's key.
    pub rechnungsnummer: String,
    /// The Guthaben in cents, positive.
    pub amount_ct: i64,
    /// The Rechnungsdatum the two weeks run from.
    pub rechnungsdatum: Date,
    /// `guthaben_faellig(rechnungsdatum)`.
    pub due_on: Date,
}

/// A decimal EUR string in cents, rounded kaufmännisch; never through `f64`.
fn eur_to_ct(s: &str) -> Option<i64> {
    use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive as _};
    use std::str::FromStr as _;
    (Decimal::from_str(s).ok()? * Decimal::ONE_HUNDRED)
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_i64()
}

/// The payout a `de.billing.rechnung.erstellt` owes, if any.
///
/// `None` for anything but a Schlussrechnung (`schlussrechnung: true`, not a
/// correction) whose `zuZahlen` is negative. A Schlussrechnung with a Guthaben
/// and no readable Rechnungsdatum or Rechnungsnummer is refused: the due date
/// cannot be stated without it, and a guessed one reports a late payout on
/// time.
///
/// # Errors
///
/// The reason the event cannot raise its item.
pub fn aus_rechnung(data: &serde_json::Value) -> Result<Option<NewCredit>, String> {
    let schluss = data
        .get("schlussrechnung")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let correction = data
        .get("is_correction")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !schluss || correction {
        return Ok(None);
    }
    let rechnung = data.get("rechnung");
    let Some(zu_zahlen_ct) = rechnung
        .and_then(|r| r.get("zuZahlen"))
        .and_then(|g| g.get("wert"))
        .and_then(serde_json::Value::as_str)
        .and_then(eur_to_ct)
    else {
        return Err("a Schlussrechnung carries no readable zuZahlen".to_owned());
    };
    if zu_zahlen_ct >= 0 {
        return Ok(None);
    }
    let rechnungsnummer = rechnung
        .and_then(|r| r.get("rechnungsnummer"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("a Schlussrechnung with a Guthaben carries no rechnungsnummer")?;
    // BO4E types the Rechnungsdatum `date-time`; billingd writes the date at
    // midnight UTC, so the date is read in the offset the payload carries.
    let rechnungsdatum = rechnung
        .and_then(|r| r.get("rechnungsdatum"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                .ok()
                .map(time::OffsetDateTime::date)
        })
        .ok_or("a Schlussrechnung with a Guthaben carries no readable rechnungsdatum")?;
    Ok(Some(NewCredit {
        rechnungsnummer: rechnungsnummer.to_owned(),
        amount_ct: zu_zahlen_ct.saturating_neg(),
        rechnungsdatum,
        due_on: mako_fristen::vorlauf::guthaben_faellig(rechnungsdatum),
    }))
}

/// Raise the item; a second event for the same Rechnungsnummer raises none.
/// `true` when this call raised it.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn raise(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    lf_mp_id: &str,
    credit: &NewCredit,
    source_ce_id: &str,
) -> anyhow::Result<bool> {
    let done = sqlx::query(
        "INSERT INTO credit_payouts \
         (tenant, rechnungsnummer, malo_id, lf_mp_id, amount_ct, rechnungsdatum, due_on, \
          source_ce_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
         ON CONFLICT (tenant, rechnungsnummer) DO NOTHING",
    )
    .bind(tenant)
    .bind(&credit.rechnungsnummer)
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(credit.amount_ct)
    .bind(credit.rechnungsdatum)
    .bind(credit.due_on)
    .bind(source_ce_id)
    .execute(pool)
    .await
    .context("raise credit payout")?;
    Ok(done.rows_affected() == 1)
}

/// Cancel the open item of the invoice `rechnungsnummer` a Storno reverses.
/// `true` when an open item was cancelled.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn cancel(
    pool: &PgPool,
    tenant: &str,
    rechnungsnummer: &str,
    storno_ce_id: &str,
) -> anyhow::Result<bool> {
    let done = sqlx::query(
        "UPDATE credit_payouts \
            SET status = 'cancelled', closed_at = now(), storno_ce_id = $3 \
          WHERE tenant = $1 AND rechnungsnummer = $2 AND status = 'open'",
    )
    .bind(tenant)
    .bind(rechnungsnummer)
    .bind(storno_ce_id)
    .execute(pool)
    .await
    .context("cancel credit payout")?;
    if done.rows_affected() == 0 {
        let paid: Option<bool> = sqlx::query_scalar(
            "SELECT status = 'paid' FROM credit_payouts WHERE tenant = $1 AND rechnungsnummer = $2",
        )
        .bind(tenant)
        .bind(rechnungsnummer)
        .fetch_optional(pool)
        .await
        .context("read credit payout")?;
        if paid == Some(true) {
            tracing::warn!(
                rechnungsnummer,
                "accountingd: a Storno reverses a Schlussrechnung whose Guthaben was already \
                 paid out — the payout stands on the Kontokorrent"
            );
        }
    }
    Ok(done.rows_affected() == 1)
}

/// An open item is overdue from the day after its due date.
#[must_use]
pub fn ueberfaellig(due_on: Date, today: Date) -> bool {
    today > due_on
}

// ── The read ──────────────────────────────────────────────────────────────────

/// One Guthaben owed back from a Schlussrechnung.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Guthabenauszahlung {
    /// The item, as `POST /api/v1/credit-payouts/{payout_id}/paid` names it.
    pub payout_id: String,
    /// The Schlussrechnung that owes it.
    pub rechnungsnummer: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The Lieferant's MP-ID the account is kept under.
    pub lf_mp_id: String,
    /// The Guthaben in cents.
    pub amount_ct: i64,
    /// `amount_ct` as a decimal EUR string.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub amount_eur: String,
    /// The Schlussrechnung's date.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub rechnungsdatum: String,
    /// The last day it may be paid out (§ 40c Abs. 3 EnWG, two weeks).
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub due_on: String,
    /// Where it stands.
    pub status: Status,
    /// Open and past `due_on` on today's Europe/Berlin date.
    pub overdue: bool,
}

/// The payout items (`list_credit_payouts`, `GET /api/v1/credit-payouts`).
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Guthabenauszahlungen {
    /// The items, earliest due date first.
    pub payouts: Vec<Guthabenauszahlung>,
    /// How many items `payouts` holds.
    pub count: usize,
    /// How many of them are overdue.
    pub overdue_count: usize,
    /// The page is full: more may match than were listed.
    pub saturated: bool,
}

/// `list_credit_payouts`' parameters — the query of `GET /api/v1/credit-payouts`.
#[derive(Debug, Default, Deserialize, JsonSchema, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct CreditPayoutQuery {
    /// Only items in this state (default `open`).
    pub status: Option<Status>,
    /// Only open items past their due date.
    pub overdue: Option<bool>,
    /// Restrict to one Marktlokation.
    pub malo_id: Option<String>,
    /// Maximum rows (default 100, at most 1000).
    pub limit: Option<i64>,
}

/// The items `q` selects, earliest due date first, judged overdue on `today`.
///
/// # Errors
///
/// `500` on a database failure.
pub async fn list(
    pool: &PgPool,
    tenant: &str,
    q: &CreditPayoutQuery,
    today: Date,
) -> Result<Guthabenauszahlungen, ApiError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let overdue_only = q.overdue.unwrap_or(false);
    // Only an open item can be overdue, so asking for overdue ones is asking
    // for open ones.
    let status = if overdue_only {
        Status::Open
    } else {
        q.status.unwrap_or(Status::Open)
    };
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, i64, Date, Date, String)>(
        "SELECT payout_id, rechnungsnummer, malo_id, lf_mp_id, amount_ct, rechnungsdatum, \
                due_on, status \
           FROM credit_payouts \
          WHERE tenant = $1 AND status = $2 \
            AND ($3::text IS NULL OR malo_id = $3) \
            AND (NOT $4 OR due_on < $5) \
          ORDER BY due_on, rechnungsnummer \
          LIMIT $6",
    )
    .bind(tenant)
    .bind(status.as_str())
    .bind(q.malo_id.as_deref())
    .bind(overdue_only)
    .bind(today)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    let payouts = rows
        .into_iter()
        .map(|r| {
            let status = Status::parse(&r.7).ok_or_else(|| {
                ApiError::internal(anyhow::anyhow!("credit_payouts.status {:?}", r.7))
            })?;
            Ok(Guthabenauszahlung {
                payout_id: r.0.to_string(),
                rechnungsnummer: r.1,
                malo_id: r.2,
                lf_mp_id: r.3,
                amount_ct: r.4,
                amount_eur: crate::handlers::format_ct_as_eur(r.4),
                rechnungsdatum: r.5.to_string(),
                due_on: r.6.to_string(),
                overdue: status == Status::Open && ueberfaellig(r.6, today),
                status,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Guthabenauszahlungen {
        count: payouts.len(),
        overdue_count: payouts.iter().filter(|p| p.overdue).count(),
        saturated: i64::try_from(payouts.len()).unwrap_or(i64::MAX) >= limit,
        payouts,
    })
}

// ── The payout ────────────────────────────────────────────────────────────────

/// Body of `POST /api/v1/credit-payouts/{payout_id}/paid`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PaidRequest {
    /// The value date of the transfer (YYYY-MM-DD).
    #[schema(format = Date)]
    pub paid_on: String,
    /// The bank reference of the transfer.
    pub reference: Option<String>,
}

/// What a payout booked.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct GuthabenAusgezahlt {
    /// The item that was paid.
    pub payout_id: String,
    /// The ledger entry the payout is (or will be, once the ledger answers).
    pub ledger_entry_id: String,
    /// The amount in cents.
    pub amount_ct: i64,
}

/// Book the payout of item `payout_id` and close it, in one transaction.
///
/// The booking is a `ZAHLUNG` debit of the Guthaben — money leaving the bank
/// account brings the Kontokorrent back to zero — keyed
/// `credit-payout:{payout_id}`, so the item is paid out once.
///
/// # Errors
///
/// `422 invalid_date` for a `paid_on` that is no date; `404
/// credit_payout_not_found`; `409 credit_payout_not_open` for an item paid or
/// cancelled; `500` on a storage failure.
pub async fn pay(
    pool: &PgPool,
    ledger: &crate::ledger::PgLedger,
    tenant: &str,
    payout_id: Uuid,
    req: &PaidRequest,
    actor: &str,
) -> Result<GuthabenAusgezahlt, ApiError> {
    let paid_on = Date::parse(
        &req.paid_on,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| {
        ApiError::unprocessable(format!("paid_on {:?} is not a date", req.paid_on))
            .with_code("invalid_date")
    })?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    let (rechnungsnummer, malo_id, lf_mp_id, amount_ct, status) =
        sqlx::query_as::<_, (String, String, String, i64, String)>(
            "SELECT rechnungsnummer, malo_id, lf_mp_id, amount_ct, status \
               FROM credit_payouts WHERE tenant = $1 AND payout_id = $2 FOR UPDATE",
        )
        .bind(tenant)
        .bind(payout_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?
        .ok_or_else(|| {
            ApiError::not_found(format!("no credit payout {payout_id}"))
                .with_code("credit_payout_not_found")
        })?;
    if status != Status::Open.as_str() {
        return Err(
            ApiError::conflict(format!("credit payout {payout_id} is {status}"))
                .with_code("credit_payout_not_open"),
        );
    }
    let key = format!("credit-payout:{payout_id}");
    let reference = req.reference.as_deref().unwrap_or(&rechnungsnummer);
    let draft = crate::pg::draft(
        &malo_id,
        &lf_mp_id,
        "ZAHLUNG",
        amount_ct,
        &key,
        None,
        Some(reference),
        mako_fristen::heute(),
        paid_on,
        Some("Auszahlung Guthaben aus Schlussrechnung (§ 40c Abs. 3 EnWG)"),
        Some(actor),
    );
    let ledger_id = crate::booking::stage(&mut tx, ledger, tenant, &draft)
        .await
        .map_err(|e| e.to_api())?;
    sqlx::query(
        "UPDATE credit_payouts \
            SET status = 'paid', closed_at = now(), ledger_entry_id = $3, paid_by = $4 \
          WHERE tenant = $1 AND payout_id = $2",
    )
    .bind(tenant)
    .bind(payout_id)
    .bind(ledger_id)
    .bind(actor)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    tx.commit()
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    if let Err(e) = crate::booking::book(ledger, pool, tenant, &key).await {
        tracing::warn!(key = %key, error = %e, "accountingd: credit payout booking deferred");
    }
    Ok(GuthabenAusgezahlt {
        payout_id: payout_id.to_string(),
        ledger_entry_id: ledger_id.to_string(),
        amount_ct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn schluss(zu_zahlen: &str, datum: &str) -> serde_json::Value {
        serde_json::json!({
            "malo_id": "51238696012",
            "is_correction": false,
            "schlussrechnung": true,
            "rechnung": {
                "rechnungsnummer": "RE-2026-000042",
                "rechnungsdatum": datum,
                "gesamtbrutto": { "wert": "100.00", "waehrung": "EUR" },
                "zuZahlen": { "wert": zu_zahlen, "waehrung": "EUR" },
            },
        })
    }

    /// A credit dated 01.04. is due 15.04. and overdue on 16.04.
    #[test]
    fn a_credit_dated_1_april_is_overdue_on_16_april() {
        let c = aus_rechnung(&schluss("-42.17", "2026-04-01T00:00:00Z"))
            .expect("readable")
            .expect("a Guthaben is owed");
        assert_eq!(c.amount_ct, 4217);
        assert_eq!(c.rechnungsnummer, "RE-2026-000042");
        assert_eq!(c.due_on, date!(2026 - 04 - 15));
        assert!(!ueberfaellig(c.due_on, date!(2026 - 04 - 15)));
        assert!(ueberfaellig(c.due_on, date!(2026 - 04 - 16)));
    }

    /// An amount still owed, an ordinary invoice and a Storno owe no payout.
    #[test]
    fn only_a_schlussrechnung_credit_raises_an_item() {
        assert_eq!(
            aus_rechnung(&schluss("12.00", "2026-04-01T00:00:00Z")),
            Ok(None)
        );
        let mut ordinary = schluss("-12.00", "2026-04-01T00:00:00Z");
        ordinary["schlussrechnung"] = false.into();
        assert_eq!(aus_rechnung(&ordinary), Ok(None));
        let mut storno = schluss("-12.00", "2026-04-01T00:00:00Z");
        storno["is_correction"] = true.into();
        assert_eq!(aus_rechnung(&storno), Ok(None));
    }

    /// A Guthaben without its Rechnungsdatum is refused, never given a date.
    #[test]
    fn a_credit_without_its_date_is_refused() {
        let mut no_date = schluss("-12.00", "x");
        assert!(aus_rechnung(&no_date).is_err());
        no_date["rechnung"]
            .as_object_mut()
            .expect("object")
            .remove("rechnungsdatum");
        assert!(aus_rechnung(&no_date).is_err());
    }

    /// The `status` `CHECK` and the states the service writes agree both ways.
    #[test]
    fn the_status_check_is_the_states() {
        let states: Vec<&str> = Status::ALL.iter().map(|s| s.as_str()).collect();
        mako_service::schema_check::assert_agrees_in(
            include_str!("../migrations/0001_schema.sql"),
            "credit_payouts",
            "status",
            &states,
        );
    }
}
