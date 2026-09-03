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
//! - DVGW-Nachrichtenbeschreibung SSQNOT 5.7 (ORDRSP / UN D.07A S3), §3.2
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

/// SSQNOT 5.7 §3.2 — the segment examples of the Segmentlayout, wrapped in an
/// interchange. The Absender the specification prints has twelve digits; it is
/// carried as printed.
const SSQNOT: &[u8] = b"UNB+UNOA:3+987004760000:502+9870112500011:502+180104:2056+IC5'\
UNH+123456+ORDRSP:D:07A:UN:DVGW17'\
BGM+BAG::332+SSQNOT00052'\
DTM+Z05:0:805'\
DTM+137:201801042056:203'\
DTM+Z01:201801010500201802010500:719'\
RFF+Z13:70095'\
NAD+MS+987004760000::332'\
NAD+MR+9870112500011::332'\
LIN+1'\
LOC+Z99'\
DTM+2:201801010500201802010500:719'\
QTY+ZY2:6782:KWH'\
STS+A1G::332'\
NAD+ZSH+NETZKONTONR::332'\
LIN+2'\
LOC+Z99'\
DTM+2:201801010500201802010500:719'\
QTY+ZY0:120:KWH'\
STS+A1G::332'\
NAD+ZSH+NETZKONTONR::332'\
UNS+S'\
UNT+22+123456'\
UNZ+1+IC5'";

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

    let ssqnot = platform.parse(SSQNOT).expect("SSQNOT must parse");
    assert_eq!(ssqnot.message_type, DvgwMessageType::Ssqnot);
    assert_eq!(ssqnot.document, DvgwDocument::MehrMindermengenmeldung);
    assert_eq!(ssqnot.carrier.as_str(), "ORDRSP");
    assert_eq!(ssqnot.pruefidentifikator.unwrap().as_u32(), 70_095);
}

// ── SSQNOT ───────────────────────────────────────────────────────────────────

/// The Mehr-/Mindermengenmeldung reads as one record: Netzkonto, Zeitraum,
/// Verfahren and the two energies in kWh — not rates, so no integration.
#[test]
fn a_ssqnot_reads_as_one_mehr_mindermengen_record() {
    use dvgw_edi::ssqnot::{MehrMindermengenmeldung, Verfahren};

    let msg = DvgwPlatform::default().parse(SSQNOT).unwrap();
    let record = MehrMindermengenmeldung::from_message(&msg).expect("a clean SSQNOT reads");
    assert_eq!(record.netzkonto, "NETZKONTONR");
    assert_eq!(record.netzbetreiber, "987004760000");
    assert_eq!(record.verfahren, Verfahren::Slp);
    assert_eq!(record.zeitraum.start, datetime!(2018-01-01 05:00 UTC));
    assert_eq!(record.zeitraum.end, datetime!(2018-02-01 05:00 UTC));
    assert_eq!(record.mehrmenge_kwh.to_string(), "120");
    assert_eq!(record.mindermenge_kwh.to_string(), "6782");
    assert_eq!(record.saldo_kwh().to_string(), "-6662");

    // `KWH` is an energy: a month-long period must not multiply it.
    let totals = msg.energy_by_qualifier();
    assert_eq!(totals["ZY2"].to_string(), "6782");
    assert_eq!(totals["ZY0"].to_string(), "120");
}

/// SSQNOT 5.7 §3.3: the 2-Tupel (Netzkonto, Netzbetreiber) assigns the message,
/// and the process is one Abrechnungszeitraum of that Netzkonto.
#[test]
fn a_ssqnot_correlates_by_netzkonto_and_netzbetreiber() {
    use dvgw_edi::Zuordnung;

    let msg = DvgwPlatform::default().parse(SSQNOT).unwrap();
    let key = msg
        .correlation_key()
        .expect("70095 has a published Zuordnung");
    assert_eq!(key.zuordnung, Zuordnung::MehrMindermengen);
    assert_eq!(key.to_string(), "ZO-T1:SSQNOT|NETZKONTONR|987004760000");
    assert_eq!(
        msg.process_key().as_deref(),
        Some("ZO-T1:SSQNOT|NETZKONTONR|987004760000|2018-01-01..2018-02-01")
    );
}

/// The Segmentlayout rows SSQNOT adds: the Verfahren is Muss, the Menge is a
/// natural number in kWh, and the RLM Anwendungsfall is retired.
#[test]
fn a_ssqnot_is_held_to_its_own_rows() {
    let platform = DvgwPlatform::default();
    let clean = platform.validate(SSQNOT).unwrap();
    assert!(clean.is_valid(), "{:?}", clean.errors().collect::<Vec<_>>());
    assert_eq!(
        clean.warnings().count(),
        0,
        "{:?}",
        clean.warnings().collect::<Vec<_>>()
    );

    let no_sts = String::from_utf8(SSQNOT.to_vec())
        .unwrap()
        .replacen("STS+A1G::332'", "", 1);
    let report = platform.validate(no_sts.as_bytes()).unwrap();
    assert!(
        report
            .errors()
            .any(|i| i.rule_id == Some("DVGW-STS-REQUIRED"))
    );

    let fraction = String::from_utf8(SSQNOT.to_vec())
        .unwrap()
        .replace("QTY+ZY2:6782:KWH", "QTY+ZY2:6782.5:KWH");
    let report = platform.validate(fraction.as_bytes()).unwrap();
    assert!(
        report
            .warnings()
            .any(|i| i.rule_id == Some("DVGW-QTY-INTEGER"))
    );

    let rlm = String::from_utf8(SSQNOT.to_vec())
        .unwrap()
        .replace("RFF+Z13:70095", "RFF+Z13:70096")
        .replace("STS+A1G", "STS+A2G");
    let report = platform.validate(rlm.as_bytes()).unwrap();
    assert!(
        report
            .warnings()
            .filter(|i| i.rule_id == Some("DVGW-PID-RETIRED"))
            .count()
            >= 2,
        "70096 and STS+A2G are both retired for a 2018 Zeitraum"
    );
}

/// A Netzbetreiber writes the SSQNOT the MGV expects, and reads it back.
#[test]
fn a_built_ssqnot_parses_back_and_validates() {
    use dvgw_edi::model::{qty, sts};

    let zeitraum = dvgw_edi::DvgwPeriod {
        start: datetime!(2026-03-01 05:00 UTC),
        end: datetime!(2026-04-01 05:00 UTC),
    };
    let wire = MessageBuilder::new(DvgwDocument::MehrMindermengenmeldung)
        .document_number("SSQNOT00052")
        .pruefidentifikator(70_095)
        .message_datetime(datetime!(2026-04-10 09:00 UTC))
        .validity_period(zeitraum)
        .sender("9870012345678")
        .receiver("9800505300009")
        .position(
            Position::new()
                .location("Z99", None)
                .quantity(qty::MEHRMENGE, "120", zeitraum)
                .status(sts::SLP)
                .party(nad::NETZKONTO_ZO_T3, "THE0NKH712345678"),
        )
        .position(
            Position::new()
                .location("Z99", None)
                .quantity(qty::MINDERMENGE, "6782", zeitraum)
                .status(sts::SLP)
                .party(nad::NETZKONTO_ZO_T3, "THE0NKH712345678"),
        )
        .build()
        .expect("builds");
    let rendered = String::from_utf8(wire.clone()).unwrap();
    assert!(
        rendered.starts_with("UNH+1+ORDRSP:D:07A:UN:DVGW17'"),
        "{rendered}"
    );
    assert!(rendered.contains("BGM+BAG::332+SSQNOT00052'"), "{rendered}");
    assert!(
        rendered.contains("QTY+ZY0:120:KWH'STS+A1G::332'"),
        "{rendered}"
    );

    let report = DvgwPlatform::default().validate(&wire).unwrap();
    assert!(
        report.is_valid(),
        "{:?}",
        report.errors().collect::<Vec<_>>()
    );
    let record = dvgw_edi::ssqnot::MehrMindermengenmeldung::from_message(
        &DvgwPlatform::default().parse(&wire).unwrap(),
    )
    .unwrap();
    assert_eq!(record.saldo_kwh().to_string(), "-6662");
}

// ── Rows that depend on the Anwendungsfall ───────────────────────────────────

/// `RFF+ANX` is a `D` group: only the six Allokationsclearing columns mark it
/// Muss. A SLP-Allokation without one is conformant.
#[test]
fn the_clearingnummer_is_required_by_the_clearing_columns_only() {
    let without = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("RFF+ANX:CLEARINGNUMMER'", "");
    let platform = DvgwPlatform::default();
    let report = platform.validate(without.as_bytes()).unwrap();
    assert!(
        report.is_valid(),
        "70001 lists no RFF+ANX: {:?}",
        report.errors().collect::<Vec<_>>()
    );
    let clearing = without.replace("RFF+Z13:70001", "RFF+Z13:70008");
    let report = platform.validate(clearing.as_bytes()).unwrap();
    assert!(report.errors().any(|i| i.rule_id == Some("DVGW-RFF-ANX")));
}

/// The Anwendungsfall fixes the document: a code from another column of the
/// same family is family-consistent and still the wrong business message.
#[test]
fn the_document_must_be_the_one_the_anwendungsfall_publishes() {
    let platform = DvgwPlatform::default();
    // 70001 publishes `X1G` (SLP-Allokation); `X5G` is the endgültige, 70005's.
    let wrong = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("BGM+X1G", "BGM+X5G");
    let report = platform.validate(wrong.as_bytes()).unwrap();
    assert!(
        report
            .errors()
            .any(|i| i.rule_id == Some("DVGW-PID-DOCUMENT")),
        "{:?}",
        report.errors().collect::<Vec<_>>()
    );
    // The same code under its own Prüfidentifikator is conformant.
    let right = wrong.replace("RFF+Z13:70001", "RFF+Z13:70005");
    assert!(platform.validate(right.as_bytes()).unwrap().is_valid());
}

/// NOMINT 4.6 §2: `DTM+9` is Erforderlich beside `RFF+AGO`.
#[test]
fn a_re_nomination_names_when_the_original_was_processed() {
    let msg = DvgwPlatform::default().parse(NOMINT).unwrap();
    assert_eq!(
        msg.original_nomination_datetime,
        Some(datetime!(2018-01-04 20:56 UTC))
    );
    let without = String::from_utf8(NOMINT.to_vec())
        .unwrap()
        .replace("DTM+9:201801042056:203'", "");
    let report = DvgwPlatform::default()
        .validate(without.as_bytes())
        .unwrap();
    assert!(
        report
            .errors()
            .any(|i| i.rule_id == Some("DVGW-RFF-AGO-DTM"))
    );
}

/// The position `NAD` rows follow the family: a NOMRES for a physical point
/// names only the interne Bilanzkreis (`ZES` is `D`), an ALOCAT names both.
#[test]
fn the_position_nad_rows_follow_the_family() {
    let nomres = String::from_utf8(NOMRES_VHP.to_vec())
        .unwrap()
        .replace("NAD+ZES+THE0BFH000000002::332'", "");
    let platform = DvgwPlatform::default();
    let report = platform.validate(nomres.as_bytes()).unwrap();
    assert!(
        report.is_valid(),
        "ZES is dependent on a NOMRES: {:?}",
        report.errors().collect::<Vec<_>>()
    );
    let alocat = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("NAD+ZSZ+THE0NKH712345678::332'", "");
    let report = platform.validate(alocat.as_bytes()).unwrap();
    assert!(report.errors().any(|i| i.rule_id == Some("DVGW-NAD-ITEM")));
}

/// A nomination may state energy (`KWH`); an ALOCAT may state a daily rate
/// (`KW2`). Neither is a foreign unit.
#[test]
fn the_admitted_units_follow_the_family() {
    let platform = DvgwPlatform::default();
    let kwh = String::from_utf8(NOMINT.to_vec())
        .unwrap()
        .replace("QTY+Z03:6782:KW1", "QTY+Z03:6782:KWH");
    let report = platform.validate(kwh.as_bytes()).unwrap();
    assert!(
        !report
            .warnings()
            .any(|i| i.rule_id == Some("DVGW-QTY-UNIT"))
    );
    let msg = platform.parse(kwh.as_bytes()).unwrap();
    assert_eq!(msg.energy_by_qualifier()["Z03"].to_string(), "6782");

    let kw2 = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("QTY+Z03:4000:KW1", "QTY+Z03:4000:KW2");
    let msg = platform.parse(kw2.as_bytes()).unwrap();
    // 4000 kWh/d over one gas day.
    assert_eq!(msg.energy_by_qualifier()["Z03"].to_string(), "4000");
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
            .any(|i| i.rule_id == Some("DVGW-QTY-UNIT") && i.severity == Severity::Warning),
        "an ALOCAT states rates: KWH is not admitted there"
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
        .original_nomination("1234", datetime!(2026-02-28 18:00 UTC))
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
    assert!(rendered.contains("RFF+Z13:70030'RFF+AGO:1234'DTM+9:202602281800:203'"));

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
UNH+1+ORDRSP:D:07B:UN:1.1c'BGM+231+19110'DTM+137:202608041045?+00:303'\
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
    // Bilanzkreis from NAD+ZEU, Zeitreihentyp from the STS under the quantity
    // (§3.3: `SG36 SG37 STS`, not LIN C212, which is always `Z01`). The fixture
    // carries NAD+ZSZ (Netzkonto) rather than NAD+ZSH, so that slot is empty —
    // and the key says so rather than shifting the Zeitreihentyp into its place.
    assert_eq!(
        key.elements,
        vec![
            "THE0BFH123456789".to_owned(),
            String::new(),
            "09G".to_owned()
        ]
    );
    assert!(!key.is_complete());
    assert_eq!(key.to_string(), "ZO-T3|THE0BFH123456789||09G");
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
    // The key already names the gas day, so the process key adds nothing —
    // and a sender that assembles it must land on the same string.
    assert_eq!(nomint.process_key(), Some(nomint_key.to_string()));
    assert_eq!(
        dvgw_edi::CorrelationKey::nominierung(
            time::macros::date!(2026 - 03 - 01),
            "37Z005053MH0000D",
            "THE0BFH000000001",
            "THE0BFH000000002",
        ),
        nomint_key
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
    // `MWH` is a unit no DVGW Segmentlayout lists.
    let wrong_unit = String::from_utf8(ALOCAT.to_vec())
        .unwrap()
        .replace("QTY+Z03:4000:KW1", "QTY+Z03:4000:MWH");
    let msg = DvgwPlatform::default()
        .parse(wrong_unit.as_bytes())
        .unwrap();
    assert!(
        msg.energy_by_qualifier().is_empty(),
        "an unknown unit must not be assumed to be a rate or an energy"
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

/// The DVGW column admits one `DTM+2` and one `QTY` per `LOC` group, so a
/// profile the builder writes repeats the `LOC` — and a profile packed under
/// one `LOC` is read whole and reported.
#[test]
fn a_profile_is_written_as_one_loc_group_per_period() {
    let hour = |h: i64| dvgw_edi::DvgwPeriod {
        start: datetime!(2026-03-01 05:00 UTC) + time::Duration::hours(h),
        end: datetime!(2026-03-01 06:00 UTC) + time::Duration::hours(h),
    };
    let wire = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
        .document_number("NOMINT77")
        .pruefidentifikator(70_030)
        .message_datetime(datetime!(2026-02-28 20:56 UTC))
        .validity_period(dvgw_edi::DvgwPeriod {
            start: hour(0).start,
            end: hour(2).end,
        })
        .sender("A")
        .receiver("B")
        .position(
            Position::new()
                .location("Z19", Some("P"))
                .quantity("Z02", "100", hour(0))
                .quantity("Z02", "200", hour(1))
                .quantity("Z02", "300", hour(2))
                .party(nad::BILANZKREIS_INTERN, "BK1"),
        )
        .build()
        .expect("builds");
    let rendered = String::from_utf8(wire.clone()).unwrap();
    assert_eq!(rendered.matches("LOC+Z19+P::332'").count(), 3, "{rendered}");
    let report = DvgwPlatform::default().validate(&wire).unwrap();
    assert!(report.is_valid());
    assert!(!report.warnings().any(|i| i.rule_id == Some("DVGW-LOC-MAX")));
    assert_eq!(
        DvgwPlatform::default()
            .parse(&wire)
            .unwrap()
            .energy_by_qualifier()["Z02"]
            .to_string(),
        "600"
    );
}
