// Segment occurrence indices (from `enumerate()`) are bounded by EDIFACT message
// limits (~10 000 segments max) and therefore fit safely in u16/u8.
#![allow(clippy::cast_possible_truncation)]

use std::sync::Arc;

use edifact_rs::{ProfileRulePack, ValidationIssue, ValidationSeverity};

/// A caller-supplied validation rule pack that can be merged on top of all
/// built-in validation layers when calling `EdiEnergyMessage::validate_with_pack`.
///
/// `CustomRulePack` insulates callers from the internal `edifact-rs`
/// `ProfileRulePack` type: no direct dependency on `edifact-rs` is required
/// to construct a `CustomRulePack`.
///
/// # Example
///
/// ```rust
/// use edi_energy::CustomRulePack;
///
/// let pack = CustomRulePack::new("my-business-rules")
///     .require_segment("STS")
///     .forbid_segment("CNT");
/// ```
#[must_use]
pub struct CustomRulePack(ProfileRulePack);

impl CustomRulePack {
    /// Create an empty rule pack with a human-readable name used in rule IDs.
    pub fn new(name: impl Into<String>) -> Self {
        Self(ProfileRulePack::new(name))
    }

    /// Add a rule that requires the given EDIFACT segment tag to be present at
    /// least once.  Emits an error-severity issue when the segment is absent.
    pub fn require_segment(mut self, tag: &'static str) -> Self {
        let rule_id: Arc<str> = format!("CUSTOM-{tag}-REQUIRED").into();
        let msg: Arc<str> = format!("required segment {tag} is missing").into();
        let rule_id_inner = Arc::clone(&rule_id);
        let msg_inner = Arc::clone(&msg);
        self.0 = self
            .0
            .with_named_stateless_rule_fn(rule_id, move |segs, issues| {
                if !segs.iter().any(|s| s.tag == tag) {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, (*msg_inner).to_owned())
                            .with_rule_id((*rule_id_inner).to_owned())
                            .with_segment(tag.to_owned()),
                    );
                }
            });
        self
    }

    /// Add a rule that forbids the given EDIFACT segment tag.
    /// Emits an error-severity issue when the segment is present.
    pub fn forbid_segment(mut self, tag: &'static str) -> Self {
        let rule_id: Arc<str> = format!("CUSTOM-{tag}-FORBIDDEN").into();
        let msg: Arc<str> = format!("segment {tag} must not appear").into();
        let rule_id_inner = Arc::clone(&rule_id);
        let msg_inner = Arc::clone(&msg);
        self.0 = self
            .0
            .with_named_stateless_rule_fn(rule_id, move |segs, issues| {
                for (occ, _seg) in segs.iter().enumerate().filter(|(_, s)| s.tag == tag) {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, (*msg_inner).to_owned())
                            .with_rule_id((*rule_id_inner).to_owned())
                            .with_segment(tag.to_owned())
                            .with_segment_occurrence(occ as u16),
                    );
                }
            });
        self
    }

    /// Add a rule that requires the given segment's first element (element 0,
    /// component 0) to be present and contain one of the `allowed` qualifier values.
    /// Emits an error-severity issue when the qualifier is absent or not in the set.
    pub fn require_qualifier(
        mut self,
        tag: &'static str,
        allowed: &'static [&'static str],
    ) -> Self {
        let rule_id: Arc<str> = format!("CUSTOM-{tag}-QUALIFIER").into();
        let rule_id_inner = Arc::clone(&rule_id);
        self.0 = self.0.with_named_stateless_rule_fn(rule_id, move |segs, issues| {
            for (occ, seg) in segs.iter().enumerate().filter(|(_, s)| s.tag == tag) {
                match seg.element_str(0) {
                    Some(q) if allowed.contains(&q) => {}
                    Some(_) => {
                        let msg = format!(
                            "segment {tag} element 0: qualifier is not in the allowed set ({} allowed value(s))",
                            allowed.len()
                        );
                        issues.push(
                            ValidationIssue::new(ValidationSeverity::Error, msg)
                                .with_rule_id((*rule_id_inner).to_owned())
                                .with_segment(tag.to_owned())
                                .with_segment_occurrence(occ as u16)
                                .with_element_index(0)
                                .with_component_index(0),
                        );
                    }
                    None => {
                        let msg = format!(
                            "segment {tag} is missing the qualifier (element 0)"
                        );
                        issues.push(
                            ValidationIssue::new(ValidationSeverity::Error, msg)
                                .with_rule_id((*rule_id_inner).to_owned())
                                .with_segment(tag.to_owned())
                                .with_segment_occurrence(occ as u16)
                                .with_element_index(0),
                        );
                    }
                }
            }
        });
        self
    }

    /// Convert into the internal `ProfileRulePack`.
    ///
    /// This is `pub(crate)` to keep the `edifact-rs` dependency internal.
    #[allow(dead_code)]
    pub(crate) fn into_inner(self) -> ProfileRulePack {
        self.0
    }

    /// Add a rule that is evaluated once per occurrence of the named segment group
    /// (e.g. `"SG2"`, `"SG5"`).
    ///
    /// The closure receives:
    /// - `occurrence` — 0-based index of this group occurrence within its parent
    ///   (first `SG5` = 0, second `SG5` = 1, …).
    /// - `segs` — the flat slice of [`edifact_rs::Segment`] values contained
    ///   within this group occurrence.  Only segments that belong to this
    ///   occurrence are included; no additional filtering is needed.
    /// - `issues` — append [`ValidationIssue`] values here to report violations.
    ///
    /// The `group_id` must match a group name defined in the message type's MIG
    /// (e.g. `"SG2"` for UTILMD, `"SG6"` for MSCONS).  If no group with that ID
    /// exists in the validated message, the rule is silently skipped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edi_energy::CustomRulePack;
    /// use edifact_rs::{ValidationIssue, ValidationSeverity};
    ///
    /// // Require that every SG5 occurrence in MSCONS contains at least one LOC.
    /// let pack = CustomRulePack::new("my-mscons-rules")
    ///     .add_group_rule("SG6", "MY-SG6-LOC-REQ", |_occ, segs, issues| {
    ///         if !segs.iter().any(|s| s.tag == "LOC") {
    ///             issues.push(
    ///                 ValidationIssue::new(
    ///                     ValidationSeverity::Error,
    ///                     "SG6 group is missing required LOC segment".to_owned(),
    ///                 )
    ///                 .with_rule_id("MY-SG6-LOC-REQ"),
    ///             );
    ///         }
    ///     });
    /// ```
    pub fn add_group_rule<F>(
        mut self,
        group_id: impl Into<std::sync::Arc<str>>,
        rule_id: impl Into<std::sync::Arc<str>>,
        rule: F,
    ) -> Self
    where
        F: Fn(usize, &[edifact_rs::Segment<'_>], &mut Vec<ValidationIssue>) + Send + Sync + 'static,
    {
        self.0 = self.0.with_scoped_group_rule_fn(
            group_id,
            rule_id,
            move |group, segs, _ctx, issues| {
                rule(group.occurrence_index, segs, issues);
            },
        );
        self
    }

    /// Add a rule that checks the value at a specific element and component position
    /// of the given segment tag.
    ///
    /// This is a generalisation of [`require_qualifier`][Self::require_qualifier] that
    /// works at any `(element_index, component_index)` position rather than only the
    /// first qualifier element.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edi_energy::CustomRulePack;
    ///
    /// // Require that CCI element 2 component 0 is one of the approved codes.
    /// let pack = CustomRulePack::new("my-rules")
    ///     .check_element("CCI", 2, 0, &["Z01", "Z02"]);
    /// ```
    pub fn check_element(
        mut self,
        tag: &'static str,
        element_index: usize,
        component_index: usize,
        allowed: &'static [&'static str],
    ) -> Self {
        let rule_id: Arc<str> =
            format!("CUSTOM-{tag}-E{element_index}C{component_index}-VALUE").into();
        let rule_id_inner = Arc::clone(&rule_id);
        self.0 = self.0.with_named_stateless_rule_fn(rule_id, move |segs, issues| {
            for (occ, seg) in segs.iter().enumerate().filter(|(_, s)| s.tag == tag) {
                let actual = seg.component_str(element_index, component_index);
                match actual {
                    Some(v) if allowed.contains(&v) => {}
                    Some(_) => {
                        let msg = format!(
                            "segment {tag} element {element_index} component {component_index}: \
                             value is not in the allowed set ({} allowed value(s))",
                            allowed.len()
                        );
                        issues.push(
                            ValidationIssue::new(ValidationSeverity::Error, msg)
                                .with_rule_id((*rule_id_inner).to_owned())
                                .with_segment(tag.to_owned())
                                .with_segment_occurrence(occ as u16)
                                .with_element_index(element_index as u8)
                                .with_component_index(component_index as u8),
                        );
                    }
                    None => {
                        let msg = format!(
                            "segment {tag} element {element_index} component {component_index} is absent"
                        );
                        issues.push(
                            ValidationIssue::new(ValidationSeverity::Error, msg)
                                .with_rule_id((*rule_id_inner).to_owned())
                                .with_segment(tag.to_owned())
                                .with_segment_occurrence(occ as u16)
                                .with_element_index(element_index as u8)
                                .with_component_index(component_index as u8),
                        );
                    }
                }
            }
        });
        self
    }

    /// Add a rule that validates the value at a specific element position using a
    /// caller-supplied predicate function.
    ///
    /// Use this to enforce format constraints that cannot be expressed as a fixed
    /// code list — for example OBIS code structure, GLN check-digit, or date format.
    ///
    /// The `description` string is included in the error message to describe the
    /// expected format (e.g. `"OBIS code (format: A-B:C.D.E*F)"`).
    ///
    /// # Example
    ///
    /// ```rust
    /// use edi_energy::CustomRulePack;
    ///
    /// // Require that PIA element 1 component 0 looks like an OBIS code.
    /// let pack = CustomRulePack::new("my-rules")
    ///     .check_format("PIA", 1, 0, |v| v.contains(':'), "OBIS code (must contain ':')");
    /// ```
    pub fn check_format<F>(
        mut self,
        tag: &'static str,
        element_index: usize,
        component_index: usize,
        validator: F,
        description: &'static str,
    ) -> Self
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        let rule_id: Arc<str> =
            format!("CUSTOM-{tag}-E{element_index}C{component_index}-FORMAT").into();
        let rule_id_inner = Arc::clone(&rule_id);
        self.0 = self.0.with_named_stateless_rule_fn(rule_id, move |segs, issues| {
            for (occ, seg) in segs.iter().enumerate().filter(|(_, s)| s.tag == tag) {
                if let Some(v) = seg.component_str(element_index, component_index) {
                    if !validator(v) {
                        let msg = format!(
                            "segment {tag} element {element_index} component {component_index}: \
                             value does not match expected format ({description})"
                        );
                        issues.push(
                            ValidationIssue::new(ValidationSeverity::Error, msg)
                                .with_rule_id((*rule_id_inner).to_owned())
                                .with_segment(tag.to_owned())
                                .with_segment_occurrence(occ as u16)
                                .with_element_index(element_index as u8)
                                .with_component_index(component_index as u8),
                        );
                    }
                }
            }
        });
        self
    }

    /// Add a rule that requires both `tag_a` and `tag_b` to be present together.
    ///
    /// Emits an error if `tag_a` is present but `tag_b` is absent, or vice versa.
    /// Use this for segments that must always appear in pairs (e.g. `CTA` and `COM`
    /// in contact-information patterns).
    ///
    /// # Example
    ///
    /// ```rust
    /// use edi_energy::CustomRulePack;
    ///
    /// let pack = CustomRulePack::new("my-rules")
    ///     .require_segment_combination("CTA", "COM");
    /// ```
    pub fn require_segment_combination(mut self, tag_a: &'static str, tag_b: &'static str) -> Self {
        let rule_id: Arc<str> = format!("CUSTOM-{tag_a}-{tag_b}-PAIR").into();
        let rule_id_inner = Arc::clone(&rule_id);
        self.0 = self.0.with_named_stateless_rule_fn(rule_id, move |segs, issues| {
            let has_a = segs.iter().any(|s| s.tag == tag_a);
            let has_b = segs.iter().any(|s| s.tag == tag_b);
            if has_a && !has_b {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("segment {tag_a} is present but its required companion {tag_b} is absent"),
                    )
                    .with_rule_id((*rule_id_inner).to_owned())
                    .with_segment(tag_b.to_owned()),
                );
            } else if has_b && !has_a {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!("segment {tag_b} is present but its required companion {tag_a} is absent"),
                    )
                    .with_rule_id((*rule_id_inner).to_owned())
                    .with_segment(tag_a.to_owned()),
                );
            }
        });
        self
    }
}

impl std::fmt::Debug for CustomRulePack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomRulePack").finish_non_exhaustive()
    }
}
