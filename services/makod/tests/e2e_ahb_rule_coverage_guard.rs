//! Guard: every routed Prüfidentifikator must carry AHB rules.
//!
//! For a Prüfidentifikator the profile has no column for, validation raises
//! `AHB-UNKNOWN-PID` as a **warning** and applies no AHB rules;
//! `report.is_valid()` stays `true`, so a PID that mako routes but that never
//! made it into the profile passes the AHB layer unchecked. The MIG layer
//! still applies, and the warning is the only trace.
//!
//! That is not hypothetical. ORDERS 17008/17116/17117 were lost because only the
//! first column of each multi-PID AHB table survived import, and the UTILMD
//! profiles are missing a large block of published PIDs for the same reason.
//! `validate-profiles` catches a PID *lost between releases*; it cannot catch one
//! that was never imported. This test closes that hole by cross-checking the
//! routers — the set of PIDs mako actually accepts — against the profiles.
//!
//! Uses only committed code and profiles: no PDFs, no network, CI-safe.

use std::collections::BTreeMap;

use edi_energy::{MessageType, Platform, Pruefidentifikator};
use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;

/// PIDs deliberately routed without AHB rules, with the reason.
///
/// Keep this empty where possible. An entry is a promise that the PID's rules
/// are genuinely not applicable — not a place to park an import gap.
const RULELESS_BY_DESIGN: &[(u32, &str)] = &[
    // CONTRL is a technical acknowledgement: the AHB defines no
    // Prüfidentifikatoren for it at all (its profiles are `pid_exempt`).
];

/// Routed PIDs whose AHB column is not in the profiles — an acknowledged
/// backlog. Every column of every AHB in `profiles/sources.json` is imported,
/// so this is empty; a PID *not* in this list that lacks rules is a regression
/// (a column the importer lost, or a PID routed that no AHB defines).
const KNOWN_PROFILE_GAPS: &[u32] = &[];

/// Message type a PID belongs to, from its leading digits.
///
/// 29xxx is declared by both APERAK and COMDIS. Either resolves the same way
/// here — both profiles carry identical rules for 29001/29002 — so the coarse
/// mapping is sufficient for a has-rules check.
fn message_type_of(pid: u32) -> Option<MessageType> {
    Some(match pid / 1000 {
        13 => MessageType::Mscons,
        15 => MessageType::Quotes,
        17 => MessageType::Orders,
        19 => MessageType::Ordrsp,
        21 => MessageType::Iftsta,
        23 => MessageType::Insrpt,
        25 => MessageType::Utilts,
        27 => MessageType::Pricat,
        29 => MessageType::Aperak,
        31 => MessageType::Invoic,
        33 => MessageType::Remadv,
        35 => MessageType::Reqote,
        37 => MessageType::Partin,
        39 => MessageType::Ordchg,
        44 | 55 => MessageType::Utilmd,
        _ => return None,
    })
}

#[tokio::test]
async fn every_routed_pid_has_ahb_rules() {
    // The one production list — see `makod::startup::production_modules`. Never
    // restate the stack here: a guard with its own copy silently stops seeing a
    // module the daemon registers.
    let modules: Vec<Box<dyn EngineModule>> = makod::startup::production_modules();
    let roles = DeploymentRoles::all();
    let platform = Platform::with_all_profiles();

    // (pid -> workflow) across every module, each in its own router so the
    // deliberate cross-module PID overlaps do not collide.
    let mut routed: BTreeMap<u32, String> = BTreeMap::new();
    for m in &modules {
        let mut router = PidRouter::new();
        m.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if let Some(wf) = router.route(pid) {
                routed.entry(pid).or_insert_with(|| wf.to_owned());
            }
        }
        for (pid, _sparte, wf) in router.registered_commodity_entries() {
            routed.entry(pid).or_insert_with(|| wf.to_owned());
        }
    }
    assert!(!routed.is_empty(), "expected registered PIDs");

    let exempt: BTreeMap<u32, &str> = RULELESS_BY_DESIGN.iter().copied().collect();
    let mut gaps: Vec<String> = Vec::new();
    let mut with_rules = 0usize;
    let mut closed: Vec<u32> = Vec::new();

    for (pid, workflow) in &routed {
        if exempt.contains_key(pid) {
            continue;
        }
        let Some(mt) = message_type_of(*pid) else {
            continue;
        };
        let Ok(p) = Pruefidentifikator::new(*pid) else {
            continue;
        };

        // A PID has rules when *some* shipped profile of its message type
        // defines them. Release selection is a separate concern
        // (`validate-profiles` enforces continuity); the question here is only
        // whether the PID was ever imported at all.
        //
        // The discriminator is the pack *name*, not its rule count: the
        // stand-in for an unknown PID carries exactly one (warning) rule, so
        // counting rules would accept every gap.
        let has_rules = platform
            .registry()
            .profiles_for(mt)
            .any(|prof| prof.has_anwendungsfall(p));

        if has_rules {
            with_rules += 1;
        }
        match (has_rules, KNOWN_PROFILE_GAPS.contains(pid)) {
            (false, false) => gaps.push(format!("  {pid} (routed by `{workflow}`, {mt:?})")),
            (true, true) => closed.push(*pid),
            _ => {}
        }
    }

    // `site/templates/index.html` states this next to the routed count, and the
    // figure is read out of the page rather than restated here: a constant
    // claiming what the page says drifts the moment the page is edited alone.
    // The two describe the same catalogue from different sides, so a PID gaining
    // rules moves the page.
    let advertised = landing_page_pids_with_rules();
    assert_eq!(
        with_rules, advertised,
        "site/templates/index.html says {advertised} routed PIDs carry AHB rules, \
         the engine has {with_rules} — update the page"
    );

    assert!(
        closed.is_empty(),
        "these PIDs now have AHB rules — remove them from KNOWN_PROFILE_GAPS so the \
         list keeps shrinking: {closed:?}"
    );

    assert!(
        gaps.is_empty(),
        "these PIDs are routed but have no AHB rules in any shipped profile, so \
         inbound messages carrying them are validated against an empty rule pack \
         and silently pass:\n{}\n\nRe-import the profile from the BDEW AHB (see \
         `cargo xtask extract-pdf`). If a PID is genuinely rule-free, add it to \
         RULELESS_BY_DESIGN with the reason.",
        gaps.join("\n")
    );
}

/// The landing page's „N additionally carry validated AHB segment rules" figure.
///
/// Read out of `site/templates/index.html`, because a constant restating it is a
/// claim rather than a check.
fn landing_page_pids_with_rules() -> usize {
    const SENTENCE: &str = "additionally carry";
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/templates/index.html");
    let html = std::fs::read_to_string(&path).expect("the landing page template");
    let (before, _) = html
        .split_once(SENTENCE)
        .unwrap_or_else(|| panic!("{} no longer says \"{SENTENCE}\"", path.display()));
    let (_, number) = before
        .rsplit_once("<strong>")
        .and_then(|(_, tail)| tail.split_once("</strong>").map(|(n, r)| (r, n)))
        .unwrap_or_else(|| panic!("no <strong>N</strong> before \"{SENTENCE}\""));
    number
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("{number:?} is not a count: {e}"))
}
