//! Every BO4E fixture the runnable demos `PUT` must survive the gate.
//!
//! A BO4E payload with the wrong field names does not fail loudly: `decode`
//! tolerates unknown fields on the inbound path by design, so a misspelt
//! `marktlokations_id` lands in the extension bag and the typed
//! `marktlokationsId` reads back `None` — a fixture that stores nothing while
//! every request returns `201`.
//!
//! [`ensure_conformant`] is the outbound check, and the right one here: a demo
//! fixture is authored, not received, so mako should not ship an example of a
//! document it would refuse to send.

use std::path::{Path, PathBuf};

use mako_markt::bo4e::{Bo4eRejection, ensure_conformant, gate};

/// The repository's `demos/` directory.
fn demos_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../demos")
        .canonicalize()
        .expect("demos/ is part of the repository")
}

/// Every `.json` file under `demos/`, sorted for a deterministic failure order.
fn demo_json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            demo_json_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

/// What one fixture had to say for itself.
#[derive(Debug)]
enum Gated {
    /// Decoded as the BO4E type its `_typ` names and put through the gate.
    Checked,
    /// No BO4E discriminant at all — a service-native fixture (an `einsd`
    /// Anlage, a `marktd` partner), which the gate has nothing to say about.
    NotBo4e,
    /// A `_typ` no arm below covers, so nothing decoded the document and no
    /// field name in it was checked.
    Uncovered(String),
}

/// Decode `data` as the BO4E type its `_typ` names and run the outbound check.
fn check(data: &serde_json::Value) -> Result<Gated, Bo4eRejection> {
    macro_rules! gated {
        ($($typ:literal => $ty:ty),* $(,)?) => {
            match data.get("_typ").and_then(serde_json::Value::as_str) {
                $(Some($typ) => {
                    let v: $ty = gate::decode(data.clone())?;
                    ensure_conformant(&v)?;
                    Ok(Gated::Checked)
                })*
                Some(other) => Ok(Gated::Uncovered(other.to_owned())),
                None => Ok(Gated::NotBo4e),
            }
        };
    }
    gated! {
        "MARKTLOKATION"         => rubo4e::current::Marktlokation,
        "MESSLOKATION"          => rubo4e::current::Messlokation,
        "NETZLOKATION"          => rubo4e::current::Netzlokation,
        "GESCHAEFTSPARTNER"     => rubo4e::current::Geschaeftspartner,
        "PREISBLATTNETZNUTZUNG" => rubo4e::current::PreisblattNetznutzung,
        "RECHNUNG"              => rubo4e::current::Rechnung,
    }
}

/// Whether `data` is a BO4E document that failed to name its own type.
///
/// Three tells, all seen in the demo fixtures: a misspelt discriminant on the
/// object itself, a correctly spelt one on a nested object, and the
/// snake_case field names BO4E never uses on the wire.
fn looks_like_bo4e(data: &serde_json::Value) -> bool {
    let Some(obj) = data.as_object() else {
        return false;
    };
    if obj.contains_key("_typ") {
        // It named a type; `check` already decided the type is one we gate.
        return false;
    }
    obj.keys()
        .any(|k| k.ends_with("_typ") || k.eq_ignore_ascii_case("botyp"))
        || obj
            .values()
            .filter_map(serde_json::Value::as_object)
            .any(|nested| nested.keys().any(|k| k.ends_with("_typ")))
}

#[test]
fn every_demo_bo4e_fixture_is_conformant() {
    let root = demos_dir();
    let mut files = Vec::new();
    demo_json_files(&root, &mut files);

    let mut failures = Vec::new();
    let mut checked = 0_usize;
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let raw = std::fs::read_to_string(path).expect("fixture is readable");
        let json: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{rel}: not valid JSON: {e}"));
                continue;
            }
        };
        // The marktd wire shape wraps the document in `data`; a bare document
        // is the shape the service-native endpoints take.
        let data = json.get("data").unwrap_or(&json);
        match check(data) {
            // A BO4E document announces itself with `_typ`. A payload that
            // carries a *near-miss* discriminant — `bo_typ`, `boTyp`, a nested
            // `lokationsadresse._typ` under a typeless parent — is a BO4E
            // document whose spelling drifted, and skipping it is how
            // `eeg-billing/fixtures/malo.json` kept its ID silently dropped.
            Ok(Gated::NotBo4e) if looks_like_bo4e(data) => failures.push(format!(
                "{rel}: looks like a BO4E document but names no `_typ` \
                 (keys: {}) — `decode` would drop every typed field",
                data.as_object()
                    .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_default()
            )),
            Ok(Gated::NotBo4e) => {}
            // A `_typ` with no arm is the same silent pass as no `_typ` at all:
            // the document is never decoded, so no field name in it is checked
            // and no price it carries is looked at. A misspelt discriminant in
            // the arm list is enough to put a fixture there, which is why an
            // uncovered `_typ` fails rather than passing.
            Ok(Gated::Uncovered(typ)) => failures.push(format!(
                "{rel}: fixture names `_typ` {typ:?} which this gate does not cover \
                 — add an arm to `check`, or the document ships unvalidated"
            )),
            Ok(Gated::Checked) => checked += 1,
            Err(e) => failures.push(format!(
                "{rel}: {}",
                serde_json::to_string(&e.to_json()).unwrap_or_else(|_| "<unprintable>".to_owned())
            )),
        }
    }

    assert!(
        checked > 0,
        "no BO4E fixture was recognised under {} — every `_typ` stopped matching, \
         which is the silent failure this test exists to catch",
        root.display()
    );
    assert!(
        failures.is_empty(),
        "{} demo BO4E fixture(s) would be refused on the outbound path:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The gate must not answer "nothing to check" for a document that names a type.
///
/// An arm whose spelling drifts from BO4E's — `PREISBLATT_NETZNUTZUNG` for
/// `PREISBLATTNETZNUTZUNG` — takes its fixture out of the scan without changing
/// a single assertion, so the drift has to be a failure in its own right.
#[test]
fn a_typ_no_arm_covers_is_a_failure() {
    let uncovered = serde_json::json!({ "_typ": "PREISBLATT_NETZNUTZUNG" });
    assert!(
        matches!(check(&uncovered), Ok(Gated::Uncovered(t)) if t == "PREISBLATT_NETZNUTZUNG"),
        "a `_typ` outside the arm list must be reported, not skipped"
    );

    let covered = serde_json::json!({ "_typ": "PREISBLATTNETZNUTZUNG" });
    assert!(matches!(check(&covered), Ok(Gated::Checked)));

    let native = serde_json::json!({ "anlagen_schluessel": "EEG-1" });
    assert!(matches!(check(&native), Ok(Gated::NotBo4e)));
}
