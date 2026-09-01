//! The published Prüfidentifikator inventory, and mako's coverage of it.
//!
//! `validate-profiles` compares one profile release against the previous one,
//! so it proves nothing was *lost* and is blind to a Prüfidentifikator that was
//! never imported at all. This is the other direction, against the only
//! statement of what exists: BDEW's *Anwendungsübersicht der
//! Prüfidentifikatoren*. Where coverage is short, `ahb_rule_pack` answers a
//! warning-only `unknown-pid` pack and `is_valid()` stays `true` in both
//! directions, so the figure has to be computed rather than maintained by hand.
//!
//! Two commands:
//!
//! - **`import-pid-overview`** reads the published workbook and writes
//!   [`OVERVIEW_PATH`] — the one step needing the source document.
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
const COVERED_FLOOR: usize = 370;

/// Prüfidentifikatoren mako's profiles carry that the overview does not list,
/// each with the reason it stays.
const SHIPPED_NOT_PUBLISHED: &[(&str, &str)] = &[
    (
        "19115",
        "Ablehnung Anforderung bilanzierte Menge — carried by the ORDRSP profile; \
         confirm against the ORDRSP AHB and either retire it or record why it stays",
    ),
    (
        "21015",
        "withdrawn by IFTSTA AHB 2.1 Änd-ID 27061, but AHB 2.0g and the fv20251001 \
         profile still publish it and EDIFACT has no Übergangsfrist — it stays until \
         that profile goes",
    ),
    (
        "21024",
        "carried by the IFTSTA profile; confirm against the IFTSTA AHB and either \
         retire it or record why it stays",
    ),
    (
        "21026",
        "carried by the IFTSTA profile; confirm against the IFTSTA AHB and either \
         retire it or record why it stays",
    ),
];

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

/// Extract the inventory from `xlsx` and write [`OVERVIEW_PATH`].
///
/// # Errors
///
/// When the workbook cannot be read or does not carry the expected sheet.
pub fn import(workspace_root: &Path, xlsx: &Path) -> Result<(), String> {
    let bytes = std::fs::read(xlsx).map_err(|e| format!("read {}: {e}", xlsx.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("{} is not a readable xlsx: {e}", xlsx.display()))?;

    let shared = shared_strings(&mut zip)?;
    let sheet_part = sheet_part(&mut zip, SHEET)?;
    let sheet_xml = entry(&mut zip, &sheet_part)?;

    let mut ahbs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows(&sheet_xml) {
        let cells = cells(row, &shared);
        let (Some(ahb), Some(pid)) = (cells.get(COL_AHB), cells.get(COL_PID)) else {
            continue;
        };
        let (ahb, pid) = (ahb.trim(), pid.trim());
        // The header row and the layout notes both fail this.
        if ahb.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) || pid.is_empty() {
            continue;
        }
        ahbs.entry(ahb.to_owned())
            .or_default()
            .insert(pid.to_owned());
    }
    if ahbs.is_empty() {
        return Err(format!(
            "no Prüfidentifikator rows found in sheet {SHEET:?} — has the workbook layout changed?"
        ));
    }

    let source = xlsx.file_name().map_or_else(
        || xlsx.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    let overview = Overview {
        schema_version: 1,
        source_document: format!(
            "BDEW Anwendungsübersicht der Prüfidentifikatoren ({source}), sheet „{SHEET}\""
        ),
        ahbs,
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
             import-pid-overview <Anwendungsübersicht .xlsx>`"
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

    let shipped = shipped_pids(workspace_root);
    if shipped.is_empty() {
        eprintln!("check-pid-coverage: no profile carries a Prüfidentifikator — layout changed?");
        return false;
    }

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
             `ahb_rule_pack` then answers a warning-only `unknown-pid` pack in both \
             directions."
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
        pruefidentifikatoren: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        /// The profiles carry the Prüfidentifikator as a number; the published
        /// overview carries it as text. Both are the same five digits.
        code: u32,
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
            a.pruefidentifikatoren
                .into_iter()
                .map(|e| e.code.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{attr, cells, rows, unescape};

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
}
