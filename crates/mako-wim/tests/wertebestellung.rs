//! WiM Strom Teil 2, Kapitel 4 — ESA Wertebestellung.
//!
//! Every Frist asserted here is quoted from the Festlegung text in the module
//! documentation of `mako_wim::wertebestellung`.

use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::Workflow,
};
/// The Pflicht-Messprodukt for a Marktlokation Lastgang (Codeliste der
/// Konfigurationen 1.4 §4.6.1, `9991 00000 305 6`), ordered as a subscription.
fn gegenstand() -> Box<mako_wim::esa::Bestellgegenstand> {
    Box::new(mako_wim::esa::Bestellgegenstand {
        messprodukt: "9991000003056".to_owned(),
        wunschtermin: time::macros::date!(2026 - 03 - 01),
        zeitraum_bis: None,
        abonnement: mako_wim::esa::Abonnement::StartAbo,
        smgw: None,
    })
}

use mako_wim::wertebestellung::{
    ABBESTELLUNG_PID, ABLEHNUNG_PID, ANFRAGE_PID, ANGEBOT_PID, ANGEBOT_WINDOW_LABEL,
    ANTWORT_WINDOW_LABEL, BESTAETIGUNG_PID, BESTELLUNG_PID, BINDUNGSFRIST_LABEL, Lokationsebene,
    STORNIERUNG_PID, WertebestellungCommand as C, WertebestellungEvent as E,
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
        gegenstand: gegenstand(),
        pid: ANFRAGE_PID,
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696012".to_owned(),
        message_ref: mref("REQ-1"),
        quittung: quittung(),
        consent_block: None,
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
        angebot: angebot_terms(),
        fruehester_start: None,
        message_ref: mref("QUO-1"),
        // Bindungsfrist: two weeks out.
        bindungsfrist: datetime!(2026-03-16 17:00 UTC),
    }
}

fn bestellung() -> C {
    C::ReceiveBestellung {
        abonnement: mako_wim::esa::Abonnement::StartAbo,
        pid: BESTELLUNG_PID,
        message_ref: mref("ORD-1"),
        quittung: Zustellquittung::positive(datetime!(2026-03-09 09:00 UTC)),
        consent_block: None,
    }
}

/// The Muss content of a QUOTES 15003 Angebot: `SG4 CUX`, an Artikel-ID with
/// its `SG31 PRI+CAL` price, and the OBIS-Kennzahlen the subscription delivers.
/// A 15003 without them is not an offer — it is the Ablehnung.
fn angebot_terms() -> Box<mako_wim::esa::Angebot> {
    Box::new(mako_wim::esa::Angebot {
        waehrung: Some("EUR".to_owned()),
        preise: vec![mako_wim::esa::Preisposition {
            artikel_id: "9990001100002".to_owned(),
            preistyp: mako_wim::esa::Preistyp::Betrieb,
            betrag: "0.004500".to_owned(),
            einheit: "DAY".to_owned(),
        }],
        obis_kennzahlen: vec!["1-1:1.29.0".to_owned()],
        einrichtung_bis: None,
    })
}

fn accept_bestellung() -> C {
    C::AnswerBestellung {
        // `E_0256` A11 — „Bestellung ist angenommen“. The code's Cluster is
        // what puts the answer on 19011; there is no separate accept flag.
        antwort_code: "A11".to_owned(),
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
    assert_eq!(data.lokations_id, "51238696012");
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
        gegenstand: gegenstand(),
        pid: ANFRAGE_PID,
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696012".to_owned(),
        message_ref: mref("REQ-NEG"),
        quittung: Zustellquittung::negative(datetime!(2026-03-02 09:00 UTC)),
        consent_block: None,
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
        abonnement: mako_wim::esa::Abonnement::StartAbo,
        pid: BESTELLUNG_PID,
        message_ref: mref("ORD-LATE"),
        quittung: Zustellquittung::positive(datetime!(2026-03-17 09:00 UTC)),
        consent_block: None,
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
            pid: STORNIERUNG_PID,
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
            pid: STORNIERUNG_PID,
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
            pid: ABBESTELLUNG_PID,
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
            pid: STORNIERUNG_PID,
            message_ref: mref("CHG-3"),
            quittung: quittung(),
        },
        C::AnswerStornierung {
            // `E_0257` A02 — the Abo has already started delivering.
            antwort_code: "A02".to_owned(),
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
fn an_antwortcode_outside_the_tree_is_refused() {
    let mut state = S::default();
    for cmd in [anfrage(), angebot(), bestellung()] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    // `A02` is published by `E_0257` (Stornierung), not by `E_0256`. Answering
    // a Bestellung with it would state a reason the tree does not define.
    let err = W::handle(
        &state,
        C::AnswerBestellung {
            antwort_code: "A02".to_owned(),
            message_ref: mref("RSP-NEG"),
            reason: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("E_0256"), "got: {err}");

    // A code published by the tree carries its own Bedeutung as the Grund —
    // UC 4.1 Nr. 4's „informiert der MSB den ESA über die Gründe" is satisfied
    // by the Antwortcode, not by free text the MSB has to invent.
    let out = W::handle(
        &state,
        C::AnswerBestellung {
            antwort_code: "A09".to_owned(),
            message_ref: mref("RSP-NEG"),
            reason: None,
        },
    )
    .unwrap();
    let ob = &out.outbox[0];
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(ABLEHNUNG_PID.as_u32())),
        "the code's Cluster puts the answer on the Ablehnungs-PID"
    );
    assert_eq!(ob.payload["antwort_codeliste"].as_str(), Some("E_0256"));
}

/// `E_0254` publishes four refusals, so a Beendigung is not always
/// confirmable — and a refused one leaves the delivery running rather than
/// silently ending it.
#[test]
fn a_refused_abbestellung_leaves_the_delivery_running() {
    let mut state = S::default();
    for cmd in [
        anfrage(),
        angebot(),
        bestellung(),
        accept_bestellung(),
        C::ReceiveAbbestellung {
            pid: ABBESTELLUNG_PID,
            message_ref: mref("ORD-AB"),
            beendigung_zum: datetime!(2026-04-01 00:00 UTC),
            quittung: quittung(),
        },
    ] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    assert_eq!(state.label(), "AbbestellungEingegangen");
    // `A01` — die Bestellung war eine einmalige Übermittlung, sie ist zu
    // stornieren.
    let out = W::handle(
        &state,
        C::AnswerAbbestellung {
            antwort_code: "A01".to_owned(),
            message_ref: mref("RSP-AB"),
            reason: None,
        },
    )
    .unwrap();
    assert_eq!(
        out.outbox[0].payload["pid"].as_u64(),
        Some(u64::from(ABLEHNUNG_PID.as_u32()))
    );
    assert_eq!(
        out.outbox[0].payload["antwort_codeliste"].as_str(),
        Some("E_0254")
    );
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    assert_eq!(
        state.label(),
        "BestellungBestaetigt",
        "a refused Beendigung must not end the delivery"
    );
}

// ── PID guards ────────────────────────────────────────────────────────────────

#[test]
fn each_step_rejects_a_foreign_pid() {
    // 55001 is a GPKE UTILMD Lieferbeginn — foreign to the ESA Wertebestellung
    // Anfrage step, which only accepts REQOTE 35002.
    let wrong = C::ReceiveAnfrage {
        gegenstand: gegenstand(),
        pid: pid(55001),
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696012".to_owned(),
        message_ref: mref("REQ-X"),
        quittung: quittung(),
        consent_block: None,
    };
    let err = W::handle(&S::default(), wrong).unwrap_err();
    assert!(
        err.to_string().contains(&ANFRAGE_PID.as_u32().to_string()),
        "the rejection must name the expected PID: {err}"
    );
}

#[test]
fn an_anfrage_without_a_location_id_is_rejected() {
    let bad = C::ReceiveAnfrage {
        gegenstand: gegenstand(),
        pid: ANFRAGE_PID,
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Netzlokation,
        lokations_id: "  ".to_owned(),
        message_ref: mref("REQ-Y"),
        quittung: quittung(),
        consent_block: None,
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
    // UC 4.4: the MSB notifies the ESA on the wire via IFTSTA 21042
    // (WiM Umsetzungsstatus, STS 4405 = 105 „beendet").
    let ob = out
        .outbox
        .first()
        .expect("IFTSTA 21042 Beendigung emitted to the ESA");
    assert_eq!(&*ob.message_type, "IFTSTA");
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(
            mako_wim::wertebestellung::BEENDIGUNG_MSB_PID.as_u32()
        ))
    );
    assert_eq!(ob.payload["sts_code"].as_str(), Some("105"));
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

// ── PID identity ──────────────────────────────────────────────────────────────

/// The ESA Werteanfrage is REQOTE **35003**, not 35002.
///
/// REQOTE AHB 1.1 §4.3 gives the Kommunikation as *ESA an MSB* and labels the
/// `SG1 RFF+Z13` text "35003 Anfrage von Werten für ESA"; the PID overview 4.0
/// lists 35003 under WiM Strom Teil 2 and nowhere else.
///
/// 35002 is §4.2 "Anfrage zur Rechnungsabwicklung des Messstellenbetriebs über
/// den LF" (LF → MSB, WiM Teil 1) — a different process with a different sender
/// role. Sending it here would also collide with the Preisanfrage stream, which
/// only a sender-role classifier could then resolve; with 35003 there is nothing
/// to resolve.
#[test]
fn the_esa_werteanfrage_is_reqote_35003() {
    assert_eq!(
        mako_wim::wertebestellung::ANFRAGE_PID.as_u32(),
        35_003,
        "35002 is Rechnungsabwicklung MSB über LF (LF → MSB), a different process"
    );
    // The Angebot that answers it is ESA-specific too.
    assert_eq!(mako_wim::wertebestellung::ANGEBOT_PID.as_u32(), 15_003);
}

/// The Preisanfrage REQOTE set and the ESA Anfrage must stay disjoint, or both
/// workflows would claim the same inbound message.
#[test]
fn the_preisanfrage_reqote_set_excludes_the_esa_anfrage() {
    let anfrage = mako_wim::wertebestellung::ANFRAGE_PID.as_u32();
    assert!(
        !mako_wim::preisanfrage::REQOTE_PIDS.contains(&anfrage),
        "35003 belongs to wertebestellung, not the Preisanfrage stream"
    );
    assert!(
        !mako_wim::preisanfrage::QUOTES_PIDS
            .contains(&mako_wim::wertebestellung::ANGEBOT_PID.as_u32()),
        "15003 answers 35003 and belongs to wertebestellung"
    );
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
            ESA_INBOUND_PIDS.iter().any(|p| p.as_u32() == pid),
            "ESA deployment must receive PID {pid}"
        );
    }
}

// ── Outbound leg — the MSB answers the ESA on the wire ────────────────────────

#[test]
fn send_angebot_enqueues_quotes_15003_to_the_esa() {
    let mut state = S::default();
    let out = W::handle(&state, anfrage()).unwrap();
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    let out = W::handle(&state, angebot()).unwrap();
    assert_eq!(out.outbox.len(), 1, "an Angebot must be sent on the wire");
    let ob = &out.outbox[0];
    assert_eq!(ob.message_type.as_ref(), "QUOTES");
    assert_eq!(ob.recipient.as_ref(), "9900555000005"); // the ESA
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(ANGEBOT_PID.as_u32()))
    );
    assert_eq!(ob.payload["sender"].as_str(), Some("9900357000004")); // the MSB
}

#[test]
fn answer_bestellung_enqueues_ordrsp_confirm_or_reject() {
    let confirm = drive_to_outbox(
        vec![anfrage(), angebot(), bestellung()],
        accept_bestellung(),
    );
    assert_eq!(confirm.message_type.as_ref(), "ORDRSP");
    assert_eq!(
        confirm.payload["pid"].as_u64(),
        Some(u64::from(BESTAETIGUNG_PID.as_u32()))
    );
    // The ORDRSP carries no LOC — it echoes the Bestellung's Belegnummer
    // (`ORD-1`) in `RFF+ON`, the answer's published Zuordnungsschlüssel
    // `ZG-T14`. (`ACW` is the Storno-Antwort's key, not this one.)
    assert_eq!(
        confirm.payload["korrelation_ref"].as_str(),
        Some("ORD-1"),
        "ORDRSP must echo the answered Bestellung Belegnummer"
    );
    // `IMD+7081` and the EBD the Prüfschritt code belongs to are Muss on every
    // ESA answer (ORDRSP AHB 1.1b §4.15, conditions [17]/[21]-[23]).
    assert_eq!(confirm.payload["abonnement"].as_str(), Some("Z01"));
    assert_eq!(
        confirm.payload["antwort_codeliste"].as_str(),
        Some("E_0256")
    );
    assert_eq!(confirm.payload["antwort_code"].as_str(), Some("A11"));
    assert!(
        confirm.payload["location"].is_null(),
        "an ORDRSP carries no LOC"
    );

    let reject = drive_to_outbox(
        vec![anfrage(), angebot(), bestellung()],
        C::AnswerBestellung {
            // `E_0256` A09 — die Gerätetechnik misst die Werte nicht.
            antwort_code: "A09".to_owned(),
            message_ref: mref("RSP-2"),
            reason: Some("Messprodukt nicht lieferbar".to_owned()),
        },
    );
    assert_eq!(
        reject.payload["pid"].as_u64(),
        Some(u64::from(ABLEHNUNG_PID.as_u32()))
    );
}

/// A revoked consent (gated at ingest) turns the Werteanfrage straight into a
/// QUOTES 15003 Ablehnung — the process ends in `Abgelehnt`, no Angebot window.
#[test]
fn a_blocked_consent_rejects_the_anfrage_with_a_quotes_ablehnung() {
    let blocked = C::ReceiveAnfrage {
        gegenstand: gegenstand(),
        pid: ANFRAGE_PID,
        esa: mp("9900555000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696012".to_owned(),
        message_ref: mref("REQ-BLOCKED"),
        quittung: quittung(),
        consent_block: Some("Einwilligung wurde widerrufen".to_owned()),
    };
    let out = W::handle(&S::default(), blocked).unwrap();
    // No Angebot deadline is armed — the process is done.
    assert!(out.deadlines.is_empty(), "a blocked Anfrage arms no window");
    let ob = out.outbox.into_iter().next().expect("Ablehnung sent");
    assert_eq!(&*ob.message_type, "QUOTES");
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(ANGEBOT_PID.as_u32()))
    );
    // Folding the event lands the process in Abgelehnt.
    let state = W::apply(S::default(), &out.events[0]);
    assert_eq!(state.label(), "Abgelehnt");
}

/// Consent can be revoked between the Angebot and the Bestellung — a blocked
/// order is answered with an ORDRSP 19012 Ablehnung.
#[test]
fn a_blocked_consent_rejects_the_bestellung_with_an_ordrsp_ablehnung() {
    let mut state = S::default();
    for cmd in [anfrage(), angebot()] {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    let blocked = C::ReceiveBestellung {
        abonnement: mako_wim::esa::Abonnement::StartAbo,
        pid: BESTELLUNG_PID,
        message_ref: mref("ORD-BLOCKED"),
        quittung: Zustellquittung::positive(datetime!(2026-03-09 09:00 UTC)),
        consent_block: Some("Einwilligung wurde widerrufen".to_owned()),
    };
    let out = W::handle(&state, blocked).unwrap();
    let ob = out.outbox.into_iter().next().expect("Ablehnung sent");
    assert_eq!(&*ob.message_type, "ORDRSP");
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(ABLEHNUNG_PID.as_u32()))
    );
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    assert_eq!(state.label(), "Abgelehnt");
}

/// Drive `cmds` to build state, then run `final_cmd` and return its single outbox.
fn drive_to_outbox(cmds: Vec<C>, final_cmd: C) -> mako_engine::outbox::PendingOutbox {
    let mut state = S::default();
    for cmd in cmds {
        let out = W::handle(&state, cmd).unwrap();
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }
    let out = W::handle(&state, final_cmd).unwrap();
    out.outbox
        .into_iter()
        .next()
        .expect("an answer must be sent")
}

// ── UC 4.2 — Typ-2 value delivery (outbound MSCONS 13027, MSB → ESA) ───────────

use mako_wim::wertebestellung::WERTE_UEBERMITTLUNG_PID;

fn typ2_reads() -> serde_json::Value {
    serde_json::json!([
        { "dtm_from": "2026-03-10T00:00:00Z", "dtm_to": "2026-03-10T00:15:00Z",
          "quantity_kwh": "0.250", "obis_code": "1-0:1.29.0" },
        { "dtm_from": "2026-03-10T00:15:00Z", "dtm_to": "2026-03-10T00:30:00Z",
          "quantity_kwh": "0.310", "obis_code": "1-0:1.29.0" }
    ])
}

/// A confirmed Bestellung authorises delivery: `LiefereWerte` emits an outbound
/// MSCONS 13027 addressed to the ESA and records the transmission.
#[test]
fn liefere_werte_emits_mscons_13027_addressed_to_the_esa() {
    let state = authorised();
    let out = W::handle(
        &state,
        C::LiefereWerte {
            message_ref: mref("WERTE-1"),
            reads: typ2_reads(),
        },
    )
    .unwrap();
    // The wire message is MSCONS 13027, recipient = the ESA.
    let ob = out.outbox.first().expect("MSCONS delivery sent");
    assert_eq!(&*ob.message_type, "MSCONS");
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(WERTE_UEBERMITTLUNG_PID.as_u32()))
    );
    assert_eq!(ob.payload["receiver_mp_id"].as_str(), Some("9900555000005"));
    assert_eq!(ob.recipient.as_ref(), "9900555000005");
    assert_eq!(ob.payload["reads"].as_array().map(Vec::len), Some(2));
    // An auditable transmission event is recorded, and delivery has begun.
    assert!(matches!(
        out.events.as_slice(),
        [E::WerteUebermittelt {
            interval_count: 2,
            ..
        }]
    ));
    let mut s = state.clone();
    for ev in &out.events {
        s = W::apply(s.clone(), ev);
    }
    assert!(
        !s.lieferung_erlaubt() || matches!(s.label(), "BestellungBestaetigt"),
        "state stays authorised"
    );
}

/// Delivery is refused before a Bestellung is confirmed — the §60 Abs. 1 gate.
#[test]
fn delivery_without_a_confirmed_bestellung_is_refused() {
    // Only the Anfrage received; no Angebot, no Bestellung.
    let mut state = S::default();
    let out = W::handle(&state, anfrage()).unwrap();
    for ev in &out.events {
        state = W::apply(state.clone(), ev);
    }
    let err = W::handle(
        &state,
        C::LiefereWerte {
            message_ref: mref("WERTE-X"),
            reads: typ2_reads(),
        },
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("BestellungBestaetigt"),
        "delivery must gate on a confirmed Bestellung, got: {err}"
    );
}

/// A delivery with no interval values is rejected.
#[test]
fn a_delivery_with_no_intervals_is_rejected() {
    let state = authorised();
    let err = W::handle(
        &state,
        C::LiefereWerte {
            message_ref: mref("WERTE-EMPTY"),
            reads: serde_json::json!([]),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("Intervallwerte"), "got: {err}");
}

// ── The notification contract with processd ───────────────────────────────────

/// Every inbound step of Kapitel 4 obliges the MSB to answer — 35003 within
/// 5 Werktage, 17007/17008/39002 within 2 — and §34 Abs. 2 S. 2 Nr. 10 MsbG
/// makes serving an ESA a mandatory Zusatzleistung, so none of them is
/// optional. This workflow emitted no `ProcessInitiated` at all, so
/// `processd`'s ESA module — the three `mako-pruefung` walks, the operator
/// queue and its Fristen — subscribed to an event that was never published and
/// never ran once.
#[test]
fn every_inbound_step_notifies_its_observers() {
    let mut state = S::default();
    let mut seen = Vec::new();

    for cmd in [anfrage(), angebot(), bestellung()] {
        let inbound = matches!(cmd, C::ReceiveAnfrage { .. } | C::ReceiveBestellung { .. });
        let out = W::handle(&state, cmd).expect("step accepted");
        if inbound {
            let n = out
                .outbox
                .iter()
                .find(|o| &*o.message_type == "ProcessInitiated")
                .expect("an inbound ESA step must notify");
            seen.push(n.payload.clone());
        }
        for ev in &out.events {
            state = W::apply(state.clone(), ev);
        }
    }

    assert_eq!(seen.len(), 2, "35003 and 17007 both notify");
    // The payload is the contract `processd::esa_module::EsaOrderPayload`
    // parses; a field it cannot find does not fail, it escalates.
    for p in &seen {
        assert!(p["pid"].as_u64().is_some());
        assert!(p["malo_id"].as_str().is_some());
        assert!(p["esa_mp_id"].as_str().is_some());
        assert!(p["messprodukt"].as_str().is_some());
        assert!(p["abonnement"].as_str().is_some());
    }
    // `E_0256` Prüfschritt 1 asks about the Bindungsfrist at Bestellung time,
    // one state after the Angebot stated it.
    assert!(
        seen[1]["bindungsfrist"].as_str().is_some(),
        "the Bestellung notification carries the offer window: {:?}",
        seen[1]
    );
}

/// `E_0254` Prüfschritt 2 compares the requested end against the Abo start, and
/// Prüfschritt 4 against the values already delivered. Both live here, not in
/// the message: the Abbestellung's own `DTM+203` *is* the requested end, so
/// deriving the start from it made Prüfschritt 2 false by construction and
/// refused every Abbestellung — the GDPR-Art.-7(3) Widerruf included.
#[test]
fn the_abbestellung_notification_carries_the_dates_e0254_needs() {
    let state = drive(vec![
        anfrage(),
        angebot(),
        bestellung(),
        accept_bestellung(),
    ])
    .expect("handshake");
    let out = W::handle(
        &state,
        C::ReceiveAbbestellung {
            pid: ABBESTELLUNG_PID,
            message_ref: mref("ESA-AB-1"),
            beendigung_zum: datetime!(2026-06-01 00:00 UTC),
            quittung: quittung(),
        },
    )
    .expect("Abbestellung accepted");

    let n = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "ProcessInitiated")
        .expect("the Abbestellung notifies");
    assert_eq!(n.payload["abonnement"].as_str(), Some("Z02"));
    assert_eq!(n.payload["ausfuehrungsdatum"].as_str(), Some("2026-06-01"));
    let abo_beginn = n.payload["abo_beginn"]
        .as_str()
        .expect("the Abo start is carried");
    assert_ne!(
        abo_beginn, "2026-06-01",
        "the Abo start must not be the requested end"
    );
}
