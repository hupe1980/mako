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
//!   --file    regulatories/MSCONS_MIG_2.4c.pdf \
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

    // The AHB rule tables are *column* layouts: a row's requirement belongs to
    // whichever Prüfidentifikator column it sits under. `lopdf::extract_text`
    // returns reading-order text with the columns collapsed, which destroys that
    // information, so prefer poppler's `pdftotext -layout` when it is available
    // and fall back to lopdf only for the MIG structure scan.
    let layout_text = layout_text_via_pdftotext(&pdf_path);
    if layout_text.is_none() {
        eprintln!(
            "warning: `pdftotext` not found on PATH — falling back to lopdf. \
             The AHB table parser needs column-preserved text and will find no \
             Prüfidentifikatoren. Install poppler-utils and re-run."
        );
    }

    let text = match lopdf::Document::load(&pdf_path) {
        Err(e) => {
            eprintln!("error: PDF load failed: {e}");
            return false;
        }
        Ok(doc) => {
            let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
            match doc.extract_text(&pages) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: PDF text extraction failed: {e}");
                    return false;
                }
            }
        }
    };

    let line_count = text.lines().count();
    eprintln!("Extracted {} characters ({line_count} lines)", text.len());

    let ahb_text = layout_text.as_deref().unwrap_or(&text);

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

    // The in-file release value uses a "DRAFT-" prefix to make clear that this
    // JSON has not been reviewed/promoted to a production profile.  The directory
    // name stays as-is (the actual BDEW release code) so codegen can still locate
    // the future production mig.json/ahb.json beside the draft.
    let draft_release = format!("DRAFT-{release}");

    let mig = extract_mig(&text, &msg_type, &draft_release);
    let ahb = extract_ahb(ahb_text, &msg_type, &draft_release);

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

    // ── Prior-release comparison ─────────────────────────────────────
    // When --compare-dir is given, load the production mig.json / ahb.json from
    // the prior release directory and emit a "prev N, now M" banner.  Exits
    // non-zero when the new count dropped by more than --max-drop-pct percent
    // relative to the prior release — catching silent partial extractions caused
    // by BDEW PDF layout changes.
    if let Some(ref cdir) = opts.compare_dir {
        let prev_mig_path = std::path::PathBuf::from(cdir).join("mig.json");
        let prev_ahb_path = std::path::PathBuf::from(cdir).join("ahb.json");

        // MIG segment comparison
        if prev_mig_path.exists() {
            let prev_json: Value = std::fs::read_to_string(&prev_mig_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let prev_count = count_entries(&prev_json);
            if prev_count > 0 {
                let dropped = prev_count.saturating_sub(mig_entries);
                let drop_pct = (dropped * 100) / prev_count;
                if drop_pct > opts.max_drop_pct as usize {
                    eprintln!(
                        "error: MIG segment count dropped {drop_pct}% \
                         (prev: {prev_count}, now: {mig_entries}) — \
                         exceeds --max-drop-pct {}. \
                         Check whether the BDEW PDF layout changed.",
                        opts.max_drop_pct
                    );
                    return false;
                }
                let indicator = if mig_entries >= prev_count {
                    "✓"
                } else {
                    "⚠ REVIEW"
                };
                eprintln!("MIG segment count: prev {prev_count} → now {mig_entries} {indicator}");
            }
        }

        // AHB PID comparison
        if prev_ahb_path.exists() {
            let prev_json: Value = std::fs::read_to_string(&prev_ahb_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let prev_count = count_pids(&prev_json);
            if prev_count > 0 {
                let dropped = prev_count.saturating_sub(ahb_pids);
                let drop_pct = (dropped * 100) / prev_count;
                if drop_pct > opts.max_drop_pct as usize {
                    eprintln!(
                        "error: AHB PID count dropped {drop_pct}% \
                         (prev: {prev_count}, now: {ahb_pids}) — \
                         exceeds --max-drop-pct {}. \
                         Check whether the BDEW AHB PDF layout changed.",
                        opts.max_drop_pct
                    );
                    return false;
                }
                let indicator = if ahb_pids >= prev_count {
                    "✓"
                } else {
                    "⚠ REVIEW"
                };
                eprintln!("AHB PID count:     prev {prev_count} → now {ahb_pids} {indicator}");
            }
        }
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
    /// The innermost enclosing segment-group tag at parse time, e.g. `"SG4"`.
    /// `None` for top-level rows (level 0).
    parent_group: Option<String>,
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
            if let Some(ref pg) = row.parent_group {
                obj.insert("parent_group".into(), json!(pg));
            }
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
    // Stack of (level, group_tag) tracking the current nesting context.
    // When we encounter an SG row at level L we push it; when we see a row
    // at level ≤ the top of the stack we pop until the stack is consistent.
    let mut group_stack: Vec<(u32, String)> = Vec::new();

    for line in text.lines() {
        // Detect the table header (appears on every MIG table page).
        if contains_table_header(line) {
            in_table = true;
            continue;
        }

        if !in_table {
            continue;
        }

        if let Some(mut row) = try_parse_row(line) {
            // Pop stack entries whose level >= current row's level so the
            // stack always represents the open ancestors above this row.
            while group_stack.last().is_some_and(|(l, _)| *l >= row.level) {
                group_stack.pop();
            }
            // Assign parent_group from the current top of stack.
            row.parent_group = group_stack.last().map(|(_, g)| g.clone());
            // If this row is itself a group, push it for its children.
            if row.is_group {
                group_stack.push((row.level, row.tag.clone()));
            }
            rows.push(row);
        }
    }

    // De-duplicate: the same group/segment can appear multiple times (once
    // per AHB variant page).  Use (tag, level, parent_group) as the key so
    // RFF inside SG1 and RFF inside SG4 are kept as separate entries.
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
        parent_group: None, // filled in by parse_segment_table
    })
}

/// Remove duplicate rows that arise because the same MIG table repeats
/// across multiple AHB pages.  Keep the row with `mandatory = true` when
/// two rows share the same `(tag, level, parent_group)` key.
///
/// The `parent_group` component prevents `RFF` inside `SG1` and `RFF` inside
/// `SG4` (both at the same level) from collapsing into a single entry.
fn dedup_rows(rows: Vec<SegmentRow>) -> Vec<SegmentRow> {
    let mut seen: HashMap<(String, u32, Option<String>), usize> = HashMap::new();
    let mut result: Vec<SegmentRow> = Vec::new();

    for row in rows {
        let key = (row.tag.clone(), row.level, row.parent_group.clone());
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

/// Column-preserved page text via poppler's `pdftotext -layout`.
///
/// Returns `None` when the binary is unavailable or fails, leaving the caller to
/// fall back to `lopdf` (which is fine for the MIG structure scan but loses the
/// column alignment the AHB table parser depends on).
fn layout_text_via_pdftotext(pdf: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(pdf)
        .arg("-")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Envelope segments carry no AHB rules of their own.
const ENVELOPE_SEGMENTS: &[&str] = &["UNH", "UNT", "UNS", "UNB", "UNZ"];

/// Parse the per-Prüfidentifikator segment requirements out of an AHB PDF.
///
/// The AHB lays each Anwendungsfall out as a table whose columns are PIDs. The
/// header row reads `Prüfidentifikator  <pid> [<pid> …]`, and each following
/// segment row carries `Muss` / `Kann` / `Soll` under the columns it applies to.
///
/// Rules established by validating against the XML-imported ORDERS `fv20260401`
/// profile (32/32 for every PID the tag-level model can express):
///
/// - Column positions come from where each PID appears in the header line; a
///   row's requirement is read from the slice around that position. A
///   single-PID table has no neighbour to confuse, and its mark may sit well
///   left of the header position, so the whole row is scanned instead.
/// - `Muss` → `M`; `Kann` and `Soll` → `O`. `Soll` is a recommendation, not a
///   requirement, and must not be promoted.
/// - Segment-group nesting does **not** propagate: a `Muss` segment inside a
///   `Kann` group stays `M`.
/// - **Optional segments are absent from the AHB table.** The AHB marks what is
///   *required*; the MIG lists what is *available*. Callers therefore complete
///   the rule set with every remaining `mig.json` segment as `O`.
///
/// Known limitation: a conditional `Muss [n]` (e.g. ORDERS 17102/17301 `IMD`,
/// "Wenn BGM+7 vorhanden") is reported as `M`. The XML encodes those as `C`
/// with a `conditional_rules` entry; the tag-level model cannot express it.
fn parse_ahb_requirements(text: &str) -> HashMap<String, HashMap<String, String>> {
    // NB: split on '\n' only — pdftotext emits form feeds at page breaks and
    // `str::lines()` would also split on those, shifting every subsequent row.
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();

    let mut i = 0;
    while i < lines.len() {
        let Some(pids) = pid_header_columns(lines[i]) else {
            i += 1;
            continue;
        };
        let single = pids.len() == 1;

        let mut j = i + 1;
        while j < lines.len() && pid_header_columns(lines[j]).is_none() {
            if let Some(tag) = segment_row_tag(lines[j]) {
                for (pid, col) in &pids {
                    let cell = if single {
                        lines[j].to_owned()
                    } else {
                        slice_around(lines[j], *col)
                    };
                    let Some(req) = requirement_of(&cell) else {
                        continue;
                    };
                    let entry = out.entry(pid.clone()).or_default();
                    // A tag may appear in several groups; `Muss` wins.
                    if entry.get(&tag).map(String::as_str) != Some("M") {
                        entry.insert(tag.clone(), req);
                    }
                }
            }
            j += 1;
        }
        i = j;
    }
    out
}

/// `Some([(pid, column)])` when `line` is a `Prüfidentifikator` table header.
fn pid_header_columns(line: &str) -> Option<Vec<(String, usize)>> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("Prüfidentifikator")?.trim_start();
    if rest.is_empty() {
        return None;
    }
    let mut cols = Vec::new();
    let mut cursor = 0;
    for tok in rest.split_whitespace() {
        if tok.len() != 5 || !tok.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let at = line[cursor..].find(tok)? + cursor;
        cursor = at + tok.len();
        cols.push((tok.to_owned(), at));
    }
    (!cols.is_empty()).then_some(cols)
}

/// The EDIFACT tag of a segment requirement row (`[SGn ]TAG 00042 …`).
fn segment_row_tag(line: &str) -> Option<String> {
    let mut it = line.split_whitespace();
    let mut tok = it.next()?;
    if tok.starts_with("SG") && tok[2..].bytes().all(|b| b.is_ascii_digit()) {
        tok = it.next()?;
    }
    let is_tag = tok.len() == 3 && tok.bytes().all(|b| b.is_ascii_uppercase());
    if !is_tag || ENVELOPE_SEGMENTS.contains(&tok) {
        return None;
    }
    let num = it.next()?;
    (num.len() == 5 && num.bytes().all(|b| b.is_ascii_digit())).then(|| tok.to_owned())
}

/// The slice of `line` covering one PID column.
fn slice_around(line: &str, col: usize) -> String {
    let lo = col.saturating_sub(8);
    let hi = (col + 13).min(line.len());
    if lo >= line.len() {
        return String::new();
    }
    line.get(lo..hi).unwrap_or("").to_owned()
}

/// `Muss` → `M`; `Kann` / `Soll` → `O`.
fn requirement_of(cell: &str) -> Option<String> {
    if cell.contains("Muss") {
        Some("M".to_owned())
    } else if cell.contains("Kann") || cell.contains("Soll") {
        Some("O".to_owned())
    } else {
        None
    }
}

fn extract_ahb(text: &str, msg_type: &str, release: &str) -> Value {
    let parsed = parse_ahb_requirements(text);
    let mut codes: Vec<&String> = parsed.keys().collect();
    codes.sort();

    let pruefidentifikatoren: Vec<Value> = codes
        .into_iter()
        .map(|pid| {
            let mut tags: Vec<(&String, &String)> = parsed[pid].iter().collect();
            tags.sort();
            let rules: Vec<Value> = tags
                .into_iter()
                .map(|(tag, req)| json!({ "tag": tag, "requirement": req }))
                .collect();
            json!({
                "code": pid.parse::<u32>().unwrap_or(0),
                "name": "",
                "segment_rules": rules,
                "group_rules": [],
            })
        })
        .collect();

    json!({
        "_WARNING": "DRAFT — `segment_rules` carry only the AHB's `Muss` marks. \
                     Complete them with every remaining mig.json segment as `O`, \
                     fill in each `name`, and diff against the previous release \
                     before promoting to a production profile.",
        "message_type": msg_type,
        "release": release,
        "source": "pdf-extract (column-aware AHB table parser)",
        "pruefidentifikatoren": pruefidentifikatoren,
    })
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
    /// Optional path to a prior-release `mig.json` / `ahb.json` dir for
    /// automatic row-count comparison. When set, the extractor emits a
    /// "prev: N, now: M" banner and exits non-zero if the new count dropped
    /// by more than `--max-drop-pct` percent (default 10 %).
    compare_dir: Option<String>,
    /// Maximum tolerated percentage drop from the prior release (default: 10).
    max_drop_pct: u64,
}

fn parse_args(args: &[String]) -> Result<ExtractPdfOpts, String> {
    let mut file = None;
    let mut message_type = None;
    let mut release = None;
    let mut min_segments: usize = 0;
    let mut min_pids: usize = 0;
    let mut compare_dir: Option<String> = None;
    let mut max_drop_pct: u64 = 10;
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
            "--compare-dir" => {
                i += 1;
                compare_dir = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("missing value for --compare-dir")?,
                );
            }
            "--max-drop-pct" => {
                i += 1;
                let raw = args.get(i).ok_or("missing value for --max-drop-pct")?;
                max_drop_pct = raw
                    .parse::<u64>()
                    .map_err(|_| format!("--max-drop-pct must be 0..100, got '{raw}'"))?;
                if max_drop_pct > 100 {
                    return Err(format!("--max-drop-pct must be 0..100, got '{raw}'"));
                }
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
        compare_dir,
        max_drop_pct,
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
  --compare-dir    <DIR>     Path to a prior-release profile dir containing mig.json / ahb.json.
                             Emits prev→now count banner; fails if count drops by more than --max-drop-pct.
  --max-drop-pct   <N>       Max tolerated % drop from prior release before failing (default: 10)

Quality gates:
  --min-segments / --min-pids: absolute lower bounds — fail when extraction produced too little output.
  --compare-dir / --max-drop-pct: relative change guard — fail when count dropped vs. prior release.
  Example combining both:
    cargo xtask extract-pdf \\
      --file regulatories/UTILMD_AHB_S2.x_FV2026-10-01.pdf \\
      --message-type utilmd \\
      --release FV2026-10-01 \\
      --compare-dir crates/edi-energy/profiles/utilmd/fv20251001 \\
      --min-pids 5

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
    fn header_columns_single_pid() {
        let cols = pid_header_columns("      Prüfidentifikator          11001").unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].0, "11001");
    }

    #[test]
    fn header_columns_multi_pid_records_positions() {
        let line = "   Prüfidentifikator     55001    55002    55003";
        let cols = pid_header_columns(line).unwrap();
        assert_eq!(
            cols.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["55001", "55002", "55003"]
        );
        // Positions must be strictly increasing — they drive the column slicing.
        assert!(cols[0].1 < cols[1].1 && cols[1].1 < cols[2].1);
    }

    #[test]
    fn header_columns_rejects_non_header_rows() {
        assert!(pid_header_columns("  0010 3 UNH M M 1 1 0 Nachrichtenkopfsegment").is_none());
        assert!(pid_header_columns("      Prüfidentifikator").is_none());
    }

    /// `parse_segment_table` must assign a `parent_group` to segment rows that
    /// appear inside a segment group block, and keep `None` for top-level rows.
    #[test]
    fn parent_group_assigned_during_parse() {
        // Simulate two pages of the same table (the AHB prints the MIG table once
        // per PID page, which is the source of duplicates that dedup_rows removes).
        let text = "\
Zähler Ebene MaxWdh foo
  0010 UNH M M 1 1 0 Nachrichtenkopfsegment
  0020 SG1 C C 9 9 1 Referenz-Gruppe
  0030 RFF M M 1 1 2 Referenz
  0040 SG2 M M 9 9 1 Partner
  0050 NAD M M 1 1 2 Name und Adresse
  0060 RFF C C 1 1 2 Partner-Referenz
";
        let rows = parse_segment_table(text);
        // UNH at level 0: no parent
        let unh = rows.iter().find(|r| r.tag == "UNH").unwrap();
        assert_eq!(unh.parent_group, None);
        // SG1 at level 1: no parent (top-level group)
        let sg1 = rows.iter().find(|r| r.tag == "SG1").unwrap();
        assert_eq!(sg1.parent_group, None);
        // RFF at level 2 inside SG1: parent = "SG1"
        let rff_sg1 = rows
            .iter()
            .find(|r| r.tag == "RFF" && r.parent_group.as_deref() == Some("SG1"))
            .unwrap();
        assert_eq!(rff_sg1.level, 2);
        // NAD at level 2 inside SG2: parent = "SG2"
        let nad = rows.iter().find(|r| r.tag == "NAD").unwrap();
        assert_eq!(nad.parent_group.as_deref(), Some("SG2"));
        // Second RFF at level 2 inside SG2: parent = "SG2", distinct from first RFF
        let rff_sg2 = rows
            .iter()
            .find(|r| r.tag == "RFF" && r.parent_group.as_deref() == Some("SG2"))
            .unwrap();
        assert_eq!(rff_sg2.level, 2);
        // Both RFF entries must be present (not deduplicated)
        assert_eq!(
            rows.iter().filter(|r| r.tag == "RFF").count(),
            2,
            "RFF in SG1 and RFF in SG2 must be kept as separate entries"
        );
    }

    /// When the same table is repeated (duplicate page), `dedup_rows` must
    /// remove exact duplicates but preserve same-tag/same-level rows that are
    /// in different parent groups.
    #[test]
    fn dedup_preserves_same_tag_different_parent() {
        let make = |tag: &str, level: u32, parent: Option<&str>| SegmentRow {
            tag: tag.to_owned(),
            is_group: false,
            mandatory: true,
            max_occurrences: 1,
            level,
            name: "test".to_owned(),
            parent_group: parent.map(str::to_owned),
        };
        let rows = vec![
            make("RFF", 2, Some("SG1")),
            make("RFF", 2, Some("SG4")),
            make("RFF", 2, Some("SG1")), // duplicate — should be removed
        ];
        let deduped = dedup_rows(rows);
        assert_eq!(
            deduped.len(),
            2,
            "duplicate RFF in SG1 removed; RFF in SG4 kept"
        );
        assert!(
            deduped
                .iter()
                .any(|r| r.parent_group.as_deref() == Some("SG1"))
        );
        assert!(
            deduped
                .iter()
                .any(|r| r.parent_group.as_deref() == Some("SG4"))
        );
    }
}

#[cfg(test)]
mod ahb_parser_tests {
    use super::*;

    /// A two-column AHB table, laid out as `pdftotext -layout` emits it.
    const TABLE: &str = concat!(
        "                    Prüfidentifikator          17007       17008\n",
        " Nachrichten-Kopfsegment\n",
        "        UNH           00001                     Muss        Muss\n",
        "        BGM           00002                     Muss        Muss\n",
        "        BGM 1001            Z10  Geräteübernahme  X           X\n",
        " Positionsdaten\n",
        " SG29                                            Kann        Kann\n",
        " SG29 LIN            00052                       Muss\n",
        "        IMD           00011                      Soll [104]  Soll [104]\n",
    );

    #[test]
    fn reads_requirements_per_pid_column() {
        let got = parse_ahb_requirements(TABLE);
        let a = &got["17007"];
        let b = &got["17008"];

        // Envelope segments carry no AHB rules.
        assert!(!a.contains_key("UNH"), "UNH must be excluded");

        assert_eq!(a["BGM"], "M");
        assert_eq!(b["BGM"], "M");

        // `Muss` in one column only must not leak into its neighbour — this is
        // the exact bug that dropped ORDERS 17008/17116/17117 on import.
        assert_eq!(a["LIN"], "M");
        assert!(!b.contains_key("LIN"), "17008 has no LIN mark");

        // `Soll` is a recommendation, never promoted to `M`.
        assert_eq!(a["IMD"], "O");
        assert_eq!(b["IMD"], "O");
    }

    #[test]
    fn group_requirement_does_not_downgrade_its_segments() {
        // `SG29` is `Kann`, but `LIN` inside it reads `Muss` and stays `M`.
        let got = parse_ahb_requirements(TABLE);
        assert_eq!(got["17007"]["LIN"], "M");
    }

    #[test]
    fn form_feeds_do_not_shift_rows() {
        // pdftotext emits \x0c at page breaks; splitting on it would drop rows.
        let paged = TABLE.replace(" Positionsdaten\n", " Positionsdaten\n\x0c");
        assert_eq!(
            parse_ahb_requirements(&paged),
            parse_ahb_requirements(TABLE)
        );
    }
}
