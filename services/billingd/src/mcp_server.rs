//! MCP server for `billingd` — Multi-Product Billing Engine.
//!
//! ## Tools (12)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_billing_records` | List billing records for a MaLo |
//! | `get_billing_record` | Get a single billing record with full Rechnung BO4E |
//! | `preview_billing` | Dry-run billing calculation (no persist, no CloudEvent) |
//! | `get_xrechnung` | Fetch XRechnung 3.0 / ZUGFeRD 2.3 CII XML for B2G submission |
//! | `check_billing_anomaly` | AI anomaly detection: rolling 3-month average vs latest invoice |
//! | `list_vpp_settlements` | List VPP aggregation settlement records |
//! | `list_corrections` | List Korrekturrechnung / Stornorechnung records (§22 MessZV) |
//! | `calculate_billing` | Trigger a billing calculation run for a MaLo |
//! | `list_product_categories` | Describe all 13 billing categories and their required fields |
//! | `get_billing_summary` | Aggregate billing stats per MaLo (total billed, avg monthly) |
//! | `validate_tariff_config` | Pre-flight validation: §41b iMSys guard, KAV plausibility, missing fields |
//! | `explain_invoice_position` | Explain how a billing position was calculated (PositionTrace audit) |
//!
//! ## Prompts (6)
//!
//! | Prompt | Description |
//! |---|---|
//! | `order-to-cash` | Full Order-to-Cash: GPKE Lieferbeginn → Jahresabschluss |
//! | `preview-invoice` | Step-by-step: preview a customer invoice before billing run |
//! | `check-dynamic-tariff` | Step-by-step: verify §41a dynamic tariff configuration |
//! | `14a-steuerungsrabatt` | Configure §14a EnWG Steuerungsrabatt (Wärmepumpe / Wallbox) |
//! | `eeg-billing` | Configure EEG/EINSPEISUNG billing for feed-in plants |
//! | `gas-billing` | Configure Gas billing — Brennwertkorrektur, BEHG CO₂, H2-blend |

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

#[derive(Clone)]
pub struct BillingdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// Self base URL (e.g. `"http://localhost:9280"`) — used by MCP tools to call the HTTP API.
    pub self_url: String,
    /// Seller name for XRechnung generation (BG-4).
    pub seller_name: String,
    /// Seller VAT-ID for XRechnung (BT-31, e.g. `DE123456789`).
    pub seller_vat_id: Option<String>,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateTariffParams {
    /// TariffInput JSON string to validate (same format as tarifbd product JSONB).
    pub tariff_json: String,
    /// Metering mode to test against (SLP, RLM, IMSYS). Relevant for §41b check.
    pub metering_mode: Option<String>,
    /// Optional MaLo-ID for context (informational only).
    pub malo_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExplainPositionParams {
    /// UUID of the billing record containing the position.
    pub record_id: String,
    /// 1-based position number from the invoice (positionsnummer). Mutually exclusive with description_keyword.
    pub position_number: Option<u32>,
    /// Keyword to match in the position description (positionstext). Mutually exclusive with position_number.
    pub description_keyword: Option<String>,
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
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List billing records. Filter by malo_id, lf_mp_id, or outcome (generated/dispatched/paid/disputed). Returns summary without full Rechnung BO4E.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
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

    #[tool(
        description = "Get a single billing record by UUID, including the full BO4E Rechnung JSON payload. Use this to inspect line items, totals, and invoice status.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
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
            Ok(None) => Err(McpError::invalid_params(
                format!("record {id} not found"),
                None,
            )),
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
        // Call the billingd preview endpoint via HTTP (carries tarifbd/edmd/marktd context)
        let url = format!(
            "{}/api/v1/billing/{}/preview",
            self.state.self_url, params.malo_id
        );
        let body = serde_json::json!({
            "lf_mp_id": params.lf_mp_id,
            "nb_mp_id": params.nb_mp_id,
            "period_from": params.period_from,
            "period_to": params.period_to,
        });
        match reqwest::Client::new().post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                ContentBlock::json(json)
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                Err(McpError::internal_error(
                    format!("Preview failed ({status}): {text}"),
                    None,
                ))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
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
                use crate::xrechnung::{XRechnungInfo, build_zugferd_cii_xml};
                use rust_decimal_macros::dec;
                let info = XRechnungInfo {
                    invoice_number: row
                        .rechnung_json
                        .get("rechnungsnummer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("UNKNOWN")
                        .to_owned(),
                    issue_date: row.period_to,
                    due_date: None,
                    period_from: row.period_from,
                    period_to: row.period_to,
                    seller_mp_id: self.state.tenant.clone(),
                    seller_name: self.state.seller_name.clone(),
                    seller_vat_id: self.state.seller_vat_id.clone(),
                    seller_address: None,
                    buyer_id: row.malo_id.clone(),
                    buyer_name: row.malo_id.clone(),
                    malo_id: row.malo_id.clone(),
                    positions: Vec::new(),
                    netto_eur: row.total_netto_eur.unwrap_or(dec!(0)),
                    mwst_eur: row
                        .total_brutto_eur
                        .and_then(|b| row.total_netto_eur.map(|n| b - n))
                        .unwrap_or(dec!(0)),
                    brutto_eur: row.total_brutto_eur.unwrap_or(dec!(0)),
                    tax_subtotals: Vec::new(),
                    vat_rate_pct: dec!(19),
                };
                let xml = build_zugferd_cii_xml(&info);
                ContentBlock::json(serde_json::json!({
                    "billing_record_id": id,
                    "xrechnung_xml": xml,
                    "standard": "ZUGFeRD 2.3 / XRechnung 3.0 (EN 16931)",
                    "note": "Submit to ZRE (Zentraler Rechnungseingang) for B2G invoices."
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::invalid_params(
                format!("record {id} not found"),
                None,
            )),
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

    // ── VPP Aggregation Settlement (B12 — RED III Article 17) ────────────────

    #[tool(
        description = "List VPP (Virtual Power Plant) aggregation settlement records for a VPP portfolio. \
Returns billing records with category=VPP showing dispatch events, total flexibility kWh, and Einsatzkosten. \
CloudEvent de.vpp.settlement.berechnet is emitted per settlement. RED III Article 17 / §41b EnWG (expected 2026).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_vpp_settlements(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        let lf_mp_id = p.get("lf_mp_id").and_then(|v| v.as_str());
        let limit = p
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20)
            .min(100);
        // VPP records use category=VPP stored under the vpp_id as malo_id.
        let vpp_malo = p.get("vpp_id").and_then(|v| v.as_str());
        match list_billing_records(&self.state.pool, vpp_malo, lf_mp_id, None, limit).await {
            Ok(rows) => {
                let vpp_rows: Vec<_> = rows
                    .iter()
                    .filter(|r| r.category.starts_with("VPP"))
                    .collect();
                ContentBlock::json(serde_json::json!({
                    "count": vpp_rows.len(),
                    "records": vpp_rows,
                    "hint": "POST /api/v1/billing/vpp/{vpp_id} to generate a new VPP settlement from dispatch events. CloudEvent de.vpp.settlement.berechnet is emitted to ERP."
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List Korrekturrechnung and Stornorechnung records (§22 MessZV audit trail). \
Returns all correction/reversal billing records for a MaLo. \
Each record includes original_record_id, correction_reason, and whether it negates the original.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_corrections(
        &self,
        Parameters(params): Parameters<ListRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        // Fetch all records; filter is_correction = true in memory to avoid
        // a separate pg function (corrections are rare — no perf concern).
        match list_billing_records(
            &self.state.pool,
            params.malo_id.as_deref(),
            params.lf_mp_id.as_deref(),
            None, // outcome filter not applied — show all corrections
            params.limit.unwrap_or(50).min(200),
        )
        .await
        {
            Ok(rows) => {
                let corrections: Vec<_> = rows.iter().filter(|r| r.is_correction).collect();
                ContentBlock::json(serde_json::json!({
                    "count": corrections.len(),
                    "records": corrections,
                    "note": "Use POST /api/v1/billing/{id}/correction to create a new correction."
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Trigger a billing calculation run for a MaLo. \
Calls POST /api/v1/billing/{malo_id}/calculate, persists the Rechnung, and emits de.billing.rechnung.erstellt. \
Use preview_billing first to verify the result without side effects.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn calculate_billing(
        &self,
        Parameters(params): Parameters<PreviewParams>,
    ) -> Result<CallToolResult, McpError> {
        let url = format!(
            "{}/api/v1/billing/{}/calculate",
            self.state.self_url, params.malo_id
        );
        let body = serde_json::json!({
            "lf_mp_id": params.lf_mp_id,
            "nb_mp_id": params.nb_mp_id,
            "period_from": params.period_from,
            "period_to": params.period_to,
        });
        match reqwest::Client::new().post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                ContentBlock::json(json)
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(resp) => {
                let st = resp.status();
                let txt = resp.text().await.unwrap_or_default();
                Err(McpError::internal_error(
                    format!("Billing run failed ({st}): {txt}"),
                    None,
                ))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all 12 billing product categories with their required and optional \
TariffInput fields. Use this to discover what fields to set in tarifbd for a given product type. \
Returns a structured description of STROM, GAS, WAERME, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, \
WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, and BUNDLE (§41a dynamic STROM also covered).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_product_categories(
        &self,
        Parameters(_p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let categories = serde_json::json!([
            { "category": "STROM", "description": "Standard electricity — Eintarif/Zweitarif/Mehrtarif", "required": ["arbeitspreis_ct_per_kwh"], "optional": ["grundpreis_ct_per_day", "arbeitspreis_ht_ct_per_kwh", "arbeitspreis_nt_ct_per_kwh", "dynamic_epex", "dynamic_epex_floor_ct_kwh"], "regulatory": "§41a EnWG for dynamic; §3 StromStG levy included" },
            { "category": "GAS", "description": "Natural gas with Brennwertkorrektur and CO₂ levies", "required": ["gas_arbeitspreis_ct_per_kwh_hs"], "optional": ["gas_grundpreis_ct_per_day", "energiesteuer_gas_ct_per_kwh_override", "behg_gas_ct_per_kwh_override"], "regulatory": "§25 Nr. 4 MessEV (Brennwertkorrektur), §2 EnergieStG, BEHG" },
            { "category": "WAERME", "description": "Fernwärme — Grundpreis, Arbeitspreis, Leistungspreis", "required": ["waerme_arbeitspreis_ct_per_kwh"], "optional": ["waerme_grundpreis_eur_per_month", "waerme_leistungspreis_eur_per_kw_month", "mwst_rate_override: 0.07 for renewable Fernwärme"], "regulatory": "§12 Abs.2 Nr.1 UStG: 7% MwSt for renewable heat (set mwst_rate_override: 0.07)" },
            { "category": "SOLAR", "description": "Solar self-consumption, Mieterstrom §38a, §42a GGV community solar", "required": ["solar_arbeitspreis_ct_per_kwh"], "optional": ["mieterstrom_aufschlag_ct_per_kwh", "gemeinschaft_rabatt_ct_per_kwh", "solar_include_stromsteuer", "mwst_rate_override: 0 for PV ≤30kWp from 2023"], "regulatory": "§12 Abs.3 UStG: 0% MwSt for PV ≤30kWp since 01.01.2023 (set mwst_rate_override: 0)" },
            { "category": "EEG", "description": "EEG feed-in Vergütung — credit note to plant operator (LF role, contractual)", "required": ["eeg_verguetungssatz_ct_per_kwh"], "optional": ["eeg_marktpraemie_ct_per_kwh", "eeg_managementpraemie_ct_per_kwh", "kwkg_zuschlag_ct_per_kwh"], "meter": "eeg_meter.einspeisung_kwh, eeg_meter.kwh_during_negative_epex (§51 contractual suspension)", "regulatory": "§21 EEG Vergütung; §20 EEG Marktprämie; §51 EEG Negativpreisregel (contractual for LF)" },
            { "category": "EINSPEISUNG", "description": "Direktvermarktung settlement — Marktwert minus Vermarktungsgebühr", "required": ["marktwert_ct_per_kwh"], "optional": ["vermarktungsgebuehr_ct_per_kwh"], "regulatory": "§20 EEG Direktvermarktung; Direktvermarkter bears negative-price risk (§51 does NOT apply)" },
            { "category": "WAERMEPUMPE", "description": "Heat pump electricity with §14a EnWG Steuerungsrabatt Modul 1/3", "required": ["arbeitspreis_ct_per_kwh"], "optional": ["steuerungsrabatt_modul1_eur_per_kw_year", "steuerungsrabatt_modul3_eur_per_kw_year"], "meter": "meter.spitzenleistung_kw (required for §14a), meter.steuerung_stunden (Modul 3)", "regulatory": "§14a EnWG; BK6-22-300; mandatory for controlled devices ≥3.7kW from 01.01.2024" },
            { "category": "WALLBOX", "description": "EV charging box with §14a EnWG Steuerungsrabatt Modul 1/3 — same as WAERMEPUMPE", "required": ["arbeitspreis_ct_per_kwh"], "optional": ["steuerungsrabatt_modul1_eur_per_kw_year", "steuerungsrabatt_modul3_eur_per_kw_year"], "regulatory": "§14a EnWG same as WAERMEPUMPE" },
            { "category": "HEMS", "description": "Home Energy Management System — platform subscription + optimization events", "required": ["hems_subscription_eur_per_month"], "optional": ["hems_optimization_event_eur", "hems_readout_event_eur"], "meter": "hems_meter.months, hems_meter.optimization_events, hems_meter.readout_events" },
            { "category": "EMOBILITY", "description": "EV charging CPO/EMSP — service fee + kWh + session fees", "required": ["emobility_service_fee_eur or emobility_kwh_price_ct"], "optional": ["emobility_session_fee_eur", "emobility_roaming_fee_eur"], "meter": "emobility_meter.months, emobility_meter.kwh_charged, emobility_meter.sessions" },
            { "category": "ENERGIEDIENSTLEISTUNG", "description": "Energy services (MSB, maintenance, analytics) — flat fee + event count", "required": ["service_fee_eur or service_event_price_eur"], "optional": [], "meter": "service_meter.months, service_meter.event_count" },
            { "category": "BUNDLE", "description": "Composite product — NOT YET IMPLEMENTED. Submit individual calculate requests per component.", "required": [], "note": "Returns 501 Not Implemented. Submit separate requests per component product." }
        ]);
        ContentBlock::json(categories)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Validate a TariffInput configuration for regulatory compliance before billing. Checks: §41b iMSys requirement for dynamic tariffs, missing mandatory fields, KAV rate plausibility, StromsteuerBefreiung certificate reminders. Returns warnings and errors without triggering a calculation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn validate_tariff_config(
        &self,
        Parameters(params): Parameters<ValidateTariffParams>,
    ) -> Result<CallToolResult, McpError> {
        use energy_billing::{
            BillingContext, GridInput, InvoiceType, MeterInput, MeteringMode, Product, Quantities,
            RegulatoryRates,
        };
        use time::macros::date;

        let tariff: Product = serde_json::from_str(&params.tariff_json)
            .map_err(|e| McpError::invalid_params(format!("invalid Product JSON: {e}"), None))?;

        let rates = RegulatoryRates::default();
        let grid = GridInput::default();

        // Build engine for validation — use a synthetic one-month context.
        let engine = tariff.build_engine(&grid, &rates);

        // Build a context with the requested metering mode for §41b checks.
        let metering_mode = params
            .metering_mode
            .as_deref()
            .map(|m| match m.to_uppercase().as_str() {
                "IMSYS" | "SMART_METER" => MeteringMode::Imsys,
                "RLM" => MeteringMode::Rlm,
                _ => MeteringMode::Slp,
            })
            .unwrap_or_default();

        let ctx = BillingContext {
            malo_id: params.malo_id.unwrap_or_else(|| "00000000000".to_owned()),
            lf_mp_id: "9900000000001".to_owned(),
            rechnungsnummer: "VALIDATE".to_owned(),
            period_from: date!(2026 - 01 - 01),
            period_to: date!(2026 - 01 - 31),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: rates,
            ..Default::default()
        };

        let quantities = Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: rust_decimal_macros::dec!(100),
                metering_mode: metering_mode.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let warnings = engine.validate(&ctx, &quantities);

        // Additional static checks independent of engine.validate():
        let mut extra_checks: Vec<serde_json::Value> = Vec::new();

        // Check §41b: dynamic_epex requires iMSys
        let is_dynamic = matches!(&tariff, Product::Strom(e) if e.dynamic_epex)
            || matches!(&tariff, Product::Waermepumpe(c) if c.base.dynamic_epex)
            || matches!(&tariff, Product::Wallbox(c) if c.base.dynamic_epex);
        if is_dynamic && metering_mode != MeteringMode::Imsys {
            extra_checks.push(serde_json::json!({
                "code": "SECT41B_IMSYS_REQUIRED",
                "severity": "Error",
                "message": "§41b Abs. 2 EnWG: dynamic_epex=true requires MeteringMode::Imsys. Set metering_mode to IMSYS or switch to a fixed-price tariff."
            }));
        }

        // Check §9 exemption: industrie_stromsteuer_befreiung legacy flag migration reminder
        if matches!(&tariff, Product::Strom(e) if e.industrie_stromsteuer_befreiung && e.stromsteuer_befreiung == energy_billing::StromsteuerBefreiung::Keine)
            || matches!(&tariff, Product::Waermepumpe(c) if c.base.industrie_stromsteuer_befreiung)
        {
            extra_checks.push(serde_json::json!({
                "code": "STROMSTEUER_BEFREIUNG_LEGACY_FLAG",
                "severity": "Warning",
                "message": "industrie_stromsteuer_befreiung=true is a legacy flag. Migrate to stromsteuer_befreiung=INDUSTRIE_PRODUKTIONES_GEWERBE for typed §9 StromStG exemption tracking."
            }));
        }

        // Check §42 EnWG: energiequellen should be set for STROM products
        if matches!(tariff.category_str(), "STROM" | "WAERMEPUMPE" | "WALLBOX") {
            let lacks_eq = match &tariff {
                Product::Strom(e) => e.energiequellen.is_none(),
                Product::Waermepumpe(c) | Product::Wallbox(c) => c.base.energiequellen.is_none(),
                _ => false,
            };
            if lacks_eq {
                extra_checks.push(serde_json::json!({
                    "code": "SECT42_ENERGIEMIX_MISSING",
                    "severity": "Warning",
                    "message": "§42 Abs. 1 + Abs. 2 Nr. 2 EnWG: electricity tariffs should declare energiemix or energiequellen (incl. co2_g_per_kwh). Required on every electricity invoice."
                }));
            }
        }

        let warning_json: Vec<serde_json::Value> = warnings
            .iter()
            .map(|w| {
                serde_json::json!({
                    "code": w.code,
                    "severity": format!("{:?}", w.severity),
                    "message": w.message,
                })
            })
            .collect();

        let has_errors = warnings
            .iter()
            .any(|w| w.severity == energy_billing::WarningSeverity::Error)
            || extra_checks.iter().any(|c| c["severity"] == "Error");

        ContentBlock::json(serde_json::json!({
            "category": tariff.category_str(),
            "valid": !has_errors,
            "warnings": warning_json,
            "additional_checks": extra_checks,
            "metering_mode_tested": format!("{:?}", metering_mode),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Explain how a specific billing position was calculated. Returns the full PositionTrace: formula, inputs, regulatory citations, tariff source, and pro-rata fraction. Use this for invoice audit, customer disputes, or regulatory compliance review.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn explain_invoice_position(
        &self,
        Parameters(params): Parameters<ExplainPositionParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_billing_record;
        let Ok(record_id) = params.record_id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params(
                "record_id must be a valid UUID",
                None,
            ));
        };

        let record = fetch_billing_record(&self.state.pool, record_id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::invalid_params(format!("record {record_id} not found"), None)
            })?;

        // Extract position from the stored rechnung_json
        let rechnung = &record.rechnung_json;
        let Some(positions) = rechnung["rechnungspositionen"].as_array() else {
            return Err(McpError::internal_error(
                "no rechnungspositionen in record",
                None,
            ));
        };

        // Find by 1-based position number or by description keyword
        let target = if let Some(pos_nr) = params.position_number {
            positions
                .iter()
                .find(|p| p["positionsnummer"].as_u64() == Some(pos_nr as u64))
        } else if let Some(ref keyword) = params.description_keyword {
            let kw_lower = keyword.to_lowercase();
            positions.iter().find(|p| {
                p["positionstext"]
                    .as_str()
                    .map(|t| t.to_lowercase().contains(&kw_lower))
                    .unwrap_or(false)
            })
        } else {
            return Err(McpError::invalid_params(
                "provide either position_number or description_keyword",
                None,
            ));
        };

        match target {
            Some(pos) => {
                // Return the position with its trace if present
                let explanation = serde_json::json!({
                    "record_id": record_id,
                    "malo_id": record.malo_id,
                    "period": format!("{} – {}", record.period_from, record.period_to),
                    "position": pos,
                    "explanation": {
                        "positionstext": pos["positionstext"],
                        "menge": pos["positionsMenge"],
                        "einzelpreis": pos["einzelpreis"],
                        "gesamtpreis": pos["gesamtpreis"],
                        "rechtsgrundlage": pos["rechtlicheGrundlage"],
                        "kategorie": pos["kategorie"],
                        "trace": pos.get("trace"),
                        "note": "The 'trace' field contains formula, input_quantity, input_unit_price_eur, gross_eur, regulatory_basis, tariff_source, and pro_rata_fraction for full audit reconstruction."
                    }
                });
                ContentBlock::json(explanation)
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            None => Err(McpError::invalid_params(
                "position not found — check position_number or description_keyword",
                None,
            )),
        }
    }

    #[tool(
        description = "Aggregate billing statistics for a MaLo: total billed, monthly average, category breakdown. Use to spot billing trends and verify consistent tariff application.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_billing_summary(
        &self,
        Parameters(params): Parameters<ListRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_billing_records;
        use rust_decimal::Decimal;
        let malo_id = params.malo_id.as_deref();
        let lf_mp_id = params.lf_mp_id.as_deref();
        match list_billing_records(&self.state.pool, malo_id, lf_mp_id, None, 100).await {
            Ok(rows) => {
                let mut total_brutto = Decimal::ZERO;
                let mut count = 0usize;
                let mut by_category: std::collections::HashMap<String, (usize, Decimal)> =
                    std::collections::HashMap::new();
                for row in &rows {
                    if let Some(brutto) = row.total_brutto_eur {
                        total_brutto += brutto;
                        count += 1;
                        let cat = row.category.clone();
                        let e = by_category.entry(cat).or_insert((0, Decimal::ZERO));
                        e.0 += 1;
                        e.1 += brutto;
                    }
                }
                let avg_monthly = if count > 0 {
                    total_brutto / Decimal::from(count)
                } else {
                    Decimal::ZERO
                };
                let category_summary: Vec<_> = by_category.iter()
                    .map(|(cat, (cnt, total))| serde_json::json!({ "category": cat, "count": cnt, "total_brutto_eur": total.to_string() }))
                    .collect();
                ContentBlock::json(serde_json::json!({
                    "malo_id": malo_id,
                    "lf_mp_id": lf_mp_id,
                    "record_count": count,
                    "total_brutto_eur": total_brutto.to_string(),
                    "avg_monthly_brutto_eur": avg_monthly.to_string(),
                    "by_category": category_summary,
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
            PromptMessage::new_text(
                Role::User,
                "Preview the next billing invoice for a customer.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
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
            PromptMessage::new_text(
                Role::User,
                "Verify the §41a dynamic EPEX tariff is correctly configured.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "For §41a dynamic tariff (mandatory for iMSys customers since Jan 2025):\n                 1. Verify the product in tarifbd has dynamic_epex: true\n                 2. Verify EPEX day-ahead prices are imported for the billing period:\n                    PUT /api/v1/epex-prices/{date} in tarifbd\n                 3. Verify the customer has 15-min Lastgang data in edmd:\n                    GET /api/v1/lastgang/{malo_id}?from=...&to=...\n                 4. Run a preview: POST /api/v1/billing/{malo_id}/preview with dynamic product\n\n                 If Lastgang is unavailable, billingd falls back to static arbeitsmenge_kwh billing.",
            ),
        ]
    }

    #[prompt(
        name = "14a-steuerungsrabatt",
        description = "Step-by-step: configure §14a EnWG Steuerungsrabatt billing for Wärmepumpe or Wallbox"
    )]
    async fn steuerungsrabatt_14a_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I set up §14a billing for a heat pump customer?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "§14a EnWG (Steuerbarkeitsrabatt) has 3 implementation models:\n\n\
                **Modul 1 — Capacity-based NNE reduction (kW/year)**\n\
                In tarifbd: set `steuerungsrabatt_modul1_eur_per_kw_year` in the WAERMEPUMPE/WALLBOX product.\n\
                Example: 150 EUR/kW/year → 5 kW WP → 750 EUR/year Netzentgelteinsparung (vor MwSt).\n\
                Requires: spitzenleistung_kw in the meter reading or billing request.\n\
                Formula: kW × rate_eur_per_kw_year / 12 × billing_months → credit position.\n\n\
                **Modul 2 — Event-based (per dispatch hour) — NOT YET IN billingd**\n\
                Requires ZeitvariablePreisposition from marktd + actual controlled kWh from edmd.\n\
                Planned: integrate with processd §14a Steuerungsauftrag CloudEvent pipeline.\n\n\
                **Modul 3 — Load-shedding compensation (Laststeuerung hours × kW)**\n\
                In tarifbd: set `steuerungsrabatt_modul3_eur_per_kw_year` in the product.\n\
                Requires: steuerung_stunden in the meter reading (from agentd/processd).\n\
                Formula: kW × rate × (steuerung_stunden / 8760) → credit position.\n\n\
                **Setup steps:**\n\
                1. GET tarifbd /api/v1/products → find WAERMEPUMPE or WALLBOX product\n\
                2. PUT tarifbd /api/v1/products/{id} add steuerungsrabatt_modul1_eur_per_kw_year\n\
                3. POST /api/v1/billing/{malo_id}/preview — verify Steuerungsrabatt position appears\n\
                4. Check: position tagged 'steuerungsrabatt_modul1' → negative credit amount\n\
                5. Confirm: brutto_eur is LOWER than without §14a\n\n\
                **Regulatory basis:** §14a EnWG (Gesetz zur Änderung des EnWG 2022), BNetzA Festlegung BK6-22-300.\n\
                Pflicht ab 01.01.2024 für alle neuen steuerbaren Anlagen ≥3.7 kW.",
            ),
        ]
    }

    #[prompt(
        name = "eeg-billing",
        description = "Configure EEG/EINSPEISUNG billing for feed-in plants — Vergütung, Direktvermarktung, KWKG"
    )]
    async fn eeg_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I set up billing for a solar feed-in customer?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "EEG billing in billingd covers two categories:\n\n\
                **Category `EEG` — Vergütung (statutory feed-in tariff, §21 EEG)**\n\
                Used when the plant receives a fixed kWh rate for 20 years.\n\
                In tarifbd: set `eeg_verguetungssatz_ct_per_kwh` (e.g. 8.51 for ≤10 kWp solar 2024).\n\
                Optional additions:\n\
                - `eeg_marktpraemie_ct_per_kwh`: Gleitende Marktprämie (§20 EEG, Direktvermarktung)\n\
                - `eeg_managementpraemie_ct_per_kwh`: Managementprämie (0.4 ct/kWh ≤100 MW)\n\
                - `kwkg_zuschlag_ct_per_kwh`: KWKG Zuschlag for CHP plants (§7 KWKG 2023)\n\
                Input: `eeg_meter { einspeisung_kwh: 500 }`\n\
                Output: GUTSCHRIFT Rechnung (LF pays the plant owner)\n\n\
                **Category `EINSPEISUNG` — Direktvermarktung (market price, §20 EEG)**\n\
                Used when the Direktvermarkter sells to the spot market.\n\
                In tarifbd: set `marktwert_ct_per_kwh` (e.g. current EPEX monthly average).\n\
                Optional: `vermarktungsgebuehr_ct_per_kwh` (Direktvermarkter service fee deducted).\n\
                Input: `eeg_meter { einspeisung_kwh: 800 }`\n\
                Output: GUTSCHRIFT Rechnung (net settlement: Marktwert − Gebühr)\n\n\
                **Typical workflow:**\n\
                1. GET einsd /api/v1/anlagen/{tr_id} → verify Fördermodell and Vergütungssatz\n\
                2. GET einsd /api/v1/settlements → check if einsd already settled this month\n\
                3. GET edmd /api/v1/deliveries/{malo_id} → verify Einspeisung kWh available\n\
                4. POST /api/v1/billing/{malo_id}/preview with eeg_meter override\n\
                5. POST /api/v1/billing/{malo_id}/calculate → creates GUTSCHRIFT\n\
                6. accountingd auto-posts EEG_GUTSCHRIFT credit via de.billing.gutschrift.erstellt\n\n\
                ⚠ **Double-booking risk**: If einsd already emitted de.eeg.verguetung.berechnet\n\
                for this period, do NOT also run EEG billing in billingd — that would double-credit\n\
                the plant owner. Choose one path: einsd settlement OR billingd EEG billing, not both.",
            ),
        ]
    }

    #[prompt(
        name = "gas-billing",
        description = "Configure Gas billing — Brennwertkorrektur (§25 Nr. 4 MessEV), BEHG CO₂, H2-blend, L-Gas"
    )]
    async fn gas_billing_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I set up Gas billing with BEHG and Brennwertkorrektur?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "Gas billing in billingd has three input paths:\n\n\
                **Path 1 — Direct kWh_Hs (preferred for iMSys / MSCONS data)**\n\
                Supply `kwh_hs` directly in `gas_meter`. The Brennwertkorrektur position\n\
                appears in the invoice with quantity = 0 m³ (informational only, §25 Nr. 4 MessEV).\n\
                ```json\n{ \"gas_meter\": { \"kwh_hs\": 450.5 } }\n```\n\n\
                **Path 2 — m³ × Brennwert × Zustandszahl**\n\
                Supply `messung_qm3`, `brennwert_kwh_per_qm3`, `zustandszahl`.\n\
                billingd computes: kWh_Hs = m³ × Hs × Z (rounded to 3dp).\n\
                ```json\n{ \"gas_meter\": { \"messung_qm3\": 42.3, \"brennwert_kwh_per_qm3\": 10.68, \"zustandszahl\": 0.964 } }\n```\n\n\
                **H2-Blend Gas (hydrogen-blended natural gas)**\n\
                For H2-blended gas, set `gasqualitaet: \"H2_BLEND\"` in gas_meter.\n\
                IMPORTANT: The Brennwert used for billing is ALWAYS the measured value from\n\
                edmd/marktd (already reflects actual H2 blend ratio). Do NOT apply an\n\
                additional correction — that would double-correct. `gasqualitaet` is\n\
                a ZusatzAttribut annotation only (regulatory audit trail, DVGW G 260).\n\
                To auto-fetch gasqualitaet: billingd fetches from marktd if not supplied.\n\n\
                **Regulatory rates (configure in billingd.toml `[rates]`):**\n\
                | Rate | Default | Legal basis |\n\
                |---|---|---|\n\
                | Energiesteuer | 0.55 ct/kWh_Hs | §2 Nr. 3 EnergieStG |\n\
                | BEHG CO₂ | 1.109 ct/kWh_Hs | 55 EUR/t CO₂ × 0.20160 kg/kWh (2025) |\n\
                | MwSt | 19% | Standard; 7% for Fernwärme (§12 Abs.2 Nr.1 UStG) |\n\n\
                **Grid pass-through (from marktd PreisblattNetznutzung):**\n\
                Supply via `grid` override: `gas_nne_grundpreis_eur_per_year`, \n\
                `gas_nne_arbeitspreis_ct_per_kwh`, `gas_ka_ct_per_kwh`,\n\
                `gas_bilanzierungsumlage_ct_per_kwh` (GaBi Gas 2.1 (BK7-24-01-008) Bilanzierungsumlagekonten).\n\n\
                **L-Gas vs H-Gas:**\n\
                L-Gas has lower Brennwert (~9.5 kWh/m³ vs H-Gas ~10.55 kWh/m³).\n\
                Always use the measured Brennwert from the MSB/GNB, not the default.\n\
                The default fallback (10.55) is only for development/testing.",
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
    state.auth.authenticate(request, next).await
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
