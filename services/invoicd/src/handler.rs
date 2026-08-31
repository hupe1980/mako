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
    /// The Marktrolle this deployment receives invoices in — `[identity]
    /// marktrolle`. Half of the PID-31009 Use-Case lookup; `IMD+7081` is the
    /// other half. See [`crate::config::EmpfaengerRolle`].
    pub marktrolle: crate::config::EmpfaengerRolle,
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
    /// `SG1 RFF+ACE` — the order this invoice answers, Muss on the WiM- and
    /// MSB-Rechnung (INVOIC AHB 1.0b segment 00020). BO4E has no field for it,
    /// so it rides the process payload; `E_0264` Prüfschritt 40 is what
    /// compares it against the order on record.
    bestellung_ref: Option<String>,
    /// `IMD+7081` — the Rechnungstyp. `KON` („Abrechnung von Konfigurationen
    /// (Universalbestellprozess)") is the **ESA** Use-Case of WiM Teil 2
    /// Kap. 4.5 stated on the wire; `MSB` is the Messstellenbetrieb billed
    /// toward NB or LF. One PID, three Use-Cases.
    rechnungstyp: Option<String>,
}

/// `IMD+7081` = `KON` „Abrechnung von Konfigurationen (Universalbestellprozess)"
/// — the ESA billing Use-Case, as the wire states it.
const RECHNUNGSTYP_ESA: &str = "KON";

/// `IMD+7081` = `TEC` „Abrechnung von Technik" — a Leistung of the MSB's
/// **Preisblatt B**, ordered through the AWH „Änderung der Technik an
/// Lokationen" round and billed on the same PID 31009 as the
/// Messstellenbetrieb.
///
/// This is the discriminator the AHB gives for the fourth and fifth Use-Case of
/// 31009 (INVOIC AHB 1.0b, segment `IMD 7081`). Without it a Preisblatt-B
/// invoice is checked against `E_0566`/`E_0210` — trees whose codes mean
/// something else — instead of `E_0273`/`E_0270`.
const RECHNUNGSTYP_TECHNIK: &str = "TEC";

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
    let Checked {
        report,
        markt_antwort,
        storno_antwort,
    } = run_check(state, route, &incoming, process_id).await;
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

    // ── Answer the market partner — or deliberately do not ──────────────────
    //
    // `E_0267` („Prüfen, **ob** Antwort auf Stornierung erforderlich") has a
    // third outcome the other trees do not: **no message at all**. A Storno of
    // an invoice this ESA had itself refused with a Nicht-Zahlungsavis, or had
    // not answered yet, needs no answer — „dann ist auf die Stornorechnung
    // keine Antwort zu senden" (Prüfschritt 80).
    //
    // The receipt is written and the ERP is told either way; only the market
    // message is withheld. Sending one anyway answers a message the MSB is not
    // waiting on, and now that REMADVs actually reach the wire that is a
    // message the counterparty has to reconcile away.
    if let Some(mako_pruefung::esa::StornoAntwort::KeineAntwort { grund }) = storno_antwort {
        info!(
            %process_id, pid, grund,
            "invoicd: Stornorechnung needs no answer (E_0267 Prüfschritt 80) —              receipt recorded, no REMADV sent"
        );
        emit_receipt_event(
            state,
            &PaymentEventCtx {
                process_id,
                pid,
                direction: pg::receipts::DIRECTION_INBOUND,
                sender_mp_id: &incoming.sender_mp_id,
                outcome: verdict.label,
                pay_by: incoming.rechnung.faelligkeitsdatum_date(),
                findings_count: report.findings.len(),
                dispatched: false,
            },
        )
        .await;
        return;
    }

    let (command, payload) = if verdict.dispute {
        let reason = dispute_reason(&report.findings);
        let mut answer = serde_json::json!({
            "invoice_ref": incoming.invoice_ref,
            "ablehnungsgrund": reason,
        });
        let empfaenger = rechnung_empfaenger(state.marktrolle, markt_antwort.as_ref());
        // `IMD+7081` = `TEC` marks a Leistung of the Preisblatt B; `MSB` (or
        // nothing) the Messstellenbetrieb itself.
        let gegenstand = rechnungsgegenstand(incoming.rechnungstyp.as_deref());
        match abweichungsgrund(
            pid,
            empfaenger,
            gegenstand,
            markt_antwort.as_ref(),
            storno_antwort.as_ref(),
            &report.findings,
        ) {
            Some(grund) => {
                warn!(
                    %process_id, pid,
                    tree = %grund.ebd, remadv_pid = grund.remadv_pid,
                    codes = ?grund.befunde.iter().map(|b| &b.code).collect::<Vec<_>>(),
                    %reason,
                    "invoicd: disputing invoice"
                );
                if let Some(obj) = answer.as_object_mut() {
                    // `SG7 AJT` — DE 4465 the code(s), DE 1082 the EBD they come
                    // from — plus the Prüfidentifikator the answer's own shape
                    // requires (33002 for one code, 33003/33004 for a set).
                    obj.insert("antwort_codeliste".to_owned(), serde_json::json!(grund.ebd));
                    obj.insert(
                        "antwort_code".to_owned(),
                        serde_json::json!(grund.befunde.first().map(|b| &b.code)),
                    );
                    obj.insert(
                        "antwort_befunde".to_owned(),
                        serde_json::to_value(&grund.befunde).unwrap_or_default(),
                    );
                    obj.insert("remadv_pid".to_owned(), serde_json::json!(grund.remadv_pid));
                }
            }
            None => {
                // No tree this service can resolve a code in. `SG7 AJT` is Muss
                // on every Nicht-Zahlungsavis, and a code the named tree does
                // not publish is a non-conformant answer rather than an
                // approximate one — `A70` is `E_0406` Prüfschritt 900 and means
                // nothing in `E_0210`. So the refusal goes out without one and
                // says so, for an operator to complete.
                warn!(
                    %process_id, pid, %reason,
                    "invoicd: disputing invoice without an Antwortcode — no walked \
                     Entscheidungsbaum for this PID and Empfänger; SG7 AJT must be \
                     supplied by an operator before the REMADV is conformant"
                );
            }
        }
        (route.reject, answer)
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
            // INVOIC transmits it in `SG8 DTM+265` as DE 2379 `303`
            // (`CCYYMMDDHHMMZZZ`), and a consumer comparing it against a Frist
            // wants the date, not an offset it has to normalise first.
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

    // The BO4E gate, on its received-document setting. Stages 1–3 refuse: a
    // document that will not type — a wrong `_typ`, an out-of-schema
    // `rechnungstyp` decoding to `Unknown` — has nothing for the checker to
    // adjudicate, and dead-lettering it is the honest outcome.
    //
    // The BO4E *rules* deliberately do not refuse here. A `gesamtbrutto` that
    // is not net plus tax is a **disputable** invoice, and the market's answer
    // to it is a REMADV naming the defect — which `invoic-checker` stage 3
    // already produces as a finding. Refusing to parse would replace that
    // answer with silence and a dead letter for an operator to find.
    let (rechnung, bo4e_failures) =
        mako_markt::bo4e::decode_received::<Rechnung>(rechnung_json.clone())
            .map_err(|e| format!("Rechnung is not a readable BO4E document: {e}"))?;
    for f in &bo4e_failures {
        warn!(
            %process_id, pid = route.pid, path = %f.path, reason = %f.message,
            "invoicd: inbound Rechnung breaks a BO4E-stated rule — the check stages \
             below turn it into a finding, and the finding into a dispute"
        );
    }

    let malo_id = rechnung
        .marktlokation
        .as_ref()
        .and_then(|ml| ml.marktlokations_id.as_ref())
        .map(ToString::to_string)
        .or_else(|| data["malo_id"].as_str().map(str::to_owned));

    Ok(Incoming {
        invoice_ref,
        bestellung_ref: data["bestellung_ref"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        rechnungstyp: data["rechnungstyp"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
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

/// What a checked invoice produced: the operator-facing report, and — when the
/// invoice belongs to a Use-Case whose tree this service walks — the **market**
/// answer, in published Antwortcodes.
///
/// The two are not the same statement. [`CheckReport`] is mako's own
/// vocabulary and drives the ERP event and the § 147 AO receipt; the
/// [`RechnungsAntwort`](mako_pruefung::esa::RechnungsAntwort) is what the
/// counterparty's own systems resolve, and it also says which REMADV
/// Prüfidentifikator the answer must ride.
struct Checked {
    report: CheckReport,
    markt_antwort: Option<mako_pruefung::esa::RechnungsAntwort>,
    /// `E_0267` on an inbound Stornorechnung — **including its third outcome**,
    /// which is that no answer is owed at all.
    storno_antwort: Option<mako_pruefung::esa::StornoAntwort>,
}

/// Run the route's plausibility check, and the market tree where there is one.
///
/// # The ESA branch, and what selects it
///
/// INVOIC 31009 is „MSB-Rechnung" toward the **NB, the LF or the ESA** (WiM
/// Teil 1 Kap. 6.2 / Teil 2 Kap. 4.5), and the three share neither a price
/// basis nor an Entscheidungsbaum. `PreisblattMessung` is what an MSB
/// *publishes* toward the NB and the LF; there is none for the Kapitel-4.6
/// Messprodukte, because § 35 MsbG leaves the Entgelt for a Zusatzleistung to
/// be agreed per request. An ESA running the sheet path therefore got
/// `TariffNotFound` and **no price check at all**.
///
/// What it has instead is the QUOTES 15003 it ordered against, and **an
/// accepted offer on record for (this MSB, us) is what selects the branch** —
/// not a configured role. It is precisely the statement „we are the ESA in this
/// relationship, and this is what we agreed to pay", and it is the same fact
/// that makes the answer window the 4. WT before the Zahlungsziel rather than
/// the LF's *zum* Zahlungsziel.
async fn run_check(
    state: &HandlerState,
    route: &PidRoute,
    inc: &Incoming,
    process_id: Uuid,
) -> Checked {
    // ── The ESA's Stornorechnung: `E_0267` ───────────────────────────────────
    //
    // A Storno is answered — or deliberately **not** answered — on how the
    // *original* was answered, which is a fact about this ESA's own books
    // rather than about the document. `esa_storno_fakten` reads it from the
    // receipts table; an original nothing can be found for leaves the tree at
    // Prüfschritt 10 and refuses, which is the honest answer.
    if invoic_checker::is_stornierung(&inc.rechnung)
        && let Some(fakten) = esa_storno_fakten(state, inc).await
    {
        return Checked {
            report: InvoicCheckEngine::check_storno(route.pid, &inc.rechnung, &state.check_config),
            markt_antwort: None,
            // A Storno belongs to whichever Use-Case issued the invoice it
            // cancels, so the family is the same lookup — and `E_0267` /
            // `E_0272` / `E_0275` are step-for-step identical, so an
            // unresolvable family falls back to the ESA one rather than
            // skipping the check.
            storno_antwort: Some(invoic_checker::antwort_auf_stornorechnung(
                familie_fuer(route.pid, state.marktrolle, inc.rechnungstyp.as_deref())
                    .unwrap_or(mako_pruefung::rechnung::ESA),
                &inc.rechnung,
                &fakten,
            )),
        };
    }
    if route.check == CheckKind::Messung && !invoic_checker::is_stornierung(&inc.rechnung) {
        let billing_date = billing_date_of(&inc.rechnung);
        let agreed = esa_preise(state, inc, billing_date).await;
        // **The wire states the Use-Case.** `IMD+7081` = `KON` („Abrechnung von
        // Konfigurationen (Universalbestellprozess)") is the ESA billing of WiM
        // Teil 2 Kap. 4.5 — the Kapitel-4.6 Messprodukte *are* the
        // Konfigurationen, and the Universalbestellprozess is the handshake
        // that ordered them. `MSB` is the Messstellenbetrieb billed toward the
        // NB or the LF.
        //
        // An accepted offer on record corroborates it, and is the fallback for
        // a sender that omitted the qualifier — but it must not be the *only*
        // signal: the 4-Werktage answer window is owed whether or not mako
        // filed the offer, and treating an unfiled subscription as the LF case
        // gives away four Werktage that do not exist.
        if inc.rechnungstyp.as_deref() == Some(RECHNUNGSTYP_ESA) || !agreed.is_empty() {
            // The ÜT — the day the Übertragungsdatei was received — is what
            // every WiM Frist is measured from, and `E_0264` Prüfschritte 20
            // and 70 compare against it.
            let mut fakten = invoic_checker::EmpfaengerFakten::neu(mako_fristen::heute());
            // Prüfschritt 50 — `A05`. Answerable from this service's own
            // receipt store for every family, so it is answered here rather
            // than left defaulted.
            fakten.rechnungsnummer_bereits_verwendet =
                rechnungsnummer_bereits_verwendet(state, inc, process_id).await;
            // **Prüfschritt 40 is answerable, not assumed.** WiM Teil 2
            // UC 4.5.1: „Eine Rechnung referenziert auf die zugrundeliegende
            // Bestellung", and INVOIC AHB 1.0b makes `SG1 RFF+ACE` Muss on the
            // 31009 carrying the ORDERS Dokumentennummer (`IMD++KON` → hint
            // `[501]`). So the invoice names its order and mako holds the
            // orders it placed — the two either match or the invoice bills
            // against something this ESA never ordered.
            //
            // An invoice that names **no** order at all is Prüfschritt 40 by
            // itself: the reference is Muss, and its absence is exactly the
            // „auf keiner Bestellung basiert" the code is for.
            fakten.bestellung_bekannt = match inc.bestellung_ref.as_deref() {
                Some(r) => state
                    .marktd
                    .esa_messprodukt_of_bestellung(r)
                    .await
                    .unwrap_or_else(|e| {
                        // A marktd outage must not refuse a correct invoice:
                        // the ESA would be stating to the market that the MSB
                        // billed against no order, on evidence it does not have.
                        warn!(error = %e, bestellung_ref = r,
                              "invoicd: order lookup failed — E_0264 Prüfschritt 40 passed");
                        Some(r.to_owned())
                    })
                    .is_some(),
                None => false,
            };
            return Checked {
                report: InvoicCheckEngine::check_esa_rechnung(
                    &inc.sender_mp_id,
                    &inc.rechnung,
                    &agreed,
                    &state.check_config,
                ),
                markt_antwort: Some(invoic_checker::antwort_auf_rechnung(
                    mako_pruefung::rechnung::ESA,
                    &inc.rechnung,
                    &agreed,
                    &fakten,
                    &state.check_config,
                    mako_pruefung::HolidayCalendar::BdewMaKo,
                )),
                storno_antwort: None,
            };
        }

        // ── Abrechnung der Leistungen des Preisblatts B ───────────────────────
        //
        // `IMD+7081` = `TEC` „Abrechnung von Technik": the same PID 31009, a
        // different Use-Case, and its own quartet of trees — `E_0270`/`E_0271`/
        // `E_0276`/`E_0272` toward an LF, `E_0273`/`E_0274`/`E_0277`/`E_0275`
        // toward an NB (AWH „Prozesse zur Änderung der Technik an Lokationen"
        // Kap. 9.3/9.4). The walk is the same one the ESA family runs; only the
        // alphabet and two Kopf-Prüfschritte differ.
        if let Some(familie) =
            familie_fuer(route.pid, state.marktrolle, inc.rechnungstyp.as_deref())
            && familie != mako_pruefung::rechnung::ESA
        {
            let fakten = preisblatt_b_fakten(state, inc, process_id).await;
            return Checked {
                report: run_report(state, route, inc).await,
                markt_antwort: Some(invoic_checker::antwort_auf_rechnung(
                    familie,
                    &inc.rechnung,
                    &agreed,
                    &fakten,
                    &state.check_config,
                    mako_pruefung::HolidayCalendar::BdewMaKo,
                )),
                storno_antwort: None,
            };
        }
    }
    Checked {
        report: run_report(state, route, inc).await,
        markt_antwort: None,
        storno_antwort: None,
    }
}

/// What the recipient's own records contribute to a **Preisblatt-B** invoice.
///
/// Two Prüfschritte the ESA trees do not publish:
///
/// - **80** — is the Preisblatt version the invoice bills against on file? The
///   sheet arrives as PRICAT 27002, where it is called „Preisblatt Technik".
/// - **90** — was this Abrechnungszeitraum already settled by an accepted, not
///   cancelled invoice? The code's Hinweis makes naming that Rechnungsnummer
///   part of the answer, so this is the number and not a flag.
///
/// Both are `None`: neither store exists here — 80 wants a Preisblatt-B version
/// register in `marktd`, 90 an `invoic_receipts` query keyed on
/// (Rechnungssteller, Abrechnungszeitraum). `None` never refuses, so the
/// remaining twenty-two Prüfschritte decide.
async fn preisblatt_b_fakten(
    state: &HandlerState,
    inc: &Incoming,
    process_id: Uuid,
) -> invoic_checker::EmpfaengerFakten {
    let mut fakten = invoic_checker::EmpfaengerFakten::neu(mako_fristen::heute());
    // Prüfschritt 40 — „Basiert die Rechnung auf einer Bestellung des
    // Rechnungsempfängers?" A Preisblatt-B Leistung is always billed against a
    // confirmed ORDERS 17011, and INVOIC AHB 1.0b makes `SG1 RFF+ACE` Muss, so
    // an invoice naming no order fails the step on the wire alone.
    fakten.bestellung_bekannt = inc.bestellung_ref.is_some();
    fakten.rechnungsnummer_bereits_verwendet =
        rechnungsnummer_bereits_verwendet(state, inc, process_id).await;
    fakten
}

/// **Prüfschritt 50** — `A05`, „Rechnungsnummer wurde bereits verwendet".
///
/// Answered from this service's own receipt store, which is the only place that
/// knows. A lookup failure answers `false`: a database blip is not evidence
/// that a number was reused, and `A05` on a correct invoice is a binding
/// refusal to the market.
async fn rechnungsnummer_bereits_verwendet(
    state: &HandlerState,
    inc: &Incoming,
    process_id: Uuid,
) -> bool {
    let Some(nummer) = inc.rechnung.rechnungsnummer.as_deref() else {
        return false;
    };
    match pg::receipts::rechnungsnummer_bereits_verwendet(
        &state.pool,
        &state.tenant,
        &inc.sender_mp_id,
        nummer,
        process_id,
    )
    .await
    {
        Ok(v) => v,
        Err(err) => {
            warn!(%err, "invoicd: Rechnungsnummer-Dublettenprüfung fehlgeschlagen — Prüfschritt 50 wird nicht geprüft");
            false
        }
    }
}

/// The period an invoice settles decides which price sheet version applies.
///
/// Falling back to today rather than a fixed date keeps a Rechnung with no
/// dates at all comparing against the sheet in force now, instead of one from a
/// hard-coded year that quietly stops existing.
fn billing_date_of(rechnung: &Rechnung) -> time::Date {
    rechnung
        .billing_period()
        .map(|p| *p.start())
        .or_else(|| rechnung.rechnungsdatum_date())
        .unwrap_or_else(mako_fristen::heute)
}

/// How this ESA answered the invoice a Stornorechnung cancels — `E_0267`
/// Prüfschritte 70/80, and the only thing that decides whether an answer is
/// owed at all.
///
/// Returns `None` when the ESA branch does not apply: no accepted Angebot is on
/// record for this MSB, so this is somebody else's Storno and the ordinary
/// arithmetic path answers it. That is the same signal that selects the ESA
/// branch for a 31009 — an offer on record *is* the statement „we are the ESA
/// in this relationship".
async fn esa_storno_fakten(
    state: &HandlerState,
    inc: &Incoming,
) -> Option<invoic_checker::StornoEmpfaengerFakten> {
    let billing_date = billing_date_of(&inc.rechnung);
    if esa_preise(state, inc, billing_date).await.is_empty() {
        return None;
    }
    let original = inc.rechnung.original_rechnungsnummer.as_deref()?;
    // The receipt of the original carries the outcome we sent: `Ok` means a
    // Zahlungsavis went out, `Dispute` a Nicht-Zahlungsavis, and no row at all
    // means we never answered it.
    let ursprungsantwort = match pg::receipt_outcome(&state.pool, &state.tenant, original).await {
        Ok(Some(outcome)) if outcome == "Dispute" => {
            mako_pruefung::esa::UrsprungsAntwort::Abgelehnt
        }
        Ok(Some(_)) => mako_pruefung::esa::UrsprungsAntwort::Zugestimmt,
        Ok(None) => mako_pruefung::esa::UrsprungsAntwort::Unbeantwortet,
        Err(e) => {
            // Not a guess: without the original's outcome the tree cannot reach
            // 70 or 80, and answering either way is wrong half the time.
            warn!(error = %e, original, "invoicd: original receipt lookup failed — E_0267 skipped");
            return None;
        }
    };
    Some(invoic_checker::StornoEmpfaengerFakten {
        ursprungsrechnung_bekannt: !matches!(
            ursprungsantwort,
            mako_pruefung::esa::UrsprungsAntwort::Unbeantwortet
        ),
        ..invoic_checker::StornoEmpfaengerFakten::neu(ursprungsantwort)
    })
}

/// The accepted QUOTES 15003 of this (MSB, us) pair, as `(Artikel-ID, Preis)`.
async fn esa_preise(
    state: &HandlerState,
    inc: &Incoming,
    billing_date: time::Date,
) -> Vec<(String, invoic_checker::amount::EuroAmount)> {
    state
        .marktd
        .esa_preise(&inc.sender_mp_id, &state.tenant, billing_date)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            Some((
                p.artikel_id.clone(),
                invoic_checker::amount::euro_from_decimal(p.betrag)?,
            ))
        })
        .collect()
}

async fn run_report(state: &HandlerState, route: &PidRoute, inc: &Incoming) -> CheckReport {
    let pid = route.pid;
    let rechnung = &inc.rechnung;

    // A Rechnung flagged `ist_storno` carries the original's amounts negated,
    // whatever its PID. Comparing those against a tariff disputes every line,
    // so the arithmetic-only check applies to the flag as well as to PID 31004.
    if route.check == CheckKind::ArithmetikNur || invoic_checker::is_stornierung(rechnung) {
        return InvoicCheckEngine::check_storno(pid, rechnung, &state.check_config);
    }

    let billing_date = billing_date_of(rechnung);

    if route.check == CheckKind::Messung {
        // The ESA branch is decided in `run_check`, before this function runs:
        // it needs the accepted Angebot for the market tree as well as for the
        // price check, and fetching it twice would be the only way to keep the
        // decision here. Reaching this line means no offer is on record, so the
        // invoice is an MSB-Rechnung toward an NB or an LF and prices against
        // the published `PreisblattMessung`.
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

/// Which Marktrolle received this invoice, for the tree and the answer window.
///
/// A walked `E_0264` means [`run_check`] already identified the ESA Use-Case —
/// from `IMD+7081` = `KON` on the wire, or from an accepted Angebot on record.
/// Everything else falls to the LF/MSB arm, which is both the longest answer
/// window and the tree whose codes are not walked, so a misread refuses nothing
/// it should not.
const fn rechnung_empfaenger(
    eigene_rolle: crate::config::EmpfaengerRolle,
    markt_antwort: Option<&mako_pruefung::rechnung::RechnungsAntwort>,
) -> mako_fristen::vorlauf::RechnungEmpfaenger {
    use mako_fristen::vorlauf::RechnungEmpfaenger;
    // A walked ESA answer *is* the ESA Use-Case — it only exists where the wire
    // said `IMD++KON` or an accepted Angebot corroborated it, and that is a
    // stronger signal than configuration.
    if markt_antwort.is_some() {
        return RechnungEmpfaenger::Esa;
    }
    eigene_rolle.rechnung_empfaenger()
}

/// The [`RechnungsFamilie`](mako_pruefung::rechnung::RechnungsFamilie) whose
/// walk answers this invoice, or `None` where mako carries no Codeliste for it.
///
/// Both facts are required and neither is guessable: the recipient's Marktrolle
/// (`[identity] marktrolle`) narrows PID 31009 to at most two Use-Cases and
/// `IMD+7081` picks between them. `None` means the tree is *named* correctly by
/// [`mako_pruefung::codes::rechnungspruefung`] but its codes are not carried —
/// `E_0566` and `E_0210` — so the answer goes out without a code rather than
/// with one borrowed from another tree.
fn familie_fuer(
    pid: u32,
    eigene_rolle: crate::config::EmpfaengerRolle,
    rechnungstyp: Option<&str>,
) -> Option<mako_pruefung::rechnung::RechnungsFamilie> {
    mako_pruefung::rechnung::familie_fuer(
        pid,
        eigene_rolle.rechnung_empfaenger(),
        rechnungsgegenstand(rechnungstyp),
    )
}

/// What an inbound INVOIC 31009 bills — the second half of the tree lookup.
///
/// The recipient's Marktrolle narrows PID 31009 to at most two Use-Cases; this
/// picks between them. The wire carries no flag: INVOIC AHB 1.0b has only
/// `SG1 RFF+Z13` with the PID. What tells them apart is which Preisblatt the
/// positions' Artikel-IDs come from — `SG26 LIN` DE 7140, Bedingung `[520]` —
/// and a Leistung of the Preisblatt B is always billed against a **confirmed
/// Bestellung** out of the AWH „Änderung der Technik an Lokationen" round.
///
/// The wire does say it, in one element: `IMD+7081` distinguishes `MSB`
/// „Rechnung für Messstellenbetrieb" from **`TEC` „Abrechnung von Technik"**,
/// which is the Preisblatt-B invoice (INVOIC AHB 1.0b, segment `IMD 7081`;
/// `KON` is the ESA's, handled by the Empfänger arm).
///
/// Anything else — including an absent `IMD` — is read as the
/// Messstellenbetrieb, and that direction is deliberate. `E_0270`/`E_0273`
/// open with „Basiert die Rechnung auf einer Bestellung des
/// Rechnungsempfängers?" and refuse with `A04` when there is none, so treating
/// an unlabelled invoice as Preisblatt B would refuse every ordinary
/// Messstellenbetriebsrechnung. Reading it the other way names a tree whose
/// codes are not walked here, which answers without a code — a weaker
/// statement, not a wrong one.
fn rechnungsgegenstand(rechnungstyp: Option<&str>) -> mako_pruefung::codes::MsbRechnungsgegenstand {
    use mako_pruefung::codes::MsbRechnungsgegenstand as G;
    if rechnungstyp == Some(RECHNUNGSTYP_TECHNIK) {
        G::PreisblattB
    } else {
        G::Messstellenbetrieb
    }
}

/// The `SG7 AJT` a REMADV Abweisung carries — the code(s), the tree that
/// publishes them, and the Prüfidentifikator their shape requires.
///
/// # The tree is not a constant
///
/// Every invoice Use-Case has its own quartet, and PID 31009 alone has three
/// of them ([`mako_pruefung::codes::rechnungspruefung`]). Stamping `E_0406` on
/// all of them named the Netznutzungsabrechnung tree on an ESA's answer, whose
/// codes mean something else entirely — `A70` is the `E_0406`
/// Summenprüfung and is undefined in `E_0264`, whose own total check is `A24`.
/// `E_0406` is not even admissible on REMADV 33002 (REMADV AHB 1.0a § 3.1.1
/// lists the trees DE 1082 accepts, and it is not among them).
///
/// Returns `None` when this service walks no tree for the Use-Case: the answer
/// then goes out without an `AJT` and an operator completes it, which is
/// incomplete rather than wrong.
fn abweichungsgrund(
    pid: u32,
    empfaenger: mako_fristen::vorlauf::RechnungEmpfaenger,
    gegenstand: mako_pruefung::codes::MsbRechnungsgegenstand,
    markt_antwort: Option<&mako_pruefung::esa::RechnungsAntwort>,
    storno_antwort: Option<&mako_pruefung::esa::StornoAntwort>,
    findings: &[invoic_checker::Finding],
) -> Option<mako_invoic::RemadvAntwort> {
    // `E_0267` answers with **one** code and rides the plain Abweisung 33002 —
    // the one tree of the ESA family REMADV AHB 1.0a § 3.1.1 admits in DE 1082.
    // Its `KeineAntwort` outcome never reaches here: the caller returns before
    // building an answer at all.
    if let Some(mako_pruefung::esa::StornoAntwort::Ablehnen {
        antwort,
        pruefschritt,
        detail,
    }) = storno_antwort
    {
        return Some(mako_invoic::RemadvAntwort {
            ebd: antwort.tree.clone(),
            remadv_pid: mako_invoic::ABWEISUNG_PID,
            befunde: vec![mako_invoic::RemadvBefund {
                code: antwort.antwortcode.clone(),
                ebene: "kopf".to_owned(),
                positionsnummer: None,
                detail: Some(format!("Prüfschritt {pruefschritt}: {detail}")),
            }],
        });
    }

    // The ESA's own tree was walked in full: its Befunde already carry the
    // Ebene and the Positionsnummer, and it knows its own REMADV PID.
    //
    // A walk that found **nothing** returns `None` rather than falling through:
    // the tree said the invoice is payable, and a dispute reached here on
    // mako's own monetary threshold instead. That escalation has no published
    // code, and borrowing one from a Prüfschritt the walk cleared would state
    // to the market that a check failed which did not.
    if let Some(a) = markt_antwort {
        if a.ist_zustimmung() {
            return None;
        }
        return Some(mako_invoic::RemadvAntwort {
            ebd: a.tree.to_owned(),
            remadv_pid: a.remadv_pid(),
            befunde: a
                .befunde
                .iter()
                .map(|b| mako_invoic::RemadvBefund {
                    code: b.antwort.antwortcode.clone(),
                    ebene: match b.ebene {
                        mako_pruefung::esa::Ebene::Kopf => "kopf".to_owned(),
                        mako_pruefung::esa::Ebene::Position(_) => "position".to_owned(),
                        mako_pruefung::esa::Ebene::Summe => "summe".to_owned(),
                    },
                    positionsnummer: match b.ebene {
                        mako_pruefung::esa::Ebene::Position(nr) => Some(nr),
                        _ => None,
                    },
                    detail: Some(b.detail.clone()),
                })
                .collect(),
        });
    }

    // Everything else: a tree named from the table, and a code only where this
    // service actually carries that tree's Codeliste. Today that is the
    // Netznutzungsabrechnung alone — `E_0210`, `E_0259` and `E_0566` are named
    // correctly and answered without a code until their Codelisten are walked.
    let trees = mako_pruefung::codes::rechnungspruefung(pid, empfaenger, gegenstand)?;
    if trees.rechnung != mako_pruefung::codes::EBD_NETZNUTZUNGSRECHNUNG {
        return None;
    }
    let code = netznutzung_antwortcode(findings);
    Some(mako_invoic::RemadvAntwort {
        ebd: trees.rechnung.to_owned(),
        // A single code from an unwalked tree is a Summen-level statement, and
        // 33003 is „Abweisung Kopf und Summe".
        remadv_pid: 33_003,
        befunde: vec![mako_invoic::RemadvBefund {
            code: code.to_owned(),
            ebene: if findings
                .iter()
                .any(|f| f.is_dispute && f.line_number.is_some())
            {
                "position".to_owned()
            } else {
                "summe".to_owned()
            },
            positionsnummer: None,
            detail: Some(dispute_reason(findings)),
        }],
    })
}

/// The `E_0406` Antwortcode a Netznutzungsabrechnung Abweisung carries.
///
/// Only one finding maps to a code with an exact counterpart in the tree:
/// [`FindingKind::TotalMismatch`] is Prüfschritt 900 („Entspricht der
/// Rechnungsbetrag der Summe aller Rechnungspositionen?"), which is `A70`.
/// Everything else lands on the catch-alls, which the BDEW requires to carry a
/// written Erläuterung — supplied here from the finding text.
///
/// The full tree — 205 Prüfschritte over Kopf-, Positions- und Summenebene,
/// answering with a *set* of (Positionsnummer, code) pairs — is not walked
/// here; see `mako_pruefung::codes::E_0406_CODES`. The ESA tree `E_0264` **is**
/// walked, in `invoic_checker::rechnung`.
fn netznutzung_antwortcode(findings: &[invoic_checker::Finding]) -> &'static str {
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
        assert_eq!(netznutzung_antwortcode(&[total]), "A70");

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
            netznutzung_antwortcode(&[line]),
            "A99",
            "a faulty line takes the Positionsebene catch-all"
        );

        assert_eq!(
            netznutzung_antwortcode(&[]),
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

    /// **The tree is not a constant.** PID 31009 toward an ESA is `E_0264`;
    /// stamping `E_0406` on it named the Netznutzungsabrechnung tree, whose
    /// `A70` is undefined there — and which REMADV AHB 1.0a § 3.1.1 does not
    /// even admit in DE 1082 of the plain Abweisung.
    #[test]
    fn the_answer_names_the_tree_the_use_case_publishes() {
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        use mako_pruefung::codes::MsbRechnungsgegenstand as G;

        // Netznutzung: `E_0406`, with the codes this service resolves.
        let total = Finding {
            kind: FindingKind::TotalMismatch,
            is_dispute: true,
            message: "Gesamtnetto weicht ab".into(),
            line_number: None,
            expected: None,
            actual: None,
            deviation_pct: None,
        };
        let nn = abweichungsgrund(
            31_002,
            R::LieferantOderMsb,
            G::Messstellenbetrieb,
            None,
            None,
            &[total],
        )
        .expect("E_0406 codes are carried");
        assert_eq!(nn.ebd, "E_0406");
        assert_eq!(nn.erster_code(), Some("A70"));

        // 31009 toward an LF is `E_0210`, whose codes this service does not
        // carry — so it emits **no** code rather than one from another tree.
        assert!(
            abweichungsgrund(
                31_009,
                R::LieferantOderMsb,
                G::Messstellenbetrieb,
                None,
                None,
                &[]
            )
            .is_none(),
            "an unwalked tree must not borrow another tree's codes"
        );
    }

    /// The ESA answer comes from the walked `E_0264`, keeps every Befund with
    /// its Ebene and Positionsnummer, and rides the Prüfidentifikator that
    /// shape requires — 33004 for position-level refusals.
    #[test]
    fn the_esa_answer_carries_the_whole_walk() {
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        use mako_pruefung::codes::MsbRechnungsgegenstand as G;
        use mako_pruefung::esa;

        let walk = esa::pruefe_rechnung(
            &esa::RechnungsFakten {
                einwaende_entkraeftet: None,
                ustg_konform: Some(true),
                rechnungsdatum: time::macros::date!(2026 - 04 - 01),
                eingangsdatum: time::macros::date!(2026 - 04 - 01),
                leistungszeitraum: None,
                bestellung_bekannt: true,
                rechnungsnummer_bereits_verwendet: false,
                faelliger_betrag_nicht_negativ: true,
                zahlungsziel: None,
                preisblatt_version_gueltig: None,
                zeitraum_bereits_abgerechnet_in: None,
                sonstiger_kopffehler: None,
                positionen: vec![esa::PositionsFakten {
                    positionsnummer: 7,
                    artikel_id: Some("9990001100002".to_owned()),
                    artikel_id_aus_bestellung: true,
                    leistung_erbracht: Some(true),
                    preis_wie_angebot: Some(false),
                    steuersatz_korrekt: Some(true),
                    zeitraum: None,
                    bereits_abgerechnet_in: None,
                    rechenfehler: false,
                    sonstiger_fehler: None,
                }],
                fehlende_artikel_ids: Vec::new(),
                steuersaetze: Vec::new(),
                rechnungsbetrag_stimmt: true,
                sonstiger_summenfehler: None,
            },
            mako_pruefung::HolidayCalendar::BdewMaKo,
        );

        let grund = abweichungsgrund(
            31_009,
            R::Esa,
            G::Messstellenbetrieb,
            Some(&walk),
            None,
            &[],
        )
        .expect("E_0264 is walked");
        assert_eq!(grund.ebd, "E_0264");
        assert_eq!(grund.remadv_pid, 33_004, "Abweisung Position");
        assert_eq!(grund.erster_code(), Some("A11"));
        assert_eq!(grund.befunde[0].positionsnummer, Some(7));
        assert_eq!(grund.befunde[0].ebene, "position");
    }

    /// A clean ESA invoice produces no Abweichungsgrund at all — the answer is
    /// the Zahlungsavis 33001, which carries no `AJT`.
    #[test]
    fn a_clean_esa_invoice_states_no_antwortcode() {
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        use mako_pruefung::codes::MsbRechnungsgegenstand as G;
        let clean = mako_pruefung::esa::RechnungsAntwort {
            tree: "E_0264",
            befunde: Vec::new(),
        };
        assert!(clean.ist_zustimmung());
        assert!(
            abweichungsgrund(
                31_009,
                R::Esa,
                G::Messstellenbetrieb,
                Some(&clean),
                None,
                &[]
            )
            .is_none()
        );
    }

    /// **`E_0267`'s three outcomes, and the quiet one is the point.** A Storno
    /// of an invoice this ESA had itself refused — or had not answered yet —
    /// needs no answer at all (Prüfschritt 80). A refusal rides the plain
    /// Abweisung 33002, the one Prüfidentifikator REMADV AHB 1.0a § 3.1.1
    /// admits `E_0267` in.
    #[test]
    fn the_storno_answer_is_a_three_way_decision() {
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        use mako_pruefung::codes::MsbRechnungsgegenstand as G;
        use mako_pruefung::esa::{StornoAntwort, UrsprungsAntwort, pruefe_stornorechnung};

        let facts = |ursprungsantwort| mako_pruefung::esa::StornoFakten {
            ursprungsrechnung_bekannt: true,
            rechnungsnummer_bereits_verwendet: false,
            ustg_konform: Some(true),
            bereits_storniert: false,
            rechnungstyp_identisch: true,
            zeitraum_identisch: true,
            betraege_negiert_identisch: true,
            sonstiger_fehler: None,
            ursprungsantwort,
        };

        // Paid → confirm with the Zahlungsavis, which carries no code.
        let zugestimmt = pruefe_stornorechnung(&facts(UrsprungsAntwort::Zugestimmt));
        assert_eq!(zugestimmt.remadv_pid(), Some(33_001));
        assert!(
            abweichungsgrund(
                31_004,
                R::Esa,
                G::Messstellenbetrieb,
                None,
                Some(&zugestimmt),
                &[]
            )
            .is_none(),
            "a confirmed Storno states no Antwortcode"
        );

        // Refused or unanswered → **no message**. The caller returns before
        // building an answer, so nothing reaches the wire.
        for quiet in [UrsprungsAntwort::Abgelehnt, UrsprungsAntwort::Unbeantwortet] {
            let a = pruefe_stornorechnung(&facts(quiet));
            assert!(matches!(a, StornoAntwort::KeineAntwort { .. }), "{quiet:?}");
            assert_eq!(a.remadv_pid(), None, "{quiet:?}");
        }

        // A defective Storno refuses with its own code on 33002 — never on
        // 33003/33004, which DE 1082 does not admit `E_0267` in.
        let doppelt = pruefe_stornorechnung(&mako_pruefung::esa::StornoFakten {
            bereits_storniert: true,
            ..facts(UrsprungsAntwort::Zugestimmt)
        });
        let grund = abweichungsgrund(
            31_004,
            R::Esa,
            G::Messstellenbetrieb,
            None,
            Some(&doppelt),
            &[],
        )
        .expect("a refused Storno states why");
        assert_eq!(grund.ebd, "E_0267");
        assert_eq!(grund.remadv_pid, 33_002);
        assert_eq!(grund.erster_code(), Some("A02"));
        assert!(
            grund.befunde[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("Prüfschritt 20")),
            "the answer names the Prüfschritt it came from: {:?}",
            grund.befunde[0].detail
        );
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

    /// PID 31009 carries five Use-Cases and the recipient's Marktrolle narrows
    /// it to two. `IMD+7081` is what closes it: `TEC` „Abrechnung von Technik"
    /// is a Leistung of the Preisblatt B and resolves to `E_0270`/`E_0273`,
    /// where `MSB` and an absent element resolve to `E_0210`/`E_0566`.
    ///
    /// Getting this wrong names a tree whose codes mean something else — the
    /// same class of defect as stamping `E_0406` on an ESA answer.
    #[test]
    fn imd_7081_tec_selects_the_preisblatt_b_trees() {
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        use mako_pruefung::codes::{MsbRechnungsgegenstand as G, rechnungspruefung};

        assert_eq!(rechnungsgegenstand(Some("TEC")), G::PreisblattB);
        for typ in [Some("MSB"), Some("KON"), Some("WIM"), None] {
            assert_eq!(
                rechnungsgegenstand(typ),
                G::Messstellenbetrieb,
                "rechnungstyp {typ:?}"
            );
        }

        let tree = |empf, typ| {
            rechnungspruefung(31_009, empf, rechnungsgegenstand(typ))
                .expect("31009 is published for every Marktrolle")
                .rechnung
        };
        assert_eq!(tree(R::Netzbetreiber, Some("MSB")), "E_0566");
        assert_eq!(tree(R::Netzbetreiber, Some("TEC")), "E_0273");
        assert_eq!(tree(R::LieferantOderMsb, Some("MSB")), "E_0210");
        assert_eq!(tree(R::LieferantOderMsb, Some("TEC")), "E_0270");
        // An ESA has no Preisblatt B — its prices come from the accepted
        // QUOTES 15003 — so `TEC` does not move its tree.
        assert_eq!(tree(R::Esa, Some("TEC")), "E_0264");
    }

    /// The Preisblatt-B path: `IMD+7081` = `TEC` plus the deployment's own
    /// Marktrolle select one of two quartets, and the ESA's `E_0264` is not
    /// among them.
    #[test]
    fn tec_and_the_configured_role_select_the_preisblatt_b_family() {
        use crate::config::EmpfaengerRolle as Rolle;
        use mako_pruefung::rechnung::{ESA, PREISBLATT_B_LF, PREISBLATT_B_NB};

        assert_eq!(
            familie_fuer(31_009, Rolle::Lieferant, Some("TEC")),
            Some(PREISBLATT_B_LF)
        );
        assert_eq!(
            familie_fuer(31_009, Rolle::Netzbetreiber, Some("TEC")),
            Some(PREISBLATT_B_NB)
        );
        // An ESA has no Preisblatt B — `TEC` does not move its family.
        assert_eq!(familie_fuer(31_009, Rolle::Esa, Some("TEC")), Some(ESA));

        // `MSB` and an absent qualifier are the Messstellenbetrieb, whose
        // Codelisten this service does not carry: the tree is named correctly
        // by `rechnungspruefung` and the answer goes out without a code.
        for typ in [Some("MSB"), None] {
            for rolle in [Rolle::Lieferant, Rolle::Netzbetreiber] {
                assert_eq!(
                    familie_fuer(31_009, rolle, typ),
                    None,
                    "rolle={rolle:?} typ={typ:?}"
                );
            }
        }
    }

    /// The role is configuration and the ESA signal is evidence — a walked
    /// `E_0264` answer outranks it, because it only exists where the wire said
    /// `KON` or an accepted Angebot corroborated it.
    #[test]
    fn a_walked_esa_answer_outranks_the_configured_role() {
        use crate::config::EmpfaengerRolle as Rolle;
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;

        assert_eq!(
            rechnung_empfaenger(Rolle::Lieferant, None),
            R::LieferantOderMsb
        );
        assert_eq!(
            rechnung_empfaenger(Rolle::Netzbetreiber, None),
            R::Netzbetreiber
        );
        assert_eq!(rechnung_empfaenger(Rolle::Esa, None), R::Esa);

        let walked = mako_pruefung::rechnung::RechnungsAntwort {
            tree: mako_pruefung::codes::EBD_ESA_RECHNUNG,
            befunde: vec![],
        };
        for rolle in [Rolle::Lieferant, Rolle::Netzbetreiber, Rolle::Esa] {
            assert_eq!(
                rechnung_empfaenger(rolle, Some(&walked)),
                R::Esa,
                "{rolle:?}"
            );
        }
    }
}
