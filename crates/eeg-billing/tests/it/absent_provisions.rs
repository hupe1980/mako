//! The § 101 Abs. 2 EEG 2023 provisions mako does not apply stay unapplied
//! until a `version` gate dates them.
//!
//! § 101 Abs. 2 Satz 1 reserves § 28c Abs. 1, § 39d, the listed parts of
//! § 39g, § 39h Abs. 3 Satz 1 and § 39i Abs. 2 Satz 1 und 2 and Abs. 2a until
//! the Commission's approval, and Satz 2 applies the Fassung of 24.02.2025
//! until then (Arbeitsausgabe 23.12.2025, p. 218). None is applied today. A
//! settlement path that names one — in code, not a comment — without a
//! `version` gate in the same file applies it undated, which is what this test
//! refuses.

use std::path::Path;

/// The absent provisions, as a settlement path would name them.
const ABWESEND: [&str; 6] = [
    "28c",
    "39d",
    "39g",
    "39h Abs. 3",
    "39i Abs. 2 ",
    "39i Abs. 2a",
];

/// What makes a file's mention dated: the § 101 Abs. 2 Fassung selector or an
/// approval constant.
const GATES: [&str; 2] = ["sect101_abs2_fassung", "GENEHMIGT_AB"];

fn verstoesse(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read source");
        if GATES.iter().any(|g| text.contains(g)) {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            let normal = code.replace("§ ", "§");
            if ABWESEND.iter().any(|p| normal.contains(&format!("§{p}"))) {
                out.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    out
}

#[test]
fn no_settlement_path_names_an_absent_provision_ungated() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = verstoesse(&manifest.join("src"));
    // The einsd service settles through this crate; in the workspace its
    // sources sit beside it.
    found.extend(verstoesse(&manifest.join("../../services/einsd/src")));
    assert!(
        found.is_empty(),
        "a § 101 Abs. 2 provision is named without a version gate:\n{}",
        found.join("\n")
    );
}

/// The scan sees a provision named in code and lets a comment pass.
#[test]
fn the_scan_finds_a_named_provision() {
    let dir = std::env::temp_dir().join(format!("absent-provisions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("a.rs"),
        "// § 39d is a comment\nlet basis = \"§ 39d EEG 2023\";\n",
    )
    .expect("write");
    let found = verstoesse(&dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(found.len(), 1, "{found:?}");
}
