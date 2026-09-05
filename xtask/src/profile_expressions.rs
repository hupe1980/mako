//! The Bedingung expressions in a committed profile read.
//!
//! `Status::parse` is what the validator runs on every AHB status and operand:
//! a string it refuses carries no condition at all, so the place is judged as
//! if it were unconditioned. The AHB tables are read off a PDF, where a column
//! break cuts an expression in half — `X [321]) ∨`, `X ([67] ∧ ([529] ∨` — and
//! the fragment left over is not the AHB's rule.
//!
//! The corruption is in the extraction, not in the AHB, so this does not
//! rewrite the profiles: it pins the fragments that are there in
//! [`ALLOWLIST_FILE`] with their occurrence counts, and refuses any that is
//! new or any that occurs more often than the file records. The count can only
//! shrink.
//!
//! Regenerate the file after an import that changes it:
//!
//! ```bash
//! BLESS_PROFILE_EXPRESSIONS=1 cargo xtask validate-profiles
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use edi_energy::profile::conditions::Status;

/// Where the known fragments are recorded, relative to the workspace root.
pub const ALLOWLIST_FILE: &str = "xtask/profile-expression-defects.json";

/// Per profile directory, the fragments it carries and how often.
pub type Ledger = BTreeMap<String, BTreeMap<String, usize>>;

/// Every `status` or `operand` string of `ahb` that cites a Bedingung and that
/// [`Status::parse`] refuses, with its number of occurrences.
#[must_use]
pub fn malformed(ahb: &Value) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut check = |text: &str| {
        if text.contains('[') && Status::parse(text).is_none() {
            *out.entry(text.to_owned()).or_default() += 1;
        }
    };
    fn walk(node: &Value, check: &mut impl FnMut(&str)) {
        match node {
            Value::Object(m) => {
                for (k, v) in m {
                    if matches!(k.as_str(), "status" | "operand") {
                        match v {
                            Value::String(s) => check(s),
                            Value::Array(a) => {
                                a.iter().filter_map(Value::as_str).for_each(&mut *check)
                            }
                            _ => {}
                        }
                    }
                    walk(v, check);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, check)),
            _ => {}
        }
    }
    if let Some(faelle) = ahb.get("anwendungsfaelle") {
        walk(faelle, &mut check);
    }
    if let Some(packages) = ahb.get("packages").and_then(Value::as_object) {
        for (id, expr) in packages {
            // A Paketvoraussetzung is an expression without a status word;
            // `X` is a column operand the reader took along.
            if let Some(expr) = expr.as_str()
                && !expr.trim().is_empty()
                && edi_energy::profile::conditions::Expr::parse(expr).is_err()
            {
                *out.entry(format!("[{id}] {expr}")).or_default() += 1;
            }
        }
    }
    out
}

/// The recorded fragments; empty when the file is absent.
#[must_use]
pub fn allowlist(root: &Path) -> Ledger {
    std::fs::read_to_string(root.join(ALLOWLIST_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<Allowlist>(&s).ok())
        .map(|a| a.profiles)
        .unwrap_or_default()
}

#[derive(serde::Deserialize, serde::Serialize)]
struct Allowlist {
    /// What the file is, for whoever opens it first.
    comment: Vec<String>,
    profiles: Ledger,
}

/// Hold `found` against the allowlist for one profile directory.
///
/// Returns one message per fragment that is new or more frequent than
/// recorded, and one note per fragment the profile no longer carries.
#[must_use]
pub fn compare(dir: &str, found: &BTreeMap<String, usize>, allowed: &Ledger) -> Vec<String> {
    let known = allowed.get(dir);
    let mut errors = Vec::new();
    for (expr, count) in found {
        match known.and_then(|k| k.get(expr)) {
            Some(&allowed) if *count <= allowed => {}
            Some(&allowed) => errors.push(format!(
                "{dir}: the status {expr:?} does not read and occurs {count} times, {allowed} recorded in {ALLOWLIST_FILE}"
            )),
            None => errors.push(format!(
                "{dir}: the status {expr:?} does not read — the Bedingung expression is cut off, so the place would be judged unconditioned"
            )),
        }
    }
    errors
}

/// Write the ledger, so the file records what the profiles carry today.
///
/// # Errors
///
/// When the file cannot be written.
pub fn bless(root: &Path, ledger: Ledger) -> Result<(), String> {
    let doc = Allowlist {
        comment: vec![
            "Bedingung expressions the profile import writes truncated: a PDF column break cut them in half.".into(),
            "The AHB rule they stand for is not recoverable from the extraction, so they are recorded here rather than guessed at.".into(),
            "Per profile directory: the status text as committed, and how often it occurs.".into(),
            "`cargo xtask validate-profiles` refuses any fragment that is new or more frequent than recorded.".into(),
            "Regenerate with BLESS_PROFILE_EXPRESSIONS=1 cargo xtask validate-profiles.".into(),
        ],
        profiles: ledger,
    };
    let json = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(root.join(ALLOWLIST_FILE), format!("{json}\n")).map_err(|e| e.to_string())
}
