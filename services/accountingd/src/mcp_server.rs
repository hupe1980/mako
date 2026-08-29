//! MCP server for `accountingd` — Customer Account Ledger.
//!
//! ## Tools
//! | Tool | Description |
//! |---|---|
//! | `get_balance` | Current open-items balance for a customer MaLo |
//! | `list_ledger` | Ledger movements (debit/credit) for a MaLo, with the opening balance the window starts from |
//! | `list_dunning` | Active dunning cases |
//! | `list_overdue` | All accounts with overdue invoices |
//! | `list_sepa_collections` | SEPA collections and their lifecycle (SUBMITTED → SETTLED/REJECTED/RETURNED/REVERSED) |
//! | `suggest_payment_match` | AI payment reconciliation: match CAMT.054 to open Rechnungen |

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
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AccountingdMcpState {
    pub pool: PgPool,
    /// The doubleentry ledger — authoritative balances, statements, open items.
    pub ledger: Arc<crate::ledger::PgLedger>,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// SEPA creditor identity from config — pain.008 needs all three.
    pub creditor_iban: Option<String>,
    pub creditor_name: Option<String>,
    pub creditor_id: Option<String>,
    /// The operator's own `Cdtr/PstlAdr` — mandatory from 2026-11-15.
    pub creditor_address: crate::sepa::AddressParts,
    /// Keys the IBAN lookup hash, so a payment can be resolved by the
    /// counterparty account the bank reported.
    pub iban_key: Option<[u8; 32]>,
    /// pain.008 schema version to emit (validated from config at startup).
    pub pain008_schema: crate::sepa::DirectDebitSchema,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MaloParams {
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LedgerParams {
    pub malo_id: String,
    pub lf_mp_id: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct JahresabschlussParams {
    /// 11-digit MaLo-ID for which to run the annual settlement.
    pub malo_id: String,
    /// Billing year (YYYY) — defaults to previous calendar year.
    pub year: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AbschlagCycleParams {
    /// Day of month to process (1–28). Defaults to today's day.
    /// Set explicitly to process a specific billing day (e.g. catchup runs).
    pub day_of_month: Option<i16>,
    /// Dry-run: if true, returns counts without posting ledger entries.
    pub dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AbgrenzungParams {
    /// Cutoff date for the period-end accrual (YYYY-MM-DD).
    /// Defaults to today. Use last day of month for Monatsabschluss.
    pub cutoff_date: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateAbschlagParams {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// New monthly advance payment in ct (× 10⁻² EUR). 0 disables the advance payment.
    pub abschlag_ct: i64,
    /// Day of month for SEPA direct debit (1–28). Defaults to current setting.
    pub billing_day: Option<i16>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportPaymentsParams {
    /// CAMT.054 payment entries. Each entry: `{ "malo_id": "...", "amount_ct": 5000, "value_date": "2026-01-15", "reference": "..." }`.
    pub entries: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SepaCollectionParams {
    /// Lifecycle filter: `SUBMITTED` | `SETTLED` | `REJECTED` | `RETURNED` | `REVERSED`.
    pub status: Option<String>,
    /// Restrict to one customer MaLo.
    pub malo_id: Option<String>,
    /// Maximum rows (default 100, capped at 1000).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OverdueParams {
    /// Minimum days overdue (default 1).
    pub days_overdue: Option<i64>,
}

/// Parameters for payment reconciliation.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SuggestPaymentMatchParams {
    /// Payment amount in 1/100 EUR cents (e.g. 12500 = 125.00 EUR).
    pub amount_ct: i64,
    /// Payment reference / Verwendungszweck from the bank statement.
    pub reference: String,
    /// Counterparty IBAN, when the bank reported one — the strongest evidence
    /// of who paid, and the rung that resolves without any guessing.
    pub iban: Option<String>,
    /// `EndToEndId`, when the bank echoed one back.
    pub end_to_end_id: Option<String>,
    /// Value date of the payment (YYYY-MM-DD).
    pub value_date: Option<String>,
    /// Fuzzy tolerance for the fallback rung: how many percent the open balance
    /// may deviate from the payment (default 2 %). Ignored when the reference
    /// identifies the account outright.
    pub tolerance_pct: Option<f64>,
}

#[derive(Debug, schemars::JsonSchema, serde::Deserialize)]
pub struct ManualBuchungParams {
    pub malo_id: String,
    /// Buchungsart. One of: RECHNUNG, ZAHLUNG, GUTSCHRIFT, EEG_GUTSCHRIFT, EEG_MARKTPRAEMIE,
    /// BANKRUECKLAST, MAHNGEBUEHR, VERZUGSZINSEN, ABSCHLAG, ABSCHLAG_VERRECHNUNG, JAHRESABSCHLUSS, KORREKTUR, STORNO.
    pub entry_type: String,
    /// Amount in ct (× 10⁻² EUR). Positive = debit; negative = credit.
    pub amount_ct: i64,
    /// External reference for audit trail (invoice number, CAMT ref, etc.).
    pub reference_id: Option<String>,
    /// Human-readable description for the Kontoauszug.
    pub description: Option<String>,
}

#[derive(Clone)]
pub struct AccountingdMcpHandler {
    state: Arc<AccountingdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<AccountingdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<AccountingdMcpHandler>,
}

#[tool_router]
impl AccountingdMcpHandler {
    fn new(state: Arc<AccountingdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "Get the current open-items balance (in 1/100 EUR cents) for a customer MaLo. Negative = credit; positive = amount owed.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_balance(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_account;
        match fetch_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        {
            Ok(Some(a)) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "balance_ct": a.balance_ct,
                "balance_eur": format!("{:.2}", a.balance_ct as f64 / 100.0),
                "abschlag_ct": a.abschlag_ct,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("account for {} not found", p.malo_id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List ledger movements (RECHNUNG, ZAHLUNG, GUTSCHRIFT, ABSCHLAG, etc.) for a MaLo, newest first, with the `opening_ct` the window starts from — `opening_ct` plus the movements is the newest line's `running_ct`, so a page truncated by `limit` still adds up.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_ledger(
        &self,
        Parameters(p): Parameters<LedgerParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_account, list_ledger};
        let acct = match fetch_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Err(McpError::invalid_params(
                    format!("account for {} not found", p.malo_id),
                    None,
                ));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };
        match list_ledger(
            &self.state.ledger,
            &acct.lf_mp_id,
            &acct.malo_id,
            doubleentry::BalanceQuery::all(),
            p.limit.unwrap_or(50),
        )
        .await
        {
            Ok(window) => ContentBlock::json(serde_json::to_value(window).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List active dunning cases (Mahnstufe 1-3). Returns cases with amount_due_ct, due_date, and stufe.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_dunning(
        &self,
        Parameters(_): Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_open_dunning;
        match list_open_dunning(&self.state.pool, &self.state.tenant, 100).await {
            Ok(cases) => ContentBlock::json(serde_json::to_value(cases).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List SEPA direct-debit collections and where each one stands. \
Each entry is one mandate collected in a pain.008 run, with its Mandatsreferenz (= EndToEndId), \
PmtInfId, amount and status: SUBMITTED (in flight), SETTLED (a bank booking or an accepted pain.002 \
confirmed it), REJECTED (pain.002 RJCT — the money never moved, so the receivable is still open and \
the mandate needs attention), RETURNED (a camt.054 Rückläufer after settlement) or REVERSED (the \
creditor gave it back via pain.007). \
Filter with `status` and/or `malo_id`. Use this to reconcile a collection run, to find the mandates \
a rejection batch hit, or to pick the entry_id an operator needs for POST /api/v1/sepa/reversals — \
issuing the reversal itself is an operator decision and is not exposed as a tool.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_sepa_collections(
        &self,
        Parameters(p): Parameters<SepaCollectionParams>,
    ) -> Result<CallToolResult, McpError> {
        match crate::pg::list_collection_entries(
            &self.state.pool,
            &self.state.tenant,
            p.status.as_deref(),
            p.malo_id.as_deref(),
            p.limit.unwrap_or(100).clamp(1, 1000),
        )
        .await
        {
            Ok(entries) => ContentBlock::json(serde_json::json!({
                "count":   entries.len(),
                "entries": entries,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all accounts with overdue invoices. Returns accounts with balance_ct > 0 and the oldest unpaid entry date.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue(
        &self,
        Parameters(p): Parameters<OverdueParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_overdue_accounts;
        match list_overdue_accounts(
            &self.state.pool,
            &self.state.tenant,
            1_i64,
            p.days_overdue.unwrap_or(100),
        )
        .await
        {
            Ok(accounts) => ContentBlock::json(serde_json::json!({
                "count": accounts.len(),
                "accounts": accounts,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
    #[tool(
        description = "Update the monthly advance payment (Abschlag) for a customer MaLo in ct. \
Call after the annual Jahresabschluss to re-calibrate based on actual consumption. \
Also sets the SEPA billing_day (day of month for direct debit).",
        annotations(idempotent_hint = true, open_world_hint = false)
    )]
    async fn update_abschlag(
        &self,
        Parameters(p): Parameters<UpdateAbschlagParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::UpdateAccountRequest;
        use crate::pg::{fetch_account, update_account};
        // Fetch to get lf_mp_id (required for update_account's composite key).
        let acct = match fetch_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Err(McpError::invalid_params(
                    format!("account for {} not found", p.malo_id),
                    None,
                ));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };
        match update_account(
            &self.state.pool,
            &p.malo_id,
            &acct.lf_mp_id,
            None,
            UpdateAccountRequest {
                iban: None,
                mandatsref: None,
                abschlag_ct: Some(p.abschlag_ct),
                billing_day: p.billing_day,
                address: Default::default(),
            },
        )
        .await
        {
            Ok(()) => ContentBlock::json(serde_json::json!({
                "malo_id": p.malo_id,
                "abschlag_ct": p.abschlag_ct,
                "billing_day": p.billing_day,
                "status": "updated",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Import CAMT.054 bank statement entries to match incoming payments against open items. \
Each entry requires: iban, amount_ct (positive = credit), value_date (YYYY-MM-DD), and reference. \
Returns count of matched and unmatched entries.",
        annotations(idempotent_hint = false, open_world_hint = false)
    )]
    async fn import_payments(
        &self,
        Parameters(p): Parameters<ImportPaymentsParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut matched = 0usize;
        let mut unmatched = 0usize;
        for entry in &p.entries {
            let malo_id = entry.get("malo_id").and_then(|v| v.as_str());
            let amount_ct = entry.get("amount_ct").and_then(|v| v.as_i64());
            let reference = entry
                .get("reference")
                .and_then(|v| v.as_str())
                .unwrap_or("CAMT.054 import");
            if let (Some(malo), Some(amt)) = (malo_id, amount_ct) {
                use crate::pg::{fetch_account, post_entry};
                if let Ok(Some(acct)) = fetch_account(
                    &self.state.pool,
                    malo,
                    &self.state.tenant,
                    &self.state.tenant,
                )
                .await
                {
                    let today = time::OffsetDateTime::now_utc().date();
                    let _ = post_entry(
                        &self.state.ledger,
                        &self.state.pool,
                        &self.state.tenant,
                        &acct.malo_id,
                        &acct.lf_mp_id,
                        "ZAHLUNG",
                        -amt,
                        &format!("mcp-camt:{reference}"),
                        None,
                        Some(reference),
                        today,
                        today,
                        Some("CAMT.054 Zahlungseingang"),
                        None,
                    )
                    .await;
                    matched += 1;
                } else {
                    unmatched += 1;
                }
            } else {
                unmatched += 1;
            }
        }
        ContentBlock::json(serde_json::json!({
            "matched": matched,
            "unmatched": unmatched,
            "total": p.entries.len(),
            "hint": if unmatched > 0 {
                "Some entries could not be matched. Check malo_id values against accountingd accounts."
            } else {
                "All entries matched successfully."
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Generate a SEPA pain.008 XML for all active mandates with a positive account balance. \
Returns the XML as a string ready for submission to the bank / payment service provider. \
Only generates for MaLo accounts that have an IBAN + signed mandate (sequence_type = FRST or RCUR).",
        annotations(
            read_only_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn run_sepa_collection(&self) -> Result<CallToolResult, McpError> {
        use crate::pg::list_accounts_with_mandates;
        use crate::sepa::build_pain_008;
        match list_accounts_with_mandates(&self.state.pool, &self.state.tenant).await {
            Ok(accounts) => {
                let refs: Vec<(&crate::pg::SepaMandateRow, i64)> = accounts
                    .iter()
                    .map(|(mandate, acct)| (mandate, acct.abschlag_ct))
                    .collect();
                let (Some(creditor_iban), Some(creditor_id)) = (
                    self.state.creditor_iban.as_deref(),
                    self.state.creditor_id.as_deref(),
                ) else {
                    return Err(McpError::internal_error(
                        "SEPA creditor identity incomplete — set creditor_iban and \
                         creditor_id (Gläubiger-ID) in accountingd.toml"
                            .to_owned(),
                        None,
                    ));
                };
                let creditor_name = self
                    .state
                    .creditor_name
                    .as_deref()
                    .unwrap_or(&self.state.tenant);
                let collection_date =
                    (time::OffsetDateTime::now_utc() + time::Duration::days(2)).date();
                let creditor = crate::sepa::CreditorIdentity {
                    iban: creditor_iban,
                    name: creditor_name,
                    creditor_id,
                    address: Some(&self.state.creditor_address),
                };
                match build_pain_008(
                    &creditor,
                    collection_date,
                    &refs,
                    self.state.pain008_schema,
                ) {
                    Ok(run) => ContentBlock::json(serde_json::json!({
                        "mandate_count": refs.len(),
                        "msg_id": run.msg_id,
                        "collection_date": collection_date.to_string(),
                        "entry_count": run.entry_count,
                        "total_ct": run.total_ct,
                        "groups": run.groups,
                        "pain_008_xml": &run.xml,
                        "hint": "Submit the XML to your bank / payment gateway — one message, one PmtInf group per SequenceType. Persist it with POST /api/v1/sepa/run if the file is actually going to the bank."
                    }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None)),
                    Err(e) => Err(McpError::internal_error(
                        format!("pain.008 generation failed: {e}"),
                        None,
                    )),
                }
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Preview the annual Jahresabschluss settlement for a customer MaLo. \
Nets the year's whole Kontokorrent movement — billing, the Abschlag pair, cash and \
Verzugsschaden — and returns settlement_ct (positive = Nachzahlung; negative = \
Erstattung/refund) plus the recommended new monthly Abschlag (annual billing ÷ 12, \
§40 Abs. 1 EnWG). Read-only: committing the settlement, booking the refund and \
recalibrating the Abschlag is POST /api/v1/jahresabschluss/{malo_id}, idempotent per \
(MaLo, year). \
Regulatory: §40 Abs. 1 EnWG — Abschlag must reflect actual estimated consumption.",
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn trigger_jahresabschluss(
        &self,
        Parameters(p): Parameters<JahresabschlussParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_account;
        let acct = match fetch_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Err(McpError::invalid_params(
                    format!("account for {} not found", p.malo_id),
                    None,
                ));
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };
        // The declared default — "previous calendar year" — was documented and
        // never applied: `p.year` only ever reached the JSON echo, so an
        // omitted year produced a settlement over no year at all.
        let year = p
            .year
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().year() - 1);
        // The same arithmetic the REST endpoint commits, over the same source.
        // This was a second implementation — the last 500 ledger rows, ABSCHLAG
        // and RECHNUNG only, with no year filter at all — so the preview
        // disagreed with the settlement it previews on any account carrying a
        // chargeback, a direct payment, or more than 500 movements.
        let sums = match self
            .state
            .ledger
            .year_kind_sums(&acct.lf_mp_id, &acct.malo_id, year)
            .await
        {
            Ok(s) => s,
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };
        let s = crate::handlers::JahresabschlussSums::from_kind_sums(&sums);
        let recommended_abschlag = (s.rechnung_sum.abs() / 12).max(0);
        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "year": year,
            "rechnung_sum_ct": s.rechnung_sum,
            "abschlag_net_ct": s.abschlag_sum,
            "zahlung_net_ct": s.zahlung_sum,
            "verzugsschaden_ct": s.verzugsschaden_sum,
            "sonstige_ct": s.sonstige_sum,
            "settlement_ct": s.settlement_ct,
            "settlement_eur": format!("{:.2}", s.settlement_ct as f64 / 100.0),
            "recommended_monthly_abschlag_ct": recommended_abschlag,
            "action": if s.settlement_ct > 0 {
                "NACHZAHLUNG: the open receivable stands; collect it via SEPA or dunning"
            } else if s.settlement_ct < 0 {
                "ERSTATTUNG: POST /api/v1/jahresabschluss/{malo_id} books the refund and pays it out"
            } else {
                "AUSGEGLICHEN: no adjustment needed"
            },
            "next_step": "POST /api/v1/jahresabschluss/{malo_id} commits it (idempotent per MaLo and year).",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Run the monthly Abschlagslauf (advance payment cycle) for all accounts \
due on the specified billing_day. Raises one Abschlagsforderung per account: a DEBIT on the \
customer Kontokorrent against Erhaltene Anzahlungen, plus a register row carrying the USt \
rate the advance was raised at (§ 14 Abs. 5 Satz 2 UStG). The advance is a demand, not a \
receipt — the money arrives later as a ZAHLUNG credit that clears this debit FIFO. \
Without automation, operators must trigger this manually each month — missed runs cause \
SEPA pre-notification failures. \
⚠ dry_run=true returns affected account count without posting entries.",
        annotations(
            read_only_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn run_abschlag_cycle(
        &self,
        Parameters(p): Parameters<AbschlagCycleParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{find_accounts_due, raise_abschlagsforderung};
        let dry_run = p.dry_run.unwrap_or(false);
        // Determine billing day (today or explicit)
        let today = time::OffsetDateTime::now_utc().date();
        let day = p.day_of_month.unwrap_or(today.day() as i16);
        let accounts = match find_accounts_due(&self.state.pool, &self.state.tenant, day).await {
            Ok(a) => a,
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };
        let mut processed = 0usize;
        let mut errors: Vec<String> = Vec::new();
        if !dry_run {
            let periode = today.replace_day(1).unwrap_or(today);
            for acct in &accounts {
                match raise_abschlagsforderung(
                    &self.state.ledger,
                    &self.state.pool,
                    &self.state.tenant,
                    acct,
                    periode,
                    today,
                    today,
                )
                .await
                {
                    Ok(_) => processed += 1,
                    Err(e) => errors.push(format!("{}: {e}", acct.malo_id)),
                }
            }
        }
        ContentBlock::json(serde_json::json!({
            "billing_day": day,
            "date": today.to_string(),
            "dry_run": dry_run,
            "accounts_due": accounts.len(),
            "processed": if dry_run { 0 } else { processed },
            "errors": errors,
            "next_step": "Run run_sepa_collection within N-5 bank business days to generate pain.008 XML.",
            "hint": if dry_run { "Set dry_run=false to actually raise the Abschlagsforderungen." } else { "Abschlagsforderungen raised. Check list_overdue for collection status." },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Compute period-end bilanzielle Abgrenzung (HGB §250 accruals) for ERP booking. \
Returns: pRAP (passive Rechnungsabgrenzungsposten = deferred revenue from advance payments), \
aRAP guidance (active RAP for unbilled energy — requires edmd data, computed by ERP), \
and the recommended ERP journal entries for Monatsabschluss / Jahresabschluss. \
pRAP is read from the ledger: the credit balance of Erhaltene Anzahlungen — advances demanded \
and not yet absorbed by a settling invoice. Not proxied from customer credit balances, which \
can equally be a Gutschrift or an overpayment. \
aRAP (unbilled) cannot be computed here — requires GET edmd /api/v1/billing-period per MaLo.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn compute_bilanzielle_abgrenzung(
        &self,
        Parameters(p): Parameters<AbgrenzungParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::compute_abgrenzung;
        let cutoff = p.cutoff_date.as_deref().unwrap_or("today").to_owned();
        let today = time::OffsetDateTime::now_utc().date();
        let (prap_ct, abschlag_total_ct, accounts_with_advance) = match compute_abgrenzung(
            &self.state.ledger,
            &self.state.pool,
            &self.state.tenant,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };

        ContentBlock::json(serde_json::json!({
            "cutoff_date": cutoff,
            "computed_at": today.to_string(),

            // ── Passive Rechnungsabgrenzungsposten (pRAP) ─────────────────
            // The open advance obligation: Erhaltene Anzahlungen, credited when
            // an Abschlag is demanded and debited when a settling invoice
            // absorbs it. § 266 Abs. 3 C.3 HGB.
            "prap_ct": prap_ct,
            "prap_eur": format!("{:.2}", prap_ct as f64 / 100.0),
            "prap_erp_entry": {
                "debit":  "Umsatzerlöse Energie (SKR03: 8400)",
                "credit": "Erhaltene Anzahlungen (SKR03: 1718 / SKR04: 3272)",
                "amount_eur": format!("{:.2}", prap_ct as f64 / 100.0),
                "explanation": "Advances demanded ahead of the supply they pay for; released when the settling invoice absorbs them."
            },

            // ── Aktive Rechnungsabgrenzungsposten (aRAP) ─────────────────
            // Energy delivered but not yet billed.
            // CANNOT be computed here — requires edmd Lastgang data.
            "arap_note": "aRAP (unbilled energy accrual) must be computed by ERP:                          for each MaLo call GET edmd /api/v1/billing-period/{malo_id}                          and compare arbeitsmenge_kwh × current_tariff_rate to last_invoice_amount.",
            "arap_erp_entry": {
                "debit":  "Forderungen aus Lieferungen und Leistungen (SKR03: 1400)",
                "credit": "Umsatzerlöse Energie (SKR03: 8400)",
                "amount": "calculate from edmd arbeitsmenge × tariff for unbilled period"
            },

            // ── Summary ───────────────────────────────────────────────────
            "accounts_with_advance": accounts_with_advance,
            "monthly_abschlag_total_ct": abschlag_total_ct,
            "monthly_abschlag_total_eur": format!("{:.2}", abschlag_total_ct as f64 / 100.0),

            "regulatory_basis": "HGB §250 (Rechnungsabgrenzungsposten), §252 (Realisationsprinzip).                                  Required for §243 HGB Jahresabschluss compliance.",
            "audit_note": "pRAP must be reversed at start of next accounting period.                           Document reversal dates and amounts in the Anlagenspiegel.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    // ── AI Payment Reconciliation (B14 / L7) ─────────────────────────────────

    #[tool(
        description = "Reconcile an incoming bank transfer against open receivables. \
Runs the same resolution ladder the camt importer uses, strongest evidence first: an exact \
Mandatsreferenz, EndToEndId or MaLo-ID found as a whole token in the payment reference resolves \
the account outright (confidence EXACT — no amount guessing involved). Only when nothing in the \
reference identifies the payer does it fall back to ranking accounts by how close their open \
balance is to the payment (confidence HIGH/MEDIUM/LOW). \
Pass `iban` when the bank reported the counterparty account — that is the strongest evidence of all. \
For each candidate: account_id, malo_id, open_balance_ct, matched_by, similarity_score. \
Confirm with post_manual_booking or by importing the bank file.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn suggest_payment_match(
        &self,
        Parameters(p): Parameters<SuggestPaymentMatchParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;

        // ── The exact rung ───────────────────────────────────────────────────
        //
        // If the reference names a customer, there is nothing to rank: the
        // payer said who they are. The previous version of this tool went
        // straight to fuzzy amount matching and scored a "reference substring"
        // signal whose inputs were hardcoded to `None`, so two of its three
        // reference branches were unreachable and the score it reported was
        // amount proximity wearing a reference-matching label.
        let iban_hash = p
            .iban
            .as_deref()
            .map(|iban| crate::ledger::iban_hash(self.state.iban_key.as_ref(), iban));
        if let Ok(Some(hit)) = crate::pg::resolve_account_for_payment(
            &self.state.pool,
            &self.state.tenant,
            crate::pg::PaymentClues {
                iban_hash: iban_hash.as_deref(),
                end_to_end_id: p.end_to_end_id.as_deref(),
                remittance: Some(&p.reference),
            },
        )
        .await
        {
            let balance_ct: i64 = sqlx::query_scalar(
                "SELECT balance_ct FROM accounts WHERE account_id = $1 AND tenant = $2",
            )
            .bind(hit.account_id)
            .bind(&self.state.tenant)
            .fetch_optional(&self.state.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
            return ContentBlock::json(serde_json::json!({
                "payment_amount_ct":  p.amount_ct,
                "payment_amount_eur": crate::sepa::ct_to_eur_str(p.amount_ct),
                "payment_reference":  p.reference,
                "candidates_count":   1,
                "candidates": [{
                    "account_id":       hit.account_id,
                    "malo_id":          hit.malo_id,
                    "lf_mp_id":         hit.lf_mp_id,
                    "open_balance_ct":  balance_ct,
                    "open_balance_eur": crate::sepa::ct_to_eur_str(balance_ct),
                    "matched_by":       hit.matched_by,
                    "confidence":       "EXACT",
                    "residual_ct":      balance_ct - p.amount_ct,
                }],
                "note": "The payment identifies its own account — no amount guessing was involved. \
                         `residual_ct` is what stays open after booking it (0 = settles the balance exactly).",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None));
        }

        // ── The fuzzy rung ───────────────────────────────────────────────────
        //
        // Nothing in the payment says who sent it, so all that is left is "who
        // owes roughly this much". Ranked, never auto-booked.
        let tol_pct = p.tolerance_pct.unwrap_or(2.0);
        let tol_factor = 1.0 + tol_pct / 100.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let lo = (p.amount_ct as f64 / tol_factor) as i64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let hi = (p.amount_ct as f64 * tol_factor) as i64;

        // Candidate matching is on the balance cache (the authoritative balance
        // is the doubleentry ledger; this cache is refreshed from it on every
        // post), so a set-based scan stays cheap.
        let rows = sqlx::query(
            r"SELECT a.account_id, a.malo_id, a.lf_mp_id, a.balance_ct
              FROM accounts a
              WHERE a.tenant = $1
                AND a.balance_ct BETWEEN $2 AND $3
              ORDER BY ABS(a.balance_ct - $4) ASC
              LIMIT 10",
        )
        .bind(&self.state.tenant)
        .bind(lo)
        .bind(hi)
        .bind(p.amount_ct)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let candidates: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let account_id: uuid::Uuid = r.try_get("account_id").unwrap_or_default();
                let malo_id: String = r.try_get("malo_id").unwrap_or_default();
                let lf_mp_id: String = r.try_get("lf_mp_id").unwrap_or_default();
                let balance_ct: i64 = r.try_get("balance_ct").unwrap_or_default();

                // Amount proximity is the only signal here — the reference
                // already failed to identify anyone. Scoring it as if it were
                // two independent signals overstated the confidence.
                #[allow(clippy::cast_precision_loss)]
                let closeness = if p.amount_ct == 0 {
                    0.0_f64
                } else {
                    1.0 - (balance_ct - p.amount_ct).unsigned_abs() as f64
                        / p.amount_ct.unsigned_abs() as f64
                };
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let score = (closeness.clamp(0.0, 1.0) * 100.0).round() as u32;
                // No fuzzy match is ever HIGH: an amount that happens to line up
                // is not evidence of who paid. The exact rung above is the only
                // one safe to auto-book.
                let confidence = if score >= 99 { "MEDIUM" } else { "LOW" };

                serde_json::json!({
                    "account_id":       account_id,
                    "malo_id":          malo_id,
                    "lf_mp_id":         lf_mp_id,
                    "open_balance_ct":  balance_ct,
                    "open_balance_eur": crate::sepa::ct_to_eur_str(balance_ct),
                    "matched_by":       "amount_proximity",
                    "similarity_score": score,
                    "confidence":       confidence,
                    "residual_ct":      balance_ct - p.amount_ct,
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "payment_amount_ct":  p.amount_ct,
            "payment_amount_eur": crate::sepa::ct_to_eur_str(p.amount_ct),
            "payment_reference":  p.reference,
            "tolerance_pct":      tol_pct,
            "candidates_count":   candidates.len(),
            "candidates":         candidates,
            "note": "Nothing in the reference identified the payer, so these are ranked by amount \
                     proximity alone — a coincidence, not evidence. Review before booking; ask the \
                     customer to quote their Mandatsreferenz or MaLo-ID to make the next one exact.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Post a manual ledger entry (Buchung) to a customer account. \
Use for: ZAHLUNG (incoming bank transfer), BANKRUECKLAST (returned SEPA direct debit), \
KORREKTUR (operator adjustment), GUTSCHRIFT (one-off credit). \
The entry immediately updates the account balance. \
Allowed entry_type: RECHNUNG, ZAHLUNG, GUTSCHRIFT, EEG_GUTSCHRIFT, EEG_MARKTPRAEMIE, \
BANKRUECKLAST, MAHNGEBUEHR, VERZUGSZINSEN, ABSCHLAG, ABSCHLAG_VERRECHNUNG, JAHRESABSCHLUSS, KORREKTUR, STORNO. \
amount_ct: positive = debit (increases balance); negative = credit (reduces balance). \
⚠ This is an authorised operator action — always document via reference_id and description.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn post_manual_booking(
        &self,
        Parameters(p): Parameters<ManualBuchungParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{post_entry, upsert_account};
        use time::OffsetDateTime;

        upsert_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let today = OffsetDateTime::now_utc().date();
        // reference_id (when given) makes the post idempotent; else a fresh key.
        let idempotency = p
            .reference_id
            .clone()
            .unwrap_or_else(|| format!("mcp-manual:{}", uuid::Uuid::new_v4()));
        let entry_id = post_entry(
            &self.state.ledger,
            &self.state.pool,
            &self.state.tenant,
            &p.malo_id,
            &self.state.tenant,
            &p.entry_type,
            p.amount_ct,
            &idempotency,
            None,
            p.reference_id.as_deref(),
            today,
            today,
            p.description.as_deref(),
            None,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        ContentBlock::json(serde_json::json!({
            "entry_id": entry_id,
            "malo_id": p.malo_id,
            "entry_type": p.entry_type,
            "amount_ct": p.amount_ct,
            "amount_eur": crate::handlers::format_ct_as_eur(p.amount_ct),
            "booking_date": today.to_string(),
            "committed": true,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl AccountingdMcpHandler {
    #[prompt(
        name = "check-customer-account",
        description = "Step-by-step: review a customer account and plan collection action"
    )]
    async fn check_customer_account_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Review customer account status and determine collection action.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `get_balance` to check current open-items balance.\n                 2. Use `list_ledger` to see recent RECHNUNG/ZAHLUNG/GUTSCHRIFT/ABSCHLAG history.\n                 3. If balance > 0 (overdue): check `list_dunning` for active dunning cases.\n                 4. For missing payments: `import_payments` with CAMT.054 bank entries to match.\n                 5. Monthly Abschlagslauf: `run_abschlag_cycle` raises the Abschlagsforderungen (day=billing_day).\n                 6. Monthly SEPA: `run_sepa_collection` → pain.008 XML (send N-5 bank days before due).\n                 7. After Jahresabschluss: `trigger_jahresabschluss` → review → `update_abschlag`.\n                 8. Period-end HGB accruals: `compute_bilanzielle_abgrenzung` → ERP pRAP/aRAP booking.\n                 9. Mahnstufe 3: de.accounting.sperrauftrag → sperrd → IFTSTA 21039 to NB.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for AccountingdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("accountingd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "accountingd MCP — Customer Account Ledger (Massenkontokorrent, LF role).\n\
                 Running debit/credit ledger per MaLo; SEPA direct debit; Mahnwesen Mahnstufe 1-3.\n\n\
                 **Order-to-Cash integration:**\n\
                 - Inbound: de.billing.rechnung.erstellt → RECHNUNG debit entry\n\
                 - Inbound: CAMT.054 bank statement → use `import_payments` for ZAHLUNG credit\n\
                 - Outbound: pain.008 XML → use `run_sepa_collection` for monthly Abschlag collection\n\
                 - Dunning: `list_dunning` → escalate → de.accounting.sperrauftrag → sperrd\n\n\
                 **⚠ EEG double-booking prevention (§20-21 EEG 2023):**\n\
                 If billingd includes an EEG Gutschrift as a negative Rechnungsposition,\n\
                 the resulting debit from de.billing.rechnung.erstellt is already net of EEG.\n\
                 Do NOT post a separate credit for de.eeg.verguetung.berechnet for the same\n\
                 customer/period — that path is only for Direktvermarkter standalone settlement.\n\n\
                 Use `get_balance` for open-items balance.\n\
                 Use `update_abschlag` after Jahresabschluss to recalibrate monthly advance.",
            )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AccountingdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<AccountingdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = AccountingdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
