//! Text-level guards tying billingd's SQL to its schema.
//!
//! `insert_billing_record` once shipped with two independent faults — a missing
//! NOT-NULL `tenant` and an `ON CONFLICT` that could not match the partial
//! unique index — so it failed on every call, and nothing noticed because
//! nothing tested `pg.rs`. These run on every `cargo test`, no database needed;
//! `records_integration.rs` proves the same rules against real PostgreSQL.

const PG: &str = include_str!("../src/pg.rs");
const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");
/// Every place a document number could be invented.
const NUMBER_SITES: [(&str, &str); 6] = [
    ("calculate", include_str!("../src/handlers/calculate.rs")),
    ("correction", include_str!("../src/handlers/correction.rs")),
    ("ggv", include_str!("../src/handlers/ggv.rs")),
    (
        "sammelrechnung",
        include_str!("../src/handlers/sammelrechnung.rs"),
    ),
    ("vpp", include_str!("../src/handlers/vpp.rs")),
    ("billing_runs", include_str!("../src/billing_runs.rs")),
];

/// Strip `--` line comments so a rule cannot be satisfied by prose, and
/// collapse runs of whitespace so a reformatted statement still matches.
fn code_only(src: &str) -> String {
    let stripped: String = src
        .lines()
        .map(|l| match l.find("--") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The four predicate clauses of `br_unique_original`. The upsert must repeat
/// every one of them and the index must state every one of them; PostgreSQL
/// cannot infer a partial index from a column list, and a predicate that has
/// drifted on one side silently stops matching.
const INDEX_PREDICATE: [&str; 4] = [
    "is_correction = false",
    "sammelrechnung_id IS NULL",
    "outcome <> 'cancelled'",
    "category <> 'VPP'",
];

const CONFLICT_TARGET: &str =
    "ON CONFLICT (malo_id, lf_mp_id, period_from, period_to, product_code, tenant)";

#[test]
fn the_record_upsert_matches_its_partial_index() {
    let code = code_only(PG);
    let i = code
        .find(CONFLICT_TARGET)
        .expect("upsert names all six index columns, tenant included");
    let window = &code[i..(i + 700).min(code.len())];
    for clause in INDEX_PREDICATE {
        assert!(
            window.contains(clause),
            "the upsert must repeat br_unique_original's `{clause}`:\n{window}"
        );
    }
    assert!(
        window.contains("billing_records.outcome = 'generated'"),
        "a dispatched record must never be overwritten:\n{window}"
    );
}

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
    for clause in INDEX_PREDICATE {
        assert!(
            stmt.contains(clause),
            "the index must state `{clause}`: {stmt}"
        );
    }
}

/// § 14 Abs. 4 Nr. 4 UStG is a database constraint, not a convention: the
/// number lived in JSONB where nothing could enforce it, and a collision would
/// have surfaced years later in an audit rather than at write time.
#[test]
fn the_rechnungsnummer_is_unique_per_tenant() {
    let schema = code_only(SCHEMA);
    assert!(
        schema.contains(
            "CREATE UNIQUE INDEX br_unique_rechnungsnummer ON billing_records (tenant, rechnungsnummer)"
        ),
        "§14 Abs. 4 Nr. 4 UStG needs a unique index over (tenant, rechnungsnummer)"
    );
    assert!(
        schema.contains("rechnungsnummer TEXT NOT NULL"),
        "every stored document must carry its number"
    );
}

/// Every INSERT into billing_records must supply `tenant` and `rechnungsnummer`
/// — both NOT NULL with no default, so omitting either fails at runtime, not at
/// compile time.
#[test]
fn every_record_insert_supplies_tenant_and_rechnungsnummer() {
    let code = code_only(PG);
    let mut rest = code.as_str();
    let mut checked = 0;
    while let Some(i) = rest.find("INSERT INTO billing_records") {
        let window = &rest[i..(i + 260).min(rest.len())];
        assert!(
            window.contains("tenant"),
            "an INSERT omits the NOT NULL tenant column:\n{window}"
        );
        assert!(
            window.contains("rechnungsnummer"),
            "an INSERT omits the NOT NULL rechnungsnummer column:\n{window}"
        );
        checked += 1;
        rest = &rest[i + 1..];
    }
    assert!(
        checked >= 2,
        "both insert paths (original upsert, correction) are checked, found {checked}"
    );
}

/// The outbox is the only record of a dispatched event. A `ce_id` column
/// duplicated that fact, was never written outside tests, and left an index
/// predicate keyed on a value that was always NULL.
#[test]
fn no_column_shadows_the_outbox() {
    let schema = code_only(SCHEMA);
    assert!(
        !schema.contains("ce_id"),
        "dispatch state lives in the outbox and `outcome`, not in a shadow column"
    );
}

/// A Storno releases its period. Every read that answers "is this period
/// covered" must therefore exclude cancelled rows, or a reversed invoice keeps
/// the customer's window looking billed while no live document exists.
#[test]
fn coverage_reads_ignore_cancelled_periods() {
    let code = code_only(PG);
    let i = code
        .find("pub async fn billing_record_exists_for_period")
        .expect("the sweep's idempotency probe exists");
    let window = &code[i..(i + 700).min(code.len())];
    assert!(
        window.contains("outcome <> 'cancelled'"),
        "a cancelled original must not count as coverage:\n{window}"
    );
}

/// Rust source with its comment lines removed, so a rule is proven against the
/// code and not against prose that merely mentions the pattern.
fn rust_code_only(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// § 14 Abs. 4 Nr. 4 UStG wants a **fortlaufende** number, and the store wants a
/// re-issuable one.
///
/// A number derived from the billed facts — `BILL-{malo}-{product}-{from}`,
/// `KORR-{original}`, `GGV-{ggv}-{malo}-{from}`, `VPP-{vpp}-{date}-{tx}` — is not
/// sequential, and worse: re-billing a period after a Storno reproduces the
/// *cancelled original's own string*, which `br_unique_rechnungsnummer`
/// refuses. That makes Storno-und-Neuberechnung impossible while the correction
/// endpoint recommends it, and a test can miss it entirely simply because
/// it hand-picked a different number.
#[test]
fn the_number_series_is_a_counter_and_not_a_derived_string() {
    let schema = code_only(SCHEMA);
    assert!(
        schema.contains("CREATE TABLE invoice_number_series"),
        "the fortlaufende series needs a counter table"
    );
    assert!(
        code_only(PG).contains("pub async fn allocate_rechnungsnummer"),
        "the counter needs an allocation function"
    );
    for (name, src) in NUMBER_SITES {
        let src = &rust_code_only(src);
        for derived in ["\"BILL-{", "\"KORR-{", "\"GGV-{", "\"VPP-{", "\"SAMMEL-{"] {
            assert!(
                !src.contains(derived),
                "{name} derives a Rechnungsnummer from billed facts ({derived}…): \
                 the period cannot then be re-billed after a Storno — allocate from \
                 `invoice_number_series` instead"
            );
        }
    }
}

/// Issuing a document must not depend on whether an ERP happens to be wired up.
///
/// Calling `mark_dispatched_tx` only inside `if erp_webhook_url.is_some()` would
/// leave every invoice of an operator without an ERP in `generated` forever:
/// rewritable by the next run, never template-pinned, and outside the § 147 AO
/// reproducibility guarantee the rest of the design assumes.
#[test]
fn issuance_does_not_depend_on_the_erp_webhook() {
    for (name, src) in NUMBER_SITES {
        let code = code_only(&rust_code_only(src));
        for (i, _) in code.match_indices("mark_dispatched_tx") {
            let before = &code[i.saturating_sub(200)..i];
            assert!(
                !before.contains("erp_webhook_url.is_some()"),
                "{name} stamps a record issued only when an ERP webhook is configured"
            );
        }
    }
}

/// A statistical baseline and the risk gate must look at the same invoices.
///
/// Counting the per-MaLo children of a Sammelrechnung alongside the bundle that
/// already contains them, or keeping reversed invoices in the average, shows an
/// analyst a "rolling average" that is not the one the invoice was scored
/// against.
#[test]
fn every_baseline_reads_the_same_population() {
    let code = code_only(PG);
    for f in [
        "pub async fn check_billing_anomaly",
        "pub async fn risk_context",
        "pub async fn billing_summary",
    ] {
        let i = code.find(f).unwrap_or_else(|| panic!("{f} exists"));
        let window = &code[i..(i + 1400).min(code.len())];
        assert!(
            window.contains("sammelrechnung_id IS NULL"),
            "{f} counts bundle children alongside their bundle"
        );
        assert!(
            window.contains("outcome <> 'cancelled'"),
            "{f} keeps reversed invoices in the baseline"
        );
    }
}
