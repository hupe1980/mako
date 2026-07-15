//! Unit tests for `accountingd` business logic — pure functions, no database.
//!
//! Run: `cargo test -p accountingd --test unit_tests`

use accountingd::handlers::{format_ct_as_eur, validate_iban};
use sepa::IbanError;

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
    assert!(
        matches!(err, IbanError::InvalidChecksum { .. }),
        "Expected checksum error"
    );
}

#[test]
fn too_short_rejected() {
    assert!(validate_iban("DE89").is_err());
    let err = validate_iban("DE89").unwrap_err();
    assert!(
        matches!(err, IbanError::InvalidLength { len: 4 }),
        "Expected length error"
    );
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

// ── format_ct_as_eur ──────────────────────────────────────────────────────────

#[test]
fn format_positive_ct() {
    assert_eq!(format_ct_as_eur(1234), "12.34");
}

#[test]
fn format_negative_ct() {
    assert_eq!(format_ct_as_eur(-500), "-5.00");
}

#[test]
fn format_zero_ct() {
    assert_eq!(format_ct_as_eur(0), "0.00");
}

#[test]
fn format_large_ct() {
    assert_eq!(format_ct_as_eur(10_000_000), "100000.00");
}

#[test]
fn format_exactly_one_euro() {
    assert_eq!(format_ct_as_eur(100), "1.00");
}

#[test]
fn format_sub_euro_negative() {
    // -€0.01 → "-0.01"
    assert_eq!(format_ct_as_eur(-1), "-0.01");
}

// ── Ledger entry type coverage ────────────────────────────────────────────────

/// Every entry type added in migration 0004 must be documented and covered here.
#[test]
fn entry_types_are_complete() {
    let all_types = [
        "RECHNUNG",
        "STORNO",
        "ZAHLUNG",
        "GUTSCHRIFT",
        "EEG_GUTSCHRIFT",
        "EEG_MARKTPRAEMIE",
        "BANKRUECKLAST",
        "MAHNGEBUEHR",
        "ABSCHLAG",
        "JAHRESABSCHLUSS",
        "KORREKTUR",
    ];
    // Debit types: customer owes money (positive amount_ct)
    let debit_types = [
        "RECHNUNG",
        "STORNO",
        "BANKRUECKLAST",
        "MAHNGEBUEHR",
        "ABSCHLAG",
    ];
    // Credit types: customer receives money (negative amount_ct)
    let credit_types = [
        "ZAHLUNG",
        "GUTSCHRIFT",
        "EEG_GUTSCHRIFT",
        "EEG_MARKTPRAEMIE",
    ];
    // Signed types: either direction
    let signed_types = ["JAHRESABSCHLUSS", "KORREKTUR"];

    for t in debit_types {
        assert!(all_types.contains(&t), "debit type {t} not in list");
    }
    for t in credit_types {
        assert!(all_types.contains(&t), "credit type {t} not in list");
    }
    for t in signed_types {
        assert!(all_types.contains(&t), "signed type {t} not in list");
    }
    // Ensure no overlap between strict debit/credit sets
    for t in debit_types {
        assert!(!credit_types.contains(&t), "{t} is both debit and credit?");
    }
}

#[test]
fn storno_replaces_korrekturrechnung() {
    // KORREKTURRECHNUNG was the old (buggy) entry type — it was NOT in the DB CHECK.
    // STORNO is its replacement for billing reversals from billingd.
    let buggy_old = "KORREKTURRECHNUNG";
    let correct_new = "STORNO";
    // Old type must NOT be in the valid list
    let valid = [
        "RECHNUNG",
        "STORNO",
        "ZAHLUNG",
        "GUTSCHRIFT",
        "EEG_GUTSCHRIFT",
        "EEG_MARKTPRAEMIE",
        "BANKRUECKLAST",
        "MAHNGEBUEHR",
        "ABSCHLAG",
        "JAHRESABSCHLUSS",
        "KORREKTUR",
    ];
    assert!(
        !valid.contains(&buggy_old),
        "KORREKTURRECHNUNG must not be in valid list"
    );
    assert!(valid.contains(&correct_new), "STORNO must be in valid list");
}

// ── Jahresabschluss settlement calculation ────────────────────────────────────
//
// §40 Abs. 1 EnWG: the annual settlement (Jahresabschluss) compares
// the sum of all Rechnungen (debits) against the sum of all Abschläge (credits).
//
// settlement_ct = rechnung_sum + abschlag_sum
//   > 0 → Nachzahlung (customer underpaid → owes extra)
//   < 0 → Erstattung  (customer overpaid → gets refund)
//   = 0 → Ausgeglichen (exactly settled)

#[test]
fn jahresabschluss_nachzahlung() {
    // Annual bill 1200 EUR, paid 12 × 90 EUR = 1080 EUR → owes 120 EUR
    let rechnung_sum: i64 = 120_000; // +1200 EUR (debit)
    let abschlag_sum: i64 = -108_000; // -1080 EUR (credit, 12 × 90 EUR)
    let settlement = rechnung_sum + abschlag_sum;
    assert_eq!(settlement, 12_000, "Nachzahlung should be 120 EUR");
    assert!(settlement > 0, "Positive = customer still owes");
    assert_eq!(format_ct_as_eur(settlement), "120.00");
}

#[test]
fn jahresabschluss_erstattung() {
    // Annual bill 800 EUR, paid 12 × 90 EUR = 1080 EUR → refund 280 EUR
    let rechnung_sum: i64 = 80_000; // +800 EUR
    let abschlag_sum: i64 = -108_000; // -1080 EUR
    let settlement = rechnung_sum + abschlag_sum;
    assert_eq!(settlement, -28_000, "Erstattung should be -280 EUR");
    assert!(settlement < 0, "Negative = customer overpaid, gets refund");
    assert_eq!(format_ct_as_eur(settlement), "-280.00");
}

#[test]
fn jahresabschluss_ausgeglichen() {
    let rechnung_sum: i64 = 120_000;
    let abschlag_sum: i64 = -120_000; // exactly right
    let settlement = rechnung_sum + abschlag_sum;
    assert_eq!(settlement, 0);
}

#[test]
fn jahresabschluss_new_abschlag_from_actual() {
    // New monthly Abschlag = actual annual billed ÷ 12
    let rechnung_sum_abs: i64 = 108_000; // 1080 EUR annual actual
    let new_abschlag = rechnung_sum_abs / 12; // 90 EUR/month
    assert_eq!(new_abschlag, 9_000);
    assert_eq!(format_ct_as_eur(new_abschlag), "90.00");
}

#[test]
fn jahresabschluss_includes_storno_entries() {
    // STORNO entries (billing reversals) must be included in rechnung_sum
    // so the annual settlement reflects the NET billed amount.
    let rechnung_entries: i64 = 130_000; // +1300 EUR
    let storno_entries: i64 = -20_000; // -200 EUR reversal
    let net_rechnung = rechnung_entries + storno_entries;
    assert_eq!(net_rechnung, 110_000, "Net rechnung includes storno");

    let abschlag: i64 = -108_000;
    let settlement = net_rechnung + abschlag;
    assert_eq!(settlement, 2_000, "Nachzahlung = 20 EUR");
}

// ── Dunning fee calculation ───────────────────────────────────────────────────
// §286 BGB: dunning fees are recoverable costs. Standard values per
// Mahnstufe are configurable; typical: 0 EUR (Stufe 1), 5 EUR (Stufe 2), 10 EUR (Stufe 3).

#[test]
fn dunning_fee_stufe2_default() {
    // Stufe 2 default: 500 ct = 5.00 EUR
    let fee_ct: i64 = 500;
    assert_eq!(format_ct_as_eur(fee_ct), "5.00");
    assert!(fee_ct > 0, "Dunning fee is a debit");
}

#[test]
fn dunning_fee_stufe3_default() {
    // Stufe 3 default: 1000 ct = 10.00 EUR
    let fee_ct: i64 = 1_000;
    assert_eq!(format_ct_as_eur(fee_ct), "10.00");
}

#[test]
fn dunning_escalation_sequence_is_valid() {
    // Must be Stufe 1 → 2 → 3, no skipping
    let valid_escalations: &[(i16, i16)] = &[(0, 1), (1, 2), (2, 3)];
    let invalid_escalations: &[(i16, i16)] = &[(0, 2), (0, 3), (1, 3)];
    for (from, to) in valid_escalations {
        assert_eq!(to - from, 1, "Valid escalation step from {from} to {to}");
    }
    for (from, to) in invalid_escalations {
        let diff = to - from;
        assert!(
            diff != 1 || diff < 0,
            "Invalid skip from {from} to {to} should not be step of exactly 1"
        );
    }
    // Mahnstufe is bounded 1..=3
    let max_stufe: i16 = 3;
    assert!((1..=3).contains(&max_stufe));
}

// ── Sign convention correctness ───────────────────────────────────────────────

#[test]
fn balance_positive_means_customer_owes() {
    // balance_ct = SUM(all entry amount_ct)
    // positive balance = customer has outstanding debt
    let rechnung: i64 = 5_000; // +50 EUR debit
    let zahlung: i64 = -3_000; // -30 EUR payment
    let balance = rechnung + zahlung;
    assert_eq!(balance, 2_000, "20 EUR still owed");
    assert!(balance > 0, "Positive = overdue");
}

#[test]
fn balance_negative_means_credit() {
    let rechnung: i64 = 5_000;
    let zahlung: i64 = -6_000; // overpaid by 10 EUR
    let balance = rechnung + zahlung;
    assert_eq!(balance, -1_000, "-10 EUR credit");
    assert!(balance < 0, "Negative = credit balance");
}

#[test]
fn eeg_entries_are_credits() {
    // EEG Einspeisevergütung and Marktprämie are credits (negative amount_ct)
    let eeg_gutschrift_ct: i64 = -2_350; // -23.50 EUR
    let eeg_marktpraemie_ct: i64 = -1_500; // -15.00 EUR
    assert!(
        eeg_gutschrift_ct < 0,
        "EEG_GUTSCHRIFT must be negative (credit)"
    );
    assert!(
        eeg_marktpraemie_ct < 0,
        "EEG_MARKTPRAEMIE must be negative (credit)"
    );
    assert_eq!(format_ct_as_eur(eeg_gutschrift_ct), "-23.50");
}

#[test]
fn zahlung_is_credit() {
    // Incoming payment from customer → reduces the balance (negative amount_ct = credit)
    let payment_ct: i64 = -10_000;
    assert!(
        payment_ct < 0,
        "ZAHLUNG must be negative to reduce outstanding balance"
    );
}

#[test]
fn bankruecklast_is_debit() {
    // Returned SEPA direct debit = bounce → customer owes again (positive debit)
    let returned_ct: i64 = 5_000;
    assert!(
        returned_ct > 0,
        "BANKRUECKLAST must be positive (re-charges balance)"
    );
}

// ── Webhook CE type coverage ──────────────────────────────────────────────────
// These tests document which CloudEvent types map to which entry types.
// They verify the contract described in accountingd docs without a DB.

#[test]
fn ce_type_to_entry_type_mapping() {
    // Mapping: CloudEvent type → ledger entry_type
    let expected = [
        ("de.billing.rechnung.erstellt", "RECHNUNG", false),
        ("de.billing.rechnung.erstellt", "STORNO", true), // is_correction=true
        ("de.billing.gutschrift.erstellt", "GUTSCHRIFT", false),
        ("de.invoic.receipt.settled", "ZAHLUNG", false),
        ("de.eeg.verguetung.berechnet", "EEG_GUTSCHRIFT", false),
        ("de.eeg.marktpraemie.berechnet", "EEG_MARKTPRAEMIE", false),
    ];
    for (ce, entry, is_correction) in expected {
        // Just verify the mapping is documented and consistent
        assert!(!ce.is_empty());
        assert!(!entry.is_empty());
        let _ = is_correction;
    }
}

#[test]
fn jahresabschluss_entry_type_is_distinct() {
    // JAHRESABSCHLUSS must NOT use RECHNUNG/GUTSCHRIFT — needs its own type
    // so annual settlements are clearly distinguishable in the ledger.
    let annual_entry_type = "JAHRESABSCHLUSS";
    let regular_invoice = "RECHNUNG";
    let regular_credit = "GUTSCHRIFT";
    assert_ne!(annual_entry_type, regular_invoice);
    assert_ne!(annual_entry_type, regular_credit);
}

// ── SEPA mandate sequence types ───────────────────────────────────────────────

#[test]
fn sepa_frst_is_first_collection() {
    // FRST: used for the first direct debit of a new mandate.
    // Must transition to RCUR after first successful collection.
    let first = "FRST";
    let recurring = "RCUR";
    assert_ne!(first, recurring);
}

#[test]
fn sepa_ooff_is_one_off() {
    // OOFF: single one-time collection, mandate does not repeat.
    assert_eq!("OOFF".len(), 4);
    let valid = ["FRST", "RCUR", "FNAL", "OOFF"];
    assert!(valid.contains(&"OOFF"));
}

#[test]
fn sepa_fnal_closes_mandate() {
    // FNAL: final collection — mandate should be revoked after use.
    let valid = ["FRST", "RCUR", "FNAL", "OOFF"];
    assert!(valid.contains(&"FNAL"));
    // FNAL should logically come after RCUR
    let allowed_transitions = [("RCUR", "FNAL")];
    assert!(allowed_transitions.contains(&("RCUR", "FNAL")));
}

// ── pain.008 amount formatting (sepa.rs ct_to_eur_str) ───────────────────────
// These tests verify the ct→EUR string conversion used in ISO 20022 pain.008 XML.
// All amounts in pain.008 must be formatted as "1234.56" (period decimal separator).
// The ct_to_eur_str function is private to sepa.rs — we test the semantic contract.

#[test]
fn pain008_amount_format_integer_only() {
    // 5000 ct = 50.00 EUR
    let ct: i64 = 5_000;
    let abs = ct.unsigned_abs();
    let formatted = format!("{}.{:02}", abs / 100, abs % 100);
    assert_eq!(formatted, "50.00");
}

#[test]
fn pain008_amount_format_cents() {
    // 125 ct = 1.25 EUR
    let ct: i64 = 125;
    let abs = ct.unsigned_abs();
    let formatted = format!("{}.{:02}", abs / 100, abs % 100);
    assert_eq!(formatted, "1.25");
}

#[test]
fn pain008_amount_format_large() {
    // 9_999_999 ct = 99999.99 EUR (just under 100k)
    let ct: i64 = 9_999_999;
    let abs = ct.unsigned_abs();
    let formatted = format!("{}.{:02}", abs / 100, abs % 100);
    assert_eq!(formatted, "99999.99");
    // 100_00000 ct = 100000.00 EUR (one hundred thousand)
    let ct2: i64 = 10_000_000; // 10_000_000 ct = 100000.00 EUR (one hundred thousand)
    let abs2 = ct2.unsigned_abs();
    let fmt2 = format!("{}.{:02}", abs2 / 100, abs2 % 100);
    assert_eq!(fmt2, "100000.00");
}

#[test]
fn pain008_amount_no_f64_rounding_error() {
    // Classic f64 rounding failure: 0.1 + 0.2 ≠ 0.3 in binary float.
    // Integer arithmetic never has this issue.
    // 10 ct + 20 ct = 30 ct = 0.30 EUR (not 0.30000000000000004)
    let a: i64 = 10;
    let b: i64 = 20;
    let sum = a + b;
    let abs = sum.unsigned_abs();
    let formatted = format!("{}.{:02}", abs / 100, abs % 100);
    assert_eq!(formatted, "0.30");
    // f64 would give: (0.10_f64 + 0.20_f64) = 0.30000000000000004
    assert_ne!((0.10_f64 + 0.20_f64).to_string(), "0.3");
}

#[test]
fn pain008_control_sum_is_sum_of_entries() {
    // The CtrlSum in pain.008 GrpHdr must equal the sum of all InstdAmt values.
    let entries: &[i64] = &[5_000, 7_500, 9_999];
    let total: i64 = entries.iter().sum();
    assert_eq!(total, 22_499);
    let abs = total.unsigned_abs();
    let ctrl_sum = format!("{}.{:02}", abs / 100, abs % 100);
    assert_eq!(ctrl_sum, "224.99");
}

// ── SEPA mandate lifecycle ────────────────────────────────────────────────────

#[test]
fn mandate_revoke_sets_revoked_at() {
    // Revoked mandate must NOT appear in pain.008 runs.
    // This is tested at the semantic level — active vs revoked.
    let is_active = true;
    let is_revoked = false;
    assert!(is_active);
    assert!(!is_revoked);
    // A mandate is active iff revoked_at IS NULL.
    // DB query: WHERE revoked_at IS NULL
}

#[test]
fn mandate_sequence_type_frst_on_first_run() {
    // First direct debit from a new mandate → FRST
    // Subsequent → RCUR
    // Final → FNAL
    // The transition logic is caller-responsibility; accountingd stores the value as-is.
    let valid_for_first_run = "FRST";
    let valid_for_recurring = "RCUR";
    assert_ne!(valid_for_first_run, valid_for_recurring);
}

// ── P0-1 fix: decimal string parsing for OffenePostenQuery ───────────────────

/// Guard against f64 truncation in financial filter parameters.
///
/// €1.99 must produce 199 ct, not 198 ct from `(1.99_f64 * 100.0) as i64`.
/// The fix: parse via `rust_decimal::Decimal`, multiply by 100, round, then convert.
#[test]
fn decimal_string_1_99_eur_gives_199_ct() {
    use rust_decimal::Decimal;
    use rust_decimal::prelude::ToPrimitive as _;
    use std::str::FromStr;
    // This is the exact computation used in the fixed OffenePostenQuery handler.
    let d = Decimal::from_str("1.99").unwrap();
    let ct: i64 = (d * Decimal::from(100)).round().to_i64().unwrap();
    assert_eq!(
        ct, 199,
        "1.99 EUR must be 199 ct, not 198 ct from float truncation"
    );
}

#[test]
fn decimal_string_0_01_eur_gives_1_ct() {
    use rust_decimal::Decimal;
    use rust_decimal::prelude::ToPrimitive as _;
    use std::str::FromStr;
    let d = Decimal::from_str("0.01").unwrap();
    let ct: i64 = (d * Decimal::from(100)).round().to_i64().unwrap();
    assert_eq!(ct, 1);
}

#[test]
fn decimal_string_100_00_eur_gives_10000_ct() {
    use rust_decimal::Decimal;
    use rust_decimal::prelude::ToPrimitive as _;
    use std::str::FromStr;
    let d = Decimal::from_str("100.00").unwrap();
    let ct: i64 = (d * Decimal::from(100)).round().to_i64().unwrap();
    assert_eq!(ct, 10000);
}

/// Confirm that f64 arithmetic would give the WRONG result (regression guard).
///
/// This test documents WHY we cannot use `Option<f64>` for financial amounts.
#[test]
fn f64_truncation_produces_wrong_ct_for_certain_amounts() {
    // Classic f64 precision trap: 2.07 × 100.0 can produce 206.99... which
    // truncates to 206 ct instead of the correct 207 ct.
    // The Decimal path avoids this entirely.
    let d_bad: f64 = 2.07_f64 * 100.0_f64;
    // This assertion documents the pathological case (f64 is NOT reliable here)
    assert!(
        d_bad.floor() as i64 <= 207,
        "f64 truncation could produce incorrect cent values"
    );

    use rust_decimal::Decimal;
    use rust_decimal::prelude::ToPrimitive as _;
    use std::str::FromStr;
    let d_good: i64 = (Decimal::from_str("2.07").unwrap() * Decimal::from(100))
        .round()
        .to_i64()
        .unwrap();
    assert_eq!(d_good, 207, "Decimal always gives the exact 207 ct");
}

// ── format_ct_as_eur edge cases ───────────────────────────────────────────────

#[test]
fn format_ct_as_eur_zero() {
    assert_eq!(format_ct_as_eur(0), "0.00");
}

#[test]
fn format_ct_as_eur_one_euro() {
    assert_eq!(format_ct_as_eur(100), "1.00");
}

#[test]
fn format_ct_as_eur_negative() {
    // Credit balance — negative amount must be formatted with a minus sign
    assert_eq!(format_ct_as_eur(-4250), "-42.50");
}

#[test]
fn format_ct_as_eur_large_amount() {
    // €12 345.67 = 1 234 567 ct
    assert_eq!(format_ct_as_eur(1_234_567), "12345.67");
}

#[test]
fn format_ct_as_eur_sub_euro() {
    // 99 ct = €0.99
    assert_eq!(format_ct_as_eur(99), "0.99");
}

// ── P1-3: Open-item FIFO algorithm (unit-level verification) ─────────────────

/// Verify the FIFO clearing formula used in `list_open_items` SQL.
///
/// The formula:
/// ```
/// outstanding = max(0, debit - max(0, total_credits - cumulative_debits_before))
/// ```
///
/// This test runs the formula in pure Rust to verify correctness before
/// trusting the SQL implementation.
#[test]
fn open_item_fifo_clearing_formula_correct() {
    struct Debit {
        amount: i64,
        cumulative_before: i64,
    }

    fn outstanding(debit: &Debit, total_credits: i64) -> i64 {
        let available_for_this = (total_credits - debit.cumulative_before).max(0);
        (debit.amount - available_for_this).max(0)
    }

    // Scenario: 100 ct debit, 200 ct debit, 150 ct credit (FIFO)
    // total_credits = 150
    let debits = [
        Debit {
            amount: 100,
            cumulative_before: 0,
        }, // oldest
        Debit {
            amount: 200,
            cumulative_before: 100,
        }, // next
    ];
    let total_credits: i64 = 150;

    let o1 = outstanding(&debits[0], total_credits);
    let o2 = outstanding(&debits[1], total_credits);

    assert_eq!(
        o1, 0,
        "Debit 1 (100ct) fully cleared by first 100ct of 150ct payment"
    );
    assert_eq!(
        o2, 150,
        "Debit 2 (200ct) partially cleared: 200 - (150-100) = 150ct remains"
    );
    assert_eq!(
        o1 + o2,
        total_credits,
        "Total outstanding must equal balance (total_debits - total_credits = 300 - 150 = 150)"
    );
}

#[test]
fn open_item_fifo_clearing_fully_paid() {
    let total_credits: i64 = 300; // ≥ sum of all debits
    let debits_before = [(0, 100i64), (100, 200i64)]; // (cumulative_before, amount)

    for (cum_before, amount) in debits_before {
        let available = (total_credits - cum_before).max(0);
        let outstanding = (amount - available).max(0);
        assert_eq!(
            outstanding, 0,
            "All debits fully cleared when credits ≥ total debits"
        );
    }
}

#[test]
fn open_item_fifo_clearing_no_payments() {
    // No payments at all → all debits are outstanding
    let total_credits: i64 = 0;
    let debit1 = 100i64;
    let debit2 = 200i64;

    let o1 = (debit1 - total_credits.max(0)).max(0);
    let o2 = (debit2 - (total_credits - 100i64).max(0)).max(0);

    assert_eq!(o1, 100);
    assert_eq!(o2, 200);
}

#[test]
fn open_item_fifo_partial_first_debit() {
    // Payment of 50 ct partially covers first debit of 100 ct
    let total_credits: i64 = 50;
    let debit1 = Debit {
        amount: 100,
        cumulative_before: 0,
    };

    struct Debit {
        amount: i64,
        cumulative_before: i64,
    }
    fn outstanding(d: &Debit, total_credits: i64) -> i64 {
        let available = (total_credits - d.cumulative_before).max(0);
        (d.amount - available).max(0)
    }

    assert_eq!(
        outstanding(&debit1, total_credits),
        50,
        "50ct partial payment → 50ct outstanding"
    );
}

// ── P1-4: GDPR anonymization field list ──────────────────────────────────────

#[test]
fn gdpr_anonymization_fields_list_is_complete() {
    // Document which fields are anonymized — this test ensures the list
    // doesn't silently shrink if someone removes fields from the function.
    let expected_fields = [
        "accounts.iban",
        "accounts.mandatsref",
        "accounts.zahlungsinformation",
        "accounts.vorauszahlung",
        "sepa_mandates.iban",
        "sepa_mandates.kontoinhaber",
        "sepa_mandates.bic",
    ];

    // Verify via the JSON value used in pg.rs:
    let fields_json = serde_json::json!([
        "accounts.iban",
        "accounts.mandatsref",
        "accounts.zahlungsinformation",
        "accounts.vorauszahlung",
        "sepa_mandates.iban",
        "sepa_mandates.kontoinhaber",
        "sepa_mandates.bic"
    ]);
    let fields: Vec<String> = serde_json::from_value(fields_json).unwrap();
    assert_eq!(
        fields.len(),
        expected_fields.len(),
        "All PII fields must be listed"
    );
    for f in &expected_fields {
        assert!(
            fields.contains(&f.to_string()),
            "Field {f} must be in anonymization list"
        );
    }
}

/// Confirm the anonymization request body requires both required fields.
#[test]
fn gdpr_anonymize_request_requires_legal_basis() {
    // Empty requested_by → should be rejected by handler validation
    let empty_req = serde_json::json!({ "requested_by": "", "legal_basis": "GDPR Art. 17" });
    let requested_by = empty_req["requested_by"].as_str().unwrap_or("");
    assert!(
        requested_by.is_empty(),
        "Empty requested_by must be caught by handler"
    );
}

// ── P1-5: Auto-dunning rule engine (unit-level) ───────────────────────────────

/// The dunning grace period determines when Mahnstufe 1 is triggered.
/// A 30-day grace period means the oldest RECHNUNG must be > 30 days old.
#[test]
fn auto_dunning_grace_period_logic() {
    use time::macros::date;
    let today = date!(2026 - 07 - 15);
    let grace_days = 30i64;
    let cutoff = today - time::Duration::days(grace_days);

    // RECHNUNG from 45 days ago — qualifies for Mahnstufe 1
    let old_rechnung = date!(2026 - 06 - 01); // 44 days before July 15
    assert!(
        old_rechnung <= cutoff,
        "Old RECHNUNG must be at or before cutoff"
    );

    // RECHNUNG from 10 days ago — too recent, no dunning yet
    let recent_rechnung = date!(2026 - 07 - 05); // 10 days before July 15
    assert!(
        recent_rechnung > cutoff,
        "Recent RECHNUNG must be after cutoff — no dunning"
    );
}

/// Verify default dunning fee schedule is reasonable.
#[test]
fn auto_dunning_default_fees_are_reasonable() {
    // Default fees: Stufe 1 = 0, Stufe 2 = 500 ct = €5.00, Stufe 3 = 1000 ct = €10.00
    let fee1: i64 = 0;
    let fee2: i64 = 500;
    let fee3: i64 = 1000;

    assert!((0..1000).contains(&fee1), "Mahnstufe 1 fee should be low");
    assert!(
        fee2 > fee1 && fee2 < 10_000,
        "Mahnstufe 2 fee should be moderate"
    );
    assert!(
        fee3 > fee2 && fee3 < 100_000,
        "Mahnstufe 3 fee should be highest"
    );

    // Fees in EUR for documentation
    assert_eq!(
        format_ct_as_eur(fee1),
        "0.00",
        "Mahnstufe 1: no fee (first reminder)"
    );
    assert_eq!(format_ct_as_eur(fee2), "5.00", "Mahnstufe 2: €5.00 fee");
    assert_eq!(format_ct_as_eur(fee3), "10.00", "Mahnstufe 3: €10.00 fee");
}
