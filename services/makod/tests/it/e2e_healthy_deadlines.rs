//! A healthy process survives every deadline it registered.
//!
//! Deadlines are registered when a message arrives and are never cancelled when
//! the process answers, so every one of them fires — on the healthy path as
//! much as on the failing one. Whether a firing acts is the workflow's own
//! decision ([`mako_engine::workflow::Workflow::on_deadline`]): the label names
//! the window, and the state says whether that window is still open.
//!
//! These scenarios drive a process through the real deadline dispatcher
//! ([`makod::orchestrator::deadline_dispatch::dispatch_deadline`]) against the
//! production store, with every counterparty answering in time, and require
//! that no firing turns the process `Rejected`:
//!
//! - **GeLi Gas Lieferbeginn (44001).** The GNB answers inside its 4-Werktage
//!   window (GeLi Gas 3.0 Kap. 3.2.3). In Gas an APERAK is a
//!   Verarbeitbarkeitsfehlermeldung only (APERAK AHB 1.1 §2.3), so a
//!   processable Anmeldung sends none, and no APERAK window may be left to
//!   fire on it.
//! - **WiM Beginn Messstellenbetrieb (55042 Strom, 44042 Gas).** The NB
//!   confirms within 5 Werktage and then waits for the MSBN's Mitteilung über
//!   den Gesamtvorgang (WiM Teil 1 Kap. 2.3.2 Nr. 7) — the answer window it
//!   already met fires while it waits.

use mako_engine::process::RunOptions;
use std::sync::Arc;

use mako_engine::{
    deadline::DeadlineStore as _,
    ids::TenantId,
    lf_vorgang::LfVorgangsdaten,
    process::Process,
    store_slatedb::SlateDbStore,
    types::{DeviceId, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::Workflow,
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GasSupplierChangeState, GeliGasSupplierChangeWorkflow,
};
use mako_wim::{DeviceChangeCommand, DeviceChangeState, WimDeviceChangeWorkflow};
use makod::orchestrator::deadline_dispatch::{dispatch_deadline, production_drivers};
use time::macros::datetime;

const LF_ID: &str = "4012345000023";
const NB_ID: &str = "9900357000004";
const MSBN_ID: &str = "4012345000030";
const MALO: &str = "51238696012";
const MELO: &str = "DE0001000001234567890000000000001";
const FV: &str = "FV2025-10-01";

/// Monday 2026-03-02, 09:00 Berlin — no holiday within the windows below.
const RECEIVED_AT: time::OffsetDateTime = datetime!(2026-03-02 08:00 UTC);

/// The MSBN reports the Gesamtvorgang the day after the Zuordnungsbeginn
/// 2026-03-16 — well inside its 10-Werktage window (Kap. 2.3.2 Nr. 7).
const GESAMTVORGANG_AT: time::OffsetDateTime = datetime!(2026-03-17 08:00 UTC);

/// Past every window any scenario registers.
const NEVER: time::OffsetDateTime = datetime!(2030-01-01 00:00 UTC);

async fn production_store() -> Arc<SlateDbStore> {
    Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory production store"),
    )
}

/// Fire every deadline registered on `process`'s stream that is due by
/// `until`, soonest first, the way the scheduler does: dispatch, then retire it.
///
/// Returns the labels that fired, so a scenario can say which windows it ran
/// through.
async fn fire_due<W: Workflow>(
    store: &Arc<SlateDbStore>,
    process: &Process<W, Arc<SlateDbStore>>,
    until: time::OffsetDateTime,
) -> Vec<String> {
    let deadlines = store.as_deadline_store();
    let mut due = deadlines
        .for_stream(process.stream_id())
        .await
        .expect("read the stream's deadlines");
    due.retain(|d| d.due_at() <= until);
    due.sort_by_key(mako_engine::deadline::Deadline::due_at);
    let mut fired = Vec::new();
    for deadline in due {
        fired.push(deadline.label().to_owned());
        let id = deadline.deadline_id();
        dispatch_deadline(
            production_drivers(),
            deadline,
            Arc::clone(store),
            store.as_snapshot_store(),
            0,
        )
        .await
        .expect("a fired deadline dispatches");
        deadlines.cancel(id).await.expect("retire the deadline");
    }
    fired
}

// ── GeLi Gas Lieferbeginn ────────────────────────────────────────────────────

fn gas_anmeldung() -> GasSupplierChangeCommand {
    GasSupplierChangeCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(44_001).expect("PID"),
        sender: MarktpartnerCode::new(LF_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        malo_id: MaLo::new(MALO),
        document_date: "20260302".to_owned(),
        process_date: "20260401".to_owned(),
        message_ref: MessageRef::new("GAS-ANM-0001"),
        validation_passed: true,
        validation_errors: vec![],
        received_at: RECEIVED_AT,
        bilanzierungsmethode: None,
        vorgang: LfVorgangsdaten::default(),
        fallgruppe: None,
        gasqualitaet: None,
    }
}

/// A GNB that confirms a 44001 in time ends `AntwortGesendet`, whatever fires.
#[tokio::test]
async fn a_healthy_gas_anmeldung_survives_every_deadline() {
    let store = production_store().await;
    let tenant = TenantId::from_party_id(NB_ID);
    let process = Process::<GeliGasSupplierChangeWorkflow, _>::new(
        Arc::clone(&store),
        tenant,
        WorkflowId::new(mako_geli_gas::WORKFLOW_NAME, FV),
    );
    // The ingest spawn registers the GNB's answer window beside whatever the
    // workflow registers itself; mirror it.
    let answer_window = mako_engine::deadline::Deadline::new(
        process.stream_id().clone(),
        process.process_id(),
        tenant,
        WorkflowId::new(mako_geli_gas::WORKFLOW_NAME, FV),
        mako_geli_gas::LIEFERBEGINN_RESPONSE_WINDOW_LABEL,
        mako_fristen::antwort::antwort_deadline(44_001, RECEIVED_AT).expect("44001 Frist"),
    );
    process
        .run(
            gas_anmeldung(),
            RunOptions::new().deadlines(&[answer_window]),
        )
        .await
        .expect("a valid 44001 spawns");

    // The GNB confirms on the day it received the Anmeldung.
    process
        .run(
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                antwort_code: "E15".to_owned(),
                bemerkung: None,
                obligations: vec![],
            },
            RunOptions::new().attempts(3),
        )
        .await
        .expect("the GNB answers");

    let fired = fire_due(&store, &process, NEVER).await;
    assert!(
        !fired.is_empty(),
        "the scenario must run through at least the answer window"
    );

    let state = process.state().await.expect("replay");
    assert!(
        matches!(state, GasSupplierChangeState::AntwortGesendet { .. }),
        "an Anmeldung the GNB confirmed in time must not be ended by a deadline \
         (fired: {fired:?}); got {state:?}"
    );

    // And the healthy path continues: supply starts.
    process
        .run(
            GasSupplierChangeCommand::Activate,
            RunOptions::new().attempts(3),
        )
        .await
        .expect("an answered Anmeldung activates");
    assert!(matches!(
        process.state().await.expect("replay"),
        GasSupplierChangeState::Active(_)
    ));
}

/// A processable Gas Anmeldung sends no APERAK, so it owes none.
#[tokio::test]
async fn a_processable_gas_anmeldung_registers_no_aperak_window() {
    let store = production_store().await;
    let tenant = TenantId::from_party_id(NB_ID);
    let process = Process::<GeliGasSupplierChangeWorkflow, _>::new(
        Arc::clone(&store),
        tenant,
        WorkflowId::new(mako_geli_gas::WORKFLOW_NAME, FV),
    );
    process
        .run(gas_anmeldung(), RunOptions::new().deadlines(&[]))
        .await
        .expect("a valid 44001 spawns");
    let labels: Vec<String> = store
        .as_deadline_store()
        .for_stream(process.stream_id())
        .await
        .expect("read deadlines")
        .iter()
        .map(|d| d.label().to_owned())
        .collect();
    assert!(
        !labels
            .iter()
            .any(|l| l.starts_with(mako_fristen::APERAK_WINDOW_LABEL_PREFIX)),
        "no APERAK goes out for a processable Gas Anmeldung (APERAK AHB 1.1 §2.3), \
         so no APERAK delivery window may be armed; got {labels:?}"
    );
}

// ── WiM Beginn Messstellenbetrieb ────────────────────────────────────────────

fn msb_anmeldung(pid: u32) -> DeviceChangeCommand {
    DeviceChangeCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(pid).expect("PID"),
        sender: MarktpartnerCode::new(MSBN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        melo_id: MeLo::new(MELO),
        device_id: DeviceId::new("ZHR-1"),
        document_date: "20260302".to_owned(),
        message_ref: MessageRef::new("WIM-ANM-0001"),
        vorgangsnummer: Some("VG-1".to_owned()),
        process_date: Some("20260316".to_owned()),
        transaktionsgrund: Some("E03".to_owned()),
        validation_passed: true,
        validation_errors: vec![],
        received_at: RECEIVED_AT,
    }
}

/// The NB answers a Beginn MSB in time and waits for the Gesamtvorgang; every
/// window it registered fires on the way, and the Zuordnung still completes.
async fn healthy_beginn_msb(pid: u32, strom: bool) {
    let store = production_store().await;
    let tenant = TenantId::from_party_id(NB_ID);
    let process = Process::<WimDeviceChangeWorkflow, _>::new(
        Arc::clone(&store),
        tenant,
        WorkflowId::new(mako_wim::WORKFLOW_NAME, FV),
    );
    process
        .run(msb_anmeldung(pid), RunOptions::new().deadlines(&[]))
        .await
        .expect("a valid Anmeldung MSB spawns");
    if strom {
        // Strom acknowledges a processable UTILMD with an Anerkennungsmeldung.
        process
            .run(
                DeviceChangeCommand::DispatchAperak {
                    positive: true,
                    reason: None,
                },
                RunOptions::new().attempts(3),
            )
            .await
            .expect("the APERAK goes out");
    }
    process
        .run(
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: true,
                antwort_code: "E15".to_owned(),
                bemerkung: None,
                abweichender_termin: None,
            },
            RunOptions::new().attempts(3),
        )
        .await
        .expect("the NB confirms in time");

    // Everything that comes due before the MSBN's Mitteilung über den
    // Gesamtvorgang fires while the NB waits for it.
    let fired = fire_due(&store, &process, GESAMTVORGANG_AT).await;
    let waiting = process.state().await.expect("replay");
    assert!(
        matches!(waiting, DeviceChangeState::AntwortGesendet(_)),
        "PID {pid}: an Anmeldung the NB confirmed in time waits for the Gesamtvorgang; \
         a deadline must not end it (fired: {fired:?}); got {waiting:?}"
    );

    // The MSBN reports the successful Gesamtvorgang, the NB assigns.
    process
        .run(
            DeviceChangeCommand::ReceiveGesamtvorgang {
                pid: Pruefidentifikator::new(mako_wim::geraetewechsel::GESAMTVORGANG_ERFOLG_PID)
                    .expect("PID"),
                zuordnungsbeginn: Some("20260316".to_owned()),
                message_ref: MessageRef::new("WIM-GV-0001"),
                received_at: GESAMTVORGANG_AT,
            },
            RunOptions::new().attempts(3),
        )
        .await
        .expect("the Gesamtvorgang arrives");
    process
        .run(
            DeviceChangeCommand::DispatchZuordnung { zugeordnet: true },
            RunOptions::new().attempts(3),
        )
        .await
        .expect("the NB assigns the MSBN");

    let late = fire_due(&store, &process, NEVER).await;
    let done = process.state().await.expect("replay");
    assert!(
        matches!(done, DeviceChangeState::Completed(_)),
        "PID {pid}: a completed Zuordnung must survive the windows still on its \
         stream (fired: {late:?}); got {done:?}"
    );
}

#[tokio::test]
async fn a_healthy_strom_beginn_msb_waits_for_the_gesamtvorgang() {
    healthy_beginn_msb(55_042, true).await;
}

#[tokio::test]
async fn a_healthy_gas_beginn_msb_waits_for_the_gesamtvorgang() {
    healthy_beginn_msb(44_042, false).await;
}

// ── The dispatcher refuses what no workflow owns ─────────────────────────────

/// The MaBiS Clearingfenster closes on its own label and on nothing else.
///
/// The close is the one deadline `mabis-billing` owns
/// ([`mako_mabis::CLEARING_ENDE_LABEL`]); a label it does not own reaches no
/// workflow, closes nothing and is reported as unowned — it is never mapped onto
/// a default Abrechnungslauf.
#[tokio::test]
async fn an_unowned_label_closes_no_clearing_window() {
    use mako_mabis::{
        BillingCommand, BillingState, MabisBillingWorkflow, SUMMENZEITREIHE_PID, SzrVersion,
        zeitreihen::{Familie, Kategorie, Zeitreihe},
    };
    use makod::orchestrator::deadline_dispatch::Fired;

    let store = production_store().await;
    let tenant = TenantId::from_party_id(NB_ID);
    let process = Process::<MabisBillingWorkflow, _>::new(
        Arc::clone(&store),
        tenant,
        WorkflowId::new(mako_mabis::BILLING_WORKFLOW_NAME, FV),
    );
    process
        .run(
            BillingCommand::ReceiveSummenzeitreihe {
                pid: Pruefidentifikator::new(SUMMENZEITREIHE_PID).expect("PID"),
                zeitreihe: Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B))
                    .expect("Tabelle-1 row"),
                mabis_zp: mako_mabis::MabisZaehlpunktId::new("DE0001111222233334444555566667777")
                    .expect("33 chars"),
                bilanzierungsmonat: mako_engine::types::BillingPeriod::new("2026-01"),
                version: SzrVersion::new("20260101120001+00").expect("17 chars"),
                im_erstaufschlag: true,
                clearing_ende: None,
                absender: MarktpartnerCode::new(NB_ID),
                biko_id: mako_engine::types::BikoId::new("10YDE-VE-TRANSMIX"),
                message_ref: MessageRef::new("MSCONS-1"),
            },
            RunOptions::new().attempts(3),
        )
        .await
        .expect("the Summenzeitreihe opens the settlement");

    let deadline = |label: &str| {
        mako_engine::deadline::Deadline::new(
            process.stream_id().clone(),
            process.process_id(),
            tenant,
            WorkflowId::new(mako_mabis::BILLING_WORKFLOW_NAME, FV),
            label,
            RECEIVED_AT,
        )
    };

    let fired = dispatch_deadline(
        production_drivers(),
        deadline("mabis-kbka-clearingfenster-ende"),
        Arc::clone(&store),
        store.as_snapshot_store(),
        0,
    )
    .await
    .expect("dispatch");
    assert_eq!(fired, Fired::Unowned);
    assert!(
        matches!(
            process.state().await.expect("replay"),
            BillingState::Offen(_)
        ),
        "a label the workflow does not own must close nothing"
    );

    let fired = dispatch_deadline(
        production_drivers(),
        deadline(mako_mabis::CLEARING_ENDE_LABEL),
        Arc::clone(&store),
        store.as_snapshot_store(),
        0,
    )
    .await
    .expect("dispatch");
    assert_eq!(fired, Fired::Acted);
    assert!(matches!(
        process.state().await.expect("replay"),
        BillingState::Geschlossen(_)
    ));
}
