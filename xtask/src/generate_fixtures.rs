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
    /// Anwendungszeitpunkt — the day this format version goes on the wire.
    #[serde(default)]
    valid_from: Option<String>,
    /// Last day it is on the wire, `None` for the open-ended current one.
    #[serde(default)]
    valid_until: Option<String>,
}

#[derive(Deserialize)]
struct AhbProfile {
    pruefidentifikatoren: Vec<PidEntry>,
}

#[derive(Deserialize)]
struct PidEntry {
    code: u32,
    /// The AHB rules for this PID, so a fixture can honour the ones that decide
    /// whether it parses as the Anwendungsfall it claims to be.
    #[serde(default)]
    segment_rules: Vec<SegmentRule>,
}

#[derive(Deserialize)]
struct SegmentRule {
    tag: String,
    #[serde(default)]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
}

impl PidEntry {
    /// `BGM` DE 1001 — the Nachrichtenfunktion the AHB admits for this
    /// Anwendungsfall.
    ///
    /// A synthetic fixture is not an acceptance test, but it must not
    /// *contradict* the profile it was generated from: `BGM+E01` on a 55007
    /// declares an Anmeldung where the AHB requires an Abmeldung, and anyone
    /// reading the fixture to learn the message shape learns the wrong one.
    fn bgm_qualifier(&self) -> Option<&str> {
        self.segment_rules
            .iter()
            .find(|r| r.tag == "BGM")?
            .qualifier_restrictions
            .get("1001")?
            .first()
            .map(String::as_str)
    }
}

// ── Per-message-type template metadata ──────────────────────────────────────

/// Everything needed to render a minimal fixture for one message type.
struct TypeMeta {
    /// EDIFACT directory portion of UNH (e.g. `"MSCONS:D:04B:UN"`).
    /// The EDI@Energy release is appended at render time.
    unh_prefix: &'static str,
    /// Returns a BGM line (with trailing `'`) for the given PID code.
    ///
    /// The second argument is the DE 1001 qualifier the profile restricts this
    /// PID to, where it states one; templates that do not carry a DE 1001
    /// ignore it.
    bgm: fn(u32, Option<&str>) -> String,
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
            bgm: |pid, _| bgm_8digit("312+", pid, "+9"),
            extra: &[],
            seg_count_base: 7,
        }),
        "comdis" => Some(TypeMeta {
            unh_prefix: "COMDIS:D:17A:UN",
            // ABL prefix used in practice; RFF+Z13 carries the pure PID for coverage.
            bgm: |pid, _| bgm_alphanum("739+ABL", pid, "001", ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "iftsta" => Some(TypeMeta {
            unh_prefix: "IFTSTA:D:18A:UN",
            bgm: |pid, _| bgm_8digit("Z03+", pid, ""),
            extra: &[],
            seg_count_base: 6,
        }),
        "insrpt" => Some(TypeMeta {
            unh_prefix: "INSRPT:D:96A:UN",
            bgm: |pid, _| bgm_8digit("4+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "invoic" => Some(TypeMeta {
            unh_prefix: "INVOIC:D:06A:UN",
            bgm: |pid, _| bgm_8digit("380+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "mscons" => Some(TypeMeta {
            unh_prefix: "MSCONS:D:04B:UN",
            bgm: |pid, _| format!("BGM+7:::+{pid:08}::+9'"),
            extra: &["UNS+D'", "LOC+172+51238696781'", "QTY+220:1500.000:KWH'"],
            seg_count_base: 10,
        }),
        "ordchg" => Some(TypeMeta {
            unh_prefix: "ORDCHG:D:20B:UN",
            bgm: |pid, _| bgm_8digit("Z51+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "orders" => Some(TypeMeta {
            unh_prefix: "ORDERS:D:09B:UN",
            bgm: |pid, _| bgm_8digit("Z55+", pid, "+9"),
            extra: &[],
            seg_count_base: 7,
        }),
        "ordrsp" => Some(TypeMeta {
            unh_prefix: "ORDRSP:D:10A:UN",
            bgm: |pid, _| bgm_8digit("7+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "partin" => Some(TypeMeta {
            unh_prefix: "PARTIN:D:20B:UN",
            bgm: |pid, _| bgm_8digit("35+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "pricat" => Some(TypeMeta {
            unh_prefix: "PRICAT:D:20B:UN",
            // PRIC prefix used in practice; RFF+Z13 carries the pure PID.
            bgm: |pid, _| bgm_alphanum("Z32+PRIC", pid, "001", ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "quotes" => Some(TypeMeta {
            unh_prefix: "QUOTES:D:10A:UN",
            bgm: |pid, _| bgm_8digit("310+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "remadv" => Some(TypeMeta {
            unh_prefix: "REMADV:D:05A:UN",
            bgm: |pid, _| bgm_8digit("239+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "reqote" => Some(TypeMeta {
            unh_prefix: "REQOTE:D:10A:UN",
            bgm: |pid, _| bgm_8digit("311+", pid, ""),
            extra: &[],
            seg_count_base: 7,
        }),
        "utilmd" => Some(TypeMeta {
            unh_prefix: "UTILMD:D:11A:UN",
            bgm: |pid, de1001| format!("BGM+{}:::+{pid:08}::+9'", de1001.unwrap_or("E01")),
            // `IDE+24` is the only Vorgangs-Qualifier UTILMD defines (DE 7495);
            // DE 7402 carries a Vorgangsnummer. The Marktlokation follows in
            // `SG5 LOC+Z16`.
            extra: &["IDE+24+VORGANG-0001'", "LOC+Z16+51238696781'"],
            // UNH, BGM, DTM, RFF, NAD, NAD and UNT — `extra` is added on top.
            seg_count_base: 7,
        }),
        "utilts" => Some(TypeMeta {
            unh_prefix: "UTILTS:D:18A:UN",
            // UTILTS prefix in practice; RFF+Z13 carries the pure PID.
            bgm: |pid, _| bgm_alphanum("Z36+UTILTS", pid, "001", ""),
            extra: &[],
            seg_count_base: 7,
        }),
        _ => None,
    }
}

// ── Fixture rendering ────────────────────────────────────────────────────────

fn render_fixture(meta: &TypeMeta, pid: u32, release: &str, de1001: Option<&str>) -> String {
    let bgm_line = (meta.bgm)(pid, de1001);
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

/// `(message_type_lower, pid_code)` → the release a fixture should carry.
///
/// The one **in force on `today`**, not the newest one shipped. A fixture is a
/// message mako must be able to receive, and `valid_from` is a hard edge
/// (Allgemeine Festlegungen 6.1 §2.5): a message stamped with the next format
/// version is rejected until its Anwendungszeitpunkt, so generating the corpus
/// at the newest shipped release stamps every fixture with a code that is not
/// on the wire yet and leaves the in-force one with no witness at all.
///
/// Ordering is by `valid_from`, never by the release string — release codes are
/// BDEW labels, not versions, and `"2.10" < "2.9"` under a string sort.
fn collect_active_pids(
    profiles_dir: &str,
    today: time::Date,
) -> BTreeMap<(String, u32), ActivePid> {
    let mut map: BTreeMap<(String, u32), ActivePid> = BTreeMap::new();
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

            let rank = rank_for(
                iso_date(mig.valid_from.as_ref()),
                iso_date(mig.valid_until.as_ref()),
                today,
            );

            for p in ahb.pruefidentifikatoren {
                let key = (msg_type.clone(), p.code);
                let candidate = ActivePid {
                    release: mig.release.clone(),
                    rank,
                    bgm_qualifier: p.bgm_qualifier().map(ToOwned::to_owned),
                };
                match map.entry(key) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(candidate);
                    }
                    std::collections::btree_map::Entry::Occupied(mut slot) => {
                        if candidate.rank > slot.get().rank {
                            slot.insert(candidate);
                        }
                    }
                }
            }
        }
    }
    map
}

/// One PID's in-force release and the AHB facts a fixture must honour.
#[derive(Clone)]
struct ActivePid {
    release: String,
    /// How well this profile fits `today`; the highest wins. See [`rank_for`].
    rank: (u8, i64),
    /// `BGM` DE 1001, where the profile restricts it.
    bgm_qualifier: Option<String>,
}

/// Parse an ISO 8601 `yyyy-mm-dd` profile date.
fn iso_date(raw: Option<&String>) -> Option<time::Date> {
    let raw = raw?;
    time::Date::parse(raw, &time::format_description::well_known::Iso8601::DATE).ok()
}

/// Rank a profile's fitness to stand for "what is on the wire on `today`".
///
/// Tier first, then a tie-break within the tier:
///
/// | tier | profile | tie-break |
/// |---|---|---|
/// | 2 | in force on `today` | the later `valid_from` |
/// | 1 | not yet in force | the *nearer* Anwendungszeitpunkt |
/// | 0 | superseded | the later `valid_from` |
///
/// A profile with no dates at all ranks as in force — a legacy profile carrying
/// no lifecycle is the only thing that could stand for its message type.
fn rank_for(
    valid_from: Option<time::Date>,
    valid_until: Option<time::Date>,
    today: time::Date,
) -> (u8, i64) {
    let epoch = time::Date::from_ordinal_date(2000, 1).expect("2000-001 is a valid date");
    let days = |d: Option<time::Date>| d.map_or(0, |d| (d - epoch).whole_days());
    match valid_from {
        Some(from) if from > today => (1, -days(valid_from)),
        _ if valid_until.is_some_and(|until| until < today) => (0, days(valid_from)),
        _ => (2, days(valid_from)),
    }
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

    // Every PID across the non-archived profiles, each at the release in force
    // today — the code a counterparty would actually put on the wire.
    let today = time::OffsetDateTime::now_utc().date();
    let active_pids = collect_active_pids(&profiles_dir, today);
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
    let mut by_type: BTreeMap<&str, Vec<(u32, &ActivePid)>> = BTreeMap::new();
    for ((mt, pid), active) in &active_pids {
        if let Some(ref f) = msg_type_filter
            && mt != f
        {
            continue;
        }
        by_type.entry(mt.as_str()).or_default().push((*pid, active));
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

        for (pid, active) in pid_list {
            if covered.contains(&pid) {
                skipped += 1;
                continue;
            }

            let content =
                render_fixture(&meta, pid, &active.release, active.bgm_qualifier.as_deref());
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
