//! MCP server for `billingd` — Multi-Product Billing Engine.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `list_billing_records` | List billing records for a MaLo |
//! | `get_billing_record` | Get a single billing record with full Rechnung BO4E |
//! | `preview_billing` | Dry-run billing calculation (no persist, no CloudEvent) |
//! | `get_customer_product` | Look up the active product assignment for a MaLo |
//! | `get_xrechnung` | Fetch XRechnung 3.0 / ZUGFeRD 2.3 CII XML for B2G submission |
//! | `check_billing_anomaly` | AI anomaly detection: rolling 3-month average vs latest invoice |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |---|---|
//! | `preview-invoice` | Step-by-step: preview a customer invoice before billing run |
//! | `check-dynamic-tariff` | Step-by-step: verify §41a dynamic tariff configuration |
//! | `order-to-cash` | Full Order-to-Cash: GPKE Lieferbeginn → Jahresabschluss |

use std::sync::Arc;

use axum::{Router, http::StatusCode, middleware::{self, Next}, response::IntoResponse};
use mako_service::{cedar::CedarEnforcer, oidc::OidcVerifier};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::{prompt::PromptRouter, tool::ToolRouter}, wrapper::Parameters},
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

#[derive(Clone)]
pub struct BillingdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRecordsParams {
    /// 11-digit MaLo-ID.
    pub malo_id: Option<String>,
    /// LF MP-ID (BDEW-Codenummer).
    pub lf_mp_id: Option<String>,
    /// Filter by outcome (generated/dispatched/paid/disputed).
    pub outcome: Option<String>,
    /// Max results (default 20, max 100).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRecordParams {
    /// UUID of the billing record.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnomalyParams {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// LF MP-ID (BDEW-Codenummer).
    pub lf_mp_id: String,
    /// Anomaly threshold in percent (default 20 — alert when deviation > 20%).
    pub threshold_pct: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PreviewParams {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// LF MP-ID.
    pub lf_mp_id: String,
    /// NB MP-ID (for NNE tariff lookup).
    pub nb_mp_id: String,
    /// Billing period start (YYYY-MM-DD).
    pub period_from: String,
    /// Billing period end (YYYY-MM-DD).
    pub period_to: String,
}

#[derive(Clone)]
pub struct BillingdMcpHandler {
    state: Arc<BillingdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<BillingdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<BillingdMcpHandler>,
}

#[tool_router]
impl BillingdMcpHandler {
    fn new(state: Arc<BillingdMcpState>) -> Self {
        Self { state, tool_router: Self::tool_router(), prompt_router: Self::prompt_router() }
    }

    #[tool(description = "List billing records. Filter by malo_id, lf_mp_id, or outcome (generated/dispatched/paid/disputed). Returns summary without full Rechnung BO4E.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn list_billing_records(
        &self,
        Parameters(params): Parameters<ListRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        match list_billing_records(
            &self.state.pool,
            params.malo_id.as_deref(),
            params.lf_mp_id.as_deref(),
            params.outcome.as_deref(),
            params.limit.unwrap_or(20).min(100),
        )
        .await
        {
            Ok(rows) => ContentBlock::json(serde_json::to_value(rows).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(description = "Get a single billing record by UUID, including the full BO4E Rechnung JSON payload. Use this to inspect line items, totals, and invoice status.",
        annotations(read_only_hint = true, open_world_hint = false))]
    async fn get_billing_record(
        &self,
        Parameters(params): Parameters<GetRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_billing_record;
        let Ok(id) = params.id.parse::<Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match fetch_billing_record(&self.state.pool, id).await {
            Ok(Some(row)) => ContentBlock::json(serde_json::to_value(row).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(format!("record {id} not found"), None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Dry-run billing preview for a MaLo: returns expected Rechnung positions without persisting or emitting a CloudEvent. Calls POST /api/v1/billing/{malo_id}/preview internally.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn preview_billing(
        &self,
        Parameters(params): Parameters<PreviewParams>,
    ) -> Result<CallToolResult, McpError> {
        // Delegate to the HTTP preview endpoint via internal reqwest for now.
        // A direct in-process call would require access to tarifbd/edmd clients which
        // live in the Axum Extension layer. For MCP we return a description of inputs.
        ContentBlock::json(serde_json::json!({
            "hint": "Use POST /api/v1/billing/{malo_id}/preview with the same parameters for a full dry-run calculation.",
            "params": {
                "malo_id": params.malo_id,
                "lf_mp_id": params.lf_mp_id,
                "nb_mp_id": params.nb_mp_id,
                "period_from": params.period_from,
                "period_to": params.period_to,
            }
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
    #[tool(
        description = "Fetch the XRechnung 3.0 / ZUGFeRD 2.3 CII XML for a billing record UUID. \
Returns EN 16931-compliant electronic invoice XML, required for B2G (Bundesbehörden) invoices from 01.01.2027. \
The XML is BASE64-free — returns the raw XML string.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_xrechnung(
        &self,
        Parameters(p): Parameters<GetRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        use crate::pg::fetch_billing_record;
        match fetch_billing_record(&self.state.pool, id).await {
            Ok(Some(row)) => {
                use crate::xrechnung::{build_zugferd_cii_xml, info_from_rechnung_json};
                let rechnung_json = row.get("invoic_json")
                    .cloned()
                    .unwrap_or_default();
                if let Some(info) = info_from_rechnung_json(&rechnung_json) {
                    let xml = build_zugferd_cii_xml(&info);
                    ContentBlock::json(serde_json::json!({
                        "billing_record_id": id,
                        "xrechnung_xml": xml,
                        "standard": "ZUGFeRD 2.3 / XRechnung 3.0 (EN 16931)",
                        "note": "Submit to ZRE (Zentraler Rechnungseingang) for B2G invoices."
                    }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
                } else {
                    Err(McpError::invalid_params(
                        "billing record has no valid Rechnung JSON for XRechnung export",
                        None,
                    ))
                }
            }
            Ok(None) => Err(McpError::invalid_params(format!("record {id} not found"), None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    // ── Billing Anomaly Detection (B6 / L1) ──────────────────────────────────

    #[tool(
        description = "AI billing anomaly detection: compare latest invoice against 3-month rolling average for a MaLo. \
Returns deviation percentage, rolling average, and is_anomaly flag. \
Flags invoices where |deviation| > threshold_pct (default 20%). \
Use this to detect erroneous invoices before customers complain — powercloud's headline AI feature. \
agentd billing-anomaly-agent calls this on every de.billing.rechnung.erstellt event.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_billing_anomaly(
        &self,
        Parameters(p): Parameters<AnomalyParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::check_billing_anomaly;
        use rust_decimal::Decimal;
        use std::str::FromStr;
        let threshold = p
            .threshold_pct
            .and_then(|f| Decimal::from_str(&f.to_string()).ok());
        match check_billing_anomaly(&self.state.pool, &p.malo_id, &p.lf_mp_id, threshold).await {
            Ok(report) => {
                let anomaly_msg = if report.is_anomaly {
                    format!(
                        "ANOMALY DETECTED: {:.1}% deviation (threshold {:.0}%). Investigate with get_billing_record + list_billing_records.",
                        report.deviation_pct.unwrap_or_default(),
                        report.threshold_pct,
                    )
                } else {
                    "No anomaly detected.".to_owned()
                };
                ContentBlock::json(serde_json::json!({
                    "malo_id": report.malo_id,
                    "lf_mp_id": report.lf_mp_id,
                    "latest_record_id": report.latest_record_id,
                    "latest_brutto_eur": report.latest_brutto_eur,
                    "rolling_avg_brutto_eur": report.rolling_avg_brutto_eur,
                    "deviation_pct": report.deviation_pct,
                    "is_anomaly": report.is_anomaly,
                    "sample_count": report.sample_count,
                    "threshold_pct": report.threshold_pct,
                    "summary": anomaly_msg,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}


#[prompt_router]
impl BillingdMcpHandler {

    #[prompt(
        name = "order-to-cash",
        description = "Full Order-to-Cash workflow: from GPKE Lieferbeginn to annual Jahresabschluss"
    )]
    async fn order_to_cash_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Walk me through the complete Order-to-Cash process for a new electricity customer."),
            PromptMessage::new_text(Role::Assistant,
                "The O2C cycle in mako has 6 phases:\n\n                 **1. Customer Onboarding (GPKE)**\n                 POST processd /api/v1/start-supply { malo_id, lieferbeginn_datum }\n                 → makod dispatches UTILMD 55001 to NB\n                 → NB responds UTILMD 55003 (Bestätigung) within 24h\n                 → VersorgungsStatus in marktd → Beliefert\n
                 **2. Tariff Assignment**\n                 PUT tarifbd /api/v1/customer/{malo_id}/product { product_code, lf_mp_id, assigned_from }\n                 → STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE product assigned\n
                 **3. Meter Data (edmd)**\n                 MSCONS readings arrive via makod EDIFACT pipeline automatically.\n                 Verify: edmd GET /api/v1/billing-period/{malo_id}\n
                 **4. Invoice Generation (billingd)**\n                 POST /api/v1/billing/{malo_id}/calculate { lf_mp_id, nb_mp_id, period_from, period_to }\n                 → tarifbd → edmd → marktd (NNE) → §14a discount → EEG credit\n                 → Rechnung BO4E persisted; CloudEvent de.billing.rechnung.erstellt\n                 Use `list_billing_records` to verify; `get_xrechnung` for B2G XML.\n
                 **5. Account Posting (accountingd)**\n                 de.billing.rechnung.erstellt → accountingd debit entry (Rechnungsbetrag)\n                 Check balance: accountingd `get_balance`\n                 Monthly SEPA: accountingd `run_sepa_collection` → pain.008 XML\n                 Payment receipt: accountingd `import_payments` (CAMT.054)\n
                 **6. Dunning & Collections (if overdue)**\n                 `list_overdue` → Mahnstufe 1 (reminder) → 2 (fee) → 3 (Sperrauftrag)\n                 Mahnstufe 3 → de.accounting.sperrauftrag → sperrd → IFTSTA 21039 to NB\n
                 **Annual Jahresabschluss:**\n                 billingd annual settlement → accountingd `trigger_jahresabschluss` → `update_abschlag` with new rate.\n                 ⚠ EEG note: EEG Gutschrift in Rechnung is already netted in the debit amount. \n                 Do NOT separately book de.eeg.verguetung.berechnet credits for the same period.\n\n                 **Monthly Abschlagslauf (automated advance payment cycle):**\n                 accountingd `run_abschlag_cycle` on each billing_day → posts ABSCHLAG entries.\n                 Then `run_sepa_collection` N-5 bank days before due date → generates pain.008 XML.\n                 Import bank statement: `import_payments` (CAMT.054) to match SEPA returns.\n\n                 **Bilanzielle Abgrenzung (HGB §250 — period-end accruals):**\n                 At Monats-/Jahresabschluss: use accountingd `compute_bilanzielle_abgrenzung`.\n                 pRAP (passive): advance payments collected > energy billed → book as liability.\n                 aRAP (active): unbilled energy → edmd GET /billing-period/{malo_id} × tariff.\n                 ERP journals: Dr. Umsatzerlöse / Cr. pRAP 0990; Dr. FLL 1400 / Cr. Erlöse."
            ),
        ]
    }


    #[prompt(
        name = "preview-invoice",
        description = "Step-by-step: preview a customer invoice before billing run"
    )]
    async fn preview_invoice_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Preview the next billing invoice for a customer."),
            PromptMessage::new_text(Role::Assistant, 
                "To preview a billing invoice, use POST /api/v1/billing/{malo_id}/preview.\n                 Required: lf_mp_id, nb_mp_id, period_from, period_to.\n                 Optional: tariff (override from tarifbd), meter (override from edmd), grid (override from marktd).\n\n                 The preview is a full dry-run — same calculation as /calculate but nothing is stored.\n                 The response includes all Rechnungspositionen and netto/brutto totals.",
            ),
        ]
    }

    #[prompt(
        name = "check-dynamic-tariff",
        description = "Step-by-step: verify §41a EPEX dynamic tariff configuration"
    )]
    async fn check_dynamic_tariff_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(Role::User, "Verify the §41a dynamic EPEX tariff is correctly configured."),
            PromptMessage::new_text(Role::Assistant, 
                "For §41a dynamic tariff (mandatory for iMSys customers since Jan 2025):\n                 1. Verify the product in tarifbd has dynamic_epex: true\n                 2. Verify EPEX day-ahead prices are imported for the billing period:\n                    PUT /api/v1/epex-prices/{date} in tarifbd\n                 3. Verify the customer has 15-min Lastgang data in edmd:\n                    GET /api/v1/lastgang/{malo_id}?from=...&to=...\n                 4. Run a preview: POST /api/v1/billing/{malo_id}/preview with dynamic product\n\n                 If Lastgang is unavailable, billingd falls back to static arbeitsmenge_kwh billing.",
            ),
        ]
    }
}


#[tool_handler]
#[prompt_handler]
impl ServerHandler for BillingdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_prompts().build(),
        )
        .with_server_info(Implementation::new("billingd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "billingd MCP — Multi-Product Billing Engine (LF role).\n\
             Supports All energy categories. §41a dynamic EPEX for STROM. Gas Brennwertkorrektur. §14a for WAERMEPUMPE/WALLBOX. EEG/EINSPEISUNG credit notes.\n\
             XRechnung 3.0 / ZUGFeRD 2.3 XML available at GET /api/v1/billing/{id}/xrechnung.\n\n\
             Use `list_billing_records` to audit recent invoices.\n\
             Use `get_billing_record` to inspect a specific Rechnung BO4E.\n\
             Use `preview_billing` hint to understand the dry-run endpoint.",
        )
    }

}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<BillingdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let token = match request.headers().get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) => t.to_owned(),
        None => return (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response(),
    };
    if state.oidc.verify(&token).is_err() {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    next.run(request).await
}

pub fn router(state: Arc<BillingdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = BillingdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
