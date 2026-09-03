//! The MIG and AHB checks over a resolved message.
//!
//! **MIG** (rule ids `MIG-…`): every segment sits at a place of the
//! Nachrichtenstruktur, in order; mandatory places are filled and none is
//! repeated beyond its maximum; each data element keeps its BDEW status
//! (`M`/`R` present, `N` absent), its representation (`an..35`, `n11`) and,
//! where the MIG lists codes, one of them.
//!
//! **AHB** (rule ids `AHB-<pid>-…`): the Prüfschablone of the message's
//! Anwendungsfall — a segment or group with no status in that column is not
//! to be used; one marked `Muss` (unconditionally, or under a Voraussetzung
//! the message satisfies) must be present; a data element takes only the
//! codes the column marks. Allgemeine Festlegungen 6.1d Kap. 6 gives the
//! reading; [`super::conditions`] the Bedingungen.

use std::collections::HashSet;

use edifact_rs::{Segment, ValidationIssue, ValidationSeverity};

use super::Profile;
use super::conditions::{ConditionKind, Scope, Status, Truth, Voraussetzung};
use super::model::{Anwendungsfall, Element, ElementRule, SegmentNode};
use super::structure::{InstanceId, Kind, NodeId, Resolution, Structure};

/// Rule id of the advisory raised when no Anwendungsfall can be selected.
pub const AHB_SKIP_NO_PID: &str = "AHB-SKIP-NO-PID";
/// Rule id of the warning raised for a Prüfidentifikator the profile does not
/// carry.
pub const AHB_UNKNOWN_PID: &str = "AHB-UNKNOWN-PID";

/// Run every MIG and AHB check. `segments` is the message from `UNH` to `UNT`.
#[must_use]
pub fn validate(
    profile: &Profile,
    segments: &[Segment<'_>],
    pid: Option<u32>,
) -> Vec<ValidationIssue> {
    let structure = &profile.structure;
    let res = structure.resolve(segments);
    let mut issues = Vec::new();
    let selected = select_anwendungsfall(profile, segments, pid);
    let af = match &selected {
        Selected::Some(af) => Some(*af),
        _ => None,
    };
    mig_checks(structure, &res, segments, af, &mut issues);
    match selected {
        Selected::Some(af) => ahb_checks(profile, af, &res, segments, &mut issues),
        Selected::UnknownPid(p) => issues.push(
            ValidationIssue::new(
                ValidationSeverity::Warning,
                format!(
                    "Prüfidentifikator {p} is not an Anwendungsfall of {} {} — AHB rules were not applied",
                    profile.mig.message_type, profile.mig.release
                ),
            )
            .with_rule_id(AHB_UNKNOWN_PID)
            .with_context_entry("pid", p.to_string()),
        ),
        Selected::None => match best_fit(profile, &res, segments) {
            Some(af) => ahb_checks(profile, af, &res, segments, &mut issues),
            None => issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Warning,
                    "no Anwendungsfall could be selected: the message carries no Prüfidentifikator and its BGM matches no column — AHB rules were not applied",
                )
                .with_rule_id(AHB_SKIP_NO_PID),
            ),
        },
    }
    issues
}

enum Selected<'p> {
    Some(&'p Anwendungsfall),
    UnknownPid(u32),
    None,
}

/// The column to check against: by Prüfidentifikator, or — for message types
/// published without one (APERAK, CONTRL) — by the `BGM` DE 1001 the columns
/// admit.
/// How a column is named in rule ids: its Prüfidentifikator, or `col<n>`
/// for a message type published without.
fn column_key(profile: &Profile, af: &Anwendungsfall) -> String {
    af.pid.map_or_else(
        || {
            let index = profile
                .ahb
                .anwendungsfaelle
                .iter()
                .position(|a| std::ptr::eq(a, af))
                .unwrap_or(0);
            format!("col{}", index + 1)
        },
        |p| p.to_string(),
    )
}

fn select_anwendungsfall<'p>(
    profile: &'p Profile,
    segments: &[Segment<'_>],
    pid: Option<u32>,
) -> Selected<'p> {
    if let Some(p) = pid {
        return match profile.anwendungsfall(p) {
            Some(af) => Selected::Some(af),
            None if profile.mig.pid_exempt
                || profile.ahb.anwendungsfaelle.iter().all(|a| a.pid.is_none()) =>
            {
                by_bgm(profile, segments)
            }
            None => Selected::UnknownPid(p),
        };
    }
    by_bgm(profile, segments)
}

fn by_bgm<'p>(profile: &'p Profile, segments: &[Segment<'_>]) -> Selected<'p> {
    let bgm_code = segments
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(0, 0))
        .unwrap_or("");
    let bgm_nr = profile
        .structure
        .layouts
        .iter()
        .find(|l| l.tag == "BGM")
        .map(|l| l.nr.clone())
        .unwrap_or_default();
    let candidates: Vec<&Anwendungsfall> = profile
        .ahb
        .anwendungsfaelle
        .iter()
        .filter(|af| {
            af.element_rules(&bgm_nr)
                .filter(|e| e.de == "1001")
                .any(|e| {
                    e.operands
                        .iter()
                        .any(|o| o.code.as_deref() == Some(bgm_code))
                })
        })
        .collect();
    match candidates.as_slice() {
        [one] => Selected::Some(one),
        [] if profile.ahb.anwendungsfaelle.len() == 1 => {
            Selected::Some(&profile.ahb.anwendungsfaelle[0])
        }
        _ => Selected::None,
    }
}

/// For a message type whose columns carry neither a Prüfidentifikator nor
/// a `BGM` (CONTRL: Empfangsbestätigung, Syntaxfehler in der Übertragung,
/// Syntaxfehler in der Nachricht), the column the message fits best — the
/// one with the fewest findings; the first on a tie.
fn best_fit<'p>(
    profile: &'p Profile,
    res: &Resolution,
    segments: &[Segment<'_>],
) -> Option<&'p Anwendungsfall> {
    if !profile.mig.pid_exempt && profile.ahb.anwendungsfaelle.iter().any(|a| a.pid.is_some()) {
        return None;
    }
    profile
        .ahb
        .anwendungsfaelle
        .iter()
        .map(|af| {
            let mut found = Vec::new();
            ahb_checks(profile, af, res, segments, &mut found);
            (
                found
                    .iter()
                    .filter(|i| i.severity == ValidationSeverity::Error)
                    .count(),
                af,
            )
        })
        .min_by_key(|(n, _)| *n)
        .map(|(_, af)| af)
}

// ── MIG ───────────────────────────────────────────────────────────────────────

/// The MIG's own checks. Its `R` (BDEW-erforderlich) is the union over all
/// Anwendungsfälle: once a column is selected, an `R` place or data element
/// the column does not list is not to be used and is not demanded here;
/// `M` (UN/EDIFACT-mandatory) stands regardless.
// The walk over instances and their children is one pass; splitting it would
// duplicate the per-node bookkeeping.
#[allow(clippy::too_many_lines)]
fn mig_checks(
    structure: &Structure,
    res: &Resolution,
    segments: &[Segment<'_>],
    af: Option<&Anwendungsfall>,
    issues: &mut Vec<ValidationIssue>,
) {
    for &i in &res.unresolved {
        let seg = &segments[i];
        let expected: Vec<String> = candidates_for(structure, &seg.tag);
        let hint = if expected.is_empty() {
            format!("the MIG defines no place for a {} segment", seg.tag)
        } else {
            format!("the MIG's {} places are {}", seg.tag, expected.join(", "))
        };
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "segment {} (message segment {i}) fits no place of the Nachrichtenstruktur from here on — out of order, in a wrong group, or with qualifiers no place admits; {hint}",
                    seg.tag
                ),
            )
            .with_rule_id("MIG-STRUCTURE")
            .with_segment(seg.tag.to_string())
            .with_segment_occurrence(u16::try_from(i).unwrap_or(u16::MAX))
            .with_span(seg.span),
        );
    }

    for (inst_id, inst) in res.instances.iter().enumerate() {
        let children: &[NodeId] = match inst.node {
            Some(n) => &structure.nodes[n].children,
            None => &structure.root,
        };
        for &child in children {
            let node = &structure.nodes[child];
            match &node.kind {
                Kind::Segment { nr, tag, .. } => {
                    let count = res.count(inst_id, child);
                    let required = node.status == "M"
                        || (node.status == "R"
                            && af.is_none_or(|a| a.segment_status(nr).is_some()));
                    if required && count == 0 {
                        issues.push(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "segment {tag} „{}“ (Nr {nr}) is mandatory in the MIG{} but missing",
                                    node.name,
                                    in_group(structure, inst.node)
                                ),
                            )
                            .with_rule_id(format!("MIG-{nr}-{tag}-REQUIRED"))
                            .with_segment(tag.clone())
                            .with_context_entry("nr", nr.clone()),
                        );
                    }
                    if node.max > 0 && count > node.max as usize {
                        issues.push(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "segment {tag} „{}“ (Nr {nr}) occurs {count} times{}; the MIG allows {}",
                                    node.name,
                                    in_group(structure, inst.node),
                                    node.max
                                ),
                            )
                            .with_rule_id(format!("MIG-{nr}-{tag}-MAX"))
                            .with_segment(tag.clone()),
                        );
                    }
                }
                Kind::Group { group } => {
                    let count = res.group_count(inst_id, child);
                    let trigger = structure.trigger(child);
                    let trigger_nr = trigger.and_then(|t| structure.nr(t)).unwrap_or("?");
                    let required = node.status == "M"
                        || (node.status == "R"
                            && af.is_none_or(|a| {
                                a.group_status(group, trigger_nr)
                                    .or_else(|| a.segment_status(trigger_nr))
                                    .is_some()
                            }));
                    if required && count == 0 {
                        issues.push(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "segment group {group} „{}“ (Nr {trigger_nr}) is mandatory in the MIG{} but missing",
                                    node.name,
                                    in_group(structure, inst.node)
                                ),
                            )
                            .with_rule_id(format!("MIG-{group}-{trigger_nr}-REQUIRED"))
                            .with_segment_group(group.clone())
                            .with_context_entry("nr", trigger_nr.to_owned()),
                        );
                    }
                    if node.max > 0 && count > node.max as usize {
                        issues.push(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "segment group {group} „{}“ (Nr {trigger_nr}) occurs {count} times{}; the MIG allows {}",
                                    node.name,
                                    in_group(structure, inst.node),
                                    node.max
                                ),
                            )
                            .with_rule_id(format!("MIG-{group}-{trigger_nr}-MAX"))
                            .with_segment_group(group.clone()),
                        );
                    }
                }
            }
        }
    }

    for (i, seg) in segments.iter().enumerate() {
        let Some(a) = res.assigned[i] else { continue };
        let Some(layout) = structure.layout(a.node) else {
            continue;
        };
        element_checks(layout, seg, i, af, issues);
    }
}

fn in_group(structure: &Structure, node: Option<NodeId>) -> String {
    match node {
        Some(n) => format!(" in {}", structure.group(n).unwrap_or("?")),
        None => String::new(),
    }
}

fn candidates_for(structure: &Structure, tag: &str) -> Vec<String> {
    structure
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            Kind::Segment {
                nr,
                tag: t,
                discriminators,
                ..
            } if t == tag => {
                let codes: Vec<&str> = discriminators
                    .first()
                    .map(|d| d.codes.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                Some(if codes.is_empty() {
                    format!("{nr} „{}“", n.name)
                } else {
                    format!("{nr} „{}“ ({}+{})", n.name, tag, codes.join("/"))
                })
            }
            _ => None,
        })
        .take(12)
        .collect()
}

/// Data-element checks of one segment against its layout.
#[allow(clippy::too_many_lines)]
fn element_checks(
    layout: &SegmentNode,
    seg: &Segment<'_>,
    index: usize,
    af: Option<&Anwendungsfall>,
    issues: &mut Vec<ValidationIssue>,
) {
    let nr = &layout.nr;
    let tag = &layout.tag;
    // Whether the column lists a data element (a composite: any component).
    let listed = |el: &Element| {
        af.is_none_or(|a| {
            a.element_rules(nr)
                .any(|r| r.de == el.id || el.components.iter().any(|c| c.id == r.de))
        })
    };
    let mut checked_positions = 0usize;
    for (ei, el) in layout.elements.iter().enumerate() {
        checked_positions = ei + 1;
        if el.components.is_empty() {
            check_leaf(el, seg, ei, 0, nr, tag, index, true, listed(el), issues);
        } else {
            // A composite's own status: R/M means at least its first
            // component is filled.
            let present = (0..el.components.len())
                .any(|ci| seg.component_str(ei, ci).is_some_and(|v| !v.is_empty()));
            let required = el.status == "M" || (el.status == "R" && listed(el));
            if required && !present {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!(
                            "{tag} (Nr {nr}): composite {} „{}“ is mandatory in the MIG but empty",
                            el.id, el.name
                        ),
                    )
                    .with_rule_id(format!("MIG-{nr}-{tag}-{}-REQUIRED", el.id))
                    .with_segment(tag.clone())
                    .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX))
                    .with_element_index(u8::try_from(ei).unwrap_or(u8::MAX)),
                );
            }
            if el.status == "N" && present {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!(
                            "{tag} (Nr {nr}): composite {} „{}“ is not used in the MIG but filled",
                            el.id, el.name
                        ),
                    )
                    .with_rule_id(format!("MIG-{nr}-{tag}-{}-NOTUSED", el.id))
                    .with_segment(tag.clone())
                    .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX))
                    .with_element_index(u8::try_from(ei).unwrap_or(u8::MAX)),
                );
            }
            // A component's `M` binds only once its composite is used: an
            // absent `D` composite leaves its mandatory components empty.
            for (ci, comp) in el.components.iter().enumerate() {
                check_leaf(
                    comp,
                    seg,
                    ei,
                    ci,
                    nr,
                    tag,
                    index,
                    present,
                    listed(comp),
                    issues,
                );
            }
            // Components beyond the layout.
            if let Some(element) = seg.get_element(ei)
                && element.components().count() > el.components.len()
                && element
                    .components()
                    .skip(el.components.len())
                    .any(|c| !c.is_empty())
            {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!(
                            "{tag} (Nr {nr}): composite {} carries more components than the MIG defines ({})",
                            el.id,
                            el.components.len()
                        ),
                    )
                    .with_rule_id(format!("MIG-{nr}-{tag}-{}-EXTRA", el.id))
                    .with_segment(tag.clone())
                    .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX))
                    .with_element_index(u8::try_from(ei).unwrap_or(u8::MAX)),
                );
            }
        }
    }
    if seg.elements.len() > checked_positions
        && seg.elements[checked_positions..]
            .iter()
            .any(|e| e.components().any(|c| !c.is_empty()))
    {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "{tag} (Nr {nr}): {} elements on the wire, the MIG defines {}",
                    seg.elements.len(),
                    layout.elements.len()
                ),
            )
            .with_rule_id(format!("MIG-{nr}-{tag}-EXTRA"))
            .with_segment(tag.clone())
            .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX)),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn check_leaf(
    el: &Element,
    seg: &Segment<'_>,
    ei: usize,
    ci: usize,
    nr: &str,
    tag: &str,
    index: usize,
    composite_used: bool,
    listed: bool,
    issues: &mut Vec<ValidationIssue>,
) {
    let value = seg.component_str(ei, ci).unwrap_or("");
    let at = |issue: ValidationIssue| {
        issue
            .with_context_entry("nr", nr.to_owned())
            .with_context_entry("de", el.id.clone())
            .with_segment(tag.to_owned())
            .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX))
            .with_element_index(u8::try_from(ei).unwrap_or(u8::MAX))
            .with_component_index(u8::try_from(ci).unwrap_or(u8::MAX))
    };
    if value.is_empty() {
        if composite_used && (el.status == "M" || (el.status == "R" && listed)) {
            issues.push(at(ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "{tag} (Nr {nr}): DE {} „{}“ is mandatory in the MIG but empty",
                    el.id, el.name
                ),
            )
            .with_rule_id(format!("MIG-{nr}-{tag}-{}-REQUIRED", el.id))));
        }
        return;
    }
    if el.status == "N" {
        issues.push(at(ValidationIssue::new(
            ValidationSeverity::Error,
            format!(
                "{tag} (Nr {nr}): DE {} „{}“ is not used in the MIG but carries {value:?}",
                el.id, el.name
            ),
        )
        .with_rule_id(format!("MIG-{nr}-{tag}-{}-NOTUSED", el.id))));
        return;
    }
    if let Some(format) = &el.format
        && let Some(problem) = format_problem(format, value)
    {
        issues.push(at(ValidationIssue::new(
            ValidationSeverity::Error,
            format!(
                "{tag} (Nr {nr}): DE {} „{}“ is {format} in the MIG; {value:?} {problem}",
                el.id, el.name
            ),
        )
        .with_rule_id(format!("MIG-{nr}-{tag}-{}-FORMAT", el.id))));
    }
    if el.is_code_list() && !el.codes.iter().any(|c| c.code == value) {
        let admitted: Vec<&str> = el.codes.iter().map(|c| c.code.as_str()).collect();
        issues.push(at(ValidationIssue::new(
            ValidationSeverity::Error,
            format!(
                "{tag} (Nr {nr}): DE {} „{}“ is {value:?}; the MIG admits {}",
                el.id,
                el.name,
                admitted.join(", ")
            ),
        )
        .with_rule_id(format!("MIG-{nr}-{tag}-{}-CODE", el.id))));
    }
}

/// Check `value` against a representation such as `an..35`, `n11`, `a1`.
fn format_problem(format: &str, value: &str) -> Option<String> {
    let (kind, rest) = if let Some(r) = format.strip_prefix("an") {
        ("an", r)
    } else if let Some(r) = format.strip_prefix('a') {
        ("a", r)
    } else if let Some(r) = format.strip_prefix('n') {
        ("n", r)
    } else {
        return None;
    };
    let (variable, len) = match rest.strip_prefix("..") {
        Some(l) => (true, l),
        None => (false, rest),
    };
    let len: usize = len.parse().ok()?;
    let count = if kind == "n" {
        value.chars().filter(char::is_ascii_digit).count()
    } else {
        value.chars().count()
    };
    if kind == "n"
        && !value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | ',' | 'E' | 'e'))
    {
        return Some("is not numeric".into());
    }
    if kind == "a" && value.chars().any(|c| c.is_ascii_digit()) {
        return Some("contains digits".into());
    }
    if count > len {
        return Some(format!("is {count} characters long"));
    }
    if !variable && count != len {
        return Some(format!("is {count} characters long, not {len}"));
    }
    None
}

// ── AHB ───────────────────────────────────────────────────────────────────────

/// What a column says about a place, once its conditions are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// `Muss` — unconditionally or under a Voraussetzung the message meets.
    Required,
    /// May be present.
    Optional,
    /// Every status is a `Muss` whose Voraussetzung the message does not meet.
    Forbidden,
}

struct Ctx<'a, 'd> {
    structure: &'a Structure,
    res: &'a Resolution,
    segments: &'a [Segment<'d>],
    conditions: &'a std::collections::BTreeMap<String, String>,
}

impl Ctx<'_, '_> {
    /// The value of Bedingung `id` for a row evaluated inside `instance`.
    fn truth(&self, id: &str, instance: InstanceId) -> Truth {
        match ConditionKind::of(id) {
            ConditionKind::Voraussetzung => {}
            ConditionKind::Paket => return Truth::Unknown,
            _ => return Truth::Neutral,
        }
        let Some(text) = self.conditions.get(id) else {
            return Truth::Unknown;
        };
        // A numbered Bedingung that states a constraint rather than a
        // precondition („Innerhalb eines SG4 IDE müssen alle DE1131 … den
        // identischen Wert enthalten") does not gate the place.
        if !super::conditions::is_precondition(text) {
            return Truth::Neutral;
        }
        let Some(v) = Voraussetzung::parse(text) else {
            return Truth::Unknown;
        };
        let range = |scope: &Scope| -> std::ops::Range<usize> {
            match scope {
                Scope::Message => 0..self.segments.len(),
                Scope::Group(g) => match self.res.enclosing(self.structure, instance, g) {
                    Some(i) => self.res.instances[i].first..self.res.instances[i].last,
                    None => 0..self.segments.len(),
                },
            }
        };
        match v {
            Voraussetzung::Present {
                scope,
                pattern,
                negate,
            } => {
                let found = self.segments[range(&scope)]
                    .iter()
                    .any(|s| pattern.matches(s));
                Truth::from(found != negate)
            }
            Voraussetzung::Count {
                scope,
                pattern,
                more_than,
            } => {
                let n = self.segments[range(&scope)]
                    .iter()
                    .filter(|s| pattern.matches(s))
                    .count();
                Truth::from(n > more_than)
            }
            Voraussetzung::ElementValue {
                scope,
                tag,
                de,
                value,
                negate,
                suffix,
            } => {
                let r = range(&scope);
                let found = (r.start..r.end).any(|i| {
                    let seg = &self.segments[i];
                    if seg.tag != tag {
                        return false;
                    }
                    let Some(a) = self.res.assigned[i] else {
                        return false;
                    };
                    let Some(layout) = self.structure.layout(a.node) else {
                        return false;
                    };
                    layout.locate(&de, 0).is_some_and(|(ei, ci, _)| {
                        seg.component_str(ei, ci).is_some_and(|v| {
                            if suffix {
                                v.len() >= 2 && v.ends_with(value.as_str())
                            } else {
                                v == value
                            }
                        })
                    })
                });
                Truth::from(found != negate)
            }
        }
    }

    fn requirement(&self, statuses: &[String], instance: InstanceId) -> Requirement {
        let mut required = false;
        let mut permitted = false;
        let mut decided = false;
        for text in statuses {
            let Some(status) = Status::parse(text) else {
                // An unreadable status: never a ground for rejection.
                permitted = true;
                continue;
            };
            if !status.kind.is_receiver_checkable() {
                permitted = true;
                continue;
            }
            let truth = status
                .expr
                .as_ref()
                .map_or(Truth::True, |e| e.eval(&mut |id| self.truth(id, instance)));
            match truth {
                Truth::True | Truth::Neutral => {
                    required = true;
                    permitted = true;
                    decided = true;
                }
                Truth::False => decided = true,
                Truth::Unknown => permitted = true,
            }
        }
        if required {
            Requirement::Required
        } else if permitted || !decided {
            Requirement::Optional
        } else {
            Requirement::Forbidden
        }
    }
}

impl From<bool> for Truth {
    fn from(b: bool) -> Self {
        if b { Truth::True } else { Truth::False }
    }
}

#[allow(clippy::too_many_lines)]
fn ahb_checks(
    profile: &Profile,
    af: &Anwendungsfall,
    res: &Resolution,
    segments: &[Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let structure = &profile.structure;
    let ctx = Ctx {
        structure,
        res,
        segments,
        conditions: &profile.ahb.conditions,
    };
    let pid = column_key(profile, af);
    let tag_issue = |issue: ValidationIssue| issue.with_context_entry("pid", pid.clone());

    for (inst_id, inst) in res.instances.iter().enumerate() {
        let children: &[NodeId] = match inst.node {
            Some(n) => &structure.nodes[n].children,
            None => &structure.root,
        };
        for &child in children {
            let node = &structure.nodes[child];
            match &node.kind {
                Kind::Segment { nr, tag, .. } => {
                    let count = res.count(inst_id, child);
                    match af.segment_status(nr) {
                        None => {
                            if count > 0 {
                                issues.push(tag_issue(
                                    ValidationIssue::new(
                                        ValidationSeverity::Error,
                                        format!(
                                            "{tag} „{}“ (Nr {nr}) is not part of the Prüfschablone of {pid}{}",
                                            node.name,
                                            in_group(structure, inst.node)
                                        ),
                                    )
                                    .with_rule_id(format!("AHB-{pid}-{nr}-{tag}-NOT-PERMITTED"))
                                    .with_segment(tag.clone())
                                    .with_context_entry("nr", nr.clone()),
                                ));
                            }
                        }
                        Some(statuses) => match ctx.requirement(statuses, inst_id) {
                            Requirement::Required if count == 0 => issues.push(tag_issue(
                                ValidationIssue::new(
                                    ValidationSeverity::Error,
                                    format!(
                                        "{tag} „{}“ (Nr {nr}) is Muss for {pid}{} but missing — AHB status {}",
                                        node.name,
                                        in_group(structure, inst.node),
                                        statuses.join(" | ")
                                    ),
                                )
                                .with_rule_id(format!("AHB-{pid}-{nr}-{tag}-MISSING"))
                                .with_segment(tag.clone())
                                .with_context_entry("nr", nr.clone()),
                            )),
                            Requirement::Forbidden if count > 0 => issues.push(tag_issue(
                                ValidationIssue::new(
                                    ValidationSeverity::Error,
                                    format!(
                                        "{tag} „{}“ (Nr {nr}) is present but its Voraussetzung for {pid} is not met — AHB status {}",
                                        node.name,
                                        statuses.join(" | ")
                                    ),
                                )
                                .with_rule_id(format!("AHB-{pid}-{nr}-{tag}-NOT-PERMITTED"))
                                .with_segment(tag.clone())
                                .with_context_entry("nr", nr.clone()),
                            )),
                            _ => {}
                        },
                    }
                }
                Kind::Group { group } => {
                    let count = res.group_count(inst_id, child);
                    let Some(trigger) = structure.trigger(child) else {
                        continue;
                    };
                    let Some(trigger_nr) = structure.nr(trigger) else {
                        continue;
                    };
                    let statuses = af
                        .group_status(group, trigger_nr)
                        .or_else(|| af.segment_status(trigger_nr));
                    let Some(statuses) = statuses else { continue };
                    match ctx.requirement(statuses, inst_id) {
                        Requirement::Required if count == 0 => issues.push(tag_issue(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "segment group {group} „{}“ (Nr {trigger_nr}) is Muss for {pid}{} but missing — AHB status {}",
                                    node.name,
                                    in_group(structure, inst.node),
                                    statuses.join(" | ")
                                ),
                            )
                            .with_rule_id(format!("AHB-{pid}-{group}-{trigger_nr}-MISSING"))
                            .with_segment_group(group.clone())
                            .with_context_entry("nr", trigger_nr.to_owned()),
                        )),
                        Requirement::Forbidden if count > 0 => issues.push(tag_issue(
                            ValidationIssue::new(
                                ValidationSeverity::Error,
                                format!(
                                    "segment group {group} „{}“ (Nr {trigger_nr}) is present but its Voraussetzung for {pid} is not met — AHB status {}",
                                    node.name,
                                    statuses.join(" | ")
                                ),
                            )
                            .with_rule_id(format!("AHB-{pid}-{group}-{trigger_nr}-NOT-PERMITTED"))
                            .with_segment_group(group.clone())
                            .with_context_entry("nr", trigger_nr.to_owned()),
                        )),
                        _ => {}
                    }
                }
            }
        }
    }

    for (i, seg) in segments.iter().enumerate() {
        let Some(a) = res.assigned[i] else { continue };
        let Some(layout) = structure.layout(a.node) else {
            continue;
        };
        if af.segment_status(&layout.nr).is_none() {
            continue;
        }
        element_rules(&ctx, af, &pid, layout, seg, i, a.instance, issues);
    }
}

#[allow(clippy::too_many_arguments)]
fn element_rules(
    ctx: &Ctx<'_, '_>,
    af: &Anwendungsfall,
    pid: &str,
    layout: &SegmentNode,
    seg: &Segment<'_>,
    index: usize,
    instance: InstanceId,
    issues: &mut Vec<ValidationIssue>,
) {
    let nr = &layout.nr;
    let tag = &layout.tag;
    let mut ruled: HashSet<(usize, usize)> = HashSet::new();
    for rule in af.element_rules(nr) {
        let Some((ei, ci, el)) = layout.locate(&rule.de, rule.occurrence) else {
            continue;
        };
        ruled.insert((ei, ci));
        let value = seg.component_str(ei, ci).unwrap_or("");
        let (required, mut admitted, mut coded) = operands(ctx, rule, instance);
        // An operand on the element rather than on its codes (`X` beside
        // `UNH` DE 0065) admits what the MIG's code list admits.
        if !coded && el.is_code_list() {
            coded = true;
            admitted = el.codes.iter().map(|c| c.code.clone()).collect();
        }
        let at = |issue: ValidationIssue| {
            issue
                .with_context_entry("pid", pid.to_owned())
                .with_context_entry("nr", nr.to_owned())
                .with_context_entry("de", el.id.clone())
                .with_segment(tag.clone())
                .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX))
                .with_element_index(u8::try_from(ei).unwrap_or(u8::MAX))
                .with_component_index(u8::try_from(ci).unwrap_or(u8::MAX))
        };
        if value.is_empty() {
            if required {
                issues.push(at(ValidationIssue::new(
                    ValidationSeverity::Error,
                    format!(
                        "{tag} (Nr {nr}): DE {} „{}“ is required by the Prüfschablone of {pid} but empty{}",
                        el.id,
                        el.name,
                        if coded { format!(" — admitted: {}", admitted.join(", ")) } else { String::new() }
                    ),
                )
                .with_rule_id(format!("AHB-{pid}-{nr}-{tag}-{}-MISSING", el.id))));
            }
            continue;
        }
        if coded && !admitted.iter().any(|c| c == value) {
            issues.push(at(ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "{tag} (Nr {nr}): DE {} „{}“ is {value:?}; the Prüfschablone of {pid} admits {}",
                    el.id,
                    el.name,
                    if admitted.is_empty() { "no code here".to_owned() } else { admitted.join(", ") }
                ),
            )
            .with_rule_id(format!("AHB-{pid}-{nr}-{tag}-{}-CODE", el.id))));
        }
    }
    // A data element the column does not list is not to be used.
    for (ei, ci, el) in layout.leaves() {
        if ruled.contains(&(ei, ci)) || el.status == "N" {
            continue;
        }
        let value = seg.component_str(ei, ci).unwrap_or("");
        if value.is_empty() {
            continue;
        }
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "{tag} (Nr {nr}): DE {} „{}“ carries {value:?} but is not part of the Prüfschablone of {pid}",
                    el.id, el.name
                ),
            )
            .with_rule_id(format!("AHB-{pid}-{nr}-{tag}-{}-NOT-PERMITTED", el.id))
            .with_context_entry("pid", pid.to_owned())
            .with_context_entry("nr", nr.to_owned())
            .with_context_entry("de", el.id.clone())
            .with_segment(tag.clone())
            .with_segment_occurrence(u16::try_from(index).unwrap_or(u16::MAX))
            .with_element_index(u8::try_from(ei).unwrap_or(u8::MAX))
            .with_component_index(u8::try_from(ci).unwrap_or(u8::MAX)),
        );
    }
}

/// Read a data element's operands: whether a value is required, which codes
/// are admitted, and whether the element is coded at all.
fn operands(
    ctx: &Ctx<'_, '_>,
    rule: &ElementRule,
    instance: InstanceId,
) -> (bool, Vec<String>, bool) {
    let mut required = false;
    let mut admitted: Vec<String> = Vec::new();
    let mut coded = false;
    for op in &rule.operands {
        let Some(status) = Status::parse(&op.operand) else {
            // Unreadable: admit, never require.
            if let Some(c) = &op.code {
                coded = true;
                admitted.push(c.clone());
            }
            continue;
        };
        let truth = status
            .expr
            .as_ref()
            .map_or(Truth::True, |e| e.eval(&mut |id| ctx.truth(id, instance)));
        let this_required =
            status.kind.is_receiver_checkable() && matches!(truth, Truth::True | Truth::Neutral);
        let this_admitted = !(status.kind.is_receiver_checkable() && truth == Truth::False);
        match &op.code {
            Some(c) => {
                coded = true;
                if this_admitted {
                    admitted.push(c.clone());
                }
                if this_required {
                    required = true;
                }
            }
            None => {
                if this_required {
                    required = true;
                }
            }
        }
    }
    (required, admitted, coded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representations() {
        assert_eq!(format_problem("an..35", "9900357000004"), None);
        assert!(format_problem("an..3", "ABCD").is_some());
        assert_eq!(format_problem("n11", "51238696781"), None);
        assert!(format_problem("n11", "5123869678").is_some());
        assert!(format_problem("n..6", "12a").is_some());
        assert_eq!(format_problem("a1", "C"), None);
        assert!(format_problem("a1", "1").is_some());
    }
}
