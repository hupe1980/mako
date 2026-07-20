//! WiM Strom Teil 2, Kapitel 4 — ESA Wertebestellung.
//!
//! Every Frist asserted here is quoted from the Festlegung text in the module
//! documentation of `mako_wim::wertebestellung`.

use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::Workflow,
};
use mako_wim::wertebestellung::{
    ANFRAGE_PID, ANGEBOT_WINDOW_LABEL, ANTWORT_WINDOW_LABEL, BESTELLUNG_PID, BINDUNGSFRIST_LABEL,
    Lokationsebene, STORNIERUNG_PID, WertebestellungCommand as C, WertebestellungEvent as E,
    WertebestellungState as S, WimWertebestellungWorkflow as W, Zustellquittung,
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

/// Monday 2026-03-02, 09:00 UTC.
fn quittung() -> Zustellquittung {
    Zustellquittung::positive(datetime!(2026-03-02 09:00 UTC))
}

fn anfrage() -> C {
    C::ReceiveAnfrage {
        pid: pid(ANFRAGE_PID),
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696780".to_owned(),
        message_ref: mref("REQ-1"),
        quittung: quittung(),
    }
}

/// Replay a command sequence, folding each event into the state.
fn drive(cmds: Vec<C>) -> Result<S, WorkflowError> {
    let mut state = S::default();
    for cmd in cmds {
        let out = W::handle(&state, cmd)?;
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    Ok(state)
}

fn angebot() -> C {
    C::SendAngebot {
        message_ref: mref("QUO-1"),
        // Bindungsfrist: two weeks out.
        bindungsfrist: datetime!(2026-03-16 17:00 UTC),
    }
}

fn bestellung() -> C {
    C::ReceiveBestellung {
        pid: pid(BESTELLUNG_PID),
        message_ref: mref("ORD-1"),
        quittung: Zustellquittung::positive(datetime!(2026-03-09 09:00 UTC)),
    }
}

fn accept_bestellung() -> C {
    C::AnswerBestellung {
        accept: true,
        message_ref: mref("RSP-1"),
        reason: None,
    }
}

// ── Happy path ────────────────────────────────────────────────────────────────

#[test]
fn full_ordering_handshake_reaches_authorised_delivery() {
    let state = drive(vec![
        anfrage(),
        angebot(),
        bestellung(),
        accept_bestellung(),
    ])
    .unwrap();
    assert_eq!(state.label(), "BestellungBestaetigt");
    assert!(
        state.lieferung_erlaubt(),
        "UC 4.2 Vorbedingung: the MSB may deliver only after accepting the Bestellung"
    );
    let data = state.data().expect("process data");
    assert_eq!(data.lokations_id, "51238696780");
    assert_eq!(data.ebene, Lokationsebene::Marktlokation);
}

#[test]
fn delivery_is_not_authorised_before_the_bestellung_is_accepted() {
    for cmds in [
        vec![anfrage()],
        vec![anfrage(), angebot()],
        vec![anfrage(), angebot(), bestellung()],
    ] {
        let state = drive(cmds).unwrap();
        assert!(
            !state.lieferung_erlaubt(),
            "{} must not authorise delivery",
            state.label()
        );
    }
}

// ── Fristen keyed on the ÜT ───────────────────────────────────────────────────

/// UC 4.1 Nr. 2: "spätester ÜT ist der 5. WT nach dem ÜT von Nr. 1".
/// Monday 2026-03-02 + 5 Werktage = Monday 2026-03-09.
#[test]
fn anfrage_starts_a_five_werktage_angebot_window_from_the_uet() {
    let out = W::handle(&S::default(), anfrage()).unwrap();
    let dl = out
        .deadlines
        .iter()
        .find(|d| d.label == ANGEBOT_WINDOW_LABEL)
        .expect("Angebot window registered");
    assert_eq!(dl.due_at.date(), time::macros::date!(2026 - 03 - 09));
}

/// UC 4.1 Nr. 4: "spätester ÜT ist der 2. WT nach dem ÜT von Nr. 3".
/// Monday 2026-03-09 + 2 Werktage = Wednesday 2026-03-11.
#[test]
fn bestellung_starts_a_two_werktage_answer_window_from_the_uet() {
    let mut state = S::default();
    for cmd in [anfrage(), angebot()] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    let out = W::handle(&state, bestellung()).unwrap();
    let dl = out
        .deadlines
        .iter()
        .find(|d| d.label == ANTWORT_WINDOW_LABEL)
        .expect("answer window registered");
    assert_eq!(dl.due_at.date(), time::macros::date!(2026 - 03 - 11));
}

/// GPKE Teil 1: the ÜT is usable "nur ... sofern es sich um eine positive
/// Zustellquittung bzw. Response-Nachricht handelt". A negative acknowledgement
/// must not start a Frist the market partner is not bound by.
#[test]
fn a_negative_zustellquittung_cannot_start_a_frist() {
    let cmd = C::ReceiveAnfrage {
        pid: pid(ANFRAGE_PID),
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696780".to_owned(),
        message_ref: mref("REQ-NEG"),
        quittung: Zustellquittung::negative(datetime!(2026-03-02 09:00 UTC)),
    };
    let err = W::handle(&S::default(), cmd).unwrap_err();
    assert!(
        err.to_string().contains("Zustellquittung"),
        "expected the negative-acknowledgement guard, got: {err}"
    );
}

/// UC 4.1 Nr. 3 bounds the Bestellung by the MSB's own Bindungsfrist rather than
/// by a fixed Werktage count.
#[test]
fn angebot_registers_the_bindungsfrist_as_the_ordering_deadline() {
    let mut state = S::default();
    let out = W::handle(&state, anfrage()).unwrap();
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    let out = W::handle(&state, angebot()).unwrap();
    let dl = out
        .deadlines
        .iter()
        .find(|d| d.label == BINDUNGSFRIST_LABEL)
        .expect("Bindungsfrist registered");
    assert_eq!(dl.due_at, datetime!(2026-03-16 17:00 UTC));
}

#[test]
fn a_bestellung_after_the_bindungsfrist_is_rejected() {
    let mut state = S::default();
    for cmd in [anfrage(), angebot()] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    let late = C::ReceiveBestellung {
        pid: pid(BESTELLUNG_PID),
        message_ref: mref("ORD-LATE"),
        quittung: Zustellquittung::positive(datetime!(2026-03-17 09:00 UTC)),
    };
    let err = W::handle(&state, late).unwrap_err();
    assert!(
        err.to_string().contains("Bindungsfrist"),
        "expected the Bindungsfrist guard, got: {err}"
    );
}

// ── Stornierung vs Abbestellung ───────────────────────────────────────────────

fn authorised() -> S {
    drive(vec![
        anfrage(),
        angebot(),
        bestellung(),
        accept_bestellung(),
    ])
    .unwrap()
}

/// UC 4.1 Nr. 5 admits a Stornierung only while delivery has not begun.
#[test]
fn stornierung_is_allowed_before_delivery_begins() {
    let state = authorised();
    let out = W::handle(
        &state,
        C::ReceiveStornierung {
            pid: pid(STORNIERUNG_PID),
            message_ref: mref("CHG-1"),
            quittung: quittung(),
        },
    )
    .unwrap();
    assert!(matches!(
        out.events.as_slice(),
        [E::StornierungEingegangen { .. }]
    ));
}

/// Once values have gone out the ESA must use the Abbestellung (UC 4.3) instead —
/// UC 4.3 Vorbedingung: "Eine Stornierung der Bestellung ist nicht mehr möglich".
#[test]
fn stornierung_is_refused_once_delivery_has_begun() {
    let mut state = authorised();
    let out = W::handle(&state, C::MarkLieferungBegonnen).unwrap();
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }

    let err = W::handle(
        &state,
        C::ReceiveStornierung {
            pid: pid(STORNIERUNG_PID),
            message_ref: mref("CHG-2"),
            quittung: quittung(),
        },
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("Abbestellung"),
        "the refusal must point at the Abbestellung route, got: {err}"
    );

    // ...and the Abbestellung itself is accepted in that state.
    let out = W::handle(
        &state,
        C::ReceiveAbbestellung {
            pid: pid(BESTELLUNG_PID),
            message_ref: mref("ORD-END"),
            beendigung_zum: datetime!(2026-04-01 00:00 UTC),
            quittung: quittung(),
        },
    )
    .unwrap();
    assert!(matches!(
        out.events.as_slice(),
        [E::AbbestellungEingegangen { .. }]
    ));
}

#[test]
fn marking_delivery_begun_is_idempotent() {
    let mut state = authorised();
    for _ in 0..2 {
        let out = W::handle(&state, C::MarkLieferungBegonnen).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    assert!(matches!(
        state,
        S::BestellungBestaetigt {
            lieferung_begonnen: true,
            ..
        }
    ));
    // A third call emits nothing.
    assert!(
        W::handle(&state, C::MarkLieferungBegonnen)
            .unwrap()
            .events
            .is_empty()
    );
}

/// A refused Stornierung leaves the Bestellung standing rather than ending it.
#[test]
fn refused_stornierung_restores_the_confirmed_bestellung() {
    let mut state = authorised();
    for cmd in [
        C::ReceiveStornierung {
            pid: pid(STORNIERUNG_PID),
            message_ref: mref("CHG-3"),
            quittung: quittung(),
        },
        C::AnswerStornierung {
            accept: false,
            message_ref: mref("RSP-STORNO"),
            reason: Some("Übermittlung bereits eingerichtet".to_owned()),
        },
    ] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    assert_eq!(state.label(), "BestellungBestaetigt");
    assert!(state.lieferung_erlaubt());
}

// ── Rejections must carry a reason ────────────────────────────────────────────

/// UC 4.1 Nr. 4: "informiert der MSB den ESA über die Gründe".
#[test]
fn rejecting_a_bestellung_requires_a_reason() {
    let mut state = S::default();
    for cmd in [anfrage(), angebot(), bestellung()] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    let err = W::handle(
        &state,
        C::AnswerBestellung {
            accept: false,
            message_ref: mref("RSP-NEG"),
            reason: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("Begründung"), "got: {err}");
}

// ── PID guards ────────────────────────────────────────────────────────────────

#[test]
fn each_step_rejects_a_foreign_pid() {
    // 21042 is IFTSTA "EnFG / Statusmeldung Privilegierungsinformation" and has
    // nothing to do with the ESA processes.
    let wrong = C::ReceiveAnfrage {
        pid: pid(21042),
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696780".to_owned(),
        message_ref: mref("REQ-X"),
        quittung: quittung(),
    };
    let err = W::handle(&S::default(), wrong).unwrap_err();
    assert!(err.to_string().contains("35002"), "got: {err}");
}

#[test]
fn an_anfrage_without_a_location_id_is_rejected() {
    let bad = C::ReceiveAnfrage {
        pid: pid(ANFRAGE_PID),
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Netzlokation,
        lokations_id: "  ".to_owned(),
        message_ref: mref("REQ-Y"),
        quittung: quittung(),
    };
    let err = W::handle(&S::default(), bad).unwrap_err();
    assert!(err.to_string().contains("Netzlokation"), "got: {err}");
}

// ── UC 4.4 — termination by the MSB ───────────────────────────────────────────

#[test]
fn msb_can_terminate_a_running_delivery() {
    let state = authorised();
    let out = W::handle(
        &state,
        C::BeendenDurchMsb {
            message_ref: mref("END-1"),
            beendigung_zum: datetime!(2026-05-01 00:00 UTC),
            reason: "Neuzuordnung der Messlokation zu einem anderen MSB".to_owned(),
        },
    )
    .unwrap();
    let mut state = state;
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    assert!(matches!(
        state,
        S::Beendet {
            durch_msb: true,
            ..
        }
    ));
    assert!(!state.lieferung_erlaubt());
}

#[test]
fn msb_cannot_terminate_a_delivery_that_was_never_authorised() {
    let err = W::handle(
        &S::default(),
        C::BeendenDurchMsb {
            message_ref: mref("END-2"),
            beendigung_zum: datetime!(2026-05-01 00:00 UTC),
            reason: "x".to_owned(),
        },
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("BestellungBestaetigt"),
        "got: {err}"
    );
}

// ── Fristversäumnis ───────────────────────────────────────────────────────────

#[test]
fn an_unanswered_window_records_a_fristversaeumnis() {
    let mut state = S::default();
    let out = W::handle(&state, anfrage()).unwrap();
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    let out = W::handle(
        &state,
        C::TimeoutExpired {
            deadline_id: mako_engine::ids::DeadlineId::new(),
            label: ANGEBOT_WINDOW_LABEL.into(),
        },
    )
    .unwrap();
    assert!(matches!(out.events.as_slice(), [E::FristVersaeumt { .. }]));
}

/// The Bindungsfrist lapsing without a Bestellung ends the offer; that is not a
/// Fristversäumnis by either party.
#[test]
fn a_lapsed_bindungsfrist_is_not_a_fristversaeumnis() {
    let mut state = S::default();
    for cmd in [anfrage(), angebot()] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    let out = W::handle(
        &state,
        C::TimeoutExpired {
            deadline_id: mako_engine::ids::DeadlineId::new(),
            label: BINDUNGSFRIST_LABEL.into(),
        },
    )
    .unwrap();
    assert!(out.events.is_empty());
}

// ── REQOTE classification ─────────────────────────────────────────────────────

use mako_wim::wertebestellung::{ReqoteKind, classify_reqote, has_messprodukt};

/// An ESA is a registered market role (PARTIN 37006), so the sender's role is
/// decisive on its own.
#[test]
fn a_reqote_from_an_esa_is_a_werteanfrage() {
    assert_eq!(
        classify_reqote(true, false),
        ReqoteKind::EsaWerteanfrage,
        "the sender's registered role alone must decide"
    );
}

/// A Werteanfrage names the Messprodukt it wants delivered; a Preisanfrage asks
/// for a price sheet and carries none.
#[test]
fn a_messprodukt_marks_a_werteanfrage_even_without_a_known_role() {
    assert_eq!(classify_reqote(false, true), ReqoteKind::EsaWerteanfrage);
}

/// Neither signal → Preisanfrage, preserving existing routing.
#[test]
fn an_unmarked_reqote_stays_a_preisanfrage() {
    assert_eq!(classify_reqote(false, false), ReqoteKind::Preisanfrage);
}

#[test]
fn messprodukt_detection_ignores_blank_product_codes() {
    assert!(!has_messprodukt(Vec::<&str>::new()));
    assert!(!has_messprodukt(vec!["", "   "]));
    assert!(has_messprodukt(vec!["", "Z01"]));
}

// ── Role-gated PID registration ───────────────────────────────────────────────

use mako_wim::wertebestellung::{ESA_INBOUND_PIDS, INBOUND_PIDS};

/// The MSB side and the ESA side must never claim the same PID, or an
/// integrated deployment holding both roles would hit the router's conflict
/// guard at build time.
#[test]
fn msb_and_esa_pid_sets_are_disjoint() {
    for pid in INBOUND_PIDS {
        assert!(
            !ESA_INBOUND_PIDS.contains(pid),
            "PID {pid} is claimed by both the MSB and the ESA side"
        );
    }
}

/// The ESA receives exactly the answers the MSB sends.
#[test]
fn esa_inbound_covers_every_msb_answer() {
    for pid in [19011_u32, 19012, 19013, 19014, 15003] {
        assert!(
            ESA_INBOUND_PIDS.contains(&pid),
            "ESA deployment must receive PID {pid}"
        );
    }
}
