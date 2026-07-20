//! Text-level guards tying billingd's SQL to its schema.
//!
//! `insert_billing_record` shipped with two independent faults — a missing
//! NOT-NULL `tenant` and an `ON CONFLICT` that could not match the partial
//! unique index — so it failed on every call, and nothing noticed because
//! nothing tested `pg.rs`. These run on every `cargo test`, no database needed;
//! `records_integration.rs` proves the same rules against real PostgreSQL.

const PG: &str = include_str!("../src/pg.rs");
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

/// The record upsert must name every column of the partial index — including
/// `tenant` — and repeat the index predicate; PostgreSQL cannot infer a partial
/// index from a column list.
#[test]
fn the_record_upsert_matches_its_partial_index() {
    let code = code_only(PG);
    let i = code
        .find("ON CONFLICT (malo_id, lf_mp_id, period_from, period_to, product_code, tenant)")
        .expect("upsert names all six index columns, tenant included");
    let window = &code[i..(i + 700).min(code.len())];
    assert!(
        window.contains("is_correction = false") && window.contains("sammelrechnung_id IS NULL"),
        "the upsert must repeat br_unique_original's predicate:\n{window}"
    );
    assert!(
        window.contains("outcome = 'generated'"),
        "a dispatched record must never be overwritten:\n{window}"
    );
}

/// The schema's unique index is partial over exactly those six columns.
#[test]
fn the_unique_index_is_what_the_upsert_assumes() {
    let schema = code_only(SCHEMA);
    let i = schema
        .find("CREATE UNIQUE INDEX br_unique_original")
        .expect("br_unique_original exists");
    let stmt = &schema[i..schema[i..].find(';').map_or(schema.len(), |e| i + e)];
    assert!(
        stmt.contains("tenant"),
        "tenant is part of the identity: {stmt}"
    );
    assert!(
        stmt.contains("WHERE is_correction = false AND sammelrechnung_id IS NULL"),
        "the index must stay partial: {stmt}"
    );
}

/// Every INSERT into billing_records must supply `tenant` — it is NOT NULL with
/// no default, so omitting it fails at runtime, not at compile time.
#[test]
fn every_record_insert_supplies_the_tenant() {
    let code = code_only(PG);
    let mut rest = code.as_str();
    let mut checked = 0;
    while let Some(i) = rest.find("INSERT INTO billing_records") {
        let window = &rest[i..(i + 200).min(rest.len())];
        assert!(
            window.contains("tenant"),
            "an INSERT omits the NOT NULL tenant column:\n{window}"
        );
        checked += 1;
        rest = &rest[i + 1..];
    }
    assert!(
        checked >= 3,
        "all three insert paths are checked, found {checked}"
    );
}
