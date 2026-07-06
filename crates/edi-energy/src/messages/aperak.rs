use edifact_rs::{
    EdifactDeserialize, EdifactSerialize, EventEmitter, OwnedSegment, ProfileRulePack,
    ValidationIssue, ValidationSeverity,
};

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{
            Bgm, Dtm, Erc, Ftx, Nad, Rff, collect_dtm, find_bgm, find_nad, find_rff,
            try_deserialize,
        },
    },
};

// ── Segment group types ───────────────────────────────────────────────────────

/// An application error group in APERAK (SG2: ERC + FTX + RFF).
///
/// Each instance reports one application-level error detected in the
/// referenced business message.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AperakError {
    /// ERC — application error code (mandatory).
    pub erc: Erc,
    /// FTX — free-text description of the error (zero or more).
    pub ftx: Vec<Ftx>,
    /// RFF — references related to this error (e.g. erroneous value).
    pub references: Vec<Rff>,
}

// ── AperakMessage ─────────────────────────────────────────────────────────────

/// APERAK — Application Error and Acknowledgement.
///
/// Acknowledges receipt of an application message and reports application-level
/// errors in the German energy market.
///
/// # Typed access
///
/// | Field        | Segment   | Meaning                                     |
/// |--------------|-----------|---------------------------------------------|
/// | `bgm`        | BGM       | Document code and acknowledgement reference |
/// | `dtm`        | DTM       | Message date/time segments                  |
/// | `sender`     | NAD+MS    | Message sender                              |
/// | `receiver`   | NAD+MR    | Message recipient                           |
/// | `ref_acw`    | RFF+ACW   | Acknowledgement reference number            |
/// | `errors`     | SG2/ERC   | Application error groups                    |
///
/// Each [`AperakError`] holds an [`Erc`] error code plus optional [`Ftx`]
/// free-text descriptions and [`Rff`] references.
#[derive(Debug, Clone)]
pub struct AperakMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message.
    bgm: Option<Bgm>,
    /// DTM — message-level date/time segments.
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message recipient.
    receiver: Option<Nad>,
    /// RFF+ACW — acknowledgement reference number.  Must be present in a
    /// valid APERAK (rule `SEM-APERAK-REF-MISSING`).
    ref_acw: Option<Rff>,
    /// SG2 — application error groups (ERC + optional FTX / RFF).
    errors: Vec<AperakError>,
}

impl AperakMessage {
    pub(crate) fn from_parts(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
    ) -> Self {
        let (bgm, dtm, sender, receiver, ref_acw, errors) = {
            let borrowed: Vec<edifact_rs::Segment<'_>> =
                segments.iter().map(|s| s.as_borrowed()).collect();
            (
                find_bgm(&borrowed),
                collect_dtm(&borrowed),
                find_nad(&borrowed, "MS"),
                find_nad(&borrowed, "MR"),
                find_rff(&borrowed, "ACW"),
                parse_errors(&borrowed),
            )
        };
        Self {
            core: MessageCore::new(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
                MessageType::Aperak,
            ),
            bgm,
            dtm,
            sender,
            receiver,
            ref_acw,
            errors,
        }
    }

    /// The EDI@Energy release / association code from UNH (DE 0057).
    #[must_use]
    pub fn assoc_code(&self) -> &str {
        &self.core.assoc_code
    }

    /// Raw parsed segments (authoritative for validation and serialization).
    #[must_use]
    pub fn segments(&self) -> &[OwnedSegment] {
        &self.core.segments
    }

    /// BGM — beginning of message.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn bgm(&self) -> Option<&Bgm> {
        self.bgm.as_ref()
    }

    /// DTM — message-level date/time segments.
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

    /// RFF+ACW — acknowledgement reference.  Returns `None` when absent.
    #[must_use]
    pub fn ref_acw(&self) -> Option<&Rff> {
        self.ref_acw.as_ref()
    }

    /// SG2 — application error groups.
    #[must_use]
    pub fn errors(&self) -> &[AperakError] {
        &self.errors
    }
}

// ── EdifactDeserialize ────────────────────────────────────────────────────────

impl EdifactDeserialize for AperakMessage {
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

impl EdifactSerialize for AperakMessage {
    fn edifact_serialize<E: EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), edifact_rs::EdifactError> {
        self.core.emit_segments(emitter)
    }
}
impl_edi_energy_message!(AperakMessage, sem = aperak_semantic_pack());

// ── segment group parsers ─────────────────────────────────────────────────────

/// Parse SG2 error groups (ERC + optional FTX/RFF) from the message.
///
/// Each `ERC` segment starts a new [`AperakError`].  Any `FTX` or `RFF`
/// segments that follow (before the next `ERC` or `UNT`) belong to it.
fn parse_errors(segments: &[edifact_rs::Segment<'_>]) -> Vec<AperakError> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < segments.len() {
        if segments[i].tag != "ERC" {
            i += 1;
            continue;
        }
        let Some(erc) = try_deserialize::<Erc>(&segments[i]) else {
            i += 1;
            continue;
        };
        let mut ftx = Vec::new();
        let mut references = Vec::new();
        let mut j = i + 1;
        while j < segments.len() && segments[j].tag != "ERC" && segments[j].tag != "UNT" {
            match segments[j].tag {
                "FTX" => {
                    if let Some(f) = try_deserialize::<Ftx>(&segments[j]) {
                        ftx.push(f);
                    }
                }
                "RFF" => {
                    if let Some(r) = try_deserialize::<Rff>(&segments[j]) {
                        references.push(r);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        result.push(AperakError {
            erc,
            ftx,
            references,
        });
        i = j;
    }
    result
}

// ── Layer 5: APERAK semantic rule pack ───────────────────────────────────────

/// Build the APERAK semantic rule pack (Layer 5).
///
/// Rules:
/// - [`rule_sem_aperak_ref_missing`]: An `RFF+ACW` reference to the original
///   message is mandatory in every APERAK.
fn aperak_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("APERAK-SEM")
        .for_message_type("APERAK")
        .with_stateless_rule_fn(rule_sem_aperak_ref_missing)
}

/// `SEM-APERAK-REF-MISSING` — Every APERAK must reference the message it is
/// responding to via `RFF+ACW`.
///
/// The `ACW` qualifier (DE 1153 = "Acknowledgement") in an `RFF` segment
/// carries the document/message number of the transaction being acknowledged
/// or rejected. Without it the recipient cannot correlate the APERAK to the
/// original request.
fn rule_sem_aperak_ref_missing(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let has_acw = segments
        .iter()
        .filter(|s| s.tag == "RFF")
        .any(|s| s.get_element(0).and_then(|e| e.get_component(0)) == Some("ACW"));
    // Capture the span of the first RFF for source location (best-effort: span points
    // to the RFF that was checked, even though the ACW variant is absent).
    let first_rff_span = segments.iter().find(|s| s.tag == "RFF").map(|s| s.span);
    if !has_acw {
        let mut issue = ValidationIssue::new(
            ValidationSeverity::Error,
            "APERAK must contain an RFF+ACW reference to the acknowledged/rejected \
             transaction (DE 1153 = ACW)",
        )
        .with_rule_id("SEM-APERAK-REF-MISSING")
        .with_segment("RFF")
        .with_suggestion(
            "Add an RFF segment with DE 1153 = ACW and set DE 1154 to the \
             message reference (UNH DE 0062) of the acknowledged or rejected message",
        );
        if let Some(span) = first_rff_span {
            issue = issue.with_span(span);
        }
        issues.push(issue);
    }
}
