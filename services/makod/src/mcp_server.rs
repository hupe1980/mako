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
//! | `get_partner` | Get a specific trading partner by GLN |
//! | `get_health` | Query daemon health and uptime |
//!
//! ## Resources
//!
//! | URI template | Description |
//! |---|---|
//! | `malo://{malo_id}` | Marktlokation master-data record |
//! | `partner://{gln}` | Trading-partner record |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `gpke-lieferbeginn` | Step-by-step guide: GPKE Lieferbeginn (electricity supplier change) |
//! | `geli-lieferbeginn` | Step-by-step guide: GeLi Gas Lieferbeginn (gas supplier change) |
//! | `wim-geraetewechsel` | Step-by-step guide: WiM Gerätewechsel (meter device change) |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_engine::{ids::TenantId, partner::PartnerStore as _, store_slatedb::SlateDbPartnerStore};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::cedar_authz::CedarAuthorizer;
use crate::commands_api::{
    COMMAND_REGISTRY, CommandsApiState, DispatchOutcome, dispatch_command, validate_command,
};
use crate::malo_cache::SlateDbMaloCache;

// ── Shared state ──────────────────────────────────────────────────────────────

/// Shared state injected into each MCP handler instance.
pub struct MakodMcpState {
    /// Operator tenant ID — all cache and store operations are scoped to this.
    pub tenant_id: TenantId,
    /// Operator tenant ID as a plain string (used in log messages).
    pub tenant_id_str: String,
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
    /// | `wim.geraetewechsel.beauftragen` | `melo_id` (11-digit MeLo), `wechseldatum` |
    /// | `mabis.abrechnung.einleiten` | `bilanzierungsgebiet`, `abrechnungszeitraum_von`, `abrechnungszeitraum_bis` |
    #[schemars(example = "{\"malo_id\": \"10001234567\", \"lieferbeginn_datum\": \"2026-10-01\"}")]
    pub payload: serde_json::Value,

    /// Optional stable UUID for idempotency.
    ///
    /// Provide the same key on retries to prevent double-execution on
    /// transient network errors. A random UUID is generated when omitted.
    pub idempotency_key: Option<String>,
}

/// Parameters for `get_malo`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMaloParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(description = "11-digit Marktlokations-ID, e.g. \"10001234567\"")]
    pub malo_id: String,
}

/// Parameters for `get_partner`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPartnerParams {
    /// 13-digit GLN of the trading partner.
    #[schemars(description = "13-digit Global Location Number of the trading partner")]
    pub gln: String,
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

// ── MCP handler ───────────────────────────────────────────────────────────────

/// MCP server handler — one instance per MCP session (clone-per-request).
#[derive(Clone)]
pub struct MakodMcpHandler {
    state: Arc<MakodMcpState>,
    #[allow(dead_code)] // used by the #[tool_router] macro-generated dispatch code
    tool_router: ToolRouter<MakodMcpHandler>,
}

#[tool_router]
impl MakodMcpHandler {
    fn new(state: Arc<MakodMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
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
        description = "List all MaKo commands available for this instance (filtered to configured Marktrollen). Call this before submit_command to discover what you can submit."
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
                        .any(|r| configured.contains(&r.to_string()))
            })
            .map(|d| {
                let effective_roles: Vec<&str> = d
                    .permitted_roles
                    .iter()
                    .copied()
                    .filter(|r| configured.contains(&r.to_string()))
                    .collect();
                serde_json::json!({
                    "command": d.name,
                    "marktrolle": if effective_roles.len() == 1 {
                        serde_json::json!(effective_roles[0])
                    } else {
                        serde_json::json!(effective_roles)
                    },
                    "pid": if d.primary_pid == 0 { serde_json::Value::Null } else { serde_json::json!(d.primary_pid) },
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
    /// resolves trading-partner GLNs from the MaLo cache and generates the
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
        description = "Submit a MaKo process command (GPKE, GeLi Gas, WiM, MABIS). Use list_commands first to see what's available and what payload fields are required."
    )]
    #[instrument(skip(self), fields(command = %p.command))]
    async fn submit_command(
        &self,
        Parameters(p): Parameters<SubmitCommandParams>,
    ) -> Result<CallToolResult, McpError> {
        let cmd_lower = p.command.to_lowercase();
        let asserted = p.marktrolle.as_deref().map(str::to_uppercase);

        let effective_role = validate_command(
            &cmd_lower,
            asserted.as_deref(),
            &self.state.commands.configured_marktrollen,
        )
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let idempotency_key = p
            .idempotency_key
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        tracing::info!(
            command = %cmd_lower,
            marktrolle = %effective_role,
            idempotency_key = %idempotency_key,
            "MCP: submit_command",
        );

        match dispatch_command(&self.state.commands, &cmd_lower, &p.payload).await {
            Ok(outcome) => {
                let (process_id, status) = match outcome {
                    DispatchOutcome::Spawned { process_id } => (process_id.to_string(), "spawned"),
                    DispatchOutcome::Dispatched { process_id } => {
                        (process_id.to_string(), "dispatched")
                    }
                };
                ContentBlock::json(serde_json::json!({
                    "idempotency_key": idempotency_key,
                    "command": cmd_lower,
                    "marktrolle": effective_role,
                    "process_id": process_id,
                    "status": status,
                    "next_steps": next_steps_hint(&cmd_lower),
                }))
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
    #[tool(description = "Read a cached Marktlokation (MaLo) record by its 11-digit ID")]
    #[instrument(skip(self), fields(malo_id = %p.malo_id))]
    async fn get_malo(
        &self,
        Parameters(p): Parameters<GetMaloParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .state
            .malo_cache
            .get(&self.state.tenant_id_str, &p.malo_id)
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
    /// Returns a paginated array of partner records, each containing the GLN,
    /// AS4 endpoint URL, market roles, and communication channels. An empty
    /// array is returned when no partners have been registered yet.
    ///
    /// Use `limit` and `cursor` to page through large directories.
    #[tool(description = "List registered trading partners for this tenant (paginated, max 500)")]
    #[instrument(skip(self), fields(limit = ?p.limit, cursor = ?p.cursor))]
    async fn list_partners(
        &self,
        Parameters(p): Parameters<ListPartnersParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(100).clamp(1, 500);

        match self.state.partner_store.list(self.state.tenant_id).await {
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

    /// Get a specific trading partner by GLN.
    ///
    /// Returns the full partner record including the AS4 inbox URL, market
    /// roles, and communication channel configuration. Returns an error if
    /// the partner is not registered.
    #[tool(description = "Get a trading partner record by 13-digit GLN")]
    #[instrument(skip(self), fields(gln = %p.gln))]
    async fn get_partner(
        &self,
        Parameters(p): Parameters<GetPartnerParams>,
    ) -> Result<CallToolResult, McpError> {
        let gln = mako_engine::types::MarktpartnerCode::new(p.gln.as_str());

        match self
            .state
            .partner_store
            .get(self.state.tenant_id, &gln)
            .await
        {
            Ok(Some(record)) => ContentBlock::json(&record)
                .map(|block| CallToolResult::success(vec![block]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "partner_not_found: Partner '{}' is not registered. \
                 Register it via PUT /admin/partners/{}.",
                p.gln, p.gln
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
    #[tool(description = "Get makod health status, version, and MaLo cache statistics")]
    #[instrument(skip(self))]
    async fn get_health(&self) -> Result<CallToolResult, McpError> {
        let cache_stats = self
            .state
            .malo_cache
            .stats(&self.state.tenant_id_str)
            .await
            .ok();

        ContentBlock::json(serde_json::json!({
            "status": "ok",
            "version": self.state.version,
            "tenant_id": self.state.tenant_id_str,
            "configured_marktrollen": self.state.commands.configured_marktrollen,
            "malo_cache": cache_stats.map(|s| serde_json::json!({
                "count": s.count,
                "last_upsert": s.last_upsert,
            })),
        }))
        .map(|block| CallToolResult::success(vec![block]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[tool_handler]
impl ServerHandler for MakodMcpHandler {
    fn get_info(&self) -> ServerInfo {
        let configured = &self.state.commands.configured_marktrollen;

        let cmd_lines: String = COMMAND_REGISTRY
            .iter()
            .filter(|d| {
                !d.name.ends_with(".empfangen")
                    && d.permitted_roles
                        .iter()
                        .any(|r| configured.contains(&r.to_string()))
            })
            .map(|d| {
                let effective_roles: Vec<&str> = d
                    .permitted_roles
                    .iter()
                    .copied()
                    .filter(|r| configured.contains(&r.to_string()))
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
             | Process | Deadline | Source |\n\
             |---|---|---|\n\
             | GPKE | 24 wall-clock hours | BK6-22-024 |\n\
             | WiM | 5 Werktage | BK6-24-174 |\n\
             | GeLi Gas | 10 Werktage | BK7-24-01-009 |\n\
             | MABIS | 1 Werktag | BK6-24-174 § 13.8 |\n\
             \n\
             Werktag = Mon–Sat, excl. public holidays. All deadlines in German local time (CET/CEST).\n\
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
             Use the `gpke-lieferbeginn`, `geli-lieferbeginn`, and `wim-geraetewechsel` prompts\n\
             for guided step-by-step workflows with pre-filled instructions.",
            ver = self.state.version,
            tenant = self.state.tenant_id_str,
            roles = configured.join(", "),
        );

        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
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
            meta: None,
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
                         Contains NB/MSB GLNs, MeLo IDs, and address data resolved from \
                         the BDEW API-Webdienste Strom.",
                    )
                    .with_mime_type("application/json"),
                ResourceTemplate::new("partner://{gln}", "Trading partner")
                    .with_description(
                        "Trading-partner record identified by its 13-digit GLN. \
                         Contains the AS4 inbox URL, market roles, and communication \
                         channel configuration.",
                    )
                    .with_mime_type("application/json"),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();

        if let Some(malo_id) = uri.strip_prefix("malo://") {
            return match self
                .state
                .malo_cache
                .get(&self.state.tenant_id_str, malo_id)
                .await
            {
                Ok(Some(record)) => {
                    let json =
                        serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_owned());
                    Ok(ReadResourceResult::new(vec![
                        ResourceContents::text(json, uri).with_mime_type("application/json"),
                    ]))
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

        if let Some(gln_str) = uri.strip_prefix("partner://") {
            let gln = mako_engine::types::MarktpartnerCode::new(gln_str);
            return match self
                .state
                .partner_store
                .get(self.state.tenant_id, &gln)
                .await
            {
                Ok(Some(record)) => {
                    let json =
                        serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_owned());
                    Ok(ReadResourceResult::new(vec![
                        ResourceContents::text(json, uri).with_mime_type("application/json"),
                    ]))
                }
                Ok(None) => Err(McpError::resource_not_found(
                    format!("Partner '{gln_str}' not registered"),
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

    // ── Prompts ─────────────────────────────────────────────────────────────────────────────

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            prompts: vec![
                Prompt::new(
                    "gpke-lieferbeginn",
                    Some("Guided workflow: GPKE Lieferbeginn Strom (electricity supplier change)"),
                    Some(vec![
                        PromptArgument::new("malo_id")
                            .with_description("11-digit Marktlokations-ID")
                            .with_required(true),
                        PromptArgument::new("lieferbeginn_datum")
                            .with_description("Supply start date (YYYY-MM-DD)")
                            .with_required(true),
                    ]),
                ),
                Prompt::new(
                    "geli-lieferbeginn",
                    Some("Guided workflow: GeLi Gas Lieferbeginn (gas supplier change)"),
                    Some(vec![
                        PromptArgument::new("malo_id")
                            .with_description("11-digit gas Marktlokations-ID")
                            .with_required(true),
                        PromptArgument::new("lieferbeginn_datum")
                            .with_description("Supply start date (YYYY-MM-DD)")
                            .with_required(true),
                    ]),
                ),
                Prompt::new(
                    "wim-geraetewechsel",
                    Some("Guided workflow: WiM Gerätewechsel (meter device change)"),
                    Some(vec![
                        PromptArgument::new("melo_id")
                            .with_description("11-digit Messlokations-ID")
                            .with_required(true),
                        PromptArgument::new("wechseldatum")
                            .with_description("Meter change date (YYYY-MM-DD)")
                            .with_required(true),
                        PromptArgument::new("marktrolle")
                            .with_description("NB or MSB")
                            .with_required(true),
                    ]),
                ),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let args = request.arguments.unwrap_or_default();
        let arg = |key: &str| {
            args.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("<not provided>")
                .to_owned()
        };

        match request.name.as_str() {
            "gpke-lieferbeginn" => {
                let malo_id = arg("malo_id");
                let date = arg("lieferbeginn_datum");
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    format!(
                        "I need to initiate a GPKE Lieferbeginn (electricity supplier change) \
                         for MaLo {malo_id} starting {date}.\n\
                         \n\
                         Please:\n\
                         1. Call `get_malo` with malo_id=\"{malo_id}\" to verify the MaLo is \
                            cached and show me the NB/MSB GLNs.\n\
                         2. If found, call `submit_command` with:\n\
                            - command: \"gpke.lieferbeginn.anmelden\"\n\
                            - payload: {{\"malo_id\": \"{malo_id}\", \"lieferbeginn_datum\": \"{date}\"}}\n\
                         3. Report the process_id and status.\n\
                         4. Explain next steps: the NB has 24 wall-clock hours to respond \
                            with a Bestätigung (PID 55003/55004) per BK6-22-024."
                    ),
                )]))
            }
            "geli-lieferbeginn" => {
                let malo_id = arg("malo_id");
                let date = arg("lieferbeginn_datum");
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    format!(
                        "I need to initiate a GeLi Gas Lieferbeginn (gas supplier change) \
                         for gas MaLo {malo_id} starting {date}.\n\
                         \n\
                         Please:\n\
                         1. Call `get_malo` with malo_id=\"{malo_id}\" to verify the MaLo \
                            is cached and show me the GNB GLN.\n\
                         2. If found, call `submit_command` with:\n\
                            - command: \"geli.lieferbeginn.anmelden\"\n\
                            - payload: {{\"malo_id\": \"{malo_id}\", \"lieferbeginn_datum\": \"{date}\"}}\n\
                         3. Report the process_id and status.\n\
                         4. Explain next steps: the GNB has 10 Werktage to respond per \
                            BK7-24-01-009 (Saturday counts as Werktag; \
                            public holidays do not; German local time)."
                    ),
                )]))
            }
            "wim-geraetewechsel" => {
                let melo_id = arg("melo_id");
                let date = arg("wechseldatum");
                let marktrolle = arg("marktrolle");
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    format!(
                        "I need to initiate a WiM Gerätewechsel (meter device change) \
                         for MeLo {melo_id} on {date}, acting as {marktrolle}.\n\
                         \n\
                         Please:\n\
                         1. Call `submit_command` with:\n\
                            - command: \"wim.geraetewechsel.beauftragen\"\n\
                            - marktrolle: \"{marktrolle}\"\n\
                            - payload: {{\"melo_id\": \"{melo_id}\", \"wechseldatum\": \"{date}\"}}\n\
                         2. Report the process_id and status.\n\
                         3. Explain next steps: the MSB has 5 Werktage to confirm or reject \
                            per BK6-24-174."
                    ),
                )]))
            }
            name => Err(McpError::resource_not_found(
                format!("Unknown prompt: {name}"),
                None,
            )),
        }
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
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    use crate::cedar_authz::McpResource;

    let identity = match state.cedar.authenticate(request.headers()) {
        Some(id) => id,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Authorization: Bearer <token> required for /mcp",
            )
                .into_response();
        }
    };

    if !state.cedar.authorize_mcp(
        &identity,
        &McpResource {
            tenant: &state.tenant_id_str,
        },
    ) {
        return (
            StatusCode::FORBIDDEN,
            "403 Forbidden: UseMcp permission denied",
        )
            .into_response();
    }

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
/// Returns a short human-readable hint about what to expect after a command
/// succeeds, based on the applicable BDEW regulatory deadline.
fn next_steps_hint(command: &str) -> &'static str {
    match command {
        "gpke.lieferbeginn.anmelden" | "gpke.lieferende.anmelden" | "gpke.kuendigung.anmelden" => {
            "NB has 24 wall-clock hours to respond with a Bestätigung/Ablehnung (BK6-22-024)."
        }
        "geli.lieferbeginn.anmelden" | "geli.lieferende.anmelden" => {
            "GNB has 10 Werktage to respond with a Bestätigung/Ablehnung (BK7-24-01-009)."
        }
        "wim.geraetewechsel.beauftragen" => {
            "MSB has 5 Werktage to respond with a Bestätigung/Ablehnung (BK6-24-174)."
        }
        "mabis.abrechnung.einleiten" | "mabis.abrechnung.daten-einreichen" => {
            "ÜNB has 1 Werktag to issue a Prüfmitteilung IFTSTA (BK6-24-174 § 13.8)."
        }
        _ => "Engine accepted the command. Monitor the ERP webhook for status updates.",
    }
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
    }
}
