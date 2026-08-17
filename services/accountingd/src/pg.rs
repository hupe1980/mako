//! PostgreSQL persistence for `accountingd`.

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::ledger::{PgLedger, PostEntry};

/// Post a money movement through the doubleentry ledger, then refresh the
/// `accounts.balance_ct` read cache to the authoritative ledger net.
///
/// This is the single money choke point (replacing the old `write_entry` +
/// `journal_lines` machinery). doubleentry is the authoritative, tamper-evident
/// system of record; `balance_ct` is a cache set **absolutely** from the ledger
/// net (never incremented, so it cannot drift by arithmetic).
///
/// Idempotent by `idempotency` — a CloudEvent id, a bank transaction id, or a
/// deterministic key (`ABSCHLAG-{malo}-{YYYY}-{MM}`, `mahngebuehr:{malo}:{stufe}:{date}`).
/// A replay is a no-op returning the original entry id. Returns the doubleentry
/// `EntryId` as a `Uuid` (for linking satellites like interest_charges).
#[allow(clippy::too_many_arguments)]
pub async fn post_entry(
    ledger: &PgLedger,
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    lf_mp_id: &str,
    entry_type: &str,
    amount_ct: i64,
    idempotency: &str,
    correlation: Option<&str>,
    reference: Option<&str>,
    booking_date: Date,
    value_date: Date,
    description: Option<&str>,
    actor: Option<&str>,
) -> anyhow::Result<Uuid> {
    let mut req = PostEntry::new(
        malo_id,
        lf_mp_id,
        entry_type,
        amount_ct,
        idempotency,
        booking_date,
    )
    .with_value_date(value_date);
    if let Some(desc) = description {
        req = req.with_description(desc);
    }
    if let Some(corr) = correlation {
        req = req.with_correlation(corr);
    }
    if let Some(reference) = reference {
        req = req.with_document(reference);
    }
    if let Some(actor) = actor {
        req = req.with_actor(actor);
    }
    let posted = ledger.post(req).await?;

    // Refresh the read cache to the authoritative ledger net. Absolute, not
    // incremental — idempotent under replay and immune to arithmetic drift.
    let net = ledger.balance_ct(lf_mp_id, malo_id).await?;
    let updated = sqlx::query(
        "UPDATE accounts SET balance_ct = $1, updated_at = now() \
         WHERE malo_id = $2 AND lf_mp_id = $3 AND tenant = $4",
    )
    .bind(net)
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("refresh balance cache")?;
    if updated.rows_affected() == 0 {
        tracing::warn!(
            malo = %malo_id,
            "post_entry: no accounts row for balance-cache refresh (ledger remains authoritative)"
        );
    }

    // Keep the Offene-Posten assignments current: match open credits against open
    // debits FIFO (§ 252 HGB per-receivable tracking). Best-effort — a clearing
    // failure must never fail the money post; the balance stays authoritative.
    if let Err(e) = ledger
        .apply_fifo_clearing(lf_mp_id, malo_id, booking_date)
        .await
    {
        tracing::warn!(malo = %malo_id, error = %e, "post_entry: FIFO clearing skipped");
    }

    Ok(*posted.id.as_uuid())
}

// ── Account ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AccountRow {
    pub account_id: Uuid,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub tenant: String,
    pub kunden_nr: Option<String>,
    pub iban: Option<String>,
    pub mandatsref: Option<String>,
    pub abschlag_ct: i64,
    pub billing_day: i16,
    pub balance_ct: i64,
    /// `PstlAdr` parts — see [`AccountRow::postal_address`].
    pub addr_town: Option<String>,
    pub addr_country: Option<String>,
    pub addr_street: Option<String>,
    pub addr_building_number: Option<String>,
    pub addr_post_code: Option<String>,
    pub addr_country_subdivision: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl AccountRow {
    /// This counterparty's postal address.
    ///
    /// `Cdtr/PstlAdr` when accountingd pays the account (EEG Vergütung, a
    /// Jahresabschluss-Erstattung), and the fallback for `Dbtr/PstlAdr` when a
    /// mandate carries none. Mandatory from the EPC structured-address cut-over
    /// on 15 November 2026; until then an empty set of parts emits no `PstlAdr`.
    #[must_use]
    pub fn postal_address(&self) -> crate::sepa::AddressParts {
        crate::sepa::AddressParts {
            town: self.addr_town.clone(),
            country: self.addr_country.clone(),
            street: self.addr_street.clone(),
            building_number: self.addr_building_number.clone(),
            post_code: self.addr_post_code.clone(),
            country_subdivision: self.addr_country_subdivision.clone(),
        }
    }
}

pub async fn upsert_account(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        // ON CONFLICT must match the UNIQUE (malo_id, lf_mp_id, tenant) constraint exactly.
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

/// Record the account's BO4E Sparte, learned from a billing CloudEvent.
///
/// Written only when it is not already known or when it changed: a
/// Sparte-switch is real (a customer taking gas as well), but rewriting the
/// column on every invoice would churn `updated_at` for nothing.
pub async fn set_account_sparte(
    executor: impl sqlx::PgExecutor<'_>,
    account_id: Uuid,
    tenant: &str,
    sparte: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE accounts SET sparte = $3, updated_at = now() \
         WHERE account_id = $1 AND tenant = $2 AND sparte IS DISTINCT FROM $3",
    )
    .bind(account_id)
    .bind(tenant)
    .bind(sparte)
    .execute(executor)
    .await
    .context("set_account_sparte")?;
    Ok(())
}

/// Link an account to its business partner (vertragd `kunden_nr`).
pub async fn set_account_kunden_nr(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
    kunden_nr: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        "UPDATE accounts SET kunden_nr = $1, updated_at = now() \
         WHERE malo_id = $2 AND lf_mp_id = $3 AND tenant = $4",
    )
    .bind(kunden_nr)
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("set_account_kunden_nr")?;
    Ok(r.rows_affected())
}

/// All accounts belonging to one business partner (cross-MaLo).
pub async fn list_accounts_by_bp(
    pool: &PgPool,
    tenant: &str,
    kunden_nr: &str,
) -> anyhow::Result<Vec<AccountRow>> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT * FROM accounts WHERE tenant = $1 AND kunden_nr = $2 ORDER BY malo_id",
    )
    .bind(tenant)
    .bind(kunden_nr)
    .fetch_all(pool)
    .await
    .context("list_accounts_by_bp")
}

/// Consolidated balance across all of a business partner's accounts (ct).
pub async fn bp_consolidated_balance(
    pool: &PgPool,
    tenant: &str,
    kunden_nr: &str,
) -> anyhow::Result<i64> {
    let total: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(balance_ct)::bigint FROM accounts WHERE tenant = $1 AND kunden_nr = $2",
    )
    .bind(tenant)
    .bind(kunden_nr)
    .fetch_one(pool)
    .await
    .context("bp_consolidated_balance")?;
    Ok(total.unwrap_or(0))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAccountRequest {
    pub iban: Option<String>,
    pub mandatsref: Option<String>,
    pub abschlag_ct: Option<i64>,
    pub billing_day: Option<i16>,
    /// Postal address (`PstlAdr`). Each part is `COALESCE`d, so an omitted part
    /// leaves the stored value alone — the same shape as every other field here.
    #[serde(default)]
    pub address: crate::sepa::AddressParts,
}

/// The lookup hash written alongside a new `accounts.iban`. `None` when the
/// request carries no IBAN, so the `COALESCE` leaves the stored hash alone.
fn account_iban_hash(iban: Option<&str>, iban_key: Option<&[u8; 32]>) -> Option<String> {
    iban.map(|iban| crate::ledger::iban_hash(iban_key, iban))
}

pub async fn update_account(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    iban_key: Option<&[u8; 32]>,
    req: UpdateAccountRequest,
) -> anyhow::Result<()> {
    let iban_hash = account_iban_hash(req.iban.as_deref(), iban_key);
    sqlx::query(
        // add tenant parameter — previously missing, allowing cross-tenant modification.
        r"UPDATE accounts SET
              iban        = COALESCE($3, iban),
              iban_hash   = COALESCE($7, iban_hash),
              mandatsref  = COALESCE($4, mandatsref),
              abschlag_ct = COALESCE($5, abschlag_ct),
              billing_day = COALESCE($6, billing_day),
              addr_town            = COALESCE($8,  addr_town),
              addr_country         = COALESCE($9,  addr_country),
              addr_street          = COALESCE($10, addr_street),
              addr_building_number = COALESCE($11, addr_building_number),
              addr_post_code       = COALESCE($12, addr_post_code),
              addr_country_subdivision = COALESCE($13, addr_country_subdivision),
              updated_at  = now()
          WHERE malo_id = $1 AND lf_mp_id = $2",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(req.iban)
    .bind(req.mandatsref)
    .bind(req.abschlag_ct)
    .bind(req.billing_day)
    .bind(iban_hash)
    .bind(req.address.town)
    .bind(req.address.country)
    .bind(req.address.street)
    .bind(req.address.building_number)
    .bind(req.address.post_code)
    .bind(req.address.country_subdivision)
    .execute(pool)
    .await
    .context("update_account")?;
    Ok(())
}

/// Tenant-scoped variant of `update_account` —
/// Always filter by tenant to prevent cross-tenant data modification.
pub async fn update_account_tenanted(
    executor: impl sqlx::PgExecutor<'_>,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
    iban_key: Option<&[u8; 32]>,
    req: UpdateAccountRequest,
) -> anyhow::Result<()> {
    let iban_hash = account_iban_hash(req.iban.as_deref(), iban_key);
    let rows_affected = sqlx::query(
        r"UPDATE accounts SET
              iban        = COALESCE($4, iban),
              iban_hash   = COALESCE($8, iban_hash),
              mandatsref  = COALESCE($5, mandatsref),
              abschlag_ct = COALESCE($6, abschlag_ct),
              billing_day = COALESCE($7, billing_day),
              addr_town            = COALESCE($9,  addr_town),
              addr_country         = COALESCE($10, addr_country),
              addr_street          = COALESCE($11, addr_street),
              addr_building_number = COALESCE($12, addr_building_number),
              addr_post_code       = COALESCE($13, addr_post_code),
              addr_country_subdivision = COALESCE($14, addr_country_subdivision),
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
    .bind(iban_hash)
    .bind(req.address.town)
    .bind(req.address.country)
    .bind(req.address.street)
    .bind(req.address.building_number)
    .bind(req.address.post_code)
    .bind(req.address.country_subdivision)
    .execute(executor)
    .await
    .context("update_account_tenanted")?
    .rows_affected();

    if rows_affected == 0 {
        anyhow::bail!("account not found: malo_id={malo_id} lf_mp_id={lf_mp_id} tenant={tenant}");
    }
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
        "SELECT zahlbetrag_ct FROM jahresabschluss_runs \
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
    executor: impl sqlx::PgExecutor<'_>,
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
    .execute(executor)
    .await
    .context("record_jahresabschluss")?;
    Ok(())
}

/// Persist a SEPA pain.008 batch — the run row **and** what it collected.
///
/// Inserts into `sepa_collection_runs` and one `sepa_collection_entries` row per
/// mandate, in one transaction. If a run already exists for the same
/// `(tenant, collection_date)` it is updated and its entries replaced, so a
/// regenerated batch cannot leave a stale entry claiming to have been
/// collected. Returns the run's ID.
///
/// The entry rows are what later attributes a bank reply: a pain.002 rejection
/// names an `EndToEndId`, a camt booking names a `PmtInfId` in its `Btch` block,
/// and a pain.007 reversal must restate the original amount and mandate exactly
/// as submitted.
pub async fn persist_sepa_collection(
    pool: &PgPool,
    tenant: &str,
    collection_date: time::Date,
    run: &crate::sepa::Pain008Run,
) -> anyhow::Result<Uuid> {
    let mut tx = pool
        .begin()
        .await
        .context("persist_sepa_collection: begin")?;
    let row = sqlx::query(
        r"INSERT INTO sepa_collection_runs
              (tenant, collection_date, msg_id, pain008_xml, total_ct, mandate_count)
          VALUES ($1, $2, $3, $4, $5, $6)
          ON CONFLICT (tenant, collection_date) DO UPDATE
          SET msg_id         = EXCLUDED.msg_id,
              pain008_xml    = EXCLUDED.pain008_xml,
              total_ct       = EXCLUDED.total_ct,
              mandate_count  = EXCLUDED.mandate_count
          RETURNING run_id",
    )
    .bind(tenant)
    .bind(collection_date)
    .bind(&run.msg_id)
    .bind(&run.xml)
    .bind(run.total_ct)
    .bind(i32::try_from(run.entry_count).unwrap_or(i32::MAX))
    .fetch_one(&mut *tx)
    .await
    .context("persist_sepa_collection")?;
    let run_id: Uuid = row.try_get("run_id")?;

    sqlx::query("DELETE FROM sepa_collection_entries WHERE run_id = $1")
        .bind(run_id)
        .execute(&mut *tx)
        .await
        .context("persist_sepa_collection: clear entries")?;

    for entry in &run.entries {
        let inserted = sqlx::query(
            r"INSERT INTO sepa_collection_entries
                  (run_id, tenant, mandate_id, account_id, mandatsref, end_to_end_id,
                   payment_info_id, sequence_type, amount_ct)
              SELECT $1, $2, sm.mandate_id, sm.account_id, $4, $5, $6, $7, $8
              FROM sepa_mandates sm
              WHERE sm.mandate_id = $3",
        )
        .bind(run_id)
        .bind(tenant)
        .bind(entry.mandate_id)
        .bind(&entry.mandatsref)
        .bind(&entry.mandatsref) // EndToEndId == Mandatsreferenz
        .bind(&entry.payment_info_id)
        .bind(&entry.sequence_type)
        .bind(entry.amount_ct)
        .execute(&mut *tx)
        .await
        .context("persist_sepa_collection: entry")?
        .rows_affected();
        // The mandate was read moments ago to build the file, so this only
        // happens if it was deleted in between. Say so rather than leaving a
        // collection in the XML with no row to attribute a bank reply to.
        if inserted == 0 {
            tracing::warn!(
                mandate_id = %entry.mandate_id,
                mandatsref = %entry.mandatsref,
                "accountingd: collected mandate vanished between build and persist — \
                 its bank replies will be unattributable"
            );
        }
    }

    tx.commit()
        .await
        .context("persist_sepa_collection: commit")?;
    Ok(run_id)
}

/// One collected mandate, joined with everything a pain.007 reversal restates.
///
/// The IBAN, account holder and signature date come from `sepa_mandates` rather
/// than being duplicated on the entry row, so GDPR Art. 17 erasure keeps working
/// from one place — and a reversal for an erased mandate is correctly
/// impossible rather than built from a stale copy.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CollectionEntryRow {
    pub entry_id: Uuid,
    pub run_id: Uuid,
    pub tenant: String,
    pub mandate_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub mandatsref: String,
    pub end_to_end_id: String,
    pub payment_info_id: String,
    pub sequence_type: String,
    pub amount_ct: i64,
    pub status: String,
    pub status_reason: Option<String>,
    /// From `sepa_collection_runs`.
    pub msg_id: String,
    pub collection_date: Date,
    /// From `sepa_mandates` — `None` once the mandate row is gone.
    pub debtor_iban: Option<String>,
    pub debtor_bic: Option<String>,
    pub debtor_name: Option<String>,
    pub mandate_signed_at: Option<Date>,
    pub malo_id: Option<String>,
    pub lf_mp_id: Option<String>,
}

const COLLECTION_ENTRY_SELECT: &str = r"
    SELECT ce.entry_id, ce.run_id, ce.tenant, ce.mandate_id, ce.account_id,
           ce.mandatsref, ce.end_to_end_id, ce.payment_info_id, ce.sequence_type,
           ce.amount_ct, ce.status, ce.status_reason,
           r.msg_id, r.collection_date,
           sm.iban          AS debtor_iban,
           sm.bic           AS debtor_bic,
           sm.kontoinhaber  AS debtor_name,
           sm.signed_at     AS mandate_signed_at,
           a.malo_id, a.lf_mp_id
    FROM sepa_collection_entries ce
    JOIN sepa_collection_runs r ON r.run_id = ce.run_id
    LEFT JOIN sepa_mandates sm  ON sm.mandate_id = ce.mandate_id
    LEFT JOIN accounts a        ON a.account_id = ce.account_id
";

/// Fetch one collected entry by its id.
pub async fn fetch_collection_entry(
    pool: &PgPool,
    entry_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<CollectionEntryRow>> {
    sqlx::query_as::<_, CollectionEntryRow>(&format!(
        "{COLLECTION_ENTRY_SELECT} WHERE ce.entry_id = $1 AND ce.tenant = $2"
    ))
    .bind(entry_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_collection_entry")
}

/// Find the collected entry a bank reply refers to by its `EndToEndId`.
///
/// Newest first: the same Mandatsreferenz is reused every month, so a reply has
/// to attach to the most recent collection that carried it.
pub async fn find_collection_entry_by_e2e(
    pool: &PgPool,
    tenant: &str,
    end_to_end_id: &str,
) -> anyhow::Result<Option<CollectionEntryRow>> {
    sqlx::query_as::<_, CollectionEntryRow>(&format!(
        "{COLLECTION_ENTRY_SELECT} WHERE ce.tenant = $1 AND ce.end_to_end_id = $2 \
         ORDER BY r.collection_date DESC LIMIT 1"
    ))
    .bind(tenant)
    .bind(end_to_end_id)
    .fetch_optional(pool)
    .await
    .context("find_collection_entry_by_e2e")
}

// ── Resolving an incoming payment to a customer account ──────────────────────

/// The account an incoming bank transaction belongs to, and how it was found.
#[derive(Debug, Clone, Serialize)]
pub struct AccountMatch {
    pub account_id: Uuid,
    pub malo_id: String,
    pub lf_mp_id: String,
    /// Which rung of the ladder matched — carried onto the CloudEvent so a
    /// reconciliation agent can tell a bank-asserted match from an inferred one.
    pub matched_by: &'static str,
}

/// Everything a bank transaction offers for identifying its payer.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaymentClues<'a> {
    /// The counterparty IBAN the bank reported, already hashed.
    pub iban_hash: Option<&'a str>,
    /// `EndToEndId` — for a collection accountingd sent, this is the
    /// Mandatsreferenz coming straight back.
    pub end_to_end_id: Option<&'a str>,
    /// Verwendungszweck / `AddtlTxInf` — free text a human typed.
    pub remittance: Option<&'a str>,
}

/// Resolve an incoming payment to the account that owes the money.
///
/// Matching on the counterparty IBAN alone is the single biggest reconciliation
/// gap in a retail ledger: a customer paying from a spouse's account, an
/// employer's, or a second account they never told anyone about produces a
/// transaction with an IBAN nobody has on file. It books nowhere, and the
/// receivable stays open against a customer who has already paid.
///
/// The ladder runs strongest-first, and every rung below the first is an
/// *inference* — `matched_by` records which one, so a reconciliation agent can
/// treat them differently:
///
/// | Rung | Evidence | Why it is trusted this much |
/// |---|---|---|
/// | `iban` | the bank says whose account it is | the payment instrument itself |
/// | `end_to_end_id` | a reference accountingd generated and the bank echoed | machine-to-machine, no human typing |
/// | `remittance_token` | an exact Mandatsreferenz or MaLo-ID inside the free text | a human copied it correctly |
///
/// The free-text rung is deliberately **exact-token**, never substring: the
/// remittance is split on non-alphanumeric boundaries and the whole tokens are
/// looked up against the unique indexes. A `LIKE '%…%'` scan would match a
/// Mandatsreferenz that merely happens to be a prefix of another, and would
/// book a stranger's payment onto a customer's account.
pub async fn resolve_account_for_payment(
    pool: &PgPool,
    tenant: &str,
    clues: PaymentClues<'_>,
) -> anyhow::Result<Option<AccountMatch>> {
    // ── 1. The counterparty IBAN, as the bank reported it ────────────────────
    if let Some(hash) = clues.iban_hash {
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT account_id, malo_id, lf_mp_id FROM accounts \
             WHERE iban_hash = $1 AND tenant = $2 LIMIT 1",
        )
        .bind(hash)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("resolve_account_for_payment: iban")?;
        if let Some((account_id, malo_id, lf_mp_id)) = row {
            return Ok(Some(AccountMatch {
                account_id,
                malo_id,
                lf_mp_id,
                matched_by: "iban",
            }));
        }
    }

    // ── 2. The EndToEndId accountingd itself generated ───────────────────────
    //
    // A returned collection carries the Mandatsreferenz back verbatim, so this
    // is exact — and it is the rung that catches a Rückläufer debited from an
    // account whose IBAN has since changed.
    if let Some(e2e) = clues.end_to_end_id.filter(|s| !s.trim().is_empty()) {
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            r"SELECT a.account_id, a.malo_id, a.lf_mp_id
              FROM accounts a
              WHERE a.tenant = $1
                AND (a.account_id IN (SELECT account_id FROM sepa_collection_entries
                                      WHERE tenant = $1 AND end_to_end_id = $2)
                  OR a.account_id IN (SELECT account_id FROM sepa_mandates
                                      WHERE tenant = $1 AND mandatsref = $2))
              LIMIT 1",
        )
        .bind(tenant)
        .bind(e2e.trim())
        .fetch_optional(pool)
        .await
        .context("resolve_account_for_payment: end_to_end_id")?;
        if let Some((account_id, malo_id, lf_mp_id)) = row {
            return Ok(Some(AccountMatch {
                account_id,
                malo_id,
                lf_mp_id,
                matched_by: "end_to_end_id",
            }));
        }
    }

    // ── 3. An exact identifier a human typed into the Verwendungszweck ───────
    let Some(text) = clues.remittance.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let tokens = remittance_tokens(text);
    if tokens.is_empty() {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        r"SELECT DISTINCT a.account_id, a.malo_id, a.lf_mp_id
          FROM accounts a
          WHERE a.tenant = $1
            AND (a.malo_id = ANY($2)
              OR a.account_id IN (SELECT account_id FROM sepa_mandates
                                  WHERE tenant = $1 AND mandatsref_norm = ANY($2)))
          LIMIT 2",
    )
    .bind(tenant)
    .bind(&tokens)
    .fetch_all(pool)
    .await
    .context("resolve_account_for_payment: remittance")?;

    // Two accounts matching means the text named two customers — a batch
    // reference, or a token that is an identifier for one and noise for
    // another. Booking either would be a guess.
    match row.len() {
        1 => {
            let (account_id, malo_id, lf_mp_id) = row.into_iter().next().expect("len == 1");
            Ok(Some(AccountMatch {
                account_id,
                malo_id,
                lf_mp_id,
                matched_by: "remittance_token",
            }))
        }
        0 => Ok(None),
        _ => {
            tracing::warn!(
                "accountingd: remittance text names more than one account — not guessing"
            );
            Ok(None)
        }
    }
}

/// The number of adjacent words a single identifier may have been broken into.
///
/// `MND-000123` splits into two, `RF18 5390 0754 7034` into four. Beyond that a
/// run is a sentence, not an identifier, and every extra length multiplies the
/// candidate list for nothing.
const MAX_TOKEN_RUN: usize = 4;

/// The minimum length a candidate must have to be worth looking up.
///
/// Below four characters a token carries no identifying power and only widens
/// the lookup — `MND`, `Abs`, `07` match nothing anyone meant.
const MIN_TOKEN_LEN: usize = 4;

/// Split a Verwendungszweck into every whole identifier it could contain.
///
/// A customer keys an identifier back in however their bank's form lets them:
/// `MND-000123`, `MND 000123`, `mnd000123`. Splitting on non-alphanumerics alone
/// would produce `MND` and `000123` and match neither, so every **contiguous
/// run** of up to four words is also joined and offered as a
/// candidate. That is what makes `MND 000123` in the free text find the mandate
/// stored as `MND-000123`, whose `mandatsref_norm` is `MND000123`.
///
/// The candidates are matched by **equality** against the normalised columns,
/// never by `LIKE '%…%'`: a substring scan would match a Mandatsreferenz that
/// merely happens to be a prefix of another and book a stranger's payment onto
/// a customer's account.
#[must_use]
pub fn remittance_tokens(text: &str) -> Vec<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_uppercase)
        .collect();

    let mut out = Vec::new();
    for start in 0..words.len() {
        let mut joined = String::new();
        for word in words.iter().skip(start).take(MAX_TOKEN_RUN) {
            joined.push_str(word);
            if joined.len() >= MIN_TOKEN_LEN {
                out.push(joined.clone());
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// One collected mandate, flattened for a listing.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CollectionEntrySummary {
    pub entry_id: Uuid,
    pub malo_id: Option<String>,
    pub mandatsref: String,
    pub end_to_end_id: String,
    pub payment_info_id: String,
    pub sequence_type: String,
    pub amount_ct: i64,
    pub status: String,
    pub status_reason: Option<String>,
    pub collection_date: Date,
}

/// Collections across all runs, newest first, optionally filtered.
pub async fn list_collection_entries(
    pool: &PgPool,
    tenant: &str,
    status: Option<&str>,
    malo_id: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<CollectionEntrySummary>> {
    sqlx::query_as::<_, CollectionEntrySummary>(
        r"SELECT ce.entry_id, a.malo_id, ce.mandatsref, ce.end_to_end_id,
                 ce.payment_info_id, ce.sequence_type, ce.amount_ct,
                 ce.status, ce.status_reason, r.collection_date
          FROM sepa_collection_entries ce
          JOIN sepa_collection_runs r ON r.run_id = ce.run_id
          LEFT JOIN accounts a ON a.account_id = ce.account_id
          WHERE ce.tenant = $1
            AND ($2::text IS NULL OR ce.status = $2)
            AND ($3::text IS NULL OR a.malo_id = $3)
          ORDER BY r.collection_date DESC, ce.mandatsref
          LIMIT $4",
    )
    .bind(tenant)
    .bind(status)
    .bind(malo_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_collection_entries")
}

/// Record the outcome the bank reported for a collected entry.
///
/// Only advances a `SUBMITTED` entry: a settled collection that is later
/// returned is a separate R-transaction, and a second pain.002 for an entry that
/// already moved on must not rewrite its history.
pub async fn set_collection_entry_status(
    executor: impl sqlx::PgExecutor<'_>,
    entry_id: Uuid,
    status: &str,
    reason: Option<&str>,
) -> anyhow::Result<bool> {
    let r = sqlx::query(
        "UPDATE sepa_collection_entries
         SET status = $2, status_reason = $3, status_at = now()
         WHERE entry_id = $1 AND status = 'SUBMITTED'",
    )
    .bind(entry_id)
    .bind(status)
    .bind(reason)
    .execute(executor)
    .await
    .context("set_collection_entry_status")?;
    Ok(r.rows_affected() > 0)
}

/// Mark every still-open collection of one submitted message as rejected.
///
/// A pain.002 can bounce a whole file with no per-transaction detail — a schema
/// fault, a creditor identity the bank refuses, a collection date it will not
/// accept. Without this every one of those collections sits at `SUBMITTED`
/// forever, waiting for money that is never coming. `msg_id` is the
/// `GrpHdr/MsgId` accountingd sent, which the report quotes as `OrgnlMsgId`.
///
/// Returns how many entries moved.
pub async fn reject_submitted_entries_of_run(
    pool: &PgPool,
    tenant: &str,
    msg_id: &str,
    reason: &str,
) -> anyhow::Result<usize> {
    let r = sqlx::query(
        "UPDATE sepa_collection_entries ce
         SET status = 'REJECTED', status_reason = $3, status_at = now()
         FROM sepa_collection_runs r
         WHERE r.run_id = ce.run_id
           AND ce.tenant = $1 AND r.msg_id = $2
           AND ce.status = 'SUBMITTED'",
    )
    .bind(tenant)
    .bind(msg_id)
    .bind(reason)
    .execute(pool)
    .await
    .context("reject_submitted_entries_of_run")?;
    Ok(usize::try_from(r.rows_affected()).unwrap_or(usize::MAX))
}

/// The same, for one `PmtInf` group the bank rejected without itemising it.
pub async fn reject_submitted_entries_of_group(
    pool: &PgPool,
    tenant: &str,
    payment_info_id: &str,
    reason: &str,
) -> anyhow::Result<usize> {
    let r = sqlx::query(
        "UPDATE sepa_collection_entries
         SET status = 'REJECTED', status_reason = $3, status_at = now()
         WHERE tenant = $1 AND payment_info_id = $2 AND status = 'SUBMITTED'",
    )
    .bind(tenant)
    .bind(payment_info_id)
    .bind(reason)
    .execute(pool)
    .await
    .context("reject_submitted_entries_of_group")?;
    Ok(usize::try_from(r.rows_affected()).unwrap_or(usize::MAX))
}

/// Record a generated pain.007 reversal.
#[allow(clippy::too_many_arguments)]
pub async fn record_sepa_reversal(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    entry: &CollectionEntryRow,
    reversal: &crate::sepa::Pain007Reversal,
    reversed_amount_ct: i64,
    reason_code: &str,
    ledger_entry_id: Option<Uuid>,
    created_by: Option<&str>,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO sepa_reversals
              (tenant, collection_entry_id, msg_id, original_msg_id,
               original_payment_info_id, original_end_to_end_id,
               original_amount_ct, reversed_amount_ct, reason_code,
               pain007_xml, ledger_entry_id, created_by)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
          RETURNING reversal_id",
    )
    .bind(tenant)
    .bind(entry.entry_id)
    .bind(&reversal.msg_id)
    .bind(&entry.msg_id)
    .bind(&entry.payment_info_id)
    .bind(&entry.end_to_end_id)
    .bind(entry.amount_ct)
    .bind(reversed_amount_ct)
    .bind(reason_code)
    .bind(&reversal.xml)
    .bind(ledger_entry_id)
    .bind(created_by)
    .fetch_one(executor)
    .await
    .context("record_sepa_reversal")?;
    Ok(row.try_get("reversal_id")?)
}

/// Atomically claim a SEPA collection run for dispatch.
///
/// Returns `true` only for the caller that flips the run from a non-dispatched
/// state to `DISPATCHED`; a second replica or a same-day restart gets `false`
/// and must NOT re-POST the pain.008 (which would double-collect at the bank).
pub async fn mark_sepa_collection_dispatched(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: Uuid,
) -> anyhow::Result<bool> {
    let r = sqlx::query(
        "UPDATE sepa_collection_runs
         SET dispatch_status = 'DISPATCHED', dispatched_at = now()
         WHERE run_id = $1 AND dispatch_status != 'DISPATCHED'",
    )
    .bind(run_id)
    .execute(executor)
    .await
    .context("mark_sepa_collection_dispatched")?;
    Ok(r.rows_affected() > 0)
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

// The old `write_entry` / `write_entry_with_value_date` (which INSERTed into
// `ledger_entries`, incremented the `balance_ct` cache, and posted the
// `journal_lines` SKR shadow) are gone. Every money movement now flows through
// `post_entry` above → the doubleentry ledger, which is balanced, immutable,
// provable, and idempotent, with the balance cache set absolutely from the ledger.

/// One movement on a customer's Kontokorrent, for display — derived from the
/// doubleentry statement (the authoritative log), newest first.
#[derive(Debug, Serialize)]
pub struct LedgerLine {
    /// The doubleentry entry id.
    pub entry_id: Uuid,
    /// The Buchungsart (doubleentry `kind`), e.g. `RECHNUNG`, `ZAHLUNG`.
    pub entry_type: Option<String>,
    /// `"D"` (debit — increases the receivable) or `"C"` (credit).
    pub side: &'static str,
    /// The movement in minor units (always positive; `side` carries the sign).
    pub amount_ct: i64,
    /// Signed contribution to the balance (debit +, credit −).
    pub signed_ct: i64,
    /// The running balance (signed net) after this movement, in minor units.
    pub running_ct: i64,
    pub booking_date: Date,
}

/// The customer's recent Kontokorrent movements (newest first), from the ledger.
pub async fn list_ledger(
    ledger: &PgLedger,
    lf_mp_id: &str,
    malo_id: &str,
    limit: i64,
) -> anyhow::Result<Vec<LedgerLine>> {
    use doubleentry::Direction;
    use doubleentry::storage::PostingCursor;
    let cap = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);
    let mut window: std::collections::VecDeque<LedgerLine> = std::collections::VecDeque::new();
    let mut cursor = PostingCursor::start();
    loop {
        let page = ledger.statement(lf_mp_id, malo_id, cursor).await?;
        for line in &page.lines {
            let (side, signed): (&'static str, i64) = match line.direction {
                Direction::Debit => ("D", line.amount.to_minor()),
                Direction::Credit => ("C", -line.amount.to_minor()),
            };
            window.push_back(LedgerLine {
                entry_id: *line.posting.entry.as_uuid(),
                entry_type: line.kind.as_ref().map(|k| k.as_str().to_owned()),
                side,
                amount_ct: line.amount.to_minor(),
                signed_ct: signed,
                running_ct: line.running.signed_net().map(|a| a.to_minor()).unwrap_or(0),
                booking_date: line.booking_date,
            });
            if window.len() > cap {
                window.pop_front();
            }
        }
        match page.next {
            Some(next) => cursor = next,
            None => break,
        }
    }
    let mut lines: Vec<LedgerLine> = window.into_iter().collect();
    lines.reverse(); // newest first
    Ok(lines)
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
    /// The account's BO4E Sparte, joined in — drives the ISO 20022 `Purp/Cd`
    /// on the collection. `None` for an account that has never been billed.
    pub sparte: Option<String>,
    /// `Dbtr/PstlAdr` parts — see [`SepaMandateRow::debtor_address`].
    pub debtor_town: Option<String>,
    pub debtor_country: Option<String>,
    pub debtor_street: Option<String>,
    pub debtor_building_number: Option<String>,
    pub debtor_post_code: Option<String>,
    pub debtor_country_subdivision: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl SepaMandateRow {
    /// The debtor's postal address, as the pain.008 builder wants it.
    ///
    /// Mandatory from the EPC structured-address cut-over on 15 November 2026;
    /// until then an empty set of parts emits no `PstlAdr` at all.
    #[must_use]
    pub fn debtor_address(&self) -> crate::sepa::AddressParts {
        crate::sepa::AddressParts {
            town: self.debtor_town.clone(),
            country: self.debtor_country.clone(),
            street: self.debtor_street.clone(),
            building_number: self.debtor_building_number.clone(),
            post_code: self.debtor_post_code.clone(),
            country_subdivision: self.debtor_country_subdivision.clone(),
        }
    }
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
    /// Debtor postal address (`Dbtr/PstlAdr`). Optional until 15 November 2026,
    /// after which the EPC schemes require `town` + `country`.
    #[serde(default)]
    pub debtor_address: crate::sepa::AddressParts,
}

pub async fn create_mandate(
    pool: &PgPool,
    tenant: &str,
    iban_key: Option<&[u8; 32]>,
    req: CreateMandateRequest,
) -> anyhow::Result<Uuid> {
    use time::format_description::well_known::Iso8601;
    let signed_at = Date::parse(&req.signed_at, &Iso8601::DEFAULT).context("parse signed_at")?;

    // Look up account.
    let account_id = upsert_account(pool, &req.malo_id, &req.lf_mp_id, tenant).await?;

    let row = sqlx::query(
        // ON CONFLICT uses (tenant, mandatsref) per the schema unique index,
        // not the old global UNIQUE (mandatsref). This prevents cross-tenant collisions.
        r"INSERT INTO sepa_mandates
              (account_id, tenant, iban, bic, kontoinhaber, mandatsref, sequence_type, signed_at,
               debtor_town, debtor_country, debtor_street, debtor_building_number,
               debtor_post_code, debtor_country_subdivision)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
          ON CONFLICT (tenant, mandatsref) DO UPDATE
          SET iban = EXCLUDED.iban, bic = EXCLUDED.bic,
              kontoinhaber = EXCLUDED.kontoinhaber,
              sequence_type = EXCLUDED.sequence_type,
              signed_at = EXCLUDED.signed_at,
              debtor_town = EXCLUDED.debtor_town,
              debtor_country = EXCLUDED.debtor_country,
              debtor_street = EXCLUDED.debtor_street,
              debtor_building_number = EXCLUDED.debtor_building_number,
              debtor_post_code = EXCLUDED.debtor_post_code,
              debtor_country_subdivision = EXCLUDED.debtor_country_subdivision,
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
    .bind(&req.debtor_address.town)
    .bind(&req.debtor_address.country)
    .bind(&req.debtor_address.street)
    .bind(&req.debtor_address.building_number)
    .bind(&req.debtor_address.post_code)
    .bind(&req.debtor_address.country_subdivision)
    .fetch_one(pool)
    .await
    .context("create_mandate")?;

    // Link iban + mandatsref to account for fast lookup. `iban_hash` is the key
    // CAMT.054 matching resolves the account by, so it must be written here too.
    sqlx::query(
        "UPDATE accounts SET iban = $4, iban_hash = $5, mandatsref = $6, updated_at = now()
         WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3",
    )
    .bind(&req.malo_id)
    .bind(&req.lf_mp_id)
    .bind(tenant)
    .bind(&req.iban)
    .bind(crate::ledger::iban_hash(iban_key, &req.iban))
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
    tenant: &str,
) -> anyhow::Result<Option<SepaMandateRow>> {
    sqlx::query_as::<_, SepaMandateRow>(
        "SELECT sm.*, a.sparte FROM sepa_mandates sm \
         JOIN accounts a ON a.account_id = sm.account_id \
         WHERE sm.mandate_id = $1 AND sm.tenant = $2",
    )
    .bind(mandate_id)
    .bind(tenant)
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
        r"SELECT sm.*, a.sparte FROM sepa_mandates sm
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
    exec: impl sqlx::PgExecutor<'_>,
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
    .fetch_one(exec)
    .await
    .context("create_dunning_case")?;
    Ok(row.try_get("id")?)
}

pub async fn resolve_dunning_case(pool: &PgPool, id: Uuid, tenant: &str) -> anyhow::Result<u64> {
    let r = sqlx::query(
        "UPDATE dunning_cases SET resolved_at = now() \
         WHERE id = $1 AND tenant = $2 AND resolved_at IS NULL",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("resolve_dunning_case")?;
    Ok(r.rows_affected())
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
                 sm.debtor_town, sm.debtor_country, sm.debtor_street,
                 sm.debtor_building_number, sm.debtor_post_code,
                 sm.debtor_country_subdivision,
                 sm.updated_at  AS sm_updated_at,
                 a.sparte,
                 a.account_id, a.malo_id, a.lf_mp_id, a.tenant,
                 a.iban         AS a_iban,
                 a.mandatsref   AS a_mandatsref,
                 a.abschlag_ct, a.billing_day, a.balance_ct,
                 a.addr_town, a.addr_country, a.addr_street,
                 a.addr_building_number, a.addr_post_code,
                 a.addr_country_subdivision,
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
            sparte: r.try_get("sparte")?,
            debtor_town: r.try_get("debtor_town")?,
            debtor_country: r.try_get("debtor_country")?,
            debtor_street: r.try_get("debtor_street")?,
            debtor_building_number: r.try_get("debtor_building_number")?,
            debtor_post_code: r.try_get("debtor_post_code")?,
            debtor_country_subdivision: r.try_get("debtor_country_subdivision")?,
            updated_at: r.try_get("sm_updated_at")?,
        };
        let account = AccountRow {
            account_id: r.try_get("account_id")?,
            malo_id: r.try_get("malo_id")?,
            lf_mp_id: r.try_get("lf_mp_id")?,
            tenant: r.try_get("tenant")?,
            kunden_nr: r.try_get("kunden_nr").ok(),
            iban: r.try_get("a_iban")?,
            mandatsref: r.try_get("a_mandatsref")?,
            abschlag_ct: r.try_get("abschlag_ct")?,
            billing_day: r.try_get("billing_day")?,
            balance_ct: r.try_get("balance_ct")?,
            addr_town: r.try_get("addr_town")?,
            addr_country: r.try_get("addr_country")?,
            addr_street: r.try_get("addr_street")?,
            addr_building_number: r.try_get("addr_building_number")?,
            addr_post_code: r.try_get("addr_post_code")?,
            addr_country_subdivision: r.try_get("addr_country_subdivision")?,
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
                 sm.debtor_town, sm.debtor_country, sm.debtor_street,
                 sm.debtor_building_number, sm.debtor_post_code,
                 sm.debtor_country_subdivision,
                 sm.updated_at  AS sm_updated_at,
                 a.sparte,
                 a.account_id, a.malo_id, a.lf_mp_id, a.tenant,
                 a.iban         AS a_iban,
                 a.mandatsref   AS a_mandatsref,
                 a.abschlag_ct, a.billing_day, a.balance_ct,
                 a.addr_town, a.addr_country, a.addr_street,
                 a.addr_building_number, a.addr_post_code,
                 a.addr_country_subdivision,
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
            sparte: r.try_get("sparte")?,
            debtor_town: r.try_get("debtor_town")?,
            debtor_country: r.try_get("debtor_country")?,
            debtor_street: r.try_get("debtor_street")?,
            debtor_building_number: r.try_get("debtor_building_number")?,
            debtor_post_code: r.try_get("debtor_post_code")?,
            debtor_country_subdivision: r.try_get("debtor_country_subdivision")?,
            updated_at: r.try_get("sm_updated_at")?,
        };
        let account = AccountRow {
            account_id: r.try_get("account_id")?,
            malo_id: r.try_get("malo_id")?,
            lf_mp_id: r.try_get("lf_mp_id")?,
            tenant: r.try_get("tenant")?,
            kunden_nr: r.try_get("kunden_nr").ok(),
            iban: r.try_get("a_iban")?,
            mandatsref: r.try_get("a_mandatsref")?,
            abschlag_ct: r.try_get("abschlag_ct")?,
            billing_day: r.try_get("billing_day")?,
            balance_ct: r.try_get("balance_ct")?,
            addr_town: r.try_get("addr_town")?,
            addr_country: r.try_get("addr_country")?,
            addr_street: r.try_get("addr_street")?,
            addr_building_number: r.try_get("addr_building_number")?,
            addr_post_code: r.try_get("addr_post_code")?,
            addr_country_subdivision: r.try_get("addr_country_subdivision")?,
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
/// This function is the  for balance cache drift detection.
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
    ledger: &PgLedger,
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
    repair: bool,
) -> anyhow::Result<BalanceReconcileResult> {
    // The authoritative balance is the doubleentry Kontokorrent net.
    let recomputed = ledger.balance_ct(lf_mp_id, malo_id).await?;

    let acct = sqlx::query(
        "SELECT account_id, balance_ct FROM accounts \
         WHERE malo_id = $1 AND lf_mp_id = $2 AND tenant = $3",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("reconcile: fetch account")?;

    let Some(row) = acct else {
        anyhow::bail!("account not found for reconciliation");
    };
    let account_id: Uuid = row.try_get("account_id")?;
    let cached: i64 = row.try_get("balance_ct")?;
    let drift = cached - recomputed;

    if drift != 0 {
        tracing::warn!(
            account_id = %account_id,
            malo_id,
            cached_ct = cached,
            recomputed_ct = recomputed,
            drift_ct = drift,
            "accountingd: balance cache drift vs. doubleentry ledger detected"
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
            tracing::info!(account_id = %account_id, "accountingd: balance cache repaired from ledger");
        }
    }

    Ok(BalanceReconcileResult {
        account_id,
        malo_id: malo_id.to_owned(),
        cached_balance_ct: cached,
        recomputed_balance_ct: recomputed,
        is_consistent: drift == 0,
        drift_ct: drift,
    })
}

// ── Open-item management ────────────────────────────────────────────────

/// The customer's authoritative **Offene Posten** — open debit items (invoices,
/// fees) with a positive residual after recorded FIFO clearings (§ 252 HGB
/// per-receivable tracking). Clearings are recorded on every post, so this
/// reflects what has actually been paid, not a gross list.
pub async fn list_open_items(
    ledger: &PgLedger,
    lf_mp_id: &str,
    malo_id: &str,
) -> anyhow::Result<Vec<crate::ledger::OpenReceivable>> {
    ledger.open_receivables(lf_mp_id, malo_id).await
}

// ── GDPR Art. 17 anonymization ─────────────────────────────────────────

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
    // A postal address is personal data in its own right — the EPC cut-over
    // made mako store one, so erasure has to reach it. The address *snapshots*
    // on `eeg_payout_orders` are deliberately left alone: they are part of a
    // Buchungsbeleg and carry the statutory retention the ledger entries do.
    let anonymized_fields = serde_json::json!([
        "accounts.iban",
        "accounts.mandatsref",
        "accounts.zahlungsinformation",
        "accounts.vorauszahlung",
        "accounts.addr_*",
        "sepa_mandates.iban",
        "sepa_mandates.kontoinhaber",
        "sepa_mandates.bic",
        "sepa_mandates.debtor_*"
    ]);

    let mut tx = pool.begin().await.context("anonymize: begin tx")?;

    // 1. Anonymize accounts table PII.
    sqlx::query(
        "UPDATE accounts
         SET iban               = 'ANONYMIZED',
             mandatsref         = NULL,
             zahlungsinformation = NULL,
             vorauszahlung      = NULL,
             addr_town          = NULL,
             addr_country       = NULL,
             addr_street        = NULL,
             addr_building_number = NULL,
             addr_post_code     = NULL,
             addr_country_subdivision = NULL,
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
             debtor_town   = NULL,
             debtor_country = NULL,
             debtor_street = NULL,
             debtor_building_number = NULL,
             debtor_post_code = NULL,
             debtor_country_subdivision = NULL,
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

// ── Automatic Mahnwesen (dunning rule engine) ──────────────────────────

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
/// This is the **dunning rule engine**. It evaluates every active account
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
/// Try to acquire a session-level PostgreSQL advisory lock for a worker.
///
/// Returns the held connection when the lock is won (a second replica gets
/// `None` and must skip the cycle) — so only one instance runs a given worker
/// at a time, on top of the per-run idempotency guards. Call
/// [`release_worker_lock`] with the same connection when done.
pub async fn try_worker_lock(
    pool: &PgPool,
    key: i64,
) -> Option<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    let mut conn = pool.acquire().await.ok()?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(&mut *conn)
        .await
        .ok()?;
    if got { Some(conn) } else { None }
}

/// Release a worker advisory lock held on `conn` (same connection that took it).
pub async fn release_worker_lock(conn: &mut sqlx::PgConnection, key: i64) {
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(conn)
        .await;
}

/// Advisory-lock keys (stable, distinct per worker).
pub const LOCK_ABSCHLAG: i64 = 0x_acc0_0001;
pub const LOCK_SEPA_N5: i64 = 0x_acc0_0002;
pub const LOCK_DUNNING: i64 = 0x_acc0_0003;

/// Live financial metrics for the Prometheus `/metrics` endpoint.
#[derive(Debug, Default)]
pub struct FinancialMetrics {
    pub accounts_total: i64,
    pub open_receivables_ct: i64,
    pub credit_balances_ct: i64,
    pub dunning_stufe1: i64,
    pub dunning_stufe2: i64,
    pub dunning_stufe3: i64,
    pub sepa_runs_pending: i64,
    pub sperrung_pending: i64,
    /// Collections submitted to the bank with no reply yet — no pain.002, no
    /// camt booking. A number that only grows means bank replies are not
    /// arriving at all, which looks identical to "everything settled" from the
    /// ledger alone.
    pub sepa_collections_open: i64,
    /// Collections the bank refused (pain.002 `RJCT`). The money never moved,
    /// so each one is a receivable still open against a mandate that needs
    /// attention.
    pub sepa_collections_rejected: i64,
    /// Settled collections the debtor took back (camt R-transaction). Each
    /// carries an R-transaction fee and re-opens the receivable.
    pub sepa_collections_returned: i64,
    /// Amount, in ct, sitting in `SUBMITTED` — the money in flight.
    pub sepa_collections_open_ct: i64,
}

pub async fn financial_metrics(pool: &PgPool, tenant: &str) -> anyhow::Result<FinancialMetrics> {
    let row = sqlx::query(
        r"SELECT
            (SELECT COUNT(*) FROM accounts WHERE tenant = $1)::bigint AS accounts_total,
            (SELECT COALESCE(SUM(balance_ct), 0) FROM accounts WHERE tenant = $1 AND balance_ct > 0)::bigint AS open_receivables_ct,
            (SELECT COALESCE(-SUM(balance_ct), 0) FROM accounts WHERE tenant = $1 AND balance_ct < 0)::bigint AS credit_balances_ct,
            (SELECT COUNT(*) FROM dunning_cases WHERE tenant = $1 AND stufe = 1 AND resolved_at IS NULL)::bigint AS d1,
            (SELECT COUNT(*) FROM dunning_cases WHERE tenant = $1 AND stufe = 2 AND resolved_at IS NULL)::bigint AS d2,
            (SELECT COUNT(*) FROM dunning_cases WHERE tenant = $1 AND stufe = 3 AND resolved_at IS NULL)::bigint AS d3,
            (SELECT COUNT(*) FROM sepa_collection_runs WHERE tenant = $1 AND dispatch_status = 'PENDING')::bigint AS sepa_pending,
            (SELECT COUNT(*) FROM dunning_cases WHERE tenant = $1 AND stufe = 3 AND resolved_at IS NULL AND sperrauftrag_ce_id IS NOT NULL)::bigint AS sperrung_pending,
            (SELECT COUNT(*) FROM sepa_collection_entries WHERE tenant = $1 AND status = 'SUBMITTED')::bigint AS coll_open,
            (SELECT COUNT(*) FROM sepa_collection_entries WHERE tenant = $1 AND status = 'REJECTED')::bigint AS coll_rejected,
            (SELECT COUNT(*) FROM sepa_collection_entries WHERE tenant = $1 AND status = 'RETURNED')::bigint AS coll_returned,
            (SELECT COALESCE(SUM(amount_ct), 0) FROM sepa_collection_entries WHERE tenant = $1 AND status = 'SUBMITTED')::bigint AS coll_open_ct",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("financial_metrics")?;
    Ok(FinancialMetrics {
        accounts_total: row.get("accounts_total"),
        open_receivables_ct: row.get("open_receivables_ct"),
        credit_balances_ct: row.get("credit_balances_ct"),
        dunning_stufe1: row.get("d1"),
        dunning_stufe2: row.get("d2"),
        dunning_stufe3: row.get("d3"),
        sepa_runs_pending: row.get("sepa_pending"),
        sperrung_pending: row.get("sperrung_pending"),
        sepa_collections_open: row.get("coll_open"),
        sepa_collections_rejected: row.get("coll_rejected"),
        sepa_collections_returned: row.get("coll_returned"),
        sepa_collections_open_ct: row.get("coll_open_ct"),
    })
}

/// Rows returned by every §§41f/41g phase query: `(case_id, malo_id, lf_mp_id, amount_ct)`.
type SperrCandidate = (Uuid, String, String, i64);

fn to_sperr_candidates(rows: Vec<sqlx::postgres::PgRow>) -> Vec<SperrCandidate> {
    rows.into_iter()
        .map(|r| {
            (
                r.get::<Uuid, _>("id"),
                r.get::<String, _>("malo_id"),
                r.get::<String, _>("lf_mp_id"),
                r.get::<i64, _>("amount_due_ct"),
            )
        })
        .collect()
}

/// **Phase 1 (§41f Abs. 1 EnWG) — Sperrandrohung candidates.**
///
/// Mahnstufe-3, unresolved, not yet threatened, not halted by an
/// Abwendungsvereinbarung (§41g Abs. 1) or an Unverhältnismäßigkeit/
/// Schutzbedürftigkeit flag (§41f Abs. 1 Satz 2 / Abs. 2), and past **both**
/// §41f Abs. 3 thresholds:
///
/// - **Satz 2** — an absolute floor of `threshold_ct` (≥ 100 EUR).
/// - **Satz 1** — a consumption-relative gate: arrears ≥ **2×** the agreed
///   monthly Abschlag (`accounts.abschlag_ct`); *wenn keine Abschläge vereinbart
///   sind* (i.e. `abschlag_ct = 0`), ≥ **⅙** of the most recent expected annual
///   bill (`jahresabschluss_runs.annual_bill_ct`).
///
/// When neither an Abschlag nor a prior Jahresrechnung is on record the Satz-1
/// gate cannot be established, the `CASE` yields `NULL`, and the case is
/// **conservatively excluded** — mako never disconnects without a provable
/// consumption basis. Populate `abschlag_ct` (or run a Jahresabschluss) to arm
/// the sequence for such an account.
pub async fn list_androhung_candidates(
    pool: &PgPool,
    tenant: &str,
    threshold_ct: i64,
) -> anyhow::Result<Vec<SperrCandidate>> {
    let rows = sqlx::query(
        r"SELECT dc.id, a.malo_id, a.lf_mp_id, dc.amount_due_ct
          FROM dunning_cases dc
          JOIN accounts a ON a.account_id = dc.account_id
          WHERE dc.tenant = $1
            AND dc.stufe = 3
            AND dc.resolved_at IS NULL
            AND dc.sperrandrohung_at IS NULL
            AND dc.abwendung_vereinbart_at IS NULL
            AND dc.unverhaeltnismaessig_seit IS NULL
            AND dc.amount_due_ct >= $2                       -- §41f Abs. 3 S. 2: ≥ 100 EUR floor
            AND dc.amount_due_ct >= (                        -- §41f Abs. 3 S. 1: consumption gate
                  CASE
                    WHEN a.abschlag_ct > 0 THEN 2 * a.abschlag_ct
                    ELSE (SELECT jr.annual_bill_ct / 6
                          FROM jahresabschluss_runs jr
                          WHERE jr.tenant = dc.tenant AND jr.malo_id = a.malo_id
                          ORDER BY jr.billing_year DESC
                          LIMIT 1)
                  END)",
    )
    .bind(tenant)
    .bind(threshold_ct)
    .fetch_all(pool)
    .await
    .context("list_androhung_candidates")?;
    Ok(to_sperr_candidates(rows))
}

/// Record that the Sperrandrohung was issued (opens the 4-Wochen-Frist).
///
/// Takes an executor so the caller can commit it in the **same transaction** as
/// the `de.accounting.sperrandrohung` outbox row — the Androhung is a legal act
/// (a letter the ERP must send), so state and dispatch must be atomic.
pub async fn mark_sperrandrohung(
    exec: impl sqlx::PgExecutor<'_>,
    case_id: Uuid,
    tenant: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE dunning_cases SET sperrandrohung_at = now() \
         WHERE id = $1 AND tenant = $2 AND sperrandrohung_at IS NULL",
    )
    .bind(case_id)
    .bind(tenant)
    .execute(exec)
    .await
    .context("mark_sperrandrohung")?;
    Ok(())
}

/// **Phase 2 (§41f Abs. 5 EnWG) — Sperrankündigung candidates.**
///
/// The 4-Wochen Androhung Frist (`androhung_frist_days`) has elapsed, the case is
/// still unresolved and un-halted, and no Ankündigung has been sent yet.
pub async fn list_ankuendigung_candidates(
    pool: &PgPool,
    tenant: &str,
    androhung_frist_days: i64,
) -> anyhow::Result<Vec<SperrCandidate>> {
    let rows = sqlx::query(
        r"SELECT dc.id, a.malo_id, a.lf_mp_id, dc.amount_due_ct
          FROM dunning_cases dc
          JOIN accounts a ON a.account_id = dc.account_id
          WHERE dc.tenant = $1
            AND dc.stufe = 3
            AND dc.resolved_at IS NULL
            AND dc.abwendung_vereinbart_at IS NULL
            AND dc.unverhaeltnismaessig_seit IS NULL
            AND dc.sperrandrohung_at IS NOT NULL
            AND dc.sperrankuendigung_at IS NULL
            AND dc.sperrandrohung_at + make_interval(days => $2::int) <= now()",
    )
    .bind(tenant)
    .bind(androhung_frist_days)
    .fetch_all(pool)
    .await
    .context("list_ankuendigung_candidates")?;
    Ok(to_sperr_candidates(rows))
}

/// Record the Sperrankündigung and the concrete planned disconnection date
/// (`geplantes_sperrdatum` = today + 8 Werktage, computed by the caller).
///
/// Takes an executor so the caller can commit it in the **same transaction** as
/// the `de.accounting.sperrankuendigung` outbox row (persist-before-dispatch).
pub async fn mark_sperrankuendigung(
    exec: impl sqlx::PgExecutor<'_>,
    case_id: Uuid,
    tenant: &str,
    geplantes_sperrdatum: time::Date,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE dunning_cases SET sperrankuendigung_at = now(), geplantes_sperrdatum = $3 \
         WHERE id = $1 AND tenant = $2 AND sperrankuendigung_at IS NULL",
    )
    .bind(case_id)
    .bind(tenant)
    .bind(geplantes_sperrdatum)
    .execute(exec)
    .await
    .context("mark_sperrankuendigung")?;
    Ok(())
}

/// **Phase 3 — Sperrauftrag candidates.**
///
/// The announced disconnection date (`geplantes_sperrdatum`, = Ankündigung +
/// 8 Werktage per §41f Abs. 5) has arrived, the case is still unresolved and
/// un-halted, and no Sperrauftrag has been handed to sperrd yet.
pub async fn list_sperrauftrag_candidates(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Vec<SperrCandidate>> {
    let rows = sqlx::query(
        r"SELECT dc.id, a.malo_id, a.lf_mp_id, dc.amount_due_ct
          FROM dunning_cases dc
          JOIN accounts a ON a.account_id = dc.account_id
          WHERE dc.tenant = $1
            AND dc.stufe = 3
            AND dc.resolved_at IS NULL
            AND dc.abwendung_vereinbart_at IS NULL
            AND dc.unverhaeltnismaessig_seit IS NULL
            AND dc.sperrankuendigung_at IS NOT NULL
            AND dc.sperrauftrag_ce_id IS NULL
            AND dc.geplantes_sperrdatum IS NOT NULL
            AND dc.geplantes_sperrdatum <= CURRENT_DATE",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_sperrauftrag_candidates")?;
    Ok(to_sperr_candidates(rows))
}

/// §41g Abs. 1 Satz 10 EnWG — the customer accepted an Abwendungsvereinbarung
/// (payment agreement) in Textform: disconnection **must not** proceed.
///
/// The agreement covers the arrears of the **supply point**, and disconnection is
/// per supply point, so this halts **every** open dunning case of the account that
/// owns `case_id` — not just that one case (auto-dunning creates a fresh case per
/// Mahnstufe, so an account can carry more than one open case). Returns the number
/// of cases halted (0 → the case id was not found). See [`markiere_unverhaeltnismaessig`].
pub async fn vereinbare_abwendung(
    pool: &PgPool,
    case_id: Uuid,
    tenant: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        "UPDATE dunning_cases SET abwendung_vereinbart_at = now() \
         WHERE tenant = $2 AND resolved_at IS NULL \
           AND account_id = (SELECT account_id FROM dunning_cases WHERE id = $1 AND tenant = $2)",
    )
    .bind(case_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("vereinbare_abwendung")?;
    Ok(r.rows_affected())
}

/// §41f Abs. 1 Satz 2 / Abs. 2 EnWG — the disconnection is disproportionate
/// (the customer showed a payment prospect, or besondere Schutzbedürftigkeit /
/// konkrete Gefahr für Leib oder Leben).
///
/// Like [`vereinbare_abwendung`], the Schutzbedürftigkeit attaches to the supply
/// point, so this halts **every** open dunning case of the account owning
/// `case_id`. Returns the number of cases halted (0 → the case id was not found).
pub async fn markiere_unverhaeltnismaessig(
    pool: &PgPool,
    case_id: Uuid,
    tenant: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        "UPDATE dunning_cases SET unverhaeltnismaessig_seit = now() \
         WHERE tenant = $2 AND resolved_at IS NULL \
           AND account_id = (SELECT account_id FROM dunning_cases WHERE id = $1 AND tenant = $2)",
    )
    .bind(case_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("markiere_unverhaeltnismaessig")?;
    Ok(r.rows_affected())
}

/// Record that a Sperrauftrag was handed to sperrd (idempotency: won't re-post).
pub async fn mark_sperrauftrag_dispatched(
    exec: impl sqlx::PgExecutor<'_>,
    case_id: Uuid,
    tenant: &str,
    reference: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE dunning_cases SET sperrauftrag_ce_id = $1 \
         WHERE id = $2 AND tenant = $3",
    )
    .bind(reference)
    .bind(case_id)
    .bind(tenant)
    .execute(exec)
    .await
    .context("mark_sperrauftrag_dispatched")?;
    Ok(())
}

/// Open a Mahnstufe case and announce it as `de.accounting.mahnung.issued`.
///
/// The event was declared in the catalog and documented as emitted, and
/// `agentd`'s payment-reconciliation agent triggers on it — but nothing ever
/// produced it. (`MAHNUNG_ISSUED` appeared in this file only as a *correlation
/// string* on the Mahngebühr ledger entry, which is not an emission.)
///
/// Case insert and outbox enqueue share one transaction: the outbox is
/// persist-before-dispatch, so a crash between them would open a dunning case
/// nobody downstream ever hears about.
#[allow(clippy::too_many_arguments)]
pub async fn create_dunning_case_announced(
    pool: &PgPool,
    tenant: &str,
    account_id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    stufe: i16,
    amount_due_ct: i64,
    due_date: Date,
) -> anyhow::Result<Uuid> {
    let mut tx = pool.begin().await?;
    let case_id =
        create_dunning_case(&mut *tx, account_id, tenant, stufe, amount_due_ct, due_date).await?;

    let ce = mako_service::CloudEvent::new(
        mako_service::source("accountingd", tenant),
        mako_events::accounting::MAHNUNG_ISSUED,
        malo_id,
        serde_json::json!({
            "malo_id":       malo_id,
            "lf_mp_id":      lf_mp_id,
            "mahnstufe":     stufe,
            "amount_due_ct": amount_due_ct,
            "amount_eur":    format!("{:.2}", amount_due_ct as f64 / 100.0),
            "due_date":      due_date.to_string(),
            "case_id":       case_id.to_string(),
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
    tx.commit().await?;
    Ok(case_id)
}

pub async fn run_auto_dunning(
    ledger: &PgLedger,
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
    // Pre-filter cheaply in SQL on the balance cache + no-open-case; the "debt
    // aged past grace" signal comes from the ledger (a charge booked on/before
    // cutoff), checked per candidate below.
    let prefiltered: Vec<(Uuid, String, String, i64)> = sqlx::query(
        r"SELECT a.account_id, a.malo_id, a.lf_mp_id, a.balance_ct
          FROM accounts a
          WHERE a.tenant = $1
            AND a.balance_ct > 0
            AND a.anonymized_at IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM dunning_cases dc
                WHERE dc.account_id = a.account_id
                  AND dc.resolved_at IS NULL
            )",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("auto_dunning: find Mahnstufe1 candidates")?
    .into_iter()
    .map(|r| {
        (
            r.try_get::<Uuid, _>("account_id").unwrap_or(Uuid::nil()),
            r.try_get::<String, _>("malo_id").unwrap_or_default(),
            r.try_get::<String, _>("lf_mp_id").unwrap_or_default(),
            r.try_get::<i64, _>("balance_ct").unwrap_or(0),
        )
    })
    .collect();

    let mut candidates: Vec<(Uuid, String, String, i64)> = Vec::new();
    for cand in prefiltered {
        if ledger
            .has_debit_on_or_before(&cand.2, &cand.1, cutoff)
            .await
            .unwrap_or(false)
        {
            candidates.push(cand);
        }
    }

    let stufe1_due_date = today + time::Duration::days(14); // 14-day payment deadline

    for (account_id, malo_id, lf_mp_id, balance_ct) in &candidates {
        let case_id = create_dunning_case_announced(
            pool,
            tenant,
            *account_id,
            malo_id,
            lf_mp_id,
            1,
            *balance_ct,
            stufe1_due_date,
        )
        .await
        .context("auto_dunning: create Mahnstufe1")?;

        // Post Mahngebühr if configured > 0. Deterministic idempotency key per
        // (malo, Mahnstufe, run-date) — a same-day re-run replays as a no-op
        // instead of double-charging the fee (fixes the old un-idempotent write).
        if fee_stufe1_ct > 0 {
            let _ = post_entry(
                ledger,
                pool,
                tenant,
                malo_id,
                lf_mp_id,
                "MAHNGEBUEHR",
                fee_stufe1_ct,
                &format!("mahngebuehr:{malo_id}:1:{today}"),
                Some(mako_events::accounting::MAHNUNG_ISSUED),
                Some(&case_id.to_string()),
                today,
                today,
                Some("Mahngebühr Mahnstufe 1"),
                None,
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
    let overdue_stufe1: Vec<(Uuid, Uuid, String, String, i64)> = sqlx::query(
        r"SELECT dc.id, dc.account_id, a.malo_id, a.lf_mp_id, dc.amount_due_ct
          FROM dunning_cases dc
          JOIN accounts a ON a.account_id = dc.account_id
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
            r.try_get::<String, _>("malo_id").unwrap_or_default(),
            r.try_get::<String, _>("lf_mp_id").unwrap_or_default(),
            r.try_get::<i64, _>("amount_due_ct").unwrap_or(0),
        )
    })
    .collect();

    let stufe2_due_date = today + time::Duration::days(14);

    for (old_case_id, account_id, malo_id, lf_mp_id, amount_due_ct) in &overdue_stufe1 {
        // Resolve the old Mahnstufe 1 case.
        resolve_dunning_case(pool, *old_case_id, tenant)
            .await
            .context("auto_dunning: resolve Mahnstufe1")?;

        let case_id = create_dunning_case_announced(
            pool,
            tenant,
            *account_id,
            malo_id,
            lf_mp_id,
            2,
            *amount_due_ct,
            stufe2_due_date,
        )
        .await
        .context("auto_dunning: create Mahnstufe2")?;

        if fee_stufe2_ct > 0 {
            let _ = post_entry(
                ledger,
                pool,
                tenant,
                malo_id,
                lf_mp_id,
                "MAHNGEBUEHR",
                fee_stufe2_ct,
                &format!("mahngebuehr:{malo_id}:2:{today}"),
                Some(mako_events::accounting::MAHNUNG_ISSUED),
                Some(&case_id.to_string()),
                today,
                today,
                Some("Mahngebühr Mahnstufe 2"),
                None,
            )
            .await;
        }

        escalated += 1;
        tracing::info!(account_id = %account_id, "accountingd: auto-dunning escalated to Mahnstufe 2");
    }

    // ── Step 3: Escalate Mahnstufe 2 → 3 + Sperrauftrag ─────────────────────
    let overdue_stufe2: Vec<(Uuid, Uuid, String, String, i64)> = sqlx::query(
        r"SELECT dc.id, dc.account_id, a.malo_id, a.lf_mp_id, dc.amount_due_ct
          FROM dunning_cases dc
          JOIN accounts a ON a.account_id = dc.account_id
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
            r.try_get::<String, _>("malo_id").unwrap_or_default(),
            r.try_get::<String, _>("lf_mp_id").unwrap_or_default(),
            r.try_get::<i64, _>("amount_due_ct").unwrap_or(0),
        )
    })
    .collect();

    let stufe3_due_date = today + time::Duration::days(7); // shorter final deadline

    for (old_case_id, account_id, malo_id, lf_mp_id, amount_due_ct) in &overdue_stufe2 {
        resolve_dunning_case(pool, *old_case_id, tenant)
            .await
            .context("auto_dunning: resolve Mahnstufe2")?;

        let _case_id = create_dunning_case_announced(
            pool,
            tenant,
            *account_id,
            malo_id,
            lf_mp_id,
            3,
            *amount_due_ct,
            stufe3_due_date,
        )
        .await
        .context("auto_dunning: create Mahnstufe3")?;

        if fee_stufe3_ct > 0 {
            let _ = post_entry(
                ledger,
                pool,
                tenant,
                malo_id,
                lf_mp_id,
                "MAHNGEBUEHR",
                fee_stufe3_ct,
                &format!("mahngebuehr:{malo_id}:3:{today}"),
                Some(mako_events::accounting::MAHNUNG_ISSUED),
                Some(&_case_id.to_string()),
                today,
                today,
                Some("Mahngebühr Mahnstufe 3"),
                None,
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

// ── SKR double-entry mapping now lives in the doubleentry ledger ──────────────
//
// The SKR 03/04 journal mapping (JournalMapping / journal_mapping) and the
// `journal_lines` shadow inserter are gone. accountingd's chart of accounts and
// the entry_type→postings mapping live in `ledger.rs` (Chart::contra); doubleentry
// posts the balanced entry and enforces Soll=Haben in-engine and in its schema.

// ── Aging analysis ────────────────────────────────────────────────────────────

/// Aging bucket for open receivables.
#[derive(Debug, Serialize)]
pub struct AgingBucket {
    pub bucket: &'static str, // "0-30d", "31-60d", "61-90d", ">90d"
    pub account_count: i64,
    pub total_ct: i64,
    pub total_eur: String,
}

/// Aging analysis: group overdue account balances by days-overdue bucket.
///
/// Uses `accounts.balance_ct` (cached) as the outstanding amount per MaLo.
/// Overdue date is approximated from the oldest unresolved `dunning_cases.issued_at`
/// when present, or the account `updated_at` otherwise.
///
/// Returns four buckets: 0–30 days, 31–60 days, 61–90 days, >90 days.
pub async fn list_aging_buckets(pool: &PgPool, tenant: &str) -> anyhow::Result<Vec<AgingBucket>> {
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
        ("0-30d", 0i32, 30i32),
        ("31-60d", 31, 60),
        ("61-90d", 61, 90),
        (">90d", 91, i32::MAX),
    ] {
        let (account_count, total_ct) = rows
            .iter()
            .find(|r| {
                r.try_get::<&str, _>("bucket")
                    .map(|b| b == *label)
                    .unwrap_or(false)
            })
            .map(|r| {
                (
                    r.try_get::<i64, _>("account_count").unwrap_or(0),
                    r.try_get::<i64, _>("total_ct").unwrap_or(0),
                )
            })
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
    pub id: Uuid,
    pub account_id: Uuid,
    pub tenant: String,
    pub invoice_reference: Option<String>,
    pub principal_ct: i64,
    pub interest_ct: i64,
    pub rate_pct: rust_decimal::Decimal,
    pub ecb_base_rate_pct: rust_decimal::Decimal,
    pub customer_type: String,
    pub period_from: time::Date,
    pub period_to: time::Date,
    pub legal_basis: String,
    pub ledger_entry_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
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
            tracing::warn!(
                "accountingd: no ECB base rate found — using 2.00% fallback. Seed ecb_base_rates table."
            );
            Ok(rust_decimal::Decimal::new(200, 2)) // 2.00%
        }
    }
}

/// Create a Verzugszinsen (default interest) charge and the linked MAHNGEBUEHR ledger entry.
#[allow(clippy::too_many_arguments)]
pub async fn create_interest_charge(
    ledger: &PgLedger,
    pool: &PgPool,
    account_id: Uuid,
    tenant: &str,
    malo_id: &str,
    lf_mp_id: &str,
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

    // Create linked MAHNGEBUEHR ledger entry (deterministic idempotency key per
    // malo + interest period — a re-post is a no-op, not a double charge).
    let ledger_id = post_entry(
        ledger,
        pool,
        tenant,
        malo_id,
        lf_mp_id,
        "MAHNGEBUEHR",
        interest_ct,
        &format!("interest:{malo_id}:{period_from}:{period_to}"),
        None,
        invoice_reference,
        period_to,
        period_to,
        Some(legal_basis),
        None,
    )
    .await
    .context("create_interest_charge: ledger entry")?;

    // The satellite row, the outbox announcement and the idempotency guard all
    // commit together. Previously the insert had no unique key and the enqueue
    // ran in its own transaction *after* it: an enqueue failure returned `Err`,
    // the caller retried, and the ledger (idempotent on its own key) stayed
    // right while this table grew a second charge for the same period.
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, InterestChargeRow>(
        r"INSERT INTO interest_charges
              (account_id, tenant, invoice_reference, principal_ct, interest_ct, rate_pct,
               ecb_base_rate_pct, customer_type, period_from, period_to, legal_basis, ledger_entry_id)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
          ON CONFLICT (tenant, account_id, period_from, period_to) DO NOTHING
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
    .bind(ledger_id)
    .fetch_optional(&mut *tx)
    .await
    .context("create_interest_charge: insert")?;

    let Some(row) = row else {
        // Already charged for this period — return the existing row and do not
        // announce again.
        tx.rollback().await.ok();
        return sqlx::query_as::<_, InterestChargeRow>(
            "SELECT * FROM interest_charges \
             WHERE tenant = $1 AND account_id = $2 AND period_from = $3 AND period_to = $4",
        )
        .bind(tenant)
        .bind(account_id)
        .bind(period_from)
        .bind(period_to)
        .fetch_one(pool)
        .await
        .context("create_interest_charge: fetch existing");
    };

    // Verzugszinsen are a charge to the customer, so the ERP has to learn of
    // them to put them on the next statement. `INTEREST_CHARGED` was declared
    // in the catalog and never emitted.
    let ce = mako_service::CloudEvent::new(
        mako_service::source("accountingd", tenant),
        mako_events::accounting::INTEREST_CHARGED,
        malo_id,
        serde_json::json!({
            "malo_id":           malo_id,
            "lf_mp_id":          lf_mp_id,
            "principal_ct":      principal_ct,
            "interest_ct":       interest_ct,
            "interest_eur":      format!("{:.2}", interest_ct as f64 / 100.0),
            "rate_pct":          annual_rate.to_string(),
            "customer_type":     customer_type,
            "period_from":       period_from.to_string(),
            "period_to":         period_to.to_string(),
            "rechtsgrundlage":   legal_basis,
            "invoice_reference": invoice_reference,
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
    tx.commit().await?;

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
    pub plan_id: Uuid,
    pub account_id: Uuid,
    pub tenant: String,
    pub total_ct: i64,
    pub installment_ct: i64,
    pub installment_count: i32,
    pub billing_day: i16,
    pub status: String,
    pub dunning_case_id: Option<Uuid>,
    pub operator_sub: Option<String>,
    pub note: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PaymentPlanInstallmentRow {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub tenant: String,
    pub installment_no: i32,
    pub due_date: time::Date,
    pub amount_ct: i64,
    pub status: String,
    pub ledger_entry_id: Option<Uuid>,
    pub paid_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct CreatePaymentPlanRequest {
    pub malo_id: String,
    pub lf_mp_id: Option<String>,
    pub total_ct: i64,
    pub installment_ct: i64,
    pub billing_day: i16,
    pub first_due_date: String, // ISO 8601 date
    pub dunning_case_id: Option<Uuid>,
    pub note: Option<String>,
    pub operator_sub: Option<String>,
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
                time::Month::try_from(((first_due.month() as u8 - 1 + n as u8) % 12) + 1)
                    .unwrap_or(time::Month::January),
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
#[allow(clippy::too_many_arguments)]
pub async fn record_bank_import(
    pool: &PgPool,
    tenant: &str,
    bank_transaction_id: &str,
    amount_ct: i64,
    iban: Option<&str>,
    value_date: time::Date,
    ledger_entry_id: Option<Uuid>,
    payment_info_id: Option<&str>,
    end_to_end_id: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO bank_import_log \
         (tenant, bank_transaction_id, amount_ct, iban, value_date, ledger_entry_id, \
          payment_info_id, end_to_end_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (tenant, bank_transaction_id) DO NOTHING",
    )
    .bind(tenant)
    .bind(bank_transaction_id)
    .bind(amount_ct)
    .bind(iban)
    .bind(value_date)
    .bind(ledger_entry_id)
    .bind(payment_info_id)
    .bind(end_to_end_id)
    .execute(pool)
    .await
    .context("record_bank_import")?;
    Ok(())
}
