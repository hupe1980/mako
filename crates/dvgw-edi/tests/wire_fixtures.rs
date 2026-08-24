//! Round-trips against the example messages printed in the specifications.
//!
//! Every fixture below is copied from a primary source, not written to match the
//! parser. That direction matters: the previous fixtures were authored from the
//! implementation and so agreed with it while disagreeing with every real
//! message — they asserted `UNH+1+ALOCAT:5:11a`, a header no DVGW sender emits.
//!
//! Sources:
//! - DVGW-Nachrichtenbeschreibung ALOCAT 5.11a (ORDRSP / UN D.07A S3), §3.2
//! - DVGW-Nachrichtenbeschreibung NOMINT 4.6 (ORDERS / UN D.07A S3), §3.2
//! - Trading Hub Europe, "Quick guide for Edig@s message formats at the
//!   THE Virtual Trading Point", §3.2 (NOMINT 4.6) and §3.3.1 (NOMRES 4.7)

use dvgw_edi::{
    DvgwDocument, DvgwMessageType, DvgwPlatform, Error, MessageBuilder, Position, Severity,
    model::{imd, nad},
};
use time::macros::datetime;

// ── Fixtures ─────────────────────────────────────────────────────────────────

/// ALOCAT 5.11a §3.2, wrapped in the interchange the spec omits.
const ALOCAT: &[u8] = b"UNB+UNOA:3+9870012345678:502+9800505300009:502+180101:1200+IC1'\
UNH+123456+ORDRSP:D:07A:UN:5.11a'\
BGM+X1G::332+ALOCAT123456'\
DTM+Z05:0:805'\
DTM+137:201801011200:203'\
DTM+Z01:201801010500201801020500:719'\
RFF+ANX:CLEARINGNUMMER'\
RFF+Z13:70001'\
NAD+MS+9870012345678::332'\
NAD+MR+9800505300009::332'\
LIN+1++:Z01::332'\
LOC+Z99'\
DTM+2:201801010500201801020500:719'\
QTY+Z03:4000:KW1'\
STS+09G::332'\
NAD+ZEU+THE0BFH123456789::332'\
NAD+ZSZ+THE0NKH712345678::332'\
UNS+S'\
UNT+17+123456'\
UNZ+1+IC1'";

/// NOMINT 4.6 §3.2.
const NOMINT: &[u8] = b"UNB+UNOA:3+9870009700005:502+9870009700005:502+180104:2056+IC2'\
UNH+1+ORDERS:D:07A:UN:DVGW18'\
BGM+01G::332+NOMINT00052'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:201801050400201801060400:719'\
RFF+Z13:70030'\
RFF+AGO:1234'\
DTM+9:201801042056:203'\
NAD+MS+9870009700005::332'\
NAD+MR+9870009700005::332'\
NAD+ZSY+9870009700005::332'\
LIN+1'\
LOC+Z19+ABCD1234::332'\
DTM+2:201801050400201801060400:719'\
QTY+Z03:6782:KW1'\
NAD+ZEU+BK-CODE-1::332'\
NAD+ZES+BK-CODE-2::332'\
UNS+S'\
UNT+19+1'\
UNZ+1+IC2'";

/// Trading Hub Europe §3.3.1 — NOMRES 4.7, confirmation of a VHP nomination.
/// Four positions, alternating `17G` (own side) and `18G` (counterparty).
const NOMRES_VHP: &[u8] =
    b"UNB+UNOA:3+9800505300009:502+5200000000000:14+211001:1334+THE123456789'\
UNH+0123456789+ORDRSP:D:07A:UN:DVGW17'\
BGM+19G::332+NOMRES0123456789'\
DTM+Z05:0:805'\
DTM+137:202111081234:203'\
DTM+Z01:202110020400202110030400:719'\
RFF+Z13:70037'\
NAD+MS+9800505300009::332'\
NAD+MR+5200000000000::9'\
LIN+1'\
IMD++05G+17G::332'\
LOC+Z19+37Z005053MH0000D::332'\
DTM+2:202110020400202110030400:719'\
QTY+Z02:100:KW1'\
NAD+ZEU+THE0BFH000000001::332'\
NAD+ZES+THE0BFH000000002::332'\
LIN+2'\
IMD++05G+18G::332'\
LOC+Z19+37Z005053MH0000D::332'\
DTM+2:202110020400202110030400:719'\
QTY+Z03:100:KW1'\
NAD+ZEU+THE0BFH000000001::332'\
NAD+ZES+THE0BFH000000002::332'\
UNS+S'\
UNT+24+0123456789'\
UNZ+1+THE123456789'";

// ── Identity ─────────────────────────────────────────────────────────────────

#[test]
fn alocat_is_identified_by_its_document_code_not_by_unh() {
    let msg = DvgwPlatform::default()
        .parse(ALOCAT)
        .expect("ALOCAT must parse");
    assert_eq!(msg.message_type, DvgwMessageType::Alocat);
    assert_eq!(msg.document, DvgwDocument::AllokationSlp);
    // The carrier is ORDRSP — matching UNH against "ALOCAT" would reject this.
    assert_eq!(msg.carrier.as_str(), "ORDRSP");
    assert_eq!(msg.version.as_ref().map(|v| v.as_str()), Some("5.11a"));
    assert_eq!(msg.document_number.as_deref(), Some("ALOCAT123456"));
}

#[test]
fn nomint_and_nomres_are_identified_by_their_document_codes() {
    let platform = DvgwPlatform::default();
    let nomint = platform.parse(NOMINT).expect("NOMINT must parse");
    assert_eq!(nomint.message_type, DvgwMessageType::Nomint);
    assert_eq!(nomint.document, DvgwDocument::NominierungTransportkunde);
    assert_eq!(nomint.carrier.as_str(), "ORDERS");

    let nomres = platform.parse(NOMRES_VHP).expect("NOMRES must parse");
    assert_eq!(nomres.message_type, DvgwMessageType::Nomres);
    assert_eq!(nomres.document, DvgwDocument::VhpMatchingBenachrichtigung);
    assert_eq!(nomres.carrier.as_str(), "ORDRSP");
}

#[test]
fn a_carrier_that_contradicts_the_document_code_is_refused() {
    // NOMINT's 01G rides ORDERS; claiming ORDRSP is a message no sender emits.
    let broken = NOMINT.to_vec();
    let broken = String::from_utf8(broken)
        .unwrap()
        .replace("ORDERS:D:07A", "ORDRSP:D:07A");
    let err = DvgwPlatform::default()
        .parse(broken.as_bytes())
        .expect_err("the two identifying fields disagree");
    assert!(matches!(
        err,
        Error::CarrierMismatch {
            document: "01G",
            ..
        }
    ));
}

#[test]
fn an_unknown_document_code_is_refused_rather_than_guessed() {
    let broken = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("BGM+X1G", "BGM+ZZZ");
    let err = DvgwPlatform::default()
        .parse(broken.as_bytes())
        .expect_err("ZZZ is not a DVGW document code");
    assert!(matches!(err, Error::UnknownDocumentCode { .. }));
}

// ── The Prüfidentifikator is on the wire ─────────────────────────────────────

#[test]
fn the_pruefidentifikator_comes_from_rff_z13() {
    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    let pid = msg.pruefidentifikator.expect("RFF+Z13 carries it");
    assert_eq!(pid.as_u32(), 70_001);
    let info = pid.info().expect("70001 is published");
    assert_eq!(info.message_type, DvgwMessageType::Alocat);
    assert_eq!(info.direction, "NB an MGV");
}

#[test]
fn rff_ago_not_rff_z13_correlates_a_re_nomination() {
    let msg = DvgwPlatform::default().parse(NOMINT).unwrap();
    assert_eq!(msg.original_nomination_ref(), Some("1234"));
    // RFF+Z13 is the process code, so it must not be mistaken for a back-reference.
    assert_eq!(msg.pruefidentifikator.unwrap().as_u32(), 70_030);
}

// ── Dates ────────────────────────────────────────────────────────────────────

#[test]
fn the_gas_day_is_dtm_z01_and_dtm_137_is_the_message_timestamp() {
    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    let gas_day = msg
        .validity_period
        .expect("DTM+Z01 is the Gültigkeitszeitraum");
    assert_eq!(gas_day.start, datetime!(2018-01-01 05:00 UTC));
    assert_eq!(gas_day.end, datetime!(2018-01-02 05:00 UTC));
    assert_eq!(gas_day.duration(), time::Duration::hours(24));
    // DTM+137 is when the message was written, a different thing entirely.
    assert_eq!(msg.message_datetime, Some(datetime!(2018-01-01 12:00 UTC)));
    assert_eq!(msg.timezone, time::UtcOffset::UTC);
}

// ── Structure ────────────────────────────────────────────────────────────────

#[test]
fn alocat_keeps_a_position_whose_loc_carries_no_code() {
    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    assert_eq!(msg.items.len(), 1);
    let item = &msg.items[0];
    assert_eq!(
        item.item_type.as_deref(),
        Some("Z01"),
        "Zeitreihentyp from LIN C212"
    );
    assert_eq!(item.locations.len(), 1);
    // `LOC+Z99` has no location code — requiring one drops the whole position.
    assert_eq!(item.locations[0].qualifier, "Z99");
    assert_eq!(item.locations[0].code, None);
    assert_eq!(item.locations[0].quantities.len(), 1);

    let qty = &item.locations[0].quantities[0];
    assert_eq!(qty.qualifier, "Z03");
    assert_eq!(qty.value.unwrap().to_string(), "4000");
    assert_eq!(qty.unit.as_deref(), Some("KW1"));
    assert_eq!(qty.status, vec!["09G".to_owned()]);
    assert_eq!(qty.period.unwrap().start, datetime!(2018-01-01 05:00 UTC));

    assert_eq!(
        item.party(nad::BILANZKREIS_INTERN).unwrap().id,
        "THE0BFH123456789"
    );
    assert_eq!(item.party(nad::NETZKONTO).unwrap().id, "THE0NKH712345678");
    assert_eq!(msg.clearingnummer(), Some("CLEARINGNUMMER"));
}

#[test]
fn nomres_positions_stay_separable_by_their_imd_label() {
    let msg = DvgwPlatform::default().parse(NOMRES_VHP).unwrap();
    assert_eq!(msg.items.len(), 2);
    assert_eq!(msg.items[0].description_code(), Some(imd::NOMINIERT));
    assert_eq!(msg.items[1].description_code(), Some(imd::GEGENSEITE));
    // Without IMD both positions read as 100 KW1 at the same location and the
    // counterparty's quantity is indistinguishable from one's own.
    assert_eq!(msg.items[0].locations[0].quantities[0].qualifier, "Z02");
    assert_eq!(msg.items[1].locations[0].quantities[0].qualifier, "Z03");
    assert_eq!(msg.quantities().count(), 2);
}

/// Edig@s `SG37` repeats up to 199 times: a `LOC` group is a time series.
#[test]
fn every_quantity_of_a_profile_survives_the_walk() {
    let profile = b"UNB+UNOA:3+A:502+B:502+260301:0500+IC3'\
UNH+1+ORDERS:D:07A:UN:DVGW17'\
BGM+55G::332+NOMINT77'\
DTM+Z05:0:805'\
DTM+137:202603010400:203'\
DTM+Z01:202603010500202603010800:719'\
RFF+Z13:70031'\
NAD+MS+A::332'\
NAD+MR+B::332'\
LIN+1'\
LOC+Z19+37Z005053MH0000D::332'\
DTM+2:202603010500202603010600:719'\
QTY+Z02:100:KW1'\
DTM+2:202603010600202603010700:719'\
QTY+Z02:200:KW1'\
DTM+2:202603010700202603010800:719'\
QTY+Z02:300:KW1'\
NAD+ZEU+BK1::332'\
NAD+ZES+BK2::332'\
UNS+S'\
UNT+19+1'\
UNZ+1+IC3'";

    let msg = DvgwPlatform::default().parse(profile).unwrap();
    let quantities: Vec<_> = msg.quantities().collect();
    assert_eq!(
        quantities.len(),
        3,
        "keeping only the last hour loses the profile"
    );
    assert_eq!(quantities[0].value.unwrap().to_string(), "100");
    assert_eq!(quantities[2].value.unwrap().to_string(), "300");
    // Each quantity keeps the period of the DTM+2 that preceded it.
    assert_eq!(
        quantities[0].period.unwrap().start,
        datetime!(2026-03-01 05:00 UTC)
    );
    assert_eq!(
        quantities[2].period.unwrap().start,
        datetime!(2026-03-01 07:00 UTC)
    );
}

/// One `UNH`…`UNT` window per message, even when several share an envelope.
#[test]
fn a_multi_message_interchange_does_not_merge_into_one() {
    let mut raw = Vec::new();
    raw.extend_from_slice(b"UNB+UNOA:3+A:502+B:502+260301:0500+IC4'");
    for (n, doc) in [(1, "01G"), (2, "55G")] {
        raw.extend_from_slice(
            format!(
                "UNH+{n}+ORDERS:D:07A:UN:DVGW17'BGM+{doc}::332+NOMINT{n}'DTM+Z05:0:805'\
DTM+137:202603010400:203'DTM+Z01:202603010500202603020500:719'RFF+Z13:7003{n}'\
NAD+MS+A::332'NAD+MR+B::332'LIN+1'LOC+Z19+P::332'\
DTM+2:202603010500202603020500:719'QTY+Z02:{n}0:KW1'NAD+ZEU+BK1::332'NAD+ZES+BK2::332'\
UNS+S'UNT+15+{n}'"
            )
            .as_bytes(),
        );
    }
    raw.extend_from_slice(b"UNZ+2+IC4'");

    let messages: Vec<_> = DvgwPlatform::default()
        .parse_interchange(&raw)
        .collect::<Result<Vec<_>, _>>()
        .expect("both messages must parse");
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].document,
        DvgwDocument::NominierungTransportkunde
    );
    assert_eq!(
        messages[1].document,
        DvgwDocument::NominierungVirtuellerHandelspunkt
    );
    assert_eq!(messages[0].quantities().count(), 1);
    assert_eq!(messages[1].quantities().count(), 1);
}

// ── Validation ───────────────────────────────────────────────────────────────

#[test]
fn the_specification_examples_validate_clean() {
    let platform = DvgwPlatform::default();
    for (name, raw) in [
        ("ALOCAT", ALOCAT),
        ("NOMINT", NOMINT),
        ("NOMRES", NOMRES_VHP),
    ] {
        let report = platform.validate(raw).expect("must parse");
        let errors: Vec<String> = report.errors().map(ToString::to_string).collect();
        assert!(
            report.is_valid(),
            "{name} is a published example and must validate clean: {errors:#?}"
        );
    }
}

#[test]
fn a_missing_pruefidentifikator_is_an_error_not_a_shrug() {
    let broken = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("RFF+Z13:70001'", "");
    let report = DvgwPlatform::default().validate(broken.as_bytes()).unwrap();
    assert!(!report.is_valid());
    assert!(report.errors().any(|i| i.rule_id == Some("DVGW-RFF-Z13")));
}

#[test]
fn a_dtm_that_contradicts_its_own_format_code_is_reported() {
    // 719 promises two stamps; this carries one.
    let broken = String::from_utf8(ALOCAT.to_vec()).unwrap().replace(
        "DTM+Z01:201801010500201801020500:719",
        "DTM+Z01:20180101:719",
    );
    let report = DvgwPlatform::default().validate(broken.as_bytes()).unwrap();
    assert!(
        report
            .errors()
            .any(|i| i.rule_id == Some("DVGW-DTM-UNDECODABLE"))
    );
}

#[test]
fn a_bdew_pruefidentifikator_is_out_of_range_for_dvgw() {
    let broken = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("RFF+Z13:70001", "RFF+Z13:55001");
    let report = DvgwPlatform::default().validate(broken.as_bytes()).unwrap();
    assert!(
        report
            .errors()
            .any(|i| i.rule_id == Some("DVGW-RFF-Z13-RANGE"))
    );
}

#[test]
fn a_quantity_in_the_wrong_unit_warns_without_failing_the_message() {
    let broken = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("QTY+Z03:4000:KW1", "QTY+Z03:4000:KWH");
    let report = DvgwPlatform::default().validate(broken.as_bytes()).unwrap();
    assert!(report.is_valid(), "the unit is advisory, not a Muss row");
    assert!(
        report
            .warnings()
            .any(|i| i.rule_id == Some("DVGW-QTY-UNIT") && i.severity == Severity::Warning)
    );
}

#[test]
fn a_nomres_position_without_imd_is_flagged() {
    let broken = String::from_utf8(NOMRES_VHP.to_vec())
        .unwrap()
        .replace("IMD++05G+17G::332'", "");
    let report = DvgwPlatform::default().validate(broken.as_bytes()).unwrap();
    assert!(
        report
            .warnings()
            .any(|i| i.rule_id == Some("DVGW-IMD-REQUIRED"))
    );
}

// ── Writing ──────────────────────────────────────────────────────────────────

#[test]
fn a_built_nomint_parses_back_and_validates() {
    let gas_day = dvgw_edi::DvgwPeriod {
        start: datetime!(2026-03-01 05:00 UTC),
        end: datetime!(2026-03-02 05:00 UTC),
    };
    let wire = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
        .message_ref("1")
        .document_number("NOMINT00052")
        .version("DVGW17")
        .pruefidentifikator(70_030)
        .message_datetime(datetime!(2026-02-28 20:56 UTC))
        .validity_period(gas_day)
        .sender("9870009700005")
        .receiver("9870009700006")
        .original_nomination_ref("1234")
        .position(
            Position::new()
                .location("Z19", Some("ABCD1234"))
                .quantity("Z03", "6782", gas_day)
                .party(nad::BILANZKREIS_INTERN, "BK-CODE-1")
                .party(nad::BILANZKREIS_EXTERN, "BK-CODE-2"),
        )
        .build()
        .expect("every mandatory field was set");

    let rendered = String::from_utf8(wire.clone()).unwrap();
    assert!(
        rendered.starts_with("UNH+1+ORDERS:D:07A:UN:DVGW17'"),
        "{rendered}"
    );
    assert!(rendered.contains("BGM+01G::332+NOMINT00052'"));
    assert!(rendered.contains("DTM+Z01:202603010500202603020500:719'"));
    assert!(rendered.contains("RFF+Z13:70030'"));

    let report = DvgwPlatform::default().validate(&wire).unwrap();
    let errors: Vec<String> = report.errors().map(ToString::to_string).collect();
    assert!(
        report.is_valid(),
        "a message we wrote must satisfy our own rules: {errors:#?}"
    );

    let msg = DvgwPlatform::default().parse(&wire).unwrap();
    assert_eq!(msg.validity_period, Some(gas_day));
    assert_eq!(
        msg.quantities().next().unwrap().value.unwrap().to_string(),
        "6782"
    );
}

#[test]
fn the_builder_refuses_to_emit_an_incomplete_message() {
    let err = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
        .document_number("X")
        .build()
        .expect_err("no Prüfidentifikator was set");
    assert!(matches!(err, Error::Serialize(_)));
}

#[test]
fn the_unt_segment_count_a_builder_writes_is_correct() {
    let gas_day = dvgw_edi::DvgwPeriod {
        start: datetime!(2026-03-01 05:00 UTC),
        end: datetime!(2026-03-02 05:00 UTC),
    };
    let wire = MessageBuilder::new(DvgwDocument::AllokationSlp)
        .document_number("ALOCAT1")
        .pruefidentifikator(70_001)
        .message_datetime(datetime!(2026-03-01 04:00 UTC))
        .validity_period(gas_day)
        .clearingnummer("CLR1")
        .sender("A")
        .receiver("B")
        .position(
            Position::new()
                .item_type("Z01")
                .location("Z99", None)
                .quantity("Z03", "4000", gas_day)
                .status("09G")
                .party(nad::BILANZKREIS_INTERN, "BK1")
                .party(nad::NETZKONTO, "NK1"),
        )
        .build()
        .unwrap();

    let rendered = String::from_utf8(wire).unwrap();
    let declared: usize = rendered
        .split('\'')
        .find(|s| s.starts_with("UNT+"))
        .and_then(|s| s.split('+').nth(1))
        .unwrap()
        .parse()
        .unwrap();
    let actual = rendered.matches('\'').count();
    assert_eq!(declared, actual, "UNT DE 0074 counts UNH…UNT inclusive");
}

/// A value carrying an EDIFACT service character must not close the segment.
///
/// Outbound messages are assembled from identifiers a counterparty supplied —
/// a Bilanzkreis code, a Dokumentennummer echoed back. Writing one unescaped
/// lets that counterparty end the segment early and have everything after it
/// read as segments of its choosing.
#[test]
fn service_characters_in_a_value_are_escaped_not_emitted_raw() {
    let gas_day = dvgw_edi::DvgwPeriod {
        start: datetime!(2026-03-01 05:00 UTC),
        end: datetime!(2026-03-02 05:00 UTC),
    };
    let hostile = "BK'NAD+MR+IMPOSTOR::332";

    let wire = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
        .document_number("NOMINT1")
        .version("DVGW17")
        .pruefidentifikator(70_030)
        .message_datetime(datetime!(2026-02-28 20:56 UTC))
        .validity_period(gas_day)
        .sender("A")
        .receiver("B")
        .position(
            Position::new()
                .location("Z19", Some("P"))
                .quantity("Z03", "1", gas_day)
                .party(nad::BILANZKREIS_INTERN, hostile)
                .party(nad::BILANZKREIS_EXTERN, "BK2"),
        )
        .build()
        .expect("builds");

    let rendered = String::from_utf8(wire.clone()).unwrap();
    assert!(
        rendered.contains("BK?'NAD?+MR?+IMPOSTOR?:?:332"),
        "the service characters must be escaped: {rendered}"
    );

    // And the injected text must come back as one value, not as segments.
    let msg = DvgwPlatform::default().parse(&wire).expect("re-parses");
    assert_eq!(
        msg.items[0].party(nad::BILANZKREIS_INTERN).unwrap().id,
        hostile,
        "the escaped value must round-trip intact"
    );
    assert_eq!(
        msg.receiver().map(|p| p.id.as_str()),
        Some("B"),
        "the injected NAD+MR must not have displaced the real receiver"
    );
}

// ── Sniffing at an ingest boundary ───────────────────────────────────────────

/// The sniffer must separate the two families, and `UNH` cannot do it.
#[test]
fn sniff_separates_dvgw_from_bdew_by_the_document_code() {
    assert_eq!(
        dvgw_edi::sniff(ALOCAT),
        Some(DvgwDocument::AllokationSlp),
        "a real ALOCAT must be recognised"
    );
    assert_eq!(
        dvgw_edi::sniff(NOMINT),
        Some(DvgwDocument::NominierungTransportkunde)
    );
    assert_eq!(
        dvgw_edi::sniff(NOMRES_VHP),
        Some(DvgwDocument::VhpMatchingBenachrichtigung)
    );

    // A BDEW ORDRSP — the same UNH carrier an ALOCAT uses, so anything keying on
    // `UNH` would claim this one too.
    let bdew_ordrsp = b"UNB+UNOC:3+A:500+B:500+260804:1045+REF1'\
UNH+1+ORDRSP:D:07B:UN:1.1c'BGM+231+19110'DTM+137:20260804:102'\
NAD+MS+A::293'NAD+MR+B::293'UNT+6+1'UNZ+1+REF1'";
    assert_eq!(
        dvgw_edi::sniff(bdew_ordrsp),
        None,
        "a BDEW ORDRSP must not be claimed by the DVGW parser"
    );

    assert_eq!(dvgw_edi::sniff(b"not edifact at all"), None);
    assert_eq!(dvgw_edi::sniff(b""), None);
}

/// The same bytes must not parse as both families with different meanings.
#[test]
fn a_dvgw_message_is_not_mistaken_for_its_bdew_carrier() {
    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    // The DVGW parser reads the Prüfidentifikator from RFF+Z13…
    assert_eq!(msg.pruefidentifikator.unwrap().as_u32(), 70_001);
    // …and 70001 is outside every BDEW range, so a router that saw this message
    // as a BDEW ORDRSP would find no workflow rather than the wrong one.
    assert!(!(10_000..=59_999).contains(&msg.pruefidentifikator.unwrap().as_u32()));
}

// ── Zuordnung (ALOCAT 5.11a §3.3) ────────────────────────────────────────────

/// The ALOCAT fixture is PID 70001, which §3.3 assigns `ZO-T3`
/// (Bilanzkreis, Netzkontonummer, Zeitreihentyp).
#[test]
fn the_alocat_fixture_correlates_by_its_published_tuple() {
    use dvgw_edi::Zuordnung;

    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    let key = msg
        .correlation_key()
        .expect("70001 has a published Zuordnung");
    assert_eq!(key.zuordnung, Zuordnung::ZoT3);
    // Bilanzkreis from NAD+ZEU, Zeitreihentyp from LIN C212. The fixture carries
    // NAD+ZSZ (Netzkonto) rather than NAD+ZSH, so that slot is empty — and the
    // key says so rather than shifting the Zeitreihentyp into its place.
    assert_eq!(
        key.elements,
        vec![
            "THE0BFH123456789".to_owned(),
            String::new(),
            "Z01".to_owned()
        ]
    );
    assert!(!key.is_complete());
    assert_eq!(key.to_string(), "ZO-T3|THE0BFH123456789||Z01");
    assert!(!key.zuordnung.assigns_to_geschaeftsvorfall());
}

/// A NOMINT and the NOMRES answering it must land on the same key.
///
/// They have to: a NOMRES carries one `RFF`, and it is the Prüfidentifikator.
/// There is no reference back to the nomination, so the business key is the only
/// thing that pairs them.
#[test]
fn a_nomint_and_its_nomres_share_a_correlation_key() {
    use dvgw_edi::Zuordnung;

    let gas_day = dvgw_edi::DvgwPeriod {
        start: datetime!(2026-03-01 05:00 UTC),
        end: datetime!(2026-03-02 05:00 UTC),
    };
    let build = |doc: DvgwDocument, pid: u32, qualifier: &str| {
        MessageBuilder::new(doc)
            .document_number("DOC1")
            .version("DVGW17")
            .pruefidentifikator(pid)
            .message_datetime(datetime!(2026-02-28 20:56 UTC))
            .validity_period(gas_day)
            .sender("A")
            .receiver("B")
            .position(
                Position::new()
                    .location("Z19", Some("37Z005053MH0000D"))
                    .quantity(qualifier, "100", gas_day)
                    .party(nad::BILANZKREIS_INTERN, "THE0BFH000000001")
                    .party(nad::BILANZKREIS_EXTERN, "THE0BFH000000002"),
            )
            .build()
            .expect("builds")
    };

    let platform = DvgwPlatform::default();
    let nomint = platform
        .parse(&build(
            DvgwDocument::NominierungVirtuellerHandelspunkt,
            70_031,
            "Z02",
        ))
        .unwrap();
    let nomres = platform
        .parse(&build(DvgwDocument::VhpBestaetigung, 70_038, "Z02"))
        .unwrap();

    let nomint_key = nomint.correlation_key().unwrap();
    let nomres_key = nomres.correlation_key().unwrap();
    assert_eq!(nomint_key.zuordnung, Zuordnung::Nominierung);
    assert_eq!(
        nomint_key, nomres_key,
        "the answer must reach the nomination it answers"
    );
    assert!(nomint_key.is_complete());
    assert_eq!(
        nomint_key.to_string(),
        "Nominierung|2026-03-01|37Z005053MH0000D|THE0BFH000000001|THE0BFH000000002"
    );
}

/// A clearing message assigns to an open Geschäftsvorfall by Clearingnummer —
/// not to the allocation stream it corrects.
#[test]
fn a_clearing_message_correlates_by_its_clearingnummer() {
    use dvgw_edi::Zuordnung;

    let clearing = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("RFF+Z13:70001", "RFF+Z13:70008");
    let msg = DvgwPlatform::default().parse(clearing.as_bytes()).unwrap();
    let key = msg.correlation_key().unwrap();
    assert_eq!(key.zuordnung, Zuordnung::ZgT1);
    assert!(key.zuordnung.assigns_to_geschaeftsvorfall());
    assert_eq!(key.elements, vec!["CLEARINGNUMMER".to_owned()]);
    assert!(key.is_complete());
}

/// A message whose Prüfidentifikator has no published Zuordnung has no defined
/// way to reach a process, and must say so rather than be attached to a guess.
#[test]
fn an_unassigned_pid_yields_no_correlation_key() {
    let unassigned = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("RFF+Z13:70001", "RFF+Z13:70500");
    let msg = DvgwPlatform::default()
        .parse(unassigned.as_bytes())
        .unwrap();
    assert_eq!(msg.correlation_key(), None);
}

#[test]
fn the_gas_day_accessor_reads_dtm_z01() {
    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    assert_eq!(msg.gas_day(), Some(time::macros::date!(2018 - 01 - 01)));
}

// ── Energy aggregation ───────────────────────────────────────────────────────

/// A `QTY` is a rate, so a profile's energy is the integral, not the sum.
///
/// Adding 100 + 200 + 300 kWh/h yields 600 in no unit at all. Integrating three
/// one-hour steps yields 600 kWh — the same number here only because the periods
/// are one hour each, which is exactly why the mistake survives casual testing.
#[test]
fn energy_is_the_integral_of_the_rate_over_its_period() {
    let profile = b"UNB+UNOA:3+A:502+B:502+260301:0500+IC3'\
UNH+1+ORDERS:D:07A:UN:DVGW17'\
BGM+55G::332+NOMINT77'\
DTM+Z05:0:805'\
DTM+137:202603010400:203'\
DTM+Z01:202603010500202603010800:719'\
RFF+Z13:70031'\
NAD+MS+A::332'\
NAD+MR+B::332'\
LIN+1'\
LOC+Z19+P::332'\
DTM+2:202603010500202603010600:719'\
QTY+Z02:100:KW1'\
DTM+2:202603010600202603010800:719'\
QTY+Z02:200:KW1'\
NAD+ZEU+BK1::332'\
NAD+ZES+BK2::332'\
UNS+S'\
UNT+17+1'\
UNZ+1+IC3'";

    let msg = DvgwPlatform::default().parse(profile).unwrap();
    let totals = msg.energy_by_qualifier();
    // 100 kWh/h for one hour + 200 kWh/h for *two* hours = 500 kWh.
    // The naive sum of the values would be 300.
    assert_eq!(
        totals.get("Z02").map(ToString::to_string),
        Some("500".to_owned())
    );
    assert!(msg.energy_is_complete());
}

/// Directions are kept apart: a total across `Z02` and `Z03` is a net position
/// wearing a total's clothes.
#[test]
fn entry_and_exit_quantities_are_totalled_separately() {
    let both = b"UNB+UNOA:3+A:502+B:502+260301:0500+IC4'\
UNH+1+ORDERS:D:07A:UN:DVGW17'\
BGM+55G::332+NOMINT78'\
DTM+Z05:0:805'\
DTM+137:202603010400:203'\
DTM+Z01:202603010500202603020500:719'\
RFF+Z13:70031'\
NAD+MS+A::332'\
NAD+MR+B::332'\
LIN+1'\
LOC+Z19+P::332'\
DTM+2:202603010500202603020500:719'\
QTY+Z02:100:KW1'\
NAD+ZEU+BK1::332'\
NAD+ZES+BK2::332'\
LIN+2'\
LOC+Z19+P::332'\
DTM+2:202603010500202603020500:719'\
QTY+Z03:20:KW1'\
NAD+ZEU+BK1::332'\
NAD+ZES+BK3::332'\
UNS+S'\
UNT+21+1'\
UNZ+1+IC4'";

    let msg = DvgwPlatform::default().parse(both).unwrap();
    let totals = msg.energy_by_qualifier();
    assert_eq!(totals.len(), 2, "the two directions must not be merged");
    assert_eq!(
        totals.get("Z02").map(ToString::to_string),
        Some("2400".to_owned())
    );
    assert_eq!(
        totals.get("Z03").map(ToString::to_string),
        Some("480".to_owned())
    );
}

/// A quantity that cannot be integrated is omitted, and the message says so.
#[test]
fn an_unconvertible_quantity_makes_the_total_a_floor() {
    let wrong_unit = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("QTY+Z03:4000:KW1", "QTY+Z03:4000:KWH");
    let msg = DvgwPlatform::default()
        .parse(wrong_unit.as_bytes())
        .unwrap();
    assert!(
        msg.energy_by_qualifier().is_empty(),
        "an unknown unit must not be assumed to be a rate"
    );
    assert!(
        !msg.energy_is_complete(),
        "and the caller must be able to tell the total is incomplete"
    );
}

/// The ALOCAT fixture: 4000 kWh/h across a 24-hour gas day.
#[test]
fn the_alocat_fixture_integrates_over_its_gas_day() {
    let msg = DvgwPlatform::default().parse(ALOCAT).unwrap();
    assert_eq!(
        msg.energy_by_qualifier()
            .get("Z03")
            .map(ToString::to_string),
        Some("96000".to_owned())
    );
    assert!(msg.energy_is_complete());
}

/// The sniff must not require a whole interchange to be tokenised.
///
/// It is the first thing every ingest path runs, on every request, including the
/// BDEW ones it will decline. An interchange far past any sane message limit must
/// still be declined promptly rather than tokenised to find out.
#[test]
fn sniffing_reads_only_the_head_of_the_interchange() {
    // A BDEW interchange whose first message is followed by an enormous tail.
    let mut huge = String::from(
        "UNB+UNOC:3+A:500+B:500+260804:1045+REF1'\
UNH+1+ORDRSP:D:07B:UN:1.1c'BGM+231+19110'",
    );
    for i in 0..200_000 {
        huge.push_str(&format!("FTX+ZZZ+++filler-{i}'"));
    }
    huge.push_str("UNT+6+1'UNZ+1+REF1'");

    let started = std::time::Instant::now();
    assert_eq!(dvgw_edi::sniff(huge.as_bytes()), None);
    // Generous by orders of magnitude: the point is that this is not linear in
    // the interchange, which at this size would be plainly visible.
    assert!(
        started.elapsed() < std::time::Duration::from_millis(250),
        "the sniff appears to tokenise the whole interchange: {:?}",
        started.elapsed()
    );

    // And a DVGW interchange with the same tail is still recognised, because
    // `BGM` is where it always is.
    let dvgw_huge = huge.replace("BGM+231+19110'", "BGM+X1G::332+ALOCAT1'");
    assert_eq!(
        dvgw_edi::sniff(dvgw_huge.as_bytes()),
        Some(DvgwDocument::AllokationSlp)
    );
}

/// A Menge without its period is refused, not silently unconvertible.
///
/// A `QTY` is a rate, so a quantity with no `DTM+2` means nothing — and
/// defaulting it to the message's `DTM+Z01` would multiply one hour's rate
/// across the whole gas day. The Segmentlayout marks the period `R`, so its
/// absence is reportable rather than something to work around.
#[test]
fn a_quantity_without_its_period_is_an_error() {
    let no_period = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("DTM+2:201801010500201801020500:719'", "");
    let report = DvgwPlatform::default()
        .validate(no_period.as_bytes())
        .unwrap();
    assert!(!report.is_valid());
    assert!(
        report
            .errors()
            .any(|i| i.rule_id == Some("DVGW-DTM-2-REQUIRED")),
        "the missing period must be named: {:#?}",
        report.errors().map(ToString::to_string).collect::<Vec<_>>()
    );

    // And it must not be conjured from the Gültigkeitszeitraum.
    let msg = DvgwPlatform::default().parse(no_period.as_bytes()).unwrap();
    assert_eq!(msg.quantities().next().unwrap().period, None);
    assert!(msg.energy_by_qualifier().is_empty());
    assert!(!msg.energy_is_complete());
}
