//! Guard: every catalogued Referenzreihe is sourced, licensed and read, and no
//! value ships for a series mako may not distribute.
//!
//! `mako_markt::referenzreihen` is the one list of published figures billing
//! reads. Three defects keep a list like that from being what it claims:
//!
//! 1. **An unsourced or unlicensed series.** A row without a publisher, a
//!    source URL, a legal anchor or a licence is a figure nobody can check or
//!    lawfully redistribute. The catalogue's types make the fields mandatory;
//!    this refuses them empty, and a source that is not `https://`.
//! 2. **A series nothing reads.** A catalogued series with its citation and no
//!    reader reads as wired precisely because the citation is there. A series
//!    counts as read when production code outside `mako-markt` and `marktd`
//!    (which serve every series generically) names its variant
//!    (`Reihe::KavGrenzpreisStrom`); comments and test code do not count.
//!    [`OHNE_LESER`] names the series still waiting for their reader, each with
//!    what it waits on, and may only shrink: a series that gains a reader
//!    fails until its entry goes.
//! 3. **A seed row mako may not ship, or one the crate does not state.** Every
//!    `INSERT INTO referenzwerte` in a `.sql` file under `services/` or `dev/`
//!    is read: a row for an unknown series or a `Betreiber` series is refused
//!    (the repository ships no EPEX value), and the rows of a `Frei`
//!    series must be exactly [`mako_markt::referenzreihen::Reihe::seed`], so a
//!    hand-edited figure cannot drift from the crate.

use std::path::Path;

use mako_markt::referenzreihen::{Lizenz, Reihe, ReiheInfo};

/// Series with no production reader yet, and what each waits on.
///
/// This list may only shrink.
const OHNE_LESER: &[(&str, &str)] = &[
    (
        "KavDurchschnittserloesGas",
        "the KAV § 2 Abs. 5 Nr. 2 scaling of 1,50 ct/kWh by Durchschnittserlöse is not \
         derived: the Gas Grenzpreis stays caller-supplied until it is",
    ),
    (
        "EegMonatsmarktwert",
        "einsd reads its own `marktwert_preise` table, which overwrites a provisional \
         figure with the final one, until it reads the store",
    ),
    (
        "EegJahresmarktwert",
        "einsd reads its own `marktwert_preise` table, which overwrites a provisional \
         figure with the final one, until it reads the store",
    ),
    (
        "DayAheadDeLu",
        "productd and einsd read their own operator-filled EPEX tables until they read \
         the store",
    ),
];

/// Trees a reader may live in.
const SCANNED: &[&str] = &["crates", "services"];

/// Code that serves every series generically and therefore names none as a
/// reader would.
const GENERIC: &[&str] = &["crates/mako-markt/", "services/marktd/"];

/// Trees whose `.sql` files are searched for seed rows.
const SEED_TREES: &[&str] = &["services", "dev"];

pub fn run(workspace_root: &Path) -> bool {
    let mut findings = catalogue_findings(&Reihe::ALL.map(Reihe::info));

    let mut sources = String::new();
    let mut scanned = 0usize;
    for dir in SCANNED {
        concat_production_sources(
            &workspace_root.join(dir),
            workspace_root,
            &mut scanned,
            &mut sources,
        );
    }
    if scanned == 0 {
        eprintln!("check-referenzreihen: read no source under {SCANNED:?} — the layout changed");
        return false;
    }
    let variants: Vec<String> = Reihe::ALL.iter().map(|r| format!("{r:?}")).collect();
    findings.extend(reader_findings(&variants, &sources, OHNE_LESER));

    let mut sql_files = Vec::new();
    for dir in SEED_TREES {
        collect_sql(&workspace_root.join(dir), &mut sql_files);
    }
    if sql_files.is_empty() {
        eprintln!("check-referenzreihen: found no .sql file under {SEED_TREES:?}");
        return false;
    }
    let mut seeded: Vec<SeedRow> = Vec::new();
    for path in &sql_files {
        let Ok(sql) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .display()
            .to_string();
        seeded.extend(seed_rows(&sql).into_iter().map(|mut r| {
            r.file = rel.clone();
            r
        }));
    }
    findings.extend(seed_findings(&seeded));

    if findings.is_empty() {
        println!(
            "check-referenzreihen: {} series sourced and licensed, {} read ({} waiting on a \
             reader), {} seed row(s) consistent with the catalogue",
            Reihe::ALL.len(),
            Reihe::ALL.len() - OHNE_LESER.len(),
            OHNE_LESER.len(),
            seeded.len()
        );
        return true;
    }
    for f in &findings {
        eprintln!("check-referenzreihen: {f}");
    }
    false
}

/// Rule 1 — every entry names its publisher, source, anchor and licence.
fn catalogue_findings(entries: &[ReiheInfo]) -> Vec<String> {
    let mut out = Vec::new();
    for e in entries {
        let k = e.schluessel;
        for (field, value) in [
            ("bezeichnung", e.bezeichnung),
            ("einheit", e.einheit),
            ("herausgeber", e.herausgeber),
            ("rechtsgrundlage", e.rechtsgrundlage),
        ] {
            if value.trim().is_empty() {
                out.push(format!("`{k}` has an empty {field}"));
            }
        }
        if !e.quelle_url.starts_with("https://") {
            out.push(format!(
                "`{k}` names no https source (`{}`) — a figure nobody can check",
                e.quelle_url
            ));
        }
        if let Lizenz::Frei {
            namensnennung,
            lizenz_url,
        } = e.lizenz
            && (namensnennung.trim().is_empty() || !lizenz_url.starts_with("https://"))
        {
            out.push(format!(
                "`{k}` is Frei without the attribution and the https page its licence was \
                 read on"
            ));
        }
    }
    out
}

/// Rule 2 — every series is named by a reader or waits in `exempt`.
fn reader_findings(variants: &[String], sources: &str, exempt: &[(&str, &str)]) -> Vec<String> {
    let mut out = Vec::new();
    for v in variants {
        let read = sources.contains(&format!("Reihe::{v}"));
        let waiting = exempt.iter().any(|(e, _)| e == v);
        match (read, waiting) {
            (false, false) => out.push(format!(
                "`Reihe::{v}` is catalogued and read by nothing — wire a reader or list it in \
                 OHNE_LESER with what it waits on"
            )),
            (true, true) => out.push(format!(
                "`Reihe::{v}` now has a reader — remove it from OHNE_LESER"
            )),
            _ => {}
        }
    }
    for (e, _) in exempt {
        if !variants.iter().any(|v| v == e) {
            out.push(format!(
                "OHNE_LESER names `{e}`, which the catalogue no longer has"
            ));
        }
    }
    out
}

/// One seeded row, as far as this guard reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeedRow {
    file: String,
    reihe: String,
    dim: String,
    wert: String,
}

/// Rule 3 — no seed for an unknown or `Betreiber` series, and a `Frei`
/// series' rows are exactly the crate's.
fn seed_findings(rows: &[SeedRow]) -> Vec<String> {
    let mut out = Vec::new();
    for row in rows {
        match Reihe::from_schluessel(&row.reihe) {
            None => out.push(format!(
                "{}: seeds `{}`, which is no catalogued series",
                row.file, row.reihe
            )),
            Some(r) if r.info().lizenz == Lizenz::Betreiber => out.push(format!(
                "{}: seeds `{}`, a Betreiber series — its values are the operator's to load \
                 under its own licence, never shipped",
                row.file, row.reihe
            )),
            Some(r) => {
                let stated = r.seed().iter().any(|s| {
                    s.dim == row.dim
                        && s.wert.normalize().to_string() == normalize_number(&row.wert)
                });
                if !stated {
                    out.push(format!(
                        "{}: seeds `{}` ({:?}) = {}, which `Reihe::seed` does not state — the \
                         crate is the one place a seeded figure is written",
                        row.file, row.reihe, row.dim, row.wert
                    ));
                }
            }
        }
    }
    for r in Reihe::ALL {
        let in_sql = rows
            .iter()
            .filter(|row| row.reihe == r.schluessel())
            .count();
        if in_sql < r.seed().len() {
            out.push(format!(
                "`{r}` states {} seed value(s) and the schema seeds {in_sql}",
                r.seed().len()
            ));
        }
    }
    out
}

/// `12.3400` → `12.34`, as `Decimal::normalize` prints it.
fn normalize_number(s: &str) -> String {
    let s = s.trim();
    if !s.contains('.') {
        return s.to_owned();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The `(reihe, dim, …, wert, …)` tuples of every `INSERT INTO referenzwerte`.
///
/// The seed's column order is fixed: `reihe, dim, gilt, wert, …`.
fn seed_rows(sql: &str) -> Vec<SeedRow> {
    let mut out = Vec::new();
    let mut rest = sql;
    while let Some(i) = rest.find("INSERT INTO referenzwerte") {
        let tail = &rest[i..];
        let end = tail.find(';').unwrap_or(tail.len());
        let stmt = &tail[..end];
        if let Some(v) = stmt.find("VALUES") {
            for tuple in tuples(&stmt[v + "VALUES".len()..]) {
                let f = split_fields(&tuple);
                if f.len() >= 4 {
                    out.push(SeedRow {
                        file: String::new(),
                        reihe: unquote(&f[0]),
                        dim: unquote(&f[1]),
                        wert: f[3].clone(),
                    });
                }
            }
        }
        rest = &tail[end..];
    }
    out
}

/// The top-level `( … )` groups of a `VALUES` list.
fn tuples(values: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut cur = String::new();
    for c in values.chars() {
        match c {
            '\'' => {
                quoted = !quoted;
                if depth > 0 {
                    cur.push(c);
                }
            }
            '(' if !quoted => {
                if depth > 0 {
                    cur.push(c);
                }
                depth += 1;
            }
            ')' if !quoted => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
            }
            _ if depth > 0 => cur.push(c),
            _ => {}
        }
    }
    out
}

/// Split one tuple body on its top-level commas.
fn split_fields(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut cur = String::new();
    for c in body.chars() {
        match c {
            '\'' => {
                quoted = !quoted;
                cur.push(c);
            }
            '(' if !quoted => {
                depth += 1;
                cur.push(c);
            }
            ')' if !quoted => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' if !quoted && depth == 0 => out.push(std::mem::take(&mut cur).trim().to_owned()),
            _ => cur.push(c),
        }
    }
    out.push(cur.trim().to_owned());
    out
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('\'').to_owned()
}

fn collect_sql(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "node_modules")
            {
                continue;
            }
            collect_sql(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            out.push(path);
        }
    }
}

/// Every production `.rs` file under `dir` outside [`GENERIC`], comment-stripped
/// and cut at its first `#[cfg(test)]`; `tests/` and `benches/` are skipped.
fn concat_production_sources(dir: &Path, root: &Path, scanned: &mut usize, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "tests" || n == "benches")
            {
                continue;
            }
            concat_production_sources(&path, root, scanned, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if GENERIC.iter().any(|g| rel.starts_with(g)) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        *scanned += 1;
        out.push_str(&crate::check_vorlauf_consulted::strip_comments(
            src.split("#[cfg(test)]").next().unwrap_or_default(),
        ));
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::referenzreihen::{Faelligkeit, Granularitaet};

    fn entry(quelle_url: &'static str, lizenz: Lizenz) -> ReiheInfo {
        ReiheInfo {
            schluessel: "probe",
            bezeichnung: "Probe",
            einheit: "ct/kWh",
            granularitaet: Granularitaet::Kalenderjahr,
            dimensionen: &[],
            herausgeber: "Herausgeber",
            quelle_url,
            rechtsgrundlage: "KAV § 2 Abs. 4",
            faelligkeit: Faelligkeit::Ungeregelt { hinweis: "-" },
            lizenz,
        }
    }

    #[test]
    fn the_shipped_catalogue_is_sourced_and_licensed() {
        assert_eq!(
            catalogue_findings(&Reihe::ALL.map(Reihe::info)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_series_without_a_checkable_source_or_licence_is_refused() {
        assert!(catalogue_findings(&[entry("https://x.de/", Lizenz::Betreiber)]).is_empty());
        assert_eq!(
            catalogue_findings(&[entry("", Lizenz::Betreiber)]).len(),
            1,
            "no source"
        );
        let mut e = entry("https://x.de/", Lizenz::Betreiber);
        e.herausgeber = " ";
        assert_eq!(catalogue_findings(&[e]).len(), 1, "no publisher");
        assert_eq!(
            catalogue_findings(&[entry(
                "https://x.de/",
                Lizenz::Frei {
                    namensnennung: "",
                    lizenz_url: "https://x.de/l",
                }
            )])
            .len(),
            1,
            "Frei without attribution"
        );
    }

    #[test]
    fn a_series_only_a_comment_or_a_test_names_is_unread() {
        let v = vec!["KavGrenzpreisStrom".to_owned()];
        assert!(reader_findings(&v, "let r = Reihe::KavGrenzpreisStrom;", &[]).is_empty());
        assert_eq!(reader_findings(&v, "", &[]).len(), 1);
        let commented = crate::check_vorlauf_consulted::strip_comments(
            "// Reihe::KavGrenzpreisStrom is read below\nfn f() {}",
        );
        assert_eq!(reader_findings(&v, &commented, &[]).len(), 1);
    }

    #[test]
    fn an_exemption_cannot_outlive_its_reason() {
        let v = vec!["DayAheadDeLu".to_owned()];
        let exempt = [("DayAheadDeLu", "waits")];
        assert!(reader_findings(&v, "", &exempt).is_empty());
        assert_eq!(
            reader_findings(&v, "x(Reihe::DayAheadDeLu)", &exempt).len(),
            1,
            "a read series stays exempt"
        );
        assert_eq!(
            reader_findings(&[], "", &exempt).len(),
            1,
            "an exemption for a removed series"
        );
    }

    #[test]
    fn every_exemption_names_a_catalogued_series_and_a_reason() {
        for (v, why) in OHNE_LESER {
            assert!(
                Reihe::ALL.iter().any(|r| format!("{r:?}") == *v),
                "{v} is not catalogued"
            );
            assert!(why.len() > 20, "{v} needs a reason");
        }
    }

    const SQL: &str = "CREATE TABLE referenzwerte (x int);\n\
        INSERT INTO referenzwerte (reihe, dim, gilt, wert, status, quelle_url, herkunft) VALUES\n\
          ('day_ahead_de_lu', '', tstzrange('2026-01-01 00:00+01', '2026-01-01 00:15+01', '[)'), 8.123, 'endgueltig', 'https://x', 'seed'),\n\
          ('kav_grenzpreis_strom', '', tstzrange('2025-01-01 00:00+01', '2026-01-01 00:00+01', '[)'), 17.5, 'endgueltig', 'https://x', 'seed'),\n\
          ('erfunden', '', tstzrange('2025-01-01 00:00+01', '2026-01-01 00:00+01', '[)'), 1, 'endgueltig', 'https://x', 'seed');\n";

    #[test]
    fn seed_rows_are_read_with_their_series_dimension_and_value() {
        let rows = seed_rows(SQL);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].reihe, "day_ahead_de_lu");
        assert_eq!(rows[0].dim, "");
        assert_eq!(rows[0].wert, "8.123");
    }

    /// The repository ships no EPEX value; and a Frei figure the
    /// crate does not state is a hand-edited one.
    #[test]
    fn a_betreiber_an_unknown_or_an_unstated_seed_row_is_refused() {
        let findings = seed_findings(&seed_rows(SQL));
        assert_eq!(findings.len(), 3, "{findings:#?}");
        assert!(findings[0].contains("Betreiber"));
        assert!(findings[1].contains("does not state"));
        assert!(findings[2].contains("no catalogued series"));
        assert!(seed_findings(&[]).is_empty());
    }

    #[test]
    fn numbers_compare_as_decimals() {
        assert_eq!(normalize_number("17.500"), "17.5");
        assert_eq!(normalize_number("17.0"), "17");
        assert_eq!(normalize_number("17"), "17");
    }
}
