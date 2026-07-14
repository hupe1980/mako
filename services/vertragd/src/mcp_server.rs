//! MCP server for `vertragd` — Contract & Customer Management.

use axum::{
    Router,
    middleware::{self, Next},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
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

#[derive(Clone)]
pub struct VertragdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VertragIdParams {
    pub id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KundeSubParams {
    pub oidc_sub: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListParams {
    pub limit: Option<i64>,
}

#[derive(Clone)]
pub struct VertragdMcpHandler {
    state: Arc<VertragdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<VertragdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: rmcp::handler::server::router::prompt::PromptRouter<VertragdMcpHandler>,
}

#[tool_router]
impl VertragdMcpHandler {
    fn new(state: Arc<VertragdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "Get a Versorgungsvertrag and all its Vertragskomponenten (STROM/GAS/HEMS/...) by contract UUID.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_vertrag_status(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_vertrag, list_komponenten};
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        match fetch_vertrag(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(v)) => {
                let komp = list_komponenten(&self.state.pool, id)
                    .await
                    .unwrap_or_default();
                ContentBlock::json(serde_json::json!({ "vertrag": v, "komponenten": komp }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("Vertrag {} not found", id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all open Versorgungsverträge (AKTIV, IN_BEARBEITUNG, TEILERFUELLUNG, GEKÜNDIGT).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_offene_vertraege(
        &self,
        Parameters(p): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_offene_vertraege;
        match list_offene_vertraege(
            &self.state.pool,
            &self.state.tenant,
            p.limit.unwrap_or(50).min(200),
        )
        .await
        {
            Ok(rows) => {
                ContentBlock::json(serde_json::json!({ "count": rows.len(), "vertraege": rows }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Resolve an OIDC sub to a customer profile and their active MaLo IDs. Used for portald authorization.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_kunde_by_sub(
        &self,
        Parameters(p): Parameters<KundeSubParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_kunde_by_sub, list_aktive_malo_ids};
        match fetch_kunde_by_sub(&self.state.pool, &p.oidc_sub, &self.state.tenant).await {
            Ok(Some(k)) => {
                let malo_ids = list_aktive_malo_ids(&self.state.pool, k.id, &self.state.tenant)
                    .await
                    .unwrap_or_default();
                ContentBlock::json(serde_json::json!({ "kunde": k, "active_malo_ids": malo_ids }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("No customer with sub={}", p.oidc_sub),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List Versorgungsverträge expiring within N days (vertragsende or preisgarantie_bis).
    ///
    /// Regulatory basis: §13 GasGVV / §14 StromGVV — 30-day advance notice for renewal.
    /// §41 EnWG — customer notification before price-lock expiry.
    #[tool(
        description = "List contracts expiring within N days (vertragsende or preisgarantie_bis). Default: 30 days. Use for proactive renewal outreach (§13 GasGVV / §41 EnWG).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_expiring_contracts(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::find_expiring_vertraege;
        let days = p
            .get("days")
            .and_then(|v| v.as_i64())
            .unwrap_or(30)
            .clamp(1, 365);
        match find_expiring_vertraege(&self.state.pool, &self.state.tenant, days).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "look_ahead_days": days,
                "vertraege": rows,
                "hint": "Check auto_renewal field — contracts with auto_renewal=true should receive 30-day advance notice (§13 GasGVV / §14 StromGVV).",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List all upcoming Tarifwechsel (planned but not yet applied).
    ///
    /// Includes §41 Abs. 3 EnWG 6-week notification status.
    #[tool(
        description = "List all Vertragskomponenten with a pending future Tarifwechsel. Shows whether the §41 Abs. 3 EnWG 6-week advance notification was sent.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pending_tarifwechsel(
        &self,
        Parameters(_): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let today = time::OffsetDateTime::now_utc().date();
        // Fetch all rows with pending_wirksamkeit in the future.
        let rows = sqlx::query_as::<_, crate::pg::PendingTarifwechselRow>(
            r"SELECT k.id AS komp_id, k.vertrag_id, k.malo_id, k.lf_mp_id,
                     k.product_code AS current_product_code, k.pending_product_code,
                     k.pending_wirksamkeit, k.preisanpassung_notif_sent, k.tenant
              FROM vertragskomponenten k
              WHERE k.tenant = $1
                AND k.pending_product_code IS NOT NULL
                AND (k.pending_wirksamkeit IS NULL OR k.pending_wirksamkeit >= $2)
              ORDER BY k.pending_wirksamkeit ASC",
        )
        .bind(&self.state.tenant)
        .bind(today)
        .fetch_all(&self.state.pool)
        .await
        .unwrap_or_default();
        ContentBlock::json(serde_json::json!({
            "count": rows.len(),
            "pending": rows,
            "regulatory_note": "§41 Abs. 3 EnWG: customer must be notified ≥6 weeks before effective date. preisanpassung_notif_sent=false = notification still pending.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Find Vertragskomponenten stuck in ANGEMELDET status beyond MaKo deadline.
    ///
    /// GPKE §20 EnWG: Strom 5 WT, GeLi Gas 10 WT.
    #[tool(
        description = "Find MaKo components stuck in ANGEMELDET status. threshold_days defaults to 5 (Strom/GPKE). Set 10 for Gas/GeLi. Returns components needing operator escalation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn find_stuck_workflows(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::find_stuck_komponents;
        let threshold = p
            .get("threshold_days")
            .and_then(|v| v.as_i64())
            .unwrap_or(5);
        match find_stuck_komponents(&self.state.pool, &self.state.tenant, threshold).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "stuck_count": rows.len(),
                "threshold_days": threshold,
                "alert": !rows.is_empty(),
                "components": rows,
                "regulatory_basis": "GPKE §20 EnWG: 5 WT (Strom) / 10 WT (Gas/GeLi Gas)",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Get full B2B portfolio for a customer (all active MaLo/Sparte combos).
    #[tool(
        description = "Get all active Vertragskomponenten for a B2B customer (portfolio view). Returns one row per MaLo/Sparte. Useful for Sammelrechnung enumeration.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_customer_portfolio(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_portfolio_by_kunde;
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid kunden_id UUID", None))?;
        match list_portfolio_by_kunde(&self.state.pool, id, &self.state.tenant).await {
            Ok(rows) => {
                let total_malos = rows.iter().filter(|r| r.malo_id.is_some()).count();
                ContentBlock::json(serde_json::json!({
                    "kunden_id": id,
                    "total_active": rows.len(),
                    "total_malos": total_malos,
                    "komponenten": rows,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List all Kunden for this tenant (operator / CRM view).
    ///
    /// Use `?kundentyp=B2C|B2B_SLP|B2B_RLM|B2B_HV` to filter.
    #[tool(
        description = "List all customers (Kunden) for this LF tenant. Filter by kundentyp. Lightweight — no JSONB blobs. Use for CRM/ERP sync and churn analysis.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_alle_kunden(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_kunden;
        let kundentyp = p.get("kundentyp").and_then(|v| v.as_str());
        let limit = p
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100)
            .clamp(1, 500);
        match list_kunden(&self.state.pool, &self.state.tenant, kundentyp, limit).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "kunden": rows,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Compute earliest valid Kündigung date for a contract.
    ///
    /// Returns the minimum `lieferende` that respects the notice period (§14 StromGVV / §13 GasGVV).
    #[tool(
        description = "Compute the earliest valid Kündigung date for a contract given today's date and the contract's kuendigungsfrist_monate. Returns regulatory minimum lieferende (§14 StromGVV / §13 GasGVV).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn compute_kuendigungsfrist(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::earliest_kuendigungsdatum;
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        match crate::pg::fetch_vertrag(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(v)) => {
                let today = time::OffsetDateTime::now_utc().date();
                let earliest = earliest_kuendigungsdatum(today, v.kuendigungsfrist_monate);
                ContentBlock::json(serde_json::json!({
                    "vertrag_id": id,
                    "today": today.to_string(),
                    "kuendigungsfrist_monate": v.kuendigungsfrist_monate,
                    "earliest_lieferende": earliest.to_string(),
                    "preisgarantie_bis": v.preisgarantie_bis.map(|d| d.to_string()),
                    "regulatory_basis": "§14 StromGVV / §13 GasGVV",
                    "hint": if v.preisgarantie_bis.is_some_and(|g| g >= earliest) {
                        "Note: Preisgarantie may restrict Tarifwechsel but not Kündigung itself."
                    } else {
                        "No active Preisgarantie restriction."
                    },
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("Vertrag {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl VertragdMcpHandler {
    #[prompt(
        description = "Review open contracts and identify stuck MaKo workflows or expiring Verträge"
    )]
    fn o2c_review(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Review the open contract fulfillment pipeline and identify any stuck or expiring contracts.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_offene_vertraege` to see all contracts IN_BEARBEITUNG or TEILERFUELLUNG.\n\
                 2. For each, use `get_vertrag_status` to see which Vertragskomponenten are stuck.\n\
                 3. Use `find_stuck_workflows` with threshold_days=5 (Strom) or 10 (Gas) — ANGEMELDET > deadline → operator escalation.\n\
                 4. ABGELEHNT: check abgelehnt_erc (A02=MaLo not in NB grid, A05=LF not registered).\n\
                 5. Use `list_expiring_contracts` with days=30 — contracts needing renewal contact.\n\
                 6. Use `list_pending_tarifwechsel` — check preisanpassung_notif_sent=false → 6-week notice not sent.\n\
                 7. B2B customers: use `get_customer_portfolio` for full MaLo/Sparte overview.",
            ),
        ]
    }

    #[prompt(
        description = "B2B customer onboarding: Rahmenvertrag + N Versorgungsverträge + portal access"
    )]
    fn b2b_onboarding(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I onboard a new B2B customer with multiple delivery sites?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**B2B Onboarding Workflow (Rahmenvertrag + N Versorgungsverträge)**\n\n\
                 **Step 1** — Create Kunde (legal entity):\n\
                 `POST /api/v1/kunden` with `kundentyp=B2B_RLM`, `organisations_id`, `umsatzsteuer_id`.\n\n\
                 **Step 2** — Create Rahmenvertrag (portfolio framework):\n\
                 `POST /api/v1/kunden/{id}/rahmenvertraege` with `gueltig_von`, `kuendigungsfrist_monate`, `rechnungsstellung=SAMMEL`.\n\n\
                 **Step 3** — Create Versorgungsvertrag per delivery site:\n\
                 `POST /api/v1/kunden/{id}/vertraege` with `rahmenvertrag_id`, one `komponenten` entry per sparte.\n\
                 → Automatically dispatches GPKE Lieferbeginn (Strom) or GeLi Gas (Gas) to processd.\n\n\
                 **Step 4** — Add portal users:\n\
                 `POST /api/v1/kunden/{id}/identitaeten` for each OIDC user.\n\
                 Set `rolle=ADMIN` for CEO, `rolle=FINANZEN` for accountant, `standort_filter=Werk Nord` for site manager.\n\n\
                 **Step 5** — Set Preisgarantie if applicable:\n\
                 `PUT /api/v1/vertraege/{id}/preisgarantie` with BO4E Preisgarantie COM.\n\n\
                 **Monitoring:** Use `find_stuck_workflows` after 5 Werktage (Strom) / 10 Werktage (Gas) — ANGEMELDET = MaKo not yet confirmed.",
            ),
        ]
    }
}

#[prompt_handler]
#[tool_handler]
impl ServerHandler for VertragdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("vertragd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "vertragd MCP — B2B + B2C Contract & Customer Management.\n\
             Manages Kunden, Rahmenverträge (B2B framework), and Versorgungsverträge.\n\n\
             ## Tools (10)\n\
             - `get_vertrag_status` — full contract + components by UUID\n\
             - `list_offene_vertraege` — AKTIV/IN_BEARBEITUNG/TEILERFUELLUNG/GEKÜNDIGT\n\
             - `get_kunde_by_sub` — OIDC sub → Kunde + active MaLo IDs (portald auth)\n\
             - `list_expiring_contracts` — vertragsende/preisgarantie_bis within N days (§41 EnWG)\n\
             - `list_pending_tarifwechsel` — upcoming Tarifwechsel + §41 Abs. 3 EnWG notification status\n\
             - `find_stuck_workflows` — ANGEMELDET > 5WT (Strom) / 10WT (Gas) — §20 EnWG\n\
             - `get_customer_portfolio` — B2B MaLo/Sparte portfolio overview\n\
             - `list_alle_kunden` — all customers for CRM/ERP sync\n\
             - `compute_kuendigungsfrist` — earliest valid Kündigung date (§14 StromGVV / §13 GasGVV)\n\n\
             ## Prompts (2)\n\
             - `o2c_review` — full O2C pipeline review + stuck/expiring detection\n\
             - `b2b_onboarding` — step-by-step Rahmenvertrag + N Versorgungsverträge workflow",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<VertragdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<VertragdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = VertragdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
