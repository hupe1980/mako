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
//! - every Formatbedingung `[9xx]` and Zeitpunktangabe `[UBn]` a binding
//!   operand cites has an evaluator in `edi_energy::profile::formatbedingung`,
//!   bar the [`UNEVALUATED_FORMATBEDINGUNGEN`] ratchet;
//! - every profile on a regular Anwendungszeitpunkt states the
//!   `publikationsdatum` Allgemeine Festlegungen 6.1d § 2.5 fixes for it, and an
//!   ausserordentliche release states none;
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
    #[serde(default)]
    publikationsdatum: Option<String>,
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
    #[serde(default)]
    publikationsdatum: Option<String>,
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

/// Formatbedingungen a binding place cites and
/// [`edi_energy::profile::formatbedingung`] does not yet answer.
///
/// Each needs something its number alone does not give — a document outside the
/// mirror, or state the single value does not carry. The list is a ratchet: it
/// may only shrink, and [`unregistered_formatbedingungen`] refuses both a new
/// number that is not here and a row here that has since gained an evaluator.
const UNEVALUATED_FORMATBEDINGUNGEN: &[(&str, &str)] = &[
    (
        "911",
        "„1 bis n, je Nachricht oder Segmentgruppe bei 1 beginnend und fortlaufend \
         aufsteigend“ — the ascending half is a property of the whole message, and a \
         per-value evaluator that checked only ≥ 1 would report a broken sequence as sound",
    ),
    (
        "941",
        "„Format: Artikelnummer“ — unlike [942]/[943]/[959] this names no shape, so what \
         it admits is the Codeliste der Artikelnummern und Artikel-ID itself; that is a \
         code-list import rather than an evaluator",
    ),
    (
        "952",
        "Gerätenummer nach DIN 43863-5 — the standard is sold by Beuth and is not in the \
         mirror, so its shape cannot be sourced here",
    ),
    (
        "967",
        "Zertifikatskörper gemäß X.509.1 / BSI TR-03109-4 — a certificate parser, not a \
         format screen",
    ),
];

/// Every `[9xx]` a binding operand cites has an evaluator, or a reason not to.
///
/// A Formatbedingung or Zeitpunktangabe nothing evaluates is a silent permit
/// on every message:
/// [`edi_energy::profile::formatbedingung::evaluate`] returns `Unregistered`,
/// the validator reads it as `Unknown`, and the place is admitted whatever it
/// carries. Catching it here means it is refused once, at import, against the
/// profile that introduced it — rather than never.
fn unregistered_formatbedingungen(dir: &str, ahb: &serde_json::Value) -> Vec<String> {
    use edi_energy::profile::formatbedingung;

    let mut cited: BTreeMap<String, String> = BTreeMap::new();
    for af in ahb
        .get("anwendungsfaelle")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let pid = af
            .get("pid")
            .map_or_else(|| "?".to_owned(), std::string::ToString::to_string);
        for el in af
            .get("elements")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let de = el.get("de").and_then(|v| v.as_str()).unwrap_or("?");
            for op in el
                .get("operands")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                let Some(text) = op.get("operand").and_then(|v| v.as_str()) else {
                    continue;
                };
                // Only a place the receiver may refuse: a `Soll` or `Kann`
                // column states a format nobody checks either way.
                if !matches!(text.split_whitespace().next(), Some("X" | "M")) {
                    continue;
                }
                for id in format_ids(text) {
                    cited.entry(id).or_insert_with(|| format!("{pid} DE {de}"));
                }
            }
        }
    }

    let mut out = Vec::new();
    for (id, whence) in &cited {
        if formatbedingung::is_registered(id) {
            continue;
        }
        if UNEVALUATED_FORMATBEDINGUNGEN.iter().any(|(n, _)| n == id) {
            continue;
        }
        out.push(format!(
            "{dir}: Formatbedingung [{id}] is cited by a binding place ({whence}) and \
             `formatbedingung::evaluate` does not answer it, so the value it \
             constrains is admitted unchecked. Register an evaluator, or add [{id}] \
             to `UNEVALUATED_FORMATBEDINGUNGEN` with the reason it cannot have one"
        ));
    }
    out
}

/// The Formatbedingungen and Zeitpunktangaben an operand expression cites.
///
/// `[UB1]`–`[UB3]` are in because Allgemeine Festlegungen 6.1d Kap. 3.8 defines
/// them as expressions over `[931]`–`[935]`: they constrain a value exactly as
/// a `[9xx]` does, and leaving them out of the ratchet would leave 1 220
/// binding places outside it.
fn format_ids(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find(']') else { break };
        let id = &rest[..close];
        rest = &rest[close + 1..];
        let numbered =
            id.len() == 3 && id.starts_with('9') && id.bytes().all(|b| b.is_ascii_digit());
        let zeitpunkt =
            id.len() == 3 && id.starts_with("UB") && id.ends_with(|c: char| c.is_ascii_digit());
        if numbered || zeitpunkt {
            out.insert(id.to_owned());
        }
    }
    out
}

/// Every segment `Nr` in a MIG structure.
/// Every code a Bedingung names is a code the profile itself admits.
///
/// A Bedingung reads „Wenn SG4 ERC+Z29 vorhanden." — it *conditions* something
/// on a code, so that code has to appear in some Anwendungsfall's operand list
/// for that segment. The two are printed in different columns of the same AHB
/// table, so a parse that loses one keeps the other and the profile stays
/// internally inconsistent in a way nothing else notices: the condition text is
/// never evaluated against a message, while the operand list *is* — a dropped
/// code turns into „the Prüfschablone admits no such code" at validation time.
///
/// APERAK AHB 1.0 breaks `SG4 ERC` DE 9321 across a page and lost eleven codes
/// that way, `Z29` among them, while conditions `[5]` and `[9]`–`[13]` went on
/// citing six of them.
///
/// Two shapes are out of scope and skipped rather than reported:
///
/// - a `Hinweis:` naming a value carried by *another* message type
///   („Wert aus BGM+Z33 DE1004 der IFTSTA"), which this profile cannot hold;
/// - a segment whose qualifier the AHB leaves free — COMDIS `SG3 AJT` DE 4465
///   takes its codes from the EBD named beside it, so the AHB prints a bare
///   `X` and the profile admits no code list to check against.
fn codes_cited_by_conditions_exist(dir: &str, ahb: &serde_json::Value, mig: &Mig) -> Vec<String> {
    let mut out = Vec::new();
    let Some(conditions) = ahb.get("conditions").and_then(|v| v.as_object()) else {
        return out;
    };

    // Segment `Nr` → tag, from the MIG's Nachrichtenstruktur.
    let mut tag_of: BTreeMap<String, String> = BTreeMap::new();
    collect_tags(&mig.structure, &mut tag_of);

    // Tag → (codes admitted anywhere, whether any operand is free-form).
    let mut per_tag: BTreeMap<String, (BTreeSet<String>, bool)> = BTreeMap::new();
    for af in ahb
        .get("anwendungsfaelle")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        for el in af
            .get("elements")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let Some(tag) = el
                .get("nr")
                .and_then(|v| v.as_str())
                .and_then(|nr| tag_of.get(nr))
            else {
                continue;
            };
            let entry = per_tag.entry(tag.clone()).or_default();
            for op in el
                .get("operands")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                match op.get("code").and_then(|v| v.as_str()) {
                    Some(c) => {
                        entry.0.insert(c.to_owned());
                    }
                    None => entry.1 = true,
                }
            }
        }
    }

    for (id, text) in conditions {
        let Some(text) = text.as_str() else { continue };
        if text.trim_start().starts_with("Hinweis") {
            continue;
        }
        for (tag, code) in cited_codes(text) {
            let Some((admitted, free_form)) = per_tag.get(&tag) else {
                continue;
            };
            if *free_form || admitted.is_empty() || admitted.contains(&code) {
                continue;
            }
            out.push(format!(
                "{dir}: Bedingung [{id}] cites {tag}+{code}, which no Anwendungsfall admits — {text}"
            ));
        }
    }
    out
}

/// Walk the MIG structure, recording each segment node's `nr` → `tag`.
fn collect_tags(nodes: &[serde_json::Value], out: &mut BTreeMap<String, String>) {
    for n in nodes {
        if let (Some(nr), Some(tag)) = (
            n.get("nr").and_then(|v| v.as_str()),
            n.get("tag").and_then(|v| v.as_str()),
        ) {
            out.insert(nr.to_owned(), tag.to_owned());
        }
        if let Some(children) = n.get("children").and_then(|v| v.as_array()) {
            collect_tags(children, out);
        }
        if let Some(children) = n.get("segments").and_then(|v| v.as_array()) {
            collect_tags(children, out);
        }
    }
}

/// The `TAG+CODE` pairs a Bedingung text names.
///
/// A trailing `/` list (`AJT+A01/A04/A06`) names several under one tag.
fn cited_codes(text: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c != '+' || i < 3 {
            continue;
        }
        // A three-letter uppercase segment tag directly before the `+`, and
        // nothing alphanumeric before it.
        if !chars[i - 3..i].iter().all(|c| c.is_ascii_uppercase()) {
            continue;
        }
        if i >= 4 && chars[i - 4].is_ascii_alphanumeric() {
            continue;
        }
        let tag: String = chars[i - 3..i].iter().collect();
        let mut j = i + 1;
        let mut cur = String::new();
        while j < chars.len() {
            let ch = chars[j];
            if ch.is_ascii_alphanumeric() {
                cur.push(ch);
            } else if (ch == '/' || ch == ' ') && !cur.is_empty() {
                out.insert((tag.clone(), std::mem::take(&mut cur)));
                // A space ends the list unless a `/` continues it.
                if ch == ' ' && chars.get(j + 1) != Some(&'/') {
                    break;
                }
            } else {
                break;
            }
            j += 1;
        }
        if !cur.is_empty() {
            out.insert((tag.clone(), cur));
        }
    }
    out
}

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

/// The `publikationsdatum` Allgemeine Festlegungen 6.1d § 2.5 fixes for a
/// profile applying on `valid_from`, or `None` for an ausserordentliche release.
///
/// § 2.5.1 and § 2.5.2 set one timetable each and there are only two: a release
/// applying **01.10.** has its consulted documents published **01.04.**, and one
/// applying **01.04.** publishes **01.10.** Six months is the Festlegung's own
/// schedule, so for a regular Anwendungszeitpunkt the date is not observed — it
/// is *entailed*, and a stored value that disagrees is a typo in one of the two
/// fields.
///
/// An ausserordentliche release — mako carries 01.01.2026 and 06.06.2025 — sits
/// outside both timetables. Its Veröffentlichungszeitpunkt is whatever BDEW
/// chose and this function cannot derive it, which is why such a profile states
/// none rather than a guessed one.
///
/// **This is why no release lead time can be computed from the field.** It
/// restates `valid_from`; a metric over it would measure the Festlegung's
/// schedule against itself. BDEW publishes no per-document publication date
/// either — the catalogue carries a `publicationDate` column and leaves it empty
/// on every record — so the figure has to come from when a profile actually
/// entered this repository, which is git's to answer and not this file's.
fn entailed_publikationsdatum(valid_from: &str) -> Option<String> {
    let d = date(valid_from)?;
    match (d.month() as u8, d.day()) {
        (10, 1) => Some(format!("{}-04-01", d.year())),
        (4, 1) => Some(format!("{}-10-01", d.year() - 1)),
        _ => None,
    }
}

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

        // Allgemeine Festlegungen 6.1d § 2.5 — see `entailed_publikationsdatum`.
        match (
            entailed_publikationsdatum(&src.valid_from),
            src.publikationsdatum.as_deref(),
        ) {
            (Some(entailed), Some(stated)) if stated != entailed => errors.push(format!(
                "{dir}: publikationsdatum {stated} contradicts valid_from {}. \
                 Allgemeine Festlegungen 6.1d § 2.5 publishes a release applying \
                 {} on {entailed}; one of the two dates is a typo",
                src.valid_from, src.valid_from,
            )),
            (Some(entailed), None) => errors.push(format!(
                "{dir}: valid_from {} is a regular Anwendungszeitpunkt, so \
                 Allgemeine Festlegungen 6.1d § 2.5 fixes its publikationsdatum at \
                 {entailed} — state it",
                src.valid_from,
            )),
            (None, Some(stated)) => errors.push(format!(
                "{dir}: valid_from {} is an ausserordentliche Anwendungszeitpunkt, \
                 which neither § 2.5.1 nor § 2.5.2 covers, so the stated \
                 publikationsdatum {stated} is not entailed by anything. Remove it, \
                 or carry the date the document itself names",
                src.valid_from,
            )),
            _ => {}
        }
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
                errors.extend(codes_cited_by_conditions_exist(dir, &raw, &mig));
                errors.extend(unregistered_formatbedingungen(dir, &raw));
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
        // `import-profiles` copies this across, but it only proves the copy
        // where the document mirror is — and the mirror is gitignored, so on a
        // CI checkout that comparison skips. Here it runs everywhere.
        if mig.publikationsdatum != src.publikationsdatum {
            e(format!(
                "publikationsdatum {:?} does not match sources.json {:?}",
                mig.publikationsdatum, src.publikationsdatum
            ));
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

#[cfg(test)]
mod tests {
    use super::{UNEVALUATED_FORMATBEDINGUNGEN, entailed_publikationsdatum, format_ids};

    /// The exemption list only shrinks.
    ///
    /// `unregistered_formatbedingungen` catches a number that gains no
    /// evaluator; this catches the other direction, which no run of the guard
    /// can see — a row left standing after its evaluator was written. Without
    /// it the list becomes a place a registered number can hide.
    #[test]
    fn an_exemption_whose_evaluator_exists_is_refused() {
        for (id, _) in UNEVALUATED_FORMATBEDINGUNGEN {
            assert!(
                !edi_energy::profile::formatbedingung::is_registered(id),
                "[{id}] has an evaluator now — delete its row from \
                 UNEVALUATED_FORMATBEDINGUNGEN"
            );
        }
    }

    /// A reason is worth nothing if it describes a number no AHB cites.
    #[test]
    fn every_exemption_is_still_cited_by_a_binding_place() {
        let profiles = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join(super::PROFILES_DIR);
        let mut cited = std::collections::BTreeSet::new();
        let mut seen_profiles = 0;
        for ty in std::fs::read_dir(&profiles).into_iter().flatten().flatten() {
            for fv in std::fs::read_dir(ty.path()).into_iter().flatten().flatten() {
                let Ok(text) = std::fs::read_to_string(fv.path().join("ahb.json")) else {
                    continue;
                };
                seen_profiles += 1;
                let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                for af in raw["anwendungsfaelle"].as_array().into_iter().flatten() {
                    for el in af["elements"].as_array().into_iter().flatten() {
                        for op in el["operands"].as_array().into_iter().flatten() {
                            let Some(t) = op["operand"].as_str() else {
                                continue;
                            };
                            if matches!(t.split_whitespace().next(), Some("X" | "M")) {
                                cited.extend(format_ids(t));
                            }
                        }
                    }
                }
            }
        }
        assert!(seen_profiles > 20, "read only {seen_profiles} profiles");
        for (id, _) in UNEVALUATED_FORMATBEDINGUNGEN {
            assert!(
                cited.contains(*id),
                "[{id}] is exempted but no binding place cites it — delete the row"
            );
        }
    }

    #[test]
    fn only_a_nine_hundred_number_or_a_ub_constrains_a_value() {
        let ids = format_ids("X [914] ∧ [937] [22] ∨ [2000] ∧ [1P0..1] [UB1]");
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            vec!["914".to_owned(), "937".to_owned(), "UB1".to_owned()]
        );
    }

    /// Allgemeine Festlegungen 6.1d § 2.5.1 — a release applying **01.10.** has
    /// its consulted documents published **01.04.** of the same year.
    #[test]
    fn an_october_release_publishes_in_april_of_the_same_year() {
        assert_eq!(
            entailed_publikationsdatum("2026-10-01").as_deref(),
            Some("2026-04-01")
        );
    }

    /// § 2.5.2 — a release applying **01.04.** publishes **01.10.** of the year
    /// before. The year decrement is the half a six-month subtraction written by
    /// hand gets wrong.
    #[test]
    fn an_april_release_publishes_in_october_of_the_year_before() {
        assert_eq!(
            entailed_publikationsdatum("2026-04-01").as_deref(),
            Some("2025-10-01")
        );
    }

    /// An ausserordentliche release sits outside both timetables, so nothing is
    /// entailed and the profile states no `publikationsdatum` rather than a
    /// guessed one. mako carries two: 01.01.2026 and 06.06.2025.
    #[test]
    fn an_ausserordentliche_release_entails_no_publication_date() {
        assert_eq!(entailed_publikationsdatum("2026-01-01"), None);
        assert_eq!(entailed_publikationsdatum("2025-06-06"), None);
    }

    /// The input a passing run would never show: a date six months off in the
    /// wrong direction still reads as plausible, and only the timetable says
    /// which of the pair is wrong.
    #[test]
    fn a_plausible_wrong_date_is_not_what_the_timetable_entails() {
        // Naively "valid_from minus six months" with the year left alone.
        assert_ne!(
            entailed_publikationsdatum("2026-04-01").as_deref(),
            Some("2026-10-01")
        );
        // An unparseable date entails nothing rather than defaulting.
        assert_eq!(entailed_publikationsdatum("not-a-date"), None);
    }
}
