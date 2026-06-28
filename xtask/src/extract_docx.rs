//! `cargo xtask extract-docx` — extract EDI@Energy MIG/AHB data from a BDEW DOCX file.
//!
//! BDEW publishes DOCX files alongside PDFs for every MIG and AHB document.
//! A DOCX is a ZIP archive containing `word/document.xml` (Office Open XML).
//! Tables in that XML have exact column boundaries (`<w:tbl>/<w:tr>/<w:tc>`),
//! which eliminates the whitespace-heuristic parsing needed for PDF extraction.
//!
//! # What this extractor produces
//!
//! **MIG draft** (`mig.draft.json`): all rows from the
//! "Nachrichtenstruktur / Segmentlayout" table — each row carries the segment
//! tag, level, mandatory flag, max-occurrences, and description.
//!
//! **AHB draft** (`ahb.draft.json`): for AHB DOCX files, the multi-column
//! status table maps each segment row to each Prüfidentifikator column.
//! Each PID column yields a list of segment rules with the BDEW status
//! character (`M`, `K`, `S`, `D`, `O`, `X`) and any condition expression.
//!
//! Both output files carry a `"_WARNING"` key and require human review before
//! being promoted to production profiles.
//!
//! # Usage
//!
//! ```text
//! cargo xtask extract-docx \
//!   --file    docs/pdfs/MSCONS_MIG_2.5.docx \
//!   --message-type mscons \
//!   --release 2.5
//!
//! cargo xtask extract-docx \
//!   --file    docs/pdfs/UTILMD_AHB_Strom_S2.2.docx \
//!   --message-type utilmd \
//!   --release S2.2 \
//!   --mode ahb
//! ```
//!
//! # DOCX table format
//!
//! ## MIG column layout (Nachrichtenstruktur table)
//!
//! | Zähler | Seg-Nr | Tag | St_std | St_BDEW | MaxWdh_std | MaxWdh_BDEW | Ebene | Bezeichnung |
//! |--------|--------|-----|--------|---------|------------|-------------|-------|-------------|
//!
//! ## AHB column layout (Anwendungshandbuch table)
//!
//! | Zähler | Tag | Bezeichnung | Status_PID1 | Bedingung_PID1 | Status_PID2 | … |
//! |--------|-----|-------------|-------------|----------------|-------------|---|
//!
//! The exact column count varies by message type and PID combination.
//! PIDs are listed in the header row as 5-digit codes.

use std::{
    collections::HashMap,
    io::{Cursor, Read},
    path::PathBuf,
};

use serde_json::{Value, json};

// ── Public entry point ────────────────────────────────────────────────────────

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

    let docx_path = PathBuf::from(&opts.file);
    if !docx_path.exists() {
        eprintln!("error: DOCX file not found: {}", opts.file);
        return false;
    }

    eprintln!("Opening DOCX: {}", opts.file);
    let docx_bytes = match std::fs::read(&docx_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read DOCX: {e}");
            return false;
        }
    };

    let document_xml = match read_document_xml(&docx_bytes) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: cannot extract word/document.xml from DOCX: {e}");
            return false;
        }
    };
    eprintln!("Extracted word/document.xml ({} bytes)", document_xml.len());

    let release = opts
        .release
        .unwrap_or_else(|| infer_release_from_path(&opts.file));
    let msg_type = opts.message_type.to_uppercase();

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

    let draft_release = format!("DRAFT-{release}");

    match opts.mode {
        ExtractMode::Mig => {
            let mig = extract_mig_from_xml(&document_xml, &msg_type, &draft_release);
            let seg_count = mig
                .get("segments")
                .and_then(|s| s.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            eprintln!("MIG segments extracted: {seg_count}");
            if seg_count == 0 {
                eprintln!(
                    "warning: no MIG segment rows found — check that the DOCX contains a Nachrichtenstruktur table"
                );
            }
            let out_path = out_dir.join("mig.draft.json");
            match write_json(&out_path, &mig) {
                Ok(()) => eprintln!("wrote {}", out_path.display()),
                Err(e) => {
                    eprintln!("error writing {}: {e}", out_path.display());
                    return false;
                }
            }
        }
        ExtractMode::Ahb => {
            let ahb = extract_ahb_from_xml(&document_xml, &msg_type, &draft_release);
            let pid_count = ahb
                .get("pruefidentifikatoren")
                .and_then(|p| p.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            eprintln!("AHB Prüfidentifikatoren extracted: {pid_count}");
            if pid_count == 0 {
                eprintln!("warning: no PIDs found — check that the DOCX is an AHB (not a MIG)");
            }
            let out_path = out_dir.join("ahb.draft.json");
            match write_json(&out_path, &ahb) {
                Ok(()) => eprintln!("wrote {}", out_path.display()),
                Err(e) => {
                    eprintln!("error writing {}: {e}", out_path.display());
                    return false;
                }
            }
        }
        ExtractMode::Both => {
            let mig = extract_mig_from_xml(&document_xml, &msg_type, &draft_release);
            let ahb = extract_ahb_from_xml(&document_xml, &msg_type, &draft_release);
            let seg_count = mig
                .get("segments")
                .and_then(|s| s.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            let pid_count = ahb
                .get("pruefidentifikatoren")
                .and_then(|p| p.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            eprintln!("MIG segments: {seg_count}  AHB PIDs: {pid_count}");

            let mig_path = out_dir.join("mig.draft.json");
            let ahb_path = out_dir.join("ahb.draft.json");
            for (path, val) in [(&mig_path, &mig), (&ahb_path, &ahb)] {
                match write_json(path, val) {
                    Ok(()) => eprintln!("wrote {}", path.display()),
                    Err(e) => {
                        eprintln!("error writing {}: {e}", path.display());
                        return false;
                    }
                }
            }
        }
    }

    eprintln!();
    eprintln!("IMPORTANT: Draft files require human review before use as production profiles.");
    true
}

// ── DOCX / ZIP reading ────────────────────────────────────────────────────────

/// Extract `word/document.xml` from a DOCX ZIP archive.
fn read_document_xml(docx_bytes: &[u8]) -> Result<String, String> {
    let cursor = Cursor::new(docx_bytes);
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| format!("zip open: {e}"))?;
    let mut file = zip
        .by_name("word/document.xml")
        .map_err(|_| "word/document.xml not found in DOCX ZIP".to_owned())?;
    let mut xml = String::new();
    file.read_to_string(&mut xml)
        .map_err(|e| format!("read word/document.xml: {e}"))?;
    Ok(xml)
}

// ── XML table parser ──────────────────────────────────────────────────────────

/// A parsed table from `word/document.xml`.
///
/// Each table is a `Vec<Vec<String>>` — outer = rows, inner = cell texts.
type DocxTable = Vec<Vec<String>>;

/// Extract all tables from Office Open XML `word/document.xml` content.
///
/// Uses a streaming state-machine over quick-xml events to avoid building
/// a full DOM.  The algorithm tracks:
/// 1. When we enter a `<w:tbl>` — start collecting a new table.
/// 2. When we enter a `<w:tr>` — start a new row.
/// 3. When we enter a `<w:tc>` — start a new cell.
/// 4. `<w:t>` text content is appended to the current cell buffer.
/// 5. On `</w:tc>` the cell buffer is flushed into the current row.
/// 6. On `</w:tr>` the row is flushed into the current table.
/// 7. On `</w:tbl>` the table is pushed onto the output list.
fn extract_tables_from_xml(xml: &str) -> Vec<DocxTable> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut tables: Vec<DocxTable> = Vec::new();
    let mut current_table: Option<DocxTable> = None;
    let mut current_row: Option<Vec<String>> = None;
    let mut current_cell: Option<String> = None;
    // Nesting depth inside <w:tbl> to handle nested tables correctly.
    let mut tbl_depth: u32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name_bytes = e.name();
                let local = local_name(name_bytes.as_ref());
                match local {
                    b"tbl" => {
                        tbl_depth += 1;
                        if tbl_depth == 1 {
                            current_table = Some(Vec::new());
                        }
                    }
                    b"tr" if tbl_depth == 1 => {
                        current_row = Some(Vec::new());
                    }
                    b"tc" if tbl_depth == 1 => {
                        current_cell = Some(String::new());
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name();
                let local = local_name(name_bytes.as_ref());
                match local {
                    b"tbl" => {
                        if tbl_depth == 1 {
                            if let Some(t) = current_table.take() {
                                tables.push(t);
                            }
                        }
                        tbl_depth = tbl_depth.saturating_sub(1);
                    }
                    b"tr" if tbl_depth == 1 => {
                        if let (Some(mut table), Some(row)) =
                            (current_table.take(), current_row.take())
                        {
                            table.push(row);
                            current_table = Some(table);
                        }
                    }
                    b"tc" if tbl_depth == 1 => {
                        if let (Some(row), Some(cell)) = (current_row.as_mut(), current_cell.take())
                        {
                            row.push(cell.trim().to_owned());
                        }
                    }
                    b"p" if tbl_depth == 1 => {
                        // Paragraph break inside a cell → insert a space so
                        // multi-paragraph cells don't merge words.
                        if let Some(cell) = current_cell.as_mut() {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if tbl_depth == 1 {
                    if let Some(cell) = current_cell.as_mut() {
                        if let Ok(text) = e.decode() {
                            cell.push_str(&text);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    tables
}

/// Strip the namespace prefix from an XML element name.
///
/// `w:tbl` → `tbl`, `tbl` → `tbl`.
fn local_name(name: &[u8]) -> &[u8] {
    name.iter()
        .position(|&b| b == b':')
        .map_or(name, |i| &name[i + 1..])
}

// ── MIG extraction ────────────────────────────────────────────────────────────

/// Identify the MIG Nachrichtenstruktur table among all tables in the document.
///
/// The MIG table has a header row containing "Zähler" and "Ebene" and a status
/// keyword.  We pick the **first** table whose header row matches.
fn find_mig_table(tables: &[DocxTable]) -> Option<&DocxTable> {
    tables.iter().find(|t| {
        t.first().is_some_and(|header| {
            let h = header.join(" ");
            (h.contains("Z\u{00e4}hler") || h.contains("Zaehler"))
                && h.contains("Ebene")
                && (h.contains("Status") || h.contains("MaxWdh") || h.contains("MaxWiederh"))
        })
    })
}

/// Column indices detected from the MIG table header row.
struct MigColumns {
    tag: usize,
    level: usize,
    status_bdew: usize,
    max_wdh_bdew: usize,
    name: usize,
}

/// Detect MIG column positions from the header row.
///
/// We look for fixed header cell strings rather than positional assumptions
/// so the parser survives minor BDEW table-layout changes between releases.
fn detect_mig_columns(header: &[String]) -> Option<MigColumns> {
    let col = |needle: &str| -> Option<usize> {
        header.iter().position(|h| {
            let h = h.to_lowercase();
            h.contains(&needle.to_lowercase())
        })
    };

    // "Segment / Gruppe" or "Seg-Nr" / "Segmentname" column
    let tag = col("Segment")
        .or_else(|| col("Seg-Nr"))
        .or_else(|| col("Gruppe"))?;

    let level = col("Ebene")?;

    // The BDEW-specific status column is the second "Status" column (after the
    // UN/std one).  Look for "BDEW" in the same cell or fall back to the last
    // "Status" column.
    let status_bdew = header
        .iter()
        .rposition(|h| h.to_lowercase().contains("status"))
        .or_else(|| col("St."))?;

    let max_wdh_bdew = header
        .iter()
        .rposition(|h| {
            let h = h.to_lowercase();
            h.contains("maxwdh") || h.contains("max. wdh") || h.contains("maxwiederh")
        })
        .or_else(|| col("Wdh"))?;

    let name = col("Bezeichnung")
        .or_else(|| col("Name"))
        .or_else(|| header.len().checked_sub(1))?;

    Some(MigColumns {
        tag,
        level,
        status_bdew,
        max_wdh_bdew,
        name,
    })
}

fn extract_mig_from_xml(xml: &str, msg_type: &str, release: &str) -> Value {
    let tables = extract_tables_from_xml(xml);
    let Some(mig_table) = find_mig_table(&tables) else {
        return json!({
            "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-docx`. No MIG table found.",
            "message_type": msg_type,
            "release": release,
            "source": "docx-extract",
            "segments": [],
        });
    };

    let mut rows_iter = mig_table.iter();
    let header = rows_iter.next().cloned().unwrap_or_default();
    let Some(cols) = detect_mig_columns(&header) else {
        return json!({
            "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-docx`. Could not detect MIG columns.",
            "message_type": msg_type,
            "release": release,
            "source": "docx-extract",
            "segments": [],
        });
    };

    fn get_cell(row: &[String], idx: usize) -> &str {
        row.get(idx).map(String::as_str).unwrap_or("")
    }

    // Stack of (level, group_tag) for parent_group tracking.
    let mut group_stack: Vec<(u32, String)> = Vec::new();
    let mut segments: Vec<Value> = Vec::new();

    for row in rows_iter {
        let tag_raw = get_cell(row, cols.tag).trim();
        if tag_raw.is_empty() {
            continue;
        }

        let is_group = tag_raw.starts_with("SG")
            && tag_raw.len() > 2
            && tag_raw[2..].bytes().all(|b| b.is_ascii_digit());

        let is_segment = !is_group
            && tag_raw.len() >= 3
            && tag_raw.len() <= 6
            && tag_raw.bytes().all(|b| b.is_ascii_uppercase());

        if !is_group && !is_segment {
            continue;
        }

        let level_raw = get_cell(row, cols.level);
        let level: u32 = level_raw.trim().parse().unwrap_or(0);

        // Maintain parent-group stack.
        while group_stack.last().is_some_and(|(l, _)| *l >= level) {
            group_stack.pop();
        }
        let parent_group: Option<String> = group_stack.last().map(|(_, g)| g.clone());

        if is_group {
            group_stack.push((level, tag_raw.to_owned()));
        }

        let status_raw = get_cell(row, cols.status_bdew).trim().to_uppercase();
        let mandatory = matches!(status_raw.as_str(), "M" | "R");

        let max_occ: u64 = get_cell(row, cols.max_wdh_bdew)
            .trim()
            .replace('.', "") // some DOCX files use 999.999
            .parse()
            .unwrap_or(1);

        let name = get_cell(row, cols.name).trim().to_owned();

        let mut obj = serde_json::Map::new();
        if is_group {
            obj.insert("group".into(), json!(tag_raw));
        } else {
            obj.insert("tag".into(), json!(tag_raw));
        }
        obj.insert("name".into(), json!(name));
        obj.insert("mandatory".into(), json!(mandatory));
        obj.insert("max_occurrences".into(), json!(max_occ));
        obj.insert("level".into(), json!(level));
        if let Some(pg) = parent_group {
            obj.insert("parent_group".into(), json!(pg));
        }
        segments.push(Value::Object(obj));
    }

    json!({
        "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-docx`. \
                     Requires human review before promotion to a production profile.",
        "message_type": msg_type,
        "release": release,
        "source": "docx-extract (OOXML table parser)",
        "segments": segments,
    })
}

// ── AHB extraction ────────────────────────────────────────────────────────────

/// Identify AHB tables: a table whose header row contains 5-digit PID codes.
///
/// Returns all matching tables (one per process table in the AHB).
fn find_ahb_tables(tables: &[DocxTable]) -> Vec<&DocxTable> {
    tables
        .iter()
        .filter(|t| {
            t.first()
                .is_some_and(|header| header.iter().any(|cell| is_pid_cell(cell)))
        })
        .collect()
}

fn is_pid_cell(s: &str) -> bool {
    let s = s.trim();
    s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit()) && {
        s.parse::<u32>()
            .map(|v| (10000..=99999).contains(&v))
            .unwrap_or(false)
    }
}

fn extract_ahb_from_xml(xml: &str, msg_type: &str, release: &str) -> Value {
    let tables = extract_tables_from_xml(xml);
    let ahb_tables = find_ahb_tables(&tables);

    if ahb_tables.is_empty() {
        return json!({
            "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-docx`. No AHB tables found.",
            "message_type": msg_type,
            "release": release,
            "source": "docx-extract",
            "pruefidentifikatoren": [],
        });
    }

    // Collect all PIDs and their per-segment rules across all AHB tables.
    let mut pid_order: Vec<String> = Vec::new();
    let mut pid_rules: HashMap<String, Vec<Value>> = HashMap::new();

    for table in ahb_tables {
        let mut rows_iter = table.iter();
        let header = rows_iter.next().cloned().unwrap_or_default();

        // Find which columns are PID columns and which is the segment tag column.
        let tag_col = header.iter().position(|h| {
            let h = h.to_lowercase();
            h.contains("segment") || h.contains("seg-nr") || h.contains("gruppe")
        });

        // Collect (column_index, pid_string) pairs.
        let pid_cols: Vec<(usize, String)> = header
            .iter()
            .enumerate()
            .filter(|(_, cell)| is_pid_cell(cell))
            .map(|(i, cell)| (i, cell.trim().to_owned()))
            .collect();

        for pid_str in pid_cols.iter().map(|(_, p)| p.clone()) {
            if let std::collections::hash_map::Entry::Vacant(e) = pid_rules.entry(pid_str.clone()) {
                pid_order.push(pid_str);
                e.insert(Vec::new());
            }
        }

        for row in rows_iter {
            let tag_raw = tag_col
                .and_then(|i| row.get(i))
                .map(String::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();

            if tag_raw.is_empty() {
                continue;
            }

            for (col_idx, pid) in &pid_cols {
                let cell = row.get(*col_idx).map(String::as_str).unwrap_or("").trim();
                if cell.is_empty() || cell == "-" {
                    continue;
                }
                // The cell may contain "M", "K", "S", "D", "O", "X" followed
                // by an optional condition expression on the same or next line.
                let (status, condition) = parse_ahb_cell(cell);
                let rule = if let Some(cond) = condition {
                    json!({ "tag": tag_raw, "status": status, "condition": cond })
                } else {
                    json!({ "tag": tag_raw, "status": status })
                };
                pid_rules.entry(pid.clone()).or_default().push(rule);
            }
        }
    }

    let pruefidentifikatoren: Vec<Value> = pid_order
        .into_iter()
        .map(|pid| {
            let rules = pid_rules.remove(&pid).unwrap_or_default();
            json!({ "pruefidentifikator": pid, "segment_rules": rules })
        })
        .collect();

    json!({
        "_WARNING": "DRAFT — auto-generated by `cargo xtask extract-docx`. \
                     Requires human review before use as a production profile.",
        "message_type": msg_type,
        "release": release,
        "source": "docx-extract (OOXML multi-column AHB table parser)",
        "pruefidentifikatoren": pruefidentifikatoren,
    })
}

/// Parse a single AHB table cell.
///
/// Returns `(status_char, optional_condition_expression)`.
///
/// Examples:
/// - `"M"`            → `("M", None)`
/// - `"K\n[1]"`       → `("K", Some("[1]"))`
/// - `"K [1] U [2]"`  → `("K", Some("[1] U [2]"))`
fn parse_ahb_cell(cell: &str) -> (String, Option<String>) {
    let trimmed = cell.trim();
    // Status char is the first non-whitespace character.
    let status = trimmed
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();

    // Everything after the status char (and optional whitespace/newline) is
    // the condition expression.
    let rest = trimmed[status.len()..].trim();
    let condition = if rest.is_empty() {
        None
    } else {
        Some(rest.to_owned())
    };

    (status, condition)
}

// ── Release inference ─────────────────────────────────────────────────────────

fn infer_release_from_path(file: &str) -> String {
    let path = std::path::Path::new(file);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
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

// ── JSON writer ───────────────────────────────────────────────────────────────

fn write_json(path: &std::path::Path, v: &Value) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

// ── CLI argument parsing ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum ExtractMode {
    /// Extract only the MIG segment table.
    Mig,
    /// Extract only the AHB per-PID rules.
    Ahb,
    /// Attempt to extract both from the same DOCX.
    Both,
}

struct ExtractDocxOpts {
    file: String,
    message_type: String,
    release: Option<String>,
    mode: ExtractMode,
}

fn parse_args(args: &[String]) -> Result<ExtractDocxOpts, String> {
    let mut file = None;
    let mut message_type = None;
    let mut release = None;
    let mut mode = ExtractMode::Both;
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
            "--mode" => {
                i += 1;
                let raw = args.get(i).ok_or("missing value for --mode")?;
                mode = match raw.as_str() {
                    "mig" => ExtractMode::Mig,
                    "ahb" => ExtractMode::Ahb,
                    "both" => ExtractMode::Both,
                    other => return Err(format!("--mode must be mig|ahb|both, got '{other}'")),
                };
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(ExtractDocxOpts {
        file: file.ok_or("--file is required")?,
        message_type: message_type.ok_or("--message-type is required")?,
        release,
        mode,
    })
}

const USAGE: &str = "\
Usage: cargo xtask extract-docx --file <PATH> --message-type <TYPE> [OPTIONS]

Arguments:
  --file           <PATH>    Path to the MIG or AHB DOCX file
  --message-type   <TYPE>    Message type (e.g. utilmd, mscons, aperak)
  --release        <REL>     EDI@Energy release (inferred from file path if omitted)
  --mode           <MODE>    What to extract: mig | ahb | both (default: both)

Output (inside crates/edi-energy/profiles/<type>/<release>/):
  mig.draft.json   MIG segment table (--mode mig or --mode both)
  ahb.draft.json   AHB per-PID segment rules (--mode ahb or --mode both)

Both output files contain a \"_WARNING\" key and MUST be reviewed before
being promoted to production profiles.

Advantages over extract-pdf:
  - Exact column boundaries (no whitespace heuristics)
  - AHB multi-column tables: one status cell per PID, with condition expressions
  - No PDF font-encoding issues or ligature artefacts
";

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_docx_xml(tables: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>{tables}</w:body>
</w:document>"#
        )
    }

    fn tbl(rows: &[&[&str]]) -> String {
        let mut s = "<w:tbl>".to_owned();
        for row in rows {
            s.push_str("<w:tr>");
            for cell in *row {
                s.push_str(&format!(
                    "<w:tc><w:p><w:r><w:t>{cell}</w:t></w:r></w:p></w:tc>"
                ));
            }
            s.push_str("</w:tr>");
        }
        s.push_str("</w:tbl>");
        s
    }

    #[test]
    fn extract_tables_basic() {
        let xml = minimal_docx_xml(&tbl(&[&["Header A", "Header B"], &["Cell 1", "Cell 2"]]));
        let tables = extract_tables_from_xml(&xml);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0][0], ["Header A", "Header B"]);
        assert_eq!(tables[0][1], ["Cell 1", "Cell 2"]);
    }

    #[test]
    fn local_name_strips_prefix() {
        assert_eq!(local_name(b"w:tbl"), b"tbl");
        assert_eq!(local_name(b"tbl"), b"tbl");
        assert_eq!(local_name(b"r:id"), b"id");
    }

    #[test]
    fn parse_ahb_cell_status_only() {
        let (status, cond) = parse_ahb_cell("M");
        assert_eq!(status, "M");
        assert!(cond.is_none());
    }

    #[test]
    fn parse_ahb_cell_with_condition() {
        let (status, cond) = parse_ahb_cell("K [1] U [2]");
        assert_eq!(status, "K");
        assert_eq!(cond.as_deref(), Some("[1] U [2]"));
    }

    #[test]
    fn is_pid_cell_valid() {
        assert!(is_pid_cell("55001"));
        assert!(is_pid_cell("11001"));
        assert!(!is_pid_cell("1234")); // too short
        assert!(!is_pid_cell("999999")); // out of range
        assert!(!is_pid_cell("BGM")); // not numeric
    }

    #[test]
    fn find_ahb_tables_detects_pid_header() {
        let xml = minimal_docx_xml(&tbl(&[
            &["Segment", "55001", "55002"],
            &["UNH", "M", "M"],
            &["BGM", "M", "M"],
        ]));
        let tables = extract_tables_from_xml(&xml);
        let ahb = find_ahb_tables(&tables);
        assert_eq!(ahb.len(), 1);
    }

    #[test]
    fn mig_extraction_roundtrip() {
        // Simulate a minimal MIG table with two segment groups and shared segment tag.
        let xml = minimal_docx_xml(&tbl(&[
            &[
                "Zähler",
                "Segment / Gruppe",
                "Status",
                "Status",
                "Max. Wdh.",
                "Ebene",
                "Bezeichnung",
            ],
            &["0010", "UNH", "M", "M", "1", "0", "Nachrichtenkopf"],
            &["0020", "SG1", "C", "C", "99", "1", "Referenz-Gruppe"],
            &["0030", "RFF", "M", "M", "1", "2", "Referenz"],
            &["0040", "SG2", "M", "M", "1", "1", "Partner"],
            &["0050", "NAD", "M", "M", "1", "2", "Name und Adresse"],
            &["0060", "RFF", "C", "C", "1", "2", "Partner-Referenz"],
        ]));
        let result = extract_mig_from_xml(&xml, "UTILMD", "DRAFT-test");
        let segs = result["segments"].as_array().expect("segments array");
        // Both RFF entries must be present
        let rff_count = segs
            .iter()
            .filter(|s| s.get("tag").and_then(|t| t.as_str()) == Some("RFF"))
            .count();
        assert_eq!(
            rff_count, 2,
            "RFF in SG1 and RFF in SG2 must both be present"
        );
        // parent_group of first RFF = SG1
        let rff1 = segs
            .iter()
            .find(|s| s.get("tag").and_then(|v| v.as_str()) == Some("RFF"))
            .unwrap();
        assert_eq!(rff1["parent_group"].as_str(), Some("SG1"));
    }

    #[test]
    fn ahb_extraction_multi_pid() {
        let xml = minimal_docx_xml(&tbl(&[
            &["Segment", "55001", "55002"],
            &["UNH", "M", "M"],
            &["BGM", "M", "K [1]"],
            &["DTM", "M", ""],
        ]));
        let result = extract_ahb_from_xml(&xml, "UTILMD", "DRAFT-test");
        let pids = result["pruefidentifikatoren"].as_array().expect("pids");
        assert_eq!(pids.len(), 2);
        let pid2 = pids
            .iter()
            .find(|p| p["pruefidentifikator"].as_str() == Some("55002"))
            .unwrap();
        let rules = pid2["segment_rules"].as_array().unwrap();
        let bgm_rule = rules
            .iter()
            .find(|r| r["tag"].as_str() == Some("BGM"))
            .unwrap();
        assert_eq!(bgm_rule["status"].as_str(), Some("K"));
        assert_eq!(bgm_rule["condition"].as_str(), Some("[1]"));
        // DTM with empty cell — should be absent for PID 55002
        assert!(!rules.iter().any(|r| r["tag"].as_str() == Some("DTM")));
    }

    #[test]
    fn version_inference_docx() {
        assert_eq!(
            infer_release_from_path("docs/pdfs/MSCONS_MIG_2.5.docx"),
            "2.5"
        );
        assert_eq!(
            infer_release_from_path("docs/pdfs/UTILMD_AHB_Strom_S2.2.docx"),
            "S2.2"
        );
    }
}
