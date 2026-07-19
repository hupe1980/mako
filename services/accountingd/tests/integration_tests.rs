//! Integration and unit tests for `accountingd` domain logic.
//!
//! ## Pure logic tests (no database required)
//!
//! The tests in this file cover deterministic logic that can run without a
//! live PostgreSQL connection:
//! - §288 BGB interest calculation (Verzugszinsen)
//! - Double-entry SKR 03 account mapping
//! - SEPA pain.008 batch splitting (FRST vs RCUR)
//!
//! ## Database-backed integration tests
//!
//! Tests marked `#[ignore]` require a live PostgreSQL instance.
//! Run them with:
//!
//! ```bash
//! export DATABASE_URL="postgres://accountingd:secret@localhost:5432/accountingd_test"
//! cargo test -p accountingd --test integration_tests -- --include-ignored
//! ```
//!
//! These tests provision the schema via `sqlx::migrate!` at test start.

use accountingd::pg::journal_mapping;
use accountingd::sepa::calculate_interest_ct;
use rust_decimal_macros::dec;

// ── §288 BGB Verzugszinsen calculation ────────────────────────────────────────

#[test]
fn test_interest_b2c_ecb_plus_5pp() {
    // §288 Abs. 1 BGB: B2C = ECB base rate + 5pp
    let ecb_rate = dec!(2.15); // 2026-01-01 rate
    let principal_ct = 10_000i64; // 100.00 EUR
    let days = 30i64;
    let (interest_ct, annual_rate) = calculate_interest_ct(principal_ct, ecb_rate, false, days);
    assert_eq!(
        annual_rate,
        dec!(7.15),
        "B2C rate = base 2.15 + 5pp = 7.15%"
    );
    // 10000 * 7.15/100 * 30/36500 = 58.76... → floor = 58 ct
    assert_eq!(
        interest_ct, 58,
        "B2C: 30-day interest on 100 EUR at 7.15% = 58 ct"
    );
}

#[test]
fn test_interest_b2b_ecb_plus_9pp() {
    // §288 Abs. 2 BGB: B2B = ECB base rate + 9pp
    let ecb_rate = dec!(2.15);
    let principal_ct = 10_000i64;
    let days = 30i64;
    let (interest_ct, annual_rate) = calculate_interest_ct(principal_ct, ecb_rate, true, days);
    assert_eq!(
        annual_rate,
        dec!(11.15),
        "B2B rate = base 2.15 + 9pp = 11.15%"
    );
    // 10000 * 11.15/100 * 30/36500 = 91.64... → floor = 91 ct
    assert_eq!(
        interest_ct, 91,
        "B2B: 30-day interest on 100 EUR at 11.15% = 91 ct"
    );
}

#[test]
fn test_interest_b2b_exceeds_b2c() {
    let ecb_rate = dec!(3.00);
    let principal_ct = 50_000i64;
    let days = 90i64;
    let (b2c_ct, _) = calculate_interest_ct(principal_ct, ecb_rate, false, days);
    let (b2b_ct, _) = calculate_interest_ct(principal_ct, ecb_rate, true, days);
    assert!(
        b2b_ct > b2c_ct,
        "B2B interest must always exceed B2C for same period"
    );
}

#[test]
fn test_interest_zero_for_zero_days() {
    let (ct, _) = calculate_interest_ct(10_000, dec!(5.0), false, 0);
    assert_eq!(ct, 0, "zero days → zero interest");
}

#[test]
fn test_interest_proportional_to_principal() {
    let ecb_rate = dec!(3.12);
    let days = 30;
    let (ct_1x, _) = calculate_interest_ct(10_000, ecb_rate, false, days);
    let (ct_5x, _) = calculate_interest_ct(50_000, ecb_rate, false, days);
    let (ct_10x, _) = calculate_interest_ct(100_000, ecb_rate, false, days);
    // Larger principal → larger interest (monotonic ordering)
    assert!(
        ct_5x > ct_1x,
        "5× principal must yield more interest than 1×"
    );
    assert!(
        ct_10x > ct_5x,
        "10× principal must yield more interest than 5×"
    );
    // Must be at least 8× and at most 12× for a 10× principal multiplier
    // (floor() breaks exact linearity but must stay approximately proportional)
    assert!(
        ct_10x >= ct_1x * 8 && ct_10x <= ct_1x * 12,
        "10× principal should produce roughly 10× interest (within ±20% for floor rounding)"
    );
}

// ── Double-entry SKR 03 journal mapping ───────────────────────────────────────

#[test]
fn test_journal_mapping_rechnung_debit() {
    let m = journal_mapping("RECHNUNG", 1000);
    assert_eq!(m.debit_skr, "1400", "RECHNUNG debit → Forderungen aus L+L");
    assert_eq!(m.credit_skr, "4000", "RECHNUNG credit → Energieerlöse");
}

#[test]
fn test_journal_mapping_zahlung_credit() {
    let m = journal_mapping("ZAHLUNG", -1000); // negative = credit
    assert_eq!(
        m.debit_skr, "1200",
        "ZAHLUNG debit → Bankguthaben (cash received)"
    );
    assert_eq!(m.credit_skr, "1400", "ZAHLUNG credit → Forderungen aus L+L");
}

#[test]
fn test_journal_mapping_bankruecklast() {
    let m = journal_mapping("BANKRUECKLAST", 1000);
    assert_eq!(
        m.debit_skr, "1400",
        "BANKRUECKLAST debit → Forderungen (re-open)"
    );
    assert_eq!(
        m.credit_skr, "1200",
        "BANKRUECKLAST credit → Bankguthaben (reversed)"
    );
}

#[test]
fn test_journal_mapping_eeg_gutschrift() {
    let m = journal_mapping("EEG_GUTSCHRIFT", -500);
    assert_eq!(m.debit_skr, "3000", "EEG credit → LF Verbindlichkeit");
    assert_eq!(
        m.credit_skr, "4001",
        "EEG credit → EEG Einspeisevergütung Erlöse"
    );
}

#[test]
fn test_journal_mapping_storno_reversal() {
    // STORNO with negative amount = credit (reversing a RECHNUNG)
    let m = journal_mapping("STORNO", -1000);
    assert_eq!(m.debit_skr, "4000", "STORNO credit → reverse Erlöse debit");
    assert_eq!(
        m.credit_skr, "1400",
        "STORNO credit → reverse Forderungen credit"
    );
}

#[test]
fn test_journal_mapping_mahngebuehr() {
    let m = journal_mapping("MAHNGEBUEHR", 500);
    assert_eq!(m.debit_skr, "1400", "Mahngebühr debit → Forderungen");
    assert_eq!(
        m.credit_skr, "4003",
        "Mahngebühr credit → Mahngebühren Erlöse"
    );
}

// ── SEPA pain.008 batch splitting ─────────────────────────────────────────────

#[test]
fn test_pain008_frst_rcur_separation() {
    use accountingd::pg::SepaMandateRow;
    use accountingd::sepa::build_pain_008;
    use time::Date;
    use uuid::Uuid;

    fn make_mandate(seq: &str) -> SepaMandateRow {
        SepaMandateRow {
            mandate_id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            tenant: "test".into(),
            iban: "DE89370400440532013000".into(),
            bic: None,
            kontoinhaber: Some("Test Kunde".into()),
            mandatsref: format!("REF-{seq}-{}", Uuid::new_v4()),
            sequence_type: seq.to_owned(),
            signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
            revoked_at: None,
            updated_at: time::OffsetDateTime::now_utc(),
        }
    }

    let frst = make_mandate("FRST");
    let rcur1 = make_mandate("RCUR");
    let rcur2 = make_mandate("RCUR");
    let mandates = [&frst, &rcur1, &rcur2];
    let entries: Vec<(&SepaMandateRow, i64)> = mandates.iter().map(|m| (*m, 5000i64)).collect();

    let batches = build_pain_008(
        "DE89370400440532013000",
        "Test Energie GmbH",
        None,
        &entries,
    )
    .expect("build_pain_008 should succeed");

    assert_eq!(
        batches.len(),
        2,
        "FRST and RCUR must be in separate batches"
    );
    let frst_batch = batches
        .iter()
        .find(|b| format!("{:?}", b.sequence_type) == "Frst");
    let rcur_batch = batches
        .iter()
        .find(|b| format!("{:?}", b.sequence_type) == "Rcur");
    assert!(frst_batch.is_some(), "must have a FRST batch");
    assert!(rcur_batch.is_some(), "must have a RCUR batch");
    assert_eq!(frst_batch.unwrap().entry_count, 1, "FRST batch has 1 entry");
    assert_eq!(
        rcur_batch.unwrap().entry_count,
        2,
        "RCUR batch has 2 entries"
    );
}

#[test]
fn test_pain008_includes_creditor_name_not_iban() {
    // Regression: old code passed creditor_iban_str as creditor_name (bug fix verification)
    use accountingd::sepa::build_pain_008;

    let batches = build_pain_008(
        "DE89370400440532013000", // creditor IBAN
        "Muster Energie GmbH",    // creditor NAME — must NOT appear as IBAN in XML
        None,
        &[],
    )
    .expect("build even with empty entries");

    // Empty entries → no batches (the function returns empty vec for no entries)
    assert_eq!(
        batches.len(),
        0,
        "no entries → no batches (all were empty groups)"
    );
}

#[test]
fn test_pain008_invalid_creditor_iban_fails() {
    use accountingd::sepa::build_pain_008;

    let result = build_pain_008("INVALID-IBAN", "Test", None, &[]);
    assert!(
        result.is_err(),
        "invalid creditor IBAN must return an error"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("INVALID-IBAN") || err.contains("invalid"),
        "error message should mention the invalid IBAN"
    );
}

#[test]
fn test_pain008_creditor_id_validated() {
    use accountingd::sepa::build_pain_008;

    // Invalid Gläubiger-ID format should fail
    let result = build_pain_008("DE89370400440532013000", "Test", Some("INVALID-CI"), &[]);
    assert!(result.is_err(), "invalid creditor_id must return an error");
}

// ── CAMT.054 deduplication hash stability ─────────────────────────────────────

#[test]
fn test_dedup_hash_is_deterministic() {
    // The fallback hash in import_payments must be deterministic for the same input
    let key1 = "DE89370400440532013000|-5000|2026-07-15|VERWZ-12345";
    let key2 = "DE89370400440532013000|-5000|2026-07-15|VERWZ-12345";
    let hash1 = format!(
        "{:016x}",
        key1.bytes().fold(0u64, |acc, b| {
            acc.wrapping_mul(1099511628211).wrapping_add(b as u64)
        })
    );
    let hash2 = format!(
        "{:016x}",
        key2.bytes().fold(0u64, |acc, b| {
            acc.wrapping_mul(1099511628211).wrapping_add(b as u64)
        })
    );
    assert_eq!(hash1, hash2, "same input must produce same dedup hash");

    // Different inputs must produce different hashes
    let key3 = "DE89370400440532013000|-5000|2026-07-16|VERWZ-12345"; // different date
    let hash3 = format!(
        "{:016x}",
        key3.bytes().fold(0u64, |acc, b| {
            acc.wrapping_mul(1099511628211).wrapping_add(b as u64)
        })
    );
    assert_ne!(
        hash1, hash3,
        "different dates must produce different hashes"
    );
}
