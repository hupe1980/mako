//! `cargo xtask validate-profiles` — the committed profiles are consistent.
//!
//! `import-profiles --check` proves a profile against its source PDF, but only
//! where the document mirror is. This runs everywhere: it holds the profile
//! files against `sources.json`, against each other and against the mirror's
//! manifest when that is present.
//!
//! - every `sources.json` entry has a directory with `mig.json` and `ahb.json`,
//!   and every profile directory is in `sources.json`;
//! - `message_type`, `release`, `valid_from`, `valid_until` and `ahb_version`
//!   agree between the files and the manifest entry;
//! - per message type and track the validity windows neither overlap nor leave
//!   a gap, and exactly one is open-ended;
//! - Prüfidentifikatoren are five digits, unique within a profile, and a code
//!   the previous release carried is still there unless [`RETIRED_PIDS`]
//!   explains its absence;
//! - every AHB row names a segment `Nr` the MIG structure has, and every column
//!   lists `UNH`;
//! - every `[n]` a status expression or an operand cites has its Bedingung text;
//! - every status and operand that cites a Bedingung reads as an expression,
//!   bar the truncations [`crate::profile_expressions::ALLOWLIST_FILE`] records;
//! - the `source.sha256` matches the mirrored document when the mirror is here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

const PROFILES_DIR: &str = "crates/edi-energy/profiles";
const MIRROR_MANIFEST: &str = "regulatories/bdew-mako/manifest.json";

/// Prüfidentifikatoren BDEW retired: (message type, track suffix, PID, why).
/// A PID that vanishes between two releases and is not listed here is an
/// import regression.
const RETIRED_PIDS: &[(&str, &str, u32, &str)] = &[
    (
        "ORDERS",
        "",
        17003,
        "retired in ORDERS AHB 1.1b (01.04.2026) — Beauftragung zur Änderung der Technik (Messlokationsänderung Gas)",
    ),
    (
        "ORDERS",
        "",
        17114,
        "retired in ORDERS AHB 1.1b (01.04.2026) — Anforderung der bilanzierten Menge",
    ),
    (
        "IFTSTA",
        "",
        21015,
        "withdrawn by IFTSTA AHB 2.1 Änd-ID 27061 (01.10.2026)",
    ),
    (
        "IFTSTA",
        "",
        21024,
        "withdrawn by IFTSTA AHB 2.1 (01.10.2026) — Messstellenumbau, Änderungshistorie",
    ),
    (
        "IFTSTA",
        "",
        21026,
        "withdrawn by IFTSTA AHB 2.1 (01.10.2026) — Messstellenumbau, Änderungshistorie",
    ),
    (
        "ORDRSP",
        "",
        19115,
        "withdrawn by ORDRSP AHB 1.1b (01.10.2026) — Ablehnung Anforderung bilanzierte Menge",
    ),
];

#[derive(Deserialize)]
struct Sources {
    profiles: BTreeMap<String, Source>,
}

#[derive(Deserialize)]
struct Source {
    release: String,
    #[serde(default)]
    track: Option<String>,
    valid_from: String,
    #[serde(default)]
    valid_until: Option<String>,
    ahb_version: String,
    mig: String,
    ahb: String,
}

#[derive(Deserialize)]
struct Mig {
    schema_version: u32,
    message_type: String,
    release: String,
    #[serde(default)]
    track: Option<String>,
    valid_from: String,
    #[serde(default)]
    valid_until: Option<String>,
    ahb_version: String,
    source: FileSource,
    structure: Vec<serde_json::Value>,
    #[serde(default)]
    envelope: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Ahb {
    schema_version: u32,
    message_type: String,
    release: String,
    ahb_version: String,
    source: FileSource,
    anwendungsfaelle: Vec<Anwendungsfall>,
}

#[derive(Deserialize)]
struct FileSource {
    file: String,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Deserialize)]
struct Anwendungsfall {
    #[serde(default)]
    pid: Option<u32>,
    name: String,
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    #[serde(default)]
    nr: Option<String>,
    #[serde(default)]
    before: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    files: BTreeMap<String, ManifestEntry>,
}

#[derive(Deserialize)]
struct ManifestEntry {
    #[serde(default)]
    sha256: Option<String>,
}

/// Every segment `Nr` in a MIG structure.
fn nrs(nodes: &[serde_json::Value], out: &mut BTreeSet<String>) {
    for n in nodes {
        if let Some(nr) = n.get("nr").and_then(|v| v.as_str()) {
            out.insert(nr.to_owned());
        }
        if let Some(children) = n.get("children").and_then(|v| v.as_array()) {
            nrs(children, out);
        }
    }
}

/// One profile's window and Prüfidentifikatoren: (dir, valid_from, valid_until, pids).
type Span = (String, time::Date, Option<time::Date>, BTreeSet<u32>);

fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

fn date(s: &str) -> Option<time::Date> {
    let f = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(s, &f).ok()
}

pub fn run(workspace_root: &str) -> bool {
    let root = Path::new(workspace_root);
    let profiles = root.join(PROFILES_DIR);
    let mut errors: Vec<String> = Vec::new();

    let sources: Sources = match load(&profiles.join("sources.json")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return false;
        }
    };
    let manifest: BTreeMap<String, ManifestEntry> = load::<Manifest>(&root.join(MIRROR_MANIFEST))
        .map(|m| m.files)
        .unwrap_or_default();
    let allowed = crate::profile_expressions::allowlist(root);
    let bless = std::env::var_os("BLESS_PROFILE_EXPRESSIONS").is_some();
    let mut found_expressions = crate::profile_expressions::Ledger::new();

    // Directories ↔ sources.json.
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for ty in std::fs::read_dir(&profiles).into_iter().flatten().flatten() {
        if !ty.path().is_dir() {
            continue;
        }
        for fv in std::fs::read_dir(ty.path()).into_iter().flatten().flatten() {
            if fv.path().join("mig.json").is_file() || fv.path().join("ahb.json").is_file() {
                dirs.insert(format!(
                    "{}/{}",
                    ty.file_name().to_string_lossy(),
                    fv.file_name().to_string_lossy()
                ));
            }
        }
    }
    for dir in &dirs {
        if !sources.profiles.contains_key(dir) {
            errors.push(format!("{dir}: profile directory is not in sources.json"));
        }
    }

    // (message type, track) → [(dir, from, until, pids)]
    let mut chains: BTreeMap<(String, String), Vec<Span>> = BTreeMap::new();

    for (dir, src) in &sources.profiles {
        let (ty, fv) = match dir.split_once('/') {
            Some(p) => p,
            None => {
                errors.push(format!("{dir}: profile directory must be <type>/<fv>"));
                continue;
            }
        };
        let message_type = ty.to_ascii_uppercase();
        let mig: Mig = match load(&profiles.join(dir).join("mig.json")) {
            Ok(m) => m,
            Err(e) => {
                errors.push(format!("{dir}: {e}"));
                continue;
            }
        };
        let ahb_path = profiles.join(dir).join("ahb.json");
        let ahb: Ahb = match load(&ahb_path) {
            Ok(a) => a,
            Err(e) => {
                errors.push(format!("{dir}: {e}"));
                continue;
            }
        };
        // Every `[n]` a status or an operand cites has its Bedingung text. The
        // importer refuses to write a profile that fails this, but only where
        // the document mirror is; the committed file is what ships.
        match load::<serde_json::Value>(&ahb_path) {
            Ok(raw) => {
                let missing = crate::import_profiles::unresolved_conditions(&raw);
                if !missing.is_empty() {
                    errors.push(format!(
                        "{dir}: {} Bedingungen are cited but have no text: {:?}",
                        missing.len(),
                        missing.iter().take(20).collect::<Vec<_>>()
                    ));
                }
                let malformed = crate::profile_expressions::malformed(&raw);
                if !malformed.is_empty() {
                    found_expressions.insert(dir.clone(), malformed.clone());
                }
                if !bless {
                    errors.extend(crate::profile_expressions::compare(
                        dir, &malformed, &allowed,
                    ));
                }
            }
            Err(e) => errors.push(format!("{dir}: {e}")),
        }
        let mut e = |msg: String| errors.push(format!("{dir}: {msg}"));
        if mig.schema_version != 2 || ahb.schema_version != 2 {
            e("schema_version must be 2".into());
        }
        if mig.message_type != message_type || ahb.message_type != message_type {
            e(format!(
                "message_type {:?}/{:?} does not match the directory",
                mig.message_type, ahb.message_type
            ));
        }
        if mig.release != src.release || ahb.release != src.release {
            e(format!(
                "release {:?}/{:?} does not match sources.json {:?}",
                mig.release, ahb.release, src.release
            ));
        }
        if mig.track != src.track {
            e(format!(
                "track {:?} does not match sources.json {:?}",
                mig.track, src.track
            ));
        }
        if mig.valid_from != src.valid_from || mig.valid_until != src.valid_until {
            e("valid_from/valid_until do not match sources.json".into());
        }
        if mig.ahb_version != src.ahb_version || ahb.ahb_version != src.ahb_version {
            e(format!(
                "ahb_version {:?}/{:?} does not match sources.json {:?}",
                mig.ahb_version, ahb.ahb_version, src.ahb_version
            ));
        }
        if mig.source.file != src.mig || ahb.source.file != src.ahb {
            e("source.file does not name the sources.json document".into());
        }
        let expected_fv = format!("fv{}", src.valid_from.replace('-', ""));
        if !fv.starts_with(&expected_fv) {
            e(format!(
                "directory {fv:?} does not start with {expected_fv:?} (valid_from {})",
                src.valid_from
            ));
        }
        for (what, file, sha) in [
            ("mig", &src.mig, &mig.source.sha256),
            ("ahb", &src.ahb, &ahb.source.sha256),
        ] {
            if let (Some(entry), Some(sha)) = (manifest.get(file), sha)
                && entry.sha256.as_deref().is_some_and(|m| m != sha)
            {
                e(format!(
                    "{what}.json was imported from a different {file} than the mirror holds (sha256 differs)"
                ));
            }
        }

        // PIDs and rows.
        let mut all_nrs = BTreeSet::new();
        nrs(&mig.structure, &mut all_nrs);
        let unh = all_nrs.iter().next().cloned().unwrap_or_default();
        // The AHB tables list the interchange envelope too.
        nrs(&mig.envelope, &mut all_nrs);
        let mut pids: BTreeSet<u32> = BTreeSet::new();
        for af in &ahb.anwendungsfaelle {
            if let Some(pid) = af.pid {
                if !(10_000..=99_999).contains(&pid) {
                    e(format!(
                        "Anwendungsfall {:?}: Prüfidentifikator {pid} is not five digits",
                        af.name
                    ));
                }
                if !pids.insert(pid) {
                    e(format!("Prüfidentifikator {pid} appears twice"));
                }
            }
            let mut has_unh = false;
            for row in &af.rows {
                if let Some(nr) = row.nr.as_deref().or(row.before.as_deref()) {
                    if !all_nrs.contains(nr) {
                        e(format!(
                            "Anwendungsfall {:?}: row Nr {nr} is not in the MIG structure",
                            af.name
                        ));
                    }
                    if nr == unh {
                        has_unh = true;
                    }
                }
            }
            if !has_unh {
                e(format!("Anwendungsfall {:?} lists no UNH row", af.name));
            }
        }
        let Some(from) = date(&src.valid_from) else {
            e(format!("valid_from {:?} is not a date", src.valid_from));
            continue;
        };
        let until = src.valid_until.as_deref().and_then(date);
        if src.valid_until.is_some() && until.is_none() {
            e(format!("valid_until {:?} is not a date", src.valid_until));
        }
        chains
            .entry((message_type.clone(), src.track.clone().unwrap_or_default()))
            .or_default()
            .push((dir.clone(), from, until, pids));
    }

    // Continuity per chain.
    for ((ty, track), chain) in &mut chains {
        chain.sort_by_key(|(_, from, _, _)| *from);
        let suffix = if track.is_empty() { "" } else { "_gas" };
        for pair in chain.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            match a.2 {
                None => errors.push(format!("{}: is open-ended but {} follows it", a.0, b.0)),
                Some(until) => {
                    if until.next_day() != Some(b.1) {
                        errors.push(format!(
                            "{}: ends {} but {} starts {}",
                            a.0, until, b.0, b.1
                        ));
                    }
                }
            }
            for pid in a.3.difference(&b.3) {
                let retired = RETIRED_PIDS
                    .iter()
                    .any(|(t, s, p, _)| t == ty && *s == suffix && p == pid);
                if !retired {
                    errors.push(format!(
                        "{}: Prüfidentifikator {pid} of {} is gone — an import regression unless BDEW retired it (then list it in RETIRED_PIDS)",
                        b.0, a.0
                    ));
                }
            }
        }
        if let Some(last) = chain.last()
            && last.2.is_some()
        {
            errors.push(format!(
                "{}: the newest {ty}{suffix} profile is not open-ended",
                last.0
            ));
        }
    }

    if bless {
        if let Err(e) = crate::profile_expressions::bless(root, found_expressions.clone()) {
            errors.push(format!("cannot write the expression ledger: {e}"));
        }
    }

    for e in &errors {
        eprintln!("error   {e}");
    }
    let (kinds, occurrences) = found_expressions.values().fold((0, 0), |(k, o), m| {
        (k + m.len(), o + m.values().sum::<usize>())
    });
    let mut worst: Vec<(&String, usize)> = found_expressions
        .iter()
        .map(|(dir, m)| (dir, m.values().sum::<usize>()))
        .collect();
    worst.sort_by_key(|(dir, n)| (std::cmp::Reverse(*n), dir.as_str()));
    eprintln!(
        "validate-profiles: {} profiles, {occurrences} truncated status expression(s) in {kinds} distinct shapes; worst {:?}; {} error(s)",
        sources.profiles.len(),
        worst.iter().take(5).collect::<Vec<_>>(),
        errors.len()
    );
    errors.is_empty()
}
