//! An inbound message is judged on the instant the door received it, and its
//! APERAK delivery window is derived in one place.
//!
//! The Übertragungstag is the „Tag des Empfangs der Übertragungsdatei" (GPKE
//! Teil 1 Kap. 7 „Fristenberechnung", S. 27). A message dispatched after
//! midnight was still received the day before, so the command the adapter
//! builds, the CloudEvent `time` processd counts its Antwortfrist from, and the
//! APERAK window all carry the receipt instant, never the dispatch instant.
//!
//! The APERAK window follows the message type, the Sparte and the PID (APERAK
//! AHB 1.1 § 2.4.1 Strom, § 2.3.1 Gas) and is registered on a spawn and on a
//! resume alike. A Gas APERAK reports a Verarbeitbarkeitsfehler only (§ 2.3), so
//! its window exists only beside an APERAK that was enqueued.

use std::any::Any;
use std::sync::Arc;

use mako_engine::{
    deadline::DeadlineStore as _, ids::TenantId, outbox::OutboxStore as _,
    store_slatedb::SlateDbStore, version::FormatVersion,
};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN: &str = "9900357000004";

/// Tue 2026-07-07 12:00 CEST — months before any dispatch of this suite.
const RECEIVED_AT: time::OffsetDateTime = time::macros::datetime!(2026-07-07 10:00 UTC);

/// The Marktlokation the 55001 skeleton names in `LOC+Z16`.
const MALO: &str = "51238696781";

/// The profile skeleton of `pid` with its `LOC+172` naming [`MALO`], so a
/// message the skeleton leaves unlocated correlates with the 55001.
fn located(mt: edi_energy::MessageType, pid: u32) -> edi_energy::AnyMessage {
    let wire = String::from_utf8(skeleton(mt, pid)).expect("UTF-8");
    let wire = match wire.find("LOC+172") {
        Some(at) => {
            let end = at + wire[at..].find('\'').expect("segment terminator");
            format!("{}LOC+172+{MALO}{}", &wire[..at], &wire[end..])
        }
        None => wire,
    };
    edi_energy::parse(wire.as_bytes()).expect("the located skeleton parses")
}

fn fixture(mt: edi_energy::MessageType, pid: u32) -> edi_energy::AnyMessage {
    edi_energy::parse(&skeleton(mt, pid)).expect("the skeleton parses")
}

fn skeleton(mt: edi_energy::MessageType, pid: u32) -> Vec<u8> {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(mt)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN.to_owned(),
            },
        )
        .expect("skeleton")
}

async fn setup() -> (SlateDbStore, EdifactIngestDispatcher) {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    (store, dispatcher)
}

/// The labels and instants of every deadline registered so far.
async fn windows(store: &SlateDbStore) -> Vec<(String, time::OffsetDateTime)> {
    store
        .as_deadline_store()
        .due_now(100)
        .await
        .expect("read deadlines")
        .deadlines
        .iter()
        .filter(|d| mako_fristen::is_delivery_window(d.label()))
        .map(|d| (d.label().to_owned(), d.due_at()))
        .collect()
}

/// Every outbox entry a 55001 enqueues reports the receipt instant as its
/// occurrence — the CloudEvent `time` processd reads as `received_at`.
#[tokio::test]
async fn the_outbox_reports_the_receipt_instant() {
    let (store, d) = setup().await;
    let msg = fixture(edi_energy::MessageType::Utilmd, 55001);
    let outcome = d
        .dispatch(&msg, "gpke-supplier-change", 55001, RECEIVED_AT)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );
    let queued = store.pending_now(10).await.expect("outbox");
    assert!(
        !queued.is_empty(),
        "the Anmeldung enqueues its notifications"
    );
    for m in &queued {
        assert_eq!(
            m.occurred_at,
            Some(RECEIVED_AT),
            "{} must report the receipt instant, not the dispatch instant",
            m.message_type
        );
    }
}

/// The adapter stamps the receipt instant, not the instant it runs — the EoG
/// answer window is counted from it (GPKE Teil 2, Ersatz-/Grundversorgung).
#[test]
fn the_adapter_command_carries_the_receipt_instant() {
    use mako_gpke::eog::EogCommand;
    let msg = fixture(edi_energy::MessageType::Utilmd, 55013);
    let cmd = makod::adapters::gpke_eog_registry(RECEIVED_AT)
        .dispatch(&msg as &dyn Any, &FormatVersion::new("FV2025-10-01"))
        .expect("the Zuordnung adapts");
    let EogCommand::ReceiveAnmeldung { received_at, .. } = cmd else {
        panic!("expected ReceiveAnmeldung");
    };
    assert_eq!(received_at, RECEIVED_AT);
}

/// A Strom MSCONS is not a UTILMD or an ORDERS: its APERAK is owed by the next
/// Werktag 12 Uhr (APERAK AHB 1.1 § 2.4.1, first paragraph).
#[tokio::test]
async fn a_strom_mscons_spawn_carries_the_next_werktag_window() {
    let (store, d) = setup().await;
    let msg = located(edi_energy::MessageType::Mscons, 13018);
    let outcome = d
        .dispatch(&msg, "gpke-messwerte", 13018, RECEIVED_AT)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        windows(&store).await,
        vec![(
            mako_fristen::APERAK_STROM_ALLGEMEIN_LABEL.to_owned(),
            mako_fristen::aperak_strom_allgemein_due_at(RECEIVED_AT),
        )]
    );
}

/// A Strom UTILMD opens the 45-minute window; a message that resumes the
/// process opens its own window too — the obligation is per message.
#[tokio::test]
async fn a_resume_carries_its_aperak_window() {
    let (store, d) = setup().await;
    let anmeldung = fixture(edi_energy::MessageType::Utilmd, 55001);
    d.dispatch(&anmeldung, "gpke-supplier-change", 55001, RECEIVED_AT)
        .await
        .expect("spawn");
    let later = RECEIVED_AT + time::Duration::hours(1);
    let vollzug = located(edi_energy::MessageType::Iftsta, 21024);
    let outcome = d
        .dispatch(&vollzug, "gpke-supplier-change", 21024, later)
        .await
        .expect("resume");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "the Vollzugsmeldung resumes the Anmeldung: {outcome:?}"
    );
    let mut got = windows(&store).await;
    got.sort();
    let mut want = vec![
        (
            mako_fristen::APERAK_STROM_WINDOW_LABEL.to_owned(),
            mako_fristen::aperak_strom_due_at(RECEIVED_AT),
        ),
        (
            mako_fristen::APERAK_STROM_ALLGEMEIN_LABEL.to_owned(),
            mako_fristen::aperak_strom_allgemein_due_at(later),
        ),
    ];
    want.sort();
    assert_eq!(got, want);
}

/// A processable Gas message is answered by silence (APERAK AHB 1.1 § 2.3), so
/// no window watches for an APERAK that is never owed.
#[tokio::test]
async fn a_processable_gas_message_carries_no_aperak_window() {
    let (store, d) = setup().await;
    let msg = fixture(edi_energy::MessageType::Utilmd, 44001);
    d.dispatch(&msg, "geli-gas-supplier-change", 44001, RECEIVED_AT)
        .await
        .expect("dispatch");
    let queued_aperak = store
        .pending_now(10)
        .await
        .expect("outbox")
        .iter()
        .any(|m| &*m.message_type == "APERAK");
    let got = windows(&store).await;
    if queued_aperak {
        // A Verarbeitbarkeitsfehler: the Initialprozess window watches it.
        assert_eq!(
            got,
            vec![(
                mako_fristen::APERAK_GAS_INITIALPROZESS_LABEL.to_owned(),
                mako_fristen::aperak_gas_initialprozess_due_at(RECEIVED_AT),
            )]
        );
    } else {
        assert!(got.is_empty(), "no APERAK is owed, so no window: {got:?}");
    }
}

/// The APERAKs a dispatch enqueued, as typed payloads.
async fn aperaks(store: &SlateDbStore) -> Vec<(String, mako_engine::payload::AperakPayload)> {
    store
        .pending_now(50)
        .await
        .expect("outbox")
        .into_iter()
        .filter(|m| &*m.message_type == "APERAK")
        .map(|m| {
            (
                m.recipient.to_string(),
                serde_json::from_value(m.payload).expect("an APERAK payload"),
            )
        })
        .collect()
}

/// A processable Strom message the workflow does not answer with an APERAK
/// itself gets exactly one Anerkennungsmeldung (APERAK AHB 1.1 § 2.4), naming
/// the message it acknowledges (`SG2 RFF+ACE`, `RFF+AGO`).
async fn one_anerkennung(msg: edi_energy::AnyMessage, pid: u32, workflow: &str) {
    use edi_energy::EdiEnergyMessage as _;
    let (store, d) = setup().await;
    let outcome = d
        .dispatch(&msg, workflow, pid, RECEIVED_AT)
        .await
        .expect("dispatch");
    assert!(
        matches!(
            outcome,
            IngestOutcome::Spawned { .. } | IngestOutcome::Dispatched { .. }
        ),
        "{outcome:?}"
    );
    let sent = aperaks(&store).await;
    assert_eq!(sent.len(), 1, "{workflow}: exactly one APERAK: {sent:?}");
    let (recipient, aperak) = &sent[0];
    assert_eq!(recipient, "4012345000023", "the APERAK answers the sender");
    assert_eq!(aperak.pid, mako_engine::payload::APERAK_PID_ANERKENNUNG);
    assert_eq!(aperak.sender, OWN);
    assert_eq!(aperak.orig_message_ref, msg.message_ref());
    assert_eq!(
        aperak.orig_document_number.as_deref(),
        Some(format!("DOK{pid}").as_str())
    );
    assert_eq!(aperak.orig_document_date.as_deref(), Some("202609302200"));
}

#[tokio::test]
async fn a_strom_mscons_is_acknowledged() {
    one_anerkennung(
        located(edi_energy::MessageType::Mscons, 13018),
        13018,
        "gpke-messwerte",
    )
    .await;
}

#[tokio::test]
async fn a_strom_invoic_is_acknowledged() {
    one_anerkennung(
        fixture(edi_energy::MessageType::Invoic, 31001),
        31001,
        "gpke-abrechnung",
    )
    .await;
}

/// A workflow that answers with its own APERAK keeps the only one.
#[tokio::test]
async fn a_workflows_own_anerkennung_is_the_only_one() {
    let (store, d) = setup().await;
    let msg = fixture(edi_energy::MessageType::Utilmd, 55001);
    d.dispatch(&msg, "gpke-supplier-change", 55001, RECEIVED_AT)
        .await
        .expect("dispatch");
    assert_eq!(aperaks(&store).await.len(), 1);
}

/// The WiM Gerätewechsel decides its APERAK on a later command, so its
/// receipt gets none from the door.
#[tokio::test]
async fn a_later_decided_aperak_is_not_preempted() {
    let (store, d) = setup().await;
    let msg = fixture(edi_energy::MessageType::Utilmd, 55042);
    d.dispatch(&msg, "wim-device-change", 55042, RECEIVED_AT)
        .await
        .expect("dispatch");
    let sent = aperaks(&store).await;
    assert!(
        sent.iter().all(|(_, a)| a.is_fehler()),
        "no Anerkennungsmeldung before the operator decides: {sent:?}"
    );
}

/// Gas has no Anerkennungsmeldung (APERAK AHB 1.1 § 2.3).
#[tokio::test]
async fn a_gas_message_gets_no_anerkennung() {
    let (store, d) = setup().await;
    let msg = located(edi_energy::MessageType::Mscons, 13008);
    d.dispatch(&msg, "geli-gas-mscons", 13008, RECEIVED_AT)
        .await
        .expect("dispatch");
    assert!(aperaks(&store).await.is_empty());
}

#[tokio::test]
async fn a_strom_pricat_is_acknowledged() {
    one_anerkennung(
        fixture(edi_energy::MessageType::Pricat, 27002),
        27002,
        "wim-preisliste",
    )
    .await;
}
