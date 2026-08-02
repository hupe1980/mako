//! Commodity-aware ingest routing.
//!
//! A Sparte-split shared PID (INSRPT 23001 — registered by both `mako-wim` and
//! `mako-wim-gas`) must resolve to the Strom or Gas workflow based on the
//! interchange recipient's Sparte (UNB DE0010), not by the last-write-wins
//! unambiguous table. Guards `EdifactApiState::resolve_workflow` /
//! `edifact_api::resolve_workflow`, which the AS4 + REST ingest paths call.

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

/// Mirror a combined Strom+Gas deployment: both WiM modules register INSRPT
/// 23001 — Gas last, so the unambiguous `route()` fallback resolves to Gas.
fn combined_router() -> PidRouter {
    let mut r = PidRouter::new();
    r.register(23001, "wim-insrpt");
    r.register_with_sparte(23001, Sparte::Strom, "wim-insrpt");
    r.register(23001, "wim-gas-insrpt"); // WimGasModule registered after WimModule
    r.register_with_sparte(23001, Sparte::Gas, "wim-gas-insrpt");
    r.register(55001, "gpke-supplier-change"); // non-Sparte-split control PID
    r
}

#[test]
fn strom_recipient_routes_shared_pid_to_strom_workflow() {
    let (router, reg) = (combined_router(), combined_registry());
    // The fix: without commodity-aware routing, `route(23001)` is the last-write
    // "wim-gas-insrpt" — wrong for a Strom interchange.
    assert_eq!(
        resolve_workflow(&router, &reg, 23001, STROM_NB),
        Some("wim-insrpt"),
    );
}

#[test]
fn gas_recipient_routes_shared_pid_to_gas_workflow() {
    let (router, reg) = (combined_router(), combined_registry());
    assert_eq!(
        resolve_workflow(&router, &reg, 23001, GAS_GNB),
        Some("wim-gas-insrpt"),
    );
}

#[test]
fn neutral_or_unknown_recipient_falls_back_to_unambiguous_table() {
    let (router, reg) = (combined_router(), combined_registry());
    // Both (RB) and non-own recipients use the unambiguous fallback (Gas, last write).
    assert_eq!(
        resolve_workflow(&router, &reg, 23001, NEUTRAL_RB),
        Some("wim-gas-insrpt"),
    );
    assert_eq!(
        resolve_workflow(&router, &reg, 23001, "9999999999999"),
        Some("wim-gas-insrpt"),
    );
}

#[test]
fn geraeteubernahme_shared_pid_splits_by_sparte() {
    // ORDERS 17001 (Geräteübernahme Anfrage) is shared Strom/Gas. Mirror the
    // module registrations: WimModule (Strom) then WimGasModule (Gas, last write).
    let mut r = PidRouter::new();
    r.register(17001, "wim-geraeteubernahme");
    r.register_with_sparte(17001, Sparte::Strom, "wim-geraeteubernahme");
    r.register(17001, "wim-gas-geraeteubernahme"); // Gas wins the unambiguous fallback
    r.register_with_sparte(17001, Sparte::Gas, "wim-gas-geraeteubernahme");
    let reg = combined_registry();

    assert_eq!(
        resolve_workflow(&r, &reg, 17001, STROM_NB),
        Some("wim-geraeteubernahme"),
        "Strom recipient must reach the Strom Geräteübernahme workflow"
    );
    assert_eq!(
        resolve_workflow(&r, &reg, 17001, GAS_GNB),
        Some("wim-gas-geraeteubernahme"),
        "Gas recipient must reach the Gas Geräteübernahme workflow"
    );
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
