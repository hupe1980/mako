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
    pub version: String,
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
    pub version: &'a str,
    pub sender_mp_id: &'a str,
    pub receiver_mp_id: &'a str,
    pub tenant: &'a str,
}

/// Create a new submission run in `pending` status.
pub async fn insert_run(pool: &PgPool, p: InsertRunParams<'_>) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO submission_runs
         (bilanzierungsgebiet_id, period_from, period_to, version,
          sender_mp_id, receiver_mp_id, tenant)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         RETURNING id",
    )
    .bind(p.bilanzierungsgebiet_id)
    .bind(p.period_from)
    .bind(p.period_to)
    .bind(p.version)
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
pub async fn mark_failed(pool: &PgPool, id: Uuid, error_msg: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submission_runs
            SET status = 'failed', error_msg = $2, attempt_count = attempt_count + 1
          WHERE id = $1",
    )
    .bind(id)
    .bind(error_msg)
    .execute(pool)
    .await?;
    Ok(())
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
