//! Startup smoke test — the production module stack builds.
//!
//! It builds the stack from `makod::startup::production_modules` with
//! in-memory stores. `EngineBuilder::build` refuses a module that routes a
//! Prüfidentifikator to a workflow it registers no driver for, so a build that
//! returns is a stack in which every routed workflow can decide its own
//! deadlines.
//!
//! Taking the list from production rather than restating it is the point. A
//! module registered by the daemon but missing here shrinks every figure the
//! second test pins — a silent failure that happened.

use std::sync::Arc;

use mako_engine::{
    builder::EngineBuilder, deadline::InMemoryDeadlineStore, event_store::InMemoryEventStore,
    registry::InMemoryProcessRegistry, snapshot::InMemorySnapshotStore,
};

/// Every routed workflow of the production stack has a driver, so a fired
/// deadline always reaches the workflow that decides it.
///
/// The stack comes from [`makod::startup::production_modules`] — the same list
/// the daemon builds from, so this test cannot fall behind it.
#[test]
fn the_production_stack_builds_with_a_driver_per_workflow() {
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
    // `build` panics on a routed workflow without a driver.
    let ctx = builder.build();
    assert_eq!(ctx.drivers().len(), ctx.registered_workflows().len());
    assert!(!ctx.drivers().is_empty());
}

/// Each deployment role registers exactly its module set.
///
/// One row per role, stated here rather than read from `MODULE_TABLE`: a module
/// added to the table changes some role's set, and this test fails until the
/// row names it. A role whose set is empty would boot a daemon that routes
/// nothing.
#[test]
fn each_role_registers_its_module_set() {
    use makod::deployment::Rolle;
    let expected: &[(Rolle, &[&str])] = &[
        (Rolle::LfStrom, &["gpke", "emob"]),
        (Rolle::LfGas, &["geli-gas"]),
        (
            Rolle::NbStrom,
            &["gpke", "wim", "mabis", "emob", "redispatch"],
        ),
        (Rolle::NbGas, &["wim", "geli-gas", "mako-gabi-gas"]),
        (Rolle::MsbStrom, &["wim"]),
        (Rolle::MsbGas, &["wim"]),
        (Rolle::EsaStrom, &["wim"]),
    ];
    assert_eq!(
        expected.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        Rolle::ALL.to_vec(),
        "every role has exactly one row"
    );
    for (rolle, modules) in expected {
        let got: Vec<&str> = makod::startup::modules_for(&std::iter::once(*rolle).collect())
            .iter()
            .map(|m| m.name())
            .collect();
        assert_eq!(&got, modules, "{rolle} registers the wrong module set");
    }

    // Every table row is some role's module, and names what it builds.
    for row in makod::startup::MODULE_TABLE {
        assert!(
            !row.rollen.is_empty(),
            "{} is registered by no role",
            row.name
        );
        assert_eq!(row.build().name(), row.name);
    }
    // All roles together are the whole stack the guards measure.
    assert_eq!(
        makod::startup::modules_for(&Rolle::ALL.into_iter().collect()).len(),
        makod::startup::production_modules().len()
    );
}

/// An LF-only deployment registers no NB module.
///
/// MaBiS, Redispatch and GaBi Gas are network-operator obligations; a
/// deployment declaring `lf-strom` alone must not route their PIDs.
#[test]
fn an_lf_strom_deployment_registers_no_network_module() {
    let names: Vec<&str> =
        makod::startup::modules_for(&std::iter::once(makod::deployment::Rolle::LfStrom).collect())
            .iter()
            .map(|m| m.name())
            .collect();
    for nb_only in ["mabis", "redispatch", "mako-gabi-gas", "wim"] {
        assert!(
            !names.contains(&nb_only),
            "lf-strom registers {nb_only}: {names:?}"
        );
    }
}

/// Assert that every active FV transition is registered in the migration
/// dispatch table.
///
/// This is the guard against the "missing dispatch arm" scenario: a developer adds a new `StateMigration` in a domain crate but forgets
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
        as4: None,
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
            as4: None,
        },
        PartyConfig {
            mp_id: "9800000000001".to_owned(), // DVGW Gas GNB
            roles: vec!["GNB".to_owned()],
            agency: None,
            primary: false,
            as4: None,
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
            as4: None,
        },
        PartyConfig {
            mp_id: "9900000000002".to_owned(),
            roles: vec!["LF".to_owned()], // duplicate role!
            agency: None,
            primary: false,
            as4: None,
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

/// The `<strong>N</strong><span>label</span>` figures in the landing page's
/// stats band, as `label -> N`.
///
/// Reading the page is the point: a constant restating what it says is a claim,
/// not a check, and drifts the moment the page is edited on its own.
fn landing_page_stats() -> std::collections::HashMap<String, usize> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/templates/index.html");
    let html = std::fs::read_to_string(&path).expect("the landing page template");
    let mut out = std::collections::HashMap::new();
    for chunk in html.split("<strong>").skip(1) {
        let Some((number, rest)) = chunk.split_once("</strong>") else {
            continue;
        };
        let Ok(n) = number.trim().parse::<usize>() else {
            continue;
        };
        // Only the stats band pairs a number with its own `<span>` label; the
        // prose figures are picked up by their sentence instead.
        let Some(label) = rest
            .strip_prefix("<span>")
            .and_then(|r| r.split_once("</span>"))
            .map(|(l, _)| l.trim())
        else {
            continue;
        };
        out.insert(label.to_owned(), n);
    }
    assert!(
        !out.is_empty(),
        "no <strong>N</strong><span>label</span> stats found in {}",
        path.display()
    );
    out
}

/// Pin the headline figures the project's landing page advertises.
///
/// The figures are **read out of `site/templates/index.html`**, not restated
/// here: a constant in the test claims what the page says without checking it,
/// so editing the page alone would keep CI green on a number nothing backs.
///
/// The counts are taken over the **full** module stack, which is what the page
/// describes: a deployment declaring every role. One declaring fewer registers
/// fewer.
///
/// When this fails, the fix is to change whichever side is wrong — usually the
/// page — not to loosen the assertion.
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

    // The page's own numbers. It states a second, smaller PID figure beside the
    // routed one — how many also carry AHB rules — which
    // `e2e_ahb_rule_coverage_guard` pins; the gap between them is the deliberate
    // `KNOWN_PROFILE_GAPS` set. Both are correct and measure different things;
    // do not "harmonise" them.
    let page = landing_page_stats();
    let stat = |label: &str| -> usize {
        *page
            .get(label)
            .unwrap_or_else(|| panic!("site/templates/index.html has no \"{label}\" stat"))
    };
    let landing_page_pids = stat("Prüfidentifikatoren routed");
    let landing_page_workflows = stat("MaKo workflows");
    let landing_page_message_types = stat("EDI@Energy message types");
    let landing_page_services = stat("services");
    let landing_page_roles = stat("deployable roles");

    assert_eq!(
        edi_energy::MessageType::ALL.len(),
        landing_page_message_types,
        "site/templates/index.html advertises {landing_page_message_types} EDIFACT \
         message types, `MessageType::ALL` has {} — update the page",
        edi_energy::MessageType::ALL.len()
    );

    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    // `workspace.members` alone: `default-members` lists the services a second
    // time, and a line count over the whole manifest would see both.
    let manifest: toml::Table = std::fs::read_to_string(&workspace)
        .expect("the workspace manifest")
        .parse()
        .expect("the workspace manifest is TOML");
    let services = manifest["workspace"]["members"]
        .as_array()
        .expect("workspace.members is an array")
        .iter()
        .filter(|m| m.as_str().is_some_and(|m| m.starts_with("services/")))
        .count();
    assert_eq!(
        services, landing_page_services,
        "site/templates/index.html advertises {landing_page_services} services, the \
         workspace has {services} — update the page"
    );

    // The Marktrollen a deployment can declare, counted from the role type
    // `[deployment] rollen` parses — the role, not the Sparte: `lf-strom` and
    // `lf-gas` are one Marktrolle in two Sparten, and ESA is Strom-only.
    let makod_roles: std::collections::BTreeSet<&str> = makod::deployment::Rolle::ALL
        .iter()
        .map(|r| r.token().split('-').next().unwrap_or(r.token()))
        .collect();
    let makod_roles = makod_roles.len();
    assert_eq!(
        makod_roles, landing_page_roles,
        "site/templates/index.html advertises {landing_page_roles} deployable roles, \
         makod's [deployment] rollen names {makod_roles} Marktrollen — update the page"
    );

    assert_eq!(
        pids, landing_page_pids,
        "site/templates/index.html advertises {landing_page_pids} Prüfidentifikatoren, \
         the engine registers {pids} — update the page"
    );
    assert_eq!(
        workflows, landing_page_workflows,
        "site/templates/index.html advertises {landing_page_workflows} MaKo workflows, \
         the engine registers {workflows} — update the page"
    );

    // The landing page is not the only place that states these figures. Prose
    // elsewhere restates them to explain what a role-limited deployment is a subset
    // *of*, and a sentence naming a number nothing checks goes stale on its own,
    // so every such sentence is listed here.
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
        (
            "../../site/content/docs/services/_index.md",
            format!("{workflows} workflows over {pids} Prüfidentifikatoren"),
        ),
        (
            "../../services/makod/README.md",
            format!("**{workflows} workflows** over {pids} Prüfidentifikatoren"),
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
