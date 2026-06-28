// Shared AHB validation helper functions used by every generated profile module.
//
// These functions are called from the per-PID rule closures that `cargo xtask
// codegen` emits in each `<type>_<release>.rs` file.  Keeping them here
// (rather than duplicating them in every profile file) eliminates ~4,000 lines
// of identical generated code and removes all `#[allow(dead_code)]` lint
// suppressions from the generated directory.
//
// **Do not edit by hand** — the function signatures are part of the code
// generation contract.  If a signature must change, update `emit_ahb_rule_pack`
// in `xtask/src/codegen.rs` to match, then run `cargo xtask codegen`.
//
// `#[allow(dead_code)]` on every helper: not every compiled feature combination
// calls every function (e.g. a profile with no SOLL segments never calls
// `ahb_check_soll`).  Item-level allows keep scope minimal while permitting the
// full helper set to compile under any feature subset.
// Segment occurrence indices are bounded by EDIFACT message limits and fit safely in u16/u8.
#![allow(clippy::cast_possible_truncation)]

use edifact_rs::{ValidationIssue, ValidationSeverity};

/// Require a segment to be present at least once.
///
/// Emits an `Error`-severity issue when `tag` is absent from `segments`.
#[inline]
#[allow(dead_code)]
pub(super) fn ahb_check_mandatory(
    segments: &[edifact_rs::Segment<'_>],
    tag: &str,
    rule_id: &str,
    msg: &str,
    pid: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if !segments.iter().any(|s| s.tag == tag) {
        issues.push(
            ValidationIssue::new(ValidationSeverity::Error, msg.to_owned())
                .with_rule_id(rule_id)
                .with_segment(tag.to_owned())
                .with_context_entry("pid", pid),
        );
    }
}

/// Require a segment to be present (Soll / "should") — emits `Warning`.
///
/// Unlike [`ahb_check_mandatory`], a missing segment is a warning, not an error.
#[inline]
#[allow(dead_code)]
pub(super) fn ahb_check_soll(
    segments: &[edifact_rs::Segment<'_>],
    tag: &str,
    rule_id: &str,
    msg: &str,
    pid: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if !segments.iter().any(|s| s.tag == tag) {
        issues.push(
            ValidationIssue::new(ValidationSeverity::Warning, msg.to_owned())
                .with_rule_id(rule_id)
                .with_segment(tag.to_owned())
                .with_context_entry("pid", pid),
        );
    }
}

/// Forbid a segment — emits `Error` for every occurrence of `tag`.
#[inline]
#[allow(dead_code)]
pub(super) fn ahb_check_not_used(
    segments: &[edifact_rs::Segment<'_>],
    tag: &str,
    rule_id: &str,
    msg: &str,
    pid: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if segments.iter().any(|s| s.tag == tag) {
        issues.push(
            ValidationIssue::new(ValidationSeverity::Error, msg.to_owned())
                .with_rule_id(rule_id)
                .with_segment(tag.to_owned())
                .with_context_entry("pid", pid),
        );
    }
}

/// Check that every occurrence of `tag` has an allowed qualifier (element 0, component 0).
///
/// `is_allowed` is a predicate that returns `true` for valid qualifier values.
/// Emits `Error` for every occurrence whose qualifier is absent or not allowed.
#[inline]
#[allow(dead_code)]
pub(super) fn ahb_check_qualifier(
    segments: &[edifact_rs::Segment<'_>],
    tag: &str,
    rule_id: &str,
    msg: &str,
    is_allowed: impl Fn(&str) -> bool,
    pid: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for (occ, seg) in segments.iter().filter(|s| s.tag == tag).enumerate() {
        if let Some(q) = seg.element_str(0) {
            if !is_allowed(q) {
                issues.push(
                    ValidationIssue::new(ValidationSeverity::Error, msg.to_owned())
                        .with_rule_id(rule_id)
                        .with_segment(tag.to_owned())
                        .with_segment_occurrence(occ as u16)
                        .with_element_index(0)
                        .with_component_index(0)
                        .with_context_entry("pid", pid),
                );
            }
        }
    }
}

/// Check a specific element value (not necessarily element 0) against a predicate.
///
/// Emits `Error` for every occurrence of `tag` where the value at `element_index`
/// is absent or rejected by `is_allowed`.
#[inline]
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(super) fn ahb_check_field_value(
    segments: &[edifact_rs::Segment<'_>],
    tag: &str,
    element_index: usize,
    rule_id: &str,
    msg: &str,
    is_allowed: impl Fn(&str) -> bool,
    pid: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for (occ, seg) in segments.iter().filter(|s| s.tag == tag).enumerate() {
        if let Some(v) = seg.element_str(element_index) {
            if !is_allowed(v) {
                issues.push(
                    ValidationIssue::new(ValidationSeverity::Error, msg.to_owned())
                        .with_rule_id(rule_id)
                        .with_segment(tag.to_owned())
                        .with_segment_occurrence(occ as u16)
                        .with_element_index(element_index as u8)
                        .with_component_index(0)
                        .with_context_entry("pid", pid),
                );
            }
        }
    }
}

/// Require at least one `tag` occurrence to carry a specific qualifier.
///
/// `is_required` returns `true` for the qualifier value(s) that must be present.
/// Emits `Error` when no occurrence of `tag` has a matching qualifier.
#[inline]
#[allow(dead_code)]
pub(super) fn ahb_check_required_qualifier(
    segments: &[edifact_rs::Segment<'_>],
    tag: &str,
    rule_id: &str,
    msg: &str,
    is_required: impl Fn(&str) -> bool,
    pid: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let found = segments
        .iter()
        .filter(|s| s.tag == tag)
        .any(|s| s.element_str(0).is_some_and(&is_required));
    if !found {
        issues.push(
            ValidationIssue::new(ValidationSeverity::Error, msg.to_owned())
                .with_rule_id(rule_id)
                .with_segment(tag.to_owned())
                .with_context_entry("pid", pid),
        );
    }
}

/// Conditional check: run `check_fn` only when `condition_fn` returns `true`.
///
/// Both predicates receive the full segment slice.  Used for AHB rules that
/// apply only when a specific qualifier or value is present elsewhere in the
/// message.
#[inline]
#[allow(dead_code)]
pub(super) fn ahb_check_conditional(
    segments: &[edifact_rs::Segment<'_>],
    condition_fn: impl Fn(&[edifact_rs::Segment<'_>]) -> bool,
    check_fn: impl Fn(&[edifact_rs::Segment<'_>], &mut Vec<ValidationIssue>),
    issues: &mut Vec<ValidationIssue>,
) {
    if condition_fn(segments) {
        check_fn(segments, issues);
    }
}
