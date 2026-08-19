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

use rubo4e::current::Zaehlertyp;

fn migration_sql() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_initial.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Extract the quoted values of the `CHECK (<column> ... IN (...))` list.
fn check_values(sql: &str, column: &str) -> Vec<String> {
    let anchor = format!("CHECK ({column} IS NULL OR {column} IN (");
    let start = sql
        .find(&anchor)
        .unwrap_or_else(|| panic!("no CHECK list found for column `{column}`"))
        + anchor.len();
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
