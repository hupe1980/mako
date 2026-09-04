//! `cargo xtask import-profiles` — regenerate every profile from its BDEW
//! publication.
//!
//! `crates/edi-energy/profiles/sources.json` names, per profile directory, the
//! MIG and AHB PDF in the document mirror and the dates that frame the
//! Formatversion. This task reads those PDFs (`pdftotext -layout`), parses the
//! Nachrichtenstruktur, the Segmentlayout and the Prüfschablonen, and writes
//! `<dir>/mig.json` and `<dir>/ahb.json`. The JSON files are derived
//! artefacts: they are committed so the crate builds without the mirror, and
//! `--check` proves they still match their sources.
//!
//! ```text
//! cargo xtask import-profiles                    # all profiles
//! cargo xtask import-profiles --profile utilmd/fv20261001
//! cargo xtask import-profiles --check            # drift guard (needs the mirror)
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::bdew::{self, ahb, mig};

const PROFILES_DIR: &str = "crates/edi-energy/profiles";
const MIRROR_DIR: &str = "regulatories/bdew-mako";

#[derive(Debug, Deserialize)]
struct Sources {
    profiles: BTreeMap<String, Source>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Source {
    pub release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    pub valid_from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publikationsdatum: Option<String>,
    pub ahb_version: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pid_exempt: bool,
    /// Prüfidentifikatoren the BDEW Anwendungsübersicht assigns to columns
    /// whose AHB prints none (APERAK: 29001 Fehlermeldung, 29002
    /// Anerkennungsmeldung), by column name. The wire does not carry them;
    /// the column is selected by `BGM` DE 1001.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pids: BTreeMap<String, u32>,
    pub mig: String,
    pub ahb: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    files: BTreeMap<String, ManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    title: String,
    #[serde(default)]
    sha256: String,
}

/// Entry point.
pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let root = Path::new(workspace_root);
    let check = args.iter().any(|a| a == "--check");
    let only: Option<&str> = args
        .iter()
        .position(|a| a == "--profile")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);

    let sources: Sources =
        match std::fs::read_to_string(root.join(PROFILES_DIR).join("sources.json"))
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read profiles/sources.json: {e}");
                return false;
            }
        };
    let manifest: BTreeMap<String, ManifestEntry> =
        std::fs::read_to_string(root.join(MIRROR_DIR).join("manifest.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Manifest>(&s).ok())
            .map(|m| m.files)
            .unwrap_or_default();

    // Without the document mirror (gitignored) or poppler there is nothing to
    // compare against; a check that cannot run says so rather than failing.
    let mirror_present = root.join(MIRROR_DIR).join("manifest.json").is_file();
    let pdftotext_present = std::process::Command::new("pdftotext")
        .arg("-v")
        .output()
        .is_ok();
    if check && !(mirror_present && pdftotext_present) {
        eprintln!(
            "import-profiles --check: SKIP — {} (run `cargo xtask sync-regulatories --download` and install poppler)",
            if mirror_present {
                "pdftotext is not installed"
            } else {
                "regulatories/bdew-mako/ is not mirrored here"
            }
        );
        return true;
    }

    let mut ok = true;
    let mut written = 0;
    for (dir, src) in &sources.profiles {
        if only.is_some_and(|o| o != dir) {
            continue;
        }
        match import_one(root, &manifest, dir, src) {
            Ok((mig_json, ahb_json)) => {
                let out_dir = root.join(PROFILES_DIR).join(dir);
                for (name, value) in [("mig.json", mig_json), ("ahb.json", ahb_json)] {
                    let path = out_dir.join(name);
                    let text = serde_json::to_string_pretty(&value).unwrap() + "\n";
                    if check {
                        let current = std::fs::read_to_string(&path).unwrap_or_default();
                        if current != text {
                            eprintln!("DRIFT   {dir}/{name} differs from its source PDF");
                            ok = false;
                        }
                    } else {
                        if let Err(e) = std::fs::create_dir_all(&out_dir) {
                            eprintln!("error: {}: {e}", out_dir.display());
                            return false;
                        }
                        if let Err(e) = std::fs::write(&path, text) {
                            eprintln!("error: {}: {e}", path.display());
                            return false;
                        }
                        written += 1;
                    }
                }
                eprintln!("ok      {dir}");
            }
            Err(e) => {
                eprintln!("error   {dir}: {e}");
                ok = false;
            }
        }
    }
    if !check {
        eprintln!("import-profiles: {written} files written");
    }
    ok
}

fn import_one(
    root: &Path,
    manifest: &BTreeMap<String, ManifestEntry>,
    dir: &str,
    src: &Source,
) -> Result<(Value, Value), String> {
    let message_type = dir
        .split('/')
        .next()
        .ok_or("profile directory must be <type>/<fv>")?
        .to_ascii_uppercase();
    let mig_pdf = mirror_path(root, &src.mig)?;
    let ahb_pdf = mirror_path(root, &src.ahb)?;
    let mig_doc = mig::parse(&bdew::pdf_lines(&mig_pdf)?, &message_type)
        .map_err(|e| format!("{}: {e}", src.mig))?;
    let stamp = |mut doc: ahb::AhbDoc| -> Result<ahb::AhbDoc, String> {
        for (name, pid) in &src.pids {
            let af = doc
                .anwendungsfaelle
                .iter_mut()
                .find(|a| a.name == *name)
                .ok_or_else(|| format!("{}: no column named {name:?} to carry {pid}", src.ahb))?;
            af.pid = Some(*pid);
        }
        Ok(doc)
    };
    let ahb_doc = stamp(
        ahb::parse(&bdew::pdf_lines(&ahb_pdf)?, &mig_doc)
            .map_err(|e| format!("{}: {e}", src.ahb))?,
    )?;

    sanity(&message_type, src, &mig_doc, &ahb_doc)?;

    let source = |file: &str| {
        let entry = manifest.get(file);
        json!({
            "file": file,
            "title": entry.map(|e| e.title.clone()),
            "sha256": entry.map(|e| e.sha256.clone()),
        })
    };
    let pid_source = if ahb_doc.anwendungsfaelle.iter().any(|a| {
        a.elements
            .iter()
            .any(|e| e.de == "1153" && e.operands.iter().any(|o| o.code.as_deref() == Some("Z13")))
    }) {
        "rff_z13"
    } else {
        "bgm_de1004"
    };

    let mut mig_json = json!({
        "schema_version": 2,
        "message_type": message_type,
        "release": src.release,
        "valid_from": src.valid_from,
        "ahb_version": src.ahb_version,
        "pid_source": pid_source,
        "source": source(&src.mig),
        "structure": mig_doc.structure,
        "envelope": mig_doc.envelope,
    });
    let obj = mig_json.as_object_mut().unwrap();
    if let Some(t) = &src.track {
        obj.insert("track".into(), json!(t));
    }
    if let Some(u) = &src.valid_until {
        obj.insert("valid_until".into(), json!(u));
    }
    if let Some(p) = &src.publikationsdatum {
        obj.insert("publikationsdatum".into(), json!(p));
    }
    if src.pid_exempt {
        obj.insert("pid_exempt".into(), json!(true));
    }

    let ahb_json = json!({
        "schema_version": 2,
        "message_type": message_type,
        "release": src.release,
        "ahb_version": src.ahb_version,
        "source": source(&src.ahb),
        "conditions": ahb_doc.conditions,
        "packages": ahb_doc.packages,
        "anwendungsfaelle": ahb_doc.anwendungsfaelle,
    });

    let missing = unresolved_conditions(&ahb_json);
    if !missing.is_empty() {
        return Err(format!(
            "{message_type}: {} Bedingungen are cited but have no text: {:?}",
            missing.len(),
            missing.iter().take(20).collect::<Vec<_>>()
        ));
    }

    Ok((mig_json, ahb_json))
}

fn mirror_path(root: &Path, file: &str) -> Result<PathBuf, String> {
    let p = root.join(MIRROR_DIR).join(file);
    if p.exists() {
        Ok(p)
    } else {
        Err(format!(
            "{file} is not in the document mirror — run `cargo xtask sync-regulatories --download`"
        ))
    }
}

/// The invariants every extraction must meet before it is written.
fn sanity(
    message_type: &str,
    src: &Source,
    mig_doc: &mig::MigDoc,
    ahb_doc: &ahb::AhbDoc,
) -> Result<(), String> {
    let segs = mig_doc.segments();
    let tags: Vec<&str> = segs.iter().map(|s| s.tag.as_str()).collect();
    if tags.first() != Some(&"UNH") || tags.last() != Some(&"UNT") {
        return Err(format!(
            "structure must run from UNH to UNT, found {:?} … {:?}",
            tags.first(),
            tags.last()
        ));
    }
    if let Some(s) = segs.iter().find(|s| s.elements.is_empty()) {
        return Err(format!("segment {} {} has no layout", s.nr, s.tag));
    }
    let unh = segs[0];
    let has_release = unh
        .elements
        .iter()
        .flat_map(|e| e.components.iter().chain(std::iter::once(e)))
        .any(|e| e.id == "0057" && e.codes.iter().any(|c| c.code == src.release));
    if !has_release {
        return Err(format!(
            "UNH DE 0057 of the MIG does not admit the wire code {:?}",
            src.release
        ));
    }
    let nrs: std::collections::BTreeSet<&str> = segs
        .iter()
        .map(|s| s.nr.as_str())
        .chain(mig_doc.envelope.iter().map(|s| s.nr.as_str()))
        .collect();
    for af in &ahb_doc.anwendungsfaelle {
        let label = af.pid.map_or_else(|| af.name.clone(), |p| p.to_string());
        if af.rows.is_empty() {
            return Err(format!("{message_type} Anwendungsfall {label}: no rows"));
        }
        for row in &af.rows {
            if let Some(nr) = &row.nr
                && !nrs.contains(nr.as_str())
            {
                return Err(format!(
                    "{message_type} Anwendungsfall {label}: AHB row {nr} is not in the MIG structure"
                ));
            }
        }
        let has_unh = af
            .rows
            .iter()
            .any(|r| r.nr.as_deref() == Some(unh.nr.as_str()));
        if !has_unh {
            let first: Vec<String> = af
                .rows
                .iter()
                .take(6)
                .map(|r| {
                    format!(
                        "{}{}={}",
                        r.nr.clone().unwrap_or_default(),
                        r.group.clone().unwrap_or_default(),
                        r.status.join("|")
                    )
                })
                .collect();
            return Err(format!(
                "{message_type} Anwendungsfall {label} ({}): no UNH row; first rows {first:?}",
                af.chapter.clone().unwrap_or_default()
            ));
        }
    }
    if !src.pid_exempt && ahb_doc.anwendungsfaelle.iter().any(|a| a.pid.is_none()) {
        return Err(format!(
            "{message_type}: an Anwendungsfall without Prüfidentifikator"
        ));
    }
    Ok(())
}

/// Every `[n]` a status expression or an operand cites, and that no Bedingungen
/// or Pakete table gives a text.
///
/// A Bedingung the reader loses — its number swallowed by the status expression
/// left of it, or its entry printed next to the header a page break repeats —
/// leaves the citation dangling, and the evaluator then reads the place as
/// unconditioned. The citation is what proves the text was found.
///
/// Takes the serialised profile so the same check guards the import and the
/// committed file, the second of which is all `validate-profiles` has.
pub fn unresolved_conditions(ahb: &Value) -> BTreeSet<String> {
    let table = |key: &str| -> BTreeSet<String> {
        ahb.get(key)
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let conditions = table("conditions");
    let packages = table("packages");

    let mut missing = BTreeSet::new();
    let mut cite = |expr: &str| {
        for id in condition_ids(expr) {
            // `[UB1]`–`[UB3]` are the Zeitpunktangaben of the Allgemeinen
            // Festlegungen; no AHB reprints their text.
            // `[UB1]`–`[UB3]` carry no AHB text (above). A Paket is cited
            // with the occurrence it stands for — `[1P0..1]` — and the table
            // keys it by the Paket alone.
            let known = if id.starts_with("UB") {
                true
            } else if let Some(p) = id.find('P') {
                packages.contains(&id[..=p])
            } else {
                conditions.contains(&id)
            };
            if !known {
                missing.insert(id);
            }
        }
    };
    // Every string under `anwendungsfaelle` that can carry an expression, plus
    // the Paketvoraussetzungen themselves.
    fn walk(node: &Value, cite: &mut impl FnMut(&str)) {
        match node {
            Value::Object(m) => {
                for (k, v) in m {
                    if matches!(k.as_str(), "status" | "operand") {
                        match v {
                            Value::String(s) => cite(s),
                            Value::Array(a) => {
                                for e in a.iter().filter_map(Value::as_str) {
                                    cite(e);
                                }
                            }
                            _ => {}
                        }
                    }
                    walk(v, cite);
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v, cite);
                }
            }
            _ => {}
        }
    }
    if let Some(faelle) = ahb.get("anwendungsfaelle") {
        walk(faelle, &mut cite);
    }
    if let Some(pkgs) = ahb.get("packages").and_then(Value::as_object) {
        for expr in pkgs.values().filter_map(Value::as_str) {
            cite(expr);
        }
    }
    missing
}

/// The `[n]` references in one status expression or operand.
fn condition_ids(expr: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = expr;
    while let Some(open) = rest.find('[') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find(']') else { break };
        let id = &rest[..close];
        // `10`, `2061`, `UB1`, `1P0..1`.
        if !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'P' | 'U' | 'B' | '.'))
        {
            ids.push(id.to_owned());
        }
        rest = &rest[close + 1..];
    }
    ids
}
