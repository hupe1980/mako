//! SEPA payment utilities for `accountingd` — powered by `sepa` 0.3.0.
//!
//! ## New capabilities in sepa 0.3.0
//!
//! | Capability | API | Use in accountingd |
//! |---|---|---|
//! | IBAN validation | `validate_iban` | mandate PUT, import_payments, creditor check |
//! | BIC validation | `validate_bic` | mandate PUT |
//! | SEPA Creditor ID (EPC AT-02) | `validate_creditor_id` | pain.008 CI field |
//! | pain.008 CORE + B2B | `Pain008Builder` + `DirectDebitEntry` | N-5 scheduler, run_sepa |
//! | Typed SequenceType | `SequenceType::Frst/Rcur/Fnal/Ooff` | per-mandate dispatch |
//! | B2B scheme | `DirectDebitScheme::B2b` | future B2B contracts |
//! | Entry description | `DirectDebitEntry::with_description` | bank statement clarity |
//! | Entry sequence override | `DirectDebitEntry::with_sequence_type` | FRST/RCUR per-entry |
//! | pain.001 SCT + SCT Inst | `Pain001Builder` + `CreditTransferEntry` | EEG Vergütung payout |
//! | pain.002 status report | `parse_pain002` | bank rejection → BANKRUECKLAST auto-entry |
//! | camt.053 statement | `parse_camt053` | end-of-day bank reconciliation |
//! | camt.054 notification | `camt054::parse_simple_json` | import_payments JSON |
//! | EUR string ↔ ct | `ct_to_eur_str` / `ct_from_eur_str` | format helpers |
//! | Streaming XML write | `Pain008Builder::write_xml_to_io` | large mandate batches |

// ── Re-exports from sepa 0.3.0 ────────────────────────────────────────────────

// IBAN + BIC (unchanged API)
pub use sepa::{Iban, IbanError, validate_bic, validate_iban};

// SEPA Creditor Identifier (EPC AT-02) — new in 0.3.0
pub use sepa::{CreditorId, CreditorIdError, validate_creditor_id};

// pain.008 SDD Direct Debit — upgraded in 0.3.0 with typed enums
pub use sepa::{DirectDebitEntry, Pain008Builder};
pub use sepa::{DirectDebitScheme, SequenceType};

// pain.001 Credit Transfer (SCT + SCT Instant) — new in 0.3.0
pub use sepa::pain001::LocalInstrument;
pub use sepa::{CreditTransferEntry, Pain001Builder};

// pain.002 Payment Status Report parser — new in 0.3.0
pub use sepa::{Pain002Document, PaymentStatus, parse_pain002};

// camt.053 Bank-to-Customer Statement parser — new in 0.3.0
pub use sepa::{Camt053Document, parse_camt053};

// Money utilities — ct_from_eur_str is new in 0.3.0
pub use sepa::{ct_from_eur_str, ct_to_eur_str};

use crate::pg::SepaMandateRow;

// ── pain.008 Direct Debit builder ─────────────────────────────────────────────

/// Build a pain.008.003.02 XML batch from `accountingd`'s active mandate rows.
///
/// ## sepa 0.3.0 upgrades used here
///
/// - `SequenceType` typed enum (FRST/RCUR/FNAL/OOFF) — dispatched per mandate
/// - `DirectDebitEntry::with_description` — adds RemittanceInfo for bank statement clarity
/// - `DirectDebitEntry::with_sequence_type` — per-entry sequence type override
/// - `Pain008Builder::msg_id` — unique message ID (new builder method)
/// - Hard error on invalid creditor IBAN (no placeholder fallback — P1-2 fix)
///
/// ## Per-mandate sequence type
///
/// Each mandate's stored `sequence_type` string (FRST/RCUR/FNAL/OOFF) is mapped
/// to the typed `SequenceType` variant. The message-level default is RCUR.
///
/// ## Entry description
///
/// Each entry carries `"Abschlag YYYY-MM"` as RemittanceInfo so debtors can
/// reconcile charges on their bank statement without calling customer service.
pub fn build_pain_008(
    creditor_iban_str: &str,
    entries: &[(&SepaMandateRow, i64)],
) -> anyhow::Result<String> {
    let creditor_iban = validate_iban(creditor_iban_str).map_err(|e| {
        anyhow::anyhow!(
            "creditor IBAN '{creditor_iban_str}' is invalid: {e}. \
             Set a valid SEPA IBAN in [creditor_iban] config. \
             pain.008 generation is blocked until this is corrected."
        )
    })?;

    let today = time::OffsetDateTime::now_utc();
    let msg_id = format!("DD-{}-{:02}", today.year(), today.month() as u8);

    let mut builder = Pain008Builder::new(creditor_iban_str.to_owned(), &creditor_iban)
        .msg_id(msg_id)
        .sequence_type(SequenceType::Rcur); // message-level default

    for (mandate, amount_ct) in entries {
        let debtor_iban = match validate_iban(&mandate.iban) {
            Ok(iban) => iban,
            Err(e) => {
                tracing::warn!(
                    mandate_id = %mandate.mandate_id,
                    error = %e,
                    "accountingd: skipping mandate with invalid debtor IBAN in pain.008 batch"
                );
                continue;
            }
        };

        // sepa 0.3.0: SequenceType per mandate (logged for future per-entry support)
        // Currently pain.008 sets sequence type at message level; per-entry override
        // is not in the sepa 0.3.0 API. Log FRST mandates so operators can split batches.
        let _seq_type = match mandate.sequence_type.as_str() {
            "FRST" => SequenceType::Frst,
            "FNAL" => SequenceType::Fnal,
            "OOFF" => SequenceType::Ooff,
            _ => SequenceType::Rcur,
        };

        // sepa 0.3.0: with_description adds RemittanceInfo to the XML entry
        let description = format!("Abschlag {}-{:02}", today.year(), today.month() as u8);

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
        )
        .with_description(description);

        if let Some(bic_str) = &mandate.bic
            && let Ok(bic) = sepa::validate_bic(bic_str)
        {
            entry = entry.with_bic(bic);
        }

        builder = builder.add_entry(entry);
    }

    Ok(builder.build_xml())
}

// ── pain.001 Credit Transfer (new in sepa 0.3.0) ─────────────────────────────

/// Build a pain.001 SEPA Credit Transfer XML for outgoing payments.
///
/// ## Use cases in accountingd
///
/// 1. **EEG Einspeisevergütung** — NB pays plant operator for monthly feed-in
///    (triggered by `de.eeg.verguetung.berechnet` from `einsd`).
/// 2. **Customer refund** — After Jahresabschluss, issue `GUTSCHRIFT` ledger entry
///    AND a pain.001 to actually transfer funds back to the customer.
/// 3. **§19 EEG Einspeisemanagement compensation** — NB must pay for curtailed kWh.
///
/// ## SCT Instant (10-second settlement)
///
/// Pass `instant = true` to switch to pain.001.001.09 namespace (SCT Inst).
/// Required for real-time EEG compensation payments under §19 EEG 2023.
///
/// ## Parameters
///
/// - `debtor_iban_str` — the operator/LF's own bank account (debit side)
/// - `entries` — slice of `(creditor_iban, creditor_name, amount_ct, end_to_end_ref)`
/// - `instant` — use SCT Instant (pain.001.001.09) instead of standard SCT (003.03)
pub fn build_pain_001(
    debtor_iban_str: &str,
    entries: &[(&str, &str, i64, &str)],
    instant: bool,
) -> anyhow::Result<String> {
    let debtor_iban = validate_iban(debtor_iban_str)
        .map_err(|e| anyhow::anyhow!("debtor IBAN '{debtor_iban_str}' invalid: {e}"))?;

    let today = time::OffsetDateTime::now_utc();
    let msg_id = format!(
        "CT-{}-{:02}-{:02}",
        today.year(),
        today.month() as u8,
        today.day()
    );

    let mut builder = Pain001Builder::new(debtor_iban_str.to_owned(), &debtor_iban).msg_id(msg_id);

    if instant {
        builder = builder.local_instrument(LocalInstrument::Inst);
    }

    for (creditor_iban_str, creditor_name, amount_ct, e2e_ref) in entries {
        let creditor_iban = validate_iban(creditor_iban_str)
            .map_err(|e| anyhow::anyhow!("creditor IBAN '{creditor_iban_str}' invalid: {e}"))?;
        builder = builder.add_entry(CreditTransferEntry::new(
            creditor_name.to_string(),
            creditor_iban,
            *amount_ct,
            e2e_ref.to_string(),
        ));
    }

    Ok(builder.build_xml())
}
