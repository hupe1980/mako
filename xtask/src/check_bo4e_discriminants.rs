//! Guard: a BO4E `_typ` is read off the type, never written down.
//!
//! ## Why
//!
//! BO4E pins every schema's discriminant with a JSON Schema `const`, and the
//! generated types expose it three ways that cannot disagree with each other:
//!
//! - `T::TYP` — the discriminant enum value,
//! - `T::TYP_WIRE` — its wire string,
//! - `Default::default()` — which *stamps* `_typ` on every BO **and** every COM.
//!
//! A hand-written discriminant is a fourth spelling, and it is the one that can
//! be wrong. Two shapes are refused:
//!
//! ```ignore
//! // 1. Redundant: `..Default::default()` on the same literal already sets it.
//! Zaehlzeitdefinition { typ: Some(BoTyp::Zaehlzeitdefinition), ..Default::default() }
//!
//! // 2. Assembling a BO4E document as JSON, spelling `_typ` by hand.
//! serde_json::json!({ "_typ": "GESCHAEFTSPARTNER", "organisationsname": name })
//! ```
//!
//! The second is the damaging one, and not because of the discriminant. A
//! document built as `json!` skips the typed constructor entirely, so nothing
//! checks its field names against the schema — `rubo4e` captures unknown keys
//! in `_additional` rather than rejecting them, so a misspelled field decodes
//! cleanly, reads back as `None`, and ships with the value missing. A decode
//! round-trip does not catch it either, for the same reason.
//!
//! Build the value typed and let `rubo4e` stamp the discriminant.
//!
//! ## What is allowed
//!
//! Reading (`data.get("_typ")`), the gate's own injection, and test fixtures —
//! a fixture *is* an untrusted payload, and writing one by hand is the point.

use std::collections::BTreeSet;
use std::path::Path;

/// Files exempt from the `"_typ"` literal rule, with the reason.
///
/// Empty, and worth keeping that way: the gate itself injects `_typ`, but it
/// injects `T::TYP_WIRE` — a value read off the type — so even the one place
/// that writes the field never spells a discriminant. An entry here is a claim
/// that a literal discriminant is correct somewhere, which should have to be
/// argued in writing.
const EXEMPT: &[(&str, &str)] = &[];

/// Run the guard. Returns `true` when the tree is clean.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings: Vec<String> = Vec::new();
    let exempt: BTreeSet<&str> = EXEMPT.iter().map(|(p, _)| *p).collect();

    for dir in ["crates", "services"] {
        collect(
            &workspace_root.join(dir),
            workspace_root,
            &exempt,
            &mut findings,
        );
    }

    if findings.is_empty() {
        println!(
            "check-bo4e-discriminants: no hand-written `_typ` in shipped code \
             ({} documented exemption(s))",
            EXEMPT.len()
        );
        return true;
    }

    eprintln!(
        "ERROR: {} hand-written BO4E discriminant(s):\n",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!(
        "\nA BO4E `_typ` is the type's own fact. Build the value typed and let\n\
         `rubo4e` stamp it — `Default` does so for every BO and every COM since\n\
         every COM, and `T::TYP_WIRE` reads it without a value. Assembling a document\n\
         as `json!` also skips every field-name check: rubo4e absorbs unknown\n\
         keys into `_additional`, so a misspelled field ships as missing."
    );
    false
}

fn collect(dir: &Path, root: &Path, exempt: &BTreeSet<&str>, findings: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, root, exempt, findings);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        // A test file is allowed to spell a payload by hand — that is what an
        // untrusted fixture is. Only shipped code is scanned.
        if rel.contains("/tests/") || rel.ends_with("/tests.rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line_no, hit) in offending_lines(&src, exempt.contains(rel.as_str())) {
            findings.push(format!("{rel}:{line_no}  {hit}"));
        }
    }
}

/// Every offending line in one file, as `(1-based line, message)`.
///
/// Comments are stripped first: a `_typ` in a doc comment is documentation, and
/// the shapes this refuses are worth *showing* in prose.
fn offending_lines(src: &str, typ_literal_exempt: bool) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_test_mod = false;
    let mut test_mod_depth = 0i32;
    let mut depth = 0i32;

    for (i, raw) in src.lines().enumerate() {
        let line = strip_comment(raw);
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;

        // Track `#[cfg(test)] mod …` so a unit-test module inside a shipped
        // file is skipped for the same reason a `tests/` file is.
        if !in_test_mod && line.contains("mod ") && line.contains('{') && was_cfg_test(src, i) {
            in_test_mod = true;
            test_mod_depth = depth;
        }
        depth += opens - closes;
        if in_test_mod && depth <= test_mod_depth {
            in_test_mod = false;
            continue;
        }
        if in_test_mod {
            continue;
        }

        // 1. `typ: Some(BoTyp::X)` / `typ: Some(ComTyp::X)` in a struct literal.
        if writes_discriminant_field(&line) {
            out.push((
                i + 1,
                "a hand-written `typ:` field — `..Default::default()` stamps `_typ`                  for BOs and COMs, and `T::new(..)` does for the two BOs that have                  no `Default`"
                    .to_owned(),
            ));
            continue;
        }

        // 2. `"_typ"` written as a value, i.e. followed by `:` and a string, or
        //    inserted into a map. Reading (`get("_typ")`) is untouched.
        if !typ_literal_exempt && writes_typ(&line) {
            out.push((
                i + 1,
                "`\"_typ\"` written by hand — build the value typed so rubo4e stamps it".to_owned(),
            ));
        }
    }
    out
}

/// A `typ:` field written by hand — either a spelled-out discriminant
/// (`typ: Some(BoTyp::X)`) or, worse, `typ: None`.
///
/// `typ: None` is the shape that reaches the wire: it serialises to a document
/// with **no** `_typ` at all, which every other BO4E implementation stamps. It
/// appears where a BO has no `Default` — `Lastgang` and `Tarif`, whose schemas
/// mark a field required — and the struct is spelled out field by field. Those
/// types have a `new()` that stamps the discriminant and is the
/// `..Default::default()` stand-in.
fn writes_discriminant_field(line: &str) -> bool {
    let Some(pos) = line.find("typ: ") else {
        return false;
    };
    // Only a field named exactly `typ`, not `profil_typ` / `kundentyp` / …
    let before = line[..pos].trim_end();
    if before.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
        return false;
    }
    let rest = &line[pos + "typ: ".len()..];
    if rest.starts_with("None") {
        return true;
    }
    let Some(rest) = rest.strip_prefix("Some(") else {
        return false;
    };
    let rest = rest.strip_prefix("rubo4e::current::").unwrap_or(rest);
    rest.starts_with("BoTyp::") || rest.starts_with("ComTyp::")
}

/// Does this line write a `_typ` **literal**?
///
/// Reading (`data.get("_typ")`) is fine, and so is writing a value that came
/// *off a type* — the gate's own `obj.insert("_typ".into(), expected.into())`
/// injects `T::TYP_WIRE`, which is the very thing this rule wants people to do.
/// Only a spelled-out discriminant is refused.
fn writes_typ(line: &str) -> bool {
    // `.insert("_typ".into(), "MARKTLOKATION".into())`
    if let Some(pos) = line.find(".insert(\"_typ\"") {
        let rest = &line[pos..];
        if let Some(comma) = rest.find(',')
            && rest[comma + 1..].trim_start().starts_with('"')
        {
            return true;
        }
        return false;
    }
    // `"_typ": "MARKTLOKATION"` inside a `json!` / map literal.
    let Some(pos) = line.find("\"_typ\"") else {
        return false;
    };
    let rest = line[pos + 6..].trim_start();
    rest.starts_with(':') && rest[1..].trim_start().starts_with('"')
}

/// Strip a trailing `//` comment, ignoring one inside a string literal.
fn strip_comment(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && bytes.get(i + 1) == Some(&b'/') => return line[..i].to_owned(),
            _ => {}
        }
        i += 1;
    }
    line.to_owned()
}

/// Was the line before `idx` a `#[cfg(test)]` attribute?
fn was_cfg_test(src: &str, idx: usize) -> bool {
    let before: Vec<&str> = src.lines().take(idx).collect();
    before
        .iter()
        .rev()
        .take(3)
        .any(|l| l.trim_start().starts_with("#[cfg(test)]"))
}
