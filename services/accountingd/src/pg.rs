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
        r"INSERT INTO accounts (malo_id, lf_mp_id, tenant)
          VALUES ($1, $2, $3)
          ON CONFLICT (malo_id, lf_mp_id) DO UPDATE SET updated_at = now()
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
        r"INSERT INTO sepa_mandates
              (account_id, tenant, iban, bic, kontoinhaber, mandatsref, sequence_type, signed_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
          ON CONFLICT (mandatsref) DO UPDATE
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
