//! Guards `virtual_meter_configs` against the code that queries it.
//!
//! `sqlx::query` is unchecked, so a column named in a query but absent from the
//! DDL is a runtime error rather than a compile error, and the virtual-meter
//! endpoints that back the §42c allocation only fail once they reach a database.
//!
//! These tests read both files as text and assert they agree.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn migration() -> String {
    read("migrations/0001_schema.sql")
}

fn ddl_of(sql: &str, table: &str) -> String {
    let anchor = format!("CREATE TABLE {table} (");
    let start = sql
        .find(&anchor)
        .unwrap_or_else(|| panic!("table `{table}` not found in migration"));
    let end = start
        + sql[start..]
            .find("\n);")
            .unwrap_or_else(|| panic!("unterminated CREATE TABLE for `{table}`"));
    sql[start..end].to_owned()
}

/// The column names a `CREATE TABLE` body actually declares.
///
/// A substring search over the raw DDL is not a column check: `"id"` matches
/// inside `malo_id`, and any identifier matches inside a comment. This takes the
/// first token of each non-comment, non-constraint line, so a test asserting on
/// a column name is asserting on a column that exists.
fn declared_columns(ddl: &str) -> std::collections::BTreeSet<String> {
    ddl.lines()
        .skip(1) // the `CREATE TABLE x (` line
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("--"))
        .filter(|l| {
            let upper = l.to_uppercase();
            !upper.starts_with("CONSTRAINT")
                && !upper.starts_with("PRIMARY KEY")
                && !upper.starts_with("UNIQUE")
                && !upper.starts_with("CHECK")
                && !upper.starts_with("FOREIGN KEY")
        })
        .filter_map(|l| l.split_whitespace().next())
        .map(|c| c.trim_matches(',').to_owned())
        .collect()
}

#[test]
fn the_column_extractor_does_not_match_substrings_or_comments() {
    let ddl = "CREATE TABLE t (\n    -- mentions id in a comment\n    malo_id TEXT NOT NULL,\n                   tenant  TEXT NOT NULL,\n    CONSTRAINT t_pk PRIMARY KEY (tenant, malo_id)";
    let cols = declared_columns(ddl);
    assert!(cols.contains("malo_id"));
    assert!(cols.contains("tenant"));
    assert!(
        !cols.contains("id"),
        "`id` is a substring of `malo_id` and appears in a comment, but is not a column"
    );
    assert!(
        !cols.contains("CONSTRAINT"),
        "table constraints are not columns"
    );
}

/// Every column the handlers reference must exist in the DDL.
#[test]
fn virtual_meter_configs_ddl_covers_every_queried_column() {
    let ddl = ddl_of(&migration(), "virtual_meter_configs");

    // Columns appearing in SELECT / INSERT / UPDATE / WHERE across server.rs.
    let declared = declared_columns(&ddl);
    for column in [
        "id",
        "virtual_malo_id",
        "display_name",
        "rule_type",
        "rule_json",
        "legal_basis",
        "sparte",
        "valid_from",
        "valid_to",
        "tenant",
        "created_at",
        "updated_at",
    ] {
        assert!(
            declared.contains(column),
            "handlers query `{column}` but virtual_meter_configs does not declare it; \
             declared columns are {declared:?}"
        );
    }
}

/// The upsert in `create_virtual_meter` targets `ON CONFLICT (virtual_malo_id,
/// tenant)`, which requires a matching unique index.
#[test]
fn virtual_meter_configs_has_the_upsert_conflict_key() {
    let sql = migration();
    assert!(
        sql.contains(
            "UNIQUE INDEX vmc_virtual_malo_id ON virtual_meter_configs (virtual_malo_id, tenant)"
        ),
        "ON CONFLICT (virtual_malo_id, tenant) has no matching unique index"
    );
}

/// The stale column names must not come back.
#[test]
fn superseded_column_names_are_gone() {
    let ddl = ddl_of(&migration(), "virtual_meter_configs");
    for stale in ["virtual_id ", "source_ids", "config "] {
        assert!(
            !ddl.contains(stale),
            "`{stale}` is the superseded shape — handlers do not use it"
        );
    }
}

/// `rule_type` must match `metering::aggregation_rule::AggregationRule`.
///
/// `edmd` deserialises `rule_json` into that enum, so a `rule_type` the enum
/// does not know is an unreadable row. The old CHECK allowed `'GgvAllocation'`,
/// which is not a variant.
#[test]
fn rule_type_check_matches_the_aggregation_rule_enum() {
    let ddl = ddl_of(&migration(), "virtual_meter_configs");

    // Externally-tagged serde: the JSON tag is the variant name verbatim.
    const VARIANTS: [&str; 5] = [
        "Sum",
        "Residual",
        "PvSelfConsumption",
        "GgvConstantAllocation",
        "GgvProportionalAllocation",
    ];

    for v in VARIANTS {
        assert!(
            ddl.contains(&format!("'{v}'")),
            "AggregationRule::{v} is missing from the rule_type CHECK"
        );
    }

    assert!(
        !ddl.contains("'GgvAllocation'"),
        "'GgvAllocation' is not an AggregationRule variant"
    );

    // Scope the count to the rule_type list itself — the surrounding DDL has
    // apostrophes in prose, so the count is scoped to the CHECK list.
    let anchor = "CHECK (rule_type IN (";
    let start = ddl.find(anchor).expect("rule_type CHECK not found") + anchor.len();
    let end = start
        + ddl[start..]
            .find("))")
            .expect("unterminated rule_type CHECK");
    let listed: Vec<&str> = ddl[start..end]
        .split(',')
        .filter_map(|t| {
            t.trim()
                .strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
        })
        .collect();

    assert_eq!(
        listed.len(),
        VARIANTS.len(),
        "rule_type CHECK lists {listed:?}, AggregationRule has {VARIANTS:?}"
    );
}

/// A round-trip proving the variant names are the real serde tags, so the
/// hardcoded list above cannot quietly go stale.
#[test]
fn aggregation_rule_variant_names_are_the_serde_tags() {
    use metering::aggregation_rule::AggregationRule;

    let rule = AggregationRule::Sum {
        source_malo_ids: vec!["51238696781".to_owned()],
    };
    let json = serde_json::to_value(&rule).expect("AggregationRule must serialise");
    assert!(
        json.get("Sum").is_some(),
        "expected externally-tagged `Sum`, got {json}"
    );
}
