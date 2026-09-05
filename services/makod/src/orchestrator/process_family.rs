//! The `family` metric label, derived in one place.
//!
//! Three metrics carry it — `makod_process_initiated_total{family}`,
//! `makod_process_completed_total{family,result}` and the deadline counters —
//! and a dashboard is only useful if they join. They did not:
//!
//! | Site | Derivation | `geli.lieferbeginn.anmelden` / `geli-gas-lieferbeginn` |
//! |---|---|---|
//! | `commands_api::types::command_family` | command name's first segment, with the Gas families joined | `geli-gas` |
//! | `core::erp_adapter` | `workflow_name.split('-').next()` | `geli` |
//! | `orchestrator::deadline_dispatch` | `workflow_name.split('-').next()` | `geli` |
//!
//! So `makod_process_initiated_total{family="geli-gas"}` and
//! `makod_process_completed_total{family="geli"}` were different time series,
//! and every initiated-versus-completed panel showed a GeLi Gas family that
//! starts processes and never finishes any next to one that finishes processes
//! nobody started. GaBi Gas had the same split.
//!
//! The naive split had two further failure modes the table hides. It cut
//! `geli-gas-…` at the first hyphen, which is not where the family name ends;
//! and it returned the *empty string* for the messages `commands_api::netzzugang`
//! enqueues directly, because those are not workflow output and carry no
//! `workflow_name` — `"".split('-').next()` is `Some("")`, so the `unwrap_or`
//! that was meant to catch this never ran and the label was blank rather than
//! wrong-but-visible.
//!
//! # The shape
//!
//! One [`FAMILIES`] table. Each row owns a label, the workflow-name prefix that
//! produces it and the command-name prefixes that initiate it, so the two
//! derivations are two lookups into the same row and cannot spell the label
//! differently. Longest workflow prefix wins, which is what makes `geli-gas-`
//! beat a hypothetical `geli-`.
//!
//! # Where a command's family comes from
//!
//! A command's family is the family of the *workflow it starts*, not of its own
//! name — the completion side has nothing but the workflow name to label with,
//! so anything else cannot join. Most command prefixes and workflow prefixes
//! agree, but the `invoic.*` and `maloid.*` commands do not start an `invoic-`
//! or `maloid-` workflow (there is none): they are entered against the GPKE,
//! WiM and GeLi Gas billing workflows, and are mapped by full command name in
//! the `COMMAND_OVERRIDES` table below.

/// One process family: the metric label and the two names that produce it.
#[derive(Debug, Clone, Copy)]
pub struct Family {
    /// The `family` label value. One spelling, used by every metric.
    pub label: &'static str,
    /// The workflow-name prefix (with its trailing hyphen), or `None` for a
    /// family that runs no workflow.
    pub workflow_prefix: Option<&'static str>,
    /// Command-name first segments that start a workflow of this family.
    pub command_prefixes: &'static [&'static str],
}

/// The process families, one row each.
///
/// Adding a domain crate to `startup::production_modules` means adding a row
/// here; the tests below fail on a workflow name no row claims.
pub const FAMILIES: &[Family] = &[
    Family {
        label: "gpke",
        workflow_prefix: Some("gpke-"),
        // `maloid.*` continues a `gpke-supplier-change` process, and the
        // `invoic.nne*`/`invoic.mmm` commands enter `gpke-abrechnung`.
        command_prefixes: &["gpke", "maloid"],
    },
    Family {
        label: "wim",
        workflow_prefix: Some("wim-"),
        command_prefixes: &["wim"],
    },
    Family {
        label: "esa",
        workflow_prefix: Some("esa-"),
        command_prefixes: &["esa"],
    },
    Family {
        label: "geli-gas",
        workflow_prefix: Some("geli-gas-"),
        command_prefixes: &["geli"],
    },
    Family {
        label: "gabi-gas",
        workflow_prefix: Some("gabi-gas-"),
        command_prefixes: &["gabi"],
    },
    Family {
        label: "mabis",
        workflow_prefix: Some("mabis-"),
        command_prefixes: &["mabis"],
    },
    Family {
        label: "emob",
        workflow_prefix: Some("emob-"),
        command_prefixes: &[],
    },
    Family {
        label: "redispatch",
        workflow_prefix: Some("redispatch-"),
        command_prefixes: &[],
    },
    Family {
        // No workflow: `commands_api::netzzugang` enqueues an outbox message
        // directly, so nothing ever stamps a workflow name and no completion
        // is counted. The initiation counter is still worth having, and the
        // label has to exist for it to be readable.
        label: "netzzugang",
        workflow_prefix: None,
        command_prefixes: &["netzzugang"],
    },
];

/// Commands whose family is not their own first segment.
///
/// `invoic.*` is a message type, not a process family: the same INVOIC leaves
/// three different workflows. Keyed on the full command name so a new
/// `invoic.*` command has to state which workflow it enters instead of
/// inheriting a wrong answer.
const COMMAND_OVERRIDES: &[(&str, &str)] = &[
    // → `gpke-abrechnung` (NN-/Abschlags-/MMM-Rechnung, PIDs 31001/31002/31005).
    ("invoic.nne.stellen", "gpke"),
    ("invoic.nne-abschlag.stellen", "gpke"),
    ("invoic.mmm.stellen", "gpke"),
    // → `wim-invoic` (Stornorechnung 31004 against an MSB invoice).
    ("invoic.stornorechnung.annehmen", "wim"),
    ("invoic.stornorechnung.ablehnen", "wim"),
    // → `geli-gas-sperrprozesse-invoic` (Rechnung sonstige Leistung 31011).
    ("invoic.sonstige-leistung.stellen", "geli-gas"),
    ("invoic.sonstige-leistung.annehmen", "geli-gas"),
    ("invoic.sonstige-leistung.ablehnen", "geli-gas"),
];

/// The label for a family nothing in the table claims.
///
/// Never the empty string: a blank label is invisible in a legend, and the
/// completion counter used to emit one for every message that carried no
/// workflow name.
pub const UNKNOWN: &str = "other";

/// The `family` label for a workflow name.
///
/// Longest prefix wins, so `geli-gas-lieferbeginn` is `geli-gas` and not
/// whatever a shorter `geli-` row would say.
#[must_use]
pub fn from_workflow(workflow_name: &str) -> &'static str {
    FAMILIES
        .iter()
        .filter(|f| {
            f.workflow_prefix
                .is_some_and(|p| workflow_name.starts_with(p))
        })
        .max_by_key(|f| f.workflow_prefix.map_or(0, str::len))
        .map_or(UNKNOWN, |f| f.label)
}

/// The `family` label for an ERP command name.
///
/// The same string [`from_workflow`] returns for the workflow this command
/// starts — that is the whole point of the shared table.
#[must_use]
pub fn from_command(command: &str) -> &'static str {
    if let Some((_, label)) = COMMAND_OVERRIDES.iter().find(|(name, _)| *name == command) {
        return label;
    }
    let prefix = command.split('.').next().unwrap_or_default();
    FAMILIES
        .iter()
        .find(|f| f.command_prefixes.contains(&prefix))
        .map_or(UNKNOWN, |f| f.label)
}

#[cfg(test)]
mod tests {
    use super::{FAMILIES, UNKNOWN, from_command, from_workflow};

    /// The join that was broken: the label a command initiates under is the
    /// label its workflow completes under.
    #[test]
    fn the_two_derivations_agree_on_the_gas_families() {
        assert_eq!(from_command("geli.lieferbeginn.anmelden"), "geli-gas");
        assert_eq!(from_workflow("geli-gas-lieferbeginn"), "geli-gas");
        assert_eq!(
            from_command("geli.lieferbeginn.anmelden"),
            from_workflow("geli-gas-lieferbeginn"),
        );
        assert_eq!(
            from_command("gabi.invoic.senden"),
            from_workflow("gabi-gas-invoic"),
        );
    }

    #[test]
    fn an_invoic_command_takes_the_family_of_the_workflow_it_enters() {
        // One message type, three workflows — the reason these are keyed on the
        // full command name.
        assert_eq!(from_command("invoic.nne.stellen"), "gpke");
        assert_eq!(from_command("invoic.stornorechnung.annehmen"), "wim");
        assert_eq!(from_command("invoic.sonstige-leistung.stellen"), "geli-gas");
    }

    #[test]
    fn an_unstamped_workflow_name_is_not_a_blank_label() {
        assert_eq!(from_workflow(""), UNKNOWN);
        assert_eq!(from_workflow("something-new"), UNKNOWN);
        assert_eq!(from_command("something.new"), UNKNOWN);
    }

    /// Every workflow the engine can actually run is claimed by a row.
    ///
    /// `production_modules()` is the list makod builds its engine from, so a
    /// workflow it names is one that can complete and be counted. One that no
    /// row claims is counted as `other`, which is where a whole domain's
    /// completions go to be invisible.
    #[test]
    fn every_workflow_the_registry_knows_has_a_family() {
        let mut checked = 0_usize;
        for module in crate::startup::production_modules() {
            for name in module.workflow_names() {
                assert_ne!(
                    from_workflow(name),
                    UNKNOWN,
                    "workflow '{name}' matches no row of FAMILIES, so every                      process of that family completes under family=\"{UNKNOWN}\""
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no workflow names — the extractor has drifted");
    }

    /// Each family's workflow prefix really is a prefix of a real workflow name.
    ///
    /// The prefix is what makes the two derivations meet: `command_prefixes`
    /// answers the initiation counter and `workflow_prefix` the completion
    /// counter, and they only produce the same label if the prefix matches the
    /// names the engine actually uses. `geli-` instead of `geli-gas-` is the
    /// shape of mistake this catches — it still matches, but a `geli-gas-…`
    /// name would then be claimed by whichever row is longer.
    #[test]
    fn every_family_prefix_names_workflows_that_exist() {
        let names: Vec<String> = crate::startup::production_modules()
            .iter()
            .flat_map(|m| m.workflow_names().iter().map(|n| (*n).to_owned()))
            .collect();
        for family in FAMILIES {
            let Some(prefix) = family.workflow_prefix else {
                continue;
            };
            assert!(
                names.iter().any(|n| n.starts_with(prefix)),
                "family '{}' claims the workflow prefix '{prefix}', which no                  workflow in the registry starts with",
                family.label
            );
        }
    }

    /// Every command's family is one a workflow completes under.
    ///
    /// This is the join that was broken: `geli.*` counted initiations under
    /// `geli-gas` while `geli-gas-*` workflows counted completions under
    /// `geli`, so the two series never met and the family read as "starts
    /// processes, finishes none". The exception is a family that runs no
    /// workflow at all, which the table has to declare.
    #[test]
    fn every_command_family_is_one_a_workflow_completes_under() {
        let completed: std::collections::BTreeSet<&'static str> =
            crate::startup::production_modules()
                .iter()
                .flat_map(|m| m.workflow_names().iter().map(|n| from_workflow(n)))
                .collect();
        for desc in crate::orchestrator::commands_api::COMMAND_REGISTRY {
            let label = from_command(desc.name);
            let workflowless = FAMILIES
                .iter()
                .any(|f| f.label == label && f.workflow_prefix.is_none());
            assert!(
                completed.contains(label) || workflowless,
                "command '{}' counts initiations under family=\"{label}\", which                  no workflow completes under — the two counters cannot join",
                desc.name
            );
        }
    }

    /// Nothing derives the family label a second time.
    ///
    /// Both defects were a private `workflow_name.split('-')`. The table makes
    /// the two derivations agree only for as long as they are the only two.
    #[test]
    fn no_other_site_derives_the_family_label() {
        const SITES: &[(&str, &str)] = &[
            (
                "core/erp_adapter.rs",
                include_str!("../core/erp_adapter.rs"),
            ),
            (
                "orchestrator/deadline_dispatch.rs",
                include_str!("deadline_dispatch.rs"),
            ),
            (
                "orchestrator/commands_api/handler.rs",
                include_str!("commands_api/handler.rs"),
            ),
            (
                "orchestrator/commands_api/types.rs",
                include_str!("commands_api/types.rs"),
            ),
        ];
        for (file, src) in SITES {
            for (n, line) in src.lines().enumerate() {
                assert!(
                    line.trim_start().starts_with("//")
                        || !(line.contains(".split('-')") || line.contains(".split(\"-\")")),
                    "{file}:{} splits a name to derive a label — call                      `process_family::from_workflow` instead",
                    n + 1
                );
            }
        }
    }

    #[test]
    fn every_label_is_distinct() {
        let mut labels: Vec<&str> = FAMILIES.iter().map(|f| f.label).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(before, labels.len(), "two families share a label");
        assert!(
            !labels.contains(&UNKNOWN),
            "'{UNKNOWN}' is the fallback, not a family"
        );
    }
}
