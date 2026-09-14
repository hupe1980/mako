//! Guard: every catalogued Mindestvorlaufzeit is consulted by something.
//!
//! `mako_fristen::vorlauf::WIM` is a table of published windows, each row
//! carrying its chapter citation. A row that nothing reads is the worst shape a
//! regulatory constant can take: it reads as implemented precisely *because* the
//! citation is right there, so a reviewer checking „is the 20-WT Ende-MSB window
//! handled?" finds the number, the Fundstelle and the correct value, and stops.
//! The window is still never applied to a message.
//!
//! This is not hypothetical. Nineteen of the WiM rows were in exactly that state
//! — sourced, tested for their arithmetic, and consulted by no production caller
//! — while the crate's own docs described them as the Fristen mako enforces.
//!
//! ## What counts as consulted
//!
//! A row is consulted when production code names it, either by its `key` (the
//! `vorlauf("…")` lookup) or by a constant its `shape` is built from
//! (`ABMELDUNG_WT`, `REALISIERUNGSKORRIDOR_WT`, …). Both are evidence that some
//! call site reached the row; neither proves the verdict is acted on, which is
//! what the domain crate's own tests are for.
//!
//! The scan is over everything *outside* `mako-fristen`: the catalogue reaching
//! its own rows is what a table nothing consults looks like from the inside.
//! Most rows are named at the call site. Some are reached one hop away, through
//! a helper in the catalogue's own file — `anmeldung_vorlauf` resolves two rows
//! by key — so a helper counts as a reader **only when something outside
//! `mako-fristen` calls the helper**. That second condition is not bookkeeping:
//! `rechnung_antwort_spaetester_uet` reads three rows and has no production
//! caller at all, so crediting it would have hidden three orphans behind one.
//!
//! Test code is excluded deliberately. A row exercised only by a unit test of
//! its own arithmetic is the defect, not the refutation of it.
//!
//! ## The exemption list
//!
//! [`UNCHECKED`] names the rows that are still only a table, each with what it
//! would take to wire it. It may only shrink: a row that gains a caller fails
//! the check until its entry goes, so an exemption cannot outlive its reason.

use std::path::Path;

/// The catalogue.
const TABLE: &str = "crates/mako-fristen/src/vorlauf.rs";

/// The crate the catalogue lives in.
///
/// Excluded from the caller scan wholesale: the catalogue reaching its own rows
/// is what a table nothing consults looks like from the inside. It participates
/// only through [`helpers`], and only for a helper something outside calls.
const CATALOGUE_CRATE: &str = "crates/mako-fristen/";

/// Trees that may consult it.
const SCANNED: &[&str] = &["crates", "services"];

/// Rows that no production caller reaches yet, and what wiring each needs.
///
/// This list may only shrink.
const UNCHECKED: &[(&str, &str)] = &[
    (
        "wim.antwort-rechnung",
        "REMADV 33001 — `rechnung_antwort_spaetester_uet` covers the same window for the \
         LF/MSB branch by returning the Zahlungsziel unchanged, so it never names this row",
    ),
    (
        "wim.mitteilung-rechnung-korrekt",
        "COMDIS 29001 — `MITTEILUNG_RECHNUNG_KORREKT_WT` is the MSB↔NB window; only the \
         ESA twin `esa_comdis_spaetester_uet` has a caller, and it reads a different constant",
    ),
    (
        "wim.antwort-geraetewechselabsicht",
        "ORDRSP 19015 — `antwort::WIM` already holds the same two Werktage for this PID and \
         is what makod registers; this row restates it for callers reading the Vorlauf side",
    ),
    (
        "wim.verpflichtungsanfrage",
        "the 8.–5. WT window is anchored on the *vorläufig bestätigte* Zuordnungsende, \
         which mako records on the Bestätigung rather than on the 55168 being judged",
    ),
    (
        "wim.gmsb-uebernahme-anstoss",
        "has no PID: the Anstoß is a gMSB-internal act, so there is no inbound message to \
         judge and the window binds mako only where it plays gMSB",
    ),
    (
        "wim.mitteilung-gesamtvorgang",
        "IFTSTA 21009 — needs the bestätigter Zuordnungsbeginn from the MSB-Wechsel Vorgang, \
         which `mako-wim` carries but the IFTSTA leg is parsed without",
    ),
    (
        "wim.scheitern-gesamtvorgang",
        "IFTSTA 21013, same anchor as `wim.mitteilung-gesamtvorgang`",
    ),
    (
        "wim.scheitern-aenderung-technik",
        "has no PID: the 3-WT window binds mako's own outbound Scheitern-Meldung, so it is a \
         send-side deadline rather than an inbound check",
    ),
    (
        "wim.information-bestandsschutz-eigenausbau",
        "IFTSTA 21030/21031 — the answer leg is already held to the same three Werktage by \
         `antwort::WIM`; this row duplicates it for callers holding the answer PID",
    ),
    (
        "wim.vorabinformation-ersteinbau-ims.an-lf-und-nb",
        "has no PID: the 3-Monats-Vorabinformation to LF and NB is mako's own outbound \
         obligation when it plays gMSB",
    ),
    (
        "wim.angebot-rechnungsabwicklung",
        "QUOTES 15002 — anchored on the ÜT of the NB's LF-Zuordnungsmitteilung, which is a \
         different Vorgang than the Angebot being judged",
    ),
    (
        "wim.preisblatt-lf",
        "PRICAT 27002 — mako neither issues nor validates Preisblätter; the row documents \
         the window an operator's own price process must respect",
    ),
    (
        "wim.preisblatt-nb.initial",
        "PRICAT 27002, anchored on the day EDIFACT communication was established — a \
         deployment fact mako does not record",
    ),
    (
        "wim.preisblatt-nb.aenderung",
        "PRICAT 27002, anchored on the Inkrafttreten of the operator's own price change",
    ),
    (
        "wim.rechnung-dienstleistungen",
        "INVOIC 31003 — needs the Leistungsende, which the invoice carries per position \
         rather than per Vorgang",
    ),
];

/// Scan the catalogue and its callers.
///
/// Returns `true` when every row is consulted or documented as not yet wired.
pub fn run(workspace_root: &Path) -> bool {
    let Ok(table) = std::fs::read_to_string(workspace_root.join(TABLE)) else {
        eprintln!("check-vorlauf-consulted: {TABLE} is unreadable — the scan cannot decide");
        return false;
    };
    let rows = parse_rows(&table);
    if rows.is_empty() {
        eprintln!(
            "check-vorlauf-consulted: no `VorlaufObligation` row found in {TABLE} — \
             the catalogue's shape has probably changed"
        );
        return false;
    }

    let mut callers = String::new();
    let mut scanned = 0usize;
    for dir in SCANNED {
        concat_production_sources(
            &workspace_root.join(dir),
            workspace_root,
            &mut scanned,
            &mut callers,
        );
    }
    // Helpers in the catalogue that resolve a row, and are themselves called
    // from outside it.
    let reached_helpers: Vec<Helper> = helpers(&table)
        .into_iter()
        .filter(|h| callers.contains(&format!("{}(", h.name)))
        .collect();
    if scanned == 0 {
        eprintln!(
            "check-vorlauf-consulted: the scan read no source under any of {SCANNED:?} — \
             the layout has probably changed"
        );
        return false;
    }

    let mut orphans: Vec<&str> = Vec::new();
    let mut stale: Vec<&str> = Vec::new();
    let mut consulted = 0usize;
    for row in &rows {
        let quoted = format!("\"{}\"", row.key);
        let named = callers.contains(&quoted)
            || row.consts.iter().any(|c| callers.contains(c.as_str()))
            || reached_helpers.iter().any(|h| {
                h.body.contains(&quoted) || row.consts.iter().any(|c| h.body.contains(c.as_str()))
            });
        let exempt = UNCHECKED.iter().any(|(k, _)| *k == row.key);
        match (named, exempt) {
            (true, false) => consulted += 1,
            (false, false) => orphans.push(&row.key),
            (true, true) => stale.push(&row.key),
            (false, true) => {}
        }
    }

    if orphans.is_empty() && stale.is_empty() {
        println!(
            "check-vorlauf-consulted: {consulted} of {} catalogued Vorlauffrist(en) are \
             consulted by production code ({} documented as not yet wired)",
            rows.len(),
            UNCHECKED.len()
        );
        return true;
    }

    for key in &orphans {
        eprintln!(
            "check-vorlauf-consulted: `{key}` is published with its citation and read by \
             nothing — either wire it or add it to UNCHECKED with what it needs"
        );
    }
    for key in &stale {
        eprintln!(
            "check-vorlauf-consulted: `{key}` is listed as not yet wired and now has a \
             caller — remove its entry from UNCHECKED"
        );
    }
    false
}

/// One catalogued row, as far as this scan reads it.
struct Row {
    /// The lookup slug.
    key: String,
    /// Constants named in its `shape`, if any.
    consts: Vec<String>,
}

/// Every `key:` / `shape:` pair in the catalogue.
///
/// A hand-rolled scan rather than a parse: the table is a `const` array of
/// struct literals in one file, and a `syn` dependency in `xtask` to read two
/// fields would cost more than it settles.
fn parse_rows(src: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut pending: Option<String> = None;
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("key: \"")
            && let Some(key) = rest.split('"').next()
        {
            pending = Some(key.to_owned());
        } else if let Some(rest) = line.strip_prefix("shape: ")
            && let Some(key) = pending.take()
        {
            rows.push(Row {
                key,
                consts: screaming_idents(rest),
            });
        }
    }
    rows
}

/// The `SCREAMING_SNAKE` identifiers in one expression.
fn screaming_idents(expr: &str) -> Vec<String> {
    expr.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| {
            w.len() > 3
                && w.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .map(ToOwned::to_owned)
        .collect()
}

/// Every production `.rs` file under `dir`, concatenated.
///
/// Skips `tests/` and `benches/` trees and everything from a file's first
/// `#[cfg(test)]` onwards: a row exercised only by a test of its own arithmetic
/// is the defect this guard is about. In the catalogue's own file the table
/// literal is dropped too, so a row cannot count as read by its own
/// declaration.
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
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if rel
            .to_string_lossy()
            .replace('\\', "/")
            .starts_with(CATALOGUE_CRATE)
        {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        *scanned += 1;
        out.push_str(src.split("#[cfg(test)]").next().unwrap_or_default());
    }
}

/// A public function in the catalogue that may resolve a row.
struct Helper {
    /// Its name, as a caller would write it.
    name: String,
    /// Its source, where a key or a constant would appear.
    body: String,
}

/// The public functions of the catalogue, each with its own body.
///
/// A body runs from the signature to the first `}` in column 0, which is where a
/// top-level `fn` closes. Bounding it there rather than at the next signature
/// matters: the stretch between two functions holds the module's `pub const`
/// declarations, and absorbing those into the preceding helper credits its
/// callers with rows it never reads.
fn helpers(src: &str) -> Vec<Helper> {
    let src = without_table_literal(src.split("#[cfg(test)]").next().unwrap_or_default());
    let mut starts: Vec<(usize, String)> = Vec::new();
    for pat in ["\npub fn ", "\npub const fn "] {
        let mut from = 0usize;
        while let Some(i) = src[from..].find(pat) {
            let at = from + i;
            let rest = &src[at + pat.len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                starts.push((at, name));
            }
            from = at + pat.len();
        }
    }
    starts.sort_by_key(|(at, _)| *at);
    starts
        .iter()
        .map(|(at, name)| {
            let rest = &src[*at..];
            let end = rest.find("\n}\n").map_or(rest.len(), |i| i + 2);
            Helper {
                name: name.clone(),
                body: rest[..end].to_owned(),
            }
        })
        .collect()
}

/// The catalogue's source with the `WIM` table literal removed.
///
/// Everything from `pub const WIM` to the `];` that closes it, so the `key:`
/// fields inside cannot count as callers of themselves.
fn without_table_literal(src: &str) -> String {
    let Some(start) = src.find("pub const WIM") else {
        return src.to_owned();
    };
    let Some(end) = src[start..].find("\n];") else {
        return src[..start].to_owned();
    };
    format!("{}{}", &src[..start], &src[start + end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper counts only when something calls it. `rechnung_antwort_spaetester_uet`
    /// reads three rows and has no production caller, which is why the second
    /// condition exists at all.
    #[test]
    fn helpers_are_found_with_the_rows_they_resolve() {
        let hs = helpers(
            "\npub fn anmeldung_vorlauf(e: bool) -> VorlaufShape {\n\
             vorlauf(\"wim.anmeldung-msb\")\n}\n\
             \npub fn realisierungskorridor(d: Date) -> R {\n\
             REALISIERUNGSKORRIDOR_WT\n}\n",
        );
        assert_eq!(hs.len(), 2);
        assert_eq!(hs[0].name, "anmeldung_vorlauf");
        assert!(hs[0].body.contains("wim.anmeldung-msb"));
        assert_eq!(hs[1].name, "realisierungskorridor");
        assert!(
            !hs[1].body.contains("wim.anmeldung-msb"),
            "a helper must not absorb the one before it"
        );
    }

    /// A body stops at its own closing brace, so module constants declared
    /// between two functions belong to neither.
    #[test]
    fn a_helper_body_stops_at_its_closing_brace() {
        let hs = helpers(
            "\npub fn reads_nothing() -> u32 {\n    7\n}\n\
             \n/// Between the two.\npub const ANTWORT_IMS_RECHNUNG_NB_WT: u32 = 4;\n\
             \npub fn also_reads_nothing() -> u32 {\n    8\n}\n",
        );
        assert_eq!(hs.len(), 2);
        assert!(
            !hs[0].body.contains("ANTWORT_IMS_RECHNUNG_NB_WT"),
            "a constant after the closing brace is not part of the helper"
        );
    }

    /// The table literal is dropped, and only it.
    #[test]
    fn the_table_cannot_count_as_its_own_caller() {
        let src = "fn helper() { vorlauf(\"wim.anmeldung-msb\") }\n\
                   pub const WIM: &[VorlaufObligation] = &[\n\
                   key: \"wim.ende-msb\",\n\
                   ];\n\
                   fn after() { vorlauf(\"wim.verpflichtungsanfrage\") }";
        let out = without_table_literal(src);
        assert!(
            out.contains("wim.anmeldung-msb"),
            "helpers before it survive"
        );
        assert!(
            out.contains("wim.verpflichtungsanfrage"),
            "code after it survives"
        );
        assert!(!out.contains("wim.ende-msb"), "the table itself is dropped");
    }

    /// A scan that read nothing certifies nothing.
    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!run(Path::new("/nonexistent-mako-root")));
    }

    /// Every exemption says what wiring it needs, not merely that it is one.
    #[test]
    fn every_exemption_states_what_it_needs() {
        for (key, reason) in UNCHECKED {
            assert!(!key.is_empty());
            assert!(
                reason.len() > 40,
                "{key}'s exemption must say what wiring it needs"
            );
        }
    }

    /// The exemption list names real rows. A typo would silently exempt nothing
    /// and leave the row it meant to cover reported as an orphan — which fails
    /// loudly — but a *renamed* row would leave a dead entry behind.
    #[test]
    fn no_exemption_names_a_row_that_is_gone() {
        let table = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../crates/mako-fristen/src/vorlauf.rs"
        ))
        .expect("the catalogue is a workspace file");
        let keys: Vec<String> = parse_rows(&table).into_iter().map(|r| r.key).collect();
        for (key, _) in UNCHECKED {
            assert!(
                keys.iter().any(|k| k == key),
                "UNCHECKED names `{key}`, which the catalogue no longer has"
            );
        }
    }

    /// The row scan reads both fields, and misses neither a keyless shape nor a
    /// shape built from a constant.
    #[test]
    fn rows_carry_their_key_and_their_constants() {
        let rows = parse_rows(
            r#"
            VorlaufObligation {
                key: "wim.ende-msb",
                shape: VorlaufShape::LatestWerktageBefore(ABMELDUNG_WT),
            },
            VorlaufObligation {
                key: "wim.gmsb-uebernahme-anstoss",
                shape: VorlaufShape::LatestWerktageBefore(4),
            },
            "#,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "wim.ende-msb");
        assert_eq!(rows[0].consts, ["ABMELDUNG_WT"]);
        assert_eq!(rows[1].key, "wim.gmsb-uebernahme-anstoss");
        assert!(rows[1].consts.is_empty());
    }

    /// Only `SCREAMING_SNAKE` names count — a variant name is not a constant a
    /// caller can be found by.
    #[test]
    fn variant_names_are_not_mistaken_for_constants() {
        assert!(screaming_idents("VorlaufShape::LatestWerktageBefore(20)").is_empty());
        assert_eq!(
            screaming_idents("VorlaufShape::Korridor(REALISIERUNGSKORRIDOR_WT)"),
            ["REALISIERUNGSKORRIDOR_WT"]
        );
    }
}
