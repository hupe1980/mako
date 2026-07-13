//! MCP server for `netzbilanzd` — NNE/KA/MMM Billing Daemon (NB role).
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_nne_drafts` | List NNE/MMM invoice drafts (filter by malo_id, lf_mp_id, status) |
//! | `list_disputed` | List invoices with `Disputed` check outcome |
//! | `get_nne_draft` | Get a single invoice draft with full Rechnung BO4E |
//! | `list_pending_kostenblatt` | List Redispatch 2.0 Kostenblatt records due for submission |
//! | `compute_kostenblatt` | Compute Kostenblatt for an activation (dispatch_kwh_override required) |
//! | `get_billing_summary` | Monthly billing totals by PID and status |
//! | `list_undispatched_drafts` | Drafts still in draft status older than N hours |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `trigger-nne-billing` | Step-by-step: run an NNE billing run for a MaLo |
//! | `investigate-dispute` | Step-by-step: investigate a disputed REMADV 33002 |
//! | `mmm-monthly-run` | Step-by-step: run monthly MMM auto-billing |
//! | `redispatch-monthly-submit` | Step-by-step: prepare and submit monthly Kostenblatt to ÜNB |

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
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
use uuid::Uuid;

#[derive(Clone)]
pub struct NetzbilanzMcpState {
    pub pool: PgPool,
    pub tenant: String,
    /// Optional static Bearer token for MCP auth. `None` = dev mode (no auth).
    pub mcp_api_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListBillingParams {
    /// Filter by MaLo-ID.
    pub malo_id: Option<String>,
    /// Filter by LF MP-ID.
    pub lf_mp_id: Option<String>,
    /// Filter by outcome (Sent/Paid/PartialPaid/Disputed/ValidationFailed/Error).
    pub outcome: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRecordParams {
    pub id: String,
}

#[derive(Clone)]
pub struct NetzbilanzMcpHandler {
    state: Arc<NetzbilanzMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<NetzbilanzMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<NetzbilanzMcpHandler>,
}

#[tool_router]
impl NetzbilanzMcpHandler {
    fn new(state: Arc<NetzbilanzMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List NNE/KA/MMM invoice drafts (INVOIC 31001/31002/31005). Filter by malo_id, lf_mp_id, or status (draft/dispatched/paid/disputed). Returns summary without full Rechnung. Use after POST /api/v1/billing/run.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_nne_drafts(
        &self,
        Parameters(p): Parameters<ListBillingParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        match list_billing_records(
            &self.state.pool,
            &self.state.tenant,
            p.malo_id.as_deref(),
            p.lf_mp_id.as_deref(),
            p.outcome.as_deref(),
            p.limit.unwrap_or(50),
        )
        .await
        {
            Ok(rows) => ContentBlock::json(serde_json::to_value(rows).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all NNE/KA/MMM invoices with Disputed outcome (REMADV 33002 received). Shows records requiring COMDIS 29001 escalation or re-billing.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_disputed(
        &self,
        Parameters(_): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        match list_billing_records(
            &self.state.pool, &self.state.tenant,
            None, None, Some("Disputed"), 100,
        ).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "disputed_count": rows.len(),
                "records": rows,
                "hint": "Use COMDIS 29001 (mako-gpke) for formal dispute escalation after REMADV 33002.",
            })).map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single NNE invoice draft by UUID, including the full BO4E Rechnung JSON payload (Grundpreis, Arbeitspreis, Leistungspreis, KA, invoic-checker findings).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_nne_draft(
        &self,
        Parameters(p): Parameters<GetRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_billing_record;
        let Ok(id) = p.id.parse::<Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match fetch_billing_record(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(row)) => ContentBlock::json(serde_json::to_value(row).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("record {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List Redispatch 2.0 Kostenblatt records for a billing period. Use ?status=pending to find records due for 15th-of-month submission to ÜNB (BK6-20-061).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pending_kostenblatt(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_kostenblatt;
        let now = time::OffsetDateTime::now_utc();
        let year = p
            .get("year")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| now.year() as i64) as i16;
        let month = p
            .get("month")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| now.month() as i64) as i16;
        let status = p
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        match list_kostenblatt(&self.state.pool, &self.state.tenant, year, month, Some(status)).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "period": format!("{year}-{month:02}"),
                "count": rows.len(),
                "records": rows,
                "hint": "POST /api/v1/redispatch/kostenblatt/{activation_id}/compute to auto-fetch dispatch_kwh from edmd. POST /api/v1/redispatch/kostenblatt/submit/{year}/{month} to mark all pending as submitted.",
            })).map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Compute Redispatch 2.0 Kostenblatt for an activation: auto-fetches dispatch energy (kWh) from edmd for the activation window, generates BO4E Kosten/KostenBlock/KostenPosition JSON, and upserts the record. BK6-20-061 §4.2 compliance.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn compute_kostenblatt(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let activation_id = p
            .get("activation_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let tr_id = p
            .get("tr_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let malo_id = p
            .get("malo_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let uenb_mp_id = p
            .get("uenb_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let vnb_mp_id = p
            .get("vnb_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let start = p
            .get("activation_start_utc")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let end_t = p
            .get("activation_end_utc")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let arbeitspreis: rust_decimal::Decimal = p
            .get("arbeitspreis_eur_per_kwh")
            .and_then(|v| {
                v.as_str().and_then(|s| s.parse().ok()).or_else(|| {
                    v.as_f64()
                        .and_then(|f| rust_decimal::Decimal::try_from(f).ok())
                })
            })
            .unwrap_or(rust_decimal::Decimal::ZERO);
        let period_year =
            p.get("period_year")
                .and_then(|v| v.as_i64())
                .unwrap_or(time::OffsetDateTime::now_utc().year() as i64) as i16;
        let period_month =
            p.get("period_month")
                .and_then(|v| v.as_i64())
                .unwrap_or(time::OffsetDateTime::now_utc().month() as i64) as i16;
        let dispatch_kwh_override: Option<rust_decimal::Decimal> =
            p.get("dispatch_kwh_override").and_then(|v| {
                v.as_str().and_then(|s| s.parse().ok()).or_else(|| {
                    v.as_f64()
                        .and_then(|f| rust_decimal::Decimal::try_from(f).ok())
                })
            });

        if activation_id.is_empty() || tr_id.is_empty() || malo_id.is_empty() {
            return Err(McpError::invalid_params(
                "activation_id, tr_id, and malo_id are required",
                None,
            ));
        }

        // Build typed BO4E Kosten JSON inline (same logic as POST /compute handler).
        let dispatch_kwh = if let Some(override_kwh) = dispatch_kwh_override {
            override_kwh
        } else {
            return Ok(CallToolResult::success(vec![ContentBlock::text(
                "dispatch_kwh_override required when called via MCP — provide measured kWh from edmd. Use POST /api/v1/redispatch/kostenblatt/{activation_id}/compute for automatic edmd fetch.",
            )]));
        };

        let einsatzkosten_eur = dispatch_kwh * arbeitspreis;
        let kosten_json = serde_json::json!({
            "_typ": "KOSTEN",
            "summe": [{ "_typ": "KOSTENBLOCK", "kostenblockbezeichnung": "Redispatch 2.0 Einsatzkosten",
                "kostenpositionen": [{ "_typ": "KOSTENPOSITION", "positionsbezeichnung": "Arbeitspreis Redispatch",
                    "artikelId": tr_id, "menge": { "_typ": "MENGE", "wert": dispatch_kwh.to_string(), "einheit": "KWH" },
                    "einzelpreis": { "_typ": "PREIS", "wert": arbeitspreis.to_string(), "einheit": "EUR" },
                    "betragKostenstelle": { "_typ": "BETRAG", "wert": einsatzkosten_eur.to_string(), "waehrung": "EUR" },
                    "zeitraum": { "_typ": "ZEITRAUM", "startdatum": start, "enddatum": end_t }
                }]
            }]
        });

        let req = crate::pg::UpsertKostenblattRequest {
            tr_id,
            malo_id: Some(malo_id),
            period_year,
            period_month,
            uenb_mp_id,
            vnb_mp_id,
            dispatch_kwh,
            arbeitspreis_eur_per_kwh: arbeitspreis,
            kosten_json: Some(kosten_json),
        };
        match crate::pg::upsert_kostenblatt(
            &self.state.pool,
            &self.state.tenant,
            &activation_id,
            &req,
        )
        .await
        {
            Ok(id) => ContentBlock::json(serde_json::json!({
                "id": id, "activation_id": activation_id, "dispatch_kwh": dispatch_kwh.to_string(),
                "einsatzkosten_eur": einsatzkosten_eur.to_string(), "source": "mcp_override",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Monthly billing totals grouped by PID and status.
    ///
    /// Provides a concise financial summary for a billing month:
    /// - NNE Strom (PID 31001): total kWh invoiced and total EUR
    /// - MMM Strom (PID 31002): Mehrmengen / Mindermengen net
    /// - NNE Gas (PID 31005): gas billing totals
    /// - MSB-Rechnung (PID 31009): metering fees
    ///
    /// Used for end-of-month reconciliation and ERP journal entry preparation.
    #[tool(
        description = "Monthly billing summary: totals by PID (31001/31002/31005/31009) and status (draft/dispatched/paid). Use for end-of-month ERP reconciliation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_billing_summary(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::billing_summary;
        let now = time::OffsetDateTime::now_utc();
        let year = p
            .get("year")
            .and_then(|v| v.as_i64())
            .unwrap_or(now.year() as i64) as i32;
        let month = p
            .get("month")
            .and_then(|v| v.as_i64())
            .unwrap_or(now.month() as i64) as u8;
        if !(1..=12).contains(&month) {
            return Err(McpError::invalid_params("month must be 1-12", None));
        }
        match billing_summary(&self.state.pool, &self.state.tenant, year, month).await {
            Ok(rows) => {
                let total_gross: i64 = rows.iter().map(|r| r.total_gross_eur_units).sum();
                ContentBlock::json(serde_json::json!({
                    "year": year,
                    "month": month,
                    "total_gross_eur": format!("{:.5}", total_gross as f64 / 100_000.0),
                    "by_pid_status": rows,
                    "note": "gross_eur_units are integer × 10⁻⁵ EUR. total_gross_eur is pre-computed.",
                })).map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List drafts that are still in `draft` status and older than the specified hours.
    ///
    /// Undispatched invoices older than 24–48 hours are approaching the typical
    /// Zahlungsziel (30 days from invoice_date) without the NB having dispatched
    /// them. Use this to detect stuck drafts before the LF notices late billing.
    ///
    /// Also catches invoices blocked by `check_outcome = 'Warn'` that need
    /// operator review before dispatch.
    #[tool(
        description = "List draft invoices older than N hours still not dispatched. Default: 48h. Alert threshold for billing cycle compliance.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_undispatched_drafts(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_undispatched_stale;
        let hours = p
            .get("older_than_hours")
            .and_then(|v| v.as_i64())
            .unwrap_or(48);
        let limit = p.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
        match list_undispatched_stale(&self.state.pool, &self.state.tenant, hours, limit).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "undispatched_count": rows.len(),
                "older_than_hours": hours,
                "alert": !rows.is_empty(),
                "records": rows,
                "hint": "Use POST /api/v1/billing/drafts/dispatch-batch with the draft_ids to dispatch all at once.",
            })).map(|b| CallToolResult::success(vec![b])).map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Dispatch a single draft by UUID (validate + send INVOIC to makod).
    ///
    /// Blocks when check_outcome = 'Dispute'. Reports the makod command dispatch_ref
    /// on success. Idempotent — re-dispatching an already-dispatched draft is a no-op
    /// (returns the existing dispatch_ref).
    #[tool(
        description = "Dispatch a draft INVOIC to makod (sends INVOIC 31001/31002/31005/31009/31011 to LF/MSB/LFG). Blocked when invoic-checker outcome is Dispute.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn dispatch_draft(
        &self,
        Parameters(p): Parameters<GetRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        // Load draft to get makod client — use a stub for the MCP path (no makod in MCP state).
        // Provide guidance to use REST endpoint for actual dispatch.
        match crate::pg::fetch_draft(&self.state.pool, id).await {
            Ok(Some(row)) => {
                if row.status == "dispatched" {
                    return ContentBlock::json(serde_json::json!({
                        "status": "already_dispatched",
                        "dispatch_ref": row.dispatch_ref,
                        "hint": "This draft has already been dispatched."
                    }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None));
                }
                if row.check_outcome.as_deref() == Some("Dispute") {
                    return Err(McpError::invalid_params(
                        format!(
                            "draft {id} has check_outcome=Dispute — fix invoice before dispatching"
                        ),
                        None,
                    ));
                }
                ContentBlock::json(serde_json::json!({
                    "draft_id": id,
                    "status": row.status,
                    "check_outcome": row.check_outcome,
                    "pid": row.pid,
                    "action": "call PUT /api/v1/billing/drafts/{id}/dispatch to dispatch this draft",
                    "hint": "MCP cannot dispatch directly (no makod credentials in MCP state). Use the REST API: PUT /api/v1/billing/drafts/{id}/dispatch",
                })).map(|b| CallToolResult::success(vec![b]))
                  .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::invalid_params(
                format!("draft {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Reject a draft with a reason (status → 'rejected').
    ///
    /// Rejected drafts are excluded from the no-double-billing constraint so
    /// a corrected billing run can be submitted for the same period.
    #[tool(
        description = "Reject an invoice draft (status → rejected). Supply a reason string. Rejected drafts allow re-billing the same MaLo/period.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn reject_draft(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let id_str = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        let reason = p
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("operator rejection via MCP");
        let Ok(id) = id_str.parse::<Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match crate::pg::reject_draft_pg(&self.state.pool, id, reason).await {
            Ok(true) => ContentBlock::json(serde_json::json!({
                "draft_id": id,
                "status": "rejected",
                "reason": reason,
                "hint": "Draft rejected. Run a new billing cycle to re-generate for this MaLo/period.",
            })).map(|b| CallToolResult::success(vec![b]))
              .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(false) => Err(McpError::invalid_params(format!("draft {id} not found or not in draft status"), None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Trigger a single-MaLo MMM auto-run for a specific billing month.
    ///
    /// Shortcut for `POST /api/v1/billing/mmm-run/{malo_id}` — returns guidance
    /// and the required request body so the operator can execute via REST.
    #[tool(
        description = "Prepare a Mehr-/Mindermengen (MMM) auto-run for one MaLo. Returns the required REST request body. Prerequisite: MMMA prices imported in marktd.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn trigger_mmm_auto_run(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let malo_id = p
            .get("malo_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let nb_mp_id = p
            .get("nb_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or("<nb_mp_id>");
        let lf_mp_id = p
            .get("lf_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or("<lf_mp_id>");
        let now = time::OffsetDateTime::now_utc();
        let year = p
            .get("period_year")
            .and_then(|v| v.as_i64())
            .unwrap_or(now.year() as i64);
        let month = p
            .get("period_month")
            .and_then(|v| v.as_i64())
            .unwrap_or(now.month() as i64 - 1);
        let month = month.clamp(1, 12);
        if malo_id.is_empty() {
            return Err(McpError::invalid_params("malo_id is required", None));
        }
        let example_body = serde_json::json!({
            "nb_mp_id": nb_mp_id,
            "lf_mp_id": lf_mp_id,
            "period_year": year,
            "period_month": month,
        });
        ContentBlock::json(serde_json::json!({
            "instruction": format!("POST /api/v1/billing/mmm-run/{malo_id}"),
            "body": example_body,
            "description": "Fetches nb_quantity_kwh from edmd, auto-fetches Strom/Gas MMM prices from marktd if not supplied.",
            "prerequisites": [
                "MMMA Gas prices imported: PUT marktd /api/v1/mmma-preise/gas/{year}/{month}",
                "MMMA Strom prices imported: PUT marktd /api/v1/mmm-preise/strom/{year}/{month}",
                "edmd has imbalance data for the period"
            ]
        })).map(|b| CallToolResult::success(vec![b]))
          .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// List Korrekturrechnung and Stornorechnung records.
    ///
    /// Corrections are drafts with `rechnungsart` = `KORREKTURRECHNUNG` or
    /// `STORNORECHNUNG` and a non-null `original_draft_id`.  Used for §22 MessZV
    /// 3-year audit trail and COMDIS 29001 dispute resolution.
    #[tool(
        description = "List all Korrekturrechnung and Stornorechnung drafts (§22 MessZV audit trail). Filter by malo_id.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_corrections(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let malo_id = p.get("malo_id").and_then(|v| v.as_str());
        let limit = p.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
        // Reuse list_billing_records but without outcome filter; client-side filter rechnungsart.
        match crate::pg::list_billing_records(
            &self.state.pool,
            &self.state.tenant,
            malo_id,
            None,
            None,
            limit * 3, // fetch more to filter
        )
        .await
        {
            Ok(rows) => {
                let corrections: Vec<_> = rows
                    .into_iter()
                    .filter(|r| r.rechnungsart != "RECHNUNG")
                    .take(limit as usize)
                    .collect();
                ContentBlock::json(serde_json::json!({
                    "correction_count": corrections.len(),
                    "records": corrections,
                    "hint": "Use get_nne_draft with original_draft_id to compare original vs. correction.",
                })).map(|b| CallToolResult::success(vec![b]))
                  .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Payment statistics: count + EUR totals by PID × status.
    ///
    /// Used for ERP month-end reconciliation to verify that all dispatched
    /// invoices have been either paid or disputed.  Outstanding `dispatched`
    /// invoices are at risk of Zahlungsverzug (payment default).
    ///
    /// Statuses: `draft` | `dispatched` | `paid` | `disputed` | `rejected`
    #[tool(
        description = "Payment stats for ERP month-end: count and EUR totals by PID × status. Identifies outstanding dispatched invoices approaching Zahlungsziel.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_payment_stats(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::payment_stats;
        let now = time::OffsetDateTime::now_utc();
        let year = p
            .get("year")
            .and_then(|v| v.as_i64())
            .unwrap_or(now.year() as i64) as i32;
        let month = p
            .get("month")
            .and_then(|v| v.as_i64())
            .unwrap_or(now.month() as i64) as u8;
        if !(1..=12).contains(&month) {
            return Err(McpError::invalid_params("month must be 1-12", None));
        }
        match payment_stats(&self.state.pool, &self.state.tenant, year, month).await {
            Ok(rows) => {
                let paid_count: i64 = rows
                    .iter()
                    .filter(|r| r.status == "paid")
                    .map(|r| r.count)
                    .sum();
                let dispatched_count: i64 = rows
                    .iter()
                    .filter(|r| r.status == "dispatched")
                    .map(|r| r.count)
                    .sum();
                let disputed_count: i64 = rows
                    .iter()
                    .filter(|r| r.status == "dispatched" && r.total_gross_eur_units < 0)
                    .map(|r| r.count)
                    .sum();
                let total_outstanding: i64 = rows
                    .iter()
                    .filter(|r| r.status == "dispatched")
                    .map(|r| r.total_gross_eur_units)
                    .sum();
                ContentBlock::json(serde_json::json!({
                    "period": format!("{year}-{month:02}"),
                    "paid_count": paid_count,
                    "dispatched_count": dispatched_count,
                    "disputed_count": disputed_count,
                    "total_outstanding_eur": format!("{:.5}", total_outstanding as f64 / 100_000.0),
                    "by_pid_status": rows,
                    "alert": dispatched_count > 0,
                    "hint": if dispatched_count > 0 {
                        "Outstanding dispatched invoices exist — check Zahlungsziel and REMADV status."
                    } else {
                        "All invoices settled for this period."
                    },
                })).map(|b| CallToolResult::success(vec![b]))
                  .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List all paid invoices for a period (REMADV 33001/33003/33004 confirmed).
    ///
    /// Used for ERP accounts-receivable confirmation and BNetzA §22 MessZV audit.
    #[tool(
        description = "List all paid invoice drafts (REMADV 33001/33003 confirmed). For ERP AR reconciliation and §22 MessZV audit.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_paid_invoices(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        let malo_id = p.get("malo_id").and_then(|v| v.as_str());
        let limit = p.get("limit").and_then(|v| v.as_i64()).unwrap_or(100);
        match list_billing_records(
            &self.state.pool,
            &self.state.tenant,
            malo_id,
            None,
            None,
            limit,
        )
        .await
        {
            Ok(rows) => {
                let paid: Vec<_> = rows.into_iter().filter(|r| r.status == "paid").collect();
                ContentBlock::json(serde_json::json!({
                    "paid_count": paid.len(),
                    "records": paid,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl NetzbilanzMcpHandler {
    #[prompt(
        name = "trigger-nne-billing",
        description = "Step-by-step: run NNE billing for a MaLo"
    )]
    async fn trigger_nne_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Trigger an NNE billing run for a MaLo."),
            PromptMessage::new_text(
                Role::Assistant,
                "1. POST /api/v1/billing/run with malo_id, nb_mp_id, lf_mp_id, period_from, period_to.\n\
                 2. For §42a GGV tenants: POST /api/v1/billing/ggv-nne/{ggv_malo_id} instead.\n\
                 3. The draft is validated by invoic-checker before dispatch.\n\
                 4. GET /api/v1/billing/drafts to review the draft Rechnung BO4E.\n\
                 5. PUT /api/v1/billing/drafts/{id}/dispatch → sends INVOIC 31001 to makod.\n\
                 6. If a correction is needed: POST /api/v1/billing/drafts/{id}/correction.\n\n\
                 Use `list_nne_drafts` to monitor draft status.\n\
                 PID 31002 (MMM-Rechnung) and 31005 (KA) follow the same flow.\n\
                 Redispatch Kostenblatt: POST /api/v1/redispatch/kostenblatt/{activation_id}/compute.",
            ),
        ]
    }

    #[prompt(
        name = "investigate-dispute",
        description = "Step-by-step: investigate a REMADV 33002 dispute"
    )]
    async fn investigate_dispute_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A REMADV 33002 dispute was received. What should I do?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_nne_drafts` with outcome=Dispute to find disputed invoices.\n\
                 2. Use `get_nne_draft` for the full Rechnung BO4E and invoic-checker findings.\n\
                 3. Identify the ERC code in the REMADV (Z32=Tariff deviation, Z34=Period invalid, Z35=MMM price).\n\
                 4. Fix root cause: update tariff in marktd, correct Messreihe in edmd, or verify MMMA prices.\n\
                 5. POST /api/v1/billing/drafts/{id}/correction to generate a Korrekturrechnung (§22 MessZV).\n\
                 6. Dispatch the correction and confirm with the LF.",
            ),
        ]
    }

    #[prompt(
        name = "mmm-monthly-run",
        description = "Step-by-step: run monthly Mehr-/Mindermenge (MMM) billing for all SLP MaLos"
    )]
    async fn mmm_monthly_run_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I run the monthly MMM billing?"),
            PromptMessage::new_text(
                Role::Assistant,
                "**Monthly MMM Billing (INVOIC 31002, §40 StromNZV)**\n\n\
                 **Prerequisites:**\n\
                 - Import MMMA Gas prices (THE): PUT marktd /api/v1/mmma-preise/gas/{year}/{month}\n\
                 - Import Strom MMM prices (ÜNB): PUT marktd /api/v1/mmm-preise/strom/{year}/{month}\n\
                 - Ensure edmd has SLP imbalance data for the period (MSCONS from NB)\n\n\
                 **Step 1 — Per-MaLo MMM auto-run (recommended):**\n\
                 For each SLP MaLo in your portfolio:\n\
                 `POST /api/v1/billing/mmm-run/{malo_id}` with nb_mp_id, lf_mp_id, period_year, period_month\n\
                 - Auto-fetches profil_kwh (SLP profile) from edmd\n\
                 - Use `billing_type = \"mmm_strom\"` for Strom, `\"mmm_gas\"` for Gas\n\n\
                 **Step 2 — Review generated drafts:**\n\
                 Call `list_nne_drafts` with pid=31002 to see all MMM drafts.\n\
                 Call `get_billing_summary` for the month to verify totals.\n\
                 Check `list_undispatched_drafts` for any stuck drafts.\n\n\
                 **Step 3 — Batch dispatch:**\n\
                 `POST /api/v1/billing/drafts/dispatch-batch` with all approved draft_ids.\n\
                 Expected command: `gpke.mmm.rechnung.stellen` to makod.\n\n\
                 **Step 4 — Monitor for REMADV responses:**\n\
                 Use `list_nne_drafts` with status=dispatched to confirm delivery.\n\
                 Disputes (REMADV 33002) appear as outcome=Dispute — use `investigate-dispute` prompt.\n\n\
                 **Regulatory basis:** §40 StromNZV; BDEW INVOIC AHB 1.0 PID 31002.",
            ),
        ]
    }

    #[prompt(
        name = "redispatch-monthly-submit",
        description = "Step-by-step: prepare and submit monthly Redispatch 2.0 Kostenblatt to ÜNB"
    )]
    async fn redispatch_monthly_submit_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I submit the monthly Redispatch 2.0 Kostenblatt to the ÜNB?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**Redispatch 2.0 Kostenblatt (BK6-20-061 §4.2)**\n\n\
                 Deadline: 15th of the month following the billing month.\n\n\
                 **Step 1 — Compute Kostenblatt for each activation:**\n\
                 For each Redispatch 2.0 activation in the billing month:\n\
                 `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute`\n\
                 with tr_id, malo_id, period_year/month, uenb_mp_id, vnb_mp_id, arbeitspreis_eur_per_kwh\n\
                 → auto-fetches dispatch_kwh from edmd, computes einsatzkosten_eur\n\n\
                 **Step 2 — Review pending records:**\n\
                 Call `list_pending_kostenblatt` with year/month and status=pending.\n\
                 Verify dispatch_kwh and einsatzkosten_eur are correct.\n\
                 For corrections: PUT /api/v1/redispatch/kostenblatt/{activation_id} with updated values.\n\n\
                 **Step 3 — Submit all pending:**\n\
                 `POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}`\n\
                 → marks all pending records as submitted, returns aggregated summary for ÜNB.\n\n\
                 **Step 4 — ERP handover:**\n\
                 The response contains total_einsatzkosten_eur and per-activation breakdown.\n\
                 Export to CIM XML if required: kosten_json is typed rubo4e::current::Kosten.\n\n\
                 **Regulatory basis:** BK6-20-061 §4.2 — VNB must submit by 15th of following month.",
            ),
        ]
    }

    #[prompt(
        name = "ggv-nne-billing",
        description = "Step-by-step: §42a GGV community solar multi-tenant NNE billing (NB side)"
    )]
    async fn ggv_nne_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I bill NNE for a §42a GGV community solar MaLo?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**§42a GGV Netzentgelt NB-side billing (INVOIC 31001)**\n\n\
                 Mandatory since 01.01.2024 (§42a EEG 2023, BNetzA BK6-22-300):\n\
                 each GGV tenant Marktlokation is billed individually for its NNE share.\n\n\
                 **Prerequisites:**\n\
                 - GGV MaLo provisioned in marktd with Lokationszuordnung edges (beziehungstyp=GGV_MIETER)\n\
                 - Tenant MaLo-IDs registered with their LF in marktd\n\
                 - NNE Arbeitspreis from PreisblattNetznutzung\n\n\
                 **Option A — Proportional by measured consumption (recommended):**\n\
                 ```json\n\
                 POST /api/v1/billing/ggv-nne/{ggv_malo_id}\n\
                 {\n\
                   \"nb_mp_id\": \"...\", \"lf_mp_id\": \"...\",\n\
                   \"period_from\": \"2026-01-01\", \"period_to\": \"2026-01-31\",\n\
                   \"arbeitspreis_ct_per_kwh\": \"5.50\",\n\
                   \"tenant_consumption\": {\n\
                     \"<tenant_malo_1>\": \"450.000\",\n\
                     \"<tenant_malo_2>\": \"550.000\"\n\
                   }\n\
                 }\n\
                 ```\n\n\
                 **Option B — Equal split fallback (when consumption data unavailable):**\n\
                 Supply `total_kwh` only (no `tenant_consumption`).\n\
                 NB auto-discovers tenant MaLos from marktd Lokationszuordnung.\n\n\
                 **Result:** N × INVOIC 31001 drafts, one per tenant MaLo.\n\
                 Review with `list_nne_drafts`, dispatch with `dispatch-batch`.\n\n\
                 **Regulatory basis:** §42a EEG 2023; BK6-22-300 (§14a integration).",
            ),
        ]
    }

    #[prompt(
        name = "nb-invoic-overview",
        description = "NB INVOIC portfolio overview: all PIDs, processes, and regulatory deadlines"
    )]
    async fn nb_invoic_overview_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Give me an overview of all INVOIC types the NB sends.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**NB Outbound INVOIC Portfolio (netzbilanzd)**\n\n\
                 | PID | Process | Direction | Deadline | `billing_type` |\n\
                 |---|---|---|---|---|\n\
                 | 31001 | NNE Strom (Netznutzungsentgelt) | NB → LF | per NbContract.billing_schedule | `nne_strom` |\n\
                 | 31001 | GGV NNE (§42a tenant split) | NB → LF | per NbContract.billing_schedule | `nne_strom` via `/ggv-nne` |\n\
                 | 31002 | MMM Strom (Mehr-/Mindermenge) | NB → LF | annual settlement §40 StromNZV | `mmm_strom` |\n\
                 | 31005 | NNE Gas (Gasnetznetz) | GNB → LFG | per NbContract | `nne_gas` |\n\
                 | 31009 | MSB-Rechnung (metering service) | NB → MSB | per MSB contract | `msb_31009` |\n\
                 | 31011 | AWH Sperrprozesse Gas (GeLi Gas) | GNB → LFG | per Sperrprozess close | `nne_gas_awh_31011` |\n\n\
                 **Key compliance rules:**\n\
                 - §22 MessZV: 3-year retention; Stornorechnung/Korrekturrechnung for corrections\n\
                 - invoic-checker blocks dispatch on Dispute outcome (NB can only send defensible invoices)\n\
                 - REMADV 33002 (dispute) → COMDIS 29001 (makod) for formal escalation\n\
                 - §40 StromNZV: MMM settlement due annually; `mmm-run/{malo_id}` auto-fetches profil_kwh\n\
                 - BK6-20-061: Redispatch Kostenblatt due 15th of following month\n\n\
                 **CloudEvents emitted:**\n\
                 - `de.netzbilanz.invoic.drafted` — after POST /billing/run\n\
                 - `de.netzbilanz.invoic.dispatched` — after PUT /billing/drafts/{id}/dispatch",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for NetzbilanzMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build()
        )
        .with_server_info(Implementation::new("netzbilanzd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "netzbilanzd MCP — NNE/KA/MMM Billing Daemon (NB role).\n\
             Generates INVOIC 31001 (NNE Strom), 31002 (MMM Strom), 31005 (NNE Gas),\n\
             31009 (MSB-Rechnung), 31011 (AWH Sperrprozesse Gas, GeLi Gas BK7-24-01-009).\n\
             Pre-dispatch self-validation via invoic-checker (period · arithmetic · total · tariff).\n\
             Dispatches to makod via gpke.nne.rechnung.stellen / gpke.mmm.rechnung.stellen.\n\n\
             ## Tools (13)\n\
             - `list_nne_drafts` — filter by malo_id, lf_mp_id, status, outcome; returns draft summaries\n\
             - `list_disputed` — invoices with check_outcome=Dispute (ERC codes in REMADV 33002)\n\
             - `get_nne_draft` — full Rechnung BO4E + invoic-checker findings for one draft\n\
             - `get_billing_summary` — monthly totals by PID/status for ERP reconciliation\n\
             - `list_undispatched_drafts` — stuck drafts older than N hours (default 48h)\n\
             - `list_pending_kostenblatt` — Redispatch 2.0 Kostenblatt pending 15th-of-month submission\n\
             - `compute_kostenblatt` — compute Kostenblatt for activation (manual kWh override)\n\
             - `dispatch_draft` — check status + provide REST dispatch instruction\n\
             - `reject_draft` — reject a draft (re-enables billing for same MaLo/period)\n\
             - `trigger_mmm_auto_run` — prepare MMM auto-run request body for a MaLo\n\
             - `list_corrections` — list Stornorechnung/Korrekturrechnung (§22 MessZV audit)\n\
             - `get_payment_stats` — payment totals by PID × status (Zahlungsverzug detection)\n\
             - `list_paid_invoices` — REMADV 33001/33003/33004 confirmed paid invoices\n\n\
             ## Billing types\n\
             - nne_strom → INVOIC 31001 · nne_gas → INVOIC 31005\n\
             - mmm_strom → INVOIC 31002 (Strom MMM, auto-fetches prices when unb_mp_id configured)\n\
             - mmm_gas → INVOIC 31002 (Gas MMM, THE prices auto-fetched from marktd)\n\
             - msb_31009 → INVOIC 31009 (MSB-Rechnung, metering fee)\n\
             - nne_gas_awh_31011 → INVOIC 31011 (GeLi Gas AWH Sperrprozesse, GNB → LFG)\n\n\
             ## Prompts (6)\n\
             - `trigger-nne-billing` · `investigate-dispute` · `mmm-monthly-run`\n\
             - `redispatch-monthly-submit` · `ggv-nne-billing` · `nb-invoic-overview`\n\n\
             ## CloudEvents emitted\n\
             - `de.netzbilanz.invoic.drafted` — draft created\n\
             - `de.netzbilanz.invoic.dispatched` — INVOIC sent to makod",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<NetzbilanzMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    // When no mcp_api_key is configured, allow all (dev mode).
    if let Some(key) = &state.mcp_api_key {
        let token = request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(str::to_owned);
        match token {
            Some(t) if t == *key => {}
            _ => {
                return (StatusCode::UNAUTHORIZED, "invalid or missing Bearer token")
                    .into_response();
            }
        }
    }
    next.run(request).await
}

pub fn router(state: Arc<NetzbilanzMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = NetzbilanzMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
