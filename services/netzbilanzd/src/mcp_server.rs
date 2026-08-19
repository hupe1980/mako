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

use crate::pg::{self, DraftFilter};

/// Shared state for the MCP tools.
#[derive(Clone)]
pub struct NetzbilanzMcpState {
    /// Database pool.
    pub pool: PgPool,
    /// The tenant every query is scoped to.
    pub tenant: String,
    /// API-key / OIDC / dev-mode authentication.
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

/// The MCP handler.
#[derive(Clone)]
pub struct NetzbilanzMcpHandler {
    state: Arc<NetzbilanzMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<NetzbilanzMcpHandler>,
    #[allow(dead_code)]
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
                 carries `sparte`, which selects StromNEV §21 or GasNEV §14, decides whether \
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
                   BDEW series — § 13 Abs. 3 StromNZV, no per-operator variant)\n\n\
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
                 path, `billing_period` means the monthly aggregate was used because no Lastgang \
                 existed — for a 15-minute activation that is wrong by three orders of \
                 magnitude and should be replaced with a verified `dispatch_kwh_override`.\n\n\
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
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for NetzbilanzMcpHandler {
    fn get_info(&self) -> ServerInfo {
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
             Redispatch 2.0 Kostenblatt and the §13a Abs. 2 Vergütung.\n\n\
             **This surface is read-only.** Dispatching an invoice sends EDIFACT to a \
             counterparty and starts a payment obligation, and the only way back is a \
             Stornorechnung — so settling, dispatching, rejecting and correcting all live on \
             the REST API, where the action is attributable to an operator. Read here; act there.\n\n\
             ## Tools (8)\n\
             - `list_drafts` — filter by MaLo, party, PID, Sparte, status, Rechnungsart, verdict\n\
             - `get_draft` — one invoice in full: BO4E document, settlement input, findings, warnings\n\
             - `list_disputed` — REMADV Abweisungen with their ERC codes\n\
             - `list_undispatched` — drafts past their dispatch window\n\
             - `list_corrections` — the Storno / Korrektur chain\n\
             - `get_billing_summary` — monthly totals by PID, Sparte, status, Rechnungsart\n\
             - `list_pending_kostenblatt` — Redispatch cost sheets awaiting the 15th\n\
             - `list_kostenblatt_gaps` — activations registered but never quantified\n\n\
             ## Prompts (6)\n\
             `nb-invoic-overview` · `run-nne-billing` · `mmm-monthly-run` · \
             `investigate-dispute` · `ggv-nne-billing` · `redispatch-monthly-submit`",
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
