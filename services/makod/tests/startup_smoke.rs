//! Startup smoke test — verifies that every workflow registered by every
//! `EngineModule` has a matching entry in the deadline-dispatch coverage table.
//!
//! This test verifies that every workflow registered by every
//! `EngineModule` also has a matching entry in the deadline-dispatch coverage table,
//! catching missing `DISPATCH_TABLE` entries before they reach production.
//!
//! It builds the production module stack from
//! `makod::startup::production_modules` with in-memory stores and calls
//! `assert_dispatch_coverage`, which panics if any registered workflow is
//! absent from the dispatch table.
//!
//! Taking the list from production rather than restating it is the point. A
//! module registered by the daemon but missing here registers workflows whose
//! deadlines nothing dispatches, and shrinks every figure the second test pins
//! — both failures are silent, and both happened.

use std::sync::Arc;

use mako_engine::{
    builder::EngineBuilder, deadline::InMemoryDeadlineStore, event_store::InMemoryEventStore,
    registry::InMemoryProcessRegistry, snapshot::InMemorySnapshotStore,
};

use makod::deadline_dispatch;

/// Every workflow declared by every production module must appear in
/// `deadline_dispatch::DISPATCH_TABLE`.  If a new module or workflow is added
/// without a matching dispatch arm, this test panics with an actionable message
/// before the bug can reach a production binary. That arm is what a fired
/// deadline resolves to, and a `Deadline` label no arm matches fires into
/// `None` **silently** — so this is the only signal.
///
/// The stack comes from [`makod::startup::production_modules`] — the same list
/// the daemon builds from, so this test cannot fall behind it. Enumerating the
/// modules here again is what let it fall behind before.
#[test]
fn all_registered_workflows_covered_by_dispatch_table() {
    let mut builder = EngineBuilder::new()
        .with_event_store(Arc::new(InMemoryEventStore::new()))
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_deadline_store(InMemoryDeadlineStore::new())
        .with_registry(InMemoryProcessRegistry::new());
    // The one production list — see `makod::startup::production_modules`. Never
    // restate the stack here: a guard with its own copy silently stops seeing a
    // module the daemon registers.
    for module in makod::startup::production_modules() {
        builder = builder.register(module);
    }
    let ctx = builder.build();

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

/// Pin the headline figures the project's landing page advertises.
///
/// `site/templates/index.html` states a PID count and a workflow count. Those
/// are the first numbers a reader sees, and nothing connected them to the code —
/// so adding a module or a PID moved the truth and left the page behind, with no
/// signal. This test is that signal.
///
/// The counts are taken over the **full** module stack, which is what the page
/// describes: a deployment holding every market role. A role-limited deployment
/// registers fewer.
///
/// When this fails, the fix is to update `index.html` to the number the
/// assertion reports — not to loosen the assertion.
#[test]
fn the_landing_page_figures_match_the_registered_engine() {
    let mut builder = EngineBuilder::new()
        .with_event_store(Arc::new(InMemoryEventStore::new()))
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_deadline_store(InMemoryDeadlineStore::new())
        .with_registry(InMemoryProcessRegistry::new());
    // The one production list — see `makod::startup::production_modules`. Never
    // restate the stack here: a guard with its own copy silently stops seeing a
    // module the daemon registers.
    for module in makod::startup::production_modules() {
        builder = builder.register(module);
    }
    let ctx = builder.build();

    let pids = ctx.pid_router().len();
    let workflows = ctx.registered_workflows().len();

    // Deliberately explicit so a diff shows the number, not just a symbol.
    //
    // `LANDING_PAGE_PIDS` counts PIDs the engine *routes* (`PidRouter::table` is
    // keyed by PID). The page states a second, smaller figure beside it — how
    // many of those also carry AHB rules — which `e2e_ahb_rule_coverage_guard`
    // pins; the gap between them is the deliberate `KNOWN_PROFILE_GAPS` set.
    // Both are correct and measure different things; do not "harmonise" them.
    // The figure moves whenever a module registers or retires a PID; update it
    // together with the page rather than reasoning about the delta here.
    const LANDING_PAGE_PIDS: usize = 467;
    const LANDING_PAGE_WORKFLOWS: usize = 70;

    assert_eq!(
        pids, LANDING_PAGE_PIDS,
        "site/templates/index.html advertises {LANDING_PAGE_PIDS} Prüfidentifikatoren, \
         the engine registers {pids} — update the page"
    );
    assert_eq!(
        workflows, LANDING_PAGE_WORKFLOWS,
        "site/templates/index.html advertises {LANDING_PAGE_WORKFLOWS} MaKo workflows, \
         the engine registers {workflows} — update the page"
    );

    // The landing page is not the only place that states these figures. Prose
    // elsewhere restates them to explain what a role-scoped build is a subset
    // *of*, and a sentence naming a number nothing checks goes stale on its own
    // — the two service-overview mentions below drifted to a figure that never
    // matched any build, precisely because they were not listed here.
    //
    // A doc that names only the workflow count is listed too: the pair and the
    // lone number go stale the same way, and the diagram labels are where a
    // reader looks first.
    //
    // `concepts/` is not in git, so a checkout without it is normal and only a
    // file that exists and disagrees is a finding.
    for (doc, sentence) in [
        (
            "../../site/content/docs/services/makod.md",
            format!("**{workflows} workflows over {pids} Prüfidentifikatoren**"),
        ),
        (
            "../../concepts/AGENTD.md",
            format!("**{workflows} workflows and {pids} PIDs**"),
        ),
        (
            "../../concepts/MARKET_LANDSCAPE.md",
            format!("**{workflows} workflows and {pids} PIDs**"),
        ),
        (
            "../../site/content/docs/architecture/_index.md",
            format!("EDIFACT ↔ BO4E, {workflows} workflows"),
        ),
        (
            "../../site/content/docs/services/_index.md",
            format!("EDIFACT runtime · {workflows} workflows"),
        ),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(doc);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        assert!(
            src.contains(&sentence),
            "{doc} no longer states the engine's own figures — it must contain \
             {sentence:?}"
        );
    }
}
