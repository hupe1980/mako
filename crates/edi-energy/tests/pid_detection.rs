//! The Prüfidentifikator is detectable from **either** published location.
//!
//! Every BGM DE 1004 row in the MSCONS, REQOTE, ORDERS, ORDRSP, ORDCHG and
//! QUOTES handbooks reads „Dokumentennummer"; the Prüfidentifikator travels in
//! `SG1 RFF+Z13`. A parser that reads only DE 1004 finds nothing in a
//! conformant partner message and drops it — with no APERAK, because a message
//! whose PID is unknown never reaches the routing that would produce one.
//!
//! Reading only `RFF+Z13` has the mirror failure against a partner that follows
//! the other convention, so both are tried and the profile's declared source
//! goes first.

use edi_energy::EdiEnergyMessage as _;

/// Build a conformant MSCONS 13027 „Werte nach Typ 2" (MSCONS AHB 3.2 §11.2).
fn mscons_13027(document_number: &str) -> Vec<u8> {
    edi_energy::builders::MsconsBuilder::new(edi_energy::Release::new("2.4c"))
        .sender("9900357000004")
        .receiver("9905550000005")
        .document_code("Z83")
        .document_number(document_number)
        .document_date_303()
        .header_reference("AGI", "ORDERDOC0001")
        .pruefidentifikator(edi_energy::Pruefidentifikator::new(13027).expect("valid PID"))
        .serialize()
        .expect("serialize")
}

/// A partner message carries a real Belegnummer in DE 1004 — the PID is only
/// in `RFF+Z13`. Reading DE 1004 as the PID drops this shape silently.
#[test]
fn a_conformant_partner_mscons_is_detected() {
    let wire = mscons_13027("BELEG0001");
    let text = String::from_utf8(wire.clone()).expect("utf-8");
    assert!(
        text.contains("BGM+Z83+BELEG0001+9"),
        "DE 1004 is a Dokumentennummer, DE 1001 is Z83: {text}"
    );
    assert!(text.contains("RFF+Z13:13027"), "{text}");

    let msg = edi_energy::parse(&wire).expect("parse");
    assert_eq!(
        msg.detect_pruefidentifikator()
            .expect("detected from RFF+Z13")
            .as_u32(),
        13027
    );
}

/// The Werte-nach-Typ-2 segments the AHB makes Muss beyond the shared skeleton.
#[test]
fn the_typ2_delivery_carries_its_mandatory_segments() {
    let text = String::from_utf8(mscons_13027("BELEG0001")).expect("utf-8");
    // BGM 1001 = Z83 „Werte nach Typ 2"; `7` (Prozessdatenbericht) is refused
    // by the generated rule `AHB-13027-BGM-1001-Q`.
    assert!(text.contains("BGM+Z83+"), "{text}");
    // DTM+137 in format 303 (CCYYMMDDHHMMZZZ), condition [931] fixing ZZZ=+00.
    assert!(text.contains(":303'"), "DTM must be format 303: {text}");
    // SG1 RFF+AGI — hint [574]: the Belegnummer of the ORDERS that ordered the
    // values, i.e. what ties a delivery to its subscription.
    assert!(text.contains("RFF+AGI:ORDERDOC0001"), "{text}");
}

/// A message that carries the PID in DE 1004 still parses, so a counterparty
/// using that convention is not cut off.
#[test]
fn the_bgm_convention_still_parses() {
    let wire = mscons_13027("13027");
    let msg = edi_energy::parse(&wire).expect("parse");
    assert_eq!(
        msg.detect_pruefidentifikator().expect("detected").as_u32(),
        13027
    );
}

/// Every message type whose AHB gives BGM DE 1004 as a *Dokumentennummer* must
/// put the Prüfidentifikator in `RFF+Z13` and leave DE 1004 alone.
///
/// The failure this pins is systemic rather than per-message: mako once wrote
/// the PID into DE 1004 everywhere, which is non-conformant on a Muss field and
/// makes a conformant partner's message undetectable.
#[test]
fn the_pid_never_occupies_the_bgm_dokumentennummer() {
    use edi_energy::builders::{
        OrdchgBuilder, OrdersBuilder, OrdrespBuilder, QuotesBuilder, ReqoteBuilder,
    };
    let r = edi_energy::Release::new;

    let cases: Vec<(&str, u32, Vec<u8>)> = vec![
        (
            "REQOTE",
            35_003,
            ReqoteBuilder::new(r("1.3c"))
                .sender("9905550000005")
                .receiver("9900357000004")
                .document_code("Z57")
                .pruefidentifikator(35_003)
                .message_ref("BELEGREQ00001")
                .serialize()
                .expect("reqote"),
        ),
        (
            "QUOTES",
            15_003,
            QuotesBuilder::new(r("1.3c"))
                .sender("9900357000004")
                .receiver("9905550000005")
                .document_code("Z57")
                .pruefidentifikator(15_003)
                .message_ref("BELEGQUO00001")
                .serialize()
                .expect("quotes"),
        ),
        (
            "ORDERS",
            17_007,
            OrdersBuilder::new(r("1.4c"))
                .sender("9905550000005")
                .receiver("9900357000004")
                .document_code("Z57")
                .pruefidentifikator(17_007)
                .message_ref("BELEGORD00001")
                .serialize()
                .expect("orders"),
        ),
        (
            "ORDCHG",
            39_002,
            OrdchgBuilder::new(r("1.2"))
                .sender("9905550000005")
                .receiver("9900357000004")
                .document_code("Z57")
                .pruefidentifikator(39_002)
                .message_ref("BELEGOCH00001")
                .serialize()
                .expect("ordchg"),
        ),
        (
            "ORDRSP",
            19_011,
            OrdrespBuilder::new(r("1.4c"))
                .sender("9900357000004")
                .receiver("9905550000005")
                .document_code("Z57")
                .pruefidentifikator(19_011)
                .message_ref("BELEGRSP00001")
                .serialize()
                .expect("ordrsp"),
        ),
    ];

    for (name, pid, wire) in cases {
        let text = String::from_utf8(wire.clone()).expect("utf-8");
        assert!(
            text.contains(&format!("RFF+Z13:{pid}")),
            "{name}: the PID belongs in RFF+Z13 — {text}"
        );
        assert!(
            !text.contains(&format!("BGM+Z57+{pid}")),
            "{name}: DE 1004 is the Dokumentennummer, not the PID — {text}"
        );
        assert!(
            text.contains("+BELEG"),
            "{name}: DE 1004 must carry the Belegnummer — {text}"
        );
        let parsed = edi_energy::parse(&wire).unwrap_or_else(|e| panic!("{name} parse: {e}"));
        assert_eq!(
            parsed
                .detect_pruefidentifikator()
                .unwrap_or_else(|e| panic!("{name}: PID must be detectable: {e}"))
                .as_u32(),
            pid,
            "{name}"
        );
    }
}
