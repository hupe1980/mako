//! SEPA payment utilities for `accountingd` — powered by the `sepa` crate.
//!
//! | Capability | API | Use in accountingd |
//! |---|---|---|
//! | IBAN validation | `validate_iban` | mandate PUT, payment import, creditor check |
//! | BIC validation | `validate_bic` | mandate PUT |
//! | SEPA Creditor ID (EPC AT-02) | `validate_creditor_id` | pain.008 `CdtrSchmeId` (mandatory) |
//! | Structured postal address | `PostalAddress` | `PstlAdr` on both sides (EPC cut-over) |
//! | pain.008 CORE + B2B | `Pain008Builder` + `DirectDebitGroup` | N-5 scheduler, `/sepa/run` |
//! | Multi-group messages | one `PmtInf` per `SequenceType` in **one file** | single submission + single audit row |
//! | pain.001 SCT + SCT Inst | `Pain001Builder` + `CreditTransferGroup` | EEG Vergütung payout |
//! | pain.007 SDD reversal | `Pain007Builder` + `ReversalGroup` | `/sepa/reversals` |
//! | pain.002 status report | `parse_pain002` | `/sepa/pain002` — RJCT → BANKRUECKLAST, VoP |
//! | camt.052 report | `parse_camt052` | `/payments/import/camt052` (intraday, booked entries only) |
//! | camt.053 statement | `parse_camt053` | `/payments/import/camt053` (end-of-day) |
//! | camt.054 notification | `parse_camt054` | `/payments/import/camt054` |
//! | EUR string ↔ ct | `ct_to_eur_str` / `ct_from_eur_str` | format helpers |
//!
//! Schema defaults are the current SEPA releases (`pain.008.001.08`,
//! `pain.001.001.09`); names, addresses and remittance text are transliterated
//! into the SEPA character set (`ä → ae`, DK style) while identifiers are
//! rejected on out-of-set characters — the bank echoes identifiers back
//! verbatim, so rewriting them would break reconciliation.
//!
//! ## One sign convention
//!
//! Everything read off a bank file uses the camt convention: **positive = credit
//! to our account**. [`bank_to_ledger_ct`] is the single place that flips it into
//! accountingd's open-items convention (positive = Forderung). The flat-JSON
//! import follows it too — one sign convention across the service.

// ── Re-exports from the sepa crate ────────────────────────────────────────────

pub use sepa::camt::{AccountRef, BatchInfo, CashEntry, EntryDetail, EntryStatus};
pub use sepa::pain001::{ExecutionMoment, LocalInstrument};
pub use sepa::pain002::{ReasonCode, TransactionStatus, VerificationOutcome};
pub use sepa::pain007::{
    OriginalCollection, Pain007Builder, ReversalEntry, ReversalGroup, ReversalReason,
};
pub use sepa::{AddressError, AddressFormat, PostalAddress};
pub use sepa::{Camt052Document, Camt052Report, parse_camt052};
pub use sepa::{Camt053Document, Camt053Statement, parse_camt053};
pub use sepa::{Camt054Document, parse_camt054};
pub use sepa::{CreditTransferEntry, CreditTransferGroup, Pain001Builder};
pub use sepa::{CreditTransferSchema, DirectDebitSchema};
pub use sepa::{CreditorId, CreditorIdError, validate_creditor_id};
pub use sepa::{DirectDebitEntry, DirectDebitGroup, Pain008Builder};
pub use sepa::{DirectDebitScheme, SequenceType};
pub use sepa::{Iban, IbanError, validate_bic, validate_iban};
pub use sepa::{IsoDate, IsoDateTime};
pub use sepa::{Pain002Document, PaymentStatus, parse_pain002};
pub use sepa::{Purpose, PurposeCodeError};
pub use sepa::{ct_from_eur_str, ct_to_eur_str};

use crate::pg::SepaMandateRow;

// ── Sign convention ───────────────────────────────────────────────────────────

/// Bank-statement sign → ledger sign.
///
/// A camt entry is signed from the bank's point of view (`CdtDbtInd`): positive
/// is money arriving in our account. accountingd's ledger is an open-items
/// account where positive is a *Forderung*, so an incoming payment **reduces**
/// the balance and a returned direct debit **re-opens** it. One negation, in one
/// place — the crate removed `Camt054Entry::to_ledger_ct`, whose opposite
/// convention sitting beside `CashEntry::signed_ct` was a rounding error waiting
/// to happen.
#[must_use]
pub const fn bank_to_ledger_ct(signed_ct: i64) -> i64 {
    -signed_ct
}

// ── Postal addresses (EPC structured-address cut-over) ────────────────────────

/// The parts of an ISO 20022 `PstlAdr` that accountingd stores and emits.
///
/// ## The 15 November 2026 cut-over
///
/// Version 1.1 of the 2025 SEPA rulebooks, in force since 5 October 2025, ends
/// the unstructured address at **15 November 2026** (version 1.0 said the 22nd;
/// the date moved to land with that year's Swift Standards MX release). From
/// then a scheme message must carry `TwnNm` and `Ctry`, so both are required
/// here — [`PostalAddress::new`] takes them and the unstructured form is
/// unrepresentable. It is an address deadline, not a message-version one:
/// `pain.001.001.09` and `pain.008.001.08` have been mandatory since
/// 19 November 2023.
///
/// An address is optional until the cut-over. Absent parts emit no `PstlAdr` at
/// all; a **partially** filled address is an error rather than a silent
/// omission, because "we thought we sent the address" is exactly the failure the
/// cut-over will surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddressParts {
    /// `TwnNm` — mandatory from the cut-over.
    pub town: Option<String>,
    /// `Ctry` — ISO 3166-1 alpha-2, mandatory from the cut-over.
    pub country: Option<String>,
    /// `StrtNm`.
    pub street: Option<String>,
    /// `BldgNb`.
    pub building_number: Option<String>,
    /// `PstCd`.
    pub post_code: Option<String>,
    /// `CtrySubDvsn` (Bundesland / state).
    pub country_subdivision: Option<String>,
}

/// Why an [`AddressParts`] could not be turned into a `PstlAdr`.
#[derive(Debug, thiserror::Error)]
pub enum AddressPartsError {
    /// Street, post code or building number was given without both `TwnNm` and
    /// `Ctry` — the two the EPC schemes require from 15 November 2026.
    #[error(
        "incomplete postal address: town and country are both required \
         (EPC structured address, mandatory from 2026-11-15), got town={town:?} country={country:?}"
    )]
    Incomplete {
        /// The `TwnNm` that was supplied, if any.
        town: Option<String>,
        /// The `Ctry` that was supplied, if any.
        country: Option<String>,
    },
    /// The crate refused a part (unknown country, over-long element, …).
    #[error("invalid postal address: {0}")]
    Invalid(#[from] AddressError),
}

impl AddressParts {
    /// `true` when no part is set — no `PstlAdr` is emitted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.town.is_none()
            && self.country.is_none()
            && self.street.is_none()
            && self.building_number.is_none()
            && self.post_code.is_none()
            && self.country_subdivision.is_none()
    }

    /// Build the typed [`PostalAddress`], or `Ok(None)` when nothing is stored.
    ///
    /// # Errors
    ///
    /// [`AddressPartsError::Incomplete`] when parts are present without both
    /// town and country; [`AddressPartsError::Invalid`] when the crate rejects a
    /// part (an unknown `Ctry`, an over-long element, an illegal character).
    pub fn to_postal_address(&self) -> Result<Option<PostalAddress>, AddressPartsError> {
        if self.is_empty() {
            return Ok(None);
        }
        let (Some(town), Some(country)) = (self.town.as_deref(), self.country.as_deref()) else {
            return Err(AddressPartsError::Incomplete {
                town: self.town.clone(),
                country: self.country.clone(),
            });
        };
        let mut addr = PostalAddress::new(town, country)?;
        if let Some(v) = &self.street {
            addr = addr.street(v.clone());
        }
        if let Some(v) = &self.building_number {
            addr = addr.building_number(v.clone());
        }
        if let Some(v) = &self.post_code {
            addr = addr.post_code(v.clone());
        }
        if let Some(v) = &self.country_subdivision {
            addr = addr.country_subdivision(v.clone());
        }
        Ok(Some(addr))
    }
}

/// Resolve an address for a message whose schema may not carry one.
///
/// The two legacy DK schemas (`pain.008.003.02`, `pain.001.003.03`) have no
/// structured address type — their `PostalAddressSEPA` holds only `Ctry` and two
/// `AdrLine`s — so the crate refuses with `UnsupportedBySchema` rather than
/// emitting something its own XSD rejects. Dropping the address is the right
/// answer there: the operator pinned that schema deliberately, and an address
/// the wire format cannot carry must not block a collection run. It is logged,
/// because it stops being acceptable on 15 November 2026.
fn address_for_schema(
    parts: Option<&AddressParts>,
    supported: bool,
    party: &str,
) -> anyhow::Result<Option<PostalAddress>> {
    let Some(parts) = parts.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let address = parts
        .to_postal_address()
        .map_err(|e| anyhow::anyhow!("{party} address: {e}"))?;
    if address.is_some() && !supported {
        tracing::warn!(
            party,
            "accountingd: the pinned SEPA schema has no structured address type — \
             PstlAdr omitted. The EPC schemes require a structured address from \
             2026-11-15; move to the current schema version before then."
        );
        return Ok(None);
    }
    Ok(address)
}

// ── Party identities ──────────────────────────────────────────────────────────

/// The creditor side of a pain.008 collection (and of a pain.007 reversal).
///
/// Bundled rather than passed as four positional arguments: every field is
/// operator configuration read from the same `[creditor_*]` keys, and the group
/// is built once per `SequenceType`.
#[derive(Debug, Clone, Copy)]
pub struct CreditorIdentity<'a> {
    /// IBAN of the creditor's (LF's) own bank account.
    pub iban: &'a str,
    /// Legal name, emitted as `Cdtr/Nm` (transliterated).
    pub name: &'a str,
    /// SEPA Creditor Identifier (EPC AT-02) — mandatory, `CdtrSchmeId`.
    pub creditor_id: &'a str,
    /// `Cdtr/PstlAdr`. `None` until the operator configures it.
    pub address: Option<&'a AddressParts>,
}

/// The debtor side of a pain.001 credit transfer — the LF's own account.
#[derive(Debug, Clone, Copy)]
pub struct DebtorIdentity<'a> {
    /// IBAN of the debtor's (LF's) own bank account.
    pub iban: &'a str,
    /// Legal name, emitted as `Dbtr/Nm` (transliterated).
    pub name: &'a str,
    /// `Dbtr/PstlAdr`. `None` until the operator configures it.
    pub address: Option<&'a AddressParts>,
}

/// One creditor line of a pain.001 batch.
#[derive(Debug, Clone, Copy)]
pub struct CreditTransferItem<'a> {
    /// Beneficiary IBAN.
    pub iban: &'a str,
    /// Beneficiary name (`Cdtr/Nm`).
    pub name: &'a str,
    /// Amount in ct — must be positive.
    pub amount_ct: i64,
    /// `EndToEndId` — the reference the bank echoes back in pain.002 and camt.
    pub end_to_end_ref: &'a str,
    /// `Cdtr/PstlAdr` for this beneficiary.
    pub address: Option<&'a AddressParts>,
}

// ── Schema-version resolution (config → typed enum) ───────────────────────────

/// Resolve a configured pain.008 schema string (e.g. `"pain.008.001.02"`) to the
/// typed [`DirectDebitSchema`]. `None` → the current default. An unknown value is
/// a hard error so a bank-incompatible version fails loudly at startup, not on a
/// rejected batch.
///
/// # Errors
///
/// When `cfg` is neither a known message identifier nor a known namespace URN.
pub fn resolve_pain008_schema(cfg: Option<&str>) -> anyhow::Result<DirectDebitSchema> {
    match cfg {
        Some(s) => s
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid pain008_schema '{s}': {e}")),
        None => Ok(DirectDebitSchema::default()),
    }
}

/// Resolve a configured pain.001 schema string (e.g. `"pain.001.001.03"`) to the
/// typed [`CreditTransferSchema`]. `None` → the current default; unknown → error.
///
/// # Errors
///
/// When `cfg` is neither a known message identifier nor a known namespace URN.
pub fn resolve_pain001_schema(cfg: Option<&str>) -> anyhow::Result<CreditTransferSchema> {
    match cfg {
        Some(s) => s
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid pain001_schema '{s}': {e}")),
        None => Ok(CreditTransferSchema::default()),
    }
}

// ── pain.008 run output ───────────────────────────────────────────────────────

/// One pain.008 message covering a collection date — a single file with one
/// `PmtInf` group per `SequenceType` present in the input.
///
/// The SEPA SDD Rulebook requires `FRST` and `RCUR` collections in separate
/// payment-information blocks; these blocks live in **one message**, so a
/// collection run is one bank submission and one audit row in
/// `sepa_collection_runs`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain008Run {
    /// Generated pain.008 XML (validated before serialisation).
    pub xml: String,
    /// `GrpHdr/MsgId` — the key the bank's pain.002 reply quotes back.
    pub msg_id: String,
    /// Total amount in ct across all groups.
    pub total_ct: i64,
    /// Number of mandate entries across all groups.
    pub entry_count: usize,
    /// Per-`PmtInf` breakdown, in emission order.
    pub groups: Vec<Pain008GroupInfo>,
    /// Per-entry breakdown, in emission order — persisted to
    /// `sepa_collection_entries` so a booked camt entry, a pain.002 rejection
    /// and a pain.007 reversal can all be matched back to what was collected.
    #[serde(skip_serializing)]
    pub entries: Vec<Pain008EntryInfo>,
}

/// Summary of one `PmtInf` block inside a [`Pain008Run`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain008GroupInfo {
    /// Scheme of the block: `"CORE"` or `"B2B"`. One `PmtInf` carries exactly
    /// one, because the two are different rulebooks with different refund rights.
    pub scheme: String,
    /// SEPA SequenceType of the block (`"FRST"`, `"RCUR"`, `"FNAL"`, `"OOFF"`).
    pub sequence_type: String,
    /// `PmtInfId` of the block — the key the bank echoes in `NtryDtls/Btch`.
    pub payment_info_id: String,
    /// Mandate entries in this block.
    pub entry_count: usize,
    /// Total amount in ct in this block.
    pub total_ct: i64,
}

/// One collected mandate inside a [`Pain008Run`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain008EntryInfo {
    /// The mandate this entry collects against.
    pub mandate_id: uuid::Uuid,
    /// Mandatsreferenz (AT-01), also the `EndToEndId`.
    pub mandatsref: String,
    /// `PmtInfId` of the group this entry sits in.
    pub payment_info_id: String,
    /// SequenceType of the group.
    pub sequence_type: String,
    /// Amount collected, in ct.
    pub amount_ct: i64,
}

// ── pain.008 Direct Debit builder ─────────────────────────────────────────────

/// Build one pain.008 message from `accountingd`'s active mandate rows.
///
/// ## FRST/RCUR separation (SEPA Rulebook §3.8)
///
/// Entries are grouped by `SequenceType` into separate `PmtInf` blocks of the
/// **same message**, emitted in the fixed order FRST, RCUR, FNAL, OOFF with
/// `PmtInfId = <MsgId>-<SEQ>`. The crate rejects a duplicate `PmtInfId` across
/// groups (it is the key a bank echoes in `pain.002` and in `NtryDtls/Btch`), so
/// the suffix is load-bearing rather than cosmetic.
///
/// ## Gläubiger-ID (EPC AT-02, mandatory)
///
/// `creditor.creditor_id` is required: the EPC rulebook mandates `CdtrSchmeId`,
/// and the sepa crate refuses to build a group without it. Obtain the identifier
/// from the Bundesbank creditor registry.
///
/// ## Parameters
///
/// - `creditor`        — the LF's SEPA creditor identity (IBAN, name, CI, address)
/// - `collection_date` — requested collection date (`ReqdColltnDt`)
/// - `entries`         — slice of `(mandate_row, amount_ct)` pairs. The purpose
///   code comes from the mandate's account Sparte, carried on the row.
/// - `schema`          — pain.008 XML schema version to emit (a bank's required
///   version, e.g. `pain.008.001.02`); use `Default::default()` for the current one
///
/// # Errors
///
/// When the creditor IBAN or Creditor Identifier is invalid, when a configured
/// address is incomplete, when no billable mandate survives validation, or when
/// the crate refuses to build the message.
pub fn build_pain_008(
    creditor: &CreditorIdentity<'_>,
    collection_date: time::Date,
    entries: &[(&SepaMandateRow, i64)],
    schema: DirectDebitSchema,
) -> anyhow::Result<Pain008Run> {
    let creditor_iban = validate_iban(creditor.iban).map_err(|e| {
        anyhow::anyhow!(
            "creditor IBAN '{}' is invalid: {e}. \
             Set a valid SEPA IBAN in [creditor_iban] config. \
             pain.008 generation is blocked until this is corrected.",
            creditor.iban
        )
    })?;
    let creditor_id = validate_creditor_id(creditor.creditor_id).map_err(|e| {
        anyhow::anyhow!(
            "creditor_id '{}' is invalid: {e}. \
             Set the SEPA Creditor Identifier (Bundesbank registry) in \
             [creditor_id] config — the EPC rulebook mandates CdtrSchmeId.",
            creditor.creditor_id
        )
    })?;
    let creditor_address = address_for_schema(
        creditor.address,
        schema.supports_postal_address(),
        "creditor",
    )?;

    let today = time::OffsetDateTime::now_utc();
    let msg_id = format!(
        "DD-{}-{:02}-{:02}",
        collection_date.year(),
        collection_date.month() as u8,
        collection_date.day()
    );
    // Dates are typed (`IsoDate`), validated once at construction, so a malformed
    // date is unrepresentable in the batch rather than a `build()` error.
    let collection_iso = IsoDate::try_from(collection_date).map_err(|e| {
        anyhow::anyhow!("collection date {collection_date} is not a valid ISO date: {e}")
    })?;

    // Fixed emission order — deterministic files for golden tests and audits.
    const SEQ_ORDER: [(&str, SequenceType); 4] = [
        ("FRST", SequenceType::Frst),
        ("RCUR", SequenceType::Rcur),
        ("FNAL", SequenceType::Fnal),
        ("OOFF", SequenceType::Ooff),
    ];

    let mut builder = Pain008Builder::new(creditor.name)
        .msg_id(msg_id.clone())
        .schema(schema);
    let mut groups_info = Vec::new();
    let mut entries_info = Vec::new();
    let mut total_ct = 0i64;
    let mut entry_count = 0usize;

    // One `PmtInf` per (scheme, SequenceType). Both axes are needed, and for the
    // same reason: the DK subset makes `SvcLvl`/`LclInstrm`/`SeqTp` an
    // all-or-nothing block, so a group carries exactly one scheme and one
    // sequence type. CORE and B2B are also different rulebooks — a CORE debtor
    // has an unconditional 8-week refund right and a B2B debtor has none — so
    // mixing them in one group would not just be schema-invalid, it would
    // misstate what the debtor agreed to.
    for (scheme_key, scheme) in [
        ("CORE", DirectDebitScheme::Core),
        ("B2B", DirectDebitScheme::B2b),
    ] {
        for (seq_key, seq_type) in SEQ_ORDER {
            let group_entries: Vec<&(&SepaMandateRow, i64)> = entries
                .iter()
                .filter(|(m, _)| {
                    normalise_sequence_type(&m.sequence_type) == seq_key
                        && m.scheme.eq_ignore_ascii_case(scheme_key)
                })
                .collect();
            if group_entries.is_empty() {
                continue;
            }

            let payment_info_id = format!("{msg_id}-{scheme_key}-{seq_key}");
            // The group borrows the Creditor Identifier (`&CreditorId`), so the same
            // creditor identity is not cloned once per group.
            let mut group = DirectDebitGroup::new(creditor.name, &creditor_iban, &creditor_id)
                .scheme(scheme)
                .sequence_type(seq_type)
                .collection_date(collection_iso)
                .payment_info_id(payment_info_id.clone());
            if let Some(address) = creditor_address.clone() {
                group = group.creditor_address(address);
            }

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

                let signed_iso = match IsoDate::try_from(mandate.signed_at) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(
                            mandate_id = %mandate.mandate_id,
                            error = %e,
                            "accountingd: skipping mandate with invalid signature date in pain.008"
                        );
                        continue;
                    }
                };
                let description = format!("Abschlag {}-{:02}", today.year(), today.month() as u8);
                let mut entry = DirectDebitEntry::new(
                    mandate.mandatsref.clone(),
                    signed_iso,
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
                    && let Ok(bic) = validate_bic(bic_str)
                {
                    entry = entry.with_bic(bic);
                }

                // `Purp/Cd` — what the collection is for, as the debtor's statement
                // and their accounting software read it. Transaction level only;
                // informational, instructing no bank.
                if let Some(purpose) = mandate.sparte.as_deref().and_then(purpose_for_sparte) {
                    entry = entry.with_purpose(purpose);
                }

                // `Dbtr/PstlAdr` — the debtor's own address, stored per mandate.
                match address_for_schema(
                    Some(&mandate.debtor_address()),
                    schema.supports_postal_address(),
                    "debtor",
                ) {
                    Ok(Some(address)) => entry = entry.with_debtor_address(address),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            mandate_id = %mandate.mandate_id,
                            error = %e,
                            "accountingd: skipping mandate with an unusable debtor address in pain.008"
                        );
                        continue;
                    }
                }

                group_ct += *amount_ct;
                group_n += 1;
                entries_info.push(Pain008EntryInfo {
                    mandate_id: mandate.mandate_id,
                    mandatsref: mandate.mandatsref.clone(),
                    payment_info_id: payment_info_id.clone(),
                    sequence_type: seq_key.to_owned(),
                    amount_ct: *amount_ct,
                });
                group = group.add_entry(entry);
            }

            if group_n == 0 {
                continue; // every mandate in this sequence had an invalid IBAN
            }
            total_ct += group_ct;
            entry_count += group_n;
            groups_info.push(Pain008GroupInfo {
                scheme: scheme_key.to_owned(),
                sequence_type: seq_key.to_owned(),
                payment_info_id,
                entry_count: group_n,
                total_ct: group_ct,
            });
            builder = builder.add_group(group);
        }
    }

    if entry_count == 0 {
        anyhow::bail!("pain.008 run has no billable mandates (all entries invalid or empty)");
    }

    let xml = builder
        .build()
        .map_err(|e| anyhow::anyhow!("pain.008 validation failed: {e}"))?;

    Ok(Pain008Run {
        xml,
        msg_id,
        total_ct,
        entry_count,
        groups: groups_info,
        entries: entries_info,
    })
}

/// The ISO 20022 `Purp/Cd` for a BO4E Sparte.
///
/// `ExternalPurpose1Code` has a code for each utility bill, and it is what the
/// debtor's statement and their accounting software read to categorise a
/// collection. It is **informational** — placed at transaction level, passed to
/// the counterparty, instructing no bank — which is exactly why omitting it
/// costs nothing technically and everything in legibility: an energy supplier's
/// Lastschrift with no purpose is indistinguishable from any other on the
/// statement.
///
/// `None` for a Sparte with no single code. `STROM_UND_GAS` is the case that
/// matters: a combined supply is two purposes, and picking either would tell
/// the debtor's software something false. `WASSER`/`ABWASSER` both map to
/// `WTER` — ISO has no separate waste-water code.
#[must_use]
pub fn purpose_for_sparte(sparte: &str) -> Option<Purpose> {
    match sparte {
        "STROM" => Some(Purpose::Elec),
        "GAS" => Some(Purpose::Gasb),
        // Heat is billed as an energy supply; ISO has no district-heating code,
        // so the generic energy-industry code is the honest choice.
        "FERNWAERME" | "NAHWAERME" => Some(Purpose::Other("ENRG".to_owned())),
        "WASSER" | "ABWASSER" => Some(Purpose::Wter),
        _ => None,
    }
}

/// Map a stored `sequence_type` onto the four SEPA codes.
///
/// Anything unrecognised collects as `RCUR`: a mandate that has already been
/// used is recurring, and the DB CHECK constraint keeps real values in range.
#[must_use]
pub fn normalise_sequence_type(stored: &str) -> &'static str {
    match stored {
        "FRST" => "FRST",
        "FNAL" => "FNAL",
        "OOFF" => "OOFF",
        _ => "RCUR",
    }
}

/// The typed [`DirectDebitScheme`] for a stored `scheme` string.
///
/// Unknown values fall back to `CORE`, the consumer scheme: it is the one with
/// the *stronger* debtor protection, so mis-reading a scheme errs toward giving
/// the debtor a refund right rather than removing one.
#[must_use]
pub fn scheme_of(stored: &str) -> DirectDebitScheme {
    if stored.eq_ignore_ascii_case("B2B") {
        DirectDebitScheme::B2b
    } else {
        DirectDebitScheme::Core
    }
}

/// The typed [`SequenceType`] for a stored `sequence_type` string.
#[must_use]
pub fn sequence_type_of(stored: &str) -> SequenceType {
    match normalise_sequence_type(stored) {
        "FRST" => SequenceType::Frst,
        "FNAL" => SequenceType::Fnal,
        "OOFF" => SequenceType::Ooff,
        _ => SequenceType::Rcur,
    }
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
/// Pass `instant = true` to emit `LclInstrm = INST`. The message stays on the
/// configured schema; `pain.001.003.03` has no `LclInstrm` element at all, so
/// the crate rejects that combination rather than writing an element the XSD
/// forbids. Debtor agents without a known BIC use the EPC "IBAN only" form —
/// `NOTPROVIDED` is never written as a BIC.
///
/// ## Execution date
///
/// `execution_date` is always set explicitly. The crate's own default became
/// "today" in 0.6 (it had been an unexplained five days out, borrowed from
/// pain.008's pre-notification floor), and a payment date is not something to
/// inherit from a library default.
///
/// # Errors
///
/// When an IBAN or a configured address is invalid, or the crate refuses to
/// build the message.
pub fn build_pain_001(
    debtor: &DebtorIdentity<'_>,
    entries: &[CreditTransferItem<'_>],
    execution_date: time::Date,
    instant: bool,
    schema: CreditTransferSchema,
) -> anyhow::Result<String> {
    let debtor_iban = validate_iban(debtor.iban)
        .map_err(|e| anyhow::anyhow!("debtor IBAN '{}' invalid: {e}", debtor.iban))?;
    let supports_address = schema.supports_postal_address();
    let debtor_address = address_for_schema(debtor.address, supports_address, "debtor")?;

    let today = time::OffsetDateTime::now_utc();
    let msg_id = format!(
        "CT-{}-{:02}-{:02}",
        today.year(),
        today.month() as u8,
        today.day()
    );
    let execution_iso = IsoDate::try_from(execution_date).map_err(|e| {
        anyhow::anyhow!("execution date {execution_date} is not a valid ISO date: {e}")
    })?;

    let mut group =
        CreditTransferGroup::new(debtor.name, &debtor_iban).execution_date(execution_iso);
    if instant {
        group = group.local_instrument(LocalInstrument::Inst);
    }
    if let Some(address) = debtor_address {
        group = group.debtor_address(address);
    }
    for item in entries {
        let creditor_iban = validate_iban(item.iban)
            .map_err(|e| anyhow::anyhow!("creditor IBAN '{}' invalid: {e}", item.iban))?;
        let mut entry = CreditTransferEntry::new(
            item.name.to_owned(),
            creditor_iban,
            item.amount_ct,
            item.end_to_end_ref.to_owned(),
        );
        if let Some(address) = address_for_schema(item.address, supports_address, "creditor")? {
            entry = entry.with_creditor_address(address);
        }
        group = group.add_entry(entry);
    }

    Pain001Builder::new(debtor.name)
        .msg_id(msg_id)
        .schema(schema)
        .add_group(group)
        .build()
        .map_err(|e| anyhow::anyhow!("pain.001 validation failed: {e}"))
}

// ── pain.007 SEPA Direct Debit reversal ──────────────────────────────────────

/// One collection being reversed, as read back out of `sepa_collection_entries`.
#[derive(Debug, Clone)]
pub struct ReversalRequest<'a> {
    /// `GrpHdr/MsgId` of the pain.008 that carried the collection.
    pub original_msg_id: &'a str,
    /// `PmtInfId` of the group the collection sat in.
    pub original_payment_info_id: &'a str,
    /// `EndToEndId` of the collection (accountingd uses the Mandatsreferenz).
    pub original_end_to_end_id: &'a str,
    /// Amount originally collected, in ct.
    pub original_amount_ct: i64,
    /// Amount to give back — `None` reverses the whole collection. A partial
    /// reversal may not exceed what was collected; the crate enforces that.
    pub reversed_amount_ct: Option<i64>,
    /// `RvslRsnInf/Rsn/Cd`.
    pub reason: ReversalReason,
    /// Mandatsreferenz (AT-01).
    pub mandate_ref: &'a str,
    /// Date the mandate was signed.
    pub mandate_signed_at: time::Date,
    /// Requested collection date of the original.
    pub collection_date: time::Date,
    /// SEPA sequence type the original collection carried.
    pub sequence_type: &'a str,
    /// The scheme the **original** collection went out under, `CORE` or `B2B`.
    ///
    /// A reversal restates the original mandate exactly as submitted (the DK
    /// subset makes `OrgnlTxRef` mandatory), so it must repeat the original's
    /// scheme — not a default. Hard-coding CORE here restated a B2B collection
    /// as a CORE one, which is a different agreement with a different refund
    /// right.
    pub scheme: &'a str,
    /// Debtor name as it was sent.
    pub debtor_name: &'a str,
    /// Debtor IBAN as it was sent.
    pub debtor_iban: &'a str,
    /// Debtor agent BIC, when the mandate records one.
    pub debtor_bic: Option<&'a str>,
}

/// A generated pain.007 reversal message.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pain007Reversal {
    /// Generated pain.007.001.09 XML (validated before serialisation).
    pub xml: String,
    /// `GrpHdr/MsgId` of the reversal message.
    pub msg_id: String,
    /// `PmtInfId` of the reversal group.
    pub payment_info_id: String,
    /// Number of reversed entries.
    pub entry_count: usize,
    /// Total reversed amount in ct.
    pub total_ct: i64,
}

/// Build a pain.007 reversing settled direct-debit collections.
///
/// A reversal is the creditor sending a settled collection back — the
/// counterpart to a debtor-initiated refund (which arrives as camt.054) and to a
/// reject (which arrives as pain.002). In accountingd it is what an operator
/// runs after collecting an Abschlag twice, or collecting after the customer had
/// already paid by transfer.
///
/// `OrgnlTxRef` is **required**: plain ISO permits a reversal carrying only
/// references, but the Deutsche Kreditwirtschaft's technical validation subset
/// makes the original transaction reference and the mandate inside it mandatory,
/// so the references-only form is not one a German bank accepts. Every field it
/// needs comes out of `sepa_collection_entries` and `sepa_mandates` rather than
/// from operator input, so a reversal cannot disagree with what was collected.
///
/// # Errors
///
/// When an identifier is invalid, when the reversals span more than one original
/// `PmtInfId`, when a reversal exceeds what was collected, or when the crate
/// refuses to build the message.
pub fn build_pain_007(
    creditor: &CreditorIdentity<'_>,
    reversals: &[ReversalRequest<'_>],
    original_schema: DirectDebitSchema,
) -> anyhow::Result<Pain007Reversal> {
    let Some(first) = reversals.first() else {
        anyhow::bail!("pain.007 needs at least one reversal");
    };
    // One `PmtInf` group per original group: `OrgnlPmtInfId` identifies exactly
    // one submitted block, so entries from two blocks cannot share a group.
    if let Some(other) = reversals
        .iter()
        .find(|r| r.original_payment_info_id != first.original_payment_info_id)
    {
        anyhow::bail!(
            "pain.007 reversals must all belong to one original PmtInfId — \
             got '{}' and '{}'",
            first.original_payment_info_id,
            other.original_payment_info_id
        );
    }

    let creditor_iban = validate_iban(creditor.iban)
        .map_err(|e| anyhow::anyhow!("creditor IBAN '{}' invalid: {e}", creditor.iban))?;
    let creditor_id = validate_creditor_id(creditor.creditor_id)
        .map_err(|e| anyhow::anyhow!("creditor_id '{}' invalid: {e}", creditor.creditor_id))?;

    // `MsgId` and `RvslPmtInfId` are both Max35Text, and the second is derived
    // from the first — so the identifier has to leave room for its own suffix.
    // 24 hex characters of a v4 UUID is 96 bits, ample for a message id that
    // only has to be unique per creditor.
    let unique = uuid::Uuid::new_v4().simple().to_string();
    let msg_id = format!("RV-{}", &unique[..24]);
    let payment_info_id = format!("{msg_id}-01");
    let mut group = ReversalGroup::new(first.original_payment_info_id)
        .reversal_payment_info_id(payment_info_id.clone());
    let mut total_ct = 0i64;

    for r in reversals {
        let debtor_iban = validate_iban(r.debtor_iban)
            .map_err(|e| anyhow::anyhow!("debtor IBAN '{}' invalid: {e}", r.debtor_iban))?;
        let signed_iso = IsoDate::try_from(r.mandate_signed_at).map_err(|e| {
            anyhow::anyhow!(
                "mandate signature date {} invalid: {e}",
                r.mandate_signed_at
            )
        })?;
        let collection_iso = IsoDate::try_from(r.collection_date)
            .map_err(|e| anyhow::anyhow!("collection date {} invalid: {e}", r.collection_date))?;

        let mut original = OriginalCollection::new(r.mandate_ref, signed_iso)
            .collection_date(collection_iso)
            .creditor_id(creditor_id.clone())
            // The DK subset makes `SvcLvl`/`LclInstrm`/`SeqTp` an all-or-nothing
            // block, so a half-filled one is unconstructible rather than
            // schema-invalid — and the pair must agree with the original
            // collection, scheme included.
            .payment_type(scheme_of(r.scheme), sequence_type_of(r.sequence_type))
            .debtor(r.debtor_name, debtor_iban)
            .creditor(creditor.name, creditor_iban.clone());
        if let Some(bic_str) = r.debtor_bic
            && let Ok(bic) = validate_bic(bic_str)
        {
            original = original.debtor_agent(bic);
        }

        let mut entry = ReversalEntry::new(
            r.original_end_to_end_id.to_owned(),
            r.original_amount_ct,
            r.reason.clone(),
            original,
        );
        if let Some(amount) = r.reversed_amount_ct {
            entry = entry.reversed_amount(amount);
        }
        total_ct += r.reversed_amount_ct.unwrap_or(r.original_amount_ct);
        group = group.add_entry(entry);
    }

    let builder = Pain007Builder::new(creditor.name, first.original_msg_id)
        .msg_id(msg_id.clone())
        .original_schema(original_schema)
        .add_group(group);

    let xml = builder
        .build()
        .map_err(|e| anyhow::anyhow!("pain.007 validation failed: {e}"))?;

    Ok(Pain007Reversal {
        xml,
        msg_id,
        payment_info_id,
        entry_count: reversals.len(),
        total_ct,
    })
}

// ── Flat bank-export import (formerly `sepa::camt054::parse_simple_json`) ─────

/// One row of a flat bank export — a CSV-turned-JSON or an ERP's payment feed.
///
/// The `sepa` crate removed `camt054::parse_simple_json` in 0.6, correctly: the
/// shape is not specified anywhere, so it does not belong in a module named
/// after an ISO 20022 message, and its `to_ledger_ct` carried the opposite sign
/// convention to `CashEntry::signed_ct` sitting beside it. The shape *is*
/// accountingd's own import contract, so it lives here — parsed with `serde`,
/// with amounts through [`ct_from_eur_str`] (integer ct, never `f64`) and dates
/// through [`IsoDate::parse`], and with the **one** sign convention
/// [`bank_to_ledger_ct`] applies for every bank file.
///
/// Prefer `POST /api/v1/payments/import/camt054` where the bank offers real
/// camt: it carries `EndToEndId`, the `Btch` block and return reason codes, none
/// of which survive a flattening.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BankStatementRow {
    /// The counterparty IBAN — the customer account the payment came from.
    pub iban: String,
    /// Amount in EUR as a decimal string (`"155.42"`). Positive is money
    /// arriving; negative is a return.
    pub amount_eur: String,
    /// Value date, ISO 8601 (`YYYY-MM-DD`).
    pub date: String,
    /// Verwendungszweck.
    #[serde(default)]
    pub reference: Option<String>,
    /// `EndToEndId`, when the export carries one.
    #[serde(default)]
    pub end_to_end_id: Option<String>,
    /// The bank's own transaction id — the deduplication key when present.
    #[serde(default)]
    pub bank_transaction_id: Option<String>,
    /// EPC return reason code (`MD06`, `AC04`, …) when the row is a Rückläufer.
    #[serde(default)]
    pub return_reason_code: Option<String>,
}

/// Why a [`BankStatementRow`] could not be used.
#[derive(Debug, thiserror::Error)]
pub enum BankRowError {
    /// The row did not deserialise into [`BankStatementRow`].
    #[error("malformed row: {0}")]
    Malformed(#[from] serde_json::Error),
    /// `iban` failed ISO 13616 validation.
    #[error("invalid IBAN: {0}")]
    Iban(#[from] IbanError),
    /// `amount_eur` was empty, malformed or out of range.
    #[error("invalid amount_eur: {0}")]
    Amount(#[from] sepa::AmountError),
    /// `date` was not an ISO 8601 calendar date.
    #[error("invalid date: {0}")]
    Date(#[from] sepa::DateError),
    /// The date is a valid ISO date the local calendar cannot represent.
    #[error("date {0} out of range")]
    DateRange(String),
}

/// A validated flat import row, in the camt sign convention.
#[derive(Debug, Clone)]
pub struct BankStatementEntry {
    /// The validated counterparty IBAN.
    pub iban: Iban,
    /// Signed amount in ct, **bank's point of view**: positive = credit to us.
    /// Convert with [`bank_to_ledger_ct`] before posting.
    pub signed_ct: i64,
    /// Value date.
    pub date: time::Date,
    /// Verwendungszweck, empty when the export carried none.
    pub reference: String,
    /// `EndToEndId`, when present.
    pub end_to_end_id: Option<String>,
    /// The bank's transaction id, when present.
    pub bank_transaction_id: Option<String>,
    /// EPC return reason code, when the row is a Rückläufer.
    pub return_reason_code: Option<String>,
}

impl BankStatementEntry {
    /// Parse and validate one flat export row.
    ///
    /// # Errors
    ///
    /// [`BankRowError`] naming the field and the reason — a skipped bank row is
    /// diagnosable rather than silently dropped.
    pub fn parse(raw: &serde_json::Value) -> Result<Self, BankRowError> {
        let row: BankStatementRow = serde_json::from_value(raw.clone())?;
        let iban = validate_iban(&row.iban)?;
        let signed_ct = ct_from_eur_str(&row.amount_eur)?;
        let iso = IsoDate::parse(&row.date)?;
        let date =
            time::Date::try_from(iso).map_err(|_| BankRowError::DateRange(row.date.clone()))?;
        Ok(Self {
            iban,
            signed_ct,
            date,
            reference: row.reference.unwrap_or_default(),
            end_to_end_id: row.end_to_end_id,
            bank_transaction_id: row.bank_transaction_id,
            return_reason_code: row.return_reason_code,
        })
    }

    /// Whether this row gives money back — an explicit EPC return reason code,
    /// or a debit against our account.
    ///
    /// The removed crate helper derived this from a `return_reason` the flat
    /// format never carried, so it was always `false`: a negative amount booked
    /// as an ordinary `ZAHLUNG` with a positive ledger effect.
    #[must_use]
    pub fn is_return(&self) -> bool {
        self.return_reason_code.is_some() || self.signed_ct < 0
    }

    /// The signed amount in accountingd's open-items convention.
    #[must_use]
    pub const fn ledger_ct(&self) -> i64 {
        bank_to_ledger_ct(self.signed_ct)
    }

    /// Statement description for the ledger entry.
    #[must_use]
    pub fn description(&self) -> String {
        match &self.return_reason_code {
            Some(code) => format!("Rückläufer ({code})"),
            None if self.signed_ct < 0 => "Rückläufer".to_owned(),
            None => "Zahlungseingang".to_owned(),
        }
    }
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
#[must_use]
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
