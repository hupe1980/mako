//! Guard: a business date is a Europe/Berlin date.
//!
//! Every date the German energy market states — a Lieferbeginn, a
//! Rechnungsdatum, the day a Frist starts counting, the day a price slice takes
//! effect, the year an Abrechnung settles — is a calendar date in German local
//! time. Reading it off the UTC clock is wrong for one hour every night (two in
//! summer), silently and without a signal in the value.
//!
//! | Idiom | What it answers |
//! |---|---|
//! | `OffsetDateTime::now_utc().date()` | the UTC calendar date |
//! | `now_utc().year()` / `.month()` / `.day()` | a UTC calendar *component* |
//! | `let now = now_utc(); … now.day()` | the same, one binding removed |
//! | SQL `current_date` | the *session* time zone's date |
//! | SQL `now()::date`, `extract(year FROM now())`, `to_char(now(), …)` | the same |
//!
//! The replacements are [`mako_fristen::heute`] / `berlin_date` / `berlin_now`
//! on the Rust side and the `heute()` SQL function each schema defines. This
//! check refuses the UTC idioms so the next one is caught at `just ci` rather
//! than at a month boundary, where an invoice dated into the previous month, an
//! Abschlagslauf raised for the wrong day-of-month cohort or a Frist counted
//! from the wrong day is expensive and quiet.
//!
//! ## What is deliberately not a business date
//!
//! A timestamp **on the EDIFACT wire** is UTC by rule: Allgemeine Festlegungen
//! §3 states „Die Angabe von Zeiten in einer EDIFACT Nachricht erfolgt in
//! koordinierter Weltzeit", DTM format 303 fixes DE 2380 to `+00`, and §2.12
//! dates the Content-Disposition filename „bei Erzeugung der Datei in UTC".
//! The modules that encode those values are exempt by path; everything else
//! reading a calendar component off the clock is a business date.

use std::path::{Path, PathBuf};

/// A single offending site.
type Finding = (PathBuf, usize, String);

/// Paths whose UTC calendar components are the EDIFACT wire encoding, not a
/// business date (Allgemeine Festlegungen §3 and §2.12).
const WIRE_ENCODERS: &[&str] = &[
    "crates/edi-energy/src/builders",
    "services/makod/src/transport/as4_sender.rs",
    "services/makod/src/orchestrator/edifact_renderer/mod.rs",
];

/// Paths that define or enforce the rule, and therefore name the UTC clock.
const RULE_SITES: &[&str] = &[
    "crates/mako-fristen/src/lib.rs",
    "xtask/src/check_business_dates.rs",
];

/// Scan the workspace for UTC-dated business dates.
///
/// Returns `true` when every business date is a Berlin date.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    for dir in ["services", "crates", "xtask"] {
        collect(&workspace_root.join(dir), workspace_root, &mut findings);
    }

    if findings.is_empty() {
        println!("check-business-dates: every business date is a Europe/Berlin date");
        return true;
    }

    eprintln!(
        "check-business-dates: {} site(s) read a business date off the UTC clock:",
        findings.len()
    );
    for (path, line, text) in &findings {
        eprintln!("  {}:{line}  {}", path.display(), text.trim());
    }
    eprintln!(
        "\nRust: use `mako_fristen::heute()` for today, `berlin_date(instant)` for \
         the day of a stored instant, `berlin_now()` for the German wall clock.\n\
         SQL:  use the schema's `heute()` function, not `current_date` or `now()`."
    );
    false
}

/// Every `.rs` and `.sql` file under `dir`.
fn collect(dir: &Path, root: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, root, findings);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("rs" | "sql")) {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if is_exempt(rel) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, i) in offending_lines(&src) {
            findings.push((path.clone(), i, line));
        }
    }
}

/// Whether `rel` (workspace-relative) is a wire encoder or a rule site.
fn is_exempt(rel: &Path) -> bool {
    let rel = rel.to_string_lossy().replace('\\', "/");
    WIRE_ENCODERS
        .iter()
        .chain(RULE_SITES)
        .any(|p| rel == *p || rel.starts_with(&format!("{p}/")))
}

/// The calendar components a business date is read out of.
const COMPONENTS: &[&str] = &["date", "year", "month", "day"];

/// The offending lines of one source file, as `(line, 1-based number)`.
///
/// Split from the filesystem so the rule is testable against exact text. The
/// scan is per file rather than per line because the common shape binds the
/// clock first and reads the component several lines later.
#[must_use]
pub fn offending_lines(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    // Locals currently holding `OffsetDateTime::now_utc()`. Cleared at a
    // top-level item boundary so a `now` parameter elsewhere is not implicated.
    let mut clock_bindings: Vec<String> = Vec::new();

    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        // Prose describing the rule is not a violation of it.
        if trimmed.starts_with("//") || trimmed.starts_with("--") {
            continue;
        }
        if starts_top_level_item(line) {
            clock_bindings.clear();
        }

        if reads_component_of(line, "now_utc()")
            || sql_reads_the_clock(line)
            || clock_bindings.iter().any(|b| reads_component_of(line, b))
        {
            out.push((line.to_owned(), i + 1));
        }

        match binding_of_now_utc(line) {
            Some(name) => clock_bindings.push(name),
            None => {
                // A rebinding of the same name to something else ends its life
                // as the clock.
                if let Some(name) = rebound_name(line) {
                    clock_bindings.retain(|b| *b != name);
                }
            }
        }
    }
    out
}

/// Whether `line` starts a new top-level item, ending the previous one's scope.
fn starts_top_level_item(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    if first.is_whitespace() {
        return false;
    }
    line.starts_with('}')
        || [
            "fn ",
            "pub fn ",
            "async fn ",
            "pub async fn ",
            "impl ",
            "mod ",
        ]
        .iter()
        .any(|kw| line.starts_with(kw))
}

/// Whether `line` reads a calendar component off `receiver` (`a.date()`,
/// `a . year ()`, …).
fn reads_component_of(line: &str, receiver: &str) -> bool {
    let mut rest = line;
    while let Some(at) = rest.find(receiver) {
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = rest[at + receiver.len()..].trim_start();
        if before_ok
            && let Some(tail) = after.strip_prefix('.')
            && COMPONENTS.iter().any(|c| {
                tail.trim_start()
                    .strip_prefix(*c)
                    .is_some_and(|t| t.trim_start().starts_with('('))
            })
        {
            return true;
        }
        rest = &rest[at + receiver.len()..];
    }
    false
}

/// The local bound to the UTC clock on `line`, if any.
///
/// Any `let x = … now_utc() …;` counts, not only a binding whose whole
/// right-hand side is the call: `let now = now_utc() - Duration::days(14);`
/// and `let now = OffsetDateTime::now_utc().replace_time(midnight);` are the
/// same clock one operator further on, and `now.day()` on either answers the
/// UTC day.
///
/// A right-hand side that names a German-local helper is the *fix*, not the
/// bug — `berlin_date(now_utc())` is a Berlin date, and reading a component off
/// it is correct.
fn binding_of_now_utc(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let (name, tail) = rest.split_once('=')?;
    let name = name.split(':').next()?.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let tail = tail.trim();
    if tail.contains("berlin_") || tail.contains("heute(") {
        return None;
    }
    tail.contains("now_utc()").then(|| name.to_owned())
}

/// The local rebound on `line` to anything at all.
fn rebound_name(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let (name, _) = rest.split_once('=')?;
    let name = name.split(':').next()?.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| name.to_owned())
}

/// The one zone a German business date may be read in.
const BERLIN: &str = "'EUROPE/BERLIN'";

/// Whether `line` derives a calendar date or component from the SQL clock.
///
/// `now()` itself is fine — it is an instant, and `TIMESTAMPTZ DEFAULT now()`
/// is the right way to stamp one. What is refused is casting or extracting a
/// *civil* value out of it without naming the time zone, in any of the
/// spellings Postgres offers: `now()::date`, `cast(now() as date)`,
/// `date(now())`, `date_trunc('day', now())::date` and the cast chain
/// `now()::timestamp::date` all answer the same wrong question.
///
/// And `AT TIME ZONE` is the sharpest of them. Every schema defines `heute()`
/// as `(now() AT TIME ZONE 'Europe/Berlin')::date`, so the wrong-zone twin is
/// one word away and reads as deliberate: it names *a* zone, which is exactly
/// what the correct form does. A civil value taken in any other zone is a
/// finding.
fn sql_reads_the_clock(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    if upper.contains("CURRENT_DATE") || upper.contains("LOCALTIMESTAMP") {
        return true;
    }
    let squeezed: String = upper.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.contains("CURRENT_TIMESTAMP::DATE")
        || squeezed.contains("TO_CHAR(NOW()")
        || squeezed.contains("CAST(NOW()ASDATE")
        || squeezed.contains("DATE(NOW())")
        || (squeezed.contains("FROMNOW())") && squeezed.contains("EXTRACT("))
        || cast_to_date_after_now(&squeezed)
        || civil_value_in_another_zone(&squeezed)
}

/// Whether a `now()` is cast to a date, through any chain of casts and parens.
///
/// `now()::date`, `now()::timestamp::date` and `date_trunc('day', now())::date`
/// are one rule: after the call, nothing but `)` and `::<type>` stands between
/// the clock and the date it is reduced to.
fn cast_to_date_after_now(squeezed: &str) -> bool {
    let mut rest = squeezed;
    while let Some(at) = rest.find("NOW()") {
        let mut after = &rest[at + "NOW()".len()..];
        loop {
            if let Some(tail) = after.strip_prefix(')') {
                after = tail;
                continue;
            }
            let Some(tail) = after.strip_prefix("::") else {
                break;
            };
            let ident: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if ident.is_empty() {
                break;
            }
            if ident == "DATE" {
                return true;
            }
            after = &tail[ident.len()..];
        }
        rest = &rest[at + "NOW()".len()..];
    }
    false
}

/// Whether a value is moved to a zone other than Berlin and then read as a
/// civil date or component.
fn civil_value_in_another_zone(squeezed: &str) -> bool {
    let extracted = squeezed.contains("EXTRACT(") || squeezed.contains("TO_CHAR(");
    let mut rest = squeezed;
    while let Some(at) = rest.find("ATTIMEZONE") {
        let after = &rest[at + "ATTIMEZONE".len()..];
        rest = after;
        if after.starts_with(BERLIN) {
            continue;
        }
        // Step over the zone literal to see what is done with the result.
        let tail = after
            .strip_prefix('\'')
            .and_then(|t| t.find('\'').map(|i| &after[i + 2..]))
            .unwrap_or(after);
        if tail.trim_start_matches(')').starts_with("::DATE") || extracted {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{is_exempt, offending_lines};
    use std::path::Path;

    #[test]
    fn flags_the_inline_rust_idioms() {
        let src = "let today = OffsetDateTime::now_utc().date();\n\
                   let year = time::OffsetDateTime::now_utc().year();\n\
                   let m = OffsetDateTime::now_utc().month() as u8;\n";
        assert_eq!(offending_lines(src).len(), 3);
    }

    #[test]
    fn flags_a_component_read_through_a_binding() {
        // The shape every worker loop has: bind the clock, read the calendar
        // several lines later.
        let src = "fn worker() {\n\
                   \x20   let now = time::OffsetDateTime::now_utc();\n\
                   \x20   let label = something(&now);\n\
                   \x20   let day = now.day();\n\
                   }\n";
        let hits = offending_lines(src);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].0.contains("now.day()"));
    }

    #[test]
    fn an_instant_is_not_a_business_date() {
        // Binding the clock and passing it on as an instant is the correct use.
        let src = "fn worker() {\n\
                   \x20   let now = time::OffsetDateTime::now_utc();\n\
                   \x20   record(now);\n\
                   \x20   let due = now + Duration::hours(6);\n\
                   }\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn a_binding_does_not_leak_into_the_next_item() {
        let src = "fn a() {\n\
                   \x20   let now = time::OffsetDateTime::now_utc();\n\
                   }\n\
                   fn b(now: time::Date) -> i32 {\n\
                   \x20   now.year()\n\
                   }\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn rebinding_the_name_ends_its_life_as_the_clock() {
        let src = "fn a() {\n\
                   \x20   let now = time::OffsetDateTime::now_utc();\n\
                   \x20   let now = mako_fristen::berlin_now();\n\
                   \x20   let d = now.day();\n\
                   }\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn flags_the_sql_idioms() {
        let src = "\"SELECT 1 WHERE d <= CURRENT_DATE\"\n\
                   \"SELECT now()::date\"\n\
                   \"... DEFAULT extract(year FROM now())\"\n\
                   \"... DEFAULT 'RV-' || to_char(now(), 'YYYY')\"\n";
        assert_eq!(offending_lines(src).len(), 4);
    }

    /// The spellings a per-idiom list misses. Postgres offers five ways to
    /// reduce `now()` to a civil date and they all answer the UTC one.
    #[test]
    fn flags_every_spelling_of_a_civil_date_off_the_clock() {
        let src = "\"SELECT date_trunc('day', now())::date\"\n\
                   \"SELECT CAST(now() AS date)\"\n\
                   \"SELECT date(now())\"\n\
                   \"SELECT now()::timestamp::date\"\n\
                   \"SELECT now()::date\"\n";
        assert_eq!(offending_lines(src).len(), 5, "{:?}", offending_lines(src));
    }

    /// The wrong-zone twin of `heute()`. It names *a* zone, which is what the
    /// correct form does, so nothing but the zone itself gives it away.
    #[test]
    fn flags_a_civil_value_taken_in_another_zone() {
        let src = "\"SELECT (now() AT TIME ZONE 'UTC')::date\"\n\
                   \"SELECT extract(year FROM (now() AT TIME ZONE 'UTC'))\"\n";
        assert_eq!(offending_lines(src).len(), 2, "{:?}", offending_lines(src));
    }

    /// …and the form every schema defines `heute()` with must pass, or the
    /// rule above would flag the definition it is measured against.
    #[test]
    fn the_berlin_zone_is_the_correct_form() {
        let src = "\"CREATE FUNCTION heute() RETURNS date AS $$ \
                   SELECT (now() AT TIME ZONE 'Europe/Berlin')::date $$\"\n\
                   \"SELECT created_at AT TIME ZONE 'UTC' AS shown\"\n";
        assert!(
            offending_lines(src).is_empty(),
            "{:?}",
            offending_lines(src)
        );
    }

    /// The clock one operator further on is still the clock.
    #[test]
    fn a_derived_binding_is_still_the_utc_clock() {
        let src = "fn history() {\n\
                   \x20   let since = time::OffsetDateTime::now_utc() - Duration::days(30);\n\
                   \x20   let label = since.date();\n\
                   }\n";
        let hits = offending_lines(src);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].0.contains("since.date()"));
    }

    /// A right-hand side that converts to German local time is the fix.
    #[test]
    fn a_berlin_conversion_is_not_a_clock_binding() {
        let src = "fn a() {\n\
                   \x20   let today = mako_fristen::berlin_date(OffsetDateTime::now_utc());\n\
                   \x20   let y = today.year();\n\
                   }\n";
        assert!(
            offending_lines(src).is_empty(),
            "{:?}",
            offending_lines(src)
        );
    }

    #[test]
    fn an_sql_instant_column_is_not_a_business_date() {
        let src = "\"created_at TIMESTAMPTZ NOT NULL DEFAULT now()\"\n\
                   \"version TIMESTAMPTZ NOT NULL DEFAULT date_trunc('second', now())\"\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn accepts_the_berlin_forms() {
        let src = "let today = mako_fristen::heute();\n\
                   let d = mako_fristen::berlin_date(instant);\n\
                   \"SELECT 1 WHERE d <= heute()\"\n\
                   \"... DEFAULT extract(year FROM heute())\"\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn prose_naming_the_rule_is_not_the_rule() {
        let src = "/// `now_utc().date()` answers the UTC date, not the German one.\n\
                   -- `current_date` is the session time zone's date.\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn the_wire_encoders_are_exempt() {
        assert!(is_exempt(Path::new(
            "crates/edi-energy/src/builders/pricat.rs"
        )));
        assert!(is_exempt(Path::new(
            "services/makod/src/transport/as4_sender.rs"
        )));
        assert!(!is_exempt(Path::new("services/marktd/src/mmma_worker.rs")));
    }
}
