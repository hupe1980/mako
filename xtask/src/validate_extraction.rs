//! Measure how well `extract-pdf` reproduces the hand-curated profiles.
//!
//! # Why this exists
//!
//! The AHB backlog is gated on curation, not tooling: 79 routed
//! Prüfidentifikatoren have no AHB rules because promoting an extraction draft
//! unreviewed would change how real messages validate. That gate is easy to
//! state and easy to hand-wave past — "the drafts look fine" is a judgement
//! nobody can check.
//!
//! This turns it into a number. For every profile that has both a curated
//! `ahb.json` and a generated `ahb.draft.json`, it compares the **mandatory
//! segment set** per Prüfidentifikator and classifies the result:
//!
//! | Verdict | Meaning | Consequence of shipping |
//! |---|---|---|
//! | `exact` | draft == curated | safe |
//! | `superset` | draft marks *more* segments `M` | **rejects valid messages** |
//! | `subset` | draft marks *fewer* | accepts invalid ones (same as today's vacuous state) |
//! | `differs` | neither contains the other | both failure modes at once |
//!
//! A `superset` is the dangerous verdict, and it is the common one today. Until
//! this reports predominantly `exact`, the extraction is not ready to author
//! profiles for PIDs nobody has reviewed — an approach that cannot re-derive the
//! profiles a human already checked has not earned the right to write new ones.
//!
//! # Usage
//!
//! ```bash
//! cargo xtask extract-pdf --file <AHB.pdf> --message-type utilmd --release FV2026-10-01
//! cargo xtask validate-extraction
//! ```
//!
//! Drafts are gitignored build artefacts, so with none present this reports
//! nothing and exits 0 — it never fails a build for lack of input.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn profiles_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/edi-energy/profiles"
    ))
}

/// `code -> mandatory segment tags` from one `ahb*.json`.
fn mandatory_sets(path: &Path) -> Option<BTreeMap<u32, BTreeSet<String>>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let mut out = BTreeMap::new();
    for e in v["pruefidentifikatoren"].as_array()? {
        let Some(code) = e["code"].as_u64() else {
            continue;
        };
        let mut set = BTreeSet::new();
        for r in e["segment_rules"].as_array().into_iter().flatten() {
            if r["requirement"].as_str() == Some("M")
                && let Some(tag) = r["tag"].as_str()
            {
                set.insert(tag.to_owned());
            }
        }
        out.insert(code as u32, set);
    }
    Some(out)
}

#[derive(Default)]
struct Tally {
    exact: usize,
    superset: Vec<u32>,
    subset: Vec<u32>,
    differs: Vec<u32>,
}

pub fn validate_extraction() {
    let root = profiles_root();
    let mut any = false;
    let mut worst_ok = true;

    let Ok(entries) = std::fs::read_dir(&root) else {
        println!(
            "validate-extraction: no profiles directory at {}",
            root.display()
        );
        return;
    };
    for ty in entries.flatten() {
        if !ty.path().is_dir() {
            continue;
        }
        let Ok(rels) = std::fs::read_dir(ty.path()) else {
            continue;
        };
        for rel in rels.flatten() {
            let dir = rel.path();
            let (curated, draft) = (dir.join("ahb.json"), dir.join("ahb.draft.json"));
            if !draft.exists() {
                continue;
            }
            let (Some(cur), Some(drf)) = (mandatory_sets(&curated), mandatory_sets(&draft)) else {
                continue;
            };
            any = true;

            let mut t = Tally::default();
            for (code, cur_set) in &cur {
                let Some(drf_set) = drf.get(code) else {
                    continue;
                };
                if drf_set == cur_set {
                    t.exact += 1;
                } else if cur_set.is_subset(drf_set) {
                    t.superset.push(*code);
                } else if drf_set.is_subset(cur_set) {
                    t.subset.push(*code);
                } else {
                    t.differs.push(*code);
                }
            }
            let compared = t.exact + t.superset.len() + t.subset.len() + t.differs.len();
            if compared == 0 {
                continue;
            }
            let pct = t.exact * 100 / compared;
            let label = format!(
                "{}/{}",
                ty.file_name().to_string_lossy(),
                rel.file_name().to_string_lossy()
            );
            println!(
                "{label:<28} exact {:>3}/{compared} ({pct:>3}%)  superset {:>3}  subset {:>3}  differs {:>3}",
                t.exact,
                t.superset.len(),
                t.subset.len(),
                t.differs.len()
            );
            if !t.superset.is_empty() {
                worst_ok = false;
                let sample: Vec<String> = t.superset.iter().take(8).map(u32::to_string).collect();
                println!(
                    "    superset PIDs (would reject valid messages): {}{}",
                    sample.join(", "),
                    if t.superset.len() > 8 { ", …" } else { "" }
                );
            }
        }
    }

    if !any {
        println!(
            "validate-extraction: no ahb.draft.json found — run `cargo xtask extract-pdf` first."
        );
        return;
    }
    if worst_ok {
        println!("\nvalidate-extraction: no superset verdicts — drafts do not over-constrain.");
    } else {
        println!(
            "\nvalidate-extraction: superset verdicts present. Promoting these drafts would \
             reject valid messages; the extraction is not ready to author unreviewed PIDs."
        );
    }
}
