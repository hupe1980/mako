//! `cargo xtask release-diff` — compare two profile releases side-by-side.
//!
//! Usage:
//!   cargo xtask release-diff --message-type UTILMD --from fv20251001 --to fv20261001
//!   cargo xtask release-diff --message-type UTILMD --from 5.5.3a --to 5.5.4a
//!
//! The `--from` and `--to` arguments accept either profile folder names
//! (e.g. `fv20251001`) or wire release codes (e.g. `5.5.3a`).  When a wire
//! release code is given, the most-recently-published folder whose `mig.json`
//! carries that release code is selected automatically.  If the wire code is
//! ambiguous (two folders share the same release code), a warning is printed and
//! the latest one by `valid_from` date is used.
//!
//! Output: Markdown diff of MIG segments, AHB Pruefidentifikatoren, and codelists.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// Resolve a `--from` / `--to` argument to a profile folder name.
///
/// Accepts either a folder name (`fv20251001`) or a wire release code
/// (`5.5.3a`).  For wire codes, scans the type directory for `mig.json` files
/// whose `"release"` field matches the code and returns the folder with the
/// latest `"valid_from"` date (or lexicographically last folder name as
/// tie-breaker).  Returns `None` when no match is found.
fn resolve_release_arg(profiles_dir: &str, msg_type: &str, arg: &str) -> Option<String> {
    let type_dir = format!("{profiles_dir}/{msg_type}");

    // Fast path: if the argument is already a folder name, validate it exists.
    let direct = format!("{type_dir}/{arg}");
    if std::path::Path::new(&direct).is_dir() {
        return Some(arg.to_owned());
    }

    // Slow path: treat `arg` as a wire release code and search for a matching folder.
    let rd = std::fs::read_dir(&type_dir).ok()?;
    let mut candidates: Vec<(String, String)> = Vec::new(); // (valid_from_or_folder, folder_name)

    for entry in rd.filter_map(std::result::Result::ok) {
        let folder = entry.file_name().to_string_lossy().into_owned();
        let mig_path = format!("{type_dir}/{folder}/mig.json");
        if let Ok(src) = std::fs::read_to_string(&mig_path)
            && let Ok(val) = serde_json::from_str::<Value>(&src)
        {
            let release = val.get("release").and_then(|v| v.as_str()).unwrap_or("");
            if release == arg {
                let valid_from = val
                    .get("valid_from")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&folder)
                    .to_owned();
                candidates.push((valid_from, folder));
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }
    if candidates.len() > 1 {
        eprintln!(
            "warning: release code {arg:?} matches multiple folders for {}: {}",
            msg_type.to_uppercase(),
            candidates
                .iter()
                .map(|(_, f)| f.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        eprintln!("         Using the latest folder by valid_from date.");
    }

    // Sort by (valid_from, folder) descending and take the last one.
    candidates.sort();
    candidates.into_iter().next_back().map(|(_, folder)| folder)
}

/// Run `release-diff`.  Returns `true` when no differences were found.
pub(crate) fn run(workspace_root: &str, args: &[String]) -> bool {
    let profiles_dir = format!("{workspace_root}/crates/edi-energy/profiles");

    // Parse CLI flags.
    let mut message_type: Option<String> = None;
    let mut from_release: Option<String> = None;
    let mut to_release: Option<String> = None;
    let mut output_file: Option<String> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--message-type" => {
                message_type = iter.next().map(|s| s.to_lowercase());
            }
            "--from" => {
                from_release = iter.next().cloned();
            }
            "--to" => {
                to_release = iter.next().cloned();
            }
            "--output-file" => {
                output_file = iter.next().cloned();
            }
            other => {
                eprintln!("error: unknown argument `{other}`");
                eprintln!();
                eprintln!(
                    "Usage: cargo xtask release-diff --message-type <TYPE> --from <FOLDER|RELEASE> --to <FOLDER|RELEASE>"
                );
                eprintln!("Examples:");
                eprintln!(
                    "  cargo xtask release-diff --message-type mscons --from fv20240401 --to fv20251001"
                );
                eprintln!("  cargo xtask release-diff --message-type utilmd --from S2.0 --to S2.1");
                return false;
            }
        }
    }

    let (Some(msg_type), Some(from_arg), Some(to_arg)) = (message_type, from_release, to_release)
    else {
        eprintln!(
            "error: --message-type, --from, and --to are all required\n\
             Usage: cargo xtask release-diff --message-type <TYPE> --from <FOLDER|RELEASE> --to <FOLDER|RELEASE>\n\
             Examples:\n\
             \x20 cargo xtask release-diff --message-type mscons --from fv20240401 --to fv20251001\n\
             \x20 cargo xtask release-diff --message-type utilmd --from S2.0 --to S2.1"
        );
        return false;
    };

    // Resolve --from / --to to folder names (accept both folder names and wire codes).
    let Some(from) = resolve_release_arg(&profiles_dir, &msg_type, &from_arg) else {
        eprintln!(
            "error: could not resolve `{from_arg}` as a folder name or wire release code for message type `{}`.",
            msg_type.to_uppercase()
        );
        eprintln!("       Check that the profile directory or mig.json release field matches.");
        return false;
    };
    let Some(to) = resolve_release_arg(&profiles_dir, &msg_type, &to_arg) else {
        eprintln!(
            "error: could not resolve `{to_arg}` as a folder name or wire release code for message type `{}`.",
            msg_type.to_uppercase()
        );
        eprintln!("       Check that the profile directory or mig.json release field matches.");
        return false;
    };

    let from_dir = format!("{profiles_dir}/{msg_type}/{from}");
    let to_dir = format!("{profiles_dir}/{msg_type}/{to}");

    let display_type = msg_type.to_uppercase();
    let mut report = String::new();

    // Use the original argument in the report header for readability, but note
    // the resolved folder when it differs.
    let from_label = if from_arg == from {
        from_arg.clone()
    } else {
        format!("{from_arg} ({from})")
    };
    let to_label = if to_arg == to {
        to_arg.clone()
    } else {
        format!("{to_arg} ({to})")
    };
    report.push_str(&format!(
        "# Release diff: {display_type} {from_label} → {to_label}\n\n"
    ));

    let mig_changed = diff_mig_into(&from_dir, &to_dir, &from_label, &to_label, &mut report);
    report.push('\n');
    let ahb_changed = diff_ahb_into(&from_dir, &to_dir, &from_label, &to_label, &mut report);
    report.push('\n');
    let codelists_changed =
        diff_codelists_into(&from_dir, &to_dir, &from_label, &to_label, &mut report);

    let any_changed = mig_changed || ahb_changed || codelists_changed;
    if !any_changed {
        report.push('\n');
        report.push_str(&format!(
            "*No differences found between {from_label} and {to_label}.*\n"
        ));
    }

    // Write or print the report.
    if let Some(path) = output_file {
        std::fs::write(&path, &report)
            .unwrap_or_else(|e| eprintln!("error: cannot write output file '{path}': {e}"));
        println!("release-diff report written to {path}");
    } else {
        print!("{report}");
    }

    // Exit code 0 = no differences; exit code 1 = differences found (signal in CI).
    !any_changed
}

// ── MIG diff ──────────────────────────────────────────────────────────────────

fn diff_mig_into(from_dir: &str, to_dir: &str, from: &str, to: &str, out: &mut String) -> bool {
    let from_path = format!("{from_dir}/mig.json");
    let to_path = format!("{to_dir}/mig.json");

    let from_mig = match load_json(&from_path) {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!("## MIG\n\n> ⚠ cannot load {from_path}: {e}\n\n"));
            return false;
        }
    };
    let to_mig = match load_json(&to_path) {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!("## MIG\n\n> ⚠ cannot load {to_path}: {e}\n\n"));
            return false;
        }
    };

    let from_segs = collect_segment_tags(&from_mig);
    let to_segs = collect_segment_tags(&to_mig);

    let mut added: Vec<_> = to_segs.difference(&from_segs).cloned().collect();
    let mut removed: Vec<_> = from_segs.difference(&to_segs).cloned().collect();
    added.sort();
    removed.sort();

    // Detect max_occurrences changes.
    let from_card = collect_segment_max_occ(&from_mig);
    let to_card = collect_segment_max_occ(&to_mig);
    let mut card_changes: Vec<String> = Vec::new();
    for (tag, from_max) in &from_card {
        if let Some(to_max) = to_card.get(tag)
            && from_max != to_max
        {
            card_changes.push(format!(
                "  - `{tag}`: max_occurrences {from_max} → {to_max}"
            ));
        }
    }

    // Detect mandatory/conditional status changes.
    let from_status = collect_segment_status(&from_mig);
    let to_status = collect_segment_status(&to_mig);
    let mut status_changes: Vec<String> = Vec::new();
    for (tag, from_mand) in &from_status {
        if let Some(to_mand) = to_status.get(tag)
            && from_mand != to_mand
        {
            let f = if *from_mand { "M" } else { "C" };
            let t = if *to_mand { "M" } else { "C" };
            status_changes.push(format!("  - `{tag}`: {f} → {t}"));
        }
    }

    let changed = !added.is_empty()
        || !removed.is_empty()
        || !card_changes.is_empty()
        || !status_changes.is_empty();
    out.push_str(&format!("## MIG — Segment changes ({from} → {to})\n\n"));

    if !changed {
        out.push_str("No MIG segment changes.\n");
    } else {
        if !added.is_empty() {
            out.push_str("### Added segments\n\n");
            for tag in &added {
                out.push_str(&format!("- `{tag}`\n"));
            }
            out.push('\n');
        }
        if !removed.is_empty() {
            out.push_str("### Removed segments\n\n");
            for tag in &removed {
                out.push_str(&format!("- `{tag}`\n"));
            }
            out.push('\n');
        }
        if !status_changes.is_empty() {
            out.push_str("### Mandatory/conditional status changes\n\n");
            for change in &status_changes {
                out.push_str(&format!("{change}\n"));
            }
            out.push('\n');
        }
        if !card_changes.is_empty() {
            out.push_str("### Cardinality changes\n\n");
            for change in &card_changes {
                out.push_str(&format!("{change}\n"));
            }
        }
    }

    changed
}

fn collect_segment_tags(mig: &Value) -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    if let Some(segs) = mig["segments"].as_array() {
        for seg in segs {
            if let Some(tag) = seg["tag"].as_str() {
                tags.insert(tag.to_owned());
            }
        }
    }
    // Walk groups recursively.
    if let Some(groups) = mig["segment_groups"].as_array() {
        for group in groups {
            collect_group_tags(group, &mut tags);
        }
    }
    tags
}

fn collect_group_tags(group: &Value, tags: &mut BTreeSet<String>) {
    if let Some(segs) = group["segments"].as_array() {
        for seg in segs {
            if let Some(tag) = seg["tag"].as_str() {
                tags.insert(tag.to_owned());
            }
        }
    }
    if let Some(nested) = group["groups"].as_array() {
        for g in nested {
            collect_group_tags(g, tags);
        }
    }
}

fn collect_segment_max_occ(mig: &Value) -> BTreeMap<String, u64> {
    let mut map = BTreeMap::new();
    if let Some(segs) = mig["segments"].as_array() {
        for seg in segs {
            if let (Some(tag), Some(max)) = (seg["tag"].as_str(), seg["max_occurrences"].as_u64()) {
                map.insert(tag.to_owned(), max);
            }
        }
    }
    if let Some(groups) = mig["segment_groups"].as_array() {
        for group in groups {
            collect_group_max_occ(group, &mut map);
        }
    }
    map
}

fn collect_group_max_occ(group: &Value, map: &mut BTreeMap<String, u64>) {
    if let Some(segs) = group["segments"].as_array() {
        for seg in segs {
            if let (Some(tag), Some(max)) = (seg["tag"].as_str(), seg["max_occurrences"].as_u64()) {
                map.entry(tag.to_owned()).or_insert(max);
            }
        }
    }
    if let Some(nested) = group["groups"].as_array() {
        for g in nested {
            collect_group_max_occ(g, map);
        }
    }
}

fn collect_segment_status(mig: &Value) -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    let add = |map: &mut BTreeMap<String, bool>, seg: &Value| {
        if let Some(tag) = seg["tag"].as_str() {
            let mandatory = seg["mandatory"].as_bool().unwrap_or(false);
            map.entry(tag.to_owned()).or_insert(mandatory);
        }
    };
    if let Some(segs) = mig["segments"].as_array() {
        for seg in segs {
            add(&mut map, seg);
        }
    }
    if let Some(groups) = mig["segment_groups"].as_array() {
        for group in groups {
            collect_group_status(group, &mut map);
        }
    }
    map
}

fn collect_group_status(group: &Value, map: &mut BTreeMap<String, bool>) {
    if let Some(segs) = group["segments"].as_array() {
        for seg in segs {
            if let Some(tag) = seg["tag"].as_str() {
                let mandatory = seg["mandatory"].as_bool().unwrap_or(false);
                map.entry(tag.to_owned()).or_insert(mandatory);
            }
        }
    }
    if let Some(nested) = group["groups"].as_array() {
        for g in nested {
            collect_group_status(g, map);
        }
    }
}

// ── AHB diff ──────────────────────────────────────────────────────────────────

fn diff_ahb_into(from_dir: &str, to_dir: &str, from: &str, to: &str, out: &mut String) -> bool {
    let from_path = format!("{from_dir}/ahb.json");
    let to_path = format!("{to_dir}/ahb.json");

    let from_ahb = match load_json(&from_path) {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!("## AHB\n\n> ⚠ cannot load {from_path}: {e}\n\n"));
            return false;
        }
    };
    let to_ahb = match load_json(&to_path) {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!("## AHB\n\n> ⚠ cannot load {to_path}: {e}\n\n"));
            return false;
        }
    };

    let from_pids = collect_pids(&from_ahb);
    let to_pids = collect_pids(&to_ahb);

    let mut added_pids: Vec<_> = to_pids
        .keys()
        .filter(|k| !from_pids.contains_key(*k))
        .cloned()
        .collect();
    let mut removed_pids: Vec<_> = from_pids
        .keys()
        .filter(|k| !to_pids.contains_key(*k))
        .cloned()
        .collect();
    added_pids.sort();
    removed_pids.sort();

    // Detect rule changes for common PIDs.
    let mut rule_changes: Vec<String> = Vec::new();
    let mut common_codes: Vec<u64> = from_pids
        .keys()
        .filter(|k| to_pids.contains_key(*k))
        .cloned()
        .collect();
    common_codes.sort();
    for code in common_codes {
        let from_rules = &from_pids[&code];
        let to_rules = &to_pids[&code];
        let mut added_rules: Vec<_> = to_rules.difference(from_rules).cloned().collect();
        let mut removed_rules: Vec<_> = from_rules.difference(to_rules).cloned().collect();
        added_rules.sort();
        removed_rules.sort();
        if !added_rules.is_empty() || !removed_rules.is_empty() {
            rule_changes.push(format!("**PID {code:08}**"));
            for r in &added_rules {
                rule_changes.push(format!("  - added rule: `{r}`"));
            }
            for r in &removed_rules {
                rule_changes.push(format!("  - removed rule: `{r}`"));
            }
        }
    }

    let changed = !added_pids.is_empty() || !removed_pids.is_empty() || !rule_changes.is_empty();
    out.push_str(&format!(
        "## AHB — Pruefidentifikator changes ({from} → {to})\n\n"
    ));

    if !changed {
        out.push_str("No AHB Pruefidentifikator changes.\n");
    } else {
        if !added_pids.is_empty() {
            out.push_str("### Added Pruefidentifikatoren\n\n");
            for code in &added_pids {
                let name = to_ahb["pruefidentifikatoren"]
                    .as_array()
                    .and_then(|arr| arr.iter().find(|p| p["code"].as_u64() == Some(*code)))
                    .and_then(|p| p["name"].as_str())
                    .unwrap_or("?");
                out.push_str(&format!("- `{code:08}` — {name}\n"));
            }
            out.push('\n');
        }
        if !removed_pids.is_empty() {
            out.push_str("### Removed Pruefidentifikatoren\n\n");
            for code in &removed_pids {
                let name = from_ahb["pruefidentifikatoren"]
                    .as_array()
                    .and_then(|arr| arr.iter().find(|p| p["code"].as_u64() == Some(*code)))
                    .and_then(|p| p["name"].as_str())
                    .unwrap_or("?");
                out.push_str(&format!("- `{code:08}` — {name}\n"));
            }
            out.push('\n');
        }
        if !rule_changes.is_empty() {
            out.push_str("### Rule changes for existing Pruefidentifikatoren\n\n");
            for line in &rule_changes {
                out.push_str(&format!("{line}\n"));
            }
        }
    }

    changed
}

fn collect_pids(ahb: &Value) -> BTreeMap<u64, BTreeSet<String>> {
    let mut map = BTreeMap::new();
    if let Some(pids) = ahb["pruefidentifikatoren"].as_array() {
        for pid in pids {
            let code = match pid["code"].as_u64() {
                Some(c) => c,
                None => continue,
            };
            let mut rules = BTreeSet::new();
            if let Some(seg_rules) = pid["segment_rules"].as_array() {
                for rule in seg_rules {
                    let tag = match rule["tag"].as_str() {
                        Some(t) => t,
                        None => continue,
                    };
                    let req = rule["requirement"].as_str().unwrap_or("?");
                    // Base requirement rule
                    rules.insert(format!("{tag}:{req}"));

                    // Qualifier restriction rules: "{tag}.{de_id}:Q:[val1,val2,...]"
                    // qualifier_restrictions is a JSON object {"de_id": ["val1", "val2"]}.
                    if let Some(qrs) = rule["qualifier_restrictions"].as_object() {
                        for (de_id, vals_val) in qrs {
                            let mut vals: Vec<&str> = vals_val
                                .as_array()
                                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();
                            vals.sort_unstable();
                            rules.insert(format!("{tag}.{de_id}:Q:[{}]", vals.join(",")));
                        }
                    }

                    // Field value rules: "{tag}.{element}@{idx}:V:[val1,val2,...]"
                    if let Some(frs) = rule["field_rules"].as_array() {
                        for fr in frs {
                            let element = fr["element"].as_str().unwrap_or("?");
                            let idx = fr["element_index"].as_u64().unwrap_or(0);
                            let mut vals: Vec<&str> = fr["allowed_values"]
                                .as_array()
                                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();
                            vals.sort_unstable();
                            rules.insert(format!("{tag}.{element}@{idx}:V:[{}]", vals.join(",")));
                        }
                    }

                    // Required qualifier rules: "{tag}.{de_id}:RQ:[val1,val2,...]"
                    // required_qualifiers is a JSON object {"de_id": ["val1", "val2"]}.
                    if let Some(rqs) = rule["required_qualifiers"].as_object() {
                        for (de_id, vals_val) in rqs {
                            let mut vals: Vec<&str> = vals_val
                                .as_array()
                                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();
                            vals.sort_unstable();
                            rules.insert(format!("{tag}.{de_id}:RQ:[{}]", vals.join(",")));
                        }
                    }

                    // Conditional rules: "{tag}:C:{op}:{when_tag}[={when_value}]->{then_req}"
                    if let Some(crs) = rule["conditional_rules"].as_array() {
                        for (i, cr) in crs.iter().enumerate() {
                            let op = cr["operator"].as_str().unwrap_or("I");
                            let when_tag = cr["when_tag"].as_str().unwrap_or("?");
                            let when_val = cr["when_value"].as_str().unwrap_or("");
                            let then_req = cr["then_requirement"].as_str().unwrap_or("?");
                            let trigger = if when_val.is_empty() {
                                when_tag.to_owned()
                            } else {
                                format!("{when_tag}={when_val}")
                            };
                            rules.insert(format!("{tag}:C{i}:{op}:{trigger}->{then_req}"));
                        }
                    }
                }
            }
            map.insert(code, rules);
        }
    }
    map
}

// ── Codelists diff ─────────────────────────────────────────────────────────────

fn diff_codelists_into(
    from_dir: &str,
    to_dir: &str,
    from: &str,
    to: &str,
    out: &mut String,
) -> bool {
    let from_path = format!("{from_dir}/codelists.json");
    let to_path = format!("{to_dir}/codelists.json");

    let from_cl = match load_json(&from_path) {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!(
                "## Codelists\n\n> ⚠ cannot load {from_path}: {e}\n\n"
            ));
            return false;
        }
    };
    let to_cl = match load_json(&to_path) {
        Ok(v) => v,
        Err(e) => {
            out.push_str(&format!(
                "## Codelists\n\n> ⚠ cannot load {to_path}: {e}\n\n"
            ));
            return false;
        }
    };

    let from_lists = collect_codelists(&from_cl);
    let to_lists = collect_codelists(&to_cl);

    let all_des: BTreeSet<String> = from_lists.keys().chain(to_lists.keys()).cloned().collect();
    let all_des: Vec<_> = {
        let mut v: Vec<_> = all_des.into_iter().collect();
        v.sort();
        v
    };

    let mut changes: Vec<String> = Vec::new();
    for de in &all_des {
        let from_codes = from_lists.get(de).cloned().unwrap_or_default();
        let to_codes = to_lists.get(de).cloned().unwrap_or_default();
        let mut added: Vec<_> = to_codes.difference(&from_codes).cloned().collect();
        let mut removed: Vec<_> = from_codes.difference(&to_codes).cloned().collect();
        added.sort();
        removed.sort();
        if !added.is_empty() || !removed.is_empty() {
            changes.push(format!("**DE {de}**"));
            for c in &added {
                changes.push(format!("  - added: `{c}`"));
            }
            for c in &removed {
                changes.push(format!("  - removed: `{c}`"));
            }
        }
    }

    let changed = !changes.is_empty();
    out.push_str(&format!("## Codelists — changes ({from} → {to})\n\n"));
    if !changed {
        out.push_str("No codelist changes.\n");
    } else {
        for line in &changes {
            out.push_str(&format!("{line}\n"));
        }
    }
    changed
}

fn collect_codelists(cl: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut map = BTreeMap::new();
    if let Some(lists) = cl["lists"].as_object() {
        for (de, codes) in lists {
            let mut set = BTreeSet::new();
            if let Some(arr) = codes.as_array() {
                for code in arr {
                    if let Some(s) = code.as_str() {
                        set.insert(s.to_owned());
                    }
                }
            }
            map.insert(de.clone(), set);
        }
    }
    map
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn load_json(path: &str) -> Result<Value, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}
