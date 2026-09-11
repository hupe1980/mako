//! Guard: the payment rows `demos/o2c` imports are rows `accountingd` accepts.
//!
//! See `services/vertragd/tests/demo_payloads.rs` for why. This one matters for
//! a reason of its own: `POST /api/v1/payments/import` takes an **array of
//! untyped JSON**, and each row is parsed individually — so a malformed row is
//! counted as `malformed` in the summary and the import still returns `200`.
//! A demo whose rows do not parse therefore reports success while booking
//! nothing, which is precisely the failure mode a smoke test exists to catch.

use accountingd::sepa::BankStatementEntry;

/// The demo's payment row.
#[test]
fn the_o2c_payment_row_parses() {
    let entry = BankStatementEntry::parse(&serde_json::json!({
        "iban": "DE02120300000000202051",
        "amount_eur": "108.68",
        "date": "2026-09-10",
        "reference": "Rechnung RE-2026-000001 Marktlokation 51238696781",
        "bank_transaction_id": "DEMO-PAY-51238696781"
    }))
    .expect("accountingd would skip this demo row as malformed and still answer 200");

    // Cents, because that is what can be owed and paid.
    assert_eq!(entry.signed_ct, 10_868);
    assert_eq!(entry.iban.as_str(), "DE02120300000000202051");
    assert!(
        entry.reference.contains("51238696781"),
        "the Verwendungszweck names the MaLo — the rung of the resolution ladder \
         this demo lands on, because the account was opened from a billing event \
         and carries no bank details"
    );
}

/// A field the row does not have is a refusal, not a discarded value.
///
/// The row type has always denied unknown fields; what this pins is that the
/// **demo** is held to it. A `"betrag"` where `amount_eur` belongs would
/// otherwise be a row counted as malformed inside a `200`.
#[test]
fn a_field_the_row_does_not_have_is_refused() {
    let err = BankStatementEntry::parse(&serde_json::json!({
        "iban": "DE02120300000000202051",
        "amount_eur": "108.68",
        "date": "2026-09-10",
        "verwendungszweck": "Rechnung RE-2026-000001"
    }))
    .expect_err("the field is `reference`");
    assert!(err.to_string().contains("verwendungszweck"), "{err}");
}
