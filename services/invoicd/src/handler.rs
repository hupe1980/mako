//! Inbound `MarktEvent` webhook — one pipeline for every INVOIC PID.
//!
//! # The pipeline
//!
//! ```text
//! verify HMAC → parse event → route_for(pid) → deserialize Rechnung
//!    → run the route's check → decide accept/dispute
//!    → PERSIST the receipt  ← § 147 AO: before anything is sent
//!    → dispatch the answer command to makod
//!    → mark dispatched → notify the ERP
//! ```
//!
//! Every PID takes this path. What varies — the check, the price sheet, the
//! command names — is data in [`crate::routing`], not a copy of the pipeline.
//!
//! # Persist first
//!
//! A received INVOIC is a Buchungsbeleg (§ 147 Abs. 3 AO, § 14b UStG, 8-year
//! retention). The receipt is written before the answer is sent, and a write
//! failure **aborts the dispatch**: the event is dead-lettered and redelivered
//! rather than answered off the record. The REMADV deadline is days; the audit
//! obligation is eight years.
//!
//! # Nothing is dropped silently
//!
//! An event that cannot become a receipt — no message reference, an unparseable
//! Rechnung, a `makod` that cannot supply one — goes to `invoic_dlq` with the
//! reason, and `invoicd_dlq_open_total` counts it.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use invoic_checker::{CheckConfig, CheckOutcome, CheckReport, InvoicCheckEngine};
use mako_markt::makod_client::{ForwardCommand, MakodClient};
use rubo4e::current::Rechnung;
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::pg;
use crate::routing::{CheckKind, PidRoute, route_for};

/// Shared application state for the webhook handler.
#[derive(Clone)]
pub struct HandlerState {
    pub marktd: mako_markt::marktd_client::MarktdClient,
    pub makod: MakodClient,
    pub check_config: Arc<CheckConfig>,
    pub inbound_secret: Arc<Option<SecretString>>,
    /// `Warn` escalates to `Dispute` when the invoice net total exceeds this,
    /// in `Amount<5>` raw units (10⁻⁵ EUR). `0` never escalates.
    pub auto_dispute_threshold_raw: i64,
    /// The receipt store. § 147 AO makes it mandatory, so it is not optional.
    pub pool: sqlx::PgPool,
    /// Operator tenant written to every row.
    pub tenant: String,
    /// ERP webhook for `de.invoic.receipt.*` CloudEvents.
    pub erp_webhook_url: Option<String>,
    /// Standard Webhooks signing secret for outbound ERP deliveries.
    pub erp_hmac_secret: Option<SecretString>,
    /// `edmd` — required by `POST /api/v1/selbstausstellen`, which reads the
    /// measured quantity for the Bilanzierungsmonat from it.
    pub edmd: Option<mako_service::http::Upstream>,
    pub http_client: reqwest::Client,
}

/// `POST /webhook` — receive a `MarktEvent` CloudEvent from `marktd`.
///
/// Always answers `204` once the signature verifies: the event is `marktd`'s to
/// retry only when delivery failed, and a business-level problem is recorded in
/// `invoic_dlq` rather than bounced back as an HTTP error that would be retried
/// forever with the same result.
pub async fn handle_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // The shared verifier also refuses a stale `webhook-timestamp`, so a
    // captured POST cannot be replayed into the receipt store.
    let secret = (*state.inbound_secret)
        .as_ref()
        .map(|s| s.expose_secret().as_bytes().to_vec());
    if let Err(err) = mako_service::webhook::verify_request(secret.as_deref(), &headers, &body) {
        warn!(%err, "invoicd: inbound webhook refused");
        return StatusCode::from(err).into_response();
    }

    // `MarktEvent` implements only `Serialize`, so the envelope is read as
    // generic JSON rather than coupling to an internal `Deserialize`.
    let Ok(event) = serde_json::from_slice::<serde_json::Value>(&body) else {
        warn!("invoicd: inbound webhook body is not JSON");
        return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
    };

    let ce_type = event["type"].as_str().unwrap_or_default();
    let data = &event["data"];
    let pid = data["pid"].as_u64().unwrap_or(0) as u32;

    if ce_type != mako_events::mako::PROCESS_INITIATED {
        debug!(
            ce_type,
            pid, "invoicd: event ignored (not process.initiated)"
        );
        return StatusCode::NO_CONTENT.into_response();
    }
    let Some(route) = route_for(pid) else {
        debug!(pid, "invoicd: PID not answered by this service");
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(process_id) = event["subject"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
    else {
        warn!(
            pid,
            subject = event["subject"].as_str(),
            "invoicd: process.initiated has no parseable UUID subject — cannot correlate"
        );
        return StatusCode::NO_CONTENT.into_response();
    };

    process_invoic(&state, route, process_id, data).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Everything one inbound INVOIC needs, once the envelope has been read.
struct Incoming {
    invoice_ref: String,
    sender_mp_id: String,
    receiver_gln: String,
    malo_id: Option<String>,
    rechnung: Rechnung,
    rechnung_json: serde_json::Value,
}

/// Check one INVOIC and answer it.
async fn process_invoic(
    state: &HandlerState,
    route: &'static PidRoute,
    process_id: Uuid,
    data: &serde_json::Value,
) {
    let pid = route.pid;
    let incoming = match extract(state, route, process_id, data).await {
        Ok(i) => i,
        Err(reason) => {
            warn!(pid, %process_id, %reason, "invoicd: INVOIC dead-lettered");
            dead_letter(state, process_id, pid, data, &reason).await;
            return;
        }
    };

    let received_at = OffsetDateTime::now_utc();
    let report = run_check(state, route, &incoming).await;
    let checked_at = OffsetDateTime::now_utc();

    let verdict = Verdict::of(
        &report,
        state.auto_dispute_threshold_raw,
        &incoming.rechnung,
    );
    info!(
        %process_id, pid,
        outcome = verdict.label,
        findings = report.findings.len(),
        lines = report.line_items_checked,
        "invoicd: INVOIC check complete"
    );

    // ── § 147 AO / GoBD: persist before anything is sent ────────────────────
    //
    // A write failure aborts the dispatch. Answering an invoice that is not in
    // the audit trail trades an eight-year obligation for a deadline measured
    // in days; the event is dead-lettered and `marktd` redelivers it.
    let row = pg::ReceiptRow {
        process_id,
        invoice_ref: Some(incoming.invoice_ref.clone()),
        rechnungsnummer: incoming.rechnung.rechnungsnummer.clone(),
        pid: pid as i16,
        direction: pg::receipts::DIRECTION_INBOUND.to_owned(),
        sender_mp_id: incoming.sender_mp_id.clone(),
        receiver_gln: incoming.receiver_gln.clone(),
        malo_id: incoming.malo_id.clone(),
        rechnung: incoming.rechnung_json.clone(),
        bo4e_version: pg::bo4e_version(&incoming.rechnung).to_owned(),
        outcome: verdict.label.to_owned(),
        findings: serde_json::to_value(&report.findings).unwrap_or_else(|_| serde_json::json!([])),
        // Already the `date-time` the BO4E schema declares, so the TIMESTAMPTZ
        // column takes it as it stands.
        pay_by: incoming.rechnung.faelligkeitsdatum,
        received_at,
        checked_at,
        dispatched_at: None,
        tenant: state.tenant.clone(),
    };
    if let Err(err) = pg::upsert_receipt(&state.pool, &row).await {
        warn!(
            %err, %process_id, pid,
            "invoicd: receipt persist failed — refusing to answer an invoice that is not in \
             the § 147 AO audit trail; dead-lettering for redelivery"
        );
        dead_letter(
            state,
            process_id,
            pid,
            data,
            &format!("receipt persist failed: {err}"),
        )
        .await;
        return;
    }

    // ── Answer the market partner ────────────────────────────────────────────
    let (command, payload) = if verdict.dispute {
        let reason = dispute_reason(&report.findings);
        let antwort_code = dispute_antwortcode(&report.findings);
        warn!(%process_id, pid, %reason, antwort_code, "invoicd: disputing invoice");
        (
            route.reject,
            serde_json::json!({
                "invoice_ref": incoming.invoice_ref,
                "ablehnungsgrund": reason,
                // `SG7 AJT` — DE 4465 the code, DE 1082 the EBD it comes from.
                "antwort_code": antwort_code,
                "antwort_ebd": mako_pruefung::codes::EBD_NETZNUTZUNGSRECHNUNG,
            }),
        )
    } else {
        (
            route.accept,
            serde_json::json!({ "invoice_ref": incoming.invoice_ref }),
        )
    };
    let dispatched = dispatch(state, process_id, route, command, payload).await;

    // Accepted or disputed: the ERP hears about every checked invoice. A
    // dispute is the outcome an accounts-payable team most needs.
    emit_receipt_event(
        state,
        &PaymentEventCtx {
            process_id,
            pid,
            direction: pg::receipts::DIRECTION_INBOUND,
            sender_mp_id: &incoming.sender_mp_id,
            outcome: verdict.label,
            // The event carries the Zahlungsziel as a calendar date: BDEW
            // INVOIC transmits it as DTM+92 qualifier 102, a bare YYYYMMDD, and
            // a consumer comparing it against a Frist wants the date, not an
            // offset it has to normalise first.
            pay_by: incoming.rechnung.faelligkeitsdatum_date(),
            findings_count: report.findings.len(),
            dispatched,
        },
    )
    .await;
}

/// Read the event payload into [`Incoming`], or say why it cannot be processed.
async fn extract(
    state: &HandlerState,
    route: &PidRoute,
    process_id: Uuid,
    data: &serde_json::Value,
) -> Result<Incoming, String> {
    // The EDIFACT message reference is the business key `makod` routes the
    // answer command by. Without it the invoice can be checked but never
    // answered, which is a dead letter rather than a silent return.
    let invoice_ref = data["invoice_ref"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("invoice_ref missing from process.initiated payload — the answer cannot be routed")?
        .to_owned();

    // The workflow embeds the Rechnung in the outbox payload; `makod` is the
    // fallback for a process whose payload carries none.
    let rechnung_json = if data["rechnung"].is_object() {
        data["rechnung"].clone()
    } else {
        info!(%process_id, pid = route.pid, "invoicd: rechnung not in payload — fetching from makod");
        match state.makod.get_invoic_rechnung(process_id).await {
            Ok(Some(v)) => v,
            Ok(None) => return Err("makod has no Rechnung for this process".to_owned()),
            Err(e) => return Err(format!("makod Rechnung fetch failed: {e}")),
        }
    };

    let rechnung: Rechnung = serde_json::from_value(rechnung_json.clone())
        .map_err(|e| format!("Rechnung does not deserialize as BO4E: {e}"))?;

    let malo_id = rechnung
        .marktlokation
        .as_ref()
        .and_then(|ml| ml.marktlokations_id.as_ref())
        .map(ToString::to_string)
        .or_else(|| data["malo_id"].as_str().map(str::to_owned));

    Ok(Incoming {
        invoice_ref,
        sender_mp_id: data["sender_mp_id"].as_str().unwrap_or_default().to_owned(),
        receiver_gln: data["receiver_gln"]
            .as_str()
            .unwrap_or(&state.tenant)
            .to_owned(),
        malo_id,
        rechnung,
        rechnung_json,
    })
}

/// Run the route's plausibility check.
async fn run_check(state: &HandlerState, route: &PidRoute, inc: &Incoming) -> CheckReport {
    let pid = route.pid;
    let rechnung = &inc.rechnung;

    // A Rechnung flagged `ist_storno` carries the original's amounts negated,
    // whatever its PID. Comparing those against a tariff disputes every line,
    // so the arithmetic-only check applies to the flag as well as to PID 31004.
    if route.check == CheckKind::ArithmetikNur || invoic_checker::is_stornierung(rechnung) {
        return InvoicCheckEngine::check_storno(pid, rechnung, &state.check_config);
    }

    // The period the invoice settles decides which price sheet version applies.
    // Falling back to today rather than a fixed date keeps a Rechnung with no
    // dates at all comparing against the sheet in force now, instead of one
    // from a hard-coded year that quietly stops existing.
    let billing_date = rechnung
        .billing_period()
        .map(|p| *p.start())
        .or_else(|| rechnung.rechnungsdatum_date())
        .unwrap_or_else(|| OffsetDateTime::now_utc().date());

    if route.check == CheckKind::Messung {
        let sheet = state
            .marktd
            .get_preisblatt_messung(&inc.sender_mp_id, billing_date)
            .await
            .ok()
            .flatten();
        // Discount lines are validated against the AufAbschlag entries carried
        // on the sheet (PRICAT 27001–27003), so the MSB cannot add undocumented
        // ones. The list is an extension field on `PreisblattMessung`.
        let contracted: Vec<String> = sheet
            .as_ref()
            .and_then(|pm| {
                use rubo4e::json::Bo4eExtensionData as _;
                pm.extension_data()
                    .get("auf_abschlaege")?
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e["name"].as_str().map(str::to_owned))
                            .collect()
                    })
            })
            .unwrap_or_default();
        return InvoicCheckEngine::check_msb_rechnung_with_aufabschlaege(
            &inc.sender_mp_id,
            rechnung,
            sheet.as_ref(),
            &contracted,
            &state.check_config,
        );
    }

    let mut store = invoic_checker::tariff::InMemoryPreisblattStore::new();
    if let Some(sheet) = state
        .marktd
        .get_preisblatt(&inc.sender_mp_id, billing_date)
        .await
        .ok()
        .flatten()
    {
        store.insert(inc.sender_mp_id.clone(), sheet);
    }
    let mut report = InvoicCheckEngine::check(
        pid,
        &inc.sender_mp_id,
        rechnung,
        &store,
        &state.check_config,
    );

    // ── Stage 6: Mehr-/Mindermengen settlement prices ────────────────────────
    let (year, month) = (billing_date.year(), billing_date.month() as u8);
    let prices = match route.check {
        CheckKind::NetznutzungMitMmmStrom => state
            .marktd
            .get_mmm_strom(year, month)
            .await
            .ok()
            .flatten()
            .map(|r| (r.mehr_ct_kwh, r.minder_ct_kwh)),
        CheckKind::NetznutzungMitMmmGas => state
            .marktd
            .get_mmma_gas(year, month, GAS_MGV)
            .await
            .ok()
            .flatten()
            .map(|r| (r.mehr_ct_kwh, r.minder_ct_kwh)),
        _ => None,
    };
    let Some((mehr, minder)) = prices else {
        if matches!(
            route.check,
            CheckKind::NetznutzungMitMmmStrom | CheckKind::NetznutzungMitMmmGas
        ) {
            debug!(
                pid,
                year, month, "invoicd: MMM reference prices not in marktd — stage 6 skipped"
            );
        }
        return report;
    };

    let findings =
        InvoicCheckEngine::check_mmm_settlement(rechnung, mehr, minder, &state.check_config);
    if !findings.is_empty() {
        let escalation = findings
            .iter()
            .map(|f| {
                if f.is_dispute {
                    CheckOutcome::Dispute
                } else {
                    CheckOutcome::Warn
                }
            })
            .max()
            .unwrap_or(CheckOutcome::Ok);
        report.outcome = report.outcome.max(escalation);
        report.findings.extend(findings);
    }
    report
}

/// Trading Hub Europe — the single German Gas Marktgebietsverantwortlicher
/// since the NCG/GASPOOL merger on 01.10.2021.
const GAS_MGV: &str = "THE";

/// What to do with a checked invoice, and what to record.
struct Verdict {
    dispute: bool,
    label: &'static str,
}

impl Verdict {
    fn of(report: &CheckReport, threshold_raw: i64, rechnung: &Rechnung) -> Self {
        let dispute = match report.outcome {
            CheckOutcome::Ok => false,
            // A warning escalates only when the money at stake justifies a
            // human looking at it. `0` (the default) approves every warning.
            CheckOutcome::Warn => {
                threshold_raw > 0
                    && report
                        .total_net_invoic
                        .is_some_and(|t| t.to_raw() > threshold_raw)
            }
            CheckOutcome::Dispute => true,
        };
        let label = if dispute {
            "Dispute"
        } else if invoic_checker::is_stornierung(rechnung) {
            // A Stornorechnung is accepted on a reduced check (reference,
            // period, arithmetic), so it is recorded as accepted-with-remarks
            // rather than as a fully validated invoice.
            "AcceptedPartial"
        } else if report.outcome == CheckOutcome::Warn {
            "Warn"
        } else {
            "Ok"
        };
        Self { dispute, label }
    }
}

/// Send the answer command and mark the receipt dispatched. Returns whether it
/// went out.
///
/// A failure leaves `dispatched_at NULL`, which is what
/// `GET /api/v1/overdue-remadv` and the `invoicd_overdue_remadv_total` gauge
/// watch — the invoice is checked and recorded, and an operator can re-dispatch
/// it from the receipt.
async fn dispatch(
    state: &HandlerState,
    process_id: Uuid,
    route: &PidRoute,
    command: &str,
    payload: serde_json::Value,
) -> bool {
    let key = Uuid::new_v5(&process_id, route.salt).to_string();
    let cmd = ForwardCommand {
        marktrolle: None,
        command: command.to_owned(),
        malo_id: None,
        melo_id: None,
        payload,
    };
    match state.makod.post_command(&key, &cmd).await {
        Ok(_) => {
            if let Err(err) =
                pg::receipts::mark_dispatched(&state.pool, process_id, OffsetDateTime::now_utc())
                    .await
            {
                warn!(%err, %process_id, "invoicd: answer sent but receipt not marked dispatched");
            }
            true
        }
        Err(err) => {
            warn!(
                %err, %process_id, pid = route.pid, command,
                "invoicd: answer dispatch failed — receipt stays undispatched for re-dispatch"
            );
            false
        }
    }
}

/// Record an event that could not become a receipt.
///
/// Redelivery of the same event updates the row rather than adding one, so the
/// queue depth is the number of distinct stuck invoices — the number an alert
/// can be written against.
async fn dead_letter(
    state: &HandlerState,
    process_id: Uuid,
    pid: u32,
    data: &serde_json::Value,
    reason: &str,
) {
    let malo_id = data["malo_id"].as_str();
    let res = sqlx::query(
        r"INSERT INTO invoic_dlq (process_id, pid, malo_id, raw_event, failure_reason, tenant)
          VALUES ($1, $2, $3, $4, $5, $6)
          ON CONFLICT (tenant, process_id) WHERE process_id IS NOT NULL
          DO UPDATE SET failure_reason = EXCLUDED.failure_reason,
                        raw_event      = EXCLUDED.raw_event,
                        failed_at      = now(),
                        resolved_at    = NULL",
    )
    .bind(process_id)
    .bind(pid as i16)
    .bind(malo_id)
    .bind(data)
    .bind(reason)
    .bind(&state.tenant)
    .execute(&state.pool)
    .await;
    if let Err(e) = res {
        // Nothing is left that can record this invoice, so the log line is the
        // last trace of it — it carries the payload.
        warn!(
            %e, %process_id, pid, %reason, event = %data,
            "invoicd: dead-letter write failed — this INVOIC is recorded nowhere"
        );
    }
}

// ── ERP notification ──────────────────────────────────────────────────────────

/// Context for [`emit_receipt_event`].
pub struct PaymentEventCtx<'a> {
    pub process_id: Uuid,
    pub pid: u32,
    pub direction: &'a str,
    pub sender_mp_id: &'a str,
    pub outcome: &'a str,
    pub pay_by: Option<time::Date>,
    pub findings_count: usize,
    /// Whether the market answer went out. The ERP needs it: a settled invoice
    /// whose REMADV never left is not one it may pay against.
    pub dispatched: bool,
}

/// Notify the ERP about a checked invoice.
///
/// Delivery is **durable at-least-once**. This is the first attempt, made
/// inline; on any failure the row stays selectable by the outbox worker
/// (`erp_notified_at IS NULL`), which retries it with backoff until the attempt
/// cap. A `4xx` is dead-lettered immediately — the ERP rejected these exact
/// bytes, and burning the full 2.5 h backoff window will not change that.
///
/// The market answer is always dispatched before this runs: an ERP webhook
/// never delays a regulatory obligation.
pub async fn emit_receipt_event(state: &HandlerState, ctx: &PaymentEventCtx<'_>) {
    let Some(url) = &state.erp_webhook_url else {
        return;
    };

    let ce = mako_service::CloudEvent::new(
        mako_service::source("invoicd", &state.tenant),
        ce_type_for(ctx.outcome),
        ctx.process_id.to_string(),
        serde_json::json!({
            "process_id":     ctx.process_id.to_string(),
            "pid":            ctx.pid,
            "direction":      ctx.direction,
            "sender_mp_id":   ctx.sender_mp_id,
            "outcome":        ctx.outcome,
            "pay_by":         ctx.pay_by.map(|d| d.to_string()),
            "findings_count": ctx.findings_count,
            "dispatched":     ctx.dispatched,
        }),
    );

    let secret = state
        .erp_hmac_secret
        .as_ref()
        .map(|s| s.expose_secret().as_bytes());
    match mako_service::post_ce_with_retry(&state.http_client, url, &ce, secret).await {
        Ok(()) => {
            debug!(process_id = %ctx.process_id, "invoicd: ERP receipt event delivered");
            let _ = pg::receipts::mark_erp_notified(
                &state.pool,
                ctx.process_id,
                OffsetDateTime::now_utc(),
            )
            .await;
        }
        Err(e) if e.is_permanent() => {
            warn!(
                process_id = %ctx.process_id, erp_url = %url, error = %e,
                "invoicd: ERP webhook rejected the event — dead-lettering (check ERP webhook config)"
            );
            let _ = pg::receipts::dead_letter_erp(&state.pool, ctx.process_id).await;
        }
        Err(e) => {
            warn!(
                process_id = %ctx.process_id, erp_url = %url, error = %e,
                "invoicd: ERP webhook delivery failed — the outbox worker will retry"
            );
            let _ = pg::receipts::record_erp_failure(&state.pool, ctx.process_id, 0).await;
        }
    }
}

/// The CloudEvent type an outcome is announced under.
///
/// Shared with the outbox worker so a retried delivery carries the same type as
/// the inline attempt would have.
#[must_use]
pub fn ce_type_for(outcome: &str) -> &'static str {
    match outcome {
        "Dispute" => mako_events::invoic::RECEIPT_DISPUTED,
        "Dispatched" => mako_events::invoic::RECEIPT_DISPATCHED,
        _ => mako_events::invoic::RECEIPT_SETTLED,
    }
}

/// The `E_0406` Antwortcode a REMADV Abweisung carries in `AJT` DE 4465.
///
/// A rejection without a code gives the invoice sender nothing to correct, and
/// the MIG marks `AJT` DE 4465 Muss on the Abweichungsgrund segment.
///
/// Only one finding maps to a code with an exact counterpart in the tree:
/// [`FindingKind::TotalMismatch`] is Prüfschritt 900 („Entspricht der
/// Rechnungsbetrag der Summe aller Rechnungspositionen?"), which is `A70`.
/// Everything else lands on the catch-alls, which the BDEW requires to carry a
/// written Erläuterung — supplied here from the finding text.
///
/// The full tree — 205 Prüfschritte over Kopf-, Positions- und Summenebene,
/// answering with a *set* of (Positionsnummer, code) pairs — is not walked
/// here; see `mako_pruefung::codes::E_0406_CODES`.
fn dispute_antwortcode(findings: &[invoic_checker::Finding]) -> &'static str {
    use invoic_checker::FindingKind;
    let disputes = findings.iter().filter(|f| f.is_dispute);
    for f in disputes {
        if matches!(f.kind, FindingKind::TotalMismatch) {
            // Prüfschritt 900, Cluster: Ablehnung auf Summenebene.
            return "A70";
        }
    }
    // Positionsebene catch-all when a line was at fault, Summenebene otherwise.
    if findings
        .iter()
        .any(|f| f.is_dispute && f.line_number.is_some())
    {
        "A99"
    } else {
        "A96"
    }
}

/// A human-readable dispute reason from the findings.
///
/// Falls back to the monetary escalation when no individual finding disputed —
/// that is the only other way `Dispute` is reached, and "no reason given" on a
/// REMADV 33002 is not an answer the counterparty can act on.
fn dispute_reason(findings: &[invoic_checker::Finding]) -> String {
    let specific: Vec<&str> = findings
        .iter()
        .filter(|f| f.is_dispute)
        .map(|f| f.message.as_str())
        .collect();
    if specific.is_empty() {
        "Automatische Ablehnung: Rechnungsbetrag überschreitet Freigabegrenze".to_owned()
    } else {
        specific.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use invoic_checker::{EuroAmount, Finding, FindingKind, amount::RoundingStrategy};

    use super::*;

    fn report(outcome: CheckOutcome, total_eur: &str) -> CheckReport {
        CheckReport {
            outcome,
            findings: Vec::new(),
            pid: 31002,
            total_net_invoic: EuroAmount::from_decimal_rounded(
                total_eur.parse().expect("decimal"),
                RoundingStrategy::MidpointAwayFromZero,
            )
            .ok(),
            total_net_computed: None,
            line_items_checked: 0,
        }
    }

    fn plain() -> Rechnung {
        Rechnung::default()
    }

    fn storno() -> Rechnung {
        Rechnung {
            ist_storno: Some(true),
            ..Rechnung::default()
        }
    }

    /// `Ok` is accepted and `Dispute` is not, whatever the threshold.
    #[test]
    fn the_unambiguous_outcomes_ignore_the_threshold() {
        for threshold in [0i64, 100_000] {
            assert!(!Verdict::of(&report(CheckOutcome::Ok, "9999"), threshold, &plain()).dispute);
            assert!(Verdict::of(&report(CheckOutcome::Dispute, "1"), threshold, &plain()).dispute);
        }
    }

    /// A warning escalates strictly above the threshold. Exactly at it is not
    /// above it — the boundary decides whether a human looks at the invoice.
    #[test]
    fn a_warning_escalates_only_above_the_threshold() {
        let threshold = 250 * 100_000; // 250,00 EUR in 10⁻⁵ EUR units
        assert!(!Verdict::of(&report(CheckOutcome::Warn, "250.00"), threshold, &plain()).dispute);
        assert!(Verdict::of(&report(CheckOutcome::Warn, "250.01"), threshold, &plain()).dispute);
        // The default disables escalation entirely.
        assert!(!Verdict::of(&report(CheckOutcome::Warn, "999999"), 0, &plain()).dispute);
    }

    /// An invoice with no stated total cannot be compared to a money threshold,
    /// so it is not escalated on one.
    #[test]
    fn a_warning_without_a_total_is_not_escalated() {
        let mut r = report(CheckOutcome::Warn, "1");
        r.total_net_invoic = None;
        assert!(!Verdict::of(&r, 1, &plain()).dispute);
    }

    /// A Storno accepted on the reduced check is recorded as such — reading it
    /// back as a fully validated `Ok` would overstate what was checked.
    #[test]
    fn an_accepted_storno_is_labelled_partial() {
        assert_eq!(
            Verdict::of(&report(CheckOutcome::Ok, "0"), 0, &storno()).label,
            "AcceptedPartial"
        );
        // A disputed Storno is still a dispute.
        assert_eq!(
            Verdict::of(&report(CheckOutcome::Dispute, "0"), 0, &storno()).label,
            "Dispute"
        );
    }

    /// Every label the verdict produces must satisfy the `outcome` CHECK, or
    /// the receipt insert is rejected at runtime by a schema the compiler never
    /// sees. `direction` failed exactly this way with a capitalised literal.
    #[test]
    fn every_verdict_label_is_in_the_outcome_check() {
        const ALLOWED: &[&str] = &[
            "Ok",
            "AcceptedPartial",
            "Warn",
            "Dispute",
            "Resolved",
            "Dispatched",
            "Paid",
        ];
        let schema = include_str!("../migrations/0001_schema.sql");
        for outcome in [CheckOutcome::Ok, CheckOutcome::Warn, CheckOutcome::Dispute] {
            for rechnung in [plain(), storno()] {
                for threshold in [0i64, 1] {
                    let label = Verdict::of(&report(outcome, "500"), threshold, &rechnung).label;
                    assert!(ALLOWED.contains(&label), "unknown label {label:?}");
                    assert!(
                        schema.contains(&format!("'{label}'")),
                        "the schema's outcome CHECK does not list {label:?}"
                    );
                }
            }
        }
    }

    /// The Antwortcode is the machine-readable half of the same obligation.
    ///
    /// `TotalMismatch` is `E_0406` Prüfschritt 900 exactly; the rest land on a
    /// catch-all, chosen by whether a line or the sum was at fault.
    #[test]
    fn a_dispute_states_a_machine_readable_code() {
        let total = Finding {
            kind: FindingKind::TotalMismatch,
            is_dispute: true,
            message: "Gesamtnetto weicht ab".into(),
            line_number: None,
            expected: None,
            actual: None,
            deviation_pct: None,
        };
        assert_eq!(dispute_antwortcode(&[total]), "A70");

        let line = Finding {
            kind: FindingKind::ArithmeticError,
            is_dispute: true,
            message: "Position 3 rechnet nicht".into(),
            line_number: Some(3),
            expected: None,
            actual: None,
            deviation_pct: None,
        };
        assert_eq!(
            dispute_antwortcode(&[line]),
            "A99",
            "a faulty line takes the Positionsebene catch-all"
        );

        assert_eq!(
            dispute_antwortcode(&[]),
            "A96",
            "a monetary escalation with no finding takes the Summenebene catch-all"
        );
    }

    /// Every code this handler can emit must be one `E_0406` publishes.
    #[test]
    fn every_emitted_code_is_published_by_its_ebd() {
        for code in ["A70", "A99", "A96"] {
            assert!(
                mako_pruefung::codes::lookup(mako_pruefung::codes::EBD_NETZNUTZUNGSRECHNUNG, code)
                    .is_some(),
                "{code} is not published by E_0406"
            );
        }
    }

    /// A dispute must always carry a reason: `REMADV 33002` with an empty
    /// `ablehnungsgrund` gives the counterparty nothing to correct.
    #[test]
    fn a_dispute_always_states_a_reason() {
        assert!(dispute_reason(&[]).contains("Automatische Ablehnung"));
        let findings = vec![
            Finding {
                kind: FindingKind::TariffDeviation,
                is_dispute: true,
                message: "Einzelpreis weicht ab".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
            Finding {
                kind: FindingKind::PeriodInvalid,
                is_dispute: false,
                message: "nur ein Hinweis".into(),
                line_number: None,
                expected: None,
                actual: None,
                deviation_pct: None,
            },
        ];
        let reason = dispute_reason(&findings);
        assert!(reason.contains("Einzelpreis weicht ab"));
        assert!(
            !reason.contains("nur ein Hinweis"),
            "a non-disputing finding is not a rejection ground"
        );
    }

    /// The outcome labels and the CloudEvent types must not drift apart: an
    /// unmapped label would announce a dispute as a settlement.
    #[test]
    fn each_outcome_announces_the_matching_event() {
        assert_eq!(
            ce_type_for("Dispute"),
            mako_events::invoic::RECEIPT_DISPUTED
        );
        assert_eq!(
            ce_type_for("Dispatched"),
            mako_events::invoic::RECEIPT_DISPATCHED
        );
        for settled in ["Ok", "Warn", "AcceptedPartial", "Resolved", "Paid"] {
            assert_eq!(
                ce_type_for(settled),
                mako_events::invoic::RECEIPT_SETTLED,
                "{settled}"
            );
        }
    }
}
