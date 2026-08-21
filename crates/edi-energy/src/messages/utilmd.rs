use edifact_rs::{
    EdifactDeserialize, EdifactSerialize, EventEmitter, OwnedSegment, ProfileRulePack,
    ValidationIssue, ValidationSeverity,
};

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{
            Bgm, Dtm, Ftx, Ide, Loc, Nad, Rff, Sts, collect_dtm, find_bgm, find_nad,
            try_deserialize,
        },
    },
};

// ── Segment group types ───────────────────────────────────────────────────────

/// A header-section reference group (UTILMD SG1: RFF + optional DTM).
///
/// Carries the Pruefidentifikator reference and similar header references.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UtilmdReference {
    /// RFF — reference qualifier and identifier.
    pub rff: Rff,
    /// DTM — validity date / version for this reference (optional).
    pub dtm: Vec<Dtm>,
}

/// A per-metering-point transaction group (UTILMD SG4: IDE + nested segments).
///
/// Each instance represents one grid-connection or metering-point process
/// (e.g. supplier switch, deregistration) within the message.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UtilmdTransaction {
    /// IDE — `24` Vorgang (or `Z01` Liste) plus the **Vorgangsnummer**.
    ///
    /// Not a location: read the Marktlokation from
    /// [`marktlokation()`](Self::marktlokation).
    pub ide: Ide,
    /// DTM — date/time segments scoped to this Vorgang.
    pub dtm: Vec<Dtm>,
    /// SG5 LOC — every Lokation this Vorgang names, in wire order.
    pub locations: Vec<Loc>,
    /// SG6 RFF — references related to this Vorgang (e.g. Vorgangsnummer der
    /// Anfragenachricht).
    pub references: Vec<Rff>,
    /// STS — `7` Transaktionsgrund and `E01` Status der Antwort, raw.
    pub sts: Vec<Sts>,
    /// FTX — free-text remarks scoped to this Vorgang.
    pub ftx: Vec<Ftx>,
    /// `STS+7` parsed across its repeated `C556` composites.
    transaktionsgrund: Option<crate::utilmd_codes::Transaktionsgrund>,
    /// `STS+E01` parsed with its DE 1131 EBD reference.
    antwort: Option<crate::utilmd_codes::AntwortStatus>,
}

impl UtilmdTransaction {
    /// The `IDE` DE 7402 Vorgangsnummer.
    #[must_use]
    pub fn vorgangsnummer(&self) -> Option<&str> {
        self.ide.object_id.as_deref()
    }

    /// The ID carried by the first `SG5 LOC` with the given DE 3227 qualifier.
    #[must_use]
    pub fn location(&self, lokationstyp: crate::Lokationstyp) -> Option<&str> {
        let want = lokationstyp.qualifier_code();
        self.locations
            .iter()
            .find(|l| l.qualifier == want)
            .and_then(|l| l.location_id.as_deref())
    }

    /// `SG5 LOC+Z16` — the Marktlokations-ID.
    #[must_use]
    pub fn marktlokation(&self) -> Option<&str> {
        self.location(crate::Lokationstyp::Marktlokation)
    }

    /// `SG5 LOC+Z17` — the Messlokations-ID.
    #[must_use]
    pub fn messlokation(&self) -> Option<&str> {
        self.location(crate::Lokationstyp::Messlokation)
    }

    /// The first `SG4 DTM` with the given DE 2005 qualifier.
    ///
    /// Use the [`dtm`](crate::utilmd_codes::dtm) constants: `92` Beginn zum,
    /// `93` Ende zum, `154` ÜT der Lieferanmeldung des LFN.
    #[must_use]
    pub fn date(&self, qualifier: &str) -> Option<&str> {
        self.dtm
            .iter()
            .find(|d| d.qualifier == qualifier)
            .and_then(|d| d.value.as_deref())
    }

    /// `SG4 STS+7` — the Transaktionsgrund with its Ergänzung.
    ///
    /// The Ergänzung (`ZW3` erzeugende / `ZW4` verbrauchende Marktlokation,
    /// `ZW5` Tranche, `ZAP` ruhende Marktlokation) selects the branch every
    /// answering EBD opens with, so it is returned alongside the Grund rather
    /// than left for the caller to dig out of the repeated composite.
    #[must_use]
    pub fn transaktionsgrund(&self) -> Option<crate::utilmd_codes::Transaktionsgrund> {
        self.transaktionsgrund.clone()
    }

    /// `SG4 STS+E01` — the EBD Antwortcode a Bestätigung or Ablehnung carries.
    ///
    /// `None` on an Anfrage, and on an answer that omits the segment the AHB
    /// marks Muss — which the receiving side needs to see rather than infer.
    #[must_use]
    pub fn antwort(&self) -> Option<&crate::utilmd_codes::AntwortStatus> {
        self.antwort.as_ref()
    }
}

// ── UtilmdMessage ─────────────────────────────────────────────────────────────

/// UTILMD — Utilities Master Data message.
///
/// Used in the German energy market for grid-connection processes such as
/// supplier switches, registrations, cancellations, and meter installations.
///
/// # Typed access
///
/// Commonly-used segment data is pre-extracted into public fields:
///
/// | Field      | Segment | DE / meaning                               |
/// |------------|---------|---------------------------------------------|
/// | `bgm`          | BGM     | Document code and Pruefidentifikator        |
/// | `dtm`          | DTM+137 | Message date/time (+ other DTM variants)    |
/// | `sender`       | NAD+MS  | Message sender (party ID, name)             |
/// | `receiver`     | NAD+MR  | Message recipient (party ID, name)          |
/// | `references`   | SG1/RFF | Header references (Pruefidentifikator, etc.)|
/// | `transactions` | SG4/IDE | Per-metering-point transaction groups       |
///
/// The raw [`OwnedSegment`] list is available via [`segments()`][Self::segments]
/// for any segment not covered by the typed fields.
///
/// # Multiple format versions
///
/// A single `UtilmdMessage` type covers **all registered UTILMD release
/// versions** (e.g. `5.5.3a`, `5.5.4a`).  Version dispatch works as follows:
///
/// 1. The EDI@Energy release string (EDIFACT UNH element 1, component 4 —
///    "association assigned code") is stored verbatim in `self.assoc_code()`.
/// 2. [`validate()`][crate::EdiEnergyMessage::validate] calls
///    [`detect_release()`][crate::EdiEnergyMessage::detect_release], which maps
///    `assoc_code` to a [`Release`][crate::Release] and looks it up in the
///    global [`ReleaseRegistry`][crate::registry::ReleaseRegistry].
/// 3. Validation runs against the profile registered for **that specific
///    release**.  Two messages with different release codes are each validated
///    against their own profile — there is no cross-version fallback.
/// 4. Typed field extraction is version-agnostic: EDIFACT segment structure is
///    backward-compatible within a UTILMD track, so `bgm`, `dtm`, `sender`,
///    `receiver`, `references`, and `transactions` are populated regardless of
///    release version.
///
/// To pin validation to a specific profile regardless of the message's own
/// release code, use
/// [`validate_against(release)`][crate::EdiEnergyMessage::validate_against].
#[derive(Debug, Clone)]
pub struct UtilmdMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message.  Always present in a valid UTILMD.
    bgm: Option<Bgm>,
    /// DTM — message-level date/time segments.
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message recipient.
    receiver: Option<Nad>,
    /// SG1 — header references (Pruefidentifikator, MMMA, etc.).
    references: Vec<UtilmdReference>,
    /// SG4 — per-metering-point / per-process transaction groups.
    transactions: Vec<UtilmdTransaction>,
}

impl UtilmdMessage {
    /// Construct from already-parsed owned segments.
    ///
    /// Typed fields (`bgm`, `dtm`, `sender`, `receiver`) are pre-extracted
    /// from the segment list for convenient access.  If a segment is absent or
    /// malformed the corresponding field is `None` / empty — the raw segments
    /// are always authoritative for validation.
    pub(crate) fn from_parts(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
    ) -> Self {
        // Extract typed fields inside a scoped block so the borrow on `segments`
        // ends before it is moved into MessageCore.
        let (bgm, dtm, sender, receiver, references, transactions) = {
            let borrowed: Vec<edifact_rs::Segment<'_>> =
                segments.iter().map(|s| s.as_borrowed()).collect();
            (
                find_bgm(&borrowed),
                collect_dtm(&borrowed),
                find_nad(&borrowed, "MS"),
                find_nad(&borrowed, "MR"),
                parse_references(&borrowed),
                parse_transactions(&borrowed),
            )
        };
        Self {
            core: MessageCore::new(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
                MessageType::Utilmd,
            ),
            bgm,
            dtm,
            sender,
            receiver,
            references,
            transactions,
        }
    }

    /// The EDI@Energy release / association code from UNH (DE 0057), e.g. `"5.5.3a"`.
    #[must_use]
    pub fn assoc_code(&self) -> &str {
        &self.core.assoc_code
    }

    /// Raw parsed segments (authoritative for validation and serialization).
    #[must_use]
    pub fn segments(&self) -> &[OwnedSegment] {
        &self.core.segments
    }

    /// BGM — beginning of message.  Returns `None` when the segment was absent or malformed.
    #[must_use]
    pub fn bgm(&self) -> Option<&Bgm> {
        self.bgm.as_ref()
    }

    /// DTM — message-level date/time segments (before the first transaction group).
    #[must_use]
    pub fn dtm(&self) -> &[Dtm] {
        &self.dtm
    }

    /// NAD+MS — message sender.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn sender(&self) -> Option<&Nad> {
        self.sender.as_ref()
    }

    /// NAD+MR — message recipient.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn receiver(&self) -> Option<&Nad> {
        self.receiver.as_ref()
    }

    /// SG1 — header references (Pruefidentifikator, MMMA, etc.).
    #[must_use]
    pub fn references(&self) -> &[UtilmdReference] {
        &self.references
    }

    /// SG4 — per-metering-point / per-process transaction groups.
    #[must_use]
    pub fn transactions(&self) -> &[UtilmdTransaction] {
        &self.transactions
    }
}

// ── EdifactDeserialize ────────────────────────────────────────────────────────

impl EdifactDeserialize for UtilmdMessage {
    fn edifact_deserialize(
        segments: &[edifact_rs::Segment<'_>],
    ) -> Result<Self, edifact_rs::EdifactError> {
        let (message_ref, assoc_code) = MessageCore::extract_unh_fields(segments)?;
        let pid = MessageCore::extract_bgm_pid(segments);
        let owned: Vec<OwnedSegment> = segments.iter().cloned().map(OwnedSegment::from).collect();
        Ok(Self::from_parts(owned, message_ref, assoc_code, pid))
    }
}

// ── EdifactSerialize ──────────────────────────────────────────────────────────

impl EdifactSerialize for UtilmdMessage {
    fn edifact_serialize<E: EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), edifact_rs::EdifactError> {
        self.core.emit_segments(emitter)
    }
}

impl_edi_energy_message!(UtilmdMessage, sem = utilmd_semantic_pack());

// ── segment group parsers ─────────────────────────────────────────────────────

/// Parse SG1 reference groups (RFF + optional DTM) from the header section.
fn parse_references(segments: &[edifact_rs::Segment<'_>]) -> Vec<UtilmdReference> {
    // Header references appear before the first IDE segment.
    let end = segments
        .iter()
        .position(|s| s.tag == "IDE")
        .unwrap_or(segments.len());
    let header = &segments[..end];

    let mut result = Vec::new();
    let mut i = 0;
    while i < header.len() {
        if header[i].tag != "RFF" {
            i += 1;
            continue;
        }
        let Some(rff) = try_deserialize::<Rff>(&header[i]) else {
            i += 1;
            continue;
        };
        let mut dtm = Vec::new();
        let mut j = i + 1;
        while j < header.len() && header[j].tag == "DTM" {
            if let Some(d) = try_deserialize::<Dtm>(&header[j]) {
                dtm.push(d);
            }
            j += 1;
        }
        result.push(UtilmdReference { rff, dtm });
        i = j;
    }
    result
}

/// Parse SG4 transaction groups (IDE + nested DTM/LOC/RFF) from the message.
///
/// Each `IDE` starts a new [`UtilmdTransaction`].
fn parse_transactions(segments: &[edifact_rs::Segment<'_>]) -> Vec<UtilmdTransaction> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < segments.len() {
        if segments[i].tag != "IDE" {
            i += 1;
            continue;
        }
        let Some(ide) = try_deserialize::<Ide>(&segments[i]) else {
            i += 1;
            continue;
        };

        let mut dtm = Vec::new();
        let mut locations = Vec::new();
        let mut references = Vec::new();
        let mut sts = Vec::new();
        let mut ftx = Vec::new();
        let mut transaktionsgrund = None;
        let mut antwort = None;
        let mut j = i + 1;

        while j < segments.len() && segments[j].tag != "IDE" && segments[j].tag != "UNT" {
            match segments[j].tag {
                "DTM" => {
                    if let Some(d) = try_deserialize::<Dtm>(&segments[j]) {
                        dtm.push(d);
                    }
                }
                "LOC" => {
                    if let Some(l) = try_deserialize::<Loc>(&segments[j]) {
                        locations.push(l);
                    }
                }
                "RFF" => {
                    if let Some(r) = try_deserialize::<Rff>(&segments[j]) {
                        references.push(r);
                    }
                }
                "STS" => {
                    // The repeated `C556` composites are read positionally from
                    // the raw segment: DE 9013 resolves to the *first*
                    // occurrence only, so a code-addressed read cannot see the
                    // Ergänzung at element 4 or the DE 1131 EBD reference.
                    match sts_category(&segments[j]) {
                        Some(crate::utilmd_codes::STS_TRANSAKTIONSGRUND) => {
                            transaktionsgrund = parse_transaktionsgrund(&segments[j]);
                        }
                        Some(crate::utilmd_codes::STS_STATUS_ANTWORT) => {
                            antwort = parse_antwort(&segments[j]);
                        }
                        _ => {}
                    }
                    if let Some(s) = try_deserialize::<Sts>(&segments[j]) {
                        sts.push(s);
                    }
                }
                "FTX" => {
                    if let Some(f) = try_deserialize::<Ftx>(&segments[j]) {
                        ftx.push(f);
                    }
                }
                _ => {}
            }
            j += 1;
        }

        result.push(UtilmdTransaction {
            ide,
            dtm,
            locations,
            references,
            sts,
            ftx,
            transaktionsgrund,
            antwort,
        });
        i = j;
    }
    result
}

/// `STS` DE 9015 — the Statuskategorie, read from element 1 component 0.
fn sts_category<'a>(seg: &'a edifact_rs::Segment<'_>) -> Option<&'a str> {
    seg.get_element(0)
        .and_then(|e| e.get_component(0))
        .filter(|c| !c.is_empty())
}

/// `STS+7++<grund>+<ergaenzung>+<befristet>` — the three repeated `C556`
/// composites at elements 3, 4 and 5 (zero-based 2, 3, 4).
fn parse_transaktionsgrund(
    seg: &edifact_rs::Segment<'_>,
) -> Option<crate::utilmd_codes::Transaktionsgrund> {
    let at = |idx: usize| {
        seg.get_element(idx)
            .and_then(|e| e.get_component(0))
            .filter(|c| !c.is_empty())
            .map(ToOwned::to_owned)
    };
    Some(crate::utilmd_codes::Transaktionsgrund {
        grund: at(2)?,
        ergaenzung: at(3),
        befristet: at(4),
    })
}

/// `STS+E01++<code>:<ebd>` — the Antwortcode in DE 9013 and the EBD id in
/// DE 1131, both inside the single `C556` at element 3 (zero-based 2).
fn parse_antwort(seg: &edifact_rs::Segment<'_>) -> Option<crate::utilmd_codes::AntwortStatus> {
    let c556 = seg.get_element(2)?;
    let code = c556.get_component(0).filter(|c| !c.is_empty())?;
    Some(crate::utilmd_codes::AntwortStatus {
        code: code.to_owned(),
        ebd: c556
            .get_component(1)
            .filter(|c| !c.is_empty())
            .map(ToOwned::to_owned),
    })
}

// ── Layer 5: UTILMD semantic rule pack ───────────────────────────────────────

/// Build the UTILMD semantic rule pack (Layer 5).
///
/// These rules check business-level constraints that are not expressible in
/// the structural MIG/AHB schemas:
/// - [`rule_sem_lokations_id_format`]: `SG5 LOC` location IDs must match the
///   BDEW scheme for the Lokationstyp the qualifier names.
fn utilmd_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("UTILMD-SEM")
        .for_message_type("UTILMD")
        .with_stateless_rule_fn(rule_sem_lokations_id_format)
}

/// `SEM-UTILMD-LOKATIONS-ID` — validate `SG5 LOC` location identifiers.
///
/// The Lokations-ID lives in `SG5 LOC` DE 3225, keyed by the DE 3227
/// Lokationstyp — **not** in `IDE`, whose DE 7402 carries the sender's own
/// Vorgangsnummer and is deliberately free-form (`an..35`).
///
/// Only the qualifiers whose ID scheme the BDEW fixes are checked:
/// `Z16`/`Z22` Marktlokation (11 chars) and `Z17` Messlokation (33 chars). A
/// Netzlokation, Tranche or Ressourcen-ID has its own scheme and is left alone.
fn rule_sem_lokations_id_format(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    use crate::utilmd_codes::loc;
    for seg in segments.iter().filter(|s| s.tag == "LOC") {
        let qualifier = seg
            .get_element(0)
            .and_then(|e| e.get_component(0))
            .unwrap_or("");
        let expects_location_id = matches!(
            qualifier,
            q if q == loc::MARKTLOKATION
                || q == loc::RUHENDE_MARKTLOKATION
                || q == loc::MESSLOKATION
        );
        if !expects_location_id {
            continue;
        }
        let id = seg
            .get_element(1)
            .and_then(|e| e.get_component(0))
            .unwrap_or("");
        if id.is_empty() || super::common::is_valid_location_id(id) {
            continue;
        }
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "LOC+{qualifier} element 3225: value is neither a \
                     Marktlokations-ID ([A-Z0-9]{{11}}) nor a Messlokations-ID (33 characters)"
                ),
            )
            .with_span(seg.span)
            .with_rule_id("SEM-UTILMD-LOKATIONS-ID")
            .with_segment("LOC")
            .with_suggestion(
                "LOC+Z16 / LOC+Z22 carry an 11-character Marktlokations-ID matching \
                 [A-Z0-9]{11}; LOC+Z17 carries a 33-character Messlokations-ID starting \
                 with an ISO 3166-1 country code",
            ),
        );
    }
}
