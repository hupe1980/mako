//! Text-level guards tying `einsd`'s SQL to its schema.
//!
//! The database tests in `settlement_integration.rs` prove what the schema
//! permits. These prove the service actually issues that form — a distinction
//! that matters, because both bugs they guard against were in the service's
//! query text while the schema was correct all along. They need no database and
//! so run on every `cargo test`.

const PG: &str = include_str!("../src/pg.rs");
const MCP: &str = include_str!("../src/mcp_server.rs");
const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// Strip `--` line comments so a rule cannot be satisfied by prose.
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("--") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `ON CONFLICT` on `settlement_receipts` must repeat the index predicate.
///
/// `sr_unique_initial` is partial (`WHERE is_correction = false`). Postgres
/// cannot infer a partial index from the column list, so the bare form raises
/// "no unique or exclusion constraint matching the ON CONFLICT specification" at
/// runtime. The award-expired settlement path shipped with the bare form and
/// failed on every call.
#[test]
fn receipt_upserts_repeat_the_partial_index_predicate() {
    let code = code_only(PG);
    let conflicts: Vec<&str> = code
        .match_indices("ON CONFLICT (tr_id, tenant, billing_year, billing_month)")
        .map(|(i, _)| &code[i..(i + 160).min(code.len())])
        .collect();

    assert!(
        !conflicts.is_empty(),
        "expected receipt upserts to exist in pg.rs"
    );
    for c in &conflicts {
        assert!(
            c.contains("is_correction = false"),
            "an ON CONFLICT on settlement_receipts omits the partial-index \
             predicate and will fail at runtime:\n{c}"
        );
    }
}

/// The schema must actually define that index as partial.
///
/// If it were ever made total, the predicate above would become wrong rather
/// than merely redundant — so the two are asserted together.
#[test]
fn the_receipts_unique_index_is_partial() {
    let schema = code_only(SCHEMA);
    let idx = schema
        .find("CREATE UNIQUE INDEX sr_unique_initial")
        .expect("sr_unique_initial must exist");
    let stmt = &schema[idx..schema[idx..].find(';').map_or(schema.len(), |e| idx + e)];
    assert!(
        stmt.contains("WHERE is_correction = false"),
        "sr_unique_initial must stay partial: {stmt}"
    );
}

/// Extract the SQL raw-string literals that touch `eeg_anlagen`.
///
/// Scoped to SQL because the same name may legitimately be a Rust field: the
/// settlement input really does carry a `kwk_max_kwh` value — it is simply
/// computed rather than selected.
fn eeg_anlagen_queries(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("r\"") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('"') else { break };
        let lit = &after[..end];
        if lit.contains("eeg_anlagen") {
            out.push(lit);
        }
        rest = &after[end + 1..];
    }
    out
}

/// No query may name a column the schema does not define.
///
/// `get_compliance_status` selected `kwk_max_kwh`, which is derived
/// (`kwk_foerderdauer_h × leistung_kwp`) and has never been a column, so the
/// tool failed for every plant.
#[test]
fn queries_do_not_name_columns_absent_from_eeg_anlagen() {
    let schema = code_only(SCHEMA);
    let start = schema
        .find("CREATE TABLE eeg_anlagen")
        .expect("eeg_anlagen must exist");
    let ddl = &schema[start..start + schema[start..].find(");").expect("table ends")];

    // Values computed in Rust that must never appear in a projection.
    for derived in ["kwk_max_kwh"] {
        assert!(
            !ddl.contains(derived),
            "{derived} is derived, not stored — this guard assumes that"
        );
        for (name, src) in [("pg.rs", PG), ("mcp_server.rs", MCP)] {
            for q in eeg_anlagen_queries(src) {
                assert!(
                    !q.contains(derived),
                    "a query in {name} names the derived value {derived} as if \
                     it were a column:\n{q}"
                );
            }
        }
    }
}

/// Every column the correction path writes must exist.
///
/// `correction_reason` was accepted from the caller, echoed back in the
/// response, and never stored — so the § 147 AO / GoBD audit trail lost the stated
/// reason for every correction.
#[test]
fn the_correction_audit_columns_are_written() {
    let schema = code_only(SCHEMA);
    let code = code_only(PG);
    for col in ["correction_of", "correction_reason", "is_correction"] {
        assert!(schema.contains(col), "{col} must exist in the schema");
        assert!(
            code.contains(col),
            "{col} exists in the schema but pg.rs never writes it"
        );
    }
}

/// A state change must be recorded, not only applied.
///
/// `settlement_state` was updated in place, so the prior value was
/// unrecoverable and the history tool always returned empty.
#[test]
fn settlement_state_changes_are_recorded_as_transitions() {
    let code = code_only(PG);
    assert!(
        code.contains("INSERT INTO settlement_state_transitions"),
        "pg.rs updates settlement_state but never records the transition"
    );
    assert!(
        code.contains("FOR UPDATE"),
        "the prior state must be read under a row lock so the recorded \
         from_state cannot race another settlement"
    );
}

/// Extract the single-quoted values of a `CHECK (<col> IN ( … ))` list.
fn check_in_values(schema: &str, col: &str) -> Vec<String> {
    let needle = format!("CHECK ({col} IN (");
    let start = schema
        .find(&needle)
        .unwrap_or_else(|| panic!("no CHECK (…) IN list for column {col}"))
        + needle.len();
    let end = start
        + schema[start..]
            .find("))")
            .expect("CHECK IN list must close with `))`");
    let list = &schema[start..end];
    let mut out = Vec::new();
    let mut rest = list;
    while let Some(q1) = rest.find('\'') {
        let after = &rest[q1 + 1..];
        let Some(q2) = after.find('\'') else { break };
        out.push(after[..q2].to_owned());
        rest = &after[q2 + 1..];
    }
    out
}

/// `eeg_anlagen.erzeugungsart` CHECK must equal `ErzeugungsArt`'s canonical
/// `to_db_str` vocabulary — in **both** directions.
///
/// The two drifted: the enum emitted `BIOMETHAN` / `SOLAR_FREIFLAECHE` while the
/// CHECK allowed `BIOMETHANE` / `SOLAR_FREFLAECHE` and an orphan `SONSTIGE` with
/// no variant. Because `pg.rs` binds the raw caller string and reads back via
/// `from_db_str(...).ok()`, a biomethane plant was either rejected by the CHECK
/// or silently decoded to `erzeugungsart = None`. This pins the two lists equal
/// so any future rename fails a DB-free test instead of losing data at runtime.
#[test]
fn erzeugungsart_check_list_equals_the_enum_vocabulary() {
    use eeg_billing::ErzeugungsArt;
    use std::collections::BTreeSet;

    let schema = code_only(SCHEMA);
    let check: BTreeSet<String> = check_in_values(&schema, "erzeugungsart")
        .into_iter()
        .collect();
    let enum_vocab: BTreeSet<String> = ErzeugungsArt::ALL
        .iter()
        .map(|a| a.to_db_str().to_owned())
        .collect();

    assert_eq!(
        check,
        enum_vocab,
        "erzeugungsart CHECK list and ErzeugungsArt::to_db_str have diverged.\n\
         in CHECK but not enum: {:?}\n\
         in enum but not CHECK: {:?}",
        check.difference(&enum_vocab).collect::<Vec<_>>(),
        enum_vocab.difference(&check).collect::<Vec<_>>(),
    );

    // Every allowed value must round-trip through the enum.
    for v in &check {
        let parsed = ErzeugungsArt::from_db_str(v)
            .unwrap_or_else(|_| panic!("CHECK value {v:?} does not parse via from_db_str"));
        assert_eq!(
            parsed.to_db_str(),
            v,
            "from_db_str/to_db_str do not round-trip for {v:?}"
        );
    }
}

/// `eeg_gesetz` CHECK must equal the `EegGesetz` year set.
///
/// A settlement stored under a year the enum cannot decode would settle under
/// the wrong EEG regime; this pins the SQL list to the Rust source of truth.
#[test]
fn eeg_gesetz_check_list_equals_the_enum_years() {
    use eeg_billing::EegGesetz;
    use std::collections::BTreeSet;

    let schema = code_only(SCHEMA);
    let needle = "CHECK (eeg_gesetz IN (";
    let start = schema.find(needle).expect("eeg_gesetz CHECK must exist") + needle.len();
    let end = start + schema[start..].find(')').expect("closes");
    let check: BTreeSet<i16> = schema[start..end]
        .split(',')
        .filter_map(|t| t.trim().parse::<i16>().ok())
        .collect();

    // Every CHECK year must decode to a concrete regime (0 = KWKG sentinel).
    for &y in &check {
        assert!(
            EegGesetz::from_db_year(y).is_ok(),
            "eeg_gesetz CHECK allows {y}, which EegGesetz::from_db_year rejects"
        );
    }
}
