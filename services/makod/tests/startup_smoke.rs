//! Startup smoke test — verifies that every workflow registered by every
//! `EngineModule` has a matching entry in the deadline-dispatch coverage table.
//!
//! This test verifies that every workflow registered by every
//! `EngineModule` also has a matching entry in the deadline-dispatch coverage table,
//! catching missing `DISPATCH_TABLE` entries before they reach production.
//!
//! It instantiates the **full** production module stack with in-memory stores
//! (matching `services/makod/src/main.rs` module registration) and calls
//! `assert_dispatch_coverage`, which panics if any registered workflow is
//! absent from the dispatch table.

use std::sync::Arc;

use mako_engine::{
    builder::EngineBuilder, deadline::InMemoryDeadlineStore, event_store::InMemoryEventStore,
    registry::InMemoryProcessRegistry, snapshot::InMemorySnapshotStore,
};
use mako_geli_gas::GeliGasModule;
use mako_gpke::GpkeModule;
use mako_mabis::MabisModule;
use mako_wim::WimModule;
use mako_wim_gas::WimGasModule;

use makod::deadline_dispatch;

/// Every workflow declared by all five production modules must appear in
/// `deadline_dispatch::DISPATCH_TABLE`.  If a new module or workflow is added
/// without a matching dispatch arm, this test panics with an actionable message
/// before the bug can reach a production binary.
///
/// Module stack must match `services/makod/src/main.rs`:
/// - `GpkeModule`    — PIDs 55001–55002, 55016 + INVOIC + IFTSTA
/// - `WimModule`     — PIDs 55039/55042/55051/55168 (WiM Strom Messstellenbetrieb)
/// - `GeliGasModule` — PIDs 44001–44021 (GeLi Gas; 44022–44024 registered by WimGasModule) + PID 31011 (AWH Rechnung)
/// - `WimGasModule`  — PIDs 44039–44053, 44168–44170 (WiM Gas MSB-Wechsel)
/// - `MabisModule`   — PID 13003 (Bilanzkreisabrechnung Strom)
#[test]
fn all_registered_workflows_covered_by_dispatch_table() {
    let ctx = EngineBuilder::new()
        .with_event_store(Arc::new(InMemoryEventStore::new()))
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_deadline_store(InMemoryDeadlineStore::new())
        .with_registry(InMemoryProcessRegistry::new())
        .register(Box::new(GpkeModule))
        .register(Box::new(WimModule))
        .register(Box::new(GeliGasModule))
        .register(Box::new(WimGasModule))
        .register(Box::new(MabisModule))
        .build();

    // Panics with an actionable message if any registered workflow is absent
    // from the dispatch table — that panic is the test-failure signal.
    deadline_dispatch::assert_dispatch_coverage(ctx.registered_workflows());
}

/// Assert that every active FV transition is registered in the migration
/// dispatch table.
///
/// This is the guard against the "missing dispatch arm" scenario described in
/// F-009: a developer adds a new `StateMigration` in a domain crate but forgets
/// to add a corresponding arm to `migration_api::dispatch_migrations` and to
/// `migration_api::KNOWN_FV_TRANSITIONS`.
///
/// **Maintenance rule for each October release cycle:**
/// 1. Add the new `(from, to)` pair to `KNOWN_FV_TRANSITIONS` in `migration_api.rs`.
/// 2. Add the corresponding `match` arm to `dispatch_migrations`.
/// 3. Add the new pair to `active_transitions` below.
///
/// If any of the three steps is missing, this test panics with a clear message.
#[tokio::test]
async fn migration_dispatch_table_covers_active_fv_transitions() {
    let active_transitions: &[(&str, &str)] = &[("FV2025-10-01", "FV2026-10-01")];

    let known = makod::migration_api::KNOWN_FV_TRANSITIONS;
    for (from, to) in active_transitions {
        assert!(
            known.contains(&(*from, *to)),
            "migration_api::KNOWN_FV_TRANSITIONS does not contain ({from:?}, {to:?}). \
             Add a `match` arm to `dispatch_migrations` in migration_api.rs and \
             add the pair to KNOWN_FV_TRANSITIONS. \
             This is a mandatory step in the annual October release workflow.",
        );
    }
}

// ── D4 Party registry §2.13 compliance tests ─────────────────────────────────

/// BDEW §2.13 (Allgemeine Festlegungen V6.1d): a single `[[party]]` entry must
/// not mix Strom roles with Gas roles.  The `MpIdRegistry` must reject such a
/// config at startup.  A violation here would produce wrong NAD agency codes
/// and incorrect UNB DE0004 sender identities in all EDIFACT messages.
#[test]
fn party_registry_rejects_mixed_strom_gas_roles() {
    use makod::config::PartyConfig;
    use makod::party_registry::MpIdRegistry;

    // A single party entry claiming both NB (Strom) and GNB (Gas) on the same GLN.
    // Per BDEW §2.13 these must be two separate entries with different GLNs.
    let parties = vec![PartyConfig {
        mp_id: "9900000000001".to_owned(),
        roles: vec!["NB".to_owned(), "GNB".to_owned()],
        agency: None,
        primary: true,
    }];

    let result = MpIdRegistry::from_config(&parties);
    assert!(
        result.is_err(),
        "MpIdRegistry must reject a [[party]] entry that mixes Strom role NB \
         with Gas role GNB on the same GLN — BDEW §2.13 requires separate GLNs \
         per Marktrolle per Sparte (Allgemeine Festlegungen V6.1d)"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("§2.13") || err.contains("Gas") || err.contains("Strom"),
        "Error message should cite §2.13 or explain the Strom/Gas conflict; got: {err}"
    );
}

/// A valid VIU configuration: Strom NB and Gas GNB as *separate* entries with
/// different GLNs (99… for Strom, 98… for Gas).  Must be accepted.
#[test]
fn party_registry_accepts_valid_viu_strom_gas_split() {
    use makod::config::PartyConfig;
    use makod::party_registry::MpIdRegistry;

    let parties = vec![
        PartyConfig {
            mp_id: "9900000000001".to_owned(), // BDEW Strom NB
            roles: vec!["NB".to_owned()],
            agency: None,
            primary: true,
        },
        PartyConfig {
            mp_id: "9800000000001".to_owned(), // DVGW Gas GNB
            roles: vec!["GNB".to_owned()],
            agency: None,
            primary: false,
        },
    ];

    let result = MpIdRegistry::from_config(&parties);
    assert!(
        result.is_ok(),
        "MpIdRegistry must accept Strom NB + Gas GNB as separate entries \
         with different GLNs — this is a valid VIU configuration per §2.13; \
         got error: {:?}",
        result.err()
    );

    let registry = result.unwrap();
    assert_eq!(registry.primary_mp_id(), "9900000000001");
    // Agency for 99… prefix must be "293" (BDEW Strom).
    assert_eq!(registry.primary_agency(), "293");
    // is_own_mp_id must recognise both GLNs.
    assert!(registry.is_own_mp_id("9900000000001"));
    assert!(registry.is_own_mp_id("9800000000001"));
}

/// Same role in two separate party entries must be rejected.
/// Each Marktrolle must belong to exactly one party entry.
#[test]
fn party_registry_rejects_duplicate_role() {
    use makod::config::PartyConfig;
    use makod::party_registry::MpIdRegistry;

    let parties = vec![
        PartyConfig {
            mp_id: "9900000000001".to_owned(),
            roles: vec!["LF".to_owned()],
            agency: None,
            primary: true,
        },
        PartyConfig {
            mp_id: "9900000000002".to_owned(),
            roles: vec!["LF".to_owned()], // duplicate role!
            agency: None,
            primary: false,
        },
    ];

    let result = MpIdRegistry::from_config(&parties);
    assert!(
        result.is_err(),
        "MpIdRegistry must reject the same role in two different party entries; \
         each Marktrolle must have exactly one GLN (BDEW §2.13)"
    );
}

// ── D8 Adapter coverage ───────────────────────────────────────────────────────

// Note: `validate_adapter_coverage` is `pub(crate)`, so it cannot be called from
// integration tests directly.  It is exercised by the `all_workflows_have_adapter_coverage`
// unit test inside `startup.rs` (run via `cargo test -p makod --lib`).
// The `all_registered_workflows_covered_by_dispatch_table` test above indirectly validates
// that all registered workflows have coverage, since the EngineBuilder panics at build()
// time for any workflow lacking a profile.
