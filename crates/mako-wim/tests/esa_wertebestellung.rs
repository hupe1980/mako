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

use mako_wim::esa::{EBD_ESA_BEENDIGUNG, EBD_ESA_BESTELLUNG, EBD_ESA_STORNIERUNG};
use mako_wim::esa::{Preisposition, Preistyp};
use mako_wim::esa_wertebestellung::{
    ABBESTELLUNG_PID, ANFRAGE_PID, ANGEBOT_WINDOW_LABEL, ANTWORT_WINDOW_LABEL, BEENDIGUNG_MSB_PID,
    BESTAETIGUNG_PID, BESTELLUNG_PID, BINDUNGSFRIST_LABEL, EsaWertebestellungCommand as C,
    EsaWertebestellungEvent, EsaWertebestellungState as S, EsaWertebestellungWorkflow as W,
    Lokationsebene, STORNIERUNG_PID, STORNO_ABLEHNUNG_PID, STORNO_BESTAETIGUNG_PID,
};
use mako_wim::esa_wertebestellung::{Angebot, Antwort};
use time::macros::datetime;

/// A priced Angebot, which is what tells an offer from a refusal — the QUOTES
/// AHB 1.1a makes `DTM+273` Muss on the only published 15003 use case, so the
/// Bindungsfrist cannot.
fn angebot() -> Box<Angebot> {
    Box::new(Angebot {
        waehrung: Some("EUR".to_owned()),
        preise: vec![Preisposition {
            artikel_id: "9990001100002".to_owned(),
            preistyp: Preistyp::Betrieb,
            betrag: "0.004500".to_owned(),
            einheit: "DAY".to_owned(),
        }],
        obis_kennzahlen: vec!["1-1:1.29.0".to_owned()],
        einrichtung_bis: None,
    })
}

/// The `E_0256` Zustimmungscode an ORDRSP 19011 must carry.
fn zustimmung_bestellung() -> Option<Antwort> {
    Some(Antwort::new("A11", Some(EBD_ESA_BESTELLUNG.to_owned())))
}

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
            angebot: angebot(),
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
            antwort: zustimmung_bestellung(),
            smgw_quelle: None,
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
            angebot: angebot(),
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
            angebot: angebot(),
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
            antwort: zustimmung_bestellung(),
            smgw_quelle: None,
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
            antwort: zustimmung_bestellung(),
            smgw_quelle: None,
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
            angebot: angebot(),
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
            antwort: Some(Antwort::new("A09", Some(EBD_ESA_BESTELLUNG.to_owned()))),
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
            antwort: Some(Antwort::new("A04", Some(EBD_ESA_STORNIERUNG.to_owned()))),
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
            message_ref: Some(mref("QUO-REJ")),
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
            angebot: angebot(),
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

// ── Regressions ───────────────────────────────────────────────────────────────

/// UC 4.1 Nr. 3 admits no Bestellung past the Bindungsfrist, so `AngebotErhalten`
/// can never advance once it lapses. Leaving the process there kept it occupying
/// its (Meldepunkt, Messprodukt) business key for ever: `SendBestellung` could
/// only ever error, and the duplicate guard refused every replacement order — so
/// the ESA could never request those values again.
#[test]
fn a_lapsed_bindungsfrist_releases_the_business_key() {
    use mako_engine::workflow::OccupiesBusinessKey as _;

    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            angebot: angebot(),
            fruehester_start: None,
            message_ref: mref("QUO-1"),
            bindungsfrist: datetime!(2000-01-01 00:00 UTC),
        },
    )
    .unwrap();
    assert!(s.occupies_business_key(), "the offer holds the key");

    let (s, _) = step(
        &s,
        C::TimeoutExpired {
            deadline_id: mako_engine::ids::DeadlineId::new(),
            label: BINDUNGSFRIST_LABEL.into(),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Abgelehnt");
    assert!(
        !s.occupies_business_key(),
        "a lapsed offer must free the (Meldepunkt, Messprodukt) key"
    );
}

/// …while a missed ORDRSP is an anomaly, not a terminal state: an authorised
/// delivery is not voided because the MSB was late with a confirmation.
#[test]
fn a_missed_ordrsp_window_does_not_void_an_authorised_delivery() {
    let s = beliefert();
    let (s, out) = step(
        &s,
        C::TimeoutExpired {
            deadline_id: mako_engine::ids::DeadlineId::new(),
            label: ANTWORT_WINDOW_LABEL.into(),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beliefert");
    assert!(matches!(
        out.events.first(),
        Some(EsaWertebestellungEvent::FristVersaeumt { .. })
    ));
}

/// `lieferung_begonnen` is a fact about the subscription, not about the state
/// the handshake happens to be in. Held inside the `Beliefert` variant it was
/// lost on every Storno round trip and re-invented on the way back — `true`
/// after a refused Abbestellung, `false` after a refused Stornierung, neither
/// of which anything had observed.
#[test]
fn a_refused_stornierung_does_not_reset_the_delivery_flag() {
    let s = beliefert();
    let (s, _) = step(
        &s,
        C::SendStornierung {
            message_ref: mref("ESA-ST-1"),
        },
    )
    .unwrap();
    // The first values land while the Stornierung is still in flight — exactly
    // the case `E_0257` `A02` exists for.
    let (s, _) = step(&s, C::MarkLieferungBegonnen).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveStornierungAntwort {
            pid: STORNO_ABLEHNUNG_PID,
            message_ref: mref("RSP-ST-REJ"),
            antwort: Some(Antwort::new("A02", Some(EBD_ESA_STORNIERUNG.to_owned()))),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beliefert");
    assert!(
        W::handle(
            &s,
            C::SendStornierung {
                message_ref: mref("ESA-ST-2"),
            }
        )
        .is_err(),
        "the delivery has begun, so a second 39002 the MSB must refuse is not sent"
    );
}

/// UC 4.3's Vorbedingung is a running Abo. `E_0254` Prüfschritt 1 refuses the
/// Beendigung of a one-shot with `A01` by construction, so sending it spends a
/// 2-Werktage window to be told what the Codeliste already says — and the Abo
/// mode has not changed afterwards, so the ESA can only repeat it.
#[test]
fn a_one_shot_is_stornierbar_not_abbestellbar() {
    let einmalig = C::SendWerteanfrage {
        gegenstand: Box::new(mako_wim::esa::Bestellgegenstand {
            abonnement: mako_wim::esa::Abonnement::OhneAbo,
            ..*gegenstand()
        }),
        esa: mp("9905550000005"),
        msb: mp("9900357000004"),
        ebene: Lokationsebene::Marktlokation,
        lokations_id: "51238696012".to_owned(),
        message_ref: mref("ESA-WA-1"),
    };
    let (s, _) = step(&S::default(), einmalig).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            angebot: angebot(),
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
            antwort: zustimmung_bestellung(),
            smgw_quelle: None,
        },
    )
    .unwrap();
    let err = W::handle(
        &s,
        C::SendAbbestellung {
            message_ref: mref("ESA-AB-1"),
            beendigung_zum: datetime!(2026-04-01 00:00 UTC),
            grund: "einwilligung_widerrufen".to_owned(),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("stornierbar"), "got: {err}");
    // The Stornierung is the path that works.
    assert!(
        W::handle(
            &s,
            C::SendStornierung {
                message_ref: mref("ESA-ST-1"),
            }
        )
        .is_ok()
    );
}

/// `DTM+469` „Startdatum, frühestes/r" is **Muss** on the QUOTES 15003: the MSB
/// has already said it cannot serve an earlier date. Ordering the original
/// Wunschtermin anyway puts a `DTM+203` on the wire that the offer excluded.
#[test]
fn the_bestellung_honours_the_offered_earliest_start() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            angebot: angebot(),
            // Later than the 2026-03-01 Wunschtermin.
            fruehester_start: Some(datetime!(2026-05-15 00:00 UTC)),
            message_ref: mref("QUO-1"),
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
    assert_eq!(
        ob.payload["ausfuehrungsdatum"].as_str(),
        Some("2026-05-15"),
        "DTM+203 must not precede the MSB's DTM+469"
    );
}

/// …and an earlier `DTM+469` does not pull the delivery forward: the ESA asked
/// for a date and the offer only bounds it from below.
#[test]
fn an_earlier_offered_start_does_not_move_the_wunschtermin() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            angebot: angebot(),
            fruehester_start: Some(datetime!(2026-01-01 00:00 UTC)),
            message_ref: mref("QUO-1"),
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
    assert_eq!(
        out.outbox[0].payload["ausfuehrungsdatum"].as_str(),
        Some("2026-03-01")
    );
}

/// The `SG2 AJT` is the whole content of a refusal — ORDRSP AHB 1.1b §4.15
/// gives 19011–19014 no free-text segment at all. An ESA that keeps only a
/// prose `reason` cannot tell `A08` (consent expired — renew and re-order)
/// from `A10` (Lokationsbündel — split the request) from `A09` (Gerätetechnik
/// — nothing to retry).
#[test]
fn a_refusal_carries_its_published_antwortcode() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            angebot: angebot(),
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
    let (s, out) = step(
        &s,
        C::ReceiveAblehnung {
            message_ref: mref("RSP-REJ"),
            antwort: Some(Antwort::new("A08", Some(EBD_ESA_BESTELLUNG.to_owned()))),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Abgelehnt");
    let Some(EsaWertebestellungEvent::BestellungAbgelehnt {
        antwort, reason, ..
    }) = out.events.first()
    else {
        panic!("expected BestellungAbgelehnt, got {:?}", out.events);
    };
    assert_eq!(antwort.as_ref().unwrap().antwortcode, "A08");
    assert!(reason.contains("Einwilligung"), "{reason}");
}

/// ORDRSP AHB 1.1b conditions `[17]`/`[18]` bind the `AJT` Cluster to the answer
/// PID. A 19011 quoting an Ablehnungscode is a message whose halves disagree —
/// recorded as its own event, then read by PID, because resolving it by the code
/// would silently turn a confirmation into a refusal.
#[test]
fn an_answer_whose_cluster_contradicts_its_pid_is_recorded() {
    let (s, _) = step(&S::default(), werteanfrage()).unwrap();
    let (s, _) = step(
        &s,
        C::ReceiveAngebot {
            angebot: angebot(),
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
    let (s, out) = step(
        &s,
        C::ReceiveBestaetigung {
            message_ref: mref("RSP-1"),
            // `A08` is an Ablehnungscode on a Bestätigungs-PID.
            antwort: Some(Antwort::new("A08", Some(EBD_ESA_BESTELLUNG.to_owned()))),
            smgw_quelle: None,
        },
    )
    .unwrap();
    assert!(
        matches!(
            out.events.first(),
            Some(EsaWertebestellungEvent::AntwortWidersprichtSich { .. })
        ),
        "got {:?}",
        out.events
    );
    assert_eq!(s.label(), "Beliefert", "the PID still decides");
}

/// The Angebot's prices and OBIS list survive into the process: the MSB's later
/// INVOIC 31009 (UC 4.5) is checked against the offer the ESA accepted, and the
/// OBIS registers are what a delivery-surveillance sweep compares against.
#[test]
fn the_offer_survives_into_the_running_subscription() {
    let s = beliefert();
    let data = s.data().expect("Beliefert holds data");
    assert_eq!(data.angebot.waehrung.as_deref(), Some("EUR"));
    assert_eq!(
        data.angebot
            .preis(Preistyp::Betrieb)
            .map(|p| p.betrag.as_str()),
        Some("0.004500")
    );
    assert_eq!(data.angebot.obis_kennzahlen, ["1-1:1.29.0"]);
    assert_eq!(
        data.letzte_antwort.as_ref().map(|a| a.antwortcode.as_str()),
        Some("A11")
    );
}

/// A refused Abbestellung of a Widerruf is a compliance incident, and the
/// `E_0254` code is what tells the operator which one.
#[test]
fn a_refused_abbestellung_keeps_the_code_and_the_delivery() {
    let s = beliefert();
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
    let (s, out) = step(
        &s,
        C::ReceiveAblehnung {
            message_ref: mref("RSP-AB-REJ"),
            antwort: Some(Antwort::new("A04", Some(EBD_ESA_BEENDIGUNG.to_owned()))),
        },
    )
    .unwrap();
    assert_eq!(s.label(), "Beliefert");
    let Some(EsaWertebestellungEvent::AbbestellungAbgelehnt { reason, .. }) = out.events.first()
    else {
        panic!("expected AbbestellungAbgelehnt, got {:?}", out.events);
    };
    assert!(reason.contains("E_0254"), "{reason}");
    // The delivery flag survives, so a Stornierung is still correctly refused.
    assert!(
        W::handle(
            &s,
            C::SendStornierung {
                message_ref: mref("X")
            }
        )
        .is_err()
    );
}
