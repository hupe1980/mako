//! HTTP handlers for `netzbilanzd`.
//!
//! Every handler returns [`mako_service::ApiResult`], so failures render as one
//! JSON problem body with the right status, and an internal error is logged
//! rather than echoed. Hand-rolled `(StatusCode, e.to_string())` tuples return
//! bare text and put database error strings — table names, constraint names,
//! sometimes the connection string — in the response body.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_markt::{makod_client::MakodClient, marktd_client::MarktdClient};
use mako_service::{ApiError, ApiResult};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::billing::{self, Resolver};
use crate::config::NetzbilanzConfig;
use crate::pg::{self, AuditQuery, DraftFilter, InsertDraftError, NewDraft};
use crate::request::{BillingRunRequest, SettlementRequest};

/// Shorthand for the handler's shared state.
type Cfg = Extension<Arc<NetzbilanzConfig>>;

// Every JSON request body in this service is `deny_unknown_fields`, for the
// reason [`crate::request`] spells out: a field that is accepted and ignored is
// a charge that silently does not happen. A misspelt `konzessionsabgabe` on a
// GGV run drops the Konzessionsabgabe; a misspelt `dispatch_kwh_override` on a
// Kostenblatt compute falls back to `edmd` without saying so. Query strings are
// deliberately not strict — an unknown query parameter is a cache-buster or a
// proxy artefact, not a missing charge.

/// Positions one `POST /billing/run` may carry.
///
/// A GGV building, a Turnus batch or a monthly MMM sweep all sit far below
/// this; anything above it is a portfolio job that belongs in several runs.
const MAX_POSITIONS_PER_RUN: usize = 1_000;

// ── CloudEvents ───────────────────────────────────────────────────────────────

/// Enqueue a business CloudEvent in the caller's transaction.
///
/// The event and the write it describes commit together; the outbox worker
/// delivers it with at-least-once semantics, signing and dead-lettering.
async fn emit(
    conn: &mut sqlx::PgConnection,
    cfg: &NetzbilanzConfig,
    ce_type: &'static str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    if cfg.erp_webhook_url.is_none() {
        // No worker runs without a URL, so enqueuing would grow the table forever.
        return Ok(());
    }
    let ce = mako_service::CloudEvent::new(
        mako_service::source("netzbilanzd", &cfg.tenant),
        ce_type,
        String::new(),
        payload,
    )
    .without_subject();
    mako_service::outbox::enqueue(conn, &ce).await
}

// ── POST /api/v1/billing/run ──────────────────────────────────────────────────

/// `POST /api/v1/billing/run`
///
/// Settle every position, render each as a BO4E `Rechnung`, check it, and store
/// it as a draft. The whole run is one transaction: invoice numbers, drafts and
/// `invoic.drafted` events commit together, so a failed run consumes no invoice
/// number and leaves no orphaned event.
///
/// # Errors
///
/// - `422` when a position does not describe a computable settlement.
/// - `409` when a position's MaLo, period and Prüfidentifikator are already billed.
pub async fn run_billing(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Cfg,
    Json(mut req): Json<BillingRunRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if req.positions.is_empty() {
        return Err(ApiError::bad_request("positions must not be empty"));
    }
    // The whole run is one transaction, so its size is how long a pooled
    // connection and the Rechnungskreis row lock are held. An unbounded run
    // blocks every concurrent billing job behind it for as long as it takes.
    if req.positions.len() > MAX_POSITIONS_PER_RUN {
        return Err(ApiError::bad_request(format!(
            "a billing run carries at most {MAX_POSITIONS_PER_RUN} positions ({} supplied) — \
             split it; every run is one transaction",
            req.positions.len()
        )));
    }
    let drafted = draft_positions(
        &pool,
        &marktd,
        &cfg,
        &mut req.positions,
        req.invoice_date,
        req.due_date,
        req.rechnungskreis.as_deref(),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "drafted": drafted.len(), "drafts": drafted })),
    ))
}

/// Resolve, settle, render, check and store a batch of positions in one
/// transaction, emitting one `invoic.drafted` event per draft.
///
/// The whole batch is atomic. A run that bills six of nine tenants of a §42b
/// building and reports success is worse than one that bills none: the missing
/// three are invisible, and re-running trips the double-billing guard on the six
/// that did land.
///
/// # Errors
///
/// - `409` when a position's MaLo, period and Prüfidentifikator are already billed.
/// - `422` when a position does not describe a computable settlement.
pub async fn draft_positions(
    pool: &PgPool,
    marktd: &Arc<MarktdClient>,
    cfg: &NetzbilanzConfig,
    positions: &mut [crate::request::BillingPositionRequest],
    invoice_date: time::Date,
    due_date: time::Date,
    rechnungskreis: Option<&str>,
) -> ApiResult<Vec<serde_json::Value>> {
    if due_date < invoice_date {
        return Err(ApiError::unprocessable(format!(
            "due_date {due_date} is before invoice_date {invoice_date}"
        )));
    }

    // Auto-fetch runs before the transaction opens: it is network I/O, and
    // holding a transaction across it pins a pooled connection for the duration
    // of every marktd round-trip.
    let mut resolver = Resolver::new(marktd);
    for position in &mut *positions {
        resolver
            .resolve(position)
            .await
            .map_err(|e| ApiError::unprocessable(format!("{e:#}")))?;
    }

    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    let mut drafted = Vec::with_capacity(positions.len());

    for position in &*positions {
        let settlement =
            billing::settle(position).map_err(|e| ApiError::unprocessable(format!("{e:#}")))?;

        let rechnungsnummer =
            pg::next_rechnungsnummer(&mut tx, &cfg.tenant, rechnungskreis, invoice_date.year())
                .await
                .map_err(ApiError::Internal)?;

        let abschlaege = pg::load_abschlaege(
            &mut tx,
            &cfg.tenant,
            &position.malo_id,
            &position.abschlaege,
        )
        .await
        .map_err(ApiError::Internal)?
        .map_err(|problems| {
            ApiError::unprocessable(format!(
                "these Abschlagsrechnungen cannot be deducted: {}",
                problems.join("; ")
            ))
        })?;

        let settled = billing::render_and_check(
            settlement,
            rechnungsnummer.clone(),
            invoice_date,
            due_date,
            billing::DocumentFacts {
                cadence: position.cadence,
                abschlaege,
                ..billing::DocumentFacts::default()
            },
        );
        let id = store(
            &mut tx,
            cfg,
            position,
            &settled,
            "RECHNUNG",
            None,
            None,
            invoice_date,
            due_date,
        )
        .await?;

        emit(
            &mut tx,
            cfg,
            mako_events::netzbilanz::INVOIC_DRAFTED,
            serde_json::json!({
                "draft_id": id,
                "tenant": cfg.tenant,
                "malo_id": position.malo_id,
                "rechnungsnummer": rechnungsnummer,
                "pid": settled.pid,
                "check_outcome": pg::outcome_str(settled.report.outcome),
                // What the ERP books: the gross, and what is left to collect
                // after any Abschläge. They differ whenever the invoice settles
                // payments already received, and an ERP given only one of them
                // books the wrong figure.
                "brutto_eur": settled.settlement.steuer.brutto_eur().to_string(),
                "zu_zahlen_eur": settled.zu_zahlen_eur.to_string(),
            }),
        )
        .await
        .map_err(ApiError::from)?;

        drafted.push(serde_json::json!({
            "draft_id": id,
            "malo_id": position.malo_id,
            "rechnungsnummer": rechnungsnummer,
            "pid": settled.pid,
            "sparte": sparte_code(position.settlement.sparte()),
            "check_outcome": pg::outcome_str(settled.report.outcome),
            // Surfaced, not merely stored: a run that produced ten Warn drafts
            // and said nothing is a run whose warnings nobody read.
            "check_findings": settled.report.findings,
            "settlement_warnings": settled.settlement.warnings,
            "netto_eur": settled.settlement.total_eur.to_string(),
            "steuer_eur": settled.settlement.steuer.steuer_eur.to_string(),
            "brutto_eur": settled.settlement.steuer.brutto_eur().to_string(),
            // What is owed after any Abschläge — the figure the payment run uses.
            "zu_zahlen_eur": settled.zu_zahlen_eur.to_string(),
        }));
    }

    tx.commit().await.map_err(ApiError::from)?;
    Ok(drafted)
}

/// Persist one settled invoice, translating a double-billing conflict into 409.
#[allow(clippy::too_many_arguments)]
async fn store(
    conn: &mut sqlx::PgConnection,
    cfg: &NetzbilanzConfig,
    position: &crate::request::BillingPositionRequest,
    settled: &billing::SettledInvoice,
    rechnungsart: &str,
    original_draft_id: Option<Uuid>,
    korrektur_grund: Option<&str>,
    invoice_date: time::Date,
    due_date: time::Date,
) -> ApiResult<Uuid> {
    let sparte = sparte_code(position.settlement.sparte());
    let settlement_type = format!("{:?}", settled.settlement.settlement_type);
    let draft = NewDraft {
        tenant: &cfg.tenant,
        malo_id: &position.malo_id,
        sender_mp_id: &settled.settlement.sender_mp_id,
        recipient_mp_id: &settled.settlement.recipient_mp_id,
        pid: i32::try_from(settled.pid).unwrap_or_default(),
        sparte,
        settlement_type: &settlement_type,
        period_from: position.period_from,
        period_to: position.period_to,
        rechnungsnummer: &settled.rechnungsnummer,
        invoice_date,
        due_date,
        settlement_input: serde_json::to_value(&position.settlement)
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?,
        rechnung: serde_json::to_value(&settled.rechnung)
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?,
        netto_eur_units: billing::eur_units(settled.settlement.total_eur)
            .map_err(ApiError::Internal)?,
        steuer_eur_units: billing::eur_units(settled.settlement.steuer.steuer_eur)
            .map_err(ApiError::Internal)?,
        brutto_eur_units: billing::eur_units(settled.settlement.steuer.brutto_eur())
            .map_err(ApiError::Internal)?,
        zu_zahlen_eur_units: billing::eur_units(settled.zu_zahlen_eur)
            .map_err(ApiError::Internal)?,
        steuer_kategorie: settled.settlement.steuer.kategorie.code(),
        steuer_satz_prozent: settled.settlement.steuer.satz_prozent,
        check_outcome: settled.report.outcome,
        check_findings: serde_json::to_value(&settled.report.findings)
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?,
        settlement_warnings: serde_json::to_value(&settled.settlement.warnings)
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?,
        rechnungsart,
        original_draft_id,
        korrektur_grund,
    };
    pg::insert_draft(conn, &draft).await.map_err(|e| match e {
        InsertDraftError::AlreadyBilled
        | InsertDraftError::AbschlagAlreadyBilled
        | InsertDraftError::DuplicateRechnungsnummer
        | InsertDraftError::AlreadyReversed => {
            ApiError::conflict(format!("{} ({e})", position.malo_id))
        }
        InsertDraftError::Database(e) => ApiError::Internal(e),
    })
}

/// The three amounts an invoice states, in units of 10⁻⁵ EUR.
///
/// Compared as a whole rather than field by field: a reversal that matches the
/// net but not the tax is not a reversal, and asserting on one number is how
/// that goes unnoticed.
#[derive(Debug, PartialEq, Eq)]
struct Totals {
    netto: i64,
    steuer: i64,
    brutto: i64,
}

impl Totals {
    /// The totals a settlement result carries.
    fn of(settlement: &grid_billing::SettlementResult) -> anyhow::Result<Self> {
        Ok(Self {
            netto: billing::eur_units(settlement.total_eur)?,
            steuer: billing::eur_units(settlement.steuer.steuer_eur)?,
            brutto: billing::eur_units(settlement.steuer.brutto_eur())?,
        })
    }
}

impl std::fmt::Display for Totals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "netto {} + Steuer {} = brutto {} EUR",
            billing::format_eur(self.netto),
            billing::format_eur(self.steuer),
            billing::format_eur(self.brutto),
        )
    }
}

/// The `sparte` column's value for a settlement.
const fn sparte_code(sparte: grid_billing::Sparte) -> &'static str {
    match sparte {
        grid_billing::Sparte::Strom => "STROM",
        grid_billing::Sparte::Gas => "GAS",
    }
}

// ── GET /api/v1/billing/drafts ────────────────────────────────────────────────

/// Query string for the draft listing.
#[derive(Debug, Deserialize)]
pub struct DraftsQuery {
    /// Lifecycle status.
    pub status: Option<String>,
    /// MaLo-ID.
    pub malo_id: Option<String>,
    /// Issuing party.
    pub sender_mp_id: Option<String>,
    /// Billed party.
    pub recipient_mp_id: Option<String>,
    /// BDEW Prüfidentifikator.
    pub pid: Option<i32>,
    /// `STROM` or `GAS` — PID 31002 and 31005 are shared between the Sparten,
    /// so the Prüfidentifikator alone cannot separate them.
    pub sparte: Option<String>,
    /// `invoic-checker` verdict.
    pub check_outcome: Option<String>,
    /// `RECHNUNG` / `STORNORECHNUNG` / `KORREKTURRECHNUNG`.
    pub rechnungsart: Option<String>,
    /// `next_cursor` from the previous page.
    pub after: Option<String>,
    /// Maximum rows (capped at 1 000).
    pub limit: Option<i64>,
}

/// Normalise a caller-supplied Sparte to the stored code, or refuse it.
///
/// `sparte=strom` and `sparte=Strom` are the same question; `sparte=STORM` is a
/// typo that would otherwise return an empty page and read as "no gas invoices
/// exist".
fn sparte_filter(raw: Option<&str>) -> ApiResult<Option<&'static str>> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v.eq_ignore_ascii_case("strom") => Ok(Some("STROM")),
        Some(v) if v.eq_ignore_ascii_case("gas") => Ok(Some("GAS")),
        Some(other) => Err(ApiError::bad_request(format!(
            "sparte must be Strom or Gas, not {other:?}"
        ))),
    }
}

/// Parse a page cursor, refusing a malformed one rather than silently
/// restarting at page one — a caller walking pages would loop forever.
fn cursor(raw: Option<&str>) -> ApiResult<Option<pg::Cursor>> {
    match raw {
        None => Ok(None),
        Some(raw) => pg::Cursor::parse(raw).map(Some).ok_or_else(|| {
            ApiError::bad_request("after must be a next_cursor from a previous page")
        }),
    }
}

/// The cursor that resumes after the last row of a page, if the page is full.
///
/// A short page is the last one, and handing back a cursor there invites one
/// more round-trip that returns nothing.
fn next_cursor(rows: &[pg::DraftSummaryRow], limit: i64) -> Option<String> {
    let last = rows.last()?;
    (i64::try_from(rows.len()).unwrap_or(i64::MAX) >= limit)
        .then(|| pg::Cursor::encode(last.created_at, last.id))
        .flatten()
}

/// `GET /api/v1/billing/drafts`
///
/// # Errors
///
/// Propagates database failures as 500.
pub async fn list_drafts(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Query(q): Query<DraftsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1_000);
    let filter = DraftFilter {
        status: q.status.as_deref(),
        malo_id: q.malo_id.as_deref(),
        sender_mp_id: q.sender_mp_id.as_deref(),
        recipient_mp_id: q.recipient_mp_id.as_deref(),
        pid: q.pid,
        sparte: sparte_filter(q.sparte.as_deref())?,
        check_outcome: q.check_outcome.as_deref(),
        rechnungsart: q.rechnungsart.as_deref(),
        after: cursor(q.after.as_deref())?,
        limit,
    };
    let rows = pg::list_drafts(&pool, &cfg.tenant, &filter)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({
        "count": rows.len(),
        "next_cursor": next_cursor(&rows, limit),
        "drafts": rows,
    })))
}

/// `GET /api/v1/billing/drafts/{id}`
///
/// # Errors
///
/// `404` when the draft does not exist for this tenant.
pub async fn get_draft(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<pg::DraftRow>> {
    pg::fetch_draft(&pool, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

// ── PUT /api/v1/billing/drafts/{id}/dispatch ─────────────────────────────────

/// `PUT /api/v1/billing/drafts/{id}/dispatch`
///
/// Attaches any Fremdkosten, re-checks the document that will actually leave the
/// house, then hands it to `makod`. The re-check runs on the amended document,
/// not on the verdict stored at drafting time — that one describes a document
/// the counterparty never sees.
///
/// # Errors
///
/// - `404` when the draft does not exist for this tenant.
/// - `409` when it is no longer in `draft` status.
/// - `422` when the checker disputes the document, or `makod` refuses it.
pub async fn dispatch_draft(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    let outcome = dispatch_one(&mut tx, &makod, &cfg, id).await?;
    emit(
        &mut tx,
        &cfg,
        mako_events::netzbilanz::INVOIC_DISPATCHED,
        serde_json::json!({
            "draft_id": id,
            "tenant": cfg.tenant,
            "dispatch_ref": outcome.dispatch_ref,
            "rechnungsnummer": outcome.rechnungsnummer,
        }),
    )
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "draft_id": id,
        "status": "dispatched",
        "dispatch_ref": outcome.dispatch_ref,
        "rechnungsnummer": outcome.rechnungsnummer,
        "check_outcome": outcome.check_outcome,
    })))
}

/// What a successful dispatch produced.
struct DispatchOutcome {
    dispatch_ref: String,
    rechnungsnummer: String,
    check_outcome: &'static str,
}

/// Validate, amend and dispatch one draft inside an open transaction.
async fn dispatch_one(
    conn: &mut sqlx::PgConnection,
    makod: &MakodClient,
    cfg: &NetzbilanzConfig,
    id: Uuid,
) -> ApiResult<DispatchOutcome> {
    let row = pg::fetch_draft(&mut *conn, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;

    if row.status != "draft" {
        return Err(ApiError::conflict(format!(
            "draft is already {}",
            row.status
        )));
    }

    let mut rechnung: rubo4e::current::Rechnung = serde_json::from_value(row.rechnung.clone())
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e).context("stored Rechnung")))?;

    // BO4E models external cost pass-through as a first-class `fremdkosten`
    // field. It does not need to travel as a free-text ZusatzAttribut, and the
    // LF's own parser reads the typed field.
    if let Some(fk) = pg::fetch_fremdkosten(&mut *conn, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
    {
        let typed: rubo4e::current::Fremdkosten = serde_json::from_value(fk.fremdkosten_json)
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e).context("stored Fremdkosten")))?;
        rechnung.fremdkosten = Some(Box::new(typed));
    }

    let pid = u32::try_from(row.pid).unwrap_or_default();
    let report = billing::check(&rechnung, &row.sender_mp_id, pid);
    if report.outcome == invoic_checker::CheckOutcome::Dispute {
        return Err(ApiError::unprocessable(format!(
            "invoic-checker disputes this invoice — fix it before dispatching: {}",
            report
                .findings
                .iter()
                .filter(|f| f.is_dispute)
                .map(|f| f.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    let command = makod_command(pid, &row.sparte)?;
    let payload =
        serde_json::to_value(&rechnung).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    let accepted = makod
        .post_command(
            &format!("netzbilanzd-invoic-{id}"),
            &mako_markt::makod_client::ForwardCommand {
                command: command.to_owned(),
                marktrolle: Some(marktrolle_for(pid, &row.sparte).to_owned()),
                malo_id: Some(row.malo_id.clone()),
                melo_id: None,
                payload: serde_json::json!({
                    // The business key the inbound REMADV correlates on. It has
                    // to be the number printed on the document the counterparty
                    // received, and makod rejects the command without it.
                    "invoice_ref":     row.rechnungsnummer,
                    "sender_mp_id":    row.sender_mp_id,
                    "recipient_mp_id": row.recipient_mp_id,
                    "pid":             pid,
                    "sparte":          row.sparte,
                    "rechnung":        payload,
                }),
            },
        )
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, %id, "netzbilanzd: makod refused the INVOIC");
            ApiError::Unprocessable(format!("dispatch to makod failed: {e}"))
        })?;

    let dispatch_ref = accepted.process_id.to_string();
    let stored =
        serde_json::to_value(&rechnung).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    let findings = serde_json::to_value(&report.findings)
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    // The stored document is now the dispatched one, so the stored verdict has
    // to be the verdict on *it*. Keeping the drafting verdict beside an amended
    // document would describe something that was never sent.
    if !pg::mark_dispatched(
        &mut *conn,
        &cfg.tenant,
        id,
        &dispatch_ref,
        &stored,
        report.outcome,
        &findings,
    )
    .await
    .map_err(ApiError::Internal)?
    {
        return Err(ApiError::conflict("draft changed status concurrently"));
    }

    Ok(DispatchOutcome {
        dispatch_ref,
        rechnungsnummer: row.rechnungsnummer,
        check_outcome: pg::outcome_str(report.outcome),
    })
}

/// The `makod` command that carries a given Prüfidentifikator and Sparte.
///
/// NN-Rechnung Strom and Gas share PID 31002 but not the command: the Gas one is
/// permitted for the `GNB` role, so a gas network operator's deployment would be
/// refused the Strom command on role grounds alone.
fn makod_command(pid: u32, sparte: &str) -> ApiResult<&'static str> {
    match (pid, sparte) {
        // A payment on account. One command for both Sparten: the Abschlag
        // prices no energy, so nothing about it is Sparte-specific.
        (31001, _) => Ok("gpke.nne-abschlag.rechnung.stellen"),
        (31002, "GAS") => Ok("gpke.nne-gas.rechnung.stellen"),
        (31002, _) => Ok("gpke.nne.rechnung.stellen"),
        (31005, _) => Ok("gpke.mmm.rechnung.stellen"),
        (31009, _) => Ok("wim.msb-rechnung.stellen"),
        // BK7-24-01-009 §5.4 — the GNB bills the LFG for abrechnungswürdige
        // Handlungen performed during the Sperrprozess.
        (31011, _) => Ok("geli.gas.awh-rechnung.stellen"),
        (other, _) => Err(ApiError::Internal(anyhow::anyhow!(
            "no makod command for Prüfidentifikator {other}"
        ))),
    }
}

/// Which market role issues a given Prüfidentifikator and Sparte.
///
/// For a command permitted to more than one role, `makod` checks the assertion
/// against the deployment's licensed roles. Three of the six here are permitted
/// to `NB` **and** `GNB` (Abschlag, NN-Rechnung Gas, GeLi Gas AWH), so a gas
/// invoice asserts `GNB`: a `--marktrollen GNB` deployment is the only kind that
/// issues those three, and asserting `NB` fails its licence check. Picking the
/// Gas *command* is only half of it; the role follows the Sparte too.
///
/// PID 31009 is the other direction: the Messstellenbetreiber issues it in all
/// seven of its Anwendungsfälle, so the assertion is `MSB` regardless of Sparte.
fn marktrolle_for(pid: u32, sparte: &str) -> &'static str {
    match (pid, sparte) {
        (31009, _) => "MSB",
        // 31011 is Gas by construction; 31001 and 31002 carry both Sparten.
        (31011, _) | (_, "GAS") => "GNB",
        _ => "NB",
    }
}

// ── PUT /api/v1/billing/drafts/{id}/reject ───────────────────────────────────

/// Request body for a rejection.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectRequest {
    /// Why the operator is discarding the draft.
    pub reason: String,
}

/// `PUT /api/v1/billing/drafts/{id}/reject`
///
/// # Errors
///
/// `404` when the draft does not exist for this tenant or is no longer a draft.
pub async fn reject_draft(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
    Json(req): Json<RejectRequest>,
) -> ApiResult<StatusCode> {
    if req.reason.trim().is_empty() {
        return Err(ApiError::bad_request("reason must not be empty"));
    }
    if pg::reject_draft(&pool, &cfg.tenant, id, &req.reason)
        .await
        .map_err(ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// ── POST /api/v1/billing/drafts/dispatch-batch ────────────────────────────────

/// Request body for a batch dispatch.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchBatchRequest {
    /// The drafts to dispatch.
    pub draft_ids: Vec<Uuid>,
}

/// `POST /api/v1/billing/drafts/dispatch-batch`
///
/// Each draft dispatches in its own transaction, so one refusal does not roll
/// back the ones that already went out.
///
/// # Errors
///
/// `400` for an empty or oversized batch.
pub async fn post_dispatch_batch(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Extension(cfg): Cfg,
    Json(req): Json<DispatchBatchRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if req.draft_ids.is_empty() {
        return Err(ApiError::bad_request("draft_ids must not be empty"));
    }
    if req.draft_ids.len() > 500 {
        return Err(ApiError::bad_request("batch size must not exceed 500"));
    }

    let mut succeeded = Vec::new();
    let mut failures = Vec::new();
    for id in req.draft_ids {
        let mut tx = pool.begin().await.map_err(ApiError::from)?;
        match dispatch_one(&mut tx, &makod, &cfg, id).await {
            Ok(outcome) => {
                let committed = async {
                    emit(
                        &mut tx,
                        &cfg,
                        mako_events::netzbilanz::INVOIC_DISPATCHED,
                        serde_json::json!({
                            "draft_id": id,
                            "tenant": cfg.tenant,
                            "dispatch_ref": outcome.dispatch_ref,
                            "rechnungsnummer": outcome.rechnungsnummer,
                        }),
                    )
                    .await?;
                    tx.commit().await
                }
                .await;
                match committed {
                    Ok(()) => succeeded.push(serde_json::json!({
                        "draft_id": id,
                        "dispatch_ref": outcome.dispatch_ref,
                        "rechnungsnummer": outcome.rechnungsnummer,
                    })),
                    Err(e) => failures.push(failure(id, &e.to_string())),
                }
            }
            Err(e) => {
                // The transaction rolls back on drop, so a refused dispatch
                // leaves the draft exactly as it was.
                failures.push(failure(id, &e.to_string()));
            }
        }
    }

    let status = if failures.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };
    Ok((
        status,
        Json(serde_json::json!({
            "succeeded": succeeded.len(),
            "failed": failures.len(),
            "dispatched": succeeded,
            "failures": failures,
        })),
    ))
}

fn failure(id: Uuid, reason: &str) -> serde_json::Value {
    serde_json::json!({ "draft_id": id, "reason": reason })
}

// ── POST /api/v1/billing/drafts/{id}/storno ───────────────────────────────────

/// Request body for a Stornorechnung.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StornoRequest {
    /// Why the settlement is being reversed.
    pub grund: grid_billing::KorrekturGrund,
    /// Issue date of the Stornorechnung. Defaults to today.
    #[serde(default)]
    pub invoice_date: Option<time::Date>,
    /// Payment due date. Defaults to 30 days after the issue date.
    #[serde(default)]
    pub due_date: Option<time::Date>,
}

/// `POST /api/v1/billing/drafts/{id}/storno`
///
/// Reverses a settled invoice by **recomputing** it from its stored input and
/// negating the result through `grid_billing::reverse`, then rendering that as
/// a Stornorechnung with its own invoice number.
///
/// Recomputing is what makes the reversal honest. Editing the stored document
/// instead — negating the total and leaving the positions positive — produces a
/// Storno whose parts do not sum to its whole, and one that never sets
/// `ist_storno` or `original_rechnungsnummer` is not a Storno to the receiving
/// LF at all: it runs the same `invoic-checker`, and stage 0 reads exactly those
/// two fields.
///
/// # Errors
///
/// - `404` when the original does not exist for this tenant.
/// - `409` when the original was never dispatched or was rejected (reject it, or
///   leave it rejected, and bill again), when it is already reversed, or when
///   replaying its stored input no longer reproduces the amounts it was issued
///   for.
pub async fn post_storno(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
    Json(req): Json<StornoRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut tx = pool.begin().await.map_err(ApiError::from)?;

    let (original, input) = pg::load_settlement_input(&mut tx, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;

    if original.rechnungsart != "RECHNUNG" {
        return Err(ApiError::conflict(
            "only an original RECHNUNG can be reversed",
        ));
    }
    // `draft` and `rejected` both mean "never left the house". Reversing either
    // issues a credit note against an invoice no counterparty received, booking
    // a negative amount an ERP will happily pay out.
    if matches!(original.status.as_str(), "draft" | "rejected") {
        return Err(ApiError::conflict(format!(
            "this invoice is '{}' — it was never dispatched, so there is nothing to reverse. \
             Reject the draft (or leave it rejected) and bill the period again.",
            original.status
        )));
    }

    let invoice_date = req
        .invoice_date
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    let due_date = req
        .due_date
        .unwrap_or_else(|| invoice_date.saturating_add(time::Duration::days(30)));

    let position = crate::request::BillingPositionRequest {
        malo_id: original.malo_id.clone(),
        period_from: original.period_from,
        period_to: original.period_to,
        // A reversal mirrors the original document, and the original's own
        // Abschlag deductions are cancelled with it rather than re-applied.
        cadence: None,
        abschlaege: Vec::new(),
        settlement: input,
    };
    let recomputed =
        billing::settle(&position).map_err(|e| ApiError::unprocessable(format!("{e:#}")))?;

    // A reversal has to negate the invoice that was actually sent — exactly.
    // Recomputation normally reproduces it, but the engine reads tabled figures
    // (the EnFG levy rates for the delivery year, the KAV ceilings, the
    // regulatory regime for the period), and a table corrected since the
    // original was issued would produce a near-miss: a Storno that cancels most
    // of an invoice and silently leaves a residue nothing downstream reconciles.
    //
    // So it is checked, and a mismatch is refused rather than rounded past.
    //
    // All three amounts are compared, not only the net. The tax is derived from
    // facts the input carries (the §13b Wiederverkäufer status, the rate window
    // the period falls in), so a corrected table can leave the net identical and
    // the tax different — and a reversal that cancels the net but not the
    // Umsatzsteuer leaves a §14c Abs. 1 liability standing.
    let replayed = Totals::of(&recomputed).map_err(ApiError::Internal)?;
    let issued = Totals {
        netto: original.netto_eur_units,
        steuer: original.steuer_eur_units,
        brutto: original.brutto_eur_units,
    };
    if replayed != issued {
        return Err(ApiError::conflict(format!(
            "recomputing this invoice from its stored settlement input yields {replayed}, but it \
             was issued for {issued} — a reversal must negate what was sent, exactly. Something \
             the engine reads has changed since (a published levy rate, a statutory ceiling, a \
             tax-rate window, the regime for the period). Investigate before reversing."
        )));
    }

    let reversal = grid_billing::reverse(&recomputed, req.grund);

    let rechnungsnummer = pg::next_rechnungsnummer(
        &mut tx,
        &cfg.tenant,
        rechnungskreis_of(&original.rechnungsnummer),
        invoice_date.year(),
    )
    .await
    .map_err(ApiError::Internal)?;

    let settled = billing::render_and_check(
        reversal,
        rechnungsnummer.clone(),
        invoice_date,
        due_date,
        billing::DocumentFacts {
            correction_of: Some(original.rechnungsnummer.clone()),
            ..billing::DocumentFacts::default()
        },
    );
    let storno_id = store(
        &mut tx,
        &cfg,
        &position,
        &settled,
        "STORNORECHNUNG",
        Some(id),
        Some(req.grund.code()),
        invoice_date,
        due_date,
    )
    .await?;

    emit(
        &mut tx,
        &cfg,
        mako_events::netzbilanz::INVOIC_DRAFTED,
        serde_json::json!({
            "draft_id": storno_id,
            "tenant": cfg.tenant,
            "rechnungsart": "STORNORECHNUNG",
            "original_draft_id": id,
            "rechnungsnummer": rechnungsnummer,
        }),
    )
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "storno_draft_id": storno_id,
            "original_draft_id": id,
            "rechnungsnummer": rechnungsnummer,
            "original_rechnungsnummer": original.rechnungsnummer,
            "korrektur_grund": req.grund.code(),
            "total_eur": settled.settlement.total_eur.to_string(),
            "next": "review, then PUT /api/v1/billing/drafts/{storno_draft_id}/dispatch",
        })),
    ))
}

/// Request body for a Korrekturrechnung.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KorrekturRequest {
    /// Why the settlement is being recalculated.
    pub grund: grid_billing::KorrekturGrund,
    /// The corrected settlement input. Same shape as a billing-run position.
    pub settlement: SettlementRequest,
    /// The billing cadence of the corrected document.
    #[serde(default)]
    pub cadence: Option<grid_billing::Rechnungscharakter>,
    /// Abschlagsrechnungen the corrected invoice settles, by draft ID.
    #[serde(default)]
    pub abschlaege: Vec<Uuid>,
    /// Issue date. Defaults to today.
    #[serde(default)]
    pub invoice_date: Option<time::Date>,
    /// Payment due date. Defaults to 30 days after the issue date.
    #[serde(default)]
    pub due_date: Option<time::Date>,
}

/// `POST /api/v1/billing/drafts/{id}/korrektur`
///
/// Issues the corrected invoice for a period. A correction is a **new
/// settlement**, computed from corrected inputs by the same engine — not an
/// operator-supplied `Rechnung` JSON blob.
///
/// **The original must be reversed first.** A Korrekturrechnung here carries the
/// *whole* corrected amount, not the difference, so issuing one against a live
/// original bills the period twice — and both documents are well-formed, so
/// nothing downstream notices. The Storno is what makes the pair net out, which
/// is the Storno-und-Neuberechnung flow German accounting expects. A draft that
/// was never dispatched needs no correction at all: reject it and bill again.
///
/// **And it must correct the same invoice.** `original_draft_id` says which one,
/// but the corrected settlement arrives from the caller: it could name a
/// different settlement kind, Sparte or counterparty. Corrections are exempt
/// from the double-billing guard, so such a document would be stored without
/// complaint — a second, unrelated invoice wearing a correction's clothes.
///
/// # Errors
///
/// - `404` when the original does not exist for this tenant.
/// - `409` when the original is still live — reverse it first — or was never
///   dispatched at all.
/// - `422` when the corrected inputs do not describe a computable settlement,
///   or describe a different invoice than the one being corrected.
pub async fn post_korrektur(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
    Json(req): Json<KorrekturRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    let original = pg::fetch_draft(&mut *tx, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;

    if original.rechnungsart != "RECHNUNG" {
        return Err(ApiError::conflict(
            "corrections attach to the original RECHNUNG, not to another correction",
        ));
    }
    if matches!(original.status.as_str(), "draft" | "rejected") {
        return Err(ApiError::conflict(format!(
            "this invoice is '{}' — it was never dispatched, so there is nothing to correct. \
             Reject the draft and run the billing again.",
            original.status
        )));
    }
    if !pg::has_storno(&mut tx, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
    {
        return Err(ApiError::conflict(
            "reverse the original first (POST /storno). This Korrekturrechnung carries the \
             whole corrected amount, not the difference, so issuing it against a live invoice \
             bills the period twice — and both documents look correct.",
        ));
    }

    let invoice_date = req
        .invoice_date
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    let due_date = req
        .due_date
        .unwrap_or_else(|| invoice_date.saturating_add(time::Duration::days(30)));

    let mut position = crate::request::BillingPositionRequest {
        malo_id: original.malo_id.clone(),
        period_from: original.period_from,
        period_to: original.period_to,
        cadence: req.cadence,
        abschlaege: req.abschlaege.clone(),
        settlement: req.settlement,
    };
    Resolver::new(&marktd)
        .resolve(&mut position)
        .await
        .map_err(|e| ApiError::unprocessable(format!("{e:#}")))?;

    let mut replacement =
        billing::settle(&position).map_err(|e| ApiError::unprocessable(format!("{e:#}")))?;

    // A correction replaces one specific document, and `original_draft_id` says
    // which. Nothing else pinned the two together: the corrected settlement is
    // a caller-supplied `SettlementRequest`, so it could name a different
    // Sparte, a different counterparty, or a different settlement kind
    // altogether — and corrections are exempt from the double-billing guard, so
    // a "correction" that is really a second, unrelated invoice would be stored
    // without complaint, linked to an original it has nothing to do with.
    let settlement_type = format!("{:?}", replacement.settlement_type);
    corrects_the_same_invoice(
        &Identity {
            settlement_type: &settlement_type,
            sparte: sparte_code(position.settlement.sparte()),
            sender_mp_id: &replacement.sender_mp_id,
            recipient_mp_id: &replacement.recipient_mp_id,
        },
        &Identity {
            settlement_type: &original.settlement_type,
            sparte: &original.sparte,
            sender_mp_id: &original.sender_mp_id,
            recipient_mp_id: &original.recipient_mp_id,
        },
    )?;

    // What `grid_billing::correct` does to the replacement, without building the
    // reversal it would return alongside — the Storno already exists, and this
    // handler refuses to run until it does.
    replacement.status = grid_billing::SettlementStatus::Correction;
    replacement.korrektur_grund = Some(req.grund);

    let rechnungsnummer = pg::next_rechnungsnummer(
        &mut tx,
        &cfg.tenant,
        rechnungskreis_of(&original.rechnungsnummer),
        invoice_date.year(),
    )
    .await
    .map_err(ApiError::Internal)?;

    let abschlaege = pg::load_abschlaege(&mut tx, &cfg.tenant, &original.malo_id, &req.abschlaege)
        .await
        .map_err(ApiError::Internal)?
        .map_err(|problems| {
            ApiError::unprocessable(format!(
                "these Abschlagsrechnungen cannot be deducted: {}",
                problems.join("; ")
            ))
        })?;
    let settled = billing::render_and_check(
        replacement,
        rechnungsnummer.clone(),
        invoice_date,
        due_date,
        billing::DocumentFacts {
            correction_of: Some(original.rechnungsnummer.clone()),
            cadence: req.cadence,
            abschlaege,
        },
    );
    let korrektur_id = store(
        &mut tx,
        &cfg,
        &position,
        &settled,
        "KORREKTURRECHNUNG",
        Some(id),
        Some(req.grund.code()),
        invoice_date,
        due_date,
    )
    .await?;

    emit(
        &mut tx,
        &cfg,
        mako_events::netzbilanz::INVOIC_DRAFTED,
        serde_json::json!({
            "draft_id": korrektur_id,
            "tenant": cfg.tenant,
            "rechnungsart": "KORREKTURRECHNUNG",
            "original_draft_id": id,
            "rechnungsnummer": rechnungsnummer,
        }),
    )
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "korrektur_draft_id": korrektur_id,
            "original_draft_id": id,
            "rechnungsnummer": rechnungsnummer,
            "korrektur_grund": req.grund.code(),
            "total_eur": settled.settlement.total_eur.to_string(),
            "check_outcome": pg::outcome_str(settled.report.outcome),
        })),
    ))
}

/// What makes two invoices the same invoice for correction purposes.
struct Identity<'a> {
    settlement_type: &'a str,
    sparte: &'a str,
    sender_mp_id: &'a str,
    recipient_mp_id: &'a str,
}

/// Refuse a "correction" that is really a different invoice.
///
/// # Errors
///
/// `422` naming the first field that differs.
fn corrects_the_same_invoice(corrected: &Identity<'_>, original: &Identity<'_>) -> ApiResult<()> {
    let differing = [
        (
            "settlement_type",
            corrected.settlement_type,
            original.settlement_type,
        ),
        ("sparte", corrected.sparte, original.sparte),
        (
            "sender_mp_id",
            corrected.sender_mp_id,
            original.sender_mp_id,
        ),
        (
            "recipient_mp_id",
            corrected.recipient_mp_id,
            original.recipient_mp_id,
        ),
    ]
    .into_iter()
    .find(|(_, now, was)| now != was);

    match differing {
        None => Ok(()),
        Some((field, now, was)) => Err(ApiError::unprocessable(format!(
            "the corrected settlement changes {field} from {was:?} to {now:?}. A \
             Korrekturrechnung re-issues the *same* invoice with corrected figures; a different \
             settlement kind, Sparte or counterparty is a different invoice, and belongs in its \
             own billing run."
        ))),
    }
}

/// The Rechnungskreis a previous invoice number was issued under.
///
/// Numbers are `[<kreis>-]<year>-<seq>`, so a correction stays in the series
/// its original belongs to rather than starting a new one.
fn rechnungskreis_of(rechnungsnummer: &str) -> Option<&str> {
    let parts: Vec<&str> = rechnungsnummer.rsplitn(3, '-').collect();
    (parts.len() == 3).then(|| parts[2])
}

// ── REMADV lifecycle ──────────────────────────────────────────────────────────

/// Request body for `mark-paid`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkPaidRequest {
    /// The reference carried on the REMADV 33001.
    pub remadv_ref: String,
}

/// `PUT /api/v1/billing/drafts/{id}/mark-paid`
///
/// REMADV **33001** is the only Zahlungsbestätigung; 33002, 33003 and 33004 are
/// all Abweisungen and belong on `mark-disputed`.
///
/// # Errors
///
/// `404` when the draft does not exist for this tenant or is not dispatched.
pub async fn mark_paid(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
    Json(req): Json<MarkPaidRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    if !pg::mark_paid(&mut tx, &cfg.tenant, id, &req.remadv_ref)
        .await
        .map_err(ApiError::Internal)?
    {
        return Err(ApiError::NotFound);
    }
    emit(
        &mut tx,
        &cfg,
        mako_events::netzbilanz::INVOIC_PAID,
        serde_json::json!({ "draft_id": id, "tenant": cfg.tenant, "remadv_ref": req.remadv_ref }),
    )
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(Json(
        serde_json::json!({ "draft_id": id, "status": "paid", "remadv_ref": req.remadv_ref }),
    ))
}

/// Request body for `mark-disputed`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkDisputedRequest {
    /// The EDIFACT ERC code from the REMADV Abweisung.
    pub erc_code: String,
    /// The counterparty's stated reason.
    pub reason: String,
}

/// `PUT /api/v1/billing/drafts/{id}/mark-disputed`
///
/// REMADV 33002/33003/33004. The dispute lands in its own status and its own
/// columns; the NB's pre-dispatch verdict is left intact, because it is the
/// evidence that says whether the invoice was defensible when it was sent.
///
/// # Errors
///
/// `404` when the draft does not exist for this tenant or is not dispatched.
pub async fn mark_disputed(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(id): Path<Uuid>,
    Json(req): Json<MarkDisputedRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    if !pg::mark_disputed(&mut tx, &cfg.tenant, id, &req.erc_code, &req.reason)
        .await
        .map_err(ApiError::Internal)?
    {
        return Err(ApiError::NotFound);
    }
    emit(
        &mut tx,
        &cfg,
        mako_events::netzbilanz::INVOIC_DISPUTED,
        serde_json::json!({
            "draft_id": id,
            "tenant": cfg.tenant,
            "erc_code": req.erc_code,
            "reason": req.reason,
        }),
    )
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "draft_id": id,
        "status": "disputed",
        "erc_code": req.erc_code,
        "next": "issue a Storno + Korrektur, or escalate via makod COMDIS 29001",
    })))
}

/// Inbound REMADV CloudEvent.
#[derive(Debug, Deserialize)]
pub struct RemadvWebhookBody {
    /// The CloudEvent type.
    #[serde(rename = "type")]
    pub ce_type: String,
    /// The event payload.
    pub data: serde_json::Value,
}

/// `POST /api/v1/webhooks/remadv`
///
/// Closes the payment lifecycle from `makod` or an ERP bridge without operator
/// intervention.
///
/// # Errors
///
/// - `401` when the HMAC signature does not verify.
/// - `400` on a malformed body or a `data.draft_id` that is not a UUID.
/// - `404` when the referenced draft is not in a state the event applies to.
pub async fn post_remadv_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> ApiResult<StatusCode> {
    // A forged REMADV would mark an invoice paid, or contest one that was not.
    if let Some(secret) = &cfg.inbound_secret {
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !mako_service::webhook::verify_hmac(secret.as_bytes(), &raw, provided) {
            tracing::warn!("netzbilanzd: inbound REMADV signature mismatch — rejected");
            return Err(ApiError::Unauthorized);
        }
    }

    let body: RemadvWebhookBody = serde_json::from_slice(&raw)
        .map_err(|e| ApiError::bad_request(format!("malformed CloudEvent: {e}")))?;
    let id: Uuid = body
        .data
        .get("draft_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ApiError::bad_request("data.draft_id must be a UUID"))?;

    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    // Delivery is at-least-once, so the same REMADV arrives more than once.
    // The status is read inside the transaction, before the transition, so a
    // replay can be told apart from an event that genuinely does not apply.
    let before = pg::fetch_draft(&mut *tx, &cfg.tenant, id)
        .await
        .map_err(ApiError::Internal)?
        .map(|row| row.status);

    let (applied, ce_type, terminal) = match body.ce_type.as_str() {
        mako_events::invoic::RECEIPT_SETTLED => {
            let remadv_ref = body
                .data
                .get("remadv_ref")
                .and_then(|v| v.as_str())
                .unwrap_or("REMADV");
            (
                pg::mark_paid(&mut tx, &cfg.tenant, id, remadv_ref)
                    .await
                    .map_err(ApiError::Internal)?,
                mako_events::netzbilanz::INVOIC_PAID,
                "paid",
            )
        }
        mako_events::invoic::RECEIPT_DISPUTED => {
            let erc = body
                .data
                .get("erc_code")
                .and_then(|v| v.as_str())
                .unwrap_or("Z00");
            let reason = body
                .data
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("REMADV Abweisung");
            (
                pg::mark_disputed(&mut tx, &cfg.tenant, id, erc, reason)
                    .await
                    .map_err(ApiError::Internal)?,
                mako_events::netzbilanz::INVOIC_DISPUTED,
                "disputed",
            )
        }
        other => {
            tracing::debug!(ce_type = other, "netzbilanzd: REMADV event not handled");
            return Ok(StatusCode::NO_CONTENT);
        }
    };

    if !applied {
        // The invoice already holds the state this event asks for, so this is a
        // redelivery and there is nothing to do; answering 404 would tell a
        // retrying sender it had failed. Anything else really is a 404: an
        // unknown draft, or one in a status the transition does not reach.
        if before.as_deref() == Some(terminal) {
            tracing::debug!(%id, ce_type = %body.ce_type, "netzbilanzd: REMADV already applied");
            return Ok(StatusCode::NO_CONTENT);
        }
        return Err(ApiError::NotFound);
    }
    emit(
        &mut tx,
        &cfg,
        ce_type,
        serde_json::json!({ "draft_id": id, "tenant": cfg.tenant }),
    )
    .await
    .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Reporting ─────────────────────────────────────────────────────────────────

/// Query string for the monthly summary.
#[derive(Debug, Deserialize)]
pub struct SummaryQuery {
    /// Calendar year. Defaults to the current one.
    pub year: Option<i32>,
    /// Calendar month. Defaults to the current one.
    pub month: Option<u8>,
}

/// `GET /api/v1/billing/summary?year=&month=`
///
/// # Errors
///
/// `400` for a month outside 1–12.
pub async fn get_billing_summary(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Query(q): Query<SummaryQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = time::OffsetDateTime::now_utc();
    let year = q.year.unwrap_or_else(|| now.year());
    let month = q.month.unwrap_or(now.month() as u8);
    if !(1..=12).contains(&month) {
        return Err(ApiError::bad_request("month must be 1–12"));
    }
    let rows = pg::billing_summary(&pool, &cfg.tenant, year, month)
        .await
        .map_err(ApiError::Internal)?;
    let sum = |f: fn(&pg::BillingSummaryRow) -> i64| rows.iter().map(f).sum::<i64>();
    Ok(Json(serde_json::json!({
        "year": year,
        "month": month,
        // Formatted from the integers, never through an f64: these are the
        // figures a month-end reconciliation is checked against.
        "total_netto_eur":  billing::format_eur(sum(|r| r.netto_eur_units)),
        "total_steuer_eur": billing::format_eur(sum(|r| r.steuer_eur_units)),
        "total_brutto_eur": billing::format_eur(sum(|r| r.brutto_eur_units)),
        // What is left to collect. On a portfolio with Abschläge this is below
        // the gross, and it is the figure a month-end reconciliation wants —
        // the gross is what was invoiced, not what anyone will pay.
        "total_zu_zahlen_eur": billing::format_eur(sum(|r| r.zu_zahlen_eur_units)),
        "by_group": rows,
    })))
}

/// Query string for the audit export.
#[derive(Debug, Deserialize)]
pub struct AuditExportQuery {
    /// Earliest delivery-period start.
    pub from: Option<time::Date>,
    /// Latest delivery-period end.
    pub to: Option<time::Date>,
    /// BDEW Prüfidentifikator.
    pub pid: Option<i32>,
    /// Lifecycle status.
    pub status: Option<String>,
    /// `next_cursor` from the previous page.
    pub after: Option<String>,
    /// Maximum rows (capped at 50 000).
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/audit`
///
/// § 147 Abs. 3 AO / § 14b UStG export. Invoices are Buchungsbelege: eight
/// years, reduced from ten with effect from 01.01.2025.
///
/// # Errors
///
/// Propagates database failures as 500.
pub async fn get_billing_audit(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Query(q): Query<AuditExportQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(10_000).clamp(1, 50_000);
    let rows = pg::list_audit(
        &pool,
        &AuditQuery {
            tenant: cfg.tenant.clone(),
            from: q.from,
            to: q.to,
            pid: q.pid,
            status: q.status,
            after: cursor(q.after.as_deref())?,
            limit,
        },
    )
    .await
    .map_err(ApiError::Internal)?;
    let next = rows.last().and_then(|last| {
        (i64::try_from(rows.len()).unwrap_or(i64::MAX) >= limit)
            .then(|| pg::Cursor::encode(last.created_at, last.id))
            .flatten()
    });
    Ok(Json(serde_json::json!({
        "count": rows.len(),
        "next_cursor": next,
        "records": rows,
        "retention": "§ 147 Abs. 3 AO / § 14b UStG — invoices are Buchungsbelege, 8 years",
        "full_document": "GET /api/v1/billing/drafts/{id}",
    })))
}

/// Query string for the per-MaLo history.
#[derive(Debug, Deserialize)]
pub struct MaloHistoryQuery {
    /// Maximum rows (capped at 1 000).
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/malo/{malo_id}`
///
/// # Errors
///
/// Propagates database failures as 500.
pub async fn get_malo_billing_history(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(malo_id): Path<String>,
    Query(q): Query<MaloHistoryQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::billing_history_for_malo(
        &pool,
        &cfg.tenant,
        &malo_id,
        q.limit.unwrap_or(100).clamp(1, 1_000),
    )
    .await
    .map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({
        "malo_id": malo_id,
        "count": rows.len(),
        "records": rows,
    })))
}

// ── Fremdkosten ───────────────────────────────────────────────────────────────

/// `PUT /api/v1/billing/fremdkosten/{draft_id}`
///
/// Attaches typed external costs to a draft. They are merged into the
/// `Rechnung`'s own `fremdkosten` field at dispatch.
///
/// Fremdkosten are **informational**: BO4E models them as a cost breakdown
/// beside the invoice rather than positions that add to it, so attaching them
/// changes what the document explains and not what it charges. Third-party
/// costs the counterparty actually owes belong in the settlement.
///
/// Only a `draft` accepts them, because the merge happens at dispatch. Taking
/// them on a dispatched or paid invoice stored costs that would never reach the
/// counterparty, and `GET` would then show a document nobody was sent.
///
/// # Errors
///
/// - `404` when the draft does not exist for this tenant.
/// - `409` when the draft has already left the house.
/// - `422` when the payload is not a `rubo4e::current::Fremdkosten`.
pub async fn put_fremdkosten(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(draft_id): Path<Uuid>,
    Json(req): Json<pg::UpsertFremdkostenRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Typed round-trip: the stored JSON has to *be* a Fremdkosten, not merely
    // carry the right `_typ`, because dispatch deserialises it into the document.
    serde_json::from_value::<rubo4e::current::Fremdkosten>(req.fremdkosten_json.clone())
        .map_err(|e| ApiError::unprocessable(format!("invalid Fremdkosten payload: {e}")))?;

    // Checked before the insert so a missing draft is a 404 rather than a
    // foreign-key violation matched on its error text.
    let draft = pg::fetch_draft(&pool, &cfg.tenant, draft_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    if draft.status != "draft" {
        return Err(ApiError::conflict(format!(
            "this invoice is '{}' — Fremdkosten are merged into the document at dispatch, so \
             attaching them now would store costs the counterparty never receives. Reverse and \
             re-issue the invoice if they belong on it.",
            draft.status
        )));
    }

    let id = pg::upsert_fremdkosten(&pool, &cfg.tenant, draft_id, &req)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({
        "id": id,
        "draft_id": draft_id,
        "total_eur": req.total_eur.to_string(),
        "applied": "merged into Rechnung.fremdkosten on dispatch",
    })))
}

/// `GET /api/v1/billing/fremdkosten/{draft_id}`
///
/// # Errors
///
/// `404` when no Fremdkosten are attached for this tenant.
pub async fn get_fremdkosten(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(draft_id): Path<Uuid>,
) -> ApiResult<Json<pg::FremdkostenRow>> {
    pg::fetch_fremdkosten(&pool, &cfg.tenant, draft_id)
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A correction stays in the series its original was issued under.
    #[test]
    fn a_correction_keeps_the_rechnungskreis() {
        assert_eq!(rechnungskreis_of("NNE-2026-000001"), Some("NNE"));
        assert_eq!(rechnungskreis_of("NNE-GAS-2026-000001"), Some("NNE-GAS"));
        // No Rechnungskreis was named, so the correction names none either.
        assert_eq!(rechnungskreis_of("2026-000001"), None);
    }

    /// The asserted Marktrolle follows the PID **and** the Sparte.
    ///
    /// `makod` checks the assertion against the deployment's licensed roles for
    /// any command permitted to more than one, so a flat `NB` would lock a gas
    /// network operator out of the gas invoices it alone issues.
    #[test]
    fn the_marktrolle_follows_the_pid_and_the_sparte() {
        // 31009 is inverted: the MSB issues it, in either Sparte.
        assert_eq!(marktrolle_for(31009, "STROM"), "MSB");
        assert_eq!(marktrolle_for(31009, "GAS"), "MSB");
        // Abschlag and NN-Rechnung carry both Sparten; the role follows.
        assert_eq!(marktrolle_for(31001, "STROM"), "NB");
        assert_eq!(marktrolle_for(31001, "GAS"), "GNB");
        assert_eq!(marktrolle_for(31002, "STROM"), "NB");
        assert_eq!(marktrolle_for(31002, "GAS"), "GNB");
        assert_eq!(marktrolle_for(31005, "STROM"), "NB");
        // GeLi Gas AWH exists only in the gas Sperrprozess.
        assert_eq!(marktrolle_for(31011, "GAS"), "GNB");
    }

    /// A correction that is really a different invoice is refused.
    ///
    /// `original_draft_id` links the two, but the corrected settlement is
    /// caller-supplied — and corrections are exempt from the double-billing
    /// guard, so an unrelated second invoice would be stored without complaint.
    #[test]
    fn a_correction_must_correct_the_same_invoice() {
        let original = Identity {
            settlement_type: "NneStrom",
            sparte: "STROM",
            sender_mp_id: "9900357000004",
            recipient_mp_id: "9900012345678",
        };
        let same = Identity {
            ..original_copy(&original)
        };
        assert!(corrects_the_same_invoice(&same, &original).is_ok());

        for (label, changed) in [
            (
                "settlement kind",
                Identity {
                    settlement_type: "MmmStrom",
                    ..original_copy(&original)
                },
            ),
            (
                "Sparte",
                Identity {
                    sparte: "GAS",
                    ..original_copy(&original)
                },
            ),
            (
                "issuer",
                Identity {
                    sender_mp_id: "9900999000001",
                    ..original_copy(&original)
                },
            ),
            (
                "billed party",
                Identity {
                    recipient_mp_id: "9900111000002",
                    ..original_copy(&original)
                },
            ),
        ] {
            let err = corrects_the_same_invoice(&changed, &original)
                .expect_err("a changed {label} is a different invoice");
            assert_eq!(
                err.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "changed {label}"
            );
        }
    }

    /// Copy an identity so a test can vary one field of it.
    fn original_copy<'a>(from: &Identity<'a>) -> Identity<'a> {
        Identity {
            settlement_type: from.settlement_type,
            sparte: from.sparte,
            sender_mp_id: from.sender_mp_id,
            recipient_mp_id: from.recipient_mp_id,
        }
    }

    /// The Sparte filter accepts what an operator would type, and refuses a typo.
    #[test]
    fn the_sparte_filter_normalises_and_refuses_a_typo() {
        assert_eq!(sparte_filter(Some("Strom")).expect("ok"), Some("STROM"));
        assert_eq!(sparte_filter(Some("gas")).expect("ok"), Some("GAS"));
        assert_eq!(sparte_filter(Some(" GAS ")).expect("ok"), Some("GAS"));
        assert_eq!(sparte_filter(None).expect("ok"), None);
        // A typo must not read as "no gas invoices exist".
        assert!(sparte_filter(Some("STORM")).is_err());
    }

    /// A page cursor round-trips, and a malformed one is refused.
    #[test]
    fn a_page_cursor_round_trips() {
        // A non-UTC instant, because that is the one that breaks: `+01:00`
        // renders a `+`, and a `+` in a query string decodes as a space.
        let local = time::macros::datetime!(2026-02-01 09:30:00 +1);
        let encoded_local = pg::Cursor::encode(local, Uuid::nil()).expect("encodes");
        assert!(
            encoded_local.contains('Z') && !encoded_local.contains('+'),
            "a cursor must survive a query string: {encoded_local}"
        );
        assert_eq!(
            cursor(Some(&encoded_local))
                .expect("parses")
                .expect("some")
                .created_at,
            local,
            "normalising to UTC must not move the instant"
        );

        let at = time::macros::datetime!(2026-02-01 08:30:00 UTC);
        let id = Uuid::from_u128(0x1234_5678_90ab_cdef_1234_5678_90ab_cdef);
        let encoded = pg::Cursor::encode(at, id).expect("encodes");
        let parsed = cursor(Some(&encoded)).expect("parses").expect("some");
        assert_eq!(parsed.created_at, at);
        assert_eq!(parsed.id, id);
        // A caller walking pages must be told, not silently restarted at page one.
        assert!(cursor(Some("not-a-cursor")).is_err());
        assert!(cursor(None).expect("none is fine").is_none());
    }

    /// The last page hands back no cursor — a full page does.
    #[test]
    fn only_a_full_page_offers_a_next_cursor() {
        assert_eq!(next_cursor(&[], 10), None);
    }

    /// NN-Rechnung Strom and Gas share PID 31002 but not the makod command —
    /// the Gas one is what a GNB-role deployment is permitted to send.
    #[test]
    fn the_makod_command_follows_the_pid_and_the_sparte() {
        assert_eq!(
            makod_command(31002, "STROM").expect("strom NNE"),
            "gpke.nne.rechnung.stellen"
        );
        assert_eq!(
            makod_command(31002, "GAS").expect("gas NNE"),
            "gpke.nne-gas.rechnung.stellen"
        );
        assert_eq!(
            makod_command(31005, "STROM").expect("MMM"),
            "gpke.mmm.rechnung.stellen"
        );
        assert_eq!(
            makod_command(31009, "STROM").expect("MSB"),
            "wim.msb-rechnung.stellen"
        );
        assert_eq!(
            makod_command(31011, "GAS").expect("AWH"),
            "geli.gas.awh-rechnung.stellen"
        );
        // An Abschlag prices no energy, so one command serves both Sparten.
        for sparte in ["STROM", "GAS"] {
            assert_eq!(
                makod_command(31001, sparte).expect("Abschlag"),
                "gpke.nne-abschlag.rechnung.stellen"
            );
        }
    }
}
