//! Guards the hand-written SQL `CHECK` lists against the enums they mirror.
//!
//! A `CHECK` list is opaque text to the compiler, so nothing otherwise ties it to
//! the BO4E enum it reproduces. §42c Energy-Sharing eligibility reads
//! `zaehler_typ`, and a value the enum does not know deserialises to `UNKNOWN`
//! rather than failing, so drift here degrades a delivery point silently.

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
/// Postgres accepts any string here, so an invalid one is only detected when it
/// fails to match what the application writes.
#[test]
fn zaehler_typ_check_values_are_real_bo4e_values() {
    let sql = migration_sql();
    let values = check_values(&sql, "zaehler_typ");
    assert!(!values.is_empty(), "CHECK list parsed as empty");

    for v in &values {
        let parsed: Result<Zaehlertyp, _> =
            serde_json::from_value(serde_json::Value::String(v.clone()));
        let parsed = parsed.unwrap_or_else(|e| panic!("`{v}` is not a BO4E Zaehlertyp: {e}"));

        // `Unknown` is BO4E's forward-compatibility catch-all: unrecognised wire
        // values deserialise into it rather than failing, so it must be excluded
        // for the check above to have force.
        if v != "UNKNOWN" {
            assert_ne!(
                parsed,
                Zaehlertyp::Unknown,
                "`{v}` fell through to Zaehlertyp::Unknown — it is not a real variant"
            );
        }
    }
}

/// The CHECK list must cover the whole enum.
///
/// Pinned by count: `rubo4e` does not expose variant iteration without the
/// `strum` feature, so a bare count is the available signal. When a BO4E release
/// adds a Zaehlertyp this fails, which is the point — the list needs a decision,
/// not silent divergence.
#[test]
fn zaehler_typ_check_covers_every_bo4e_variant() {
    let sql = migration_sql();
    let values = check_values(&sql, "zaehler_typ");

    const BO4E_V202607_ZAEHLERTYP_VARIANTS: usize = 14;
    assert_eq!(
        values.len(),
        BO4E_V202607_ZAEHLERTYP_VARIANTS,
        "zaehler_typ CHECK has {} values, BO4E v202607 Zaehlertyp has {}. \
         Reconcile the migration with rubo4e and update this constant.",
        values.len(),
        BO4E_V202607_ZAEHLERTYP_VARIANTS
    );
}

/// `Zaehlertyp` and `Geraetetyp` disagree on how many `s` belong in
/// "Messsystem"; this pins the `Zaehlertyp` spelling.
#[test]
fn imsys_spelling_is_the_zaehlertyp_one() {
    let sql = migration_sql();
    let values = check_values(&sql, "zaehler_typ");

    assert!(
        values.iter().any(|v| v == "INTELLIGENTES_MESSSYSTEM"),
        "Zaehlertyp uses INTELLIGENTES_MESSSYSTEM (three s)"
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
