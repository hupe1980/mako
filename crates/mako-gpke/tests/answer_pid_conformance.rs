//! The GPKE answer-PID mapping must agree with the shared AHB table.
//!
//! `mako-gpke` keeps its own `response_pid_for` because the domain crates
//! deliberately do not depend on `edi-energy` in production — they use only
//! `mako-engine`'s `ProfileRequirement` (see the dev-dependency comment in
//! `Cargo.toml`). That layering means the AHB triples exist in two places, so
//! this test is what keeps them from drifting.
//!
//! `edi_energy::answer_pids` is also what `makotest` binds, so a divergence
//! here means a simulated counterparty would answer with a PID this workflow
//! rejects — the exact failure the shared table exists to prevent.

use mako_gpke::wechselprozesse::{GpkeSupplierChangeWorkflow, SupplierChangeCommand};

/// Drive the workflow far enough to observe the response PID it derives.
fn response_pid(anfrage: u32, accepted: bool) -> Option<u32> {
    use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
    use mako_engine::workflow::Workflow;

    let pid = Pruefidentifikator::new(anfrage).ok()?;
    let state = GpkeSupplierChangeWorkflow::apply(
        Default::default(),
        &mako_gpke::wechselprozesse::SupplierChangeEvent::Initiated {
            pruefidentifikator: pid,
            location_id: MaLo::new("10001234558"),
            new_supplier: MarktpartnerCode::new("4012345000009"),
            grid_operator: MarktpartnerCode::new("9900123456789"),
            document_date: "20261001".to_owned(),
            process_date: "20261001".to_owned(),
            transaktionsgrund: None,
            message_ref: MessageRef::new("MSG-1"),
        },
    );
    let state = GpkeSupplierChangeWorkflow::apply(
        state,
        &mako_gpke::wechselprozesse::SupplierChangeEvent::ValidationPassed {
            message_ref: MessageRef::new("MSG-1"),
        },
    );
    let out = GpkeSupplierChangeWorkflow::handle(
        &state,
        SupplierChangeCommand::SendAntwort {
            antwort: nb_antwort(accepted),
            obligations: vec![],
            lfa_lieferende: None,
        },
    )
    .ok()?;

    match out.events.first()? {
        mako_gpke::wechselprozesse::SupplierChangeEvent::AntwortGesendet {
            response_pid, ..
        } => response_pid.map(|p| p.as_u32()),
        _ => None,
    }
}

#[test]
fn gpke_response_pids_match_the_shared_ahb_table() {
    // Every Strom request PID the shared table knows must round-trip through
    // the workflow to the same Bestätigung / Ablehnung.
    let mut checked = 0;
    for anfrage in 55000u32..=55999 {
        let Some((bestaetigung, ablehnung)) = edi_energy::answer_pids(anfrage) else {
            continue;
        };
        assert_eq!(
            response_pid(anfrage, true),
            Some(bestaetigung),
            "PID {anfrage}: workflow Bestätigung disagrees with edi_energy::answer_pids"
        );
        assert_eq!(
            response_pid(anfrage, false),
            Some(ablehnung),
            "PID {anfrage}: workflow Ablehnung disagrees with edi_energy::answer_pids"
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected the GPKE Strom triples, checked {checked}"
    );
}

/// The shared table must not claim answers for a PID the workflow rejects.
#[test]
fn the_shared_table_covers_exactly_the_workflows_request_pids() {
    for anfrage in 55000u32..=55999 {
        let workflow_answers = response_pid(anfrage, true).is_some();
        let table_answers = edi_energy::bestaetigung_pid(anfrage).is_some();
        assert_eq!(
            workflow_answers, table_answers,
            "PID {anfrage}: workflow answers={workflow_answers} but shared \
             table answers={table_answers} — one of them is wrong"
        );
    }
}

/// The NB's answer code for a Lieferbeginn — `A51` (`E_0623`) or `A07`
/// (`E_0622`). `SG4 STS+E01` is Muss, so there is no codeless answer.
fn nb_antwort(accepted: bool) -> mako_gpke::LfAntwort {
    let (code, ebd) = if accepted {
        ("A51", "E_0623")
    } else {
        ("A07", "E_0622")
    };
    mako_gpke::LfAntwort {
        antwort_code: code.to_owned(),
        ebd: Some(ebd.to_owned()),
        zustimmung: accepted,
        bemerkung: None,
        bilanzkreis: None,
        termin: None,
    }
}
