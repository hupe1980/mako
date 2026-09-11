//! The `direct_push_sessions` idempotency key, shared by the four push doors.
//!
//! A push door stores readings and raises CloudEvents a billing recompute hangs
//! off. The readings upsert on their primary key, so a replay costs nothing
//! there; the events do not, so the door has to know whether it is the first
//! request to carry this `session_id`.
//!
//! The key is **claimed before the work and committed after it**. Claiming is
//! the insert itself — `ON CONFLICT DO NOTHING` over the `(tenant, session_id)`
//! primary key, so two simultaneous requests carrying one session id cannot both
//! read "new" and both ingest. A claim that never reaches [`commit`] stays
//! `partial` and the next request takes it over, which keeps a wholly failed
//! batch retryable.
//!
//! `malo_id` is recorded and verified, never part of the key: the table is keyed
//! `(tenant, session_id)`, so a caller reusing one session id for a second
//! Marktlokation is making a mistake the door has to name rather than silently
//! file under the first MaLo.

use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use mako_service::Json;
use serde_json::Value;
use time::OffsetDateTime;

/// Identity of one push session.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Session<'a> {
    pub tenant: &'a str,
    pub session_id: &'a str,
    pub malo_id: &'a str,
    /// `direct_push_sessions.source` — the door the readings arrived through.
    pub source: &'a str,
}

/// What a [`claim`] found.
#[derive(Debug)]
pub(crate) enum Claim {
    /// This request owns the session: do the work, then [`commit`].
    Claimed,
    /// A previous request finished it — replay its recorded result.
    Committed {
        interval_count: i32,
        quality_summary: Option<Value>,
    },
    /// A previous attempt claimed it and never committed; this request takes over.
    Resumed,
    /// The session id is already in use for a different Marktlokation.
    MaloMismatch { recorded: String },
}

/// Claim `session` for this request.
///
/// # Errors
///
/// Returns the `sqlx::Error` unchanged. A failed claim is **not** evidence that
/// the session is new, so a door must refuse rather than ingest.
pub(crate) async fn claim(pool: &sqlx::PgPool, session: Session<'_>) -> Result<Claim, sqlx::Error> {
    let claimed: Option<i32> = sqlx::query_scalar(
        r"INSERT INTO direct_push_sessions (session_id, malo_id, source, status, tenant)
          VALUES ($1, $2, $3, 'partial', $4)
          ON CONFLICT (tenant, session_id) DO NOTHING
          RETURNING 1",
    )
    .bind(session.session_id)
    .bind(session.malo_id)
    .bind(session.source)
    .bind(session.tenant)
    .fetch_optional(pool)
    .await?;
    if claimed.is_some() {
        return Ok(Claim::Claimed);
    }

    let row: (String, String, i32, Option<Value>) = sqlx::query_as(
        r"SELECT malo_id, status, interval_count, quality_summary
            FROM direct_push_sessions
           WHERE tenant = $1 AND session_id = $2",
    )
    .bind(session.tenant)
    .bind(session.session_id)
    .fetch_one(pool)
    .await?;

    let (recorded_malo, status, interval_count, quality_summary) = row;
    if recorded_malo != session.malo_id {
        return Ok(Claim::MaloMismatch {
            recorded: recorded_malo,
        });
    }
    if status == "committed" {
        return Ok(Claim::Committed {
            interval_count,
            quality_summary,
        });
    }
    Ok(Claim::Resumed)
}

/// What [`commit`] records beside the session's identity.
#[derive(Debug, Default)]
pub(crate) struct Committed<'a> {
    pub obis_code: Option<&'a str>,
    pub interval_count: i32,
    pub period_from: Option<OffsetDateTime>,
    pub period_to: Option<OffsetDateTime>,
    pub quality_summary: Option<Value>,
    /// IoT door only — the undecoded uplink frame and its provenance.
    pub raw_payload: Option<&'a str>,
    pub transport: Option<&'a str>,
    pub device_id: Option<&'a str>,
}

/// Mark the session committed, so a replay is recognised as one.
///
/// # Errors
///
/// Returns the `sqlx::Error` unchanged. The readings are already stored when
/// this runs, but a lost commit breaks the idempotency the door promises: the
/// next replay would re-raise every CloudEvent. It is a failure of the request,
/// not a footnote to it.
pub(crate) async fn commit(
    pool: &sqlx::PgPool,
    session: Session<'_>,
    c: Committed<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"UPDATE direct_push_sessions
             SET status          = 'committed',
                 obis_code       = COALESCE($3, obis_code),
                 interval_count  = $4,
                 period_from     = $5,
                 period_to       = $6,
                 quality_summary = COALESCE($7, quality_summary),
                 raw_payload     = COALESCE($8, raw_payload),
                 transport       = COALESCE($9, transport),
                 device_id       = COALESCE($10, device_id)
           WHERE tenant = $1 AND session_id = $2",
    )
    .bind(session.tenant)
    .bind(session.session_id)
    .bind(c.obis_code)
    .bind(c.interval_count)
    .bind(c.period_from)
    .bind(c.period_to)
    .bind(c.quality_summary)
    .bind(c.raw_payload)
    .bind(c.transport)
    .bind(c.device_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The response a door owes when it cannot establish whether a session is new.
pub(crate) fn unverifiable(malo_id: &str, door: &str, e: &sqlx::Error) -> Response {
    tracing::error!(malo_id, door, error = %e, "edmd: push session claim failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "could not verify whether this session was already committed",
        })),
    )
        .into_response()
}

/// The response a door owes when the session id belongs to another MaLo.
pub(crate) fn malo_mismatch(session_id: &str, asked: &str, recorded: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "session_id already in use for a different Marktlokation",
            "session_id": session_id,
            "requested_malo_id": asked,
            "recorded_malo_id": recorded,
        })),
    )
        .into_response()
}

/// The response a door owes when the commit record could not be written.
///
/// The readings landed; what is lost is the evidence that they did, so a replay
/// would ingest them again and re-raise the events. Reporting success would make
/// the door's idempotency claim untrue.
pub(crate) fn commit_failed(malo_id: &str, door: &str, e: &sqlx::Error) -> Response {
    tracing::error!(malo_id, door, error = %e, "edmd: push session commit failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "error": "readings were stored but the session could not be committed; \
                      retry with the same session_id",
        })),
    )
        .into_response()
}
