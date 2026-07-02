//! Boundary tests for PIDs 44022–44024 role-conditional routing in `GeliGasModule`.
//!
//! ## Regulatory background
//!
//! PIDs 44022–44024 (GeLi Gas / WiM Gas Stornierung) are multi-domain per BDEW PID 3.3/4.0.
//! `GeliGasModule` registers them role-conditionally:
//!
//!   - `Nb`-only: 44022/44023/44024 → `geli-gas-stornierung` (GNB receives Anfrage)
//!   - `Lf`-only: 44023/44024 → `geli-gas-stornierung-lf` (LF receives GNB response)
//!   - `Nb+Lf`: both workflows active (no conflict — different PIDs)
//!   - `Msb`/`Nmsb`/`all()`: NOT registered here; `WimGasModule` owns them (`wim-gas-stornierung`)
//!
//!   - PID 44022 (Anfrage nach Stornierung): LFN/LFA → GNB
//!   - PID 44023 (Bestätigung Stornierung): GNB → LFN/LFA
//!   - PID 44024 (Ablehnung Stornierung): GNB → LFN/LFA
//!
//! Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0 / WiM Gas).

use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
use mako_geli_gas::GeliGasModule;

// ── Tests ────────────────────────────────────────────────────────────────────

/// `Nb`-only deployment — GeliGasModule registers 44022 as `geli-gas-stornierung`.
/// Only PID 44022 (inbound Anfrage) is registered; 44023/44024 are outbound from GNB
/// and dispatched via the outbox — they do not need inbound PID-router registration.
#[test]
fn nb_only_registers_stornierung_anfrage_as_geli_gas() {
    let nb = DeploymentRoles::nb();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &nb);

    // 44022 (inbound Anfrage) must be registered for the GNB
    assert_eq!(
        router.route(44022),
        Some("geli-gas-stornierung"),
        "PID 44022: expected geli-gas-stornierung for Nb-only deployment",
    );
    // 44023/44024 are outbound from GNB — must NOT be registered for inbound routing
    // (they are dispatched via the outbox to LFN/LFA, not received inbound by GNB)
    for pid in [44023_u32, 44024] {
        assert!(
            router.route(pid).is_none(),
            "PID {pid}: must NOT be registered for inbound routing in Nb-only deployment \
             (outbound GNB → LF response — dispatched via outbox, not inbound)",
        );
    }
}

/// `all()` — backward-compatible default: GeliGasModule must NOT register 44022–44024.
/// In combined deployments, `WimGasModule` owns these PIDs (`wim-gas-stornierung`).
#[test]
fn all_roles_does_not_register_stornierung() {
    let all = DeploymentRoles::all();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &all);

    for pid in [44022_u32, 44023, 44024] {
        assert!(
            router.route(pid).is_none(),
            "PID {pid}: GeliGasModule must NOT register 44022–44024 for all() roles \
             (WimGasModule owns them in backward-compat mode)",
        );
    }
}

/// `Msb`-only — GeliGasModule must NOT register 44022–44024.
/// gMSB deployments use WimGasModule's `wim-gas-stornierung`.
#[test]
fn msb_role_does_not_register_stornierung() {
    let msb = DeploymentRoles::msb();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &msb);

    for pid in [44022_u32, 44023, 44024] {
        assert!(
            router.route(pid).is_none(),
            "PID {pid}: GeliGasModule must NOT register 44022–44024 for Msb role",
        );
    }
}

/// `Nmsb`-only — same as Msb: GeliGasModule must NOT register 44022–44024.
#[test]
fn nmsb_role_does_not_register_stornierung() {
    let nmsb = DeploymentRoles::nmsb();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &nmsb);

    for pid in [44022_u32, 44023, 44024] {
        assert!(
            router.route(pid).is_none(),
            "PID {pid}: GeliGasModule must NOT register 44022–44024 for Nmsb role",
        );
    }
}

/// `Nb + Msb` combined — GeliGasModule must NOT register 44022–44024.
/// WimGasModule's `Msb` condition fires, so the GNB+gMSB combined deployment
/// uses `wim-gas-stornierung`. This avoids a routing conflict.
#[test]
fn nb_msb_combined_does_not_register_stornierung_in_geli_gas() {
    let nb_msb = DeploymentRoles::nb_msb();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &nb_msb);

    for pid in [44022_u32, 44023, 44024] {
        assert!(
            router.route(pid).is_none(),
            "PID {pid}: GeliGasModule must NOT register 44022–44024 for Nb+Msb combined \
             deployment (WimGasModule handles them via Msb condition)",
        );
    }
}

/// Unconditional GeLi Gas PIDs (44001–44021) are always registered regardless of role.
#[test]
fn unconditional_pids_always_registered() {
    for roles in [
        DeploymentRoles::all(),
        DeploymentRoles::nb(),
        DeploymentRoles::lf(),
        DeploymentRoles::msb(),
    ] {
        let mut router = PidRouter::new();
        GeliGasModule.register_pids_with_roles(&mut router, &roles);

        // GeLi Gas Lieferantenwechsel Gas (supply-switching)
        for pid in [44001_u32, 44002, 44003, 44004, 44005, 44006] {
            assert!(
                router.route(pid).is_some(),
                "PID {pid} (Lieferbeginn/-ende) must always be registered",
            );
        }
        // GeLi Gas Abmeldung NN vom NB
        for pid in [44007_u32, 44008, 44009] {
            assert!(
                router.route(pid).is_some(),
                "PID {pid} (Abmeldung NN) must always be registered",
            );
        }
    }
}

/// `Nb`-only routing isolation: PID 44022 routes to `geli-gas-stornierung`,
/// confirming no cross-contamination from WimGasModule.
#[test]
fn nb_stornierung_routing_is_geli_gas_not_wim_gas() {
    let nb = DeploymentRoles::nb();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &nb);

    // 44022 is the inbound Anfrage on GNB side
    assert_ne!(
        router.route(44022),
        Some("wim-gas-stornierung"),
        "PID 44022: Nb-only deployment must NOT route to wim-gas-stornierung via GeliGasModule",
    );
    assert_eq!(
        router.route(44022),
        Some("geli-gas-stornierung"),
        "PID 44022: Nb-only deployment must route to geli-gas-stornierung",
    );
}

/// `Lf`-only deployment — GeliGasModule registers 44023/44024 as `geli-gas-stornierung-lf`.
/// PID 44022 is ERP-initiated outbound and must NOT be registered for inbound routing.
#[test]
fn lf_only_registers_antwort_pids_as_stornierung_lf() {
    let lf = DeploymentRoles::lf();
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &lf);

    // 44023/44024 must route to LF-side workflow
    for pid in [44023_u32, 44024] {
        assert_eq!(
            router.route(pid),
            Some("geli-gas-stornierung-lf"),
            "PID {pid}: Lf-only deployment must route to geli-gas-stornierung-lf",
        );
    }
    // 44022 is outbound-only from LF perspective — must NOT be registered for inbound routing
    assert!(
        router.route(44022).is_none(),
        "PID 44022: must NOT be registered for inbound routing in Lf-only deployment \
         (it is ERP-initiated outbound; 44022 is only registered on GNB/Nb-only side)",
    );
}

/// `Nb + Lf` combined deployment — both GNB-side and LF-side workflows active.
/// No routing conflict: 44022 → GNB-side; 44023/44024 → LF-side (different PIDs).
/// This covers a utility that acts simultaneously as GNB and LFN/LFA.
#[test]
fn nb_lf_combined_registers_both_workflows_without_conflict() {
    let nb_lf = DeploymentRoles::from_roles([
        mako_engine::marktrolle::Marktrolle::Nb,
        mako_engine::marktrolle::Marktrolle::Lf,
    ]);
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &nb_lf);

    // GNB receives 44022 inbound from peer LFN/LFA
    assert_eq!(
        router.route(44022),
        Some("geli-gas-stornierung"),
        "PID 44022: Nb+Lf combined deployment must route to geli-gas-stornierung (GNB-side)",
    );
    // Own LF-side receives 44023/44024 inbound from GNB
    for pid in [44023_u32, 44024] {
        assert_eq!(
            router.route(pid),
            Some("geli-gas-stornierung-lf"),
            "PID {pid}: Nb+Lf combined deployment must route to geli-gas-stornierung-lf (LF-side)",
        );
    }
}

/// `Lf` with `Msb` — GeliGasModule must NOT register any Stornierung PIDs.
/// WimGasModule's `Msb` condition fires instead.
#[test]
fn lf_msb_combined_does_not_register_stornierung_in_geli_gas() {
    let lf_msb = DeploymentRoles::from_roles([
        mako_engine::marktrolle::Marktrolle::Lf,
        mako_engine::marktrolle::Marktrolle::Msb,
    ]);
    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &lf_msb);

    for pid in [44022_u32, 44023, 44024] {
        assert!(
            router.route(pid).is_none(),
            "PID {pid}: GeliGasModule must NOT register Stornierung PIDs when Msb is present \
             (WimGasModule owns them)",
        );
    }
}
