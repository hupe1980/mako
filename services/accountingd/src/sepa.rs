//! SEPA payment utilities for `accountingd` — powered by the `sepa` crate.
//!
//! | Capability | API | Use in accountingd |
//! |---|---|---|
//! | IBAN validation | `validate_iban` | mandate PUT, import_payments, creditor check |
//! | BIC validation | `validate_bic` | mandate PUT |
//! | SEPA Creditor ID (EPC AT-02) | `validate_creditor_id` | pain.008 `CdtrSchmeId` (mandatory) |
//! | pain.008 CORE + B2B | `Pain008Builder` + `DirectDebitGroup` | N-5 scheduler, run_sepa |
//! | Multi-group messages | one `PmtInf` per `SequenceType` in **one file** | single submission + single audit row |
//! | pain.001 SCT + SCT Inst | `Pain001Builder` + `CreditTransferGroup` | EEG Vergütung payout |
//! | pain.002 status report | `parse_pain002` | bank rejection → BANKRUECKLAST auto-entry |
//! | camt.053 statement | `parse_camt053` | end-of-day bank reconciliation |
//! | camt.054 notification | `parse_camt054` (XML) + `parse_simple_json` | payment import |
//! | EUR string ↔ ct | `ct_to_eur_str` / `ct_from_eur_str` | format helpers |
//!
//! Schema defaults are the current SEPA releases (`pain.008.001.08`,
//! `pain.001.001.09`); names and remittance text are transliterated into the
//! SEPA character set (`ä → ae`, DK style) while identifiers are rejected on
//! out-of-set characters — the bank echoes identifiers back verbatim, so
//! rewriting them would break reconciliation.

// ── Re-exports from the sepa crate ────────────────────────────────────────────

pub use sepa::camt::{CashEntry, EntryDetail};
pub use sepa::pain001::LocalInstrument;
pub use sepa::{Camt053Document, parse_camt053};
pub use sepa::{Camt054Document, parse_camt054};
pub use sepa::{CreditTransferEntry, CreditTransferGroup, Pain001Builder};
pub use sepa::{CreditorId, CreditorIdError, validate_creditor_id};
pub use sepa::{DirectDebitEntry, DirectDebitGroup, Pain008Builder};
pub use sepa::{DirectDebitScheme, SequenceType};
pub use sepa::{Iban, IbanError, validate_bic, validate_iban};
pub use sepa::{Pain002Document, PaymentStatus, parse_pain002};
pub use sepa::{ct_from_eur_str, ct_to_eur_str};

use crate::pg::SepaMandateRow;

// ── pain.008 run output ───────────────────────────────────────────────────────

/// One pain.008 message covering a collection date — a single file with one
/// `PmtInf` group per `SequenceType` present in the input.
///
/// The SEPA SDD Rulebook requires `FRST` and `RCUR` collections in separate
/// payment-information blocks; since sepa 0.4 those blocks live in **one
/// message**, so a collection run is one bank submission and one audit row in
/// `sepa_collection_runs` (whose `UNIQUE (tenant, collection_date)` previously
/// silently dropped the second of two per-sequence files).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain008Run {
    /// Generated pain.008.001.08 XML (validated before serialisation).
    pub xml: String,
    /// Total amount in ct across all groups.
    pub total_ct: i64,
    /// Number of mandate entries across all groups.
    pub entry_count: usize,
    /// Per-`PmtInf` breakdown, in emission order.
    pub groups: Vec<Pain008GroupInfo>,
}

/// Summary of one `PmtInf` block inside a [`Pain008Run`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain008GroupInfo {
    /// SEPA SequenceType of the block (`"FRST"`, `"RCUR"`, `"FNAL"`, `"OOFF"`).
    pub sequence_type: String,
    /// Mandate entries in this block.
    pub entry_count: usize,
    /// Total amount in ct in this block.
    pub total_ct: i64,
}

// ── pain.008 Direct Debit builder ─────────────────────────────────────────────

/// Build one pain.008 message from `accountingd`'s active mandate rows.
///
/// ## FRST/RCUR separation (SEPA Rulebook §3.8)
///
/// Entries are grouped by `SequenceType` into separate `PmtInf` blocks of the
/// **same message**, emitted in the fixed order FRST, RCUR, FNAL, OOFF with
/// `PmtInfId = <MsgId>-<SEQ>` for bank-statement reconciliation.
///
/// ## Gläubiger-ID (EPC AT-02, mandatory)
///
/// `creditor_id_str` is required: the EPC rulebook mandates `CdtrSchmeId`, and
/// the sepa crate refuses to build a group without it. Obtain the identifier
/// from the Bundesbank creditor registry.
///
/// ## Parameters
///
/// - `creditor_iban_str` — IBAN of the LF's bank account (creditor side)
/// - `creditor_name`     — name of the LF/creditor (transliterated to SEPA charset)
/// - `creditor_id_str`   — SEPA Creditor Identifier (EPC AT-02)
/// - `collection_date`   — requested collection date (`ReqdColltnDt`)
/// - `entries`           — slice of `(mandate_row, amount_ct)` pairs
pub fn build_pain_008(
    creditor_iban_str: &str,
    creditor_name: &str,
    creditor_id_str: &str,
    collection_date: time::Date,
    entries: &[(&SepaMandateRow, i64)],
) -> anyhow::Result<Pain008Run> {
    let creditor_iban = validate_iban(creditor_iban_str).map_err(|e| {
        anyhow::anyhow!(
            "creditor IBAN '{creditor_iban_str}' is invalid: {e}. \
             Set a valid SEPA IBAN in [creditor_iban] config. \
             pain.008 generation is blocked until this is corrected."
        )
    })?;
    let creditor_id = validate_creditor_id(creditor_id_str).map_err(|e| {
        anyhow::anyhow!(
            "creditor_id '{creditor_id_str}' is invalid: {e}. \
             Set the SEPA Creditor Identifier (Bundesbank registry) in \
             [creditor_id] config — the EPC rulebook mandates CdtrSchmeId."
        )
    })?;

    let today = time::OffsetDateTime::now_utc();
    let msg_id = format!(
        "DD-{}-{:02}-{:02}",
        collection_date.year(),
        collection_date.month() as u8,
        collection_date.day()
    );
    let collection_date_str = format!(
        "{}-{:02}-{:02}",
        collection_date.year(),
        collection_date.month() as u8,
        collection_date.day()
    );

    // Fixed emission order — deterministic files for golden tests and audits.
    const SEQ_ORDER: [(&str, SequenceType); 4] = [
        ("FRST", SequenceType::Frst),
        ("RCUR", SequenceType::Rcur),
        ("FNAL", SequenceType::Fnal),
        ("OOFF", SequenceType::Ooff),
    ];

    let mut builder = Pain008Builder::new(creditor_name).msg_id(msg_id.clone());
    let mut groups_info = Vec::new();
    let mut total_ct = 0i64;
    let mut entry_count = 0usize;

    for (seq_key, seq_type) in SEQ_ORDER {
        let group_entries: Vec<&(&SepaMandateRow, i64)> = entries
            .iter()
            .filter(|(m, _)| {
                let key = match m.sequence_type.as_str() {
                    "FRST" => "FRST",
                    "FNAL" => "FNAL",
                    "OOFF" => "OOFF",
                    _ => "RCUR",
                };
                key == seq_key
            })
            .collect();
        if group_entries.is_empty() {
            continue;
        }

        let mut group = DirectDebitGroup::new(creditor_name, &creditor_iban, creditor_id.clone())
            .sequence_type(seq_type)
            .collection_date(collection_date_str.clone())
            .payment_info_id(format!("{msg_id}-{seq_key}"));

        let mut group_ct = 0i64;
        let mut group_n = 0usize;
        for (mandate, amount_ct) in group_entries {
            let debtor_iban = match validate_iban(&mandate.iban) {
                Ok(iban) => iban,
                Err(e) => {
                    tracing::warn!(
                        mandate_id = %mandate.mandate_id,
                        error = %e,
                        "accountingd: skipping mandate with invalid debtor IBAN in pain.008"
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

            group_ct += *amount_ct;
            group_n += 1;
            group = group.add_entry(entry);
        }

        if group_n == 0 {
            continue; // every mandate in this sequence had an invalid IBAN
        }
        total_ct += group_ct;
        entry_count += group_n;
        groups_info.push(Pain008GroupInfo {
            sequence_type: seq_key.to_owned(),
            entry_count: group_n,
            total_ct: group_ct,
        });
        builder = builder.add_group(group);
    }

    if entry_count == 0 {
        anyhow::bail!("pain.008 run has no billable mandates (all entries invalid or empty)");
    }

    let xml = builder
        .build()
        .map_err(|e| anyhow::anyhow!("pain.008 validation failed: {e}"))?;

    Ok(Pain008Run {
        xml,
        total_ct,
        entry_count,
        groups: groups_info,
    })
}

// ── pain.001 Credit Transfer ─────────────────────────────────────────────────

/// Build a pain.001 SEPA Credit Transfer XML for outgoing payments.
///
/// ## Use cases in accountingd
///
/// 1. **EEG Einspeisevergütung** — NB pays plant operator for monthly feed-in
///    (triggered by `de.eeg.verguetung.berechnet` from `einsd`).
/// 2. **Customer refund** — after Jahresabschluss, issue a `GUTSCHRIFT` ledger
///    entry AND a pain.001 to actually transfer funds back to the customer.
/// 3. **§19 EEG Einspeisemanagement compensation** — NB pays for curtailed kWh.
///
/// ## SCT Instant
///
/// Pass `instant = true` to emit `LclInstrm = INST`. The message stays on
/// `pain.001.001.09`, where `ReqdExctnDt` is emitted in the schema-valid
/// `<Dt>` choice form. Debtor agents without a known BIC use the EPC
/// "IBAN only" form — `NOTPROVIDED` is never written as a BIC.
///
/// ## Parameters
///
/// - `debtor_name`     — the operator/LF's legal name (`Dbtr/Nm`)
/// - `debtor_iban_str` — the operator/LF's own bank account (debit side)
/// - `entries`         — slice of `(creditor_iban, creditor_name, amount_ct, end_to_end_ref)`
/// - `instant`         — request SCT Instant execution
pub fn build_pain_001(
    debtor_name: &str,
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

    let mut group = CreditTransferGroup::new(debtor_name, &debtor_iban);
    if instant {
        group = group.local_instrument(LocalInstrument::Inst);
    }
    for (creditor_iban_str, creditor_name, amount_ct, e2e_ref) in entries {
        let creditor_iban = validate_iban(creditor_iban_str)
            .map_err(|e| anyhow::anyhow!("creditor IBAN '{creditor_iban_str}' invalid: {e}"))?;
        group = group.add_entry(CreditTransferEntry::new(
            creditor_name.to_string(),
            creditor_iban,
            *amount_ct,
            e2e_ref.to_string(),
        ));
    }

    Pain001Builder::new(debtor_name)
        .msg_id(msg_id)
        .add_group(group)
        .build()
        .map_err(|e| anyhow::anyhow!("pain.001 validation failed: {e}"))
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
    use rust_decimal::dec;
    use rust_decimal::prelude::*;

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
