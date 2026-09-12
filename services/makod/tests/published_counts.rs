//! Guard: the workflow and Prüfidentifikator counts the published docs state
//! must be the counts the daemon actually registers.
//!
//! Two claims live here. "71 workflows over 469 Prüfidentifikatoren" appears in the
//! landing docs, in the `makod` operator guide and in the `agentd` concept as
//! the scope claim a reader sizes the platform by. Nothing held it to the
//! registry, so registering a workflow or routing a PID moved the truth and
//! left five documents stating the old number — the same drift
//! `xtask check-bo4e-coverage` exists to stop for the BO4E type count.
//!
//! Both numbers come from the one production module list
//! (`makod::startup::production_modules`) with every Marktrolle active, which is
//! the widest configuration and therefore the one a scope claim describes.
//!
//! The second is the answer-obligation catalogue and its two splits. A total on
//! its own does not hold them: an obligation that moves between family tables
//! leaves the total right and two family figures wrong, which is how "GPKE 28,
//! WIM 20" outlived a catalogue holding 29 and 19.

use std::collections::{BTreeMap, BTreeSet};

use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;

/// Documents carrying the claim, and the phrase it appears in.
const CLAIMANTS: &[&str] = &[
    // The landing page states both numbers as headline stats, which is where a
    // stale one is seen most and noticed least. `e2e_ahb_rule_coverage_guard`
    // reads its „N additionally carry AHB rules" figure out of the same file.
    "../../site/templates/index.html",
    "../../README.md",
    "../../site/content/docs/services/_index.md",
    "../../site/content/docs/services/makod.md",
    // makod's own README states both numbers and was not a claimant, so the
    // service most likely to be read by someone changing the registry was the
    // one document free to drift.
    "README.md",
    "../../site/content/docs/architecture/_index.md",
    "../../concepts/AGENTD.md",
    // States the same two figures plus a per-family breakdown that has to sum
    // to them, which is how it drifted while every other claimant held.
    "../../concepts/MARKET_LANDSCAPE.md",
    // Names this guard as what holds it, which only became true once it was
    // listed here. It writes both figures in bold, which is why `collapse`
    // strips emphasis.
    "../../site/content/docs/architecture/domain-model.md",
];

/// Markup out, one space in its place.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut depth = 0usize;
    for ch in html.chars() {
        match ch {
            '<' => {
                depth += 1;
                out.push(' ');
            }
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Runs of whitespace to a single space, so a claim split across lines still
/// reads as one phrase — and Markdown emphasis out, so `**469** PIDs` is the
/// same claim as `469 PIDs`.
///
/// A literal `contains` is blind to emphasis, and a bolded figure is exactly how
/// a document states a headline number: the phrase then never matches, and the
/// claimant silently stops being checked while still reading as checked.
fn collapse(text: &str) -> String {
    text.replace("**", "")
        .replace('*', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn registered() -> (usize, usize) {
    let modules: Vec<Box<dyn EngineModule>> = makod::startup::production_modules();
    let roles = DeploymentRoles::all();

    let mut pids: BTreeMap<u32, String> = BTreeMap::new();
    let mut workflows: BTreeSet<String> = BTreeSet::new();
    for module in &modules {
        // One router per module: the deliberate cross-module PID overlaps would
        // otherwise collide.
        let mut router = PidRouter::new();
        module.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if let Some(wf) = router.route(pid) {
                pids.entry(pid).or_insert_with(|| wf.to_owned());
            }
        }
        for (pid, _sparte, wf) in router.registered_commodity_entries() {
            pids.entry(pid).or_insert_with(|| wf.to_owned());
        }
        for name in module.workflow_names() {
            workflows.insert((*name).to_owned());
        }
    }
    (workflows.len(), pids.len())
}

#[test]
fn every_published_scope_claim_matches_the_registry() {
    let (workflows, pids) = registered();
    let mut missing = Vec::new();

    for path in CLAIMANTS {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let Ok(raw) = std::fs::read_to_string(&full) else {
            // `concepts/` is not in git, so a checkout without it is normal and
            // only costs this guard the claims that live there. Every tracked
            // claimant must still be present.
            assert!(
                path.contains("/concepts/"),
                "{} is a tracked claimant and must be readable",
                full.display()
            );
            eprintln!("skipping: {} is not present", full.display());
            continue;
        };
        // The landing page states both numbers as markup
        // (`<strong>469</strong><span>Prüfidentifikatoren routed</span>`), so tags
        // become spaces and runs of whitespace collapse. On a Markdown file this
        // is a no-op.
        let text = collapse(&strip_tags(&raw));

        // "Prüfidentifikatoren" and "PIDs" are the same claim in different
        // registers; either spelling counts, and a document must carry both
        // numbers, or the claim is half-stated.
        for (count, units) in [
            (workflows, &["workflows", "MaKo workflows"][..]),
            (pids, &["Prüfidentifikatoren", "PIDs"][..]),
        ] {
            if units.iter().any(|u| text.contains(&format!("{count} {u}"))) {
                continue;
            }
            // Report what the file does say, so the fix is a substitution
            // rather than a search.
            let words: Vec<&str> = text.split_whitespace().collect();
            let stated: Vec<&str> = words
                .windows(2)
                .filter(|w| units.contains(&w[1].trim_end_matches([',', '.', ')', '*'])))
                .map(|w| w[0].trim_start_matches('*'))
                .collect();
            missing.push(format!(
                "{path}: expected `{count} {}`, file states {stated:?}",
                units[0]
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "the published scope claim has drifted from the registry \
         ({workflows} workflows, {pids} Prüfidentifikatoren):\n  {}",
        missing.join("\n  ")
    );
}

/// The answer-obligation catalogue, split the way the docs split it.
///
/// A total alone does not hold: `mako_fristen::antwort::all()` folds five
/// family tables, and an obligation moved from one to another leaves the total
/// right and both family figures wrong — which is what happened to GPKE and
/// WiM. Both the per-family and the per-answer-message split are stated, so
/// both are checked.
#[test]
fn the_answer_obligation_split_matches_the_catalogue() {
    use mako_fristen::antwort::Family;

    let mut by_family: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut by_message: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut total = 0usize;
    for obligation in mako_fristen::antwort::all() {
        total += 1;
        *by_family
            .entry(match obligation.family {
                Family::Gpke => "GPKE",
                Family::Wim => "WiM",
                Family::GeliGas => "GeLi Gas",
                Family::WimGas => "WiM Gas",
                Family::Emob => "EMob",
            })
            .or_default() += 1;
        // The answer's Prüfidentifikator names its message type by range; the
        // docs quote the same split.
        *by_message
            .entry(match obligation.antwort_pids.0 / 1000 {
                55 | 44 => "UTILMD",
                19 => "ORDRSP",
                21 => "IFTSTA",
                15 => "QUOTES",
                17 => "ORDERS",
                13 => "MSCONS",
                33 => "REMADV",
                other => panic!("PID range {other}xxx has no message type in this guard"),
            })
            .or_default() += 1;
    }

    // `concepts/` is not in git; the site page is, and states the message split.
    let claims: &[(&str, String)] = &[
        (
            "../../concepts/MAKOTEST.md",
            format!(
                "into **{total}** obligations (GPKE {}, WiM {}, GeLi Gas {}, WiM Gas {}, \
                 EMob {})",
                by_family["GPKE"],
                by_family["WiM"],
                by_family["GeLi Gas"],
                by_family["WiM Gas"],
                by_family["EMob"],
            ),
        ),
        (
            "../../site/content/docs/reference/makotest.md",
            format!(
                "All **{total}** published obligations are answerable — UTILMD {}, ORDRSP {}, \
                 IFTSTA {}, QUOTES {}, ORDERS {}",
                by_message["UTILMD"],
                by_message["ORDRSP"],
                by_message["IFTSTA"],
                by_message["QUOTES"],
                by_message["ORDERS"],
            ),
        ),
    ];

    for (path, claim) in claims {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let Ok(raw) = std::fs::read_to_string(&full) else {
            assert!(
                path.contains("/concepts/"),
                "{} is a tracked claimant and must be readable",
                full.display()
            );
            eprintln!("skipping: {} is not present", full.display());
            continue;
        };
        // The claim goes through `collapse` too: it is written the way the
        // document writes it, emphasis and all, and the comparison is on the
        // text both sides reduce to.
        assert!(
            collapse(&raw).contains(collapse(claim).as_str()),
            "{path} must state `{claim}`"
        );
    }
}
