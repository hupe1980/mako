//! MCP server for `processd`.
//!
//! Exposes STP (Standardisierter Technischer Prozess) decisions and LF approval
//! queue reads via the MCP Streamable HTTP transport (spec 2025-11-25).
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_decisions`            | List recent NB Anmeldung STP decisions |
//! | `get_decision`              | Get a single decision by process_id |
//! | `get_stp_rate`              | Approval rate for NB STP decisions over N days |
//! | `get_stp_breakdown_by_erc`  | Rejection counts by ERC code (root-cause analysis) |
//! | `list_affiliate_decisions`  | Gleichbehandlung: decisions for affiliate-initiated requests |
//! | `list_pending_approvals`    | List LF approval-queue entries needing operator action |
//! | `get_queue_entry`           | Get a single LF approval-queue entry by its UUID |
//! | `approve_queue_entry`       | Approve a pending queue entry (dispatch einwilligung) |
//! | `reject_queue_entry`        | Reject a pending queue entry (dispatch ablehnen) |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse as _,
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

/// processd's own base URL, derived from the configured `makod` URL.
///
/// The two run side by side in every deployment this ships with, so the host is
/// shared and only the port differs. The port is **replaced**, not substituted
/// textually: `replace(":8080", ":8580")` leaves a URL that states no explicit
/// port completely unchanged, so the approve `POST` goes to makod carrying
/// processd's queue path — a request that authenticates, 404s, and looks from
/// here like a processd failure.
///
/// Returns `None` when the URL has no host to rebuild from, and the caller
/// refuses rather than guessing: dispatching an irreversible market decision to
/// an address derived by accident is the outcome worth avoiding.
fn self_base_url(makod_url: &str) -> Option<String> {
    let trimmed = makod_url.trim_end_matches('/');
    let (scheme, rest) = trimmed.split_once("://")?;
    // Authority ends at the first `/`; anything after it is a path we drop,
    // since this is a base URL.
    let authority = rest.split('/').next()?;
    let host = authority.rsplit_once(':').map_or(authority, |(h, port)| {
        // Only treat the tail as a port when it is one — an IPv6 literal such
        // as `[::1]` also contains colons.
        if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
            h
        } else {
            authority
        }
    });
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}:8580"))
}

#[derive(Clone)]
pub struct ProcessdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// makod base URL — required for approve/reject dispatch.
    pub makod_url: String,
    /// makod API key for command dispatch.
    pub makod_api_key: secrecy::SecretString,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDecisionsParams {
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDecisionParams {
    /// UUID of the process (from the `subject` field of the CloudEvent).
    pub process_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStpRateParams {
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StpBreakdownParams {
    /// Number of days to look back (default 30).
    pub days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AffiliateDecisionsParams {
    /// Number of days to look back (default 90 — §20 EnWG audit window).
    pub days: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListQueueParams {
    pub status: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueueActionParams {
    /// UUID of the approval queue entry to approve or reject.
    pub id: String,
    /// Who decided, as **verified by the MCP middleware** — never the caller.
    ///
    /// `mcp_auth_middleware` overwrites whatever arrives here with the identity
    /// behind the presented token, so a caller cannot name someone else. It is
    /// `Option` because the middleware leaves it unset when nothing identifies
    /// a caller (dev mode, no Cedar), and an unattributable decision is refused
    /// rather than recorded against a placeholder: this value becomes § 20
    /// Abs. 1 EnWG parity evidence and a GoBD record.
    #[serde(default)]
    pub decided_by: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetQueueEntryParams {
    pub id: String,
}

#[derive(Clone)]
pub struct ProcessdMcpHandler {
    state: Arc<ProcessdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<ProcessdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<ProcessdMcpHandler>,
}

#[tool_router]
impl ProcessdMcpHandler {
    fn new(state: Arc<ProcessdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List recent Anmeldung STP decisions (NB role). Returns decisions ordered by decided_at descending.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_decisions(
        &self,
        Parameters(params): Parameters<ListDecisionsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let limit = params.limit.unwrap_or(50).min(200);
        match repo.list(&self.state.tenant, limit).await {
            Ok(records) => ContentBlock::json(serde_json::to_value(records).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single NB Anmeldung decision by process_id. NB role. Returns the decision outcome, ERC code (if reject), and whether §20 EnWG affiliate check was triggered.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_decision(
        &self,
        Parameters(params): Parameters<GetDecisionParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let Ok(process_id) = params.process_id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params(
                "process_id must be a valid UUID",
                None,
            ));
        };
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        match repo
            .find_by_process_id(process_id, &self.state.tenant)
            .await
        {
            Ok(Some(rec)) => ContentBlock::json(serde_json::to_value(rec).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("no decision found for process_id {process_id}"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get Anmeldung rejection breakdown by ERC code for the last N days. \
NB role. Returns (antwortcode, count) pairs ordered by frequency. \
Use this when STP drops below 95% to identify the root cause: \
A02 = MaLo nimmt nicht an der MaKo teil (Stillgelegt/Ruhend), \
A06 = andere Anmeldung in Bearbeitung (duplicate), \
A07 (Strom) / E17 (Gas) = Datums-/Fristverletzung (LFW24 future rule; \
Gas 6-week retroactive window for E01/E02, 10 WT lead for E03), \
A05 = Anforderungen nicht erfüllbar (Bilanzierungsgebiet/unknown partner). \
Escalate = data gap (grid record missing) or affiliate initiator \
(§20 EnWG — operator must approve manually).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_stp_breakdown_by_erc(
        &self,
        Parameters(params): Parameters<StpBreakdownParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let days = params.days.unwrap_or(30);
        match repo.stp_breakdown_by_erc(&self.state.tenant, days).await {
            Ok(rows) => {
                let breakdown: Vec<serde_json::Value> = rows
                    .into_iter()
                    .map(|(erc, count)| {
                        let erc_str = erc.as_deref().unwrap_or("null (internal)");
                        serde_json::json!({
                            "antwortcode": erc_str,
                            "count": count,
                            "remediation": match erc.as_deref() {
                                Some("A02") => "PUT /api/v1/malos/{malo_id}/grid in marktd (NB-role grid provisioning)",
                                Some("A05") => "PUT /api/v1/preisblaetter/{nb_mp_id} in marktd with current tariff",
                                Some("A06") => "LF submitted a date outside the valid Vorlauffrist window",
                                Some("A99") => "Internal error — check processd logs for details",
                                _ => "Unknown ERC — check decision detail field",
                            }
                        })
                    })
                    .collect();
                ContentBlock::json(serde_json::json!({
                    "window_days": days,
                    "breakdown": breakdown,
                    "note": "Only Reject decisions are included. Escalate decisions are not counted.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List NB Anmeldung decisions where `initiator_is_affiliate = true` — §20 EnWG \
Diskriminierungsfreiheitspflicht audit. Returns decisions where the LF MP-ID matches the operator's \
own MP-ID. These MUST NOT be auto-accepted (§ 20 Abs. 1 EnWG; § 7a Abs. 5 EnWG Gleichbehandlung). \
Use `obsd.get_kpi_report` for the aggregated §20 parity report.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_affiliate_decisions(
        &self,
        Parameters(params): Parameters<AffiliateDecisionsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let days = params.days.unwrap_or(90);
        let limit = params.limit.unwrap_or(100).min(500);
        match repo
            .list_affiliate_decisions(&self.state.tenant, days, limit)
            .await
        {
            Ok(records) => ContentBlock::json(serde_json::json!({
                "window_days": days,
                "count": records.len(),
                "records": serde_json::to_value(records).unwrap_or_default(),
                "regulatory_note": "§20 EnWG: affiliate-initiated Anmeldungen must not be auto-accepted. \
Every entry here must have decision=Escalate (requiring operator review). \
Any entry with decision=Accept indicates a §20 EnWG violation — report to BNetzA compliance team.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get the Anmeldung STP rate for the last N days. NB role. Returns a float 0.0–1.0 or null when no decisions exist.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_stp_rate(
        &self,
        Parameters(params): Parameters<GetStpRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let days = params.days.unwrap_or(30);
        match repo.stp_rate(&self.state.tenant, days).await {
            Ok(rate) => ContentBlock::json(serde_json::json!({
                "stp_rate": rate, "window_days": days, "target": 0.95,
                "compliant": rate.is_none_or(|r| r >= 0.95),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List LF approval-queue entries needing operator action (status: Pending/Approved/Rejected/Expired).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pending_approvals(
        &self,
        Parameters(params): Parameters<ListQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::approval::{PgApprovalQueue, QueueStatus};
        let queue = PgApprovalQueue::new(self.state.pool.clone());
        let status: Option<QueueStatus> = params
            .status
            .as_deref()
            .map(|s| s.parse().unwrap_or(QueueStatus::Pending))
            .or(Some(QueueStatus::Pending));
        let limit = params.limit.unwrap_or(50);
        match queue.list(&self.state.tenant, status, limit).await {
            Ok(entries) => ContentBlock::json(serde_json::to_value(entries).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single LF approval queue entry by its UUID.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_queue_entry(
        &self,
        Parameters(params): Parameters<GetQueueEntryParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::approval::PgApprovalQueue;
        let Ok(id) = params.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        let queue = PgApprovalQueue::new(self.state.pool.clone());
        match queue.find_by_id(id, &self.state.tenant).await {
            Ok(Some(entry)) => ContentBlock::json(serde_json::to_value(entry).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("approval queue entry {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
    #[tool(
        description = "Approve a pending LF E_0624 Einwilligung queue entry. \
Dispatches `gpke.nb-lieferende.bestaetigen` (PID 55008 Strom) or \
`geli.stornierung.initiieren` (Gas 44022/44023) to makod, then marks the entry Approved. \
⚠ Regulatory: the 45-min APERAK window applies from the original process.initiated event. \
Use `list_pending_approvals` first to check `expires_at` before approving.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn approve_queue_entry(
        &self,
        Parameters(p): Parameters<QueueActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        // processd's own approval endpoint, reached over the loopback so the
        // REST handler's Cedar check runs rather than being bypassed.
        // Set by `mcp_auth_middleware` from the verified token. Absent means
        // the decision cannot be attributed, and an unattributable § 20
        // Abs. 1 EnWG record is refused rather than written.
        let Some(decided_by) = p.decided_by.as_deref() else {
            return Err(McpError::internal_error(
                "decision not attributable to a verified caller",
                None,
            ));
        };
        let Some(base) = self_base_url(&self.state.makod_url) else {
            return Err(McpError::internal_error(
                "cannot derive processd's own base URL from the configured makod_url",
                None,
            ));
        };
        let up = mako_service::http::Upstream::new(
            "processd",
            &base,
            Some(self.state.makod_api_key.clone()),
            mako_service::http::default_client(),
        );
        match crate::server::QUEUE_APPROVE
            .request(&up, id)
            .header("X-Decided-By", decided_by)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status() == 204 => {
                ContentBlock::json(serde_json::json!({
                    "id": p.id,
                    "status": "Approved",
                    "note": "einwilligung dispatched to makod; queue entry marked Approved.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "approve failed: HTTP {}",
                resp.status()
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Reject a pending LF E_0624 Einwilligung queue entry. \
Dispatches `gpke.nb-lieferende.ablehnen` (PID 55009) to makod, then marks the entry Rejected. \
Use this when §20 EnWG affiliate check is the reason (operator override). \
For §20 parity data: use `obsd` `get_kpi_report`.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn reject_queue_entry(
        &self,
        Parameters(p): Parameters<QueueActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        // processd's own approval endpoint, reached over the loopback so the
        // REST handler's Cedar check runs rather than being bypassed.
        // Set by `mcp_auth_middleware` from the verified token. Absent means
        // the decision cannot be attributed, and an unattributable § 20
        // Abs. 1 EnWG record is refused rather than written.
        let Some(decided_by) = p.decided_by.as_deref() else {
            return Err(McpError::internal_error(
                "decision not attributable to a verified caller",
                None,
            ));
        };
        let Some(base) = self_base_url(&self.state.makod_url) else {
            return Err(McpError::internal_error(
                "cannot derive processd's own base URL from the configured makod_url",
                None,
            ));
        };
        let up = mako_service::http::Upstream::new(
            "processd",
            &base,
            Some(self.state.makod_api_key.clone()),
            mako_service::http::default_client(),
        );
        match crate::server::QUEUE_REJECT
            .request(&up, id)
            .header("X-Decided-By", decided_by)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status() == 204 => {
                ContentBlock::json(serde_json::json!({
                    "id": p.id,
                    "status": "Rejected",
                    "note": "ablehnen dispatched to makod; queue entry marked Rejected.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "reject failed: HTTP {}",
                resp.status()
            ))])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl ProcessdMcpHandler {
    #[prompt(
        name = "triage-nb-rejection",
        description = "Step-by-step: investigate why an NB Anmeldung was rejected"
    )]
    async fn triage_nb_rejection_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "An NB rejected a Lieferbeginn Anmeldung. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_decision` with the process_id to see the ERC code.\n\
                 2. ERC codes from `mako-pruefung`:\n\
                    - A02: MaLo not found in marktd (malo_grid missing or bilanzierungsgebiet mismatch)\n\
                    - A05: Preisblatt missing for the NB MP-ID + Sparte combination\n\
                    - A06: Lieferbeginn date outside allowed range (too far future / past)\n\
                    - A99: internal processing error (check processd logs)\n\
                 3. AFFILIATE (not an ERC): the decision carries `initiator_is_affiliate = true` — \
                    auto-accept was blocked under the § 20 Abs. 1 Satz 1 affiliate rule even though the checks passed. \
                    These surface via `list_affiliate_decisions`, not the ERC breakdown.\n\
                 4. Fix A02: PUT /api/v1/malos/{malo_id}/grid in marktd with correct netzebene/bilanzierungsgebiet.\n\
                 5. Fix A05: PUT /api/v1/preisblaetter/{nb_mp_id} in marktd with current tariff.\n\
                 6. Fix AFFILIATE: submit manual approval via POST /api/v1/queue/{id}/approve.",
            ),
        ]
    }

    #[prompt(
        name = "investigate-stp-drop",
        description = "Step-by-step: investigate why the NB STP rate dropped below 95%"
    )]
    async fn investigate_stp_drop_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "The NB STP rate dropped below 95%. How do I diagnose and fix this?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Call `get_stp_rate(days=7)` to confirm the current rate and scope.\n\
                 2. Call `get_stp_breakdown_by_erc(days=7)` to identify the dominant ERC code.\n\n\
                 ## By ERC code:\n\n\
                 **A02 (MaLo not found / grid data missing)**\n\
                 → Provision the grid record: PUT `/api/v1/malos/{malo_id}/grid` in marktd (NB role).\n\
                 → Verify `GET /api/v1/malos/{malo_id}/grid` returns netzebene + bilanzierungsgebiet.\n\n\
                 **A05 (Preisblatt / NB not registered)**\n\
                 → Check `GET /api/v1/partners/{nb_mp_id}` in marktd — partner must exist.\n\
                 → Check `GET /api/v1/preisblaetter/{nb_mp_id}` — must have active Preisblatt.\n\
                 → If expired: PUT /api/v1/preisblaetter/{nb_mp_id} with updated tariff.\n\n\
                 **A06 (Lieferbeginn date out of range)**\n\
                 → LF submitted a date outside the valid window (too far future or past).\n\
                 → Check UTILMD AHB for the PID-specific Vorlauffrist rules.\n\
                 → No action needed on NB side — this is an LF error.\n\n\
                 **AFFILIATE (§ 20 Abs. 1 Satz 1 affiliate rule — not an ERC)**\n\
                 → Affiliate-initiated Anmeldungen pass the checks but auto-accept is blocked; \
                   they are tracked by the `initiator_is_affiliate` marker, not an ERC bucket.\n\
                 → Call `list_affiliate_decisions(days=7)` to see affected MaLos.\n\
                 → Each entry requires manual operator review before acceptance.\n\
                 → Approve via POST /api/v1/queue/{id}/approve.\n\n\
                 3. After fixing root causes, STP should recover on the next batch of Anmeldungen.",
            ),
        ]
    }

    #[prompt(
        name = "triage-msb-wechsel",
        description = "Step-by-step: investigate an MSB-Wechsel rejection (PIDs 55039/55042)"
    )]
    async fn triage_msb_wechsel_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "An MSB-Wechsel (WiM Strom PID 55042) was rejected. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. MSB-Wechsel rejections come from `processd`'s msb_module with ERC codes:\n\
                    - A02: MeLo not found in marktd device registry — check PUT /api/v1/melos/{melo_id}/zaehler\n\
                    - A05: nMSB not registered in partner directory — add via PUT /api/v1/partners\n\
                 2. Escalations (not rejections) occur for:\n\
                    - MeLo has an iMSys device (§14a mandatory MSB — requires operator eligibility check)\n\
                    - MeLo has a SteuerbareRessource (§14a Modul check needed)\n\
                    - MeLo has no registered meters in marktd (grid data incomplete)\n\
                 3. For iMSys escalations: check §14a Modul eligibility in marktd:\n\
                    GET /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte\n\
                    If products are contracted → approve manually.\n\
                    If not → reject with ERC A97 (not eligible for nMSB assignment).\n\
                 4. For Kündigung (PID 55039): only A02 and A05 are valid rejection grounds.\n\
                    If MeLo exists and nMSB is registered → Kündigung must be accepted.",
            ),
        ]
    }

    #[prompt(
        name = "trigger-lieferbeginn",
        description = "Step-by-step: initiate a Lieferbeginn Anmeldung (Strom or Gas)"
    )]
    async fn trigger_lieferbeginn_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I trigger a Lieferbeginn Anmeldung for a MaLo?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "For Strom (GPKE UTILMD PID 55001):\n\
                 POST /api/v1/start-supply with malo_id, lieferbeginn_datum (YYYY-MM-DD).\n\
                 LFW24 Vorlauffrist: earliest Lieferbeginn is the day after the next Werktag \
                 (spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn — \
                 BK6-24-174 GPKE Teil 2, SD Lieferbeginn).\n\n\
                 For Gas (GeLi Gas UTILMD G PID 44001):\n\
                 POST /api/v1/start-supply-gas with malo_id, lieferbeginn_datum, gasqualitaet.\n\n\
                 processd validates:\n\
                 - MaLo exists in marktd with correct netzebene/bilanzierungsgebiet\n\
                 - Active Preisblatt for the NB\n\
                 - Lieferbeginn date within Vorlauffrist window\n\
                 - §20 EnWG: affiliate check (auto-accept blocked if initiator_is_affiliate)\n\n\
                 On success: makod dispatches UTILMD 55001/44001 EDIFACT to NB.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for ProcessdMcpHandler {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("processd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "processd MCP — NB Anmeldung STP decisions, §20 EnWG compliance, LF E_0624 queue.\n\
                 NB: `list_decisions`, `get_decision`, `get_stp_rate`, `get_stp_breakdown_by_erc`, `list_affiliate_decisions`.\n\
                 LF: `list_pending_approvals`, `get_queue_entry`, `approve_queue_entry`, `reject_queue_entry`.\n\
                 Prompts: `triage-nb-rejection`, `investigate-stp-drop`, `triage-msb-wechsel`, `trigger-lieferbeginn`.",
        )
    }
}

/// The Cedar action each MCP tool needs, by tool name.
///
/// The blanket `use-mcp` grant is deliberately weak — `processd.cedar` gives it
/// on tenant alone, with no role — because it gates *reaching* the surface, not
/// what may be done on it. Without a per-tool action every tool inherits that
/// one grant, so a role-less token of the right tenant reaches
/// `approve_queue_entry` and dispatches an irreversible market message.
///
/// A tool missing from this table is refused rather than defaulted: a new tool
/// that nobody mapped is exactly the one whose authority nobody has reasoned
/// about, and defaulting it open is how the gap above appears again.
fn tool_action(name: &str) -> Option<&'static str> {
    Some(match name {
        "list_decisions"
        | "get_decision"
        | "get_stp_rate"
        | "get_stp_breakdown_by_erc"
        | "list_affiliate_decisions" => "read-decisions",
        "list_pending_approvals" | "get_queue_entry" => "read-queue",
        // Irreversible: dispatches `gpke.nb-lieferende.bestaetigen` (PID 55008)
        // or `geli.stornierung.initiieren`. The market message cannot be
        // withdrawn once sent.
        "approve_queue_entry" | "reject_queue_entry" => "decide-queue",
        _ => return None,
    })
}

/// What one inbound MCP frame is, for authorization purposes.
enum McpCall {
    /// Not a `tools/call` — `initialize`, `tools/list`, a prompt. The blanket
    /// `use-mcp` gate is the whole check.
    NotATool,
    /// A `tools/call` for a tool this build knows, and the action it needs.
    Tool(&'static str),
    /// A `tools/call` naming a tool with no entry in [`tool_action`].
    UnknownTool(String),
}

fn classify(body: &[u8]) -> McpCall {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return McpCall::NotATool;
    };
    if v.get("method").and_then(serde_json::Value::as_str) != Some("tools/call") {
        return McpCall::NotATool;
    }
    let Some(name) = v
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(serde_json::Value::as_str)
    else {
        return McpCall::NotATool;
    };
    match tool_action(name) {
        Some(action) => McpCall::Tool(action),
        None => McpCall::UnknownTool(name.to_owned()),
    }
}

/// Overwrite `params.arguments.decided_by` with the verified caller's name.
///
/// Returns the rewritten frame, or `None` when the frame is not the object
/// shape a `tools/call` has — in which case nothing is guessed and the request
/// is refused.
fn stamp_decider(body: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let params = v.get_mut("params")?.as_object_mut()?;
    let args = params
        .entry("arguments")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()?;
    args.insert(
        "decided_by".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    serde_json::to_vec(&v).ok()
}

/// One MCP frame's size cap — the body must be buffered to read the tool name.
const MAX_MCP_BODY: usize = 1024 * 1024;

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<ProcessdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_MCP_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "MCP request body too large").into_response();
        }
    };
    let mut bytes = bytes;
    match classify(&bytes) {
        McpCall::NotATool => {}
        McpCall::Tool(action) => {
            if let Err(resp) = state.auth.authorize(&parts.headers, action) {
                return resp;
            }
            // An irreversible decision has to be attributable, and the only
            // identity worth recording is the one this layer just verified.
            // Stamping it into the frame is what lets the tool body see it:
            // `McpIdentity` reaches Axum extensions, which an MCP tool never
            // reads. Whatever arrived under this key is overwritten, so a
            // caller cannot name somebody else as the decider.
            if action == "decide-queue" {
                let Some(identity) = state.auth.identify(&parts.headers) else {
                    return (
                        StatusCode::FORBIDDEN,
                        "403 Forbidden: this decision cannot be attributed to a verified \
                         caller, and an unattributable § 20 Abs. 1 EnWG record is refused \
                         rather than written against a placeholder",
                    )
                        .into_response();
                };
                match stamp_decider(&bytes, &identity.name) {
                    Some(rewritten) => bytes = rewritten.into(),
                    None => {
                        return (
                            StatusCode::BAD_REQUEST,
                            "400 Bad Request: malformed tools/call frame",
                        )
                            .into_response();
                    }
                }
            }
        }
        McpCall::UnknownTool(name) => {
            tracing::warn!(tool = %name, "processd: MCP tool carries no Cedar action");
            return (
                StatusCode::FORBIDDEN,
                format!(
                    "403 Forbidden: MCP tool {name:?} carries no Cedar action, so it cannot be \
                     authorized — add it to `tool_action`"
                ),
            )
                .into_response();
        }
    }
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<ProcessdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = ProcessdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool the surface exposes maps to an action, and the two
    /// irreversible ones need more than the blanket `use-mcp` grant.
    ///
    /// `processd.cedar` grants `use-mcp` on tenant alone with no role. Without
    /// a per-tool action every tool inherits exactly that, so a role-less token
    /// of the right tenant reaches `approve_queue_entry` and dispatches a
    /// market message that cannot be withdrawn.
    #[test]
    fn the_irreversible_tools_need_the_decide_action() {
        assert_eq!(tool_action("approve_queue_entry"), Some("decide-queue"));
        assert_eq!(tool_action("reject_queue_entry"), Some("decide-queue"));
        assert_eq!(tool_action("list_pending_approvals"), Some("read-queue"));
        assert_eq!(tool_action("list_decisions"), Some("read-decisions"));
    }

    /// A tool nobody mapped is refused, not defaulted onto the blanket grant.
    #[test]
    fn an_unmapped_tool_is_refused() {
        assert_eq!(tool_action("some_future_tool"), None);
        let frame = br#"{"method":"tools/call","params":{"name":"some_future_tool"}}"#;
        assert!(matches!(classify(frame), McpCall::UnknownTool(_)));
    }

    /// `tools/list` and `initialize` are not tool calls.
    #[test]
    fn a_non_tool_frame_is_not_gated_per_tool() {
        assert!(matches!(
            classify(br#"{"method":"tools/list"}"#),
            McpCall::NotATool
        ));
        assert!(matches!(classify(b"not json"), McpCall::NotATool));
    }

    /// The decider the middleware verified replaces whatever the caller sent.
    #[test]
    fn the_stamped_decider_overwrites_a_caller_supplied_one() {
        let frame = br#"{"method":"tools/call","params":{"name":"approve_queue_entry","arguments":{"id":"x","decided_by":"someone-else"}}}"#;
        let out = stamp_decider(frame, "verified-operator").expect("stamped");
        let v: serde_json::Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(v["params"]["arguments"]["decided_by"], "verified-operator");
        assert_eq!(v["params"]["arguments"]["id"], "x");
    }

    /// A frame carrying no `arguments` still gets the decider.
    #[test]
    fn the_decider_is_stamped_into_a_frame_without_arguments() {
        let frame = br#"{"method":"tools/call","params":{"name":"approve_queue_entry"}}"#;
        let out = stamp_decider(frame, "op").expect("stamped");
        let v: serde_json::Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(v["params"]["arguments"]["decided_by"], "op");
    }

    /// The loopback target replaces the port rather than substituting text.
    ///
    /// `replace(":8080", ":8580")` leaves a URL that states no explicit port
    /// unchanged, so the approve `POST` goes to **makod** carrying processd's
    /// queue path — it authenticates, 404s, and reads from here as a processd
    /// failure.
    #[test]
    fn the_self_url_always_carries_processds_own_port() {
        assert_eq!(
            self_base_url("http://makod:8080").as_deref(),
            Some("http://makod:8580")
        );
        assert_eq!(
            self_base_url("http://makod").as_deref(),
            Some("http://makod:8580"),
            "a URL with no explicit port must still reach processd"
        );
        assert_eq!(
            self_base_url("https://host:9999/").as_deref(),
            Some("https://host:8580")
        );
        assert_eq!(
            self_base_url("http://makod:8080/api/v1").as_deref(),
            Some("http://makod:8580"),
            "a base URL carries no path"
        );
    }

    /// Nothing is guessed from a URL with no host.
    #[test]
    fn an_unusable_makod_url_yields_no_target() {
        assert_eq!(self_base_url("makod:8080"), None);
        assert_eq!(self_base_url("http://"), None);
    }
}
