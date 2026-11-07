//! MCP (Model Context Protocol) server for `makod`.
//!
//! Exposes process state and commands to LLM tooling via the MCP protocol
//! (<https://modelcontextprotocol.io>). Mounted at `/mcp` on the existing
//! `--http-addr` port — no separate TCP socket required.
//!
//! ## Authentication
//!
//! The `/mcp` path is protected by the same Cedar ABAC layer as every other
//! HTTP endpoint.  Every HTTP request (initial `POST /mcp` handshake and
//! subsequent SSE streams) must carry `Authorization: Bearer <token>`.
//! Unauthenticated requests are rejected with `401 Unauthorized` before they
//! reach the MCP session layer.
//!
//! ## Transport
//!
//! Uses the MCP Streamable HTTP transport (spec 2025-11-25): clients `POST`
//! to `/mcp` for JSON-RPC requests and `GET` to `/mcp` for SSE event streams.
//! This is compatible with Claude Desktop, VS Code Copilot, and any
//! MCP-capable client that supports Streamable HTTP.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_commands` | List commands available for this instance's configured Marktrollen |
//! | `submit_command` | Trigger a MaKo process command (same as `POST /api/v1/commands`) |
//! | `get_malo` | Read a cached Marktlokation record |
//! | `list_partners` | List all registered trading partners |
//! | `get_partner` | Get a specific trading partner by MP-ID |
//! | `get_health` | Query daemon health and uptime |
//! | `get_process` | Look up an active process by business key (malo_id, melo_id, or vorgang_id) |
//! | `get_process_history` | A process's events, oldest first, its market messages marked by direction and PID |
//! | `list_overdue_deadlines` | List processes with expired regulatory deadlines |
//!
//! ## Resources
//!
//! | URI template | Description |
//! |---|---|
//! | `malo://{malo_id}` | Marktlokation master-data record |
//! | `partner://{mp_id}` | Trading-partner record |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `gpke-lieferbeginn` | Step-by-step guide: GPKE Lieferbeginn (electricity supplier change) |
//! | `geli-lieferbeginn` | Step-by-step guide: GeLi Gas Lieferbeginn (gas supplier change) |
//! | `wim-geraetewechsel` | Step-by-step guide: WiM Gerätewechsel (meter device change) |
//! | `msb-preisanfrage` | Step-by-step guide: MSB Preisanfrage (REQOTE/QUOTES PIDs 35001/15001) |
//! | `wim-gas-anmeldung` | Step-by-step guide: WiM Gas MSB-Wechsel (GMSB Anmeldung approval) |
//! | `gpke-sperrung` | Step-by-step guide: GPKE Sperrung Strom (disconnection confirmation) |

use std::sync::Arc;

use axum::{
    Router,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_engine::deadline::DeadlineStore as _;
use mako_engine::store_slatedb::{SlateDbDeadlineStore, SlateDbStore};
use mako_engine::{ids::TenantId, partner::PartnerStore as _, store_slatedb::SlateDbPartnerStore};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars,
    service::{Peer, RequestContext},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::cedar_authz::CedarAuthorizer;

/// Expired deadlines returned by one `list_overdue_deadlines` call.
const OVERDUE_PAGE: usize = 50;

/// Expired deadlines scanned before § 6a EnWG filtering is applied.
///
/// Wider than [`OVERDUE_PAGE`] because the authorization filter runs on the
/// scan result: in a combined-role deployment a page-sized scan could return
/// nothing but the other market role's deadlines, hiding the caller's own.
const OVERDUE_SCAN_WINDOW: usize = 400;
use crate::commands_api::{
    COMMAND_REGISTRY, CommandsApiState, DispatchOutcome, dispatch_command, validate_command,
};
use crate::malo_cache::SlateDbMaloCache;

// ── Shared state ──────────────────────────────────────────────────────────────

/// Shared state injected into each MCP handler instance.
pub struct MakodMcpState {
    /// Operator tenant — BDEW/DVGW Codenummer or MP-ID (data-isolation key).
    ///
    /// Single source of truth for tenant identity in this state struct.
    /// When `TenantId` (UUID) is needed for SlateDB operations, derive it
    /// inline: `TenantId::from_party_id(&self.tenant)`.
    pub tenant: String,
    /// Daemon version string (from `makod --version`).
    pub version: String,
    /// Cedar authorizer — authenticates incoming Bearer tokens.
    pub cedar: Arc<CedarAuthorizer>,
    /// Delegated-to state for command dispatch.
    pub commands: Arc<CommandsApiState>,
    /// MaLo master-data cache.
    pub malo_cache: Arc<SlateDbMaloCache>,
    /// Trading-partner store.
    pub partner_store: Arc<SlateDbPartnerStore>,
    /// Process registry — used by `get_process` to look up active processes by business key.
    pub process_store: Arc<SlateDbStore>,
    /// Deadline store — used by `list_overdue_deadlines` to surface expired process timers.
    pub deadline_store: SlateDbDeadlineStore,
}

// ── Tool parameter types ───────────────────────────────────────────────────────

/// Parameters for `submit_command`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitCommandParams {
    /// Dotted command name in the format `<domain>.<prozess>.<aktion>`.
    ///
    /// Use `list_commands` first to discover all available commands for this
    /// instance. Examples: `gpke.lieferbeginn.anmelden`,
    /// `wim.geraetewechsel.beauftragen`, `geli.lieferbeginn.anmelden`.
    ///
    /// **Single-role commands** (e.g. `gpke.lieferbeginn.anmelden` → always
    /// `LF`) do not need `marktrolle`. **Multi-role commands** (e.g.
    /// `wim.geraetewechsel.beauftragen` → `NB` or `MSB`) require it.
    #[schemars(example = "\"gpke.lieferbeginn.anmelden\"")]
    pub command: String,

    /// Marktrolle override — required only for multi-role commands.
    ///
    /// Permitted values: `LF`, `LFG`, `NB`, `GNB`, `MSB`, `BKV`, `ÜNB`.
    /// Omit for single-role commands.
    #[schemars(example = "\"NB\"")]
    pub marktrolle: Option<String>,

    /// Command-specific payload fields.
    ///
    /// | Command | Required fields |
    /// |---|---|
    /// | `gpke.lieferbeginn.anmelden` | `malo_id` (11-digit), `lieferbeginn_datum` (YYYY-MM-DD) |
    /// | `gpke.lieferende.anmelden` | `malo_id`, `lieferende_datum` |
    /// | `gpke.kuendigung.anmelden` | `malo_id`, `kuendigung_datum` |
    /// | `geli.lieferbeginn.anmelden` | `malo_id` (gas), `lieferbeginn_datum` |
    /// | `geli.lieferende.anmelden` | `malo_id` (gas), `lieferende_datum` |
    /// | `wim.geraetewechsel.beauftragen` | `melo_id`, `process_date` (YYYYMMDD), `receiver_mp_id` |
    /// | `mabis.abrechnung.einleiten` | `bilanzierungsgebiet`, `abrechnungszeitraum_von`, `abrechnungszeitraum_bis` |
    #[schemars(example = "{\"malo_id\": \"10001234558\", \"lieferbeginn_datum\": \"2026-10-01\"}")]
    pub payload: serde_json::Value,

    /// Optional stable key identifying this business request.
    ///
    /// Supply the same value on a retry and the recorded response is replayed
    /// instead of the command running twice; the record lives for 24 hours.
    /// Reusing one key for a different command or payload is refused. When
    /// omitted a random UUID is echoed back and no replay record is kept.
    pub idempotency_key: Option<String>,
}

/// Parameters for `get_malo`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMaloParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(description = "11-digit Marktlokations-ID, e.g. \"10001234558\"")]
    pub malo_id: String,
}

/// Parameters for `get_partner`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPartnerParams {
    /// Marktpartner-ID of the trading partner.
    #[schemars(description = "13-digit Global Location Number of the trading partner")]
    pub mp_id: String,
}

/// Parameters for `list_partners`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPartnersParams {
    /// Maximum number of partner records to return (1–500). Defaults to 100.
    ///
    /// Use together with `cursor` to page through large partner directories.
    #[schemars(description = "Maximum number of results to return (1-500, default 100)")]
    pub limit: Option<usize>,

    /// Opaque pagination cursor returned in the previous response.
    ///
    /// Pass the `next_cursor` value from the previous response to retrieve
    /// the next page. Omit or pass `null` to start from the beginning.
    #[schemars(description = "Pagination cursor from a previous list_partners response")]
    pub cursor: Option<String>,
}

/// Parameters for `get_process`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProcessParams {
    /// Business key identifying the process — typically the `malo_id` (11-digit)
    /// for GPKE and GeLi Gas workflows, the `melo_id` for WiM Strom workflows,
    /// or the `vorgang_id` for WiM Gas Stornierung.
    ///
    /// This is the same key you would pass in the `payload` field when submitting
    /// the initial `anmelden` command. The registry maps it to the internal process
    /// stream ID.
    #[schemars(
        description = "Business key: malo_id (GPKE/GeLi Gas), melo_id (WiM Strom), or vorgang_id (WiM Gas Stornierung)"
    )]
    pub business_key: String,
}

/// What `get_process` returns: every process the business key is correlated
/// with that the caller may read. Its JSON Schema is the tool's
/// `outputSchema`, and `mcp.snapshot.json` pins it for the Desk's generated
/// types.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProcessesView {
    /// The business key the caller passed.
    pub business_key: String,
    /// The processes indexed under the key — concurrent ones (a Lieferbeginn
    /// beside a Sperrung) and finished ones alike: the correlation index is
    /// append-only, so whether a process is still running is its own state,
    /// not the index's.
    pub processes: Vec<ProcessView>,
}

/// What `list_dead_letters` returns: the most recent page and how many dead
/// letters the store holds in all.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeadLetters {
    /// The most recent first, at most 20.
    pub dead_letters: Vec<DeadLetter>,
    /// Every dead letter the store holds — not the length of the page.
    pub gesamt: usize,
}

/// One message that permanently failed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeadLetter {
    /// When it was rejected, RFC 3339 in UTC; `None` when the stored instant
    /// cannot be read as one.
    #[schemars(extend("format" = "date-time"))]
    pub rejected_at: Option<String>,
    /// The rejection category.
    pub reason_label: String,
    /// What went wrong, in the engine's words.
    pub reason_detail: String,
}

/// The stored instant when it is RFC 3339 — the form the engine records —
/// and `None` rather than a fabricated instant when it is not.
fn rfc3339(stored: String) -> Option<String> {
    time::OffsetDateTime::parse(&stored, &time::format_description::well_known::Rfc3339)
        .is_ok()
        .then_some(stored)
}

/// Dead letters `list_dead_letters` returns per call.
pub const DEAD_LETTER_PAGE: usize = 20;

impl DeadLetters {
    /// The page of `all` (most recent first) and the count of all of them.
    fn of(all: Vec<mako_engine::store_slatedb::DeadLetterRecord>) -> Self {
        let gesamt = all.len();
        Self {
            dead_letters: all
                .into_iter()
                .take(DEAD_LETTER_PAGE)
                .map(|r| DeadLetter {
                    rejected_at: rfc3339(r.rejected_at),
                    reason_label: r.reason_label,
                    reason_detail: r.reason_detail,
                })
                .collect(),
            gesamt,
        }
    }
}

/// Which process's history to read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProcessHistoryParams {
    /// The process id, as `get_process` returns it.
    pub process_id: String,
}

/// A process's history, oldest first, as `get_process_history` returns it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProcessHistory {
    pub process_id: String,
    /// The workflow running the process.
    pub workflow_name: String,
    /// Every event of the process, in the order it was recorded.
    pub eintraege: Vec<HistoryEntry>,
}

/// One recorded event. `richtung`, `pid` and `message_ref` are present when the
/// event is a market message, as its workflow states it — never derived from
/// `event_type`, which is an identifier.
#[derive(Debug, Serialize, JsonSchema)]
pub struct HistoryEntry {
    /// 1-based position in the process's stream.
    pub sequence: u64,
    /// When the event was recorded, RFC 3339 in UTC.
    #[schemars(extend("format" = "date-time"))]
    pub recorded_at: Option<String>,
    /// The workflow's name for the event.
    pub event_type: String,
    /// `eingang` or `ausgang` for a market message.
    pub richtung: Option<String>,
    /// The message's Prüfidentifikator.
    pub pid: Option<u32>,
    /// The message's document number (`BGM` C106).
    pub message_ref: Option<String>,
    /// DE 9013 — the code the answer gives (`A02`, `E15`, `Z12`, …).
    pub antwortcode: Option<String>,
    /// DE 1131 — the Codeliste or Entscheidungsbaum that defines the code
    /// (`E_0622`, `S_0090`).
    pub codeliste: Option<String>,
    /// The BDEW's wording for the code in that list, from the catalogue.
    /// `None` when the list does not publish the code — never another tree's
    /// meaning.
    pub bedeutung: Option<String>,
}

/// What kind of deadline a label is: a delivery window `mako_fristen` owns
/// (`aperak`, `contrl`) or one the workflow registered (`prozess`).
fn deadline_art(label: &str) -> &'static str {
    if mako_fristen::discharges_delivery_window("APERAK", label) {
        "aperak"
    } else if mako_fristen::discharges_delivery_window("CONTRL", label) {
        "contrl"
    } else {
        "prozess"
    }
}

/// A process's events as `get_process_history` answers them: what the
/// workflow stamped, nothing read off the event type.
fn history(process_id: &str, events: &[mako_engine::envelope::EventEnvelope]) -> ProcessHistory {
    use time::format_description::well_known::Rfc3339;
    ProcessHistory {
        process_id: process_id.to_owned(),
        workflow_name: events
            .first()
            .map(|e| e.workflow_id.name.as_ref().to_owned())
            .unwrap_or_default(),
        eintraege: events
            .iter()
            .map(|e| HistoryEntry {
                sequence: e.sequence_number,
                recorded_at: e.timestamp.format(&Rfc3339).ok(),
                event_type: e.event_type.to_string(),
                richtung: e.verlauf.as_ref().map(|v| {
                    match v.richtung {
                        mako_engine::envelope::Richtung::Eingang => "eingang",
                        mako_engine::envelope::Richtung::Ausgang => "ausgang",
                    }
                    .to_owned()
                }),
                pid: e.verlauf.as_ref().and_then(|v| v.pid),
                message_ref: e.verlauf.as_ref().and_then(|v| v.message_ref.clone()),
                antwortcode: e.antwort.as_ref().map(|a| a.code.clone()),
                codeliste: e.antwort.as_ref().and_then(|a| a.codeliste.clone()),
                bedeutung: e.antwort.as_ref().and_then(|a| {
                    let liste = a.codeliste.as_deref()?;
                    mako_pruefung::codes::lies(liste, &a.code).map(|c| c.bedeutung.to_owned())
                }),
            })
            .collect(),
    }
}

/// One process under a business key, with the deadlines pending on it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProcessView {
    /// The process's event stream.
    pub stream_id: String,
    /// The workflow running the process.
    pub workflow_name: String,
    /// The process identifier within its workflow.
    pub process_id: String,
    /// The deadlines registered on the process's stream.
    pub deadlines: Vec<DeadlineView>,
}

/// One deadline pending on a process.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeadlineView {
    /// The deadline's identifier.
    pub deadline_id: String,
    /// The label its workflow declares in `DEADLINE_LABELS`.
    pub label: String,
    /// `aperak` or `contrl` for a delivery window — the labels
    /// `mako_fristen` owns — and `prozess` for every deadline the workflow
    /// itself registered.
    pub art: String,
    /// When the deadline fires, RFC 3339 in UTC.
    #[schemars(extend("format" = "date-time"))]
    pub due_at: Option<String>,
}

/// The processes a business key names. A process is indexed under its
/// business key in the correlation index when it starts — from an inbound
/// message or a command alike — while the registry holds process ids only, so
/// the index answers a MaLo-, MeLo- or Vorgangsnummer and the registry a
/// process id.
async fn processes_named(
    registry: &impl mako_engine::registry::ProcessRegistry,
    tenant: TenantId,
    key: &mako_engine::registry::RegistryKey,
) -> Result<Vec<mako_engine::ids::ProcessIdentity>, mako_engine::error::EngineError> {
    let correlated = registry.lookup_correlated(tenant, key.as_str()).await?;
    if !correlated.is_empty() {
        return Ok(correlated);
    }
    Ok(registry.lookup(tenant, key).await?.into_iter().collect())
}

/// Workflow name used when a tool reads across every workflow at once.
///
/// `ReadProcess` carries the workflow in its Cedar context so a VIU deployment
/// can scope a principal to one arm (§ 6a EnWG). A cross-workflow listing has no
/// single workflow to name, so it is evaluated against this wildcard: an
/// unscoped grant permits it, a `context.workflow like "gpke-*"` grant does not.
const ANY_WORKFLOW: &str = "*";

// ── MCP handler ───────────────────────────────────────────────────────────────

/// MCP server handler — one instance per MCP session (clone-per-request).
#[derive(Clone)]
pub struct MakodMcpHandler {
    state: Arc<MakodMcpState>,
    #[expect(
        dead_code,
        reason = "used by the #[tool_router] macro-generated dispatch code"
    )]
    tool_router: ToolRouter<MakodMcpHandler>,
    #[expect(
        dead_code,
        reason = "used by the #[prompt_router] macro-generated dispatch code"
    )]
    prompt_router: PromptRouter<MakodMcpHandler>,
}

/// Extract the identity `mcp_auth_middleware` attached to the request.
///
/// Every tool that reads tenant data needs it, so the extraction lives here
/// rather than being repeated per tool.
fn caller(
    parts: &axum::http::request::Parts,
) -> Result<crate::cedar_authz::CallerIdentity, McpError> {
    parts
        .extensions
        .get::<crate::cedar_authz::CallerIdentity>()
        .cloned()
        .ok_or_else(|| {
            McpError::internal_error(
                "authenticated identity missing from request context".to_owned(),
                None,
            )
        })
}

#[tool_router]
impl MakodMcpHandler {
    fn new(state: Arc<MakodMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Gate a tool on the same Cedar action its REST twin enforces.
    ///
    /// `UseMcp` authorizes the *transport* — reaching the endpoint at all. It
    /// has never been meant to carry the per-resource grants, which is why
    /// `submit_command` evaluates `SubmitCommand` and the process tools
    /// evaluate `ReadProcess`. The reads below were the gap: a principal
    /// holding only `UseMcp` could pull the MaLo cache and the entire partner
    /// directory through MCP while `GET /admin/partners` refused it — and the
    /// shipped least-privilege policy grants `mcp-agent` `AdminMaloRead`
    /// precisely because the author expected this check to exist.
    fn require_malo_read(&self, parts: &axum::http::request::Parts) -> Result<(), McpError> {
        let identity = caller(parts)?;
        if self.state.cedar.authorize_malo(
            &identity,
            crate::cedar_authz::MakoAction::AdminMaloRead,
            &crate::cedar_authz::MaloResource {
                tenant: &self.state.tenant,
                malo_id: None,
            },
        ) {
            Ok(())
        } else {
            Err(McpError::invalid_request(
                "Cedar policy denied AdminMaloRead".to_owned(),
                None,
            ))
        }
    }

    /// Gate a tool on `AdminPartnerRead` — the action `GET /admin/partners`
    /// enforces.
    fn require_partner_read(&self, parts: &axum::http::request::Parts) -> Result<(), McpError> {
        let identity = caller(parts)?;
        if self.state.cedar.authorize_partner(
            &identity,
            crate::cedar_authz::MakoAction::AdminPartnerRead,
            &crate::cedar_authz::PartnerResource {
                tenant: &self.state.tenant,
                mp_id: None,
            },
        ) {
            Ok(())
        } else {
            Err(McpError::invalid_request(
                "Cedar policy denied AdminPartnerRead".to_owned(),
                None,
            ))
        }
    }

    /// Gate a cross-workflow process read on `ReadProcess`.
    ///
    /// The listing spans every workflow, so it is evaluated against the
    /// wildcard workflow name. A policy that scopes a principal to one arm of a
    /// VIU deployment (`context.workflow like "gpke-sperrung*"`) therefore
    /// denies the unfiltered listing outright — which is the intended § 6a EnWG
    /// answer: an NB-scoped principal must not receive a cross-workflow view,
    /// and there is nothing meaningful to filter it down to.
    fn require_any_process_read(&self, parts: &axum::http::request::Parts) -> Result<(), McpError> {
        let identity = caller(parts)?;
        if self.state.cedar.authorize_process_read(
            &identity,
            &crate::cedar_authz::ProcessResource {
                tenant: &self.state.tenant,
                workflow: ANY_WORKFLOW,
            },
        ) {
            Ok(())
        } else {
            Err(McpError::invalid_request(
                "Cedar policy denied ReadProcess across workflows".to_owned(),
                None,
            ))
        }
    }

    /// List all MaKo commands available for this instance's configured Marktrollen.
    ///
    /// Returns a filtered list of commands with their Marktrolle(n), primary
    /// Prüfidentifikator (PID), and whether a `marktrolle` override is required
    /// at dispatch time. Call this before `submit_command` to discover what is
    /// available and what payload fields are expected.
    ///
    /// IFTSTA sink commands (`*.empfangen`) are excluded — those are driven by
    /// inbound EDIFACT messages, not by ERP/LLM initiations.
    #[tool(
        description = "List all MaKo commands available for this instance (filtered to configured Marktrollen). Call this before submit_command to discover what you can submit.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    #[instrument(skip(self))]
    async fn list_commands(&self) -> Result<CallToolResult, McpError> {
        let configured = &self.state.commands.configured_marktrollen;

        let commands: Vec<serde_json::Value> = COMMAND_REGISTRY
            .iter()
            .filter(|d| {
                !d.name.ends_with(".empfangen")
                    && d.permitted_roles
                        .iter()
                        .any(|r| configured.iter().any(|c| c == r.as_code()))
            })
            .map(|d| {
                let effective_roles: Vec<&str> = d
                    .permitted_roles
                    .iter()
                    .filter(|r| configured.iter().any(|c| c == r.as_code()))
                    .map(|r| r.as_code())
                    .collect();
                serde_json::json!({
                    "command": d.name,
                    "marktrolle": if effective_roles.len() == 1 {
                        serde_json::json!(effective_roles[0])
                    } else {
                        serde_json::json!(effective_roles)
                    },
                    "pid": d.primary_pid.map_or(serde_json::Value::Null, |p| serde_json::json!(p.as_u32())),
                    "multi_role": effective_roles.len() > 1,
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "configured_marktrollen": configured,
            "commands": commands,
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Submit a MaKo process command.
    ///
    /// Triggers a GPKE, GeLi Gas, WiM, or MABIS workflow command. The daemon
    /// resolves trading-partner MP-IDs from the MaLo cache and generates the
    /// outbound EDIFACT message automatically.
    ///
    /// On success returns `process_id`, effective `marktrolle`, and `status`
    /// (`"spawned"` for new processes, `"dispatched"` for existing ones).
    /// Use `next_steps` in the response for the applicable regulatory deadline.
    ///
    /// Common error prefixes: `malo_not_found`, `invalid_payload`,
    /// `duplicate_process`, `process_not_found`, `role_not_configured`,
    /// `engine_error`.
    #[tool(
        description = "Submit a MaKo process command (GPKE, GeLi Gas, WiM, MABIS). Use list_commands first to see what's available and what payload fields are required.",
        annotations(
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    #[instrument(skip(self, client), fields(command = %p.command))]
    async fn submit_command(
        &self,
        Parameters(p): Parameters<SubmitCommandParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
        meta: RequestMetaObject,
        client: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let progress_token = meta.get_progress_token();

        // ── Phase 1: validation (20 %) ────────────────────────────────────
        if let Some(ref token) = progress_token {
            let _ = client
                .notify_progress(
                    ProgressNotificationParam::new(token.clone(), 0.2)
                        .with_total(1.0)
                        .with_message(String::from("Validating command and Marktrolle...")),
                )
                .await;
        }

        let cmd_lower = p.command.to_lowercase();
        let asserted = p.marktrolle.as_deref().map(str::to_uppercase);

        let effective_role = validate_command(
            &cmd_lower,
            asserted.as_deref(),
            &self.state.commands.configured_marktrollen,
        )
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        // ── Per-command Cedar authorization ───────────────────────────────
        // Transport-level `UseMcp` grants access to the MCP endpoint; it does
        // NOT grant command submission. The same `SubmitCommand` action the
        // REST `/api/v1/commands` handler evaluates runs here, with the
        // identity the transport middleware authenticated.
        let identity = parts
            .extensions
            .get::<crate::cedar_authz::CallerIdentity>()
            .cloned()
            .ok_or_else(|| {
                McpError::internal_error(
                    "authenticated identity missing from request context".to_owned(),
                    None,
                )
            })?;
        if !self.state.cedar.authorize_command(
            &identity,
            &crate::cedar_authz::CommandResource {
                name: &cmd_lower,
                marktrolle: &effective_role,
                pid: crate::commands_api::command_primary_pid(&cmd_lower)
                    .map_or(0, mako_engine::types::Pruefidentifikator::as_u32),
                tenant: &self.state.commands.tenant_id.to_string(),
            },
        ) {
            return Err(McpError::invalid_request(
                format!("Cedar policy denied SubmitCommand for {cmd_lower:?}"),
                None,
            ));
        }

        // Same idempotency contract as `POST /api/v1/commands`, so the two
        // doors cannot answer a retry differently. The key stays *optional*
        // here — an interactive agent has no ERP order ID to supply — but when
        // one is given it is honoured: a repeat replays the recorded response,
        // and reuse for a different request is refused rather than answered
        // with the first command's `process_id`.
        let tenant = self.state.commands.tenant_id.to_string();
        let fingerprint = crate::commands_api::idempotency::fingerprint(&cmd_lower, &p.payload);
        if let Some(key) = p.idempotency_key.as_deref() {
            use crate::commands_api::idempotency::{Lookup, lookup};
            match lookup(&self.state.commands.store, &tenant, key, &fingerprint).await {
                Ok(Lookup::Replay(body)) => {
                    return ContentBlock::json(body)
                        .map(|block| CallToolResult::success(vec![block]))
                        .map_err(|e| McpError::internal_error(e.message, None));
                }
                Ok(Lookup::Conflict) => {
                    return Err(McpError::invalid_request(
                        "idempotency_key_reuse: this key was already used for a                          different command or payload"
                            .to_owned(),
                        None,
                    ));
                }
                Ok(Lookup::Fresh) => {}
                Err(e) => {
                    return Err(McpError::internal_error(
                        format!("idempotency lookup failed: {e}"),
                        None,
                    ));
                }
            }
        }
        let idempotency_key = p
            .idempotency_key
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        tracing::info!(
            command = %cmd_lower,
            marktrolle = %effective_role,
            idempotency_key = %idempotency_key,
            "MCP: submit_command",
        );

        // ── Phase 2: engine dispatch (80 %) ──────────────────────────────
        if let Some(ref token) = progress_token {
            let _ = client
                .notify_progress(
                    ProgressNotificationParam::new(token.clone(), 0.8)
                        .with_total(1.0)
                        .with_message(format!("Dispatching {cmd_lower} to EDIFACT engine...")),
                )
                .await;
        }

        match dispatch_command(&self.state.commands, &cmd_lower, &p.payload).await {
            Ok(outcome) => {
                let (process_id, status) = match outcome {
                    DispatchOutcome::Spawned { process_id } => (process_id.to_string(), "spawned"),
                    DispatchOutcome::Dispatched { process_id } => {
                        (process_id.to_string(), "dispatched")
                    }
                };
                let body = serde_json::json!({
                    "idempotency_key": idempotency_key,
                    "command": cmd_lower,
                    "marktrolle": effective_role,
                    "process_id": process_id,
                    "status": status,
                    "next_steps": next_steps_hint(&cmd_lower),
                });
                if let Some(key) = p.idempotency_key.as_deref()
                    && let Err(e) = crate::commands_api::idempotency::record(
                        &self.state.commands.store,
                        &tenant,
                        key,
                        &fingerprint,
                        &body,
                    )
                    .await
                {
                    tracing::error!(
                        error = %e,
                        "MCP submit_command: idempotency record could not be written",
                    );
                }
                ContentBlock::json(body)
                    .map(|block| CallToolResult::success(vec![block]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => {
                let msg = dispatch_error_to_string(e);
                Ok(CallToolResult::error(vec![ContentBlock::text(msg)]))
            }
        }
    }

    /// Read a cached Marktlokation (MaLo) record.
    ///
    /// Returns the full `MaloIdentResultPositive` from the BDEW API-Webdienste
    /// Strom MaLo identification response. Returns an error if the MaLo is not
    /// in the cache — seed it first via `PUT /admin/malo/{malo_id}`.
    #[tool(
        description = "Read a cached Marktlokation (MaLo) record by its 11-digit ID",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    #[instrument(skip(self), fields(malo_id = %p.malo_id))]
    async fn get_malo(
        &self,
        Parameters(p): Parameters<GetMaloParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.require_malo_read(&parts)?;
        match self
            .state
            .malo_cache
            .get(&self.state.tenant, &p.malo_id)
            .await
        {
            Ok(Some(record)) => ContentBlock::json(&record)
                .map(|block| CallToolResult::success(vec![block]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "malo_not_found: MaLo '{}' is not in the cache. \
                 Seed it first via PUT /admin/malo/{}.",
                p.malo_id, p.malo_id
            ))])),
            Err(e) => Err(McpError::internal_error(
                format!("Cache read failed: {e}"),
                None,
            )),
        }
    }

    /// List all registered trading partners for this tenant.
    ///
    /// Returns a paginated array of partner records, each containing the MP-ID,
    /// AS4 endpoint URL, market roles, and communication channels. An empty
    /// array is returned when no partners have been registered yet.
    ///
    /// Use `limit` and `cursor` to page through large directories.
    #[tool(
        description = "List registered trading partners for this tenant (paginated, max 500)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    #[instrument(skip(self), fields(limit = ?p.limit, cursor = ?p.cursor))]
    async fn list_partners(
        &self,
        Parameters(p): Parameters<ListPartnersParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.require_partner_read(&parts)?;
        let limit = p.limit.unwrap_or(100).clamp(1, 500);

        match self
            .state
            .partner_store
            .list(TenantId::from_party_id(&self.state.tenant))
            .await
        {
            Ok(all_partners) => {
                // Decode the cursor as a base-10 offset index.
                let offset = p
                    .cursor
                    .as_deref()
                    .and_then(|c| c.parse::<usize>().ok())
                    .unwrap_or(0);

                let page: Vec<_> = all_partners.into_iter().skip(offset).take(limit).collect();
                let next_offset = offset + page.len();
                // Emit a cursor only when there may be more results.
                let next_cursor: Option<String> = if page.len() == limit {
                    Some(next_offset.to_string())
                } else {
                    None
                };

                ContentBlock::json(serde_json::json!({
                    "partners":    page,
                    "next_cursor": next_cursor,
                }))
                .map(|block| CallToolResult::success(vec![block]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(
                format!("Partner store read failed: {e}"),
                None,
            )),
        }
    }

    /// Get a specific trading partner by MP-ID.
    ///
    /// Returns the full partner record including the AS4 inbox URL, market
    /// roles, and communication channel configuration. Returns an error if
    /// the partner is not registered.
    #[tool(
        description = "Get a trading partner record by Marktpartner-ID",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    #[instrument(skip(self), fields(mp_id = %p.mp_id))]
    async fn get_partner(
        &self,
        Parameters(p): Parameters<GetPartnerParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.require_partner_read(&parts)?;
        let mp_id = mako_engine::types::MarktpartnerCode::new(p.mp_id.as_str());

        match self
            .state
            .partner_store
            .get(TenantId::from_party_id(&self.state.tenant), &mp_id)
            .await
        {
            Ok(Some(record)) => ContentBlock::json(&record)
                .map(|block| CallToolResult::success(vec![block]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "partner_not_found: Partner '{}' is not registered. \
                 Register it via PUT /admin/partners/{}.",
                p.mp_id, p.mp_id
            ))])),
            Err(e) => Err(McpError::internal_error(
                format!("Partner store read failed: {e}"),
                None,
            )),
        }
    }

    /// Get daemon health and runtime status.
    ///
    /// Returns the daemon version, instance ID, MaLo cache statistics, and
    /// configured Marktrollen. Useful for verifying connectivity and checking
    /// the current operational state before submitting commands.
    #[tool(
        description = "Get makod health status, version, and MaLo cache statistics",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    #[instrument(skip(self))]
    async fn get_health(&self) -> Result<CallToolResult, McpError> {
        let cache_stats = self.state.malo_cache.stats(&self.state.tenant).await.ok();

        ContentBlock::json(serde_json::json!({
            "status": "ok",
            "version": self.state.version,
            "tenant_id": self.state.tenant,
            "configured_marktrollen": self.state.commands.configured_marktrollen,
            "malo_cache": cache_stats.map(|s| serde_json::json!({
                "count": s.count,
                "last_upsert": s.last_upsert,
            })),
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Look up the MaKo processes a business key names.
    ///
    /// Returns every process the key is correlated with — its stream, workflow
    /// and pending deadlines — so a MaLo with a running Lieferbeginn and a
    /// Sperrung answers both. The key is the one the process was indexed under
    /// when it started, whether from an inbound message or a command:
    /// - GPKE / GeLi Gas → `malo_id` (11-digit Marktlokations-ID)
    /// - WiM Strom → `melo_id` (Messlokations-ID)
    /// - WiM Gas Stornierung → `vorgang_id` (Vorgangsnummer from PID 44022)
    ///
    /// A process id is accepted too. Returns `process_not_found` when the key
    /// names no process the caller may read.
    #[tool(
        description = "Look up the MaKo processes a business key names (malo_id, melo_id, vorgang_id or a process id): each process's stream, workflow and pending deadlines.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ProcessesView>()
    )]
    #[instrument(skip(self), fields(business_key = %p.business_key))]
    async fn get_process(
        &self,
        Parameters(p): Parameters<GetProcessParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        use mako_engine::registry::RegistryKey;

        let key = RegistryKey::parse(&p.business_key)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let tenant = TenantId::from_party_id(&self.state.tenant);
        let internal =
            |e: mako_engine::error::EngineError| McpError::internal_error(e.to_string(), None);
        let identities = processes_named(
            &self.state.process_store.as_process_registry(),
            tenant,
            &key,
        )
        .await
        .map_err(internal)?;

        // § 6a EnWG Informatorisches Unbundling: process reads are scoped per
        // workflow, so a VIU deployment can deny an NB-scoped principal access
        // to LF process state. A process the caller may not read is left out,
        // and none at all answers like not-found, so the reply is no oracle.
        let caller = parts
            .extensions
            .get::<crate::cedar_authz::CallerIdentity>()
            .cloned()
            .ok_or_else(|| {
                McpError::internal_error(
                    "authenticated identity missing from request context".to_owned(),
                    None,
                )
            })?;
        let mut processes = Vec::new();
        for id in identities {
            if !self.state.cedar.authorize_process_read(
                &caller,
                &crate::cedar_authz::ProcessResource {
                    tenant: &self.state.tenant,
                    workflow: id.workflow_id.name.as_ref(),
                },
            ) {
                continue;
            }
            // A store failure is an error, never „no deadlines": an empty list
            // reads as „nothing pending" on the screen a person acts from.
            let deadlines = self
                .state
                .deadline_store
                .for_stream(id.stream_id())
                .await
                .map_err(internal)?;
            processes.push(ProcessView {
                stream_id: id.stream_id().to_string(),
                workflow_name: id.workflow_id.name.as_ref().to_owned(),
                process_id: id.process_id.to_string(),
                deadlines: deadlines
                    .iter()
                    .map(|d| {
                        use time::format_description::well_known::Rfc3339;
                        DeadlineView {
                            deadline_id: d.deadline_id().to_string(),
                            label: d.label().to_owned(),
                            art: deadline_art(d.label()).to_owned(),
                            due_at: d.due_at().format(&Rfc3339).ok(),
                        }
                    })
                    .collect(),
            });
        }

        if processes.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "process_not_found: No process for business key '{}'.",
                p.business_key
            ))]));
        }
        serde_json::to_value(ProcessesView {
            business_key: p.business_key,
            processes,
        })
        .map(CallToolResult::structured)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// A process's events, oldest first, with the market messages among them
    /// marked by direction and PID — the thread the Desk's Verlauf draws.
    #[tool(
        description = "A process's recorded events, oldest first (process_id from get_process). An event that is a market message carries richtung (eingang/ausgang), pid and message_ref, as its workflow states it.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<ProcessHistory>()
    )]
    async fn get_process_history(
        &self,
        Parameters(p): Parameters<GetProcessHistoryParams>,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        use mako_engine::event_store::EventStore as _;

        let not_found = || {
            CallToolResult::error(vec![ContentBlock::text(format!(
                "process_not_found: No process '{}'.",
                p.process_id
            ))])
        };
        let Ok(uuid) = uuid::Uuid::parse_str(p.process_id.trim()) else {
            return Err(McpError::invalid_params(
                "process_id is not a process id".to_owned(),
                None,
            ));
        };
        // The stream is derived from this deployment's tenant, so a process id
        // can name no other tenant's history.
        let tenant = TenantId::from_party_id(&self.state.tenant);
        let stream = mako_engine::ids::StreamId::for_process(
            tenant,
            &mako_engine::ids::ProcessId::from_uuid(uuid),
        );
        let events = self
            .state
            .process_store
            .load(&stream)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let Some(first) = events.first() else {
            return Ok(not_found());
        };
        let caller = parts
            .extensions
            .get::<crate::cedar_authz::CallerIdentity>()
            .cloned()
            .ok_or_else(|| {
                McpError::internal_error(
                    "authenticated identity missing from request context".to_owned(),
                    None,
                )
            })?;
        // § 6a EnWG: the same per-workflow decision as `get_process`, and a
        // refusal answers like not-found, so the reply is no oracle.
        if !self.state.cedar.authorize_process_read(
            &caller,
            &crate::cedar_authz::ProcessResource {
                tenant: &self.state.tenant,
                workflow: first.workflow_id.name.as_ref(),
            },
        ) {
            return Ok(not_found());
        }
        serde_json::to_value(history(p.process_id.trim(), &events))
            .map(CallToolResult::structured)
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// List deadlines whose instant has passed and that the deadline worker has
    /// not fired yet.
    ///
    /// Deadlines are never cancelled: each fires into its process, whose
    /// `window_open` decides whether the window is still open. An answered
    /// process therefore stays listed until its timer fires, and a non-empty
    /// result says the worker is behind or failing — not that a Frist was
    /// missed. A missed Frist is the process's own outcome, which processd's
    /// read model reports (`list_overdue_processes`).
    ///
    /// The label names the window: `aperak-*` the APERAK sending window,
    /// `contrl-*` the CONTRL delivery window, every other one the business
    /// Antwortfrist of its Prüfidentifikator — all resolved by `mako_fristen`.
    ///
    /// § 6a EnWG informatorisches Unbundling applies here exactly as it does to
    /// [`Self::get_process`]: each entry names a workflow and its timing, so an
    /// unfiltered listing would tell an NB-scoped principal in a combined-role
    /// (VIU) deployment which supply-side processes exist and which are missing
    /// their regulatory windows. Entries are filtered through the same
    /// `ReadProcess` decision, which the default policy permits for every
    /// authenticated principal — a single-role deployment sees no change.
    ///
    /// Filtering happens after the store scan, so the scan window
    /// ([`OVERDUE_SCAN_WINDOW`]) is deliberately wider than the returned page
    /// ([`OVERDUE_PAGE`]) — otherwise a window full of the other arm's
    /// deadlines would starve a filtered principal of its own.
    #[tool(
        description = "Deadlines whose instant has passed and that the deadline worker has not fired yet (up to 50). \
Deadlines are never cancelled, so an answered process is listed too until its timer fires: a non-empty \
list means the worker is behind or failing, not that a Frist was missed. Whether a process missed its \
Frist is its own outcome — processd `list_overdue_processes`.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    #[instrument(skip(self))]
    async fn list_overdue_deadlines(
        &self,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .state
            .deadline_store
            .due_now(OVERDUE_SCAN_WINDOW)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let caller = parts
            .extensions
            .get::<crate::cedar_authz::CallerIdentity>()
            .cloned()
            .ok_or_else(|| {
                McpError::internal_error(
                    "authenticated identity missing from request context".to_owned(),
                    None,
                )
            })?;

        let visible: Vec<&mako_engine::deadline::Deadline> = result
            .deadlines
            .iter()
            .filter(|d| {
                self.state.cedar.authorize_process_read(
                    &caller,
                    &crate::cedar_authz::ProcessResource {
                        tenant: &self.state.tenant,
                        workflow: d.workflow_id().name.as_ref(),
                    },
                )
            })
            .collect();

        // `has_more` must stay truthful after filtering: either the store had
        // more beyond the scan window, or filtering left more than one page.
        let has_more = result.has_more || visible.len() > OVERDUE_PAGE;

        let items: Vec<serde_json::Value> = visible
            .iter()
            .take(OVERDUE_PAGE)
            .map(|d| {
                use time::format_description::well_known::Rfc3339;
                let overdue_mins = (time::OffsetDateTime::now_utc() - d.due_at()).whole_minutes();
                serde_json::json!({
                    "deadline_id": d.deadline_id().to_string(),
                    "stream_id":   d.stream_id().to_string(),
                    "label":       d.label(),
                    "due_at":      d.due_at().format(&Rfc3339).ok(),
                    "overdue_by_mins": overdue_mins,
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "overdue_count": items.len(),
            "has_more":      has_more,
            "alert":         !items.is_empty(),
            "items":         items,
            "note": "Each entry is a deadline not yet fired. The worker fires it into its \
                     process, which decides whether the window is still open.",
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Return how many processes this makod instance has started for its
    /// tenant — finished ones included.
    ///
    /// Counts the tenant's event streams (`process/{tenant}/…`), one per
    /// process. Whether a process is still running is its own replayed state,
    /// which this count does not compute; use it for capacity planning, and
    /// `list_overdue_deadlines` for what needs attention.
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        description = "Return how many MaKo processes this instance has started for its tenant, finished ones included."
    )]
    async fn list_active_processes(&self) -> Result<CallToolResult, McpError> {
        use mako_engine::event_store::EventStore as _;
        let prefix = format!("process/{}/", TenantId::from_party_id(&self.state.tenant));
        let count = self
            .state
            .process_store
            .list_streams(Some(&prefix))
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .len();
        ContentBlock::json(serde_json::json!({
            "process_count": count,
            "tenant": self.state.tenant,
            "note": "Processes started for this tenant, finished ones included — one event stream each.",
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Report, per EDIFACT message type, whether today falls inside a
    /// format-version transition window.
    ///
    /// The question this answers is the one an operator has during the annual
    /// BDEW cutover: *are both the outgoing and incoming format versions
    /// dispatchable right now, or have we already switched?*
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        description = "Report per message type whether today is inside a BDEW format-version \
                       transition window, naming the outgoing and incoming releases. Use during \
                       the annual FV cutover to confirm both versions are dispatchable."
    )]
    async fn get_format_version_coverage(&self) -> Result<CallToolResult, McpError> {
        use edi_energy::registry::{ReleaseRegistry, TransitionState};

        let registry = ReleaseRegistry::global();
        let today = mako_fristen::heute();

        // Derived from the registry rather than a hardcoded list: `MessageType`
        // is `#[non_exhaustive]`, so an enumeration written here would silently
        // omit any type added upstream — and this tool would then report full
        // coverage while saying nothing about the new one.
        let mut message_types: Vec<edi_energy::MessageType> = registry
            .all_profiles()
            .iter()
            .map(|p| p.message_type())
            .collect();
        message_types.sort_by_key(|mt| mt.as_str());
        message_types.dedup();

        let mut types = Vec::new();
        let mut in_transition = 0_usize;
        for mt in message_types {
            let entry = match registry.transition_state(mt, today, None) {
                TransitionState::Stable { profile } => serde_json::json!({
                    "message_type": mt.to_string(),
                    "state":        "stable",
                    "release":      profile.release().as_str(),
                    "valid_from":   profile.valid_from().map(|d| d.to_string()),
                    "valid_until":  profile.valid_until().map(|d| d.to_string()),
                }),
                TransitionState::Transition { outgoing, incoming } => {
                    in_transition += 1;
                    serde_json::json!({
                        "message_type":     mt.to_string(),
                        "state":            "transition",
                        "outgoing_release": outgoing.release().as_str(),
                        "incoming_release": incoming.release().as_str(),
                        "note": "Both releases must be accepted inbound. Prefer the \
                                 incoming release for outbound.",
                    })
                }
                TransitionState::None => serde_json::json!({
                    "message_type": mt.to_string(),
                    "state":        "no_profile",
                    "note": "No profile is valid on this date — inbound messages of \
                             this type cannot be validated.",
                }),
            };
            types.push(entry);
        }

        ContentBlock::json(serde_json::json!({
            "as_of":                 today.to_string(),
            "receive_tolerance_days": registry.receive_tolerance_days(),
            "types_in_transition":   in_transition,
            "message_types":         types,
            "note": "Adapter coverage itself is enforced at startup — makod panics if a \
                     workflow lacks a MessageAdapter for any known format version, so a \
                     running instance always covers every release listed here. This tool \
                     reports which releases are *in force*, which is what changes daily \
                     during a cutover.",
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Return outbox delivery status: total pending messages, oldest pending message age.
    ///
    /// Use for operational monitoring and incident triage ("are messages stuck in
    /// the outbox?").  A non-zero queue with old messages indicates a stalled
    /// outbox worker or unavailable trading partner.
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        description = "Return current AS4 outbox delivery status: pending count and age."
    )]
    async fn get_outbox_status(&self) -> Result<CallToolResult, McpError> {
        use mako_engine::outbox::OutboxStore as _;
        // pending_now gives a snapshot of up to 100 messages due for delivery now.
        let pending: Vec<mako_engine::outbox::OutboxMessage> = self
            .state
            .process_store
            .pending_now(100)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let total_in_queue: usize = self
            .state
            .process_store
            .len()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let now = time::OffsetDateTime::now_utc();
        let oldest_secs: i64 = pending
            .iter()
            .map(|m| (now - m.created_at).whole_seconds())
            .max()
            .unwrap_or(0);

        let alert = total_in_queue > 0 && oldest_secs > 300; // >5 min = potentially stuck
        ContentBlock::json(serde_json::json!({
            "pending_now_sample":    pending.len(),
            "total_in_queue":        total_in_queue,
            "oldest_pending_secs":   oldest_secs,
            "alert": alert,
            "tenant": self.state.tenant,
            "note": if alert {
                "⚠️ Messages have been in the outbox for more than 5 minutes — \
                 the outbox worker may be stalled or the recipient endpoint may be down. \
                 Check makod logs for AS4 delivery errors."
            } else {
                "Outbox is processing normally."
            },
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// The most recent permanently-failed (dead-lettered) messages, and how
    /// many the store holds in all.
    ///
    /// A dead letter is an inbound message no workflow could take or an
    /// outbound one whose delivery was exhausted; each needs an operator,
    /// and `reason_label` says which. `gesamt` is
    /// the count of every record, so a reader never takes the page for the
    /// total.
    #[tool(
        annotations(read_only_hint = true, open_world_hint = false),
        description = "The 20 most recent permanently dead-lettered messages, the most recent first, and `gesamt`: how many dead letters the store holds in all.",
        output_schema = rmcp::handler::server::tool::schema_for_output::<DeadLetters>()
    )]
    async fn list_dead_letters(
        &self,
        rmcp::handler::server::tool::Extension(parts): rmcp::handler::server::tool::Extension<
            axum::http::request::Parts,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.require_any_process_read(&parts)?;
        // The store scans its whole `dr/` key space for any limit, so reading
        // every record costs what a page did and yields the true total.
        let all = self
            .state
            .process_store
            .list_dead_letters(usize::MAX)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        serde_json::to_value(DeadLetters::of(all))
            .map(CallToolResult::structured)
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }
}

// ── Prompt argument types ─────────────────────────────────────────────────────

/// Arguments for the `gpke-lieferbeginn` prompt.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GpkeLieferbeginnArgs {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Supply start date (YYYY-MM-DD).
    pub lieferbeginn_datum: String,
}

/// Arguments for the `geli-lieferbeginn` prompt.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GeliLieferbeginnArgs {
    /// 11-digit gas Marktlokations-ID.
    pub malo_id: String,
    /// Supply start date (YYYY-MM-DD).
    pub lieferbeginn_datum: String,
}

/// Arguments for the `wim-geraetewechsel` prompt.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WimGeraetewechselArgs {
    /// 11-digit Messlokations-ID.
    pub melo_id: String,
    /// Requested execution date (YYYYMMDD, German local time).
    pub process_date: String,
    /// Marktpartner-ID of the counterparty: the NB for PIDs 55039/55042,
    /// the nMSB for 55051/55168.
    pub receiver_mp_id: String,
    /// Initiating Marktrolle: NB or MSB.
    pub marktrolle: String,
}

// ── Prompt router ─────────────────────────────────────────────────────────────

#[prompt_router]
impl MakodMcpHandler {
    #[prompt(
        name = "gpke-lieferbeginn",
        description = "Guided workflow: GPKE Lieferbeginn Strom (electricity supplier change)"
    )]
    async fn gpke_lieferbeginn_prompt(
        &self,
        Parameters(args): Parameters<GpkeLieferbeginnArgs>,
    ) -> Vec<PromptMessage> {
        let malo_id = &args.malo_id;
        let date = &args.lieferbeginn_datum;
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "I need to initiate a GPKE Lieferbeginn (electricity supplier change) \
                 for MaLo {malo_id} starting {date}.\n\
                 \n\
                 Please:\n\
                 1. Call `get_malo` with malo_id=\"{malo_id}\" to verify the MaLo is \
                    cached and show me the NB/MSB MP-IDs.\n\
                 2. If found, call `submit_command` with:\n\
                    - command: \"gpke.lieferbeginn.anmelden\"\n\
                    - payload: {{\"malo_id\": \"{malo_id}\", \"lieferbeginn_datum\": \"{date}\"}}\n\
                 3. Report the process_id and status.\n\
                 4. Explain next steps — the answer Frist, a Bestätigung (PID 55002) or \
                    Ablehnung (PID 55003):\n{frist}",
                frist = render_fristen(&[55_001]),
            ),
        )]
    }

    #[prompt(
        name = "geli-lieferbeginn",
        description = "Guided workflow: GeLi Gas Lieferbeginn (gas supplier change)"
    )]
    async fn geli_lieferbeginn_prompt(
        &self,
        Parameters(args): Parameters<GeliLieferbeginnArgs>,
    ) -> Vec<PromptMessage> {
        let malo_id = &args.malo_id;
        let date = &args.lieferbeginn_datum;
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "I need to initiate a GeLi Gas Lieferbeginn (gas supplier change) \
                 for gas MaLo {malo_id} starting {date}.\n\
                 \n\
                 Please:\n\
                 1. Call `get_malo` with malo_id=\"{malo_id}\" to verify the MaLo is \
                    cached and show me the GNB MP-ID.\n\
                 2. If found, call `submit_command` with:\n\
                    - command: \"geli.lieferbeginn.anmelden\"\n\
                    - payload: {{\"malo_id\": \"{malo_id}\", \"lieferbeginn_datum\": \"{date}\"}}\n\
                 3. Report the process_id and status.\n\
                 4. Explain next steps — the GNB's answer Frist (Saturday is not a \
                    Werktag; nor Sunday or a gesetzlicher Feiertag; German local time):\n{frist}",
                frist = render_fristen(&[44_001]),
            ),
        )]
    }

    #[prompt(
        name = "wim-geraetewechsel",
        description = "Guided workflow: WiM Gerätewechsel (meter device change)"
    )]
    async fn wim_geraetewechsel_prompt(
        &self,
        Parameters(args): Parameters<WimGeraetewechselArgs>,
    ) -> Vec<PromptMessage> {
        let melo_id = &args.melo_id;
        let date = &args.process_date;
        let receiver = &args.receiver_mp_id;
        let marktrolle = &args.marktrolle;
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "I need to initiate a WiM Gerätewechsel (meter device change) \
                 for MeLo {melo_id} on {date}, acting as {marktrolle}.\n\
                 \n\
                 Please:\n\
                 1. Call `submit_command` with:\n\
                    - command: \"wim.geraetewechsel.beauftragen\"\n\
                    - marktrolle: \"{marktrolle}\"\n\
                    - payload: {{\"melo_id\": \"{melo_id}\", \"process_date\": \"{date}\", \
                       \"receiver_mp_id\": \"{receiver}\"}}\n\
                 2. Report the process_id and status.\n\
                 3. Explain next steps — the answer Frist is the PID's:\n{frist}",
                frist = render_fristen(&[55_039, 55_042, 55_051, 55_168]),
            ),
        )]
    }
    #[prompt(
        name = "msb-preisanfrage",
        description = "Guided workflow: MSB Preisanfrage (REQOTE/QUOTES PIDs 35001/15001) and PRICAT 27003 dispatch"
    )]
    async fn msb_preisanfrage_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I request a price quote from the existing MSB (aMSB) when switching meter operators?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                format!(
                    "**MSB Preisanfrage (WiM Strom BK6-24-174)**\n\n\
                 The nMSB (new MSB) requests a price quote from the aMSB (existing MSB) before initiating a WiM-Wechsel.\n\n\
                 **Step 1 — The aMSB receives the inbound REQOTE (PID 35001) via AS4:**\n\
                 The AS4 inbound layer handles this automatically. The `wim-preisanfrage` workflow\n\
                 is spawned by `makod` when the REQOTE arrives. No ERP command needed.\n\n\
                 **Step 2 — the aMSB responds with QUOTES (PID 15001):**\n{frist}\n\
                 The Preisliste is stored in `marktd` as a `PreisblattMessung` record.\n\
                 Check: `get_partner` for the aMSB MP-ID to verify AS4 connectivity.\n\n\
                 **Step 3 — Verify process state:**\n\
                 Call `get_process` with the `melo_id` to check if the Preisanfrage process\n\
                 is active and what deadlines are pending.\n\n\
                 **Step 4 — Initiate WiM-Wechsel after price agreement:**\n\
                 `submit_command` with command: \"wim.geraetewechsel.beauftragen\"",
                    frist = render_fristen(&[35_001])
                ),
            ),
        ]
    }

    #[prompt(
        name = "wim-gas-anmeldung",
        description = "Guided workflow: WiM Gas MSB-Wechsel (GNB approves or rejects a GMSB Anmeldung request)"
    )]
    async fn wim_gas_anmeldung_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "I received a WiM Gas MSB-Wechsel Anmeldung (UTILMD G PID 44042–44053) from the new GMSB. How do I respond?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                format!(
                    "**WiM Gas MSB-Wechsel Anmeldung (AWH WiM Gas 2.0)**\n\n\
                 The GNB (Gasnetzbetreiber) answers within the PID's Frist (Saturdays, Sundays and gesetzliche Feiertage are not Werktage; German local time CET/CEST):\n{frist}\n\n\
                 **Step 1 — Find the process:**\n\
                 Call `get_process` with the gas `malo_id` from the inbound UTILMD G.\n\
                 - If `processes` lists the MSB-Wechsel workflow → proceed to step 2.\n\
                 - If `process_not_found` → the AS4 message was not yet ingested; check the AS4 inbound log.\n\n\
                 **Step 2 — Check the deadline:**\n\
                 Inspect that process's `deadlines` in the get_process response.\n\
                 A pending `response-*` deadline is when the Frist ends — answer before it. One whose `due_at` has passed \
                 has not fired yet: the deadline worker is behind (`list_overdue_deadlines`).\n\n\
                 **Step 3 — Answer.** WiM Strom and Gas run on one engine, so the \
                 answer is the Sparte-neutral MSB-Wechsel command; the Gas tree is \
                 selected from the inbound PID, not from the command name.\n\
                 Call `submit_command` with:\n\
                 - command: \"wim.geraetewechsel.bestaetigen\" or \".ablehnen\"\n\
                 - marktrolle: \"NB\" (or \"GNB\")\n\
                 - payload: {{\"melo_id\": \"<Messlokation>\", \"antwortcode\": \"<G_00xx code>\"}}\n\n\
                 **Valid ERC codes:** A02 (Bilanzierungsgebiet mismatch), A05 (unknown GMSB), A97 (invalid date), A99 (Mindestvorlauffrist not met).\n\n\
                 **Note:** `processd` auto-STP handles this automatically when configured. Manual dispatch is only needed for escalated cases.",
                    frist = render_fristen(&[44_042])
                ),
            ),
        ]
    }

    #[prompt(
        name = "gpke-sperrung",
        description = "Guided workflow: GPKE Sperrung Strom (LF confirms disconnection execution to NB)"
    )]
    async fn gpke_sperrung_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "The NB has executed a Sperrung (disconnection) for a MaLo. How do I confirm it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**GPKE Sperrung — Ausführungsmeldung (GPKE Teil 2 § 3.5)**\n\n\
                 This is the **NB side**. After the field team has (or has not) carried out the\n\
                 disconnection, the NB reports the outcome and `makod` dispatches IFTSTA 21039\n\
                 to the Lieferant.\n\n\
                 **Step 1 — Retrieve the Sperrung process:**\n\
                 Call `get_process` with the `malo_id`. Confirm `workflow_name` is\n\
                 `gpke-sperrung` (the NB-role workflow) and the process is active.\n\n\
                 **Step 2a — Execution succeeded:**\n\
                 - command: \"gpke.sperrung.bestaetigen\"\n\
                 - marktrolle: \"NB\"\n\
                 - payload: {\"malo_id\": \"<11-digit MaLo>\", \"note\": \"<optional>\"}\n\n\
                 **Step 2b — Execution failed:**\n\
                 - command: \"gpke.sperrung.fehlgeschlagen\"\n\
                 - marktrolle: \"NB\"\n\
                 - payload: {\"malo_id\": \"<11-digit MaLo>\", \"reason\": \"<why>\"}\n\n\
                 `reason` is **mandatory** on the failure path — without it the LF waits out\n\
                 its answer window with no explanation.\n\n\
                 Note: `processd` issues both commands automatically from its Sperrauftrag\n\
                 execute/fail endpoints. Call them by hand only when driving `makod` without it.\n\n\
                 The **LF side** is a different command set: `gpke.sperrung.beauftragen`\n\
                 (17115), `gpke.entsperrung.beauftragen` (17117), `gpke.sperrung.stornieren`\n\
                 (ORDCHG 39000).\n\n\
                 **Source:** GPKE Teil 2 § 3.5 (BK6-24-174 Anlage 1b); ORDERS PIDs 17115/17117, IFTSTA 21039.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for MakodMcpHandler {
    fn get_info(&self) -> ServerConfig {
        let configured = &self.state.commands.configured_marktrollen;

        let cmd_lines: String = COMMAND_REGISTRY
            .iter()
            .filter(|d| {
                !d.name.ends_with(".empfangen")
                    && d.permitted_roles
                        .iter()
                        .any(|r| configured.iter().any(|c| c == r.as_code()))
            })
            .map(|d| {
                let effective_roles: Vec<&str> = d
                    .permitted_roles
                    .iter()
                    .filter(|r| configured.iter().any(|c| c == r.as_code()))
                    .map(|r| r.as_code())
                    .collect();
                format!(
                    "  - `{}` (Marktrolle: {})",
                    d.name,
                    effective_roles.join("/")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let instructions = format!(
            "# makod v{ver} — MaKo/BDEW EDI@Energy process engine\n\
             \n\
             ## Instance\n\
             \n\
             Tenant: `{tenant}` | Marktrollen: {roles}\n\
             \n\
             ## Commands available on this instance\n\
             \n\
             {cmd_lines}\n\
             \n\
             Use `list_commands` for details (PID, multi-role flag, payload schema).\n\
             Use `submit_command` to execute. Always verify the MaLo is cached (`get_malo`) before\n\
             submitting GPKE or GeLi Gas commands.\n\
             \n\
             ## Regulatory deadlines\n\
             \n\
             The answer Frist runs per inbound PID (`mako_fristen::antwort`):\n\
             \n\
             {fristen}\n\
             \n\
             MaBiS has no Antwortfrist — the Clearingfenster of BK6-24-174 Anlage 3 \
             Kap. 3.10 Tabelle 2 bounds it. An INVOIC is answered against the \
             Zahlungsziel it carries (`SG8 DTM+265`).\n\
             \n\
             **Saturday is not a Werktag** (GPKE Teil 1 Kap. 2 and Kap. 7). A holiday observed in any single \
             Bundesland counts nationwide; 24.12. and 31.12. count as holidays. All deadlines in \
             gesetzlicher deutscher Zeit (CET/CEST).\n\
             \n\
             ## Error prefixes\n\
             \n\
             `malo_not_found` → seed via `PUT /admin/malo/{{id}}` | \
             `invalid_payload` → check required fields | \
             `duplicate_process` → active process exists | \
             `process_not_found` → initiate first | \
             `role_not_configured` → instance not started with that Marktrolle | \
             `engine_error` → internal error\n\
             \n\
             ## Prompts\n\
             \n\
             Use the `gpke-lieferbeginn`, `geli-lieferbeginn`, `wim-geraetewechsel`,\n\
             `wim-gas-anmeldung`, `gpke-sperrung`, and `msb-preisanfrage` prompts\n\
             for guided step-by-step workflows with pre-filled instructions.",
            ver = self.state.version,
            tenant = self.state.tenant,
            fristen = render_fristen(&[
                55_001, 55_004, 55_007, 55_010, 55_016, 44_001, 44_004, 44_013, 55_039, 55_042,
                55_051, 55_168, 44_039, 44_042, 44_051, 44_168,
            ]),
            roles = configured.join(", "),
        );

        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .enable_completions()
                .build(),
        )
        .with_server_info(Implementation::new("makod", &self.state.version))
        .with_instructions(instructions)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // Static resources are empty; all data is accessed via URI templates.
        Ok(ListResourcesResult {
            resources: vec![],
            next_cursor: None,
            ..Default::default()
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                ResourceTemplate::new("malo://{malo_id}", "MaLo record")
                    .with_description(
                        "Marktlokation master-data record identified by its 11-digit ID. \
                         Contains NB/MSB MP-IDs, MeLo IDs, and address data resolved from \
                         the BDEW API-Webdienste Strom.",
                    )
                    .with_mime_type("application/json"),
                ResourceTemplate::new("partner://{mp_id}", "Trading partner")
                    .with_description(
                        "Trading-partner record identified by its Marktpartner-ID. \
                         Contains the AS4 inbox URL, market roles, and communication \
                         channel configuration.",
                    )
                    .with_mime_type("application/json"),
            ],
            next_cursor: None,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.as_str();

        if let Some(malo_id) = uri.strip_prefix("malo://") {
            return match self.state.malo_cache.get(&self.state.tenant, malo_id).await {
                Ok(Some(record)) => {
                    let json =
                        serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_owned());
                    Ok(ReadResourceResult::new(vec![
                        ResourceContents::text(json, uri).with_mime_type("application/json"),
                    ])
                    .into())
                }
                Ok(None) => Err(McpError::resource_not_found(
                    format!("MaLo '{malo_id}' not in cache"),
                    None,
                )),
                Err(e) => Err(McpError::internal_error(
                    format!("Cache read failed: {e}"),
                    None,
                )),
            };
        }

        if let Some(mp_id_str) = uri.strip_prefix("partner://") {
            let mp_id = mako_engine::types::MarktpartnerCode::new(mp_id_str);
            return match self
                .state
                .partner_store
                .get(TenantId::from_party_id(&self.state.tenant), &mp_id)
                .await
            {
                Ok(Some(record)) => {
                    let json =
                        serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_owned());
                    Ok(ReadResourceResult::new(vec![
                        ResourceContents::text(json, uri).with_mime_type("application/json"),
                    ])
                    .into())
                }
                Ok(None) => Err(McpError::resource_not_found(
                    format!("Partner '{mp_id_str}' not registered"),
                    None,
                )),
                Err(e) => Err(McpError::internal_error(
                    format!("Partner store read failed: {e}"),
                    None,
                )),
            };
        }

        Err(McpError::resource_not_found(
            format!("Unknown resource URI: {uri}"),
            None,
        ))
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let partial = request.argument.value.to_lowercase();

        let values: Vec<String> = match &request.r#ref {
            // Prompt argument completions
            Reference::Prompt(p) => match (p.name.as_str(), request.argument.name.as_str()) {
                ("wim-geraetewechsel", "marktrolle") => {
                    vec!["NB".into(), "MSB".into()]
                }
                ("gpke-lieferbeginn" | "geli-lieferbeginn", "command") | (_, "command") => {
                    // Suggest available command names from the static registry
                    COMMAND_REGISTRY
                        .iter()
                        .map(|d| d.name.to_string())
                        .filter(|name| name.to_lowercase().contains(&partial))
                        .collect()
                }
                _ => vec![],
            },
            // Resource URI template variable completions
            Reference::Resource(_) => vec![],
            // Future Reference variants (non_exhaustive enum)
            _ => vec![],
        };

        let completion = CompletionInfo::with_pagination(values, None, false)
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CompleteResult::new(completion))
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

/// Axum middleware that enforces Bearer token authentication and Cedar
/// `UseMcp` authorization on the `/mcp` path.
///
/// Any request without a valid `Authorization: Bearer <token>` header is
/// rejected with `401 Unauthorized`. Authenticated principals without the
/// `UseMcp` permission are rejected with `403 Forbidden`.
async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<MakodMcpState>>,
    mut request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    use crate::cedar_authz::McpResource;

    let identity = match state.cedar.authenticate(request.headers()) {
        Some(id) => id,
        None => {
            return mako_service::ApiError::Unauthorized.into_response();
        }
    };

    if !state.cedar.authorize_mcp(
        &identity,
        &McpResource {
            tenant: &state.tenant,
        },
    ) {
        return mako_service::ApiError::forbidden("UseMcp permission denied").into_response();
    }

    // Forward the authenticated identity to the tool handlers: rmcp injects
    // the request's `http::request::Parts` (including these extensions) into
    // the tool context, so `submit_command` can run the per-command Cedar
    // check with the same principal the transport authenticated.
    request.extensions_mut().insert(identity);

    next.run(request).await
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build an axum [`Router`] that serves the MCP Streamable HTTP transport at
/// `/mcp`.
///
/// The router is protected by the Cedar `CedarAuthorizer` middleware — every
/// HTTP request (handshake + SSE stream) must carry a valid Bearer token.
///
/// Pass the same `shutdown` [`CancellationToken`] used by the rest of `makod`
/// so in-flight MCP sessions are cleaned up during graceful shutdown.
///
/// # How to mount
///
/// ```rust,ignore
/// let http_app = existing_router
///     .merge(mcp_server::router(mcp_state, shutdown_token.clone()));
/// ```
pub fn router(state: Arc<MakodMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        // No loopback restriction — `makod` runs behind a load-balancer or
        // Kubernetes Ingress. All requests still require a valid Bearer token.
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(MakodMcpHandler::new(state.clone()))
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

// ── Helpers ───────────────────────────────────────────────────────────────────
/// One answer obligation as a sentence, rendered from `mako_fristen::antwort`.
///
/// Every Frist the MCP surface states goes through here, so a sentence cannot
/// state a window the engine does not apply.
fn render_frist(o: &mako_fristen::antwort::AntwortObligation) -> String {
    format!(
        "{pid} {name}: the {who} answers {when} ({source})",
        pid = o.trigger_pid,
        name = o.name,
        who = o.answered_by,
        when = o.frist.beschreibung(),
        source = o.source,
    )
}

/// The rendered Fristen of `pids`, one per line; a PID the catalogue does not
/// know contributes nothing.
fn render_fristen(pids: &[u32]) -> String {
    pids.iter()
        .filter_map(|p| mako_fristen::antwort::antwort_obligation(*p))
        .map(render_frist)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What to expect after a command succeeds: the answer Frist of every PID the
/// command waits on, rendered from `mako_fristen::antwort` ([`render_frist`]).
/// A PID the catalogue knows no Frist for gets none.
fn next_steps_hint(command: &str) -> String {
    const WIM_STROM: &[u32] = &[55_039, 55_042, 55_051, 55_168];
    const WIM_BEIDE: &[u32] = &[
        55_039, 55_042, 55_051, 55_168, 44_039, 44_042, 44_051, 44_168,
    ];
    let (lead, pids): (&str, &[u32]) = match command {
        "gpke.lieferbeginn.anmelden" => ("Anmeldung sent.", &[55_001]),
        "gpke.lieferende.anmelden" => ("Abmeldung sent.", &[55_004]),
        "gpke.kuendigung.anmelden" => ("Kündigung sent to the LFA.", &[55_016]),
        "geli.lieferbeginn.anmelden" => ("Anmeldung sent to the GNB.", &[44_001]),
        "geli.lieferende.anmelden" => ("Abmeldung sent to the GNB.", &[44_004]),
        "geli.stornierung.initiieren" => {
            return "Stornierung (UTILMD 44022) sent. The GNB answers with UTILMD 44023 \
                    (Bestätigung) or 44024 (Ablehnung); no answer Frist is catalogued for \
                    44022."
                .to_owned();
        }
        "geli.datenabruf.anfragen" => {
            return "Datenabruf (ORDERS 17103) sent. No answer Frist is catalogued for 17103."
                .to_owned();
        }
        "geli.gas.sperrung.bestaetigen" | "gpke.sperrung.bestaetigen" => {
            return "Execution reported. IFTSTA 21039 is queued to the Lieferant \
                    (GPKE Teil 2 § 3.5)."
                .to_owned();
        }
        "gpke.sperrung.fehlgeschlagen" => {
            return "Non-execution reported with reason. IFTSTA 21039 is queued to the \
                    Lieferant so it does not wait out the answer window (GPKE Teil 2)."
                .to_owned();
        }
        "gpke.sperrung.beauftragen" => (
            "Sperrauftrag sent to the NB. After its answer the NB executes within the \
             window GPKE Teil 2 § 3.5.1.2 Nr. 1 sets after the frühestmöglichen \
             Sperrtermin and reports via IFTSTA 21039.",
            &[17_115],
        ),
        "gpke.entsperrung.beauftragen" => (
            "Entsperrauftrag sent to the NB, which reports the execution via IFTSTA 21039.",
            &[17_117],
        ),
        "gpke.sperrung.stornieren" => (
            "Stornierung (ORDCHG 39000) sent. Only valid until the NB has executed.",
            &[39_000],
        ),
        "wim.geraetewechsel.beauftragen" => (
            "MSB-Wechsel order sent. The window is the PID's:",
            WIM_STROM,
        ),
        "wim.geraetewechsel.bestaetigen" | "wim.geraetewechsel.ablehnen" => (
            "MSB-Wechsel answer dispatched. The window is the inbound PID's, not the \
             command's:",
            WIM_BEIDE,
        ),
        "mabis.abrechnung.einleiten" | "mabis.abrechnung.daten-einreichen" => {
            return "A Prüfmitteilung has no Frist (BK6-24-174 Anlage 3 Kap. 9.8.2 Nr. 1 — the \
                    receiving party „kann\" answer). It is bounded by the clearing window of \
                    Kap. 3.10 Tabelle 2: 11.–30. WT for a BG-SZR, 13.–30. WT for a BK-SZR."
                .to_owned();
        }
        _ => {
            return "Engine accepted the command. Monitor the ERP webhook for status updates."
                .to_owned();
        }
    };
    format!("{lead}\n{}", render_fristen(pids))
}

fn dispatch_error_to_string(e: crate::commands_api::DispatchError) -> String {
    use crate::commands_api::DispatchError;
    match e {
        DispatchError::MaloNotFound(id) => {
            format!(
                "malo_not_found: MaLo '{id}' is not in the cache. Seed it via PUT /admin/malo/{id}."
            )
        }
        DispatchError::InvalidPayload(msg) => {
            format!("invalid_payload: {msg}")
        }
        DispatchError::ProcessNotFound {
            business_key,
            workflow_name,
        } => {
            format!(
                "process_not_found: No active {workflow_name} process for '{business_key}'. \
                 Initiate via the corresponding anmelden command first."
            )
        }
        DispatchError::AmbiguousProcess {
            business_key,
            count,
        } => {
            format!(
                "ambiguous_process: {count} active processes for '{business_key}' — data integrity issue."
            )
        }
        DispatchError::DuplicateProcess {
            process_id,
            malo_id,
        } => {
            format!(
                "duplicate_process: An active process for MaLo '{malo_id}' (id: {process_id}) already exists."
            )
        }
        DispatchError::Engine(e) => {
            format!("engine_error: {e}")
        }
        DispatchError::NotImplemented(cmd) => {
            format!("not_implemented: Command '{cmd}' is registered but not yet dispatchable.")
        }
        DispatchError::EdiUnzulaessig(msg) => format!("edi_unzulaessig: {msg}"),
        DispatchError::MarktdUnavailable(_) => "marktd_unavailable: marktd, which holds the \
             recipient's EDI agreement, did not answer — retry."
            .to_owned(),
        DispatchError::InvoicNichtDarstellbar { message, .. } => {
            format!("invoic_nicht_darstellbar: {message}")
        }
        DispatchError::DuplicateInvoice {
            process_id,
            invoice_ref,
        } => format!(
            "duplicate_invoice: Invoice '{invoice_ref}' was already issued by process {process_id}."
        ),
    }
}

#[cfg(test)]
mod tool_inventory_tests {
    use super::MakodMcpHandler;

    /// Every MCP tool must evaluate a per-resource Cedar action, or be listed
    /// here with the reason it needs none.
    ///
    /// `UseMcp` authorizes the transport, not the data. Four tools once relied
    /// on it alone — `get_malo`, `list_partners`, `get_partner` and
    /// `list_dead_letters` — so a principal holding only `UseMcp` could read
    /// the MaLo cache, the whole partner directory and the § 147 AO
    /// dead-letter records that the equivalent REST routes refused it. The
    /// shipped least-privilege policy grants `mcp-agent` `AdminMaloRead` and
    /// withholds `AdminPartnerRead`, which is exactly the distinction the code
    /// was not making.
    #[test]
    fn every_tool_evaluates_a_cedar_action() {
        const SOURCE: &str = include_str!("mcp_server.rs");

        /// Tools that expose no tenant data and are therefore covered by the
        /// `UseMcp` transport grant alone.
        const TRANSPORT_ONLY: &[(&str, &str)] = &[
            (
                "list_commands",
                "static command catalogue for the configured Marktrollen",
            ),
            (
                "get_health",
                "daemon liveness; GET /health is public on every port",
            ),
            (
                "get_format_version_coverage",
                "the published BDEW release calendar",
            ),
            (
                "list_active_processes",
                "a bare count; names no process, MaLo or workflow",
            ),
            (
                "get_outbox_status",
                "aggregate queue depth and age; names no message",
            ),
        ];

        let exempt: std::collections::HashMap<&str, &str> =
            TRANSPORT_ONLY.iter().copied().collect();

        // Each tool body runs from its `async fn` to the next one; scan that
        // span for an authorization call.
        let bodies: Vec<(&str, &str)> = SOURCE
            .match_indices("    async fn ")
            .filter_map(|(start, _)| {
                let rest = &SOURCE[start + "    async fn ".len()..];
                let name = rest.split('(').next()?.trim();
                let end = rest.find("\n    /// ").unwrap_or(rest.len());
                Some((name, &rest[..end]))
            })
            .collect();

        let tools: std::collections::HashSet<String> = MakodMcpHandler::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();

        let mut ungated: Vec<&str> = Vec::new();
        for (name, body) in &bodies {
            if !tools.contains(*name) || exempt.contains_key(name) {
                continue;
            }
            let gated = body.contains("authorize_")
                || body.contains("require_malo_read")
                || body.contains("require_partner_read")
                || body.contains("require_any_process_read");
            if !gated {
                ungated.push(name);
            }
        }
        assert!(
            ungated.is_empty(),
            "these MCP tools evaluate no per-resource Cedar action: {ungated:?}\n\
             Gate each on the action its REST twin enforces, or add it to \
             TRANSPORT_ONLY with the reason it exposes no tenant data."
        );

        // Keep the exemption list honest: an entry for a tool that no longer
        // exists hides a real gap behind a stale name.
        let stale: Vec<&str> = exempt
            .keys()
            .filter(|n| !tools.contains(**n))
            .copied()
            .collect();
        assert!(
            stale.is_empty(),
            "TRANSPORT_ONLY names unknown tools: {stale:?}"
        );
    }

    /// The published tool list is a documented contract, and the docs state a
    /// count. Pin both here: a tool added without updating
    /// `site/content/docs/services/makod.md` fails this test rather than
    /// shipping a page that undercounts what the server exposes.
    #[test]
    fn the_tool_inventory_matches_the_published_list() {
        let mut names: Vec<String> = MakodMcpHandler::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();

        let mut expected = vec![
            "get_format_version_coverage",
            "get_health",
            "get_malo",
            "get_outbox_status",
            "get_partner",
            "get_process",
            "get_process_history",
            "list_active_processes",
            "list_commands",
            "list_dead_letters",
            "list_overdue_deadlines",
            "list_partners",
            "submit_command",
        ];
        expected.sort_unstable();

        assert_eq!(
            names, expected,
            "MCP tool inventory changed — update the tool table and the count in \
             site/content/docs/services/makod.md to match"
        );
    }

    /// `get_process` declares what it returns, and what it returns is that
    /// shape: the schema's properties are exactly the keys of a serialized
    /// `ProcessesView`, so the Desk's generated type and the wire cannot differ.
    #[test]
    fn get_process_declares_its_output_schema() {
        let tool = MakodMcpHandler::tool_router()
            .list_all()
            .into_iter()
            .find(|t| t.name == "get_process")
            .expect("get_process is served");
        let schema = tool.output_schema.expect("get_process has an outputSchema");
        let mut declared: Vec<&String> = schema["properties"]
            .as_object()
            .expect("an object schema")
            .keys()
            .collect();
        declared.sort();

        let sample = serde_json::to_value(super::ProcessesView {
            business_key: "51238696012".into(),
            processes: vec![],
        })
        .expect("serializes");
        let mut sent: Vec<&String> = sample.as_object().expect("object").keys().collect();
        sent.sort();
        assert_eq!(declared, sent);
    }

    /// A process spawned by an inbound message is indexed under its MaLo in
    /// the correlation index and has no registry entry; `get_process` must
    /// find it there. Reading the registry alone answered `process_not_found`
    /// for every process ingest started.
    #[tokio::test]
    async fn a_process_is_found_by_the_business_key_it_was_indexed_under() {
        use mako_engine::ids::{ProcessId, ProcessIdentity, TenantId};
        use mako_engine::registry::{ProcessRegistry as _, RegistryKey};
        use mako_engine::version::WorkflowId;

        let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("store");
        let registry = store.as_process_registry();
        let tenant = TenantId::from_party_id("9900357000004");
        let process_id = ProcessId::new();
        let identity = ProcessIdentity::new(
            process_id,
            tenant,
            WorkflowId::new("gpke-supplier-change", "FV2026-04-01"),
        );
        registry
            .register_correlated(tenant, "51238696012", process_id, identity.clone())
            .await
            .expect("index");

        let by_malo = super::processes_named(
            &registry,
            tenant,
            &RegistryKey::parse("51238696012").expect("key"),
        )
        .await
        .expect("lookup");
        assert_eq!(by_malo, vec![identity]);

        let unknown = super::processes_named(
            &registry,
            tenant,
            &RegistryKey::parse("51238696782").expect("key"),
        )
        .await
        .expect("lookup");
        assert!(unknown.is_empty());
    }

    /// The history of a process stored in makod's own store marks the
    /// market messages its workflow stated — a GPKE Anmeldung in, the answer
    /// out — and leaves a state change unmarked.
    #[tokio::test]
    async fn a_process_history_marks_the_messages_its_workflow_stated() {
        use mako_engine::event_store::{
            AppendBatch, AtomicAppend as _, EventStore as _, ExpectedVersion,
        };
        use mako_engine::ids::{ProcessId, StreamId, TenantId};
        use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
        use mako_engine::version::WorkflowId;
        use mako_engine::workflow::CommandContext;
        use mako_gpke::wechselprozesse::SupplierChangeEvent;

        let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("store");
        let tenant = TenantId::from_party_id("9900357000004");
        let process = ProcessId::new();
        let ctx = CommandContext::new(
            tenant,
            process,
            WorkflowId::new("gpke-supplier-change", "FV2026-04-01"),
        );
        let initiated = SupplierChangeEvent::Initiated {
            location_id: MaLo::new("51238696012"),
            new_supplier: MarktpartnerCode::new("9900000000001"),
            grid_operator: MarktpartnerCode::new("9900357000004"),
            document_date: "20261104".to_owned(),
            process_date: "20261201".to_owned(),
            transaktionsgrund: None,
            transaktionsgrund_ergaenzung: None,
            message_ref: MessageRef::new("UTILMD-1"),
            vorgangsnummer: String::new(),
            produktpaket_id: None,
            pruefidentifikator: Pruefidentifikator::const_new(55_001),
        };
        let answered = SupplierChangeEvent::AntwortGesendet {
            response_pid: Some(Pruefidentifikator::const_new(55_002)),
            accepted: true,
            reason: None,
        };
        let stream = StreamId::for_process(tenant, &process);
        let events = [
            ctx.new_event(&initiated).expect("event"),
            ctx.new_event(&answered).expect("event"),
            ctx.new_event(&SupplierChangeEvent::Activated)
                .expect("event"),
        ];
        store
            .append(AppendBatch::new(&stream, ExpectedVersion::Any, &events))
            .await
            .expect("append");

        let h = super::history(
            &process.to_string(),
            &store.load(&stream).await.expect("load"),
        );
        assert_eq!(h.workflow_name, "gpke-supplier-change");
        let marks: Vec<(Option<&str>, Option<u32>)> = h
            .eintraege
            .iter()
            .map(|e| (e.richtung.as_deref(), e.pid))
            .collect();
        assert_eq!(
            marks,
            vec![
                (Some("eingang"), Some(55_001)),
                (Some("ausgang"), Some(55_002)),
                (None, None)
            ]
        );
        assert_eq!(h.eintraege[0].message_ref.as_deref(), Some("UTILMD-1"));
    }

    /// An answer's entry names its code, the list that defines it and the
    /// BDEW's wording from that list — resolved through an `S_` Codeliste as
    /// well as an `E_` tree — and carries no wording for a code the list does
    /// not publish.
    #[test]
    fn an_answer_entry_reads_its_code_in_the_list_that_defines_it() {
        use mako_engine::envelope::{Antwortcode, EventEnvelope, NewEvent};
        use mako_engine::ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId};
        use mako_engine::version::WorkflowId;

        let tenant = TenantId::new();
        let process = ProcessId::new();
        let (ebd, _, ablehnung) = mako_pruefung::codes::WIRE_CODELISTEN[0];
        let wim =
            mako_pruefung::codes::codes_im_cluster(ebd, mako_pruefung::codes::Cluster::Ablehnung)
                .next()
                .expect("the tree publishes an Ablehnung");
        let envelope = |antwort: Antwortcode| {
            EventEnvelope::from_new(
                NewEvent::new(
                    CorrelationId::new(),
                    None,
                    ConversationId::new(),
                    process,
                    tenant,
                    WorkflowId::new("gpke-lf-anmeldung", "FV2026-04-01"),
                    "LfAnmeldungAntwortReceived",
                    1,
                    serde_json::Value::Null,
                )
                .with_antwort(antwort),
                StreamId::for_process(tenant, &process),
                1,
                time::OffsetDateTime::UNIX_EPOCH,
            )
        };
        let h = super::history(
            &process.to_string(),
            &[
                envelope(Antwortcode::new("A02", Some("E_0622"))),
                envelope(Antwortcode::new(wim.code, Some(ablehnung))),
                envelope(Antwortcode::new("Z99", Some("E_0622"))),
            ],
        );
        let a02 = &h.eintraege[0];
        assert_eq!(
            (a02.antwortcode.as_deref(), a02.codeliste.as_deref()),
            (Some("A02"), Some("E_0622"))
        );
        assert_eq!(
            a02.bedeutung.as_deref(),
            mako_pruefung::codes::lookup("E_0622", "A02").map(|c| c.bedeutung)
        );
        assert!(a02.bedeutung.is_some(), "E_0622 publishes A02");
        assert_eq!(h.eintraege[1].bedeutung.as_deref(), Some(wim.bedeutung));
        assert_eq!(h.eintraege[2].antwortcode.as_deref(), Some("Z99"));
        assert_eq!(
            h.eintraege[2].bedeutung, None,
            "never another tree's wording"
        );
    }

    /// A deadline's kind comes from the labels `mako_fristen` owns, never from
    /// a workflow's name for its own Frist.
    #[test]
    fn a_deadline_is_a_delivery_window_only_where_mako_fristen_says_so() {
        assert_eq!(
            super::deadline_art(mako_fristen::APERAK_STROM_WINDOW_LABEL),
            "aperak"
        );
        assert_eq!(
            super::deadline_art(mako_fristen::APERAK_GAS_INITIALPROZESS_LABEL),
            "aperak"
        );
        assert_eq!(
            super::deadline_art(mako_fristen::CONTRL_FRIST_LABEL),
            "contrl"
        );
        assert_eq!(super::deadline_art("gpke-antwortfrist"), "prozess");
        assert_eq!(
            super::deadline_art("esa-wertebestellung-bindungsfrist"),
            "prozess"
        );
    }

    /// The served tool contract is the committed `mcp.snapshot.json`
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it).
    #[test]
    fn the_tool_contract_matches_the_snapshot() {
        let tools: Vec<serde_json::Value> = MakodMcpHandler::tool_router()
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

    /// `gesamt` counts every dead letter, not the page: with 23 recorded the
    /// page holds 20, the most recent first, and the total says 23.
    #[test]
    fn the_dead_letter_total_is_not_the_page() {
        let all: Vec<_> = (0..23)
            .rev()
            .map(|i| mako_engine::store_slatedb::DeadLetterRecord {
                rejected_at: format!("2026-11-07T10:{i:02}:00Z"),
                reason_label: "permanent".to_owned(),
                reason_detail: format!("record {i}"),
            })
            .collect();
        let view = super::DeadLetters::of(all);
        assert_eq!(view.gesamt, 23);
        assert_eq!(view.dead_letters.len(), super::DEAD_LETTER_PAGE);
        assert_eq!(view.dead_letters[0].reason_detail, "record 22");
    }

    /// The tool answers the RFC 3339 instant the engine stores, and nothing
    /// for a string that is no RFC 3339 instant.
    #[test]
    fn a_dead_letter_instant_is_rfc3339() {
        assert_eq!(
            super::rfc3339("2026-11-07T09:15:30.25Z".to_owned()).as_deref(),
            Some("2026-11-07T09:15:30.25Z")
        );
        assert_eq!(
            super::rfc3339("2026-11-07 9:15:30.25 +00:00:00".to_owned()),
            None
        );
        assert_eq!(super::rfc3339("gestern".to_owned()), None);
    }
}

#[cfg(test)]
mod next_steps_tests {
    use super::{next_steps_hint, render_frist};
    use mako_fristen::antwort::antwort_obligation;

    /// The PIDs whose answer each command waits for.
    const COVERED: &[(&str, &[u32])] = &[
        ("gpke.lieferbeginn.anmelden", &[55_001]),
        ("gpke.lieferende.anmelden", &[55_004]),
        ("gpke.kuendigung.anmelden", &[55_016]),
        ("geli.lieferbeginn.anmelden", &[44_001]),
        ("geli.lieferende.anmelden", &[44_004]),
        ("gpke.sperrung.beauftragen", &[17_115]),
        ("gpke.entsperrung.beauftragen", &[17_117]),
        ("gpke.sperrung.stornieren", &[39_000]),
        (
            "wim.geraetewechsel.beauftragen",
            &[55_039, 55_042, 55_051, 55_168],
        ),
        (
            "wim.geraetewechsel.bestaetigen",
            &[
                55_039, 55_042, 55_051, 55_168, 44_039, 44_042, 44_051, 44_168,
            ],
        ),
        (
            "wim.geraetewechsel.ablehnen",
            &[
                55_039, 55_042, 55_051, 55_168, 44_039, 44_042, 44_051, 44_168,
            ],
        ),
    ];

    /// Every Frist a hint states is the catalogue's, rendered.
    #[test]
    fn every_hint_states_the_catalogued_frist_of_each_pid_it_covers() {
        for (command, pids) in COVERED {
            let hint = next_steps_hint(command);
            for pid in *pids {
                let o = antwort_obligation(*pid)
                    .unwrap_or_else(|| panic!("{pid} has a catalogued obligation"));
                assert!(
                    hint.contains(&render_frist(o)),
                    "{command}: the hint must state {pid}'s Frist as the catalogue does\n{hint}"
                );
            }
        }
    }

    /// A command whose PID the catalogue knows no Frist for states none.
    #[test]
    fn an_uncatalogued_pid_gets_no_frist() {
        for (command, pid) in [
            ("geli.stornierung.initiieren", 44_022),
            ("geli.datenabruf.anfragen", 17_103),
        ] {
            assert!(antwort_obligation(pid).is_none(), "{pid}");
            let hint = next_steps_hint(command);
            assert!(!hint.contains("Werktag"), "{command}: {hint}");
            assert!(
                !hint.contains("APERAK"),
                "{command}: the answers are UTILMD/ORDRSP: {hint}"
            );
        }
    }
}
