//! MCP (Model Context Protocol) server for `marktd`.
//!
//! Exposes master data reads to LLM tooling via the MCP Streamable HTTP
//! transport (spec 2025-11-25).  Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Authentication
//!
//! Every request requires `Authorization: Bearer <token>` (OIDC JWT).
//! The principal is checked against Cedar policy action `use-mcp`.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `get_malo`                        | Read a MaLo record by 11-digit ID |
//! | `list_malo`                       | List MaLos with optional filters |
//! | `get_melo`                        | Read a MeLo record |
//! | `get_melo_standorteigenschaften`  | Read BO4E Standorteigenschaften for a MeLo |
//! | `list_partners`                   | List registered trading partners |
//! | `get_partner`                     | Read a single partner by MP-ID |
//! | `get_preisblatt`                  | Read the current PreisblattNetznutzung for an NB |
//! | `get_versorgungsstatus`           | Read the current VersorgungsStatus for a MaLo |
//! | `get_versorgungsstatus_history`   | Read full supply-state change history |
//! | `get_versorgung_at`               | Point-in-time VersorgungsStatus query |
//! | `get_lokationszuordnung`          | Read active role assignments for a MaLo |
//! | `get_nb_contract`                 | Read the NB contract for a MaLo |
//! | `get_correlation`                 | Correlate process ID or ERP order ref |
//! | `list_pricat_versions`            | List available PRICAT versions for an NB |
//! | `dispatch_pricat`                 | Trigger PRICAT dispatch to LF |
//! | `get_nb_energiemix`               | Read §42 EnWG Energiemix for an NB |
//! | `get_technische_ressource`        | Read a TechnischeRessource (TR) record |
//! | `get_steuerbare_ressource`        | Read a SteuerbareRessource (SR) + §14a config |

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

// ── Shared state ──────────────────────────────────────────────────────────────

/// State injected into each MCP session.
#[derive(Clone)]
pub struct MdmdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMaloParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(example = "\"51238696781\"")]
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPreisblattParams {
    /// 13-digit GLN of the Netzbetreiber.
    #[schemars(example = "\"9904234560001\"")]
    pub nb_mp_id: String,
    /// Billing date in ISO 8601 format (YYYY-MM-DD).  Defaults to today.
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVersorgungsstatusParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(example = "\"51238696781\"")]
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPartnersParams {
    /// Maximum number of results (1–500, default 100).
    pub limit: Option<u32>,
    /// Pagination cursor (opaque string from previous response).
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetNbContractParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(example = "\"51238696781\"")]
    pub malo_id: String,
    /// Reference date (ISO 8601, YYYY-MM-DD).  Defaults to today.
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCorrelationParams {
    /// UUID of the `makod` workflow process (from `de.mako.process.initiated` CloudEvents `process_id` extension).
    pub process_id: Option<String>,
    /// ERP-side order reference (from `erp_order_id` CloudEvents extension).
    /// Matches the reference set by the ERP when submitting a command.
    pub erp_order_id: Option<String>,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PricatParams {
    /// NB GLN (BDEW-Codenummer, 13 digits, starts with 99).
    /// For Gas: DVGW-Codenummer (starts with 98).
    pub nb_mp_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMaloParams {
    /// Filter by Sparte: `STROM` or `GAS`.
    pub sparte: Option<String>,
    /// Filter by bilanzierungsmethode: `RLM`, `SLP`, `IMS`, `TLP_GEMEINSAM`, etc.
    pub bilanzierungsmethode: Option<String>,
    /// Filter by netzebene.
    pub netzebene: Option<String>,
    /// Maximum results (default 50, max 200).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMeloParams {
    /// MeLo ID (DE + 31 alphanumeric characters).
    pub melo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetLokaionsZuordnungParams {
    /// 11-digit MaLo ID.
    pub malo_id: String,
    /// Reference date (YYYY-MM-DD). Defaults to today.
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVersorgungsStatusHistoryParams {
    /// 11-digit MaLo ID.
    pub malo_id: String,
    /// Maximum results (default 20).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetNbEnergiemixParams {
    /// 13-digit NB MP-ID.
    pub nb_mp_id: String,
    /// Calendar year (defaults to most recent available).
    pub year: Option<i16>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTechnischeRessourceParams {
    /// TechnischeRessource ID (from `marktd` PUT endpoint).
    pub tr_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSteuerbareRessourceParams {
    /// SteuerbareRessource ID.
    pub sr_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPartnerParams {
    /// 13-digit BDEW/DVGW MP-ID (Marktpartner-ID).
    pub mp_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMeloStandorteigenschaftenParams {
    /// MeLo-ID (DE + 31 alphanumeric characters).
    pub melo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVersorgungAtParams {
    /// 11-digit MaLo ID.
    pub malo_id: String,
    /// Reference date (YYYY-MM-DD).  Returns the supply status valid on that date.
    pub at: String,
}

#[derive(Clone)]
pub struct MdmdMcpHandler {
    state: Arc<MdmdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<MdmdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<MdmdMcpHandler>,
}

#[tool_router]
impl MdmdMcpHandler {
    fn new(state: Arc<MdmdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Read a Marktlokation (MaLo) record by its 11-digit ID.
    ///
    /// Returns the full MaLo record including address, Sparte, NB and MSB GLNs,
    /// and associated MeLo IDs.  Returns an error when the MaLo has not been
    /// registered with this instance.
    #[tool(
        description = "Read a Marktlokation (MaLo) record by 11-digit malo_id",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_malo(
        &self,
        Parameters(p): Parameters<GetMaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query_as::<
            _,
            (
                String,            // malo_id
                String,            // sparte
                Option<String>,    // netzebene
                Option<String>,    // bilanzierungsmethode
                Option<String>,    // bilanzierungsgebiet
                Option<String>,    // gasqualitaet
                Option<String>,    // energierichtung
                Option<String>,    // regelzone
                Option<String>,    // fallgruppe
                i64,               // version
                serde_json::Value, // data (full BO4E MARKTLOKATION)
            ),
        >(
            r"SELECT malo_id, sparte, netzebene, bilanzierungsmethode,
                     bilanzierungsgebiet, gasqualitaet, energierichtung,
                     regelzone, fallgruppe, version, data
              FROM malo
              WHERE malo_id = $1",
        )
        .bind(&p.malo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                malo_id,
                sparte,
                netzebene,
                bilanzierungsmethode,
                bilanzierungsgebiet,
                gasqualitaet,
                energierichtung,
                regelzone,
                fallgruppe,
                version,
                data,
            )) => ContentBlock::json(serde_json::json!({
                "malo_id": malo_id,
                "sparte": sparte,
                "netzebene": netzebene,
                "bilanzierungsmethode": bilanzierungsmethode,
                "bilanzierungsgebiet": bilanzierungsgebiet,
                "gasqualitaet": gasqualitaet,
                "energierichtung": energierichtung,
                "regelzone": regelzone,
                "fallgruppe": fallgruppe,
                "version": version,
                "data": data,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "malo_not_found: MaLo '{}' not found.",
                p.malo_id
            ))])),
        }
    }

    /// List registered trading partners (paginated, max 500).
    ///
    /// Returns partner records with GLN, name, AS4 endpoint URL, and
    /// configured market roles.  Use `limit` and `cursor` to page through
    /// large directories.
    #[tool(
        description = "List registered trading partners (paginated, max 500)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_partners(
        &self,
        Parameters(p): Parameters<ListPartnersParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(100).clamp(1, 500) as i64;
        let offset: i64 = p
            .cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0i64);

        let rows = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<Vec<String>>,
                serde_json::Value,
            ),
        >(
            r"SELECT mp_id, display_name, makoadresse, channels
              FROM partners
              ORDER BY mp_id
              LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let partners: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(mp_id, display_name, makoadresse, channels)| {
                serde_json::json!({
                    "mp_id": mp_id,
                    "display_name": display_name,
                    "as4_endpoint": makoadresse.as_ref().and_then(|v| v.first()).cloned(),
                    "makoadresse": makoadresse,
                    "channels": channels,
                })
            })
            .collect();

        let next_cursor: Option<String> = if partners.len() == limit as usize {
            Some((offset + limit).to_string())
        } else {
            None
        };

        ContentBlock::json(serde_json::json!({
            "partners": partners,
            "next_cursor": next_cursor,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Read the current PreisblattNetznutzung for a Netzbetreiber.
    ///
    /// Returns the price sheet used by `invoicd` to validate INVOIC billing
    /// plausibility (§22 MessZV).  Pass `date` to retrieve the sheet valid
    /// on a specific billing date (default: today).
    #[tool(
        description = "Read the PreisblattNetznutzung (NNE price sheet) for an NB GLN",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_preisblatt(
        &self,
        Parameters(p): Parameters<GetPreisblattParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = p
            .date
            .as_deref()
            .and_then(|s| {
                time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
            })
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        let row = sqlx::query_as::<_, (uuid::Uuid, time::Date, serde_json::Value)>(
            r"SELECT id, valid_from, preisblatt
              FROM preisblaetter
              WHERE tenant = $1
                AND nb_mp_id = $2
                AND valid_from <= $3
              ORDER BY valid_from DESC
              LIMIT 1",
        )
        .bind(&self.state.tenant)
        .bind(&p.nb_mp_id)
        .bind(date)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((id, valid_from, preisblatt)) => ContentBlock::json(serde_json::json!({
                "id": id,
                "nb_mp_id": p.nb_mp_id,
                "valid_from": valid_from.to_string(),
                "preisblatt": preisblatt,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "preisblatt_not_found: No price sheet for NB '{}' valid on '{date}'.",
                p.nb_mp_id
            ))])),
        }
    }

    /// Read the VersorgungsStatus (supply status) for a Marktlokation.
    ///
    /// Returns the current delivery status (`Beliefert`, `Unbeliefert`, etc.),
    /// active LF and MSB GLNs, and the delivery start/end dates.
    /// This is the authoritative source for automated LFA E_0624 responses
    /// and process routing in `makod`.
    #[tool(
        description = "Read the VersorgungsStatus (delivery status) for a MaLo by 11-digit malo_id",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_versorgungsstatus(
        &self,
        Parameters(p): Parameters<GetVersorgungsstatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query_as::<
            _,
            (
                String,               // malo_id
                String,               // lieferstatus
                Option<String>,       // lf_mp_id
                Option<String>,       // lf_mp_id_next
                Option<time::Date>,   // lieferbeginn
                Option<time::Date>,   // lieferende
                Option<String>,       // msb_mp_id
                String,               // nb_mp_id
                i64,                  // version
                time::OffsetDateTime, // updated_at
            ),
        >(
            r#"SELECT malo_id, lieferstatus, lf_mp_id, lf_mp_id_next,
                      lieferbeginn, lieferende, msb_mp_id, nb_mp_id,
                      version, updated_at
               FROM versorgungsstatus
               WHERE tenant = $1 AND malo_id = $2"#,
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                malo_id,
                lieferstatus,
                lf_mp_id,
                lf_mp_id_next,
                lieferbeginn,
                lieferende,
                msb_mp_id,
                nb_mp_id,
                version,
                updated_at,
            )) => ContentBlock::json(serde_json::json!({
                "malo_id": malo_id,
                "lieferstatus": lieferstatus,
                "lf_mp_id": lf_mp_id,
                "lf_mp_id_next": lf_mp_id_next,
                "lieferbeginn": lieferbeginn.map(|d| d.to_string()),
                "lieferende": lieferende.map(|d| d.to_string()),
                "msb_mp_id": msb_mp_id,
                "nb_mp_id": nb_mp_id,
                "version": version,
                "updated_at": updated_at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "versorgungsstatus_not_found: No VersorgungsStatus for MaLo '{}' in tenant '{}'.",
                p.malo_id, self.state.tenant
            ))])),
        }
    }

    /// Read the active NB network contract for a Marktlokation.
    ///
    /// Returns netzebene, bilanzierungsmethode (RLM/SLP), billing_schedule
    /// (MONTHLY/QUARTERLY/ANNUALLY), and validity period.  `invoicd` uses this
    /// to validate billing-cycle plausibility (§22 MessZV).  Pass `date` to
    /// retrieve the contract valid on a specific billing date; defaults to today.
    #[tool(
        description = "Read the active NB network contract for a MaLo (netzebene, billing_schedule, RLM/SLP)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_nb_contract(
        &self,
        Parameters(p): Parameters<GetNbContractParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = p
            .date
            .as_deref()
            .and_then(|s| {
                time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
            })
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        let row = sqlx::query_as::<
            _,
            (
                String,             // contract_id
                String,             // malo_id
                String,             // nb_mp_id
                String,             // sparte
                String,             // netzebene
                String,             // bilanzierungsmethode
                String,             // billing_schedule
                time::Date,         // valid_from
                Option<time::Date>, // valid_to
                i64,                // version
            ),
        >(
            r#"
            SELECT contract_id, malo_id, nb_mp_id, sparte,
                   netzebene, bilanzierungsmethode, billing_schedule,
                   valid_from, valid_to, version
            FROM nb_contracts
            WHERE tenant = $1
              AND malo_id = $2
              AND valid_from <= $3
              AND (valid_to IS NULL OR valid_to >= $3)
            ORDER BY valid_from DESC
            LIMIT 1
            "#,
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .bind(date)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                contract_id,
                malo_id,
                nb_mp_id,
                sparte,
                netzebene,
                bilanzierungsmethode,
                billing_schedule,
                valid_from,
                valid_to,
                version,
            )) => ContentBlock::json(serde_json::json!({
                "contract_id": contract_id,
                "malo_id": malo_id,
                "nb_mp_id": nb_mp_id,
                "sparte": sparte,
                "netzebene": netzebene,
                "bilanzierungsmethode": bilanzierungsmethode,
                "billing_schedule": billing_schedule,
                "valid_from": valid_from.to_string(),
                "valid_to": valid_to.map(|d| d.to_string()),
                "version": version,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "nb_contract_not_found: No active NB contract for MaLo '{}' on '{date}'.",
                p.malo_id
            ))])),
        }
    }

    /// Look up a process correlation record.
    ///
    /// Provide either `process_id` (the UUID from `de.mako.process.initiated`
    /// CloudEvents `process_id` extension) or `erp_order_id` (the ERP-side work-
    /// order reference).  Returns workflow name, BDEW PID, MaLo, current status
    /// (`RUNNING` / `COMPLETED` / `FAILED`), and timing.
    ///
    /// Typical use: an ERP integration polls `get_correlation` after submitting
    /// a command via `makod` to confirm the process has reached `COMPLETED`.
    #[tool(
        description = "Get a process correlation record by process_id UUID or erp_order_id string",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_correlation(
        &self,
        Parameters(p): Parameters<GetCorrelationParams>,
    ) -> Result<CallToolResult, McpError> {
        type CorrelationRow = (
            uuid::Uuid,                   // process_id
            Option<String>,               // workflow_name
            Option<i32>,                  // pid
            Option<String>,               // malo_id
            Option<String>,               // erp_order_id
            String,                       // status
            time::OffsetDateTime,         // initiated_at
            Option<time::OffsetDateTime>, // completed_at
        );
        const COLS: &str = r#"SELECT process_id, workflow_name, pid, malo_id,
                   erp_order_id, status, initiated_at, completed_at
            FROM process_correlation"#;

        let row: Option<CorrelationRow> = if let Some(ref pid_str) = p.process_id {
            let id = pid_str
                .parse::<uuid::Uuid>()
                .map_err(|_| McpError::invalid_params("process_id must be a valid UUID", None))?;
            sqlx::query_as::<_, CorrelationRow>(&format!("{COLS} WHERE process_id = $1"))
                .bind(id)
                .fetch_optional(&self.state.pool)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?
        } else if let Some(ref erp_id) = p.erp_order_id {
            sqlx::query_as::<_, CorrelationRow>(&format!(
                "{COLS} WHERE erp_order_id = $1 ORDER BY initiated_at DESC LIMIT 1"
            ))
            .bind(erp_id)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
        } else {
            return Err(McpError::invalid_params(
                "provide either process_id or erp_order_id",
                None,
            ));
        };

        match row {
            Some((
                process_id,
                workflow_name,
                pid,
                malo_id,
                erp_order_id,
                status,
                initiated_at,
                completed_at,
            )) => ContentBlock::json(serde_json::json!({
                "process_id": process_id,
                "workflow_name": workflow_name,
                "pid": pid,
                "malo_id": malo_id,
                "erp_order_id": erp_order_id,
                "status": status,
                "initiated_at": initiated_at,
                "completed_at": completed_at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(
                "correlation_not_found: No process correlation record found for the given identifier."
                    .to_owned(),
            )])),
        }
    }
    #[tool(
        description = "List PRICAT (Preisblatt) version history for an NB MP-ID. \
Shows all PRICAT 27003 versions with valid_from/valid_to dates, dispatch_state (pending/queued/done/error), and source. \
Use after a tariff change to verify the new PRICAT was dispatched to all LF counterparties.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pricat_versions(
        &self,
        Parameters(p): Parameters<PricatParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::PgPriCatRepository;
        use mako_markt::repository::PriCatRepository as _;
        let repo = PgPriCatRepository::new(self.state.pool.clone());
        match repo.list_versions(&p.nb_mp_id, &self.state.tenant).await {
            Ok(versions) => {
                let out: Vec<serde_json::Value> = versions
                    .iter()
                    .map(|v| {
                        serde_json::json!({
                            "id": v.id,
                            "nb_mp_id": v.nb_mp_id,
                            "valid_from": v.valid_from,
                            "valid_to": v.valid_to,
                            "dispatch_state": format!("{:?}", v.dispatch_state),
                            "source": format!("{:?}", v.source),
                            "created_at": v.created_at,
                        })
                    })
                    .collect();
                ContentBlock::json(serde_json::json!({
                    "nb_mp_id": p.nb_mp_id,
                    "version_count": out.len(),
                    "versions": out,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Trigger (re-)dispatch of the latest PRICAT 27003 version for an NB to all active LF counterparties. \
Use after an AS4 connectivity incident or to force distribution to newly on-boarded LF partners. \
Returns the version_id that was queued. Actual dispatch is asynchronous. \
⚠ NB-role only — Informatorisches Unbundling: LF actors must not trigger NB PRICAT dispatch.",
        annotations(read_only_hint = false, idempotent_hint = true, open_world_hint = true)
    )]
    async fn dispatch_pricat(
        &self,
        Parameters(p): Parameters<PricatParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::PgPriCatRepository;
        use mako_markt::repository::PriCatRepository as _;
        let repo = PgPriCatRepository::new(self.state.pool.clone());
        match repo.find_latest(&p.nb_mp_id, &self.state.tenant).await {
            Ok(Some(v)) => {
                match repo.mark_queued(v.id).await {
                    Ok(()) => ContentBlock::json(serde_json::json!({
                        "version_id": v.id,
                        "nb_mp_id": p.nb_mp_id,
                        "valid_from": v.valid_from,
                        "status": "queued",
                        "note": "PRICAT dispatch enqueued. Check list_pricat_versions in ~30s for dispatch_state=done.",
                    }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None)),
                    Err(e) => Err(McpError::internal_error(e.to_string(), None)),
                }
            }
            Ok(None) => Ok(CallToolResult::error(vec![ContentBlock::text(
                format!("No PRICAT version found for NB GLN {}", p.nb_mp_id),
            )])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    // ── New tools ──────────────────────────────────────────────────────────

    #[tool(
        description = "List Marktlokationen with optional filters: sparte (STROM/GAS), bilanzierungsmethode (RLM/SLP/IMS), netzebene. Returns up to 200 MaLos.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_malo(
        &self,
        Parameters(p): Parameters<ListMaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(50).clamp(1, 200);
        let rows = sqlx::query(
            r"SELECT malo_id, sparte, netzebene, bilanzierungsmethode,
                     bilanzierungsgebiet, gasqualitaet, energierichtung, fallgruppe
              FROM malo
              WHERE ($1::text IS NULL OR sparte = $1)
                AND ($2::text IS NULL OR bilanzierungsmethode = $2)
                AND ($3::text IS NULL OR netzebene = $3)
              ORDER BY malo_id
              LIMIT $4",
        )
        .bind(&p.sparte)
        .bind(&p.bilanzierungsmethode)
        .bind(&p.netzebene)
        .bind(limit)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        let malos: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
            "malo_id": r.try_get::<String,_>("malo_id").ok(),
            "sparte": r.try_get::<String,_>("sparte").ok(),
            "netzebene": r.try_get::<Option<String>,_>("netzebene").ok().flatten(),
            "bilanzierungsmethode": r.try_get::<Option<String>,_>("bilanzierungsmethode").ok().flatten(),
            "bilanzierungsgebiet": r.try_get::<Option<String>,_>("bilanzierungsgebiet").ok().flatten(),
            "gasqualitaet": r.try_get::<Option<String>,_>("gasqualitaet").ok().flatten(),
            "energierichtung": r.try_get::<Option<String>,_>("energierichtung").ok().flatten(),
            "fallgruppe": r.try_get::<Option<String>,_>("fallgruppe").ok().flatten(),
        })).collect();

        ContentBlock::json(serde_json::json!({ "count": malos.len(), "malos": malos }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Read a Messlokation (MeLo) record by ID. Returns netzebene_messung, regelzone, standorteigenschaften, and the full BO4E data.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_melo(
        &self,
        Parameters(p): Parameters<GetMeloParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query(
            r"SELECT melo_id, malo_id, netzebene_messung, regelzone,
                     standorteigenschaften, data, version
              FROM melo WHERE melo_id = $1",
        )
        .bind(&p.melo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "melo_id": r.try_get::<String,_>("melo_id").ok(),
                "malo_id": r.try_get::<Option<String>,_>("malo_id").ok().flatten(),
                "netzebene_messung": r.try_get::<Option<String>,_>("netzebene_messung").ok().flatten(),
                "regelzone": r.try_get::<Option<String>,_>("regelzone").ok().flatten(),
                "standorteigenschaften": r.try_get::<Option<serde_json::Value>,_>("standorteigenschaften").ok().flatten(),
                "version": r.try_get::<i64,_>("version").ok(),
                "data": r.try_get::<serde_json::Value,_>("data").ok(),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "melo_not_found: MeLo '{}' not found.", p.melo_id
            ))])),
        }
    }

    #[tool(
        description = "Get temporal role assignments (Lokationszuordnung) for a MaLo: NB, MSB, LF with valid_from/valid_to dates. Essential for GPKE/GeLi process routing.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_lokationszuordnung(
        &self,
        Parameters(p): Parameters<GetLokaionsZuordnungParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = p
            .date
            .as_deref()
            .and_then(|s| {
                time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
            })
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        let rows = sqlx::query(
            r"SELECT zuordnungstyp, rollencodenummer, valid_from, valid_to
              FROM lokationszuordnung
              WHERE malo_id = $1
                AND valid_from <= $2
                AND (valid_to IS NULL OR valid_to >= $2)
              ORDER BY zuordnungstyp",
        )
        .bind(&p.malo_id)
        .bind(date)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        let assignments: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
            "zuordnungstyp": r.try_get::<String,_>("zuordnungstyp").ok(),
            "rollencodenummer": r.try_get::<String,_>("rollencodenummer").ok(),
            "valid_from": r.try_get::<time::Date,_>("valid_from").ok().map(|d| d.to_string()),
            "valid_to": r.try_get::<Option<time::Date>,_>("valid_to").ok().flatten().map(|d| d.to_string()),
        })).collect();

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "reference_date": date.to_string(),
            "count": assignments.len(),
            "assignments": assignments,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get VersorgungsStatus change history for a MaLo. Shows all supply state transitions with timestamps — useful for investigating Lieferbeginn/Lieferende timing.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_versorgungsstatus_history(
        &self,
        Parameters(p): Parameters<GetVersorgungsStatusHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(20).clamp(1, 100);
        let rows = sqlx::query(
            r"SELECT id, lieferstatus, lf_mp_id, lf_mp_id_next,
                     lf_next_lieferbeginn, lieferbeginn, lieferende,
                     msb_mp_id, nb_mp_id, valid_from
              FROM versorgungsstatus_history
              WHERE tenant = $1 AND malo_id = $2
              ORDER BY valid_from DESC
              LIMIT $3",
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .bind(limit)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        let history: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
            "id": r.try_get::<i64,_>("id").ok(),
            "lieferstatus": r.try_get::<String,_>("lieferstatus").ok(),
            "lf_mp_id": r.try_get::<Option<String>,_>("lf_mp_id").ok().flatten(),
            "lf_mp_id_next": r.try_get::<Option<String>,_>("lf_mp_id_next").ok().flatten(),
            "lf_next_lieferbeginn": r.try_get::<Option<time::Date>,_>("lf_next_lieferbeginn").ok().flatten().map(|d| d.to_string()),
            "lieferbeginn": r.try_get::<Option<time::Date>,_>("lieferbeginn").ok().flatten().map(|d| d.to_string()),
            "lieferende": r.try_get::<Option<time::Date>,_>("lieferende").ok().flatten().map(|d| d.to_string()),
            "msb_mp_id": r.try_get::<Option<String>,_>("msb_mp_id").ok().flatten(),
            "nb_mp_id": r.try_get::<String,_>("nb_mp_id").ok(),
            "valid_from": r.try_get::<time::OffsetDateTime,_>("valid_from").ok().map(|t| t.to_string()),
        })).collect();

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "count": history.len(),
            "history": history,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get the §42 EnWG annual Energiemix (grid-area renewable mix) for a Netzbetreiber. Used by LF for Reststrommix disclosure on customer bills.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_nb_energiemix(
        &self,
        Parameters(p): Parameters<GetNbEnergiemixParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query(
            r"SELECT nb_mp_id, gueltig_fuer, energiemix,
                     eeg_einspeisung_kwh, gesamtentnahme_kwh, updated_at
              FROM nb_energiemix
              WHERE tenant = $1
                AND nb_mp_id = $2
                AND ($3::smallint IS NULL OR gueltig_fuer = $3)
              ORDER BY gueltig_fuer DESC
              LIMIT 1",
        )
        .bind(&self.state.tenant)
        .bind(&p.nb_mp_id)
        .bind(p.year)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "nb_mp_id": r.try_get::<String,_>("nb_mp_id").ok(),
                "gueltig_fuer": r.try_get::<i16,_>("gueltig_fuer").ok(),
                "energiemix": r.try_get::<serde_json::Value,_>("energiemix").ok(),
                "eeg_einspeisung_kwh": r.try_get::<Option<i64>,_>("eeg_einspeisung_kwh").ok().flatten(),
                "gesamtentnahme_kwh": r.try_get::<Option<i64>,_>("gesamtentnahme_kwh").ok().flatten(),
                "updated_at": r.try_get::<time::OffsetDateTime,_>("updated_at").ok().map(|t| t.to_string()),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "nb_energiemix_not_found: No Energiemix for NB '{}'. Use PUT /api/v1/energiemix/{{nb_mp_id}} to publish.",
                p.nb_mp_id
            ))])),
        }
    }

    #[tool(
        description = "Get a TechnischeRessource (smart meter, generation unit) by TR-ID. Returns device type, installed capacity, commissioning date, and §14a EnWG steuerkanal info.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_technische_ressource(
        &self,
        Parameters(p): Parameters<GetTechnischeRessourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query(
            r"SELECT tr_id, malo_id, melo_id, tr_typ, ist_fernschaltbar, data, version, updated_at
              FROM technische_ressourcen
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&p.tr_id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "tr_id": r.try_get::<String,_>("tr_id").ok(),
                "malo_id": r.try_get::<Option<String>,_>("malo_id").ok().flatten(),
                "melo_id": r.try_get::<Option<String>,_>("melo_id").ok().flatten(),
                "tr_typ": r.try_get::<Option<String>,_>("tr_typ").ok().flatten(),
                "ist_fernschaltbar": r.try_get::<Option<bool>,_>("ist_fernschaltbar").ok().flatten(),
                "version": r.try_get::<i64,_>("version").ok(),
                "updated_at": r.try_get::<time::OffsetDateTime,_>("updated_at").ok().map(|t| t.to_string()),
                "data": r.try_get::<serde_json::Value,_>("data").ok(),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "tr_not_found: TechnischeRessource '{}' not found.", p.tr_id
            ))])),
        }
    }

    #[tool(
        description = "Get a SteuerbareRessource (controllable load, §14a EnWG) by SR-ID, including all Konfigurationsprodukte (BK6-24-174 §4.3 produktcode).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_steuerbare_ressource(
        &self,
        Parameters(p): Parameters<GetSteuerbareRessourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query(
            r"SELECT sr_id, malo_id, melo_id, data, konfigurationsprodukte, version, updated_at
              FROM steuerbare_ressourcen
              WHERE sr_id = $1 AND tenant = $2",
        )
        .bind(&p.sr_id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "sr_id": r.try_get::<String,_>("sr_id").ok(),
                "malo_id": r.try_get::<Option<String>,_>("malo_id").ok().flatten(),
                "melo_id": r.try_get::<Option<String>,_>("melo_id").ok().flatten(),
                "version": r.try_get::<i64,_>("version").ok(),
                "updated_at": r.try_get::<time::OffsetDateTime,_>("updated_at").ok().map(|t| t.to_string()),
                "data": r.try_get::<serde_json::Value,_>("data").ok(),
                "konfigurationsprodukte": r.try_get::<Option<serde_json::Value>,_>("konfigurationsprodukte").ok().flatten(),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "sr_not_found: SteuerbareRessource '{}' not found.", p.sr_id
            ))])),
        }
    }

    /// Read a single trading partner by its 13-digit MP-ID.
    ///
    /// Returns display name, AS4 / MaKo communication channels, roles, and
    /// the applicable EDIFACT identification scheme (BDEW `293`, DVGW `332`,
    /// or GS1 `9`).
    #[tool(
        description = "Read a single trading partner (Marktpartner) by 13-digit MP-ID",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_partner(
        &self,
        Parameters(p): Parameters<GetPartnerParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query(
            r"SELECT mp_id, display_name, makoadresse, channels, updated_at
              FROM partners
              WHERE mp_id = $1",
        )
        .bind(&p.mp_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "mp_id": r.try_get::<String, _>("mp_id").ok(),
                "display_name": r.try_get::<Option<String>, _>("display_name").ok().flatten(),
                "makoadresse": r.try_get::<Option<String>, _>("makoadresse").ok().flatten(),
                "channels": r.try_get::<Option<serde_json::Value>, _>("channels").ok().flatten(),
                "updated_at": r.try_get::<Option<time::OffsetDateTime>, _>("updated_at").ok().flatten().map(|t| t.to_string()),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "partner_not_found: Marktpartner '{}' not found.", p.mp_id
            ))])),
        }
    }

    /// Read the BO4E `Standorteigenschaften` for a Messlokation.
    ///
    /// `Standorteigenschaften` carries the Strom/Gas location properties
    /// required for Redispatch 2.0 `NetworkConstraintDocument` cross-references
    /// (`StandorteigenschaftenStrom`: regelzone EIC, bilanzierungsgebietEic) and
    /// Gas billing zone routing (`StandorteigenschaftenGas`: druckstufe).
    ///
    /// Returns 404-style error when the MeLo is unknown or has no Standorteigenschaften
    /// yet (populated by `nis-syncd` or a WiM Stammdaten PUT).
    #[tool(
        description = "Read BO4E Standorteigenschaften for a MeLo (needed for Redispatch 2.0 and Gas zone routing)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_melo_standorteigenschaften(
        &self,
        Parameters(p): Parameters<GetMeloStandorteigenschaftenParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query(
            r"SELECT melo_id, malo_id, standorteigenschaften, netzebene_messung, regelzone
              FROM melo
              WHERE melo_id = $1",
        )
        .bind(&p.melo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => {
                let se = r
                    .try_get::<Option<serde_json::Value>, _>("standorteigenschaften")
                    .ok()
                    .flatten();
                match se {
                    Some(v) => ContentBlock::json(serde_json::json!({
                        "melo_id": r.try_get::<String, _>("melo_id").ok(),
                        "malo_id": r.try_get::<Option<String>, _>("malo_id").ok().flatten(),
                        "netzebene_messung": r.try_get::<Option<String>, _>("netzebene_messung").ok().flatten(),
                        "regelzone": r.try_get::<Option<String>, _>("regelzone").ok().flatten(),
                        "standorteigenschaften": v,
                    }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None)),
                    None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "standorteigenschaften_not_found: MeLo '{}' has no Standorteigenschaften yet.", p.melo_id
                    ))])),
                }
            }
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "melo_not_found: MeLo '{}' not found.",
                p.melo_id
            ))])),
        }
    }

    /// Read the VersorgungsStatus valid on a specific reference date.
    ///
    /// Use this for point-in-time supply-state queries (e.g. for billing period
    /// reconstruction or gap analysis).  For the current state use
    /// `get_versorgungsstatus` instead.
    #[tool(
        description = "Read the VersorgungsStatus for a MaLo at a specific date (YYYY-MM-DD)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_versorgung_at(
        &self,
        Parameters(p): Parameters<GetVersorgungAtParams>,
    ) -> Result<CallToolResult, McpError> {
        let at_date = time::Date::parse(
            &p.at,
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .map_err(|e| McpError::invalid_params(format!("invalid date '{}': {e}", p.at), None))?;

        // Find the history row whose valid_from is the latest on or before `at_date`.
        let row = sqlx::query(
            r"SELECT h.malo_id, h.lieferstatus, h.lf_mp_id, h.lf_mp_id_next,
                     h.lieferbeginn, h.lieferende, h.msb_mp_id, h.nb_mp_id,
                     h.valid_from, h.lf_next_lieferbeginn
              FROM versorgungsstatus_history h
              WHERE h.malo_id = $1
                AND h.tenant  = $2
                AND h.valid_from <= $3
              ORDER BY h.valid_from DESC
              LIMIT 1",
        )
        .bind(&p.malo_id)
        .bind(&self.state.tenant)
        .bind(at_date)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row as _;
        match row {
            Some(r) => ContentBlock::json(serde_json::json!({
                "malo_id": r.try_get::<String, _>("malo_id").ok(),
                "lieferstatus": r.try_get::<String, _>("lieferstatus").ok(),
                "lf_mp_id": r.try_get::<Option<String>, _>("lf_mp_id").ok().flatten(),
                "lf_mp_id_next": r.try_get::<Option<String>, _>("lf_mp_id_next").ok().flatten(),
                "lieferbeginn": r.try_get::<Option<time::Date>, _>("lieferbeginn").ok().flatten().map(|d| d.to_string()),
                "lieferende": r.try_get::<Option<time::Date>, _>("lieferende").ok().flatten().map(|d| d.to_string()),
                "msb_mp_id": r.try_get::<Option<String>, _>("msb_mp_id").ok().flatten(),
                "nb_mp_id": r.try_get::<Option<String>, _>("nb_mp_id").ok().flatten(),
                "valid_from": r.try_get::<time::OffsetDateTime, _>("valid_from").ok().map(|t| t.to_string()),
                "lf_next_lieferbeginn": r.try_get::<Option<time::Date>, _>("lf_next_lieferbeginn").ok().flatten().map(|d| d.to_string()),
                "queried_at": p.at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "versorgung_not_found: No VersorgungsStatus found for MaLo '{}' on or before '{}'.",
                p.malo_id, p.at
            ))])),
        }
    }
}

#[prompt_router]
impl MdmdMcpHandler {
    #[prompt(
        name = "lookup-malo",
        description = "Step-by-step: query and interpret a Marktlokation record"
    )]
    async fn lookup_malo_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I look up and interpret a Marktlokation (MaLo) in marktd?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_malo` with the 11-digit malo_id.\n                 2. Key fields to check:\n                    - netzebene: HSS/HS/MS/NS — determines NNE tariff tier\n                    - bilanzierungsgebiet: Bilanzierungsgebiet-EIC (required for UTILMD)\n                    - energierichtung: EINSP (feed-in) or VERB (consumption)\n                    - sparte: STROM or GAS\n                    - gasqualitaet: H_GAS or L_GAS (Gas only)\n\n                 3. Use `get_versorgungsstatus` to check if the MaLo is currently supplied.\n                 4. Use `get_nb_contract` to find the active network contract (billing_schedule, RLM/SLP).",
            ),
        ]
    }

    #[prompt(
        name = "investigate-supply-gap",
        description = "Step-by-step: investigate a supply gap or VersorgungsStatus anomaly"
    )]
    async fn investigate_supply_gap_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A MaLo shows an unexpected VersorgungsStatus. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. `get_versorgungsstatus` — compare current vs expected status.\n\
                 2. `get_versorgungsstatus_history` — see all transitions with timestamps.\n\
                 3. `get_correlation` with erp_order_id — find the triggering process.\n\
                 4. `get_lokationszuordnung` — verify NB/MSB/LF role assignments are current.\n\
                 5. Status Unbeliefert means no active Liefervertrag — check:\n\
                    - Was UTILMD 55001 (Lieferbeginn) sent and APERAK received?\n\
                    - Was 55003 (NB-Bestätigung) received from NB?\n\
                    - Did processd update VersorgungsStatus on process.completed?\n\
                 6. Fix: re-trigger via processd POST /api/v1/start-supply.",
            ),
        ]
    }

    #[prompt(
        name = "versorgungswechsel-tracking",
        description = "Track a Lieferantenwechsel (supplier switch) end-to-end across marktd + makod"
    )]
    async fn versorgungswechsel_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I track a Lieferantenwechsel (GPKE supplier switch) end-to-end?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## GPKE Lieferantenwechsel Tracking (UTILMD 55001–55009)\n\n\
                 1. `get_malo` — confirm malo_id, sparte=STROM, bilanzierungsgebiet (needed for UTILMD)\n\
                 2. `get_versorgungsstatus` — check lf_mp_id_next and lf_next_lieferbeginn\n\
                    - lf_mp_id_next set = Anmeldung in flight (55001 received by NB)\n\
                    - lf_mp_id set = Beliefert (55003 confirmation received)\n\
                 3. `get_correlation` by process_id — see workflow_name=gpke-supplier-change, status=RUNNING/COMPLETED\n\
                 4. `get_versorgungsstatus_history` — review full transition timeline\n\
                 5. `get_lokationszuordnung` — verify new LF role assignment valid_from matches Lieferbeginn\n\
                 6. `get_nb_contract` — confirm NB contract valid_from ≤ Lieferbeginn\n\n\
                 GPKE deadlines: NB must respond within 24h (APERAK) and 10 Werktage (55002/55003/55004).\n\
                 APERAK 45-min deadline is enforced by makod automatically.",
            ),
        ]
    }

    #[prompt(
        name = "grid-topology",
        description = "Investigate grid topology: MaLo → MeLo → NeLo → lokationszuordnung chains"
    )]
    async fn grid_topology_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I understand the grid topology for a MaLo in marktd?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "## Grid Topology Investigation\n\n\
                 1. `get_malo` — start with malo_id; check sparte, netzebene, bilanzierungsgebiet, regelzone\n\
                 2. `get_lokationszuordnung` — temporal role chain:\n\
                    - NB (or GNB for Gas): grid operator\n\
                    - MSB: Messstellenbetreiber (meter operator)\n\
                    - LF/LFG: active supplier\n\
                 3. `get_melo` — get the MeLo linked to this MaLo:\n\
                    - netzebene_messung: where the meter is physically connected\n\
                    - standorteigenschaften: for Redispatch 2.0 Stammdaten\n\
                    - regelzone: for MABIS IFTSTA routing (→ ÜNB)\n\
                 4. For §14a EnWG steuerbare Ressourcen: `get_steuerbare_ressource`\n\
                 5. For MSB device info: `get_technische_ressource` (smart meter, generation unit)\n\
                 6. lokationszuordnungen (the graph edges): GET /api/v1/malo/{id}/lokationen\n\
                    returns BFS-traversal of all linked MaLo/MeLo/NeLo nodes.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for MdmdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("marktd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "# marktd — Master Data Management\n\
             \n\
             Provides MaLo/MeLo master data, trading partner registry, NNE price sheets, supply status, and grid topology.\n\
             \n\
             ## Tools (13)\n\
             - `get_malo` — read a MaLo by 11-digit ID (sparte, netzebene, bilanzierungsmethode, regelzone, full BO4E data)\n\
             - `list_malo` — list/filter MaLos by sparte, bilanzierungsmethode, netzebene\n\
             - `get_melo` — read a MeLo by ID (netzebene_messung, regelzone, standorteigenschaften)\n\
             - `list_partners` — list registered trading partners (GLN, AS4 endpoint, channels)\n\
             - `get_preisblatt` — read the PreisblattNetznutzung for an NB (used by invoicd for §22 MessZV)\n\
             - `get_versorgungsstatus` — read VersorgungsStatus (Beliefert/Unbeliefert/…) for a MaLo\n\
             - `get_versorgungsstatus_history` — full supply state transition history for a MaLo\n\
             - `get_lokationszuordnung` — temporal NB/MSB/LF role assignments for a MaLo\n\
             - `get_nb_contract` — active NB network contract (netzebene, billing_schedule, RLM/SLP)\n\
             - `get_correlation` — look up a process correlation by process_id or erp_order_id\n\
             - `list_pricat_versions` — PRICAT 27003 version history for an NB\n\
             - `dispatch_pricat` — trigger PRICAT re-dispatch to LF counterparties (NB-only)\n\
             - `get_nb_energiemix` — §42 EnWG annual grid-area renewable mix for an NB\n\
             - `get_technische_ressource` — smart meter / generation unit by TR-ID\n\
             - `get_steuerbare_ressource` — §14a EnWG controllable load + Konfigurationsprodukte by SR-ID\n\
             \n\
             ## Prompts (5)\n\
             `lookup-malo`, `investigate-supply-gap`, `versorgungswechsel-tracking`, `grid-topology`, `msb-preisanfrage`\n\
             \n\
             All reads are tenant-scoped. Cross-tenant access is denied by Cedar ABAC.\n\
             §9 EnWG Informatorisches Unbundling: LF actors must not access NB-private endpoints.",
            )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<MdmdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the `/mcp` Axum router for `marktd`.
pub fn router(state: Arc<MdmdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(MdmdMcpHandler::new(state.clone()))
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
