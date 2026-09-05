//! The `eeg_verguetungssaetze` seed is generated from the statute, not typed.
//!
//! `migrations/0001_schema.sql` seeds the statutory Einspeisevergütung series.
//! Its single source is [`eeg_billing::seed::verguetungssatz_rows`], which reads
//! the §§ 40–49 Startwerte and applies the statutory Absenkungen and the § 53
//! Abs. 1 deduction. A hand-edited seed row is a rate no statute contains, so
//! this test pins the two together — it parses the migration and compares it,
//! row for row, against the crate.
//!
//! It needs no database: the migration is read as text.

use rust_decimal::Decimal;
use std::str::FromStr as _;
use time::Date;
use time::format_description::well_known::Iso8601;

/// One seeded row, as the migration states it.
#[derive(Debug, PartialEq, Eq)]
struct SeedRow {
    erzeugungsart: String,
    leistung_min_kwp: Decimal,
    leistung_max_kwp: Option<Decimal>,
    verguetungsform: String,
    verguetungssatz_ct: Decimal,
    billing_start: Date,
    billing_end: Option<Date>,
    eeg_gesetz: i16,
    notes: String,
}

/// Split one `VALUES (...)` tuple into its nine fields, honouring the quoted
/// strings (the `notes` column carries commas).
fn split_fields(body: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in body.chars() {
        match c {
            '\'' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.trim().to_owned());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    fields.push(current.trim().to_owned());
    fields
}

fn parse_seed() -> Vec<SeedRow> {
    let sql = include_str!("../migrations/0001_schema.sql");
    let start = sql
        .find("INSERT INTO eeg_verguetungssaetze")
        .expect("the seed INSERT is in the migration");
    let tail = &sql[start..];
    let end = tail.find(";\n").expect("the seed INSERT is terminated");
    let values = &tail[..end];

    let mut rows = Vec::new();
    for line in values.lines() {
        let line = line.trim().trim_end_matches(',');
        let Some(body) = line.strip_prefix('(').and_then(|l| l.strip_suffix(')')) else {
            continue;
        };
        let f = split_fields(body);
        assert_eq!(f.len(), 9, "seed row has nine columns: {line}");
        let dec = |s: &str| Decimal::from_str(s).unwrap_or_else(|_| panic!("decimal {s:?}"));
        let date =
            |s: &str| Date::parse(s, &Iso8601::DATE).unwrap_or_else(|_| panic!("date {s:?}"));
        rows.push(SeedRow {
            erzeugungsart: f[0].clone(),
            leistung_min_kwp: dec(&f[1]),
            leistung_max_kwp: (f[2] != "NULL").then(|| dec(&f[2])),
            verguetungsform: f[3].clone(),
            verguetungssatz_ct: dec(&f[4]),
            billing_start: date(&f[5]),
            billing_end: (f[6] != "NULL").then(|| date(&f[6])),
            eeg_gesetz: f[7].parse().expect("eeg_gesetz"),
            notes: f[8].clone(),
        });
    }
    rows
}

fn from_crate() -> Vec<SeedRow> {
    eeg_billing::seed::verguetungssatz_rows()
        .into_iter()
        .map(|r| SeedRow {
            erzeugungsart: r.erzeugungsart.to_owned(),
            leistung_min_kwp: r.leistung_min_kwp,
            leistung_max_kwp: r.leistung_max_kwp,
            verguetungsform: r.verguetungsform.to_owned(),
            verguetungssatz_ct: r.verguetungssatz_ct,
            billing_start: r.billing_start,
            billing_end: Some(r.billing_end),
            eeg_gesetz: r.eeg_gesetz,
            notes: r.notes,
        })
        .collect()
}

/// **The seeded rates are the statutory ones.**
///
/// Every row in the migration is a row `eeg_billing::seed` produces, and every
/// row it produces is in the migration — same order, same values.
#[test]
fn the_seed_is_the_crates_statutory_series() {
    let seeded = parse_seed();
    let expected = from_crate();
    assert!(!expected.is_empty(), "the crate produces rows");
    for (i, (a, b)) in seeded.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            a, b,
            "seed row {i} differs from eeg_billing::seed — regenerate the migration"
        );
    }
    assert_eq!(
        seeded.len(),
        expected.len(),
        "the seed has {} rows, the crate produces {}",
        seeded.len(),
        expected.len()
    );
}

/// **A commissioning date resolves to exactly one row per band.**
///
/// The lookup takes the newest window that still covers the date, so an
/// unbounded `billing_end` on an older window would shadow every later one and
/// pay a 2026 plant a 2024 rate. Every seeded window is closed.
#[test]
fn every_seeded_window_is_closed() {
    for row in parse_seed() {
        assert!(
            row.billing_end.is_some(),
            "{} {} band from {} kW opening {} has no end — later windows are unreachable",
            row.erzeugungsart,
            row.verguetungsform,
            row.leistung_min_kwp,
            row.billing_start
        );
    }
}

/// **No seeded rate is a gross anzulegender Wert.**
///
/// § 53 Abs. 1 deducts 0,4 ct from solar and 0,2 ct from the rest, and the
/// column holds the Einspeisevergütung. The Startwerte themselves therefore
/// never appear: paying one would overpay by the deduction.
#[test]
fn no_row_carries_the_gross_startwert() {
    let gross: &[(&str, Decimal)] = &[
        ("SOLAR_AUFDACH", rust_decimal::dec!(8.60)),
        ("SOLAR_AUFDACH", rust_decimal::dec!(7.50)),
        ("SOLAR_AUFDACH", rust_decimal::dec!(6.20)),
        ("WASSERKRAFT", rust_decimal::dec!(12.03)),
        ("BIOMASSE", rust_decimal::dec!(12.67)),
        ("GEOTHERMIE", rust_decimal::dec!(25.20)),
        ("KLAERGAS", rust_decimal::dec!(5.93)),
    ];
    for row in parse_seed() {
        for (art, wert) in gross {
            assert!(
                !(row.erzeugungsart == *art && row.verguetungssatz_ct == *wert),
                "{art} carries the gross Startwert {wert} — § 53 Abs. 1 has not been applied"
            );
        }
    }
}
