//! `POST /api/v1/selbstausstellen` — the self-issued Mehrmengen-Rechnung.
//!
//! # What PID 31006 actually is
//!
//! **MMM Mehrmenge, selbst ausgestellt** (INVOIC AHB; Anwendungsübersicht
//! Prüfidentifikatoren 4.0). The Mehr-/Mindermenge is treated as a *Lieferung*
//! of energy, and the leg that would otherwise arrive as an invoice is written
//! by the receiving party instead — the Gutschriftverfahren of § 14 Abs. 2 Satz
//! 2 UStG. It is **not** a Netznutzungsrechnung: that is PID 31002.
//!
//! The document is therefore built by [`grid_billing::settle_mmm`] with
//! `selbstausgestellt`, which is what makes the rendered BO4E state
//! `netznutzungrechnungsart = Selbstausgestellt` and
//! `netznutzungrechnungstyp = Mehrmindermengenrechnung`. An NNE settlement
//! stamped with PID 31006 renders as a Handelsrechnung instead: individually
//! well-formed fields, and a document the AHB rejects.
//!
//! # The inputs, and where they come from
//!
//! | Input | Source | Why not elsewhere |
//! |---|---|---|
//! | `gemessen_kwh` | `edmd GET /api/v1/imbalance/{malo}/{y}/{m}` | edmd measures |
//! | `bilanziert_kwh` | the caller | edmd does not balance; the allocated quantity is a commercial figure from the LF's own Bilanzkreis |
//! | Mehr-/Mindermengenpreise | `marktd GET /api/v1/mmm-preise/strom/{y}/{m}` | one nationwide monthly BDEW series (GPKE Teil 1 Kap. 8.4) — the NB is not part of the key |
//!
//! The month is the period: Mehr-/Mindermengen settle per Bilanzierungsmonat,
//! and the price series is published per application month, so a period that
//! straddles two months has no single price to settle against.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use rust_decimal::Decimal;
use serde::Deserialize;
use time::Date;

use crate::{handler::HandlerState, pg};

/// Request body for `POST /api/v1/selbstausstellen`.
#[derive(Debug, Deserialize)]
pub struct SelbstausstellenRequest {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// BDEW-Codenummer of the Netzbetreiber the Mehrmenge is settled with.
    pub nb_mp_id: String,
    /// Bilanzierungsmonat.
    pub year: i32,
    /// Bilanzierungsmonat, 1–12.
    pub month: u8,
    /// The **bilanzierte** quantity for the month, in kWh.
    ///
    /// Required, and the caller's to supply: it is what the Bilanzkreis was
    /// charged from the allocated profile, held by the supplier, and no amount
    /// of metering data yields it. `edmd` refuses the comparison without it for
    /// the same reason.
    pub bilanziert_kwh: Decimal,
}

/// Whether §3g Wiederverkäufer status is claimed for this supply.
///
/// A Mehr-/Mindermenge is a Lieferung, so § 13b Abs. 2 Nr. 5 Buchst. b UStG can
/// shift the tax. Electricity needs **both** parties to hold the status, so a
/// self-issued invoice cannot assert it from the issuer's side alone — the
/// endpoint therefore settles at the ordinary rate. Changing that needs the
/// counterparty's USt 1 TH on file, which belongs in master data, not a request
/// body.
const WIEDERVERKAEUFER: grid_billing::Wiederverkaeuferstatus =
    grid_billing::Wiederverkaeuferstatus::KEINER;

/// `POST /api/v1/selbstausstellen`
///
/// Build, record and dispatch a self-issued Mehrmengen-Rechnung (PID 31006).
///
/// # § 147 AO / GoBD
///
/// The receipt carries `makod`'s process id, so the REMADV that answers the
/// invoice, a later Storno and the payment confirmation all find the same row.
/// A locally minted UUID would correlate with nothing.
pub async fn post_selbstausstellen(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<SelbstausstellenRequest>,
) -> Response {
    if let Err(e) = cedar.check(
        &claims.principal(),
        "dispatch-selbstausstellen",
        &state.tenant,
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match build_and_dispatch(&state, &req).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// A refusal with the status the caller should see.
struct Refusal(StatusCode, String);

impl Refusal {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn unprocessable(msg: impl Into<String>) -> Refusal {
    Refusal(StatusCode::UNPROCESSABLE_ENTITY, msg.into())
}

fn bad_gateway(msg: impl Into<String>) -> Refusal {
    Refusal(StatusCode::BAD_GATEWAY, msg.into())
}

async fn build_and_dispatch(
    state: &HandlerState,
    req: &SelbstausstellenRequest,
) -> Result<Response, Refusal> {
    let (period_from, period_to) = month_bounds(req.year, req.month)
        .ok_or_else(|| Refusal(StatusCode::BAD_REQUEST, "month must be 1–12".to_owned()))?;

    // ── The measured quantity ────────────────────────────────────────────────
    let gemessen_kwh = fetch_gemessen(state, req).await?;

    // ── The prices ───────────────────────────────────────────────────────────
    //
    // One nationwide monthly BDEW series. Refusing when the month is missing is
    // the point: settling against a neighbouring month's price produces an
    // invoice that is wrong by an amount nobody will notice.
    let prices = state
        .marktd
        .get_mmm_strom(req.year, req.month)
        .await
        .map_err(|e| bad_gateway(format!("marktd MMM price lookup failed: {e}")))?
        .ok_or_else(|| {
            unprocessable(format!(
                "no Strom Mehr-/Mindermengenpreise imported for {}-{:02} — import them via \
                 PUT /api/v1/mmm-preise/strom/{}/{} on marktd before settling the month",
                req.year, req.month, req.year, req.month
            ))
        })?;

    // ── The settlement ───────────────────────────────────────────────────────
    let period = grid_billing::SettlementPeriod::new(period_from, period_to)
        .map_err(|e| unprocessable(format!("invalid settlement period: {e}")))?;
    let input = grid_billing::MmmInput {
        malo_id: req.malo_id.clone(),
        // The NB is the counterparty even though it does not write the
        // document: `nb_mp_id`/`lf_mp_id` name the roles in the settlement, and
        // who issued the invoice is `selbstausgestellt`.
        nb_mp_id: req.nb_mp_id.clone(),
        lf_mp_id: state.tenant.clone(),
        period,
        sparte: grid_billing::Sparte::Strom,
        actual_kwh: gemessen_kwh,
        profil_kwh: req.bilanziert_kwh,
        mehr_preis_ct_per_kwh: prices.mehr_ct_kwh,
        minder_preis_ct_per_kwh: prices.minder_ct_kwh,
        wiederverkaeufer: WIEDERVERKAEUFER,
        selbstausgestellt: true,
    };
    // `grid-billing` computes an `Error`-severity finding for an input it cannot
    // bill — and nothing read it, so the invoice went to the Netzbetreiber with
    // the defect the validator had already named. A self-issued document is the
    // worst place for that: the counterparty did not write it and has to dispute
    // it back.
    let validation = grid_billing::validate_mmm_input(&input);
    if !validation.is_valid {
        let findings = validation
            .warnings
            .iter()
            .filter(|w| w.severity == grid_billing::WarningSeverity::Error)
            .map(|w| format!("[{}] {}", w.code, w.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(unprocessable(format!(
            "MMM settlement for MaLo {} is not billable: {findings}",
            req.malo_id
        )));
    }
    let settlement = grid_billing::settle_mmm(&input)
        .map_err(|e| unprocessable(format!("settlement failed: {e}")))?;

    let invoice_date = mako_fristen::heute();
    let rechnungsnummer = rechnungsnummer(&state.tenant, &req.malo_id, req.year, req.month);
    let document = grid_billing::InvoiceDocument {
        settlement,
        pid: 31006,
        rechnungsnummer: rechnungsnummer.clone(),
        correction_of: None,
        invoice_date,
        // § 41 Abs. 1 Allgemeine Festlegungen: the Zahlungsziel is stated on the
        // document. 30 days is the market default and matches the check's own
        // `max_zahlungsziel_days`.
        due_date: invoice_date + time::Duration::days(30),
        // The Mehrmenge leg has its own `netznutzungrechnungstyp`
        // (Mehrmindermengenrechnung), so it states no separate billing cadence,
        // and a self-issued invoice settles no Abschläge.
        cadence: None,
        abschlaege: Vec::new(),
    };
    let total_eur = document.settlement.total_eur;
    let rechnung = grid_billing::bo4e::into_rechnung(&document);
    // The outbound gate. `grid-billing` checks its own emissions in tests, but
    // this document is assembled here from request data — the Mehrmenge leg,
    // the period, the amounts — so no fixture covers the arithmetic the gate
    // runs. It is *dispatched to a counterparty*, who checks exactly these
    // rules (`invoic-checker` stage 3) and answers a document that does not
    // reconcile with a REMADV rejection. mako refuses a received document that
    // breaks a BO4E-stated rule; it must not issue one.
    mako_markt::bo4e::ensure_conformant(&rechnung)
        .map_err(|e| unprocessable(format!("the self-issued Rechnung is not valid BO4E: {e}")))?;
    let rechnung_json = serde_json::to_value(&rechnung)
        .map_err(|e| unprocessable(format!("Rechnung does not serialise: {e}")))?;

    // ── Dispatch, then record under makod's process id ───────────────────────
    //
    // The command is idempotent on (tenant, MaLo, month): a retry after a
    // timeout re-uses the key rather than issuing a second invoice for the same
    // Bilanzierungsmonat.
    let idempotency_key = format!("invoicd-31006-{}-{}", req.malo_id, rechnungsnummer);
    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: None,
        command: "gpke.abrechnung.selbstausstellen".to_owned(),
        malo_id: Some(req.malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "pid":             31006,
            "nb_mp_id":        req.nb_mp_id,
            "period_from":     period_from.to_string(),
            "period_to":       period_to.to_string(),
            "rechnungsnummer": rechnungsnummer,
            "total_eur":       total_eur.to_string(),
            "rechnung":        rechnung,
        }),
    };
    let accepted = state
        .makod
        .post_command(&idempotency_key, &cmd)
        .await
        .map_err(|e| bad_gateway(format!("makod dispatch failed: {e}")))?;

    let now = time::OffsetDateTime::now_utc();
    let row = pg::ReceiptRow {
        // makod's id, so the REMADV that answers this invoice, a later Storno
        // and the payment confirmation all find the same row.
        process_id: accepted.process_id,
        invoice_ref: None,
        rechnungsnummer: Some(rechnungsnummer.clone()),
        pid: 31006,
        direction: pg::receipts::DIRECTION_OUTBOUND.to_owned(),
        sender_mp_id: state.tenant.clone(),
        receiver_gln: req.nb_mp_id.clone(),
        malo_id: Some(req.malo_id.clone()),
        rechnung: rechnung_json,
        bo4e_version: pg::bo4e_version(&rechnung).to_owned(),
        outcome: "Dispatched".to_owned(),
        findings: serde_json::json!([]),
        pay_by: Some(
            document
                .due_date
                .with_time(time::Time::MIDNIGHT)
                .assume_utc(),
        ),
        received_at: now,
        checked_at: now,
        dispatched_at: Some(now),
        tenant: state.tenant.clone(),
    };
    if let Err(e) = pg::upsert_receipt(&state.pool, &row).await {
        // The invoice is out; the audit row is not. That is a § 147 AO gap an
        // operator has to close by hand, so it is reported rather than logged.
        tracing::error!(
            %e, process_id = %accepted.process_id, %rechnungsnummer,
            "invoicd: selbstausstellen dispatched but the receipt was not persisted — \
             § 147 AO gap requiring manual reconciliation"
        );
        return Err(Refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "invoice {rechnungsnummer} was dispatched as process {} but its receipt \
                 could not be persisted: {e}",
                accepted.process_id
            ),
        ));
    }

    crate::handler::emit_receipt_event(
        state,
        &crate::handler::PaymentEventCtx {
            process_id: accepted.process_id,
            pid: 31006,
            direction: pg::receipts::DIRECTION_OUTBOUND,
            sender_mp_id: &state.tenant,
            outcome: "Dispatched",
            pay_by: Some(document.due_date),
            findings_count: 0,
            dispatched: true,
        },
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "process_id":      accepted.process_id,
            "pid":             31006,
            "malo_id":         req.malo_id,
            "nb_mp_id":        req.nb_mp_id,
            "rechnungsnummer": rechnungsnummer,
            "period_from":     period_from.to_string(),
            "period_to":       period_to.to_string(),
            "gemessen_kwh":    gemessen_kwh.to_string(),
            "bilanziert_kwh":  req.bilanziert_kwh.to_string(),
            "total_eur":       total_eur.to_string(),
            "outcome":         "Dispatched",
        })),
    )
        .into_response())
}

/// Read the measured quantity for the month from `edmd`.
async fn fetch_gemessen(
    state: &HandlerState,
    req: &SelbstausstellenRequest,
) -> Result<Decimal, Refusal> {
    let edmd = state.edmd.as_ref().ok_or_else(|| {
        Refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "edmd not configured — add [edmd] url to invoicd.toml to settle Mehrmengen".to_owned(),
        )
    })?;

    /// Only the field this endpoint consumes.
    #[derive(Deserialize)]
    struct Imbalance {
        gemessen_kwh: Decimal,
    }

    let path = format!(
        "/api/v1/imbalance/{}/{}/{}",
        req.malo_id, req.year, req.month
    );
    let request = edmd.get(&path).query(&[
        ("sparte", "strom"),
        ("bilanziert_kwh", &req.bilanziert_kwh.to_string()),
    ]);
    edmd.json::<Imbalance>(request)
        .await
        .map_err(|e| bad_gateway(e.to_string()))?
        .map(|i| i.gemessen_kwh)
        .ok_or_else(|| {
            unprocessable(format!(
                "edmd has no metering data for MaLo {} in {}-{:02}",
                req.malo_id, req.year, req.month
            ))
        })
}

/// Inclusive `[first, last]` day of a calendar month.
fn month_bounds(year: i32, month: u8) -> Option<(Date, Date)> {
    let month = time::Month::try_from(month).ok()?;
    let first = Date::from_calendar_date(year, month, 1).ok()?;
    let last = first.replace_day(first.month().length(first.year())).ok()?;
    Some((first, last))
}

/// § 14 Abs. 4 Nr. 4 UStG: einmalig, and traceable to what it settles.
///
/// The MaLo and the Bilanzierungsmonat are exactly the key of the settlement,
/// so the number is stable under a retry — a timed-out dispatch re-issues the
/// same invoice rather than a second one for the same month.
fn rechnungsnummer(tenant: &str, malo_id: &str, year: i32, month: u8) -> String {
    format!("MMM-SELBST-{tenant}-{malo_id}-{year}{month:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// The period is the Bilanzierungsmonat, inclusive at both ends — a
    /// settlement short by the last day of the month under-bills it.
    #[test]
    fn a_month_spans_its_first_and_last_day() {
        assert_eq!(
            month_bounds(2026, 6),
            Some((date!(2026 - 06 - 01), date!(2026 - 06 - 30)))
        );
        assert_eq!(
            month_bounds(2026, 2),
            Some((date!(2026 - 02 - 01), date!(2026 - 02 - 28)))
        );
        assert_eq!(
            month_bounds(2024, 2),
            Some((date!(2024 - 02 - 01), date!(2024 - 02 - 29))),
            "a leap February has 29 days"
        );
        assert_eq!(
            month_bounds(2026, 12),
            Some((date!(2026 - 12 - 01), date!(2026 - 12 - 31)))
        );
    }

    /// A month outside 1–12 is a bad request, not a panic.
    #[test]
    fn an_impossible_month_is_refused() {
        assert!(month_bounds(2026, 0).is_none());
        assert!(month_bounds(2026, 13).is_none());
    }

    /// The invoice number is stable for one (MaLo, month), so a retried
    /// dispatch re-issues the same invoice instead of a second one — and
    /// distinct across MaLos and months, because § 14 Abs. 4 Nr. 4 UStG
    /// requires it to be einmalig.
    #[test]
    fn the_invoice_number_identifies_exactly_what_it_settles() {
        let a = rechnungsnummer("9900357000004", "51238696012", 2026, 6);
        assert_eq!(a, rechnungsnummer("9900357000004", "51238696012", 2026, 6));
        assert_eq!(a, "MMM-SELBST-9900357000004-51238696012-202606");
        assert_ne!(a, rechnungsnummer("9900357000004", "51238696012", 2026, 7));
        assert_ne!(a, rechnungsnummer("9900357000004", "51238696013", 2026, 6));
        assert_ne!(a, rechnungsnummer("9900012345678", "51238696012", 2026, 6));
    }

    /// A single-digit month is zero-padded, or June and December of a year
    /// collide with 2026-1-2 style numbers across the fleet.
    #[test]
    fn the_month_is_zero_padded() {
        assert!(rechnungsnummer("t", "m", 2026, 1).ends_with("202601"));
        assert!(rechnungsnummer("t", "m", 2026, 12).ends_with("202612"));
    }
}
