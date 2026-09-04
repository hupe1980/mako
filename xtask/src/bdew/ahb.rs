//! The AHB: one Prüfschablone per Anwendungsfall.
//!
//! Every AHB table has the same shape (Allgemeine Festlegungen 6.1d Kap. 6):
//! a title line `EDIFACT Struktur … Beschreibung … <one column per
//! Anwendungsfall> … Bedingung`, a `Prüfidentifikator` line naming the
//! columns, then one row per segment group, segment, data element and code.
//! A status (`Muss`, `Soll`, `Kann`, an operand `X`/`M`/`S`/`K`) belongs to
//! the column it is printed under; the rightmost column holds the Bedingungen
//! the statuses cite by number.
//!
//! The parser works on `pdftotext -layout` output, where a column keeps its
//! character offset across rows, and reads each token by the column its
//! offset falls into. It needs the MIG for one decision only: whether the
//! token after a data-element number is a code (`137`) or the start of a
//! description (`ID der Marktlokation`), which the AHB typesets identically.

use std::collections::BTreeMap;

use regex::Regex;
use serde::Serialize;

use super::mig::{MigDoc, SegmentNode};
use super::{char_pos, collapse, looks_like_code, rendered_centre, rendered_end, tokens};

/// One Anwendungsfall — a column of the AHB.
#[derive(Debug, Clone, Serialize)]
pub struct Anwendungsfall {
    /// Absent for message types BDEW publishes without Prüfidentifikatoren
    /// (APERAK, CONTRL); their columns are told apart by `BGM` DE 1001.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communication: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    pub rows: Vec<Row>,
    pub elements: Vec<ElementRule>,
}

/// The AHB status of one segment (`nr`) or one segment group (`group`,
/// positioned before the trigger segment `before`).
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// `Muss`, `Muss [10]`, `Soll [3]` … — several when the AHB states several.
    pub status: Vec<String>,
}

/// The operands of one data element of one segment: which codes are admitted
/// (with their operand text) or, for a value element, the operand alone.
#[derive(Debug, Clone, Serialize)]
pub struct ElementRule {
    pub nr: String,
    pub de: String,
    /// Which occurrence of `de` inside the segment layout — `STS` repeats
    /// `C556`/DE 9013 three times.
    #[serde(skip_serializing_if = "is_zero")]
    pub occurrence: u8,
    pub operands: Vec<Operand>,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct Operand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub operand: String,
}

/// The whole document.
#[derive(Debug, Clone)]
pub struct AhbDoc {
    pub anwendungsfaelle: Vec<Anwendungsfall>,
    /// Bedingungen by number (`10`, `931`, `2061`, `UB1`), text as printed.
    pub conditions: BTreeMap<String, String>,
    /// Pakete by id (`1P`) with their Paketvoraussetzung expression.
    pub packages: BTreeMap<String, String>,
}

struct Column {
    x: usize,
    pid: Option<u32>,
    name: String,
    communication: String,
}

struct Table {
    columns: Vec<Column>,
    desc_x: usize,
    /// Left edge of the Bedingung column: the header word, lowered to the
    /// leftmost `[n]` followed by prose the rows show (the texts start a
    /// cell or two left of the centred header word).
    bedingung_x: usize,
    tol: usize,
    /// Index into `AhbDoc::anwendungsfaelle` per column, once the header is
    /// complete.
    targets: Vec<usize>,
    header_done: bool,
    /// Header lines seen before the first segment row fixes the columns:
    /// the title's column names, their wrapped continuations and the
    /// `Kommunikation von` line.
    pending: Vec<String>,
    /// The `Prüfidentifikator` line, when the table has one.
    pid_line: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Last {
    None,
    Row,
    Element,
}

/// Parse an AHB. `mig` decides which tokens are codes.
///
/// # Errors
///
/// When no Anwendungsfall table is found.
pub fn parse(lines: &[String], mig: &MigDoc) -> Result<AhbDoc, String> {
    let by_nr: BTreeMap<&str, &SegmentNode> = mig
        .segments()
        .into_iter()
        .chain(mig.envelope.iter())
        .map(|s| (s.nr.as_str(), s))
        .collect();

    let heading = Regex::new(r"^\s*(\d+(?:\.\d+)+)\s+(\S.*?)\s*$").unwrap();
    let group_row = Regex::new(r"^\s*(SG\d+)(?:\s+(.*))?$").unwrap();
    let seg_row = Regex::new(r"^\s*(?:(SG\d+)\s+)?([A-Z]{3})\s+(\d{5})\b(.*)$").unwrap();
    let de_row = Regex::new(r"^\s*(?:(SG\d+)\s+)?([A-Z]{3})\s+(\d{4})\b(.*)$").unwrap();
    let cond_id = Regex::new(r"^\[(\d+P?|UB\d)\]$").unwrap();
    let package_id = Regex::new(r"^\[(\d+P)\]$").unwrap();

    let mut doc = AhbDoc {
        anwendungsfaelle: Vec::new(),
        conditions: BTreeMap::new(),
        packages: BTreeMap::new(),
    };
    let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
    let mut table: Option<Table> = None;
    let mut chapter: Option<String> = None;
    // The condition whose text is being read, with the text so far; committed
    // (longest reading wins) when the next one opens or the table ends.
    let mut cur_cond: Option<(String, String)> = None;
    let mut last = Last::None;
    // Per column: the last (row | element) appended, to attach continuation
    // statuses to.
    let mut last_row: Vec<Option<usize>> = Vec::new();
    let mut last_el: Vec<Option<usize>> = Vec::new();
    // The segment the running DE rows belong to, with per-DE occurrence
    // counters.
    let mut cur_nr: Option<String> = None;
    let mut cur_group: Option<String> = None;
    let mut de_occ: BTreeMap<String, u8> = BTreeMap::new();
    let mut cur_de: Option<(String, u8)> = None;
    // The code entry the running status cells belong to.
    let mut cur_code: Option<String> = None;
    let mut in_packages = false;
    // The Paket table's column starts, and the Paket the running lines belong
    // to.
    let mut packages: Option<PackageTable> = None;
    let mut cur_package: Option<String> = None;
    let mut prev_targets: Vec<usize> = Vec::new();
    // The previous table's column positions: a table published without
    // Prüfidentifikatoren continues across pages with the same columns, and
    // a page's first row may fill only some of them.
    let mut prev_columns: Vec<usize> = Vec::new();
    // Status-only lines waiting for the row they belong to: a group or segment
    // label centred over a two-line status cell lands between its two lines.
    let mut pending_status: Vec<Pending> = Vec::new();

    // `X[28]` is printed without the space now and then.
    let glued = Regex::new(r"\b(Muss|Soll|Kann|X|M|S|K)\[").unwrap();
    for line in lines {
        let unglued;
        let line: &String = if glued.is_match(line) {
            unglued = glued.replace_all(line, "$1 [").into_owned();
            &unglued
        } else {
            line
        };
        let collapsed = collapse(line);
        let t = collapsed.as_str();
        if t.contains("....") {
            continue;
        }
        // ── Übersicht der Pakete ─────────────────────────────────────────
        // Three columns: the Paket, its Paketvoraussetzung(en) — a Bedingung
        // expression that can run over several lines — and the texts of the
        // Bedingungen it cites. The header fixes where each starts.
        if t.starts_with("Paket") && t.contains("Paketvoraussetzung") {
            in_packages = true;
            packages = char_pos(line, "Paketvoraussetzung")
                .zip(char_pos(line, "Bedingungen"))
                .map(|(pv, bed)| PackageTable { pv, bed });
            continue;
        }
        if in_packages && t.starts_with("EDIFACT Struktur") && t.contains("Beschreibung") {
            in_packages = false;
            cur_package = None;
        }
        if in_packages && let Some(pt) = packages {
            if heading.is_match(line) {
                in_packages = false;
                cur_package = None;
            } else {
                // The Paket column: `[3P]` opens a row, and the rows of a
                // page-broken table resume under the repeated header.
                if let Some(k) = tokens(line)
                    .into_iter()
                    .find(|k| k.x + 2 < pt.pv && package_id.is_match(k.text))
                {
                    cur_package = package_id.captures(k.text).map(|c| c[1].to_owned());
                }
                // The Paketvoraussetzung column, joined across its lines. It
                // holds a Bedingung expression and nothing else, so prose
                // there — the Hinweis under `--`, or a paragraph the table
                // runs into — is not part of it.
                if let Some(id) = &cur_package {
                    let cell = tokens(line)
                        .into_iter()
                        .filter(|k| {
                            k.x >= pt.pv.saturating_sub(2) && k.x < pt.bed.saturating_sub(2)
                        })
                        .map(|k| k.text)
                        .collect::<Vec<_>>();
                    if !cell.is_empty() && cell.iter().all(|t| is_expression_token(t)) {
                        let entry = doc.packages.entry(id.clone()).or_default();
                        for t in cell {
                            if !entry.is_empty() {
                                entry.push(' ');
                            }
                            entry.push_str(t);
                        }
                    } else {
                        doc.packages.entry(id.clone()).or_default();
                    }
                }
                collect_condition(
                    line,
                    pt.bed.saturating_sub(2),
                    &cond_id,
                    &mut cur_cond,
                    &mut doc.conditions,
                );
                continue;
            }
        }

        // ── The Änderungshistorie closes the document ─────────────────────
        // Its rows quote table fragments (`SG1 … Muss [52]`, `PID 27003`).
        if t.starts_with("Änd-ID") {
            commit_condition(&mut cur_cond, &mut doc.conditions);
            flush_pending(&mut pending_status, &mut doc);
            break;
        }

        // ── A new table ───────────────────────────────────────────────────
        if t.starts_with("EDIFACT Struktur") && t.contains("Beschreibung") {
            flush_pending(&mut pending_status, &mut doc);
            let desc_x = char_pos(line, "Beschreibung").unwrap_or(30);
            // `Bedingung` is centred over a multi-line column header and can
            // land on a later line; until seen, the boundary is provisional.
            let bedingung_x = char_pos(line, "Bedingung").unwrap_or(usize::MAX);
            // A page break repeats the header of the table it interrupts, at
            // the same geometry. The Bedingung column runs on across it, so
            // the entry the previous page left open keeps collecting; a
            // header at a different geometry opens a different table.
            let page_break = table.as_ref().is_some_and(|t| t.desc_x == desc_x);
            if !page_break {
                commit_condition(&mut cur_cond, &mut doc.conditions);
                cur_cond = None;
            }
            table = Some(Table {
                columns: Vec::new(),
                desc_x,
                bedingung_x,
                tol: 6,
                targets: Vec::new(),
                header_done: false,
                pending: vec![line.clone()],
                pid_line: None,
            });
            last_row = Vec::new();
            last_el = Vec::new();
            last = Last::None;
            continue;
        }
        if let Some(c) = heading.captures(line)
            // A chapter heading starts at the margin; a Bedingung wrapping onto
            // a line that opens with a date (`01.08.2025 für …`) does not.
            && table
                .as_ref()
                .is_none_or(|t| c.get(1).map_or(0, |m| line[..m.start()].chars().count()) + 4 < t.desc_x)
        {
            if table.as_ref().is_none_or(|t| t.header_done) {
                chapter = Some(format!("{} {}", &c[1], collapse(&c[2])));
            }
            if table.as_ref().is_some_and(|t| t.header_done) {
                // Prose between tables.
                flush_pending(&mut pending_status, &mut doc);
                table = None;
            }
            continue;
        }
        let Some(tb) = table.as_mut() else { continue };

        // ── Table header: names, roles, Prüfidentifikatoren ──────────────
        if !tb.header_done {
            // The Bedingung column keeps printing next to the repeated header
            // a page break inserts: the text there continues the entry the
            // break interrupted, or opens the next ones.
            if tb.bedingung_x != usize::MAX && t != "Bedingung" {
                collect_condition(
                    line,
                    tb.bedingung_x,
                    &cond_id,
                    &mut cur_cond,
                    &mut doc.conditions,
                );
            }
            if t == "Bedingung" || t.ends_with(" Bedingung") {
                if let Some(x) = char_pos(line, "Bedingung") {
                    tb.bedingung_x = x;
                }
                if t == "Bedingung" {
                    continue;
                }
            }
            if t.starts_with("Prüfidentifikator") {
                // The header line — a segment-group name that happens to read
                // „Prüfidentifikator" carries no five-digit codes.
                if tokens(line)
                    .iter()
                    .any(|k| k.text.len() == 5 && k.text.chars().all(|c| c.is_ascii_digit()))
                {
                    tb.pid_line = Some(line.clone());
                }
                continue;
            }
            // The first row — a segment row (UNH) carrying a status in every
            // column, or after a page break a group or data-element row —
            // ends the header; a Prüfidentifikator line fixes the columns,
            // otherwise the row's own status tokens do.
            let first_row_end = seg_row
                .captures(line)
                .map(|c| c.get(3).map_or(0, |m| m.end()))
                .or_else(|| {
                    de_row
                        .captures(line)
                        .map(|c| c.get(3).map_or(0, |m| m.end()))
                })
                .or_else(|| {
                    group_row
                        .captures(line)
                        .filter(|c| {
                            c.get(1).map_or(0, |m| line[..m.start()].chars().count()) + 4
                                < tb.desc_x
                        })
                        .map(|c| c.get(1).map_or(0, |m| m.end()))
                });
            if let Some(rest) = first_row_end {
                let base = line[..rest].chars().count();
                let mut xs: Vec<usize> = tokens(&line[rest..])
                    .into_iter()
                    .filter(|k| {
                        base + k.x + 1 < tb.bedingung_x
                            && matches!(k.text, "Muss" | "Soll" | "Kann" | "X" | "M" | "S" | "K")
                    })
                    .map(|k| rendered_centre(base + k.x, k.text))
                    .collect();
                if let Some(pl) = &tb.pid_line {
                    let pids: Vec<_> = tokens(pl)
                        .into_iter()
                        .filter(|k| k.text.len() == 5 && k.text.chars().all(|c| c.is_ascii_digit()))
                        .collect();
                    if !pids.is_empty() {
                        xs = pids.iter().map(|k| rendered_centre(k.x, k.text)).collect();
                    }
                    tb.columns = xs
                        .iter()
                        .map(|&x| Column {
                            x,
                            pid: None,
                            name: String::new(),
                            communication: String::new(),
                        })
                        .collect();
                    tb.tol = column_tolerance(&tb.columns, tb.bedingung_x);
                    for k in pids {
                        if let Some(ci) = nearest(tb, rendered_centre(k.x, k.text)) {
                            tb.columns[ci].pid = k.text.parse().ok();
                        }
                    }
                } else {
                    if !prev_columns.is_empty() && xs.len() < prev_columns.len() {
                        xs = prev_columns.clone();
                    }
                    tb.columns = xs
                        .iter()
                        .map(|&x| Column {
                            x,
                            pid: None,
                            name: String::new(),
                            communication: String::new(),
                        })
                        .collect();
                    tb.tol = column_tolerance(&tb.columns, tb.bedingung_x);
                }
                prev_columns = tb.columns.iter().map(|c| c.x).collect();
                if tb.columns.is_empty() {
                    table = None;
                    continue;
                }
                if tb.bedingung_x == usize::MAX {
                    tb.bedingung_x = tb
                        .columns
                        .last()
                        .map_or(usize::MAX, |c| c.x + 2 * tb.tol + 6);
                }
                let pending = std::mem::take(&mut tb.pending);
                for h in &pending {
                    let hc = collapse(h);
                    let ht = hc.as_str();
                    if ht.starts_with("Kommunikation von") {
                        let from = tokens(h)
                            .iter()
                            .find(|k| k.text == "von")
                            .map_or(0, |k| k.x + 3);
                        assign_header_text(tb, h, from, |c, s| {
                            if !c.communication.is_empty() {
                                c.communication.push(' ');
                            }
                            c.communication.push_str(s);
                        });
                    } else {
                        let from = if ht.starts_with("EDIFACT Struktur") {
                            tb.desc_x + "Beschreibung".len()
                        } else {
                            tb.desc_x + 8
                        };
                        assign_header_text(tb, h, from, |c, s| {
                            if s != "Bedingung" {
                                join_name(&mut c.name, s);
                            }
                        });
                    }
                }
                let n = tb.columns.len();
                last_row = vec![None; n];
                last_el = vec![None; n];
                let previous = std::mem::take(&mut prev_targets);
                finish_header(tb, &chapter, &mut doc, &mut by_key);
                prev_targets = tb.targets.clone();
                // A page break inside a table keeps the segment the next DE
                // rows belong to; a different set of columns is a new table.
                if previous != tb.targets {
                    cur_nr = None;
                    cur_group = None;
                    cur_de = None;
                    de_occ.clear();
                }
                // fall through: the row itself is parsed below
            } else {
                // Column names sit under their columns; a line that starts in
                // the description area is a segment-group caption, not a name.
                let starts_right = tokens(line).first().is_some_and(|k| k.x >= tb.desc_x + 10);
                if !t.is_empty() && (starts_right || t.starts_with("Kommunikation von")) {
                    tb.pending.push(line.clone());
                }
                continue;
            }
        }

        // ── Condition column ─────────────────────────────────────────────
        // Collected per row kind below, after the status expression that may
        // overflow into it has been read.

        // ── Rows ─────────────────────────────────────────────────────────
        if let Some(c) = seg_row.captures(line)
            // The tag sits in the Struktur column; `PID 27002` in a Bedingung
            // text does not.
            && c.get(2).map_or(0, |m| line[..m.start()].chars().count()) + 4 < tb.desc_x
        {
            let group = c.get(1).map(|m| m.as_str().to_owned());
            let nr = c[3].to_owned();
            cur_nr = Some(nr.clone());
            cur_group = group;
            de_occ.clear();
            cur_de = None;
            cur_code = None;
            let (cells, expr_end) = column_cells(tb, line, c.get(3).map_or(0, |m| m.end()));
            collect_condition(
                line,
                tb.bedingung_x.saturating_sub(1).max(expr_end),
                &cond_id,
                &mut cur_cond,
                &mut doc.conditions,
            );
            let cells = take_pending(&mut pending_status, &mut doc, cells);
            if std::env::var_os("BDEW_DEBUG").is_some() {
                eprintln!(
                    "row {nr} cols={:?} tol={} cells={cells:?} | {}",
                    tb.columns.iter().map(|c| (c.x, c.pid)).collect::<Vec<_>>(),
                    tb.tol,
                    line.trim()
                );
            }
            for (ci, cell) in cells.into_iter().enumerate() {
                let target = tb.targets[ci];
                let af = &mut doc.anwendungsfaelle[target];
                if let Some(status) = cell {
                    af.rows.push(Row {
                        nr: Some(nr.clone()),
                        group: None,
                        before: None,
                        status: vec![status],
                    });
                    last_row[ci] = Some(af.rows.len() - 1);
                } else {
                    last_row[ci] = None;
                }
                last_el[ci] = None;
            }
            last = Last::Row;
            continue;
        }
        if let Some(c) = de_row.captures(line) {
            let Some(nr) = cur_nr.clone() else { continue };
            let de = c[3].to_owned();
            let occ = *de_occ
                .entry(de.clone())
                .and_modify(|n| *n += 1)
                .or_insert(0);
            cur_de = Some((de.clone(), occ));
            let rest_start = c.get(3).map_or(0, |m| m.end());
            let seg = by_nr.get(nr.as_str()).copied();
            let (code, cell_start) = leading_code(line, rest_start, tb.desc_x, seg, &de, occ);
            cur_code = code.clone();
            let (cells, expr_end) = column_cells(tb, line, cell_start);
            collect_condition(
                line,
                tb.bedingung_x.saturating_sub(1).max(expr_end),
                &cond_id,
                &mut cur_cond,
                &mut doc.conditions,
            );
            let cells = take_pending(&mut pending_status, &mut doc, cells);
            for (ci, cell) in cells.into_iter().enumerate() {
                let target = tb.targets[ci];
                let af = &mut doc.anwendungsfaelle[target];
                if let Some(op) = cell {
                    let idx = element_rule_index(af, &nr, &de, occ);
                    af.elements[idx].operands.push(Operand {
                        code: code.clone(),
                        operand: op,
                    });
                    last_el[ci] = Some(idx);
                } else {
                    last_el[ci] = None;
                }
            }
            last = Last::Element;
            continue;
        }
        if let Some(c) = group_row.captures(line)
            && c.get(1).map_or(0, |m| line[..m.start()].chars().count()) + 4 < tb.desc_x
        {
            let group = c[1].to_owned();
            let (cells, expr_end) = column_cells(tb, line, c.get(1).map_or(0, |m| m.end()));
            collect_condition(
                line,
                tb.bedingung_x.saturating_sub(1).max(expr_end),
                &cond_id,
                &mut cur_cond,
                &mut doc.conditions,
            );
            let cells = take_pending(&mut pending_status, &mut doc, cells);
            for (ci, cell) in cells.into_iter().enumerate() {
                let target = tb.targets[ci];
                let af = &mut doc.anwendungsfaelle[target];
                if let Some(status) = cell {
                    af.rows.push(Row {
                        nr: None,
                        group: Some(group.clone()),
                        before: None,
                        status: vec![status],
                    });
                    last_row[ci] = Some(af.rows.len() - 1);
                } else {
                    last_row[ci] = None;
                }
                last_el[ci] = None;
            }
            cur_group = Some(group);
            last = Last::Row;
            continue;
        }
        let _ = &cur_group;

        // ── Continuations: code rows and wrapped status cells ────────────
        let toks = tokens(line);
        let Some(first) = toks.first() else { continue };
        if first.x >= tb.bedingung_x.saturating_sub(1) {
            collect_condition(
                line,
                tb.bedingung_x.saturating_sub(1),
                &cond_id,
                &mut cur_cond,
                &mut doc.conditions,
            );
            continue;
        }
        // A code of the current data element on its own row sits in the code
        // column, at the start of the Beschreibung column.
        let in_code_column = first.x + 8 >= tb.desc_x && first.x < tb.desc_x + 8;
        if let (true, Some(nr), Some((de, occ))) = (in_code_column, cur_nr.clone(), cur_de.clone())
            && let Some(seg) = by_nr.get(nr.as_str())
            && admits_code(seg, &de, occ, first.text)
        {
            cur_code = Some(first.text.to_owned());
            let (cells, expr_end) = column_cells(
                tb,
                line,
                byte_end(line, first.x + first.text.chars().count()),
            );
            collect_condition(
                line,
                tb.bedingung_x.saturating_sub(1).max(expr_end),
                &cond_id,
                &mut cur_cond,
                &mut doc.conditions,
            );
            let cells = take_pending(&mut pending_status, &mut doc, cells);
            for (ci, cell) in cells.into_iter().enumerate() {
                let target = tb.targets[ci];
                let af = &mut doc.anwendungsfaelle[target];
                if let Some(op) = cell {
                    let idx = element_rule_index(af, &nr, &de, occ);
                    af.elements[idx].operands.push(Operand {
                        code: Some(first.text.to_owned()),
                        operand: op,
                    });
                    last_el[ci] = Some(idx);
                } else {
                    // The code's operands may follow on the next line
                    // (`E_0257` / `EBD Nr. E_0257  X  X`).
                    last_el[ci] = None;
                }
            }
            last = Last::Element;
            continue;
        }
        // Wrapped status / operand cells: only status-shaped tokens in the
        // column region. They are buffered: the label of a two-line cell can
        // sit on the line after its first half.
        let region: Vec<_> = toks
            .iter()
            .filter(|k| k.x + 1 >= tb.desc_x + 10 && k.x + 1 < tb.bedingung_x)
            .collect();
        if region.is_empty() || !region.iter().all(|k| is_status_token(k.text)) {
            collect_condition(
                line,
                tb.bedingung_x.saturating_sub(1),
                &cond_id,
                &mut cur_cond,
                &mut doc.conditions,
            );
            if first.x < tb.desc_x + 10 {
                // A caption in the description column separates rows.
                flush_pending(&mut pending_status, &mut doc);
            }
            continue;
        }
        let (cells, expr_end) = column_cells(tb, line, 0);
        collect_condition(
            line,
            tb.bedingung_x.saturating_sub(1).max(expr_end),
            &cond_id,
            &mut cur_cond,
            &mut doc.conditions,
        );
        let entries = cells
            .iter()
            .enumerate()
            .filter_map(|(ci, cell)| cell.as_ref().map(|c| (ci, c.clone())))
            .map(|(ci, cell)| PendingEntry {
                target: tb.targets[ci],
                row: last_row[ci],
                element: last_el[ci],
                cell,
            })
            .collect();
        pending_status.push(Pending {
            entries,
            kind: last,
            cells,
            nr: cur_nr.clone(),
            group: if matches!(last, Last::Row) && cur_nr.is_none() {
                cur_group.clone()
            } else {
                None
            },
            de: cur_de.clone(),
            code: cur_code.clone(),
        });
    }
    flush_pending(&mut pending_status, &mut doc);

    commit_condition(&mut cur_cond, &mut doc.conditions);
    if doc.anwendungsfaelle.is_empty() {
        return Err("no Anwendungsfall table found".into());
    }
    for af in &mut doc.anwendungsfaelle {
        resolve_group_before(af);
        infer_missing_statuses(af, &by_nr);
    }
    Ok(doc)
}

/// A segment row printed without a status in a column that marks the
/// segment's data elements (INVOIC `SG27 MOA` 00041) is in use: the MIG's
/// status stands in for the missing one.
fn infer_missing_statuses(af: &mut Anwendungsfall, by_nr: &BTreeMap<&str, &SegmentNode>) {
    let nrs: Vec<String> = af.elements.iter().map(|e| e.nr.clone()).collect();
    for nr in nrs {
        if af.rows.iter().any(|r| r.nr.as_deref() == Some(nr.as_str())) {
            continue;
        }
        let Some(seg) = by_nr.get(nr.as_str()) else {
            continue;
        };
        let status = if matches!(seg.status.as_str(), "M" | "R") {
            "Muss"
        } else {
            "Kann"
        };
        eprintln!(
            "warn    {}: {} {} has data-element operands but no status; {status} taken from the MIG",
            af.pid.map_or_else(|| af.name.clone(), |p| p.to_string()),
            seg.tag,
            nr
        );
        af.rows.push(Row {
            nr: Some(nr),
            group: None,
            before: None,
            status: vec![status.to_owned()],
        });
    }
}

/// A buffered status-only line.
struct Pending {
    entries: Vec<PendingEntry>,
    kind: Last,
    cells: Vec<Option<String>>,
    /// The row context, for a status whose row has no cell of its own yet.
    nr: Option<String>,
    group: Option<String>,
    de: Option<(String, u8)>,
    code: Option<String>,
}

struct PendingEntry {
    target: usize,
    row: Option<usize>,
    element: Option<usize>,
    cell: String,
}

/// Append buffered status lines to the rows they followed.
fn flush_pending(pending: &mut Vec<Pending>, doc: &mut AhbDoc) {
    for p in pending.drain(..) {
        for e in p.entries {
            let af = &mut doc.anwendungsfaelle[e.target];
            match p.kind {
                Last::Row => match e.row {
                    Some(ri) => append_status(&mut af.rows[ri].status, &e.cell),
                    // A row printed with its statuses on the next line.
                    None if p.nr.is_some() || p.group.is_some() => af.rows.push(Row {
                        nr: p.nr.clone(),
                        group: p.group.clone(),
                        before: None,
                        status: vec![e.cell.clone()],
                    }),
                    None => {}
                },
                Last::Element => match (e.element, &p.nr, &p.de) {
                    (Some(ei), _, _) => append_operand(&mut af.elements[ei].operands, &e.cell),
                    // A code entry whose operands sit on the next line.
                    (None, Some(nr), Some((de, occ))) => {
                        let idx = element_rule_index(af, nr, de, *occ);
                        af.elements[idx].operands.push(Operand {
                            code: p.code.clone(),
                            operand: e.cell.clone(),
                        });
                    }
                    _ => {}
                },
                Last::None => {}
            }
        }
    }
}

/// Decide where buffered status lines belong once a labelled row arrives:
/// when that row's own cells are empty or begin mid-expression, the buffer
/// is the first half of its cells; otherwise it belonged to the row before.
fn take_pending(
    pending: &mut Vec<Pending>,
    doc: &mut AhbDoc,
    cells: Vec<Option<String>>,
) -> Vec<Option<String>> {
    if pending.is_empty() {
        return cells;
    }
    let fragment = cells.iter().flatten().all(|c| {
        c.starts_with('[')
            || c.starts_with('∧')
            || c.starts_with('∨')
            || c.starts_with('⊻')
            || c.starts_with(')')
    });
    let all_empty = cells.iter().all(Option::is_none);
    if !(fragment || all_empty) {
        flush_pending(pending, doc);
        return cells;
    }
    let mut merged: Vec<Option<String>> = vec![None; cells.len()];
    for p in pending.drain(..) {
        for (ci, cell) in p.cells.into_iter().enumerate() {
            if let Some(c) = cell
                && ci < merged.len()
            {
                match &mut merged[ci] {
                    Some(m) => {
                        m.push(' ');
                        m.push_str(&c);
                    }
                    None => merged[ci] = Some(c),
                }
            }
        }
    }
    for (ci, cell) in cells.into_iter().enumerate() {
        if let Some(c) = cell {
            match &mut merged[ci] {
                Some(m) => {
                    m.push(' ');
                    m.push_str(&c);
                }
                None => merged[ci] = Some(c),
            }
        }
    }
    merged
}

/// Statuses wrap onto continuation lines as either a second AHB status
/// (`Soll [3]` under `Muss [2]`) or the tail of a long condition expression
/// (`[41]` under `Muss [7] ∧`). A tail continues the last status; a new
/// status word starts one.
fn append_status(statuses: &mut Vec<String>, cell: &str) {
    let starts_new =
        cell.starts_with("Muss") || cell.starts_with("Soll") || cell.starts_with("Kann");
    match statuses.last_mut() {
        Some(last) if !starts_new => {
            last.push(' ');
            last.push_str(cell);
        }
        _ => statuses.push(cell.to_owned()),
    }
}

/// A further operand of the same element (`S [165]` under `M [212]`) is its
/// own entry; a bare condition tail continues the last one.
fn append_operand(operands: &mut Vec<Operand>, cell: &str) {
    let starts_new = matches!(cell.split_whitespace().next(), Some("X" | "M" | "S" | "K"));
    match operands.last_mut() {
        Some(last) if !starts_new => {
            last.operand.push(' ');
            last.operand.push_str(cell);
        }
        Some(last) => {
            let code = last.code.clone();
            operands.push(Operand {
                code,
                operand: cell.to_owned(),
            });
        }
        None => operands.push(Operand {
            code: None,
            operand: cell.to_owned(),
        }),
    }
}

fn is_status_token(t: &str) -> bool {
    matches!(
        t,
        "Muss" | "Soll" | "Kann" | "X" | "x" | "M" | "S" | "K" | "∧" | "∨" | "⊻" | "(" | ")"
    ) || (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with("([") && t.ends_with(']'))
        || (t.starts_with('[') && t.ends_with("])"))
        || t.starts_with("Muss")
        || t.starts_with("Soll")
        || t.starts_with("Kann")
}

/// The byte offset of character `char_idx`.
fn byte_end(line: &str, char_idx: usize) -> usize {
    line.char_indices()
        .nth(char_idx)
        .map_or(line.len(), |(b, _)| b)
}

fn column_tolerance(cols: &[Column], bedingung_x: usize) -> usize {
    let mut xs: Vec<usize> = cols.iter().map(|c| c.x).collect();
    xs.push(bedingung_x);
    let min_gap = xs
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]))
        .min()
        .unwrap_or(24);
    (min_gap / 2).clamp(6, 14)
}

fn nearest(tb: &Table, x: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, c) in tb.columns.iter().enumerate() {
        let d = c.x.abs_diff(x);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.filter(|(d, _)| *d <= tb.tol + 4).map(|(_, i)| i)
}

/// Hand each word of a header line to the column whose centre is nearest the
/// word's centre. Header text is centred under its column and adjacent
/// columns' names touch, so words — not phrases — are the unit; a word one
/// space after its predecessor stays with the predecessor's column on a tie.
fn assign_header_text(
    tb: &mut Table,
    line: &str,
    from_x: usize,
    mut f: impl FnMut(&mut Column, &str),
) {
    let mut prev: Option<(usize, usize)> = None; // (end, column)
    let mut parts: Vec<Vec<&str>> = vec![Vec::new(); tb.columns.len()];
    for k in tokens(line) {
        let end = rendered_end(k.x, k.text);
        if k.x < from_x || k.x + 1 >= tb.bedingung_x {
            prev = None;
            continue;
        }
        let centre = rendered_centre(k.x, k.text);
        let mut best: Option<(usize, usize)> = None; // (distance, column)
        for (i, c) in tb.columns.iter().enumerate() {
            let d = c.x.abs_diff(centre);
            let wins = match best {
                None => true,
                Some((bd, bi)) => {
                    d < bd
                        || (d == bd
                            && prev.is_some_and(|(pe, pc)| pc == i && k.x <= pe + 1)
                            && bi != i)
                }
            };
            if wins {
                best = Some((d, i));
            }
        }
        let Some((_, ci)) = best else { continue };
        parts[ci].push(k.text);
        prev = Some((end, ci));
    }
    for (ci, words) in parts.into_iter().enumerate() {
        if !words.is_empty() {
            f(&mut tb.columns[ci], &words.join(" "));
        }
    }
}

/// Join a wrapped name fragment: `Abschlags-` + `rechnung` → `Abschlagsrechnung`.
fn join_name(name: &mut String, frag: &str) {
    if name.is_empty() {
        name.push_str(frag);
    } else if name.ends_with('-') && frag.chars().next().is_some_and(char::is_lowercase) {
        name.pop();
        name.push_str(frag);
    } else {
        name.push(' ');
        name.push_str(frag);
    }
}

fn finish_header(
    tb: &mut Table,
    chapter: &Option<String>,
    doc: &mut AhbDoc,
    by_key: &mut BTreeMap<String, usize>,
) {
    tb.header_done = true;
    tb.targets = tb
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            // Without Prüfidentifikatoren the columns keep their order from
            // table to table, and only the first table names them.
            let key = c
                .pid
                .map_or_else(|| format!("col:{i}"), |p| format!("pid:{p}"));
            *by_key.entry(key).or_insert_with(|| {
                doc.anwendungsfaelle.push(Anwendungsfall {
                    pid: c.pid,
                    name: c.name.clone(),
                    communication: Some(c.communication.clone()).filter(|s| !s.is_empty()),
                    chapter: chapter.clone(),
                    rows: Vec::new(),
                    elements: Vec::new(),
                });
                doc.anwendungsfaelle.len() - 1
            })
        })
        .collect();
}

/// The status text printed under each column on this line, from byte offset
/// `from` on, and the grid column where the last status glyph ends.
///
/// A cell is a status word with the brackets and operators that follow it
/// (or, on a continuation line, a run of brackets and operators). Cells are
/// centred under their column, so each is assigned by its centre — a long
/// expression's first word can start well left of the column it belongs to.
/// An expression may run past the Bedingung column's left edge; it ends at
/// the first `[n]` that is followed by prose rather than by an operator.
fn column_cells(tb: &mut Table, line: &str, from: usize) -> (Vec<Option<String>>, usize) {
    let mut cells: Vec<Option<String>> = vec![None; tb.columns.len()];
    let rest = &line[from..];
    let base = line[..from].chars().count();
    let toks = tokens(rest);
    // (start, end, text)
    let mut raw: Vec<(usize, usize, String)> = Vec::new();
    // The rightmost grid column a status glyph occupies. `end` below is a
    // `rendered_end`, which scales a token's width to place its centre under
    // the right column and therefore overshoots the grid; the Bedingung
    // column's own text starts at a real grid position, so the boundary
    // between them has to be measured on the grid.
    let mut grid_end = 0;
    for (i, k) in toks.iter().enumerate() {
        let x = base + k.x;
        if !is_status_token(k.text) {
            if x + 1 >= tb.bedingung_x {
                break;
            }
            continue;
        }
        let is_operator = matches!(k.text, "∧" | "∨" | "⊻" | ")");
        // A `[n]` with prose after it, right of the last column, is the
        // Bedingung column's text and says where that column starts.
        if k.text.starts_with('[')
            && toks.get(i + 1).is_some_and(|n| !is_status_token(n.text))
            && x > tb.columns.last().map_or(0, |c| c.x + tb.tol)
        {
            tb.bedingung_x = tb.bedingung_x.min(x);
            break;
        }
        if x + 1 >= tb.bedingung_x {
            let next_continues = toks
                .get(i + 1)
                .is_some_and(|n| matches!(n.text, "∧" | "∨" | "⊻" | ")"));
            let continues =
                !raw.is_empty() && (is_operator || (k.text.starts_with('[') && next_continues));
            if !continues {
                break;
            }
        }
        let is_word = matches!(
            k.text,
            "Muss" | "Soll" | "Kann" | "X" | "x" | "M" | "S" | "K"
        ) || k.text.starts_with("Muss")
            || k.text.starts_with("Soll")
            || k.text.starts_with("Kann");
        // `x` for `X` is a typo the AHBs carry now and then.
        let word = if k.text == "x" { "X" } else { k.text };
        let end = rendered_end(x, k.text);
        grid_end = grid_end.max(x + k.text.chars().count());
        match raw.last_mut() {
            // One word space on the grid is a cell or two past the glyphs.
            Some((_, last_end, text)) if !is_word && x <= *last_end + 2 => {
                text.push(' ');
                text.push_str(word);
                *last_end = end;
            }
            _ => raw.push((x, end, word.to_owned())),
        }
    }
    for (start, end, text) in raw {
        let centre = (start + end) / 2;
        // A cell is centred under its column; a `[n]` further off is the
        // Bedingung column's own text.
        let Some((ci, _)) = tb
            .columns
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| c.x.abs_diff(centre))
            .filter(|(_, c)| c.x.abs_diff(centre) <= tb.tol + 6)
        else {
            continue;
        };
        match &mut cells[ci] {
            Some(s) => {
                s.push(' ');
                s.push_str(&text);
            }
            None => cells[ci] = Some(text),
        }
    }
    (cells, grid_end)
}

/// Whether the token after the data-element number is a code the MIG admits
/// for that element, and where the operand cells start.
fn leading_code(
    line: &str,
    rest_start: usize,
    desc_x: usize,
    seg: Option<&SegmentNode>,
    de: &str,
    occ: u8,
) -> (Option<String>, usize) {
    let rest = &line[rest_start..];
    let base = line[..rest_start].chars().count();
    let Some(first) = tokens(rest).first().copied() else {
        return (None, rest_start);
    };
    let x = base + first.x;
    if x + 1 < desc_x + 12
        && looks_like_code(first.text)
        && seg.is_some_and(|s| admits_code(s, de, occ, first.text))
    {
        let end = rest_start + byte_end(rest, first.x + first.text.chars().count());
        return (Some(first.text.to_owned()), end);
    }
    (None, rest_start)
}

fn element<'a>(seg: &'a SegmentNode, de: &str, occ: u8) -> Option<&'a super::mig::Element> {
    let mut n = 0u8;
    for el in &seg.elements {
        if el.id == de {
            if n == occ {
                return Some(el);
            }
            n += 1;
        }
        for comp in &el.components {
            if comp.id == de {
                if n == occ {
                    return Some(comp);
                }
                n += 1;
            }
        }
    }
    None
}

/// Whether `token` is a code entry of data element `de`: one the MIG lists,
/// or — where the MIG lists none — a Prüfidentifikator, which the AHB
/// enumerates as codes of `RFF+Z13` DE 1154.
fn admits_code(seg: &SegmentNode, de: &str, occ: u8, token: &str) -> bool {
    element(seg, de, occ)
        .or_else(|| element(seg, de, 0))
        .is_some_and(|el| {
            el.codes.iter().any(|c| c.code == token)
                || (el.codes.is_empty()
                    && token.len() == 5
                    && token.chars().all(|c| c.is_ascii_digit()))
        })
}

fn element_rule_index(af: &mut Anwendungsfall, nr: &str, de: &str, occ: u8) -> usize {
    if let Some(i) = af
        .elements
        .iter()
        .position(|e| e.nr == nr && e.de == de && e.occurrence == occ)
    {
        return i;
    }
    af.elements.push(ElementRule {
        nr: nr.to_owned(),
        de: de.to_owned(),
        occurrence: occ,
        operands: Vec::new(),
    });
    af.elements.len() - 1
}

/// Text in the Bedingung column: `[n]` opens a condition, following text
/// continues it. The longest reading of a number wins — the same Bedingung is
/// printed wherever it is cited, and a page break can cut one short.
fn collect_condition(
    line: &str,
    from_x: usize,
    cond_id: &Regex,
    cur: &mut Option<(String, String)>,
    conditions: &mut BTreeMap<String, String>,
) {
    let toks: Vec<_> = tokens(line).into_iter().filter(|k| k.x >= from_x).collect();
    if toks.is_empty() {
        return;
    }
    let mut buf = String::new();
    for k in toks {
        if let Some(c) = cond_id.captures(k.text) {
            if !buf.is_empty() {
                append_condition(buf.trim(), cur);
                buf.clear();
            }
            commit_condition(cur, conditions);
            *cur = Some((c[1].to_owned(), String::new()));
            conditions.entry(c[1].to_owned()).or_default();
            continue;
        }
        buf.push_str(k.text);
        buf.push(' ');
    }
    if !buf.trim().is_empty() {
        append_condition(buf.trim(), cur);
    }
}

/// Where the Paket table's Paketvoraussetzung and Bedingungen columns start.
#[derive(Debug, Clone, Copy)]
struct PackageTable {
    pv: usize,
    bed: usize,
}

/// Whether a token can be part of a Paketvoraussetzung — a `[n]` reference, one
/// of the operators the AHBs use, or the status word some of them print in
/// front of it (`X [50] ∧ [528]`). Everything else in that column is prose.
fn is_expression_token(t: &str) -> bool {
    let core = t.trim_start_matches('(').trim_end_matches(')');
    matches!(t, "∧" | "∨" | "⊻" | "(" | ")")
        || matches!(t, "Muss" | "Soll" | "Kann" | "X" | "M" | "S" | "K")
        || (core.starts_with('[') && core.ends_with(']'))
}

fn append_condition(text: &str, cur: &mut Option<(String, String)>) {
    let Some((_, acc)) = cur else { return };
    if !acc.is_empty() {
        acc.push(' ');
    }
    acc.push_str(text);
}

fn commit_condition(cur: &mut Option<(String, String)>, conditions: &mut BTreeMap<String, String>) {
    let Some((id, text)) = cur.take() else { return };
    let entry = conditions.entry(id).or_default();
    if text.chars().count() > entry.chars().count() {
        *entry = text;
    }
}

/// A group row carries no `Nr`; it is positioned before the trigger segment
/// that follows it in the same column.
fn resolve_group_before(af: &mut Anwendungsfall) {
    let mut next_nr: Option<String> = None;
    for row in af.rows.iter_mut().rev() {
        match (&row.nr, &row.group) {
            (Some(nr), _) => next_nr = Some(nr.clone()),
            (None, Some(_)) => row.before = next_nr.clone(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mig;
    use super::*;

    fn mig_with(nr: &str, tag: &str, des: &[(&str, &[&str])]) -> MigDoc {
        let elements = des
            .iter()
            .map(|(id, codes)| mig::Element {
                id: (*id).to_owned(),
                name: String::new(),
                status: "M".into(),
                format: None,
                note: String::new(),
                codes: codes
                    .iter()
                    .map(|c| mig::Code {
                        code: (*c).to_owned(),
                        name: String::new(),
                    })
                    .collect(),
                components: Vec::new(),
            })
            .collect();
        MigDoc {
            structure: vec![mig::Node::Segment(SegmentNode {
                nr: nr.to_owned(),
                tag: tag.to_owned(),
                zaehler: String::new(),
                status: "M".into(),
                max: 1,
                name: String::new(),
                elements,
                example: None,
            })],
            envelope: Vec::new(),
        }
    }

    const TABLE: &str = "\
8.2 Anmeldung einer verbrauchenden Marktlokation
 EDIFACT Struktur                 Beschreibung                      Anmeldung Bestätigung Ablehnung Bedingung
                                                                    verb. MaLo Anmeldung Anmeldung
                                                                               verb. MaLo verb. MaLo
                                  Kommunikation von                   LF an NB   NB an LF   NB an LF
                                  Prüfidentifikator                    55001      55002      55003
 Beginn zum
 SG4
 SG4 DTM               00020                                             Muss   Muss [521]              [521] Hinweis: Wenn im
                                                                                                        zweiten DE 9013
 SG4      DTM 2005                92      Datum Vertragsbeginn          X           X
 SG4      DTM 2380                Datum oder Uhrzeit oder            X [UB1]     X [UB1]
                                  Zeitspanne, Wert
 SG4 DTM 2379                     303     CCYYMMDDHHMMZZZ                 X         X
 Ende zum
 SG4
 SG4 DTM               00021                                        Muss [10]   Muss [10]               [10] Wenn SG4
                                                                                                        STS+7++xxx+xxx+E01/E03
 SG4      DTM 2005                93      Datum Vertragsende            X           X
 Kunde des Lieferanten
 SG12                                                                     Muss
 SG12 NAD            00455                                                Muss
 SG12 NAD 3035             Z09           Kunde des LF                      X
 SG12 NAD 3036             Name                                            X
 SG12 NAD 3045             Z01           Struktur von                      X
                                          Personennamen
                           Z02           Struktur der                      X
                                         Firmenbezeichnung
";

    #[test]
    fn columns_are_read_by_position() {
        let mut m = mig_with(
            "00020",
            "DTM",
            &[("2005", &["92"]), ("2380", &[]), ("2379", &["303"])],
        );
        if let mig::Node::Segment(s) = &mut m.structure[0] {
            let _ = s;
        }
        m.structure.push(mig::Node::Segment(SegmentNode {
            nr: "00021".into(),
            tag: "DTM".into(),
            zaehler: String::new(),
            status: "D".into(),
            max: 1,
            name: String::new(),
            elements: vec![mig::Element {
                id: "2005".into(),
                name: String::new(),
                status: "M".into(),
                format: None,
                note: String::new(),
                codes: vec![mig::Code {
                    code: "93".into(),
                    name: String::new(),
                }],
                components: Vec::new(),
            }],
            example: None,
        }));
        m.structure.push(mig::Node::Segment(SegmentNode {
            nr: "00455".into(),
            tag: "NAD".into(),
            zaehler: String::new(),
            status: "M".into(),
            max: 1,
            name: String::new(),
            elements: vec![
                mig::Element {
                    id: "3035".into(),
                    name: String::new(),
                    status: "M".into(),
                    format: None,
                    note: String::new(),
                    codes: vec![mig::Code {
                        code: "Z09".into(),
                        name: String::new(),
                    }],
                    components: Vec::new(),
                },
                mig::Element {
                    id: "3036".into(),
                    name: String::new(),
                    status: "M".into(),
                    format: None,
                    note: String::new(),
                    codes: Vec::new(),
                    components: Vec::new(),
                },
                mig::Element {
                    id: "3045".into(),
                    name: String::new(),
                    status: "M".into(),
                    format: None,
                    note: String::new(),
                    codes: vec![
                        mig::Code {
                            code: "Z01".into(),
                            name: String::new(),
                        },
                        mig::Code {
                            code: "Z02".into(),
                            name: String::new(),
                        },
                    ],
                    components: Vec::new(),
                },
            ],
            example: None,
        }));
        let lines: Vec<String> = TABLE.lines().map(str::to_owned).collect();
        let doc = parse(&lines, &m).unwrap();
        assert_eq!(doc.anwendungsfaelle.len(), 3);
        let a = &doc.anwendungsfaelle[0];
        assert_eq!(a.pid, Some(55001));
        assert_eq!(a.name, "Anmeldung verb. MaLo");
        assert_eq!(a.communication.as_deref(), Some("LF an NB"));
        assert_eq!(
            a.chapter.as_deref(),
            Some("8.2 Anmeldung einer verbrauchenden Marktlokation")
        );
        let rows: Vec<String> = a
            .rows
            .iter()
            .map(|r| {
                format!(
                    "{}{}={}",
                    r.nr.clone().unwrap_or_default(),
                    r.group.clone().unwrap_or_default(),
                    r.status.join("|")
                )
            })
            .collect();
        assert_eq!(
            rows,
            ["00020=Muss", "00021=Muss [10]", "SG12=Muss", "00455=Muss"],
            "{:#?}",
            doc.anwendungsfaelle
        );
        assert_eq!(a.rows[2].before.as_deref(), Some("00455"));
        let b = &doc.anwendungsfaelle[1];
        assert_eq!(b.pid, Some(55002));
        assert_eq!(b.name, "Bestätigung Anmeldung verb. MaLo");
        assert_eq!(b.rows[0].status, ["Muss [521]"]);
        let c = &doc.anwendungsfaelle[2];
        assert!(c.rows.is_empty(), "55003 has no status on any row here");
        // Elements: code operands per column.
        let e2005 = a
            .elements
            .iter()
            .find(|e| e.nr == "00020" && e.de == "2005")
            .unwrap();
        assert_eq!(e2005.operands[0].code.as_deref(), Some("92"));
        assert_eq!(e2005.operands[0].operand, "X");
        let e2380 = a
            .elements
            .iter()
            .find(|e| e.nr == "00020" && e.de == "2380")
            .unwrap();
        assert_eq!(e2380.operands[0].code, None);
        assert_eq!(e2380.operands[0].operand, "X [UB1]");
        let e3045 = a
            .elements
            .iter()
            .find(|e| e.nr == "00455" && e.de == "3045")
            .unwrap();
        let codes: Vec<_> = e3045
            .operands
            .iter()
            .map(|o| o.code.clone().unwrap())
            .collect();
        assert_eq!(codes, ["Z01", "Z02"]);
        assert_eq!(doc.conditions["10"], "Wenn SG4 STS+7++xxx+xxx+E01/E03");
        assert_eq!(doc.conditions["521"], "Hinweis: Wenn im zweiten DE 9013");
    }
}
