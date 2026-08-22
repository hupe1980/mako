//! WiM ESA Wertebestellung — ESA origination side.

use mako_engine::{
    error::WorkflowError,
    types::{MarktpartnerCode, MessageRef},
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

use mako_wim::esa_wertebestellung::{
    ABBESTELLUNG_PID, ANFRAGE_PID, ANGEBOT_WINDOW_LABEL, ANTWORT_WINDOW_LABEL, BEENDIGUNG_MSB_PID,
    BESTAETIGUNG_PID, BESTELLUNG_PID, EsaWertebestellungCommand as C, EsaWertebestellungEvent,
    EsaWertebestellungState as S, EsaWertebestellungWorkflow as W, Lokationsebene, STORNIERUNG_PID,
    STORNO_BESTAETIGUNG_PID,
};
use time::macros::datetime;

fn mref(s: &str) -> MessageRef {
    MessageRef::new(s)
}

fn mp(s: &str) -> MarktpartnerCode {
    MarktpartnerCode::new(s)
}

fn werteanfrage() -> C {
    C::SendWerteanfrage {
        gegenstand: gegenstand(),
        esa: mp("9905550000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696012".to_owned(),
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
            fruehester_start: None,
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
fn werteanfrage_emits_reqote_35003_and_arms_the_angebot_window() {
    let (state, out) = step(&S::default(), werteanfrage()).unwrap();
    assert_eq!(state.label(), "AnfrageGesendet");
    let ob = out.outbox.first().expect("REQOTE sent");
    assert_eq!(&*ob.message_type, "REQOTE");
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(ANFRAGE_PID.as_u32()))
    );
    assert_eq!(ob.payload["location"].as_str(), Some("51238696012"));
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
            fruehester_start: None,
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
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(BESTELLUNG_PID.as_u32()))
    );
}

#[test]
fn ordering_after_the_bindungsfrist_is_refused() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    // A Bindungsfrist already in the past.
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            fruehester_start: None,
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
        Some(u64::from(ABBESTELLUNG_PID.as_u32()))
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
            fruehester_start: None,
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
    assert_eq!(
        ob.payload["pid"].as_u64(),
        Some(u64::from(STORNIERUNG_PID.as_u32()))
    );
    let (s, _) = step(
        &s,
        C::ReceiveStornierungAntwort {
            pid: STORNO_BESTAETIGUNG_PID,
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
/// so the MSB can correlate a message that carries no LOC (`ZG-T51`).
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
        ob.payload["korrelation_ref"].as_str(),
        Some("ESA-BE-1"),
        "ORDCHG must reference the original Bestellung Belegnummer"
    );
    assert!(
        ob.payload["location"].is_null(),
        "a conformant ORDCHG carries no LOC (ORDCHG AHB 1.1 §3.2)"
    );
    let _ = STORNIERUNG_PID;
}

// ── Correlation chain (Anwendungsübersicht der Prüfidentifikatoren 4.0) ───────

/// `ZG-T24`: the ORDERS 17007 echoes the QUOTES Angebot's Belegnummer in
/// `RFF+AAG`, and carries **no** LOC — ORDERS AHB 1.1b §4.15 lists no
/// Meldepunkt segment for the ESA order PIDs.
#[test]
fn bestellung_references_the_angebot_and_carries_no_location() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            fruehester_start: None,
            message_ref: mref("QUO-77"),
            bindungsfrist: datetime!(2099-01-01 00:00 UTC),
        },
    )
    .unwrap();
    let (_, out) = step(
        &s,
        C::SendBestellung {
            message_ref: mref("ESA-BE-1"),
        },
    )
    .unwrap();
    let ob = out.outbox.first().expect("ORDERS sent");
    assert_eq!(ob.payload["korrelation_ref"].as_str(), Some("QUO-77"));
    assert!(ob.payload["location"].is_null());
    // `IMD+7081` — the order is a subscription start.
    assert_eq!(ob.payload["abonnement"].as_str(), Some("Z01"));
}

/// `ZG-T41`: the ORDERS 17008 Abbestellung echoes the 17007's Belegnummer in
/// `RFF+ACW` and carries `IMD++Z02` (Ende Abo), which is what selects EBD
/// `E_0254` for the MSB's answer.
#[test]
fn abbestellung_references_the_bestellung_and_ends_the_abo() {
    let s = beliefert();
    let (_, out) = step(
        &s,
        C::SendAbbestellung {
            message_ref: mref("ESA-AB-1"),
            beendigung_zum: datetime!(2026-04-01 00:00 UTC),
            grund: "einwilligung_widerrufen".to_owned(),
        },
    )
    .unwrap();
    let ob = out.outbox.first().expect("ORDERS sent");
    assert_eq!(ob.payload["korrelation_ref"].as_str(), Some("ESA-BE-1"));
    assert_eq!(ob.payload["abonnement"].as_str(), Some("Z02"));
    assert!(ob.payload["location"].is_null());
}

/// The Werteanfrage puts the ordered Messprodukt and the Wunschtermin on the
/// wire — without them the MSB can neither price nor schedule.
#[test]
fn werteanfrage_carries_the_messprodukt_and_wunschtermin() {
    let (_, out) = step(&S::default(), werteanfrage()).unwrap();
    let ob = out.outbox.first().expect("REQOTE sent");
    assert_eq!(ob.payload["messprodukt"].as_str(), Some("9991000003056"));
    assert_eq!(ob.payload["wunschtermin"].as_str(), Some("2026-03-01"));
    assert_eq!(ob.payload["abonnement"].as_str(), Some("Z01"));
}

/// A product outside Codeliste der Konfigurationen Kapitel 4.6 is refused
/// before anything reaches the wire — the ESA role may order nothing else.
#[test]
fn a_messprodukt_outside_kapitel_46_is_refused() {
    let mut cmd = werteanfrage();
    if let C::SendWerteanfrage { gegenstand, .. } = &mut cmd {
        gegenstand.messprodukt = "9992000000011".to_owned();
    }
    let err = step(&S::default(), cmd).unwrap_err();
    assert!(
        format!("{err:?}").contains("Kapitel 4.6"),
        "unexpected error: {err:?}"
    );
}

/// `9991 00000 305 6` is a Marktlokation product. Asking for it with a
/// Zählpunktbezeichnung is the „nicht zugeordnet“ Fehlerfall of UC 4.1.1.
#[test]
fn a_messprodukt_ordered_at_the_wrong_level_is_refused() {
    let mut cmd = werteanfrage();
    if let C::SendWerteanfrage {
        ebene,
        lokations_id,
        ..
    } = &mut cmd
    {
        *ebene = mako_wim::esa::Lokationsebene::Messlokation;
        *lokations_id = "DE0001234567890123456789012345678".to_owned();
    }
    let err = step(&S::default(), cmd).unwrap_err();
    assert!(
        format!("{err:?}").contains("Ebene"),
        "unexpected error: {err:?}"
    );
}

// ── UC 4.4 — MSB-initiated Beendigung (IFTSTA 21042) ──────────────────────────

#[test]
fn msb_beendigung_ends_the_delivery_on_the_esa_side() {
    // A running delivery is ended when the ESA receives IFTSTA 21042.
    let s = beliefert();
    let (s, out) = step(
        &s,
        C::ReceiveBeendigungDurchMsb {
            message_ref: mref("IFT-21042-1"),
            beendigung_zum: datetime!(2026-08-01 00:00 UTC),
            reason: Some("Messstellenbetrieb endet".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beendet", "UC 4.4 terminates the process");
    assert!(
        matches!(
            out.events.first(),
            Some(EsaWertebestellungEvent::BeendetDurchMsb { .. })
        ),
        "a BeendetDurchMsb event is emitted"
    );
    let _ = BEENDIGUNG_MSB_PID;
}

#[test]
fn msb_beendigung_before_delivery_is_rejected() {
    // Only a delivery-authorised process can be ended by the MSB.
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let err = W::handle(
        &s,
        C::ReceiveBeendigungDurchMsb {
            message_ref: mref("IFT-21042-2"),
            beendigung_zum: datetime!(2026-08-01 00:00 UTC),
            reason: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("Beliefert"), "got: {err}");
}
