//! The GeLi Gas answer-PID mapping must agree with the shared AHB table.
//!
//! `mako-geli-gas` keeps its own `response_pid_for` because the domain crates
//! deliberately do not depend on `edi-energy` in production. That layering
//! means the AHB triples exist in two places, so this test is what keeps them
//! from drifting.
//!
//! `edi_energy::answer_pids` is also what `makotest` binds, so a divergence
//! here means a simulated GNB would answer with a PID this workflow rejects.

use mako_geli_gas::lieferbeginn::response_pid_for;

#[test]
fn geli_gas_response_pids_match_the_shared_ahb_table() {
    let mut checked = 0;
    for anfrage in 44000u32..=44999 {
        let Some((bestaetigung, ablehnung)) = edi_energy::answer_pids(anfrage) else {
            continue;
        };
        assert_eq!(
            response_pid_for(anfrage, true).map(|p| p.as_u32()),
            Some(bestaetigung),
            "PID {anfrage}: workflow Bestätigung disagrees with edi_energy::answer_pids"
        );
        assert_eq!(
            response_pid_for(anfrage, false).map(|p| p.as_u32()),
            Some(ablehnung),
            "PID {anfrage}: workflow Ablehnung disagrees with edi_energy::answer_pids"
        );
        checked += 1;
    }
    assert!(
        checked >= 6,
        "expected the GeLi Gas triples in the shared table, checked {checked}"
    );
}

/// The asymmetric families must be asymmetric on both sides.
///
/// 44020 is confirmable (44021) but has no Ablehnung; 44019 has neither. A
/// shared table that flattened these into a symmetric pair would make the
/// simulator offer a rejection the workflow cannot produce.
#[test]
fn asymmetric_families_agree_on_both_sides() {
    assert_eq!(
        response_pid_for(44020, true).map(|p| p.as_u32()),
        edi_energy::bestaetigung_pid(44020),
    );
    assert_eq!(response_pid_for(44020, false), None);
    assert_eq!(edi_energy::ablehnung_pid(44020), None);

    assert_eq!(response_pid_for(44019, true), None);
    assert_eq!(response_pid_for(44019, false), None);
    assert_eq!(edi_energy::bestaetigung_pid(44019), None);
}

/// Every PID the workflow answers must be in the shared table, and vice versa.
#[test]
fn the_shared_table_covers_exactly_the_workflows_request_pids() {
    for anfrage in 44000u32..=44999 {
        assert_eq!(
            response_pid_for(anfrage, true).map(|p| p.as_u32()),
            edi_energy::bestaetigung_pid(anfrage),
            "PID {anfrage}: Bestätigung coverage differs between workflow and table"
        );
        assert_eq!(
            response_pid_for(anfrage, false).map(|p| p.as_u32()),
            edi_energy::ablehnung_pid(anfrage),
            "PID {anfrage}: Ablehnung coverage differs between workflow and table"
        );
    }
}
