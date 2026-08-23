//! Commodity-aware ingest routing, and the Sparte the ingest layer resolves.
//!
//! Two distinct jobs, both keyed on the **interchange recipient's MP-ID**
//! (`UNB` DE 0010, which `edi-energy` pins equal to `NAD+MR` at parse):
//!
//! 1. A Prüfidentifikator that two *different process families* share — ORDERS
//!    17115 Sperrauftrag, answered by GPKE in Strom and GeLi Gas in Gas — must
//!    reach the right workflow, not whichever module registered last.
//! 2. A PID that one family carries in both Sparten — the WiM ORDERS/ORDRSP,
//!    INSRPT and IFTSTA legs — reaches **one** workflow either way, and the
//!    Sparte travels in the command so the workflow can pick the
//!    Entscheidungsbaum and the Codeliste.
//!
//! Guards `EdifactApiState::resolve_workflow` / `edifact_api::resolve_workflow`,
//! which the AS4 and REST ingest paths call.

use mako_engine::pid_router::PidRouter;
use mako_engine::types::Sparte;
use makod::config::PartyConfig;
use makod::edifact_api::resolve_workflow;
use makod::party_registry::MpIdRegistry;

const STROM_NB: &str = "9900001000001";
const GAS_GNB: &str = "9800001000001";
const NEUTRAL_RB: &str = "4012345000023";

fn party(mp_id: &str, role: &str, primary: bool) -> PartyConfig {
    PartyConfig {
        mp_id: mp_id.to_owned(),
        roles: vec![role.to_owned()],
        primary,
        agency: None,
    }
}

/// NB (Strom), GNB (Gas) and RB (Sparte-neutral) own parties.
fn combined_registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[
        party(STROM_NB, "NB", true),
        party(GAS_GNB, "GNB", false),
        party(NEUTRAL_RB, "RB", false),
    ])
    .expect("valid registry")
}

/// Mirror a combined Strom+Gas deployment on the Sperrprozess ORDERS: both
/// modules register 17115 — Gas last, so the unambiguous `route()` fallback
/// resolves to Gas.
fn combined_router() -> PidRouter {
    let mut r = PidRouter::new();
    r.register(17115, "gpke-sperrung");
    r.register_with_sparte(17115, Sparte::Strom, "gpke-sperrung");
    r.register(17115, "geli-gas-sperrung-lf"); // GeliGasModule registered after GpkeModule
    r.register_with_sparte(17115, Sparte::Gas, "geli-gas-sperrung-lf");
    r.register(55001, "gpke-supplier-change"); // non-Sparte-split control PID
    // The WiM legs are one workflow in both Sparten, registered unqualified.
    for pid in [17001_u32, 17009, 23001] {
        r.register(
            pid,
            if pid == 23001 {
                "wim-insrpt"
            } else {
                "wim-geraeteubernahme"
            },
        );
    }
    r
}

#[test]
fn strom_recipient_routes_a_split_pid_to_the_strom_workflow() {
    let (router, reg) = (combined_router(), combined_registry());
    // Without commodity-aware routing, `route(17115)` is the last-write
    // "geli-gas-sperrung-lf" — wrong for a Strom interchange.
    assert_eq!(
        resolve_workflow(&router, &reg, 17115, STROM_NB),
        Some("gpke-sperrung"),
    );
}

#[test]
fn gas_recipient_routes_a_split_pid_to_the_gas_workflow() {
    let (router, reg) = (combined_router(), combined_registry());
    assert_eq!(
        resolve_workflow(&router, &reg, 17115, GAS_GNB),
        Some("geli-gas-sperrung-lf"),
    );
}

#[test]
fn neutral_or_unknown_recipient_falls_back_to_unambiguous_table() {
    let (router, reg) = (combined_router(), combined_registry());
    // Both (RB) and non-own recipients use the unambiguous fallback (Gas, last write).
    assert_eq!(
        resolve_workflow(&router, &reg, 17115, NEUTRAL_RB),
        Some("geli-gas-sperrung-lf"),
    );
    assert_eq!(
        resolve_workflow(&router, &reg, 17115, "9999999999999"),
        Some("geli-gas-sperrung-lf"),
    );
}

/// The WiM legs that share a Prüfidentifikator across the Sparten reach **one**
/// workflow from either recipient.
///
/// AWH WiM Gas 2.0 restates WiM Strom Teil 1 use-case for use-case, so a second
/// workflow would differ only in the Codeliste it names — which the workflow
/// derives from the Sparte the ingest layer passes it.
#[test]
fn the_wim_legs_are_one_workflow_in_both_sparten() {
    let (router, reg) = (combined_router(), combined_registry());
    for (pid, workflow) in [
        (17_001_u32, "wim-geraeteubernahme"),
        (17_009, "wim-geraeteubernahme"),
        (23_001, "wim-insrpt"),
    ] {
        for recipient in [STROM_NB, GAS_GNB, NEUTRAL_RB] {
            assert_eq!(
                resolve_workflow(&router, &reg, pid, recipient),
                Some(workflow),
                "PID {pid} from {recipient}"
            );
        }
    }
}

#[test]
fn non_split_pid_is_sparte_independent() {
    let (router, reg) = (combined_router(), combined_registry());
    assert_eq!(
        resolve_workflow(&router, &reg, 55001, STROM_NB),
        Some("gpke-supplier-change"),
    );
    assert_eq!(
        resolve_workflow(&router, &reg, 55001, GAS_GNB),
        Some("gpke-supplier-change"),
    );
    // Unknown PID → None (dead-lettered by the caller).
    assert_eq!(resolve_workflow(&router, &reg, 99999, STROM_NB), None);
}
