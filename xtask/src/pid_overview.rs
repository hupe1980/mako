//! The published Prüfidentifikator inventory, and mako's coverage of it.
//!
//! `validate-profiles` compares one profile release against the previous one,
//! so it proves nothing was *lost* and is blind to a Prüfidentifikator that was
//! never imported at all. This is the other direction, against the only
//! statement of what exists: BDEW's *Anwendungsübersicht der
//! Prüfidentifikatoren*. Where coverage is short, validation answers the
//! `AHB-UNKNOWN-PID` warning and `is_valid()` stays `true`, so the figure has
//! to be computed rather than maintained by hand.
//!
//! Two commands:
//!
//! - **`import-pid-overview`** reads Tabelle 1 of the published PDF — the
//!   binding edition; the .xlsx is informational and serves at most as an
//!   oracle — and writes [`OVERVIEW_PATH`], the one step needing the source
//!   document.
//! - **`check-pid-coverage`** compares that file against the shipped profiles
//!   and the PID reference. It needs nothing but the repository, so it is a
//!   real gate rather than a skip on a runner without `regulatories/`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Where the extracted inventory lives, relative to the workspace root.
pub const OVERVIEW_PATH: &str = "crates/edi-energy/profiles/pid-overview.json";

/// The published Prüfidentifikator reference, relative to the workspace root.
const REFERENCE_DOC: &str = "site/content/docs/regulatory/pid-reference.md";

/// Coverage floor — the check fails below it, so the number can only rise.
///
/// Raise it in the same commit that raises the coverage. It is deliberately not
/// derived from the file it guards: a floor that recomputes itself ratchets
/// downwards as happily as up.
const COVERED_FLOOR: usize = 483;

/// Published Prüfidentifikatoren a crate other than `edi-energy` carries.
///
/// The Anwendungsübersicht lists the two SSQNOT codes beside the BDEW ones,
/// but SSQNOT is a DVGW Nachrichtenbeschreibung (5.7, an `ORDRSP` subset)
/// parsed by `dvgw-edi`; `dvgw_edi::catalogue_for(Ssqnot)` is pinned to this
/// list by the `mako-gabi-gas` tests.
const CARRIED_BY_DVGW_EDI: &[&str] = &["70095", "70096"];

/// Prüfidentifikatoren mako's profiles carry that the overview does not list,
/// each with the reason it stays.
const SHIPPED_NOT_PUBLISHED: &[(&str, &str)] = &[];

/// The published inventory, as extracted from the Anwendungsübersicht.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Overview {
    /// Format of this file.
    pub schema_version: u32,
    /// The document the inventory was extracted from, named so a reader can
    /// fetch the same edition.
    pub source_document: String,
    /// Prüfidentifikatoren per AHB, exactly as the overview groups them.
    pub ahbs: BTreeMap<String, BTreeSet<String>>,
    /// Which Sparten each Prüfidentifikator runs in, from the overview's own
    /// „Sparte Strom" and „Sparte Gas" columns.
    ///
    /// A PID appears on several rows — one per Prozessschritt — and the columns
    /// are per row, so a PID counts as running in a Sparte when **any** of its
    /// rows marks it. That is the question consumers actually ask: „can this
    /// Prüfidentifikator arrive in a Gas interchange?"
    ///
    /// Read off the overview rather than hand-maintained, because a wrongly
    /// classified PID silently suppresses or misroutes a mandatory CONTRL.
    #[serde(default)]
    pub sparten: BTreeMap<String, Sparten>,
    /// Where each Prüfidentifikator's assignment to a preceding
    /// Geschäftsvorfall is checked — the overview's „Zuordnung zu einem
    /// Geschäftsvorfall" and „Stelle der Zuordnungsprüfung" columns.
    ///
    /// `V` puts the check into the Verarbeitbarkeitsprüfung, so a failed
    /// assignment is answered with an APERAK (`Z33`); `E` puts it into the
    /// Entscheidungsbaum (APERAK AHB 1.1 Kap. 2.1.3, S. 14). Only the rows that
    /// name a Geschäftsvorfall tuple count.
    #[serde(default)]
    pub zuordnung: BTreeMap<String, Zuordnung>,
}

/// One Prüfidentifikator's assignment to a preceding Geschäftsvorfall.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Zuordnung {
    /// At least one row names a „Zuordnung zu einem Geschäftsvorfall" tuple.
    pub geschaeftsvorfall: bool,
    /// The „Stelle der Zuordnungsprüfung" values of those rows (`V`, `E`).
    pub stelle: BTreeSet<String>,
}

/// The Sparten one Prüfidentifikator runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sparten {
    /// The overview marks at least one of this PID's rows „Sparte Strom".
    pub strom: bool,
    /// The overview marks at least one of this PID's rows „Sparte Gas".
    pub gas: bool,
}

impl Overview {
    /// Every published Prüfidentifikator, across all AHBs.
    fn all(&self) -> BTreeSet<&String> {
        self.ahbs.values().flatten().collect()
    }
}

// ── import ───────────────────────────────────────────────────────────────────

/// Sheet of the workbook carrying one row per (Prüfidentifikator, Prozessschritt).
const SHEET: &str = "Prüf-ID Prozessschritt";
/// Column holding the AHB a Prüfidentifikator belongs to.
const COL_AHB: &str = "B";
/// Column holding the Prüfidentifikator.
const COL_PID: &str = "D";
/// Column marking the row as running in Sparte Strom (`X`, else `--`).
const COL_STROM: &str = "R";
/// Column marking the row as running in Sparte Gas (`X`, else `--`).
const COL_GAS: &str = "S";
/// Column naming the „Zuordnung zu einem Geschäftsvorfall" tuple (`--` for none).
const COL_ZUORDNUNG_GV: &str = "N";
/// Column „Stelle der Zuordnungsprüfung" (`V`, `E`, `--`).
const COL_STELLE: &str = "Q";
/// The headers the columns above must carry, so a reshuffled workbook fails
/// the import rather than filling a field from the wrong column.
const HEADERS: &[(&str, &str)] = &[
    (COL_AHB, "AHB"),
    (COL_PID, "Prüfidentifikator"),
    (COL_ZUORDNUNG_GV, "Zuordnung zu einem Geschäftsvorfall"),
    (COL_STELLE, "Stelle der Zuordnungs-prüfung"),
    (COL_STROM, "Sparte Strom"),
    (COL_GAS, "Sparte Gas"),
];

/// One row of Tabelle 1 „Prüfidentifikator zu Prozessschritt" that names a
/// Prüfidentifikator, reduced to the columns the inventory keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceRow {
    ahb: String,
    pid: String,
    /// „Zuordnung zu einem Geschäftsvorfall" — a tuple, or `--` for none.
    geschaeftsvorfall: String,
    /// „Stelle der Zuordnungsprüfung" — `V`, `E` or `--`.
    stelle: String,
    strom: bool,
    gas: bool,
}

/// The inventory's three maps: PIDs per AHB, Sparten per PID, Zuordnung per PID.
type Inventory = (
    BTreeMap<String, BTreeSet<String>>,
    BTreeMap<String, Sparten>,
    BTreeMap<String, Zuordnung>,
);

/// Fold the rows into the inventory's three maps.
fn inventory(rows: &[SourceRow]) -> Inventory {
    let mut ahbs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut sparten: BTreeMap<String, Sparten> = BTreeMap::new();
    let mut zuordnung: BTreeMap<String, Zuordnung> = BTreeMap::new();
    for row in rows {
        ahbs.entry(row.ahb.clone())
            .or_default()
            .insert(row.pid.clone());
        let entry = sparten.entry(row.pid.clone()).or_insert(Sparten {
            strom: false,
            gas: false,
        });
        entry.strom |= row.strom;
        entry.gas |= row.gas;

        let z = zuordnung.entry(row.pid.clone()).or_default();
        let tuple = row.geschaeftsvorfall.trim();
        if !tuple.is_empty() && tuple != "--" {
            z.geschaeftsvorfall = true;
            let stelle = row.stelle.trim();
            if !stelle.is_empty() && stelle != "--" {
                z.stelle.insert(stelle.to_owned());
            }
        }
    }
    (ahbs, sparten, zuordnung)
}

/// The rows of the workbook's sheet [`SHEET`].
fn xlsx_rows(xlsx: &Path) -> Result<Vec<SourceRow>, String> {
    let bytes = std::fs::read(xlsx).map_err(|e| format!("read {}: {e}", xlsx.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("{} is not a readable xlsx: {e}", xlsx.display()))?;

    let shared = shared_strings(&mut zip)?;
    let sheet_part = sheet_part(&mut zip, SHEET)?;
    let sheet_xml = entry(&mut zip, &sheet_part)?;

    let mut out = Vec::new();
    let mut header_seen = false;
    for row in rows(&sheet_xml) {
        let cells = cells(row, &shared);
        if !header_seen
            && cells
                .get(COL_PID)
                .is_some_and(|v| v.trim() == "Prüfidentifikator")
        {
            header_seen = true;
            for (col, want) in HEADERS {
                let got = cells.get(*col).map_or("", |v| v.trim());
                if got != *want {
                    return Err(format!(
                        "column {col} of sheet {SHEET:?} is headed {got:?}, not {want:?} — \
                         the workbook layout changed"
                    ));
                }
            }
            continue;
        }
        let (Some(ahb), Some(pid)) = (cells.get(COL_AHB), cells.get(COL_PID)) else {
            continue;
        };
        let (ahb, pid) = (ahb.trim(), pid.trim());
        // The header row and the layout notes both fail this.
        if ahb.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) || pid.is_empty() {
            continue;
        }
        // `X` marks the Sparte; the overview writes `--` for "not this one".
        let marked = |col| {
            cells
                .get(col)
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("x"))
        };
        let text = |col| {
            cells
                .get(col)
                .map_or(String::new(), |v| v.trim().to_owned())
        };
        out.push(SourceRow {
            ahb: ahb.to_owned(),
            pid: pid.to_owned(),
            geschaeftsvorfall: text(COL_ZUORDNUNG_GV),
            stelle: text(COL_STELLE),
            strom: marked(COL_STROM),
            gas: marked(COL_GAS),
        });
    }
    if !header_seen {
        return Err(format!(
            "no header row in sheet {SHEET:?} — has the workbook layout changed?"
        ));
    }
    Ok(out)
}

/// The rows of Tabelle 1 in the binding PDF, read with `pdftotext -bbox`.
fn pdf_rows(pdf: &Path) -> Result<Vec<SourceRow>, String> {
    let out = std::process::Command::new("pdftotext")
        .arg("-bbox")
        .arg(pdf)
        .arg("-")
        .output()
        .map_err(|e| format!("pdftotext (poppler-utils) is needed to read the PDF: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pdftotext {}: {}",
            pdf.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    bbox_rows(&String::from_utf8_lossy(&out.stdout))
}

/// One positioned word of a `pdftotext -bbox` page.
struct Word {
    x: f64,
    y: f64,
    text: String,
}

/// Where a word sits in Tabelle 1, by its column's left edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    LfdNr,
    Ahb,
    Pid,
    Geschaeftsvorfall,
    Stelle,
    Strom,
    Gas,
    /// Every column the inventory does not keep.
    Other,
}

/// The header words of Tabelle 1 that open a column, in page order, with the
/// column each opens. `Zuordnung` opens two: „… zu einem Objekt" first,
/// „… zu einem Geschäftsvorfall" second. A header the page lacks fails the
/// read rather than shift every column after it.
const PDF_HEADER: &[(&str, Col)] = &[
    ("AHB", Col::Ahb),
    ("Beschreibung", Col::Other),
    ("Prüfident", Col::Pid),
    ("Reaktion", Col::Other),
    ("Zuordnung", Col::Other),
    ("Zuordnung", Col::Geschaeftsvorfall),
    ("Erweiter", Col::Other),
    ("Stelle", Col::Stelle),
    ("Strom", Col::Strom),
    ("Gas", Col::Gas),
    ("Übertra", Col::Other),
];

/// Slack, in points, by which a cell may start left of its column's header
/// word: the 12.08.2026 Lesefassung sets some „Sparte Strom" `--` at x 1015
/// under a header at 1020, while the „Stelle" values beside it stop at 1000.
const COLUMN_SLACK: f64 = 6.0;
/// Slack, in points, by which a row's first line may sit above its „Lfd. Nr.".
const ROW_SLACK: f64 = 3.0;

/// Read every Tabelle 1 page of a `pdftotext -bbox` document.
///
/// Tabelle 1 is a landscape table titled „Prüfidentifikator zu Prozessschritt"
/// whose header line opens with „Lfd. Nr."; other pages (the Einleitung, the
/// other tables) are skipped. A row
/// starts at a numeric „Lfd. Nr." and runs to the next one or to the page's
/// footer rule; a cell that wraps adds its lines to the row.
fn bbox_rows(html: &str) -> Result<Vec<SourceRow>, String> {
    let word = regex::Regex::new(
        r#"<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="[\d.]+" yMax="[\d.]+">([^<]*)</word>"#,
    )
    .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut tabelle_1_pages = 0usize;
    for (page_no, page) in html.split("<page ").skip(1).enumerate() {
        let words: Vec<Word> = word
            .captures_iter(page)
            .filter_map(|c| {
                Some(Word {
                    x: c[1].parse().ok()?,
                    y: c[2].parse().ok()?,
                    text: unescape(&c[3]),
                })
            })
            .collect();
        // Each Tabelle 1 page is titled „Prüfidentifikator zu Prozessschritt"
        // above its rule; the other tables carry „Lfd." headers too.
        let titled = |t: &str| words.iter().any(|w| w.text == t && w.y < 40.0);
        if !(titled("Prüfidentifikator") && titled("Prozessschritt")) {
            continue;
        }
        let Some(header_y) = words
            .iter()
            .find(|w| w.text == "Lfd." && w.y < 150.0)
            .map(|w| w.y)
        else {
            return Err(format!(
                "page {}: a Tabelle 1 page without its „Lfd. Nr.\" header",
                page_no + 1
            ));
        };
        let mut header_line: Vec<&Word> = words
            .iter()
            .filter(|w| (w.y - header_y).abs() < 2.0)
            .collect();
        header_line.sort_by(|a, b| a.x.total_cmp(&b.x));

        // Column edges, matched in page order so the two `Zuordnung` resolve.
        let mut edges: Vec<(f64, Col)> = vec![(0.0, Col::LfdNr)];
        let mut from = 0usize;
        for (label, col) in PDF_HEADER {
            let Some(i) = header_line[from..].iter().position(|w| w.text == *label) else {
                return Err(format!(
                    "page {}: the Tabelle 1 header has no column {label:?} — has the layout changed?",
                    page_no + 1
                ));
            };
            edges.push((header_line[from + i].x - COLUMN_SLACK, *col));
            from += i + 1;
        }
        tabelle_1_pages += 1;
        let column = |x: f64| {
            edges
                .iter()
                .rev()
                .find(|(edge, _)| x >= *edge)
                .map_or(Col::Other, |(_, c)| *c)
        };

        // The footer opens with a rule of box-drawing characters.
        let footer_y = words
            .iter()
            .filter(|w| w.y > header_y + 40.0 && w.text.starts_with('─'))
            .map(|w| w.y)
            .fold(f64::INFINITY, f64::min);
        let ahb_edge = edges[1].0;
        let mut starts: Vec<f64> = words
            .iter()
            .filter(|w| {
                w.x < ahb_edge
                    && w.y > header_y + 40.0
                    && w.y < footer_y
                    && !w.text.is_empty()
                    && w.text.chars().all(|c| c.is_ascii_digit())
            })
            .map(|w| w.y)
            .collect();
        starts.sort_by(f64::total_cmp);

        for (i, &top) in starts.iter().enumerate() {
            let bottom = starts.get(i + 1).copied().unwrap_or(footer_y) - ROW_SLACK;
            let mut cell: BTreeMap<&'static str, Vec<&Word>> = BTreeMap::new();
            for w in words
                .iter()
                .filter(|w| w.y >= top - ROW_SLACK && w.y < bottom)
            {
                let key = match column(w.x) {
                    Col::Ahb => "ahb",
                    Col::Pid => "pid",
                    Col::Geschaeftsvorfall => "gv",
                    Col::Stelle => "stelle",
                    Col::Strom => "strom",
                    Col::Gas => "gas",
                    Col::LfdNr | Col::Other => continue,
                };
                cell.entry(key).or_default().push(w);
            }
            let text = |key: &str| {
                let mut ws = cell.get(key).cloned().unwrap_or_default();
                ws.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
                let mut s = String::new();
                for w in ws {
                    // „Mehr-/" + „Mindermengen" is one word broken at its slash.
                    if !s.is_empty() && !s.ends_with('/') {
                        s.push(' ');
                    }
                    s.push_str(&w.text);
                }
                s
            };
            let pid = text("pid");
            // A Prozessschritt without an Anwendungsfall writes `--` here.
            if pid.len() != 5 || !pid.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            out.push(SourceRow {
                ahb: text("ahb"),
                pid,
                geschaeftsvorfall: text("gv"),
                stelle: text("stelle"),
                strom: text("strom").eq_ignore_ascii_case("x"),
                gas: text("gas").eq_ignore_ascii_case("x"),
            });
        }
    }
    if tabelle_1_pages == 0 {
        return Err(
            "no Tabelle 1 page („Prüfidentifikator zu Prozessschritt\") — has the layout changed?"
                .to_owned(),
        );
    }
    Ok(out)
}

/// The mirrored file's catalogue title, from the mirror's manifest.
fn manifest_title(workspace_root: &Path, file_name: &str) -> Option<String> {
    let raw = std::fs::read_to_string(workspace_root.join("regulatories/bdew-mako/manifest.json"))
        .ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&raw).ok()?;
    manifest["files"][file_name]["title"]
        .as_str()
        .map(str::to_owned)
}

/// Extract the inventory from the binding PDF and write [`OVERVIEW_PATH`].
///
/// Only the PDF: the overview's own Disclaimer (S. 1) says „Die PDF-Datei ist
/// das gültige Dokument." The informational .xlsx of the same Stand may be
/// passed as `oracle`; its rows are compared with the PDF's and every
/// disagreement is reported, but nothing is taken from it.
///
/// # Errors
///
/// When the path is not a PDF, or the PDF cannot be read or carries no
/// Tabelle 1.
pub fn import(workspace_root: &Path, pdf: &Path, oracle: Option<&Path>) -> Result<(), String> {
    if !pdf
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return Err(format!(
            "{} is not the PDF — the Anwendungsübersicht's binding edition is the PDF; \
             pass an .xlsx only as --oracle",
            pdf.display()
        ));
    }
    let bytes = std::fs::read(pdf).map_err(|e| format!("read {}: {e}", pdf.display()))?;
    let sha = {
        use sha2::Digest as _;
        format!("{:x}", sha2::Sha256::digest(&bytes))
    };
    let rows = pdf_rows(pdf)?;
    if rows.is_empty() {
        return Err(format!(
            "no Prüfidentifikator rows in {} — has the layout changed?",
            pdf.display()
        ));
    }
    let (ahbs, sparten, zuordnung) = inventory(&rows);

    if let Some(xlsx) = oracle {
        let (o_ahbs, o_sparten, o_zuordnung) = inventory(&xlsx_rows(xlsx)?);
        let mut disagreements = 0usize;
        for name in ahbs.keys().chain(o_ahbs.keys()).collect::<BTreeSet<_>>() {
            let (p, o) = (ahbs.get(name), o_ahbs.get(name));
            if p != o {
                disagreements += 1;
                eprintln!(
                    "oracle: {name}: PDF {:?} / .xlsx {:?}",
                    p.map(|s| s
                        .difference(o.unwrap_or(&BTreeSet::new()))
                        .cloned()
                        .collect::<Vec<_>>()),
                    o.map(|s| s
                        .difference(p.unwrap_or(&BTreeSet::new()))
                        .cloned()
                        .collect::<Vec<_>>()),
                );
            }
        }
        for (pid, s) in &sparten {
            if o_sparten.get(pid).is_some_and(|o| o != s) {
                disagreements += 1;
                eprintln!(
                    "oracle: {pid} Sparten: PDF {s:?} / .xlsx {:?}",
                    o_sparten[pid]
                );
            }
        }
        for (pid, z) in &zuordnung {
            if o_zuordnung.get(pid).is_some_and(|o| o != z) {
                disagreements += 1;
                eprintln!(
                    "oracle: {pid} Zuordnung: PDF {z:?} / .xlsx {:?}",
                    o_zuordnung[pid]
                );
            }
        }
        println!(
            "import-pid-overview: {disagreements} disagreement(s) with the oracle {}",
            xlsx.display()
        );
    }

    let file_name = pdf
        .file_name()
        .map_or_else(|| pdf.display().to_string(), |n| n.to_string_lossy().into());
    let title = manifest_title(workspace_root, &file_name).unwrap_or_else(|| file_name.clone());
    let overview = Overview {
        schema_version: 1,
        source_document: format!(
            "BDEW {title} ({file_name}, SHA-256 {sha}), Tabelle 1 „Prüfidentifikator zu Prozessschritt\""
        ),
        ahbs,
        sparten,
        zuordnung,
    };

    let out = workspace_root.join(OVERVIEW_PATH);
    let mut json = serde_json::to_string_pretty(&overview)
        .map_err(|e| format!("serialise the inventory: {e}"))?;
    json.push('\n');
    std::fs::write(&out, json).map_err(|e| format!("write {}: {e}", out.display()))?;

    let total: usize = overview.all().len();
    println!(
        "import-pid-overview: {total} Prüfidentifikatoren across {} AHBs → {OVERVIEW_PATH}",
        overview.ahbs.len()
    );
    Ok(())
}

/// The shared-string table, indexed as the cells reference it.
fn shared_strings(
    zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
) -> Result<Vec<String>, String> {
    let xml = entry(zip, "xl/sharedStrings.xml")?;
    Ok(xml
        .split("<si>")
        .skip(1)
        .map(|si| {
            let si = si.split("</si>").next().unwrap_or_default();
            // A string can be split across runs; the value is their concatenation.
            let mut out = String::new();
            for run in si.split("<t").skip(1) {
                let Some(open) = run.find('>') else { continue };
                let Some(close) = run[open + 1..].find("</t>") else {
                    continue;
                };
                out.push_str(&unescape(&run[open + 1..open + 1 + close]));
            }
            out
        })
        .collect())
}

/// The worksheet part backing the sheet named `name`.
fn sheet_part(
    zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
    name: &str,
) -> Result<String, String> {
    let workbook = entry(zip, "xl/workbook.xml")?;
    let rid = workbook
        .split("<sheet ")
        .skip(1)
        .find_map(|s| {
            let tag = s.split('>').next()?;
            (attr(tag, "name")? == name).then(|| attr(tag, "r:id"))?
        })
        .ok_or_else(|| format!("the workbook has no sheet named {name:?}"))?;

    let rels = entry(zip, "xl/_rels/workbook.xml.rels")?;
    let target = rels
        .split("<Relationship ")
        .skip(1)
        .find_map(|s| {
            let tag = s.split('>').next()?;
            (attr(tag, "Id")? == rid).then(|| attr(tag, "Target"))?
        })
        .ok_or_else(|| format!("relationship {rid} is not declared"))?;

    Ok(format!("xl/{}", target.trim_start_matches("/xl/")))
}

/// One zip entry as UTF-8.
fn entry(
    zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
    name: &str,
) -> Result<String, String> {
    let mut file = zip
        .by_name(name)
        .map_err(|_| format!("{name} is not in the workbook"))?;
    let mut out = String::new();
    file.read_to_string(&mut out)
        .map_err(|e| format!("read {name}: {e}"))?;
    Ok(out)
}

/// The `<row>` bodies of a worksheet.
fn rows(sheet: &str) -> impl Iterator<Item = &str> {
    sheet
        .split("<row ")
        .skip(1)
        .filter_map(|r| r.split("</row>").next())
}

/// The cells of one row, keyed by column letter and resolved through `shared`.
fn cells<'a>(row: &'a str, shared: &'a [String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for cell in row.split("<c ").skip(1) {
        let Some(tag) = cell.split('>').next() else {
            continue;
        };
        let Some(reference) = attr(tag, "r") else {
            continue;
        };
        let column: String = reference
            .chars()
            .take_while(char::is_ascii_uppercase)
            .collect();
        let value = cell
            .split("<v>")
            .nth(1)
            .and_then(|v| v.split("</v>").next())
            .map(|v| {
                if attr(tag, "t").as_deref() == Some("s") {
                    v.parse::<usize>()
                        .ok()
                        .and_then(|i| shared.get(i).cloned())
                        .unwrap_or_default()
                } else {
                    unescape(v)
                }
            });
        if let Some(value) = value {
            out.insert(column, value);
        }
    }
    out
}

/// The value of `name` in an XML start tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let at = tag.match_indices(&needle).find(|(i, _)| {
        *i == 0
            || tag[..*i]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
    })?;
    let rest = &tag[at.0 + needle.len()..];
    Some(unescape(rest.split('"').next()?))
}

/// The five XML predefined entities.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

// ── check ────────────────────────────────────────────────────────────────────

/// Compare the shipped profiles against the published inventory.
///
/// Returns `true` when coverage holds at or above [`COVERED_FLOOR`] and every
/// shipped Prüfidentifikator is either published or documented.
#[must_use]
pub fn check(workspace_root: &Path) -> bool {
    let path = workspace_root.join(OVERVIEW_PATH);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!(
            "check-pid-coverage: {OVERVIEW_PATH} is missing — run `cargo xtask \
             import-pid-overview <Anwendungsübersicht .pdf>`"
        );
        return false;
    };
    let overview: Overview = match serde_json::from_str(&raw) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("check-pid-coverage: {OVERVIEW_PATH} is not readable: {e}");
            return false;
        }
    };
    // The door answers an orphan follow-up with `Z33` off this column; an
    // inventory imported before it carried the column would answer none.
    let missing: Vec<&String> = overview
        .all()
        .into_iter()
        .filter(|pid| !overview.zuordnung.contains_key(*pid))
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "check-pid-coverage: {} published Prüfidentifikatoren carry no „Stelle der \
             Zuordnungsprüfung\" — re-run `cargo xtask import-pid-overview`: {missing:?}",
            missing.len()
        );
        return false;
    }

    let mut shipped = shipped_pids(workspace_root);
    if shipped.is_empty() {
        eprintln!("check-pid-coverage: no profile carries a Prüfidentifikator — layout changed?");
        return false;
    }
    shipped.extend(CARRIED_BY_DVGW_EDI.iter().map(|p| (*p).to_owned()));

    let published = overview.all();
    let covered = published.iter().filter(|p| shipped.contains(**p)).count();

    println!(
        "check-pid-coverage: {} — {covered} of {} published Prüfidentifikatoren",
        overview.source_document,
        published.len()
    );
    let mut complete = 0_usize;
    for (ahb, pids) in &overview.ahbs {
        let missing: Vec<&String> = pids.iter().filter(|p| !shipped.contains(*p)).collect();
        if missing.is_empty() {
            complete += 1;
            println!("  {ahb:<46} {:>3}/{:<3}  complete", pids.len(), pids.len());
        } else {
            println!(
                "  {ahb:<46} {:>3}/{:<3}  missing {}",
                pids.len() - missing.len(),
                pids.len(),
                missing.len()
            );
        }
    }
    println!("  {complete} of {} AHBs complete", overview.ahbs.len());

    // The documented exceptions are the part a reader has to re-decide, so they
    // are printed rather than left to whoever opens the source.
    if !SHIPPED_NOT_PUBLISHED.is_empty() {
        println!("  carried but not published:");
        for (pid, reason) in SHIPPED_NOT_PUBLISHED {
            println!("    {pid} — {reason}");
        }
    }

    let mut ok = true;

    // Over-marking is caught the first time a valid message is rejected;
    // a PID that quietly stops being carried is not caught by anything else.
    if covered < COVERED_FLOOR {
        eprintln!(
            "\ncheck-pid-coverage: coverage fell from {COVERED_FLOOR} to {covered}. A \
             Prüfidentifikator that leaves the profiles takes its AHB rules with it, and \
             validation then answers the `AHB-UNKNOWN-PID` warning and nothing else."
        );
        ok = false;
    } else if covered > COVERED_FLOOR {
        println!("\ncheck-pid-coverage: coverage rose to {covered}; raise COVERED_FLOOR to match.");
    }

    let documented: BTreeSet<&str> = SHIPPED_NOT_PUBLISHED.iter().map(|(p, _)| *p).collect();
    let undocumented: Vec<&String> = shipped
        .iter()
        .filter(|p| !published.contains(p) && !documented.contains(p.as_str()))
        .collect();
    if !undocumented.is_empty() {
        eprintln!(
            "\ncheck-pid-coverage: these Prüfidentifikatoren are carried by a profile and \
             are absent from the published overview. Confirm each against its AHB and \
             either retire it or add it to SHIPPED_NOT_PUBLISHED with the reason:\n  {}",
            undocumented
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        ok = false;
    }

    // The reference table is what an integrator reads to answer "does this
    // Prüfidentifikator exist and who handles it". `pid_reference_guard` refuses
    // a row claiming a PID the router does not carry; this is the other
    // direction — a published PID the table never mentions reads as one that
    // does not exist.
    if let Some(absent) = published_pids_absent_from_reference(workspace_root, &published) {
        if !absent.is_empty() {
            eprintln!(
                "\ncheck-pid-coverage: {} published Prüfidentifikator(en) have no row in \
                 {REFERENCE_DOC}, so the reference understates what the market defines:\n  {}",
                absent.len(),
                absent.join(", ")
            );
            ok = false;
        }
    }

    let stale: Vec<&str> = SHIPPED_NOT_PUBLISHED
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| !shipped.contains(*p) || published.contains(&(*p).to_owned()))
        .collect();
    if !stale.is_empty() {
        eprintln!(
            "\ncheck-pid-coverage: these SHIPPED_NOT_PUBLISHED entries no longer describe \
             anything — the Prüfidentifikator is either gone from the profiles or now \
             published. Remove them:\n  {}",
            stale.join(", ")
        );
        ok = false;
    }

    ok
}

/// Published Prüfidentifikatoren with no row in the reference table.
///
/// `None` when the document is absent, which is not this check's business.
fn published_pids_absent_from_reference(
    workspace_root: &Path,
    published: &BTreeSet<&String>,
) -> Option<Vec<String>> {
    let doc = std::fs::read_to_string(workspace_root.join(REFERENCE_DOC)).ok()?;
    // A row opens with the Prüfidentifikator, optionally in backticks.
    let listed: BTreeSet<&str> = doc
        .lines()
        .filter_map(|line| {
            let cell = line.strip_prefix('|')?.trim().trim_matches('`').trim();
            let pid = cell.split('|').next()?.trim().trim_matches('`');
            (pid.len() == 5 && pid.bytes().all(|b| b.is_ascii_digit())).then_some(pid)
        })
        .collect();
    Some(
        published
            .iter()
            .filter(|p| !listed.contains(p.as_str()))
            .map(|p| (*p).clone())
            .collect(),
    )
}

/// Every Prüfidentifikator the **newest** profile of each message type carries.
///
/// Newest per `(message type, release suffix)`, because UTILMD ships a Strom and
/// a Gas profile under one message type and the two are separate releases.
fn shipped_pids(workspace_root: &Path) -> BTreeSet<String> {
    #[derive(serde::Deserialize)]
    struct Ahb {
        anwendungsfaelle: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        /// The profiles carry the Prüfidentifikator as a number; the published
        /// overview carries it as text. Both are the same five digits.
        pid: Option<u32>,
    }

    let mut newest: BTreeMap<(String, String), (String, PathBuf)> = BTreeMap::new();
    let profiles = workspace_root.join("crates/edi-energy/profiles");
    let Ok(types) = std::fs::read_dir(&profiles) else {
        return BTreeSet::new();
    };
    for message_type in types.flatten().filter(|e| e.path().is_dir()) {
        let Ok(releases) = std::fs::read_dir(message_type.path()) else {
            continue;
        };
        for release in releases.flatten().filter(|e| e.path().is_dir()) {
            let ahb = release.path().join("ahb.json");
            if !ahb.is_file() {
                continue;
            }
            let name = release.file_name().to_string_lossy().into_owned();
            let suffix = name
                .split_once('_')
                .map_or(String::new(), |(_, s)| s.to_owned());
            let key = (
                message_type.file_name().to_string_lossy().into_owned(),
                suffix,
            );
            match newest.get(&key) {
                Some((have, _)) if *have >= name => {}
                _ => {
                    newest.insert(key, (name, ahb));
                }
            }
        }
    }

    newest
        .values()
        .filter_map(|(_, path)| std::fs::read_to_string(path).ok())
        .filter_map(|raw| serde_json::from_str::<Ahb>(&raw).ok())
        .flat_map(|a| {
            a.anwendungsfaelle
                .into_iter()
                .filter_map(|e| e.pid)
                .map(|p| p.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SourceRow, attr, bbox_rows, cells, rows, unescape};

    #[test]
    fn an_attribute_is_read_by_its_whole_name() {
        let tag = r#"sheet name="Prüf-ID" sheetId="1" r:id="rId2""#;
        assert_eq!(attr(tag, "name").as_deref(), Some("Prüf-ID"));
        assert_eq!(attr(tag, "r:id").as_deref(), Some("rId2"));
        // `id` must not match inside `r:id` or `sheetId`.
        assert_eq!(attr(tag, "id"), None);
    }

    #[test]
    fn a_shared_string_cell_resolves_through_the_table() {
        let shared = vec!["MSCONS AHB".to_owned(), "unused".to_owned()];
        let row = r#"<row r="2"><c r="B2" t="s"><v>0</v></c><c r="D2"><v>13005</v></c></row>"#;
        let body = rows(row).next().expect("one row");
        let cells = cells(body, &shared);
        assert_eq!(cells.get("B").map(String::as_str), Some("MSCONS AHB"));
        assert_eq!(cells.get("D").map(String::as_str), Some("13005"));
    }

    #[test]
    fn an_empty_cell_is_absent_rather_than_blank() {
        let row = r#"<row r="3"><c r="A3" s="1"/><c r="D3"><v>55001</v></c></row>"#;
        let body = rows(row).next().expect("one row");
        let cells = cells(body, &[]);
        assert!(!cells.contains_key("A"));
        assert_eq!(cells.get("D").map(String::as_str), Some("55001"));
    }

    #[test]
    fn ampersand_is_unescaped_last() {
        assert_eq!(unescape("a &amp;lt; b"), "a &lt; b");
    }

    /// One word of a `pdftotext -bbox` page.
    fn w(x: f64, y: f64, text: &str) -> String {
        format!(
            r#"<word xMin="{x}" yMin="{y}" xMax="{}" yMax="{}">{text}</word>"#,
            x + 10.0,
            y + 9.0
        )
    }

    /// The header line of Tabelle 1, at the x positions of the 12.08.2026
    /// Lesefassung.
    fn header() -> Vec<String> {
        [
            (16.0, "Lfd."),
            (29.0, "Nr."),
            (43.0, "AHB"),
            (126.0, "Beschreibung"),
            (250.0, "Prüfident"),
            (282.0, "Reaktion"),
            (806.0, "Zuordnung"),
            (864.0, "Zuordnung"),
            (923.0, "Erweiter"),
            (953.0, "Objektei"),
            (983.0, "Stelle"),
            (1020.0, "Strom"),
            (1038.0, "Gas"),
            (1052.0, "Übertra"),
        ]
        .iter()
        .map(|(x, t)| w(*x, 59.0, t))
        .chain([
            w(514.0, 25.0, "Prüfidentifikator"),
            w(598.0, 25.0, "zu"),
            w(612.0, 25.0, "Prozessschritt"),
        ])
        .collect()
    }

    /// A data row is read by column position: the AHB cell may wrap onto the
    /// next line, a row with `--` for its Prüfidentifikator names none, and the
    /// page footer below the rule is not part of the last row.
    #[test]
    fn tabelle_1_rows_are_read_by_column() {
        let mut words = header();
        words.extend([
            // Row 1: wraps „UTILMD AHB“ / „Strom“.
            w(16.0, 118.0, "12"),
            w(43.0, 118.0, "UTILMD"),
            w(71.0, 118.0, "AHB"),
            w(126.0, 118.0, "Anmeldung"),
            w(250.0, 118.0, "55001"),
            w(806.0, 118.0, "ZO-T1"),
            w(864.0, 118.0, "--"),
            w(996.0, 118.0, "V"),
            w(1021.0, 118.0, "X"),
            w(1039.0, 118.0, "--"),
            w(1052.0, 118.0, "AS4"),
            w(43.0, 128.0, "Strom"),
            // Row 2: a Geschäftsvorfall tuple, Sparte Gas.
            w(16.0, 156.0, "13"),
            w(43.0, 157.0, "UTILMD"),
            w(71.0, 157.0, "AHB"),
            w(83.0, 157.0, "Gas"),
            w(250.0, 157.0, "44115"),
            w(864.0, 157.0, "ZG-T1"),
            w(996.0, 157.0, "E"),
            // Left of the „Strom" header, as the PDF sets it on some rows.
            w(1015.0, 157.0, "--"),
            w(1039.0, 157.0, "X"),
            // Row 3: a Prozessschritt without an Anwendungsfall.
            w(16.0, 194.0, "14"),
            w(43.0, 194.0, "--"),
            w(250.0, 194.0, "--"),
            // Footer.
            w(207.0, 813.0, "─────"),
            w(49.0, 827.0, "Anwendungsübersicht"),
        ]);
        let html = format!(
            r#"<page width="1190.52" height="841.92">{}</page>"#,
            words.concat()
        );
        let rows = bbox_rows(&html).expect("a Tabelle 1 page");
        assert_eq!(
            rows,
            vec![
                SourceRow {
                    ahb: "UTILMD AHB Strom".to_owned(),
                    pid: "55001".to_owned(),
                    geschaeftsvorfall: "--".to_owned(),
                    stelle: "V".to_owned(),
                    strom: true,
                    gas: false,
                },
                SourceRow {
                    ahb: "UTILMD AHB Gas".to_owned(),
                    pid: "44115".to_owned(),
                    geschaeftsvorfall: "ZG-T1".to_owned(),
                    stelle: "E".to_owned(),
                    strom: false,
                    gas: true,
                },
            ]
        );
    }

    /// A Tabelle 1 header that lost a column is refused: filling a field from
    /// the wrong column is the failure the header check exists to prevent.
    #[test]
    fn a_header_without_its_columns_is_refused() {
        let words: Vec<String> = header()
            .into_iter()
            .filter(|w| !w.contains(">Stelle<"))
            .collect();
        let html = format!(
            r#"<page width="1190.52" height="841.92">{}</page>"#,
            words.concat()
        );
        let err = bbox_rows(&html).unwrap_err();
        assert!(err.contains("Stelle"), "{err}");
    }

    /// The committed inventory names the edition it was read from — the binding
    /// PDF the mirror holds, by title and SHA-256 — so a later Lesefassung is a
    /// visible diff (constitution II: the PDF binds, the .xlsx informs).
    #[test]
    fn the_inventory_names_the_binding_pdf() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let overview: super::Overview = serde_json::from_str(
            &std::fs::read_to_string(root.join(super::OVERVIEW_PATH)).unwrap(),
        )
        .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("regulatories/bdew-mako/manifest.json")).unwrap(),
        )
        .unwrap();
        let named = manifest["files"]
            .as_object()
            .unwrap()
            .values()
            .find(|e| {
                e["sha256"]
                    .as_str()
                    .is_some_and(|h| overview.source_document.contains(h))
            })
            .unwrap_or_else(|| {
                panic!(
                    "source_document names no mirrored file by SHA-256: {}",
                    overview.source_document
                )
            });
        let title = named["title"].as_str().unwrap();
        assert!(
            title.starts_with("Anwendungsübersicht der Prüfidentifikatoren")
                && !title.contains("informatorische"),
            "{title}"
        );
        assert!(overview.source_document.contains(title));
    }

    /// A page of another table is skipped even when its header says „Lfd.".
    #[test]
    fn a_page_of_another_table_is_skipped() {
        let other = format!(
            r#"<page width="1190.52" height="841.92">{}{}</page>"#,
            w(16.0, 25.0, "Änderungshistorie"),
            w(16.0, 59.0, "Lfd.")
        );
        let tabelle_1 = format!(
            r#"<page width="1190.52" height="841.92">{}</page>"#,
            header().concat()
        );
        assert_eq!(bbox_rows(&(other + &tabelle_1)).unwrap(), vec![]);
    }
}
