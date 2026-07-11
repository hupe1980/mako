//! PostgreSQL persistence for `netzbilanzd`.

use anyhow::Context as _;
use invoic_checker::check::CheckOutcome;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use time::Date;
use uuid::Uuid;

// ── upsert_draft ─────────────────────────────────────────────────────────────

/// Insert a new invoice draft.  Returns the generated UUID.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_draft(
    pool: &PgPool,
    malo_id: &str,
    nb_mp_id: &str,
    lf_mp_id: &str,
    pid: i32,
    period_from: Date,
    period_to: Date,
    rechnung: serde_json::Value,
    total_eur: Decimal,
    check_outcome: CheckOutcome,
) -> anyhow::Result<Uuid> {
    let outcome_str = match check_outcome {
        CheckOutcome::Ok => "Ok",
        CheckOutcome::Warn => "Warn",
        CheckOutcome::Dispute => "Dispute",
    };

    // Store total as integer in units of 10⁻⁵ EUR for lossless round-trip.
    let total_i64: i64 = (total_eur * Decimal::from(100_000_i64))
        .round()
        .to_string()
        .parse()
        .context("total_eur to i64")?;

    let row = sqlx::query(
        r"INSERT INTO invoice_drafts
              (malo_id, nb_mp_id, lf_mp_id, pid, period_from, period_to,
               rechnung, gross_eur_units, check_outcome, status)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'draft')
          RETURNING id::TEXT",
    )
    .bind(malo_id)
    .bind(nb_mp_id)
    .bind(lf_mp_id)
    .bind(pid)
    .bind(period_from)
    .bind(period_to)
    .bind(&rechnung)
    .bind(total_i64)
    .bind(outcome_str)
    .fetch_one(pool)
    .await
    .context("insert invoice_draft")?;

    let id_str: String = row.try_get("id")?;
    id_str.parse::<Uuid>().context("parse UUID")
}

// ── list_drafts_pg ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DraftRow {
    pub id: String,
    pub malo_id: String,
    pub nb_mp_id: String,
    pub lf_mp_id: String,
    pub pid: i32,
    pub period_from: Date,
    pub period_to: Date,
    pub rechnung: serde_json::Value,
    pub gross_eur_units: i64,
    pub check_outcome: Option<String>,
    pub status: String,
    pub dispatch_ref: Option<String>,
    pub reject_reason: Option<String>,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

/// List drafts with optional filters.
pub async fn list_drafts_pg(
    pool: &PgPool,
    status: Option<&str>,
    malo_id: Option<&str>,
    nb_mp_id: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, created_at, updated_at
          FROM invoice_drafts
          WHERE ($1::TEXT IS NULL OR status = $1)
            AND ($2::TEXT IS NULL OR malo_id = $2)
            AND ($3::TEXT IS NULL OR nb_mp_id = $3)
          ORDER BY created_at DESC
          LIMIT $4",
    )
    .bind(status)
    .bind(malo_id)
    .bind(nb_mp_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_drafts_pg")
}

// ── fetch_draft ───────────────────────────────────────────────────────────────

/// Fetch a single draft by UUID.
pub async fn fetch_draft(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, created_at, updated_at
          FROM invoice_drafts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch_draft")
}

// ── approve_and_dispatch ─────────────────────────────────────────────────────

/// Validate the draft via `invoic-checker` and dispatch it to `makod` as
/// INVOIC 31001/31002/31005/31009.  Updates status to `dispatched`.
///
/// Blocked when `check_outcome == 'Dispute'` — an NB must never send an INVOIC
/// that fails its own plausibility checks.
pub async fn approve_and_dispatch(
    pool: &PgPool,
    makod: &Arc<mako_markt::makod_client::MakodClient>,
    id: Uuid,
) -> anyhow::Result<String> {
    // Verify draft exists and is in draft state.
    let row = sqlx::query(
        "SELECT status, check_outcome, malo_id, nb_mp_id, lf_mp_id, pid, rechnung
         FROM invoice_drafts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch for dispatch")?;

    let Some(row) = row else {
        anyhow::bail!("draft not found: {id}");
    };

    let status: String = row.try_get("status")?;
    let outcome: Option<String> = row.try_get("check_outcome")?;
    let malo_id: String = row.try_get("malo_id")?;
    let nb_mp_id: String = row.try_get("nb_mp_id")?;
    let lf_mp_id: String = row.try_get("lf_mp_id")?;
    let pid: i32 = row.try_get("pid")?;
    let rechnung: serde_json::Value = row.try_get("rechnung")?;

    if status != "draft" {
        anyhow::bail!("draft {id} is already {status}");
    }
    if outcome.as_deref() == Some("Dispute") {
        anyhow::bail!(
            "cannot dispatch draft {id}: invoic-checker reported Dispute — fix the invoice before dispatching"
        );
    }

    // Map PID to makod command name.
    let command = match pid {
        31001 => "gpke.nne.rechnung.stellen",
        31002 => "gpke.mmm.rechnung.stellen",
        31005 => "gpke.nne-gas.rechnung.stellen",
        31009 => "wim.msb-rechnung.stellen",
        _ => anyhow::bail!("unknown billing PID {pid}"),
    };

    // Dispatch to makod — the engine serialises this as EDIFACT INVOIC.
    let idempotency_key = format!("netzbilanzd-invoic-{id}");
    let cmd = mako_markt::makod_client::ForwardCommand {
        command: command.to_owned(),
        marktrolle: Some("NB".to_owned()),
        malo_id: Some(malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "lf_mp_id":  lf_mp_id,
            "nb_mp_id":  nb_mp_id,
            "pid":       pid,
            "rechnung":  rechnung,
        }),
    };

    let accepted = makod
        .post_command(&idempotency_key, &cmd)
        .await
        .map_err(|e| anyhow::anyhow!("dispatch to makod failed: {e}"))?;

    let dispatch_ref = accepted.process_id.to_string();

    sqlx::query(
        "UPDATE invoice_drafts SET status = 'dispatched', dispatch_ref = $1, updated_at = now() WHERE id = $2",
    )
    .bind(&dispatch_ref)
    .bind(id)
    .execute(pool)
    .await
    .context("update dispatch")?;

    Ok(dispatch_ref)
}

// ── reject_draft_pg ───────────────────────────────────────────────────────────

/// Reject a draft with an operator-supplied reason.
pub async fn reject_draft_pg(pool: &PgPool, id: Uuid, reason: &str) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        "UPDATE invoice_drafts SET status = 'rejected', reject_reason = $1, updated_at = now() WHERE id = $2 AND status = 'draft'",
    )
    .bind(reason)
    .bind(id)
    .execute(pool)
    .await
    .context("reject_draft")?
    .rows_affected();
    Ok(rows > 0)
}
