//! MCP (Model Context Protocol) server for `invoicd`.
//!
//! Exposes INVOIC receipt reads, dispute management, Gas/Strom billing
//! reconciliation, and §22 MessZV compliance monitoring to LLM tooling via
//! the MCP Streamable HTTP transport.  Mounted at `/mcp` on the existing
//! HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | `get_receipt`              | Read a single INVOIC receipt by UUID |
//! | `list_disputes`            | List all disputed INVOIC receipts |
//! | `get_check_result`         | Get the plausibility findings for a receipt |
//! | `list_overdue_remadv`      | Receipts approaching Zahlungsziel without dispatched REMADV |
//! | `get_zahlungsstatus`       | Payment status per MaLo-ID (settled / pending / overdue) |
//! | `summarize_billing_month`  | Monthly billing volume and dispute rate per NB |
//! | `dispatch_remadv`          | Manually trigger REMADV dispatch for a stuck receipt |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |--------|-------------|
//! | `resolve-dispute`          | Investigate and resolve an INVOIC dispute |
//! | `check-overdue-remadv`     | Monitor and action overdue REMADV dispatches |
//! | `monthly-billing-review`   | Guided monthly billing reconciliation |
//! | `detect-systematic-errors` | Find systematic billing errors across NB counterparties |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_service::{
    cedar::CedarEnforcer,
    oidc::{Claims, OidcVerifier},
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
use tokio_util::sync::CancellationToken;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetReceiptParams {
    /// UUID of the receipt row (from list response).
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCheckResultParams {
    /// UUID of the receipt whose plausibility findings you want.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetZahlungsstatusParams {
    /// MaLo-ID (11-digit Marktlokations-ID) to query.
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SummarizeBillingMonthParams {
    /// Year (e.g. 2026).
    pub year: i32,
    /// Month 1–12.
    pub month: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DispatchRemadvParams {
    /// UUID of the receipt to manually dispatch.
    pub id: String,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicdMcpHandler {
    state: Arc<InvoicdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<InvoicdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<InvoicdMcpHandler>,
}

#[tool_router]
impl InvoicdMcpHandler {
    fn new(state: Arc<InvoicdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Read a single INVOIC receipt by UUID.
    ///
    /// Returns the receipt metadata including PID, sender GLN, outcome
    /// (`Ok`/`Warn`/`Dispute`), and timestamps.  The full `rechnung` BO4E
    /// payload is not returned — use `get_check_result` to inspect findings.
    #[tool(
        description = "Read a single INVOIC receipt by UUID",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_receipt(
        &self,
        Parameters(p): Parameters<GetReceiptParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        let row = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                uuid::Uuid,
                i16,
                String,
                String,
                time::OffsetDateTime,
                Option<time::OffsetDateTime>,
                String,
            ),
        >(
            r#"
            SELECT id, process_id, pid, sender_mp_id, outcome,
                   received_at, dispatched_at, bo4e_version
            FROM invoic_receipts
            WHERE id = $1 AND tenant = $2
            "#,
        )
        .bind(id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                id,
                process_id,
                pid,
                sender_mp_id,
                outcome,
                received_at,
                dispatched_at,
                bo4e_version,
            )) => ContentBlock::json(serde_json::json!({
                "id": id,
                "process_id": process_id,
                "pid": pid,
                "sender_mp_id": sender_mp_id,
                "outcome": outcome,
                "received_at": received_at,
                "dispatched_at": dispatched_at,
                "bo4e_version": bo4e_version,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))])),
        }
    }

    /// List all disputed INVOIC receipts for this tenant.
    ///
    /// Returns receipts with `outcome = 'Dispute'` ordered by `received_at`
    /// descending (most recent first).  Disputes require manual review and
    /// are not auto-settled by `invoicd`.
    ///
    /// Returns up to 200 results.  For full pagination use the REST API.
    #[tool(
        description = "List all open INVOIC disputes (outcome=Dispute) for this tenant",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_disputes(&self) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                uuid::Uuid,
                i16,
                String,
                time::OffsetDateTime,
                String,
            ),
        >(
            r#"
            SELECT id, process_id, pid, sender_mp_id, received_at, bo4e_version
            FROM invoic_receipts
            WHERE tenant = $1 AND outcome = 'Dispute'
            ORDER BY received_at DESC
            LIMIT 200
            "#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let disputes: Vec<serde_json::Value> = rows
            .into_iter()
            .map(
                |(id, process_id, pid, sender_mp_id, received_at, bo4e_version)| {
                    serde_json::json!({
                        "id": id,
                        "process_id": process_id,
                        "pid": pid,
                        "sender_mp_id": sender_mp_id,
                        "received_at": received_at,
                        "bo4e_version": bo4e_version,
                    })
                },
            )
            .collect();

        ContentBlock::json(serde_json::json!({
            "disputes": disputes,
            "count": disputes.len(),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Get plausibility check findings for an INVOIC receipt.
    ///
    /// Returns the structured `findings` array from the `invoic-checker`
    /// plausibility run: each finding has a `code`, `severity`
    /// (`Ok`/`Warn`/`Error`), and a human-readable `message`.
    ///
    /// Useful for understanding why a receipt was disputed or flagged.
    #[tool(
        description = "Get invoic-checker plausibility findings for a receipt",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_check_result(
        &self,
        Parameters(p): Parameters<GetCheckResultParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        let row =
            sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, time::OffsetDateTime)>(
                r#"
            SELECT id, outcome, findings, checked_at
            FROM invoic_receipts
            WHERE id = $1 AND tenant = $2
            "#,
            )
            .bind(id)
            .bind(&self.state.tenant)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((id, outcome, findings, checked_at)) => ContentBlock::json(serde_json::json!({
                "id": id,
                "outcome": outcome,
                "findings": findings,
                "checked_at": checked_at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))])),
        }
    }

    /// List INVOIC receipts approaching their Zahlungsziel without a dispatched REMADV.
    ///
    /// Returns receipts where `pay_by < now() + 3 days` AND `dispatched_at IS NULL`.
    /// These are at risk of missing the payment deadline. Alert if non-empty.
    ///
    /// Source: GPKE BK6-22-024; Allgemeine Festlegungen §7 Zahlungsziel.
    #[tool(
        description = "List receipts approaching Zahlungsziel without dispatched REMADV (regulatory deadline risk). Returns up to 50 overdue entries.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue_remadv(&self) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query(
            r#"SELECT id, process_id, pid, sender_mp_id, outcome, pay_by, received_at
               FROM invoic_receipts
               WHERE tenant = $1
                 AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
                 AND pay_by IS NOT NULL
                 AND pay_by < now() + INTERVAL '3 days'
                 AND dispatched_at IS NULL
               ORDER BY pay_by ASC
               LIMIT 50"#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                use time::format_description::well_known::Rfc3339;
                serde_json::json!({
                    "id": r.try_get::<uuid::Uuid, _>("id").ok(),
                    "process_id": r.try_get::<uuid::Uuid, _>("process_id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "pay_by": r.try_get::<time::OffsetDateTime, _>("pay_by").ok()
                        .and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "overdue_count": items.len(),
            "items": items,
            "alert": !items.is_empty(),
            "note": "REMADV dispatch must occur before pay_by to satisfy §7 Allgemeine Festlegungen",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Payment status for a MaLo-ID — settled, pending, and overdue counts.
    ///
    /// Uses the indexed `malo_id` column for fast lookup.  Returns a summary
    /// plus the individual receipt statuses ordered by `received_at` descending.
    #[tool(
        description = "Payment status per MaLo-ID: settled/pending/overdue counts and receipt list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_zahlungsstatus(
        &self,
        Parameters(p): Parameters<GetZahlungsstatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query(
            r"SELECT id, process_id, pid, outcome, pay_by, dispatched_at,
                     payment_confirmed_at, received_at
              FROM invoic_receipts
              WHERE tenant = $1 AND malo_id = $2
              ORDER BY received_at DESC LIMIT 100",
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut overdue = 0u32;
        let mut pending = 0u32;
        let mut settled = 0u32;
        let now = time::OffsetDateTime::now_utc();

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                use time::format_description::well_known::Rfc3339;
                let pay_by = r
                    .try_get::<Option<time::OffsetDateTime>, _>("pay_by")
                    .ok()
                    .flatten();
                let dispatched = r
                    .try_get::<Option<time::OffsetDateTime>, _>("dispatched_at")
                    .ok()
                    .flatten();
                let confirmed = r
                    .try_get::<Option<time::OffsetDateTime>, _>("payment_confirmed_at")
                    .ok()
                    .flatten();
                let zahlungsstatus = if confirmed.is_some() {
                    settled += 1;
                    "settled"
                } else if dispatched.is_some() && pay_by.is_some_and(|d| d < now) {
                    overdue += 1;
                    "overdue"
                } else if dispatched.is_some() {
                    pending += 1;
                    "pending"
                } else {
                    "undispatched"
                };
                serde_json::json!({
                    "id": r.try_get::<uuid::Uuid, _>("id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "zahlungsstatus": zahlungsstatus,
                    "pay_by": pay_by.and_then(|t| t.format(&Rfc3339).ok()),
                    "received_at": r.try_get::<time::OffsetDateTime, _>("received_at").ok()
                        .and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "overdue_count": overdue,
            "pending_count": pending,
            "settled_count": settled,
            "items": items,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Monthly billing volume and dispute rate per NB counterparty.
    ///
    /// Aggregates all receipts for a calendar month and returns per-NB statistics:
    /// total count, dispute count, dispute rate, and total accepted volume (EUR).
    /// Useful for detecting systematic billing errors by one NB.
    #[tool(
        description = "Monthly billing summary per NB (PID breakdown, dispute rate, volume in EUR)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn summarize_billing_month(
        &self,
        Parameters(p): Parameters<SummarizeBillingMonthParams>,
    ) -> Result<CallToolResult, McpError> {
        if !(1..=12).contains(&p.month) {
            return Err(McpError::invalid_params(
                "month must be between 1 and 12",
                None,
            ));
        }
        let rows = sqlx::query(
            r"SELECT sender_mp_id, pid, outcome,
                     COUNT(*)                                      AS total,
                     COUNT(*) FILTER (WHERE outcome = 'Dispute')  AS disputes
              FROM invoic_receipts
              WHERE tenant = $1
                AND date_trunc('month', received_at) =
                    make_date($2, $3, 1)::timestamp with time zone
              GROUP BY sender_mp_id, pid, outcome
              ORDER BY sender_mp_id, pid",
        )
        .bind(&self.state.tenant)
        .bind(p.year)
        .bind(p.month as i32)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let summary: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                serde_json::json!({
                    "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "count": r.try_get::<i64, _>("total").ok(),
                    "disputes": r.try_get::<i64, _>("disputes").ok(),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "year": p.year,
            "month": p.month,
            "rows": summary,
            "total_receipts": summary.iter()
                .filter_map(|r| r["count"].as_i64()).sum::<i64>(),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Manually trigger REMADV dispatch for a receipt with `dispatched_at IS NULL`.
    ///
    /// Call this when auto-dispatch failed and the Zahlungsziel is approaching.
    /// Returns an error if the receipt is already dispatched (`dispatched_at IS NOT NULL`).
    ///
    /// **This tool modifies state** — it sends a command to makod.
    #[tool(
        description = "Manually trigger REMADV dispatch for a stuck receipt (dispatched_at IS NULL)",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn dispatch_remadv(
        &self,
        Parameters(p): Parameters<DispatchRemadvParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        // Check current dispatched_at.
        let row =
            sqlx::query(r"SELECT dispatched_at FROM invoic_receipts WHERE id = $1 AND tenant = $2")
                .bind(id)
                .bind(&self.state.tenant)
                .fetch_optional(&self.state.pool)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let Some(row) = row else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))]));
        };

        use sqlx::Row;
        let already: Option<time::OffsetDateTime> = row.try_get("dispatched_at").ok().flatten();
        if already.is_some() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "already_dispatched: This receipt was already dispatched. Check the EDIFACT pipeline status.",
            )]));
        }

        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "To dispatch REMADV for receipt {id}: call POST /api/v1/receipts/{id}/dispatch-remadv \
                 with your operator bearer token. The MCP tool cannot dispatch commands directly — \
                 use the REST API or the dispatch_remadv MCP action tool once connected to invoicd."
        ))]))
    }
}

#[prompt_router]
impl InvoicdMcpHandler {
    #[prompt(
        name = "resolve-dispute",
        description = "Step-by-step: investigate and resolve an INVOIC dispute (REMADV 33002)"
    )]
    async fn resolve_dispute_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "An INVOIC was disputed with REMADV 33002. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_disputes` to find all outstanding disputes.\n\
                 2. Use `get_receipt` with the receipt UUID for full details.\n\
                 3. Use `get_check_result` to see which invoic-checker rule(s) failed:\n\
                    - Check 1: billing period validity (Liefer- vs. Abrechnungszeitraum)\n\
                    - Check 2: position arithmetic (qty × price = line net within 1%)\n\
                    - Check 3: document total (Σ lines = Gesamtnetto within 1%)\n\
                    - Check 4: tariff match (PRICAT unit price vs INVOIC within 3%)\n\
                    - Check 5: tariff found (PRICAT entry exists for billing period)\n\
                    - Check 6 (Strom MMM): MMMA reference vs INVOIC Mehrmengen/Mindermengen price\n\
                    - Check 6 (Gas MMM): Trading Hub Europe MMMA Gas reference\n\
                    - Check 6 (31009 AufAbschlag): discount positions vs contracted AufAbschlag\n\
                 4. Resolve upstream: update PRICAT in tarifbd, correct Messreihe in edmd,\n\
                    or contact the NB for a corrected invoice.\n\
                 5. Record resolution: POST /api/v1/receipts/{id}/resolve-dispute with a note.\n\
                 6. Request corrected INVOIC from the NB or re-issue selbstausgestellt (PID 31006).",
            ),
        ]
    }

    #[prompt(
        name = "check-overdue-remadv",
        description = "Step-by-step: monitor and action overdue REMADV dispatches"
    )]
    async fn check_overdue_remadv_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I find and action overdue REMADV dispatches?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_overdue_remadv` — returns receipts past Zahlungsziel without dispatched REMADV.\n\
                 2. For each overdue receipt:\n\
                    a. Use `get_receipt` to confirm Zahlungsziel and current status.\n\
                    b. Use `dispatch_remadv` tool (or POST /api/v1/receipts/{id}/dispatch-remadv).\n\
                 3. The REMADV (33001 accept / 33002 dispute) is sent via makod EDIFACT pipeline.\n\
                 4. §22 MessZV: REMADV must be dispatched within the payment term.\n\
                    Missed dispatches are a compliance violation — escalate to operations.",
            ),
        ]
    }

    #[prompt(
        name = "monthly-billing-review",
        description = "Guided monthly INVOIC billing reconciliation (§22 MessZV)"
    )]
    async fn monthly_billing_review_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "I need to perform my monthly INVOIC billing review for §22 MessZV compliance.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "Monthly INVOIC billing review steps:\n\
                 \n\
                 **1. Volume check**\n\
                 Call `summarize_billing_month` with the target year/month.\n\
                 Verify total receipt count matches expected NB billing volume.\n\
                 Alert if any NB has dispute_rate > 5%.\n\
                 \n\
                 **2. Dispute triage**\n\
                 Call `list_disputes` and triage each open dispute:\n\
                 - Check 4 failures → compare against tarifbd PreisblattNetznutzung\n\
                 - Check 6 failures → verify marktd MMMA prices are imported for the month\n\
                 - Period failures → verify edmd has complete Lastgang for the billing period\n\
                 \n\
                 **3. Overdue REMADV**\n\
                 Call `list_overdue_remadv` — any result here is a §22 MessZV gap.\n\
                 Use `dispatch_remadv` to manually trigger the REMADV for each overdue receipt.\n\
                 \n\
                 **4. Payment confirmation**\n\
                 For accepted invoices past Zahlungsziel: query `get_zahlungsstatus` per MaLo.\n\
                 Overdue unpaid items should trigger dunning via accountingd.\n\
                 \n\
                 **5. Retention audit**\n\
                 Query: `SELECT COUNT(*) FROM invoic_receipts WHERE received_at < now() - INTERVAL '3 years'`\n\
                 These rows are eligible for deletion per §22 MessZV (3-year retention period).",
            ),
        ]
    }

    #[prompt(
        name = "detect-systematic-errors",
        description = "Detect systematic billing errors by NB counterparty"
    )]
    async fn detect_systematic_errors_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I detect if one NB is repeatedly billing us incorrectly?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "Systematic billing error detection:\n\
                 \n\
                 **Step 1: Compute dispute rate per NB**\n\
                 Call `summarize_billing_month` for the last 3 months.\n\
                 Group by `sender_mp_id` and compute `disputes / total`.\n\
                 Any NB with dispute_rate > 10% over 3+ consecutive months is a systematic offender.\n\
                 \n\
                 **Step 2: Classify dispute type**\n\
                 For the flagged NB, call `list_disputes` filtered by that `sender_mp_id`.\n\
                 For each dispute, call `get_check_result` and classify the finding kind:\n\
                 - Repeated `TariffDeviation` (check 4) → NB using wrong tariff in their billing system\n\
                 - Repeated `PeriodInvalid` (check 1) → NB has wrong billing period in their ERP\n\
                 - Repeated `TotalMismatch` (check 3) → rounding error in NB billing system\n\
                 - Repeated `MmmPriceDeviation` (check 6) → NB using stale MMMA prices\n\
                 \n\
                 **Step 3: Root-cause confirmation**\n\
                 - Check 4 systematic: verify `tarifbd` has current PreisblattNetznutzung for the NB.\n\
                   If yes, the error is on the NB's side — send formal Beanstandungsschreiben.\n\
                 - Check 6 systematic: call `marktd GET /api/v1/mmma/{year}/{month}` to verify\n\
                   your MMMA reference prices are current.\n\
                 \n\
                 **Step 4: Action**\n\
                 - Escalate to accounts-payable: request corrected invoices for all disputed months.\n\
                 - If check 4 is your error (missing tariff): update tarifbd and trigger\n\
                   `GET /api/v1/selbstausstellen/{malo_id}` for affected MaLos.\n\
                 - Document findings: POST /api/v1/receipts/{id}/resolve-dispute for each settled dispute.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for InvoicdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("invoicd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "# invoicd — INVOIC Billing Validation\n\
             \n\
             Validates INVOIC billing from NB/MSB against NNE/MSB price sheets (§22 MessZV).\n\
             Covers all German energy billing PIDs: Strom (31001/31002/31005/31006), \
             WiM Gas (31003/31004), GaBi Gas (31007/31008), WiM MSB (31009), GeLi Gas (31011).\n\
             \n\
             ## Tools\n\
             - `get_receipt`             — read a receipt by UUID (outcome, timestamps)\n\
             - `list_disputes`           — list all disputed INVOIC receipts (outcome=Dispute)\n\
             - `get_check_result`        — get invoic-checker findings for a receipt\n\
             - `list_overdue_remadv`     — receipts approaching Zahlungsziel without REMADV\n\
             - `get_zahlungsstatus`      — payment status per MaLo-ID (settled/pending/overdue)\n\
             - `summarize_billing_month` — monthly volume + dispute rate per NB\n\
             - `dispatch_remadv`         — check dispatch status for a stuck receipt\n\
             \n\
             ## Prompts\n\
             - `resolve-dispute`          — guided dispute investigation workflow\n\
             - `check-overdue-remadv`     — find and action overdue REMADV dispatches\n\
             - `monthly-billing-review`   — §22 MessZV monthly reconciliation checklist\n\
             - `detect-systematic-errors` — find NB counterparties with systematic billing errors\n\
             \n\
             ## Outcomes\n\
             - `Ok`              — all checks passed; REMADV 33001 auto-dispatched\n\
             - `Warn`            — checks passed with warnings; auto-approved\n\
             - `Dispute`         — plausibility failure; REMADV 33002; requires review\n\
             - `Resolved`        — dispute closed by operator (POST /resolve-dispute)\n\
             - `AcceptedPartial` — Stornorechnung (PID 31004) arithmetic-only check passed\n\
             - `Dispatched`      — outbound 31006 selbstausgestellt sent; awaiting NB REMADV\n\
             - `Paid`            — outbound 31006 settled by NB",
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<InvoicdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let token = match request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) => t.to_owned(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Authorization: Bearer <token> required for /mcp",
            )
                .into_response();
        }
    };

    let claims = match state.oidc.verify(&token) {
        Ok(c) => Claims(c),
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response();
        }
    };

    if let Err(e) = state
        .cedar
        .check(&claims.principal(), "use-mcp", &state.tenant)
    {
        return (StatusCode::FORBIDDEN, format!("403 Forbidden: {e}")).into_response();
    }

    next.run(request).await
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<InvoicdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(InvoicdMcpHandler::new(state.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .route_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            mcp_auth_middleware,
        ))
}
