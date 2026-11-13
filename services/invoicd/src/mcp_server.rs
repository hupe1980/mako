//! MCP (Model Context Protocol) server for `invoicd`.
//!
//! Exposes INVOIC receipt reads, dispute management, Gas/Strom billing
//! reconciliation, and § 147 AO / GoBD compliance monitoring to LLM tooling via
//! the MCP Streamable HTTP transport.  Mounted at `/mcp` on the existing
//! HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | `get_receipt`              | Read a single INVOIC receipt by UUID |
//! | `list_disputes`            | Disputed INVOIC receipts, typed, with the Zahlungsziel as `frist` |
//! | `get_check_result`         | Get the plausibility findings for a receipt |
//! | `list_overdue_remadv`      | Receipts approaching Zahlungsziel without dispatched REMADV |
//! | `get_zahlungsstatus`       | Payment status per MaLo-ID (settled / pending / overdue) |
//! | `summarize_billing_month`  | Monthly billing volume and dispute rate per NB |
//! | `list_exceptions`          | The two operator queues: undispatched answers and dead-lettered INVOICs |
//! | `list_held_receipts`       | Invoices held `Offen`, each with the Prüfschritte a person has to state |
//!
//! ## Prompts
//!
//! | Prompt | Description |
//! |--------|-------------|
//! | `resolve-dispute`          | Investigate and resolve an INVOIC dispute |
//! | `check-overdue-remadv`     | Monitor and action overdue REMADV dispatches |
//! | `monthly-billing-review`   | Guided monthly billing reconciliation |
//! | `detect-systematic-errors` | Find systematic billing errors across NB counterparties |

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
    transport::streamable_http_server::StreamableHttpServerConfig,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

// ── Typed results ─────────────────────────────────────────────────────────────

/// One disputed INVOIC receipt.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Disput {
    /// The receipt.
    pub id: String,
    /// The market process the INVOIC arrived in.
    pub process_id: String,
    /// BDEW Prüfidentifikator of the INVOIC.
    pub pid: i16,
    /// MP-ID of the Rechnungssteller.
    pub sender_mp_id: String,
    /// The Rechnungsnummer the Rechnungssteller gave the invoice, where the
    /// INVOIC carried one.
    pub rechnungsnummer: Option<String>,
    /// RFC 3339 in UTC.
    #[schemars(extend("format" = "date-time"))]
    pub received_at: String,
    /// BO4E schema version of the stored Rechnung.
    pub bo4e_version: String,
    /// The end of the Berlin day of the Zahlungsziel (SG8 DTM+265), RFC 3339
    /// in UTC; `None` when the INVOIC carries no Zahlungsziel.
    #[schemars(extend("format" = "date-time"))]
    pub frist: Option<String>,
    /// The refusal (REMADV) went out to the Rechnungssteller. When it did not,
    /// `POST /api/v1/receipts/{id}/dispatch-answer` sends it — a market
    /// message; when it did, what is left is closing the dispute
    /// (`POST /api/v1/receipts/{id}/resolve-dispute`), which sends nothing.
    pub abgesendet: bool,
    /// The invoic-checker findings that disputed the invoice, in its words.
    pub befunde: Vec<DisputBefund>,
}

/// One finding a dispute rests on.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DisputBefund {
    /// The finding's kind, as `get_check_result` names it (`TariffDeviation`, …)
    /// — an identifier, not for a reader.
    pub art: String,
    /// What the kind is called in German (`Preis weicht vom Preisblatt ab`).
    pub bezeichnung: String,
    /// The checker's sentence.
    pub text: String,
}

/// A stored finding kind's German name, from the checker's one table.
fn bezeichnung(kind: &serde_json::Value) -> &'static str {
    serde_json::from_value::<invoic_checker::FindingKind>(kind.clone()).map_or(
        "Befund der Rechnungsprüfung",
        invoic_checker::FindingKind::bezeichnung,
    )
}

/// The disputing findings of a stored `findings` array; anything else in it
/// — a warning, a row the checker no longer writes — is not a ground.
fn befunde(findings: &serde_json::Value) -> Vec<DisputBefund> {
    findings
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["is_dispute"].as_bool() == Some(true))
        .filter_map(|f| {
            Some(DisputBefund {
                art: f["kind"].as_str()?.to_owned(),
                bezeichnung: bezeichnung(&f["kind"]).to_owned(),
                text: f["message"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Each stored finding with its kind's German name beside the kind.
fn mit_bezeichnung(mut findings: serde_json::Value) -> serde_json::Value {
    if let Some(items) = findings.as_array_mut() {
        for f in items {
            let name = bezeichnung(&f["kind"]);
            if let Some(o) = f.as_object_mut() {
                o.insert("bezeichnung".to_owned(), name.into());
            }
        }
    }
    findings
}

/// A `list_disputes` row: `(id, process_id, pid, sender_mp_id,
/// rechnungsnummer, received_at, bo4e_version, pay_by, dispatched_at,
/// findings)`.
type DisputRow = (
    uuid::Uuid,
    uuid::Uuid,
    i16,
    String,
    Option<String>,
    time::OffsetDateTime,
    String,
    Option<time::OffsetDateTime>,
    Option<time::OffsetDateTime>,
    serde_json::Value,
);

impl Disput {
    fn from_row(
        (
            id,
            process_id,
            pid,
            sender_mp_id,
            rechnungsnummer,
            received_at,
            bo4e_version,
            pay_by,
            dispatched_at,
            findings,
        ): DisputRow,
    ) -> Self {
        Self {
            id: id.to_string(),
            process_id: process_id.to_string(),
            pid,
            sender_mp_id,
            rechnungsnummer,
            received_at: rfc3339(received_at),
            bo4e_version,
            frist: pay_by.map(|t| {
                let tag = mako_fristen::berlin_date(t);
                tag.next_day()
                    .map(mako_fristen::berlin_midnight)
                    .map(rfc3339)
                    .unwrap_or_default()
            }),
            abgesendet: dispatched_at.is_some(),
            befunde: befunde(&findings),
        }
    }
}

/// `list_disputes`' answer.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Disputes {
    /// The most recent first.
    pub disputes: Vec<Disput>,
    /// How many receipts `disputes` holds.
    pub count: usize,
    /// At least 200 receipts matched: more may be disputed than were listed.
    pub saturated: bool,
}

/// One invoice the Entscheidungsbaum left open.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GehalteneRechnung {
    /// The receipt — the id `POST /api/v1/receipts/{id}/feststellungen` takes.
    pub id: String,
    pub process_id: String,
    pub pid: i16,
    pub sender_mp_id: String,
    pub rechnungsnummer: Option<String>,
    /// `SG1 RFF+ACE` — the order a Leistungsnachweis names.
    pub bestellung_ref: Option<String>,
    /// The end of the Berlin day of the Zahlungsziel, when there is one.
    pub frist: Option<String>,
    /// The Prüfschritte the walk reached and no record answered: `pruefschritt`,
    /// `ebene` (with the Positionsnummer on the Positionsebene) and `detail`.
    #[schemars(with = "Vec<serde_json::Value>")]
    pub offen: serde_json::Value,
}

/// `list_held_receipts`' answer.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GehalteneRechnungen {
    /// The nearest Zahlungsziel first.
    pub gehalten: Vec<GehalteneRechnung>,
    pub count: usize,
    /// At least 200 receipts matched.
    pub saturated: bool,
}

fn rfc3339(t: time::OffsetDateTime) -> String {
    use time::format_description::well_known::Rfc3339;
    t.format(&Rfc3339).unwrap_or_default()
}

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// The operator policy and posture the runner built at boot.
    pub policy: mako_service::policy::PolicyReport,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetReceiptParams {
    /// UUID of the receipt row (from list response).
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCheckResultParams {
    /// UUID of the receipt whose plausibility findings you want.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetZahlungsstatusParams {
    /// MaLo-ID (11-digit Marktlokations-ID) to query.
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SummarizeBillingMonthParams {
    /// Year (e.g. 2026).
    pub year: i32,
    /// Month 1–12.
    pub month: u8,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicdMcpHandler {
    state: Arc<InvoicdMcpState>,
    #[expect(
        dead_code,
        reason = "rmcp's `#[tool_handler]` / `#[prompt_handler]` call `Self::tool_router()` / `Self::prompt_router()` per request; the stored router is never read"
    )]
    tool_router: ToolRouter<InvoicdMcpHandler>,
    #[expect(
        dead_code,
        reason = "rmcp's `#[tool_handler]` / `#[prompt_handler]` call `Self::tool_router()` / `Self::prompt_router()` per request; the stored router is never read"
    )]
    prompt_router: PromptRouter<InvoicdMcpHandler>,
}

#[tool_router]
impl InvoicdMcpHandler {
    fn new(state: Arc<InvoicdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "The operator policy and posture this invoicd deployment runs under: each setting with its value, whether it was set or defaulted, and the primary source of the value. Read-only; a setting changes only by editing the config and restarting.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_policy(&self) -> Result<CallToolResult, McpError> {
        ContentBlock::json(serde_json::to_value(&self.state.policy).unwrap_or_default())
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Read a single INVOIC receipt by UUID.
    ///
    /// Returns the receipt metadata including PID, sender GLN, outcome
    /// (`Ok`/`Warn`/`Dispute`), and timestamps.  The full `rechnung` BO4E
    /// payload is not returned — use `get_check_result` to inspect findings.
    #[tool(
        description = "Read a single INVOIC receipt by UUID",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_receipt(
        &self,
        Parameters(p): Parameters<GetReceiptParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        let row = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                uuid::Uuid,
                i16,
                String,
                String,
                time::OffsetDateTime,
                Option<time::OffsetDateTime>,
                String,
            ),
        >(
            r#"
            SELECT id, process_id, pid, sender_mp_id, outcome,
                   received_at, dispatched_at, bo4e_version
            FROM invoic_receipts
            WHERE id = $1 AND tenant = $2
            "#,
        )
        .bind(id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                id,
                process_id,
                pid,
                sender_mp_id,
                outcome,
                received_at,
                dispatched_at,
                bo4e_version,
            )) => ContentBlock::json(serde_json::json!({
                "id": id,
                "process_id": process_id,
                "pid": pid,
                "sender_mp_id": sender_mp_id,
                "outcome": outcome,
                "received_at": received_at,
                "dispatched_at": dispatched_at,
                "bo4e_version": bo4e_version,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))])),
        }
    }

    /// List the disputed INVOIC receipts for this tenant.
    ///
    /// Receipts with `outcome = 'Dispute'`, the most recent first. Disputes
    /// need manual review and are not auto-settled by `invoicd`.
    #[tool(
        description = "Open INVOIC disputes (outcome=Dispute) of this tenant, the most recent first, at most 200. `frist` is the end of the Berlin day of the invoice's Zahlungsziel (SG8 DTM+265), when it carries one. `abgesendet` says whether the refusal (REMADV) went out; `befunde` are the checker findings the dispute rests on, each with its `art` as `get_check_result` names it. `saturated` means at least 200 matched.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Disputes>()
    )]
    async fn list_disputes(&self) -> Result<CallToolResult, McpError> {
        const LIMIT: usize = 200;
        let rows = sqlx::query_as::<_, DisputRow>(
            r#"
            SELECT id, process_id, pid, sender_mp_id, rechnungsnummer, received_at,
                   bo4e_version, pay_by, dispatched_at, findings
            FROM invoic_receipts
            WHERE tenant = $1 AND outcome = 'Dispute'
            ORDER BY received_at DESC
            LIMIT 200
            "#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let disputes: Vec<Disput> = rows.into_iter().map(Disput::from_row).collect();
        Ok(CallToolResult::structured(
            serde_json::to_value(Disputes {
                count: disputes.len(),
                saturated: disputes.len() >= LIMIT,
                disputes,
            })
            .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        ))
    }

    /// The invoices held `Offen`: each walk reached a Prüfschritt no record
    /// answers, so no REMADV went out. Deciding one goes through REST — a
    /// Leistungsnachweis (`POST /api/v1/leistungsnachweise`) or a statement on
    /// the invoice (`POST /api/v1/receipts/{id}/feststellungen`) — which walks
    /// it again in place.
    #[tool(
        description = "INVOIC receipts held Offen (the Entscheidungsbaum reached a Prüfschritt no record answers; no REMADV was sent), the nearest Zahlungsziel first, at most 200. `offen` lists each unanswered Prüfschritt with its Ebene/Positionsnummer and the missing fact. Prüfschritt 310 is answered by recording a Leistungsnachweis for `bestellung_ref` (POST /api/v1/leistungsnachweise); 310/320/330/80, and in the second round after a COMDIS 29001 (E_0266) Prüfschritt 1 `einwaende_entkraeftet`, by a statement on the receipt (POST /api/v1/receipts/{id}/feststellungen) — both walk the invoice again.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<GehalteneRechnungen>()
    )]
    async fn list_held_receipts(&self) -> Result<CallToolResult, McpError> {
        const LIMIT: usize = 200;
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                uuid::Uuid,
                i16,
                String,
                Option<String>,
                Option<String>,
                Option<time::OffsetDateTime>,
                serde_json::Value,
            ),
        >(
            r"SELECT id, process_id, pid, sender_mp_id, rechnungsnummer, bestellung_ref,
                     pay_by, offen
                FROM invoic_receipts
               WHERE tenant = $1 AND outcome = 'Offen' AND dispatched_at IS NULL
               ORDER BY pay_by ASC NULLS LAST
               LIMIT 200",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let gehalten: Vec<GehalteneRechnung> = rows
            .into_iter()
            .map(
                |(
                    id,
                    process_id,
                    pid,
                    sender_mp_id,
                    rechnungsnummer,
                    bestellung_ref,
                    pay_by,
                    offen,
                )| {
                    GehalteneRechnung {
                        id: id.to_string(),
                        process_id: process_id.to_string(),
                        pid,
                        sender_mp_id,
                        rechnungsnummer,
                        bestellung_ref,
                        frist: pay_by.and_then(|t| {
                            mako_fristen::berlin_date(t)
                                .next_day()
                                .map(mako_fristen::berlin_midnight)
                                .map(rfc3339)
                        }),
                        offen,
                    }
                },
            )
            .collect();
        Ok(CallToolResult::structured(
            serde_json::to_value(GehalteneRechnungen {
                count: gehalten.len(),
                saturated: gehalten.len() >= LIMIT,
                gehalten,
            })
            .map_err(|e| McpError::internal_error(e.to_string(), None))?,
        ))
    }

    /// Get plausibility check findings for an INVOIC receipt.
    ///
    /// Returns the structured `findings` array from the `invoic-checker`
    /// plausibility run: each finding has a `code`, `severity`
    /// (`Ok`/`Warn`/`Error`), and a human-readable `message`.
    ///
    /// Useful for understanding why a receipt was disputed or flagged.
    #[tool(
        description = "Get invoic-checker plausibility findings for a receipt: each with its `kind` (the identifier to cite), its German `bezeichnung`, `is_dispute` and the checker's `message`.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_check_result(
        &self,
        Parameters(p): Parameters<GetCheckResultParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        let row =
            sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, time::OffsetDateTime)>(
                r#"
            SELECT id, outcome, findings, checked_at
            FROM invoic_receipts
            WHERE id = $1 AND tenant = $2
            "#,
            )
            .bind(id)
            .bind(&self.state.tenant)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((id, outcome, findings, checked_at)) => ContentBlock::json(serde_json::json!({
                "id": id,
                "outcome": outcome,
                "findings": mit_bezeichnung(findings),
                "checked_at": checked_at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))])),
        }
    }

    /// List INVOIC receipts approaching their Zahlungsziel without a dispatched REMADV.
    ///
    /// Returns receipts where `pay_by < now() + 3 days` AND `dispatched_at IS NULL`.
    /// These are at risk of missing the payment deadline. Alert if non-empty.
    ///
    /// The recipient answers by the Zahlungsziel the invoice states: GPKE
    /// Teil 2 Kap. 3.3.2 Nr. 2 („spätester ÜT ist der 4. WT vor dem
    /// Zahlungsziel"), WiM Teil 1 Kap. 3.6.3.8.2 („zum Zahlungsziel").
    #[tool(
        description = "List receipts approaching Zahlungsziel without dispatched REMADV (regulatory deadline risk). Returns up to 50 overdue entries.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue_remadv(&self) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query(
            r#"SELECT id, process_id, pid, sender_mp_id, outcome, pay_by, received_at
               FROM invoic_receipts
               WHERE tenant = $1
                 AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
                 AND pay_by IS NOT NULL
                 AND pay_by < now() + INTERVAL '3 days'
                 AND dispatched_at IS NULL
               ORDER BY pay_by ASC
               LIMIT 50"#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                use time::format_description::well_known::Rfc3339;
                serde_json::json!({
                    "id": r.try_get::<uuid::Uuid, _>("id").ok(),
                    "process_id": r.try_get::<uuid::Uuid, _>("process_id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "pay_by": r.try_get::<time::OffsetDateTime, _>("pay_by").ok()
                        .and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "overdue_count": items.len(),
            "items": items,
            "alert": !items.is_empty(),
            "note": "the REMADV is due by the invoice's Zahlungsziel (pay_by) — for a Netznutzungsrechnung by the 4th Werktag before it (GPKE Teil 2 Kap. 3.3.2 Nr. 2)",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Payment status for a MaLo-ID — settled, pending, and overdue counts.
    ///
    /// Uses the indexed `malo_id` column for fast lookup.  Returns a summary
    /// plus the individual receipt statuses ordered by `received_at` descending.
    #[tool(
        description = "Payment status per MaLo-ID: settled/pending/overdue counts and receipt list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_zahlungsstatus(
        &self,
        Parameters(p): Parameters<GetZahlungsstatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query(
            r"SELECT id, process_id, pid, outcome, pay_by, dispatched_at,
                     payment_confirmed_at, received_at
              FROM invoic_receipts
              WHERE tenant = $1 AND malo_id = $2
              ORDER BY received_at DESC LIMIT 100",
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut overdue = 0u32;
        let mut pending = 0u32;
        let mut settled = 0u32;
        let now = time::OffsetDateTime::now_utc();

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                use time::format_description::well_known::Rfc3339;
                let pay_by = r
                    .try_get::<Option<time::OffsetDateTime>, _>("pay_by")
                    .ok()
                    .flatten();
                let dispatched = r
                    .try_get::<Option<time::OffsetDateTime>, _>("dispatched_at")
                    .ok()
                    .flatten();
                let confirmed = r
                    .try_get::<Option<time::OffsetDateTime>, _>("payment_confirmed_at")
                    .ok()
                    .flatten();
                let zahlungsstatus = if confirmed.is_some() {
                    settled += 1;
                    "settled"
                } else if dispatched.is_some() && pay_by.is_some_and(|d| d < now) {
                    overdue += 1;
                    "overdue"
                } else if dispatched.is_some() {
                    pending += 1;
                    "pending"
                } else {
                    "undispatched"
                };
                serde_json::json!({
                    "id": r.try_get::<uuid::Uuid, _>("id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "zahlungsstatus": zahlungsstatus,
                    "pay_by": pay_by.and_then(|t| t.format(&Rfc3339).ok()),
                    "received_at": r.try_get::<time::OffsetDateTime, _>("received_at").ok()
                        .and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "overdue_count": overdue,
            "pending_count": pending,
            "settled_count": settled,
            "items": items,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Monthly billing volume and dispute rate per NB counterparty.
    ///
    /// Aggregates all receipts for a calendar month and returns per-NB statistics:
    /// total count, dispute count, dispute rate, and total accepted volume (EUR).
    /// Useful for detecting systematic billing errors by one NB.
    #[tool(
        description = "Monthly billing summary per NB (PID breakdown, dispute rate, volume in EUR)",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn summarize_billing_month(
        &self,
        Parameters(p): Parameters<SummarizeBillingMonthParams>,
    ) -> Result<CallToolResult, McpError> {
        if !(1..=12).contains(&p.month) {
            return Err(McpError::invalid_params(
                "month must be between 1 and 12",
                None,
            ));
        }
        let rows = sqlx::query(
            r"SELECT sender_mp_id, pid, outcome,
                     COUNT(*)                                      AS total,
                     COUNT(*) FILTER (WHERE outcome = 'Dispute')  AS disputes
              FROM invoic_receipts
              WHERE tenant = $1
                AND date_trunc('month', received_at AT TIME ZONE 'Europe/Berlin') =
                    make_date($2, $3, 1)::timestamp
              GROUP BY sender_mp_id, pid, outcome
              ORDER BY sender_mp_id, pid",
        )
        .bind(&self.state.tenant)
        .bind(p.year)
        .bind(p.month as i32)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let summary: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                serde_json::json!({
                    "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "count": r.try_get::<i64, _>("total").ok(),
                    "disputes": r.try_get::<i64, _>("disputes").ok(),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "year": p.year,
            "month": p.month,
            "rows": summary,
            "total_receipts": summary.iter()
                .filter_map(|r| r["count"].as_i64()).sum::<i64>(),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// The two queues an operator has to clear, with the reason for each entry.
    ///
    /// Read-only, like every tool here: acting on either queue goes through the
    /// REST API, which authorises the caller. `how_to_act` in the result says
    /// how.
    #[tool(
        description = "The invoicd exception queues: receipts whose answer never went out, and INVOICs that could not be processed at all",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_exceptions(&self) -> Result<CallToolResult, McpError> {
        let undispatched = sqlx::query(
            r"SELECT id, process_id, pid, sender_mp_id, outcome, invoice_ref, pay_by
              FROM invoic_receipts
              WHERE tenant = $1 AND dispatched_at IS NULL
              ORDER BY pay_by ASC NULLS LAST
              LIMIT 50",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let dlq = sqlx::query(
            r"SELECT id, process_id, pid, malo_id, failure_reason, failed_at
              FROM invoic_dlq
              WHERE tenant = $1 AND resolved_at IS NULL
              ORDER BY failed_at ASC
              LIMIT 50",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        use sqlx::Row;
        use time::format_description::well_known::Rfc3339;
        let undispatched: Vec<serde_json::Value> = undispatched
            .iter()
            .map(|r| {
                serde_json::json!({
                    "receipt_id":   r.try_get::<uuid::Uuid, _>("id").ok(),
                    "process_id":   r.try_get::<uuid::Uuid, _>("process_id").ok(),
                    "pid":          r.try_get::<i16, _>("pid").ok(),
                    "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                    "outcome":      r.try_get::<String, _>("outcome").ok(),
                    "invoice_ref":  r.try_get::<Option<String>, _>("invoice_ref").ok().flatten(),
                    "pay_by":       r.try_get::<Option<time::OffsetDateTime>, _>("pay_by").ok()
                        .flatten().and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();
        let dead_letters: Vec<serde_json::Value> = dlq
            .iter()
            .map(|r| {
                serde_json::json!({
                    "dlq_id":     r.try_get::<uuid::Uuid, _>("id").ok(),
                    "process_id": r.try_get::<Option<uuid::Uuid>, _>("process_id").ok().flatten(),
                    "pid":        r.try_get::<Option<i16>, _>("pid").ok().flatten(),
                    "malo_id":    r.try_get::<Option<String>, _>("malo_id").ok().flatten(),
                    "reason":     r.try_get::<String, _>("failure_reason").ok(),
                    "failed_at":  r.try_get::<time::OffsetDateTime, _>("failed_at").ok()
                        .and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "undispatched_answers": undispatched,
            "undispatched_count":   undispatched.len(),
            "dead_letters":         dead_letters,
            "dead_letter_count":    dead_letters.len(),
            "how_to_act": "Re-send an answer with POST /api/v1/receipts/{receipt_id}/dispatch-answer (operator bearer token, LF role). A dead letter has no receipt yet: fix the cause named in `reason`, then have marktd redeliver the process event.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl InvoicdMcpHandler {
    #[prompt(
        name = "resolve-dispute",
        description = "Step-by-step: investigate and resolve an INVOIC dispute (REMADV 33002)"
    )]
    async fn resolve_dispute_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "An INVOIC was disputed with REMADV 33002. How do I investigate?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_disputes` to find all outstanding disputes.\n\
                 2. Use `get_receipt` with the receipt UUID for full details.\n\
                 3. Use `get_check_result` to see which invoic-checker rule(s) failed:\n\
                    - Check 1: billing period validity (Liefer- vs. Abrechnungszeitraum)\n\
                    - Check 2: position arithmetic (qty × price = line net within 1%)\n\
                    - Check 3: document total (Σ lines = Gesamtnetto within 1%)\n\
                    - Check 4: tariff match (PRICAT unit price vs INVOIC within 3%)\n\
                    - Check 5: tariff found (PRICAT entry exists for billing period)\n\
                    - Check 6 (Strom MMM): MMMA reference vs INVOIC Mehrmengen/Mindermengen price\n\
                    - Check 6 (Gas MMM): Trading Hub Europe MMMA Gas reference\n\
                    - Check 6 (31009 AufAbschlag): discount positions vs contracted AufAbschlag\n\
                 4. Resolve upstream: update PRICAT in productd, correct Messreihe in edmd,\n\
                    or contact the NB for a corrected invoice.\n\
                 5. Record resolution: POST /api/v1/receipts/{id}/resolve-dispute with a note.\n\
                 6. Request corrected INVOIC from the NB or re-issue selbstausgestellt (PID 31006).",
            ),
        ]
    }

    #[prompt(
        name = "check-overdue-remadv",
        description = "Step-by-step: monitor and action overdue REMADV dispatches"
    )]
    async fn check_overdue_remadv_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I find and action overdue REMADV dispatches?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_overdue_remadv` — returns receipts past Zahlungsziel without dispatched REMADV.\n\
                 2. For each overdue receipt:\n\
                    a. Use `get_receipt` to confirm Zahlungsziel and current status.\n\
                    b. POST /api/v1/receipts/{id}/dispatch-answer re-sends it (LF role).\n\
                 3. The REMADV (33001 accept / 33002 dispute) is sent via makod EDIFACT pipeline.\n\
                 4. § 147 AO / GoBD: REMADV must be dispatched within the payment term.\n\
                    Missed dispatches are a compliance violation — escalate to operations.",
            ),
        ]
    }

    #[prompt(
        name = "monthly-billing-review",
        description = "Guided monthly INVOIC billing reconciliation (§ 147 AO / GoBD)"
    )]
    async fn monthly_billing_review_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "I need to perform my monthly INVOIC billing review for § 147 AO / GoBD compliance.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "Monthly INVOIC billing review steps:\n\
                 \n\
                 **1. Volume check**\n\
                 Call `summarize_billing_month` with the target year/month.\n\
                 Verify total receipt count matches expected NB billing volume.\n\
                 Alert if any NB has dispute_rate > 5%.\n\
                 \n\
                 **2. Dispute triage**\n\
                 Call `list_disputes` and triage each open dispute:\n\
                 - Check 4 failures → compare against productd PreisblattNetznutzung\n\
                 - Check 6 failures → verify marktd MMMA prices are imported for the month\n\
                 - Period failures → verify edmd has complete Lastgang for the billing period\n\
                 \n\
                 **3. Overdue REMADV**\n\
                 Call `list_overdue_remadv` — any result here is a § 147 AO / GoBD gap.\n\
                 POST /api/v1/receipts/{id}/dispatch-answer re-sends the answer for each of them.\n\
                 \n\
                 **4. Payment confirmation**\n\
                 For accepted invoices past Zahlungsziel: query `get_zahlungsstatus` per MaLo.\n\
                 Overdue unpaid items should trigger dunning via accountingd.\n\
                 \n\
                 **5. Retention audit**\n\
                 Query: `SELECT COUNT(*) FROM invoic_receipts WHERE received_at < now() - INTERVAL '8 years'`\n\
                 Only rows older than 8 years are eligible for deletion: a received INVOIC is a \
                 Buchungsbeleg (§ 147 Abs. 3 AO / § 14b UStG, 8-year retention since BEG IV).",
            ),
        ]
    }

    #[prompt(
        name = "detect-systematic-errors",
        description = "Detect systematic billing errors by NB counterparty"
    )]
    async fn detect_systematic_errors_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I detect if one NB is repeatedly billing us incorrectly?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "Systematic billing error detection:\n\
                 \n\
                 **Step 1: Compute dispute rate per NB**\n\
                 Call `summarize_billing_month` for the last 3 months.\n\
                 Group by `sender_mp_id` and compute `disputes / total`.\n\
                 Any NB with dispute_rate > 10% over 3+ consecutive months is a systematic offender.\n\
                 \n\
                 **Step 2: Classify dispute type**\n\
                 For the flagged NB, call `list_disputes` filtered by that `sender_mp_id`.\n\
                 For each dispute, call `get_check_result` and classify the finding kind:\n\
                 - Repeated `TariffDeviation` (check 4) → NB using wrong tariff in their billing system\n\
                 - Repeated `PeriodInvalid` (check 1) → NB has wrong billing period in their ERP\n\
                 - Repeated `TotalMismatch` (check 3) → rounding error in NB billing system\n\
                 - Repeated `MmmPriceDeviation` (check 6) → NB using stale MMMA prices\n\
                 \n\
                 **Step 3: Root-cause confirmation**\n\
                 - Check 4 systematic: verify `productd` has current PreisblattNetznutzung for the NB.\n\
                   If yes, the error is on the NB's side — send formal Beanstandungsschreiben.\n\
                 - Check 6 systematic: call `marktd GET /api/v1/mmm-preise/strom/{year}/{month}`\n\
                   (Gas: `/api/v1/mmma-preise/gas/{year}/{month}`) to verify the reference\n\
                   prices for the month are imported and current.\n\
                 \n\
                 **Step 4: Action**\n\
                 - Escalate to accounts-payable: request corrected invoices for all disputed months.\n\
                 - If check 4 is your error (a missing Preisblatt): import it into marktd —\n\
                   the dispute was raised against reference data you do not hold.\n\
                 - Document findings: POST /api/v1/receipts/{id}/resolve-dispute for each settled dispute.",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for InvoicdMcpHandler {
    /// The revisions every mako MCP server speaks, not the SDK's set.
    fn supported_protocol_versions(
        &self,
    ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }

    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("invoicd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "# invoicd — INVOIC Billing Validation\n\
             \n\
             Validates INVOIC billing from NB/MSB against NNE/MSB price sheets (§ 147 AO / GoBD).\n\
             Covers all German energy billing PIDs: Strom (31001/31002/31005/31006), \
             WiM Gas (31003), GaBi Gas (31007/31008), WiM MSB (31009), GeLi Gas (31011), \
             and the Sparte-neutral universal Stornorechnung (31004).\n\
             \n\
             ## Tools\n\
             - `get_receipt`             — read a receipt by UUID (outcome, timestamps)\n\
             - `list_disputes`           — list all disputed INVOIC receipts (outcome=Dispute)\n\
             - `get_check_result`        — get invoic-checker findings for a receipt\n\
             - `list_overdue_remadv`     — receipts approaching Zahlungsziel without REMADV\n\
             - `get_zahlungsstatus`      — payment status per MaLo-ID (settled/pending/overdue)\n\
             - `summarize_billing_month` — monthly volume + dispute rate per NB\n\
             - `list_exceptions`         — undispatched answers and dead-lettered INVOICs\n\
             - `list_held_receipts`      — invoices held Offen and the Prüfschritte to state\n\
             \n\
             ## Prompts\n\
             - `resolve-dispute`          — guided dispute investigation workflow\n\
             - `check-overdue-remadv`     — find and action overdue REMADV dispatches\n\
             - `monthly-billing-review`   — § 147 AO / GoBD monthly reconciliation checklist\n\
             - `detect-systematic-errors` — find NB counterparties with systematic billing errors\n\
             \n\
             ## Outcomes\n\
             - `Ok`              — all checks passed; REMADV 33001 auto-dispatched\n\
             - `Warn`            — checks passed with warnings; auto-approved\n\
             - `Dispute`         — plausibility failure; REMADV 33002; requires review\n\
             - `Resolved`        — dispute closed by an operator, who is recorded (POST /resolve-dispute)\n\
             - `AcceptedPartial` — Stornorechnung (PID 31004, any Sparte) arithmetic-only check passed\n\
             \n\
             Payment is a separate axis: `payment_confirmed_at`, not an outcome. \n\
             An invoice can be disputed and still paid, or `Ok` and unpaid.",
        )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<InvoicdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<InvoicdMcpState>, _shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)));

    let mcp_service = mako_service::mcp::service(
        {
            let state = state.clone();
            move || Ok(InvoicdMcpHandler::new(state.clone()))
        },
        config,
    );

    Router::new()
        .route_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            mcp_auth_middleware,
        ))
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    /// The served tool contract is the committed `mcp.snapshot.json`
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it); the Desk generates its types from it.
    #[test]
    fn the_tool_contract_matches_the_snapshot() {
        let tools: Vec<serde_json::Value> = InvoicdMcpHandler::tool_router()
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

    /// A dispute's Frist is the end of the Berlin day its Zahlungsziel falls
    /// on: 2026-03-30T22:00Z is 31.03. in Berlin (CEST), which ends at
    /// 22:00 UTC that day. No Zahlungsziel, no Frist.
    #[test]
    fn the_frist_is_the_end_of_the_zahlungsziel_day() {
        let row = |pay_by| {
            Disput::from_row((
                uuid::Uuid::nil(),
                uuid::Uuid::nil(),
                31002,
                "9900357000004".to_owned(),
                Some("R-4711".to_owned()),
                time::macros::datetime!(2026-03-20 10:00 UTC),
                "v202607".to_owned(),
                pay_by,
                None,
                serde_json::json!([]),
            ))
        };
        let d = row(Some(time::macros::datetime!(2026-03-30 22:00 UTC)));
        assert_eq!(d.frist.as_deref(), Some("2026-03-31T22:00:00Z"));
        assert_eq!(d.received_at, "2026-03-20T10:00:00Z");
        assert_eq!(row(None).frist, None);
    }

    /// Which decision a dispute admits is the receipt's to say: an unsent
    /// refusal is still to be sent, a sent one leaves only the resolution. And
    /// the grounds are the disputing findings, never a warning beside them.
    #[test]
    fn a_dispute_says_whether_its_refusal_went_out_and_on_what_grounds() {
        let row = |dispatched_at, findings| {
            Disput::from_row((
                uuid::Uuid::nil(),
                uuid::Uuid::nil(),
                31002,
                "9900357000004".to_owned(),
                Some("R-4711".to_owned()),
                time::macros::datetime!(2026-03-20 10:00 UTC),
                "v202607".to_owned(),
                None,
                dispatched_at,
                findings,
            ))
        };
        let findings = serde_json::json!([
            { "kind": "TariffDeviation", "is_dispute": true, "message": "Preis weicht ab" },
            { "kind": "ZahlungszielExceeded", "is_dispute": false, "message": "nur Warnung" },
        ]);
        let offen = row(None, findings.clone());
        assert!(!offen.abgesendet);
        assert_eq!(offen.befunde.len(), 1);
        assert_eq!(offen.befunde[0].art, "TariffDeviation");
        assert_eq!(
            offen.befunde[0].bezeichnung,
            "Preis weicht vom Preisblatt ab"
        );
        assert_eq!(offen.rechnungsnummer.as_deref(), Some("R-4711"));
        assert_eq!(offen.befunde[0].text, "Preis weicht ab");
        let gesendet = row(
            Some(time::macros::datetime!(2026-03-20 10:01 UTC)),
            findings,
        );
        assert!(gesendet.abgesendet);
        assert!(row(None, serde_json::Value::Null).befunde.is_empty());
    }
}
