//! Integration tests for commodity-aware INSRPT PID routing (F-007).
//!
//! Verifies that shared INSRPT PIDs (23001/23003/23004/23008) route to the
//! correct workflow depending on the commodity (`Sparte`):
//!
//! | Sparte | Workflow       | APERAK Frist | Source        |
//! |--------|----------------|--------------|---------------|
//! | Strom  | `wim-insrpt`   | 5 Werktage   | BK6-24-174    |
//! | Gas    | `wim-gas-insrpt`| 10 Werktage  | BK7-24-01-009 |
//!
//! The tests simulate the `PidRouter` state that `WimModule` + `WimGasModule`
//! produce in a combined Strom+Gas deployment and assert that
//! `route_with_sparte` resolves correctly for every shared and Gas-only PID.

use mako_engine::pid_router::PidRouter;
use mako_engine::types::Sparte;
use mako_wim::insrpt::{INSRPT_ANFRAGE_PIDS, INSRPT_ANTWORT_PIDS, WORKFLOW_NAME as STROM_WF};
use mako_wim_gas::insrpt::{INSRPT_GAS_ONLY_PIDS, INSRPT_SHARED_PIDS, WORKFLOW_NAME as GAS_WF};

// ── Shared PIDs must be distinct from Gas-only PIDs ───────────────────────────

#[test]
fn insrpt_pid_constants_are_coherent() {
    // Shared PIDs must appear in both Strom (ANFRAGE/ANTWORT) and Gas (SHARED).
    for &pid in INSRPT_SHARED_PIDS {
        assert!(
            INSRPT_ANFRAGE_PIDS.contains(&pid) || INSRPT_ANTWORT_PIDS.contains(&pid),
            "PID {pid} is in INSRPT_SHARED_PIDS but not in any WiM Strom PID set"
        );
    }
    // Gas-only PIDs must not appear in the Strom PID sets.
    for &pid in INSRPT_GAS_ONLY_PIDS {
        assert!(
            !INSRPT_ANFRAGE_PIDS.contains(&pid) && !INSRPT_ANTWORT_PIDS.contains(&pid),
            "PID {pid} is Gas-only but appears in a WiM Strom PID set (wrong!)"
        );
    }
    // No overlap between Gas-only and shared.
    for &pid in INSRPT_GAS_ONLY_PIDS {
        assert!(
            !INSRPT_SHARED_PIDS.contains(&pid),
            "PID {pid} appears in both INSRPT_GAS_ONLY_PIDS and INSRPT_SHARED_PIDS"
        );
    }
}

// ── Helper: build a router matching the combined-deployment registration ───────

fn combined_deployment_router() -> PidRouter {
    let mut router = PidRouter::new();

    // Simulate WimModule::register_pids_with_roles for INSRPT:
    for &pid in INSRPT_ANFRAGE_PIDS {
        router.register(pid, STROM_WF);
        router.register_with_sparte(pid, Sparte::Strom, STROM_WF);
    }
    for &pid in INSRPT_ANTWORT_PIDS {
        router.register(pid, STROM_WF);
        router.register_with_sparte(pid, Sparte::Strom, STROM_WF);
    }

    // Simulate WimGasModule::register_pids_with_roles for INSRPT
    // (comes after WimModule — last-write-wins on unambiguous table):
    for &pid in INSRPT_GAS_ONLY_PIDS {
        router.register(pid, GAS_WF);
    }
    for &pid in INSRPT_SHARED_PIDS {
        router.register(pid, GAS_WF); // unambiguous fallback for Gas-standalone
        router.register_with_sparte(pid, Sparte::Gas, GAS_WF);
    }

    router
}

// ── Combined deployment: commodity-aware routing ──────────────────────────────

/// In a combined deployment, shared PIDs must route to `wim-insrpt` when the
/// commodity is Strom (5 WT deadline — BK6-24-174).
#[test]
fn combined_deployment_strom_routes_to_wim_insrpt() {
    let router = combined_deployment_router();
    for &pid in INSRPT_SHARED_PIDS {
        assert_eq!(
            router.route_with_sparte(pid, Sparte::Strom),
            Some(STROM_WF),
            "PID {pid} with Sparte::Strom must route to '{STROM_WF}' (5 WT)"
        );
    }
}

/// In a combined deployment, shared PIDs must route to `wim-gas-insrpt` when
/// the commodity is Gas (10 WT deadline — BK7-24-01-009).
#[test]
fn combined_deployment_gas_routes_to_wim_gas_insrpt() {
    let router = combined_deployment_router();
    for &pid in INSRPT_SHARED_PIDS {
        assert_eq!(
            router.route_with_sparte(pid, Sparte::Gas),
            Some(GAS_WF),
            "PID {pid} with Sparte::Gas must route to '{GAS_WF}' (10 WT)"
        );
    }
}

/// Gas-only PIDs (23005/23009) must always route to `wim-gas-insrpt`
/// regardless of commodity — they do not exist in the Strom AHB.
#[test]
fn gas_only_pids_always_route_to_wim_gas_insrpt() {
    let router = combined_deployment_router();
    for &pid in INSRPT_GAS_ONLY_PIDS {
        // Plain route — these PIDs are unconditional.
        assert_eq!(
            router.route(pid),
            Some(GAS_WF),
            "Gas-only PID {pid} must unconditionally route to '{GAS_WF}'"
        );
    }
}

/// The commodity-qualified routing must never accidentally return the wrong
/// workflow: Strom must not get wim-gas-insrpt, Gas must not get wim-insrpt.
#[test]
fn no_cross_commodity_misrouting() {
    let router = combined_deployment_router();
    for &pid in INSRPT_SHARED_PIDS {
        assert_ne!(
            router.route_with_sparte(pid, Sparte::Strom),
            Some(GAS_WF),
            "PID {pid} Strom must NOT route to '{GAS_WF}'"
        );
        assert_ne!(
            router.route_with_sparte(pid, Sparte::Gas),
            Some(STROM_WF),
            "PID {pid} Gas must NOT route to '{STROM_WF}'"
        );
    }
}

// ── Strom-only deployment ─────────────────────────────────────────────────────

/// In a Strom-only deployment (no WimGasModule), all INSRPT PIDs route to
/// `wim-insrpt` — both unambiguously and via Sparte::Strom.
#[test]
fn strom_only_deployment_routes_all_insrpt_to_wim_insrpt() {
    let mut router = PidRouter::new();
    // Only WimModule registers:
    for &pid in INSRPT_ANFRAGE_PIDS {
        router.register(pid, STROM_WF);
        router.register_with_sparte(pid, Sparte::Strom, STROM_WF);
    }
    for &pid in INSRPT_ANTWORT_PIDS {
        router.register(pid, STROM_WF);
        router.register_with_sparte(pid, Sparte::Strom, STROM_WF);
    }

    for &pid in INSRPT_ANFRAGE_PIDS.iter().chain(INSRPT_ANTWORT_PIDS.iter()) {
        assert_eq!(
            router.route(pid),
            Some(STROM_WF),
            "PID {pid} must route to '{STROM_WF}' in Strom-only deployment"
        );
        assert_eq!(
            router.route_with_sparte(pid, Sparte::Strom),
            Some(STROM_WF),
            "PID {pid} Sparte::Strom must route to '{STROM_WF}' in Strom-only"
        );
    }
}

// ── Gas-only deployment ───────────────────────────────────────────────────────

/// In a Gas-only deployment (no WimModule), all INSRPT PIDs route to
/// `wim-gas-insrpt` — both unambiguously and via Sparte::Gas.
#[test]
fn gas_only_deployment_routes_all_insrpt_to_wim_gas_insrpt() {
    let mut router = PidRouter::new();
    // Only WimGasModule registers:
    for &pid in INSRPT_GAS_ONLY_PIDS {
        router.register(pid, GAS_WF);
    }
    for &pid in INSRPT_SHARED_PIDS {
        router.register(pid, GAS_WF);
        router.register_with_sparte(pid, Sparte::Gas, GAS_WF);
    }

    // Gas-only PIDs:
    for &pid in INSRPT_GAS_ONLY_PIDS {
        assert_eq!(
            router.route(pid),
            Some(GAS_WF),
            "Gas-only PID {pid} must route to '{GAS_WF}'"
        );
    }
    // Shared PIDs (no Strom module present — fallback to Gas):
    for &pid in INSRPT_SHARED_PIDS {
        assert_eq!(
            router.route(pid),
            Some(GAS_WF),
            "PID {pid} must route to '{GAS_WF}' in Gas-only deployment"
        );
        assert_eq!(
            router.route_with_sparte(pid, Sparte::Gas),
            Some(GAS_WF),
            "PID {pid} Sparte::Gas must route to '{GAS_WF}' in Gas-only"
        );
    }
}

// ── Workflow name constants ───────────────────────────────────────────────────

#[test]
fn strom_workflow_name_is_wim_insrpt() {
    assert_eq!(STROM_WF, "wim-insrpt");
}

#[test]
fn gas_workflow_name_is_wim_gas_insrpt() {
    assert_eq!(GAS_WF, "wim-gas-insrpt");
}
