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
//! be wrong. Three shapes are refused:
//!
//! ```ignore
//! // 1. Redundant: `..Default::default()` on the same literal already sets it.
//! Zaehlzeitdefinition { typ: Some(BoTyp::Zaehlzeitdefinition), ..Default::default() }
//!
//! // 2. Assembling a BO4E document as JSON, with a `_typ` key of any value.
//! serde_json::json!({ "_typ": "GESCHAEFTSPARTNER", "organisationsname": name })
//! serde_json::json!({ "_typ": Geschaeftspartner::TYP_WIRE, "organisationsname": name })
//!
//! // 3. The same document with no discriminant at all, which the rule above
//! //    cannot see: a BO4E field name written as a key is the tell.
//! serde_json::json!({ "marktlokationsId": malo, "zeitlicheGueltigkeit": z })
//! ```
//!
//! The second is the damaging one, and not because of the discriminant. A
//! document built as `json!` skips the typed constructor entirely, so nothing
//! checks its field names against the schema — `rubo4e` captures unknown keys
//! in `_additional` rather than rejecting them, so a misspelled field decodes
//! cleanly, reads back as `None`, and ships with the value missing. A decode
//! round-trip does not catch it either, for the same reason.
//!
//! That is why the `_typ` rule reads the **key**, not the value. A
//! `"_typ": T::TYP_WIRE` inside an object literal has a correct discriminant
//! and every one of the fields beside it is still unchecked, which is the whole
//! damage. And a hand-built document need not spell `_typ` at all: the third
//! rule flags an object literal carrying a BO4E field name, discriminant or no.
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
            "check-bo4e-discriminants: no BO4E document assembled by hand in shipped code \
             ({} documented exemption(s))",
            EXEMPT.len()
        );
        return true;
    }

    eprintln!(
        "ERROR: {} hand-assembled BO4E document(s) or discriminant(s):\n",
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

        // 2. `"_typ"` written as a key in an object literal, whatever the
        //    value, or inserted into a map with a spelled-out discriminant.
        //    Reading (`get("_typ")`, `json["_typ"]`) is untouched.
        if !typ_literal_exempt && writes_typ(&line) {
            out.push((
                i + 1,
                "`\"_typ\"` written as a key — a document assembled as a literal skips \
                 every field-name check, whatever the discriminant beside them says"
                    .to_owned(),
            ));
            continue;
        }

        // 3. A BO4E field name written as a key, with no discriminant needed.
        if let Some(field) = writes_a_bo4e_field(&line) {
            out.push((
                i + 1,
                format!(
                    "`\"{field}\"` written as a JSON key — that is a BO4E field, and a \
                     document built as a literal has no field-name check at all"
                ),
            ));
        }
    }
    out
}

/// BO4E field names no other vocabulary in this workspace uses.
///
/// camelCase is the tell. BO4E's wire spells a compound field
/// `zeitlicheGueltigkeit`; mako's own JSON — outbox payloads, REST projections,
/// MCP results — is snake_case throughout, and its database columns are
/// lower-case. So a camelCase key in an object literal is a BO4E document being
/// assembled by hand.
///
/// Single-word names are deliberately absent. `netzebene`, `sparte` and
/// `marktrolle` are BO4E fields *and* mako's own column names, and a rule over
/// those reports every projection this workspace serves — which is how a guard
/// gets turned off instead of fixed.
const BO4E_FIELDS: &[&str] = &[
    "marktlokationsId",
    "messlokationsId",
    "netzlokationsId",
    "zeitlicheGueltigkeit",
    "zusatzAttribute",
    "obisKennzahl",
    "netzebeneMessung",
    "messtechnischeEinordnung",
    "staffelgrenzeVon",
    "staffelgrenzeBis",
    "zeitvariablePreispositionen",
    "technischeRessourcen",
    "zaehlzeitDefinition",
    "zugehoerigeMesslokation",
    "bo4eVersion",
];

/// The BO4E field this line writes as a JSON key, if any.
///
/// A *key*, so `"obisKennzahl": value` counts and `.get("obisKennzahl")`,
/// `#[serde(rename = "obisKennzahl")]` and a `("obisKennzahl", …)` tuple do
/// not — reading a document, or naming the field on a typed struct, is exactly
/// the right thing to do with it.
fn writes_a_bo4e_field(line: &str) -> Option<&'static str> {
    BO4E_FIELDS.iter().copied().find(|field| {
        let needle = format!("\"{field}\"");
        line.find(&needle)
            .is_some_and(|at| line[at + needle.len()..].trim_start().starts_with(':'))
    })
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

/// Does this line write a `_typ` key?
///
/// Reading (`data.get("_typ")`, `json["_typ"]`) is fine. Two writes are not:
///
/// * `"_typ": <anything>` inside an object literal. The value does not matter:
///   `"_typ": T::TYP_WIRE` reads its discriminant off the type and the fields
///   beside it are still spelled by hand, unchecked.
/// * `.insert("_typ".into(), "MARKTLOKATION".into())` with a **literal** value.
///   The gate's own injection passes `T::TYP_WIRE` into an already-decoded
///   document rather than assembling one, which is the very thing this rule
///   wants people to do.
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
    // `"_typ": …` inside a `json!` / map literal.
    let Some(pos) = line.find("\"_typ\"") else {
        return false;
    };
    line[pos + 6..].trim_start().starts_with(':')
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

#[cfg(test)]
mod tests {
    use super::{offending_lines, writes_a_bo4e_field, writes_typ};

    /// The value is not the point. `T::TYP_WIRE` beside hand-spelled fields is
    /// a correct discriminant on a document nothing checks — the shape the
    /// module doc calls the damaging one.
    #[test]
    fn a_typ_key_is_a_finding_whatever_its_value() {
        assert!(writes_typ(r#"    "_typ": "GESCHAEFTSPARTNER","#));
        assert!(writes_typ(r#"    "_typ": Marktlokation::TYP_WIRE,"#));
        assert!(writes_typ(r#"    "_typ": typ_of(&value),"#));
    }

    /// Reading a discriminant off a received document is the right thing.
    #[test]
    fn reading_a_typ_is_not_writing_one() {
        assert!(!writes_typ(
            r#"    let t = data.get("_typ").and_then(Value::as_str);"#
        ));
        assert!(!writes_typ(
            r#"    assert_eq!(json["_typ"], "MARKTLOKATION");"#
        ));
        assert!(!writes_typ(
            r#"    obj.insert("_typ".into(), expected.into());"#
        ));
    }

    /// A hand-built BO4E document need not spell `_typ` at all — and without
    /// one, the `_typ` rule sees nothing while every field name goes unchecked.
    #[test]
    fn a_bo4e_field_key_is_a_finding_without_any_discriminant() {
        let src = r#"
    let doc = serde_json::json!({
        "marktlokationsId": malo,
        "zeitlicheGueltigkeit": { "startdatum": from },
    });
"#;
        let hits = offending_lines(src, false);
        assert_eq!(hits.len(), 2, "{hits:?}");
    }

    /// Naming the field on a typed struct, or reading it, is not assembling a
    /// document — and mako's own snake_case keys are not BO4E's.
    #[test]
    fn reads_renames_and_mako_keys_are_untouched() {
        assert!(writes_a_bo4e_field(r#"    .get("zusatzAttribute")"#).is_none());
        assert!(writes_a_bo4e_field(r#"    #[serde(default, rename = "obisKennzahl")]"#).is_none());
        assert!(writes_a_bo4e_field(r#"    ("netzebene", wires::<Netzebene>()),"#).is_none());
        assert!(writes_a_bo4e_field(r#"    "bilanzierungsmethode": methode,"#).is_none());
        assert!(writes_a_bo4e_field(r#"    "mabis_zaehlpunkt": data.zp,"#).is_none());
    }

    /// Prose showing the shapes the rule refuses is documentation.
    #[test]
    fn comments_are_stripped_first() {
        let src = "    // \"_typ\": \"MARKTLOKATION\" is what this refuses\n\
                   \x20   /// `\"marktlokationsId\": id` is the shape to avoid\n";
        assert!(offending_lines(src, false).is_empty());
    }
}
