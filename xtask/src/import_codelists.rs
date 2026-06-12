//! `cargo xtask import-codelists`
//!
//! Imports code values from a CSV file and merges them into the
//! `codelists.json` for the specified message type and release.
//!
//! # CSV format
//!
//! The CSV file must have three columns — the header row is required:
//!
//! ```csv
//! DE_ID,Code,Description
//! 1001,380,Invoice
//! 1001,381,Credit note
//! 3035,MS,Message sender
//! ```
//!
//! # Usage
//!
//! ```text
//! cargo xtask import-codelists \
//!   --file path/to/codes.csv \
//!   --message-type INVOIC \
//!   --release 2.8e \
//!   [--dry-run]
//! ```
//!
//! With `--dry-run`, the task prints the proposed diff without writing any files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── JSON model ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
struct Codelists {
    release: String,
    lists: BTreeMap<String, Vec<String>>,
}

// ── Public entry-point ────────────────────────────────────────────────────────

pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let file = match find_arg(args, "--file") {
        Some(v) => v,
        None => {
            eprintln!("import-codelists: --file <path> is required");
            return false;
        }
    };
    let message_type = match find_arg(args, "--message-type") {
        Some(v) => v.to_lowercase(),
        None => {
            eprintln!("import-codelists: --message-type <TYPE> is required");
            return false;
        }
    };
    let release = match find_arg(args, "--release") {
        Some(v) => v,
        None => {
            eprintln!("import-codelists: --release <RELEASE> is required");
            return false;
        }
    };
    let dry_run = args.iter().any(|a| a == "--dry-run");

    // Load CSV
    let csv_path = PathBuf::from(&file);
    let incoming = match parse_csv(&csv_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "import-codelists: cannot read CSV {}: {e}",
                csv_path.display()
            );
            return false;
        }
    };

    if incoming.is_empty() {
        eprintln!("import-codelists: CSV contained no data rows");
        return false;
    }

    // Load existing codelists.json (or create an empty one)
    let target_path = PathBuf::from(format!(
        "{workspace_root}/crates/edi-energy/profiles/{message_type}/{release}/codelists.json"
    ));
    let mut existing: Codelists = if target_path.exists() {
        let content = std::fs::read_to_string(&target_path).map_err(|e| {
            eprintln!(
                "import-codelists: cannot read {}: {e}",
                target_path.display()
            );
        });
        match content {
            Ok(c) => match serde_json::from_str(&c) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "import-codelists: cannot parse {}: {e}",
                        target_path.display()
                    );
                    return false;
                }
            },
            Err(_) => return false,
        }
    } else {
        Codelists {
            release: release.clone(),
            lists: BTreeMap::new(),
        }
    };

    // Merge: add new codes; preserve existing ones
    let mut added: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (de_id, codes) in &incoming {
        let existing_set: BTreeSet<&str> = existing
            .lists
            .get(de_id)
            .map_or_else(BTreeSet::new, |v| v.iter().map(String::as_str).collect());
        let new_codes: Vec<String> = codes
            .iter()
            .filter(|c| !existing_set.contains(c.as_str()))
            .cloned()
            .collect();
        if !new_codes.is_empty() {
            added.insert(de_id.clone(), new_codes.clone());
        }
        let entry = existing.lists.entry(de_id.clone()).or_default();
        for code in &new_codes {
            entry.push(code.clone());
        }
        // Deduplicate and sort for stable output
        let set: BTreeSet<String> = entry.drain(..).collect();
        entry.extend(set);
    }

    // Report diff
    if added.is_empty() {
        println!("import-codelists: no new codes to add (all already present)");
        return true;
    }

    for (de_id, codes) in &added {
        println!("  + DE {de_id}: {}", codes.join(", "));
    }

    if dry_run {
        println!(
            "import-codelists: dry-run — {} DE(s) would be updated in {}",
            added.len(),
            target_path.display()
        );
        return true;
    }

    // Ensure directory exists
    if let Some(parent) = target_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("import-codelists: cannot create directory: {e}");
        return false;
    }

    let json = match serde_json::to_string_pretty(&existing) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("import-codelists: cannot serialise codelists: {e}");
            return false;
        }
    };
    if let Err(e) = std::fs::write(&target_path, format!("{json}\n")) {
        eprintln!(
            "import-codelists: cannot write {}: {e}",
            target_path.display()
        );
        return false;
    }

    println!(
        "import-codelists: updated {} — {} DE(s) modified",
        target_path.display(),
        added.len()
    );
    true
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse the CSV and return a map of DE_ID → sorted list of Code values.
///
/// Expected header: `DE_ID,Code,Description`
fn parse_csv(path: &PathBuf) -> Result<BTreeMap<String, Vec<String>>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut result: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (line_no, line) in content.lines().enumerate() {
        if line_no == 0 {
            // Validate header
            let header: Vec<&str> = line.split(',').map(str::trim).collect();
            if header.len() < 2
                || header[0].to_uppercase() != "DE_ID"
                || header[1].to_uppercase() != "CODE"
            {
                return Err(format!(
                    "unexpected CSV header: expected 'DE_ID,Code,...' got '{line}'"
                ));
            }
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.splitn(3, ',').collect();
        if cols.len() < 2 {
            return Err(format!("line {}: expected at least 2 columns", line_no + 1));
        }
        let de_id = cols[0].trim().to_string();
        let code = cols[1].trim().to_string();
        if de_id.is_empty() || code.is_empty() {
            continue;
        }
        result.entry(de_id).or_default().insert(code);
    }

    Ok(result
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect())
}

fn find_arg(args: &[String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}
