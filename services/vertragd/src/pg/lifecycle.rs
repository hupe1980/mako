//! Queries behind the daily lifecycle workers.
//!
//! Each of these returns everything the worker needs to decide *and* to say
//! which rule it applied — the contract's Vertragsart and the customer's
//! Haushaltskunden-Eigenschaft travel with the row, because the applicable
//! deadline depends on both (see [`crate::domain`]).

use anyhow::Result;
use serde::Serialize;
use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

use crate::domain::Verlaengerung;

// ── Single-flight across replicas ────────────────────────────────────────────
//
// The three lifecycle workers are idempotent *per contract and per date* — a
// notice is not resent once its slice is marked — but idempotency is a
// serialisation guarantee, not a concurrency one: two replicas that read the
// same unmarked slice in the same second both build a CloudEvent and both write
// one, and the customer receives the § 41 Abs. 5 EnWG Preisanpassungsanzeige
// twice. Nothing downstream de-duplicates it; the outbox delivers what it is
// given.
//
// A PostgreSQL **session-level advisory lock** is the smallest thing that
// prevents that: one instance runs a given worker, the others skip the cycle and
// come back on the next tick. No lease table, no clock skew, and the lock is
// released by the server if the holder dies.
//
// The mechanism, the key constants and the `Option<PoolConnection>` shape are
// deliberately identical to `accountingd::pg::try_worker_lock` — see the
// coordinator note about hoisting both copies into `mako-service`. The keys
// differ so the two services never contend with one another on one database.

// The single-runner advisory lock lives in `mako-service`, so every service
// takes it the same way and a third copy cannot drift from the first two.
pub use mako_service::worker_lock::{release_worker_lock, try_worker_lock};

/// Advisory-lock keys — stable, and distinct per worker so a slow
/// Preisanpassung run does not block the unrelated Ablauf sweep.
pub const LOCK_PREISANPASSUNG: i64 = 0x_7e64_0001;
/// See [`LOCK_PREISANPASSUNG`].
pub const LOCK_AUTO_RENEWAL: i64 = 0x_7e64_0002;
/// See [`LOCK_PREISANPASSUNG`].
pub const LOCK_ABLAUF: i64 = 0x_7e64_0003;

// ── Auto-renewal (§ 309 Nr. 9 lit. b BGB) ────────────────────────────────────

/// A contract whose term is running out and that renews automatically.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutoRenewalRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub vertrags_nr: String,
    pub vertragsende: Date,
    pub renewal_monate: i32,
    pub kuendigungsfrist_monate: i32,
    /// Decides whether the extension may be a further fixed term at all.
    pub haushaltskunde: bool,
    pub bundle_code: Option<String>,
}

/// Contracts whose renewal notice is due within `look_ahead_days` and has not
/// gone out for this term yet.
///
/// The notice is tracked against the term it announces
/// (`autoerneuerung_notif_fuer`), so the daily loop sends it once instead of
/// once a day, and the next term's notice is still due.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_auto_renewal_due(
    pool: &PgPool,
    tenant: &str,
    look_ahead_days: i64,
) -> Result<Vec<AutoRenewalRow>> {
    let today = mako_fristen::heute();
    let cutoff = today + time::Duration::days(look_ahead_days);
    Ok(sqlx::query_as::<_, AutoRenewalRow>(
        r"SELECT v.id, v.kunden_id, v.vertrags_nr, v.vertragsende, v.renewal_monate,
                 v.kuendigungsfrist_monate, k.haushaltskunde, v.bundle_code
          FROM versorgungsvertraege v
          JOIN kunden k ON k.id = v.kunden_id
          WHERE v.tenant = $1
            AND v.status = 'AKTIV'
            AND v.auto_renewal = TRUE
            AND v.vertragsende IS NOT NULL
            AND v.vertragsende BETWEEN $2 AND $3
            AND v.autoerneuerung_notif_fuer IS DISTINCT FROM v.vertragsende",
    )
    .bind(tenant)
    .bind(today)
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

/// Record that the Ankündigung for the term ending `vertragsende` went out.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn mark_auto_renewal_notified(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    vertragsende: Date,
) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege
         SET autoerneuerung_notif_fuer = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(vertragsende)
    .execute(executor)
    .await?;
    Ok(())
}

/// Contracts whose term has ended and that renew automatically.
///
/// `vertragsende <= today` rather than `= today`: the worker runs once a day,
/// so a single missed run otherwise skipped the renewal permanently and left
/// the contract expired.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_auto_renewal_overdue(
    pool: &PgPool,
    tenant: &str,
    today: Date,
) -> Result<Vec<AutoRenewalRow>> {
    Ok(sqlx::query_as::<_, AutoRenewalRow>(
        r"SELECT v.id, v.kunden_id, v.vertrags_nr, v.vertragsende, v.renewal_monate,
                 v.kuendigungsfrist_monate, k.haushaltskunde, v.bundle_code
          FROM versorgungsvertraege v
          JOIN kunden k ON k.id = v.kunden_id
          WHERE v.tenant = $1
            AND v.status = 'AKTIV'
            AND v.auto_renewal = TRUE
            AND v.vertragsende IS NOT NULL
            AND v.vertragsende <= $2",
    )
    .bind(tenant)
    .bind(today)
    .fetch_all(pool)
    .await?)
}

/// Apply an automatic extension.
///
/// [`Verlaengerung::Unbefristet`] is the only lawful tacit extension of a
/// consumer contract (§ 309 Nr. 9 lit. b BGB): the term is dropped, the notice
/// period is capped at one month, and `auto_renewal` is cleared because an
/// open-ended contract has nothing left to renew. Extending such a contract by
/// another twelve months is an unenforceable clause, and it is the customer who
/// finds out.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn apply_auto_renewal(pool: &PgPool, id: Uuid, neu: Verlaengerung) -> Result<()> {
    match neu {
        Verlaengerung::Unbefristet => {
            sqlx::query(
                "UPDATE versorgungsvertraege
                 SET vertragsende = NULL,
                     auto_renewal = FALSE,
                     kuendigungsfrist_monate = LEAST(kuendigungsfrist_monate, 1),
                     updated_at = now()
                 WHERE id = $1 AND auto_renewal = TRUE AND status = 'AKTIV'",
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
        Verlaengerung::Befristet(ende) => {
            sqlx::query(
                "UPDATE versorgungsvertraege
                 SET vertragsende = $2, updated_at = now()
                 WHERE id = $1 AND auto_renewal = TRUE AND status = 'AKTIV'",
            )
            .bind(id)
            .bind(ende)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

// ── Expiry monitoring ─────────────────────────────────────────────────────────

/// A contract whose term or price guarantee runs out soon.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExpiringVertragRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub vertrags_nr: String,
    pub status: String,
    pub kundentyp: String,
    pub vertragsart: String,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub preisgarantie_bis: Option<Date>,
    pub bundle_code: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub auto_renewal: bool,
    /// The earlier of `vertragsende` and `preisgarantie_bis` — the date the
    /// notice is about, and the value the once-per-date guard compares against.
    pub faellig_am: Date,
}

/// Contracts whose `vertragsende` or `preisgarantie_bis` falls within
/// `within_days`.
///
/// `only_unnotified` restricts the result to the ones whose notice has not gone
/// out for that date yet — what the worker needs. The read-only endpoint passes
/// `false` and sees the whole window.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_expiring_vertraege(
    pool: &PgPool,
    tenant: &str,
    within_days: i64,
    only_unnotified: bool,
) -> Result<Vec<ExpiringVertragRow>> {
    let today = mako_fristen::heute();
    let cutoff = today + time::Duration::days(within_days);
    Ok(sqlx::query_as::<_, ExpiringVertragRow>(
        r"SELECT id, kunden_id, vertrags_nr, status, kundentyp, vertragsart,
                 vertragsbeginn, vertragsende, preisgarantie_bis,
                 bundle_code, standort_bezeichnung, auto_renewal,
                 LEAST(
                     COALESCE(vertragsende, 'infinity'::DATE),
                     COALESCE(preisgarantie_bis, 'infinity'::DATE)
                 ) AS faellig_am
          FROM versorgungsvertraege
          WHERE tenant = $1
            AND status IN ('AKTIV', 'GEKÜNDIGT')
            AND (
                  (vertragsende IS NOT NULL AND vertragsende BETWEEN $2 AND $3)
               OR (preisgarantie_bis IS NOT NULL AND preisgarantie_bis BETWEEN $2 AND $3)
            )
            AND (NOT $4 OR ablauf_notif_fuer IS DISTINCT FROM LEAST(
                     COALESCE(vertragsende, 'infinity'::DATE),
                     COALESCE(preisgarantie_bis, 'infinity'::DATE)
                 ))
          ORDER BY faellig_am",
    )
    .bind(tenant)
    .bind(today)
    .bind(cutoff)
    .bind(only_unnotified)
    .fetch_all(pool)
    .await?)
}

/// Record that the expiry notice for `faellig_am` went out.
///
/// Without this the daily worker re-derived the same expiry every morning and
/// the ERP received thirty copies of one notice.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn mark_ablauf_notified(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    faellig_am: Date,
) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege
         SET ablauf_notif_fuer = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(faellig_am)
    .execute(executor)
    .await?;
    Ok(())
}

// ── Stuck MaKo registrations ──────────────────────────────────────────────────

/// A Lieferstelle the Lieferant has registered at the Netzbetreiber and that
/// is not answered yet. Whether it is late is makod's to say — the deadline its
/// workflow registered for the MaLo — so this carries no age and no threshold.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, sqlx::FromRow)]
pub struct Anmeldung {
    /// The customer's id.
    pub kunden_id: String,
    /// The customer's name, as `search_kunden` shows it.
    pub name: Option<String>,
    /// The Kundennummer, if one was assigned.
    pub kunden_nr: Option<String>,
    /// The Marktlokation registered.
    pub malo_id: String,
    /// `STROM`, `GAS`, …
    pub sparte: String,
    /// The Versorgungsvertrag's number.
    pub vertrags_nr: String,
    /// The makod process carrying the Anmeldung, once makod accepted it.
    pub mako_process_id: Option<String>,
}

/// Who supplies a MaLo, as a worklist row names it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, sqlx::FromRow)]
pub struct MaloKunde {
    /// The Marktlokation asked about.
    pub malo_id: String,
    /// The customer's id.
    pub kunden_id: String,
    /// The customer's name, as `search_kunden` shows it.
    pub name: Option<String>,
    /// The Kundennummer, if one was assigned.
    pub kunden_nr: Option<String>,
    /// `STROM`, `GAS`, …
    pub sparte: String,
    /// The Vertragskomponente's status (`ANGEMELDET`, `AKTIV`, …).
    pub status: String,
}

/// For each of `malos`, the customer of its latest Vertragskomponente in this
/// tenant — one statement for a page of worklist rows, never one per row. A
/// MaLo with no component is absent.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn kunden_zu_malos(
    pool: &PgPool,
    tenant: &str,
    malos: &[String],
) -> Result<Vec<MaloKunde>> {
    Ok(sqlx::query_as::<_, MaloKunde>(
        r"SELECT DISTINCT ON (k.malo_id)
                 k.malo_id, kd.id::TEXT AS kunden_id,
                 COALESCE(
                   NULLIF(TRIM(CONCAT_WS(' ',
                     COALESCE(kd.person->>'vorname', kd.geschaeftspartner->>'vorname'),
                     COALESCE(kd.person->>'nachname', kd.geschaeftspartner->>'nachname'))), ''),
                   kd.geschaeftspartner->>'name1') AS name,
                 kd.kunden_nr, k.sparte, k.status
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          JOIN kunden kd ON kd.id = v.kunden_id
          WHERE k.tenant = $1 AND k.malo_id = ANY($2)
          ORDER BY k.malo_id, k.lieferbeginn DESC, k.id",
    )
    .bind(tenant)
    .bind(malos)
    .fetch_all(pool)
    .await?)
}

/// One page of [`list_anmeldungen`] and how many there are in all.
#[derive(Debug, Clone)]
pub struct AnmeldungenSeite {
    /// At most `limit`, by customer name.
    pub anmeldungen: Vec<Anmeldung>,
    /// Every Lieferstelle in `ANGEMELDET`, however many the page holds.
    pub gesamt: i64,
}

#[derive(sqlx::FromRow)]
struct AnmeldungRow {
    #[sqlx(flatten)]
    anmeldung: Anmeldung,
    gesamt: i64,
}

/// Every Lieferstelle in `ANGEMELDET` — with `ohne_prozess`, only those no
/// makod process carries yet — by customer name: the first `limit`
/// and the total, counted in the same statement so the two cannot disagree.
/// The total rides on every row, so `limit` must be at least 1.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_anmeldungen(
    pool: &PgPool,
    tenant: &str,
    ohne_prozess: bool,
    limit: i64,
) -> Result<AnmeldungenSeite> {
    let rows = sqlx::query_as::<_, AnmeldungRow>(
        r"SELECT kd.id::TEXT AS kunden_id,
                 COALESCE(
                   NULLIF(TRIM(CONCAT_WS(' ',
                     COALESCE(kd.person->>'vorname', kd.geschaeftspartner->>'vorname'),
                     COALESCE(kd.person->>'nachname', kd.geschaeftspartner->>'nachname'))), ''),
                   kd.geschaeftspartner->>'name1') AS name,
                 kd.kunden_nr, k.malo_id, k.sparte, v.vertrags_nr, k.mako_process_id,
                 COUNT(*) OVER () AS gesamt
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          JOIN kunden kd ON kd.id = v.kunden_id
          WHERE k.tenant = $1 AND k.status = 'ANGEMELDET' AND k.malo_id IS NOT NULL
            AND (NOT $3 OR k.mako_process_id IS NULL)
          ORDER BY name NULLS LAST, k.malo_id
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .bind(ohne_prozess)
    .fetch_all(pool)
    .await?;
    Ok(AnmeldungenSeite {
        gesamt: rows.first().map_or(0, |r| r.gesamt),
        anmeldungen: rows.into_iter().map(|r| r.anmeldung).collect(),
    })
}
