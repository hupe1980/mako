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
    pub iftsta_ref: Option<String>,
    pub iftsta_dispatched_at: Option<time::OffsetDateTime>,
    pub tenant: String,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

// ── SperrStats ────────────────────────────────────────────────────────────────

/// Aggregate statistics for Sperrung orders, used by the sperrd MCP and monitoring.
#[derive(Debug, Serialize)]
pub struct SperrStats {
    /// Total orders for this tenant.
    pub total: i64,
    /// Orders awaiting field execution.
    pub pending: i64,
    /// Successfully executed orders (IFTSTA 21039 dispatched).
    pub executed: i64,
    /// Failed field executions (operator escalation required).
    pub failed: i64,
    /// Cancelled orders.
    pub cancelled: i64,
    /// Pending orders whose `planned_date` is in the past (overdue).
    ///
    /// BK6-22-024: Sperrung must be executed within 2 Werktage of Bestelldatum.
    /// Any entry here is a potential compliance violation.
    pub overdue_pending: i64,
    /// Executed orders where the IFTSTA 21039 was NOT yet dispatched.
    ///
    /// A non-zero count means GPKE protocol violations: the LF has not received
    /// execution confirmation.  These must be resolved immediately.
    pub executed_missing_iftsta: i64,
}

// ── create_order_pg ───────────────────────────────────────────────────────────

pub async fn create_order_pg(
    pool: &PgPool,
    tenant: &str,
    req: CreateOrderRequest,
) -> anyhow::Result<Uuid> {
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
        r"INSERT INTO sperr_orders
              (malo_id, lf_mp_id, order_type, process_id, planned_date, tenant)
          VALUES ($1, $2, $3, $4, $5, $6)
          RETURNING id::TEXT",
    )
    .bind(&req.malo_id)
    .bind(&req.lf_mp_id)
    .bind(&req.order_type)
    .bind(&req.process_id)
    .bind(planned_date)
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("insert sperr_order")?;

    let id_str: String = row.try_get("id")?;
    id_str.parse::<Uuid>().context("parse UUID")
}

// ── list_orders_pg ────────────────────────────────────────────────────────────

pub async fn list_orders_pg(
    pool: &PgPool,
    tenant: &str,
    status: Option<&str>,
    malo_id: Option<&str>,
    older_than_hours: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<SperrOrderRow>> {
    sqlx::query_as::<_, SperrOrderRow>(
        r"SELECT id::TEXT, malo_id, lf_mp_id, order_type, process_id,
                 planned_date, status, executed_at, execution_note,
                 fail_reason, iftsta_ref, iftsta_dispatched_at,
                 tenant, created_at, updated_at
          FROM sperr_orders
          WHERE (tenant = $1 OR $1 = '')
            AND ($2::TEXT IS NULL OR status = $2)
            AND ($3::TEXT IS NULL OR malo_id = $3)
            AND ($4::BIGINT IS NULL OR created_at < NOW() - make_interval(hours => $4::INT))
          ORDER BY created_at DESC
          LIMIT $5",
    )
    .bind(tenant)
    .bind(status)
    .bind(malo_id)
    .bind(older_than_hours)
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
                 fail_reason, iftsta_ref, iftsta_dispatched_at,
                 tenant, created_at, updated_at
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
    // Idempotency key prevents double-dispatch on retry.
    let idempotency_key = format!("sperrd-iftsta-{id}");
    let cmd = ForwardCommand {
        command: "gpke.sperrung.bestaetigen".to_owned(),
        marktrolle: None,
        malo_id: Some(malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "lf_mp_id":    lf_mp_id,
            "process_id":  process_id,
            "executed_at": executed_at.map(|t| {
                t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
            }),
            "note": note,
        }),
    };

    let accepted = makod
        .post_command(&idempotency_key, &cmd)
        .await
        .context("dispatch IFTSTA 21039 to makod")?;

    let iftsta_ref = accepted.process_id.to_string();
    let now = time::OffsetDateTime::now_utc();

    let rows = sqlx::query(
        r"UPDATE sperr_orders
          SET status               = 'executed',
              executed_at          = $1,
              execution_note       = $2,
              iftsta_ref           = $3,
              iftsta_dispatched_at = $4,
              updated_at           = now()
          WHERE id = $5 AND status = 'pending'",
    )
    .bind(executed_at)
    .bind(note)
    .bind(&iftsta_ref)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .context("execute_order_pg update")?
    .rows_affected();

    Ok(rows > 0)
}

// ── fail_order_pg ─────────────────────────────────────────────────────────────

/// Mark an order as failed and dispatch IFTSTA 21039 reporting non-execution.
///
/// GPKE BK6-22-024 §5 requires the NB to report the outcome of a Sperrung order
/// — **including a failed attempt**. Without this dispatch the Lieferant's
/// `gpke-sperrung-lf` process hangs until its 24-hour deadline expires and the LF
/// never learns why (meter access denied, safety block, address not found, …).
pub async fn fail_order_pg(
    pool: &PgPool,
    makod: &Arc<MakodClient>,
    id: Uuid,
    reason: &str,
) -> anyhow::Result<bool> {
    // Fetch order details before the status transition so we can address the command.
    let order_row = sqlx::query(
        "SELECT malo_id, lf_mp_id, process_id FROM sperr_orders WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch order for fail")?;

    let Some(order) = order_row else {
        return Ok(false);
    };

    let malo_id: String = order.try_get("malo_id")?;
    let lf_mp_id: String = order.try_get("lf_mp_id")?;
    let process_id: Option<String> = order.try_get("process_id")?;

    let idempotency_key = format!("sperrd-iftsta-fail-{id}");
    let cmd = ForwardCommand {
        command: "gpke.sperrung.fehlgeschlagen".to_owned(),
        marktrolle: None,
        malo_id: Some(malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "lf_mp_id":   lf_mp_id,
            "process_id": process_id,
            "reason":     reason,
        }),
    };

    let accepted = makod
        .post_command(&idempotency_key, &cmd)
        .await
        .context("dispatch IFTSTA 21039 (non-execution) to makod")?;

    let iftsta_ref = accepted.process_id.to_string();
    let now = time::OffsetDateTime::now_utc();

    let rows = sqlx::query(
        r"UPDATE sperr_orders
          SET status               = 'failed',
              fail_reason          = $1,
              iftsta_ref           = $2,
              iftsta_dispatched_at = $3,
              updated_at           = now()
          WHERE id = $4 AND status = 'pending'",
    )
    .bind(reason)
    .bind(&iftsta_ref)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await
    .context("fail_order_pg")?
    .rows_affected();
    Ok(rows > 0)
}

// ── cancel_order_pg ───────────────────────────────────────────────────────────

/// Cancel a pending Sperrung order (operator-initiated; no IFTSTA dispatched).
///
/// Only `pending` orders can be cancelled.  Once `executed` or `failed`,
/// the order is terminal and cannot be cancelled.
pub async fn cancel_order_pg(pool: &PgPool, id: Uuid) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE sperr_orders
          SET status = 'cancelled', updated_at = now()
          WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(pool)
    .await
    .context("cancel_order_pg")?
    .rows_affected();
    Ok(rows > 0)
}

// ── stats_pg ──────────────────────────────────────────────────────────────────

/// Aggregate statistics for Sperrung orders for the given tenant.
///
/// Includes counts of orders by status + overdue pending orders (planned_date < today)
/// + executed orders missing IFTSTA dispatch.  Used by the sperrd MCP and monitoring.
pub async fn stats_pg(pool: &PgPool, tenant: &str) -> anyhow::Result<SperrStats> {
    let row = sqlx::query(
        r"SELECT
              COUNT(*)                                                        AS total,
              COUNT(*) FILTER (WHERE status = 'pending')                     AS pending,
              COUNT(*) FILTER (WHERE status = 'executed')                    AS executed,
              COUNT(*) FILTER (WHERE status = 'failed')                      AS failed,
              COUNT(*) FILTER (WHERE status = 'cancelled')                   AS cancelled,
              COUNT(*) FILTER (
                  WHERE status = 'pending'
                    AND planned_date IS NOT NULL
                    AND planned_date < CURRENT_DATE
              )                                                               AS overdue_pending,
              COUNT(*) FILTER (
                  WHERE status = 'executed'
                    AND iftsta_dispatched_at IS NULL
              )                                                               AS executed_missing_iftsta
          FROM sperr_orders
          WHERE (tenant = $1 OR $1 = '')",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("stats_pg")?;

    Ok(SperrStats {
        total: row.try_get("total").unwrap_or(0),
        pending: row.try_get("pending").unwrap_or(0),
        executed: row.try_get("executed").unwrap_or(0),
        failed: row.try_get("failed").unwrap_or(0),
        cancelled: row.try_get("cancelled").unwrap_or(0),
        overdue_pending: row.try_get("overdue_pending").unwrap_or(0),
        executed_missing_iftsta: row.try_get("executed_missing_iftsta").unwrap_or(0),
    })
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_order_request_fields() {
        let req = CreateOrderRequest {
            malo_id: "51238696780".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            order_type: "sperrung".to_owned(),
            process_id: Some("550e8400-e29b-41d4-a716-446655440000".to_owned()),
            planned_date: Some("2026-07-20".to_owned()),
        };
        assert_eq!(req.malo_id, "51238696780");
        assert_eq!(req.order_type, "sperrung");
        assert!(req.process_id.is_some());
    }

    #[test]
    fn sperrung_order_types_are_valid() {
        // Only 'sperrung' and 'entsperrung' are valid per DB CHECK constraint.
        let valid = ["sperrung", "entsperrung"];
        for t in valid {
            assert!(!t.is_empty(), "order_type must not be empty: {t}");
        }
    }

    #[test]
    fn sperr_stats_default_all_zero() {
        let s = SperrStats {
            total: 0,
            pending: 0,
            executed: 0,
            failed: 0,
            cancelled: 0,
            overdue_pending: 0,
            executed_missing_iftsta: 0,
        };
        assert_eq!(s.total, 0);
        assert_eq!(s.executed_missing_iftsta, 0);
    }

    #[test]
    fn executed_at_parse_roundtrip() {
        use time::format_description::well_known::Rfc3339;
        let ts = "2026-07-14T09:47:00Z";
        let parsed = time::OffsetDateTime::parse(ts, &Rfc3339).unwrap();
        let formatted = parsed.format(&Rfc3339).unwrap();
        // Round-trip should produce the same canonical string (modulo offset).
        let reparsed = time::OffsetDateTime::parse(&formatted, &Rfc3339).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn idempotency_key_format_is_stable() {
        let id = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let key = format!("sperrd-iftsta-{id}");
        assert_eq!(key, "sperrd-iftsta-550e8400-e29b-41d4-a716-446655440000");
    }
}
