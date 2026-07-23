//! WiM ESA Wertebestellung — ESA origination side.

use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::Workflow,
};
use mako_wim::esa_wertebestellung::{
    ABBESTELLUNG_PID, ANFRAGE_PID, ANGEBOT_WINDOW_LABEL, ANTWORT_WINDOW_LABEL, BESTAETIGUNG_PID,
    BESTELLUNG_PID, EsaWertebestellungCommand as C, EsaWertebestellungEvent,
    EsaWertebestellungState as S, EsaWertebestellungWorkflow as W, Lokationsebene, STORNIERUNG_PID,
    STORNO_BESTAETIGUNG_PID,
};
use time::macros::datetime;

fn pid(v: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(v).expect("valid PID")
}

fn mref(s: &str) -> MessageRef {
    MessageRef::new(s)
}

fn mp(s: &str) -> MarktpartnerCode {
    MarktpartnerCode::new(s)
}

fn werteanfrage() -> C {
    C::SendWerteanfrage {
        esa: mp("9905550000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696780".to_owned(),
        message_ref: mref("ESA-WA-1"),
    }
}

/// Fold a command's events into the state.
fn step(
    state: &S,
    cmd: C,
) -> Result<
    (
        S,
        mako_engine::workflow::WorkflowOutput<EsaWertebestellungEvent>,
    ),
    WorkflowError,
> {
    let out = W::handle(state, cmd)?;
    let mut next = state.clone();
    for ev in &out.events {
        next = W::apply(next.clone(), ev);
    }
    Ok((next, out))
}

/// Drive up to a confirmed, running delivery.
fn beliefert() -> S {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            message_ref: mref("QUO-1"),
            bindungsfrist: datetime!(2099-01-01 00:00 UTC),
        },
    )
    .unwrap();
    let (s, _) = step(
        &s,
        C::SendBestellung {
            message_ref: mref("ESA-BE-1"),
        },
    )
    .unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveBestaetigung {
            message_ref: mref("RSP-1"),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beliefert");
    s
}

#[test]
fn werteanfrage_emits_reqote_35002_and_arms_the_angebot_window() {
    let (state, out) = step(&S::default(), werteanfrage()).unwrap();
    assert_eq!(state.label(), "AnfrageGesendet");
    let ob = out.outbox.first().expect("REQOTE sent");
    assert_eq!(&*ob.message_type, "REQOTE");
    assert_eq!(ob.payload["pid"].as_u64(), Some(u64::from(ANFRAGE_PID)));
    assert_eq!(ob.payload["location"].as_str(), Some("51238696780"));
    assert!(
        out.deadlines
            .iter()
            .any(|d| d.label == ANGEBOT_WINDOW_LABEL),
        "the 5 WT Angebot window is armed"
    );
}

#[test]
fn full_handshake_reaches_running_delivery() {
    let s = beliefert();
    assert!(s.beliefert());
}

#[test]
fn bestellung_emits_orders_17007() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            message_ref: mref("QUO-1"),
            bindungsfrist: datetime!(2099-01-01 00:00 UTC),
        },
    )
    .unwrap();
    let (state, out) = step(
        &s,
        C::SendBestellung {
            message_ref: mref("ESA-BE-1"),
        },
    )
    .unwrap();
    assert_eq!(state.label(), "BestellungGesendet");
    let ob = out.outbox.first().expect("ORDERS sent");
    assert_eq!(&*ob.message_type, "ORDERS");
    assert_eq!(ob.payload["pid"].as_u64(), Some(u64::from(BESTELLUNG_PID)));
}

#[test]
fn ordering_after_the_bindungsfrist_is_refused() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    // A Bindungsfrist already in the past.
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            message_ref: mref("QUO-1"),
            bindungsfrist: datetime!(2000-01-01 00:00 UTC),
        },
    )
    .unwrap();
    let err = W::handle(
        &s,
        C::SendBestellung {
            message_ref: mref("ESA-BE-1"),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("Bindungsfrist"), "got: {err}");
}

#[test]
fn abbestellung_is_the_revocation_path_and_ends_delivery() {
    let s = beliefert();
    // A running delivery closes the Stornierung window.
    let (s, _) = step(&s, C::MarkLieferungBegonnen).unwrap();
    // Stornierung is refused after Lieferbeginn.
    assert!(
        W::handle(
            &s,
            C::SendStornierung {
                message_ref: mref("X")
            }
        )
        .is_err(),
        "Stornierung is not allowed once delivery has begun"
    );
    // The Abbestellung stops the running delivery.
    let (s, out) = step(
        &s,
        C::SendAbbestellung {
            message_ref: mref("ESA-AB-1"),
            beendigung_zum: datetime!(2026-04-01 00:00 UTC),
            grund: "einwilligung_widerrufen".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "AbbestellungGesendet");
    let ob = out.outbox.first().expect("ORDERS Abbestellung sent");
    assert_eq!(&*ob.message_type, "ORDERS");
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(ABBESTELLUNG_PID))
    );
    // The MSB confirms with ORDRSP 19011 → Beendet.
    let (s, _) = step(
        &s,
        C::ReceiveBestaetigung {
            message_ref: mref("RSP-AB"),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beendet");
}

#[test]
fn a_19011_confirms_the_bestellung_but_a_19011_after_abbestellung_ends_it() {
    // Same PID, resolved against the current state.
    let s = beliefert(); // reached via ReceiveBestaetigung in BestellungGesendet
    assert_eq!(s.label(), "Beliefert");
    let (s, _) = step(&s, C::MarkLieferungBegonnen).unwrap();
    let (s, _) = step(
        &s,
        C::SendAbbestellung {
            message_ref: mref("ESA-AB-1"),
            beendigung_zum: datetime!(2026-04-01 00:00 UTC),
            grund: "einwilligung_widerrufen".to_owned(),
        },
    )
    .unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveBestaetigung {
            message_ref: mref("R"),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beendet");
}

#[test]
fn a_rejected_bestellung_ends_the_process() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            message_ref: mref("QUO-1"),
            bindungsfrist: datetime!(2099-01-01 00:00 UTC),
        },
    )
    .unwrap();
    let (s, _) = step(
        &s,
        C::SendBestellung {
            message_ref: mref("ESA-BE-1"),
        },
    )
    .unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAblehnung {
            message_ref: mref("RSP-REJ"),
            reason: Some("Messprodukt nicht lieferbar".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Abgelehnt");
}

#[test]
fn stornierung_before_delivery_voids_the_order() {
    let s = beliefert();
    let (s, out) = step(
        &s,
        C::SendStornierung {
            message_ref: mref("ESA-ST-1"),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "StornierungGesendet");
    let ob = out.outbox.first().expect("ORDCHG sent");
    assert_eq!(&*ob.message_type, "ORDCHG");
    assert_eq!(ob.payload["pid"].as_u64(), Some(u64::from(STORNIERUNG_PID)));
    let (s, _) = step(
        &s,
        C::ReceiveStornierungAntwort {
            pid: pid(STORNO_BESTAETIGUNG_PID),
            message_ref: mref("RSP-ST"),
            reason: None,
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Storniert");
}

#[test]
fn a_missed_angebot_window_rejects_the_anfrage() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::TimeoutExpired {
            deadline_id: mako_engine::ids::DeadlineId::new(),
            label: ANGEBOT_WINDOW_LABEL.into(),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Abgelehnt");
    let _ = ANTWORT_WINDOW_LABEL;
    let _ = BESTAETIGUNG_PID;
}

/// A QUOTES 15003 with no Bindungsfrist is an Ablehnung der Anfrage → the
/// process ends in Abgelehnt (distinct from an Angebot).
#[test]
fn an_anfrage_ablehnung_ends_the_process() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAnfrageAblehnung {
            reason: Some("Messprodukt nicht lieferbar".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Abgelehnt");
}

/// The ORDCHG Stornierung must reference the original Bestellung's Belegnummer
/// so the MSB can correlate a message that carries no LOC.
#[test]
fn stornierung_references_the_original_bestellung_belegnummer() {
    let s = beliefert();
    let (s, out) = step(
        &s,
        C::SendStornierung {
            message_ref: mref("ESA-ST-9"),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "StornierungGesendet");
    let ob = out.outbox.first().expect("ORDCHG sent");
    assert_eq!(&*ob.message_type, "ORDCHG");
    // RFF+ON of the ORDCHG echoes the Bestellung (ESA-BE-1 from `beliefert`).
    assert_eq!(
        ob.payload["order_reference"].as_str(),
        Some("ESA-BE-1"),
        "ORDCHG must reference the original Bestellung Belegnummer"
    );
    let _ = STORNIERUNG_PID;
}
