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

// ── pain.008 batch output ─────────────────────────────────────────────────────

/// One pain.008 XML batch with its sequence type.
///
/// The SEPA SDD Core Rulebook requires `FRST` and `RCUR` mandates to be in separate
/// `<PmtInf>` blocks. `build_pain_008` returns one batch per unique `SequenceType`
/// found in the input entries, so callers receive at most 4 batches (FRST, RCUR, FNAL, OOFF).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain008Batch {
    /// SEPA SequenceType for all entries in this batch (serialized as string e.g. "FRST").
    #[serde(serialize_with = "serialize_sequence_type")]
    pub sequence_type: SequenceType,
    /// Generated pain.008.003.02 XML string.
    pub xml: String,
    /// Total amount in ct across all entries in this batch.
    pub total_ct: i64,
    /// Number of mandate entries in this batch.
    pub entry_count: usize,
}

fn serialize_sequence_type<S>(seq: &SequenceType, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let label = match seq {
        SequenceType::Frst => "FRST",
        SequenceType::Rcur => "RCUR",
        SequenceType::Fnal => "FNAL",
        SequenceType::Ooff => "OOFF",
        _ => "RCUR", // fallback for any future variants
    };
    s.serialize_str(label)
}

// ── pain.008 Direct Debit builder ─────────────────────────────────────────────

/// Build pain.008.003.02 XML batches from `accountingd`'s active mandate rows.
///
/// ## FRST/RCUR batch separation (SEPA Rulebook §3.8)
///
/// The EPC SEPA SDD Core Rulebook requires that `FRST` (first collection) and
/// `RCUR` (recurring) mandates be in **separate payment information blocks**.
/// Many German clearing houses enforce this at the file level.
///
/// This function returns one `Pain008Batch` per distinct `SequenceType` found
/// in `entries`.  The scheduler stores each batch separately in `sepa_collection_runs`
/// and dispatches them individually to the bank.
///
/// ## Gläubiger-ID (EPC AT-02, mandatory)
///
/// `creditor_id_str` is the SEPA Creditor Identifier (e.g. `DE74ZZZ09999999999`).
/// When `Some`, it is validated via `validate_creditor_id` and included in the
/// `CdtrSchmeId` element.  Banks will **reject** batches without a valid CI.
/// Obtain your CI from your bank or the Bundesbank creditor registry.
///
/// ## Parameters
///
/// - `creditor_iban_str` — IBAN of the LF's bank account (creditor side)
/// - `creditor_name`     — Name of the LF/creditor (e.g. "Muster Energie GmbH")
/// - `creditor_id_str`   — SEPA Creditor Identifier (EPC AT-02), **mandatory in production**
/// - `entries`           — slice of `(mandate_row, amount_ct)` pairs
pub fn build_pain_008(
    creditor_iban_str: &str,
    creditor_name: &str,
    creditor_id_str: Option<&str>,
    entries: &[(&SepaMandateRow, i64)],
) -> anyhow::Result<Vec<Pain008Batch>> {
    let creditor_iban = validate_iban(creditor_iban_str).map_err(|e| {
        anyhow::anyhow!(
            "creditor IBAN '{creditor_iban_str}' is invalid: {e}. \
             Set a valid SEPA IBAN in [creditor_iban] config. \
             pain.008 generation is blocked until this is corrected."
        )
    })?;

    // Validate and parse the Gläubiger-ID (mandatory per EPC SDD Rulebook §2.4).
    // Warn in dev mode when absent; production banks will reject without it.
    let parsed_creditor_id = match creditor_id_str {
        Some(id_str) => {
            let id = validate_creditor_id(id_str).map_err(|e| {
                anyhow::anyhow!(
                    "creditor_id '{id_str}' is invalid: {e}. \
                     Obtain your SEPA Creditor Identifier from your bank."
                )
            })?;
            Some(id)
        }
        None => {
            tracing::warn!(
                "accountingd: creditor_id not configured — pain.008 XML missing CdtrSchmeId. \
                 Banks may reject the batch. Set [creditor_id] in accountingd.toml."
            );
            None
        }
    };

    let today = time::OffsetDateTime::now_utc();

    // Group entries by SequenceType — SEPA Rulebook requires separate batches.
    use std::collections::HashMap;
    let mut groups: HashMap<&'static str, Vec<(&SepaMandateRow, i64)>> = HashMap::new();

    for &(mandate, amount_ct) in entries {
        let key: &'static str = match mandate.sequence_type.as_str() {
            "FRST" => "FRST",
            "FNAL" => "FNAL",
            "OOFF" => "OOFF",
            _ => "RCUR", // default for RCUR and unknown values
        };
        groups.entry(key).or_default().push((mandate, amount_ct));
    }

    let mut batches = Vec::with_capacity(groups.len());

    // Process each sequence type as a separate batch
    for (seq_key, group_entries) in &groups {
        let seq_type = match *seq_key {
            "FRST" => SequenceType::Frst,
            "FNAL" => SequenceType::Fnal,
            "OOFF" => SequenceType::Ooff,
            _ => SequenceType::Rcur,
        };

        let msg_id = format!("DD-{}-{:02}-{}", today.year(), today.month() as u8, seq_key);

        let mut builder = Pain008Builder::new(creditor_name, &creditor_iban)
            .msg_id(msg_id)
            .sequence_type(seq_type);

        // Apply Gläubiger-ID when configured
        if let Some(ref cid) = parsed_creditor_id {
            builder = builder.creditor_id(cid.clone());
        }

        for (mandate, amount_ct) in group_entries {
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

        if builder.entry_count() == 0 {
            continue; // skip empty batches (all mandates had invalid IBANs)
        }

        let total_ct = builder.total_ct();
        let entry_count = builder.entry_count();
        let xml = builder.build_xml();

        batches.push(Pain008Batch {
            sequence_type: seq_type,
            xml,
            total_ct,
            entry_count,
        });
    }

    Ok(batches)
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

// ── Verzugszinsen §288 BGB calculation ───────────────────────────────────────

/// Calculate default interest (Verzugszinsen) per §288 BGB.
///
/// ## §288 BGB reference rates
/// - B2C (§288 Abs. 1): ECB Basiszinssatz + 5 percentage points
/// - B2B (§288 Abs. 2): ECB Basiszinssatz + 9 percentage points
///
/// Formula: `interest_ct = principal_ct × annual_rate × days / 36500`
/// (using 365-day year, integer arithmetic, no f64)
///
/// Returns the interest amount in ct (EUR-cent), rounded down to whole cents.
pub fn calculate_interest_ct(
    principal_ct: i64,
    ecb_base_rate_pct: rust_decimal::Decimal,
    is_b2b: bool,
    days: i64,
) -> (i64, rust_decimal::Decimal) {
    use rust_decimal::prelude::*;
    use rust_decimal_macros::dec;

    let premium = if is_b2b { dec!(9) } else { dec!(5) };
    let annual_rate = ecb_base_rate_pct + premium;
    // Formula: interest = principal × annual_rate × days / (100 × 365)
    // = principal × annual_rate × days / 36500
    // Note: do NOT divide by 100 separately — 36500 = 100 × 365 already combines both.
    let interest_dec =
        Decimal::from(principal_ct) * annual_rate * Decimal::from(days) / dec!(36500);
    let interest_ct = interest_dec.floor().to_i64().unwrap_or(0);
    (interest_ct, annual_rate)
}
