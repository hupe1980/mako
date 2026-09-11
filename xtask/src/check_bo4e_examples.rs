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
//! Every fenced block in `site/content/`, `concepts/` and `demos/` that parses
//! as JSON and carries a `_typ`, at any depth. Each such object is decoded into its BO4E
//! type and run through
//! [`Bo4eExtensions::extension_paths`](rubo4e::json::Bo4eExtensions::extension_paths).
//!
//! A `_typ` that is no BO4E discriminant at all is its own finding. It is the
//! same failure as a misspelt `_typ` key one step further along: `rubo4e` has no
//! type to decode the object into, so every field it carries is unchecked, and
//! an integrator who copies it posts a document mako's own gate refuses. Only
//! the objects that actually decode are counted as checked, so the success line
//! states the number of examples this guard stands behind.
//!
//! A `_typ` naming a real BO4E type this guard has no arm for is a COM or a BO
//! outside the dispatch table: neither a finding nor counted.
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

    // `demos/` is in the scan for the same reason the other two are — more so,
    // in fact: a demo payload is what gets copied into a ticket as "this is
    // what a request looks like", and `demos/o2c` shipped a customer payload
    // whose fields the API did not have at all.
    for dir in ["site/content", "concepts", "demos"] {
        let root = workspace_root.join(dir);
        if !root.is_dir() {
            // A checkout without this directory is normal — say so, rather than
            // letting a whole documentation tree fall out of the scan in
            // silence.
            println!("check-bo4e-examples: {dir}/ is absent from this checkout — not scanned");
            continue;
        }
        let mut files = Vec::new();
        collect_docs(&root, &mut files);
        for path in files {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            for value in json_values(&path, &src) {
                for object in objects_with_typ(&value) {
                    if check_one(&rel, &object, &mut findings) {
                        checked += 1;
                    }
                }
                // A misspelt discriminant carries no `_typ`, so the walk
                // above never sees the block and nothing checks it. Worse than
                // a stray field: without `_typ` the decode produces a document
                // of no type and every typed field reads back `None`.
                for path in objects_with_misspelt_typ(&value) {
                    findings.push(format!(
                        "{rel}: object at `{path}` names its BO4E type as \
                         `bo_typ`/`boTyp`; the discriminant is `_typ`, without \
                         which every typed field decodes to `None`"
                    ));
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
        "ERROR: {} documented BO4E example(s) name a type or a field BO4E does not define:\n",
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
///
/// Returns `true` when the object decoded and its fields were checked, so the
/// caller's count states what this guard stands behind rather than what it saw.
fn check_one(rel: &str, value: &serde_json::Value, findings: &mut Vec<String>) -> bool {
    let Some(typ) = value.get("_typ").and_then(serde_json::Value::as_str) else {
        return false;
    };
    // `{ "...": "full BO4E payload" }` is prose standing in for a document, not
    // a document. `elide` handles an ellipsis in *value* position; in **key**
    // position it means the object's contents were left out, and reporting its
    // one placeholder key as an undefined field would be reporting the
    // shorthand rather than anything a reader could copy wrong.
    if value
        .as_object()
        .is_some_and(|o| o.keys().any(|k| k == "..." || k == "…"))
    {
        return false;
    }

    macro_rules! dispatch {
        ($($wire:literal => $ty:ty),* $(,)?) => {
            match typ {
                $($wire => match serde_json::from_value::<$ty>(value.clone()) {
                    // A block that does not decode is prose, a fragment, or an
                    // ellipsis — not this guard's business. Only a document
                    // that *reads* can be checked for what it carries.
                    Err(_) => return false,
                    Ok(v) => v.extension_paths(),
                },)*
                // A COM, or a BO outside the dispatch table: a real BO4E
                // discriminant with nothing here to decode it into.
                _ if rubo4e::current::BoTyp::from_wire(typ).is_ok()
                    || rubo4e::current::ComTyp::from_wire(typ).is_ok() =>
                {
                    return false;
                }
                _ => {
                    findings.push(format!(
                        "{rel}: documented `_typ` {typ:?} names no BO4E type — `rubo4e` has \
                         nothing to decode the object into, so none of its field names is \
                         checked and an integrator who copies it posts a document the gate \
                         refuses"
                    ));
                    return false;
                }
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
    true
}

/// Every object carrying a `_typ`, at any depth.
fn objects_with_typ(root: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut stack = vec![(root.clone(), None::<String>)];
    while let Some((cur, key)) = stack.pop() {
        match &cur {
            serde_json::Value::Object(o) => {
                if o.contains_key("_typ") {
                    out.push(cur.clone());
                } else if let Some(wire) = key.as_deref().and_then(typ_from_field_name) {
                    // A BO4E document whose `_typ` the *gate* injects. That is
                    // the shape a real request has — the endpoint fixes which
                    // BO it takes, so a caller repeating the discriminant adds
                    // a way to be wrong and no information — and it is exactly
                    // the shape a demo payload carries. Without this arm the
                    // field names in `"geschaeftspartner": { … }` are checked
                    // by nothing, and a misspelled `nachnamen` ships.
                    let mut stamped = o.clone();
                    stamped.insert("_typ".into(), wire.into());
                    out.push(serde_json::Value::Object(stamped));
                }
                stack.extend(o.iter().map(|(k, v)| (v.clone(), Some(k.clone()))));
            }
            serde_json::Value::Array(a) => {
                stack.extend(a.iter().map(|v| (v.clone(), key.clone())));
            }
            _ => {}
        }
    }
    out
}

/// The BO4E discriminant a **field name** implies, if it names a type.
///
/// `"geschaeftspartner"` is a `GESCHAEFTSPARTNER`, `"standort_adresse"` an
/// `ADRESSE`, `"kosten_json"` a `KOSTEN` — the same reading
/// `check-request-bodies` does over Rust field names, applied to JSON keys.
/// Anything that is not a BO4E type's own name (`data`, `payload`, `view`)
/// yields `None`.
fn typ_from_field_name(key: &str) -> Option<&'static str> {
    let base = key.strip_suffix("_json").unwrap_or(key);
    let last = base.rsplit('_').next().unwrap_or(base);
    for candidate in [base, last] {
        let wire = candidate.to_uppercase();
        if let Ok(t) = rubo4e::current::BoTyp::from_wire(&wire) {
            return Some(t.as_wire());
        }
        if let Ok(t) = rubo4e::current::ComTyp::from_wire(&wire) {
            return Some(t.as_wire());
        }
    }
    None
}

/// Replace the `...` a documented example uses for brevity with real JSON.
///
/// A block written `"preispositionen": [ ... ]` is not JSON, so the guard used
/// to skip it whole — and with it every sibling field in the same object. Nine
/// A block written with an ellipsis is invisible to the guard otherwise, and a
/// misspelt discriminant inside one is checked by nothing at all.
///
/// The elided value itself cannot be checked — that is the point of eliding it
/// — but its *siblings* can, and they are where the near-miss field names are.
fn elide(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.char_indices().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some((i, c)) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            // `...` and the single-character ellipsis both stand for "more of
            // the same"; `null` is the shortest value that keeps the JSON well
            // formed wherever they appear — as an array element, an object
            // value, or a lone member.
            '.' if src[i..].starts_with("...") => {
                chars.next();
                chars.next();
                out.push_str("null");
            }
            '\u{2026}' => out.push_str("null"),
            _ => out.push(c),
        }
    }
    // A bare `null` standing in for "more members" is not a legal object member;
    // drop it along with the comma that joins it to its neighbour.
    out.replace(", null }", " }")
        .replace(",null}", "}")
        .replace("{ null }", "{}")
        .replace("{null}", "{}")
}

/// Paths of objects that name a BO4E type under the wrong key.
///
/// Only near-misses of the discriminant itself count: an object carrying
/// `bo_typ` or `boTyp` and no `_typ` meant to be a BO4E document and is not one.
fn objects_with_misspelt_typ(root: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(String::from("$"), root.clone())];
    while let Some((path, cur)) = stack.pop() {
        match &cur {
            serde_json::Value::Object(o) => {
                if !o.contains_key("_typ")
                    && o.keys().any(|k| {
                        k.eq_ignore_ascii_case("bo_typ") || k.eq_ignore_ascii_case("boTyp")
                    })
                {
                    out.push(path.clone());
                }
                for (k, v) in o {
                    stack.push((format!("{path}.{k}"), v.clone()));
                }
            }
            serde_json::Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    stack.push((format!("{path}[{i}]"), v.clone()));
                }
            }
            _ => {}
        }
    }
    out.sort();
    out
}

/// The contents of every ``` fenced block.
/// Every JSON value a documentation or demo file carries.
///
/// Markdown keeps its examples in fenced blocks, so those are extracted first
/// and each parsed whole (HTTP-style blocks put headers in front of the body,
/// hence the scan to the first `{`). A shell script or a bare `.json` fixture
/// has no fences: its payloads are heredocs and literals scattered through
/// other text, so every `{` is tried as the start of a value and the scan
/// resumes after whatever parsed.
///
/// A payload interpolating a shell variable as a **bare** value
/// (`"jahr": ${YEAR}`) is not JSON and is skipped. Inside a string
/// (`"malo_id": "${MALO_ID}"`) — which is every substitution these demos make —
/// the document stays well-formed and is checked.
fn json_values(path: &Path, src: &str) -> Vec<serde_json::Value> {
    if path.extension().is_some_and(|e| e == "md") {
        return fenced_blocks(src)
            .iter()
            .filter_map(|block| {
                let start = block.find('{')?;
                serde_json::from_str::<serde_json::Value>(&elide(&block[start..])).ok()
            })
            .collect();
    }
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find('{') {
        let candidate = elide(&rest[i..]);
        match serde_json::Deserializer::from_str(&candidate)
            .into_iter::<serde_json::Value>()
            .next()
        {
            Some(Ok(value)) if value.is_object() => {
                out.push(value);
                // Resume after the opening brace rather than after the value:
                // `elide` may have rewritten the text, so an offset into the
                // rewritten string does not map back. One brace forward always
                // makes progress and nested objects are walked anyway.
                rest = &rest[i + 1..];
            }
            _ => rest = &rest[i + 1..],
        }
    }
    out
}

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

/// Markdown, plus the shell scripts a demo's payloads actually live in.
///
/// A demo's request bodies are heredocs inside `smoke.sh`, not fenced blocks in
/// its README — and those are the ones an integrator runs and copies. The
/// fenced-block scanner reads a `.sh` file as one block, which is exactly right
/// here: every `{ … }` in it that parses as JSON and carries a `_typ` gets
/// checked, and a `${VAR}` inside a string leaves the JSON well-formed.
fn collect_docs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_docs(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e == "md" || e == "sh" || e == "json")
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check_one;

    /// The spelling that shipped: `marktd`'s price-sheet examples documented
    /// `PREISBLATT_NETZNUTZUNG`, which BO4E spells without the underscore, so
    /// nothing decoded and nothing was checked while the success line counted
    /// the object anyway.
    #[test]
    fn an_unknown_typ_is_a_finding_and_is_not_counted() {
        let mut findings = Vec::new();
        let value = serde_json::json!({
            "_typ": "PREISBLATT_NETZNUTZUNG",
            "bezeichnung": "Netznutzungspreise 2025",
        });
        assert!(!check_one("doc.md", &value, &mut findings));
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].contains("names no BO4E type"), "{findings:?}");
    }

    /// The correct spelling decodes, is checked, and is counted.
    #[test]
    fn a_decodable_bo_is_checked_and_counted() {
        let mut findings = Vec::new();
        let value = serde_json::json!({
            "_typ": "PREISBLATTNETZNUTZUNG",
            "bezeichnung": "Netznutzungspreise 2025",
        });
        assert!(check_one("doc.md", &value, &mut findings));
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// A COM is a real BO4E discriminant with no arm in the dispatch table:
    /// nothing to decode it into, and nothing to report.
    #[test]
    fn a_com_is_neither_a_finding_nor_counted() {
        let mut findings = Vec::new();
        let value = serde_json::json!({ "_typ": "BETRAG", "wert": "12.50" });
        assert!(!check_one("doc.md", &value, &mut findings));
        assert!(findings.is_empty(), "{findings:?}");
    }
}
