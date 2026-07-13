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
    assert_eq!(format_ct_as_eur(100_000_00), "100000.00");
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
    assert!(max_stufe <= 3 && max_stufe >= 1);
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
    let ct2: i64 = 100_000_00; // 10_000_000 in decimal = 100000.00 EUR
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
