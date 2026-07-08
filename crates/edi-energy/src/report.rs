use std::fmt;

use edifact_rs::{ValidationIssue, ValidationReport};

/// Classify a rule ID into a validation layer origin string.
///
/// Used to populate [`ValidationIssueSummary::rule_origin`].
///
/// Returns `None` when `rule_id` is `None` or does not match a known prefix.
fn classify_rule_origin(rule_id: &str) -> Option<&'static str> {
    if rule_id.starts_with("MIG-") || rule_id.starts_with("UNKNOWN-MSG-TYPE") {
        Some("mig")
    } else if rule_id.starts_with("AHB-") || rule_id.starts_with("AHB-SKIP-") {
        Some("ahb")
    } else if rule_id.starts_with("SEM-") {
        Some("semantic")
    } else if rule_id.starts_with("PARSE-") {
        Some("parse")
    } else if rule_id.starts_with("DIR-") {
        Some("directory")
    } else if rule_id.starts_with("CUSTOM-") {
        Some("custom")
    } else {
        None
    }
}

/// Owned snapshot of a single validation issue.
///
/// Mirrors [`edifact_rs::ValidationIssue`] but owns all strings so it can be
/// sent across threads, stored, or serialized independently of the underlying
/// `ValidationReport`.
///
/// Unconditionally available (no feature gate required).  The `serde` feature
/// adds `#[derive(Serialize)]` so instances can be JSON-encoded directly.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct ValidationIssueSummary {
    /// Severity string: `"critical"`, `"error"`, `"warning"`, or `"info"`.
    pub severity: &'static str,
    /// Human-readable description of the issue.
    pub message: String,
    /// Stable rule identifier (e.g. `"MIG-DTM-001"` or `"AHB-13001-STS-I0"`),
    /// if available.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rule_id: Option<String>,
    /// Stable error code assigned by the validator, if any.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub error_code: Option<&'static str>,
    /// Tag of the EDIFACT segment where the issue occurred, e.g. `"DTM"`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub segment_tag: Option<String>,
    /// 0-based index of the occurrence among all segments with `segment_tag`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub segment_occurrence: Option<u16>,
    /// Segment group name in which the issue occurred (e.g. `"SG4"`), if available.
    ///
    /// Populated from [`edifact_rs::ValidationIssue::segment_group`] (edifact-rs 0.9+).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub segment_group: Option<String>,
    /// 0-based data-element index within the segment.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub element_index: Option<u8>,
    /// 0-based component index within a composite data element.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub component_index: Option<u8>,
    /// UNH message reference (DE 0062) the issue belongs to.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub message_ref: Option<String>,
    /// Suggested remediation text, if available.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub suggestion: Option<String>,
    /// Byte offset of the first byte of the affected region in the source input.
    ///
    /// Populated from [`edifact_rs::ValidationIssue::span`]`.start` (0.11.0+)
    /// when the issue carries a full span; otherwise from `issue.offset`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub offset: Option<usize>,
    /// Exclusive byte offset of the end of the affected region in the source input.
    ///
    /// Together with [`offset`](Self::offset) this forms a half-open byte range
    /// `[offset, byte_end)` that maps directly to an LSP `Range` or a `miette`
    /// source span.  `None` when the issue has no span (only a start offset or
    /// no position information at all).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub byte_end: Option<usize>,
    /// The BDEW Prüfidentifikator (process variant code) associated with this
    /// issue, when the report was produced by a PID-specific validation layer.
    ///
    /// This field enables downstream audit logs and monitoring systems to
    /// identify which BDEW process variant triggered a violation without
    /// re-reading the raw EDIFACT message — satisfying the regulatory
    /// traceability requirement for German energy market participants.
    ///
    /// `None` when the validation layer does not use PID-gated rule packs
    /// (e.g. structural Layer 1–2 checks that apply to all process variants).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub pruefidentifikator: Option<u32>,
    /// Validation layer that generated this issue.
    ///
    /// Classifies the issue by its origin in the layered validation stack:
    ///
    /// | Value | Layer | Meaning |
    /// |-------|-------|---------|
    /// | `"parse"` | L1 | EDIFACT parse / tokenizer error |
    /// | `"directory"` | L2 | Directory check (segment definitions, code lists) |
    /// | `"mig"` | L3 | MIG structural rule (segment ordering, cardinality) |
    /// | `"ahb"` | L4–L5 | AHB Bedingungsoperator rule |
    /// | `"custom"` | L6 | Caller-supplied `CustomRulePack` rule |
    ///
    /// Used by monitoring dashboards and regulatory audit systems to distinguish
    /// sender-side malformed EDI (L1–L2) from conformance violations (L3–L5)
    /// from local business-rule failures (L6).
    ///
    /// Derived from the `rule_id` prefix heuristic:
    /// - `"MIG-"` prefix → `"mig"`, `"AHB-"` → `"ahb"`, `"PARSE-"` → `"parse"`,
    ///   `"DIR-"` → `"directory"`, `"CUSTOM-"` → `"custom"`.
    /// - `None` when the `rule_id` is absent or does not match a known prefix.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rule_origin: Option<&'static str>,
}

impl ValidationIssueSummary {
    /// Convert a `ValidationIssue` to a `ValidationIssueSummary`, tagging it
    /// with the Prüfidentifikator that was active during validation.
    ///
    /// The PID is resolved in priority order:
    /// 1. `issue.context_get("pid")` — set per-issue by AHB rule closures via
    ///    `with_context_entry("pid", …)` (edifact-rs 0.9.1+,).
    /// 2. `pruefidentifikator` — report-level fallback for callers that set the
    ///    PID on the report rather than on individual issues.
    ///
    /// Use this from `EdiEnergyReport::serialize` so every serialized issue
    /// carries the PID context needed for regulatory audit logs.
    pub fn from_issue_with_pid(issue: &ValidationIssue, pruefidentifikator: Option<u32>) -> Self {
        // Per-issue context: prefer the PID embedded in the issue itself.
        let resolved_pid = issue
            .context_get("pid")
            .and_then(|s| s.parse::<u32>().ok())
            .or(pruefidentifikator);

        // Derive rule_origin from rule_id prefix.
        let rule_origin = issue.rule_id.as_deref().and_then(classify_rule_origin);

        Self {
            // Use the as_str() helper added in edifact-rs 0.9 instead of a
            // hand-written match — future severity variants are handled automatically.
            severity: issue.severity.as_str(),
            message: issue.message.clone(),
            rule_id: issue.rule_id.clone(),
            error_code: issue.error_code,
            segment_tag: issue.segment_tag.clone(),
            segment_occurrence: issue.segment_occurrence,
            segment_group: issue.segment_group.as_deref().map(str::to_owned),
            element_index: issue.element_index,
            component_index: issue.component_index,
            message_ref: issue.message_ref.clone(),
            suggestion: issue.suggestion.clone(),
            offset: issue.span().map(|s| s.start).or(issue.offset),
            byte_end: issue.span().map(|s| s.end),
            pruefidentifikator: resolved_pid,
            rule_origin,
        }
    }
}

/// The result of validating an EDI@Energy message.
///
/// Wraps [`edifact_rs::ValidationReport`] with a stable, ergonomic API.
/// Issues are divided into three severity buckets: errors, warnings, and infos.
///
/// A report is considered *valid* when it contains no error-level issues (warnings
/// and infos are allowed).
#[derive(Debug, Clone)]
pub struct EdiEnergyReport {
    inner: ValidationReport,
    /// The Prüfidentifikator the message was validated against, if known.
    ///
    /// Set when validation is performed against a PID-specific AHB rule pack.
    /// Propagated into each [`ValidationIssueSummary`] during serialization.
    pruefidentifikator: Option<u32>,
    /// The wire-format release code from the profile (e.g. `"2.4c"`, `"5.5.3a"`).
    ///
    /// Populated when validation is performed through the registry so audit logs
    /// can identify which BDEW release window was active.
    release: Option<crate::release::Release>,
    /// The AHB revision identifier (e.g. `"3.2e"`, `"2.0h"`).
    ///
    /// May carry a correction letter that differs from `release` when BDEW issues
    /// an AHB correction without a MIG change (e.g. INSRPT wire `1.1a` but AHB `1.1g`).
    /// Including this in audit records disambiguates which rule set was applied.
    ahb_revision: Option<&'static str>,
    /// The parsed interchange envelope header (UNB fields), when the message was
    /// validated from a full interchange (UNB…UNZ).
    ///
    /// `None` for bare messages (UNH…UNT only, no interchange wrapper).  Present
    /// when an interchange wrapper was detected and successfully validated by
    /// `edifact_rs::validate_envelope_owned`.
    ///
    /// Combines with the validation issues to provide a single comprehensive
    /// audit record (who sent it, when, control reference, and whether it was valid).
    pub interchange_header: Option<crate::interchange::InterchangeHeader>,
}

impl EdiEnergyReport {
    /// Construct from a raw [`ValidationReport`].
    #[must_use]
    #[allow(dead_code)] // used by feature-gated message modules
    pub(crate) fn new(inner: ValidationReport) -> Self {
        Self {
            inner,
            pruefidentifikator: None,
            release: None,
            ahb_revision: None,
            interchange_header: None,
        }
    }

    /// Construct with an associated Prüfidentifikator.
    ///
    /// Use this from message validation code that knows which PID-specific rule
    /// pack was applied, so the PID is available in serialized issue summaries.
    #[must_use]
    #[allow(dead_code)] // used by feature-gated message modules
    pub(crate) fn new_with_pid(inner: ValidationReport, pid: Option<u32>) -> Self {
        Self {
            inner,
            pruefidentifikator: pid,
            release: None,
            ahb_revision: None,
            interchange_header: None,
        }
    }

    /// Attach profile metadata for unambiguous audit-log serialization.
    ///
    /// Sets `release` (wire format release code) and `ahb_revision` on the report.
    /// Both are emitted in the serialized JSON output so downstream audit systems
    /// can identify exactly which BDEW specification version governed the validation,
    /// including AHB correction revisions that share a wire code (see.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn with_profile_meta(
        mut self,
        release: crate::release::Release,
        ahb_revision: Option<&'static str>,
    ) -> Self {
        self.release = Some(release);
        self.ahb_revision = ahb_revision;
        self
    }

    /// Attach the interchange envelope header to the report.
    ///
    /// Call this when the message was validated from a full interchange (UNB…UNZ)
    /// so that the report carries both routing metadata and validation findings as
    /// a single audit record.
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
    #[must_use]
    pub(crate) fn with_interchange_header(
        mut self,
        header: crate::interchange::InterchangeHeader,
    ) -> Self {
        self.interchange_header = Some(header);
        self
    }

    /// Error-level issues. Any entry here means the message is non-conformant.
    ///
    /// In the underlying `edifact-rs` model, `Critical`-severity issues are also
    /// stored here (both `Critical` and `Error` map to the same errors bucket).
    /// Use [`criticals`][Self::criticals] to distinguish them when needed.
    #[must_use]
    pub fn errors(&self) -> &[ValidationIssue] {
        self.inner.errors()
    }

    /// Critical-severity issues — a subset of [`errors`][Self::errors].
    ///
    /// `Critical` issues indicate a structural violation severe enough to abort
    /// further validation (e.g. a malformed `UNH` envelope segment).  They are
    /// stored in the same bucket as `Error`-severity issues by `edifact-rs`, so
    /// `is_valid()` already returns `false` when any `Critical` issue is present.
    ///
    /// This accessor filters `errors()` by `ValidationSeverity::Critical` and
    /// is useful when you need to distinguish abort-level failures from regular
    /// conformance errors in monitoring dashboards or audit logs.
    pub fn criticals(&self) -> impl Iterator<Item = &ValidationIssue> {
        use edifact_rs::ValidationSeverity;
        self.inner
            .errors()
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Critical)
    }

    /// Warning-level issues. The message may still be processable.
    #[must_use]
    pub fn warnings(&self) -> &[ValidationIssue] {
        self.inner.warnings()
    }

    /// Informational notes that do not affect validity.
    #[must_use]
    pub fn infos(&self) -> &[ValidationIssue] {
        self.inner.infos()
    }

    /// Returns `true` if there are no error-level issues.
    ///
    /// Warnings and infos do not affect this result.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.inner.is_valid()
    }

    /// Returns `true` if there is at least one error-level issue.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.inner.has_errors()
    }

    /// Returns `true` if there is at least one warning-level issue.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.inner.has_warnings()
    }

    /// Total number of issues across all severity levels.
    #[must_use]
    pub fn total_issues(&self) -> usize {
        self.inner.total_issues()
    }

    /// Iterate over all issues in order: errors → warnings → infos.
    pub fn iter_issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.inner.iter_issues()
    }

    /// Iterate over issues from a specific validation layer.
    ///
    /// Filters by the same `rule_origin` tag used in [`ValidationIssueSummary::rule_origin`]:
    /// - `"parse"` — EDIFACT parse / tokenizer errors (L1)
    /// - `"directory"` — directory structure checks (L2)
    /// - `"mig"` — MIG structural rules (L3)
    /// - `"ahb"` — AHB Bedingungsoperator rules (L4–L5)
    /// - `"custom"` — caller-supplied `CustomRulePack` rules (L6)
    ///
    /// This allows monitoring dashboards to quickly separate "sender sent garbage"
    /// (L1–L2) from "sender violated BDEW process rules" (L3–L5).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use edi_energy::{EdiEnergyMessage, Platform};
    /// let msg = Platform::with_all_profiles().parse(b"UNB+...").unwrap();
    /// let report = msg.validate().unwrap();
    /// let ahb_errors: Vec<_> = report.issues_by_origin("ahb").collect();
    /// ```
    pub fn issues_by_origin<'a>(
        &'a self,
        origin: &'a str,
    ) -> impl Iterator<Item = &'a ValidationIssue> + 'a {
        self.inner.iter_issues().filter(move |issue| {
            issue.rule_id.as_deref().and_then(classify_rule_origin) == Some(origin)
        })
    }

    /// Return a new report containing only issues whose rule identifier starts with `prefix`.
    ///
    /// This allocates a new report (O(n)). For read-only access prefer
    /// [`issues_with_rule_prefix`][Self::issues_with_rule_prefix].
    #[must_use]
    pub fn filter_by_rule_prefix(&self, prefix: &str) -> Self {
        Self {
            inner: self.inner.filter_by_rule_prefix(prefix),
            pruefidentifikator: self.pruefidentifikator,
            release: self.release.clone(),
            ahb_revision: self.ahb_revision,
            interchange_header: self.interchange_header.clone(),
        }
    }

    /// Iterate over issues whose rule identifier starts with `prefix` without cloning.
    ///
    /// This is the O(1)-allocation alternative to [`filter_by_rule_prefix`][Self::filter_by_rule_prefix].
    pub fn issues_with_rule_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = &'a ValidationIssue> + 'a {
        self.inner
            .iter_issues()
            .filter(move |issue| issue.rule_id().is_some_and(|id| id.starts_with(prefix)))
    }

    /// Return a new report containing only issues with an exact rule identifier.
    #[must_use]
    pub fn filter_by_rule_id(&self, rule_id: &str) -> Self {
        Self {
            inner: self.inner.filter_by_rule_id(rule_id),
            pruefidentifikator: self.pruefidentifikator,
            release: self.release.clone(),
            ahb_revision: self.ahb_revision,
            interchange_header: self.interchange_header.clone(),
        }
    }

    /// Iterate over all issues matching an exact profile/MIG rule identifier.
    pub fn issues_for_rule_id<'a>(
        &'a self,
        rule_id: &'a str,
    ) -> impl Iterator<Item = &'a ValidationIssue> + 'a {
        self.inner.issues_for_rule_id(rule_id)
    }

    /// A stable, deterministic text rendering suitable for snapshot tests and logs.
    #[must_use]
    pub fn render_deterministic(&self) -> String {
        self.inner.render_deterministic()
    }

    /// Convert to a `Result`, consuming `self`.
    ///
    /// Returns `Ok(())` when there are no errors, `Err(report)` otherwise.
    /// Warnings and infos are preserved in the `Err` variant.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)` when the report contains one or more error-level issues.
    pub fn into_result(self) -> Result<(), Self> {
        if self.inner.has_errors() {
            Err(self)
        } else {
            Ok(())
        }
    }

    /// Convert to a library `Result`, consuming `self`.
    ///
    /// Returns `Ok(())` when there are no errors, `Err(Error::Validation { ... })`
    /// otherwise.  Use this when you want to propagate validation failure as a
    /// first-class [`crate::Error`] variant rather than handling the raw report.
    ///
    /// Returns `Ok(self)` when the report has no error-level issues, `Err`
    /// otherwise.
    ///
    /// This is the idiomatic alternative to [`into_error_result`][Self::into_error_result]:
    /// callers that need the report for further inspection after an error can
    /// recover it from the `Err` variant via pattern matching on
    /// [`crate::Error::Validation`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Validation`] when the report contains at least
    /// one error-level issue.
    pub fn as_result(self) -> Result<Self, crate::Error> {
        if self.inner.has_errors() {
            let count = self.inner.errors().len();
            Err(crate::Error::Validation {
                count,
                report: self,
            })
        } else {
            Ok(self)
        }
    }

    /// Returns `Ok(())` when the report has no error-level issues, `Err` otherwise.
    ///
    /// Prefer [`as_result`][Self::as_result] when you need access to the report
    /// after a validation failure.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Validation`] when the report contains at least one
    /// error-level issue.
    pub fn into_error_result(self) -> Result<(), crate::Error> {
        if self.inner.has_errors() {
            let count = self.inner.errors().len();
            Err(crate::Error::Validation {
                count,
                report: self,
            })
        } else {
            Ok(())
        }
    }

    /// Convert to `Result<Self, Self>`, consuming `self`.
    ///
    /// Returns `Ok(self)` when valid (no errors), `Err(self)` when invalid.
    /// Mirrors the `edifact_rs::ValidationReport::result()` API so call-sites
    /// that use `?` propagation work symmetrically across both types.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)` when the report contains at least one error-level issue.
    #[must_use = "call into_result() or as_result() if you only need the side-effect"]
    pub fn result(self) -> Result<Self, Self> {
        if self.inner.has_errors() {
            Err(self)
        } else {
            Ok(self)
        }
    }

    /// Consume the wrapper and return the underlying [`ValidationReport`].
    #[must_use]
    pub fn into_inner(self) -> ValidationReport {
        self.inner
    }

    /// The Prüfidentifikator the message was validated against, if known.
    ///
    /// Returns `None` when validation was performed without a PID-specific rule
    /// pack (e.g. layer 1–3 structural checks only).
    #[must_use]
    pub fn pruefidentifikator(&self) -> Option<u32> {
        self.pruefidentifikator
    }

    /// The wire-format release code from the profile used during validation.
    ///
    /// Returns `None` when the profile was not resolved through the registry
    /// (e.g. when calling `validate_against` with an unknown release).
    #[must_use]
    pub fn release(&self) -> Option<&crate::release::Release> {
        self.release.as_ref()
    }

    /// The AHB revision identifier (e.g. `"3.2e"`, `"2.0h"`).
    ///
    /// May differ from the wire release code when BDEW publishes an AHB
    /// correction without a MIG change.  `None` when not tracked for this profile.
    #[must_use]
    pub fn ahb_revision(&self) -> Option<&'static str> {
        self.ahb_revision
    }

    /// Merge all issues from `other` into `self`.
    ///
    /// Useful when running multiple independent validation pipelines and
    /// combining their results into a single report.
    ///
    /// The PID of `self` is preserved; `other.pruefidentifikator` is ignored.
    pub fn merge(&mut self, other: Self) {
        self.inner.merge(other.inner);
    }
}

impl fmt::Display for EdiEnergyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} error(s), {} warning(s), {} info(s)",
            self.inner.errors().len(),
            self.inner.warnings().len(),
            self.inner.infos().len()
        )
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for EdiEnergyReport {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let errors = self.inner.errors();
        let warnings = self.inner.warnings();
        let infos = self.inner.infos();
        let pid = self.pruefidentifikator;
        // Count optional top-level fields to size the struct correctly.
        let opt_field_count = usize::from(pid.is_some())
            + usize::from(self.release.is_some())
            + usize::from(self.ahb_revision.is_some())
            + usize::from(self.interchange_header.is_some());
        let mut st = s.serialize_struct("EdiEnergyReport", 5 + opt_field_count)?;
        st.serialize_field("valid", &self.inner.is_valid())?;
        st.serialize_field(
            "errors",
            &errors
                .iter()
                .map(|i| ValidationIssueSummary::from_issue_with_pid(i, pid))
                .collect::<Vec<_>>(),
        )?;
        st.serialize_field(
            "warnings",
            &warnings
                .iter()
                .map(|i| ValidationIssueSummary::from_issue_with_pid(i, pid))
                .collect::<Vec<_>>(),
        )?;
        st.serialize_field(
            "infos",
            &infos
                .iter()
                .map(|i| ValidationIssueSummary::from_issue_with_pid(i, pid))
                .collect::<Vec<_>>(),
        )?;
        st.serialize_field(
            "totalIssues",
            &(errors.len() + warnings.len() + infos.len()),
        )?;
        if let Some(pid) = pid {
            st.serialize_field("pruefidentifikator", &pid)?;
        }
        if let Some(ref rel) = self.release {
            st.serialize_field("release", rel.as_str())?;
        }
        if let Some(rev) = self.ahb_revision {
            st.serialize_field("ahbRevision", rev)?;
        }
        if let Some(ref hdr) = self.interchange_header {
            st.serialize_field("interchangeHeader", hdr)?;
        }
        st.end()
    }
}
