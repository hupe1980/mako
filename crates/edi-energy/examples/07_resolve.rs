//! Show where every segment of a message sits in the MIG's Nachrichtenstruktur
//! and what the MIG/AHB checks say about it — the "why is this rejected" view.
//!
//! ```text
//! cargo run --example 07_resolve --all-features -- path/to/message.edi
//! cargo run --example 07_resolve --all-features -- --structure UTILMD S2.2
//! cargo run --example 07_resolve --all-features -- --pruefschablone UTILMD S2.2 55001
//! cargo run --example 07_resolve --all-features -- --skeleton UTILMD S2.2 55001
//! cargo run --example 07_resolve --all-features -- --skeleton CONTRL 2.0b '#1'
//! ```

use edi_energy::profile::structure::Kind;
use edi_energy::{EdiEnergyMessage, MessageType, Platform, Release, ReleaseRegistry};

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
                "usage: 07_resolve <message.edi> | --structure <TYPE> <RELEASE> | --pruefschablone <TYPE> <RELEASE> <PID> | --skeleton <TYPE> <RELEASE> [PID]"
            );
            Ok(())
        }
    }
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
