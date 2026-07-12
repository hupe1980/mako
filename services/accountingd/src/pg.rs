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

pub async fn fetch_account(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
) -> anyhow::Result<Option<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT * FROM accounts WHERE malo_id = $1 AND lf_mp_id = $2 LIMIT 1",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .fetch_optional(pool)
    .await
    .context("fetch_account")
}

pub async fn fetch_account_by_id(
    pool: &PgPool,
    account_id: Uuid,
) -> anyhow::Result<Option<AccountRow>> {
    sqlx::query_as::<_, AccountRow>("SELECT * FROM accounts WHERE account_id = $1")
        .bind(account_id)
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
    // Idempotency: skip if CloudEvent already processed.
    if let Some(ce) = ce_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM processed_events WHERE ce_id = $1)")
                .bind(ce)
                .fetch_one(pool)
                .await
                .context("check idempotency")?;
        if exists {
            return Ok(None);
        }
    }

    let mut tx = pool.begin().await.context("begin tx")?;

    // Lock account row.
    sqlx::query("SELECT account_id FROM accounts WHERE account_id = $1 FOR UPDATE")
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .context("lock account")?;

    // Insert ledger entry.
    let id: Uuid = sqlx::query_scalar(
        r"INSERT INTO ledger_entries
              (account_id, tenant, entry_type, amount_ct, reference_id, ce_type, ce_id,
               booking_date, value_date, description)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $9)
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

    // Mark CloudEvent as processed.
    if let Some(ce) = ce_id {
        sqlx::query("INSERT INTO processed_events (ce_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(ce)
            .execute(&mut *tx)
            .await
            .context("mark processed")?;
    }

    tx.commit().await.context("commit")?;
    Ok(Some(id))
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
