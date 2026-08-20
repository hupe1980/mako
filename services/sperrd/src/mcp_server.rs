//! MCP server for `sperrd` — Sperr-/Entsperrauftrag execution (NB role).
//!
//! **Read-only by construction**, like every MCP surface on this platform: a
//! model drives it, so the mutating decision stays with an operator on the REST
//! API. Cancelling an order is `PUT /api/v1/sperr-orders/{id}/cancel`.
//!
//! ## Tools (4)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_sperr_orders`   | The queue — filter by status, MaLo, or "due today" |
//! | `get_sperr_order`     | One order, with its ORDERS provenance and IFTSTA state |
//! | `get_sperr_stats`     | Counters, including outstanding and stuck IFTSTA |
//! | `list_due_orders`     | Pending orders whose requested execution date has arrived |
//!
//! ## Prompts (2)
//!
//! | Prompt | Description |
//! |---|---|
//! | `execute-sperrung` | Confirming a field execution and its IFTSTA 21039 |
//! | `iftsta-sweep`     | Finding and clearing outstanding Auftragsstatus messages |
//!
//! ## Deadlines
//!
//! GPKE fixes **no execution deadline in Werktagen** for the physical act. The
//! field team works to the Lieferant's own `DTM+203 Ausführungsdatum` or
//! `DTM+469 frühestes Startdatum`. BK6-22-024 §5's 24 wall-clock hours is the
//! deadline for the NB's **ORDRSP**, which `makod` tracks — not this service.

use axum::{
    Router,
    middleware::{self, Next},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::model::OrderStatus;

#[derive(Clone)]
pub struct SperrdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSperrParams {
    /// Filter by status: `pending`, `executed`, `failed`, `cancelled`.
    pub status: Option<String>,
    /// Filter to one Marktlokation.
    pub malo_id: Option<String>,
    /// Only orders whose requested execution date (`DTM+203` or `DTM+469`) has
    /// arrived — the field-dispatch list.
    #[serde(default)]
    pub due: bool,
    /// Maximum results (default 50).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSperrParams {
    /// UUID of the Sperrung order.
    pub id: String,
}

/// Shared handler + router state.
#[derive(Clone)]
pub struct SperrdMcpHandler {
    state: Arc<SperrdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<SperrdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<SperrdMcpHandler>,
}

#[tool_router]
impl SperrdMcpHandler {
    fn new(state: Arc<SperrdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List Sperr-/Entsperraufträge. Filter by status (pending/executed/failed/cancelled), by malo_id, and/or due=true for orders whose requested execution date (ORDERS DTM+203 Ausführungsdatum or DTM+469 frühestes Startdatum) has arrived. GPKE fixes no Werktage deadline for the physical act — the date is the Lieferant's own.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_sperr_orders(
        &self,
        Parameters(p): Parameters<ListSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_orders_pg;
        let status = match p
            .status
            .as_deref()
            .map(str::parse::<OrderStatus>)
            .transpose()
        {
            Ok(s) => s,
            Err(e) => return Err(McpError::invalid_params(e, None)),
        };
        match list_orders_pg(
            &self.state.pool,
            &self.state.tenant,
            status,
            p.malo_id.as_deref(),
            p.due,
            p.limit.unwrap_or(50).clamp(1, 1000),
        )
        .await
        {
            Ok(orders) => {
                let pending = orders
                    .iter()
                    .filter(|o| o.status == OrderStatus::Pending)
                    .count();
                ContentBlock::json(serde_json::json!({
                    "count": orders.len(),
                    "orders": orders,
                    "pending_count": pending,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single Sperrung/Entsperrung order by UUID. Returns execution timestamps, iftsta_ref (makod command ID), iftsta_dispatched_at, and associated ORDERS process_id.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_sperr_order(
        &self,
        Parameters(p): Parameters<GetSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_order_pg;
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match fetch_order_pg(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(order)) => ContentBlock::json(serde_json::to_value(order).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("order {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Aggregate counters: pending/executed/failed/cancelled, overdue_pending (past the requested execution date), iftsta_outstanding (terminal orders whose IFTSTA 21039 has not reached the Lieferant — their gpke-sperrung-lf process cannot close) and iftsta_stuck (of those, the ones past the retry budget, which need a human).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_sperr_stats(&self) -> Result<CallToolResult, McpError> {
        use crate::pg::stats_pg;
        match stats_pg(&self.state.pool, &self.state.tenant).await {
            Ok(s) => ContentBlock::json(serde_json::json!({
                "total":              s.total,
                "pending":            s.pending,
                "executed":           s.executed,
                "failed":             s.failed,
                "cancelled":          s.cancelled,
                "overdue_pending":    s.overdue_pending,
                "iftsta_outstanding": s.iftsta_outstanding,
                "iftsta_stuck":       s.iftsta_stuck,
                "health": {
                    // Outstanding on its own is normal for a few seconds after an
                    // execution — the retry worker is mid-flight. Stuck is not:
                    // it means the retry budget ran out.
                    "iftsta_ok": s.iftsta_stuck == 0,
                    "note": if s.iftsta_stuck > 0 {
                        "ACTION REQUIRED: iftsta_stuck orders exhausted the retry budget. \
                         The Lieferant has not been told the outcome. Check whether the \
                         makod gpke-sperrung process for each exists and is in \
                         ValidationPassed."
                    } else if s.iftsta_outstanding > 0 {
                        "IFTSTA dispatches in flight; the retry worker is still trying."
                    } else {
                        "Every terminal order has been reported to its Lieferant."
                    },
                },
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "The field-dispatch list: pending orders whose requested execution date has arrived. `soll_am` is the Lieferant's DTM+203 Ausführungsdatum (a date they require) or DTM+469 frühestes Startdatum (execute at the next opportunity, not before). `treffpunkt` is where the technician goes.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_due_orders(&self) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let rows = sqlx::query(
            r"SELECT id::TEXT, malo_id, lf_mp_id, order_type, arbeitszeit,
                     COALESCE(ausfuehrung_am, fruehestens_am) AS soll_am,
                     ausfuehrung_am IS NOT NULL                AS ist_fixtermin,
                     (CURRENT_DATE - COALESCE(ausfuehrung_am, fruehestens_am))::INT AS tage_offen,
                     treffpunkt_strasse, treffpunkt_plz, treffpunkt_ort, treffpunkt_hinweis,
                     hinweis
              FROM sperr_orders
              WHERE tenant = $1
                AND status = 'pending'
                AND COALESCE(ausfuehrung_am, fruehestens_am) <= CURRENT_DATE
              ORDER BY soll_am ASC",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let due: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id":            r.try_get::<String, _>("id").unwrap_or_default(),
                    "malo_id":       r.try_get::<String, _>("malo_id").unwrap_or_default(),
                    "lf_mp_id":      r.try_get::<String, _>("lf_mp_id").unwrap_or_default(),
                    "order_type":    r.try_get::<String, _>("order_type").unwrap_or_default(),
                    "arbeitszeit":   r.try_get::<Option<String>, _>("arbeitszeit").unwrap_or(None),
                    // ISO 8601, not `time::Date`'s derived `[year, ordinal]` array.
                    "soll_am":       r.try_get::<Option<time::Date>, _>("soll_am")
                        .unwrap_or(None).map(|d| d.to_string()),
                    "ist_fixtermin": r.try_get::<bool, _>("ist_fixtermin").unwrap_or(false),
                    "tage_offen":    r.try_get::<Option<i32>, _>("tage_offen").unwrap_or(None),
                    "treffpunkt": {
                        "strasse": r.try_get::<Option<String>, _>("treffpunkt_strasse").unwrap_or(None),
                        "plz":     r.try_get::<Option<String>, _>("treffpunkt_plz").unwrap_or(None),
                        "ort":     r.try_get::<Option<String>, _>("treffpunkt_ort").unwrap_or(None),
                        "hinweis": r.try_get::<Option<String>, _>("treffpunkt_hinweis").unwrap_or(None),
                    },
                    "hinweis":       r.try_get::<Option<String>, _>("hinweis").unwrap_or(None),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "count": due.len(),
            "due_orders": due,
            "note": "ist_fixtermin=true means the Lieferant named a specific date \
                     (DTM+203); a Gerichtsvollzieher may have set it. Otherwise the date \
                     is a DTM+469 earliest-start and the order runs at the next opportunity.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl SperrdMcpHandler {
    #[prompt(
        name = "execute-sperrung",
        description = "Confirm a field execution and check that its IFTSTA 21039 went out"
    )]
    async fn execute_sperrung_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "The field team carried out a Sperrung. How do I confirm it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**Confirming a Sperr-/Entsperrauftrag**\n\n\
                 1. `list_due_orders` — find the order. Match `malo_id` and the \
                    Treffpunkt against the field report.\n\
                 2. `PUT /api/v1/sperr-orders/{id}/execute` with:\n\
                    - `executed_at` — the actual execution time (RFC 3339). This \
                      becomes `DTM+293 Fertigstellungsdatum`, which the AHB requires \
                      to be at or before the message date, so a future value is \
                      refused.\n\
                    - `pruefschritt_code` — the EBD Prüfschritt code from the \
                      \"erfolgreich\" cluster. `SG15 STS DE9013` is a **Muss**.\n\
                    - `note` — the field reference, e.g. TW-2026-0714-001.\n\
                 3. The response tells you what happened:\n\
                    - `204` — recorded, and IFTSTA 21039 is with makod.\n\
                    - `202` — recorded, but the dispatch failed. The order is in the \
                      retry queue; the Lieferant does **not** know yet.\n\
                 4. `get_sperr_order({id})` — `iftsta_dispatched_at` is set once the \
                    Auftragsstatus is out.\n\n\
                 **If it could not be carried out**, use `.../fail` with a `reason` \
                 and a Prüfschritt code from the \"gescheitert\" cluster. That is not \
                 an error path to avoid: the IFTSTA reports `Z13 gescheitert` and is \
                 how the Lieferant learns *why* instead of waiting on a process that \
                 never closes.",
            ),
        ]
    }

    #[prompt(
        name = "iftsta-sweep",
        description = "Find and clear Auftragsstatus messages that never reached the Lieferant"
    )]
    async fn iftsta_sweep_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Check whether every executed order has been reported to its Lieferant.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**IFTSTA 21039 sweep**\n\n\
                 **Step 1 — `get_sperr_stats`**\n\
                 - `iftsta_stuck = 0`: nothing to do. Any `iftsta_outstanding` are \
                   dispatches in flight; the retry worker is on them.\n\
                 - `iftsta_stuck > 0`: those orders exhausted the retry budget. Each \
                   one is a Lieferant who does not know what happened to their \
                   Sperrauftrag, and a `gpke-sperrung-lf` process that cannot close.\n\n\
                 **Step 2 — diagnose, do not re-run**\n\
                 `list_sperr_orders(status=\"executed\")` and read `iftsta_last_error` \
                 on the affected orders. A dispatch that failed eight times is almost \
                 never transport: the usual cause is that the makod `gpke-sperrung` \
                 process for that MaLo does not exist, or is not in `ValidationPassed` \
                 — its `BestaetigueSperrung` command refuses any other state.\n\n\
                 Check in makod whether the inbound ORDERS 17115/17117 spawned a \
                 process for this MaLo at all. If it did not, the order reached this \
                 queue by another route and there is no market correspondent to \
                 report to.\n\n\
                 **Step 3 — after the cause is fixed**\n\
                 Reset `iftsta_attempts` for the affected order and the worker picks it \
                 up again on its next pass. The retry re-uses the same idempotency key, \
                 so a message that did reach makod is not sent twice.\n\n\
                 **Do not** re-run `PUT .../execute`. The order is already terminal \
                 and that route claims on `status = 'pending'`, so it returns 404 and \
                 dispatches nothing.\n\n\
                 **Output**: { outstanding: N, stuck: N, causes: [...], status: ok|action_required }",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for SperrdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("sperrd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "sperrd MCP — Sperr-/Entsperrauftrag execution queue (NB role).\n\
             Read-only: creating, executing and cancelling orders are REST \
             operations, because each has a physical or market effect.\n\n\
             ## What this service does\n\
             An ORDERS 17115 (Sperrauftrag) or 17117 (Entsperrauftrag) from a \
             Lieferant becomes a job for the field team; the outcome goes back as \
             IFTSTA 21039 (Auftragsstatus Sperren `STS+Z37` / Entsperren `STS+Z38`, \
             `Z14 erfolgreich` or `Z13 gescheitert`). Without that message the LF's \
             gpke-sperrung-lf process never terminates.\n\n\
             ## Tools (4)\n\
             - `list_sperr_orders(status, malo_id, due, limit)` — the queue\n\
             - `get_sperr_order(id)` — one order, with ORDERS provenance and IFTSTA state\n\
             - `get_sperr_stats` — counters incl. iftsta_outstanding / iftsta_stuck\n\
             - `list_due_orders` — the field-dispatch list, with the Treffpunkt\n\n\
             ## Prompts (2)\n\
             - `execute-sperrung` — confirming an execution and its IFTSTA\n\
             - `iftsta-sweep` — finding and clearing unreported outcomes\n\n\
             ## Timing\n\
             GPKE fixes **no** execution deadline in Werktagen for the physical act. \
             The date is the Lieferant's own DTM+203 Ausführungsdatum (fixed) or \
             DTM+469 frühestes Startdatum (earliest). BK6-22-024 §5's 24 wall-clock \
             hours is the deadline for the NB's **ORDRSP**, which makod tracks.",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<SperrdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<SperrdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = SperrdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
