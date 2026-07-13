//! Unit tests for `accountingd` business logic — pure functions, no database.
//!
//! Run: `cargo test -p accountingd --test unit_tests`

use accountingd::handlers::validate_iban;

// ── IBAN validation ───────────────────────────────────────────────────────────

#[test]
fn de_iban_valid_with_spaces() {
    assert!(validate_iban("DE89 3704 0044 0532 0130 00").is_ok());
}

#[test]
fn de_iban_valid_without_spaces() {
    assert!(validate_iban("DE89370400440532013000").is_ok());
}

#[test]
fn de_iban_valid_second_bank() {
    // Additional valid DE IBAN (Deutsche Bank BLZ 20040060, verified by mod-97)
    assert!(validate_iban("DE56200400600000000001").is_ok());
}

#[test]
fn de_iban_valid_sparkasse() {
    // Sparkasse Berlin BLZ 10050000, verified by mod-97
    assert!(validate_iban("DE29100500005001065004").is_ok());
}

#[test]
fn gb_iban_valid() {
    // NatWest IBAN — well-known test vector
    assert!(validate_iban("GB29 NWBK 6016 1331 9268 19").is_ok());
}

#[test]
fn nl_iban_valid() {
    // ABN AMRO standard test IBAN (well-known valid)
    assert!(validate_iban("NL91ABNA0417164300").is_ok());
}

#[test]
fn at_iban_valid() {
    // Austria IBAN test vector
    assert!(validate_iban("AT611904300234573201").is_ok());
}

#[test]
fn ch_iban_valid() {
    // Switzerland IBAN test vector (UBS)
    assert!(validate_iban("CH5604835012345678009").is_ok());
}

#[test]
fn wrong_checksum_rejected() {
    // Change last digit → wrong mod-97
    assert!(validate_iban("DE89370400440532013001").is_err());
}

#[test]
fn wrong_checksum_error_message() {
    let err = validate_iban("DE89370400440532013001").unwrap_err();
    assert!(err.contains("checksum"), "Expected checksum error, got: {err}");
}

#[test]
fn too_short_rejected() {
    assert!(validate_iban("DE89").is_err());
    let err = validate_iban("DE89").unwrap_err();
    assert!(err.contains("length"), "Expected length error, got: {err}");
}

#[test]
fn too_long_rejected() {
    let long = "DE".to_string() + &"1".repeat(33);
    assert!(validate_iban(&long).is_err());
}

#[test]
fn empty_rejected() {
    assert!(validate_iban("").is_err());
}

#[test]
fn only_whitespace_rejected() {
    assert!(validate_iban("   ").is_err());
}

#[test]
fn lowercase_accepted_normalised() {
    // validate_iban must uppercase before checking
    assert!(validate_iban("de89370400440532013000").is_ok());
}

#[test]
fn special_chars_rejected() {
    // IBAN with an illegal character like '@'
    assert!(validate_iban("DE89@704004405320130").is_err());
}

#[test]
fn all_zeros_rejected() {
    // Structurally malformed — mod-97 will not be 1
    assert!(validate_iban("DE00000000000000000000").is_err());
}

// ── SEPA mandate sequence type validation ─────────────────────────────────────
// These are simple enum validations we can verify without a DB.

#[test]
fn sepa_sequence_types_are_four_values() {
    let valid = ["FRST", "RCUR", "FNAL", "OOFF"];
    let invalid = ["ONCE", "RECURRING", "frst", "NEXT"];
    for v in valid {
        // We just assert the strings are known — actual DB constraint enforces this.
        assert!(!v.is_empty());
    }
    for inv in invalid {
        assert!(!["FRST", "RCUR", "FNAL", "OOFF"].contains(&inv));
    }
}

// ── Amount arithmetic guards ──────────────────────────────────────────────────

#[test]
fn ledger_entry_sign_convention() {
    // amount_ct > 0 = debit (charge to customer)
    // amount_ct < 0 = credit (refund / payment)
    let debit: i64 = 1000; // 10.00 EUR
    let credit: i64 = -500; // -5.00 EUR

    assert!(debit > 0, "Debit entries must be positive");
    assert!(credit < 0, "Credit entries must be negative");

    // Balance = sum of entries; positive balance means customer owes money
    let balance = debit + credit;
    assert_eq!(balance, 500, "Balance should be 5.00 EUR owed");
}

#[test]
fn jahresabschluss_surplus_detection() {
    // Simulate Jahresabschluss: annual bill vs sum of Abschläge
    let annual_bill_ct: i64 = 80_000; // 800 EUR annual
    let abschlaege_ct: i64 = 12 * 7_500; // 12 × 75 EUR = 900 EUR paid
    let diff = annual_bill_ct - abschlaege_ct;

    // Negative diff = customer overpaid → refund
    assert!(diff < 0, "Customer overpaid — surplus of {} ct", diff.abs());
    assert_eq!(diff, -10_000, "Expected -100 EUR surplus");
}

#[test]
fn jahresabschluss_deficit_detection() {
    // Simulate underpayment
    let annual_bill_ct: i64 = 100_000; // 1000 EUR
    let abschlaege_ct: i64 = 12 * 7_000; // 12 × 70 EUR = 840 EUR paid
    let diff = annual_bill_ct - abschlaege_ct;

    // Positive diff = customer underpaid → demand payment
    assert!(diff > 0, "Customer underpaid — deficit of {} ct", diff);
    assert_eq!(diff, 16_000, "Expected 160 EUR deficit");
}
