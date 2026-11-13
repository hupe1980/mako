//! The WiM answers whose columns name the message they answer, rendered from
//! the workflow's own outbox entry and held to the pre-send gate.
//!
//! - ORDRSP 19003/19004 Antwort Weiterverpflichtung — `DTM+Z02` verschobener
//!   Abmeldetermin and `SG1 RFF+ON` the ORDERS 17002's `BGM` DE 1004 (ORDRSP
//!   AHB 1.1b Kap. 4.11, S. 48–49).
//! - IFTSTA 21030/21031 Antwort auf die Vorabinformation — `SG15 RFF+ACW` /
//!   `RFF+ADY` the 21029's `BGM` and `CNI`, and on 21030 `DTM+76` its
//!   Umstellungszeitpunkt (IFTSTA AHB 2.1 Kap. 6.7, S. 33).
//! - IFTSTA 21027 Scheitermeldung of the wMSB — `STS+Z15+Z13` with `Z74`/`Z75`
//!   and no Codeliste, `SG14 LOC+172` (Kap. 6.4, S. 24–27).
//! - ORDRSP 19009/19010 Antwort Beendigung Rechnungsabwicklung — `BGM+Z29`,
//!   `DTM+203`, `IMD++Z02`, `RFF+ON` and `SG2 AJT` out of `E_0206`/`E_0209`
//!   (ORDRSP AHB 1.1b Kap. 4.14, S. 57–59).

use crate::config::PartyConfig;
use crate::party_registry::MpIdRegistry;
use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::{OutboxMessage, PendingOutbox};
use mako_engine::types::{MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator, Sparte};
use mako_engine::workflow::Workflow;

use super::render_to_wire_bytes;

const NB: &str = "9900357000004";
const MSB: &str = "9900077000006";
const WMSB: &str = "9900555000005";
const LF: &str = "9900000000027";
const MELO: &str = "DE0004096999000000000000000000009";

/// Render `pending` as the outbox worker does, hold it against the pre-send
/// gate and return the wire text.
fn sendable(pending: &PendingOutbox) -> String {
    let sender = pending
        .payload
        .get("sender")
        .and_then(serde_json::Value::as_str)
        .expect("the entry names its sender");
    let registry = MpIdRegistry::from_config(&[PartyConfig {
        mp_id: sender.to_owned(),
        roles: Vec::new(),
        primary: true,
        agency: None,
        as4: None,
    }])
    .expect("registry");
    let msg = OutboxMessage::new(
        StreamId::new("process/wim-test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        pending.message_type.clone(),
        pending.recipient.clone(),
        pending.payload.clone(),
    );
    let wire = render_to_wire_bytes(&msg, &registry)
        .unwrap_or_else(|e| panic!("{} renders: {e}", pending.message_type));
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let report = edi_energy::parse(&wire.bytes)
        .expect("parses")
        .validate_on_date(mako_fristen::heute())
        .expect("validation runs");
    assert!(
        report.is_sendable(),
        "the pre-send gate refuses it: {:?}\n{text}",
        report.sending_refusals().collect::<Vec<_>>()
    );
    text
}

/// The one entry of `message_type` in `outbox`.
fn entry<'a>(outbox: &'a [PendingOutbox], message_type: &str) -> &'a PendingOutbox {
    outbox
        .iter()
        .find(|o| &*o.message_type == message_type)
        .unwrap_or_else(|| panic!("no {message_type} in {outbox:?}"))
}

#[test]
fn the_weiterverpflichtung_answers_name_the_orders_and_the_abmeldetermin() {
    use mako_wim::weiterverpflichtung::{
        WeiterverpflichtungCommand as C, WeiterverpflichtungState as S,
        WimWeiterverpflichtungWorkflow as W,
    };
    for (code, termin, pid) in [("Z13", None, 19_003), ("Z22", Some("20270301"), 19_004)] {
        let out = W::handle(
            &S::New,
            C::ReceiveAuftrag {
                pid: Pruefidentifikator::new(17_002).expect("pid"),
                nb: MarktpartnerCode::new(NB),
                msba: MarktpartnerCode::new(MSB),
                melo_id: MeLo::new(MELO),
                verschobenes_zuordnungsende: "20270201".to_owned(),
                message_ref: MessageRef::new("UNH-17002"),
                dokumentennummer: "ORD-17002-4711".to_owned(),
                validation_passed: true,
                validation_errors: Vec::new(),
                sparte: Sparte::Strom,
            },
        )
        .expect("received");
        let state = out.events.iter().fold(S::New, W::apply);
        let out = W::handle(
            &state,
            C::DispatchAntwort {
                antwort_code: code.to_owned(),
                abweichender_termin: termin.map(str::to_owned),
            },
        )
        .expect("answered");
        let text = sendable(entry(&out.outbox, "ORDRSP"));
        assert!(text.contains(&format!("RFF+Z13:{pid}")), "{text}");
        assert!(text.contains("RFF+ON:ORD-17002-4711"), "{text}");
        // 00:00 Uhr of the German day: 01.02.2027 resp. 01.03.2027 (MEZ).
        let tag = if termin.is_some() {
            "20270228"
        } else {
            "20270131"
        };
        assert!(
            text.contains(&format!("DTM+Z02:{tag}2300?+00:303")),
            "{text}"
        );
    }
}

#[test]
fn the_ersteinbau_answers_name_the_vorabinformation() {
    use mako_wim::ersteinbau::{
        ErsteinbauCommand as C, ErsteinbauState as S, WimErsteinbauWorkflow as W,
    };
    for (code, pid) in [("A03", 21_030), ("A01", 21_031)] {
        let out = W::handle(
            &S::New,
            C::ReceiveVorabinformation {
                pid: Pruefidentifikator::new(21_029).expect("pid"),
                gmsb: MarktpartnerCode::new(MSB),
                wmsb: MarktpartnerCode::new(WMSB),
                melo_id: MeLo::new(MELO),
                umstellungszeitpunkt: "20270601".to_owned(),
                message_ref: MessageRef::new("UNH-21029"),
                referenz: Some(mako_wim::geraetewechsel::IftstaReferenz {
                    dokumentennummer: "IFT-21029-1".to_owned(),
                    vorgangsnummer: "1".to_owned(),
                }),
                validation_passed: true,
                validation_errors: Vec::new(),
            },
        )
        .expect("received");
        let state = out.events.iter().fold(S::New, W::apply);
        let out = W::handle(
            &state,
            C::DispatchAntwort {
                antwort_code: code.to_owned(),
            },
        )
        .expect("answered");
        let text = sendable(entry(&out.outbox, "IFTSTA"));
        assert!(text.contains(&format!("RFF+Z13:{pid}")), "{text}");
        assert!(text.contains("RFF+ACW:IFT-21029-1"), "{text}");
        assert!(text.contains("RFF+ADY:1"), "{text}");
        assert_eq!(
            text.contains("DTM+76:20270601:102"),
            pid == 21_030,
            "DTM+76 rides the 21030 alone:\n{text}"
        );
    }
}

#[test]
fn the_wmsbs_scheitermeldung_renders_its_21027_column() {
    use mako_wim::ersteinbau::{
        ErsteinbauCommand as C, ErsteinbauState as S, WimErsteinbauWorkflow as W,
    };
    let mut state = S::New;
    for cmd in [
        C::ReceiveVorabinformation {
            pid: Pruefidentifikator::new(21_029).expect("pid"),
            gmsb: MarktpartnerCode::new(MSB),
            wmsb: MarktpartnerCode::new(WMSB),
            melo_id: MeLo::new(MELO),
            umstellungszeitpunkt: "20270601".to_owned(),
            message_ref: MessageRef::new("UNH-21029"),
            referenz: Some(mako_wim::geraetewechsel::IftstaReferenz {
                dokumentennummer: "IFT-21029-1".to_owned(),
                vorgangsnummer: "1".to_owned(),
            }),
            validation_passed: true,
            validation_errors: Vec::new(),
        },
        C::DispatchAntwort {
            antwort_code: "A02".to_owned(),
        },
    ] {
        let out = W::handle(&state, cmd).expect("accepted");
        state = out.events.iter().fold(state, W::apply);
    }
    assert!(
        W::handle(
            &state,
            C::MeldeScheitern {
                grund: "A99".to_owned(),
                reason: "x".to_owned(),
                nb: Some(MarktpartnerCode::new(NB)),
                lf: Some(MarktpartnerCode::new(LF)),
            },
        )
        .is_err(),
        "a ground the column does not list is refused"
    );
    let out = W::handle(
        &state,
        C::MeldeScheitern {
            grund: "Z74".to_owned(),
            reason: "Zählerschrank verschlossen".to_owned(),
            nb: Some(MarktpartnerCode::new(NB)),
            lf: Some(MarktpartnerCode::new(LF)),
        },
    )
    .expect("reported");
    // „an alle im Prozess genannten Marktrollen" (IFTSTA AHB 2.1 Kap. 6.4,
    // S. 24): the gMSB and the NB by 21027 „MSB an NB / MSB", the LF by
    // 21025 „MSB an LF" — the roles of WiM Teil 1 Kap. 3.5.1 (S. 60).
    let mut empfaenger = Vec::new();
    for e in out.outbox.iter().filter(|o| &*o.message_type == "IFTSTA") {
        let text = sendable(e);
        let pid = if &*e.recipient == LF { 21_025 } else { 21_027 };
        assert!(text.contains("STS+Z15+Z13+Z74'"), "{text}");
        assert!(text.contains(&format!("LOC+172+{MELO}")), "{text}");
        assert!(text.contains(&format!("RFF+Z13:{pid}")), "{text}");
        assert!(text.contains(&format!("NAD+MR+{}", e.recipient)), "{text}");
        assert!(!text.contains("FTX"), "no SG25 beside Z74:\n{text}");
        empfaenger.push(e.recipient.to_string());
    }
    assert_eq!(empfaenger, [MSB, NB, LF]);
}

#[test]
fn the_rechnungsabwicklung_answers_carry_their_column() {
    use mako_wim::rechnungsabwicklung::{
        BeendigungDurch, RechnungsabwicklungCommand as C, RechnungsabwicklungState as S,
        WimRechnungsabwicklungWorkflow as W,
    };
    // The MSB ended it, so this party is the LF and answers out of `E_0206`.
    for (zustimmung, code, pid) in [(true, "A02", 19_009), (false, "A01", 19_010)] {
        let out = W::handle(
            &S::New,
            C::ReceiveOrders {
                pid: Pruefidentifikator::new(17_006).expect("pid"),
                sender: MarktpartnerCode::new(MSB),
                receiver: MarktpartnerCode::new(LF),
                message_ref: MessageRef::new("UNH-17006"),
                dokumentennummer: "ORD-17006-9".to_owned(),
                ausfuehrungsdatum: Some("20270101".to_owned()),
                validation_passed: true,
                validation_errors: Vec::new(),
            },
        )
        .expect("received");
        let state = out.events.iter().fold(S::New, W::apply);
        let out = W::handle(
            &state,
            C::SendAntwort {
                zustimmung,
                antwort_code: code.to_owned(),
                beendet_durch: BeendigungDurch::Msb,
                message_ref: MessageRef::new("ORDRSP-1"),
            },
        )
        .expect("answered");
        let text = sendable(entry(&out.outbox, "ORDRSP"));
        assert!(text.contains("BGM+Z29+"), "{text}");
        assert!(text.contains(&format!("RFF+Z13:{pid}")), "{text}");
        assert!(text.contains("RFF+ON:ORD-17006-9"), "{text}");
        assert!(text.contains("IMD++Z02"), "{text}");
        assert!(text.contains("DTM+203:202612312300?+00:303"), "{text}");
        assert!(text.contains(&format!("AJT+{code}+E_0206")), "{text}");
    }
}

/// The ORDERS 17006 Beendigung der Rechnungsabwicklung carries its column
/// (ORDERS AHB 1.1b Kap. 4.14.2, S. 93–94): `BGM+Z29`, `DTM+203`, `IMD++Z02`
/// Ende Abo, `SG2 LOC+172` the Marktlokation and — from the MSB (`[15]`) —
/// the one `SG29 LIN` with its `IMD` Grund `Z61`/`Z62`/`Z63`.
#[test]
fn the_beendigung_der_rechnungsabwicklung_renders_its_17006_column() {
    use mako_wim::rechnungsabwicklung::{
        BeendigungDurch, RechnungsabwicklungCommand as C, RechnungsabwicklungState as S,
        WimRechnungsabwicklungWorkflow as W,
    };
    for (beendet_durch, sender, receiver, grund) in [
        (BeendigungDurch::Lf, LF, MSB, None),
        (BeendigungDurch::Msb, MSB, LF, Some("Z61")),
    ] {
        let out = W::handle(
            &S::New,
            C::SendBeendigung {
                sender: MarktpartnerCode::new(sender),
                counterparty: MarktpartnerCode::new(receiver),
                location_id: "51238696012".to_owned(),
                message_ref: MessageRef::new("ORD-17006-OUT"),
                beendet_durch,
                ausfuehrungsdatum: "20270101".to_owned(),
                grund: grund.map(str::to_owned),
            },
        )
        .expect("sent");
        let text = sendable(entry(&out.outbox, "ORDERS"));
        assert!(text.contains("BGM+Z29+ORD-17006-OUT"), "{text}");
        assert!(text.contains("DTM+203:202612312300?+00:303"), "{text}");
        assert!(text.contains("IMD++Z02"), "{text}");
        assert!(text.contains("LOC+172+51238696012"), "{text}");
        assert!(text.contains("RFF+Z13:17006"), "{text}");
        assert_eq!(text.contains("LIN+1"), grund.is_some(), "{text}");
        if let Some(g) = grund {
            assert!(text.contains(&format!("IMD++{g}")), "{text}");
        }
    }
    // From the MSB the Grund is Muss.
    assert!(
        W::handle(
            &S::New,
            C::SendBeendigung {
                sender: MarktpartnerCode::new(MSB),
                counterparty: MarktpartnerCode::new(LF),
                location_id: "51238696012".to_owned(),
                message_ref: MessageRef::new("ORD-17006-OUT"),
                beendet_durch: BeendigungDurch::Msb,
                ausfuehrungsdatum: "20270101".to_owned(),
                grund: None,
            },
        )
        .is_err()
    );
}
