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
    let today = time::OffsetDateTime::now_utc().date();
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
pub async fn mark_auto_renewal_notified(pool: &PgPool, id: Uuid, vertragsende: Date) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege
         SET autoerneuerung_notif_fuer = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(vertragsende)
    .execute(pool)
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
    let today = time::OffsetDateTime::now_utc().date();
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
pub async fn mark_ablauf_notified(pool: &PgPool, id: Uuid, faellig_am: Date) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege
         SET ablauf_notif_fuer = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(faellig_am)
    .execute(pool)
    .await?;
    Ok(())
}

// ── Stuck MaKo registrations ──────────────────────────────────────────────────

/// A component whose registration has not been answered.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct StuckKomponenteRow {
    pub komp_id: Uuid,
    pub vertrag_id: Uuid,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub lf_mp_id: String,
    pub nb_mp_id: Option<String>,
    pub status: String,
    pub mako_process_id: Option<String>,
    pub angemeldet_since: time::OffsetDateTime,
    pub days_stuck: i64,
}

/// Components registered at the NB but still unanswered after `threshold_days`.
///
/// GPKE gives the NB a bounded time to answer a Lieferbeginn; past it the LF
/// has to chase it. Callers pass the Sparte's own threshold.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_stuck_komponents(
    pool: &PgPool,
    tenant: &str,
    threshold_days: i64,
) -> Result<Vec<StuckKomponenteRow>> {
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(threshold_days);
    Ok(sqlx::query_as::<_, StuckKomponenteRow>(
        r"SELECT k.id AS komp_id, k.vertrag_id, k.sparte, k.malo_id,
                 k.lf_mp_id, k.nb_mp_id, k.status, k.mako_process_id,
                 k.updated_at AS angemeldet_since,
                 EXTRACT(EPOCH FROM (now() - k.updated_at))::BIGINT / 86400 AS days_stuck
          FROM vertragskomponenten k
          WHERE k.tenant = $1
            AND k.status = 'ANGEMELDET'
            AND k.updated_at < $2
          ORDER BY k.updated_at ASC",
    )
    .bind(tenant)
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}
