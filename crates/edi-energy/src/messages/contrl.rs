use edifact_rs::{
    EdifactDeserialize, EdifactSerialize, EventEmitter, OwnedSegment, ProfileRulePack,
    ValidationIssue, ValidationSeverity,
};

use crate::{
    EdiEnergyMessage, EdiEnergyReport, Error, MessageType, Pruefidentifikator, Release,
    messages::{
        core::MessageCore,
        segments::{Ucd, Uci, Ucm, Ucs, find_uci, try_deserialize},
    },
};

// ── Segment group types ───────────────────────────────────────────────────────

/// A message-level acknowledgement in CONTRL (SG1 — UCM group).
///
/// Each instance acknowledges or rejects one specific message within the
/// acknowledged interchange.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ContrlMessageResponse {
    /// UCM — message acknowledgement (`action_code = "4"` = accepted, `"7"` = rejected).
    pub ucm: Ucm,
    /// SG2 — per-segment error reports (only present when message is rejected).
    pub segment_errors: Vec<ContrlSegmentError>,
}

/// A segment-level error within a rejected message (CONTRL SG2 — UCS group).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ContrlSegmentError {
    /// UCS — segment position in the erroneous message.
    pub ucs: Ucs,
    /// SG3 — per-data-element error details (present when the error is in a DE).
    pub element_errors: Vec<ContrlElementError>,
}

/// A data-element error within a rejected segment (CONTRL SG3 — UCD).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ContrlElementError {
    /// UCD — identifies the faulty data element and error code.
    pub ucd: Ucd,
}

// ── ContrlMessage ─────────────────────────────────────────────────────────────

/// CONTRL — Interchange Control Structure (syntax acknowledgement).
///
/// Acknowledges receipt and syntactic correctness of an EDIFACT interchange
/// at the UNB / UNH level.
///
/// # Typed access
///
/// | Field               | Segment | Meaning                                         |
/// |---------------------|---------|-------------------------------------------------|
/// | `uci`               | UCI     | Interchange reference and acknowledgement code  |
/// | `message_responses` | SG1/UCM | Per-message acknowledgement groups              |
///
/// Each [`ContrlMessageResponse`] holds a [`Ucm`] and zero or more
/// [`ContrlSegmentError`] / [`ContrlElementError`] sub-groups.
#[derive(Debug, Clone)]
pub struct ContrlMessage {
    pub(crate) core: MessageCore,
    /// UCI — Interchange Control Response (always present in a valid CONTRL).
    uci: Option<Uci>,
    /// SG1 — per-message acknowledgement groups (`UCM` + optional `UCS`/`UCD`).
    ///
    /// Empty for a simple positive acknowledgement (all messages accepted).
    message_responses: Vec<ContrlMessageResponse>,
}

impl ContrlMessage {
    pub(crate) fn from_parts(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
    ) -> Self {
        let (uci, message_responses) = {
            let borrowed: Vec<edifact_rs::Segment<'_>> =
                segments.iter().map(|s| s.as_borrowed()).collect();
            let uci = find_uci(&borrowed);
            let responses = parse_message_responses(&borrowed);
            (uci, responses)
        };
        Self {
            core: MessageCore::new(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
                MessageType::Contrl,
            ),
            uci,
            message_responses,
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

    /// UCI — Interchange Control Response.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn uci(&self) -> Option<&Uci> {
        self.uci.as_ref()
    }

    /// SG1 — per-message acknowledgement groups.
    #[must_use]
    pub fn message_responses(&self) -> &[ContrlMessageResponse] {
        &self.message_responses
    }
}

// ── EdifactDeserialize ────────────────────────────────────────────────────────

impl EdifactDeserialize for ContrlMessage {
    fn edifact_deserialize(
        segments: &[edifact_rs::Segment<'_>],
    ) -> Result<Self, edifact_rs::EdifactError> {
        let (message_ref, assoc_code) = MessageCore::extract_unh_fields(segments)?;
        let owned: Vec<OwnedSegment> = segments.iter().cloned().map(OwnedSegment::from).collect();
        Ok(Self::from_parts(owned, message_ref, assoc_code, None))
    }
}

// ── EdifactSerialize ──────────────────────────────────────────────────────────

impl EdifactSerialize for ContrlMessage {
    fn edifact_serialize<E: EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), edifact_rs::EdifactError> {
        self.core.emit_segments(emitter)
    }
}
impl EdiEnergyMessage for ContrlMessage {
    fn try_message_type(&self) -> Option<MessageType> {
        Some(self.core.message_type())
    }
    fn detect_release(&self) -> Result<&Release, Error> {
        self.core.detect_release()
    }
    fn message_ref(&self) -> &str {
        &self.core.message_ref
    }
    /// CONTRL is the EDIFACT syntax acknowledgement message.
    ///
    /// It does **not** use Pruefidentifikatoren — always returns
    /// [`Error::MissingPruefidentifikator`].
    fn detect_pruefidentifikator(&self) -> Result<Pruefidentifikator, Error> {
        Err(Error::MissingPruefidentifikator)
    }
    fn validate(&self) -> Result<EdiEnergyReport, Error> {
        let release = self.core.detect_release()?;
        self.core
            .validate_against_with_semantic(release, Some(contrl_semantic_pack()))
    }
    fn validate_against(&self, release: &Release) -> Result<EdiEnergyReport, Error> {
        self.core
            .validate_against_with_semantic(release, Some(contrl_semantic_pack()))
    }
    fn serialize(&self) -> Result<Vec<u8>, Error> {
        self.core.serialize()
    }
    fn segments(&self) -> &[edifact_rs::OwnedSegment] {
        &self.core.segments
    }
    fn validate_with_pack(&self, extra: crate::CustomRulePack) -> Result<EdiEnergyReport, Error> {
        self.core.validate_with_extra_pack(None, extra.into_inner())
    }
    fn validate_on_date(&self, reference_date: time::Date) -> Result<EdiEnergyReport, Error> {
        let release = self.core.detect_release()?;
        self.core
            .validate_against_with_semantic_and_registry_on_date(
                release,
                Some(contrl_semantic_pack()),
                crate::registry::ReleaseRegistry::global(),
                Some(reference_date),
            )
    }
}

// ── segment group parsers ─────────────────────────────────────────────────────

/// Parse all SG1 (UCM) groups from the segment list.
///
/// Each `UCM` segment triggers a new [`ContrlMessageResponse`].  Within it,
/// `UCS` segments trigger [`ContrlSegmentError`] sub-groups (SG2), and each
/// `UCD` after a `UCS` is wrapped in a [`ContrlElementError`] (SG3).
fn parse_message_responses(segments: &[edifact_rs::Segment<'_>]) -> Vec<ContrlMessageResponse> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < segments.len() {
        if segments[i].tag != "UCM" {
            i += 1;
            continue;
        }
        let Some(ucm) = try_deserialize::<Ucm>(&segments[i]) else {
            i += 1;
            continue;
        };

        // Collect SG2 groups until the next UCM or UNT.
        let mut segment_errors = Vec::new();
        let mut j = i + 1;
        while j < segments.len() && segments[j].tag != "UCM" && segments[j].tag != "UNT" {
            if segments[j].tag != "UCS" {
                j += 1;
                continue;
            }
            let Some(ucs_seg) = try_deserialize::<Ucs>(&segments[j]) else {
                j += 1;
                continue;
            };

            // Collect SG3 (UCD) until the next UCS, UCM, or UNT.
            let mut element_errors = Vec::new();
            let mut k = j + 1;
            while k < segments.len() && segments[k].tag == "UCD" {
                if let Some(ucd) = try_deserialize::<Ucd>(&segments[k]) {
                    element_errors.push(ContrlElementError { ucd });
                }
                k += 1;
            }
            segment_errors.push(ContrlSegmentError {
                ucs: ucs_seg,
                element_errors,
            });
            j = k;
        }

        result.push(ContrlMessageResponse {
            ucm,
            segment_errors,
        });
        i = j;
    }

    result
}

// ── Layer 5: CONTRL semantic rule pack ───────────────────────────────────────

/// UN/EDIFACT CONTRL acknowledgement codes (DE 0083).
///
/// - `4`: acknowledged (interchange accepted)
/// - `7`: interchange rejected — at least one functional group rejected
/// - `8`: interchange rejected — the entire interchange is rejected
const VALID_UCI_CODES: &[&str] = &["4", "7", "8"];

/// Build the CONTRL semantic rule pack (Layer 5).
///
/// Rules:
/// - [`rule_sem_contrl_syntax_code`]: the `UCI` acknowledgement code (DE 0083)
///   must be one of the UN/EDIFACT defined values `4`, `7`, or `8`.
fn contrl_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("CONTRL-SEM")
        .for_message_type("CONTRL")
        .with_stateless_rule_fn(rule_sem_contrl_syntax_code)
}

/// `SEM-CONTRL-SYNTAX-CODE-UNKNOWN` — Validate the `UCI` acknowledgement code
/// (DE 0083).
///
/// UN/EDIFACT defines exactly three valid values:
/// - `4` — acknowledged (interchange accepted)
/// - `7` — interchange rejected (group-level)
/// - `8` — interchange rejected (interchange-level)
///
/// Any other value indicates a malformed CONTRL message.
fn rule_sem_contrl_syntax_code(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    for seg in segments.iter().filter(|s| s.tag == "UCI") {
        // UCI: element[3] = DE 0083 (acknowledgement code)
        let code = seg.element_str(3).unwrap_or("");
        if code.is_empty() {
            continue;
        }
        if !VALID_UCI_CODES.contains(&code) {
            issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    format!(
                        "UCI acknowledgement code '{code}' is not in the UN/EDIFACT \
                         defined set (4=accepted, 7=rejected-group, 8=rejected-interchange)"
                    ),
                )
                .with_span(seg.span)
                .with_rule_id("SEM-CONTRL-SYNTAX-CODE-UNKNOWN")
                .with_segment("UCI")
                .with_suggestion(
                    "Valid UN/EDIFACT UCI acknowledgement codes (DE 0083): \
                     4 = interchange received, \
                     7 = rejected at functional-group level, \
                     8 = rejected at interchange level",
                ),
            );
        }
    }
}
