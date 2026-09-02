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
        // A profile records a mandatory segment at **either** level: flat in
        // `segment_rules`, or scoped to a group in `group_rules`. A grouped
        // segment is `O` in the flat list precisely because the group carries
        // the requirement, so reading only `segment_rules` misses it — and would
        // score a draft that moved its marks into `group_rules` as "no over-
        // constraining" while never looking at the marks at all.
        let mut set = BTreeSet::new();
        for list in ["segment_rules", "group_rules"] {
            for r in e[list].as_array().into_iter().flatten() {
                if r["requirement"].as_str() == Some("M")
                    && let Some(tag) = r["tag"].as_str()
                {
                    set.insert(tag.to_owned());
                }
            }
        }
        out.insert(code as u32, set);
    }
    Some(out)
}

/// Excess-mandatory-segment count under which a PID is worth a reviewer's time.
///
/// A superset PID cannot ship — it would reject valid messages. But "one extra
/// segment marked M" is a minutes-long check against the AHB, while "thirty" is
/// a re-read of the whole table. Ranking by excess turns 102 undifferentiated
/// PIDs into a worklist with a cheap end.
const NEAR_MISS_EXCESS: usize = 2;

#[derive(Default)]
struct Tally {
    exact: usize,
    superset: Vec<(u32, usize)>,
    /// How often each segment tag is over-marked, across all PIDs.
    excess_by_tag: std::collections::BTreeMap<String, usize>,
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
                    // How *far* off, not just that it is off: the excess is the
                    // number of segments a reviewer must judge before this PID
                    // can ship.
                    let extra: Vec<&String> = drf_set.difference(cur_set).collect();
                    for tag in &extra {
                        *t.excess_by_tag.entry((*tag).clone()).or_default() += 1;
                    }
                    t.superset.push((*code, extra.len()));
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
                t.superset.sort_by_key(|&(code, excess)| (excess, code));
                let total: usize = t.superset.iter().map(|&(_, e)| e).sum();
                let near: Vec<String> = t
                    .superset
                    .iter()
                    .take_while(|&&(_, e)| e <= NEAR_MISS_EXCESS)
                    .map(|&(code, e)| format!("{code} (+{e})"))
                    .collect();
                let worst: Vec<String> = t
                    .superset
                    .iter()
                    .rev()
                    .take(5)
                    .map(|&(code, e)| format!("{code} (+{e})"))
                    .collect();
                println!(
                    "    superset: {} PIDs, {total} excess mandatory segments in total \
                     (median +{})",
                    t.superset.len(),
                    t.superset[t.superset.len() / 2].1,
                );
                println!(
                    "      review first (≤{NEAR_MISS_EXCESS} excess, {} PIDs): {}",
                    near.len(),
                    if near.is_empty() {
                        "<none>".to_owned()
                    } else {
                        near.join(", ")
                    },
                );
                println!("      worst: {}", worst.join(", "));

                // Which *segments* the draft over-marks, not which PIDs.
                //
                // The excess is not one judgement per PID: the same handful of
                // tags recur, because a tag appearing in several segment groups
                // collapses ambiguously and a `Muss` nested in a conditioned
                // group is kept as `M`. Ranking by tag turns a pile of per-PID
                // findings back into the few extractor behaviours behind them —
                // fix one and it clears across every PID at once.
                let mut by_tag: Vec<(&str, usize)> = t
                    .excess_by_tag
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .collect();
                by_tag.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                let top: Vec<String> = by_tag
                    .iter()
                    .take(5)
                    .map(|(tag, n)| format!("{tag} {n} ({}%)", n * 100 / total.max(1)))
                    .collect();
                let covered: usize = by_tag.iter().take(5).map(|(_, n)| n).sum();
                println!(
                    "      by segment ({} distinct tags): {}  -> top 5 = {}% of the excess",
                    by_tag.len(),
                    top.join(", "),
                    covered * 100 / total.max(1),
                );
            }
            // A subset or a `differs` is the quiet verdict — the draft marks
            // *fewer* segments mandatory than the AHB, so promoting it accepts
            // messages the AHB refuses. Naming them is the whole point: a count
            // says a PID needs review without saying which one.
            if !t.subset.is_empty() {
                t.subset.sort_unstable();
                println!(
                    "    subset ({} — the draft under-marks; promoting accepts invalid \
                     messages): {}",
                    plural(t.subset.len()),
                    codes(&t.subset),
                );
            }
            if !t.differs.is_empty() {
                t.differs.sort_unstable();
                println!(
                    "    differs ({} — over- and under-marked at once): {}",
                    plural(t.differs.len()),
                    codes(&t.differs),
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

/// „1 PID" / „3 PIDs".
fn plural(n: usize) -> String {
    if n == 1 {
        "1 PID".to_owned()
    } else {
        format!("{n} PIDs")
    }
}

/// A PID list as a comma-separated string.
fn codes(pids: &[u32]) -> String {
    pids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
