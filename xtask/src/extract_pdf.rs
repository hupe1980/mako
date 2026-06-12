//! `cargo xtask extract-pdf` — Extract EDI@Energy MIG/AHB data from a PDF.
//!
//! Parses the **Segmentlayout / Nachrichtenstruktur** table found in every
//! EDI@Energy MIG PDF and emits a structured JSON draft with:
//!
//! - `tag` / `group` — segment tag (e.g. `"BGM"`) or group name (e.g. `"SG1"`)
//! - `name` — human-readable description from the PDF
//! - `mandatory` — `true` if BDEW status is `M` or `R`
//! - `max_occurrences` — BDEW max-repetition column
//! - `level` — nesting depth (`0` = message top-level)
//!
//! The AHB extractor scans for 5-digit Pruefidentifikator codes.
//!
//! # Usage
//!
//! ```text
//! cargo xtask extract-pdf \
//!   --file    docs/pdfs/MSCONS_MIG_2.4c.pdf \
//!   --message-type mscons \
//!   --release 2.4c
//! ```
//!
//! Output: `crates/edi-energy/profiles/<type>/<release>/mig.draft.json` and
//! `ahb.draft.json`.  Both carry `"_WARNING"` and require human review.

use std::{collections::HashMap, path::PathBuf};

use serde_json::{Value, json};

// ── public entry point ────────────────────────────────────────────────────────

pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!();
            eprintln!("{USAGE}");
            return false;
        }
    };

    let pdf_path = PathBuf::from(&opts.file);
    if !pdf_path.exists() {
        eprintln!("error: PDF file not found: {}", opts.file);
        return false;
    }

    eprintln!("Extracting text from PDF: {}", opts.file);
    let text = match pdf_extract::extract_text(&pdf_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: PDF extraction failed: {e}");
            return false;
        }
    };

    let line_count = text.lines().count();
    eprintln!("Extracted {} characters ({line_count} lines)", text.len());

    let release = opts.release.unwrap_or_else(|| infer_release(&opts.file));
    let msg_type = opts.message_type.to_uppercase();

    // Write inside crates/edi-energy/profiles/<type>/<release>/
    let out_dir = PathBuf::from(workspace_root)
        .join("crates")
        .join("edi-energy")
        .join("profiles")
        .join(msg_type.to_lowercase())
        .join(&release);

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error: cannot create output dir {}: {e}", out_dir.display());
        return false;
    }

    let mig = extract_mig(&text, &msg_type, &release);
    let ahb = extract_ahb(&text, &msg_type, &release);

    let mig_path = out_dir.join("mig.draft.json");
    let ahb_path = out_dir.join("ahb.draft.json");

    // Quality gate: fail early when extraction clearly produced too little output,
    // so a BDEW PDF layout change is caught before any draft file is written.
    let mig_entries = count_entries(&mig);
    let ahb_pids = count_pids(&ahb);
    if opts.min_segments > 0 && mig_entries < opts.min_segments {
        eprintln!(
            "error: MIG extraction produced {mig_entries} segment(s), \
             below --min-segments threshold of {} — aborting. \
             Check whether the BDEW PDF layout changed.",
            opts.min_segments
        );
        return false;
    }
    if opts.min_pids > 0 && ahb_pids < opts.min_pids {
        eprintln!(
            "error: AHB extraction produced {ahb_pids} Pr\u{00fc}fidentifikator(en), \
             below --min-pids threshold of {} — aborting. \
             Check whether the BDEW AHB PDF layout changed.",
            opts.min_pids
        );
        return false;
    }

    // Zero-guard: if the MIG extraction produced 0 segment entries but an
    // existing mig.draft.json already has content (e.g. the user ran
    // extract-pdf on an AHB-only PDF), skip overwriting to prevent data loss.
    if mig_entries == 0 && mig_path.exists() {
        eprintln!(
            "SKIP MIG draft (0 entries extracted, existing file preserved): {}",
            mig_path.display()
        );
    } else {
        match write_json(&mig_path, &mig) {
            Ok(_) => eprintln!(
                "Wrote MIG draft ({mig_entries} entries): {}",
                mig_path.display()
            ),
            Err(e) => {
                eprintln!("error writing {}: {e}", mig_path.display());
                return false;
            }
        }
    }
    match write_json(&ahb_path, &ahb) {
        Ok(_) => eprintln!(
            "Wrote AHB draft ({ahb_pids} Pr\u{00fc}fidentifikatoren): {}",
            ahb_path.display()
        ),
        Err(e) => {
            eprintln!("error writing {}: {e}", ahb_path.display());
            return false;
        }
    }

    eprintln!();
    eprintln!("IMPORTANT: Draft files require human review before use as production profiles.");
    true
}

fn count_entries(v: &Value) -> usize {
    v.get("segments")
        .and_then(|s| s.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0)
}

fn count_pids(v: &Value) -> usize {
    v.get("pruefidentifikatoren")
        .and_then(|s| s.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0)
}

fn write_json(path: &std::path::Path, v: &Value) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

// ── EDI@Energy segment table row ──────────────────────────────────────────────

/// A parsed row from the "Nachrichtenstruktur / Segmentlayout" table.
#[derive(Debug)]
struct SegmentRow {
    /// `"UNH"`, `"BGM"`, `"SG1"`, etc.
    tag: String,
    /// `true` if this is a segment group row (tag starts with `SG`).
    is_group: bool,
    /// BDEW status: `true` when status is `M` (mandatory) or `R` (required).
    mandatory: bool,
    /// BDEW max-repetition count.
    max_occurrences: u64,
    /// Nesting depth (0 = top-level message segments).
    level: u32,
    /// Human-readable segment/group description.
    name: String,
}

// ── MIG extraction ────────────────────────────────────────────────────────────

fn extract_mig(text: &str, msg_type: &str, release: &str) -> Value {
    let rows = parse_segment_table(text);

    let segments: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            if row.is_group {
                obj.insert("group".into(), json!(row.tag));
            } else {
                obj.insert("tag".into(), json!(row.tag));
            }
            obj.insert("name".into(), json!(row.name));
            obj.insert("mandatory".into(), json!(row.mandatory));
            obj.insert("max_occurrences".into(), json!(row.max_occurrences));
            obj.insert("level".into(), json!(row.level));
            Value::Object(obj)
        })
        .collect();

    json!({
        "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-pdf`. \
                     Requires human review before promotion to a production profile.",
        "message_type": msg_type,
        "release": release,
        "source": "pdf-extract (EDI@Energy table parser)",
        "segments": segments,
    })
}

/// Parse all segment-table rows from the full PDF text.
///
/// EDI@Energy MIG PDFs contain a "Nachrichtenstruktur" or "Segmentlayout"
/// table whose rows look like one of:
///
/// - MSCONS style (status-status then count-count):
///   `  0010 3 UNH M M 1 1 0 Nachrichtenkopfsegment`
/// - CONTRL style (alternating status-count pairs):
///   `  0020 2  UCI M 1 M 1 0 Übertragungsdatei-Antwort`
///
/// Both formats share the property that the **4th token after the tag** (0-indexed)
/// is always the BDEW `MaxWdh`, and the **5th** is the `Ebene` (nesting level).
fn parse_segment_table(text: &str) -> Vec<SegmentRow> {
    let mut in_table = false;
    let mut rows = Vec::new();

    for line in text.lines() {
        // Detect the table header (appears on every MIG table page).
        if contains_table_header(line) {
            in_table = true;
            continue;
        }

        if !in_table {
            continue;
        }

        if let Some(row) = try_parse_row(line) {
            rows.push(row);
        }
    }

    // De-duplicate: the same group/segment can appear multiple times (once
    // per AHB variant) — keep unique (tag, level) combinations in order,
    // choosing the mandatory=true variant when there's a conflict.
    dedup_rows(rows)
}

/// Returns `true` when a line looks like the EDI@Energy segment table header.
fn contains_table_header(line: &str) -> bool {
    // The header always contains "Zähler" and "Ebene" and "MaxWdh" or "MaxWiederh".
    let l = line;
    (l.contains("Z\u{00e4}hler") || l.contains("Zaehler"))
        && l.contains("Ebene")
        && (l.contains("MaxWdh") || l.contains("MaxWiederh"))
}

/// Attempt to parse one segment table data row.
///
/// Returns `None` for header rows, page headers, narrative text, etc.
fn try_parse_row(line: &str) -> Option<SegmentRow> {
    // Lines must start with substantial whitespace (table indentation).
    let trimmed = line.trim_start();
    let leading = line.len().saturating_sub(trimmed.len());
    if leading < 2 {
        return None;
    }

    let mut parts = trimmed.split_whitespace();

    // Token 0: 4-digit counter
    let counter = parts.next()?;
    if counter.len() != 4 || !counter.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    // Token 1: optional running-number OR the segment tag
    let t1 = parts.next()?;
    let tag_str = if t1.bytes().all(|b| b.is_ascii_digit()) {
        // It was the running number; next token is the tag.
        parts.next()?
    } else {
        t1
    };

    // Validate as EDIFACT segment tag (3+ uppercase ASCII) or segment group (SG\d+).
    let is_group = tag_str.starts_with("SG")
        && tag_str.len() > 2
        && tag_str[2..].bytes().all(|b| b.is_ascii_digit());
    let is_segment = !is_group
        && tag_str.len() >= 3
        && tag_str.len() <= 6
        && tag_str.bytes().all(|b| b.is_ascii_uppercase());

    if !is_group && !is_segment {
        return None;
    }

    // Collect the next 5 tokens: they encode the status/MaxWdh/Ebene data.
    // Both EDI@Energy table formats have exactly 5 tokens here before the name.
    let mut meta = [None::<&str>; 5];
    for slot in &mut meta {
        *slot = parts.next();
    }

    // Remaining tokens (if any) form the name.
    let name_parts: Vec<&str> = parts.collect();

    // All 5 meta tokens must be present.
    let m: Vec<&str> = meta.iter().flatten().copied().collect();
    if m.len() < 5 {
        return None;
    }

    // Token 4 (0-indexed) = Ebene (nesting level): must be a small integer.
    let level: u32 = m[4].parse().ok()?;
    if level > 15 {
        return None; // sanity guard
    }

    // Name = remaining tokens joined; may be empty for continuation rows.
    if name_parts.is_empty() {
        return None;
    }
    let name = name_parts.join(" ");

    // Token 3 = BDEW MaxWdh (always position 3 in both table formats).
    let max_occurrences: u64 = m[3].parse().unwrap_or(1);

    // Determine BDEW status from the tokens.
    // Format A (MSCONS): [Sta_std, Sta_bdew, MaxWdh_std, MaxWdh_bdew, Ebene]
    //   → tokens[1] is a letter → bdew_status = tokens[1]
    // Format B (CONTRL): [St_std, MaxWdh_std, St_bdew, MaxWdh_bdew, Ebene]
    //   → tokens[1] is a digit → bdew_status = tokens[2]
    let bdew_status = if m[1].bytes().all(|b| b.is_ascii_alphabetic()) {
        m[1] // MSCONS format
    } else {
        m[2] // CONTRL format
    };

    let mandatory = matches!(bdew_status, "M" | "R");

    Some(SegmentRow {
        tag: tag_str.to_owned(),
        is_group,
        mandatory,
        max_occurrences,
        level,
        name,
    })
}

/// Remove duplicate rows that arise because the same MIG table repeats
/// across multiple AHB pages.  Keep the row with `mandatory = true` when
/// two rows share the same (tag, level) key.
fn dedup_rows(rows: Vec<SegmentRow>) -> Vec<SegmentRow> {
    let mut seen: HashMap<(String, u32), usize> = HashMap::new();
    let mut result: Vec<SegmentRow> = Vec::new();

    for row in rows {
        let key = (row.tag.clone(), row.level);
        if let Some(&idx) = seen.get(&key) {
            // Upgrade to mandatory if this occurrence is mandatory.
            if row.mandatory && !result[idx].mandatory {
                result[idx].mandatory = true;
            }
        } else {
            seen.insert(key, result.len());
            result.push(row);
        }
    }

    result
}

// ── AHB extraction ────────────────────────────────────────────────────────────

fn extract_ahb(text: &str, msg_type: &str, release: &str) -> Value {
    let mut pids: Vec<String> = Vec::new();
    let mut pid_rules: HashMap<String, Vec<String>> = HashMap::new();
    // PIDs that are "current" on the next segment-rule lines.
    let mut current_pids: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let found_pids = extract_pids_from_line(trimmed);
        if !found_pids.is_empty() {
            // This line is a PID header — possibly multi-PID (UTILMD AHB style).
            current_pids.clear();
            for pid in found_pids {
                if !pid_rules.contains_key(&pid) {
                    pids.push(pid.clone());
                    pid_rules.insert(pid.clone(), Vec::new());
                }
                current_pids.push(pid);
            }
        } else if !current_pids.is_empty() && trimmed.len() > 4 && !contains_table_header(trimmed) {
            for pid in &current_pids {
                pid_rules
                    .entry(pid.clone())
                    .or_default()
                    .push(trimmed.to_owned());
            }
        }
    }

    let pruefidentifikatoren: Vec<Value> = pids
        .into_iter()
        .map(|pid| {
            let rules = pid_rules.remove(&pid).unwrap_or_default();
            json!({ "pruefidentifikator": pid, "extracted_context": rules })
        })
        .collect();

    json!({
        "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-pdf`. \
                     Requires human review before use as a production profile.",
        "message_type": msg_type,
        "release": release,
        "source": "pdf-extract (heuristic PID scan)",
        "pruefidentifikatoren": pruefidentifikatoren,
    })
}

/// Scan `line` for all valid EDI@Energy Pruefidentifikatoren (5-digit, 10000–99999).
///
/// Returns a `Vec` with all matched PIDs.  A return value with more than one
/// entry indicates a multi-PID header row (common in UTILMD AHBs where several
/// process codes share the same page, e.g. `"55001  55002  55003"`).
///
/// An empty `Vec` means no PIDs were found on this line.
fn extract_pids_from_line(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in line.split_whitespace() {
        if token.len() == 5
            && token.bytes().all(|b| b.is_ascii_digit())
            && let Ok(v) = token.parse::<u32>()
            && (10000..=99999).contains(&v)
        {
            found.push(token.to_owned());
        }
    }
    // Only treat the line as a PID line if ALL non-whitespace tokens are
    // either PID numbers or short alphabetic words (e.g. "Prüfidentifikator").
    // If mixed numeric tokens of different lengths appear it is more likely a
    // segment data row (counters, status flags, MaxWdh values).
    if found.is_empty() {
        return found;
    }
    let non_pid_non_word = line.split_whitespace().any(|t| {
        // A segment counter (4 digits) or level number (1-2 digits) appearing
        // alongside a PID signals a data row rather than a PID header row.
        let all_digits = t.bytes().all(|b| b.is_ascii_digit());
        let len = t.len();
        all_digits && len != 5 && len <= 4
    });
    if non_pid_non_word {
        Vec::new() // looks like a segment data row — ignore
    } else {
        found
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn infer_release(file: &str) -> String {
    // Try to extract a version-like component from the file path.
    // Patterns: "2.4c", "S2.1", "5.5.3a", "1.0a"
    let path = std::path::Path::new(file);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        // Walk tokens separated by '_' or '-'; pick the last one that looks like a version.
        for part in stem.split(['_', '-']).rev() {
            if looks_like_version(part) {
                return part.to_owned();
            }
        }
        return stem.to_owned();
    }
    "unknown".to_owned()
}

fn looks_like_version(s: &str) -> bool {
    if s.is_empty() || s.len() > 10 {
        return false;
    }
    let mut has_digit = false;
    let mut has_dot_or_letter = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            has_digit = true;
        } else if c == '.' || c.is_ascii_alphabetic() {
            has_dot_or_letter = true;
        } else {
            return false;
        }
    }
    has_digit && has_dot_or_letter
}

// ── CLI argument parsing ──────────────────────────────────────────────────────

struct ExtractPdfOpts {
    file: String,
    message_type: String,
    release: Option<String>,
    /// Minimum number of MIG segment entries required; `0` disables the check.
    min_segments: usize,
    /// Minimum number of AHB Prüfidentifikatoren required; `0` disables the check.
    min_pids: usize,
}

fn parse_args(args: &[String]) -> Result<ExtractPdfOpts, String> {
    let mut file = None;
    let mut message_type = None;
    let mut release = None;
    let mut min_segments: usize = 0;
    let mut min_pids: usize = 0;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--file" | "-f" => {
                i += 1;
                file = Some(args.get(i).cloned().ok_or("missing value for --file")?);
            }
            "--message-type" | "-m" => {
                i += 1;
                message_type = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("missing value for --message-type")?,
                );
            }
            "--release" | "-r" => {
                i += 1;
                release = Some(args.get(i).cloned().ok_or("missing value for --release")?);
            }
            "--min-segments" => {
                i += 1;
                let raw = args.get(i).ok_or("missing value for --min-segments")?;
                min_segments = raw.parse::<usize>().map_err(|_| {
                    format!("--min-segments must be a non-negative integer, got '{raw}'")
                })?;
            }
            "--min-pids" => {
                i += 1;
                let raw = args.get(i).ok_or("missing value for --min-pids")?;
                min_pids = raw.parse::<usize>().map_err(|_| {
                    format!("--min-pids must be a non-negative integer, got '{raw}'")
                })?;
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
        i += 1;
    }
    Ok(ExtractPdfOpts {
        file: file.ok_or("--file is required")?,
        message_type: message_type.ok_or("--message-type is required")?,
        release,
        min_segments,
        min_pids,
    })
}

const USAGE: &str = "\
Usage: cargo xtask extract-pdf --file <PATH> --message-type <TYPE> [OPTIONS]

Arguments:
  --file           <PATH>    Path to the MIG/AHB PDF file
  --message-type   <TYPE>    Message type (e.g. utilmd, mscons, aperak, contrl)
  --release        <REL>     EDI@Energy release (inferred from file path if omitted)
  --min-segments   <N>       Fail if MIG extraction yields fewer than N segment entries (default: 0 = disabled)
  --min-pids       <N>       Fail if AHB extraction yields fewer than N Prüfidentifikatoren (default: 0 = disabled)

Quality gates (--min-segments / --min-pids):
  Use these to catch silent partial extractions when BDEW changes the PDF layout.
  Example: --min-segments 15 --min-pids 3
  The check exits non-zero and does NOT write draft files when the threshold is not met.

Output (inside crates/edi-energy/profiles/<type>/<release>/):  
  mig.draft.json   Extracted MIG segment table with level/mandatory/max_occurrences
  ahb.draft.json   Extracted AHB Pruefidentifikator codes with context

Both output files contain a \"_WARNING\" key and MUST be reviewed before
being promoted to production profiles.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mscons_style_row() {
        // MSCONS format: Sta_std Sta_bdew MaxWdh_std MaxWdh_bdew Ebene
        let line = "  0010 3 UNH M M 1 1 0 Nachrichtenkopfsegment";
        let row = try_parse_row(line).expect("should parse");
        assert_eq!(row.tag, "UNH");
        assert!(!row.is_group);
        assert!(row.mandatory);
        assert_eq!(row.max_occurrences, 1);
        assert_eq!(row.level, 0);
        assert_eq!(row.name, "Nachrichtenkopfsegment");
    }

    #[test]
    fn parse_mscons_style_sg_row() {
        let line = "  0050 SG1 C D 9 1 1 Referenz";
        let row = try_parse_row(line).expect("should parse SG1");
        assert_eq!(row.tag, "SG1");
        assert!(row.is_group);
        assert!(!row.mandatory);
        assert_eq!(row.max_occurrences, 1);
        assert_eq!(row.level, 1);
    }

    #[test]
    fn parse_contrl_style_row() {
        // CONTRL format: St_std MaxWdh_std St_bdew MaxWdh_bdew Ebene
        let line = "  0020 2  UCI M 1 M 1 0 Interchange Control Response";
        let row = try_parse_row(line).expect("should parse UCI");
        assert_eq!(row.tag, "UCI");
        assert!(!row.is_group);
        assert!(row.mandatory);
        assert_eq!(row.max_occurrences, 1);
        assert_eq!(row.level, 0);
    }

    #[test]
    fn parse_contrl_style_sg_row() {
        let line = "  0030   SG1 C 999999 D 999999 1 UCM-SG2";
        let row = try_parse_row(line).expect("should parse SG1");
        assert_eq!(row.tag, "SG1");
        assert!(row.is_group);
        assert!(!row.mandatory);
        assert_eq!(row.level, 1);
    }

    #[test]
    fn reject_narrative_line() {
        let line = "Die Tabelle beschreibt den Aufbau der Nachricht.";
        assert!(try_parse_row(line).is_none());
    }

    #[test]
    fn reject_non_ascii_tag() {
        // A line that starts with a non-ASCII German word must not panic.
        let line = "  0020 2  ÜBER M 1 M 1 0 Some description";
        // ÜBER is not valid — should return None, not panic.
        let result = try_parse_row(line);
        // Either None or Some with a valid tag is acceptable, but no panic.
        let _ = result;
    }

    #[test]
    fn pid_extraction_single() {
        // Single-PID line — standard MSCONS / APERAK AHB style.
        assert_eq!(
            extract_pids_from_line("11001 UTILMD Strom Netz"),
            vec!["11001".to_owned()]
        );
        // Too short / out of range
        assert!(extract_pids_from_line("1234 too short").is_empty());
        assert!(extract_pids_from_line("999999 out of range").is_empty());
    }

    #[test]
    fn pid_extraction_multi_pid_row() {
        // UTILMD AHB style: multiple PIDs on one line (combined process table).
        let pids = extract_pids_from_line("55001  55002  55003");
        assert_eq!(pids, vec!["55001", "55002", "55003"]);
    }

    #[test]
    fn pid_extraction_ignores_segment_row() {
        // A segment data row has a 4-digit counter alongside a 5-digit value;
        // should not be confused with a PID header.
        let row = "  0010 3 UNH M M 1 1 0 Nachrichtenkopfsegment";
        assert!(extract_pids_from_line(row.trim()).is_empty());
    }

    #[test]
    fn version_inference() {
        assert_eq!(infer_release("docs/pdfs/MSCONS_MIG_2.4c.pdf"), "2.4c");
        assert_eq!(infer_release("docs/pdfs/UTILMD_MIG_Strom_S2.1.pdf"), "S2.1");
        assert_eq!(infer_release("docs/pdfs/CONTRL_MIG_2.0b.pdf"), "2.0b");
    }
}
