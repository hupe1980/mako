//! Tests for `GpkeKonfigurationWorkflow` (ORDERS 17134 — GPKE Teil 3).
//!
//! Covers:
//! - `NbSendsBeauftragung` refused by name: the 17134 column's facts have no
//!   source
//! - Wrong PID guard

use mako_engine::{
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::Workflow,
};
use mako_gpke::{GpkeKonfigurationWorkflow, KonfigurationCommand, KonfigurationState};

fn beauftragung_cmd() -> KonfigurationCommand {
    KonfigurationCommand::NbSendsBeauftragung {
        orders_pid: Pruefidentifikator::new(17134).unwrap(),
        msb_mp_id: MarktpartnerCode::new("9904357000003"),
        malo: MaLo::new("51238696781"),
        new_supplier: MarktpartnerCode::new("4012345000023"),
        message_ref: MessageRef::new("ORD-2025-001"),
    }
}

// ── The 17134 is refused before it is queued ─────────────────────────────

/// The 17134 column (ORDERS AHB 1.1b Kap. 4.17.1, S. 101–110) makes the
/// Lieferrichtung, the Lieferant and the MSB of the Marktlokation and, per
/// Lokation, the erforderlichen Produkte with their Zählzeitdefinition Muss —
/// facts the NB holds no source for. The workflow refuses by name rather than
/// queue an ORDERS the renderer and the pre-send gate would refuse.
#[test]
fn the_17134_is_refused_naming_the_facts_it_lacks() {
    let err = GpkeKonfigurationWorkflow::handle(&KonfigurationState::New, beauftragung_cmd())
        .expect_err("no 17134 is queued without its column's facts");
    let text = err.to_string();
    assert!(text.contains("Kap. 4.17.1"), "{text}");
    assert!(text.contains("Produkt"), "{text}");
    assert!(text.contains("Zählzeitdefinition"), "{text}");
}

/// An ORDERS other than the NB's 17134 is rejected.
#[test]
fn wrong_orders_pid_returns_error() {
    let state = KonfigurationState::New;
    let result = GpkeKonfigurationWorkflow::handle(
        &state,
        KonfigurationCommand::NbSendsBeauftragung {
            orders_pid: Pruefidentifikator::new(17001).unwrap(), // WiM Geräteübernahme — wrong
            msb_mp_id: MarktpartnerCode::new("9904357000003"),
            malo: MaLo::new("51238696781"),
            new_supplier: MarktpartnerCode::new("4012345000023"),
            message_ref: MessageRef::new("ORD-WRONG"),
        },
    );
    assert!(
        result.is_err(),
        "wrong PID must produce WorkflowError::Rejected"
    );
}
