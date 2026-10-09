//! No Gas-Sparte workflow enqueues an APERAK Anerkennungsmeldung.
//!
//! APERAK AHB 1.1 §2.3 (S. 21 f.): „In der Sparte Gas wird dem Absender eines
//! Geschäftsvorfalls vom Empfänger dieses Geschäftsvorfalls nur eines der beiden
//! Ergebnisse der mit diesem Geschäftsvorfall durchgeführten
//! Verarbeitbarkeitsprüfung via APERAK mitgeteilt: Das Ergebnis, dass der
//! geprüfte Geschäftsvorfall einen Verarbeitbarkeitsfehler enthält." A Gas
//! APERAK is therefore `BGM+313` (29001) only; 29002 `BGM+312` is Strom-only.
//!
//! The pin reads the source of every crate whose workflows are Gas-only —
//! `mako-geli-gas` and `mako-gabi-gas` — and refuses the constructor of the
//! Anerkennungsmeldung, its PID constant and a bare `29002` outside a comment.
//! A behavioural test per workflow covers only the paths it drives; this covers
//! the path somebody adds next.

use std::path::{Path, PathBuf};

/// What enqueues an APERAK 29002.
const FORBIDDEN: &[&str] = &["aperak_anerkennung", "APERAK_PID_ANERKENNUNG", "29002"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every non-comment line of `src` naming a forbidden token, as `file:line`.
fn offences_in(src_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(src_dir, &mut files);
    assert!(
        !files.is_empty(),
        "no source read under {}",
        src_dir.display()
    );
    let mut out = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("readable source");
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if FORBIDDEN.iter().any(|t| code.contains(t)) {
                out.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    out
}

fn gas_crate_sources() -> Vec<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![here.join("src"), here.join("../mako-gabi-gas/src")]
}

#[test]
fn no_gas_workflow_enqueues_an_anerkennungsmeldung() {
    let offences: Vec<String> = gas_crate_sources()
        .iter()
        .flat_map(|dir| offences_in(dir))
        .collect();
    assert!(
        offences.is_empty(),
        "a Gas-Sparte workflow enqueues APERAK 29002 (BGM+312), which APERAK AHB 1.1 \
         §2.3 does not allow:\n{}",
        offences.join("\n")
    );
}

/// The scan must see a planted offence, or its silence proves nothing.
#[test]
fn the_scan_refuses_a_planted_anerkennungsmeldung() {
    let dir = std::env::temp_dir().join(format!(
        "no-gas-anerkennung-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("planted.rs"),
        "// 29002 in a comment is fine\nlet o = PendingOutbox::aperak_anerkennung(a, b, c);\n",
    )
    .expect("write fixture");
    let offences = offences_in(&dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(offences.len(), 1, "{offences:?}");
    assert!(offences[0].contains("planted.rs:2"));
}
