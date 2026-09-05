//! MCP server for `billingd` — Multi-Product Billing Engine.
//!
//! ## Tools (11) — all read-only
//!
//! | Tool | Description |
//! |---|---|
//! | `list_billing_records` | List billing records for a MaLo |
//! | `get_billing_record` | Get a single billing record with full Rechnung BO4E |
//! | `preview_billing` | Dry-run billing calculation (no persist, no CloudEvent) |
//! | `get_xrechnung` | Fetch the XRechnung / CII XML rendered from the stored EN 16931 model |
//! | `check_billing_anomaly` | Rolling 3-invoice baseline vs the latest invoice |
//! | `list_vpp_settlements` | List §41e VPP dispatch settlements |
//! | `list_corrections` | List Korrekturrechnung / Stornorechnung records (§ 147 AO / GoBD) |
//! | `list_product_categories` | Describe all 13 billing categories and their required fields |
//! | `get_billing_summary` | Aggregate billing stats per MaLo or LF, counted once |
//! | `validate_tariff_config` | Pre-flight validation: §41a iMSys guard, legacy flags, §42 Energiemix |
//! | `explain_invoice_position` | Explain how a billing position was calculated (PositionTrace audit) |
//!
//! ## Why nothing here issues an invoice
//!
//! There is deliberately no `calculate_billing` tool. Issuing a Rechnung is a
//! legally binding act: it lands in `billing_records` under § 147 AO, dispatches
//! `de.billing.rechnung.erstellt` to the ledger and the ERP, and can only be
//! undone by a Stornorechnung. Model output is untrusted input everywhere else
//! in this platform, and that rule does not stop at a well-phrased tool
//! description. An agent investigates and explains; a human or a scheduled run
//! with an OIDC identity bills.
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
//! | `gas-billing` | Configure Gas billing — Brennwertkorrektur, BEHG CO₂, H-Gas / L-Gas |

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

/// State the MCP tools run against.
///
/// The read tools query the pool directly and `preview_billing` calls the same
/// `compute_preview` the HTTP endpoint does. Nothing loops back over HTTP: the
/// service requires OIDC on its own API, so a loopback carried no token and
/// answered 401 in every configuration except the dev one — the tools worked
/// exactly where they mattered least.
#[derive(Clone)]
pub struct BillingdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    pub deps: Arc<crate::clients::BillingDeps>,
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
    /// NB MP-ID (§41 Abs. 1 Nr. 5 EnWG). Resolved from marktd when omitted.
    pub nb_mp_id: Option<String>,
    /// Billing period start (YYYY-MM-DD).
    pub period_from: String,
    /// Billing period end (YYYY-MM-DD).
    pub period_to: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateTariffParams {
    /// TariffInput JSON string to validate (same format as productd product JSONB).
    pub tariff_json: String,
    /// Metering mode to test against (SLP, RLM, IMSYS). Relevant for §41a check.
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
        use crate::pg::{RecordFilter, list_billing_records};
        match list_billing_records(
            &self.state.pool,
            &self.state.tenant,
            &RecordFilter {
                malo_id: params.malo_id.as_deref(),
                lf_mp_id: params.lf_mp_id.as_deref(),
                outcome: params.outcome.as_deref(),
                limit: params.limit.unwrap_or(20).clamp(1, 100),
                ..Default::default()
            },
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
        match fetch_billing_record(&self.state.pool, &self.state.tenant, id).await {
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
        description = "Dry-run billing preview for a MaLo: the Rechnung positions, totals and \
engine warnings that a real run would produce, without persisting a record or emitting a \
CloudEvent. Runs the same pipeline as POST /api/v1/billing/{malo_id}/preview, in process.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn preview_billing(
        &self,
        Parameters(params): Parameters<PreviewParams>,
    ) -> Result<CallToolResult, McpError> {
        let req = crate::handlers::CalculateRequest {
            lf_mp_id: params.lf_mp_id,
            nb_mp_id: params.nb_mp_id,
            period_from: params.period_from,
            period_to: params.period_to,
            ..Default::default()
        };
        // The same coded error the HTTP endpoint answers with, over MCP's
        // transport: a caller's mistake is `invalid_params`, an outage is
        // `internal_error`, and the body carries the same `error.code`.
        let preview = crate::handlers::compute_preview(&self.state.deps, &params.malo_id, &req)
            .await
            .map_err(|e| {
                let body = e.body().to_string();
                if e.status().is_client_error() {
                    McpError::invalid_params(body, None)
                } else {
                    McpError::internal_error(body, None)
                }
            })?;
        ContentBlock::json(crate::handlers::preview_json(&params.malo_id, &preview))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Fetch the EN 16931 CII XML for a billing record UUID, rendered from the \
stored semantic model. A retail invoice declares plain EN 16931 in BT-24; only a document put \
through POST /api/v1/billing/{id}/submit-b2g declares the XRechnung CIUS, which additionally \
needs a Leitweg-ID (BT-10) no household has. B2G e-invoicing to federal authorities has been \
mandatory since 27.11.2020 (§4a EGovG i.V.m. ERechV); the 2027/2028 dates belong to the separate \
B2B mandate in §14 UStG. Returns the raw XML string, not BASE64.",
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
        match fetch_billing_record(&self.state.pool, &self.state.tenant, id).await {
            Ok(Some(row)) => {
                // Render CII from the stored EN 16931 model (per-line VAT intact)
                // via `en16931-formats` — the same source the HTTP endpoint uses.
                let Some(model) = row
                    .en16931_json
                    .as_ref()
                    .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
                else {
                    return Err(McpError::invalid_params(
                        format!("record {id} has no EN 16931 model — re-run the calculation"),
                        None,
                    ));
                };
                let xml = crate::einvoice::render_cii(&model);
                ContentBlock::json(serde_json::json!({
                    "billing_record_id": id,
                    "xrechnung_xml": xml,
                    "specification_id": model.specification_id,
                    "standard": "EN 16931 / CII (XRechnung 3.0 only when BT-24 says so)",
                    "note": "For a B2G submission use POST /api/v1/billing/{id}/submit-b2g — it \
completes the buyer, stamps the Leitweg-ID and proves the document against the XRechnung CIUS \
before anything is sent to ZRE / OZG-RE."
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
        description = "Statistical anomaly check: compare latest invoice against the rolling average for a MaLo. \
NOTE: every calculated invoice is also scored inline by the deterministic risk gate — \
`list_billing_records`/`get_billing_record` expose `risk_score` (0-100), `risk_band` \
(AUTO_RELEASED/SAMPLE/REVIEW/HELD) and the coded `risk_findings`; HELD records are not \
dispatched until released via POST /api/v1/billing/{id}/release. \
Returns deviation percentage, rolling average, and is_anomaly flag. \
Flags invoices where |deviation| > threshold_pct (default 20%). The baseline counts live \
originals and consolidated documents only — reversed invoices and the per-MaLo children of a \
bundle are excluded, so it is the same population the risk gate scored against. \
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
        match check_billing_anomaly(
            &self.state.pool,
            &self.state.tenant,
            &p.malo_id,
            &p.lf_mp_id,
            threshold,
        )
        .await
        {
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

    // ── §41e VPP dispatch settlement ──────────────────────────────────────────

    #[tool(
        description = "List VPP (Virtual Power Plant) aggregation settlement records for a VPP portfolio. \
Returns billing records with category=VPP showing dispatch events, total flexibility kWh, and Einsatzkosten. \
CloudEvent de.vpp.settlement.berechnet is emitted per settlement. § 41e EnWG / Art. 17 RL (EU) 2019/944.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_vpp_settlements(
        &self,
        Parameters(p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{RecordFilter, list_billing_records};
        let lf_mp_id = p.get("lf_mp_id").and_then(|v| v.as_str());
        let limit = p
            .get("limit")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(20)
            .clamp(1, 100);
        // VPP records are stored under the vpp_id in the `malo_id` column.
        let vpp_malo = p.get("vpp_id").and_then(|v| v.as_str());
        // The category filter runs in the query, not over the rows it returned:
        // a portfolio whose latest `limit` documents are all ordinary invoices
        // would otherwise answer "no settlements" while its settlements sit one
        // page further down.
        match list_billing_records(
            &self.state.pool,
            &self.state.tenant,
            &RecordFilter {
                malo_id: vpp_malo,
                lf_mp_id,
                category: Some("VPP"),
                limit,
                ..Default::default()
            },
        )
        .await
        {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "records": rows,
                "hint": "POST /api/v1/billing/vpp/{vpp_id} to settle dispatch events. Supply a \
`tx_id` per event so the manual path shares `vpp_dispatch_ledger` with the auto-settlement \
webhook and cannot pay the same flexibility twice. CloudEvent de.vpp.settlement.berechnet is \
emitted to the ERP."
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List Korrekturrechnung and Stornorechnung records (§ 147 AO / GoBD audit trail, 8 years). \
Returns all correction/reversal billing records for a MaLo. \
Each record includes original_record_id, correction_reason, and whether it negates the original.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_corrections(
        &self,
        Parameters(params): Parameters<ListRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{RecordFilter, list_billing_records};
        // `is_correction` is a query predicate, not a post-filter. Fetching a
        // page and keeping the corrections out of it meant a MaLo with fifty
        // ordinary invoices and three Stornos answered "no corrections" — an
        // audit tool (§ 147 AO) reporting that a correction chain does not
        // exist when it does.
        match list_billing_records(
            &self.state.pool,
            &self.state.tenant,
            &RecordFilter {
                malo_id: params.malo_id.as_deref(),
                lf_mp_id: params.lf_mp_id.as_deref(),
                is_correction: Some(true),
                limit: params.limit.unwrap_or(50).clamp(1, 200),
                ..Default::default()
            },
        )
        .await
        {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "records": rows,
                "note": "Use POST /api/v1/billing/{id}/correction to create a new correction. \
Each Storno takes its own number from the tenant's `ST` series and releases the original's \
period for re-billing."
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all 13 billing product categories with their required and optional \
TariffInput fields — the `Product` enum billingd dispatches on. Use this to discover what fields \
to set in productd for a given product type. Covers STROM (incl. §41a dynamic), GAS, WAERME, \
WASSER, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG \
and SHARING. A bundle is not a category: productd decomposes it into component product codes \
before billing.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_product_categories(
        &self,
        Parameters(_p): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let categories = serde_json::json!([
            { "category": "STROM", "description": "Standard electricity — Eintarif/Zweitarif/Mehrtarif", "required": ["arbeitspreis_ct_per_kwh"], "optional": ["grundpreis_ct_per_day", "arbeitspreis_ht_ct_per_kwh", "arbeitspreis_nt_ct_per_kwh", "dynamic_epex", "dynamic_epex_floor_ct_kwh"], "regulatory": "§41a EnWG for dynamic; §3 StromStG levy included" },
            { "category": "GAS", "description": "Natural gas with Brennwertkorrektur and CO₂ levies", "required": ["gas_arbeitspreis_ct_per_kwh_hs"], "optional": ["gas_grundpreis_ct_per_day", "energiesteuer_gas_ct_per_kwh_override", "behg_gas_ct_per_kwh_override"], "regulatory": "§25 Nr. 4 MessEV (Brennwertkorrektur), §2 EnergieStG, BEHG" },
            { "category": "WAERME", "description": "Fernwärme — Grundpreis, Arbeitspreis, Leistungspreis", "required": ["waerme_arbeitspreis_ct_per_kwh"], "optional": ["waerme_grundpreis_eur_per_month", "waerme_leistungspreis_eur_per_kw_month", "mwst_rate_override"], "regulatory": "District heating is standard-rated (19%); the 7% gas/Fernwärme window was §28 Abs. 5/6 UStG (2022–31.03.2024, expired) — set mwst_rate_override for a period inside it" },
            { "category": "SOLAR", "description": "Solar self-consumption, Mieterstrom §21 Abs. 3 EEG, §42b EnWG GGV community solar", "required": ["solar_arbeitspreis_ct_per_kwh"], "optional": ["grundversorgung_arbeitspreis_ct_per_kwh", "gemeinschaft_rabatt_ct_per_kwh", "stromsteuer_tarif"], "regulatory": "§12 Abs. 3 UStG is the 0 % rate on the PV **hardware supply** — it never applies to the electricity. Consumption is 19 %; a feed-in Gutschrift is 0 % only when the operator is a §19 UStG Kleinunternehmer (declared on the plant, not here). The Mieterstromzuschlag (§21 Abs. 3 EEG) is the plant operator's claim against the Netzbetreiber, settled by einsd — never a surcharge on the tenant; set grundversorgung_arbeitspreis_ct_per_kwh and the §42a Abs. 4 EnWG 90 % cap is enforced. Stromsteuer defaults to the §9 Abs. 1 Nr. 3 StromStG Kleinanlage-Befreiung (≤2 MW, räumlicher Zusammenhang) and the ground is stated on the invoice; a supply that does not qualify sets stromsteuer_tarif={\"art\":\"REGEL\"}" },
            { "category": "EEG", "description": "EEG feed-in Vergütung — credit note to plant operator (LF role, contractual)", "required": ["eeg_verguetungssatz_ct_per_kwh"], "optional": ["eeg_marktpraemie_ct_per_kwh", "eeg_managementpraemie_ct_per_kwh", "kwkg_zuschlag_ct_per_kwh"], "meter": "eeg_meter.einspeisung_kwh, eeg_meter.kwh_during_negative_epex (§51 contractual suspension)", "regulatory": "§21 EEG Vergütung; §20 EEG Marktprämie; §51 EEG Negativpreisregel (contractual for LF)" },
            { "category": "EINSPEISUNG", "description": "Direktvermarktung settlement — Marktwert minus Vermarktungsgebühr", "required": ["marktwert_ct_per_kwh"], "optional": ["vermarktungsgebuehr_ct_per_kwh"], "regulatory": "§20 EEG Direktvermarktung; Direktvermarkter bears negative-price risk (§51 does NOT apply)" },
            { "category": "WAERMEPUMPE", "description": "Heat pump electricity with §14a EnWG Steuerungsrabatt Modul 1/3", "required": ["arbeitspreis_ct_per_kwh"], "optional": ["sect14a_modul1_pauschale_eur_per_year", "sect14a_steuerungsentschaedigung_eur_per_kw_year"], "meter": "meter.steuerung_stunden (Modul 3 only — Modul 1 is a flat annual amount and needs no Leistung)", "regulatory": "§14a EnWG; BNetzA BK6-22-300 (27.11.2023), in force 01.01.2024 — applies to steuerbare Verbrauchseinrichtungen above 4.2 kW Netzanschlussleistung" },
            { "category": "WALLBOX", "description": "EV charging box with §14a EnWG Steuerungsrabatt Modul 1/3 — same as WAERMEPUMPE", "required": ["arbeitspreis_ct_per_kwh"], "optional": ["sect14a_modul1_pauschale_eur_per_year", "sect14a_steuerungsentschaedigung_eur_per_kw_year"], "regulatory": "§14a EnWG same as WAERMEPUMPE" },
            { "category": "HEMS", "description": "Home Energy Management System — platform subscription + optimization events", "required": ["hems_subscription_eur_per_month"], "optional": ["hems_optimization_event_eur", "hems_readout_event_eur"], "meter": "hems_meter.months, hems_meter.optimization_events, hems_meter.readout_events" },
            { "category": "EMOBILITY", "description": "EV charging CPO/EMSP — service fee + kWh + session fees", "required": ["emobility_service_fee_eur or emobility_kwh_price_ct"], "optional": ["emobility_session_fee_eur", "emobility_roaming_fee_eur"], "meter": "emobility_meter.months, emobility_meter.kwh_charged, emobility_meter.sessions" },
            { "category": "ENERGIEDIENSTLEISTUNG", "description": "Energy services (MSB, maintenance, analytics) — flat fee + event count", "required": ["service_fee_eur or service_event_price_eur"], "optional": [], "meter": "service_meter.months, service_meter.event_count" },
            { "category": "SHARING", "description": "§42c EnWG Energiegemeinschaft — community energy sharing credit against the residual supply", "required": ["sharing_credit_ct_per_kwh"], "optional": ["sharing_description"], "meter": "electricity meter (residual supply, from edmd) + energy_share (the community allocation)", "regulatory": "§42c EnWG, in force 01.06.2026; BNetzA Mitteilung Nr. 73 places it inside the existing supplier/Bilanzkreis model" },
            { "category": "WASSER", "description": "Municipal Trinkwasser and gesplittete Abwassergebühr", "required": ["wasser product configuration"], "meter": "wasser_meter.verbrauch_m3 (+ sealed m² for Niederschlagswasser)", "regulatory": "Trinkwasser 7 % (§12 Abs. 2 Nr. 1 i.V.m. Anlage 2 Nr. 34 UStG); Abwasser as a hoheitliche Gebühr carries no USt" }
        ]);
        ContentBlock::json(categories)
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Validate a Product configuration for regulatory compliance before billing. Runs the engine's own validation pass plus three static checks: the §41a Abs. 1 EnWG iMSys requirement for dynamic tariffs, the legacy Stromsteuer-exemption flag, and the §42 EnWG Energiemix disclosure. Returns warnings and errors without triggering a calculation.",
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

        // Build a context with the requested metering mode for §41a checks.
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
            period: energy_billing::BillingPeriod::new(
                date!(2026 - 01 - 01),
                date!(2026 - 01 - 31),
            )
            .expect("static valid period"),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: rates,
            ..Default::default()
        };

        let quantities = Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: rust_decimal::dec!(100),
                metering_mode: metering_mode.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let warnings = engine.validate(&ctx, &quantities);

        // Additional static checks independent of engine.validate():
        let mut extra_checks: Vec<serde_json::Value> = Vec::new();

        // Check §41a Abs. 1: dynamic_epex requires iMSys
        let is_dynamic = matches!(&tariff, Product::Strom(e) if e.dynamic_epex)
            || matches!(&tariff, Product::Waermepumpe(c) if c.base.dynamic_epex)
            || matches!(&tariff, Product::Wallbox(c) if c.base.dynamic_epex);
        if is_dynamic && metering_mode != MeteringMode::Imsys {
            extra_checks.push(serde_json::json!({
                "code": "SECT41A_IMSYS_REQUIRED",
                "severity": "Error",
                "message": "§41a Abs. 1 EnWG: dynamic_epex=true requires MeteringMode::Imsys. Set metering_mode to IMSYS or switch to a fixed-price tariff."
            }));
        }

        // § 9 StromStG: a Befreiung zero-rates the levy on the supplier's own
        // invoice and therefore on the supplier's own Stromsteueranmeldung. It
        // is only lawful against the customer's Erlaubnis (§ 9 Abs. 4), so an
        // operator setting one deserves to be reminded what it rests on — and
        // told which grounds are *not* exemptions at all.
        let befreiung = match &tariff {
            Product::Strom(e) => Some(e.stromsteuer_tarif),
            Product::Waermepumpe(c) | Product::Wallbox(c) => Some(c.base.stromsteuer_tarif),
            _ => None,
        }
        .filter(|t| t.is_befreit());
        if befreiung.is_some() {
            extra_checks.push(serde_json::json!({
                "code": "STROMSTEUER_BEFREIUNG_ERLAUBNIS",
                "severity": "Warning",
                "message": "stromsteuer_tarif=BEFREIUNG bills no Stromsteuer at all. § 9 Abs. 4 StromStG requires the customer's Erlaubnis on file. A produzierendes Gewerbe is NOT exempt — § 9b StromStG is a Steuerentlastung the customer claims from the Hauptzollamt after being invoiced in full; declare it in steuerentlastungen instead."
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

        let record = fetch_billing_record(&self.state.pool, &self.state.tenant, record_id)
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
                        // Every per-position fact BO4E does not model rides in
                        // `zusatzAttribute` under the `mako:` namespace. Bare
                        // `_additional` keys (`rechtlicheGrundlage`,
                        // `kategorie`) are the counterparty's slot, and reading
                        // one here is the collision the namespace prevents.
                        "rechtsgrundlage": mako_attr(pos, "mako:rechtliche_grundlage"),
                        "kategorie": mako_attr(pos, "mako:positionskategorie"),
                        "positionstyp": mako_attr(pos, "mako:positionstyp"),
                        "trace": mako_attr(pos, "mako:calculation_trace"),
                        "note": "trace carries formula, input_quantity, input_unit_price_eur, gross_eur, and regulatory_basis for audit reconstruction."
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
        description = "Aggregate billing statistics for a MaLo or an LF: record count, total \
net and gross, average gross per 30 days of supply, and a breakdown by category. Aggregated in \
the database over the whole history — not a capped page — and counted once: Storno rows and the \
per-MaLo children of a Sammelrechnung are excluded so the bundle and its parts are not both \
counted.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_billing_summary(
        &self,
        Parameters(params): Parameters<ListRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        match crate::pg::billing_summary(
            &self.state.pool,
            &self.state.tenant,
            params.malo_id.as_deref(),
            params.lf_mp_id.as_deref(),
        )
        .await
        {
            Ok(summary) => ContentBlock::json(summary)
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
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
                 **2. Tariff Assignment**\n                 POST vertragd /api/v1/vertraege/{id}/tarifwechsel { komp_id, new_product_code, wirksamkeit, initiator }\n                 → a valid-time slice; which product a MaLo is on is a contract fact, not a catalogue one\n                 `initiator` is required and names the act: `\"LIEFERANT\"` where the supplier exercises a\n                 reserved right to change the price — § 41 Abs. 5 Satz 1 EnWG notice, Satz 4\n                 Sonderkündigungsrecht, and `preise[]` must state the Umfang for a future date where\n                 vertragd renders the notice itself (the lines are the document's content); where the\n                 CloudEvent *is* the notice the ERP composes from its own price sheets and they are optional;\n                 `\"KUNDE\"` where the customer asked for the tariff, which is an agreed change,\n                 confirmed rather than announced.\n
                 **3. Meter Data (edmd)**\n                 MSCONS readings arrive via makod EDIFACT pipeline automatically.\n                 Verify: edmd GET /api/v1/billing-period/{malo_id}\n
                 **4. Invoice Generation (billingd)**\n                 POST /api/v1/billing/{malo_id}/calculate { lf_mp_id, nb_mp_id, period_from, period_to }\n                 → productd → edmd → marktd (NNE) → §14a discount → EEG credit\n                 → Rechnung BO4E persisted; CloudEvent de.billing.rechnung.erstellt\n                 Use `list_billing_records` to verify; `get_xrechnung` for B2G XML.\n
                 **5. Account Posting (accountingd)**\n                 de.billing.rechnung.erstellt → accountingd debit entry (Rechnungsbetrag)\n                 Check balance: accountingd `get_balance`\n                 Monthly SEPA: accountingd `run_sepa_collection` → pain.008 XML\n                 Payment receipt: accountingd `import_payments` (CAMT.054)\n
                 **6. Dunning & Collections (if overdue)**\n                 `list_overdue` → Mahnstufe 1 (reminder) → 2 (fee) → 3 (Sperrauftrag)\n                 Mahnstufe 3 → de.accounting.sperrauftrag → sperrd → IFTSTA 21039 to NB\n
                 **Annual Jahresabschluss:**\n                 billingd annual settlement → accountingd `trigger_jahresabschluss` → `update_abschlag` with new rate.\n                 ⚠ EEG note: EEG Gutschrift in Rechnung is already netted in the debit amount. \n                 Do NOT separately book de.eeg.verguetung.berechnet credits for the same period.\n\n                 **Monthly Abschlagslauf (automated advance payment cycle):**\n                 accountingd `run_abschlag_cycle` on each billing_day → raises the Abschlagsforderungen (debits).\n                 Then `run_sepa_collection` N-5 bank days before due date → generates pain.008 XML.\n                 Import bank statement: `import_payments` (CAMT.054) to match SEPA returns.\n\n                 **Bilanzielle Abgrenzung (HGB §250 — period-end accruals):**\n                 At Monats-/Jahresabschluss: use accountingd `compute_bilanzielle_abgrenzung`.\n                 pRAP (passive): advance payments collected > energy billed → book as liability.\n                 aRAP (active): unbilled energy → edmd GET /billing-period/{malo_id} × tariff.\n                 ERP journals: Dr. Umsatzerlöse / Cr. pRAP 0990; Dr. FLL 1400 / Cr. Erlöse."
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
                "To preview a billing invoice, use POST /api/v1/billing/{malo_id}/preview.\n\
                 Required: lf_mp_id, period_from, period_to.\n\
                 Optional: nb_mp_id (resolved from marktd when absent), tariff (override from \
productd), meter (override from edmd), grid (override from marktd).\n\n\
                 The preview runs the same pipeline as /calculate and stores nothing: no record, \
no CloudEvent, and no number consumed from the §14 UStG series. The response carries every \
Rechnungsposition, the netto/brutto totals and the engine warnings.",
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
                "For a §41a dynamic tariff (every supplier must offer one since 01.01.2025, \
owed to customers who have an iMSys):\n\
                 1. Verify the product in productd has dynamic_epex: true\n\
                 2. Verify EPEX day-ahead prices are imported for the whole billing period:\n\
                    PUT /api/v1/epex-prices/{date} in productd (15-min MTUs)\n\
                 3. Verify the customer has 15-min interval data in edmd, on the same\n\
                    endpoint billingd bills from — the /lastgang export can show data\n\
                    this returns none of (wrong direction, or all non-billable):\n\
                    GET /api/v1/energy/{malo_id}?direction=BEZUG&from=...&to=...\n\
                 4. Verify the meter is an iMSys — §41a Abs. 1 EnWG requires one, and billingd\n\
                    refuses the run with SECT41A_IMSYS_REQUIRED for MeteringMode Slp or Rlm\n\
                 5. Run a preview: POST /api/v1/billing/{malo_id}/preview\n\n\
                 **There is no fallback.** A dynamic tariff is billed per market time unit or \
not at all: a period with no Lastgang is refused (SECT41A_NO_LASTGANG) and intervals with \
consumption but no EPEX price hard-block the run (SECT41A_MISSING_EPEX_PRICES). Billing the \
static arbeitspreis instead would charge a price the contract does not contain, and dropping \
unpriced intervals would silently under-bill.",
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
                **Modul 1 — pauschale Reduzierung (a flat amount per year)**\n\
                In productd: set `sect14a_modul1_pauschale_eur_per_year` in the WAERMEPUMPE/WALLBOX product.\n\
                BK6-22-300 sets the amount as `80 EUR + 3 750 kWh × Arbeitspreis × 0,2`, so it has\n\
                **no per-kW component**. Do not read the name as a rate: 150 here means 150 EUR for a\n\
                whole year, not 150 EUR per kW.\n\
                Requires: nothing further. `spitzenleistung_kw` is **not** needed — that is exactly what\n\
                lets a household heat pump on an SLP meter have this module at all.\n\
                Formula: amount_eur_per_year × billed_years → one credit position, quantity 1, unit „Jahr\".\n\
                Example: 150 EUR/year over a 31-day period → 150 × 31/365 ≈ 12,74 EUR credit (vor MwSt).\n\n\
                **Modul 2 — Reduzierter NNE-Arbeitspreis (ct/kWh)**\n\
                In productd: set `sect14a_modul2_nne_reduktion_ct_per_kwh` in the product.\n\
                Billed by `ControllableLoadProvider` as a per-kWh credit tagged 'sect14a_modul2'.\n\
                Requires separate metering of the steuerbare Verbrauchseinrichtung, and is\n\
                **mutually exclusive with Modul 3** — both re-price the network usage, and\n\
                configuring them together is an Error-severity finding (MODUL2_AND_MODUL3).\n\n\
                **Modul 3 — Load-shedding compensation (Laststeuerung hours × kW)**\n\
                In productd: set `sect14a_steuerungsentschaedigung_eur_per_kw_year` in the product.\n\
                Requires: steuerung_stunden in the meter reading (from agentd/processd).\n\
                Formula: kW × rate × (steuerung_stunden / 8760) → credit position.\n\n\
                **Setup steps:**\n\
                1. GET productd /api/v1/products → find WAERMEPUMPE or WALLBOX product\n\
                2. PUT productd /api/v1/products/{id} add sect14a_modul1_pauschale_eur_per_year\n\
                3. POST /api/v1/billing/{malo_id}/preview — verify Steuerungsrabatt position appears\n\
                4. Check: position tagged 'sect14a_modul1' (pauschale Reduzierung) or\n\
                   'sect14a_modul2' (Arbeitspreisreduzierung) → negative credit amount\n\
                5. Confirm: brutto_eur is LOWER than without §14a\n\n\
                **Regulatory basis:** §14a EnWG, BNetzA Festlegung BK6-22-300 (27.11.2023),\n\
                in force 01.01.2024. It applies to steuerbare Verbrauchseinrichtungen in\n\
                Niederspannung with a Netzanschlussleistung **above 4.2 kW** — heat pumps,\n\
                non-public charging points, air conditioning, storage. Several devices of the\n\
                same category behind one connection count as one for the threshold, and the\n\
                network operator dims rather than disconnects (4.2 kW floor). Legacy §14a\n\
                installations migrate by 01.01.2029.",
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
                In productd: set `eeg_verguetungssatz_ct_per_kwh` (e.g. 8.51 for ≤10 kWp solar 2024).\n\
                Optional additions:\n\
                - `eeg_marktpraemie_ct_per_kwh`: Gleitende Marktprämie (§20 EEG, Direktvermarktung)\n\
                - `eeg_managementpraemie_ct_per_kwh`: Managementprämie (0.4 ct/kWh ≤100 MW)\n\
                - `kwkg_zuschlag_ct_per_kwh`: KWKG Zuschlag for CHP plants (§7 KWKG 2023)\n\
                Input: `eeg_meter { einspeisung_kwh: 500 }`\n\
                Output: GUTSCHRIFT Rechnung (LF pays the plant owner)\n\n\
                **Category `EINSPEISUNG` — Direktvermarktung (market price, §20 EEG)**\n\
                Used when the Direktvermarkter sells to the spot market.\n\
                In productd: set `marktwert_ct_per_kwh` (e.g. current EPEX monthly average).\n\
                Optional: `vermarktungsgebuehr_ct_per_kwh` (Direktvermarkter service fee deducted).\n\
                Input: `eeg_meter { einspeisung_kwh: 800 }`\n\
                Output: GUTSCHRIFT Rechnung (net settlement: Marktwert − Gebühr)\n\n\
                **Typical workflow:**\n\
                1. GET einsd /api/v1/anlagen/{tr_id} → verify Fördermodell and Vergütungssatz\n\
                2. GET einsd /api/v1/settlements → check if einsd already settled this month\n\
                3. GET edmd /api/v1/deliveries/{malo_id} → verify Einspeisung kWh available\n\
                4. POST /api/v1/billing/{malo_id}/preview with eeg_meter override\n\
                5. POST /api/v1/billing/{malo_id}/calculate → creates a credit Rechnung (negative)\n\
                6. accountingd books the credit from de.billing.rechnung.erstellt (is_correction)\n\n\
                ⚠ **Double-booking risk**: If einsd already emitted de.eeg.verguetung.berechnet\n\
                for this period, do NOT also run EEG billing in billingd — that would double-credit\n\
                the plant owner. Choose one path: einsd settlement OR billingd EEG billing, not both.",
            ),
        ]
    }

    #[prompt(
        name = "gas-billing",
        description = "Configure Gas billing — Brennwertkorrektur (§25 Nr. 4 MessEV), BEHG CO₂, H-Gas / L-Gas"
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
                ```json\n{ \"gas_meter\": { \"kwh_hs\": \"450.5\" } }\n```\n\n\
                **Path 2 — m³ × Brennwert × Zustandszahl**\n\
                Supply `messung_qm3`, `brennwert_kwh_per_qm3`, `zustandszahl`.\n\
                billingd computes: kWh_Hs = m³ × Hs × Z (rounded to 3dp).\n\
                ```json\n{ \"gas_meter\": { \"messung_qm3\": \"42.3\", \"brennwert_kwh_per_qm3\": \"10.68\", \"zustandszahl\": \"0.964\" } }\n```\n\n\
                **`gasqualitaet` (including hydrogen-blended gas)**\n\
                BO4E v202607 defines exactly two values, `H_GAS` and `L_GAS`, and those\n\
                are the only ones billingd accepts — there is no `H2_BLEND` wire value\n\
                yet, and inventing one would persist rows that go wrong when the\n\
                DVGW/BNetzA wave standardises a different spelling.\n\
                This does NOT block billing an H2-blended supply: the Brennwert used is\n\
                ALWAYS the measured value from edmd/marktd, which already reflects the\n\
                actual blend ratio. Do NOT apply an additional correction — that would\n\
                double-correct. `gasqualitaet` is a ZusatzAttribut annotation only\n\
                (regulatory audit trail, DVGW G 260), and an unrecognised value is\n\
                dropped rather than annotated.\n\
                To auto-fetch gasqualitaet: billingd fetches from marktd if not supplied.\n\n\
                **Regulatory rates (configure in billingd.toml `[rates]`):**\n\
                | Rate | Default | Legal basis |\n\
                |---|---|---|\n\
                | Energiesteuer | 0.55 ct/kWh_Hs | §2 Abs. 3 Nr. 4 EnergieStG |\n\
                | BEHG CO₂ | 1.17906516 ct/kWh_Hs | 65 EUR/t CO₂ × 0.18139464 kg/kWh_Hs (2026 default); \
                since 07/2026 the price is EEX-auctioned inside the §10 Abs. 2 BEHG corridor, so \
                billingd prefers productd's `nehs_prices` series over this table |\n\
                | MwSt | 19% | Gas and Fernwärme are standard-rated. The 7% window was \
                §28 Abs. 5/6 UStG (01.10.2022–31.03.2024) and has expired; billingd resolves \
                it automatically for periods inside it and refuses periods that straddle the \
                Stichtag |\n\n\
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
             EN 16931 CII at GET /api/v1/billing/{id}/xrechnung, PEPPOL UBL at …/ubl, the \
             ZUGFeRD PDF at …/pdf.\n\
             Nothing here issues a document: every tool is read-only, and a Rechnung is a \
             legally binding act that needs a human or a scheduled run with an OIDC identity.\n\n\
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

/// Read one `mako:`-namespaced `ZusatzAttribut` off a serialised BO4E value.
///
/// Returns `Value::Null` when absent, which is what `json!` would have produced
/// for a missing key — so a position that carries none renders the same shape.
fn mako_attr(value: &serde_json::Value, name: &str) -> serde_json::Value {
    value
        .get("zusatzAttribute")
        .and_then(|z| z.as_array())
        .and_then(|attrs| {
            attrs
                .iter()
                .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(name))
        })
        .and_then(|a| a.get("wert"))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}
