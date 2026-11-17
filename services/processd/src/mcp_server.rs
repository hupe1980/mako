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
//! | `get_stp_breakdown_by_erc`  | Refusal counts per Entscheidungsbaum and code, with the BDEW wording |
//! | `list_affiliate_decisions`  | Gleichbehandlung: decisions for affiliate-initiated requests |
//! | `list_pending_approvals`    | Freigaben of every role, earliest `expires_at` first (typed `Freigaben`) |
//! | `get_queue_entry`           | One Freigabe by its UUID (typed `Freigabe`) |
//! | `approve_queue_entry`       | Approve a pending queue entry (dispatch its answer) |
//! | `reject_queue_entry`        | Reject a pending queue entry (dispatch its refusal) |
//! | `rehearse_decision`         | A decision rehearsed: the route's checks and the body it would send, nothing claimed or sent |
//! | `list_klaerfaelle`          | Open Klärfälle of the caller's Marktrollen, the earliest due date first |
//! | `get_klaerfall`             | One Klärfall with its history (typed `KlaerfallAkte`) |
//! | `assign_klaerfall`          | Take a Klärfall, or release it |
//! | `resolve_klaerfall`         | Close a Klärfall without a market message, with a reason |
//! | `dispatch_klaerfall`        | „Korrigieren & senden": the correcting command through the Freigabe path |
//! | `get_counterparty_record`   | One counterparty's Akte in one role over a period (typed `CounterpartyRecord`) |
//! | `list_counterparties`       | Every counterparty and role that owed answers in a period |
//!
//! The process read model adds seven tools and two prompts
//! (`obs::mcp`); an NB build adds the four Sperrauftrag tools and
//! two prompts (`sperr::mcp`). They are further routers on the one
//! handler, so every tool sits behind the same `/mcp` door and the same
//! per-tool Cedar action.

use std::sync::Arc;

use axum::{
    Router,
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
    transport::streamable_http_server::StreamableHttpServerConfig,
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

/// A decision the REST route refused, as the tool's error: the status and
/// the route's problem body — its stable `code` (`pruefung_abgelehnt`,
/// `nicht_geprueft`, `stale_body`, `already_decided`, …) and the rule — so an
/// agent or a person reads why, not only that.
async fn decision_refused(verb: &str, resp: reqwest::Response) -> CallToolResult {
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    decision_refusal(verb, status, &body)
}

fn decision_refusal(verb: &str, status: u16, body: &serde_json::Value) -> CallToolResult {
    let code = body.get("code").and_then(|v| v.as_str()).unwrap_or("");
    let detail = body.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    let mut blocks = vec![ContentBlock::text(format!(
        "{verb} refused: HTTP {status} {code}: {detail}"
    ))];
    if let Ok(b) = ContentBlock::json(body.clone()) {
        blocks.push(b);
    }
    CallToolResult::error(blocks)
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
    /// The operator policy and posture the runner built at boot.
    pub policy: mako_service::policy::PolicyReport,
    /// The webhook endpoints and deliveries the two `list_webhook_*` tools read.
    pub subscriptions: mako_service::subscriptions::Subscriptions,
}

/// A page of the webhook endpoints.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWebhookEndpointsParams {
    /// Rows per page, 1–200; 50 when absent.
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

/// A page of one endpoint's deliveries.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWebhookDeliveriesParams {
    /// The endpoint's id.
    pub endpoint_id: String,
    /// `all` (default), `pending`, `delivered` or `failed`.
    pub status: Option<String>,
    /// Rows per page, 1–200; 50 when absent.
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

/// A refusal of the shared subscription store as an MCP error: a caller's
/// mistake is `invalid_params`, anything else `internal_error`.
fn api_to_mcp(e: &mako_service::ApiError) -> McpError {
    let text = e.to_string();
    if e.status().is_client_error() {
        McpError::invalid_params(text, None)
    } else {
        McpError::internal_error(text, None)
    }
}

impl ProcessdMcpState {
    /// The verified caller's Marktrollen, which a tool keeps its rows to as
    /// the REST twin does. A caller that cannot be verified, or a key that
    /// declares no role, is refused — never answered with the tenant's rows
    /// unfiltered.
    ///
    /// # Errors
    ///
    /// `invalid_request` naming the missing roles.
    pub fn caller_roles(&self, headers: &axum::http::HeaderMap) -> Result<Vec<String>, McpError> {
        match self.auth.roles(headers) {
            Some(rollen) if !rollen.is_empty() => Ok(rollen),
            _ => Err(McpError::invalid_request(
                "this tool keeps its rows to the caller's Marktrollen, and the caller \
                 carries none — give the key `roles`",
                None,
            )),
        }
    }
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

/// processd's operating target for the Anmeldung STP rate — its own goal,
/// not a regulatory one (no Festlegung sets an STP rate).
pub const STP_TARGET: f64 = 0.95;

/// What `get_stp_rate` returns.
#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct StpRate {
    /// Accepted over accepted and refused decisions, 0.0–1.0; `None` when
    /// none was decided in the window — no rate, not a zero one.
    pub stp_rate: Option<f64>,
    /// The window, in days back from now.
    pub window_days: u32,
    /// processd's operating target, 0.95.
    pub target: f64,
    /// `false` only when a rate exists and falls below `target`.
    pub compliant: bool,
}

impl StpRate {
    fn of(rate: Option<f64>, window_days: u32) -> Self {
        Self {
            stp_rate: rate,
            window_days,
            target: STP_TARGET,
            compliant: rate.is_none_or(|r| r >= STP_TARGET),
        }
    }
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
    /// `Pending` (default), `Approved`, `Rejected` or `Expired`.
    pub status: Option<String>,
    /// At most this many entries (default 50, at most 200).
    pub limit: Option<u32>,
    /// Only the entries answered under this Marktrolle (`NB`, `LF`, `MSB`);
    /// every role's when absent.
    #[serde(default)]
    pub marktrolle: Option<String>,
    /// Only the entries of this process (its UUID) — every Freigabe a
    /// Prozess holds, however long the queue.
    #[serde(default)]
    pub process_id: Option<String>,
    /// Skip this many entries first — the next page starts at the previous
    /// `offset` plus its `count` (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListKlaerfaelleParams {
    /// At most this many cases (default 50, at most 200).
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetKlaerfallParams {
    /// UUID of the Klärfall.
    pub id: String,
}

/// Parameters of the three tools that work a Klärfall.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KlaerfallActionParams {
    /// UUID of the Klärfall.
    pub id: String,
    /// Who acts, as **verified by the MCP middleware** — never the caller;
    /// whatever arrives here is overwritten.
    #[serde(default)]
    pub decided_by: Option<String>,
    /// `assign_klaerfall`: who takes the case; absent releases it.
    #[serde(default)]
    pub assignee: Option<String>,
    /// `dispatch_klaerfall`: the command to send; absent sends the proposal.
    #[serde(default)]
    pub befehl: Option<String>,
    /// `resolve_klaerfall`: why the case closes without a message (required).
    /// `dispatch_klaerfall`: why another command than the proposal is sent.
    #[serde(default)]
    pub begruendung: Option<String>,
    /// `dispatch_klaerfall`: what the decision states beside the Freigabe's
    /// frozen answer (`antwort_code`, `antwort_tree`, `bemerkung`, …), as
    /// `approve_queue_entry` takes it.
    #[serde(default)]
    pub antwort: Option<crate::pg::approval::OperatorAnswer>,
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
    /// The EBD Antwortcode, when the entry does not carry a frozen answer —
    /// an escalated entry, or any refusal.
    #[serde(default)]
    pub antwort_code: Option<String>,
    /// The Entscheidungsbaum the code is resolved in (e.g. `E_0622`).
    #[serde(default)]
    pub antwort_tree: Option<String>,
    /// The Codeliste DE 1131, where the MIG carries one.
    #[serde(default)]
    pub antwort_codeliste: Option<String>,
    /// `FTX+ACB` Erläuterung; also the Begründung of a refusal with no code.
    #[serde(default)]
    pub bemerkung: Option<String>,
    /// The Bilanzkreis an E/G Zustimmung names, where the entry lists
    /// `bilanzkreis` among the facts it still needs.
    #[serde(default)]
    pub bilanzkreis: Option<String>,
    /// `SG10 CCI+Z36` on a Strom 55014: `ZC9`, `ZD0` or `ZE3`.
    #[serde(default)]
    pub versorgungsart: Option<String>,
    /// `SG6 DTM+Z20` on a Gas 44014: 1, 3, 6 or 12.
    #[serde(default)]
    pub abrechnungsintervall_monate: Option<u8>,
    /// The Grundlage nach EnFG on a Strom 55014: `ZG0`, `ZG1` or
    /// `ZF9+<Grund der Privilegierung ZU5…ZV3>`, where the entry lists `enfg`
    /// among the facts it still needs.
    #[serde(default)]
    pub enfg: Option<String>,
    /// The `body_sha256` `rehearse_decision` reported for the body you were
    /// shown; the decision is refused when the body it would send is another.
    #[serde(default)]
    pub body_sha256: Option<String>,
}

impl QueueActionParams {
    /// The operator's answer, as the decision endpoints read it.
    fn answer(&self) -> crate::pg::approval::OperatorAnswer {
        crate::pg::approval::OperatorAnswer {
            antwort_code: self.antwort_code.clone(),
            antwort_tree: self.antwort_tree.clone(),
            antwort_codeliste: self.antwort_codeliste.clone(),
            bemerkung: self.bemerkung.clone(),
            bilanzkreis: self.bilanzkreis.clone(),
            versorgungsart: self.versorgungsart.clone(),
            abrechnungsintervall_monate: self.abrechnungsintervall_monate,
            enfg: self.enfg.clone(),
            body_sha256: self.body_sha256.clone(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetQueueEntryParams {
    pub id: String,
}

#[derive(Clone)]
pub struct ProcessdMcpHandler {
    pub(crate) state: Arc<ProcessdMcpState>,
    /// The router every request is answered from (`#[tool_handler(router =
    /// …)]`): the decision tools, the read model's and — in an NB build — the
    /// Sperrauftrag's. rmcp's default answers from `Self::tool_router()` alone,
    /// which would serve the decision tools and refuse the read model's as
    /// unknown.
    tool_router: ToolRouter<ProcessdMcpHandler>,
    prompt_router: PromptRouter<ProcessdMcpHandler>,
}

impl ProcessdMcpHandler {
    /// Every tool and prompt this build serves, in one place for the handler
    /// and the snapshot test.
    fn routers() -> (ToolRouter<Self>, PromptRouter<Self>) {
        let tool_router = Self::tool_router()
            + Self::obs_tool_router()
            + Self::decision_probe_tool_router()
            + Self::counterparty_tool_router();
        let prompt_router = Self::prompt_router() + Self::obs_prompt_router();
        #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
        let (tool_router, prompt_router) = (
            tool_router + Self::sperr_tool_router(),
            prompt_router + Self::sperr_prompt_router(),
        );
        (tool_router, prompt_router)
    }
}

impl ProcessdMcpHandler {
    /// One Klärfall step, through processd's own REST route over the loopback
    /// so its Cedar checks — the step's action and, for a dispatch, the
    /// command's own — run rather than being bypassed. The verified caller
    /// rides as `X-Decided-By`.
    async fn case_action(
        &self,
        route: crate::server::QueueRoute,
        p: &KlaerfallActionParams,
        body: &serde_json::Value,
        verb: &str,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        let Some(decided_by) = p.decided_by.as_deref() else {
            return Err(McpError::internal_error(
                "this step is not attributable to a verified caller",
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
        match route
            .request(&up, id)
            .header("X-Decided-By", decided_by)
            .json(body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let akte: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::structured(akte))
            }
            Ok(resp) => Ok(decision_refused(verb, resp).await),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[tool_router]
impl ProcessdMcpHandler {
    fn new(state: Arc<ProcessdMcpState>) -> Self {
        let (tool_router, prompt_router) = Self::routers();
        Self {
            state,
            tool_router,
            prompt_router,
        }
    }

    #[tool(
        description = "The operator policy and posture this processd deployment runs under: each setting with its value, whether it was set or defaulted, and the primary source of the value. Read-only; a setting changes only by editing the config and restarting.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_policy(&self) -> Result<CallToolResult, McpError> {
        ContentBlock::json(serde_json::to_value(&self.state.policy).unwrap_or_default())
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// The agent twin of `GET /api/v1/webhooks/endpoints`.
    #[tool(
        description = "List the webhook endpoints processd delivers its CloudEvents (de.sperr.*) to, a page ordered by name: url, event-type patterns, extension filters, owner (`config` for the endpoint `default`, the configured [sperr] erp_webhook_url; `api` for one registered through REST), whether it is active and the last delivery status. Secrets are never listed. Page on `next_cursor`.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<mako_service::page::Page<mako_service::subscriptions::Endpoint>>()
    )]
    async fn list_webhook_endpoints(
        &self,
        Parameters(p): Parameters<ListWebhookEndpointsParams>,
    ) -> Result<CallToolResult, McpError> {
        let page = mako_service::page::PageQuery {
            limit: p.limit,
            cursor: p.cursor,
        };
        let endpoints = self
            .state
            .subscriptions
            .list_endpoints(None, &page)
            .await
            .map_err(|e| api_to_mcp(&e))?;
        serde_json::to_value(endpoints)
            .map(CallToolResult::structured)
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// The agent twin of `GET /api/v1/webhooks/endpoints/{id}/deliveries`.
    #[tool(
        description = "List one webhook endpoint's deliveries, newest first: the CloudEvent id and type, attempts, the last HTTP status, latency and error, and whether it was delivered or dead-lettered. `status` narrows to `pending`, `delivered` or `failed`. Page on `next_cursor`.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<mako_service::page::Page<mako_service::subscriptions::Delivery>>()
    )]
    async fn list_webhook_deliveries(
        &self,
        Parameters(p): Parameters<ListWebhookDeliveriesParams>,
    ) -> Result<CallToolResult, McpError> {
        use mako_service::subscriptions::DeliveryFilter;
        let status = match p.status.as_deref().unwrap_or("all") {
            "all" => DeliveryFilter::All,
            "pending" => DeliveryFilter::Pending,
            "delivered" => DeliveryFilter::Delivered,
            "failed" => DeliveryFilter::Failed,
            other => {
                return Err(McpError::invalid_params(
                    format!("status {other:?} is none of all, pending, delivered, failed"),
                    None,
                ));
            }
        };
        let page = mako_service::page::PageQuery {
            limit: p.limit,
            cursor: p.cursor,
        };
        let deliveries = self
            .state
            .subscriptions
            .list_deliveries(None, &p.endpoint_id, status, &page)
            .await
            .map_err(|e| api_to_mcp(&e))?;
        serde_json::to_value(deliveries)
            .map(CallToolResult::structured)
            .map_err(|e| McpError::internal_error(e.to_string(), None))
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
        description = "Get a single NB Anmeldung decision by process_id. NB role. Returns the \
decision, the Antwortcode with the Entscheidungsbaum it was resolved under (`antwortcode_ebd`) and \
the BDEW's own wording for it (`bedeutung`), and whether the initiator is an affiliate. Quote \
`bedeutung`; never translate a code yourself — a code means something only within its tree.",
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
            Ok(Some(rec)) => ContentBlock::json(decision_json(&rec))
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
        description = "Anmeldung refusals of the last N days, counted per Entscheidungsbaum and \
Antwortcode. NB role. Each row carries `ebd`, `antwortcode`, the BDEW's wording (`bedeutung`) and \
`count`, most frequent first. Use it when the STP rate drops, and quote `bedeutung`: the same code \
is a different refusal in another tree. Escalations are not refusals and are not counted; \
affiliate-initiated ones are in `list_affiliate_decisions`.",
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
                    .map(|(ebd, code, count)| {
                        serde_json::json!({
                            "ebd": ebd,
                            "antwortcode": code,
                            "bedeutung": bedeutung(ebd.as_deref(), code.as_deref()),
                            "count": count,
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
        description = "List NB Anmeldung decisions where `initiator_is_affiliate = true`: the LF \
MP-ID is the operator's own. processd holds each for a person rather than accepting it \
automatically, so an audit can show the affiliate got no faster path than any other LF \
(§ 20 Abs. 1 Satz 1 EnWG, non-discriminatory access). Use `get_parity_report` for the \
aggregated comparison the Gleichbehandlungsbericht (§ 7a Abs. 5 Satz 3 EnWG) draws on.",
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
                "regulatory_note": "processd holds every affiliate-initiated Anmeldung for a person \
(decision=Escalate). An entry with decision=Accept means that hold was bypassed — investigate \
whether the affiliate was treated differently from other LFs (§ 20 Abs. 1 Satz 1 EnWG).",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get the Anmeldung STP rate for the last N days. NB role. `stp_rate` is \
accepted / (accepted + refused) decisions, 0.0–1.0, or null when none was decided; escalations \
are not counted. `target` is processd's operating target, not a regulatory one, and \
`compliant` is false only when a rate exists and falls below it.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<StpRate>()
    )]
    async fn get_stp_rate(
        &self,
        Parameters(params): Parameters<GetStpRateParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::anmeldung::PgAnmeldungRepository;
        let repo = PgAnmeldungRepository::new(self.state.pool.clone());
        let days = params.days.unwrap_or(30);
        match repo.stp_rate(&self.state.tenant, days).await {
            Ok(rate) => serde_json::to_value(StpRate::of(rate, days))
                .map(CallToolResult::structured)
                .map_err(|e| McpError::internal_error(e.to_string(), None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Open Klärfälle — one per message mako could not bring to its end: an \
APERAK rejection, a dead letter, a refusing answer, a lapsed Antwortfrist — the earliest due date \
first, those without a Frist last. Each names its cause as a code in the list that defines it \
(`ursache_code`, `ursache_liste`), the BDEW's wording where the list publishes one \
(`bedeutung` — quote it, never paraphrase a code), the tabulated `aktion` with its `hinweis`, the \
correcting `befehl` where a table names one, and `due_at` (the business Antwortfrist, null is \
„keine Frist“) with `overdue`. Every field is a table lookup; nothing here is model output. \
Read-only.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::klaerfall::Klaerfaelle>()
    )]
    async fn list_klaerfaelle(
        &self,
        Parameters(params): Parameters<ListKlaerfaelleParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        let rollen = self.state.caller_roles(&parts.headers)?;
        let limit = params.limit.unwrap_or(50).clamp(1, 200);
        let cases = crate::klaerfall::list_open(
            &self.state.pool,
            &self.state.tenant,
            &rollen,
            i64::from(limit),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(
            serde_json::to_value(cases)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        ))
    }

    #[tool(
        description = "One Klärfall by its UUID, with its history (`verlauf`: who assigned, \
resolved or dispatched it, the proposed command beside the one sent, the reason). The case is in \
the shape `list_klaerfaelle` lists it — `korrektur.befehl` is the correcting command where the \
case's answer is one a Freigabe sends, `korrektur.erlaubt` the commands a dispatch may send \
instead; null escalates. Every field is a table lookup. Read-only.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::klaerfall::KlaerfallAkte>()
    )]
    async fn get_klaerfall(
        &self,
        Parameters(params): Parameters<GetKlaerfallParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        let rollen = self.state.caller_roles(&parts.headers)?;
        let Ok(id) = params.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match crate::klaerfall::get(
            &self.state.pool,
            &self.state.tenant,
            id,
            &rollen,
            time::OffsetDateTime::now_utc(),
        )
        .await
        {
            Ok(Some(akte)) => Ok(CallToolResult::structured(
                serde_json::to_value(akte)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            )),
            Ok(None) => Err(McpError::invalid_params(
                format!("Klärfall {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Take a Klärfall (`assignee`), or release it (no `assignee`). Journaled \
against the verified caller. The owner stays the Marktrolle either way.",
        annotations(
            read_only_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn assign_klaerfall(
        &self,
        Parameters(p): Parameters<KlaerfallActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = serde_json::json!({ "assignee": p.assignee });
        self.case_action(crate::server::KLAERFALL_ASSIGN, &p, &body, "assign")
            .await
    }

    #[tool(
        description = "Close a Klärfall without a market message; `begruendung` is required and \
journaled against the verified caller. To send the correcting command use `dispatch_klaerfall`.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn resolve_klaerfall(
        &self,
        Parameters(p): Parameters<KlaerfallActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = serde_json::json!({ "begruendung": p.begruendung.clone().unwrap_or_default() });
        self.case_action(crate::server::KLAERFALL_RESOLVE, &p, &body, "resolve")
            .await
    }

    #[tool(
        description = "„Korrigieren & senden“: send a Klärfall's correcting command to makod \
through the Freigabe decision path — the frozen answer, the code against its \
Entscheidungsbaum and makod's pre-send check, whose refusal comes back in place. Absent \
`befehl` sends `korrektur.befehl`; another command of `korrektur.erlaubt` needs a \
`begruendung`. The proposal, the command sent and the reason are journaled against the \
verified caller. Irreversible: the market message cannot be withdrawn.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn dispatch_klaerfall(
        &self,
        Parameters(p): Parameters<KlaerfallActionParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = serde_json::json!({
            "befehl": p.befehl,
            "begruendung": p.begruendung,
            "antwort": p.antwort.clone().unwrap_or_default(),
        });
        self.case_action(crate::server::KLAERFALL_DISPATCH, &p, &body, "dispatch")
            .await
    }

    #[tool(
        description = "Approval-queue entries (Freigaben) of every role, the earliest `expires_at` \
first (status: Pending by default, or Approved/Rejected/Expired; `marktrolle` narrows to one \
role, `process_id` to one process's; page with `offset`). Each is `gehalten` — a decided \
answer approved exactly as shown in `antwort` — or `eskaliert` — the person states the answer: a code from `zustimmung` to approve, from \
`ablehnung` to reject, each with its Entscheidungsbaum (`ebd`, state it as `antwort_tree`). \
Quote `bedeutung`; a code means something only within its tree. `saturated` means at least \
`limit` entries matched.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::freigabe::Freigaben>()
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
        let limit = params.limit.unwrap_or(50).clamp(1, 200);
        let process_id =
            match params.process_id.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => Some(p.parse::<uuid::Uuid>().map_err(|_| {
                    McpError::invalid_params("process_id must be a valid UUID", None)
                })?),
                None => None,
            };
        let filter = crate::pg::approval::QueueFilter {
            status,
            marktrolle: params.marktrolle.clone().filter(|r| !r.is_empty()),
            process_id,
        };
        let entries = queue
            .list_page(
                &self.state.tenant,
                &filter,
                limit,
                params.offset.unwrap_or(0),
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let freigaben: Vec<crate::freigabe::Freigabe> = entries
            .iter()
            .map(crate::freigabe::Freigabe::from_entry)
            .collect();
        Ok(CallToolResult::structured(
            serde_json::to_value(crate::freigabe::Freigaben {
                count: freigaben.len(),
                saturated: freigaben.len() >= usize::try_from(limit).unwrap_or(usize::MAX),
                freigaben,
            })
            .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        ))
    }

    #[tool(
        description = "One approval-queue entry (Freigabe) by its UUID, in the shape \
`list_pending_approvals` lists: `art` gehalten/eskaliert, the frozen `antwort`, and the \
`zustimmung` / `ablehnung` codes of its Entscheidungsbaum with the BDEW wording.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::freigabe::Freigabe>()
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
            Ok(Some(entry)) => Ok(CallToolResult::structured(
                serde_json::to_value(crate::freigabe::Freigabe::from_entry(&entry))
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            )),
            Ok(None) => Err(McpError::invalid_params(
                format!("approval queue entry {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
    #[tool(
        description = "Approve a pending approval-queue entry: dispatch the entry's approve \
command to makod and mark it Approved. A held decision is sent with the answer frozen when it \
was queued; an escalated entry needs `antwort_code` (and `antwort_tree` where the command \
resolves one) and is refused without it. `expires_at` is the business answer Frist less an \
hour — check it with `list_pending_approvals` first.",
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
            .json(&p.answer())
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status() == 204 => {
                ContentBlock::json(serde_json::json!({
                    "id": p.id,
                    "status": "Approved",
                    "note": "answer dispatched to makod; queue entry marked Approved.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => Ok(decision_refused("approve", resp).await),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Reject a pending approval-queue entry: dispatch the entry's refusal \
command to makod and mark it Rejected. The refusal's `antwort_code` is the operator's; a \
refusal that carries no code (the ESA Werteanfrage) takes its Begründung from `bemerkung`. \
For the aggregated parity figures use `get_parity_report`.",
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
            .json(&p.answer())
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status() == 204 => {
                ContentBlock::json(serde_json::json!({
                    "id": p.id,
                    "status": "Rejected",
                    "note": "refusal dispatched to makod; queue entry marked Rejected.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => Ok(decision_refused("reject", resp).await),
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
                "1. Call `get_decision` with the process_id. A refusal carries its Antwortcode, the \
                    Entscheidungsbaum it was resolved under (`antwortcode_ebd`) and the BDEW's wording \
                    (`bedeutung`); the wording names the Prüfschritt that refused.\n\
                 2. Quote `bedeutung` — never translate a code from memory: `A02` is a different \
                    refusal in `E_0607` and `E_0622`.\n\
                 3. `detail` says which input the check read; correct that record at its owner \
                    (marktd for Marktlokation and Netzgebiet data) before the LF sends again.\n\
                 4. An affiliate-initiated Anmeldung is not refused but held for a person: \
                    `list_affiliate_decisions`, then POST /api/v1/queue/{id}/approve.",
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
                 2. Call `get_stp_breakdown_by_erc(days=7)`: refusals per tree and code, each with \
                    the BDEW's wording (`bedeutung`) naming the Prüfschritt that refused.\n\
                 3. Take the largest row and read two of its decisions with `get_decision`: \
                    `detail` names the input the Prüfschritt read. A refusal the request deserved \
                    is the LF's to correct; one caused by a missing or stale record (Marktlokation, \
                    Netzgebiet in marktd) is the NB's to fix.\n\
                 4. Affiliate-initiated Anmeldungen are held, not refused, and lower the rate \
                    without a code: `list_affiliate_decisions(days=7)`, each approved by a person.",
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
                "1. Read the process with `get_process` (read model) by its process id: `state` \
                    says whether it was refused. `erc_code` is set only when an APERAK refused \
                    the message itself — an APERAK error code, not the Antwortcode of a refusal.\n\
                 2. A WiM Antwortcode is read within the Codeliste that publishes it (`S_00xx`); \
                    report the process, never a code's meaning from memory.\n\
                 3. An escalation is not a refusal: it waits for a person in the queue \
                    (`list_pending_approvals`), who answers with a code from the same Codeliste.",
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
                 POST /api/v1/start-supply with malo_id, lieferbeginn_datum (YYYY-MM-DD) and \
                 transaktionsgrund — E01 Ein-/Auszug (Umzug) or E03 Wechsel, Muss in SG4 \
                 STS+7 (UTILMD AHB Strom 2.2 Kap. 8.2).\n\
                 LFW24 Vorlauffrist: earliest Lieferbeginn is the day after the next Werktag \
                 (spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn — \
                 BK6-24-174 GPKE Teil 2, SD Lieferbeginn).\n\n\
                 For Gas (GeLi Gas UTILMD G PID 44001):\n\
                 POST /api/v1/start-supply-gas with malo_id, process_date (YYYYMMDD), \
                 transaktionsgrund and the contract facts the Anmeldung NN makes Muss — \
                 abrechnungsintervall_monate, haushaltskunde, netznutzungsvertrag, \
                 netznutzung_zahler, gemeinderabatt_prozent, kunde, ablesekarte (UTILMD AHB \
                 Gas 1.2 Kap. 5.6). vertragd sends them from the supply contract.\n\n\
                 processd validates:\n\
                 - MaLo exists in marktd with correct netzebene/bilanzierungsgebiet\n\
                 - Active Preisblatt for the NB\n\
                 - Lieferbeginn date within Vorlauffrist window\n\
                 - affiliate check: an Anmeldung of the operator's own LF is held for a person \
                   (initiator_is_affiliate), so parity under § 20 Abs. 1 EnWG stays auditable\n\n\
                 On success: makod dispatches UTILMD 55001/44001 EDIFACT to NB.",
            ),
        ]
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for ProcessdMcpHandler {
    /// The revisions every mako MCP server speaks, not the SDK's set.
    fn supported_protocol_versions(
        &self,
    ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }

    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("processd", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions())
    }
}

/// The BDEW's wording for `code` within the tree `ebd`, from `mako-pruefung`'s
/// catalogue — `None` where either is absent or the tree does not publish it.
fn bedeutung(ebd: Option<&str>, code: Option<&str>) -> Option<&'static str> {
    mako_pruefung::codes::lookup(ebd?, code?).map(|c| c.bedeutung)
}

/// A decision as `get_decision` returns it: the stored row and the wording of
/// its Antwortcode.
fn decision_json(rec: &crate::pg::anmeldung::AnmeldungDecisionRecord) -> serde_json::Value {
    let mut v = serde_json::to_value(rec).unwrap_or_default();
    if let Some(map) = v.as_object_mut() {
        map.insert(
            "bedeutung".into(),
            bedeutung(rec.antwortcode_ebd.as_deref(), rec.antwortcode.as_deref()).into(),
        );
    }
    v
}

/// The server instructions: processd's decision tools, then the read model's,
/// then — in an NB build — the Sperrauftrag queue's.
fn instructions() -> String {
    let mut text = String::from(
        "processd MCP — NB Anmeldung STP decisions, §20 EnWG compliance, LF E_0624 queue.\n\
         NB: `list_decisions`, `get_decision`, `get_stp_rate`, `get_stp_breakdown_by_erc`, `list_affiliate_decisions`.\n\
         LF: `list_pending_approvals`, `get_queue_entry`, `approve_queue_entry`, `reject_queue_entry`.\n\
         Webhooks: `list_webhook_endpoints`, `list_webhook_deliveries` — where the CloudEvents go, failed deliveries included.\n\
         Prompts: `triage-nb-rejection`, `investigate-stp-drop`, `triage-msb-wechsel`, `trigger-lieferbeginn`.\n\n",
    );
    text.push_str(&crate::obs::mcp::instructions());
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    {
        text.push_str("\n\n");
        text.push_str(crate::sperr::mcp::instructions());
    }
    text
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
        // The same action as `GET /api/v1/policy`.
        "get_policy" => "read-policy",
        "list_decisions"
        | "get_decision"
        | "get_stp_rate"
        | "get_stp_breakdown_by_erc"
        | "list_affiliate_decisions" => "read-decisions",
        "list_pending_approvals" | "get_queue_entry" | "rehearse_decision" => "read-queue",
        "list_klaerfaelle" | "get_klaerfall" => "read-case",
        // The same action as their REST twins under `/api/v1/webhooks`.
        "list_webhook_endpoints" | "list_webhook_deliveries" => "manage-subscription",
        "assign_klaerfall" => "assign-case",
        "resolve_klaerfall" => "resolve-case",
        // Irreversible: sends a market message through the Freigabe path; the
        // route also checks the command's own `decide-queue`.
        "dispatch_klaerfall" => "dispatch-case",
        // Irreversible: dispatches `gpke.nb-lieferende.bestaetigen` (PID 55008)
        // or `geli.stornierung.initiieren`. The market message cannot be
        // withdrawn once sent.
        "approve_queue_entry" | "reject_queue_entry" => "decide-queue",
        // The process read model — the same actions as its REST routes.
        "get_process"
        | "list_processes_by_family"
        | "list_open_processes"
        | "count_open_processes" => "read-process",
        "get_kpi_report" | "get_parity_report" | "get_process_stp_rate" => "read-kpi",
        "get_counterparty_record" | "list_counterparties" => "read-counterparty",
        "list_overdue_processes" => "read-overdue",
        // The Sperrauftrag queue — dispatch data naming the household to be
        // disconnected, held to the role its REST reads require.
        #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
        "list_sperr_orders" | "get_sperr_order" | "get_sperr_stats" | "list_due_orders" => {
            "read-sperr-order"
        }
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
            return mako_service::ApiError::payload_too_large(format!(
                "MCP request body exceeds {MAX_MCP_BODY} bytes"
            ))
            .into_response();
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
            if matches!(
                action,
                "decide-queue" | "assign-case" | "resolve-case" | "dispatch-case"
            ) {
                let Some(identity) = state.auth.identify(&parts.headers) else {
                    return mako_service::ApiError::forbidden(
                        "this decision cannot be attributed to a verified caller, and an \
                         unattributable § 20 Abs. 1 EnWG record is refused rather than \
                         written against a placeholder",
                    )
                    .into_response();
                };
                match stamp_decider(&bytes, &identity.name) {
                    Some(rewritten) => bytes = rewritten.into(),
                    None => {
                        return mako_service::ApiError::bad_request("malformed tools/call frame")
                            .into_response();
                    }
                }
            }
        }
        McpCall::UnknownTool(name) => {
            tracing::warn!(tool = %name, "processd: MCP tool carries no Cedar action");
            return mako_service::ApiError::forbidden(format!(
                "MCP tool {name:?} carries no Cedar action, so it cannot be authorized — add \
                 it to `tool_action`"
            ))
            .into_response();
        }
    }
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<ProcessdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = ProcessdMcpHandler::new(Arc::clone(&state));
    let service = mako_service::mcp::service(
        move || Ok(handler.clone()),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MCP decision tools reach the REST route, which runs makod's
    /// pre-send check before it marks anything; an unchecked or refused body
    /// comes back as the tool's error carrying the route's `code`, so an
    /// agent cannot read a refused approval as a transport hiccup.
    #[test]
    fn an_unchecked_agent_proposal_is_not_approvable_unchanged_over_mcp() {
        let r = decision_refusal(
            "approve",
            422,
            &serde_json::json!({ "code": "nicht_geprueft", "detail": "makod did not check" }),
        );
        assert_eq!(r.is_error, Some(true));
        let text = serde_json::to_string(&r.content).expect("json");
        assert!(text.contains("nicht_geprueft"), "{text}");
        assert!(text.contains("HTTP 422"), "{text}");
    }

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

    /// A code's wording is looked up within its tree: `A02` refuses an
    /// Anmeldung in `E_0622` for a reason `E_0607` does not share, and a code
    /// without its tree has no wording at all.
    /// The role-independent tool contract — decisions and the read model —
    /// is the committed `mcp.snapshot.json` (`BLESS_MCP_SNAPSHOT=1` rewrites
    /// it). An NB build adds the Sperrauftrag tools, which no Desk view reads.
    #[test]
    fn the_tool_contract_matches_the_snapshot() {
        let router = ProcessdMcpHandler::tool_router()
            + ProcessdMcpHandler::obs_tool_router()
            + ProcessdMcpHandler::decision_probe_tool_router()
            + ProcessdMcpHandler::counterparty_tool_router();
        let tools: Vec<serde_json::Value> = router
            .list_all()
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .expect("tools serialize");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mcp.snapshot.json");
        if let Err(e) = mako_service::mcp_snapshot::compare(&tools, &path) {
            panic!("{e}");
        }
    }

    /// `list_webhook_endpoints` and `list_webhook_deliveries` answer with the
    /// shape their REST twins document, and need the action those check.
    #[tokio::test]
    async fn the_webhook_tools_are_twins_of_the_rest_reads() {
        let doc =
            serde_json::to_value(crate::openapi::document()).expect("the document serializes");
        let tools = ProcessdMcpHandler::tool_router().list_all();
        for (tool, path) in [
            ("list_webhook_endpoints", "/api/v1/webhooks/endpoints"),
            (
                "list_webhook_deliveries",
                "/api/v1/webhooks/endpoints/{id}/deliveries",
            ),
        ] {
            mako_service::mcp::conformance::assert_rest_twin(&tools, tool, &doc, path);
            assert_eq!(
                tool_action(tool),
                Some(mako_service::subscriptions::MANAGE),
                "{tool}"
            );
        }
    }

    /// The STP rate is judged against processd's target, and a window with
    /// no decision is no breach: the Überblick colours its tile from
    /// `compliant`, never from a threshold of its own.
    #[test]
    fn the_stp_rate_is_judged_by_the_service() {
        assert!(!StpRate::of(Some(0.94), 30).compliant);
        assert!(StpRate::of(Some(0.95), 30).compliant);
        assert!(StpRate::of(None, 30).compliant);
        assert_eq!(StpRate::of(None, 7).stp_rate, None);
    }

    #[test]
    fn a_code_is_worded_only_within_its_tree() {
        let anmeldung = bedeutung(Some("E_0622"), Some("A02")).expect("E_0622 publishes A02");
        assert!(
            anmeldung.contains("nimmt nicht an der Marktkommunikation teil"),
            "{anmeldung}"
        );
        let abmeldung = bedeutung(Some("E_0607"), Some("A02")).expect("E_0607 publishes A02");
        assert_ne!(anmeldung, abmeldung);
        assert_eq!(bedeutung(None, Some("A02")), None);
        assert_eq!(bedeutung(Some("E_0622"), None), None);
    }
}
