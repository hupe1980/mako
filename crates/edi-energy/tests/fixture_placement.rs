//! A fixture must live under a message type that actually declares its PID.
//!
//! `iftsta/valid/pid_44001.edi` was an IFTSTA interchange announcing
//! Prüfidentifikator **44001** — a UTILMD Gas code that no IFTSTA AHB defines.
//! It parsed, so nothing rejected it, while asserting a message-type/PID
//! pairing that does not exist.
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
                let Some(rest) = name
                    .strip_prefix("beispiel_")
                    .or_else(|| name.strip_prefix("pid_"))
                else {
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
        fixtures.len() > 50,
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

/// `SG6 RFF+Z13` DE 1154 carries the **Prüfidentifikator**, not a Vorgangsnummer.
///
/// The UTILMD AHB gives the element one value per Anwendungsfall — Strom 2.2
/// Kap. 10.1 prints „`SG6 RFF 1154  55039 WiM Strom / Kündigung MSB`" against
/// the Kündigung column and `55040`/`55041` against the two answers; Gas 1.2
/// Kap. 6 does the same for 44039–44041. DE 1154 is `R n5` there, so nothing
/// but the five-digit code belongs in it. The Vorgangsnummer lives in `IDE+24`
/// DE 7402 and comes back in `SG6 RFF+TN`.
///
/// Checked on the WiM MSB-Wechsel fixtures, whose AHB columns are quoted above.
/// Other families state DE 1154 differently and are out of this test's scope.
#[test]
fn wim_fixtures_put_the_pruefidentifikator_in_rff_z13() {
    const WIM_MSB_WECHSEL: &[u32] = &[
        55_039, 55_040, 55_041, 55_042, 55_043, 55_044, 55_051, 55_052, 55_053, 55_168, 55_169,
        55_170, 44_039, 44_040, 44_041, 44_042, 44_043, 44_044, 44_051, 44_052, 44_053, 44_168,
        44_169, 44_170, 44_183,
    ];
    let dir = fixtures_root().join("utilmd/valid");
    let mut checked = 0_usize;
    for entry in std::fs::read_dir(&dir).expect("utilmd/valid").flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("edi") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let Some(pid) = WIM_MSB_WECHSEL
            .iter()
            .find(|p| name.contains(&p.to_string()))
        else {
            continue;
        };
        let raw = std::fs::read_to_string(&path).expect("fixture is readable");
        let segments: Vec<&str> = raw
            .split('\'')
            .map(|s| s.trim_start_matches('\n'))
            .collect();
        let rff = segments
            .iter()
            .find_map(|s| s.strip_prefix("RFF+Z13:"))
            .unwrap_or_else(|| panic!("{}: no SG6 RFF+Z13", path.display()));
        assert_eq!(
            rff,
            pid.to_string(),
            "{}: RFF+Z13 DE 1154 must carry the Prüfidentifikator",
            path.display(),
        );
        let bgm = segments
            .iter()
            .find(|s| s.starts_with("BGM+"))
            .expect("BGM");
        assert!(
            !bgm.split('+')
                .nth(2)
                .unwrap_or_default()
                .starts_with(&pid.to_string()),
            "{}: BGM DE 1004 is the Dokumentennummer, not the Prüfidentifikator",
            path.display(),
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "expected the WiM MSB-Wechsel fixtures to be found; checked {checked}"
    );
}
