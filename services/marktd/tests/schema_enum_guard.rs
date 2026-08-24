//! Guards the hand-written SQL `CHECK` lists against the enums they mirror.
//!
//! A `CHECK` list is opaque text to the compiler, so nothing otherwise ties it to
//! the BO4E enum it reproduces. §42c Energy-Sharing eligibility reads
//! `zaehler_typ`, and a value the enum does not know deserialises to `UNKNOWN`
//! rather than failing, so drift here degrades a delivery point silently.
//!
//! Since rubo4e 0.8 these guards are **structural**: every enum exposes
//! `VARIANTS` / `COUNT` / `from_wire` / `as_wire` without the `strum` feature, so
//! the CHECK list is proved against the enum itself rather than a hand-maintained
//! magic number.
//!
//! `UNKNOWN` is not admitted. It is BO4E's forward-compatibility catch-all, not
//! a schema variant, and the device `PUT` handlers run
//! `Bo4eStrict::ensure_known_enums` before deriving the column — an
//! unrecognised Zählertyp is a 422 naming the field, so the column never needs
//! to hold one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rubo4e::current::{Bilanzierungsmethode, Energierichtung, Gasqualitaet, Zaehlertyp};

fn migration_sql() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_initial.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Extract the quoted values of the `CHECK (<column> ... IN (...))` list.
///
/// Both spellings the migrations use are accepted: the nullable form
/// `CHECK (col IS NULL OR col IN (…))` and the bare `CHECK (col IN (…))`. A
/// parser that knew only one silently reported "no CHECK list" for every column
/// written the other way, which is a guard that passes by not looking.
fn check_values(sql: &str, column: &str) -> Vec<String> {
    // Collapse whitespace first. The migrations align columns with runs of
    // spaces and wrap long lists over several lines, so an anchor written with
    // single spaces misses `CHECK (von_typ  IN (` by one character — and a
    // missed anchor reads as "no CHECK list", which is a guard that passes by
    // not looking.
    let sql: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let sql = sql.as_str();
    let anchors = [
        format!("CHECK ({column} IS NULL OR {column} IN ("),
        format!("CHECK ({column} IN ("),
    ];
    let (anchor, at) = anchors
        .iter()
        .find_map(|a| sql.find(a.as_str()).map(|i| (a, i)))
        .unwrap_or_else(|| panic!("no CHECK list found for column `{column}`"));
    let start = at + anchor.len();
    let end = start
        + sql[start..]
            .find("))")
            .unwrap_or_else(|| panic!("unterminated CHECK list for `{column}`"));

    sql[start..end]
        .split(',')
        .filter_map(|tok| {
            let t = tok.trim();
            t.strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
                .map(str::to_owned)
        })
        .collect()
}

/// Every value in the `zaehler_typ` CHECK list must be a real BO4E `Zaehlertyp`.
///
/// `from_wire` is the strict parse: it returns `Err` for typos, legacy codes and
/// the literal `"UNKNOWN"`.
#[test]
fn zaehler_typ_check_values_are_real_bo4e_values() {
    let sql = migration_sql();
    let values = check_values(&sql, "zaehler_typ");
    assert!(!values.is_empty(), "CHECK list parsed as empty");

    for v in &values {
        assert!(
            Zaehlertyp::from_wire(v).is_ok(),
            "`{v}` in the zaehler_typ CHECK list is not a real BO4E Zaehlertyp wire value"
        );
    }
}

/// The CHECK list must cover **exactly** the enum — every variant, and nothing
/// stale.
///
/// Proved by set-equality against `Zaehlertyp::VARIANTS` (via `as_wire()`), which
/// is stable for the schema version and available without `strum`. When a BO4E
/// release adds or removes a Zaehlertyp this fails with the precise delta — the
/// list needs a deliberate decision, not silent divergence.
#[test]
fn zaehler_typ_check_covers_every_bo4e_variant() {
    let sql = migration_sql();
    let list: BTreeSet<String> = check_values(&sql, "zaehler_typ").into_iter().collect();

    let enum_wire: BTreeSet<String> = Zaehlertyp::VARIANTS
        .iter()
        .map(|v| v.as_wire().to_owned())
        .collect();

    assert_eq!(
        list.len(),
        Zaehlertyp::COUNT,
        "unexpected duplicate(s) in CHECK list"
    );
    let missing: Vec<&String> = enum_wire.difference(&list).collect();
    let extra: Vec<&String> = list.difference(&enum_wire).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "zaehler_typ CHECK list is out of sync with BO4E Zaehlertyp — \
         missing {missing:?}, stale {extra:?}. Reconcile the migration with rubo4e."
    );
}

/// `Zaehlertyp` and `Geraetetyp` disagree on how many `s` belong in
/// "Messsystem" (an upstream BO4E divergence, documented on both enums since
/// rubo4e 0.8); this pins the `Zaehlertyp` spelling to the enum's own `as_wire()`.
#[test]
fn imsys_spelling_is_the_zaehlertyp_one() {
    let sql = migration_sql();
    let values = check_values(&sql, "zaehler_typ");

    // The authoritative spelling comes from the enum itself, not a string literal.
    let canonical = Zaehlertyp::IntelligentesMesssystem.as_wire();
    assert_eq!(
        canonical, "INTELLIGENTES_MESSSYSTEM",
        "Zaehlertyp uses three s"
    );

    assert!(
        values.iter().any(|v| v == canonical),
        "zaehler_typ CHECK list must carry the Zaehlertyp spelling {canonical}"
    );
    assert!(
        !values.iter().any(|v| v == "INTELLIGENTES_MESSYSTEM"),
        "INTELLIGENTES_MESSYSTEM (two s) is the Geraetetyp spelling, not Zaehlertyp"
    );
    assert!(
        !sql.contains("INTELLIGENTESMESSYSTEM"),
        "the underscore-less spelling exists in no BO4E enum"
    );
}

// ── Every BO4E-enum-backed column, not just `zaehler_typ` ────────────────────

/// Prove one column's CHECK list is exactly its BO4E enum.
///
/// Generic over the enum so the check reads the same for every column and a new
/// one costs a single line in [`every_bo4e_backed_check_matches_its_enum`].
fn assert_check_matches_enum<E: rubo4e::Bo4eEnum>(sql: &str, column: &str) {
    let list: BTreeSet<String> = check_values(sql, column).into_iter().collect();
    assert!(
        !list.is_empty(),
        "CHECK list for `{column}` parsed as empty"
    );

    let enum_wire: BTreeSet<String> = E::VARIANTS.iter().map(|v| v.as_wire().to_owned()).collect();

    assert_eq!(
        list.len(),
        E::COUNT,
        "`{column}` CHECK list has {} entries against {} enum variants — duplicates?",
        list.len(),
        E::COUNT
    );
    let missing: Vec<&String> = enum_wire.difference(&list).collect();
    let extra: Vec<&String> = list.difference(&enum_wire).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "`{column}` CHECK list is out of sync with its BO4E enum — \
         missing {missing:?}, stale {extra:?}. Reconcile the migration with rubo4e."
    );
}

/// Every column whose domain **is** a BO4E enum is proved against that enum.
///
/// `zaehler_typ` had this guard alone; the other three were hand-written lists
/// that nothing tied to the schema. BO4E changes enum membership *inside* a
/// series — v202607.0.0 → v202607.1.0 removed `Messgroesse::PREISE` and two
/// whole enums — so a list left unguarded goes stale on a patch release, and a
/// value the enum no longer knows deserialises to `Unknown` rather than failing.
///
/// Columns whose vocabulary is **mako's own** (`fallgruppe`, `netzebene`,
/// `dispatch_status`, …) are deliberately not here: they are not BO4E enums and
/// have no upstream to drift from.
#[test]
fn every_bo4e_backed_check_matches_its_enum() {
    let sql = migration_sql();
    assert_check_matches_enum::<Zaehlertyp>(&sql, "zaehler_typ");
    assert_check_matches_enum::<Gasqualitaet>(&sql, "gasqualitaet");
    assert_check_matches_enum::<Energierichtung>(&sql, "energierichtung");
    assert_check_matches_enum::<Bilanzierungsmethode>(&sql, "bilanzierungsmethode");
}

/// The enums BO4E **removed** in v202607.1.0 must not reappear as a column
/// domain.
///
/// `Lokationstyp` and `Mengenoperator` are gone from the schema, and
/// `Messgroesse::PREISE` with them. `Lokationstyp` still backs mako's
/// Lokationszuordnung graph — but as [`mako_markt::domain::Lokationstyp`], a
/// mako-owned enum with the same five wire values, precisely so a future schema
/// release cannot delete a column's domain out from under it again.
#[test]
fn the_removed_bo4e_enums_are_not_a_column_domain() {
    let sql = migration_sql();
    assert!(
        !sql.contains("'PREISE'"),
        "Messgroesse::PREISE was removed in BO4E v202607.1.0"
    );

    // The graph columns keep the five values; they are mako's now.
    let von = check_values(&sql, "von_typ");
    let expected: BTreeSet<&str> = mako_markt::domain::Lokationstyp::VARIANTS
        .iter()
        .copied()
        .collect();
    let actual: BTreeSet<&str> = von.iter().map(String::as_str).collect();
    assert_eq!(
        actual, expected,
        "von_typ must carry exactly mako's own Lokationstyp values"
    );
}
