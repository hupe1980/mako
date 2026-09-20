//! Show where every segment of a message sits in the MIG's Nachrichtenstruktur
//! and what the MIG/AHB checks say about it — the "why is this rejected" view.
//!
//! With no argument it walks the shipped profiles and builds a **skeleton** for
//! every Anwendungsfall — the minimal message its Prüfschablone admits — then
//! validates each one against that same Prüfschablone. That is the tool's own
//! proof: a profile whose Muss places cannot be satisfied by any message is a
//! profile no counterparty can send to, and the loop finds it.
//!
//! ```text
//! cargo run --example 07_resolve --all-features
//! cargo run --example 07_resolve --all-features -- path/to/message.edi
//! cargo run --example 07_resolve --all-features -- --structure UTILMD S2.1
//! cargo run --example 07_resolve --all-features -- --pruefschablone UTILMD S2.1 55001
//! cargo run --example 07_resolve --all-features -- --skeleton UTILMD S2.1 55001
//! cargo run --example 07_resolve --all-features -- --skeleton CONTRL 2.0b '#1'
//! ```

use edi_energy::profile::structure::Kind;
use edi_energy::{EdiEnergyMessage, MessageType, Platform, Release, ReleaseRegistry};

/// Declares that this example prints validation findings on its happy path.
///
/// The sweep below is the same one
/// `tests/skeletons.rs::every_anwendungsfall_has_a_conformant_skeleton`
/// asserts, and that test is where the invariant is enforced: it names the
/// places the generator cannot yet reach, one rule id each, and refuses both a
/// new one and a stale one. Asserting here as well would mean keeping that
/// list in two files, and the copy that drifts is the one nobody runs on its
/// own — so this example reports and the test decides.
///
/// Reporting means printing rule ids, which is what `just examples` scans for,
/// so it opts out of that scan by name the way `05_validate` does. What is
/// left unguarded here is guarded there.
const _EXAMPLE_EXPECTS_VALIDATION_FINDINGS: () = ();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--structure") => {
            let profile = profile(&args[1], &args[2])?;
            let s = &profile.structure;
            for (id, node) in s.nodes.iter().enumerate() {
                let indent = "  ".repeat(s.path(id).len());
                match &node.kind {
                    Kind::Group { group } => println!(
                        "{indent}{group} {} max {} — {}",
                        node.status, node.max, node.name
                    ),
                    Kind::Segment {
                        nr,
                        tag,
                        discriminators,
                        ..
                    } => {
                        let codes: Vec<String> = discriminators
                            .iter()
                            .map(|d| format!("{}.{}={}", d.element, d.component, d.codes.join("/")))
                            .collect();
                        println!(
                            "{indent}  {nr} {tag} {} max {} — {} [{}]",
                            node.status,
                            node.max,
                            node.name,
                            codes.join(" ")
                        );
                    }
                }
            }
            Ok(())
        }
        Some("--skeleton") => {
            let profile = profile(&args[1], &args[2])?;
            // A Prüfidentifikator, or `#n` for the n-th column of a message
            // type published without.
            let af = match args.get(3).map(String::as_str) {
                Some(s) if s.starts_with('#') => profile
                    .anwendungsfaelle()
                    .get(s[1..].parse::<usize>()?)
                    .ok_or("no such column")?,
                Some(s) => profile
                    .anwendungsfall(s.parse()?)
                    .ok_or("no such Anwendungsfall")?,
                None => profile
                    .anwendungsfaelle()
                    .first()
                    .ok_or("no Anwendungsfall")?,
            };
            let segs = profile.skeleton(af, &edi_energy::profile::SkeletonParties::default());
            let bytes = edifact_rs::segments_to_bytes(&segs)?;
            let text = String::from_utf8_lossy(&bytes).replace('\'', "'\n");
            print!("{text}");
            let issues = profile.validate(
                &segs,
                af.pid
                    .and_then(|p| edi_energy::Pruefidentifikator::new(p).ok()),
            );
            println!("\n{} issue(s)", issues.len());
            for i in &issues {
                println!("  [{}] {}", i.rule_id().unwrap_or("-"), i.message);
            }
            Ok(())
        }
        Some("--pruefschablone") => {
            let profile = profile(&args[1], &args[2])?;
            let pid: u32 = args[3].parse()?;
            match profile.pruefschablone(pid) {
                Some(p) => print!("{p}"),
                None => println!("{} {} has no Anwendungsfall {pid}", args[1], args[2]),
            }
            Ok(())
        }
        Some(path) => {
            let bytes = std::fs::read(path)?;
            let platform = Platform::with_all_profiles();
            let msg = platform.parse(&bytes)?;
            let release = msg.detect_release()?.clone();
            let mt = msg.try_message_type().ok_or("unknown message type")?;
            let date = platform
                .registry()
                .profiles_for(mt)
                .filter(|p| p.release() == &release)
                .filter_map(|p| p.valid_from())
                .max()
                .unwrap_or(time::Date::MAX);
            let profile = platform.registry().profile_on(mt, &release, date)?;
            let segments: Vec<_> = msg
                .segments()
                .iter()
                .filter(|s| !matches!(&*s.tag, "UNB" | "UNZ" | "UNG" | "UNE"))
                .cloned()
                .collect();
            let res = profile.resolve(&segments);
            println!("{} {} — {} segments", mt, release, segments.len());
            for (i, seg) in segments.iter().enumerate() {
                let wire =
                    edifact_rs::segments_to_bytes(std::slice::from_ref(seg)).unwrap_or_default();
                let wire = String::from_utf8_lossy(&wire);
                match res.assigned[i] {
                    Some(a) => {
                        let path = profile.structure.path(a.node).join("/");
                        println!(
                            "{i:>3} {:<9} {:<12} {:<22} {}",
                            profile.structure.nr(a.node).unwrap_or("?"),
                            path,
                            profile.structure.nodes[a.node]
                                .name
                                .chars()
                                .take(22)
                                .collect::<String>(),
                            wire.trim_end()
                        );
                    }
                    None => println!(
                        "{i:>3} {:<9} {:<12} {:<22} {}",
                        "?",
                        "",
                        "UNRESOLVED",
                        wire.trim_end()
                    ),
                }
            }
            let report = msg.validate_on_date(date)?;
            println!(
                "\n{} error(s), {} warning(s)",
                report.errors().len(),
                report.warnings().len()
            );
            for issue in report.iter_issues() {
                println!("  [{}] {}", issue.rule_id().unwrap_or("-"), issue.message);
            }
            Ok(())
        }
        None => {
            eprintln!(
                "usage: 07_resolve <message.edi> | --structure <TYPE> <RELEASE> \
                 | --pruefschablone <TYPE> <RELEASE> <PID> | --skeleton <TYPE> <RELEASE> [PID]\n\
                 no argument: build and validate a skeleton for every shipped Anwendungsfall\n"
            );
            skeleton_sweep()
        }
    }
}

/// Build the skeleton of every Anwendungsfall of every shipped profile and hold
/// each against its own Prüfschablone.
///
/// The skeleton is derived from the profile — it emits exactly the places the
/// column marks Muss — so a skeleton that does not validate means the profile
/// contradicts itself: a Muss place whose own Voraussetzung the rest of the
/// message cannot satisfy. That is a defect nothing else finds, because every
/// other check starts from a message somebody wrote.
fn skeleton_sweep() -> Result<(), Box<dyn std::error::Error>> {
    let reg = ReleaseRegistry::global();
    let mut checked = 0_usize;
    let mut failed = 0_usize;
    let mut by_type: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();

    for profile in reg.all_profiles() {
        let key = format!("{} {}", profile.message_type(), profile.release().as_str());
        for af in profile.anwendungsfaelle() {
            let segs = profile.skeleton(af, &edi_energy::profile::SkeletonParties::default());
            let pid = af
                .pid
                .and_then(|p| edi_energy::Pruefidentifikator::new(p).ok());
            let issues = profile.validate(&segs, pid);
            checked += 1;
            let entry = by_type.entry(key.clone()).or_insert((0, 0));
            entry.0 += 1;
            if !issues.is_empty() {
                failed += 1;
                entry.1 += 1;
                let name = af
                    .pid
                    .map_or_else(|| af.name.clone(), |p: u32| p.to_string());
                println!("  FAIL {key} {name} — {} issue(s)", issues.len());
                for i in &issues {
                    println!("       [{}] {}", i.rule_id().unwrap_or("-"), i.message);
                }
            }
        }
    }

    for (key, (n, bad)) in &by_type {
        println!("  {key:<18} {n:>4} Anwendungsfälle, {bad} failing");
    }
    println!("\n{checked} Anwendungsfälle checked, {failed} failing");

    // ── The ratchet ──────────────────────────────────────────────────────────
    //
    // This sweep is the outbound counterpart of validation: it renders the
    // minimal message of **every** Anwendungsfall of every shipped profile and
    // resolves it against its own Prüfschablone, so every
    // `AHB-<pid>-…-MISSING` here is a field a sender would have to source.
    //
    // Printing the number proves nothing on its own — a regression that doubles
    // it still exits `0` and still reads like a report. The count is therefore
    // asserted, and the two that remain are named rather than tolerated in bulk:
    // UTILMD S2.1 and S2.2 PID 55235, whose `SG10 Zuordnungs-Regel des ZP der
    // NGZ zur NZR` the generator's fixpoint does not force in. Fixing that is
    // generator work; letting a third case join them silently is not something
    // the fix should have to compete with.
    const KNOWN_FAILING: usize = 2;
    assert!(
        failed <= KNOWN_FAILING,
        "{failed} Anwendungsfälle fail to render a conformant skeleton, up from the \
         {KNOWN_FAILING} known (UTILMD S2.1/S2.2 55235, SG10). Every one of these is a \
         message mako would send incomplete — resolve the new ones, or move the number \
         here deliberately and say which case joined and why."
    );
    assert!(
        failed == KNOWN_FAILING,
        "only {failed} Anwendungsfälle now fail, below the {KNOWN_FAILING} recorded here. \
         Lower the ratchet in the same change that fixed them, so it keeps biting."
    );
    assert!(
        checked > 900,
        "the sweep reached only {checked} Anwendungsfälle — a shrunken corpus passes this \
         for the wrong reason"
    );

    Ok(())
}

fn profile(
    mt: &str,
    release: &str,
) -> Result<&'static edi_energy::Profile, Box<dyn std::error::Error>> {
    let mt = MessageType::from_unh_code(&mt.to_ascii_uppercase()).ok_or("unknown message type")?;
    let release = Release::new(release);
    let reg = ReleaseRegistry::global();
    let date = reg
        .profiles_for(mt)
        .filter(|p| p.release() == &release)
        .filter_map(|p| p.valid_from())
        .max()
        .unwrap_or(time::Date::MAX);
    Ok(reg.profile_on(mt, &release, date)?)
}
