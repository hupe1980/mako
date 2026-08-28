//! Guards that the ESA module listens for the PIDs `makod` actually spawns from.
//!
//! Same failure class the LF module once had (see `pid_contract.rs`): a module
//! keyed on an *answer* PID never matches an event and silently never runs.
//! The ESA handshake makes that especially easy to get wrong, because the MSB
//! both receives and sends within one process — 17007 arrives, 19011 leaves,
//! and both are plausible `u32`s that appear in the same AHB chapter.
//!
//! The module only exists in a build carrying the MSB role; `role_separation.rs`
//! asserts its absence elsewhere.
#![cfg(feature = "role-msb")]

use processd::esa_module::{ESA_ANSWERED_PIDS, EsaOrderPayload};

/// The MSB's **own** answers. An event never carries one as `makopid`.
const ANSWER_PIDS: &[u32] = &[15_003, 19_011, 19_012, 19_013, 19_014];

#[test]
fn no_esa_trigger_is_an_answer_pid() {
    for pid in ESA_ANSWERED_PIDS {
        assert!(
            !ANSWER_PIDS.contains(pid),
            "{pid} is an answer the MSB sends, not an order it receives — a module \
             keyed on it can never fire"
        );
    }
}

/// The four inbound steps of WiM Teil 2 Kap. 4, and no others.
#[test]
fn the_esa_module_answers_exactly_the_inbound_kapitel_4_pids() {
    let mut got = ESA_ANSWERED_PIDS.to_vec();
    got.sort_unstable();
    assert_eq!(
        got,
        vec![17_007, 17_008, 35_003, 39_002],
        "35003 Werteanfrage, 17007 Bestellung, 17008 Abbestellung, 39002 Stornierung"
    );
}

/// Every PID the module claims carries a published Antwortfrist, or the queue
/// entry it creates has no deadline for an operator to work to.
#[test]
fn every_answered_pid_carries_a_published_frist() {
    let received = time::macros::datetime!(2026-03-02 8:00 UTC);
    for pid in ESA_ANSWERED_PIDS {
        let w = mako_fristen::antwort::operator_window(*pid, received);
        assert!(
            w.is_regulatory,
            "PID {pid} has no published Antwortfrist — its queue entry would carry none"
        );
        assert!(
            w.source.contains("Teil 2"),
            "PID {pid} must draw its window from WiM Teil 2, got {:?}",
            w.source
        );
    }
}

/// The module reacts to the PIDs it claims, and to nothing else.
#[test]
fn parse_accepts_the_claimed_pids_and_rejects_the_answers() {
    let event = |pid: u32| {
        serde_json::json!({
            "subject": "0195f1a0-0000-7000-8000-000000000001",
            "makopid": pid,
            "time": "2026-03-02T08:00:00Z",
            "data": { "malo_id": "51238696012", "abonnement": "Z01" },
        })
    };
    for pid in ESA_ANSWERED_PIDS {
        assert!(
            EsaOrderPayload::parse(&event(*pid)).is_some(),
            "the module must react to {pid}"
        );
    }
    for pid in ANSWER_PIDS {
        assert!(
            EsaOrderPayload::parse(&event(*pid)).is_none(),
            "the module must not react to its own answer {pid}"
        );
    }
}

/// …and something actually **emits** those PIDs.
///
/// The tests above check that the module listens on the right side of the
/// handshake. They cannot catch the failure that had actually happened: the
/// MSB-side workflow emitted **no `ProcessInitiated` at all**, so
/// `de.mako.process.initiated` was never published for any Kapitel-4 PID and
/// this entire module — the four `mako_pruefung` walks, the operator queue and
/// its Fristen — subscribed to an event that did not exist and never ran once.
///
/// A listener contract is only half a contract. This drives the real MSB
/// workflow and feeds its own notification straight into the parser, so the two
/// sides of the seam are pinned to each other rather than to a fixture.
#[test]
fn the_msb_workflow_emits_what_this_module_parses() {
    use mako_engine::types::{MarktpartnerCode, MessageRef, Pruefidentifikator};
    use mako_engine::workflow::Workflow as _;
    use mako_wim::wertebestellung::{
        ANFRAGE_PID, Lokationsebene, WertebestellungCommand as C, WimWertebestellungWorkflow as W,
        Zustellquittung,
    };

    let out = W::handle(
        &mako_wim::wertebestellung::WertebestellungState::default(),
        C::ReceiveAnfrage {
            pid: ANFRAGE_PID,
            esa: MarktpartnerCode::new("9900555000005"),
            msb: MarktpartnerCode::new("9900357000004"),
            ebene: Lokationsebene::Marktlokation,
            lokations_id: "51238696012".to_owned(),
            gegenstand: Box::new(mako_wim::esa::Bestellgegenstand {
                messprodukt: "9991000003056".to_owned(),
                wunschtermin: time::macros::date!(2026 - 03 - 01),
                zeitraum_bis: None,
                abonnement: mako_wim::esa::Abonnement::StartAbo,
                smgw: None,
            }),
            message_ref: MessageRef::new("REQ-1"),
            quittung: Zustellquittung::positive(time::macros::datetime!(2026-03-02 09:00 UTC)),
            consent_block: None,
        },
    )
    .expect("the Werteanfrage is accepted");

    let notification = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "ProcessInitiated")
        .expect("an inbound ESA step must notify, or this module never runs");

    // The shape the ERP outbox worker wraps it in: `makopid` as a CloudEvents
    // extension, the workflow payload as `data`.
    let pid = notification.payload["pid"].as_u64().expect("pid") as u32;
    assert_eq!(Pruefidentifikator::new(pid).unwrap(), ANFRAGE_PID);
    let event = serde_json::json!({
        "subject": "0195f1a0-0000-7000-8000-000000000001",
        "makopid": pid,
        "time": "2026-03-02T08:00:00Z",
        "data": notification.payload,
    });

    let parsed = EsaOrderPayload::parse(&event).expect("the module must parse its own trigger");
    assert_eq!(parsed.pid, pid);
    assert_eq!(parsed.lokations_id, "51238696012");
    assert_eq!(parsed.esa_mp_id, "9900555000005");
    assert_eq!(parsed.messprodukt, "9991000003056");
    // `IMD+7081` is what both termination trees branch on; a `None` here
    // escalates every order before any lookup.
    assert_eq!(parsed.abonnement, Some(mako_wim::esa::Abonnement::StartAbo));
}
