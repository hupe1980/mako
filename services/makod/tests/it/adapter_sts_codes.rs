//! An adapter reads an Antwortcode where the MIG puts it.
//!
//! A UTILMD `STS+E01++<code>:<Codeliste>` carries the code in `C556` DE 9013 —
//! `STS` DE 4405 is empty on every conformant message — and an ORDRSP carries
//! no `STS` at all: its Antwortcode is `SG2 AJT` (ORDRSP MIG, Nr 00015).
//! `cargo xtask check-sts-codes` refuses the source shape; these hold the two
//! adapters that read the wrong place to the code a conformant message states.

use std::any::Any;

use mako_engine::version::FormatVersion;

/// A GPKE Rückmeldung 55622 on an Änderung, `A02` „übernommen mit Korrektur"
/// out of `E_0408`.
const RUECKMELDUNG: &str = "UNB+UNOC:3+9900000000001:500+9900357000004:500+261101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.2'\
BGM+E03+RM-0001'\
DTM+137:202611010000?+00:303'\
NAD+MS+9900000000001::293'\
NAD+MR+9900357000004::293'\
IDE+24+RM-V-0001'\
STS+7++ZX6+ZW4'\
STS+E01++A02:E_0408'\
LOC+Z16+51238696012'\
RFF+Z13:55622'\
UNT+12+1'\
UNZ+1+1'";

/// A MaBiS Ablehnung 19204 with its `SG2 AJT` code out of `E_0003`.
const ABLEHNUNG_19204: &str = "UNB+UNOC:3+4012345000023:14+9900357000004:500+261101:0000+1'\
UNH+1+ORDRSP:D:10A:UN:1.4c'\
BGM+7+AB-19204'\
DTM+137:202611010000?+00:303'\
RFF+Z13:19204'\
RFF+ON:ORD-0001'\
AJT+A02+E_0003'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+10+1'\
UNZ+1+1'";

#[test]
fn a_gpke_rueckmeldung_reads_its_code_from_c556() {
    let msg = edi_energy::parse(RUECKMELDUNG.as_bytes()).expect("a Rückmeldung parses");
    let cmd = makod::adapters::gpke_stammdaten_registry(time::OffsetDateTime::UNIX_EPOCH)
        .dispatch(&msg as &dyn Any, &FormatVersion::new("FV2026-10-01"))
        .expect("the Rückmeldung adapter reads it");
    match cmd {
        mako_gpke::StammdatenCommand::ReceiveRueckmeldung { qualitaet, .. } => assert_eq!(
            qualitaet,
            mako_gpke::Qualitaet::UebernommenMitKorrektur,
            "STS+E01++A02 is a Rückmeldung mit Korrektur"
        ),
        _ => panic!("expected ReceiveRueckmeldung"),
    }
}

#[test]
fn a_mabis_ablehnung_reads_its_code_from_ajt() {
    let msg = edi_energy::parse(ABLEHNUNG_19204.as_bytes()).expect("a 19204 parses");
    let cmd = makod::adapters::mabis_anforderung_ablehnung_registry()
        .dispatch(&msg as &dyn Any, &FormatVersion::new("FV2026-10-01"))
        .expect("the 19204 adapter reads its AJT");
    match cmd {
        mako_mabis::AnforderungCommand::ReceiveAblehnung { code, ebd, .. } => {
            assert_eq!((code.as_str(), ebd.as_str()), ("A02", "E_0003"));
        }
        _ => panic!("expected ReceiveAblehnung"),
    }
}
