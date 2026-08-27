//! Guard: every BO4E example in the docs uses fields BO4E actually defines.
//!
//! ## Why a documentation example is worth a CI guard
//!
//! An example is copied. A field name that BO4E does not define does not fail
//! anywhere a reader would notice: serde ignores a key no field declares,
//! `rubo4e` files it under `_additional`, the decode returns `Ok`, and the field
//! the key was meant to fill reads back `None`. So an integrator who copies the
//! example ships a document whose value is silently missing, and mako's own
//! docs are what taught them to.
//!
//! The traps are near-misses: `gueltigkeit` for `zeitlicheGueltigkeit`, or a
//! `Marktteilnehmer` field on a `Geschaeftspartner`.
//!
//! ## What it checks
//!
//! Every fenced block in `site/content/` and `concepts/` that parses as JSON and
//! carries a `_typ`, at any depth. Each such object is decoded into its BO4E
//! type and run through
//! [`Bo4eExtensions::extension_paths`](rubo4e::json::Bo4eExtensions::extension_paths).
//!
//! HTTP-style blocks (headers, blank line, body) are handled: the scan starts at
//! the first `{`.
//!
//! ## What is allowed
//!
//! [`ALLOWED`] — mako's own extension fields, which are a deliberate design
//! decision rather than a typo, each with the reason. Anything else is a
//! finding.

use rubo4e::json::Bo4eExtensions as _;
use std::path::{Path, PathBuf};

/// Extension fields mako documents on purpose, with why.
///
/// Keyed by `(BO4E _typ, extension path)`. mako's *own* vocabulary normally
/// rides in `zusatzAttribute` under the `mako:` namespace — see
/// `check-bo4e-attributes` — so a row here is the rarer case: a whole nested BO
/// that BO4E gives the parent no field for.
const ALLOWED: &[(&str, &str, &str)] = &[(
    "MESSLOKATION",
    "standorteigenschaften",
    "Standorteigenschaften is a standalone BO (#25), and BO4E gives Messlokation no field for it; \
     marktd parses it as the BO it names and reads the Regelzone EIC off the typed value",
)];

/// Run the guard. Returns `true` when every documented example is clean.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for dir in ["site/content", "concepts"] {
        let mut files = Vec::new();
        collect_md(&workspace_root.join(dir), &mut files);
        for path in files {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            for block in fenced_blocks(&src) {
                // HTTP-style examples put headers before the body.
                let Some(start) = block.find('{') else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&block[start..]) else {
                    continue;
                };
                for object in objects_with_typ(&value) {
                    checked += 1;
                    check_one(&rel, &object, &mut findings);
                }
            }
        }
    }

    if findings.is_empty() {
        println!(
            "check-bo4e-examples: {checked} documented BO4E object(s), all using fields BO4E defines"
        );
        return true;
    }

    eprintln!(
        "ERROR: {} documented BO4E example(s) use a field BO4E does not define:\n",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!(
        "\nAn example is copied. A field BO4E does not define is absorbed into\n\
         `_additional`: the decode succeeds, the field reads back `None`, and the\n\
         integrator ships a document with the value silently missing. Check the\n\
         name against the schema — the trap is a near-miss like `gueltigkeit` for\n\
         `zeitlicheGueltigkeit`. mako's own vocabulary belongs in\n\
         `zusatzAttribute` as `mako:<snake_case>` (see check-bo4e-attributes)."
    );
    false
}

/// Decode one `_typ`-carrying object and report its extension fields.
fn check_one(rel: &str, value: &serde_json::Value, findings: &mut Vec<String>) {
    let Some(typ) = value.get("_typ").and_then(serde_json::Value::as_str) else {
        return;
    };

    macro_rules! dispatch {
        ($($wire:literal => $ty:ty),* $(,)?) => {
            match typ {
                $($wire => match serde_json::from_value::<$ty>(value.clone()) {
                    // A block that does not decode is prose, a fragment, or an
                    // ellipsis — not this guard's business. Only a document
                    // that *reads* can be checked for what it carries.
                    Err(_) => return,
                    Ok(v) => v.extension_paths(),
                },)*
                _ => return,
            }
        };
    }

    let paths = dispatch!(
        "ANGEBOT" => rubo4e::current::Angebot,
        "BILANZIERUNG" => rubo4e::current::Bilanzierung,
        "ENERGIEMENGE" => rubo4e::current::Energiemenge,
        "ENERGIEMIX" => rubo4e::current::Energiemix,
        "FREMDKOSTEN" => rubo4e::current::Fremdkosten,
        "GERAET" => rubo4e::current::Geraet,
        "GESCHAEFTSPARTNER" => rubo4e::current::Geschaeftspartner,
        "KOSTEN" => rubo4e::current::Kosten,
        "LASTGANG" => rubo4e::current::Lastgang,
        "MARKTLOKATION" => rubo4e::current::Marktlokation,
        "MARKTTEILNEHMER" => rubo4e::current::Marktteilnehmer,
        "MESSLOKATION" => rubo4e::current::Messlokation,
        "NETZLOKATION" => rubo4e::current::Netzlokation,
        "PERSON" => rubo4e::current::Person,
        "PREISBLATTMESSUNG" => rubo4e::current::PreisblattMessung,
        "PREISBLATTNETZNUTZUNG" => rubo4e::current::PreisblattNetznutzung,
        "RECHNUNG" => rubo4e::current::Rechnung,
        "STANDORTEIGENSCHAFTEN" => rubo4e::current::Standorteigenschaften,
        "STEUERBARERESSOURCE" => rubo4e::current::SteuerbareRessource,
        "TARIFINFO" => rubo4e::current::Tarifinfo,
        "TARIFPREISBLATT" => rubo4e::current::Tarifpreisblatt,
        "TECHNISCHERESSOURCE" => rubo4e::current::TechnischeRessource,
        "VERTRAG" => rubo4e::current::Vertrag,
        "ZAEHLER" => rubo4e::current::Zaehler,
        "ZAEHLZEITDEFINITION" => rubo4e::current::Zaehlzeitdefinition,
        "ZEITREIHE" => rubo4e::current::Zeitreihe,
    );

    for path in paths {
        if ALLOWED
            .iter()
            .any(|(t, p, _)| *t == typ && *p == path.as_str())
        {
            continue;
        }
        findings.push(format!("{rel}  {typ}.{path}"));
    }
}

/// Every object carrying a `_typ`, at any depth.
fn objects_with_typ(root: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(cur) = stack.pop() {
        match &cur {
            serde_json::Value::Object(o) => {
                if o.contains_key("_typ") {
                    out.push(cur.clone());
                }
                stack.extend(o.values().cloned());
            }
            serde_json::Value::Array(a) => stack.extend(a.iter().cloned()),
            _ => {}
        }
    }
    out
}

/// The contents of every ``` fenced block.
fn fenced_blocks(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in src.lines() {
        if line.trim_start().starts_with("```") {
            match cur.take() {
                Some(b) => out.push(b),
                None => cur = Some(String::new()),
            }
            continue;
        }
        if let Some(b) = cur.as_mut() {
            b.push_str(line);
            b.push('\n');
        }
    }
    out
}

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}
