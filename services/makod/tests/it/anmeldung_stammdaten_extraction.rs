//! Guard: the facts a Netzbetreiber decides an Anmeldung on reach the command.
//!
//! `processd`'s NB check 4 compares the Bilanzierungsgebiet the LFN stated
//! against the one the Netzbetreiber holds. When the adapter drops the stated
//! value the check has nothing to compare and passes vacuously, so a mismatch
//! that ought to refuse the Anmeldung confirms it instead — and nothing in the
//! ordinary machinery says so, because a `None` is a legitimate state for a
//! message that genuinely omits the segment.
//!
//! `SG10 CCI+Z20` is where UTILMD Strom states it (MIG Strom Nr 00123,
//! „Bilanzierungsgebiet, in dem die Marktlokation liegt"): the EIC rides in
//! DE 7037 itself and the characteristic has no `CAV`.

use std::any::Any;

use edi_energy::Platform;
use mako_engine::version::FormatVersion;
use mako_gpke::SupplierChangeCommand;
use makod::adapters::gpke_registry;

/// A minimal but conformant UTILMD 55001 Anmeldung.
///
/// `{cci}` is spliced in so the two cases differ in exactly the segment under
/// test. The Bilanzierungsgebiet is Amprion's published Regelzonen-EIC, which
/// carries a valid ENTSO-E check character.
const ANMELDUNG: &str = "\
UNB+UNOC:3+4012345000023:14+9900357000004:500+230101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:202301010000?+00:303'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
IDE+24+VORGANG-0001'\
LOC+Z16+51238696781'\
{cci}\
UNT+9+1'\
UNZ+1+1'";

fn adapt(cci: &str) -> SupplierChangeCommand {
    let wire = ANMELDUNG.replace("{cci}", cci);
    let raw = Platform::with_all_profiles()
        .parse(wire.as_bytes())
        .expect("the fixture parses");
    gpke_registry()
        .dispatch(&raw as &dyn Any, &FormatVersion::new("FV2025-10-01"))
        .expect("the Anmeldung adapts")
}

/// The stated Bilanzierungsgebiet travels from `CCI+Z20` into the command the
/// NB decides on.
#[test]
fn the_anmeldung_carries_its_bilanzierungsgebiet() {
    let SupplierChangeCommand::ReceiveUtilmd {
        bilanzierungsgebiet,
        ..
    } = adapt("CCI+Z20++10YDE-RWENET---I'")
    else {
        panic!("expected ReceiveUtilmd");
    };
    assert_eq!(bilanzierungsgebiet.as_deref(), Some("10YDE-RWENET---I"));
}

/// A message that states none says `None` — check 4 is skipped, not decided on
/// a fabricated value.
#[test]
fn an_anmeldung_without_the_segment_states_nothing() {
    let SupplierChangeCommand::ReceiveUtilmd {
        bilanzierungsgebiet,
        ..
    } = adapt("")
    else {
        panic!("expected ReceiveUtilmd");
    };
    assert_eq!(bilanzierungsgebiet, None);
}

/// `LOC+Z20` is a Technische Ressource. DE 7059 `Z20` and DE 3227 `Z20` are one
/// segment apart and mean unrelated things, so a Technische Ressource must not
/// be read as the Bilanzierungsgebiet.
#[test]
fn a_technische_ressource_is_not_a_bilanzierungsgebiet() {
    let SupplierChangeCommand::ReceiveUtilmd {
        bilanzierungsgebiet,
        ..
    } = adapt("LOC+Z20+D12345678901'")
    else {
        panic!("expected ReceiveUtilmd");
    };
    assert_eq!(bilanzierungsgebiet, None);
}

// ── The Lieferbeginn ─────────────────────────────────────────────────────────

/// The Anmeldung's `SG4 DTM+92` „Beginn zum" reaches the command as the
/// German date it names.
///
/// An empty `process_date` is not a validation error anywhere: `processd`'s
/// `AnmeldungPayload::parse` returns `None`, and the caller reads that as "not
/// addressed to this module". The Anmeldung is then never evaluated, silently.
///
/// The value on the wire is the UTC instant at which the German day begins
/// (Allgemeine Festlegungen V6.1d Kap. 3.1): a Lieferbeginn on 01.01.2027 is
/// `202612312300+00`, whose first eight digits are the day before. The
/// command carries the German date.
///
/// The `e2e_lieferbeginn` suite cannot catch it — it submits through the REST
/// command API, where the ERP supplies `process_date` directly.
#[test]
fn the_anmeldung_carries_its_lieferbeginn() {
    for (dtm, expected) in [
        ("DTM+92:202612312300?+00:303'", "20270101"),
        ("DTM+92:202706302200?+00:303'", "20270701"),
    ] {
        let cmd = adapt(dtm);
        let SupplierChangeCommand::ReceiveUtilmd { process_date, .. } = cmd else {
            panic!("expected ReceiveUtilmd");
        };
        assert_eq!(
            process_date, expected,
            "{dtm}: the Lieferbeginn is the German day that instant begins"
        );
    }
}

/// UTC midnight begins no German day in Strom — it is 01:00 or 02:00 Berlin —
/// so the adapter refuses the date instead of reading its first eight digits.
#[test]
fn a_lieferbeginn_at_utc_midnight_is_refused() {
    let wire = ANMELDUNG.replace("{cci}", "DTM+92:202610010000?+00:303'");
    let raw = Platform::with_all_profiles()
        .parse(wire.as_bytes())
        .expect("the fixture parses");
    let Err(err) = gpke_registry().dispatch(&raw as &dyn Any, &FormatVersion::new("FV2025-10-01"))
    else {
        panic!("UTC midnight names no Strom day");
    };
    assert!(err.to_string().contains("DTM+92"), "{err}");
}

/// The Gas Anmeldung's `DTM+92` is the Gastag's 06:00 — `0400` UTC in summer
/// — and reads back to its own date.
#[test]
fn the_gas_anmeldung_carries_its_lieferbeginn() {
    let wire = "\
UNB+UNOC:3+9870000000006:502+9870000000013:502+270901:0800+GAS-1'\
UNH+1+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044001::+9'\
DTM+137:202709010800?+00:303'\
RFF+Z13:44001'\
NAD+MS+9870000000006::332'\
IDE+24+VORGANG-0001'\
DTM+92:202710010400?+00:303'\
LOC+172+51238696012'\
UNT+9+1'\
UNZ+1+GAS-1'";
    let raw = Platform::with_all_profiles()
        .parse(wire.as_bytes())
        .expect("the fixture parses");
    let cmd = makod::adapters::geli_gas_registry(time::OffsetDateTime::UNIX_EPOCH)
        .dispatch(&raw as &dyn Any, &FormatVersion::new("FV2026-04-01"))
        .expect("the Gas Anmeldung adapts");
    let mako_geli_gas::GasSupplierChangeCommand::ReceiveUtilmd { process_date, .. } = cmd else {
        panic!("expected ReceiveUtilmd");
    };
    assert_eq!(process_date, "20271001");
}

/// `163` is not a UTILMD SG4 date, and reading one must not produce a
/// Lieferbeginn.
///
/// The assertion is the inverse of the one above: it fails if somebody
/// reinstates `is_period_start()`, which would make both this and the previous
/// test pass only by accident of the fixture.
#[test]
fn a_processing_start_date_is_not_a_lieferbeginn() {
    let cmd = adapt("DTM+163:202610010000?+00:303'");
    let SupplierChangeCommand::ReceiveUtilmd { process_date, .. } = cmd else {
        panic!("expected ReceiveUtilmd");
    };
    assert_eq!(
        process_date, "",
        "DE 2005 `163` is an MSCONS qualifier; no UTILMD Anwendungsfall states \
         the Lieferbeginn with it"
    );
}
