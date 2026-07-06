//! Shared logic for concrete EDI@Energy message structs.
//!
//! Each concrete message type (UTILMD, MSCONS, …) is a thin newtype over
//! [`MessageCore`], which stores the raw parsed segments and metadata extracted
//! at parse time.
//!
//! This module is `pub(crate)` — downstream users only see the concrete types
//! and the [`EdiEnergyMessage`](crate::EdiEnergyMessage) trait.

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
use edifact_rs::{OwnedSegment, ProfileRulePack};

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
use crate::{
    EdiEnergyReport, Error, MessageType, Pruefidentifikator, Release, registry::ReleaseRegistry,
};

/// Internal storage shared by all concrete message types.
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
#[derive(Debug, Clone)]
pub(crate) struct MessageCore {
    /// All parsed segments (owned, heap-allocated), including any UNB/UNZ envelope.
    pub(crate) segments: Vec<OwnedSegment>,
    /// `true` when `segments` contains UNB/UNZ interchange envelope wrappers.
    /// Cached at construction to avoid scanning on every `validate()` call.
    has_interchange_wrapper: bool,
    /// Message reference from UNH element 0 (DE 0062).
    pub(crate) message_ref: Box<str>,
    /// Association assigned code from UNH element 1 component 4 (DE 0057).
    pub(crate) assoc_code: Box<str>,
    /// Release derived from `assoc_code`, cached at construction time to avoid
    /// re-allocating `Box<str>` + `ReleaseKind::parse` on every `validate()` call.
    pub(crate) release: Option<Release>,
    /// Prüfidentifikator extracted at parse time.
    ///
    /// For most message types this comes from BGM element 1 (DE 1004).
    /// For COMDIS it is extracted from the top-level `RFF+Z13` reference.
    pub(crate) pruefidentifikator: Option<u32>,
    /// The resolved message type discriminant.
    pub(crate) message_type: MessageType,
}

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
impl MessageCore {
    /// Construct from raw parsed data.
    pub(crate) fn new(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
        message_type: MessageType,
    ) -> Self {
        let assoc_code: Box<str> = assoc_code.into();
        let release = if assoc_code.is_empty() {
            None
        } else {
            Some(Release::new(&assoc_code))
        };
        // Cache whether this message has interchange envelope segments so
        // validate() doesn't scan the segment list on every call.
        let has_interchange_wrapper = segments.iter().any(|s| s.tag == "UNB");
        Self {
            segments,
            has_interchange_wrapper,
            message_ref: message_ref.into(),
            assoc_code,
            release,
            pruefidentifikator,
            message_type,
        }
    }

    // ── EdiEnergyMessage helpers ──────────────────────────────────────────

    pub(crate) fn message_type(&self) -> MessageType {
        self.message_type
    }

    /// Returns the cached release, or `Err(Error::MissingRelease)` when `assoc_code` was empty.
    pub(crate) fn detect_release(&self) -> Result<&Release, Error> {
        self.release.as_ref().ok_or(Error::MissingRelease)
    }

    pub(crate) fn detect_pruefidentifikator(&self) -> Result<Pruefidentifikator, Error> {
        match self.pruefidentifikator {
            None => Err(Error::MissingPruefidentifikator),
            Some(code) => Pruefidentifikator::new(code),
        }
    }

    /// Run validation layers 1–4, then optionally Layer 5 (semantic rule pack).
    ///
    /// `semantic_pack` is caller-supplied and message-type-specific. Pass
    /// `None` to run layers 1–4 only.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, release, semantic_pack),
            fields(
                message_type = %self.message_type,
                release = %release,
                message_ref = %self.message_ref,
            )
        )
    )]
    pub(crate) fn validate_against_with_semantic(
        &self,
        release: &Release,
        semantic_pack: Option<ProfileRulePack>,
    ) -> Result<EdiEnergyReport, Error> {
        self.validate_against_with_semantic_and_registry_on_date(
            release,
            semantic_pack,
            ReleaseRegistry::global(),
            None,
        )
    }

    /// Same as [`validate_against_with_semantic`] but uses `registry` instead of the global.
    pub(crate) fn validate_against_with_semantic_and_registry(
        &self,
        release: &Release,
        semantic_pack: Option<ProfileRulePack>,
        registry: &ReleaseRegistry,
    ) -> Result<EdiEnergyReport, Error> {
        self.validate_against_with_semantic_and_registry_on_date(
            release,
            semantic_pack,
            registry,
            None,
        )
    }

    /// Same as [`validate_against_with_semantic_and_registry`] but with explicit `reference_date`.
    ///
    /// `reference_date = None` falls back to `time::OffsetDateTime::now_utc().date()`.
    pub(crate) fn validate_against_with_semantic_and_registry_on_date(
        &self,
        release: &Release,
        semantic_pack: Option<ProfileRulePack>,
        registry: &ReleaseRegistry,
        reference_date: Option<time::Date>,
    ) -> Result<EdiEnergyReport, Error> {
        let date = reference_date.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
        // Layer 1: EDIFACT envelope validation — only when an interchange wrapper is
        // present.  Messages produced by builders contain UNH…UNT but no UNB/UNZ;
        // skipping the envelope check for bare messages lets callers validate
        // message-level content without first constructing a synthetic interchange.
        let interchange_header = if self.has_interchange_wrapper {
            let vi = edifact_rs::validate_envelope_owned(&self.segments).map_err(Error::Parse)?;
            Some(crate::interchange::InterchangeHeader::from_edifact_envelope(vi.interchange))
        } else {
            None
        };

        // Build the owned message-content segment slice, filtering out interchange-envelope
        // segments (UNB/UNZ interchange wrapper, UNG/UNE functional group wrapper).
        // Staying on owned segments avoids a separate Vec<Segment<'_>> borrow allocation.
        // edifact-rs 0.11 exposes group_owned_segments_indexed and validate_lenient_grouped_owned
        // so the borrowed intermediary is no longer necessary at this layer.
        let message_segments: Vec<edifact_rs::OwnedSegment> = self
            .segments
            .iter()
            .filter(|s| !matches!(s.tag.as_str(), "UNB" | "UNZ" | "UNG" | "UNE"))
            .cloned()
            .collect();
        match registry.profile_on(self.message_type, release, date) {
            Ok(profile) => {
                let dir_validator = profile.directory_validator();
                let mig_pack = profile.mig_rule_pack();
                // Layer 4: AHB rules — select the PID-specific pack when the PID is
                // detectable.  When no PID can be extracted, skip AHB rules but inject
                // a Warning-severity synthetic rule so audit logs know validation was
                // structurally incomplete.  Running the union-of-all-PIDs pack would
                // produce guaranteed false positives because mutually-exclusive qualifier
                // constraints (e.g. BGM+E01 for PID 55001 vs BGM+E0F for PID 55004)
                // all fire on the same message.
                let pid_result = self.detect_pruefidentifikator();
                let ahb_pack_opt = match pid_result {
                    Ok(pid) => Some(profile.ahb_rule_pack(Some(pid))),
                    Err(_) => None,
                };
                let mut ctx = edifact_rs::ValidationContext::builder()
                    .with_message_type(self.message_type.as_str())
                    .with_message_ref(&*self.message_ref)
                    // Short-circuit the entire validation pass on the first Critical-severity
                    // structural failure.  Critical issues indicate a segment is so malformed
                    // that further validation would only produce noise (duplicate false positives
                    // from downstream rules that assume the segment is well-formed).
                    .bail_on_first_critical(true)
                    .with_validator(
                        edifact_rs::ValidationLayer::Structure,
                        dir_validator.clone(),
                    );
                // When Pruefidentifikator cannot be determined, inject a Warning-severity
                // advisory via with_static_issue so downstream audit logs
                // know AHB Layer 4 was skipped.  Running the union-of-all-PIDs pack would
                // produce guaranteed false positives from mutually-exclusive qualifier constraints.
                if pid_result.is_err() {
                    ctx = ctx.with_static_issue(
                        edifact_rs::ValidationIssue::new(
                            edifact_rs::ValidationSeverity::Warning,
                            "AHB Layer 4 validation skipped: \
                             Pruefidentifikator could not be determined \
                             from BGM segment"
                                .to_owned(),
                        )
                        .with_rule_id("AHB-SKIP-NO-PID".to_owned()),
                    );
                }
                ctx = ctx.with_profile_pack_arc(mig_pack);
                if let Some(ahb_pack) = ahb_pack_opt {
                    ctx = ctx.with_profile_pack_arc(ahb_pack);
                }
                // Layer 5: message-type-specific semantic rule pack (optional, caller-supplied).
                if let Some(sem) = semantic_pack {
                    ctx = ctx.with_profile_pack(sem);
                }
                // Build the segment-group tree from owned segments and validate using the
                // fully-owned group-aware path.  group_owned_segments_indexed avoids a
                // second borrow allocation when no group rules are registered (the O(1) early
                // exit in validate_lenient_grouped_owned skips the borrow entirely).
                let group_schema = profile.group_schema();
                let group_tree = edifact_rs::group_owned_segments_indexed(
                    &message_segments,
                    group_schema,
                    "ROOT",
                );
                let report = ctx
                    .build()
                    .validate_lenient_grouped_owned(&group_tree, &message_segments);
                #[cfg(feature = "tracing")]
                {
                    let error_count = report.errors().len();
                    let warning_count = report.warnings().len();
                    if error_count > 0 {
                        tracing::info!(
                            errors = error_count,
                            warnings = warning_count,
                            "validation failed"
                        );
                    } else {
                        tracing::debug!(warnings = warning_count, "validation passed");
                    }
                }
                let mut report_out = EdiEnergyReport::new_with_pid(report, self.pruefidentifikator)
                    .with_profile_meta(profile.release().clone(), profile.ahb_revision());
                if let Some(hdr) = interchange_header {
                    report_out = report_out.with_interchange_header(hdr);
                }
                Ok(report_out)
            }
            Err(Error::ProfileNotFound { .. }) => {
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    release = %release,
                    message_type = %self.message_type,
                    "profile not found for release — unknown release code on production path"
                );
                Err(Error::ProfileNotFound {
                    message_type: self.message_type,
                    release: (*release).clone(),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Run all validation layers plus an extra caller-supplied rule pack.
    ///
    /// For message types that have a built-in semantic rule pack (e.g. UTILMD, MSCONS),
    /// the `extra` pack runs after it. For types without a semantic pack it is the only
    /// Layer-5 equivalent, appended after L4 (AHB).
    ///
    /// Callers that need BOTH the built-in semantic pack AND extra rules should merge
    /// them with `merge_with_override` and pass the result here, or call the public
    /// trait method `validate_with_pack` on the concrete message type.
    pub(crate) fn validate_with_extra_pack(
        &self,
        semantic_pack: Option<ProfileRulePack>,
        extra: ProfileRulePack,
    ) -> Result<EdiEnergyReport, Error> {
        self.validate_with_extra_pack_and_registry(semantic_pack, extra, ReleaseRegistry::global())
    }

    /// Same as [`validate_with_extra_pack`] but uses `registry` instead of the global.
    pub(crate) fn validate_with_extra_pack_and_registry(
        &self,
        semantic_pack: Option<ProfileRulePack>,
        extra: ProfileRulePack,
        registry: &ReleaseRegistry,
    ) -> Result<EdiEnergyReport, Error> {
        let combined = match semantic_pack {
            Some(sem) => sem
                .merge_with_override(extra)
                .expect("semantic + extra pack merge failed: incompatible release scopes"),
            None => extra,
        };
        let release = self.detect_release()?;
        self.validate_against_with_semantic_and_registry(release, Some(combined), registry)
    }

    pub(crate) fn serialize(&self) -> Result<Vec<u8>, Error> {
        Ok(edifact_rs::segments_to_bytes_owned(&self.segments)?)
    }

    // ── edifact-rs trait helpers ──────────────────────────────────────────────

    /// Extract `(message_ref, assoc_code)` from the `UNH` segment.
    ///
    /// Shared by the `EdifactDeserialize` impls on all complex message types
    /// (`AperakMessage`, `ContrlMessage`, `MsconsMessage`, `UtilmdMessage`).
    pub(crate) fn extract_unh_fields(
        segments: &[edifact_rs::Segment<'_>],
    ) -> Result<(String, String), edifact_rs::EdifactError> {
        let unh = segments.iter().find(|s| s.tag == "UNH").ok_or_else(|| {
            edifact_rs::EdifactError::MissingSegment {
                tag: "UNH".to_owned(),
                expected_position: "position 1 (message start)".to_owned(),
            }
        })?;
        let message_ref = unh.element_str(0).unwrap_or_default().to_owned();
        let assoc_code = unh
            .get_element(1)
            .and_then(|e| e.get_component(4))
            .unwrap_or_default()
            .to_owned();
        Ok((message_ref, assoc_code))
    }

    /// Extract a Prüfidentifikator from the first `BGM` segment (element 1, DE 1004).
    ///
    /// Returns `None` when no `BGM` is present or element 1 is not a valid `u32`.
    pub(crate) fn extract_bgm_pid(segments: &[edifact_rs::Segment<'_>]) -> Option<u32> {
        segments
            .iter()
            .find(|s| s.tag == "BGM")
            .and_then(|s| s.element_str(1))
            .and_then(|v| v.parse().ok())
    }

    /// Replay all raw segments through an EDIFACT event emitter.
    ///
    /// Shared `EdifactSerialize` body for all message types that store
    /// authoritative wire bytes in `self.segments` rather than re-deriving
    /// them from typed fields.
    pub(crate) fn emit_segments<E: edifact_rs::EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), edifact_rs::EdifactError> {
        for seg in &self.segments {
            emitter.emit(edifact_rs::EdifactEvent::StartSegment { tag: &seg.tag })?;
            for element in &seg.elements {
                for (i, (value, _span)) in element.components.iter().enumerate() {
                    if i == 0 {
                        emitter.emit(edifact_rs::EdifactEvent::Element { value })?;
                    } else {
                        emitter.emit(edifact_rs::EdifactEvent::ComponentElement { value })?;
                    }
                }
            }
            emitter.emit(edifact_rs::EdifactEvent::EndSegment)?;
        }
        Ok(())
    }
}
