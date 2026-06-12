//! `cargo xtask generate-fixtures`
//!
//! Generates minimal synthetic `.edi` fixture files for every
//! Prüfidentifikator that currently has no test fixture.
//!
//! Generated files are written to
//!   `crates/edi-energy/tests/fixtures/<type>/gen/pid_<code>.gen.edi`
//! and are clearly marked as synthetic via the `.gen.edi` extension.
//! They are designed exclusively to satisfy `validate-pruefids` coverage
//! (PID present in BGM DE 1004 and/or RFF+Z13), not as functional
//! end-to-end test cases.  Hand-crafted fixtures in `valid/` remain the
//! authoritative acceptance test artefacts.
//!
//! # Options
//!
//! ```text
//! --dry-run              Print what would be written without touching the FS.
//! --message-type <TYPE>  Only generate for one message type (e.g. UTILMD).
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;

// ── JSON models ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MigProfile {
    release: String,
    #[serde(default)]
    archived: bool,
}

#[derive(Deserialize)]
struct AhbProfile {
    pruefidentifikatoren: Vec<PidEntry>,
}

#[derive(Deserialize)]
struct PidEntry {
    code: u32,
}

// ── Per-message-type template metadata ──────────────────────────────────────

/// Everything needed to render a minimal fixture for one message type.
struct TypeMeta {
    /// EDIFACT directory portion of UNH (e.g. `"MSCONS:D:04B:UN"`).
    /// The EDI@Energy release is appended at render time.
    unh_prefix: &'static str,
    /// Returns a BGM line (with trailing `'`) for the given PID code.
    bgm: fn(u32) -> String,
    /// Extra lines between NAD+MR and UNT.  Empty for most types.
    extra: &'static [&'static str],
    /// Segment count inside the message (UNH … UNT inclusive).
    /// Used for the UNT control count.  `0` means computed dynamically.
    seg_count_base: u32,
}

fn bgm_8digit(prefix: &str, pid: u32, suffix: &str) -> String {
    format!("BGM+{prefix}{pid:08}{suffix}'")
}

fn bgm_alphanum(prefix: &str, pid: u32, numeric_suffix: &str, suffix: &str) -> String {
    format!("BGM+{prefix}{pid}{numeric_suffix}{suffix}'")
}

fn type_meta(msg_type: &str) -> Option<TypeMeta> {
    match msg_type {
        "aperak" => Some(TypeMeta {
            unh_prefix: "APERAK:D:07B:UN",
            bgm: |pid| bgm_8digit("312+", pid, "+9"),
            extra: &[],
            seg_count_base: 7,
        }),
        "comdis" => Some(TypeMeta {
            unh_prefix: "COMDIS:D:17A:UN",
            // ABL prefix used in practice; RFF+Z13 carries the pure PID for coverage.
            bgm: |pid| bgm_alphanum("739+ABL", pid, "001", ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "iftsta" => Some(TypeMeta {
            unh_prefix: "IFTSTA:D:18A:UN",
            bgm: |pid| bgm_8digit("Z03+", pid, ""),
            extra: &[],
            seg_count_base: 6,
        }),
        "insrpt" => Some(TypeMeta {
            unh_prefix: "INSRPT:D:96A:UN",
            bgm: |pid| bgm_8digit("4+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "invoic" => Some(TypeMeta {
            unh_prefix: "INVOIC:D:06A:UN",
            bgm: |pid| bgm_8digit("380+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "mscons" => Some(TypeMeta {
            unh_prefix: "MSCONS:D:04B:UN",
            bgm: |pid| format!("BGM+7:::+{pid:08}::+9'"),
            extra: &["UNS+D'", "LOC+172+51238696781'", "QTY+220:1500.000:KWH'"],
            seg_count_base: 10,
        }),
        "ordchg" => Some(TypeMeta {
            unh_prefix: "ORDCHG:D:20B:UN",
            bgm: |pid| bgm_8digit("Z51+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "orders" => Some(TypeMeta {
            unh_prefix: "ORDERS:D:09B:UN",
            bgm: |pid| bgm_8digit("Z55+", pid, "+9"),
            extra: &[],
            seg_count_base: 7,
        }),
        "ordrsp" => Some(TypeMeta {
            unh_prefix: "ORDRSP:D:10A:UN",
            bgm: |pid| bgm_8digit("7+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "partin" => Some(TypeMeta {
            unh_prefix: "PARTIN:D:20B:UN",
            bgm: |pid| bgm_8digit("35+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "pricat" => Some(TypeMeta {
            unh_prefix: "PRICAT:D:20B:UN",
            // PRIC prefix used in practice; RFF+Z13 carries the pure PID.
            bgm: |pid| bgm_alphanum("Z32+PRIC", pid, "001", ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "quotes" => Some(TypeMeta {
            unh_prefix: "QUOTES:D:10A:UN",
            bgm: |pid| bgm_8digit("310+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "remadv" => Some(TypeMeta {
            unh_prefix: "REMADV:D:05A:UN",
            bgm: |pid| bgm_8digit("239+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "reqote" => Some(TypeMeta {
            unh_prefix: "REQOTE:D:10A:UN",
            bgm: |pid| bgm_8digit("311+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "utilmd" => Some(TypeMeta {
            unh_prefix: "UTILMD:D:11A:UN",
            bgm: |pid| format!("BGM+E01:::+{pid:08}::+9'"),
            extra: &["IDE+Z19+51238696781::'"],
            seg_count_base: 8,
        }),
        "utilts" => Some(TypeMeta {
            unh_prefix: "UTILTS:D:18A:UN",
            // UTILTS prefix in practice; RFF+Z13 carries the pure PID.
            bgm: |pid| bgm_alphanum("Z36+UTILTS", pid, "001", ""),
            extra: &[],
            seg_count_base: 7,
        }),
        _ => None,
    }
}

// ── Fixture rendering ────────────────────────────────────────────────────────

fn render_fixture(meta: &TypeMeta, pid: u32, release: &str) -> String {
    let bgm_line = (meta.bgm)(pid);
    let seg_count = meta.seg_count_base + meta.extra.len() as u32;

    let mut lines = vec![
        "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'".to_string(),
        format!("UNH+1+{}:{}'", meta.unh_prefix, release),
        bgm_line,
        "DTM+137:20230101:102'".to_string(),
        format!("RFF+Z13:{pid}'"),
        "NAD+MS+4012345000023::293'".to_string(),
        "NAD+MR+9900357000004::293'".to_string(),
    ];
    for extra in meta.extra {
        lines.push(extra.to_string());
    }
    lines.push(format!("UNT+{seg_count}+1'"));
    lines.push("UNZ+1+1'".to_string());
    lines.join("\n") + "\n"
}

// ── Active profiles collection ───────────────────────────────────────────────

/// `(message_type_lower, pid_code)` → latest `release` string.
fn collect_active_pids(profiles_dir: &str) -> BTreeMap<(String, u32), String> {
    let mut map: BTreeMap<(String, u32), String> = BTreeMap::new();
    let base = Path::new(profiles_dir);
    let msg_type_dirs = match std::fs::read_dir(base) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("generate-fixtures: cannot read {profiles_dir}: {e}");
            return map;
        }
    };

    for msg_entry in msg_type_dirs.flatten() {
        let msg_path = msg_entry.path();
        if !msg_path.is_dir() {
            continue;
        }
        let msg_type = msg_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();

        // Skip schema sub-directory.
        if msg_type == "schemas" {
            continue;
        }

        let release_dirs = match std::fs::read_dir(&msg_path) {
            Ok(d) => d,
            Err(_) => continue,
        };

        for rel_entry in release_dirs.flatten() {
            let rel_path = rel_entry.path();
            if !rel_path.is_dir() {
                continue;
            }
            let mig_path = rel_path.join("mig.json");
            let ahb_path = rel_path.join("ahb.json");
            if !mig_path.exists() || !ahb_path.exists() {
                continue;
            }

            let mig: MigProfile =
                match serde_json::from_str(&std::fs::read_to_string(&mig_path).unwrap_or_default())
                {
                    Ok(m) => m,
                    Err(_) => continue,
                };
            if mig.archived {
                continue;
            }

            let ahb: AhbProfile =
                match serde_json::from_str(&std::fs::read_to_string(&ahb_path).unwrap_or_default())
                {
                    Ok(a) => a,
                    Err(_) => continue,
                };

            for p in ahb.pruefidentifikatoren {
                // Keep only the latest release per PID (sorted release name).
                let key = (msg_type.clone(), p.code);
                let existing = map.entry(key).or_insert_with(|| mig.release.clone());
                // Use lexicographically larger release (most recent).
                if mig.release > *existing {
                    *existing = mig.release.clone();
                }
            }
        }
    }
    map
}

// ── Covered PIDs (mirrors validate_pruefids logic) ───────────────────────────

fn collect_covered(dir: &Path, covered: &mut HashSet<u32>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_covered(&path, covered);
        } else if path.extension().and_then(|e| e.to_str()) == Some("edi")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            extract_pids_from_edi(&content, covered);
        }
    }
}

fn extract_pids_from_edi(content: &str, covered: &mut HashSet<u32>) {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("BGM") {
            let fields: Vec<&str> = trimmed.splitn(4, '+').collect();
            if fields.len() >= 3 {
                let pid_str = fields[2]
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('\'')
                    .trim();
                if let Ok(pid) = pid_str.parse::<u32>()
                    && (10_000..=99_999).contains(&pid)
                {
                    covered.insert(pid);
                }
            }
        }
        if trimmed.starts_with("RFF") {
            let fields: Vec<&str> = trimmed.splitn(3, '+').collect();
            if fields.len() >= 2 {
                let composite = fields[1].trim_end_matches('\'');
                let parts: Vec<&str> = composite.splitn(2, ':').collect();
                if parts.len() == 2 && parts[0] == "Z13" {
                    let pid_str = parts[1]
                        .split(':')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('\'')
                        .trim();
                    if let Ok(pid) = pid_str.parse::<u32>()
                        && (10_000..=99_999).contains(&pid)
                    {
                        covered.insert(pid);
                    }
                }
            }
        }
    }
}

// ── Public entry-point ────────────────────────────────────────────────────────

pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let msg_type_filter: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--message-type")
        .map(|w| w[1].to_lowercase());

    let profiles_dir = format!("{workspace_root}/crates/edi-energy/profiles");
    let fixtures_base = format!("{workspace_root}/crates/edi-energy/tests/fixtures");

    // Collect all active PIDs across all non-archived profiles.
    let active_pids = collect_active_pids(&profiles_dir);
    if active_pids.is_empty() {
        eprintln!("generate-fixtures: no active profiles found under {profiles_dir}");
        return false;
    }

    // Collect all currently covered PIDs (global, across all .edi files).
    let mut covered: HashSet<u32> = HashSet::new();
    collect_covered(Path::new(&fixtures_base), &mut covered);

    let mut generated = 0usize;
    let mut skipped = 0usize;
    let mut unknown_type = 0usize;

    // Group work by message type for cleaner output.
    let mut by_type: BTreeMap<&str, Vec<(u32, &str)>> = BTreeMap::new();
    for ((mt, pid), release) in &active_pids {
        if let Some(ref f) = msg_type_filter
            && mt != f
        {
            continue;
        }
        by_type
            .entry(mt.as_str())
            .or_default()
            .push((*pid, release.as_str()));
    }

    for (msg_type, mut pid_list) in by_type {
        let Some(meta) = type_meta(msg_type) else {
            eprintln!("generate-fixtures: no template for message type '{msg_type}' — skipping");
            unknown_type += 1;
            continue;
        };

        pid_list.sort_unstable_by_key(|(pid, _)| *pid);

        let gen_dir = format!("{fixtures_base}/{msg_type}/gen");
        if !dry_run
            && !Path::new(&gen_dir).exists()
            && let Err(e) = std::fs::create_dir_all(&gen_dir)
        {
            eprintln!("generate-fixtures: cannot create {gen_dir}: {e}");
            return false;
        }

        for (pid, release) in pid_list {
            if covered.contains(&pid) {
                skipped += 1;
                continue;
            }

            let content = render_fixture(&meta, pid, release);
            let path = format!("{gen_dir}/pid_{pid}.gen.edi");

            if dry_run {
                println!("DRY-RUN  would write {path}");
            } else {
                match std::fs::write(&path, &content) {
                    Ok(()) => println!("GENERATE {path}"),
                    Err(e) => {
                        eprintln!("generate-fixtures: cannot write {path}: {e}");
                        return false;
                    }
                }
            }
            generated += 1;
        }
    }

    eprintln!();
    if dry_run {
        eprintln!("dry-run: {generated} fixture(s) would be generated, {skipped} already covered");
    } else {
        eprintln!("{generated} fixture(s) generated, {skipped} already covered");
    }
    if unknown_type > 0 {
        eprintln!("warning: {unknown_type} message type(s) have no template and were skipped");
    }
    true
}
