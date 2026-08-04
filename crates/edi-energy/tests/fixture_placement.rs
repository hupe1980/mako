//! A fixture must live under a message type that actually declares its PID.
//!
//! `iftsta/valid/pid_44001.edi` was an IFTSTA interchange announcing
//! Prüfidentifikator **44001** — a UTILMD Gas code that no IFTSTA AHB defines.
//! It parsed, so nothing rejected it, and it counted toward `validate-pruefids`
//! coverage while asserting a message-type/PID pairing that does not exist.
//!
//! The check reads the shipped profiles rather than a hardcoded PID→type map:
//! the bands overlap in practice (29xxx is shared by APERAK and COMDIS, and
//! `comdis/valid/pid_29002.edi` is correct precisely because both profiles
//! declare 29002), so only the profiles can settle it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

fn profiles_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/profiles"))
}

/// `message type -> every PID any of its shipped profiles declares`.
fn declared_pids() -> BTreeMap<String, BTreeSet<u32>> {
    let mut out: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let root = profiles_root();
    for ty in std::fs::read_dir(&root).expect("profiles dir").flatten() {
        if !ty.path().is_dir() {
            continue;
        }
        let type_name = ty.file_name().to_string_lossy().into_owned();
        for release in std::fs::read_dir(ty.path()).into_iter().flatten().flatten() {
            let ahb = release.path().join("ahb.json");
            let Ok(raw) = std::fs::read_to_string(&ahb) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            for e in v["pruefidentifikatoren"].as_array().into_iter().flatten() {
                if let Some(code) = e["code"].as_u64() {
                    out.entry(type_name.clone())
                        .or_default()
                        .insert(code as u32);
                }
            }
        }
    }
    out
}

/// Every `pid_<code>` fixture, as `(path, message type dir, code)`.
fn named_fixtures(dir: &Path, out: &mut Vec<(PathBuf, String, u32)>) {
    for ty in std::fs::read_dir(dir).expect("fixtures dir").flatten() {
        if !ty.path().is_dir() {
            continue;
        }
        let type_name = ty.file_name().to_string_lossy().into_owned();
        let mut stack = vec![ty.path()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(rest) = name.strip_prefix("pid_") else {
                    continue;
                };
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if let Ok(code) = digits.parse::<u32>() {
                    out.push((p, type_name.clone(), code));
                }
            }
        }
    }
}

#[test]
fn every_fixture_sits_under_a_message_type_that_declares_its_pid() {
    let declared = declared_pids();
    let mut fixtures = Vec::new();
    named_fixtures(&fixtures_root(), &mut fixtures);

    assert!(
        fixtures.len() > 100,
        "only {} named fixtures found — the scan is broken",
        fixtures.len()
    );

    let mut misfiled: Vec<String> = Vec::new();
    for (path, ty, code) in &fixtures {
        // CONTRL carries no Prüfidentifikatoren at all, so its fixtures are
        // named for the case they cover rather than a PID.
        let Some(pids) = declared.get(ty) else {
            continue;
        };
        if !pids.contains(code) {
            let owners: Vec<&String> = declared
                .iter()
                .filter(|(_, set)| set.contains(code))
                .map(|(t, _)| t)
                .collect();
            misfiled.push(format!(
                "  {} — {ty} declares no PID {code}; declared by {owners:?}",
                path.strip_prefix(fixtures_root()).unwrap_or(path).display()
            ));
        }
    }

    assert!(
        misfiled.is_empty(),
        "these fixtures claim a message-type/Prüfidentifikator pairing no shipped \
         profile defines, so they assert something the AHB does not:\n{}",
        misfiled.join("\n")
    );
}
