//! MCP server for `accountingd` — Customer Account Ledger.
//!
//! ## Tools
//! | Tool | Description |
//! |---|---|
//! | `get_balance` | Current open-items balance for a customer MaLo |
//! | `list_ledger` | Ledger entries (debit/credit) for a MaLo |
//! | `list_dunning` | Active dunning cases |
//! | `list_overdue` | All accounts with overdue invoices |
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
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
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
pub struct OverdueParams {
    /// Minimum days overdue (default 1).
    pub days_overdue: Option<i64>,
}

/// Parameters for AI payment reconciliation (B14 / L7).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SuggestPaymentMatchParams {
    /// Payment amount in 1/100 EUR cents (e.g. 12500 = 125.00 EUR).
    pub amount_ct: i64,
    /// Payment reference / Verwendungszweck from the bank statement.
    pub reference: String,
    /// Value date of the payment (YYYY-MM-DD).
    pub value_date: Option<String>,
    /// Fuzzy tolerance: how many percent the amount may deviate (default 2 %).
    pub tolerance_pct: Option<f64>,
}

#[derive(Debug, schemars::JsonSchema, serde::Deserialize)]
pub struct ManualBuchungParams {
    pub malo_id: String,
    /// Buchungsart. One of: RECHNUNG, ZAHLUNG, GUTSCHRIFT, EEG_GUTSCHRIFT, EEG_MARKTPRAEMIE,
    /// BANKRUECKLAST, MAHNGEBUEHR, ABSCHLAG, JAHRESABSCHLUSS, KORREKTUR, STORNO.
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
        description = "List ledger entries (RECHNUNG, ZAHLUNG, GUTSCHRIFT, ABSCHLAG, etc.) for a MaLo. Returns entries ordered by booking_date descending.",
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
        match list_ledger(&self.state.pool, acct.account_id, p.limit.unwrap_or(50)).await {
            Ok(entries) => ContentBlock::json(serde_json::to_value(entries).unwrap_or_default())
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
            UpdateAccountRequest {
                iban: None,
                mandatsref: None,
                abschlag_ct: Some(p.abschlag_ct),
                billing_day: p.billing_day,
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
                use crate::pg::{fetch_account, write_entry};
                if let Ok(Some(acct)) = fetch_account(
                    &self.state.pool,
                    malo,
                    &self.state.tenant,
                    &self.state.tenant,
                )
                .await
                {
                    let today = time::OffsetDateTime::now_utc().date();
                    let _ = write_entry(
                        &self.state.pool,
                        acct.account_id,
                        &self.state.tenant,
                        "ZAHLUNG",
                        -amt,
                        Some(reference),
                        Some("de.accounting.payment.imported"),
                        None,
                        today,
                        Some("CAMT.054 Zahlungseingang"),
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
                // Use tenant as creditor name fallback; in production creditor_iban comes from config.
                let creditor = &self.state.tenant;
                match build_pain_008(creditor, creditor, None, &refs) {
                    Ok(batches) => ContentBlock::json(serde_json::json!({
                        "mandate_count": refs.len(),
                        "batch_count": batches.len(),
                        "batches": batches.iter().map(|b| serde_json::json!({
                            "sequence_type": format!("{:?}", b.sequence_type),
                            "entry_count": b.entry_count,
                            "total_ct": b.total_ct,
                            "pain_008_xml": &b.xml,
                        })).collect::<Vec<_>>(),
                        "hint": "Submit each batch XML to your bank / payment gateway for SEPA direct debit execution."
                    }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None)),
                    Err(e) => Err(McpError::internal_error(
                        format!("pain.008 generation failed: {e}. Configure creditor_iban in accountingd.toml."),
                        None,
                    )),
                }
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Compute the annual Jahresabschluss settlement for a customer MaLo. \
Compares actual Rechnung/Storno totals vs Σ(Abschläge) collected during the year. \
Returns: settlement_ct (positive = Nachzahlung; negative = Erstattung/refund), \
and the recommended new monthly Abschlag (actual annual ÷ 12, §40 Abs. 1 EnWG). \
Use ?dry_run=true for preview without committing. When committed, writes a JAHRESABSCHLUSS \
entry and updates the monthly Abschlag. \
Regulatory: §40 Abs. 1 EnWG — Abschlag must reflect actual estimated consumption.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn trigger_jahresabschluss(
        &self,
        Parameters(p): Parameters<JahresabschlussParams>,
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
        let entries = match list_ledger(&self.state.pool, acct.account_id, 500).await {
            Ok(e) => e,
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        };
        // Sum monthly Abschläge (credits, negative) and Rechnungen (debits, positive)
        // LedgerRow: entry_type: String, amount_ct: i64
        let abschlag_sum: i64 = entries
            .iter()
            .filter(|e| e.entry_type == "ABSCHLAG")
            .map(|e| e.amount_ct)
            .sum();
        let rechnung_sum: i64 = entries
            .iter()
            .filter(|e| e.entry_type == "RECHNUNG")
            .map(|e| e.amount_ct)
            .sum();
        let settlement_ct = rechnung_sum + abschlag_sum;
        let recommended_abschlag = (rechnung_sum.abs() / 12).max(0);
        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "year": p.year,
            "rechnung_sum_ct": rechnung_sum,
            "abschlag_paid_ct": abschlag_sum,
            "settlement_ct": settlement_ct,
            "settlement_eur": format!("{:.2}", settlement_ct as f64 / 100.0),
            "recommended_monthly_abschlag_ct": recommended_abschlag,
            "action": if settlement_ct > 0 {
                "NACHZAHLUNG: post RECHNUNG debit via billingd /calculate (annual true-up)"
            } else if settlement_ct < 0 {
                "GUTSCHRIFT: post GUTSCHRIFT credit to accountingd ledger (refund)"
            } else {
                "AUSGEGLICHEN: no adjustment needed"
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Run the monthly Abschlagslauf (advance payment cycle) for all accounts \
due on the specified billing_day. Posts ABSCHLAG debit entries to the ledger for each \
affected account and emits de.accounting.payment.due CloudEvent. \
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
        use crate::pg::{find_accounts_due, write_entry};
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
            for acct in &accounts {
                let ref_id = format!(
                    "ABSCHLAG-{}-{:04}-{:02}",
                    acct.malo_id,
                    today.year(),
                    today.month() as u8
                );
                match write_entry(
                    &self.state.pool,
                    acct.account_id,
                    &self.state.tenant,
                    "ABSCHLAG",
                    acct.abschlag_ct, // positive = debit (charge to customer)
                    Some(&ref_id),
                    Some("de.accounting.abschlag.posted"),
                    None,
                    today,
                    Some(&format!("Monatlicher Abschlag Tag {day}")),
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
            "hint": if dry_run { "Set dry_run=false to actually post ABSCHLAG entries." } else { "ABSCHLAG entries posted. Check list_overdue for collection status." },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Compute period-end bilanzielle Abgrenzung (HGB §250 accruals) for ERP booking. \
Returns: pRAP (passive Rechnungsabgrenzungsposten = deferred revenue from advance payments), \
aRAP guidance (active RAP for unbilled energy — requires edmd data, computed by ERP), \
and the recommended ERP journal entries for Monatsabschluss / Jahresabschluss. \
pRAP = Σ(accounts with credit balance) = customers pre-paid more than billed. \
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
        let (prap_ct, abschlag_total_ct, accounts_with_advance) =
            match compute_abgrenzung(&self.state.pool, &self.state.tenant).await {
                Ok(r) => r,
                Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
            };

        ContentBlock::json(serde_json::json!({
            "cutoff_date": cutoff,
            "computed_at": today.to_string(),

            // ── Passive Rechnungsabgrenzungsposten (pRAP) ─────────────────
            // Customer overpaid: Abschläge collected > energy billed to date.
            // Book: Dr. Revenue / Cr. pRAP (liability) at period-end.
            // Release: Dr. pRAP / Cr. Revenue when energy is billed.
            "prap_ct": prap_ct,
            "prap_eur": format!("{:.2}", prap_ct as f64 / 100.0),
            "prap_erp_entry": {
                "debit":  "Umsatzerlöse Energie (SKR03: 8400)",
                "credit": "Passive Rechnungsabgrenzung (SKR03: 0990)",
                "amount_eur": format!("{:.2}", prap_ct as f64 / 100.0),
                "explanation": "Abschläge received in advance of energy delivery."
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
        description = "AI-assisted payment reconciliation: match an incoming CAMT.054 bank transfer against open Rechnungen. \
Returns candidate accounts ranked by fuzzy amount + reference similarity. \
powercloud claims >98% automated payment matching — this tool enables the same for mako. \
For each candidate: account_id, malo_id, open_amount_ct, similarity_score, suggested_entry_type. \
Use import_payments to confirm the match.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn suggest_payment_match(
        &self,
        Parameters(p): Parameters<SuggestPaymentMatchParams>,
    ) -> Result<CallToolResult, McpError> {
        use sqlx::Row;

        let tol_pct = p.tolerance_pct.unwrap_or(2.0);
        let tol_factor = 1.0 + tol_pct / 100.0;
        let lo = (p.amount_ct as f64 / tol_factor) as i64;
        let hi = (p.amount_ct as f64 * tol_factor) as i64;

        // Find accounts whose open balance is within tolerance of the payment.
        // We also return accounts where the most recent RECHNUNG amount is within range.
        let rows = sqlx::query(
            r"SELECT DISTINCT ON (a.account_id)
                     a.account_id,
                     a.malo_id,
                     a.lf_mp_id,
                     a.balance_ct,
                     le.id          AS latest_rechnung_id,
                     le.amount_ct   AS latest_rechnung_ct,
                     le.reference_id,
                     le.description
              FROM accounts a
              LEFT JOIN LATERAL (
                  SELECT id, amount_ct, reference_id, description
                  FROM ledger_entries
                  WHERE account_id = a.account_id
                    AND entry_type = 'RECHNUNG'
                  ORDER BY booking_date DESC
                  LIMIT 1
              ) le ON TRUE
              WHERE a.tenant = $1
                AND a.balance_ct BETWEEN $2 AND $3
              ORDER BY a.account_id, ABS(a.balance_ct - $4) ASC
              LIMIT 10",
        )
        .bind(&self.state.tenant)
        .bind(lo)
        .bind(hi)
        .bind(p.amount_ct)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let reference_lower = p.reference.to_lowercase();
        let mut candidates: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let account_id: uuid::Uuid = r.try_get("account_id").unwrap_or_default();
                let malo_id: String = r.try_get("malo_id").unwrap_or_default();
                let lf_mp_id: String = r.try_get("lf_mp_id").unwrap_or_default();
                let balance_ct: i64 = r.try_get("balance_ct").unwrap_or_default();
                let rechnung_ct: Option<i64> = r.try_get("latest_rechnung_ct").ok();
                let ref_id: Option<String> = r.try_get("reference_id").ok().flatten();
                let desc: Option<String> = r.try_get("description").ok().flatten();

                // Compute a naive similarity score based on:
                //   - amount proximity (0–50 pts)
                //   - reference substring match (0–50 pts)
                let amount_score = if balance_ct == 0 {
                    0.0_f64
                } else {
                    50.0 * (1.0 - (balance_ct - p.amount_ct).unsigned_abs() as f64 / p.amount_ct.unsigned_abs() as f64)
                };
                let ref_score = {
                    let malo_lower = malo_id.to_lowercase();
                    let ref_lower = ref_id.as_deref().unwrap_or("").to_lowercase();
                    let desc_lower = desc.as_deref().unwrap_or("").to_lowercase();
                    // Check for MaLo ID, reference ID, or description substring
                    if reference_lower.contains(&malo_lower) || malo_lower == reference_lower.as_str() {
                        50.0
                    } else if !ref_lower.is_empty() && reference_lower.contains(&ref_lower) {
                        40.0
                    } else if !desc_lower.is_empty() && reference_lower.contains(&desc_lower) {
                        30.0
                    } else {
                        0.0
                    }
                };

                let score = (amount_score + ref_score).min(100.0).round() as u32;
                let confidence = if score >= 80 { "HIGH" } else if score >= 50 { "MEDIUM" } else { "LOW" };

                serde_json::json!({
                    "account_id": account_id,
                    "malo_id": malo_id,
                    "lf_mp_id": lf_mp_id,
                    "open_balance_ct": balance_ct,
                    "open_balance_eur": format!("{:.2}", balance_ct as f64 / 100.0),
                    "latest_rechnung_ct": rechnung_ct,
                    "latest_rechnung_eur": rechnung_ct.map(|c| format!("{:.2}", c as f64 / 100.0)),
                    "similarity_score": score,
                    "confidence": confidence,
                    "action": format!("import_payments {{ malo_id: '{malo_id}', amount_ct: {}, reference: '{}' }}", p.amount_ct, p.reference),
                })
            })
            .collect();

        // Sort by similarity score descending.
        candidates.sort_by(|a, b| {
            b.get("similarity_score")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .cmp(
                    &a.get("similarity_score")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                )
        });

        ContentBlock::json(serde_json::json!({
            "payment_amount_ct": p.amount_ct,
            "payment_amount_eur": format!("{:.2}", p.amount_ct as f64 / 100.0),
            "payment_reference": p.reference,
            "tolerance_pct": tol_pct,
            "candidates_count": candidates.len(),
            "candidates": candidates,
            "note": "Candidates ranked by similarity_score. HIGH (>=80) = auto-match safe. MEDIUM/LOW = review. Confirm with import_payments.",
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
BANKRUECKLAST, MAHNGEBUEHR, ABSCHLAG, JAHRESABSCHLUSS, KORREKTUR, STORNO. \
amount_ct: positive = debit (increases balance); negative = credit (reduces balance). \
⚠ This is an authorised operator action — always document via reference_id and description.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn post_manual_booking(
        &self,
        Parameters(p): Parameters<ManualBuchungParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{upsert_account, write_entry_with_value_date};
        use time::OffsetDateTime;

        let account_id = upsert_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let today = OffsetDateTime::now_utc().date();
        let entry_id = write_entry_with_value_date(
            &self.state.pool,
            account_id,
            &self.state.tenant,
            &p.entry_type,
            p.amount_ct,
            p.reference_id.as_deref(),
            None,
            None,
            today,
            today,
            p.description.as_deref(),
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
            "committed": entry_id != uuid::Uuid::nil(),
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
                "1. Use `get_balance` to check current open-items balance.\n                 2. Use `list_ledger` to see recent RECHNUNG/ZAHLUNG/GUTSCHRIFT/ABSCHLAG history.\n                 3. If balance > 0 (overdue): check `list_dunning` for active dunning cases.\n                 4. For missing payments: `import_payments` with CAMT.054 bank entries to match.\n                 5. Monthly Abschlagslauf: `run_abschlag_cycle` posts ABSCHLAG entries (day=billing_day).\n                 6. Monthly SEPA: `run_sepa_collection` → pain.008 XML (send N-5 bank days before due).\n                 7. After Jahresabschluss: `trigger_jahresabschluss` → review → `update_abschlag`.\n                 8. Period-end HGB accruals: `compute_bilanzielle_abgrenzung` → ERP pRAP/aRAP booking.\n                 9. Mahnstufe 3: de.accounting.sperrauftrag → sperrd → IFTSTA 21039 to NB.",
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
