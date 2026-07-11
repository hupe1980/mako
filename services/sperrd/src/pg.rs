//! PostgreSQL persistence for `sperrd`.

use anyhow::Context as _;
use mako_markt::makod_client::{ForwardCommand, MakodClient};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

// ── CreateOrderRequest ────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/sperr-orders`.
#[derive(Debug, Deserialize)]
pub struct CreateOrderRequest {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Initiating counterparty MP-ID (the LF who requested the Sperrung).
    pub lf_mp_id: String,
    /// Type: `"sperrung"` (disconnect) or `"entsperrung"` (reconnect).
    pub order_type: String,
    /// `makod` process ID — used to track back to the ORDERS workflow.
    pub process_id: Option<String>,
    /// Planned execution date.
    pub planned_date: Option<String>,
}

// ── SperrOrderRow ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SperrOrderRow {
    pub id: String,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub order_type: String,
    pub process_id: Option<String>,
    pub planned_date: Option<time::Date>,
    pub status: String, // pending | executed | failed | cancelled
    pub executed_at: Option<time::OffsetDateTime>,
    pub execution_note: Option<String>,
    pub fail_reason: Option<String>,
    pub iftsta_ref: Option<String>, // makod IFTSTA 21039 command ID
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

// ── create_order_pg ───────────────────────────────────────────────────────────

pub async fn create_order_pg(pool: &PgPool, req: CreateOrderRequest) -> anyhow::Result<Uuid> {
    let planned_date = req
        .planned_date
        .as_deref()
        .map(|s| {
            use time::format_description::well_known::Iso8601;
            time::Date::parse(s, &Iso8601::DEFAULT)
        })
        .transpose()
        .context("parse planned_date")?;

    let row = sqlx::query(
        r"INSERT INTO sperr_orders (malo_id, lf_mp_id, order_type, process_id, planned_date)
          VALUES ($1, $2, $3, $4, $5)
          RETURNING id::TEXT",
    )
    .bind(&req.malo_id)
    .bind(&req.lf_mp_id)
    .bind(&req.order_type)
    .bind(&req.process_id)
    .bind(planned_date)
    .fetch_one(pool)
    .await
    .context("insert sperr_order")?;

    let id_str: String = row.try_get("id")?;
    id_str.parse::<Uuid>().context("parse UUID")
}

// ── list_orders_pg ────────────────────────────────────────────────────────────

pub async fn list_orders_pg(
    pool: &PgPool,
    status: Option<&str>,
    malo_id: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<SperrOrderRow>> {
    sqlx::query_as::<_, SperrOrderRow>(
        r"SELECT id::TEXT, malo_id, lf_mp_id, order_type, process_id,
                 planned_date, status, executed_at, execution_note,
                 fail_reason, iftsta_ref, created_at, updated_at
          FROM sperr_orders
          WHERE ($1::TEXT IS NULL OR status = $1)
            AND ($2::TEXT IS NULL OR malo_id = $2)
          ORDER BY created_at DESC
          LIMIT $3",
    )
    .bind(status)
    .bind(malo_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_orders_pg")
}

// ── fetch_order_pg ────────────────────────────────────────────────────────────

pub async fn fetch_order_pg(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<SperrOrderRow>> {
    sqlx::query_as::<_, SperrOrderRow>(
        r"SELECT id::TEXT, malo_id, lf_mp_id, order_type, process_id,
                 planned_date, status, executed_at, execution_note,
                 fail_reason, iftsta_ref, created_at, updated_at
          FROM sperr_orders WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch_order_pg")
}

// ── execute_order_pg ─────────────────────────────────────────────────────────

/// Mark an order as executed and dispatch IFTSTA 21039 (Ausführungsbestätigung)
/// to `makod` via the `gpke.sperrung.bestaetigen` command.
///
/// This satisfies GPKE BK6-22-024 §5: the NB must send IFTSTA 21039 after
/// physical Sperrung/Entsperrung execution.
pub async fn execute_order_pg(
    pool: &PgPool,
    makod: &Arc<MakodClient>,
    id: Uuid,
    note: Option<&str>,
    executed_at_str: Option<&str>,
) -> anyhow::Result<bool> {
    let executed_at = if let Some(s) = executed_at_str {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map(Some)
            .context("parse executed_at")?
    } else {
        Some(time::OffsetDateTime::now_utc())
    };

    // Fetch order details for the makod command payload.
    let order_row = sqlx::query(
        "SELECT malo_id, lf_mp_id, process_id FROM sperr_orders WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch order for execute")?;

    let Some(order) = order_row else {
        return Ok(false);
    };

    let malo_id: String = order.try_get("malo_id")?;
    let lf_mp_id: String = order.try_get("lf_mp_id")?;
    let process_id: Option<String> = order.try_get("process_id")?;

    // Dispatch IFTSTA 21039 via makod — mandatory per GPKE BK6-22-024 §5.
    let idempotency_key = format!("sperrd-iftsta-{id}");
    let cmd = ForwardCommand {
        command: "gpke.sperrung.bestaetigen".to_owned(),
        marktrolle: None,
        malo_id: Some(malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "lf_mp_id":    lf_mp_id,
            "process_id":  process_id,
            "executed_at": executed_at.map(|t| t.to_string()),
            "note":        note,
        }),
    };

    let accepted = makod
        .post_command(&idempotency_key, &cmd)
        .await
        .context("dispatch IFTSTA 21039 to makod")?;

    let iftsta_ref = accepted.process_id.to_string();

    let rows = sqlx::query(
        r"UPDATE sperr_orders
          SET status = 'executed',
              executed_at = $1,
              execution_note = $2,
              iftsta_ref = $3,
              updated_at = now()
          WHERE id = $4 AND status = 'pending'",
    )
    .bind(executed_at)
    .bind(note)
    .bind(&iftsta_ref)
    .bind(id)
    .execute(pool)
    .await
    .context("execute_order_pg update")?
    .rows_affected();

    Ok(rows > 0)
}

// ── fail_order_pg ─────────────────────────────────────────────────────────────

pub async fn fail_order_pg(pool: &PgPool, id: Uuid, reason: &str) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE sperr_orders
          SET status = 'failed', fail_reason = $1, updated_at = now()
          WHERE id = $2 AND status = 'pending'",
    )
    .bind(reason)
    .bind(id)
    .execute(pool)
    .await
    .context("fail_order_pg")?
    .rows_affected();
    Ok(rows > 0)
}
