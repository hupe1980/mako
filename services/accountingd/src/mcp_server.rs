//! MCP server for `accountingd` — Customer Account Ledger.
//!
//! ## Tools
//! | Tool | Description |
//! |---|---|
//! | `get_balance` | Current open-items balance for a customer MaLo |
//! | `get_vorauszahlung` | The BO4E `Vorauszahlung` (Abschlag) of a customer MaLo — what `GET /api/v1/accounts/{malo_id}/vorauszahlung` serves |
//! | `list_ledger` | Ledger movements (debit/credit) for a MaLo, with the opening balance the window starts from |
//! | `list_dunning` | Active dunning cases |
//! | `list_overdue` | Accounts with an open balance, largest first |
//! | `list_sepa_collections` | SEPA collections and their lifecycle (SUBMITTED → SETTLED/REJECTED/RETURNED/REVERSED) |
//! | `list_eeg_payouts` | The EEG payout orders the Netzbetreiber ran, by plant and billing month |
//! | `list_unmatched_payments` | Bank lines no account claimed, waiting for an operator |
//! | `list_credit_payouts` | Guthaben from Schlussrechnungen owed back, and which are overdue |
//! | `suggest_payment_match` | AI payment reconciliation: match CAMT.054 to open Rechnungen |

use axum::{
    Router,
    middleware::{self, Next},
    response::IntoResponse as _,
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::StreamableHttpServerConfig,
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
    /// The operator's own `Cdtr/PstlAdr`.
    pub creditor_address: crate::sepa::AddressParts,
    /// Keys the IBAN lookup hash, so a payment can be resolved by the
    /// counterparty account the bank reported.
    pub iban_key: Option<[u8; 32]>,
    /// pain.008 schema version to emit (validated from config at startup).
    pub pain008_schema: crate::sepa::DirectDebitSchema,
    /// The posture report the runner built at boot, served by `get_policy`.
    pub policy: Arc<mako_service::policy::PolicyReport>,
}

/// A tool that takes no arguments.
///
/// **Not `serde_json::Value`.** Its JSON Schema is the empty schema — no
/// `type` — and the MCP specification requires a tool's `inputSchema` to have
/// root type `object`. `rmcp` asserts that while building the router, so a
/// single argument-less tool declared as `Parameters<serde_json::Value>`
/// panics the whole service at startup. Nothing else catches it: the router is
/// built in `main`, and no test starts the binary.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NoParams {}

/// Parameters of `get_parked_booking`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParkedKeyParams {
    /// The parked booking's key, as `list_parked_bookings` names it.
    pub key: String,
}

/// Parameters of `list_parked_bookings`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParkedParams {
    /// Max results (default 50, clamped to 1–200).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MaloParams {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// The LF MP-ID the account is held under. Defaults to the tenant, as on
    /// the REST route.
    pub lf_mp_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VorauszahlungParams {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// The LF MP-ID the account is held under. Defaults to the tenant, as on
    /// the REST route.
    pub lf_mp_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DunningParams {
    /// Maximum cases (default 100, at most 1000), newest Mahnung first.
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LedgerParams {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// The LF MP-ID the account is held under. Defaults to the tenant.
    pub lf_mp_id: Option<String>,
    /// Maximum movements, newest first (default 100, at most 1000).
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
    /// Minimum open balance in EUR, a decimal string (e.g. `"1.99"`); default 0.01.
    pub min_balance_eur: Option<String>,
    /// Maximum rows (default 100, capped at 1000), largest balance first.
    pub limit: Option<i64>,
}

#[derive(Debug, schemars::JsonSchema, serde::Deserialize)]
pub struct ManualBuchungParams {
    pub malo_id: String,
    /// Buchungsart. One of: RECHNUNG, ZAHLUNG, GUTSCHRIFT, EEG_GUTSCHRIFT, EEG_MARKTPRAEMIE,
    /// EEG_AUSZAHLUNG, EEG_RUECKZAHLUNG, BANKRUECKLAST, SEPA_STORNO, MAHNGEBUEHR, VERZUGSZINSEN,
    /// VERZUGSPAUSCHALE, ABSCHLAG, ABSCHLAG_VERRECHNUNG, JAHRESABSCHLUSS, KORREKTUR, STORNO.
    pub entry_type: String,
    /// Amount in ct (× 10⁻² EUR). Positive = debit; negative = credit.
    pub amount_ct: i64,
    /// External reference for audit trail (invoice number, CAMT ref, etc.).
    pub reference_id: Option<String>,
    /// Human-readable description for the Kontoauszug.
    pub description: Option<String>,
}

// ── Typed answers ─────────────────────────────────────────────────────────────
//
// The reads shared with the REST surface return `crate::reads`' types; a tool
// declares the same type its REST twin documents.

pub use crate::reads::{
    Kontoauszug, Kontobewegung, Kontostand, Mahnfaelle, Mahnfall, PaymentMatchParams, Saldostatus,
    UeberfaelligeKonten, UeberfaelligesKonto, Zahlungszuordnung, Zuordnungskandidat,
};

#[derive(Clone)]
pub struct AccountingdMcpHandler {
    state: Arc<AccountingdMcpState>,
    #[expect(
        dead_code,
        reason = "rmcp's `#[tool_handler]` / `#[prompt_handler]` call `Self::tool_router()` / `Self::prompt_router()` per request; the stored router is never read"
    )]
    tool_router: ToolRouter<AccountingdMcpHandler>,
    #[expect(
        dead_code,
        reason = "rmcp's `#[tool_handler]` / `#[prompt_handler]` call `Self::tool_router()` / `Self::prompt_router()` per request; the stored router is never read"
    )]
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
        description = "The operator policy and posture this accountingd deployment runs under: each entry with its value and whether it was set or defaulted. Read-only; a setting changes only by editing the config and restarting.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_policy(&self) -> Result<CallToolResult, McpError> {
        ContentBlock::json(serde_json::to_value(&*self.state.policy).unwrap_or_default())
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get the current open-items balance (in 1/100 EUR cents) for a customer MaLo. Negative = credit; positive = amount owed.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Kontostand>()
    )]
    async fn get_balance(
        &self,
        Parameters(p): Parameters<MaloParams>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::reads::balance(
                &self.state.pool,
                &self.state.tenant,
                &p.malo_id,
                p.lf_mp_id.as_deref(),
            )
            .await,
        )
    }

    #[tool(
        description = "Get the BO4E `Vorauszahlung` (monthly Abschlag) of a customer MaLo — the same body `GET /api/v1/accounts/{malo_id}/vorauszahlung` serves: the stored typed record (`source: stored`), or one derived from `abschlag_ct` when none is stored (`source: derived_from_abschlag_ct`).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_vorauszahlung(
        &self,
        Parameters(p): Parameters<VorauszahlungParams>,
    ) -> Result<CallToolResult, McpError> {
        let lf_mp_id = p.lf_mp_id.as_deref().unwrap_or(&self.state.tenant);
        match crate::pg::fetch_vorauszahlung(
            &self.state.pool,
            &p.malo_id,
            lf_mp_id,
            &self.state.tenant,
        )
        .await
        {
            Ok(Some((vzahlung, abschlag_ct))) => {
                let body = crate::handlers::vorauszahlung_view(&p.malo_id, vzahlung, abschlag_ct)
                    .map_err(|e| McpError::internal_error(e, None))?;
                ContentBlock::json(body)
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::invalid_params(
                format!("account for {} not found", p.malo_id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List ledger movements (RECHNUNG, ZAHLUNG, GUTSCHRIFT, ABSCHLAG, etc.) for a MaLo, newest first, with the `opening_ct` the window starts from — `opening_ct` plus the movements is the newest line's `running_ct`, so a page truncated by `limit` still adds up.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Kontoauszug>()
    )]
    async fn list_ledger(
        &self,
        Parameters(p): Parameters<LedgerParams>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::reads::ledger(
                &self.state.pool,
                &self.state.ledger,
                &self.state.tenant,
                &p.malo_id,
                p.lf_mp_id.as_deref(),
                p.limit,
            )
            .await,
        )
    }

    #[tool(
        description = "List open dunning cases (Mahnstufe 1-3), newest Mahnung first, `limit` of them (default 100, at most 1000): each with its account_id, stufe, amount_due_ct, issued_at and due_date. `saturated` says more may be open than were listed.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Mahnfaelle>()
    )]
    async fn list_dunning(
        &self,
        Parameters(p): Parameters<DunningParams>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::reads::dunning(&self.state.pool, &self.state.tenant, p.limit).await,
        )
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
        description = "List the EEG payout orders the Netzbetreiber ran (§ 19 Abs. 1 EEG 2023), newest first: \
each names its plant (tr_id), MaLo, billing month, amount in ct, creditor and pain.002 status. \
Filter by tr_id, malo_id, year/month, payment_type or status (PDNG, ACCP, ACSC, RJCT, CANC, or NULL \
for not yet submitted). Read-only; held to the NB role like GET /api/v1/eeg/payouts.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::reads::EegAuszahlungen>()
    )]
    async fn list_eeg_payouts(
        &self,
        Parameters(p): Parameters<crate::reads::EegPayoutQuery>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::reads::eeg_payouts(&self.state.pool, &self.state.tenant, &p).await,
        )
    }

    #[tool(
        description = "List the bank lines no account claimed (flat import and camt.052/053/054), oldest value date first, `limit` of them (default 100, at most 1000): each with payment_id, amount_ct, is_return, value_date, reference, end_to_end_id and the payer's IBAN masked. Propose an account with `suggest_payment_match`; an operator books it with POST /api/v1/payments/{payment_id}/assign.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::unmatched::UnzugeordneteZahlungen>()
    )]
    async fn list_unmatched_payments(
        &self,
        Parameters(p): Parameters<crate::handlers::UnmatchedQuery>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::unmatched::list(&self.state.pool, &self.state.tenant, p.limit).await,
        )
    }

    #[tool(
        description = "List the Guthaben Schlussrechnungen owe back (§ 40c Abs. 3 EnWG: paid out within two weeks of the Rechnungsdatum), earliest due date first: each with payout_id, rechnungsnummer, malo_id, amount_ct, due_on, status (open, paid, cancelled) and overdue. Default: the open ones; `overdue=true` only those past due_on.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::guthaben::Guthabenauszahlungen>()
    )]
    async fn list_credit_payouts(
        &self,
        Parameters(p): Parameters<crate::guthaben::CreditPayoutQuery>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::guthaben::list(
                &self.state.pool,
                &self.state.tenant,
                &p,
                mako_fristen::heute(),
            )
            .await,
        )
    }

    #[tool(
        output_schema = rmcp::handler::server::tool::schema_for_output::<UeberfaelligeKonten>(),
        description = "List the accounts with an open balance (balance_ct > 0), largest first. An EEG plant account is never listed: what a plant operator owes back is an EEG Rückforderung (GET /api/v1/eeg/rueckforderungen), not dunned.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue(
        &self,
        Parameters(p): Parameters<OverdueParams>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::reads::overdue(
                &self.state.pool,
                &self.state.tenant,
                p.min_balance_eur.as_deref(),
                p.limit,
            )
            .await,
        )
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
        use crate::pg::{fetch_account, update_account_tenanted};
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
        match update_account_tenanted(
            &self.state.pool,
            &p.malo_id,
            &acct.lf_mp_id,
            &self.state.tenant,
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
Each entry requires: malo_id, amount_ct (positive = credit) and reference. \
Returns `booked` (written to the ledger), `unmatched` (no such account) and `failed` \
(the write errored — the money is NOT applied), with the per-entry reasons in `errors`.",
        annotations(idempotent_hint = false, open_world_hint = false)
    )]
    async fn import_payments(
        &self,
        Parameters(p): Parameters<ImportPaymentsParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut booked = 0usize;
        let mut unmatched = 0usize;
        // A ledger write that failed is not a booked payment. Counting it as
        // one told the agent the money was applied and left the receivable
        // open — the worst of both answers.
        let mut failed = 0usize;
        let mut errors: Vec<String> = Vec::new();
        for entry in &p.entries {
            let malo_id = entry.get("malo_id").and_then(|v| v.as_str());
            let amount_ct = entry.get("amount_ct").and_then(|v| v.as_i64());
            let reference = entry
                .get("reference")
                .and_then(|v| v.as_str())
                .unwrap_or("CAMT.054 import");
            let (Some(malo), Some(amt)) = (malo_id, amount_ct) else {
                unmatched += 1;
                continue;
            };
            use crate::pg::{fetch_account, post_entry};
            let acct = match fetch_account(
                &self.state.pool,
                malo,
                &self.state.tenant,
                &self.state.tenant,
            )
            .await
            {
                Ok(Some(acct)) => acct,
                Ok(None) => {
                    unmatched += 1;
                    continue;
                }
                Err(e) => {
                    tracing::error!(error = %e, malo_id = %malo, "accountingd mcp: account lookup FAILED");
                    failed += 1;
                    errors.push(format!("{malo}: account lookup failed: {e}"));
                    continue;
                }
            };
            let today = mako_fristen::heute();
            match post_entry(
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
            .await
            {
                Ok(_) => booked += 1,
                Err(e) => {
                    tracing::error!(error = %e, malo_id = %malo, "accountingd mcp: ledger write FAILED");
                    failed += 1;
                    errors.push(format!("{malo}: ledger write failed: {e}"));
                }
            }
        }
        ContentBlock::json(serde_json::json!({
            "booked": booked,
            "unmatched": unmatched,
            "failed": failed,
            "errors": errors,
            "total": p.entries.len(),
            "hint": match (unmatched, failed) {
                (0, 0) => "Every entry was booked.".to_owned(),
                (_, 0) => format!("{unmatched} entry/entries name no known account — check malo_id against accountingd accounts."),
                _ => format!("{failed} entry/entries were NOT booked because the write failed — see `errors`. The money is still outstanding; do not report it as received."),
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
                let collection_date = mako_fristen::heute() + time::Duration::days(2);
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
        // Defaults to the previous German calendar year — the one a
        // Jahresabschluss settles.
        let year = p.year.unwrap_or_else(|| mako_fristen::heute().year() - 1);
        // The same arithmetic the REST endpoint commits, over the same source,
        // so the preview and the settlement it previews cannot disagree.
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
            "settlement_eur": crate::handlers::format_ct_as_eur(s.settlement_ct),
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
        let today = mako_fristen::heute();
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
        let today = mako_fristen::heute();
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
            "prap_eur": crate::handlers::format_ct_as_eur(prap_ct),
            "prap_erp_entry": {
                "debit":  "Umsatzerlöse Energie (SKR03: 8400)",
                "credit": "Erhaltene Anzahlungen (SKR03: 1718 / SKR04: 3272)",
                "amount_eur": crate::handlers::format_ct_as_eur(prap_ct),
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
            "monthly_abschlag_total_eur": crate::handlers::format_ct_as_eur(abschlag_total_ct),

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
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Zahlungszuordnung>()
    )]
    async fn suggest_payment_match(
        &self,
        Parameters(p): Parameters<PaymentMatchParams>,
    ) -> Result<CallToolResult, McpError> {
        mako_service::mcp::answer(
            crate::reads::payment_match(
                &self.state.pool,
                &self.state.tenant,
                self.state.iban_key.as_ref(),
                p,
            )
            .await,
        )
    }

    #[tool(
        description = "Post a manual ledger entry (Buchung) to a customer account. \
Use for: ZAHLUNG (incoming bank transfer), BANKRUECKLAST (returned SEPA direct debit), \
KORREKTUR (operator adjustment), GUTSCHRIFT (one-off credit). \
The entry immediately updates the account balance. \
Allowed entry_type: RECHNUNG, ZAHLUNG, GUTSCHRIFT, EEG_GUTSCHRIFT, EEG_MARKTPRAEMIE, \
EEG_AUSZAHLUNG (a settled EEG payout), EEG_RUECKZAHLUNG (a plant operator's repayment of an EEG overpayment), \
BANKRUECKLAST, SEPA_STORNO, MAHNGEBUEHR, VERZUGSZINSEN, VERZUGSPAUSCHALE, ABSCHLAG, ABSCHLAG_VERRECHNUNG, JAHRESABSCHLUSS, KORREKTUR, STORNO. \
amount_ct: positive = debit (increases balance); negative = credit (reduces balance). \
⚠ This is an authorised operator action — always document via reference_id and description.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn post_manual_booking(
        &self,
        Parameters(p): Parameters<ManualBuchungParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{post_entry, upsert_account};

        upsert_account(
            &self.state.pool,
            &p.malo_id,
            &self.state.tenant,
            &self.state.tenant,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let today = mako_fristen::heute();
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

    #[tool(
        description = "Bookings the ledger refused for good, oldest first: each a booking draft committed with its business change whose entry the ledger would not take — a date in a festgeschriebene (sealed) month (`code` 301), a date before the calendar (300), an account not yet open (304), a key already holding another booking (200). Each carries the refusal (`meldung`), what to do about it (`hinweis`) and, for a sealed month, the first open date (`vorschlag_datum`). Resolved by `POST /api/v1/bookings/{key}/rebook` on an open date or `POST /api/v1/bookings/{key}/discard` with a reason. `count` is the whole parked list; `saturated` means it is longer than `buchungen`.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::booking::GeparkteBuchungen>()
    )]
    async fn list_parked_bookings(
        &self,
        Parameters(p): Parameters<ParkedParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(50).clamp(1, 200);
        let parked = crate::booking::parked(
            &self.state.ledger,
            &self.state.pool,
            &self.state.tenant,
            limit,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(
            serde_json::to_value(parked)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        ))
    }

    #[tool(
        description = "One parked booking by its key — the same fields `list_parked_bookings` lists: the Buchungsart, MaLo, amount, refused Buchungsdatum and Beleg, the refusal (`code`, `meldung`), what to do (`hinweis`) and, for a sealed month, the first open date (`vorschlag_datum`). An unknown or already resolved key is an error.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::booking::GeparkteBuchung>()
    )]
    async fn get_parked_booking(
        &self,
        Parameters(p): Parameters<ParkedKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let found = crate::booking::parked_one(
            &self.state.ledger,
            &self.state.pool,
            &self.state.tenant,
            &p.key,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let Some(b) = found else {
            return Err(McpError::invalid_params(
                format!("no parked booking {}", p.key),
                None,
            ));
        };
        Ok(CallToolResult::structured(
            serde_json::to_value(b).map_err(|e| McpError::internal_error(e.to_string(), None))?,
        ))
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
                "1. Use `get_balance` to check current open-items balance.\n                 2. Use `list_ledger` to see recent RECHNUNG/ZAHLUNG/GUTSCHRIFT/ABSCHLAG history.\n                 3. If balance > 0 (overdue): check `list_dunning` for active dunning cases.\n                 4. For missing payments: `import_payments` with CAMT.054 bank entries to match.\n                 5. Monthly Abschlagslauf: `run_abschlag_cycle` raises the Abschlagsforderungen (day=billing_day).\n                 6. Monthly SEPA: `run_sepa_collection` → pain.008 XML (send N-5 bank days before due).\n                 7. After Jahresabschluss: `trigger_jahresabschluss` → review → `update_abschlag`.\n                 8. Period-end HGB accruals: `compute_bilanzielle_abgrenzung` → ERP pRAP/aRAP booking.\n                 9. Mahnstufe 3: de.mako.accounting.sperrauftrag → ORDERS 17115 to the NB, whose Sperrauftrag queue (processd) reports IFTSTA 21039.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for AccountingdMcpHandler {
    /// The revisions every mako MCP server speaks, not the SDK's set.
    fn supported_protocol_versions(
        &self,
    ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }

    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(Implementation::new("accountingd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "accountingd MCP — Customer Account Ledger (Massenkontokorrent, LF role).\n\
                 Running debit/credit ledger per MaLo; SEPA direct debit; Mahnwesen Mahnstufe 1-3.\n\n\
                 **Order-to-Cash integration:**\n\
                 - Inbound: de.mako.billing.rechnung.erstellt → RECHNUNG debit entry\n\
                 - Inbound: CAMT.054 bank statement → use `import_payments` for ZAHLUNG credit\n\
                 - Outbound: pain.008 XML → use `run_sepa_collection` for monthly Abschlag collection\n\
                 - Dunning: `list_dunning` → escalate → de.mako.accounting.sperrauftrag → ORDERS 17115 to the NB\n\n\
                 **⚠ EEG double-booking prevention (§20-21 EEG 2023):**\n\
                 If billingd includes an EEG Gutschrift as a negative Rechnungsposition,\n\
                 the resulting debit from de.mako.billing.rechnung.erstellt is already net of EEG.\n\
                 Do NOT post a separate credit for de.mako.eeg.verguetung.berechnet for the same\n\
                 customer/period — that path is only for Direktvermarkter standalone settlement.\n\n\
                 Use `get_balance` for open-items balance.\n\
                 Use `update_abschlag` after Jahresabschluss to recalibrate monthly advance.",
            )
    }
}

// ── Per-tool authorization ────────────────────────────────────────────────────

/// The Cedar action one MCP tool requires — the same action its REST twin
/// enforces.
///
/// The blanket `use-mcp` gate the shared middleware applies cannot tell a read
/// from a write, and this surface is not read-only: `post_manual_booking` posts
/// a Buchung to a customer's Kontokorrent, `run_abschlag_cycle` raises an
/// Abschlagsforderung against every account due that day, `import_payments`
/// books a ZAHLUNG per CAMT.054 entry, `update_abschlag` rewrites the monthly
/// advance and the SEPA billing day, and `run_sepa_collection` emits a
/// bank-submittable pain.008 carrying every mandate's IBAN. Five of the eighteen
/// tools write, so the surface is not "read-only by construction".
///
/// Every tool is mapped, not only the five that write, because accountingd's
/// *reads* are role-split too: `read-account` is open to any token of the
/// tenant, while `read-banking` and `read-books` are held to LF/MSB. A blanket
/// gate would let a role-less token read the mandate register and the whole
/// aging list through MCP, which the REST twins of those tools refuse.
///
/// `None` means the tool is unknown here, which is refused rather than served:
/// a tool added without an entry must fail closed, since the failure mode of an
/// allowlist is the newest entry being the one it misses.
pub fn tool_action(tool: &str) -> Option<&'static str> {
    Some(match tool {
        // Reads about one customer.
        "get_balance"
        | "get_vorauszahlung"
        | "list_ledger"
        | "suggest_payment_match"
        // A preview: it fetches the account and nets the year, and commits
        // nothing (`POST /api/v1/jahresabschluss/{malo_id}` is what commits).
        | "trigger_jahresabschluss" => "read-account",
        // Tenant-wide book reads — the dunning list, the aging list, the HGB
        // § 250 accrual position off the ledger.
        "list_dunning"
        | "list_overdue"
        | "list_unmatched_payments"
        | "list_credit_payouts"
        | "compute_bilanzielle_abgrenzung"
        | "list_parked_bookings"
        | "get_parked_booking" => "read-books",
        // The collection register: mandates, IBANs and the entries of a run.
        "list_sepa_collections" => "read-banking",
        // The EEG payouts are the Netzbetreiber's (§ 19 Abs. 1 EEG 2023).
        "list_eeg_payouts" => "read-eeg-payouts",
        // The posture report the runner built at boot.
        "get_policy" => "read-policy",
        // ── Writes ───────────────────────────────────────────────────────────
        "update_abschlag" => "write-account",
        "import_payments" => "import-payments",
        "run_sepa_collection" => "manage-sepa",
        "run_abschlag_cycle" | "post_manual_booking" => "post-entry",
        _ => return None,
    })
}

/// What a JSON-RPC frame asks for, as far as authorization is concerned.
enum McpCall {
    /// Not a `tools/call` — `initialize`, `tools/list`, a prompt. The blanket
    /// `use-mcp` gate is the whole check.
    NotATool,
    /// A `tools/call` for a tool this build knows, and the action it needs.
    Tool(&'static str),
    /// A `tools/call` naming a tool with no entry in [`tool_action`].
    UnknownTool(String),
}

fn classify(body: &[u8]) -> McpCall {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return McpCall::NotATool;
    };
    if v.get("method").and_then(serde_json::Value::as_str) != Some("tools/call") {
        return McpCall::NotATool;
    }
    let Some(name) = v
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(serde_json::Value::as_str)
    else {
        return McpCall::NotATool;
    };
    match tool_action(name) {
        Some(action) => McpCall::Tool(action),
        None => McpCall::UnknownTool(name.to_owned()),
    }
}

/// One MCP frame's size cap — the body must be buffered to read the tool name.
const MAX_MCP_BODY: usize = 1024 * 1024;

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AccountingdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_MCP_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return mako_service::ApiError::payload_too_large(format!(
                "MCP request body exceeds {MAX_MCP_BODY} bytes"
            ))
            .into_response();
        }
    };
    match classify(&bytes) {
        McpCall::NotATool => {}
        McpCall::Tool(action) => {
            if let Err(resp) = state.auth.authorize(&parts.headers, action) {
                return resp;
            }
        }
        McpCall::UnknownTool(name) => {
            tracing::warn!(tool = %name, "accountingd: MCP tool carries no Cedar action");
            return mako_service::error::ApiError::forbidden(format!(
                    "403 Forbidden: MCP tool {name:?} carries no Cedar action, so it cannot be                      authorized — add it to `tool_action`"
                )).into_response();
        }
    }
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<AccountingdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = AccountingdMcpHandler::new(Arc::clone(&state));
    let service = mako_service::mcp::service(
        move || Ok(handler.clone()),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        // `route_layer`, not `layer`: a layer also wraps the fallback, which a
        // merge hands every unknown path of the service.
        .route_layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `get_vorauszahlung` is on the router `tools/list` answers from, declared
    /// read-only, and held to the action its REST twin
    /// `GET /api/v1/accounts/{malo_id}/vorauszahlung` checks.
    ///
    /// The router is the list the client sees: a tool missing from it is absent
    /// from `tools/list` and refused by `tools/call`, whatever a manifest grants.
    #[test]
    fn get_vorauszahlung_is_listed_read_only_and_held_to_read_account() {
        let router = AccountingdMcpHandler::tool_router();
        let tool = router
            .get("get_vorauszahlung")
            .expect("tools/list carries get_vorauszahlung");
        assert_eq!(
            tool.annotations.as_ref().and_then(|a| a.read_only_hint),
            Some(true),
            "get_vorauszahlung reads a stored Vorauszahlung and must be declared read-only"
        );
        assert_eq!(tool_action("get_vorauszahlung"), Some("read-account"));
    }

    /// The tool contract the Desk is generated from: name, annotations and
    /// both schemas. `BLESS_MCP_SNAPSHOT=1` rewrites `mcp.snapshot.json`.
    #[test]
    fn the_tool_contract_matches_the_snapshot() {
        let tools: Vec<serde_json::Value> = AccountingdMcpHandler::tool_router()
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

    /// The REST twin of each shared read, as its path stands in the document.
    fn rest_twin(tool: &str) -> &'static str {
        match tool {
            "get_balance" => "/api/v1/accounts/{malo_id}/balance",
            "list_ledger" => "/api/v1/accounts/{malo_id}/ledger",
            "list_dunning" => "/api/v1/dunning",
            "list_overdue" => "/api/v1/offene-posten",
            "suggest_payment_match" => "/api/v1/payments/match",
            "list_unmatched_payments" => "/api/v1/payments/unmatched",
            "list_credit_payouts" => "/api/v1/credit-payouts",
            "list_eeg_payouts" => "/api/v1/eeg/payouts",
            _ => panic!("{tool} has no REST twin"),
        }
    }

    /// Assert `answer` satisfies the output schema `tool` declares, and that
    /// schema is the shape its REST twin documents.
    fn holds_to_schema<T: serde::Serialize>(tool: &str, answer: &T) {
        let tools = AccountingdMcpHandler::tool_router().list_all();
        mako_service::mcp::conformance::assert_structured_output(
            &tools,
            tool,
            &serde_json::to_value(answer).expect("serializes"),
        );
        mako_service::mcp::conformance::assert_rest_twin(
            &tools,
            tool,
            &crate::openapi::document(),
            rest_twin(tool),
        );
    }

    /// A shared read's tool is held to the Cedar action its REST twin checks:
    /// the handler's `cedar.check` names the same action `tool_action` maps
    /// the tool to.
    #[test]
    fn each_shared_read_is_authorized_as_its_rest_twin() {
        const HANDLERS: &str = include_str!("handlers.rs");
        for (tool, handler) in [
            ("get_balance", "get_balance"),
            ("list_ledger", "get_ledger"),
            ("list_dunning", "get_dunning"),
            ("list_overdue", "get_offene_posten"),
            ("suggest_payment_match", "match_payment"),
            ("list_unmatched_payments", "get_unmatched_payments"),
            ("list_credit_payouts", "get_credit_payouts"),
            ("list_eeg_payouts", "get_eeg_payouts"),
        ] {
            let start = HANDLERS
                .find(&format!("\npub async fn {handler}("))
                .unwrap_or_else(|| panic!("{handler} is in handlers.rs"));
            let rest = &HANDLERS[start + 1..];
            let body = &rest[..rest.find("\npub async fn ").unwrap_or(rest.len())];
            let action = tool_action(tool).unwrap_or_else(|| panic!("{tool} is mapped"));
            assert!(
                body.contains(&format!(".check(&claims.principal(), \"{action}\"")),
                "{tool} is authorized as {action:?}, which {handler} does not check"
            );
        }
    }

    #[test]
    fn get_balance_answers_its_declared_schema() {
        holds_to_schema(
            "get_balance",
            &Kontostand {
                malo_id: "51238696012".into(),
                lf_mp_id: "9900357000004".into(),
                status: Saldostatus::Credit,
                balance_ct: -1250,
                balance_eur: crate::handlers::format_ct_as_eur(-1250),
                abschlag_ct: 8900,
            },
        );
    }

    #[test]
    fn list_ledger_answers_its_declared_schema() {
        let window = crate::pg::LedgerWindow {
            opening_ct: 8900,
            lines: vec![crate::pg::LedgerLine {
                entry_id: uuid::Uuid::nil(),
                entry_type: Some("ZAHLUNG".into()),
                beleg: None,
                side: "C",
                amount_ct: 8900,
                signed_ct: -8900,
                running_ct: 0,
                booking_date: time::macros::date!(2026 - 11 - 02),
            }],
        };
        let auszug = Kontoauszug::new("51238696012".into(), "9900357000004".into(), window);
        assert_eq!(auszug.lines[0].booking_date, "2026-11-02");
        holds_to_schema("list_ledger", &auszug);
    }

    #[test]
    fn list_dunning_answers_its_declared_schema() {
        let fall = Mahnfall::from(crate::pg::DunningCaseRow {
            id: uuid::Uuid::nil(),
            account_id: uuid::Uuid::nil(),
            tenant: "9900357000004".into(),
            stufe: 2,
            amount_due_ct: 17_800,
            issued_at: time::macros::datetime!(2026-11-02 08:00 UTC),
            due_date: time::macros::date!(2026 - 11 - 16),
            resolved_at: None,
            sperrauftrag_ce_id: None,
        });
        assert_eq!(fall.issued_at, "2026-11-02T08:00:00Z");
        holds_to_schema(
            "list_dunning",
            &Mahnfaelle {
                faelle: vec![fall],
                count: 1,
                saturated: false,
            },
        );
    }

    #[test]
    fn list_overdue_answers_its_declared_schema() {
        holds_to_schema(
            "list_overdue",
            &UeberfaelligeKonten {
                accounts: vec![UeberfaelligesKonto {
                    account_id: uuid::Uuid::nil().to_string(),
                    malo_id: "51238696012".into(),
                    lf_mp_id: "9900357000004".into(),
                    kunden_nr: Some("K-1001".into()),
                    balance_ct: 17_800,
                    balance_eur: "178.00".into(),
                    abschlag_ct: 8900,
                    updated_at: "2026-11-02T08:00:00Z".into(),
                }],
                count: 1,
                saturated: false,
            },
        );
    }

    #[test]
    fn suggest_payment_match_answers_its_declared_schema() {
        let kandidat = |matched_by: &str, score: Option<u32>| Zuordnungskandidat {
            account_id: uuid::Uuid::nil().to_string(),
            malo_id: "51238696012".into(),
            lf_mp_id: "9900357000004".into(),
            open_balance_ct: 12_500,
            open_balance_eur: "125.00".into(),
            matched_by: matched_by.into(),
            confidence: if score.is_some() { "LOW" } else { "EXACT" }.into(),
            similarity_score: score,
            residual_ct: 0,
        };
        for (tolerance_pct, k) in [
            (None, kandidat("iban", None)),
            (Some(2.0), kandidat("amount_proximity", Some(98))),
        ] {
            holds_to_schema(
                "suggest_payment_match",
                &Zahlungszuordnung {
                    payment_amount_ct: 12_500,
                    payment_amount_eur: "125.00".into(),
                    payment_reference: "Abschlag November".into(),
                    tolerance_pct,
                    candidates_count: 1,
                    candidates: vec![k],
                    note: "…".into(),
                },
            );
        }
    }

    #[test]
    fn list_unmatched_payments_answers_its_declared_schema() {
        holds_to_schema(
            "list_unmatched_payments",
            &crate::unmatched::UnzugeordneteZahlungen {
                payments: vec![crate::unmatched::UnzugeordneteZahlung {
                    payment_id: uuid::Uuid::nil().to_string(),
                    bank_transaction_id: "TXN-1".into(),
                    amount_ct: 4217,
                    amount_eur: "42.17".into(),
                    is_return: false,
                    value_date: "2026-11-02".into(),
                    reference: "Abschlag".into(),
                    end_to_end_id: None,
                    iban_masked: Some("DE…3000".into()),
                    source: crate::unmatched::Quelle::Camt054,
                    received_at: "2026-11-02T08:00:00Z".into(),
                }],
                count: 1,
                saturated: false,
            },
        );
    }

    #[test]
    fn list_credit_payouts_answers_its_declared_schema() {
        holds_to_schema(
            "list_credit_payouts",
            &crate::guthaben::Guthabenauszahlungen {
                payouts: vec![crate::guthaben::Guthabenauszahlung {
                    payout_id: uuid::Uuid::nil().to_string(),
                    rechnungsnummer: "RE-2026-000042".into(),
                    malo_id: "51238696012".into(),
                    lf_mp_id: "9900357000004".into(),
                    amount_ct: 4217,
                    amount_eur: "42.17".into(),
                    rechnungsdatum: "2026-04-01".into(),
                    due_on: "2026-04-15".into(),
                    status: crate::guthaben::Status::Open,
                    overdue: true,
                }],
                count: 1,
                overdue_count: 1,
                saturated: false,
            },
        );
    }

    #[test]
    fn list_eeg_payouts_answers_its_declared_schema() {
        holds_to_schema(
            "list_eeg_payouts",
            &crate::reads::EegAuszahlungen {
                payouts: vec![crate::reads::EegAuszahlung {
                    payout_id: uuid::Uuid::nil().to_string(),
                    malo_id: "51238696012".into(),
                    tr_id: "E0000000001".into(),
                    billing_year: 2026,
                    billing_month: 10,
                    amount_ct: 12_345,
                    amount_eur: "123.45".into(),
                    creditor_name: "Solar Muster GbR".into(),
                    payment_type: "SCT_INST".into(),
                    end_to_end_ref: "EEG-1".into(),
                    pain002_status: Some("ACCP".into()),
                    pain002_reason: None,
                    submitted_at: Some("2026-11-02T08:00:00Z".into()),
                    settled_at: None,
                    created_at: "2026-11-02T07:00:00Z".into(),
                }],
                count: 1,
                saturated: false,
            },
        );
    }
}
