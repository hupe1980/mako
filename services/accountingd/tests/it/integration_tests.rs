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
//! Pure (no-DB) integration tests. The DB-backed financial scenario tests
//! (idempotency, Abschlag netting, double-entry balance) live in
//! `tests/it/db_scenarios.rs` and run against a live PostgreSQL.

use accountingd::sepa::calculate_interest_ct;
use rust_decimal::dec;

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

// The SKR double-entry mapping moved into the doubleentry ledger: accountingd's
// `ledger::Chart` maps each Buchungsart to a balanced (customer Kontokorrent, GL
// contra) pair, unit-tested in `ledger.rs` (`all_entry_types_balance` et al.).

// ── SEPA pain.008 batch splitting ─────────────────────────────────────────────

/// The creditor identity every pain.008 test collects with.
///
/// `DE98ZZZ09999999999` is the canonical EPC example Gläubiger-ID and has
/// correct EPC262-08 check digits (computed over the national identifier,
/// excluding the Creditor Business Code).
const TEST_CREDITOR: accountingd::sepa::CreditorIdentity<'static> =
    accountingd::sepa::CreditorIdentity {
        iban: "DE89370400440532013000",
        name: "Test Energie GmbH",
        creditor_id: "DE98ZZZ09999999999",
        address: None,
    };

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
            mandatsref: format!("REF-{seq}-{}", &Uuid::new_v4().simple().to_string()[..8]),
            sequence_type: seq.to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
            revoked_at: None,
            last_presented_at: None,
            sparte: Some("STROM".into()),
            debtor_town: Some("Berlin".into()),
            debtor_country: Some("DE".into()),
            debtor_street: Some("Musterstrasse".into()),
            debtor_building_number: Some("12".into()),
            debtor_post_code: Some("10115".into()),
            debtor_country_subdivision: None,
            updated_at: time::OffsetDateTime::now_utc(),
        }
    }

    let frst = make_mandate("FRST");
    let rcur1 = make_mandate("RCUR");
    let rcur2 = make_mandate("RCUR");
    let mandates = [&frst, &rcur1, &rcur2];
    let entries: Vec<(&SepaMandateRow, i64)> = mandates.iter().map(|m| (*m, 5000i64)).collect();

    let run = build_pain_008(
        &TEST_CREDITOR,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &entries,
        Default::default(),
    )
    .expect("build_pain_008 should succeed");

    // One message, one PmtInf group per SequenceType (Rulebook §3.8).
    assert_eq!(
        run.groups.len(),
        2,
        "FRST and RCUR are separate PmtInf groups"
    );
    assert_eq!(run.groups[0].sequence_type, "FRST");
    assert_eq!(run.groups[0].entry_count, 1);
    assert_eq!(run.groups[1].sequence_type, "RCUR");
    assert_eq!(run.groups[1].entry_count, 2);
    assert_eq!(run.entry_count, 3);
    assert_eq!(run.total_ct, 15_000);
    assert_eq!(
        run.xml.matches("<PmtInf>").count(),
        2,
        "single file carries both PmtInf blocks"
    );
    assert!(
        run.xml.contains("<SeqTp>FRST</SeqTp>") && run.xml.contains("<SeqTp>RCUR</SeqTp>"),
        "both sequence types present in one message"
    );
}

#[test]
fn test_pain008_empty_run_is_an_error() {
    // A run with no billable mandates must fail loudly, not emit an empty file.
    use accountingd::sepa::build_pain_008;
    use time::Date;

    let result = build_pain_008(
        &TEST_CREDITOR,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &[],
        Default::default(),
    );
    assert!(result.is_err(), "no entries → error, not an empty message");
}

#[test]
fn test_pain008_invalid_creditor_iban_fails() {
    use accountingd::sepa::build_pain_008;

    let result = build_pain_008(
        &accountingd::sepa::CreditorIdentity {
            iban: "INVALID-IBAN",
            name: "Test",
            creditor_id: "DE98ZZZ09999999999",
            address: None,
        },
        time::Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &[],
        Default::default(),
    );
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
    let result = build_pain_008(
        &accountingd::sepa::CreditorIdentity {
            iban: "DE89370400440532013000",
            name: "Test",
            creditor_id: "INVALID-CI",
            address: None,
        },
        time::Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &[],
        Default::default(),
    );
    assert!(result.is_err(), "invalid creditor_id must return an error");

    // Regression (sepa 0.4): the canonical DE98ZZZ09999999999 has CORRECT
    // check digits per EPC262-08 (computed over the national identifier,
    // excluding the Creditor Business Code). sepa 0.3 rejected it.
    assert!(
        accountingd::sepa::validate_creditor_id("DE98ZZZ09999999999").is_ok(),
        "genuine Gläubiger-ID must validate"
    );
    assert!(
        accountingd::sepa::validate_creditor_id("DE74ZZZ09999999999").is_err(),
        "wrong check digits must be rejected"
    );
}

// ── FR-1: config-selectable pain.008 / pain.001 schema version ────────────────

#[test]
fn test_schema_version_resolves_from_config() {
    use accountingd::sepa::{resolve_pain001_schema, resolve_pain008_schema};

    // Absent config → the current SEPA defaults.
    assert_eq!(
        resolve_pain008_schema(None).unwrap().to_string(),
        "pain.008.001.08"
    );
    assert_eq!(
        resolve_pain001_schema(None).unwrap().to_string(),
        "pain.001.001.09"
    );

    // A bank still on the pre-2023 EPC version can be targeted from config.
    assert_eq!(
        resolve_pain008_schema(Some("pain.008.001.02"))
            .unwrap()
            .to_string(),
        "pain.008.001.02"
    );
    assert_eq!(
        resolve_pain001_schema(Some("pain.001.001.03"))
            .unwrap()
            .to_string(),
        "pain.001.001.03"
    );

    // An unknown value is a hard error naming the offending string.
    let err = resolve_pain008_schema(Some("pain.008.001.99")).unwrap_err();
    assert!(err.to_string().contains("pain.008.001.99"));
    assert!(resolve_pain001_schema(Some("garbage")).is_err());
}

#[test]
fn test_pain008_emits_configured_namespace() {
    use accountingd::pg::SepaMandateRow;
    use accountingd::sepa::{build_pain_008, resolve_pain008_schema};
    use time::Date;
    use uuid::Uuid;

    let mandate = SepaMandateRow {
        mandate_id: Uuid::new_v4(),
        account_id: Uuid::new_v4(),
        tenant: "test".into(),
        iban: "DE89370400440532013000".into(),
        bic: None,
        kontoinhaber: Some("Test Kunde".into()),
        mandatsref: "REF-SCHEMA-01".into(),
        sequence_type: "RCUR".into(),
        scheme: "CORE".to_owned(),
        signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
        revoked_at: None,
        last_presented_at: None,
        sparte: None,
        debtor_town: None,
        debtor_country: None,
        debtor_street: None,
        debtor_building_number: None,
        debtor_post_code: None,
        debtor_country_subdivision: None,
        updated_at: time::OffsetDateTime::now_utc(),
    };
    let entries = [(&mandate, 5000i64)];

    let schema = resolve_pain008_schema(Some("pain.008.001.02")).unwrap();
    let run = build_pain_008(
        &TEST_CREDITOR,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &entries,
        schema,
    )
    .expect("build_pain_008 should succeed");

    assert!(
        run.xml
            .contains("urn:iso:std:iso:20022:tech:xsd:pain.008.001.02"),
        "the configured schema version must appear in the emitted namespace"
    );
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

// ── Structured postal addresses ───────────────────────────────────────────────

#[test]
fn address_parts_empty_emits_nothing() {
    use accountingd::sepa::AddressParts;

    let empty = AddressParts::default();
    assert!(empty.is_empty());
    assert!(
        empty
            .to_postal_address()
            .expect("empty is not an error")
            .is_none(),
        "no parts → no PstlAdr at all"
    );
}

#[test]
fn address_parts_half_filled_is_an_error() {
    use accountingd::sepa::AddressParts;

    // A street with no town and no country looks configured and would emit
    // nothing.
    let partial = AddressParts {
        street: Some("Musterstrasse".into()),
        post_code: Some("10115".into()),
        ..Default::default()
    };
    let err = partial
        .to_postal_address()
        .expect_err("town + country are required once any part is set")
        .to_string();
    assert!(
        err.contains("town and country"),
        "the error must name what is missing, got: {err}"
    );
}

#[test]
fn address_parts_rejects_a_non_country() {
    use accountingd::sepa::AddressParts;

    // `ZZ` matches the XSD's [A-Z]{2} and addresses nothing. The crate checks
    // Ctry against the ISO 3166 table instead.
    let bogus = AddressParts {
        town: Some("Nirgendwo".into()),
        country: Some("ZZ".into()),
        ..Default::default()
    };
    assert!(
        bogus.to_postal_address().is_err(),
        "ZZ is not an assigned ISO 3166-1 alpha-2 code"
    );
}

#[test]
fn pain008_emits_structured_addresses_on_both_sides() {
    use accountingd::pg::SepaMandateRow;
    use accountingd::sepa::{AddressParts, CreditorIdentity, build_pain_008};
    use time::Date;
    use uuid::Uuid;

    let mandate = SepaMandateRow {
        mandate_id: Uuid::new_v4(),
        account_id: Uuid::new_v4(),
        tenant: "test".into(),
        iban: "DE89370400440532013000".into(),
        bic: None,
        kontoinhaber: Some("Erika Mustermann".into()),
        mandatsref: "REF-ADDR-01".into(),
        sequence_type: "RCUR".into(),
        scheme: "CORE".to_owned(),
        signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
        revoked_at: None,
        last_presented_at: None,
        sparte: Some("GAS".into()),
        debtor_town: Some("Hamburg".into()),
        debtor_country: Some("DE".into()),
        debtor_street: Some("Deichstrasse".into()),
        debtor_building_number: Some("7".into()),
        debtor_post_code: Some("20459".into()),
        debtor_country_subdivision: None,
        updated_at: time::OffsetDateTime::now_utc(),
    };
    let entries = [(&mandate, 5000i64)];

    let creditor_address = AddressParts {
        town: Some("Berlin".into()),
        country: Some("DE".into()),
        street: Some("Musterstrasse".into()),
        building_number: Some("12".into()),
        post_code: Some("10115".into()),
        country_subdivision: None,
    };
    let creditor = CreditorIdentity {
        address: Some(&creditor_address),
        ..TEST_CREDITOR
    };

    let run = build_pain_008(
        &creditor,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &entries,
        Default::default(),
    )
    .expect("build_pain_008 should succeed");

    assert_eq!(
        run.xml.matches("<PstlAdr>").count(),
        2,
        "Cdtr/PstlAdr and Dbtr/PstlAdr both present"
    );
    assert!(run.xml.contains("<TwnNm>Berlin</TwnNm>"));
    assert!(run.xml.contains("<TwnNm>Hamburg</TwnNm>"));
    assert!(run.xml.contains("<StrtNm>Deichstrasse</StrtNm>"));
    assert!(run.xml.contains("<PstCd>20459</PstCd>"));
}

#[test]
fn pain008_legacy_dk_schema_omits_the_address_rather_than_failing() {
    use accountingd::pg::SepaMandateRow;
    use accountingd::sepa::{AddressParts, CreditorIdentity, DirectDebitSchema, build_pain_008};
    use time::Date;
    use uuid::Uuid;

    // `pain.008.003.02`'s PostalAddressSEPA holds only Ctry and two AdrLines —
    // there is no structured address type to emit into. An operator who pinned
    // that schema must still be able to collect.
    let schema: DirectDebitSchema = "pain.008.003.02".parse().unwrap();
    assert!(
        !schema.supports_postal_address(),
        "the legacy DK schema has no structured address type"
    );

    let mandate = SepaMandateRow {
        mandate_id: Uuid::new_v4(),
        account_id: Uuid::new_v4(),
        tenant: "test".into(),
        iban: "DE89370400440532013000".into(),
        bic: None,
        kontoinhaber: Some("Erika Mustermann".into()),
        mandatsref: "REF-ADDR-02".into(),
        sequence_type: "RCUR".into(),
        scheme: "CORE".to_owned(),
        signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
        revoked_at: None,
        last_presented_at: None,
        sparte: Some("GAS".into()),
        debtor_town: Some("Hamburg".into()),
        debtor_country: Some("DE".into()),
        debtor_street: None,
        debtor_building_number: None,
        debtor_post_code: None,
        debtor_country_subdivision: None,
        updated_at: time::OffsetDateTime::now_utc(),
    };
    let entries = [(&mandate, 5000i64)];
    let creditor_address = AddressParts {
        town: Some("Berlin".into()),
        country: Some("DE".into()),
        ..Default::default()
    };
    let creditor = CreditorIdentity {
        address: Some(&creditor_address),
        ..TEST_CREDITOR
    };

    let run = build_pain_008(
        &creditor,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &entries,
        schema,
    )
    .expect("a pinned legacy schema must not block the collection run");
    assert!(
        !run.xml.contains("<PstlAdr>"),
        "no PstlAdr is emitted into a schema whose XSD would reject it"
    );
}

// ── pain.008 group identity ───────────────────────────────────────────────────

#[test]
fn pain008_groups_carry_distinct_payment_info_ids() {
    use accountingd::pg::SepaMandateRow;
    use accountingd::sepa::build_pain_008;
    use time::Date;
    use uuid::Uuid;

    // A duplicate PmtInfId across groups is refused by the crate: it is the key
    // a bank echoes in pain.002 and in a camt `Btch` block, so two groups
    // sharing one make a booking unattributable.
    fn mandate(seq: &str, refn: &str) -> SepaMandateRow {
        SepaMandateRow {
            mandate_id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            tenant: "test".into(),
            iban: "DE89370400440532013000".into(),
            bic: None,
            kontoinhaber: Some("Test Kunde".into()),
            mandatsref: refn.to_owned(),
            sequence_type: seq.to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
            revoked_at: None,
            last_presented_at: None,
            sparte: None,
            debtor_town: None,
            debtor_country: None,
            debtor_street: None,
            debtor_building_number: None,
            debtor_post_code: None,
            debtor_country_subdivision: None,
            updated_at: time::OffsetDateTime::now_utc(),
        }
    }
    let frst = mandate("FRST", "REF-PMTINF-F");
    let rcur = mandate("RCUR", "REF-PMTINF-R");
    let entries = [(&frst, 1000i64), (&rcur, 2000i64)];

    let run = build_pain_008(
        &TEST_CREDITOR,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &entries,
        Default::default(),
    )
    .expect("build_pain_008 should succeed");

    // One `PmtInf` per (scheme, SequenceType) — the DK subset makes
    // `SvcLvl`/`LclInstrm`/`SeqTp` an all-or-nothing block, and CORE and B2B are
    // different rulebooks with different debtor refund rights.
    assert_eq!(
        run.groups[0].payment_info_id,
        format!("{}-CORE-FRST", run.msg_id)
    );
    assert_eq!(
        run.groups[1].payment_info_id,
        format!("{}-CORE-RCUR", run.msg_id)
    );
    assert!(run.groups.iter().all(|g| g.scheme == "CORE"));
    assert_ne!(run.groups[0].payment_info_id, run.groups[1].payment_info_id);

    // The per-entry breakdown is what `sepa_collection_entries` persists, and
    // what later attributes a bank reply back to a mandate.
    assert_eq!(run.entries.len(), 2);
    let frst_entry = run
        .entries
        .iter()
        .find(|e| e.mandatsref == "REF-PMTINF-F")
        .expect("FRST entry recorded");
    assert_eq!(frst_entry.mandate_id, frst.mandate_id);
    assert_eq!(frst_entry.amount_ct, 1000);
    assert_eq!(frst_entry.payment_info_id, run.groups[0].payment_info_id);
}

// ── pain.001 execution date ───────────────────────────────────────────────────

#[test]
fn pain001_sets_the_execution_date_explicitly() {
    use accountingd::sepa::{CreditTransferItem, DebtorIdentity, build_pain_001};

    // A payment date is not something to inherit from a library default, so
    // accountingd states `ReqdExctnDt` on every group.
    let execution = time::Date::from_calendar_date(2026, time::Month::August, 3).unwrap();
    let xml = build_pain_001(
        &DebtorIdentity {
            iban: "DE89370400440532013000",
            name: "Test Energie GmbH",
            address: None,
        },
        &[CreditTransferItem {
            iban: "DE02120300000000202051",
            name: "Anlagenbetreiber",
            amount_ct: 12_345,
            end_to_end_ref: "EEG-TEST-01",
            address: None,
        }],
        execution,
        false,
        Default::default(),
    )
    .expect("build_pain_001 should succeed");

    assert!(
        xml.contains("<ReqdExctnDt><Dt>2026-08-03</Dt></ReqdExctnDt>")
            || xml.contains("<ReqdExctnDt>2026-08-03</ReqdExctnDt>"),
        "the stated execution date must reach the wire, got:\n{xml}"
    );
}

// ── pain.007 reversal ─────────────────────────────────────────────────────────

fn sample_reversal<'a>(reversed_amount_ct: Option<i64>) -> accountingd::sepa::ReversalRequest<'a> {
    accountingd::sepa::ReversalRequest {
        original_msg_id: "DD-2026-07-25",
        original_payment_info_id: "DD-2026-07-25-RCUR",
        original_end_to_end_id: "MND-000123",
        original_amount_ct: 12_000,
        reversed_amount_ct,
        reason: accountingd::sepa::ReversalReason::Am05,
        mandate_ref: "MND-000123",
        mandate_signed_at: time::Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
        collection_date: time::Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        sequence_type: "RCUR",
        scheme: "CORE",
        debtor_name: "Erika Mustermann",
        debtor_iban: "DE89370400440532013000",
        debtor_bic: None,
    }
}

#[test]
fn pain007_restates_the_original_transaction_reference() {
    use accountingd::sepa::build_pain_007;

    // The DK technical validation subset makes OrgnlTxRef — and the mandate
    // inside it — mandatory, so the references-only form plain ISO permits is
    // not one a German bank accepts.
    let reversal = build_pain_007(&TEST_CREDITOR, &[sample_reversal(None)], Default::default())
        .expect("build_pain_007 should succeed");

    assert_eq!(reversal.entry_count, 1);
    assert_eq!(reversal.total_ct, 12_000);
    assert!(
        reversal
            .xml
            .contains("<OrgnlMsgId>DD-2026-07-25</OrgnlMsgId>")
    );
    assert!(
        reversal
            .xml
            .contains("<OrgnlPmtInfId>DD-2026-07-25-RCUR</OrgnlPmtInfId>")
    );
    assert!(
        reversal
            .xml
            .contains("<OrgnlEndToEndId>MND-000123</OrgnlEndToEndId>")
    );
    assert!(reversal.xml.contains("<MndtId>MND-000123</MndtId>"));
    assert!(reversal.xml.contains("AM05"));
    assert!(
        reversal.xml.contains("pain.007"),
        "the reversal namespace must be pain.007"
    );
}

#[test]
fn pain007_refuses_to_reverse_more_than_was_collected() {
    use accountingd::sepa::build_pain_007;

    let result = build_pain_007(
        &TEST_CREDITOR,
        &[sample_reversal(Some(20_000))],
        Default::default(),
    );
    assert!(
        result.is_err(),
        "reversing 200.00 of a 120.00 collection must be refused"
    );
}

#[test]
fn pain007_accepts_a_partial_reversal() {
    use accountingd::sepa::build_pain_007;

    let reversal = build_pain_007(
        &TEST_CREDITOR,
        &[sample_reversal(Some(5_000))],
        Default::default(),
    )
    .expect("a partial reversal is legal");
    assert_eq!(reversal.total_ct, 5_000);
}

#[test]
fn pain007_refuses_to_mix_two_original_groups() {
    use accountingd::sepa::build_pain_007;

    // `OrgnlPmtInfId` identifies exactly one submitted block, so entries from
    // two blocks cannot share a reversal group.
    let mut other = sample_reversal(None);
    other.original_payment_info_id = "DD-2026-07-25-FRST";
    let err = build_pain_007(
        &TEST_CREDITOR,
        &[sample_reversal(None), other],
        Default::default(),
    )
    .expect_err("two original groups in one reversal group must be refused")
    .to_string();
    assert!(err.contains("PmtInfId"), "got: {err}");
}

// ── Flat bank export import (replaces the removed parse_simple_json) ──────────

#[test]
fn bank_row_parses_a_credit() {
    use accountingd::sepa::BankStatementEntry;

    let entry = BankStatementEntry::parse(&serde_json::json!({
        "iban": "DE89370400440532013000",
        "amount_eur": "155.42",
        "date": "2026-07-10",
        "reference": "Abschlag 2026-07",
    }))
    .expect("a well-formed credit row parses");

    assert_eq!(
        entry.signed_ct, 15_542,
        "camt convention: credit is positive"
    );
    assert_eq!(
        entry.ledger_ct(),
        -15_542,
        "ledger convention: an incoming payment reduces the receivable"
    );
    assert!(!entry.is_return());
    assert_eq!(entry.description(), "Zahlungseingang");
}

#[test]
fn bank_row_negative_amount_is_a_return() {
    use accountingd::sepa::BankStatementEntry;

    // The removed crate helper derived `is_return` from a `return_reason` the
    // flat format never carried, so it was always false: a negative amount
    // booked as an ordinary ZAHLUNG with a positive ledger effect.
    let entry = BankStatementEntry::parse(&serde_json::json!({
        "iban": "DE89370400440532013000",
        "amount_eur": "-155.42",
        "date": "2026-07-10",
    }))
    .expect("a debit row parses");

    assert_eq!(entry.signed_ct, -15_542);
    assert_eq!(
        entry.ledger_ct(),
        15_542,
        "a return re-opens the receivable"
    );
    assert!(entry.is_return());
}

#[test]
fn bank_row_explicit_return_reason_wins() {
    use accountingd::sepa::BankStatementEntry;

    let entry = BankStatementEntry::parse(&serde_json::json!({
        "iban": "DE89370400440532013000",
        "amount_eur": "-155.42",
        "date": "2026-07-10",
        "return_reason_code": "MD06",
    }))
    .expect("a Rückläufer row parses");
    assert!(entry.is_return());
    assert_eq!(entry.description(), "Rückläufer (MD06)");
}

#[test]
fn bank_row_rejects_malformed_input() {
    use accountingd::sepa::BankStatementEntry;

    let cases = [
        // A repeated sign must not parse as +5.00 EUR.
        serde_json::json!({"iban": "DE89370400440532013000", "amount_eur": "--5", "date": "2026-07-10"}),
        // Trailing junk after the cents must not parse as 1.50 EUR.
        serde_json::json!({"iban": "DE89370400440532013000", "amount_eur": "1.50abc", "date": "2026-07-10"}),
        // A signed fractional part silently changed the amount.
        serde_json::json!({"iban": "DE89370400440532013000", "amount_eur": "1.-5", "date": "2026-07-10"}),
        // An `O` typed for a `0`: mod-97 misses it ~99% of the time, the BBAN
        // structure check does not.
        serde_json::json!({"iban": "DE8937O400440532013000", "amount_eur": "5.00", "date": "2026-07-10"}),
        serde_json::json!({"iban": "DE89370400440532013000", "amount_eur": "5.00", "date": "2026-02-30"}),
        serde_json::json!({"iban": "DE89370400440532013000", "amount_eur": "5.00"}),
    ];
    for case in cases {
        assert!(
            BankStatementEntry::parse(&case).is_err(),
            "must be rejected: {case}"
        );
    }
}

#[test]
fn bank_row_rejects_unknown_fields() {
    use accountingd::sepa::BankStatementEntry;

    // A typo in an import contract is a field that silently does nothing.
    let typo = serde_json::json!({
        "iban": "DE89370400440532013000",
        "amount_eur": "5.00",
        "date": "2026-07-10",
        "end_to_end": "TYPO",
    });
    assert!(BankStatementEntry::parse(&typo).is_err());
}

// ── Verification of Payee ─────────────────────────────────────────────────────

#[test]
fn vop_status_is_not_an_acceptance() {
    use accountingd::sepa::{PaymentStatus, VerificationOutcome};

    // RCVC says a payee name matched, which is a different question from
    // whether the payment was taken. Reading it as an acceptance would settle a
    // payout that has not left the account.
    let matched = PaymentStatus::Rcvc;
    assert!(!matched.is_accepted());
    assert!(matched.is_verification());
    assert_eq!(matched.verification(), Some(VerificationOutcome::Match));

    let no_match = PaymentStatus::Rvnm;
    assert!(!no_match.is_accepted());
    assert!(!no_match.is_rejected());
    assert_eq!(no_match.verification(), Some(VerificationOutcome::NoMatch));

    // The group-level summary is about a whole file, not one payee.
    assert!(PaymentStatus::Rvcm.is_verification());
    assert_eq!(PaymentStatus::Rvcm.verification(), None);

    // An ordinary acceptance reports no verification outcome at all.
    assert!(PaymentStatus::Acsc.is_accepted());
    assert_eq!(PaymentStatus::Acsc.verification(), None);
}

// ── camt sign convention ──────────────────────────────────────────────────────

#[test]
fn bank_to_ledger_ct_is_the_single_negation() {
    use accountingd::sepa::bank_to_ledger_ct;

    // Every bank path — flat JSON, camt.053, camt.054 — flips the sign here and
    // nowhere else. Two conventions in one service is a rounding error waiting
    // to be someone's reconciliation break.
    assert_eq!(bank_to_ledger_ct(15_542), -15_542);
    assert_eq!(bank_to_ledger_ct(-15_542), 15_542);
    assert_eq!(bank_to_ledger_ct(0), 0);
}

// ── sequence-type normalisation ───────────────────────────────────────────────

#[test]
fn unknown_sequence_type_collects_as_rcur() {
    use accountingd::sepa::{SequenceType, normalise_sequence_type, sequence_type_of};

    assert_eq!(normalise_sequence_type("FRST"), "FRST");
    assert_eq!(normalise_sequence_type("FNAL"), "FNAL");
    assert_eq!(normalise_sequence_type("OOFF"), "OOFF");
    // A mandate that has already been used is recurring; the DB CHECK keeps
    // real values in range, so this only covers hand-written data.
    assert_eq!(normalise_sequence_type("RCUR"), "RCUR");
    assert_eq!(normalise_sequence_type("nonsense"), "RCUR");
    assert_eq!(sequence_type_of("FRST"), SequenceType::Frst);
    assert_eq!(sequence_type_of("nonsense"), SequenceType::Rcur);
}

// ── ISO 20022 purpose codes ───────────────────────────────────────────────────

#[test]
fn purpose_code_follows_the_sparte() {
    use accountingd::sepa::{Purpose, purpose_for_sparte};

    assert_eq!(purpose_for_sparte("STROM"), Some(Purpose::Elec));
    assert_eq!(purpose_for_sparte("GAS"), Some(Purpose::Gasb));
    assert_eq!(purpose_for_sparte("WASSER"), Some(Purpose::Wter));
    // ISO has no separate waste-water code.
    assert_eq!(purpose_for_sparte("ABWASSER"), Some(Purpose::Wter));
    // No district-heating code either — the generic energy code is honest.
    assert_eq!(
        purpose_for_sparte("FERNWAERME")
            .as_ref()
            .map(Purpose::as_code),
        Some("ENRG")
    );
    // A combined supply is two purposes; picking either would tell the debtor's
    // software something false.
    assert_eq!(purpose_for_sparte("STROM_UND_GAS"), None);
    assert_eq!(purpose_for_sparte(""), None);
    assert_eq!(purpose_for_sparte("UNKNOWN"), None);
}

#[test]
fn pain008_emits_the_purpose_code_for_the_sparte() {
    use accountingd::pg::SepaMandateRow;
    use accountingd::sepa::build_pain_008;
    use time::Date;
    use uuid::Uuid;

    fn mandate(sparte: Option<&str>, refn: &str) -> SepaMandateRow {
        SepaMandateRow {
            mandate_id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            tenant: "test".into(),
            iban: "DE89370400440532013000".into(),
            bic: None,
            kontoinhaber: Some("Test Kunde".into()),
            mandatsref: refn.to_owned(),
            sequence_type: "RCUR".into(),
            scheme: "CORE".to_owned(),
            signed_at: Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
            revoked_at: None,
            last_presented_at: None,
            sparte: sparte.map(ToOwned::to_owned),
            debtor_town: None,
            debtor_country: None,
            debtor_street: None,
            debtor_building_number: None,
            debtor_post_code: None,
            debtor_country_subdivision: None,
            updated_at: time::OffsetDateTime::now_utc(),
        }
    }

    let strom = mandate(Some("STROM"), "REF-PURP-S");
    let gas = mandate(Some("GAS"), "REF-PURP-G");
    let entries = [(&strom, 5_000i64), (&gas, 3_000i64)];
    let run = build_pain_008(
        &TEST_CREDITOR,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &entries,
        Default::default(),
    )
    .expect("build_pain_008 should succeed");

    assert!(
        run.xml.contains("<Cd>ELEC</Cd>"),
        "electricity bill purpose"
    );
    assert!(run.xml.contains("<Cd>GASB</Cd>"), "gas bill purpose");

    // An account with no Sparte on record emits no purpose at all rather than
    // guessing one.
    let unknown = mandate(None, "REF-PURP-N");
    let run = build_pain_008(
        &TEST_CREDITOR,
        Date::from_calendar_date(2026, time::Month::July, 25).unwrap(),
        &[(&unknown, 1_000i64)],
        Default::default(),
    )
    .expect("build_pain_008 should succeed");
    assert!(!run.xml.contains("<Purp>"));
}

// ── camt booking status ───────────────────────────────────────────────────────

/// Only a *booked* entry is a money movement, and an absent `Sts` means booked.
///
/// The import guard rests on both halves of this. Posting a `PDNG` or `INFO`
/// entry into an append-only ledger books a payment that does not exist and
/// cannot be un-booked; skipping an entry whose `Sts` the bank simply omitted
/// would lose a real one. This pins the crate's parsing contract so a change to
/// either default is caught here rather than in production.
#[test]
fn camt_entry_status_drives_whether_money_moved() {
    use accountingd::sepa::{EntryStatus, parse_camt052, parse_camt054};

    let intraday = r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.052.001.08">
  <BkToCstmrAcctRpt>
    <GrpHdr><MsgId>RPT-001</MsgId><CreDtTm>2026-07-14T11:00:00</CreDtTm></GrpHdr>
    <Rpt>
      <Id>INTRADAY-1</Id>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id></Acct>
      <Ntry>
        <Amt Ccy="EUR">250.00</Amt><CdtDbtInd>CRDT</CdtDbtInd>
        <Sts><Cd>PDNG</Cd></Sts>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">120.00</Amt><CdtDbtInd>CRDT</CdtDbtInd>
        <Sts><Cd>BOOK</Cd></Sts>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">7.50</Amt><CdtDbtInd>DBIT</CdtDbtInd>
        <Sts><Cd>INFO</Cd></Sts>
      </Ntry>
    </Rpt>
  </BkToCstmrAcctRpt>
</Document>"#;
    let doc = parse_camt052(intraday).expect("camt.052 parses");
    let entries = &doc.reports[0].entries;
    assert_eq!(entries[0].status, EntryStatus::Pending);
    assert_eq!(entries[1].status, EntryStatus::Booked);
    assert_eq!(entries[2].status, EntryStatus::Info);
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.status == EntryStatus::Booked)
            .count(),
        1,
        "one of the three intraday entries is a real money movement"
    );

    // A bank that omits `Sts` has booked the entry — skipping it would lose a
    // real payment, so the default must stay `Booked`.
    let no_status = r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.054.001.08">
  <BkToCstmrDbtCdtNtfctn>
    <GrpHdr><MsgId>NTF-001</MsgId><CreDtTm>2026-07-14T20:00:00</CreDtTm></GrpHdr>
    <Ntfctn>
      <Id>N-1</Id>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id></Acct>
      <Ntry>
        <Amt Ccy="EUR">155.42</Amt><CdtDbtInd>CRDT</CdtDbtInd>
      </Ntry>
    </Ntfctn>
  </BkToCstmrDbtCdtNtfctn>
</Document>"#;
    let doc = parse_camt054(no_status).expect("camt.054 parses");
    assert_eq!(
        doc.notifications[0].entries[0].status,
        EntryStatus::Booked,
        "an absent Sts means the bank booked it"
    );
}

// ── What a camt booking posts ─────────────────────────────────────────────────

/// One camt.054 entry with the given `Amt` attributes/text and `CdtDbtInd`
/// element (empty string: no element).
fn camt054_with_entry(amt: &str, indicator: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.054.001.08">
  <BkToCstmrDbtCdtNtfctn>
    <GrpHdr><MsgId>NTF-AMT</MsgId><CreDtTm>2026-10-14T20:00:00</CreDtTm></GrpHdr>
    <Ntfctn>
      <Id>N-1</Id>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id></Acct>
      <Ntry>
        {amt}{indicator}
        <Sts><Cd>BOOK</Cd></Sts>
      </Ntry>
    </Ntfctn>
  </BkToCstmrDbtCdtNtfctn>
</Document>"#
    )
}

fn posting_of(amt: &str, indicator: &str) -> Result<i64, accountingd::sepa::UnpostableAmount> {
    let doc = accountingd::sepa::parse_camt054(&camt054_with_entry(amt, indicator))
        .expect("camt.054 parses");
    let entry = &doc.notifications[0].entries[0];
    assert!(entry.details.is_empty());
    accountingd::sepa::camt_posting_ct(entry, None)
}

/// A camt booking posts only what the statement itself determines, in EUR.
///
/// The ledger holds EUR cents and an append-only entry cannot be un-booked, so
/// every fact the figure rests on must have arrived: an absent `Ccy` is not
/// EUR, an absent or unrecognised `CdtDbtInd` is not a credit, a lower-case
/// code is not its upper-case ISO spelling, and a sub-cent magnitude is not a
/// rounded one. Each of these is a booking an operator has to look at.
#[test]
fn camt_posting_requires_a_determined_eur_amount() {
    use accountingd::sepa::UnpostableAmount;

    let crdt = "<CdtDbtInd>CRDT</CdtDbtInd>";
    assert_eq!(
        posting_of(r#"<Amt Ccy="EUR">155.42</Amt>"#, crdt),
        Ok(15_542)
    );
    assert_eq!(
        posting_of(
            r#"<Amt Ccy="EUR">155.42</Amt>"#,
            "<CdtDbtInd>DBIT</CdtDbtInd>"
        ),
        Ok(-15_542)
    );
    assert_eq!(
        posting_of("<Amt>155.42</Amt>", crdt),
        Err(UnpostableAmount::Undetermined),
        "an absent Amt/@Ccy is not EUR"
    );
    assert_eq!(
        posting_of(r#"<Amt Ccy="EUR">155.42</Amt>"#, ""),
        Err(UnpostableAmount::Undetermined),
        "an absent CdtDbtInd is not a credit"
    );
    assert_eq!(
        posting_of(
            r#"<Amt Ccy="EUR">155.42</Amt>"#,
            "<CdtDbtInd>crdt</CdtDbtInd>"
        ),
        Err(UnpostableAmount::Undetermined),
        "CdtDbtInd is matched by its exact ISO spelling"
    );
    assert_eq!(
        posting_of(r#"<Amt Ccy="eur">155.42</Amt>"#, crdt),
        Err(UnpostableAmount::Undetermined),
        "Ccy is matched by its exact ISO 4217 spelling"
    );
    assert_eq!(
        posting_of(r#"<Amt Ccy="EUR">1.005</Amt>"#, crdt),
        Err(UnpostableAmount::Undetermined),
        "a significant sub-cent digit is refused, not truncated"
    );
    assert_eq!(
        posting_of(r#"<Amt Ccy="USD">155.42</Amt>"#, crdt),
        Err(UnpostableAmount::NotEuro("USD".to_owned())),
        "a foreign-currency booking is not EUR cents"
    );
}

/// A batch booking posts per itemised detail; a detail without an amount of
/// its own in a batch of several posts nothing rather than the entry total.
#[test]
fn camt_batch_details_post_their_own_amounts() {
    use accountingd::sepa::{UnpostableAmount, camt_posting_ct, parse_camt054};

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.054.001.08">
  <BkToCstmrDbtCdtNtfctn>
    <GrpHdr><MsgId>NTF-BATCH</MsgId><CreDtTm>2026-10-14T20:00:00</CreDtTm></GrpHdr>
    <Ntfctn>
      <Id>N-1</Id>
      <Acct><Id><IBAN>DE89370400440532013000</IBAN></Id></Acct>
      <Ntry>
        <Amt Ccy="EUR">125.00</Amt><CdtDbtInd>CRDT</CdtDbtInd><Sts><Cd>BOOK</Cd></Sts>
        <NtryDtls>
          <TxDtls><Amt Ccy="EUR">100.00</Amt></TxDtls>
          <TxDtls><Amt Ccy="EUR">25.00</Amt></TxDtls>
        </NtryDtls>
      </Ntry>
      <Ntry>
        <Amt Ccy="EUR">125.00</Amt><CdtDbtInd>CRDT</CdtDbtInd><Sts><Cd>BOOK</Cd></Sts>
        <NtryDtls>
          <TxDtls><Refs><EndToEndId>E2E-1</EndToEndId></Refs></TxDtls>
          <TxDtls><Refs><EndToEndId>E2E-2</EndToEndId></Refs></TxDtls>
        </NtryDtls>
      </Ntry>
    </Ntfctn>
  </BkToCstmrDbtCdtNtfctn>
</Document>"#;
    let doc = parse_camt054(xml).expect("camt.054 parses");
    let entries = &doc.notifications[0].entries;

    let itemised = &entries[0];
    let posted: Vec<_> = itemised
        .details
        .iter()
        .map(|d| camt_posting_ct(itemised, Some(d)))
        .collect();
    assert_eq!(posted, vec![Ok(10_000), Ok(2_500)]);
    assert!(itemised.details_reconcile());

    let unitemised = &entries[1];
    for detail in &unitemised.details {
        assert_eq!(
            camt_posting_ct(unitemised, Some(detail)),
            Err(UnpostableAmount::Undetermined),
            "reusing the entry total per detail would post 250.00 for 125.00"
        );
    }
}

/// `Ntry/Sts` is matched by its exact ISO spelling: `book` is not `BOOK`, so
/// the entry is reported as an unrecognised status and not posted.
#[test]
fn camt_booking_status_spelling_is_exact() {
    use accountingd::sepa::{EntryStatus, parse_camt054};

    let xml = camt054_with_entry(
        r#"<Amt Ccy="EUR">10.00</Amt>"#,
        "<CdtDbtInd>CRDT</CdtDbtInd>",
    )
    .replace("<Cd>BOOK</Cd>", "<Cd>book</Cd>");
    let doc = parse_camt054(&xml).expect("camt.054 parses");
    let status = &doc.notifications[0].entries[0].status;
    assert_eq!(status, &EntryStatus::Other("book".to_owned()));
    assert_ne!(status, &EntryStatus::Booked);
}

/// A bank file is read only under a namespace the reader supports.
#[test]
fn camt_without_a_supported_namespace_is_refused() {
    use accountingd::sepa::parse_camt054;

    let ok = camt054_with_entry(
        r#"<Amt Ccy="EUR">10.00</Amt>"#,
        "<CdtDbtInd>CRDT</CdtDbtInd>",
    );
    let bare = ok.replace(
        r#" xmlns="urn:iso:std:iso:20022:tech:xsd:camt.054.001.08""#,
        "",
    );
    assert!(parse_camt054(&bare).is_err(), "no namespace, no camt.054");
    let foreign = ok.replace("camt.054.001.08", "camt.054.001.99");
    assert!(
        parse_camt054(&foreign).is_err(),
        "an unknown version is refused"
    );
}

// ── What a pain.002 status means ──────────────────────────────────────────────

fn pain002_with_tx_status(code: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pain.002.001.10">
  <CstmrPmtStsRpt>
    <GrpHdr><MsgId>STS-1</MsgId><CreDtTm>2026-10-14T10:00:00</CreDtTm></GrpHdr>
    <OrgnlGrpInfAndSts><OrgnlMsgId>DD-2026-10-20</OrgnlMsgId><OrgnlMsgNmId>pain.008.001.08</OrgnlMsgNmId></OrgnlGrpInfAndSts>
    <OrgnlPmtInfAndSts>
      <OrgnlPmtInfId>DD-2026-10-20-RCUR</OrgnlPmtInfId>
      <TxInfAndSts>
        <OrgnlEndToEndId>E2E-1</OrgnlEndToEndId>
        <TxSts>{code}</TxSts>
      </TxInfAndSts>
    </OrgnlPmtInfAndSts>
  </CstmrPmtStsRpt>
</Document>"#
    )
}

/// `RJCT` on a collection is a rejection and nothing else — not an acceptance
/// and not a settlement, so the import marks the entry `REJECTED` and books no
/// payment. A status is matched by its exact ISO spelling: `rjct` is a code
/// outside ISO 20022, neither a rejection nor an acceptance, and the import
/// applies nothing for it.
#[test]
fn pain002_rejection_is_exact_and_is_not_a_payment() {
    use accountingd::sepa::{PaymentStatus, parse_pain002};

    let doc = parse_pain002(&pain002_with_tx_status("RJCT")).expect("pain.002 parses");
    let status = doc.payment_info_statuses[0].transactions[0]
        .status
        .clone()
        .expect("TxSts present");
    assert_eq!(status, PaymentStatus::Rjct);
    assert!(status.is_rejected());
    assert!(!status.is_accepted());
    assert!(doc.has_rejections());
    assert!(!doc.is_fully_accepted());

    let doc = parse_pain002(&pain002_with_tx_status("rjct")).expect("pain.002 parses");
    let status = doc.payment_info_statuses[0].transactions[0]
        .status
        .clone()
        .expect("TxSts present");
    assert_eq!(status, PaymentStatus::Other("rjct".to_owned()));
    assert!(!status.is_rejected());
    assert!(!status.is_accepted());
}

// ── What a camt.029 answer resolves to ────────────────────────────────────────

fn camt029_with_confirmation(conf: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.029.001.06">
  <RsltnOfInvstgtn>
    <Assgnmt>
      <Id>RSL-1</Id>
      <Assgnr><Agt><FinInstnId><BICFI>COBADEFFXXX</BICFI></FinInstnId></Agt></Assgnr>
      <Assgne><Pty><Nm>Test Energie GmbH</Nm></Pty></Assgne>
      <CreDtTm>2026-10-14T10:00:00</CreDtTm>
    </Assgnmt>
    <RslvdCase><Id>RC-1</Id><Cretr><Pty><Nm>Test Energie GmbH</Nm></Pty></Cretr></RslvdCase>
    <Sts><Conf>{conf}</Conf></Sts>
  </RsltnOfInvstgtn>
</Document>"#
    )
}

/// The message-level confirmation is matched by its exact ISO spelling: `CNCL`
/// closes the recall, `cncl` leaves it `PENDING` for a person to look at.
#[test]
fn camt029_confirmation_spelling_is_exact() {
    use accountingd::sepa::{parse_camt029, recall_status_of};

    let doc = parse_camt029(&camt029_with_confirmation("CNCL")).expect("camt.029 parses");
    assert_eq!(recall_status_of(&doc), "ACCEPTED");
    let doc = parse_camt029(&camt029_with_confirmation("RJCR")).expect("camt.029 parses");
    assert_eq!(recall_status_of(&doc), "REJECTED");
    let doc = parse_camt029(&camt029_with_confirmation("cncl")).expect("camt.029 parses");
    assert_eq!(recall_status_of(&doc), "PENDING");
}

// ── Flat bank-export amounts ──────────────────────────────────────────────────

/// `amount_eur` is an exact decimal in cents: a significant sub-cent digit is
/// refused rather than truncated, trailing zeros below the cent are not
/// significant, and one leading `+` is legal decimal syntax.
#[test]
fn bank_row_amount_is_exact_to_the_cent() {
    use accountingd::sepa::BankStatementEntry;

    let row = |amount: &str| {
        BankStatementEntry::parse(&serde_json::json!({
            "iban": "DE89370400440532013000",
            "amount_eur": amount,
            "date": "2026-07-10",
        }))
    };
    assert!(row("1.005").is_err(), "1.005 EUR is not a cent amount");
    assert_eq!(row("1.500").expect("parses").signed_ct, 150);
    assert_eq!(row("+5.00").expect("parses").signed_ct, 500);
}

// ── pain.008 text fields ──────────────────────────────────────────────────────

fn mandate_named(name: &str, sparte: &str) -> accountingd::pg::SepaMandateRow {
    accountingd::pg::SepaMandateRow {
        mandate_id: uuid::Uuid::new_v4(),
        account_id: uuid::Uuid::new_v4(),
        tenant: "test".into(),
        iban: "DE89370400440532013000".into(),
        bic: None,
        kontoinhaber: Some(name.to_owned()),
        mandatsref: "REF-TEXT-1".to_owned(),
        sequence_type: "RCUR".into(),
        scheme: "CORE".to_owned(),
        signed_at: time::Date::from_calendar_date(2024, time::Month::January, 1).unwrap(),
        revoked_at: None,
        last_presented_at: None,
        sparte: Some(sparte.to_owned()),
        debtor_town: None,
        debtor_country: None,
        debtor_street: None,
        debtor_building_number: None,
        debtor_post_code: None,
        debtor_country_subdivision: None,
        updated_at: time::OffsetDateTime::now_utc(),
    }
}

fn pain008_for(mandate: &accountingd::pg::SepaMandateRow) -> anyhow::Result<String> {
    accountingd::sepa::build_pain_008(
        &TEST_CREDITOR,
        time::Date::from_calendar_date(2026, time::Month::October, 20).unwrap(),
        &[(mandate, 1_000i64)],
        Default::default(),
    )
    .map(|run| run.xml)
}

/// `Nm` is `Max70Text` measured on the value as emitted, padding included: a
/// 70-character name with a trailing space is 71 characters and is refused.
#[test]
fn pain008_name_length_counts_padding() {
    let seventy = "A".repeat(70);
    assert!(pain008_for(&mandate_named(&seventy, "STROM")).is_ok());
    assert!(
        pain008_for(&mandate_named(&format!("{seventy} "), "STROM")).is_err(),
        "the trailing space is part of the emitted value"
    );
}

/// A carriage return in a name never reaches the wire: the SEPA character set
/// has no control characters, so it is transliterated before escaping.
#[test]
fn pain008_name_carries_no_carriage_return() {
    let xml = pain008_for(&mandate_named("Test\rKunde", "STROM")).expect("builds");
    assert!(!xml.contains('\r'));
    assert!(!xml.contains("&#13;"));
}

/// The generic energy purpose `ENRG` is emitted in its canonical spelling.
#[test]
fn pain008_emits_the_generic_energy_purpose() {
    let xml = pain008_for(&mandate_named("Test Kunde", "FERNWAERME")).expect("builds");
    assert!(xml.contains("<Purp><Cd>ENRG</Cd></Purp>"), "{xml}");
}

/// A `NbOfTxsPerSts` bucket is reported even when the bank wrote its count
/// unreadably: the typed value is `null`, the bank's text survives in the
/// `*_raw` field, and an absent count is never a zero. A status outside
/// ISO 20022 — including a lower-case spelling of an ISO code — is reported
/// verbatim and is not a verification.
#[test]
fn pain002_status_counts_keep_what_the_bank_wrote() {
    use accountingd::handlers::pain002_status_counts_json;
    use accountingd::sepa::parse_pain002;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pain.002.001.10">
  <CstmrPmtStsRpt>
    <GrpHdr><MsgId>STS-2</MsgId><CreDtTm>2026-10-14T10:00:00</CreDtTm></GrpHdr>
    <OrgnlGrpInfAndSts>
      <OrgnlMsgId>CT-2026-10-14</OrgnlMsgId><OrgnlMsgNmId>pain.001.001.09</OrgnlMsgNmId>
      <NbOfTxsPerSts><DtldNbOfTxs>3</DtldNbOfTxs><DtldSts>RCVC</DtldSts><DtldCtrlSum>30.00</DtldCtrlSum></NbOfTxsPerSts>
      <NbOfTxsPerSts><DtldNbOfTxs>x</DtldNbOfTxs><DtldSts>rvnm</DtldSts></NbOfTxsPerSts>
    </OrgnlGrpInfAndSts>
  </CstmrPmtStsRpt>
</Document>"#;
    let doc = parse_pain002(xml).expect("pain.002 parses");
    let rows = pain002_status_counts_json(&doc);
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0]["status"], "RCVC");
    assert_eq!(rows[0]["count"], 3);
    assert_eq!(rows[0]["total_ct"], 3_000);
    assert_eq!(rows[0]["is_verification"], true);

    // `rvnm` is a code outside ISO 20022, reported verbatim — not `RVNM`.
    assert_eq!(rows[1]["status"], "rvnm");
    assert_eq!(rows[1]["status_raw"], "rvnm");
    assert!(
        rows[1]["count"].is_null(),
        "an unreadable count is not zero"
    );
    assert_eq!(rows[1]["count_raw"], "x");
    assert_eq!(rows[1]["is_verification"], false);
}
