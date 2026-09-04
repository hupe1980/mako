//! PostgreSQL persistence for `netzbilanzd`.
//!
//! # Tenant scoping
//!
//! Every query in this module takes a `tenant` and filters on it — reads,
//! updates and the correction chain alike. Deployment isolation is the
//! platform's tenancy boundary, but a column that exists and is ignored by some
//! of the queries is worse than no column at all: it reads as a guarantee that
//! is not there.

use anyhow::Context as _;
use invoic_checker::check::CheckOutcome;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{PgPool, Row};
use time::Date;
use uuid::Uuid;

use crate::request::SettlementRequest;

// ── Invoice numbering ─────────────────────────────────────────────────────────

/// Allocate the next invoice number for a tenant, series and year.
///
/// §14 Abs. 4 Nr. 4 UStG requires an *einmalig vergebene* consecutive number.
/// The counter is bumped with `INSERT … ON CONFLICT DO UPDATE … RETURNING`,
/// which takes a row lock for the rest of the transaction: two concurrent
/// billing runs serialise here rather than both reading the same last number.
///
/// Because the allocation happens inside the drafting transaction, a rolled-back
/// run consumes no number — the counter rolls back with it.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn next_rechnungsnummer(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    rechnungskreis: Option<&str>,
    year: i32,
) -> anyhow::Result<String> {
    let kreis = rechnungskreis.unwrap_or("").trim().to_owned();
    let next: i64 = sqlx::query_scalar(
        r"INSERT INTO invoice_number_seq (tenant, rechnungskreis, year, last_number)
          VALUES ($1, $2, $3, 1)
          ON CONFLICT (tenant, rechnungskreis, year)
          DO UPDATE SET last_number = invoice_number_seq.last_number + 1
          RETURNING last_number",
    )
    .bind(tenant)
    .bind(&kreis)
    .bind(i16::try_from(year).context("billing year out of range")?)
    .fetch_one(conn)
    .await
    .context("allocate invoice number")?;

    Ok(if kreis.is_empty() {
        format!("{year}-{next:06}")
    } else {
        format!("{kreis}-{year}-{next:06}")
    })
}

// ── Draft rows ────────────────────────────────────────────────────────────────

/// A draft as the API returns it.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DraftRow {
    /// Draft UUID.
    pub id: Uuid,
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// The party issuing the invoice — NB/GNB, or the MSB for PID 31009.
    pub sender_mp_id: String,
    /// The party billed — LF, or NB/LF/ESA for PID 31009.
    pub recipient_mp_id: String,
    /// BDEW Prüfidentifikator.
    pub pid: i32,
    /// `STROM` or `GAS` — PID 31002 is shared, so this is what distinguishes them.
    pub sparte: String,
    /// `grid_billing::SettlementType`.
    pub settlement_type: String,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: String,
    /// The allocated invoice number.
    pub rechnungsnummer: String,
    /// Issue date (§14 Abs. 4 Nr. 3 UStG).
    pub invoice_date: Date,
    /// Payment due date.
    pub due_date: Date,
    /// Start of the delivery period.
    pub period_from: Date,
    /// End of the delivery period.
    pub period_to: Date,
    /// The rendered BO4E `Rechnung`.
    pub rechnung: serde_json::Value,
    /// The request the settlement was computed from.
    pub settlement_input: serde_json::Value,
    /// Net total × 10⁻⁵ EUR.
    pub netto_eur_units: i64,
    /// Umsatzsteuer × 10⁻⁵ EUR. Zero under a reverse charge.
    pub steuer_eur_units: i64,
    /// Gross — `netto + steuer` × 10⁻⁵ EUR.
    pub brutto_eur_units: i64,
    /// What the recipient pays: the gross less every Abschlag this invoice
    /// settles, × 10⁻⁵ EUR. Equal to the gross when none is deducted.
    pub zu_zahlen_eur_units: i64,
    /// UNCL 5305 category: `S` taxed, `AE` reverse charge.
    pub steuer_kategorie: String,
    /// The rate in percent.
    pub steuer_satz_prozent: Decimal,
    /// `invoic-checker` verdict.
    pub check_outcome: String,
    /// The findings behind that verdict.
    pub check_findings: serde_json::Value,
    /// What the engine could not do — omitted levies, ceilings breached.
    pub settlement_warnings: serde_json::Value,
    /// Lifecycle status.
    pub status: String,
    /// `makod` process UUID once dispatched.
    pub dispatch_ref: Option<String>,
    /// When it was dispatched.
    #[serde(with = "time::serde::rfc3339::option")]
    pub dispatched_at: Option<time::OffsetDateTime>,
    /// REMADV 33001 reference once paid.
    pub remadv_ref: Option<String>,
    /// EDIFACT ERC code from a REMADV Abweisung.
    pub dispute_erc_code: Option<String>,
    /// The counterparty's stated reason for the Abweisung.
    pub dispute_reason: Option<String>,
    /// The operator's own reason for rejecting the draft.
    pub reject_reason: Option<String>,
    /// The draft this one corrects, if any.
    pub original_draft_id: Option<Uuid>,
    /// Why the recalculation happened.
    pub korrektur_grund: Option<String>,
    /// Insert time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// The columns `DraftRow` reads, in its field order.
const DRAFT_COLUMNS: &str = r"id, malo_id, sender_mp_id, recipient_mp_id, pid, sparte,
    settlement_type, rechnungsart, rechnungsnummer, invoice_date, due_date,
    period_from, period_to,
    rechnung, settlement_input, netto_eur_units, steuer_eur_units, brutto_eur_units,
    zu_zahlen_eur_units,
    steuer_kategorie, steuer_satz_prozent, check_outcome, check_findings,
    settlement_warnings, status, dispatch_ref, dispatched_at, remadv_ref,
    dispute_erc_code, dispute_reason, reject_reason, original_draft_id,
    korrektur_grund, created_at, updated_at";

/// A draft without its two large JSONB columns — for listings and history.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DraftSummaryRow {
    /// Draft UUID.
    pub id: Uuid,
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// BDEW Prüfidentifikator.
    pub pid: i32,
    /// `STROM` or `GAS`.
    pub sparte: String,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: String,
    /// The allocated invoice number.
    pub rechnungsnummer: String,
    /// Lifecycle status.
    pub status: String,
    /// `invoic-checker` verdict.
    pub check_outcome: String,
    /// Net total × 10⁻⁵ EUR.
    pub netto_eur_units: i64,
    /// Umsatzsteuer × 10⁻⁵ EUR.
    pub steuer_eur_units: i64,
    /// Gross — `netto + steuer` × 10⁻⁵ EUR.
    pub brutto_eur_units: i64,
    /// What the recipient pays after any Abschlag deduction, × 10⁻⁵ EUR.
    pub zu_zahlen_eur_units: i64,
    /// Start of the delivery period.
    pub period_from: Date,
    /// End of the delivery period.
    pub period_to: Date,
    /// `makod` process UUID once dispatched.
    pub dispatch_ref: Option<String>,
    /// Payment due date — what an overdue report measures against.
    pub due_date: Date,
    /// Insert time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

const SUMMARY_COLUMNS: &str = r"id, malo_id, pid, sparte, rechnungsart, rechnungsnummer,
    status, check_outcome, netto_eur_units, steuer_eur_units, brutto_eur_units,
    zu_zahlen_eur_units, period_from, period_to, dispatch_ref, due_date, created_at";

// ── Insert ────────────────────────────────────────────────────────────────────

/// Everything needed to persist one settled invoice.
pub struct NewDraft<'a> {
    /// Owning tenant.
    pub tenant: &'a str,
    /// 11-digit MaLo-ID.
    pub malo_id: &'a str,
    /// Issuing party.
    pub sender_mp_id: &'a str,
    /// Billed party.
    pub recipient_mp_id: &'a str,
    /// BDEW Prüfidentifikator.
    pub pid: i32,
    /// `STROM` or `GAS`.
    pub sparte: &'a str,
    /// `grid_billing::SettlementType`, as its `Debug` name.
    pub settlement_type: &'a str,
    /// Delivery period start.
    pub period_from: Date,
    /// Delivery period end.
    pub period_to: Date,
    /// The allocated invoice number.
    pub rechnungsnummer: &'a str,
    /// Issue date.
    pub invoice_date: Date,
    /// Payment due date.
    pub due_date: Date,
    /// The settlement input, stored verbatim so the figure can be replayed.
    pub settlement_input: serde_json::Value,
    /// The rendered BO4E document.
    pub rechnung: serde_json::Value,
    /// The three amounts the invoice states, × 10⁻⁵ EUR.
    pub netto_eur_units: i64,
    /// Umsatzsteuer × 10⁻⁵ EUR.
    pub steuer_eur_units: i64,
    /// Gross × 10⁻⁵ EUR.
    pub brutto_eur_units: i64,
    /// What is owed after deducting the Abschläge this invoice settles,
    /// × 10⁻⁵ EUR.
    pub zu_zahlen_eur_units: i64,
    /// UNCL 5305 category: `S` or `AE`.
    pub steuer_kategorie: &'a str,
    /// The rate in percent.
    pub steuer_satz_prozent: Decimal,
    /// `invoic-checker` verdict.
    pub check_outcome: CheckOutcome,
    /// The findings behind it.
    pub check_findings: serde_json::Value,
    /// Engine warnings.
    pub settlement_warnings: serde_json::Value,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: &'a str,
    /// The draft this corrects, for a Storno or Korrektur.
    pub original_draft_id: Option<Uuid>,
    /// Why the recalculation happened.
    pub korrektur_grund: Option<&'a str>,
}

/// The stable string form of a check outcome.
#[must_use]
pub const fn outcome_str(outcome: CheckOutcome) -> &'static str {
    match outcome {
        CheckOutcome::Ok => "Ok",
        CheckOutcome::Warn => "Warn",
        CheckOutcome::Dispute => "Dispute",
    }
}

/// Insert a settled invoice.
///
/// A duplicate is a **conflict**, not a silent no-op: upserting would return the
/// existing draft and discard the freshly computed one, so an operator who fixed
/// an input and re-ran the job would get the old figures back with a 201.
///
/// # Errors
///
/// Returns [`InsertDraftError::AlreadyBilled`] when the period is already billed, so the caller can
/// answer 409 and name the draft that occupies it.
pub async fn insert_draft(
    conn: &mut sqlx::PgConnection,
    draft: &NewDraft<'_>,
) -> Result<Uuid, InsertDraftError> {
    let row = sqlx::query(
        r"INSERT INTO invoice_drafts
              (tenant, malo_id, sender_mp_id, recipient_mp_id, pid, sparte, settlement_type,
               period_from, period_to, rechnungsnummer, invoice_date, due_date,
               settlement_input, rechnung,
               netto_eur_units, steuer_eur_units, brutto_eur_units, zu_zahlen_eur_units,
               steuer_kategorie,
               steuer_satz_prozent, check_outcome, check_findings, settlement_warnings,
               rechnungsart, original_draft_id, korrektur_grund)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                  $18, $19, $20, $21, $22, $23, $24, $25, $26)
          RETURNING id",
    )
    .bind(draft.tenant)
    .bind(draft.malo_id)
    .bind(draft.sender_mp_id)
    .bind(draft.recipient_mp_id)
    .bind(draft.pid)
    .bind(draft.sparte)
    .bind(draft.settlement_type)
    .bind(draft.period_from)
    .bind(draft.period_to)
    .bind(draft.rechnungsnummer)
    .bind(draft.invoice_date)
    .bind(draft.due_date)
    .bind(&draft.settlement_input)
    .bind(&draft.rechnung)
    .bind(draft.netto_eur_units)
    .bind(draft.steuer_eur_units)
    .bind(draft.brutto_eur_units)
    .bind(draft.zu_zahlen_eur_units)
    .bind(draft.steuer_kategorie)
    .bind(draft.steuer_satz_prozent)
    .bind(outcome_str(draft.check_outcome))
    .bind(&draft.check_findings)
    .bind(&draft.settlement_warnings)
    .bind(draft.rechnungsart)
    .bind(draft.original_draft_id)
    .bind(draft.korrektur_grund)
    .fetch_one(&mut *conn)
    .await;

    match row {
        Ok(row) => Ok(row.try_get("id").map_err(|e| {
            InsertDraftError::Database(anyhow::Error::new(e).context("read inserted draft id"))
        })?),
        Err(sqlx::Error::Database(db)) if db.constraint() == Some("id_no_double_billing") => {
            Err(InsertDraftError::AlreadyBilled)
        }
        Err(sqlx::Error::Database(db))
            if db.constraint() == Some("id_one_abschlag_per_invoice_date") =>
        {
            Err(InsertDraftError::AbschlagAlreadyBilled)
        }
        Err(sqlx::Error::Database(db)) if db.constraint() == Some("id_rechnungsnummer_unique") => {
            Err(InsertDraftError::DuplicateRechnungsnummer)
        }
        Err(sqlx::Error::Database(db)) if db.constraint() == Some("id_one_storno_per_original") => {
            Err(InsertDraftError::AlreadyReversed)
        }
        Err(e) => Err(InsertDraftError::Database(
            anyhow::Error::new(e).context("insert invoice draft"),
        )),
    }
}

/// Why an insert did not produce a draft.
#[derive(Debug, thiserror::Error)]
pub enum InsertDraftError {
    /// A live RECHNUNG already covers this MaLo, period and PID.
    #[error(
        "this MaLo, period and Prüfidentifikator are already billed — reject the existing \
         draft to re-bill the period, or issue a Storno if it was dispatched"
    )]
    AlreadyBilled,
    /// An Abschlagsrechnung for this MaLo, period and Rechnungsdatum exists.
    #[error(
        "an Abschlagsrechnung for this MaLo, period and Rechnungsdatum already exists — \
         a replayed billing run must not produce a second one. Bill the next instalment \
         under its own invoice_date, or reject the existing draft"
    )]
    AbschlagAlreadyBilled,
    /// The allocated invoice number is already in use.
    #[error("invoice number already issued")]
    DuplicateRechnungsnummer,
    /// This invoice already has a Stornorechnung.
    #[error(
        "this invoice is already reversed — a second Storno would credit the \
         counterparty twice; issue a Korrekturrechnung instead"
    )]
    AlreadyReversed,
    /// Anything else.
    #[error(transparent)]
    Database(#[from] anyhow::Error),
}

// ── Reads ─────────────────────────────────────────────────────────────────────

/// Fetch one draft, scoped to its tenant.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn fetch_draft(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    id: Uuid,
) -> anyhow::Result<Option<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(&format!(
        "SELECT {DRAFT_COLUMNS} FROM invoice_drafts WHERE tenant = $1 AND id = $2"
    ))
    .bind(tenant)
    .bind(id)
    .fetch_optional(executor)
    .await
    .context("fetch_draft")
}

/// Filters for a draft listing.
#[derive(Debug, Default)]
pub struct DraftFilter<'a> {
    /// Lifecycle status.
    pub status: Option<&'a str>,
    /// MaLo-ID.
    pub malo_id: Option<&'a str>,
    /// Issuing party — the MSB for PID 31009, the NB otherwise.
    pub sender_mp_id: Option<&'a str>,
    /// Billed party.
    pub recipient_mp_id: Option<&'a str>,
    /// BDEW Prüfidentifikator.
    pub pid: Option<i32>,
    /// `STROM` or `GAS`.
    ///
    /// PID 31002 is shared between the Sparten and 31005 likewise, so the
    /// Prüfidentifikator alone cannot answer "show me the gas invoices" —
    /// which is the question a GNB deployment asks first.
    pub sparte: Option<&'a str>,
    /// `invoic-checker` verdict.
    pub check_outcome: Option<&'a str>,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: Option<&'a str>,
    /// Page cursor: return only rows strictly older than this `(created_at, id)`.
    pub after: Option<Cursor>,
    /// Maximum rows.
    pub limit: i64,
}

/// A page cursor — the `(created_at, id)` of the last row of the previous page.
///
/// The listing is ordered by that pair, so resuming from it is a range scan on
/// `id_tenant_created`. `OFFSET` would re-read and discard every earlier row,
/// and is unstable: a draft inserted between two page requests shifts the
/// window and the caller skips a row.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// `created_at` of the last row already returned.
    pub created_at: time::OffsetDateTime,
    /// `id` of that row, breaking ties within the same instant.
    pub id: Uuid,
}

impl Cursor {
    /// Parse the opaque `"<rfc3339>_<uuid>"` form the API hands to callers.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (ts, id) = raw.rsplit_once('_')?;
        Some(Self {
            created_at: time::OffsetDateTime::parse(
                ts,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()?,
            id: id.parse().ok()?,
        })
    }

    /// Render the cursor for the next page of a result set.
    ///
    /// Normalised to UTC before formatting, so the timestamp always ends in `Z`
    /// and never in `+01:00`. A `+` in a query string decodes as a space, which
    /// would make the cursor unparseable the moment a caller passed it back —
    /// and only for connections whose session timezone was not UTC, which is
    /// exactly the kind of defect that survives every test.
    #[must_use]
    pub fn encode(created_at: time::OffsetDateTime, id: Uuid) -> Option<String> {
        created_at
            .to_offset(time::UtcOffset::UTC)
            .format(&time::format_description::well_known::Rfc3339)
            .ok()
            .map(|ts| format!("{ts}_{id}"))
    }
}

/// List drafts for a tenant.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn list_drafts(
    pool: &PgPool,
    tenant: &str,
    f: &DraftFilter<'_>,
) -> anyhow::Result<Vec<DraftSummaryRow>> {
    sqlx::query_as::<_, DraftSummaryRow>(&format!(
        r"SELECT {SUMMARY_COLUMNS}
          FROM invoice_drafts
          WHERE tenant = $1
            AND ($2::TEXT IS NULL OR status = $2)
            AND ($3::TEXT IS NULL OR malo_id = $3)
            AND ($4::TEXT IS NULL OR sender_mp_id = $4)
            AND ($5::TEXT IS NULL OR recipient_mp_id = $5)
            AND ($6::INT  IS NULL OR pid = $6)
            AND ($7::TEXT IS NULL OR check_outcome = $7)
            AND ($8::TEXT IS NULL OR rechnungsart = $8)
            AND ($9::TEXT IS NULL OR sparte = $9)
            AND ($10::TIMESTAMPTZ IS NULL OR (created_at, id) < ($10, $11))
          ORDER BY created_at DESC, id DESC
          LIMIT $12"
    ))
    .bind(tenant)
    .bind(f.status)
    .bind(f.malo_id)
    .bind(f.sender_mp_id)
    .bind(f.recipient_mp_id)
    .bind(f.pid)
    .bind(f.check_outcome)
    .bind(f.rechnungsart)
    .bind(f.sparte)
    .bind(f.after.map(|c| c.created_at))
    .bind(f.after.map_or_else(Uuid::nil, |c| c.id))
    .bind(f.limit)
    .fetch_all(pool)
    .await
    .context("list_drafts")
}

/// Billing history for one MaLo.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn billing_history_for_malo(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    limit: i64,
) -> anyhow::Result<Vec<DraftSummaryRow>> {
    list_drafts(
        pool,
        tenant,
        &DraftFilter {
            malo_id: Some(malo_id),
            limit,
            ..DraftFilter::default()
        },
    )
    .await
}

/// The Storno / Korrektur chain — every correcting document, newest first.
///
/// One query over both Rechnungsarten: two separately limited listings
/// concatenated return up to twice the limit, each truncated against its own
/// window, so neither the count nor the ordering describes the chain asked for.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn list_corrections(
    pool: &PgPool,
    tenant: &str,
    malo_id: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<DraftSummaryRow>> {
    sqlx::query_as::<_, DraftSummaryRow>(&format!(
        r"SELECT {SUMMARY_COLUMNS}
          FROM invoice_drafts
          WHERE tenant = $1
            AND rechnungsart <> 'RECHNUNG'
            AND ($2::TEXT IS NULL OR malo_id = $2)
          ORDER BY created_at DESC, id DESC
          LIMIT $3"
    ))
    .bind(tenant)
    .bind(malo_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_corrections")
}

/// Drafts still undispatched that are running out of time.
///
/// Two clocks, and a draft is reported when either has run out:
///
/// - it has sat for `stale_hours` since it was drafted; **or**
/// - its own `due_date` is within `stale_hours` — an invoice cannot be paid on
///   time if it has not been sent, and a 90-day Zahlungsziel makes the age
///   clock alone useless while a 7-day one makes it far too slow.
///
/// A draft the checker disputed is excluded: it is not overdue, it is blocked,
/// and alerting on it every hour trains an operator to ignore the alert.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn list_undispatched_stale(
    pool: &PgPool,
    tenant: &str,
    stale_hours: i64,
    limit: i64,
) -> anyhow::Result<Vec<DraftSummaryRow>> {
    sqlx::query_as::<_, DraftSummaryRow>(&format!(
        r"SELECT {SUMMARY_COLUMNS}
          FROM invoice_drafts
          WHERE tenant = $1
            AND status = 'draft'
            AND check_outcome <> 'Dispute'
            AND (
                 created_at < now() - make_interval(hours => $2::INT)
              OR due_date  <= (now() + make_interval(hours => $2::INT))::DATE
            )
          ORDER BY due_date ASC, created_at ASC
          LIMIT $3"
    ))
    .bind(tenant)
    .bind(i32::try_from(stale_hours).unwrap_or(48))
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_undispatched_stale")
}

// ── Aggregates ────────────────────────────────────────────────────────────────

/// Monthly totals grouped by PID, Sparte, status and Rechnungsart.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BillingSummaryRow {
    /// BDEW Prüfidentifikator.
    pub pid: i32,
    /// `STROM` or `GAS`.
    pub sparte: String,
    /// Lifecycle status.
    pub status: String,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: String,
    /// Number of invoices.
    pub count: i64,
    /// Summed net × 10⁻⁵ EUR.
    pub netto_eur_units: i64,
    /// Summed Umsatzsteuer × 10⁻⁵ EUR.
    pub steuer_eur_units: i64,
    /// Summed gross × 10⁻⁵ EUR.
    pub brutto_eur_units: i64,
    /// Summed collectible amount after Abschlag deductions, × 10⁻⁵ EUR.
    ///
    /// The gross is what was invoiced; this is what is left to collect. On a
    /// portfolio with Abschläge the two differ, and a month-end reconciliation
    /// that adds up the gross reconciles against a figure nobody will pay.
    pub zu_zahlen_eur_units: i64,
}

/// Monthly billing totals for a tenant.
///
/// Note the `::BIGINT` cast: PostgreSQL's `sum(bigint)` returns `numeric`, so
/// decoding it straight into an `i64` fails at runtime — which is what made
/// every call to this summary and to the payment stats return a 500.
///
/// The month is expressed as a half-open range on `period_from` rather than as
/// `date_trunc('month', period_from) = …`. Wrapping the column in a function
/// makes the predicate unindexable, so the summary seq-scanned an eight-year
/// Buchungsbeleg table to answer a question about one month of it.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn billing_summary(
    pool: &PgPool,
    tenant: &str,
    year: i32,
    month: u8,
) -> anyhow::Result<Vec<BillingSummaryRow>> {
    sqlx::query_as::<_, BillingSummaryRow>(
        r"SELECT pid, sparte, status, rechnungsart,
                 COUNT(*) AS count,
                 COALESCE(SUM(netto_eur_units),  0)::BIGINT AS netto_eur_units,
                 COALESCE(SUM(steuer_eur_units), 0)::BIGINT AS steuer_eur_units,
                 COALESCE(SUM(brutto_eur_units),    0)::BIGINT AS brutto_eur_units,
                 COALESCE(SUM(zu_zahlen_eur_units), 0)::BIGINT AS zu_zahlen_eur_units
          FROM invoice_drafts
          WHERE tenant = $1
            AND period_from >= make_date($2, $3, 1)
            AND period_from <  make_date($2, $3, 1) + INTERVAL '1 month'
          GROUP BY pid, sparte, status, rechnungsart
          ORDER BY pid, sparte, status",
    )
    .bind(tenant)
    .bind(year)
    .bind(i32::from(month))
    .fetch_all(pool)
    .await
    .context("billing_summary")
}

// ── Lifecycle transitions ─────────────────────────────────────────────────────

/// Record a successful dispatch: the document as it was actually sent, and the
/// verdict on that document.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn mark_dispatched(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
    dispatch_ref: &str,
    rechnung: &serde_json::Value,
    check_outcome: CheckOutcome,
    check_findings: &serde_json::Value,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE invoice_drafts
          SET status = 'dispatched', dispatch_ref = $3, dispatched_at = now(),
              rechnung = $4, check_outcome = $5, check_findings = $6, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND status = 'draft'",
    )
    .bind(tenant)
    .bind(id)
    .bind(dispatch_ref)
    .bind(rechnung)
    .bind(outcome_str(check_outcome))
    .bind(check_findings)
    .execute(conn)
    .await
    .context("mark_dispatched")?
    .rows_affected();
    Ok(rows > 0)
}

/// Reject a draft that has not been dispatched.
///
/// Rejection is how a period is reopened: the partial unique index excludes
/// rejected rows, so a corrected run can bill the same MaLo and period again.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn reject_draft(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
    reason: &str,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE invoice_drafts
          SET status = 'rejected', reject_reason = $3, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND status = 'draft'",
    )
    .bind(tenant)
    .bind(id)
    .bind(reason)
    .execute(pool)
    .await
    .context("reject_draft")?
    .rows_affected();
    Ok(rows > 0)
}

/// Mark a dispatched invoice paid on a REMADV 33001.
///
/// 33001 is the only Bestätigung in the REMADV family; 33002, 33003 and 33004
/// are all Abweisungen and route to [`mark_disputed`].
///
/// # Errors
///
/// Propagates any database failure.
pub async fn mark_paid(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
    remadv_ref: &str,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE invoice_drafts
          SET status = 'paid', remadv_ref = $3, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND status IN ('dispatched', 'disputed')",
    )
    .bind(tenant)
    .bind(id)
    .bind(remadv_ref)
    .execute(conn)
    .await
    .context("mark_paid")?
    .rows_affected();
    Ok(rows > 0)
}

/// Mark a dispatched invoice disputed on a REMADV Abweisung.
///
/// The dispute gets its own status and its own columns rather than overwriting
/// `check_outcome`: the NB's pre-dispatch verdict is the evidence that says
/// whether the invoice left the house defensible, and `status` is what tells a
/// settled invoice from a contested one.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn mark_disputed(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
    erc_code: &str,
    reason: &str,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE invoice_drafts
          SET status = 'disputed', dispute_erc_code = $3, dispute_reason = $4, updated_at = now()
          WHERE tenant = $1 AND id = $2 AND status = 'dispatched'",
    )
    .bind(tenant)
    .bind(id)
    .bind(erc_code)
    .bind(reason)
    .execute(conn)
    .await
    .context("mark_disputed")?
    .rows_affected();
    Ok(rows > 0)
}

/// Load the Abschlagsrechnungen a later invoice deducts, by draft ID.
///
/// The amount and the invoice number come from the stored draft, never from the
/// request: INVOIC AHB rule **\[526\]** requires the deducted amount to equal the
/// referenced Abschlagsrechnung's own Rechnungsbetrag, and a caller-supplied
/// figure is exactly the one that can disagree with it.
///
/// # Errors
///
/// Returns the draft IDs that are unusable, with why: not an Abschlagsrechnung,
/// belonging to another MaLo, never dispatched, or reversed — INVOIC AHB rule
/// **\[519\]** excludes a stornierte Abschlagsrechnung, because nothing was paid
/// on it and deducting it would credit money that never moved.
pub async fn load_abschlaege(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    malo_id: &str,
    ids: &[Uuid],
) -> anyhow::Result<Result<Vec<grid_billing::Abschlagsverrechnung>, Vec<String>>> {
    let mut deductions = Vec::with_capacity(ids.len());
    let mut problems = Vec::new();

    for &id in ids {
        let Some(row) = fetch_draft(&mut *conn, tenant, id).await? else {
            problems.push(format!("{id}: no such draft for this tenant"));
            continue;
        };
        if row.pid != 31001 {
            problems.push(format!(
                "{id}: PID {} is not an Abschlagsrechnung (31001)",
                row.pid
            ));
            continue;
        }
        if row.malo_id != malo_id {
            problems.push(format!(
                "{id}: belongs to MaLo {}, not {malo_id}",
                row.malo_id
            ));
            continue;
        }
        if row.status == "draft" || row.status == "rejected" {
            problems.push(format!(
                "{id}: status is '{}' — only an Abschlag the counterparty received can be deducted",
                row.status
            ));
            continue;
        }
        if has_storno(&mut *conn, tenant, id).await? {
            problems.push(format!(
                "{id}: reversed by a Stornorechnung — nothing was paid on it (INVOIC AHB [519])"
            ));
            continue;
        }
        deductions.push(grid_billing::Abschlagsverrechnung {
            rechnungsnummer: row.rechnungsnummer,
            rechnungsdatum: row.invoice_date,
            // The gross as billed, so the deduction matches the document the
            // counterparty holds (INVOIC AHB [526]).
            betrag_brutto_eur: Decimal::from(row.brutto_eur_units) / Decimal::from(100_000),
        });
    }

    Ok(if problems.is_empty() {
        Ok(deductions)
    } else {
        Err(problems)
    })
}

/// Whether a draft has already been reversed by a Stornorechnung.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn has_storno(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    original_draft_id: Uuid,
) -> anyhow::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (
              SELECT 1 FROM invoice_drafts
              WHERE tenant = $1 AND original_draft_id = $2
                AND rechnungsart = 'STORNORECHNUNG'
          )",
    )
    .bind(tenant)
    .bind(original_draft_id)
    .fetch_one(conn)
    .await
    .context("has_storno")
}

/// The settlement input stored on a draft, parsed back into its typed form.
///
/// # Errors
///
/// Returns an error when the draft does not exist for this tenant, or when the
/// stored input no longer matches the request model.
pub async fn load_settlement_input(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    id: Uuid,
) -> anyhow::Result<Option<(DraftRow, SettlementRequest)>> {
    let Some(row) = fetch_draft(&mut *conn, tenant, id).await? else {
        return Ok(None);
    };
    let input: SettlementRequest = serde_json::from_value(row.settlement_input.clone())
        .context("stored settlement input no longer parses as a SettlementRequest")?;
    Ok(Some((row, input)))
}

// ── Audit export ──────────────────────────────────────────────────────────────

/// Filters for the § 147 AO / GoBD export.
pub struct AuditQuery {
    /// Owning tenant.
    pub tenant: String,
    /// Earliest `period_from`.
    pub from: Option<Date>,
    /// Latest `period_to`.
    pub to: Option<Date>,
    /// BDEW Prüfidentifikator.
    pub pid: Option<i32>,
    /// Lifecycle status.
    pub status: Option<String>,
    /// Page cursor from the previous page's `next_cursor`.
    pub after: Option<Cursor>,
    /// Maximum rows.
    pub limit: i64,
}

/// One row of the audit export — no JSONB, so a full-portfolio export stays
/// a manageable payload. The rendered `Rechnung` is fetched per draft.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditRow {
    /// Draft UUID.
    pub id: Uuid,
    /// Owning tenant.
    pub tenant: String,
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Issuing party.
    pub sender_mp_id: String,
    /// Billed party.
    pub recipient_mp_id: String,
    /// BDEW Prüfidentifikator.
    pub pid: i32,
    /// `STROM` or `GAS`.
    pub sparte: String,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: String,
    /// The allocated invoice number.
    pub rechnungsnummer: String,
    /// Delivery period start.
    pub period_from: Date,
    /// Delivery period end.
    pub period_to: Date,
    /// Net total × 10⁻⁵ EUR.
    pub netto_eur_units: i64,
    /// Umsatzsteuer × 10⁻⁵ EUR.
    pub steuer_eur_units: i64,
    /// Gross × 10⁻⁵ EUR.
    pub brutto_eur_units: i64,
    /// What the recipient pays after any Abschlag deduction, × 10⁻⁵ EUR.
    pub zu_zahlen_eur_units: i64,
    /// UNCL 5305 category: `S` or `AE`.
    pub steuer_kategorie: String,
    /// `invoic-checker` verdict.
    pub check_outcome: String,
    /// Lifecycle status.
    pub status: String,
    /// `makod` process UUID.
    pub dispatch_ref: Option<String>,
    /// The draft this one corrects.
    pub original_draft_id: Option<Uuid>,
    /// Why the recalculation happened.
    pub korrektur_grund: Option<String>,
    /// BO4E schema version of the stored document.
    pub bo4e_version: String,
    /// Insert time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Export invoice records for a BNetzA / tax audit.
///
/// Ordered by `(created_at, id)` — the same pair the listing pages on — rather
/// than by delivery period. A stable total order is what makes the export
/// resumable: an eight-year portfolio does not fit one response, and a sort key
/// the cursor cannot express means the caller has to take the whole thing in
/// one request or risk skipping rows. The period is a filter, not the order.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn list_audit(pool: &PgPool, q: &AuditQuery) -> anyhow::Result<Vec<AuditRow>> {
    sqlx::query_as::<_, AuditRow>(
        r"SELECT id, tenant, malo_id, sender_mp_id, recipient_mp_id, pid, sparte, rechnungsart,
                 rechnungsnummer, period_from, period_to,
                 netto_eur_units, steuer_eur_units, brutto_eur_units, zu_zahlen_eur_units,
                 steuer_kategorie, check_outcome, status,
                 dispatch_ref, original_draft_id, korrektur_grund, bo4e_version,
                 created_at, updated_at
          FROM invoice_drafts
          WHERE tenant = $1
            AND ($2::DATE IS NULL OR period_from >= $2)
            AND ($3::DATE IS NULL OR period_to   <= $3)
            AND ($4::INT  IS NULL OR pid = $4)
            AND ($5::TEXT IS NULL OR status = $5)
            AND ($6::TIMESTAMPTZ IS NULL OR (created_at, id) < ($6, $7))
          ORDER BY created_at DESC, id DESC
          LIMIT $8",
    )
    .bind(&q.tenant)
    .bind(q.from)
    .bind(q.to)
    .bind(q.pid)
    .bind(q.status.as_deref())
    .bind(q.after.map(|c| c.created_at))
    .bind(q.after.map_or_else(Uuid::nil, |c| c.id))
    .bind(q.limit)
    .fetch_all(pool)
    .await
    .context("list_audit")
}

// ── Kostenblatt (Redispatch 2.0, BK6-20-061) ──────────────────────────────────

/// A stored Kostenblatt record.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct KostenblattRow {
    /// Record UUID.
    pub id: Uuid,
    /// Owning tenant.
    pub tenant: String,
    /// Activation this cost sheet belongs to.
    pub activation_id: String,
    /// TechnischeRessource dispatched.
    pub tr_id: String,
    /// Grid connection point.
    pub malo_id: Option<String>,
    /// Calendar year of the activation.
    pub period_year: i16,
    /// Calendar month of the activation.
    pub period_month: i16,
    /// ÜNB receiving the Kostenblatt.
    pub uenb_mp_id: String,
    /// VNB sending it.
    pub vnb_mp_id: String,
    /// Dispatched energy.
    pub dispatch_kwh: Decimal,
    /// Contract rate.
    pub arbeitspreis_eur_per_kwh: Decimal,
    /// `dispatch_kwh × arbeitspreis_eur_per_kwh`, generated by the database.
    pub einsatzkosten_eur: Option<Decimal>,
    /// Typed BO4E `Kosten` for CIM export.
    pub kosten_json: Option<serde_json::Value>,
    /// Submission status.
    pub status: String,
    /// When it was submitted.
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<time::OffsetDateTime>,
    /// Submission reference.
    pub dispatch_ref: Option<String>,
    /// Activation window start.
    #[serde(with = "time::serde::rfc3339::option")]
    pub activation_start_utc: Option<time::OffsetDateTime>,
    /// Activation window end.
    #[serde(with = "time::serde::rfc3339::option")]
    pub activation_end_utc: Option<time::OffsetDateTime>,
    /// Provenance of `dispatch_kwh`.
    pub dispatch_source: Option<String>,
    /// Insert time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

/// Request body for `PUT /api/v1/redispatch/kostenblatt/{activation_id}`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertKostenblattRequest {
    /// TechnischeRessource dispatched.
    pub tr_id: String,
    /// Grid connection point.
    pub malo_id: Option<String>,
    /// Calendar year of the activation.
    pub period_year: i16,
    /// Calendar month of the activation.
    pub period_month: i16,
    /// ÜNB receiving the Kostenblatt.
    pub uenb_mp_id: String,
    /// VNB sending it.
    pub vnb_mp_id: String,
    /// Dispatched energy in kWh.
    pub dispatch_kwh: Decimal,
    /// Contract rate in EUR/kWh.
    pub arbeitspreis_eur_per_kwh: Decimal,
    /// Typed BO4E `Kosten` for CIM export.
    pub kosten_json: Option<serde_json::Value>,
    /// Activation window start.
    pub activation_start_utc: Option<time::OffsetDateTime>,
    /// Activation window end.
    pub activation_end_utc: Option<time::OffsetDateTime>,
    /// Provenance of `dispatch_kwh`.
    pub dispatch_source: Option<String>,
}

/// Create or update a Kostenblatt record.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn upsert_kostenblatt(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    activation_id: &str,
    req: &UpsertKostenblattRequest,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO kostenblatt_records
              (tenant, activation_id, tr_id, malo_id, period_year, period_month,
               uenb_mp_id, vnb_mp_id, dispatch_kwh, arbeitspreis_eur_per_kwh, kosten_json,
               activation_start_utc, activation_end_utc, dispatch_source)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
          ON CONFLICT (tenant, activation_id, tr_id) DO UPDATE
          SET dispatch_kwh             = EXCLUDED.dispatch_kwh,
              arbeitspreis_eur_per_kwh = EXCLUDED.arbeitspreis_eur_per_kwh,
              kosten_json              = COALESCE(EXCLUDED.kosten_json, kostenblatt_records.kosten_json),
              activation_start_utc     = COALESCE(EXCLUDED.activation_start_utc, kostenblatt_records.activation_start_utc),
              activation_end_utc       = COALESCE(EXCLUDED.activation_end_utc, kostenblatt_records.activation_end_utc),
              dispatch_source          = COALESCE(EXCLUDED.dispatch_source, kostenblatt_records.dispatch_source),
              updated_at               = now()
          RETURNING id",
    )
    .bind(tenant)
    .bind(activation_id)
    .bind(&req.tr_id)
    .bind(&req.malo_id)
    .bind(req.period_year)
    .bind(req.period_month)
    .bind(&req.uenb_mp_id)
    .bind(&req.vnb_mp_id)
    .bind(req.dispatch_kwh)
    .bind(req.arbeitspreis_eur_per_kwh)
    .bind(&req.kosten_json)
    .bind(req.activation_start_utc)
    .bind(req.activation_end_utc)
    .bind(&req.dispatch_source)
    .fetch_one(executor)
    .await
    .context("upsert_kostenblatt")?;
    Ok(row.try_get("id")?)
}

/// Fetch one activation's Kostenblatt records.
///
/// An activation can dispatch several TechnischeRessourcen, so this returns all
/// of them rather than an arbitrary one.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn fetch_kostenblatt(
    pool: &PgPool,
    tenant: &str,
    activation_id: &str,
) -> anyhow::Result<Vec<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        r"SELECT * FROM kostenblatt_records
          WHERE tenant = $1 AND activation_id = $2
          ORDER BY tr_id",
    )
    .bind(tenant)
    .bind(activation_id)
    .fetch_all(pool)
    .await
    .context("fetch_kostenblatt")
}

/// List a month's Kostenblatt records, optionally by status.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn list_kostenblatt(
    pool: &PgPool,
    tenant: &str,
    period_year: i16,
    period_month: i16,
    status_filter: Option<&str>,
) -> anyhow::Result<Vec<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        r"SELECT * FROM kostenblatt_records
          WHERE tenant = $1 AND period_year = $2 AND period_month = $3
            AND ($4::TEXT IS NULL OR status = $4)
          ORDER BY created_at DESC",
    )
    .bind(tenant)
    .bind(period_year)
    .bind(period_month)
    .bind(status_filter)
    .fetch_all(pool)
    .await
    .context("list_kostenblatt")
}

/// Mark every pending record of a month submitted, in one statement.
///
/// Returns the rows it actually moved, so the caller reports a submission that
/// happened rather than one it assumed.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn submit_pending_kostenblatt(
    pool: &PgPool,
    tenant: &str,
    period_year: i16,
    period_month: i16,
    dispatch_ref: &str,
) -> anyhow::Result<Vec<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        r"UPDATE kostenblatt_records
          SET status = 'submitted', submitted_at = now(), dispatch_ref = $4, updated_at = now()
          WHERE tenant = $1 AND period_year = $2 AND period_month = $3 AND status = 'pending'
          RETURNING *",
    )
    .bind(tenant)
    .bind(period_year)
    .bind(period_month)
    .bind(dispatch_ref)
    .fetch_all(pool)
    .await
    .context("submit_pending_kostenblatt")
}

/// Records for a month whose energy quantity was never established.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn list_kostenblatt_gaps(
    pool: &PgPool,
    tenant: &str,
    period_year: i16,
    period_month: i16,
) -> anyhow::Result<Vec<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        r"SELECT * FROM kostenblatt_records
          WHERE tenant = $1 AND period_year = $2 AND period_month = $3
            AND dispatch_kwh = 0 AND dispatch_source IS NULL AND status = 'pending'
          ORDER BY created_at DESC",
    )
    .bind(tenant)
    .bind(period_year)
    .bind(period_month)
    .fetch_all(pool)
    .await
    .context("list_kostenblatt_gaps")
}

// ── Fremdkosten ───────────────────────────────────────────────────────────────

/// A stored Fremdkosten record.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct FremdkostenRow {
    /// Record UUID.
    pub id: Uuid,
    /// Owning tenant.
    pub tenant: String,
    /// The draft these costs are passed through on.
    pub draft_id: Uuid,
    /// A `rubo4e::current::Fremdkosten`.
    pub fremdkosten_json: serde_json::Value,
    /// Operator-facing description.
    pub bezeichnung: Option<String>,
    /// Sum of the positions, in EUR.
    pub total_eur: Decimal,
    /// Insert time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Request body for `PUT /api/v1/billing/fremdkosten/{draft_id}`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertFremdkostenRequest {
    /// Operator-facing description.
    pub bezeichnung: Option<String>,
    /// A full `rubo4e::current::Fremdkosten` object.
    pub fremdkosten_json: serde_json::Value,
    /// Sum of the positions, in EUR.
    pub total_eur: Decimal,
}

/// Attach (or replace) the Fremdkosten of a draft.
///
/// # Errors
///
/// Propagates any database failure, including the foreign-key violation that
/// says the draft does not exist.
pub async fn upsert_fremdkosten(
    pool: &PgPool,
    tenant: &str,
    draft_id: Uuid,
    req: &UpsertFremdkostenRequest,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO fremdkosten_records
              (tenant, draft_id, fremdkosten_json, bezeichnung, total_eur)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (tenant, draft_id) DO UPDATE
          SET fremdkosten_json = EXCLUDED.fremdkosten_json,
              bezeichnung      = COALESCE(EXCLUDED.bezeichnung, fremdkosten_records.bezeichnung),
              total_eur        = EXCLUDED.total_eur,
              updated_at       = now()
          RETURNING id",
    )
    .bind(tenant)
    .bind(draft_id)
    .bind(&req.fremdkosten_json)
    .bind(&req.bezeichnung)
    .bind(req.total_eur)
    .fetch_one(pool)
    .await
    .context("upsert_fremdkosten")?;
    Ok(row.try_get("id")?)
}

/// The Fremdkosten attached to a draft.
///
/// # Errors
///
/// Propagates any database failure.
pub async fn fetch_fremdkosten(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    draft_id: Uuid,
) -> anyhow::Result<Option<FremdkostenRow>> {
    sqlx::query_as::<_, FremdkostenRow>(
        "SELECT * FROM fremdkosten_records WHERE tenant = $1 AND draft_id = $2",
    )
    .bind(tenant)
    .bind(draft_id)
    .fetch_optional(executor)
    .await
    .context("fetch_fremdkosten")
}
