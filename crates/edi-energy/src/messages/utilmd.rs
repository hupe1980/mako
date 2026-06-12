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
    /// IDE — object / process identifier (e.g. metering-point ID).
    pub ide: Ide,
    /// DTM — date/time segments scoped to this transaction.
    pub dtm: Vec<Dtm>,
    /// LOC — location information for this transaction (e.g. grid connection
    /// area), if present.
    pub loc: Option<Loc>,
    /// RFF — references related to this transaction (e.g. contract number).
    pub references: Vec<Rff>,
    /// STS — status segments (S2.1/S2.2 only; e.g. Sperrung E07/E08).
    pub sts: Vec<Sts>,
    /// FTX — free-text remarks scoped to this transaction.
    pub ftx: Vec<Ftx>,
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
        let mut loc: Option<Loc> = None;
        let mut references = Vec::new();
        let mut sts = Vec::new();
        let mut ftx = Vec::new();
        let mut j = i + 1;

        while j < segments.len() && segments[j].tag != "IDE" && segments[j].tag != "UNT" {
            match segments[j].tag {
                "DTM" => {
                    if let Some(d) = try_deserialize::<Dtm>(&segments[j]) {
                        dtm.push(d);
                    }
                }
                "LOC" => {
                    if loc.is_none() {
                        loc = try_deserialize::<Loc>(&segments[j]);
                    }
                }
                "RFF" => {
                    if let Some(r) = try_deserialize::<Rff>(&segments[j]) {
                        references.push(r);
                    }
                }
                "STS" => {
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
            loc,
            references,
            sts,
            ftx,
        });
        i = j;
    }
    result
}

// ── Layer 5: UTILMD semantic rule pack ───────────────────────────────────────

/// Build the UTILMD semantic rule pack (Layer 5).
///
/// These rules check business-level constraints that are not expressible in
/// the structural MIG/AHB schemas:
/// - [`rule_sem_malo_format`]: IDE market-location IDs must be exactly 11
///   upper-case alphanumeric characters ([A-Z0-9]{11}).
fn utilmd_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("UTILMD-SEM")
        .for_message_type("UTILMD")
        .with_stateless_rule_fn(rule_sem_malo_format)
}

/// `SEM-UTILMD-MALO-FORMAT` — Validate market/metering location IDs in IDE
/// segments.
///
/// Every `IDE` segment that carries a non-empty `C206.7402` identifier must
/// have exactly 11 upper-case alphanumeric characters (`[A-Z0-9]{11}`).  This
/// matches the BDEW format for both Marktlokations-IDs (11-digit numbers) and
/// Messlokations-IDs (11 alphanumeric chars).
fn rule_sem_malo_format(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    for seg in segments.iter().filter(|s| s.tag == "IDE") {
        // IDE: element[0] = 7495 (type qualifier), element[1] = C206 composite.
        // C206 component[0] = 7402 (free-form identification number).
        let id = seg
            .get_element(1)
            .and_then(|e| e.get_component(0))
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }
        if !is_valid_location_id(id) {
            issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "IDE element 7402 (C206 component 0): value does not match the \
                     Markt-/Messlokations-ID format [A-Z0-9]{11}"
                        .to_owned(),
                )
                .with_rule_id("SEM-UTILMD-MALO-FORMAT")
                .with_segment("IDE"),
            );
        }
    }
}

/// Returns `true` when `id` is exactly 11 ASCII upper-case letters or digits.
#[inline]
fn is_valid_location_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}
