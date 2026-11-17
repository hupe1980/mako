//! `cargo xtask profile-diff <type> <from> <to>` — what changed between two
//! Formatversionen of one message type's Prüfschablonen.
//!
//! The annual BDEW release ships a new AHB and a new MIG per message type. What
//! a reviewer needs from that is not the two files but the delta: which
//! Prüfidentifikatoren appeared or went away, and per Prüfidentifikator which
//! places changed their status, which codes gained or lost an operand, and
//! which Bedingungen were rewritten.
//!
//! It reads the committed profiles alone — no document mirror, no PDFs — so it
//! runs in the release PR that imported them.
//!
//! `--candidate` reads `<to>` from the candidates tree (`candidates/<type>/<to>`,
//! a Konsultationsfassung) instead of the binding profiles; `--json` prints the
//! Prüfidentifikatoren that changed, appeared and went away — the list
//! `import-profiles --candidate` records and the impact report re-validates.
//!
//! ```text
//! cargo xtask profile-diff utilmd fv20251001 fv20261001
//! cargo xtask profile-diff mscons fv20260401 fv20261001 --pid 13002
//! cargo xtask profile-diff utilmd fv20261001 fv20270401 --candidate --json
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

const PROFILES_DIR: &str = "crates/edi-energy/profiles";
const CANDIDATES_DIR: &str = crate::import_profiles::CANDIDATES_DIR;

#[derive(Debug, Deserialize)]
struct Ahb {
    message_type: String,
    ahb_version: String,
    anwendungsfaelle: Vec<Anwendungsfall>,
    #[serde(default)]
    conditions: BTreeMap<String, String>,
    #[serde(default)]
    packages: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct Anwendungsfall {
    pid: Option<u32>,
    name: String,
    #[serde(default)]
    rows: Vec<Row>,
    #[serde(default)]
    elements: Vec<ElementRule>,
}

#[derive(Debug, Deserialize)]
struct Row {
    nr: Option<String>,
    group: Option<String>,
    #[serde(default)]
    status: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ElementRule {
    nr: String,
    de: String,
    #[serde(default)]
    occurrence: u8,
    #[serde(default)]
    operands: Vec<Operand>,
}

#[derive(Debug, Deserialize)]
struct Operand {
    code: Option<String>,
    operand: String,
}

#[derive(Debug, Deserialize)]
struct Mig {
    #[serde(default)]
    structure: Vec<MigNode>,
    #[serde(default)]
    envelope: Vec<MigNode>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MigNode {
    Group {
        group: String,
        #[serde(default)]
        name: String,
        children: Vec<MigNode>,
    },
    Segment {
        nr: String,
        tag: String,
        #[serde(default)]
        name: String,
    },
}

/// The MIG renumbers its segments between Nachrichtentypversionen, so a place
/// is identified for comparison by where it sits and what it is called —
/// `SG4 Vorgangs-Identifikation / STS Transaktionsgrund` — not by its `Nr`.
/// A second place with the same identity inside the same group gets an index.
fn places(mig: &Mig) -> BTreeMap<String, String> {
    fn walk(
        nodes: &[MigNode],
        group: &str,
        out: &mut BTreeMap<String, String>,
        seen: &mut BTreeMap<String, u32>,
    ) {
        for n in nodes {
            match n {
                MigNode::Group {
                    group: g,
                    name,
                    children,
                } => {
                    let label = if name.is_empty() {
                        g.clone()
                    } else {
                        format!("{g} {name}")
                    };
                    walk(children, &label, out, seen);
                }
                MigNode::Segment { nr, tag, name } => {
                    let mut key = if group.is_empty() {
                        format!("{tag} {name}")
                    } else {
                        format!("{group} / {tag} {name}")
                    };
                    let n = seen.entry(key.clone()).or_default();
                    *n += 1;
                    if *n > 1 {
                        key = format!("{key} #{n}");
                    }
                    out.insert(nr.clone(), key);
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    let mut seen = BTreeMap::new();
    walk(&mig.structure, "", &mut out, &mut seen);
    walk(&mig.envelope, "", &mut out, &mut seen);
    out
}

/// Which Prüfidentifikatoren differ between two profiles of one message type.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PidDelta {
    /// A column whose statuses or operands differ, or that cites a Bedingung
    /// or Paket whose text differs.
    pub changed_pids: Vec<u32>,
    /// A column only the new profile has.
    pub added_pids: Vec<u32>,
    /// A column only the old profile has.
    pub removed_pids: Vec<u32>,
}

/// One profile directory (`mig.json`, `ahb.json`), read for comparison.
pub struct Loaded {
    ahb: Ahb,
    places: BTreeMap<String, String>,
}

/// Read the profile in `dir`.
///
/// # Errors
///
/// A file that cannot be read or does not parse.
pub fn load(dir: &Path) -> Result<Loaded, String> {
    let read = |file: &str| {
        std::fs::read_to_string(dir.join(file))
            .map_err(|e| format!("{}/{file}: {e}", dir.display()))
    };
    let ahb: Ahb = serde_json::from_str(&read("ahb.json")?)
        .map_err(|e| format!("{}/ahb.json: {e}", dir.display()))?;
    let mig: Mig = serde_json::from_str(&read("mig.json")?)
        .map_err(|e| format!("{}/mig.json: {e}", dir.display()))?;
    Ok(Loaded {
        places: places(&mig),
        ahb,
    })
}

fn by_pid(doc: &Loaded) -> BTreeMap<u32, (Fall, BTreeSet<String>)> {
    doc.ahb
        .anwendungsfaelle
        .iter()
        .filter_map(|a| a.pid.map(|p| (p, (Fall::of(a, &doc.places), cites(a)))))
        .collect()
}

/// Every Bedingung and Paket id a column cites.
fn cites(a: &Anwendungsfall) -> BTreeSet<String> {
    let exprs = a.rows.iter().flat_map(|r| r.status.iter()).chain(
        a.elements
            .iter()
            .flat_map(|e| e.operands.iter().map(|o| &o.operand)),
    );
    let mut out = BTreeSet::new();
    for expr in exprs {
        let mut rest = expr.as_str();
        while let Some(open) = rest.find('[') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find(']') else { break };
            let id = &rest[..close];
            // A Paket is cited with its occurrence (`1P0..1`), keyed by `1P`.
            let id = id.find('P').map_or(id, |p| &id[..=p]);
            out.insert(id.to_owned());
            rest = &rest[close + 1..];
        }
    }
    out
}

/// The Prüfidentifikatoren that differ from `old` to `new`.
#[must_use]
pub fn pid_delta(old: &Loaded, new: &Loaded) -> PidDelta {
    let (a, b) = (by_pid(old), by_pid(new));
    let rewritten: BTreeSet<&String> = [
        (&old.ahb.conditions, &new.ahb.conditions),
        (&old.ahb.packages, &new.ahb.packages),
    ]
    .into_iter()
    .flat_map(|(o, n)| {
        o.keys()
            .chain(n.keys())
            .filter(|id| o.get(*id) != n.get(*id))
            .collect::<Vec<_>>()
    })
    .collect();
    let mut delta = PidDelta::default();
    for pid in a.keys().chain(b.keys()).copied().collect::<BTreeSet<u32>>() {
        match (a.get(&pid), b.get(&pid)) {
            (None, Some(_)) => delta.added_pids.push(pid),
            (Some(_), None) => delta.removed_pids.push(pid),
            (Some((fa, ca)), Some((fb, cb))) => {
                if !fa.diff(fb).is_empty() || ca.union(cb).any(|id| rewritten.contains(id)) {
                    delta.changed_pids.push(pid);
                }
            }
            (None, None) => {}
        }
    }
    delta
}

/// Entry point.
pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let mut positional: Vec<&str> = Vec::new();
    let mut only_pid: Option<u32> = None;
    let mut candidate = false;
    let mut as_json = false;
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--pid" => only_pid = rest.next().and_then(|p| p.parse().ok()),
            "--candidate" => candidate = true,
            "--json" => as_json = true,
            other if other.starts_with("--") => {}
            other => positional.push(other),
        }
    }
    let [ty, from, to] = positional[..] else {
        eprintln!(
            "usage: cargo xtask profile-diff <type> <from-fv> <to-fv> [--candidate] [--json] [--pid <n>]"
        );
        return false;
    };

    let root = Path::new(workspace_root);
    let from_dir = root.join(PROFILES_DIR).join(ty).join(from);
    let to_dir = if candidate {
        root.join(CANDIDATES_DIR).join(ty).join(to)
    } else {
        root.join(PROFILES_DIR).join(ty).join(to)
    };
    let (old_doc, new_doc) = match (load(&from_dir), load(&to_dir)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error: {e}");
            return false;
        }
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&pid_delta(&old_doc, &new_doc)).unwrap_or_default()
        );
        return true;
    }
    let (old, old_places) = (&old_doc.ahb, &old_doc.places);
    let (new, new_places) = (&new_doc.ahb, &new_doc.places);

    println!(
        "{} {from} (AHB {}) → {to} (AHB {})",
        new.message_type, old.ahb_version, new.ahb_version
    );
    println!();

    let falls = |doc: &Ahb, places: &BTreeMap<String, String>| -> BTreeMap<u32, Fall> {
        doc.anwendungsfaelle
            .iter()
            .filter_map(|a| a.pid.map(|p| (p, Fall::of(a, places))))
            .collect()
    };
    let old_falls = falls(old, old_places);
    let new_falls = falls(new, new_places);

    let mut changed = 0usize;
    let pids: BTreeSet<u32> = old_falls
        .keys()
        .chain(new_falls.keys())
        .copied()
        .filter(|p| only_pid.is_none_or(|w| w == *p))
        .collect();
    for pid in pids {
        match (old_falls.get(&pid), new_falls.get(&pid)) {
            (None, Some(f)) => {
                println!("+ {pid}  {}  (new)", f.name);
                changed += 1;
            }
            (Some(f), None) => {
                println!("- {pid}  {}  (withdrawn)", f.name);
                changed += 1;
            }
            (Some(a), Some(b)) => {
                let lines = a.diff(b);
                if !lines.is_empty() {
                    println!("  {pid}  {}", b.name);
                    for l in &lines {
                        println!("      {l}");
                    }
                    println!();
                    changed += 1;
                }
            }
            (None, None) => {}
        }
    }

    if only_pid.is_none() {
        let mut lines = text_diff("Bedingung", &old.conditions, &new.conditions);
        lines.extend(text_diff("Paket", &old.packages, &new.packages));
        if !lines.is_empty() {
            println!("  Bedingungen und Pakete");
            for l in &lines {
                println!("      {l}");
            }
            println!();
            changed += lines.len();
        }
    }

    if changed == 0 {
        println!("no differences");
    }
    true
}

/// One column, indexed for comparison.
struct Fall {
    name: String,
    /// Place → the status the column states there.
    places: BTreeMap<String, Vec<String>>,
    /// (place, DE, occurrence, code) → operand.
    operands: BTreeMap<(String, String, u8, String), String>,
}

impl Fall {
    fn of(a: &Anwendungsfall, places: &BTreeMap<String, String>) -> Self {
        let name_of = |nr: &str| places.get(nr).cloned().unwrap_or_else(|| nr.to_owned());
        let mut rows = BTreeMap::new();
        for r in &a.rows {
            let key = match (&r.nr, &r.group) {
                (Some(nr), _) => name_of(nr),
                (None, Some(g)) => format!("{g} (Segmentgruppe)"),
                (None, None) => continue,
            };
            rows.insert(key, r.status.clone());
        }
        let mut operands = BTreeMap::new();
        for e in &a.elements {
            for op in &e.operands {
                operands.insert(
                    (
                        name_of(&e.nr),
                        e.de.clone(),
                        e.occurrence,
                        op.code.clone().unwrap_or_default(),
                    ),
                    op.operand.clone(),
                );
            }
        }
        Self {
            name: a.name.clone(),
            places: rows,
            operands,
        }
    }

    fn diff(&self, to: &Self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name != to.name {
            out.push(format!("name  {:?} → {:?}", self.name, to.name));
        }
        for key in self
            .places
            .keys()
            .chain(to.places.keys())
            .collect::<BTreeSet<_>>()
        {
            match (self.places.get(key), to.places.get(key)) {
                (Some(a), Some(b)) if a != b => {
                    out.push(format!("{key}  {} → {}", a.join(" | "), b.join(" | ")));
                }
                (None, Some(b)) => out.push(format!("+ {key}  {}", b.join(" | "))),
                (Some(a), None) => out.push(format!("- {key}  {}", a.join(" | "))),
                _ => {}
            }
        }
        for key in self
            .operands
            .keys()
            .chain(to.operands.keys())
            .collect::<BTreeSet<_>>()
        {
            let (place, de, occ, code) = key;
            let occ = if *occ == 0 {
                String::new()
            } else {
                format!("#{occ}")
            };
            let code = if code.is_empty() {
                String::new()
            } else {
                format!("+{code}")
            };
            let label = format!("{place} DE{de}{occ}{code}");
            match (self.operands.get(key), to.operands.get(key)) {
                (Some(a), Some(b)) if a != b => out.push(format!("{label}  {a} → {b}")),
                (None, Some(b)) => out.push(format!("+ {label}  {b}")),
                (Some(a), None) => out.push(format!("- {label}  {a}")),
                _ => {}
            }
        }
        out
    }
}

/// Added, removed and rewritten entries of a Bedingungen or Pakete table.
fn text_diff(
    kind: &str,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for id in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        match (old.get(id), new.get(id)) {
            (Some(a), Some(b)) if a != b => {
                out.push(format!("{kind} [{id}]  {a:?}"));
                out.push(format!(
                    "{:width$}→  {b:?}",
                    "",
                    width = kind.len() + id.len() + 5
                ));
            }
            (None, Some(b)) => out.push(format!("+ {kind} [{id}]  {b:?}")),
            (Some(a), None) => out.push(format!("- {kind} [{id}]  {a:?}")),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{load, pid_delta};
    use serde_json::Value;

    const DIR: &str = "../crates/edi-energy/profiles/utilmd/fv20261001";

    fn write(name: &str, ahb: &Value) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xtask-pid-delta-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::copy(format!("{DIR}/mig.json"), dir.join("mig.json")).expect("copy");
        std::fs::write(dir.join("ahb.json"), ahb.to_string()).expect("write");
        dir
    }

    fn binding() -> Value {
        serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/ahb.json")).expect("read"))
            .expect("parses")
    }

    /// The re-validation set: a column that changed, and every column citing a
    /// Bedingung whose text changed — the text is the rule.
    #[test]
    fn a_rewritten_bedingung_changes_every_column_citing_it() {
        let old = load(std::path::Path::new(DIR)).expect("loads");
        assert!(pid_delta(&old, &old).changed_pids.is_empty());

        let mut ahb = binding();
        ahb["conditions"]["10"] = Value::String("Wenn etwas ganz anderes gilt.".into());
        let new = load(&write("bedingung", &ahb)).expect("loads");
        let delta = pid_delta(&old, &new);
        assert!(
            delta.changed_pids.contains(&55001),
            "55001 cites [10]: {delta:?}"
        );

        let mut ahb = binding();
        ahb["anwendungsfaelle"]
            .as_array_mut()
            .expect("columns")
            .retain(|a| a["pid"] != 55002);
        let delta = pid_delta(&old, &load(&write("removed", &ahb)).expect("loads"));
        assert_eq!(delta.removed_pids, [55002]);
        assert!(delta.changed_pids.is_empty(), "{delta:?}");
    }
}
