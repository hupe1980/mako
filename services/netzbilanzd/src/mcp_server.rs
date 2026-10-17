//! MCP server for `netzbilanzd` — **read-only**.
//!
//! # Why nothing here mutates
//!
//! An invoice is a legally binding document; dispatching one sends EDIFACT to a
//! counterparty and starts a payment obligation, and a Stornorechnung is the
//! only way back. Model output is untrusted input, so a tool that dispatches or
//! rejects on a model's say-so is a tool that can bill the wrong counterparty
//! from a hallucinated draft ID. Reads live here; the operator acts through the
//! REST API, where the action is attributable.
//!
//! ## Tools
//!
//! | Tool | Purpose |
//! |---|---|
//! | `list_drafts` | filter invoices by MaLo, party, PID, Sparte, status, Rechnungsart, verdict |
//! | `get_draft` | one invoice: full BO4E document, checker findings, engine warnings |
//! | `list_disputed` | invoices the counterparty rejected, with their ERC codes |
//! | `list_undispatched` | drafts sitting past their dispatch window |
//! | `list_corrections` | the Storno / Korrektur chain |
//! | `get_billing_summary` | monthly totals by PID, Sparte, status and Rechnungsart |
//! | `list_pending_kostenblatt` | Redispatch cost sheets awaiting the 15th |
//! | `list_kostenblatt_gaps` | activations registered but never quantified |
//! | `get_submission_status` | MaBiS: latest runs + open-Korrekturbedarf and failed counts |
//! | `list_failed_submissions` | MaBiS: failed runs, newest first, with `attempt_count` |
//! | `get_submission_run` | MaBiS: one Summenzeitreihe run by UUID |
//! | `list_korrekturbedarf` | MaBiS: open negative Prüfmitteilungen (§9.8.1 obligations) |
//!
//! Filing a Summenzeitreihe is not a tool either: it is a binding filing with
//! the BIKO and stays behind `POST /api/v1/mabis/sync`, where Cedar authorises
//! a person.

use std::sync::Arc;

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
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::mabis::pg::SubmissionRunRow;
use crate::mabis::server::{rfc3339, rfc3339_opt};
use crate::pg::{self, DraftFilter};

/// Shared state for the MCP tools.
#[derive(Clone)]
pub struct NetzbilanzMcpState {
    /// Database pool.
    pub pool: PgPool,
    /// The tenant every query is scoped to.
    pub tenant: String,
    /// Authentication for `/mcp`: an OIDC bearer verified and checked against
    /// the `use-mcp` Cedar action, or a configured `[mcp]` key for agent
    /// clients that mint no token.
    pub auth: mako_service::mcp_auth::McpAuth,
}

/// Filters for `list_drafts`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDraftsParams {
    /// 11-digit MaLo-ID.
    pub malo_id: Option<String>,
    /// Issuing party MP-ID — the MSB for PID 31009, the NB otherwise.
    pub sender_mp_id: Option<String>,
    /// Billed party MP-ID.
    pub recipient_mp_id: Option<String>,
    /// BDEW Prüfidentifikator: 31001, 31002, 31005, 31009 or 31011.
    pub pid: Option<i32>,
    /// `Strom` or `Gas`. PID 31002 (NN-Rechnung) and 31005 (Mehr-/Mindermengen)
    /// are shared between the Sparten, so the Prüfidentifikator alone cannot
    /// separate a gas invoice from an electricity one.
    pub sparte: Option<String>,
    /// `draft` · `dispatched` · `paid` · `disputed` · `rejected`.
    pub status: Option<String>,
    /// `RECHNUNG` · `STORNORECHNUNG` · `KORREKTURRECHNUNG`.
    pub rechnungsart: Option<String>,
    /// `invoic-checker` verdict: `Ok` · `Warn` · `Dispute`.
    pub check_outcome: Option<String>,
    /// Maximum rows (default 50, capped at 500).
    pub limit: Option<i64>,
}

/// Narrow a model-supplied Sparte to the stored code.
///
/// A value that is neither Strom nor Gas is an error rather than an ignored
/// filter: silently dropping it would answer a narrower question with the wider
/// answer, and the caller has no way to tell.
fn sparte_code(raw: Option<&str>) -> Result<Option<&'static str>, McpError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v.eq_ignore_ascii_case("strom") => Ok(Some("STROM")),
        Some(v) if v.eq_ignore_ascii_case("gas") => Ok(Some("GAS")),
        Some(other) => Err(McpError::invalid_params(
            format!("sparte must be Strom or Gas, not {other:?}"),
            None,
        )),
    }
}

/// A single draft by UUID.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DraftIdParams {
    /// The draft UUID.
    pub id: String,
}

/// A calendar month.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MonthParams {
    /// Calendar year.
    pub year: i32,
    /// Calendar month, 1–12.
    pub month: u8,
}

/// Age threshold for the undispatched listing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StaleParams {
    /// Report drafts older than this many hours (default 48).
    pub older_than_hours: Option<i64>,
}

/// How many recent MaBiS runs `get_submission_status` reads.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmissionStatusParams {
    /// Maximum recent runs to include (default 10).
    pub limit: Option<i64>,
}

/// How many failed MaBiS runs `list_failed_submissions` reads.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListFailedSubmissionsParams {
    /// Maximum results (default 20).
    pub limit: Option<i64>,
}

/// A single MaBiS submission run by UUID.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmissionRunParams {
    /// UUID of the submission run.
    pub id: String,
}

/// One MaBiS run, in the wire shape the REST surface serves — RFC 3339
/// instants, never `time`'s derived component arrays. `version` matters most:
/// §3.8.2 identifies a Summenzeitreihe by it, and the Prüfmitteilung endpoint
/// parses it back as RFC 3339.
fn run_json(r: &SubmissionRunRow) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "bilanzierungsgebiet_id": r.bilanzierungsgebiet_id,
        "period_from": r.period_from.to_string(),
        "period_to": r.period_to.to_string(),
        "version": rfc3339(r.version),
        "abrechnungslauf": r.abrechnungslauf,
        "phase": r.phase,
        "datenstatus": r.datenstatus,
        "status": r.status,
        "malo_count": r.malo_count,
        "interval_count": r.interval_count,
        "total_kwh": r.total_kwh,
        "has_substituted": r.has_substituted,
        "triggered_at": rfc3339(r.triggered_at),
        "submitted_at": rfc3339_opt(r.submitted_at),
        "acked_at": rfc3339_opt(r.acked_at),
        "message_ref": r.message_ref,
        "error_msg": r.error_msg,
        "attempt_count": r.attempt_count,
    })
}

/// The MCP handler.
#[derive(Clone)]
pub struct NetzbilanzMcpHandler {
    state: Arc<NetzbilanzMcpState>,
    #[allow(
        dead_code,
        reason = "reached only in some feature or target combinations"
    )]
    tool_router: ToolRouter<NetzbilanzMcpHandler>,
    #[allow(
        dead_code,
        reason = "reached only in some feature or target combinations"
    )]
    prompt_router: PromptRouter<NetzbilanzMcpHandler>,
}

/// Render a serialisable value as an MCP tool result.
fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let value =
        serde_json::to_value(value).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    ContentBlock::json(value)
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
}

fn internal(e: &anyhow::Error) -> McpError {
    tracing::warn!(error = ?e, "netzbilanzd MCP: query failed");
    McpError::internal_error("query failed", None)
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
        description = "List invoices this Netzbetreiber issued: Abschlagsrechnung (PID 31001), \
                       NN-Rechnung (31002, Strom and Gas), Mehr-/Mindermengensaldo (31005), \
                       MSB-Rechnung (31009) and GeLi Gas AWH (31011). Filter by MaLo, party, \
                       Prüfidentifikator, Sparte, status, Rechnungsart or invoic-checker verdict. \
                       Returns summaries; use get_draft for the full document.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_drafts(
        &self,
        Parameters(p): Parameters<ListDraftsParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = DraftFilter {
            malo_id: p.malo_id.as_deref(),
            sender_mp_id: p.sender_mp_id.as_deref(),
            recipient_mp_id: p.recipient_mp_id.as_deref(),
            pid: p.pid,
            sparte: sparte_code(p.sparte.as_deref())?,
            status: p.status.as_deref(),
            check_outcome: p.check_outcome.as_deref(),
            rechnungsart: p.rechnungsart.as_deref(),
            after: None,
            limit: p.limit.unwrap_or(50).clamp(1, 500),
        };
        let rows = pg::list_drafts(&self.state.pool, &self.state.tenant, &filter)
            .await
            .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({ "count": rows.len(), "drafts": rows }))
    }

    #[tool(
        description = "Fetch one invoice in full: the BO4E Rechnung, the settlement input it was \
                       computed from, every invoic-checker finding, and every engine warning \
                       (an omitted levy, a Konzessionsabgabe above the KAV §2 ceiling).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_draft(
        &self,
        Parameters(p): Parameters<DraftIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id must be a UUID", None))?;
        match pg::fetch_draft(&self.state.pool, &self.state.tenant, id)
            .await
            .map_err(|e| internal(&e))?
        {
            Some(row) => json_result(&row),
            None => Err(McpError::invalid_params(
                format!("no draft {id} for this tenant"),
                None,
            )),
        }
    }

    #[tool(
        description = "List invoices the counterparty rejected by REMADV (33002/33003/33004), with \
                       the EDIFACT ERC code and the stated reason. These need a Storno plus a \
                       Korrekturrechnung, or a COMDIS 29001 escalation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_disputed(
        &self,
        Parameters(p): Parameters<ListDraftsParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = DraftFilter {
            malo_id: p.malo_id.as_deref(),
            sparte: sparte_code(p.sparte.as_deref())?,
            status: Some("disputed"),
            limit: p.limit.unwrap_or(50).clamp(1, 500),
            ..DraftFilter::default()
        };
        let rows = pg::list_drafts(&self.state.pool, &self.state.tenant, &filter)
            .await
            .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({
            "count": rows.len(),
            "disputed": rows,
            "next": "POST /api/v1/billing/drafts/{id}/storno, then /korrektur with fixed inputs",
        }))
    }

    #[tool(
        description = "List drafts still undispatched after N hours (default 48). Drafts the checker \
                       disputed are excluded — those are blocked, not overdue.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_undispatched(
        &self,
        Parameters(p): Parameters<StaleParams>,
    ) -> Result<CallToolResult, McpError> {
        let hours = p.older_than_hours.unwrap_or(48).clamp(1, 8_760);
        let rows = pg::list_undispatched_stale(&self.state.pool, &self.state.tenant, hours, 200)
            .await
            .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({
            "older_than_hours": hours,
            "count": rows.len(),
            "drafts": rows,
        }))
    }

    #[tool(
        description = "List Stornorechnungen and Korrekturrechnungen — the § 147 AO / GoBD \
                       correction chain. Each links to the invoice it corrects and records why.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_corrections(
        &self,
        Parameters(p): Parameters<ListDraftsParams>,
    ) -> Result<CallToolResult, McpError> {
        // One query over both Rechnungsarten: two separately limited listings
        // concatenated return up to twice the limit, each truncated against its
        // own window.
        let rows = pg::list_corrections(
            &self.state.pool,
            &self.state.tenant,
            p.malo_id.as_deref(),
            p.limit.unwrap_or(50).clamp(1, 500),
        )
        .await
        .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({ "count": rows.len(), "corrections": rows }))
    }

    #[tool(
        description = "Monthly billing totals — net, Umsatzsteuer and gross — grouped by \
                       Prüfidentifikator, Sparte, status and Rechnungsart. Amounts are in units \
                       of 10⁻⁵ EUR, so divide by 100000 for euro.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_billing_summary(
        &self,
        Parameters(p): Parameters<MonthParams>,
    ) -> Result<CallToolResult, McpError> {
        if !(1..=12).contains(&p.month) {
            return Err(McpError::invalid_params("month must be 1–12", None));
        }
        let rows = pg::billing_summary(&self.state.pool, &self.state.tenant, p.year, p.month)
            .await
            .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({
            "year": p.year,
            "month": p.month,
            "by_group": rows,
        }))
    }

    #[tool(
        description = "List Redispatch 2.0 Kostenblatt records still pending submission to the ÜNB. \
                       BK6-20-061 §4.2 makes them due on the 15th of the following month.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pending_kostenblatt(
        &self,
        Parameters(p): Parameters<MonthParams>,
    ) -> Result<CallToolResult, McpError> {
        let (year, month) = month_key(&p)?;
        let rows = pg::list_kostenblatt(
            &self.state.pool,
            &self.state.tenant,
            year,
            month,
            Some("pending"),
        )
        .await
        .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({
            "year": p.year,
            "month": p.month,
            "pending": rows.len(),
            "deadline": format!("{}-{:02}-15", if p.month == 12 { p.year + 1 } else { p.year },
                                if p.month == 12 { 1 } else { p.month + 1 }),
            "records": rows,
        }))
    }

    #[tool(
        description = "List Redispatch activations registered for a month whose dispatched energy \
                       was never established. Each needs a compute call before the 15th, or its \
                       Einsatzkosten go to the ÜNB as zero.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_kostenblatt_gaps(
        &self,
        Parameters(p): Parameters<MonthParams>,
    ) -> Result<CallToolResult, McpError> {
        let (year, month) = month_key(&p)?;
        let rows = pg::list_kostenblatt_gaps(&self.state.pool, &self.state.tenant, year, month)
            .await
            .map_err(|e| internal(&e))?;
        json_result(&serde_json::json!({
            "year": p.year,
            "month": p.month,
            "gaps": rows.len(),
            "action": "POST /api/v1/redispatch/kostenblatt/{activation_id}/compute",
            "records": rows,
        }))
    }

    #[tool(
        description = "MaBiS submission health snapshot: the most recent Summenzeitreihe runs (with status, Datenstatus and attempt_count), plus failed-run and open-Korrekturbedarf counts. BK6-24-174 Anlage 3 §3.10: Erstaufschlag closes 10 Werktage after month end, Clearing 30.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_submission_status(
        &self,
        Parameters(p): Parameters<SubmissionStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(10).clamp(1, 100);
        let runs = crate::mabis::pg::list_runs(&self.state.pool, &self.state.tenant, limit)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let failed = runs.iter().filter(|r| r.status == "failed").count();
        let exhausted = runs
            .iter()
            .filter(|r| r.status == "failed" && r.attempt_count >= 3)
            .count();
        let korrekturbedarf =
            crate::mabis::pg::open_korrekturbedarf(&self.state.pool, &self.state.tenant)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?
                .len();

        ContentBlock::json(serde_json::json!({
            "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
            "failed_count": failed,
            // Past the scheduler's retry guard — these do not retry themselves.
            "retry_exhausted_count": exhausted,
            "open_korrekturbedarf": korrekturbedarf,
            "note": if exhausted > 0 || korrekturbedarf > 0 {
                "ACTION REQUIRED: retry-exhausted runs or open Korrekturbedarf need an operator"
            } else if failed > 0 {
                "Failed runs present; the scheduler retries while attempt_count < 3"
            } else {
                "Submission cycle healthy"
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "List failed Summenzeitreihe submission runs, newest first. attempt_count >= 3 means the scheduler has stopped retrying (retry-exhausted) and an operator must act. error_msg carries the aggregation or BIKO submission failure.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_failed_submissions(
        &self,
        Parameters(p): Parameters<ListFailedSubmissionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(20).clamp(1, 100);
        let runs = crate::mabis::pg::list_failed_runs(&self.state.pool, &self.state.tenant, limit)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        ContentBlock::json(serde_json::json!({
            "count": runs.len(),
            "failed_runs": runs.iter().map(run_json).collect::<Vec<_>>(),
            "regulatory_note":
                "BK6-24-174 Anlage 3 §3.10: a version filed within the Erstaufschlag window \
                 becomes Abrechnungsdaten directly; after it, corrections start as Prüfdaten \
                 and need a positive Prüfmitteilung.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get a single Summenzeitreihe submission run by UUID, including version (RFC 3339), phase, Datenstatus, MSCONS message_ref and error_msg.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_submission_run(
        &self,
        Parameters(p): Parameters<SubmissionRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match crate::mabis::pg::get_run(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(r)) => ContentBlock::json(run_json(&r))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("run {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List open Korrekturbedarf: negative Prüfmitteilungen (§9.8.1) with no correcting submission yet. Each must be answered by a corrected Summenzeitreihe under a higher version within the Clearing window (30 Werktage after month end).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_korrekturbedarf(&self) -> Result<CallToolResult, McpError> {
        let rows = crate::mabis::pg::open_korrekturbedarf(&self.state.pool, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let items: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(id, gebiet, from, to, version)| {
                serde_json::json!({
                    "pruefmitteilung_id": id.to_string(),
                    "bilanzierungsgebiet_id": gebiet,
                    "period_from": from.to_string(),
                    "period_to": to.to_string(),
                    "version": rfc3339(version),
                })
            })
            .collect();
        ContentBlock::json(serde_json::json!({
            "count": items.len(),
            "korrekturbedarf": items,
            "regulatory_note":
                "BK6-24-174 Anlage 3 §9.8.1: answered by a corrected Summenzeitreihe under a \
                 higher version — an operator triggers it via POST /api/v1/mabis/sync with \
                 corrects_run_id.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

/// Validate and narrow a month key to the column types.
fn month_key(p: &MonthParams) -> Result<(i16, i16), McpError> {
    if !(1..=12).contains(&p.month) {
        return Err(McpError::invalid_params("month must be 1–12", None));
    }
    let year = i16::try_from(p.year)
        .map_err(|_| McpError::invalid_params("year is out of range", None))?;
    Ok((year, i16::from(p.month)))
}

#[prompt_router]
impl NetzbilanzMcpHandler {
    #[prompt(
        name = "nb-invoic-overview",
        description = "What the NB bills, under which Prüfidentifikator, and in which direction"
    )]
    async fn nb_invoic_overview_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Which invoices does netzbilanzd issue, and how are they structured?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**Outbound invoices**\n\n\
                 | PID | Document | Direction | `billing_type` |\n\
                 |---|---|---|---|\n\
                 | 31001 | Abschlagsrechnung (payment on account) | NB → LF | `abschlag` |\n\
                 | 31002 | NN-Rechnung (Netznutzungsentgelt + Konzessionsabgabe) | NB → LF | `nne` |\n\
                 | 31005 | Mehr-/Mindermengensaldo | NB → LF | `mmm` |\n\
                 | 31009 | MSB-Rechnung (Messstellenbetrieb) | **MSB → NB/LF/ESA** | `msb` |\n\
                 | 31011 | Rechnung sonstige Leistung (GeLi Gas AWH Sperrprozesse) | GNB → LFG | `gas_awh` |\n\n\
                 **Sparte is a field, not a Prüfidentifikator.** NN-Rechnung Strom and Gas \
                 share 31002, and so do the two MMM variants share 31005. Every position \
                 carries `sparte`, which selects StromNEV §21 or GasNEV §15, decides whether \
                 the three EnFG network levies apply, and reaches the wire on `Rechnung.sparte`.\n\n\
                 **31009 is inverted.** The Messstellenbetreiber issues it in all seven of its \
                 Anwendungsfälle (PID overview 4.0); it is never addressed to one. The draft \
                 stores the MSB as `sender_mp_id`.\n\n\
                 **Lifecycle:** `draft` → `dispatched` → `paid` | `disputed`; `draft` → `rejected`. \
                 Rejecting reopens the period for re-billing. Once dispatched, the way back is a \
                 Stornorechnung, then a Korrekturrechnung.\n\n\
                 **Abschläge.** A PID 31001 Abschlagsrechnung prices no energy — one \
                 Positionszeile, one amount. The invoice that closes the period lists the \
                 Abschläge it settles and deducts them from what is **owed**: §14 Abs. 5 UStG \
                 taxed each Anzahlung on receipt, so the net and the tax stand and only \
                 `zuZahlen` moves. A reversed Abschlag is refused (INVOIC AHB [519]) and the \
                 amount always comes from the stored document ([526]). A period carries many \
                 instalments but at most **one per Rechnungsdatum** — that is what separates a \
                 cadence from a replayed billing run.\n\n\
                 Every draft stores `zu_zahlen_eur_units` beside the three amounts the invoice \
                 states, so `get_billing_summary` can answer what is owed rather than only what \
                 was invoiced.\n\n\
                 **Retention:** § 147 Abs. 3 AO / § 14b UStG — invoices are Buchungsbelege, \
                 eight years (reduced from ten with effect from 01.01.2025).",
            ),
        ]
    }

    #[prompt(
        name = "run-nne-billing",
        description = "Step-by-step: settle and dispatch a Netznutzungsentgelt invoice"
    )]
    async fn run_nne_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I bill Netznutzungsentgelt for a MaLo?"),
            PromptMessage::new_text(
                Role::Assistant,
                "**1. Settle** — `POST /api/v1/billing/run`\n\
                 ```json\n\
                 {\n  \"invoice_date\": \"2026-02-01\",\n  \"due_date\": \"2026-03-03\",\n\
                 \x20 \"rechnungskreis\": \"NNE\",\n  \"positions\": [{\n\
                 \x20   \"malo_id\": \"51238696012\",\n\
                 \x20   \"period_from\": \"2026-01-01\", \"period_to\": \"2026-01-31\",\n\
                 \x20   \"settlement\": {\n\
                 \x20     \"billing_type\": \"nne\",\n\
                 \x20     \"nb_mp_id\": \"9900357000004\", \"lf_mp_id\": \"9900012345678\",\n\
                 \x20     \"sparte\": \"Strom\",\n\
                 \x20     \"arbeitspreis\": { \"Einheitlich\": { \"menge_kwh\": \"1500\", \"preis_ct_per_kwh\": \"3.5\" } },\n\
                 \x20     \"konzessionsabgabe\": { \"satz_ct_per_kwh\": \"0.11\", \"klasse\": \"Sondervertragskunde\" }\n\
                 \x20   }\n  }]\n}\n\
                 ```\n\
                 The invoice states 19 % Umsatzsteuer: Netznutzung is a sonstige Leistung, which \
                 UStAE 13b.3a excludes from §13b. A Mehr-/Mindermenge is a Lieferung and may be \
                 reverse-charged — see `mmm-monthly-run`.\n\n\
                 The invoice number is allocated by the service (§14 Abs. 4 Nr. 4 UStG); \
                 `rechnungskreis` only names the series.\n\n\
                 **2. Read the verdict.** The response carries `check_outcome` plus every \
                 invoic-checker finding and engine warning. `Warn` is worth reading — a \
                 `KA_ABOVE_KAV_MAXIMUM` warning means the Konzessionsabgabe exceeds the KAV §2 \
                 Höchstbetrag for the customer group you named.\n\n\
                 **3. Review** — `get_draft`, or `GET /api/v1/billing/drafts/{id}`.\n\n\
                 **4. Dispatch** — `PUT /api/v1/billing/drafts/{id}/dispatch`. Any Fremdkosten \
                 attached to the draft are merged into the document first, and the whole thing \
                 is re-checked; a `Dispute` verdict blocks the send.\n\n\
                 **§14a EnWG**: pass `arbeitspreis` as `Modul1Pauschal`, \
                 `Modul2ProzentualeReduzierung` or `Modul3ZeitVariabel` (all three HT/ST/NT \
                 bands) instead of `Einheitlich`.",
            ),
        ]
    }

    #[prompt(
        name = "mmm-monthly-run",
        description = "Step-by-step: the monthly Mehr-/Mindermengen settlement"
    )]
    async fn mmm_monthly_run_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I run the monthly MMM settlement?"),
            PromptMessage::new_text(
                Role::Assistant,
                "**Prerequisites** — the published prices must be in `marktd`:\n\
                 - Gas: `PUT /api/v1/mmma-preise/gas/{year}/{month}` (Trading Hub Europe)\n\
                 - Strom: `PUT /api/v1/mmm-preise/strom/{year}/{month}` (the nationwide \
                   BDEW series, no per-operator variant — § 13 Abs. 3 StromNZV for a \
                   period to 31.12.2025, § 20 Abs. 3 EnWG via BK6-24-174 from \
                   01.01.2026)\n\n\
                 **Per MaLo** — `POST /api/v1/billing/mmm-run/{malo_id}` with `nb_mp_id`, \
                 `lf_mp_id`, `sparte`, `period_year`, `period_month` and `bilanziert_kwh`.\n\n\
                 `bilanziert_kwh` is **required and cannot be auto-fetched**: it is what the \
                 Bilanzkreis was charged from the load profile, which lives on the balancing \
                 side. `edmd` holds only the measured half. Supplying the measured total for \
                 both halves makes every saldo structurally zero.\n\n\
                 `sparte` also decides the balancing day `edmd` aggregates over — gas balances \
                 on the 06:00 Gastag, not the calendar day.\n\n\
                 **Umsatzsteuer.** A Mehr-/Mindermenge is a *Lieferung*, not a network service, \
                 so §13b Abs. 2 Nr. 5 Buchst. b can shift the tax to the recipient. State both \
                 facts on `wiederverkaeufer` — electricity needs supplier **and** recipient to \
                 hold §3g status, gas needs the recipient alone — evidenced by a valid USt 1 TH. \
                 Getting it wrong is a §14c Abs. 1 liability, not a rounding error.\n\n\
                 **Sign convention** (GPKE BK6-24-174 Teil 1 Kap. 8.4, GaBi Gas 2.1 Tenor Nr. 5), \
                 defined from the network operator's side and the opposite of the intuitive \
                 reading:\n\
                 - measured **below** profiled → ungewollte *Mehrmenge* → the NB **credits** the LF\n\
                 - measured **above** profiled → ungewollte *Mindermenge* → the NB **charges** the LF\n\n\
                 **Then** review with `list_drafts` (`pid=31005`) and \
                 `POST /api/v1/billing/drafts/dispatch-batch`.",
            ),
        ]
    }

    #[prompt(
        name = "investigate-dispute",
        description = "Step-by-step: work a REMADV Abweisung"
    )]
    async fn investigate_dispute_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "A REMADV Abweisung came back. What now?"),
            PromptMessage::new_text(
                Role::Assistant,
                "REMADV **33001** is the only Zahlungsbestätigung. 33002, 33003 and 33004 are \
                 all Abweisungen — 33003/33004 are the itemised Strom rejections, not partial \
                 payments.\n\n\
                 **1.** `list_disputed` — the invoices, their ERC codes and the stated reasons.\n\
                 **2.** `get_draft` — read `check_findings` and `settlement_warnings` first. If \
                 the NB's own checker already warned, the counterparty is usually right.\n\
                 **3.** Fix the cause where it lives: the price sheet in `marktd`, the readings \
                 in `edmd`, the master data behind the KAV group or Netzebene.\n\
                 **4.** `POST /api/v1/billing/drafts/{id}/storno` with a `grund` \
                 (`Messwertkorrektur`, `Tarifkorrektur`, `Stammdatenkorrektur`, \
                 `RegulatorischeAenderung`, `Rechenfehler`, `Clearing`, `Sonstiges`). The \
                 reversal is recomputed from the stored settlement input and negated, and it \
                 declares itself a Storno on `ist_storno` + `original_rechnungsnummer`. The \
                 recomputation must reproduce the original's net, tax **and** gross exactly, or \
                 the reversal is refused with both figures named.\n\
                 **5.** `POST /api/v1/billing/drafts/{id}/korrektur` with the corrected \
                 settlement — a new settlement from corrected inputs, not an edited document. \
                 It carries the *whole* corrected amount, so step 4 is required first and the \
                 endpoint answers 409 without it; otherwise the period is billed twice. The \
                 corrected settlement must keep the original's settlement kind, Sparte and both \
                 counterparties: a different one of any of those is a different invoice, not a \
                 correction, and answers 422.\n\
                 Only a **dispatched** invoice can be reversed or corrected. A draft or a \
                 rejected draft never reached the counterparty, so both answer 409.\n\
                 **6.** Dispatch both, or escalate through makod COMDIS 29001.\n\n\
                 The reason matters: `Rechenfehler` and `Stammdatenkorrektur` indicate a defect \
                 worth counting, `RegulatorischeAenderung` is a lawful recalculation.",
            ),
        ]
    }

    #[prompt(
        name = "ggv-nne-billing",
        description = "Step-by-step: §42b EnWG Gemeinschaftliche Gebäudeversorgung"
    )]
    async fn ggv_nne_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I bill NNE for a §42b GGV building?"),
            PromptMessage::new_text(
                Role::Assistant,
                "`POST /api/v1/billing/ggv-nne/{ggv_malo_id}` with `nb_mp_id`, `lf_mp_id`, the \
                 period, `arbeitspreis_ct_per_kwh` and `tenant_consumption` — a map from each \
                 tenant MaLo-ID to its **metered** kWh.\n\n\
                 `tenant_consumption` is required. §42b attributes the Netzentgelt to each \
                 tenant Marktlokation, and an equal split is not an attribution: it bills one \
                 tenant for another's consumption. Meter the tenants, or do not bill them \
                 individually.\n\n\
                 The whole building is settled in one transaction, so either every tenant is \
                 billed or none is — a partial run would leave the unbilled tenants invisible \
                 and trip the double-billing guard on a retry.\n\n\
                 The response reports each tenant's share of the metered total; the shares add \
                 to 100 %.",
            ),
        ]
    }

    #[prompt(
        name = "redispatch-monthly-submit",
        description = "Step-by-step: the monthly Redispatch 2.0 Kostenblatt"
    )]
    async fn redispatch_monthly_submit_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "How do I submit the monthly Kostenblatt?"),
            PromptMessage::new_text(
                Role::Assistant,
                "Due the 15th of the following month (BK6-20-061 §4.2).\n\n\
                 **1. Find what is missing** — `list_kostenblatt_gaps` for the month: \
                 activations registered but never quantified. Each would otherwise be submitted \
                 with zero Einsatzkosten.\n\n\
                 **2. Quantify each** — `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute` \
                 with `tr_id`, `malo_id`, the period, `uenb_mp_id`, `vnb_mp_id`, the activation \
                 window and `arbeitspreis_eur_per_kwh`.\n\n\
                 The energy comes from the `edmd` Lastgang summed over the exact activation \
                 window. Check `dispatch_source` on the result: `lastgang_sum` is the intended \
                 path, `einspeisung_tag` means the delivery day's Einspeisung was used because the \
                 activation window carried none — for a 15-minute activation that is wrong by \
                 up to two orders of magnitude and should be replaced with a verified \
                 `dispatch_kwh_override`.\n\n\
                 **3. Review** — `list_pending_kostenblatt`.\n\n\
                 **4. Submit** — `POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}`. \
                 The response carries the aggregate and the per-activation breakdown; \
                 `kosten_json` is a typed BO4E `Kosten` for CIM export.\n\n\
                 Compensation to the curtailed operator is a separate document: \
                 `POST /api/v1/redispatch/verguetung/{activation_id}/compute` (§13a Abs. 2 EnWG). \
                 Note that an Aufforderungsfall settles against the transmitted schedule, not \
                 against the measured Lastgang.",
            ),
        ]
    }

    #[prompt(
        name = "submission-triage",
        description = "Step-by-step: triage a failed Summenzeitreihe submission or an open Korrekturbedarf"
    )]
    async fn submission_triage_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A MaBiS submission failed or the BIKO objected. How do I triage it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**MaBiS Summenzeitreihe triage (BK6-24-174 Anlage 3)**\n\n\
                 **Step 1 — Snapshot**\n\
                 Call `get_submission_status`.\n\
                 - `retry_exhausted_count > 0`: the scheduler has given up (attempt_count >= 3) — go to Step 2.\n\
                 - `open_korrekturbedarf > 0`: the BIKO objected — go to Step 3.\n\
                 - only `failed_count > 0`: the scheduler still retries; watch, don't act.\n\n\
                 **Step 2 — Failed runs**\n\
                 Call `list_failed_submissions` and read `error_msg` per run:\n\
                 - Aggregation errors (edmd data missing): check whether the Lastgänge for the \
                 period exist and are complete before any retry.\n\
                 - BIKO submission errors (makod dispatch): a transport problem — the MSCONS \
                 never went out, so a retry files the same version safely.\n\
                 Deadline context: the Erstaufschlag window closes on the 10. Werktag after \
                 month end. A version filed later starts as Prüfdaten (needs a positive \
                 Prüfmitteilung) — say so in the escalation.\n\n\
                 **Step 3 — Korrekturbedarf**\n\
                 Call `list_korrekturbedarf`. Each entry is an open §9.8.1 obligation: a \
                 corrected Summenzeitreihe under a higher version, within the Clearing window \
                 (30 Werktage after month end). The correction itself is an operator action \
                 (`POST /api/v1/mabis/sync` with `corrects_run_id`) — name the run and the deadline \
                 in the worklist entry.\n\n\
                 **Output**: { submission_status, retry_exhausted: N, korrekturbedarf: N, action }",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for NetzbilanzMcpHandler {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("netzbilanzd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "netzbilanzd — the Netzbetreiber's outbound billing daemon.\n\n\
             Issues NN-Rechnung (PID 31002, Strom and Gas), Mehr-/Mindermengensaldo (31005), \
             MSB-Rechnung (31009, issued by the MSB) and GeLi Gas AWH (31011); carries the \
             Redispatch 2.0 Kostenblatt and the §13a Abs. 2 Vergütung; files the Summenzeitreihen \
             (MSCONS 13003) with the BIKO.\n\n\
             **This surface is read-only.** Dispatching an invoice sends EDIFACT to a \
             counterparty and starts a payment obligation, and the only way back is a \
             Stornorechnung — so settling, dispatching, rejecting and correcting all live on \
             the REST API, where the action is attributable to an operator. Read here; act there.\n\n\
             ## Tools (12)\n\
             - `list_drafts` — filter by MaLo, party, PID, Sparte, status, Rechnungsart, verdict\n\
             - `get_draft` — one invoice in full: BO4E document, settlement input, findings, warnings\n\
             - `list_disputed` — REMADV Abweisungen with their ERC codes\n\
             - `list_undispatched` — drafts past their dispatch window\n\
             - `list_corrections` — the Storno / Korrektur chain\n\
             - `get_billing_summary` — monthly totals by PID, Sparte, status, Rechnungsart\n\
             - `list_pending_kostenblatt` — Redispatch cost sheets awaiting the 15th\n\
             - `list_kostenblatt_gaps` — activations registered but never quantified\n\
             - `get_submission_status(limit)` — MaBiS: recent Summenzeitreihe runs + failed / \
             retry-exhausted / Korrekturbedarf counts\n\
             - `list_failed_submissions(limit)` — MaBiS: failed runs with attempt_count and error_msg\n\
             - `get_submission_run(id)` — MaBiS: one run, full detail\n\
             - `list_korrekturbedarf` — MaBiS: open negative Prüfmitteilungen (§9.8.1)\n\n\
             ## Prompts (7)\n\
             `nb-invoic-overview` · `run-nne-billing` · `mmm-monthly-run` · \
             `investigate-dispute` · `ggv-nne-billing` · `redispatch-monthly-submit` · \
             `submission-triage`\n\n\
             ## MaBiS timing (BK6-24-174 Anlage 3 §3.10)\n\
             Filing a Summenzeitreihe is not exposed here: it is a binding filing with the BIKO \
             and stays behind the authenticated REST surface. Erstaufschlag: 10 Werktage after \
             month end — a new version becomes Abrechnungsdaten directly. Clearing: 30 Werktage \
             — a new version is Prüfdaten until a positive Prüfmitteilung. After 30 Werktage: KBKA.",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<NetzbilanzMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

/// The MCP router, mounted at `/mcp`.
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
