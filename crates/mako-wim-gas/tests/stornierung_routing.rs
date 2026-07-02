//! Boundary tests for PIDs 44022–44024 role-conditional routing in `WimGasModule`.
//!
//! ## Regulatory background
//!
//! PIDs 44022–44024 (WiM Gas / GeLi Gas Stornierung) are multi-domain per BDEW PID 3.3/4.0.
//! `WimGasModule` registers them as `"wim-gas-stornierung"` for:
//!   - `DeploymentRoles::all()` — backward-compatible default
//!   - `Msb` role — gMSB receives 44022 inbound from LFN/LFA or sends 44023/44024
//!   - `Nmsb` role — same as Msb
//!
//! `WimGasModule` does NOT register them for `Nb`-only deployments.
//! In that case `GeliGasModule` owns them (see `mako-geli-gas` stornierung_routing tests).
//!
//! Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0 / WiM Gas).

use mako_engine::{
    builder::EngineModule,
    marktrolle::{DeploymentRoles, Marktrolle},
    pid_router::PidRouter,
};
use mako_wim_gas::WimGasModule;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn registered_pids(roles: DeploymentRoles) -> Vec<u32> {
    let mut router = PidRouter::new();
    WimGasModule.register_pids_with_roles(&mut router, &roles);
    (44000_u32..=44030)
        .filter(|&p| router.route(p).is_some())
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// `all()` — backward-compatible: 44022–44024 routed to `wim-gas-stornierung`.
#[test]
fn all_roles_registers_stornierung_pids() {
    let all = DeploymentRoles::all();
    let mut router = PidRouter::new();
    WimGasModule.register_pids_with_roles(&mut router, &all);

    for pid in [44022_u32, 44023, 44024] {
        assert_eq!(
            router.route(pid),
            Some("wim-gas-stornierung"),
            "PID {pid}: expected wim-gas-stornierung for all() roles",
        );
    }
}

/// `Msb` role — gMSB deployment registers 44022–44024 as `wim-gas-stornierung`.
#[test]
fn msb_role_registers_stornierung_pids() {
    let msb = DeploymentRoles::from_roles([Marktrolle::Msb]);
    let mut router = PidRouter::new();
    WimGasModule.register_pids_with_roles(&mut router, &msb);

    for pid in [44022_u32, 44023, 44024] {
        assert_eq!(
            router.route(pid),
            Some("wim-gas-stornierung"),
            "PID {pid}: expected wim-gas-stornierung for Msb role",
        );
    }
}

/// `Nmsb` role — also registers 44022–44024 as `wim-gas-stornierung`.
#[test]
fn nmsb_role_registers_stornierung_pids() {
    let nmsb = DeploymentRoles::from_roles([Marktrolle::Nmsb]);
    let mut router = PidRouter::new();
    WimGasModule.register_pids_with_roles(&mut router, &nmsb);

    for pid in [44022_u32, 44023, 44024] {
        assert_eq!(
            router.route(pid),
            Some("wim-gas-stornierung"),
            "PID {pid}: expected wim-gas-stornierung for Nmsb role",
        );
    }
}

/// `Nb`-only — WimGasModule must NOT register 44022–44024.
/// GeliGasModule owns them in this context (supply-change stornierung).
#[test]
fn nb_only_role_does_not_register_stornierung_pids() {
    let nb = DeploymentRoles::from_roles([Marktrolle::Nb]);
    let pids = registered_pids(nb);
    for pid in [44022_u32, 44023, 44024] {
        assert!(
            !pids.contains(&pid),
            "PID {pid}: WimGasModule must NOT register 44022–44024 for Nb-only deployments \
             (GeliGasModule owns them in this context)",
        );
    }
}

/// `Lf`-only — WimGasModule must NOT register 44022–44024.
/// LFN/LFA deployments only send these PIDs outbound; inbound confirmations come
/// on different PIDs from the GNB.
#[test]
fn lf_only_role_does_not_register_stornierung_pids() {
    let lf = DeploymentRoles::from_roles([Marktrolle::Lf]);
    let pids = registered_pids(lf);
    for pid in [44022_u32, 44023, 44024] {
        assert!(
            !pids.contains(&pid),
            "PID {pid}: WimGasModule must NOT register 44022–44024 for Lf-only deployments",
        );
    }
}

/// Unconditional WiM Gas PIDs (44039–44053, 44168–44170) are always registered,
/// regardless of role — routing them for any role is a correctness invariant.
#[test]
fn unconditional_pids_always_registered() {
    for roles in [
        DeploymentRoles::all(),
        DeploymentRoles::from_roles([Marktrolle::Nb]),
        DeploymentRoles::from_roles([Marktrolle::Msb]),
        DeploymentRoles::from_roles([Marktrolle::Lf]),
    ] {
        let mut router = PidRouter::new();
        WimGasModule.register_pids_with_roles(&mut router, &roles);

        // WiM Gas Kündigung MSB Gas
        for pid in [44039_u32, 44040, 44041] {
            assert!(
                router.route(pid).is_some(),
                "PID {pid} (Kündigung) must always be registered",
            );
        }
        // WiM Gas Anmeldung neuer MSB Gas
        for pid in [44042_u32, 44043, 44044, 44051, 44052, 44053] {
            assert!(
                router.route(pid).is_some(),
                "PID {pid} (Anmeldung/Ende) must always be registered",
            );
        }
        // WiM Gas Verpflichtungsanfrage
        for pid in [44168_u32, 44169, 44170] {
            assert!(
                router.route(pid).is_some(),
                "PID {pid} (Verpflichtungsanfrage) must always be registered",
            );
        }
    }
}
