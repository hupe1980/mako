//! Guard: the payment rows the seeder imports (`tools/mako-seed/world/`) are
//! rows `accountingd` accepts.
//!
//! See `services/vertragd/tests/it/seed_payloads.rs` for why. This one matters for
//! a reason of its own: `POST /api/v1/payments/import` takes an **array of
//! untyped JSON**, and each row is parsed individually — so a malformed row is
//! counted as `malformed` in the summary and the import still returns `200`.
//! A seed whose rows do not parse therefore reports success while booking
//! nothing, which is precisely the failure mode a smoke test exists to catch.

use accountingd::sepa::BankStatementEntry;

/// A body from the seeder's world, `tools/mako-seed/world/<file>`.
fn world(file: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/mako-seed/world")
        .join(file);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// The seeded payment row.
#[test]
fn the_seeded_payment_row_parses() {
    let rows = world("accountingd-payment.json");
    let row = rows.get(0).expect("the import is an array of rows");
    let entry = BankStatementEntry::parse(row)
        .expect("accountingd would skip this seeded row as malformed and still answer 200");

    // Cents, because that is what can be owed and paid.
    assert_eq!(entry.signed_ct, 13_248);
    assert_eq!(entry.iban.as_str(), "DE02120300000000202051");
    assert!(
        entry.reference.contains("51238696012"),
        "the Verwendungszweck names the MaLo — the rung of the resolution ladder \
         the seed lands on, because the account was opened from a billing event \
         and carries no bank details"
    );
}

/// A field the row does not have is a refusal, not a discarded value.
///
/// The row type has always denied unknown fields; what this pins is that the
/// **seed** is held to it. A `"betrag"` where `amount_eur` belongs would
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
