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

/// Extract the quoted values of a `CHECK (column IN (...))` list.
fn check_list(ddl: &str, column: &str) -> Vec<String> {
    let anchor = format!("CHECK ({column} IN (");
    let start = ddl
        .find(&anchor)
        .unwrap_or_else(|| panic!("CHECK for `{column}` not found"))
        + anchor.len();
    let end = start
        + ddl[start..]
            .find("))")
            .unwrap_or_else(|| panic!("unterminated CHECK for `{column}`"));
    ddl[start..end]
        .split(',')
        .filter_map(|t| {
            t.trim()
                .strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
                .map(str::to_owned)
        })
        .collect()
}

// NOTE: `meter_reads` and `esa_typ2_reads` are not declared in this
// migration — they (and their `source` / `quality` / `sparte` /
// `allocation_version` / `delivery_path` CHECK constraints) are created and
// owned by the `meterstore` crate. The corresponding enum⇄CHECK guards live
// with meterstore's own hot-table DDL. edmd keeps only the routing invariant:
// 13027 is *received* but *forked* to the separate Typ-2 store.

/// 13027 must be subscribed (so edmd receives it) *and* flagged as a Typ-2 PID
/// (so the ingest handler forks it away from the authoritative store into the
/// separate `esa_typ2_reads` meterstore table).
#[test]
fn typ2_pid_13027_is_received_and_forked() {
    assert!(
        edmd::domain::MSCONS_PIDS.contains(&13027),
        "edmd must still subscribe to 13027 to receive Typ-2 values"
    );
    assert!(
        edmd::domain::ESA_TYP2_PIDS.contains(&13027),
        "PID 13027 must be in ESA_TYP2_PIDS so the ingest handler routes it to the Typ-2 store"
    );
}

/// An ESA may order value delivery from the MSB (WiM Strom Teil 2 Kap. 4 · §60
/// Abs. 1 MsbG), so a reading order can be raised on its behalf — the
/// `auftraggeber_rolle` CHECK must admit `ESA` alongside LF/MSB/NB.
#[test]
fn ablese_auftraege_auftraggeber_rolle_admits_esa() {
    let ddl = ddl_of(&migration(), "ablese_auftraege");
    let listed: std::collections::BTreeSet<String> =
        check_list(&ddl, "auftraggeber_rolle").into_iter().collect();
    for role in ["LF", "MSB", "NB", "ESA"] {
        assert!(
            listed.contains(role),
            "auftraggeber_rolle CHECK must admit {role}; got {listed:?}"
        );
    }
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

/// Every TEXT `quality` column enforces the full 8-value `QualityFlag` vocabulary
/// at the DB layer, and that CHECK list is pinned to `metering::QualityFlag::CODES`
/// — so adding a flag to the enum without updating the schema fails here, not
/// silently at runtime when a new value is read back as UNKNOWN.
#[test]
fn quality_checks_match_the_quality_flag_codes() {
    let sql = migration();
    for (table, col) in [
        ("meter_billing_periods", "quality"),
        ("meter_read_corrections", "original_quality"),
        ("meter_read_corrections", "corrected_quality"),
    ] {
        let ddl = ddl_of(&sql, table);
        for code in metering::QualityFlag::CODES {
            assert!(
                ddl.contains(&format!("'{code}'")),
                "{table}.{col}: QualityFlag code {code} missing from its CHECK list"
            );
        }
    }
}

// ── Every column named in SQL exists in the DDL ───────────────────────────────
//
// `sqlx::query` is unchecked, so a column named in a query but absent from the
// DDL is a runtime error and nothing more. The guards above pinned one table by
// hand and, precisely because the list was hand-maintained, missed the next one:
// `GET /api/v1/gas-quality/{malo_id}` and the `get_gas_quality` MCP tool both
// selected `pid` from `gas_quality_data`, whose column is `source_pid`. Every
// call to either failed, and the failure showed up as a 500 rather than as a
// broken build.
//
// This walks the source instead of a list.

/// Every table `0001_schema.sql` declares, with its column names.
fn declared_tables() -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let sql = migration();
    let mut tables = std::collections::BTreeMap::new();
    for (idx, _) in sql.match_indices("CREATE TABLE ") {
        let rest = &sql[idx + "CREATE TABLE ".len()..];
        let Some(paren) = rest.find(" (") else {
            continue;
        };
        let name = rest[..paren].trim().to_owned();
        tables.insert(name.clone(), declared_columns(&ddl_of(&sql, &name)));
    }
    tables
}

/// Every `.rs` file under `src/`.
fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir)
            .expect("readable source dir")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((
                    path.display().to_string(),
                    std::fs::read_to_string(&path).expect("readable source file"),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    out
}

/// A bare column identifier, or `None` for anything that is not one: a
/// function call, a literal, a `*`, a qualified or aliased expression, a cast.
///
/// Only unambiguous bare identifiers are checked, so the guard cannot produce a
/// false failure on SQL it does not fully parse — it is a net, not a parser.
fn bare_column(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty()
        || t == "*"
        || t.contains('(')
        || t.contains(')')
        || t.contains('\'')
        || t.contains(':')
        || t.contains('.')
        || t.contains(' ')
        || t.contains('$')
        || t.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        .then(|| t.to_ascii_lowercase())
}

/// Column lists named against `table` in `sql`: `SELECT … FROM table` and
/// `INSERT INTO table (…)`.
fn columns_referenced(sql: &str, table: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let lower = sql.to_ascii_lowercase();
    let table_lc = table.to_ascii_lowercase();

    // `INSERT INTO <table> (a, b, c)`
    let insert = format!("insert into {table_lc}");
    for (idx, _) in lower.match_indices(&insert) {
        let rest = &sql[idx + insert.len()..];
        let Some(open) = rest.find('(') else { continue };
        let Some(close) = rest[open..].find(')') else {
            continue;
        };
        // Nothing but whitespace and newlines may separate the table from its
        // column list, or this is a different statement shape.
        if rest[..open].chars().any(|c| !c.is_whitespace()) {
            continue;
        }
        found.extend(
            rest[open + 1..open + close]
                .split(',')
                .filter_map(bare_column),
        );
    }

    // `SELECT <list> FROM <table>` — only single-table statements, so a bare
    // identifier is unambiguously this table's.
    let from = format!("from {table_lc}");
    for (idx, _) in lower.match_indices(&from) {
        let before = &lower[..idx];
        let Some(select_at) = before.rfind("select ") else {
            continue;
        };
        // A join or subquery makes a bare identifier ambiguous; skip those.
        let clause = &sql[select_at + "select ".len()..idx];
        if clause.to_ascii_lowercase().contains("join") || clause.contains('(') {
            continue;
        }
        found.extend(clause.split(',').filter_map(bare_column));
    }
    found
}

/// SQL words that can appear where a column would and are not columns.
const NOT_COLUMNS: &[&str] = &["distinct", "count", "now", "true", "false", "null"];

#[test]
fn every_column_named_in_sql_is_declared_in_the_ddl() {
    let tables = declared_tables();
    let mut problems: Vec<String> = Vec::new();

    for (file, source) in source_files() {
        for (table, declared) in &tables {
            for column in columns_referenced(&source, table) {
                if declared.contains(&column) || NOT_COLUMNS.contains(&column.as_str()) {
                    continue;
                }
                problems.push(format!(
                    "{file}: `{column}` is queried on `{table}`, which declares {declared:?}"
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "SQL references columns that do not exist:\n  {}",
        problems.join("\n  ")
    );
}

/// The guard is a net with real holes; this pins that it catches the shape of
/// bug it was written for, so a future simplification cannot quietly defeat it.
#[test]
fn the_column_guard_catches_a_misnamed_select() {
    let tables = declared_tables();
    let gas = tables
        .get("gas_quality_data")
        .expect("gas_quality_data is declared");
    assert!(
        gas.contains("source_pid") && !gas.contains("pid"),
        "the column is `source_pid`, never a bare `pid`"
    );

    let bad = r#"sqlx::query("SELECT period_from, pid FROM gas_quality_data WHERE malo_id = $1")"#;
    let referenced = columns_referenced(bad, "gas_quality_data");
    assert!(
        referenced.contains("pid"),
        "the extractor must see `pid` in {referenced:?}"
    );
    assert!(
        !gas.contains("pid"),
        "and the DDL must not declare it, so the guard fires"
    );
}

/// Every `ON CONFLICT (a, b)` target has a matching unique index or primary key.
///
/// Without one PostgreSQL rejects the statement at runtime. `direct_push_sessions`
/// was keyed on `session_id` alone while every read of it was tenant-scoped, so
/// two tenants using the same session id collided: the upsert landed on the other
/// tenant's row.
#[test]
fn every_on_conflict_target_has_a_matching_unique_constraint() {
    let sql = migration();
    let mut problems = Vec::new();

    for (_file, source) in source_files() {
        for (idx, _) in source.match_indices("ON CONFLICT (") {
            let rest = &source[idx + "ON CONFLICT (".len()..];
            let Some(close) = rest.find(')') else {
                continue;
            };
            let target: Vec<String> = rest[..close]
                .split(',')
                .map(|c| c.trim().to_ascii_lowercase())
                .collect();
            // A composite key must appear as a parenthesised list — a UNIQUE
            // INDEX, a table-level PRIMARY KEY, or a UNIQUE constraint. A
            // single-column target may also be an inline `col … PRIMARY KEY`.
            let key = target.join(", ");
            let lower = sql.to_ascii_lowercase();
            let listed = lower.contains(&format!("({key})"));
            let inline_pk = target.len() == 1
                && lower.lines().any(|l| {
                    let l = l.trim();
                    l.starts_with(&target[0]) && l.contains("primary key")
                });
            if !listed && !inline_pk {
                problems.push(format!("ON CONFLICT ({key}) has no matching unique key"));
            }
        }
    }

    // `ON CONFLICT ON CONSTRAINT <name>` only accepts a *table constraint*.
    // Naming a `CREATE UNIQUE INDEX` there makes PostgreSQL reject the whole
    // statement — which is how the billing-period cache came to never populate.
    for (file, source) in source_files() {
        for (idx, _) in source.match_indices("ON CONFLICT ON CONSTRAINT ") {
            let rest = &source[idx + "ON CONFLICT ON CONSTRAINT ".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let is_table_constraint = sql.contains(&format!("CONSTRAINT {name} UNIQUE"))
                || sql.contains(&format!("CONSTRAINT {name} PRIMARY KEY"));
            if !is_table_constraint {
                problems.push(format!(
                    "{file}: ON CONFLICT ON CONSTRAINT {name} — `{name}` is not a table \
                     constraint (a CREATE UNIQUE INDEX of that name does not count); \
                     use the column list instead"
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// `quality_assessments.source` must admit every `IngestionSource`.
///
/// The column records which ingest family produced a verdict, and every door
/// now scores its batch. A variant the CHECK omits fails the insert — and the
/// insert failure is deliberately only a warning, because the readings are
/// already stored — so the audit history goes silently missing for exactly the
/// door that believed it was recording one. That is how the Kafka and bulk doors
/// would have behaved the moment they started scoring.
#[test]
fn quality_assessment_source_check_covers_every_ingestion_source() {
    let ddl = ddl_of(&migration(), "quality_assessments");
    let start = ddl
        .find("CHECK (source IN (")
        .expect("quality_assessments declares a source CHECK");
    let end = start + ddl[start..].find("))").expect("terminated CHECK");
    let listed = &ddl[start..end];

    for source in edmd::domain::IngestionSource::ALL {
        let code = source.as_str();
        assert!(
            listed.contains(&format!("'{code}'")),
            "quality_assessments.source CHECK omits `{code}`; a verdict from that \
             door would fail the insert and vanish from the history"
        );
    }
    // The one legitimate non-ingest value: a retroactive re-scoring.
    assert!(
        listed.contains("'BATCH_RESCORE'"),
        "the retroactive rescore path records BATCH_RESCORE"
    );
}

/// `zsg_conversion_log.outcome` must admit every conversion outcome.
///
/// The column records what the Zählerstandsgang → Lastgang differencing did
/// across a contested span: a reconstructed register wrap, or the `AnomalyKind`
/// that refused the difference. `AnomalyKind` is `#[non_exhaustive]` and lives
/// upstream, so a kind added there would otherwise fail the audit insert at
/// runtime — and the insert is deliberately non-fatal (the readings are already
/// stored), so the § 146 Abs. 4 AO trail would go missing with a warning.
#[test]
fn zsg_outcome_check_covers_every_conversion_outcome() {
    let ddl = ddl_of(&migration(), "zsg_conversion_log");
    let start = ddl
        .find("CHECK (outcome IN (")
        .expect("zsg_conversion_log declares an outcome CHECK");
    let end = start + ddl[start..].find("))").expect("terminated CHECK");
    let listed = &ddl[start..end];

    for outcome in edmd::domain::zsg_outcomes() {
        assert!(
            listed.contains(&format!("'{outcome}'")),
            "zsg_conversion_log.outcome CHECK omits `{outcome}`; that conversion \
             outcome would fail the audit insert and vanish from the trail"
        );
    }
}

/// A Zählerstand is stored in the unit the **register** counts.
///
/// Not the Sparte's billing unit. § 25 Nr. 4 MessEV converts the *difference*
/// between two readings; a register value rewritten into kWh is no longer the
/// number on the meter, and § 40 Abs. 2 Nr. 6 EnWG puts that number on an
/// invoice for a customer to check.
#[test]
fn meter_readings_admit_both_register_units() {
    let ddl = ddl_of(&migration(), "meter_readings");
    for unit in ["KWH", "M3"] {
        assert!(
            ddl.contains(&format!("'{unit}'")),
            "meter_readings.unit CHECK omits `{unit}` — gas and water registers count m³"
        );
    }
}

/// `cls_compliance_issues.issue_type` must admit every compliance issue type.
///
/// The sweep's insert is deliberately non-fatal — a failed registration is a
/// warning, because the fleet report is derived data — so a type the CHECK omits
/// would make that issue invisible rather than loud. The three certificate
/// faults are the live example: `is_valid` answers one boolean over revoked,
/// expired and not-yet-valid, and splitting them added two types.
#[test]
fn compliance_issue_type_check_covers_every_variant() {
    let ddl = ddl_of(&migration(), "cls_compliance_issues");
    for code in [
        "CERT_EXPIRED",
        "CERT_REVOKED",
        "CERT_NOT_YET_VALID",
        "CERT_EXPIRING",
        "TLS_CERT_MISSING",
        "CLS_NOT_COMPLIANT",
        "COMMUNICATION_FAULT",
        "GATEWAY_REVOKED",
    ] {
        assert!(
            ddl.contains(&format!("'{code}'")),
            "cls_compliance_issues.issue_type CHECK omits `{code}`; that issue \
             would fail its insert and never reach the fleet report"
        );
    }
}
