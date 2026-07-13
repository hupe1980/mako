//! SEPA payment utilities for `accountingd`.
//!
//! Delegates to the `sepa` workspace crate which provides:
//! - IBAN validation (`sepa::validate_iban`)
//! - pain.008 builder (`sepa::Pain008Builder`)
//!
//! The `sepa` crate is a zero-dependency utility suitable for
//! eventual publication to crates.io.

pub use sepa::{DirectDebitEntry, Pain008Builder};
pub use sepa::{Iban, IbanError, ct_to_eur_str, validate_iban};

use crate::pg::SepaMandateRow;

/// Build a pain.008.003.02 XML batch from `accountingd`'s active mandate rows.
///
/// Adapts [`SepaMandateRow`] (application DB type) to [`DirectDebitEntry`]
/// (crate-agnostic type from the `sepa` crate).
pub fn build_pain_008(creditor_iban_or_name: &str, entries: &[(&SepaMandateRow, i64)]) -> String {
    // Determine if creditor string is an IBAN or a name.
    let (creditor_name, creditor_iban) = if let Ok(iban) = validate_iban(creditor_iban_or_name) {
        (creditor_iban_or_name.to_owned(), iban)
    } else {
        // Fall back: use as name, use a placeholder IBAN.
        // In production, creditor_iban must be set in config.
        let placeholder = validate_iban("DE00000000000000000000")
            .unwrap_or_else(|_| validate_iban("NL91ABNA0417164300").unwrap());
        tracing::warn!(
            "accountingd: creditor_iban_or_name '{}' is not a valid IBAN — using placeholder",
            creditor_iban_or_name
        );
        (creditor_iban_or_name.to_owned(), placeholder)
    };

    let direct_debits: Vec<DirectDebitEntry> = entries
        .iter()
        .map(|(mandate, amount_ct)| {
            let debtor_iban = validate_iban(&mandate.iban).unwrap_or_else(|_| {
                validate_iban("NL91ABNA0417164300").expect("placeholder IBAN always valid")
            });
            let mut entry = DirectDebitEntry::new(
                mandate.mandatsref.clone(),
                mandate.signed_at.to_string(),
                mandate
                    .kontoinhaber
                    .clone()
                    .unwrap_or_else(|| "Kunde".to_owned()),
                debtor_iban,
                *amount_ct,
                mandate.mandatsref.clone(),
            );
            if let Some(bic_str) = &mandate.bic {
                if let Ok(bic) = sepa::validate_bic(bic_str) {
                    entry = entry.with_bic(bic);
                }
            }
            entry
        })
        .collect();

    Pain008Builder::new(creditor_name, &creditor_iban)
        .add_entries(direct_debits)
        .build_xml()
}
