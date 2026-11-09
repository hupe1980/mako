//! Guard: an inbound adapter reads a Prüfschritt or Transaktionsgrund code
//! from the place the MIG puts it, never from `STS` DE 4405.
//!
//! In a UTILMD `STS` the codes ride the third composite `C556` DE 9013 —
//! `STS+7++ZT6+ZW4` (Transaktionsgrund and Ergänzung), `STS+E01++A02:E_0615`
//! (Code des Prüfschritts and its Codeliste) — and the second composite `C555`
//! DE 4405 is empty. `edi_energy`'s typed `Sts` names DE 4405 `status_code`, so
//! reading it is the obvious thing to write and yields nothing on every
//! conformant message: a 55013's Transaktionsgrund came out empty, and every
//! GPKE Rückmeldung read as `Uebernommen` whatever its code. The accessors that
//! read `C556` are `UtilmdTransaction::transaktionsgrund()` and `antwort()`.
//!
//! An ORDRSP carries no `STS` at all — its Antwortcode is `SG2 AJT` — so a raw
//! `STS` read there finds nothing either.
//!
//! ## The two rules, over the adapter and ingest trees
//!
//! 1. **No `.status_code`.** DE 4405 holds no code an adapter needs.
//! 2. **No raw `STS` segment read outside [`RAW_STS_READERS`]** — the IFTSTA
//!    readers, whose MIG does put a code in `C555` (`STS+Z04+A03:E_0026`) or
//!    reads `C556` by position, each with its reason.

use std::path::Path;

/// The trees an inbound EDIFACT message is read in.
const TREES: &[&str] = &[
    "services/makod/src/orchestrator/adapters",
    "services/makod/src/orchestrator/ingest_dispatcher",
];

/// Functions admitted to read an `STS` segment raw, with why.
const RAW_STS_READERS: &[(&str, &str)] = &[
    (
        "iftsta_antwortcode",
        "IFTSTA `STS+E01++<code>:<Codeliste>` — reads C556 (element 2) by position",
    ),
    (
        "iftsta_datenstatus",
        "IFTSTA `STS+Z04+A03:E_0026` — the IFTSTA MIG's Datenstatus rides C555 itself",
    ),
];

pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    for tree in TREES {
        collect(&workspace_root.join(tree), &mut files);
    }
    if files.is_empty() {
        eprintln!("check-sts-codes: no source file under {TREES:?} — the trees moved");
        return false;
    }
    let mut findings = Vec::new();
    for file in &files {
        let Ok(src) = std::fs::read_to_string(file) else {
            findings.push(format!("{}: cannot read", file.display()));
            continue;
        };
        let rel = file.strip_prefix(workspace_root).unwrap_or(file);
        for f in scan(&src) {
            findings.push(format!("{}:{f}", rel.display()));
        }
    }
    if findings.is_empty() {
        println!(
            "check-sts-codes: {} adapter file(s) read STS codes from C556",
            files.len()
        );
        return true;
    }
    eprintln!(
        "check-sts-codes: {} read(s) of an STS code from the wrong place — use \
         `transaktionsgrund()` / `antwort()` (C556 DE 9013), or `ajt()` on an ORDRSP:",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `line: finding` for each violation in `src`.
fn scan(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current_fn = String::new();
    for (i, line) in src.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if let Some(name) = fn_name(code) {
            current_fn = name;
        }
        if code.contains(".status_code") {
            out.push(format!(
                "{}: reads `STS` DE 4405 (`status_code`), which a conformant UTILMD leaves empty",
                i + 1
            ));
        }
        if code.contains("\"STS\"") && !RAW_STS_READERS.iter().any(|(f, _)| *f == current_fn) {
            out.push(format!(
                "{}: raw `STS` read in `{current_fn}`, not an admitted reader",
                i + 1
            ));
        }
    }
    out
}

/// The name of a function declared on this line.
fn fn_name(line: &str) -> Option<String> {
    let at = line.find("fn ")?;
    if at > 0 && !line[..at].ends_with(char::is_whitespace) {
        return None;
    }
    let rest = &line[at + 3..];
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    (end > 0).then(|| rest[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::scan;

    #[test]
    fn a_de_4405_read_is_refused() {
        let src = "fn adapter() {\n    let g = sts.status_code.clone();\n}\n";
        assert_eq!(scan(src).len(), 1);
    }

    #[test]
    fn a_raw_sts_read_outside_the_readers_is_refused() {
        let src = "fn adapter() {\n    segs.iter().find(|s| s.tag == \"STS\");\n}\n";
        assert_eq!(scan(src).len(), 1);
        let ok = "fn iftsta_datenstatus() {\n    segs.iter().find(|s| s.tag == \"STS\");\n}\n";
        assert!(scan(ok).is_empty());
    }

    #[test]
    fn the_accessors_pass() {
        let src = "fn adapter() {\n    t.transaktionsgrund().map(|g| g.grund);\n    \
                   t.antwort();\n}\n";
        assert!(scan(src).is_empty());
    }
}
