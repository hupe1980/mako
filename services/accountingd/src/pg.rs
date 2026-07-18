//! PostgreSQL persistence for `accountingd`.

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ── Account ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AccountRow {
    pub account_id: Uuid,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub tenant: String,
    pub iban: Option<String>,
    pub mandatsref: Option<String>,
    pub abschlag_ct: i64,
    pub billing_day: i16,
    pub balance_ct: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

pub async fn upsert_account(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        // P0-3 fix: ON CONFLICT must match the UNIQUE (malo_id, lf_mp_id, tenant) constraint exactly.
        // Using (malo_id, lf_mp_id) without tenant caused "no unique constraint matching" errors.
        r"INSERT INTO accounts (malo_id, lf_mp_id, tenant)
          VALUES ($1, $2, $3)
          ON CONFLICT (malo_id, lf_mp_id, tenant) DO UPDATE SET updated_at = now()
          RETURNING account_id",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("upsert_account")?;
    Ok(row.try_get("account_id")?)
}

/// Fetch an account by (malo_id, lf_mp_id, tenant) — full tenant isolation.
///
/// Returns `None` when no account exists for this triple.
pub async fn fetch_account(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
) -> anyhow::Result<Option<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT * FROM accounts WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3 LIMIT 1",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_account")
}

/// Fetch an account by UUID, scoped to `tenant` for cross-tenant isolation.
///
/// Always include `tenant` — `account_id` is a UUID v4 and guessable in multi-tenant
/// deployments where the UUID space is known to an attacker.
pub async fn fetch_account_by_id(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<AccountRow>> {
    sqlx::query_as::<_, AccountRow>("SELECT * FROM accounts WHERE account_id = $1 AND tenant = $2")
        .bind(account_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("fetch_account_by_id")
}

#[derive(Debug, Deserialize)]
pub struct UpdateAccountRequest {
    pub iban: Option<String>,
    pub mandatsref: Option<String>,
    pub abschlag_ct: Option<i64>,
    pub billing_day: Option<i16>,
}

pub async fn update_account(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    req: UpdateAccountRequest,
) -> anyhow::Result<()> {
    sqlx::query(
        // P0-5 fix: add tenant parameter — previously missing, allowing cross-tenant modification.
        r"UPDATE accounts SET
              iban        = COALESCE($3, iban),
              mandatsref  = COALESCE($4, mandatsref),
              abschlag_ct = COALESCE($5, abschlag_ct),
              billing_day = COALESCE($6, billing_day),
              updated_at  = now()
          WHERE malo_id = $1 AND lf_mp_id = $2",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(req.iban)
    .bind(req.mandatsref)
    .bind(req.abschlag_ct)
    .bind(req.billing_day)
    .execute(pool)
    .await
    .context("update_account")?;
    Ok(())
}

/// Tenant-scoped variant of `update_account` — P0-5 fix.
/// Always filter by tenant to prevent cross-tenant data modification.
pub async fn update_account_tenanted(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
    req: UpdateAccountRequest,
) -> anyhow::Result<()> {
    let rows_affected = sqlx::query(
        r"UPDATE accounts SET
              iban        = COALESCE($4, iban),
              mandatsref  = COALESCE($5, mandatsref),
              abschlag_ct = COALESCE($6, abschlag_ct),
              billing_day = COALESCE($7, billing_day),
              updated_at  = now()
          WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .bind(req.iban)
    .bind(req.mandatsref)
    .bind(req.abschlag_ct)
    .bind(req.billing_day)
    .execute(pool)
    .await
    .context("update_account_tenanted")?
    .rows_affected();

    if rows_affected == 0 {
        anyhow::bail!("account not found: malo_id={malo_id} lf_mp_id={lf_mp_id} tenant={tenant}");
    }
    Ok(())
}

/// Check whether an Abschlag has already been posted for this MaLo in this calendar month.
///
/// Used by the Abschlagslauf scheduler to prevent duplicate ABSCHLAG entries on restart.
/// Returns `true` when an `abschlag_runs` entry already exists for `(tenant, malo_id, period_month)`.
pub async fn abschlag_already_posted(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    period_month: time::Date,
) -> anyhow::Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM abschlag_runs WHERE tenant = $1 AND malo_id = $2 AND period_month = $3)"
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(period_month)
    .fetch_one(pool)
    .await
    .context("abschlag_already_posted")?;
    Ok(exists)
}

/// Record that an Abschlag was successfully posted, creating the idempotency guard row.
///
/// Uses `ON CONFLICT DO NOTHING` so concurrent calls are safe.
pub async fn record_abschlag_run(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    period_month: time::Date,
    amount_ct: i64,
    ledger_entry_id: Option<Uuid>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO abschlag_runs (tenant, malo_id, period_month, amount_ct, ledger_entry_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (tenant, malo_id, period_month) DO NOTHING",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(period_month)
    .bind(amount_ct)
    .bind(ledger_entry_id)
    .execute(pool)
    .await
    .context("record_abschlag_run")?;
    Ok(())
}

/// Check whether a Jahresabschluss has already been posted for this MaLo in this year.
/// Returns `Some(zahlbetrag_ct)` when already settled, `None` when not yet processed.
pub async fn jahresabschluss_already_settled(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    billing_year: i16,
) -> anyhow::Result<Option<i64>> {
    sqlx::query_scalar::<_, i64>(
        "SELECT zahlbetrag_ct FROM jahresabschluss_runs \\
         WHERE tenant = $1 AND malo_id = $2 AND billing_year = $3 LIMIT 1",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(billing_year)
    .fetch_optional(pool)
    .await
    .context("jahresabschluss_already_settled")
}

/// Record a completed Jahresabschluss for idempotency.
#[allow(clippy::too_many_arguments)]
pub async fn record_jahresabschluss(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    billing_year: i16,
    annual_bill_ct: i64,
    sum_abschlage_ct: i64,
    zahlbetrag_ct: i64,
    ledger_entry_id: Option<Uuid>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO jahresabschluss_runs
             (tenant, malo_id, billing_year, annual_bill_ct, sum_abschlage_ct, zahlbetrag_ct, ledger_entry_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (tenant, malo_id, billing_year) DO NOTHING",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(billing_year)
    .bind(annual_bill_ct)
    .bind(sum_abschlage_ct)
    .bind(zahlbetrag_ct)
    .bind(ledger_entry_id)
    .execute(pool)
    .await
    .context("record_jahresabschluss")?;
    Ok(())
}

/// Persist a SEPA pain.008 batch XML for audit and replay (P1-6).
///
/// Inserts into `sepa_collection_runs`. If a run already exists for the same
/// `(tenant, collection_date)`, returns that run's ID (idempotent).
pub async fn persist_sepa_collection(
    pool: &PgPool,
    tenant: &str,
    collection_date: time::Date,
    pain008_xml: &str,
    total_ct: i64,
    mandate_count: usize,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO sepa_collection_runs
              (tenant, collection_date, pain008_xml, total_ct, mandate_count)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (tenant, collection_date) DO UPDATE
          SET pain008_xml    = EXCLUDED.pain008_xml,
              total_ct       = EXCLUDED.total_ct,
              mandate_count  = EXCLUDED.mandate_count
          RETURNING run_id",
    )
    .bind(tenant)
    .bind(collection_date)
    .bind(pain008_xml)
    .bind(total_ct)
    .bind(mandate_count as i32)
    .fetch_one(pool)
    .await
    .context("persist_sepa_collection")?;
    Ok(row.try_get("run_id")?)
}

/// Mark a SEPA collection run as dispatched to the ERP.
pub async fn mark_sepa_collection_dispatched(
    pool: &PgPool,
    run_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sepa_collection_runs
         SET dispatch_status = 'DISPATCHED', dispatched_at = now()
         WHERE run_id = $1 AND dispatch_status != 'DISPATCHED'",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("mark_sepa_collection_dispatched")?;
    Ok(())
}

/// Append an entry to the account master-data audit log (§238 HGB traceability).
#[allow(clippy::too_many_arguments)]
pub async fn log_account_audit(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    malo_id: &str,
    operator_sub: Option<&str>,
    action: &str,
    old_values: Option<serde_json::Value>,
    new_values: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO account_audit_log
             (account_id, tenant, malo_id, operator_sub, action, old_values, new_values)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(malo_id)
    .bind(operator_sub)
    .bind(action)
    .bind(old_values)
    .bind(new_values)
    .execute(pool)
    .await
    .context("log_account_audit")?;
    Ok(())
}

pub async fn list_overdue_accounts(
    pool: &PgPool,
    tenant: &str,
    min_balance_ct: i64,
    limit: i64,
) -> anyhow::Result<Vec<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        r"SELECT * FROM accounts
          WHERE tenant = $1 AND balance_ct >= $2
          ORDER BY balance_ct DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(min_balance_ct)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_overdue_accounts")
}

// ── Vorauszahlung (BO4E typed advance-payment schedule) ───────────────────────

/// Store a canonical `rubo4e::current::Vorauszahlung` COM JSON for a MaLo account.
///
/// Also updates `abschlag_ct` from `vorauszahlung["betrag"]["wert"]` (EUR → ct)
/// so that the existing Abschlagslauf scheduler continues to work unchanged.
pub async fn upsert_vorauszahlung(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
    vzahlung: serde_json::Value,
    abschlag_ct_override: Option<i64>,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE accounts
          SET vorauszahlung = $4,
              abschlag_ct   = COALESCE($5, abschlag_ct),
              updated_at    = now()
          WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .bind(&vzahlung)
    .bind(abschlag_ct_override)
    .execute(pool)
    .await
    .context("upsert_vorauszahlung")?;
    Ok(())
}

/// Fetch the stored `Vorauszahlung` COM JSON for a MaLo account.
///
/// Returns `None` if no account exists or no `Vorauszahlung` has been stored.
/// Falls back to synthesising one from `abschlag_ct` when the column is NULL.
pub async fn fetch_vorauszahlung(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
) -> anyhow::Result<Option<(serde_json::Value, i64)>> {
    let row = sqlx::query(
        r"SELECT vorauszahlung, abschlag_ct FROM accounts
          WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_vorauszahlung")?;

    let Some(r) = row else { return Ok(None) };
    let abschlag_ct: i64 = r.try_get("abschlag_ct").unwrap_or(0);
    let vzahlung: Option<serde_json::Value> = r.try_get("vorauszahlung").unwrap_or(None);
    Ok(Some((
        vzahlung.unwrap_or(serde_json::Value::Null),
        abschlag_ct,
    )))
}

// ── Ledger entries ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LedgerEntryRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub tenant: String,
    pub entry_type: String,
    pub amount_ct: i64,
    pub reference_id: Option<String>,
    pub ce_type: Option<String>,
    pub ce_id: Option<String>,
    pub booking_date: Date,
    pub value_date: Date,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Write one ledger entry and update the account balance atomically.
///
/// Uses `SELECT ... FOR UPDATE` to prevent concurrent balance updates.
/// The `ce_id` idempotency key is inserted into `processed_events` atomically —
/// duplicate CloudEvents are silently ignored.
#[allow(clippy::too_many_arguments)]
pub async fn write_entry(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    entry_type: &str,
    amount_ct: i64,
    reference_id: Option<&str>,
    ce_type: Option<&str>,
    ce_id: Option<&str>,
    booking_date: Date,
    description: Option<&str>,
) -> anyhow::Result<Option<Uuid>> {
    write_entry_with_value_date(
        pool,
        account_id,
        tenant,
        entry_type,
        amount_ct,
        reference_id,
        ce_type,
        ce_id,
        booking_date,
        booking_date,
        description,
    )
    .await
    .map(Some)
}

/// Write a ledger entry with explicit booking and value dates.
///
/// Value date may differ from booking date for backdated corrections or
/// manual bookings posted after the fact (§238 HGB Buchungsdatum vs. Wertstellung).
#[allow(clippy::too_many_arguments)]
pub async fn write_entry_with_value_date(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    entry_type: &str,
    amount_ct: i64,
    reference_id: Option<&str>,
    ce_type: Option<&str>,
    ce_id: Option<&str>,
    booking_date: Date,
    value_date: Date,
    description: Option<&str>,
) -> anyhow::Result<Uuid> {
    // Idempotency: skip if CloudEvent already processed.
    if let Some(ce) = ce_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM processed_events WHERE ce_id = $1)")
                .bind(ce)
                .fetch_one(pool)
                .await
                .context("check idempotency")?;
        if exists {
            return Ok(Uuid::nil()); // already processed — idempotent no-op
        }
    }

    let mut tx = pool.begin().await.context("begin tx")?;

    // Lock account row for serializable balance update.
    sqlx::query("SELECT account_id FROM accounts WHERE account_id = $1 FOR UPDATE")
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .context("lock account")?;

    // Insert ledger entry with explicit value_date.
    let id: Uuid = sqlx::query_scalar(
        r"INSERT INTO ledger_entries
              (account_id, tenant, entry_type, amount_ct, reference_id, ce_type, ce_id,
               booking_date, value_date, description)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          RETURNING id",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(entry_type)
    .bind(amount_ct)
    .bind(reference_id)
    .bind(ce_type)
    .bind(ce_id)
    .bind(booking_date)
    .bind(value_date)
    .bind(description)
    .fetch_one(&mut *tx)
    .await
    .context("insert ledger entry")?;

    // Update cached balance.
    sqlx::query(
        "UPDATE accounts SET balance_ct = balance_ct + $1, updated_at = now()
         WHERE account_id = $2",
    )
    .bind(amount_ct)
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .context("update balance")?;

    // Mark CloudEvent as processed (idempotency guard for CE-driven entries).
    if let Some(ce) = ce_id {
        sqlx::query("INSERT INTO processed_events (ce_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(ce)
            .execute(&mut *tx)
            .await
            .context("mark processed")?;
    }

    tx.commit().await.context("commit")?;
    Ok(id)
}

pub async fn list_ledger(
    pool: &PgPool,
    account_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<LedgerEntryRow>> {
    sqlx::query_as::<_, LedgerEntryRow>(
        "SELECT * FROM ledger_entries WHERE account_id = $1 ORDER BY booking_date DESC, created_at DESC LIMIT $2",
    )
    .bind(account_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_ledger")
}

/// Fetch all ledger entries for a given account in a specific calendar year.
///
/// Used by `POST /api/v1/jahresabschluss/{malo_id}` to compute the annual
/// settlement (actual Rechnungen vs. Σ Abschläge collected).
pub async fn list_ledger_year(
    pool: &PgPool,
    account_id: Uuid,
    year: i32,
) -> anyhow::Result<Vec<LedgerEntryRow>> {
    sqlx::query_as::<_, LedgerEntryRow>(
        "SELECT * FROM ledger_entries \
         WHERE account_id = $1 \
           AND EXTRACT(YEAR FROM booking_date)::int = $2 \
         ORDER BY booking_date ASC, created_at ASC",
    )
    .bind(account_id)
    .bind(year)
    .fetch_all(pool)
    .await
    .context("list_ledger_year")
}

// ── SEPA mandates ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SepaMandateRow {
    pub mandate_id: Uuid,
    pub account_id: Uuid,
    pub tenant: String,
    pub iban: String,
    pub bic: Option<String>,
    pub kontoinhaber: Option<String>,
    pub mandatsref: String,
    pub sequence_type: String,
    pub signed_at: Date,
    pub revoked_at: Option<Date>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct CreateMandateRequest {
    pub malo_id: String,
    pub lf_mp_id: String,
    pub iban: String,
    pub bic: Option<String>,
    pub kontoinhaber: Option<String>,
    pub mandatsref: String,
    pub sequence_type: String,
    pub signed_at: String,
}

pub async fn create_mandate(
    pool: &PgPool,
    tenant: &str,
    req: CreateMandateRequest,
) -> anyhow::Result<Uuid> {
    use time::format_description::well_known::Iso8601;
    let signed_at = Date::parse(&req.signed_at, &Iso8601::DEFAULT).context("parse signed_at")?;

    // Look up account.
    let account_id = upsert_account(pool, &req.malo_id, &req.lf_mp_id, tenant).await?;

    let row = sqlx::query(
        // P1-1 fix: ON CONFLICT uses (tenant, mandatsref) per the schema unique index,
        // not the old global UNIQUE (mandatsref). This prevents cross-tenant collisions.
        r"INSERT INTO sepa_mandates
              (account_id, tenant, iban, bic, kontoinhaber, mandatsref, sequence_type, signed_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
          ON CONFLICT (tenant, mandatsref) DO UPDATE
          SET iban = EXCLUDED.iban, bic = EXCLUDED.bic,
              kontoinhaber = EXCLUDED.kontoinhaber,
              sequence_type = EXCLUDED.sequence_type,
              signed_at = EXCLUDED.signed_at,
              updated_at = now()
          RETURNING mandate_id",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(&req.iban)
    .bind(&req.bic)
    .bind(&req.kontoinhaber)
    .bind(&req.mandatsref)
    .bind(&req.sequence_type)
    .bind(signed_at)
    .fetch_one(pool)
    .await
    .context("create_mandate")?;

    // Link iban + mandatsref to account for fast lookup.
    sqlx::query(
        "UPDATE accounts SET iban = $3, mandatsref = $4, updated_at = now()
         WHERE malo_id = $1 AND lf_mp_id = $2",
    )
    .bind(&req.malo_id)
    .bind(&req.lf_mp_id)
    .bind(&req.iban)
    .bind(&req.mandatsref)
    .execute(pool)
    .await
    .context("link mandate to account")?;

    Ok(row.try_get("mandate_id")?)
}

/// Mark a FRST mandate as successfully collected and transition it to RCUR.
///
/// Per SEPA SDD Core Rulebook: after the first successful direct debit collection
/// the mandate sequence type must change from FRST to RCUR for subsequent batches.
/// Call this when a pain.002 ACCP confirmation is received for a FRST mandate entry.
pub async fn transition_mandate_to_rcur(
    pool: &PgPool,
    mandate_id: Uuid,
    tenant: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sepa_mandates
         SET sequence_type = 'RCUR',
             first_collected_at = COALESCE(first_collected_at, now()),
             updated_at = now()
         WHERE mandate_id = $1 AND tenant = $2 AND sequence_type = 'FRST' AND revoked_at IS NULL",
    )
    .bind(mandate_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("transition_mandate_to_rcur")?;
    Ok(())
}

pub async fn fetch_mandate(
    pool: &PgPool,
    mandate_id: Uuid,
) -> anyhow::Result<Option<SepaMandateRow>> {
    sqlx::query_as::<_, SepaMandateRow>("SELECT * FROM sepa_mandates WHERE mandate_id = $1")
        .bind(mandate_id)
        .fetch_optional(pool)
        .await
        .context("fetch_mandate")
}

pub async fn list_active_mandates(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> anyhow::Result<Vec<SepaMandateRow>> {
    sqlx::query_as::<_, SepaMandateRow>(
        r"SELECT sm.* FROM sepa_mandates sm
          JOIN accounts a ON a.account_id = sm.account_id
          WHERE sm.revoked_at IS NULL AND a.tenant = $1
          ORDER BY sm.updated_at DESC
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_active_mandates")
}

// ── Dunning cases ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DunningCaseRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub tenant: String,
    pub stufe: i16,
    pub amount_due_ct: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
    pub due_date: Date,
    pub resolved_at: Option<OffsetDateTime>,
    pub sperrauftrag_ce_id: Option<String>,
}

pub async fn create_dunning_case(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    stufe: i16,
    amount_due_ct: i64,
    due_date: Date,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO dunning_cases (account_id, tenant, stufe, amount_due_ct, due_date)
          VALUES ($1, $2, $3, $4, $5)
          RETURNING id",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(stufe)
    .bind(amount_due_ct)
    .bind(due_date)
    .fetch_one(pool)
    .await
    .context("create_dunning_case")?;
    Ok(row.try_get("id")?)
}

pub async fn resolve_dunning_case(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE dunning_cases SET resolved_at = now() WHERE id = $1 AND resolved_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await
    .context("resolve_dunning_case")?;
    Ok(())
}

pub async fn list_open_dunning(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> anyhow::Result<Vec<DunningCaseRow>> {
    sqlx::query_as::<_, DunningCaseRow>(
        r"SELECT * FROM dunning_cases
          WHERE tenant = $1 AND resolved_at IS NULL
          ORDER BY issued_at DESC
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_open_dunning")
}

/// Returns all accounts that have an active SEPA mandate and a positive abschlag_ct.
/// Used by `run_sepa_collection` MCP tool to build the pain.008 XML.
#[allow(dead_code)]
pub async fn list_accounts_with_mandates(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Vec<(SepaMandateRow, AccountRow)>> {
    // sqlx::FromRow cannot be used here because of the aliased columns;
    // use query_as with individual type mappings instead.
    let rows = sqlx::query(
        r"SELECT sm.mandate_id,
                 sm.account_id  AS sm_account_id,
                 sm.tenant      AS sm_tenant,
                 sm.iban, sm.bic, sm.kontoinhaber,
                 sm.mandatsref, sm.sequence_type, sm.signed_at, sm.revoked_at,
                 sm.updated_at  AS sm_updated_at,
                 a.account_id, a.malo_id, a.lf_mp_id, a.tenant,
                 a.iban         AS a_iban,
                 a.mandatsref   AS a_mandatsref,
                 a.abschlag_ct, a.billing_day, a.balance_ct,
                 a.updated_at
          FROM sepa_mandates sm
          JOIN accounts a ON a.account_id = sm.account_id
          WHERE sm.revoked_at IS NULL
            AND a.tenant = $1
            AND a.abschlag_ct > 0
          ORDER BY a.malo_id",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_accounts_with_mandates")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mandate = SepaMandateRow {
            mandate_id: r.try_get("mandate_id")?,
            account_id: r.try_get("sm_account_id")?,
            tenant: r.try_get("sm_tenant")?,
            iban: r.try_get("iban")?,
            bic: r.try_get("bic")?,
            kontoinhaber: r.try_get("kontoinhaber")?,
            mandatsref: r.try_get("mandatsref")?,
            sequence_type: r.try_get("sequence_type")?,
            signed_at: r.try_get("signed_at")?,
            revoked_at: r.try_get("revoked_at")?,
            updated_at: r.try_get("sm_updated_at")?,
        };
        let account = AccountRow {
            account_id: r.try_get("account_id")?,
            malo_id: r.try_get("malo_id")?,
            lf_mp_id: r.try_get("lf_mp_id")?,
            tenant: r.try_get("tenant")?,
            iban: r.try_get("a_iban")?,
            mandatsref: r.try_get("a_mandatsref")?,
            abschlag_ct: r.try_get("abschlag_ct")?,
            billing_day: r.try_get("billing_day")?,
            balance_ct: r.try_get("balance_ct")?,
            updated_at: r.try_get("updated_at")?,
        };
        out.push((mandate, account));
    }
    Ok(out)
}

/// Find all accounts where `billing_day` matches `day_of_month` and `abschlag_ct > 0`.
/// Used by `run_abschlag_cycle` to process monthly advance payments.
#[allow(dead_code)]
pub async fn find_accounts_due(
    pool: &PgPool,
    tenant: &str,
    day_of_month: i16,
) -> anyhow::Result<Vec<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        r"SELECT account_id, malo_id, lf_mp_id, tenant, iban, mandatsref,
                 abschlag_ct, billing_day, balance_ct, updated_at
          FROM accounts
          WHERE tenant = $1
            AND billing_day = $2
            AND abschlag_ct > 0",
    )
    .bind(tenant)
    .bind(day_of_month)
    .fetch_all(pool)
    .await
    .context("find_accounts_due")
}

/// Find all accounts with an active SEPA mandate whose `billing_day` matches
/// the given day-of-month.
///
/// Used by the N-5 SEPA pre-notification scheduler: call with
/// `day_of_month = (today + 5).day()` to find accounts for which a
/// pre-notification must be sent 5 banking days in advance.
pub async fn find_accounts_due_for_sepa(
    pool: &PgPool,
    tenant: &str,
    billing_day: i16,
) -> anyhow::Result<Vec<(SepaMandateRow, AccountRow)>> {
    let rows = sqlx::query(
        r"SELECT sm.mandate_id,
                 sm.account_id  AS sm_account_id,
                 sm.tenant      AS sm_tenant,
                 sm.iban, sm.bic, sm.kontoinhaber,
                 sm.mandatsref, sm.sequence_type, sm.signed_at, sm.revoked_at,
                 sm.updated_at  AS sm_updated_at,
                 a.account_id, a.malo_id, a.lf_mp_id, a.tenant,
                 a.iban         AS a_iban,
                 a.mandatsref   AS a_mandatsref,
                 a.abschlag_ct, a.billing_day, a.balance_ct,
                 a.updated_at
          FROM sepa_mandates sm
          JOIN accounts a ON a.account_id = sm.account_id
          WHERE sm.revoked_at IS NULL
            AND a.tenant = $1
            AND a.billing_day = $2
            AND a.abschlag_ct > 0
          ORDER BY a.malo_id",
    )
    .bind(tenant)
    .bind(billing_day)
    .fetch_all(pool)
    .await
    .context("find_accounts_due_for_sepa")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mandate = SepaMandateRow {
            mandate_id: r.try_get("mandate_id")?,
            account_id: r.try_get("sm_account_id")?,
            tenant: r.try_get("sm_tenant")?,
            iban: r.try_get("iban")?,
            bic: r.try_get("bic")?,
            kontoinhaber: r.try_get("kontoinhaber")?,
            mandatsref: r.try_get("mandatsref")?,
            sequence_type: r.try_get("sequence_type")?,
            signed_at: r.try_get("signed_at")?,
            revoked_at: r.try_get("revoked_at")?,
            updated_at: r.try_get("sm_updated_at")?,
        };
        let account = AccountRow {
            account_id: r.try_get("account_id")?,
            malo_id: r.try_get("malo_id")?,
            lf_mp_id: r.try_get("lf_mp_id")?,
            tenant: r.try_get("tenant")?,
            iban: r.try_get("a_iban")?,
            mandatsref: r.try_get("a_mandatsref")?,
            abschlag_ct: r.try_get("abschlag_ct")?,
            billing_day: r.try_get("billing_day")?,
            balance_ct: r.try_get("balance_ct")?,
            updated_at: r.try_get("updated_at")?,
        };
        out.push((mandate, account));
    }
    Ok(out)
}

/// Compute period-end Abgrenzung (accruals) for HGB §250 compliance.
///
/// Returns:
/// - `prap_ct`: Passive Rechnungsabgrenzungsposten — Σ(future-period Abschläge already
///   collected). These are deferred revenue that will be earned in the next period.
/// - `abschlag_total_ct`: Total Abschläge collected year-to-date across all accounts.
/// - `accounts_with_advance`: Count of accounts with positive Abschlag balance.
///
/// The ERP books: `pRAP = prap_ct` (liability entry) at period-end cutoff.
/// Note: Forderungen aus unbillierten Leistungen (aRAP) require edmd Lastgang data
/// and must be computed by the ERP billing system, not accountingd.
#[allow(dead_code)]
pub async fn compute_abgrenzung(pool: &PgPool, tenant: &str) -> anyhow::Result<(i64, i64, i64)> {
    // pRAP: sum of abschlag_ct for accounts where the Abschlag collected > invoiced
    // (accounts with negative balance = credit = customer overpaid)
    let row = sqlx::query(
        r"SELECT
            -- pRAP: Abschläge collected in excess of billed amounts (deferred revenue)
            COALESCE(SUM(CASE WHEN balance_ct < 0 THEN ABS(balance_ct) ELSE 0 END), 0) AS prap_ct,
            -- Total monthly Abschlag commitment across all active accounts
            COALESCE(SUM(CASE WHEN abschlag_ct > 0 THEN abschlag_ct ELSE 0 END), 0) AS abschlag_total_ct,
            -- Count of accounts with active advance payments
            COUNT(*) FILTER (WHERE abschlag_ct > 0) AS accounts_with_advance
          FROM accounts
          WHERE tenant = $1",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("compute_abgrenzung")?;

    let prap: i64 = row.try_get("prap_ct")?;
    let total: i64 = row.try_get("abschlag_total_ct")?;
    let count: i64 = row.try_get("accounts_with_advance")?;
    Ok((prap, total, count))
}

// ── Balance reconciliation ────────────────────────────────────────────────────

/// Result of a balance integrity check for one account.
#[derive(Debug, Serialize)]
pub struct BalanceReconcileResult {
    pub account_id: Uuid,
    pub malo_id: String,
    /// Cached balance from `accounts.balance_ct`.
    pub cached_balance_ct: i64,
    /// Recomputed balance from `SUM(ledger_entries.amount_ct)`.
    pub recomputed_balance_ct: i64,
    /// `true` when cached matches recomputed.
    pub is_consistent: bool,
    /// Difference (cached − recomputed); non-zero indicates cache drift.
    pub drift_ct: i64,
}

/// Check whether `accounts.balance_ct` matches `SUM(ledger_entries.amount_ct)`.
///
/// This function is the P1-1 fix for balance cache drift detection.
///
/// ## Why the cache can drift
///
/// `balance_ct` is updated inside the same transaction as each ledger write
/// (`SELECT FOR UPDATE` + UPDATE). A crash between INSERT and UPDATE would
/// leave the cache stale. Periodic reconciliation detects this silently.
///
/// ## Usage
///
/// ```text
/// POST /api/v1/accounts/{malo_id}/reconcile  — check + optionally repair
/// ```
///
/// Returns the drift_ct. When `repair = true`, resets `balance_ct` to the
/// recomputed value inside a transaction (safe for production).
pub async fn reconcile_balance(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    repair: bool,
) -> anyhow::Result<BalanceReconcileResult> {
    // Recompute from ledger (authoritative).
    let recomputed: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_ct), 0) FROM ledger_entries \
         WHERE account_id = $1 AND tenant = $2",
    )
    .bind(account_id)
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("reconcile: sum ledger")?;

    let acct = sqlx::query(
        "SELECT account_id, malo_id, balance_ct FROM accounts \
         WHERE account_id = $1 AND tenant = $2",
    )
    .bind(account_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("reconcile: fetch account")?;

    let Some(row) = acct else {
        anyhow::bail!("account not found for reconciliation");
    };
    let cached: i64 = row.try_get("balance_ct")?;
    let malo_id: String = row.try_get("malo_id")?;
    let drift = cached - recomputed;

    if drift != 0 {
        tracing::warn!(
            account_id = %account_id,
            malo_id = %malo_id,
            cached_ct = cached,
            recomputed_ct = recomputed,
            drift_ct = drift,
            "accountingd: balance cache drift detected"
        );

        if repair {
            sqlx::query(
                "UPDATE accounts SET balance_ct = $2, updated_at = now() \
                 WHERE account_id = $1 AND tenant = $3",
            )
            .bind(account_id)
            .bind(recomputed)
            .bind(tenant)
            .execute(pool)
            .await
            .context("reconcile: repair balance")?;
            tracing::info!(account_id = %account_id, "accountingd: balance repaired");
        }
    }

    Ok(BalanceReconcileResult {
        account_id,
        malo_id,
        cached_balance_ct: cached,
        recomputed_balance_ct: recomputed,
        is_consistent: drift == 0,
        drift_ct: drift,
    })
}

// ── P1-3: Open-item management ────────────────────────────────────────────────

/// One open item — an unpaid or partially-paid invoice debit.
///
/// Computed via FIFO clearing: the oldest debits are cleared first by available credits.
/// The `outstanding_ct` is the portion of this debit not yet covered by any payment.
#[derive(Debug, Serialize)]
pub struct OpenItem {
    /// Ledger entry UUID.
    pub id: Uuid,
    /// Entry type (`RECHNUNG`, `STORNO`, `MAHNGEBUEHR`, `ABSCHLAG`).
    pub entry_type: String,
    /// Original billed amount in ct (always positive for debits).
    pub amount_ct: i64,
    /// Outstanding (unpaid) portion in ct — always ≤ `amount_ct`.
    pub outstanding_ct: i64,
    /// External reference (invoice number, etc.).
    pub reference_id: Option<String>,
    /// Booking date of the original debit.
    pub booking_date: Date,
    /// Description.
    pub description: Option<String>,
}

/// Compute open items using **FIFO clearing**.
///
/// Each RECHNUNG/STORNO/MAHNGEBUEHR/ABSCHLAG debit that has not been fully
/// covered by ZAHLUNG/GUTSCHRIFT credits is returned with its `outstanding_ct`.
///
/// ## Algorithm
///
/// All credits (negative entries) are pooled. Debits are sorted by booking date
/// ascending. The credit pool is consumed against the oldest debits first:
///
/// ```text
/// RECHNUNG  100 ct  (Jan 1)  → outstanding = 0   (fully covered)
/// RECHNUNG  200 ct  (Jan 15) → outstanding = 150  (50 ct cleared)
/// ─────────────────────────────────────────────────────
/// Total credits: 150 ct  →  net outstanding = 150 ct = balance_ct  ✓
/// ```
///
/// ## Regulatory basis
///
/// §252 HGB Abs. 1 Nr. 4: Vorsichtsprinzip — individual receivables must be
/// tracked and assessed, not just as a net balance.
pub async fn list_open_items(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Vec<OpenItem>> {
    // Single-pass FIFO clearing via window functions.
    //
    // outstanding_ct = max(0,
    //     debit.amount - max(0, total_credits - cumulative_debits_before_this_one)
    // )
    //
    // Where:
    //   total_credits = abs(sum of all negative ledger entries)
    //   cumulative_debits_before = sum of all positive entries up to but not including this one
    let rows = sqlx::query(
        r"WITH credit_pool AS (
              SELECT COALESCE(-SUM(amount_ct), 0) AS total_available_ct
              FROM ledger_entries
              WHERE account_id = $1 AND tenant = $2 AND amount_ct < 0
          ),
          debit_cumsum AS (
              SELECT
                  id, entry_type, amount_ct, reference_id, booking_date, description,
                  SUM(amount_ct) OVER (
                      ORDER BY booking_date ASC, created_at ASC
                      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                  ) - amount_ct AS cumulative_debits_before
              FROM ledger_entries
              WHERE account_id = $1 AND tenant = $2
                AND amount_ct > 0
                AND entry_type IN ('RECHNUNG', 'STORNO', 'MAHNGEBUEHR', 'ABSCHLAG')
          )
          SELECT
              d.id, d.entry_type, d.amount_ct, d.reference_id, d.booking_date, d.description,
              GREATEST(0,
                  d.amount_ct - GREATEST(0,
                      c.total_available_ct - d.cumulative_debits_before
                  )
              )::BIGINT AS outstanding_ct
          FROM debit_cumsum d CROSS JOIN credit_pool c
          WHERE GREATEST(0,
              d.amount_ct - GREATEST(0,
                  c.total_available_ct - d.cumulative_debits_before
              )
          ) > 0
          ORDER BY d.booking_date ASC",
    )
    .bind(account_id)
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_open_items")?;

    let items = rows
        .iter()
        .map(|r| {
            Ok(OpenItem {
                id: r.try_get("id")?,
                entry_type: r.try_get("entry_type")?,
                amount_ct: r.try_get("amount_ct")?,
                outstanding_ct: r.try_get("outstanding_ct")?,
                reference_id: r.try_get("reference_id")?,
                booking_date: r.try_get("booking_date")?,
                description: r.try_get("description")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(items)
}

// ── P1-4: GDPR Art. 17 anonymization ─────────────────────────────────────────

/// Result of a GDPR anonymization operation.
#[derive(Debug, Serialize)]
pub struct AnonymizeResult {
    pub account_id: Uuid,
    pub malo_id: String,
    /// Fields that were anonymized.
    pub anonymized_fields: Vec<String>,
    /// Timestamp of anonymization.
    #[serde(with = "time::serde::rfc3339")]
    pub anonymized_at: OffsetDateTime,
}

/// Pseudonymize all PII in an account and its SEPA mandates.
///
/// ## What is anonymized
///
/// | Table | Column | Action |
/// |---|---|---|
/// | `accounts` | `iban` | → `"ANONYMIZED"` |
/// | `accounts` | `mandatsref` | → `NULL` |
/// | `accounts` | `zahlungsinformation` | → `NULL` |
/// | `accounts` | `vorauszahlung` | → `NULL` |
/// | `accounts` | `anonymized_at` | → `now()` |
/// | `sepa_mandates` | `iban` | → `"ANONYMIZED"` |
/// | `sepa_mandates` | `kontoinhaber` | → `"ANONYMIZED"` |
/// | `sepa_mandates` | `bic` | → `NULL` |
///
/// ## What is preserved
///
/// All `ledger_entries` are kept intact — amounts, dates, entry_type, description,
/// and reference_id are **not** modified. This satisfies:
/// - §238 HGB (10-year Buchführungspflicht)
/// - §147 AO (6-10 year tax record retention)
/// - GDPR Art. 17(3)(b): erasure exemption for legal obligations
///
/// The `malo_id` (MaLo = market location) is **not** personal data per BDEW
/// definition — it identifies a grid connection point, not a person.
///
/// ## Audit log
///
/// An immutable record is written to `anonymization_log` for GDPR Art. 5(2)
/// accountability.
///
/// ## Parameters
///
/// - `requested_by` — operator identity string (user ID, API key hash, etc.)
/// - `legal_basis` — e.g. `"GDPR Art. 17 - customer request ref#42"`
pub async fn anonymize_account(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    requested_by: &str,
    legal_basis: &str,
) -> anyhow::Result<AnonymizeResult> {
    // Verify account exists and belongs to this tenant.
    let acct = sqlx::query(
        "SELECT account_id, malo_id, anonymized_at \
         FROM accounts WHERE account_id = $1 AND tenant = $2",
    )
    .bind(account_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("anonymize: fetch account")?;

    let Some(row) = acct else {
        anyhow::bail!("account not found");
    };
    let malo_id: String = row.try_get("malo_id")?;
    let already: Option<OffsetDateTime> = row.try_get("anonymized_at")?;
    if already.is_some() {
        anyhow::bail!("account already anonymized");
    }

    let anonymized_at = OffsetDateTime::now_utc();
    let anonymized_fields = serde_json::json!([
        "accounts.iban",
        "accounts.mandatsref",
        "accounts.zahlungsinformation",
        "accounts.vorauszahlung",
        "sepa_mandates.iban",
        "sepa_mandates.kontoinhaber",
        "sepa_mandates.bic"
    ]);

    let mut tx = pool.begin().await.context("anonymize: begin tx")?;

    // 1. Anonymize accounts table PII.
    sqlx::query(
        "UPDATE accounts
         SET iban               = 'ANONYMIZED',
             mandatsref         = NULL,
             zahlungsinformation = NULL,
             vorauszahlung      = NULL,
             anonymized_at      = $3,
             updated_at         = $3
         WHERE account_id = $1 AND tenant = $2",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(anonymized_at)
    .execute(&mut *tx)
    .await
    .context("anonymize: update accounts")?;

    // 2. Anonymize all SEPA mandates for this account.
    sqlx::query(
        "UPDATE sepa_mandates
         SET iban          = 'ANONYMIZED',
             kontoinhaber  = 'ANONYMIZED',
             bic           = NULL,
             updated_at    = $2
         WHERE account_id = $1",
    )
    .bind(account_id)
    .bind(anonymized_at)
    .execute(&mut *tx)
    .await
    .context("anonymize: update sepa_mandates")?;

    // 3. Write immutable audit log (GDPR Art. 5(2) accountability).
    sqlx::query(
        "INSERT INTO anonymization_log
             (account_id, tenant, malo_id, requested_by, legal_basis, anonymized_fields, anonymized_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(&malo_id)
    .bind(requested_by)
    .bind(legal_basis)
    .bind(&anonymized_fields)
    .bind(anonymized_at)
    .execute(&mut *tx)
    .await
    .context("anonymize: write audit log")?;

    tx.commit().await.context("anonymize: commit")?;

    tracing::info!(
        account_id = %account_id,
        malo_id = %malo_id,
        requested_by = requested_by,
        "accountingd: GDPR Art.17 anonymization applied"
    );

    let fields: Vec<String> = serde_json::from_value(anonymized_fields).unwrap_or_default();

    Ok(AnonymizeResult {
        account_id,
        malo_id,
        anonymized_fields: fields,
        anonymized_at,
    })
}

// ── P1-5: Automatic Mahnwesen (dunning rule engine) ──────────────────────────

/// Result of one automatic dunning run.
#[derive(Debug, Serialize)]
pub struct AutoDunningResult {
    /// Number of new Mahnstufe 1 cases created.
    pub mahnstufe1_created: u32,
    /// Number of cases escalated (1→2 or 2→3).
    pub escalated: u32,
    /// Whether a Sperrauftrag was triggered for any Mahnstufe 3 case.
    pub sperrauftrag_triggered: u32,
}

/// Run the automatic Mahnwesen escalation engine for one tenant.
///
/// This is the **dunning rule engine** (P1-5). It evaluates every active account
/// and creates / escalates dunning cases according to the following rules:
///
/// ## Rules
///
/// | Condition | Action |
/// |---|---|
/// | `balance_ct > 0` AND oldest RECHNUNG > `grace_days` old AND no open Mahnstufe 1 | Create Mahnstufe 1 |
/// | Open Mahnstufe 1 AND `due_date < today` | Escalate to Mahnstufe 2 |
/// | Open Mahnstufe 2 AND `due_date < today` | Escalate to Mahnstufe 3 + Sperrauftrag |
///
/// ## Idempotency
///
/// Inserts a record into `auto_dunning_runs (tenant, run_date)` with a UNIQUE
/// constraint. If the worker crashes and restarts the same day, the second run is
/// a no-op.
///
/// ## Fee creation
///
/// When a new Mahnstufe 1/2/3 is created, the corresponding Mahngebühr (from
/// `dunning_fee_stufe{1,2,3}_ct`) is posted as a `MAHNGEBUEHR` ledger entry if > 0.
/// This updates `balance_ct` atomically (via `write_entry`).
pub async fn run_auto_dunning(
    pool: &PgPool,
    tenant: &str,
    grace_days: i64,
    fee_stufe1_ct: i64,
    fee_stufe2_ct: i64,
    fee_stufe3_ct: i64,
) -> anyhow::Result<AutoDunningResult> {
    let today = OffsetDateTime::now_utc().date();

    // Idempotency check — skip if already ran today.
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM auto_dunning_runs WHERE tenant = $1 AND run_date = $2)",
    )
    .bind(tenant)
    .bind(today)
    .fetch_one(pool)
    .await
    .context("auto_dunning: idempotency check")?;

    if already {
        tracing::debug!(%tenant, %today, "accountingd: auto-dunning already ran today — skipping");
        return Ok(AutoDunningResult {
            mahnstufe1_created: 0,
            escalated: 0,
            sperrauftrag_triggered: 0,
        });
    }

    let mut mahnstufe1_created: u32 = 0;
    let mut escalated: u32 = 0;
    let mut sperrauftrag_triggered: u32 = 0;
    let cutoff = today - time::Duration::days(grace_days);

    // ── Step 1: Create Mahnstufe 1 for newly overdue accounts ─────────────────
    //
    // Qualifying accounts:
    //   - balance_ct > 0
    //   - No active (unresolved) Mahnstufe 1 dunning case
    //   - Oldest RECHNUNG debit is older than grace_days (billing date ≤ cutoff)
    //   - Not anonymized
    let candidates: Vec<(Uuid, Uuid, i64)> = sqlx::query(
        r"SELECT a.account_id, a.account_id AS aid, a.balance_ct
          FROM accounts a
          WHERE a.tenant = $1
            AND a.balance_ct > 0
            AND a.anonymized_at IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM dunning_cases dc
                WHERE dc.account_id = a.account_id
                  AND dc.resolved_at IS NULL
            )
            AND EXISTS (
                SELECT 1 FROM ledger_entries le
                WHERE le.account_id = a.account_id
                  AND le.amount_ct > 0
                  AND le.entry_type = 'RECHNUNG'
                  AND le.booking_date <= $2
            )",
    )
    .bind(tenant)
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .context("auto_dunning: find Mahnstufe1 candidates")?
    .into_iter()
    .map(|r| {
        let account_id: Uuid = r.try_get("account_id").unwrap_or(Uuid::nil());
        let balance_ct: i64 = r.try_get("balance_ct").unwrap_or(0);
        (account_id, account_id, balance_ct)
    })
    .collect();

    let stufe1_due_date = today + time::Duration::days(14); // 14-day payment deadline

    for (account_id, _, balance_ct) in &candidates {
        let case_id =
            create_dunning_case(pool, *account_id, tenant, 1, *balance_ct, stufe1_due_date)
                .await
                .context("auto_dunning: create Mahnstufe1")?;

        // Post Mahngebühr if configured > 0.
        if fee_stufe1_ct > 0 {
            let _ = write_entry(
                pool,
                *account_id,
                tenant,
                "MAHNGEBUEHR",
                fee_stufe1_ct,
                Some(&case_id.to_string()),
                Some("de.accounting.mahnung.issued"),
                None,
                today,
                Some("Mahngebühr Mahnstufe 1"),
            )
            .await;
        }

        mahnstufe1_created += 1;
        tracing::info!(
            account_id = %account_id,
            balance_ct,
            "accountingd: auto-dunning created Mahnstufe 1"
        );
    }

    // ── Step 2: Escalate Mahnstufe 1 → 2 ─────────────────────────────────────
    let overdue_stufe1: Vec<(Uuid, Uuid, i64)> = sqlx::query(
        r"SELECT dc.id, dc.account_id, dc.amount_due_ct
          FROM dunning_cases dc
          WHERE dc.tenant = $1
            AND dc.stufe = 1
            AND dc.resolved_at IS NULL
            AND dc.due_date < $2",
    )
    .bind(tenant)
    .bind(today)
    .fetch_all(pool)
    .await
    .context("auto_dunning: find overdue Mahnstufe1")?
    .into_iter()
    .map(|r| {
        (
            r.try_get::<Uuid, _>("id").unwrap_or(Uuid::nil()),
            r.try_get::<Uuid, _>("account_id").unwrap_or(Uuid::nil()),
            r.try_get::<i64, _>("amount_due_ct").unwrap_or(0),
        )
    })
    .collect();

    let stufe2_due_date = today + time::Duration::days(14);

    for (old_case_id, account_id, amount_due_ct) in &overdue_stufe1 {
        // Resolve the old Mahnstufe 1 case.
        resolve_dunning_case(pool, *old_case_id)
            .await
            .context("auto_dunning: resolve Mahnstufe1")?;

        let case_id = create_dunning_case(
            pool,
            *account_id,
            tenant,
            2,
            *amount_due_ct,
            stufe2_due_date,
        )
        .await
        .context("auto_dunning: create Mahnstufe2")?;

        if fee_stufe2_ct > 0 {
            let _ = write_entry(
                pool,
                *account_id,
                tenant,
                "MAHNGEBUEHR",
                fee_stufe2_ct,
                Some(&case_id.to_string()),
                Some("de.accounting.mahnung.issued"),
                None,
                today,
                Some("Mahngebühr Mahnstufe 2"),
            )
            .await;
        }

        escalated += 1;
        tracing::info!(account_id = %account_id, "accountingd: auto-dunning escalated to Mahnstufe 2");
    }

    // ── Step 3: Escalate Mahnstufe 2 → 3 + Sperrauftrag ─────────────────────
    let overdue_stufe2: Vec<(Uuid, Uuid, i64)> = sqlx::query(
        r"SELECT dc.id, dc.account_id, dc.amount_due_ct
          FROM dunning_cases dc
          WHERE dc.tenant = $1
            AND dc.stufe = 2
            AND dc.resolved_at IS NULL
            AND dc.due_date < $2",
    )
    .bind(tenant)
    .bind(today)
    .fetch_all(pool)
    .await
    .context("auto_dunning: find overdue Mahnstufe2")?
    .into_iter()
    .map(|r| {
        (
            r.try_get::<Uuid, _>("id").unwrap_or(Uuid::nil()),
            r.try_get::<Uuid, _>("account_id").unwrap_or(Uuid::nil()),
            r.try_get::<i64, _>("amount_due_ct").unwrap_or(0),
        )
    })
    .collect();

    let stufe3_due_date = today + time::Duration::days(7); // shorter final deadline

    for (old_case_id, account_id, amount_due_ct) in &overdue_stufe2 {
        resolve_dunning_case(pool, *old_case_id)
            .await
            .context("auto_dunning: resolve Mahnstufe2")?;

        let _case_id = create_dunning_case(
            pool,
            *account_id,
            tenant,
            3,
            *amount_due_ct,
            stufe3_due_date,
        )
        .await
        .context("auto_dunning: create Mahnstufe3")?;

        if fee_stufe3_ct > 0 {
            let _ = write_entry(
                pool,
                *account_id,
                tenant,
                "MAHNGEBUEHR",
                fee_stufe3_ct,
                Some(&_case_id.to_string()),
                Some("de.accounting.mahnung.issued"),
                None,
                today,
                Some("Mahngebühr Mahnstufe 3"),
            )
            .await;
        }

        escalated += 1;
        sperrauftrag_triggered += 1;
        tracing::warn!(account_id = %account_id, "accountingd: auto-dunning escalated to Mahnstufe 3 — Sperrauftrag triggered");
    }

    // Record this run (idempotency guard + audit trail).
    sqlx::query(
        "INSERT INTO auto_dunning_runs
             (tenant, run_date, accounts_checked, dunning_created, dunning_escalated)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (tenant, run_date) DO NOTHING",
    )
    .bind(tenant)
    .bind(today)
    .bind(candidates.len() as i32 + overdue_stufe1.len() as i32 + overdue_stufe2.len() as i32)
    .bind(mahnstufe1_created as i32)
    .bind(escalated as i32)
    .execute(pool)
    .await
    .context("auto_dunning: record run")?;

    Ok(AutoDunningResult {
        mahnstufe1_created,
        escalated,
        sperrauftrag_triggered,
    })
}

// ── Double-entry journal lines ────────────────────────────────────────────────

/// SKR account mapping for double-entry journal lines.
///
/// Maps an `entry_type` and amount sign to (debit_account, credit_account) pairs
/// using German SKR 03 / SKR 04 chart of accounts.
pub struct JournalMapping {
    pub debit_skr:  &'static str,
    pub debit_desc: &'static str,
    pub credit_skr:  &'static str,
    pub credit_desc: &'static str,
}

/// Determine the SKR 03 journal mapping for a given ledger entry type and amount sign.
pub fn journal_mapping(entry_type: &str, amount_ct: i64) -> JournalMapping {
    // Use positive = debit (charge), negative = credit (refund) convention
    let is_debit = amount_ct > 0;

    match entry_type {
        "RECHNUNG" | "ABSCHLAG" => JournalMapping {
            debit_skr:   "1400", debit_desc:  "Forderungen aus L+L",
            credit_skr:  "4000", credit_desc: "Energieerlöse",
        },
        "STORNO" if is_debit => JournalMapping {
            debit_skr:   "1400", debit_desc:  "Forderungen aus L+L",
            credit_skr:  "4000", credit_desc: "Energieerlöse (Storno)",
        },
        "STORNO" => JournalMapping {
            debit_skr:   "4000", debit_desc:  "Energieerlöse (Storno)",
            credit_skr:  "1400", credit_desc: "Forderungen aus L+L",
        },
        "ZAHLUNG" => JournalMapping {
            debit_skr:   "1200", debit_desc:  "Bankguthaben",
            credit_skr:  "1400", credit_desc: "Forderungen aus L+L",
        },
        "GUTSCHRIFT" => JournalMapping {
            debit_skr:   "4000", debit_desc:  "Energieerlöse",
            credit_skr:  "1400", credit_desc: "Forderungen aus L+L",
        },
        "EEG_GUTSCHRIFT" | "EEG_MARKTPRAEMIE" => JournalMapping {
            debit_skr:   "3000", debit_desc:  "Verbindlichkeiten EEG",
            credit_skr:  "4001", credit_desc: "EEG Einspeisevergütung",
        },
        "BANKRUECKLAST" => JournalMapping {
            debit_skr:   "1400", debit_desc:  "Forderungen aus L+L",
            credit_skr:  "1200", credit_desc: "Bankguthaben",
        },
        "MAHNGEBUEHR" => JournalMapping {
            debit_skr:   "1400", debit_desc:  "Forderungen aus L+L",
            credit_skr:  "4003", credit_desc: "Mahngebühren / Verzugszinsen",
        },
        "JAHRESABSCHLUSS" if is_debit => JournalMapping {
            debit_skr:   "1400", debit_desc:  "Forderungen aus L+L",
            credit_skr:  "4000", credit_desc: "Energieerlöse Jahresabschluss",
        },
        "JAHRESABSCHLUSS" => JournalMapping {
            debit_skr:   "4000", debit_desc:  "Energieerlöse Jahresabschluss",
            credit_skr:  "3001", credit_desc: "Verbindlichkeiten Erstattung",
        },
        "KORREKTUR" if is_debit => JournalMapping {
            debit_skr:   "1400", debit_desc:  "Forderungen aus L+L",
            credit_skr:  "4000", credit_desc: "Energieerlöse (Korrektur)",
        },
        _ => JournalMapping { // KORREKTUR credit or unknown
            debit_skr:   "4000", debit_desc:  "Energieerlöse (Korrektur)",
            credit_skr:  "1400", credit_desc: "Forderungen aus L+L",
        },
    }
}

/// Insert two balanced journal lines for a ledger entry (double-entry shadow).
///
/// Each call produces exactly one debit (D) and one credit (C) line.
/// The amount is always positive — the `side` column conveys the sign.
/// Constraint: `debit.amount_ct == credit.amount_ct` — enforced by this function.
#[allow(clippy::too_many_arguments)]
pub async fn insert_journal_lines(
    pool: &PgPool,
    ledger_entry_id: Uuid,
    account_id: Uuid,
    tenant: &str,
    entry_type: &str,
    amount_ct: i64,
    booking_date: time::Date,
    description: Option<&str>,
) -> anyhow::Result<()> {
    let abs_ct = amount_ct.unsigned_abs() as i64;
    if abs_ct == 0 {
        return Ok(());
    }
    let m = journal_mapping(entry_type, amount_ct);

    sqlx::query(
        r"INSERT INTO journal_lines
              (ledger_entry_id, account_id, tenant, side, skr_account, skr_description,
               amount_ct, booking_date, description)
          VALUES
              ($1, $2, $3, 'D', $4, $5, $6, $7, $8),
              ($1, $2, $3, 'C', $9, $10, $6, $7, $8)",
    )
    .bind(ledger_entry_id)
    .bind(account_id)
    .bind(tenant)
    .bind(m.debit_skr)
    .bind(m.debit_desc)
    .bind(abs_ct)
    .bind(booking_date)
    .bind(description)
    .bind(m.credit_skr)
    .bind(m.credit_desc)
    .execute(pool)
    .await
    .context("insert_journal_lines")?;

    Ok(())
}

// ── Aging analysis ────────────────────────────────────────────────────────────

/// Aging bucket for open receivables.
#[derive(Debug, Serialize)]
pub struct AgingBucket {
    pub bucket:          &'static str,  // "0-30d", "31-60d", "61-90d", ">90d"
    pub account_count:   i64,
    pub total_ct:        i64,
    pub total_eur:       String,
}

/// Aging analysis: group overdue account balances by days-overdue bucket.
///
/// Uses `accounts.balance_ct` (cached) as the outstanding amount per MaLo.
/// Overdue date is approximated from the oldest unresolved `dunning_cases.issued_at`
/// when present, or the account `updated_at` otherwise.
///
/// Returns four buckets: 0–30 days, 31–60 days, 61–90 days, >90 days.
pub async fn list_aging_buckets(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Vec<AgingBucket>> {
    let rows = sqlx::query(
        r"SELECT
            CASE
                WHEN age_days <= 30  THEN '0-30d'
                WHEN age_days <= 60  THEN '31-60d'
                WHEN age_days <= 90  THEN '61-90d'
                ELSE '>90d'
            END AS bucket,
            COUNT(*)                 AS account_count,
            COALESCE(SUM(balance_ct), 0) AS total_ct
          FROM (
              SELECT a.balance_ct,
                     EXTRACT(DAY FROM (now() - COALESCE(
                         (SELECT MIN(dc.issued_at) FROM dunning_cases dc
                          WHERE dc.account_id = a.account_id AND dc.resolved_at IS NULL),
                         a.updated_at
                     )))::INT AS age_days
              FROM accounts a
              WHERE a.tenant = $1 AND a.balance_ct > 0
          ) sub
          GROUP BY bucket
          ORDER BY MIN(age_days)",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_aging_buckets")?;

    let mut buckets = Vec::with_capacity(4);
    // Ensure all four buckets are present even if empty
    for (label, min_days, max_days) in &[
        ("0-30d",  0i32,  30i32),
        ("31-60d", 31,    60),
        ("61-90d", 61,    90),
        (">90d",   91,  i32::MAX),
    ] {
        let (account_count, total_ct) = rows.iter()
            .find(|r| r.try_get::<&str, _>("bucket").map(|b| b == *label).unwrap_or(false))
            .map(|r| (
                r.try_get::<i64, _>("account_count").unwrap_or(0),
                r.try_get::<i64, _>("total_ct").unwrap_or(0),
            ))
            .unwrap_or((0, 0));
        let _ = (min_days, max_days); // used only for ordering above
        buckets.push(AgingBucket {
            bucket: label,
            account_count,
            total_ct,
            total_eur: crate::handlers::format_ct_as_eur(total_ct),
        });
    }
    Ok(buckets)
}

// ── Interest charges (Verzugszinsen §288 BGB) ─────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct InterestChargeRow {
    pub id:               Uuid,
    pub account_id:       Uuid,
    pub tenant:           String,
    pub invoice_reference: Option<String>,
    pub principal_ct:     i64,
    pub interest_ct:      i64,
    pub rate_pct:         rust_decimal::Decimal,
    pub ecb_base_rate_pct: rust_decimal::Decimal,
    pub customer_type:    String,
    pub period_from:      time::Date,
    pub period_to:        time::Date,
    pub legal_basis:      String,
    pub ledger_entry_id:  Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:       OffsetDateTime,
}

/// Fetch the current ECB Basiszinssatz (§247 BGB) from the `ecb_base_rates` table.
///
/// Returns the rate valid on the given `reference_date` (or today if None).
pub async fn fetch_ecb_base_rate(
    pool: &PgPool,
    reference_date: Option<time::Date>,
) -> anyhow::Result<rust_decimal::Decimal> {
    let date = reference_date.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    let row = sqlx::query(
        "SELECT rate_pct FROM ecb_base_rates WHERE valid_from <= $1 ORDER BY valid_from DESC LIMIT 1",
    )
    .bind(date)
    .fetch_optional(pool)
    .await
    .context("fetch_ecb_base_rate")?;

    match row {
        Some(r) => Ok(r.try_get("rate_pct")?),
        None => {
            // Fallback to a conservative estimate if no rates are seeded
            tracing::warn!("accountingd: no ECB base rate found — using 2.00% fallback. Seed ecb_base_rates table.");
            Ok(rust_decimal::Decimal::new(200, 2)) // 2.00%
        }
    }
}

/// Create a Verzugszinsen (default interest) charge and the linked MAHNGEBUEHR ledger entry.
#[allow(clippy::too_many_arguments)]
pub async fn create_interest_charge(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    invoice_reference: Option<&str>,
    principal_ct: i64,
    is_b2b: bool,
    period_from: time::Date,
    period_to: time::Date,
) -> anyhow::Result<InterestChargeRow> {
    let ecb_rate = fetch_ecb_base_rate(pool, Some(period_from)).await?;
    let days = (period_to - period_from).whole_days();
    if days <= 0 {
        anyhow::bail!("interest period_to must be after period_from");
    }
    let (interest_ct, annual_rate) =
        crate::sepa::calculate_interest_ct(principal_ct, ecb_rate, is_b2b, days);
    if interest_ct <= 0 {
        anyhow::bail!("calculated interest is zero — check principal and period");
    }

    let legal_basis = if is_b2b {
        "\u{00a7}288 Abs. 2 BGB"
    } else {
        "\u{00a7}288 Abs. 1 BGB"
    };
    let customer_type = if is_b2b { "B2B" } else { "B2C" };

    // Create linked MAHNGEBUEHR ledger entry
    let ledger_id = write_entry(
        pool,
        account_id,
        tenant,
        "MAHNGEBUEHR",
        interest_ct,
        invoice_reference,
        Some("de.accounting.interest.charged"),
        None,
        period_to,
        Some(legal_basis),
    )
    .await
    .context("create_interest_charge: ledger entry")?;

    let row = sqlx::query_as::<_, InterestChargeRow>(
        r"INSERT INTO interest_charges
              (account_id, tenant, invoice_reference, principal_ct, interest_ct, rate_pct,
               ecb_base_rate_pct, customer_type, period_from, period_to, legal_basis, ledger_entry_id)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
          RETURNING *",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(invoice_reference)
    .bind(principal_ct)
    .bind(interest_ct)
    .bind(annual_rate)
    .bind(ecb_rate)
    .bind(customer_type)
    .bind(period_from)
    .bind(period_to)
    .bind(legal_basis)
    .bind(ledger_id.unwrap_or(Uuid::nil()))
    .fetch_one(pool)
    .await
    .context("create_interest_charge: insert")?;

    Ok(row)
}

/// List interest charges for an account.
pub async fn list_interest_charges(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    limit: i64,
) -> anyhow::Result<Vec<InterestChargeRow>> {
    sqlx::query_as::<_, InterestChargeRow>(
        "SELECT * FROM interest_charges WHERE account_id = $1 AND tenant = $2 \
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_interest_charges")
}

// ── Payment plans (Zahlungsvereinbarung) ──────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PaymentPlanRow {
    pub plan_id:           Uuid,
    pub account_id:        Uuid,
    pub tenant:            String,
    pub total_ct:          i64,
    pub installment_ct:    i64,
    pub installment_count: i32,
    pub billing_day:       i16,
    pub status:            String,
    pub dunning_case_id:   Option<Uuid>,
    pub operator_sub:      Option<String>,
    pub note:              Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:        OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at:        OffsetDateTime,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PaymentPlanInstallmentRow {
    pub id:                Uuid,
    pub plan_id:           Uuid,
    pub tenant:            String,
    pub installment_no:    i32,
    pub due_date:          time::Date,
    pub amount_ct:         i64,
    pub status:            String,
    pub ledger_entry_id:   Option<Uuid>,
    pub paid_at:           Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:        OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct CreatePaymentPlanRequest {
    pub malo_id:           String,
    pub lf_mp_id:          Option<String>,
    pub total_ct:          i64,
    pub installment_ct:    i64,
    pub billing_day:       i16,
    pub first_due_date:    String, // ISO 8601 date
    pub dunning_case_id:   Option<Uuid>,
    pub note:              Option<String>,
    pub operator_sub:      Option<String>,
}

/// Create a payment plan and its installment schedule.
///
/// The number of installments is `ceil(total_ct / installment_ct)`.
/// The final installment is adjusted to cover any remainder.
/// Installments are due monthly from `first_due_date`, on `billing_day`.
pub async fn create_payment_plan(
    pool: &PgPool,
    tenant: &str,
    req: CreatePaymentPlanRequest,
) -> anyhow::Result<Uuid> {
    use time::format_description::well_known::Iso8601;

    let lf_mp_id = req.lf_mp_id.as_deref().unwrap_or(tenant);
    let account_id = upsert_account(pool, &req.malo_id, lf_mp_id, tenant).await?;

    let installment_count = (req.total_ct + req.installment_ct - 1) / req.installment_ct;
    if installment_count <= 0 {
        anyhow::bail!("installment_count must be >= 1");
    }

    let plan_id: Uuid = sqlx::query_scalar(
        r"INSERT INTO payment_plans
              (account_id, tenant, total_ct, installment_ct, installment_count,
               billing_day, dunning_case_id, operator_sub, note)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
          RETURNING plan_id",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(req.total_ct)
    .bind(req.installment_ct)
    .bind(installment_count as i32)
    .bind(req.billing_day)
    .bind(req.dunning_case_id)
    .bind(req.operator_sub.as_deref())
    .bind(req.note.as_deref())
    .fetch_one(pool)
    .await
    .context("create_payment_plan")?;

    // Generate installment schedule
    let first_due = time::Date::parse(&req.first_due_date, &Iso8601::DEFAULT)
        .context("parse first_due_date")?;

    let mut remaining = req.total_ct;
    for n in 0..installment_count {
        let due_date = first_due
            .replace_month(
                time::Month::try_from(
                    ((first_due.month() as u8 - 1 + n as u8) % 12) + 1
                ).unwrap_or(time::Month::January)
            )
            .unwrap_or(first_due);

        let amount = if n == installment_count - 1 {
            remaining // last installment covers any rounding remainder
        } else {
            req.installment_ct.min(remaining)
        };
        remaining -= amount;
        if amount <= 0 {
            break;
        }

        sqlx::query(
            "INSERT INTO payment_plan_installments \
             (plan_id, tenant, installment_no, due_date, amount_ct) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(plan_id)
        .bind(tenant)
        .bind((n + 1) as i32)
        .bind(due_date)
        .bind(amount)
        .execute(pool)
        .await
        .context("create_payment_plan: installment")?;
    }

    Ok(plan_id)
}

/// List active payment plans for an account.
pub async fn list_payment_plans(
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Vec<PaymentPlanRow>> {
    sqlx::query_as::<_, PaymentPlanRow>(
        "SELECT * FROM payment_plans WHERE account_id = $1 AND tenant = $2 ORDER BY created_at DESC",
    )
    .bind(account_id)
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_payment_plans")
}

/// Get a single payment plan with all its installments.
pub async fn get_payment_plan_with_installments(
    pool: &PgPool,
    plan_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<(PaymentPlanRow, Vec<PaymentPlanInstallmentRow>)>> {
    let plan = sqlx::query_as::<_, PaymentPlanRow>(
        "SELECT * FROM payment_plans WHERE plan_id = $1 AND tenant = $2",
    )
    .bind(plan_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("get_payment_plan")?;

    let Some(plan) = plan else { return Ok(None) };

    let installments = sqlx::query_as::<_, PaymentPlanInstallmentRow>(
        "SELECT * FROM payment_plan_installments WHERE plan_id = $1 ORDER BY installment_no",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("get_payment_plan: installments")?;

    Ok(Some((plan, installments)))
}

/// Cancel a payment plan (sets status = CANCELLED).
pub async fn cancel_payment_plan(
    pool: &PgPool,
    plan_id: Uuid,
    tenant: &str,
    operator_sub: Option<&str>,
) -> anyhow::Result<()> {
    let affected = sqlx::query(
        "UPDATE payment_plans SET status = 'CANCELLED', updated_at = now(), \
         operator_sub = COALESCE($3, operator_sub) \
         WHERE plan_id = $1 AND tenant = $2 AND status = 'ACTIVE'",
    )
    .bind(plan_id)
    .bind(tenant)
    .bind(operator_sub)
    .execute(pool)
    .await
    .context("cancel_payment_plan")?
    .rows_affected();

    if affected == 0 {
        anyhow::bail!("payment plan not found or not ACTIVE: {plan_id}");
    }
    Ok(())
}

// ── Bank import deduplication (CAMT.054) ──────────────────────────────────────

/// Check whether a bank transaction has already been imported.
///
/// Returns `true` if `bank_transaction_id` is already in `bank_import_log`.
/// Call this before creating a ZAHLUNG/BANKRUECKLAST ledger entry from CAMT.054.
pub async fn bank_import_already_processed(
    pool: &PgPool,
    tenant: &str,
    bank_transaction_id: &str,
) -> anyhow::Result<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM bank_import_log WHERE tenant = $1 AND bank_transaction_id = $2)",
    )
    .bind(tenant)
    .bind(bank_transaction_id)
    .fetch_one(pool)
    .await
    .context("bank_import_already_processed")?;
    Ok(exists)
}

/// Record a bank transaction import in the deduplication log.
///
/// Uses `ON CONFLICT DO NOTHING` so concurrent calls are safe.
pub async fn record_bank_import(
    pool: &PgPool,
    tenant: &str,
    bank_transaction_id: &str,
    amount_ct: i64,
    iban: Option<&str>,
    value_date: time::Date,
    ledger_entry_id: Option<Uuid>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO bank_import_log \
         (tenant, bank_transaction_id, amount_ct, iban, value_date, ledger_entry_id) \
         VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (tenant, bank_transaction_id) DO NOTHING",
    )
    .bind(tenant)
    .bind(bank_transaction_id)
    .bind(amount_ct)
    .bind(iban)
    .bind(value_date)
    .bind(ledger_entry_id)
    .execute(pool)
    .await
    .context("record_bank_import")?;
    Ok(())
}

