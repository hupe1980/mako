//! PostgreSQL persistence for `mabis-syncd` submission runs.

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Record returned from DB for a submission run.
#[derive(Debug, sqlx::FromRow)]
pub struct SubmissionRunRow {
    pub id: Uuid,
    pub bilanzierungsgebiet_id: String,
    pub period_from: Date,
    pub period_to: Date,
    /// Ascending version within the (Bilanzierungsgebiet, month) key.
    pub version: OffsetDateTime,
    /// `BKA` or `KBKA`.
    pub abrechnungslauf: String,
    /// `ERSTAUFSCHLAG` or `CLEARING`.
    pub phase: String,
    /// Assigned by the BIKO; `None` until an IFTSTA Datenstatus arrives.
    pub datenstatus: Option<String>,
    pub datenstatus_at: Option<OffsetDateTime>,
    /// The run this one corrects, if any.
    pub corrects_run_id: Option<Uuid>,
    pub sender_mp_id: String,
    pub receiver_mp_id: String,
    pub malo_count: i32,
    pub interval_count: i32,
    pub total_kwh: Option<String>,
    pub has_substituted: bool,
    pub status: String,
    pub triggered_at: OffsetDateTime,
    pub submitted_at: Option<OffsetDateTime>,
    pub acked_at: Option<OffsetDateTime>,
    pub message_ref: Option<String>,
    pub process_id: Option<Uuid>,
    pub error_msg: Option<String>,
    pub attempt_count: i32,
    pub tenant: String,
}

/// Parameters for creating a new submission run.
pub struct InsertRunParams<'a> {
    pub bilanzierungsgebiet_id: &'a str,
    pub period_from: Date,
    pub period_to: Date,
    /// Ascending version (§3.8.2), truncated to whole seconds by the caller so
    /// that it survives the second-precision wire format unchanged.
    pub version: OffsetDateTime,
    /// Which settlement run the submission belongs to.
    pub abrechnungslauf: Abrechnungslauf,
    /// Phase the submission is made in, which decides the Datenstatus the BIKO
    /// will assign.
    pub phase: SubmissionPhase,
    /// The run being corrected, when this submission answers a negative
    /// Prüfmitteilung.
    pub corrects_run_id: Option<Uuid>,
    pub sender_mp_id: &'a str,
    pub receiver_mp_id: &'a str,
    pub tenant: &'a str,
}

/// Settlement run a submission belongs to (BK6-24-174 Anlage 3 §3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abrechnungslauf {
    /// Ordinary Bilanzkreisabrechnung.
    Bka,
    /// Korrekturbilanzkreisabrechnung, which runs after the BKA closes.
    Kbka,
}

impl Abrechnungslauf {
    /// Wire/DB spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bka => "BKA",
            Self::Kbka => "KBKA",
        }
    }
}

/// Phase of the settlement calendar a submission falls in (§3.10, Tabelle 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionPhase {
    /// Erstaufschlag — a new version is assigned `Abrechnungsdaten` directly.
    Erstaufschlag,
    /// Clearingphase — a new version is `Prüfdaten` until a positive
    /// Prüfmitteilung promotes it.
    Clearing,
}

impl SubmissionPhase {
    /// Wire/DB spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Erstaufschlag => "ERSTAUFSCHLAG",
            Self::Clearing => "CLEARING",
        }
    }
}

/// Create a new submission run in `pending` status.
pub async fn insert_run(pool: &PgPool, p: InsertRunParams<'_>) -> Result<Uuid, sqlx::Error> {
    // `version` is supplied rather than defaulted, because it must be truncated
    // to whole seconds: MSCONS SG6 DTM+293 carries no more, and a stored value
    // with microseconds can never be matched again when the BIKO echoes it back.
    // A resubmission for the same period is a correction, so it takes a new
    // version rather than replacing the old row.
    let row = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO submission_runs
         (bilanzierungsgebiet_id, period_from, period_to, version,
          abrechnungslauf, phase, corrects_run_id,
          sender_mp_id, receiver_mp_id, tenant)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
         RETURNING id",
    )
    .bind(p.bilanzierungsgebiet_id)
    .bind(p.period_from)
    .bind(p.period_to)
    .bind(p.version)
    .bind(p.abrechnungslauf.as_str())
    .bind(p.phase.as_str())
    .bind(p.corrects_run_id)
    .bind(p.sender_mp_id)
    .bind(p.receiver_mp_id)
    .bind(p.tenant)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Update run status and aggregation result after successful aggregation.
pub async fn update_run_aggregated(
    pool: &PgPool,
    id: Uuid,
    malo_count: i32,
    interval_count: i32,
    total_kwh: &Decimal,
    has_substituted: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submission_runs
            SET status = 'submitted',
                malo_count = $2,
                interval_count = $3,
                total_kwh = $4,
                has_substituted = $5,
                submitted_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(malo_count)
    .bind(interval_count)
    .bind(total_kwh.to_string())
    .bind(has_substituted)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a run as acknowledged by BIKO.
pub async fn mark_acked(
    pool: &PgPool,
    id: Uuid,
    message_ref: &str,
    process_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submission_runs
            SET status = 'acked', acked_at = now(), message_ref = $2, process_id = $3
          WHERE id = $1",
    )
    .bind(id)
    .bind(message_ref)
    .bind(process_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a run as failed with an error message.
/// Mark a run failed inside the caller's transaction, returning the new
/// `attempt_count` so the failure event can carry it.
///
/// Takes a connection rather than the pool because the failure row and the
/// `de.mabis.submission.failed` outbox record must commit together: a failure
/// nothing announces reads as a submission still pending, and an announcement
/// of a failure that was never recorded is noise an operator cannot act on.
pub async fn mark_failed(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    error_msg: &str,
) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE submission_runs
            SET status = 'failed', error_msg = $2, attempt_count = attempt_count + 1
          WHERE id = $1
          RETURNING attempt_count",
    )
    .bind(id)
    .bind(error_msg)
    .fetch_one(conn)
    .await
}

/// Log a MaLo contribution to a submission run.
pub async fn insert_malo_log(
    pool: &PgPool,
    run_id: Uuid,
    malo_id: &str,
    interval_count: i32,
    total_kwh: &Decimal,
    has_gaps: bool,
    substituted_count: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO submission_malo_log
         (run_id, malo_id, interval_count, total_kwh, has_gaps, substituted_count)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(run_id)
    .bind(malo_id)
    .bind(interval_count)
    .bind(total_kwh.to_string())
    .bind(has_gaps)
    .bind(substituted_count)
    .execute(pool)
    .await?;
    Ok(())
}

/// List recent submission runs (latest first).
pub async fn list_runs(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> Result<Vec<SubmissionRunRow>, sqlx::Error> {
    sqlx::query_as::<_, SubmissionRunRow>(
        "SELECT * FROM submission_runs WHERE tenant = $1 ORDER BY triggered_at DESC LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// List failed runs, newest first — the MCP surface's triage view.
pub async fn list_failed_runs(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> Result<Vec<SubmissionRunRow>, sqlx::Error> {
    sqlx::query_as::<_, SubmissionRunRow>(
        "SELECT * FROM submission_runs
          WHERE tenant = $1 AND status = 'failed'
          ORDER BY triggered_at DESC LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// List runs in `pending` or `failed` status (retry candidates).
pub async fn list_pending_runs(
    pool: &PgPool,
    tenant: &str,
) -> Result<Vec<SubmissionRunRow>, sqlx::Error> {
    sqlx::query_as::<_, SubmissionRunRow>(
        "SELECT * FROM submission_runs
          WHERE tenant = $1 AND status IN ('pending','failed') AND attempt_count < 3
          ORDER BY triggered_at ASC",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
}

/// Get submission run by ID.
pub async fn get_run(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> Result<Option<SubmissionRunRow>, sqlx::Error> {
    sqlx::query_as::<_, SubmissionRunRow>(
        "SELECT * FROM submission_runs WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
}

// ── Inbound BIKO responses ────────────────────────────────────────────────────

/// Datenstatus values the BIKO may assign (BK6-24-174 Anlage 3 §3.8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datenstatus {
    /// Received but not yet accepted for settlement.
    Pruefdaten,
    /// Accepted for the ordinary BKA.
    Abrechnungsdaten,
    /// Accepted for the Korrekturbilanzkreisabrechnung.
    AbrechnungsdatenKbka,
    /// Settled in the BKA.
    AbgerechneteDaten,
    /// Settled in the KBKA.
    AbgerechneteDatenKbka,
}

impl Datenstatus {
    /// DB spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pruefdaten => "PRUEFDATEN",
            Self::Abrechnungsdaten => "ABRECHNUNGSDATEN",
            Self::AbrechnungsdatenKbka => "ABRECHNUNGSDATEN_KBKA",
            Self::AbgerechneteDaten => "ABGERECHNETE_DATEN",
            Self::AbgerechneteDatenKbka => "ABGERECHNETE_DATEN_KBKA",
        }
    }

    /// Parse the value carried in IFTSTA SG7 STS+Z04.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim().to_uppercase().replace([' ', '-'], "_").as_str() {
            "PRUEFDATEN" | "PRÜFDATEN" => Some(Self::Pruefdaten),
            "ABRECHNUNGSDATEN" => Some(Self::Abrechnungsdaten),
            "ABRECHNUNGSDATEN_KBKA" => Some(Self::AbrechnungsdatenKbka),
            "ABGERECHNETE_DATEN" => Some(Self::AbgerechneteDaten),
            "ABGERECHNETE_DATEN_KBKA" => Some(Self::AbgerechneteDatenKbka),
            _ => None,
        }
    }

    /// `true` when a version carrying this status is used for settlement.
    ///
    /// §3.8.3: settlement takes the highest version with `Abrechnungsdaten` or
    /// `Abrechnungsdaten KBKA`.
    #[must_use]
    pub fn settles(self) -> bool {
        matches!(self, Self::Abrechnungsdaten | Self::AbrechnungsdatenKbka)
    }
}

/// Record the Datenstatus the BIKO assigned to a submitted version.
///
/// Keyed on the 3-tuple the IFTSTA message carries — (MaBiS-Zählpunkt,
/// Betrachtungszeitraum, Version) — rather than on a local run id, so a status
/// for a version this instance did not send is still applied.
///
/// # Errors
///
/// Propagates database errors.
pub async fn record_datenstatus(
    pool: &PgPool,
    tenant: &str,
    bilanzierungsgebiet_id: &str,
    period_from: Date,
    period_to: Date,
    version: OffsetDateTime,
    status: Datenstatus,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE submission_runs
            SET datenstatus = $1, datenstatus_at = now()
          WHERE tenant = $2 AND bilanzierungsgebiet_id = $3
            AND period_from = $4 AND period_to = $5 AND version = $6",
    )
    .bind(status.as_str())
    .bind(tenant)
    .bind(bilanzierungsgebiet_id)
    .bind(period_from)
    .bind(period_to)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// The version that currently settles a period, if any.
///
/// §3.8.3: the highest version carrying `Abrechnungsdaten` or
/// `Abrechnungsdaten KBKA`. Returns `None` while every submitted version is
/// still `Prüfdaten` — the period has been filed but nothing settles yet.
///
/// # Errors
///
/// Propagates database errors.
pub async fn settling_version(
    pool: &PgPool,
    tenant: &str,
    bilanzierungsgebiet_id: &str,
    period_from: Date,
    period_to: Date,
) -> Result<Option<(Uuid, OffsetDateTime)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, version FROM submission_runs
          WHERE tenant = $1 AND bilanzierungsgebiet_id = $2
            AND period_from = $3 AND period_to = $4
            AND datenstatus IN ('ABRECHNUNGSDATEN', 'ABRECHNUNGSDATEN_KBKA')
          ORDER BY version DESC
          LIMIT 1",
    )
    .bind(tenant)
    .bind(bilanzierungsgebiet_id)
    .bind(period_from)
    .bind(period_to)
    .fetch_optional(pool)
    .await
}

/// Record an inbound Prüfmitteilung (IFTSTA PID 21000/21001).
///
/// Takes a connection so a negative Prüfmitteilung's record and the
/// `de.mabis.korrekturbedarf.opened` outbox row commit in one transaction.
///
/// # Errors
///
/// Propagates database errors.
#[allow(clippy::too_many_arguments)]
pub async fn record_pruefmitteilung(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    bilanzierungsgebiet_id: &str,
    period_from: Date,
    period_to: Date,
    version: OffsetDateTime,
    positiv: bool,
    sender_mp_id: &str,
    pid: i32,
    begruendung: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO pruefmitteilung
             (run_id, bilanzierungsgebiet_id, period_from, period_to, version,
              positiv, sender_mp_id, pid, begruendung, tenant)
         VALUES (
             (SELECT id FROM submission_runs
               WHERE tenant = $9 AND bilanzierungsgebiet_id = $1
                 AND period_from = $2 AND period_to = $3 AND version = $4),
             $1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING id",
    )
    .bind(bilanzierungsgebiet_id)
    .bind(period_from)
    .bind(period_to)
    .bind(version)
    .bind(positiv)
    .bind(sender_mp_id)
    .bind(pid)
    .bind(begruendung)
    .bind(tenant)
    .fetch_one(conn)
    .await
}

/// Negative Prüfmitteilungen with no correcting submission yet.
///
/// §9.8.1: a negative Prüfmitteilung signals Korrekturbedarf, and the ÜNB
/// answers it with a corrected BG-SZR under a higher version. An entry here is
/// an unmet obligation, not a historical record.
///
/// # Errors
///
/// Propagates database errors.
pub async fn open_korrekturbedarf(
    pool: &PgPool,
    tenant: &str,
) -> Result<Vec<(Uuid, String, Date, Date, OffsetDateTime)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, bilanzierungsgebiet_id, period_from, period_to, version
           FROM pruefmitteilung
          WHERE tenant = $1 AND NOT positiv AND corrected_by_run_id IS NULL
          ORDER BY received_at ASC",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
}

/// Close the Korrekturbedarf raised against `corrected_run_id`.
///
/// §9.8.1: the corrected BG-SZR *is* the answer to the negative Prüfmitteilung.
/// Without this the obligation stays open on `/korrekturbedarf` even after the
/// correction has been filed and acked.
///
/// Returns the number of Prüfmitteilungen closed.
///
/// # Errors
///
/// Propagates database errors.
pub async fn close_korrekturbedarf(
    pool: &PgPool,
    corrected_run_id: Uuid,
    correcting_run_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE pruefmitteilung
            SET corrected_by_run_id = $2
          WHERE run_id = $1 AND NOT positiv AND corrected_by_run_id IS NULL",
    )
    .bind(corrected_run_id)
    .bind(correcting_run_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// `true` when a run for this period already exists that has not failed.
///
/// The scheduler wakes every 5 minutes and its due-date test is true for the
/// whole `run_hour`, so without this it filed the same binding Summenzeitreihe
/// a dozen times over.
///
/// # Errors
///
/// Propagates database errors.
pub async fn has_live_run_for_period(
    pool: &PgPool,
    tenant: &str,
    bilanzierungsgebiet_id: &str,
    period_from: Date,
    period_to: Date,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM submission_runs
              WHERE tenant = $1 AND bilanzierungsgebiet_id = $2
                AND period_from = $3 AND period_to = $4
                AND status <> 'failed'
         )",
    )
    .bind(tenant)
    .bind(bilanzierungsgebiet_id)
    .bind(period_from)
    .bind(period_to)
    .fetch_one(pool)
    .await
}
