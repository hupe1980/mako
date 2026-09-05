//! A minimal message that satisfies a Prüfschablone.
//!
//! [`Profile::skeleton`] walks the Nachrichtenstruktur and emits every place
//! the Anwendungsfall marks `Muss` without a Voraussetzung, filling each
//! data element the column lists with its first admitted code or a synthetic
//! value of the right shape. The result validates against the same profile —
//! which is what makes it a witness of the extraction and the validator, a
//! starting point for a real message, and the skeleton `cargo xtask
//! generate-fixtures` writes.

use std::collections::{HashMap, HashSet};

use edifact_rs::{Element as WireElement, OwnedSegment, Segment};

use super::Profile;
use super::conditions::{Status, Truth};
use super::model::{Anwendungsfall, Element, ElementRule, SegmentNode};
use super::structure::{Kind, NodeId};

/// The identities a skeleton is addressed with.
#[derive(Debug, Clone)]
pub struct SkeletonParties {
    /// `NAD+MS` / `UNB` sender MP-ID.
    pub sender: String,
    /// `NAD+MR` / `UNB` receiver MP-ID.
    pub receiver: String,
}

impl Default for SkeletonParties {
    fn default() -> Self {
        Self {
            sender: "9900357000004".into(),
            receiver: "4012345000023".into(),
        }
    }
}

impl Profile {
    /// The minimal message of Anwendungsfall `pid` (or, for a message type
    /// published without Prüfidentifikatoren, of the `index`-th column):
    /// `UNH` … `UNT`, envelope excluded.
    #[must_use]
    pub fn skeleton(&self, af: &Anwendungsfall, parties: &SkeletonParties) -> Vec<OwnedSegment> {
        // A `Muss [n]` whose Voraussetzung the skeleton itself satisfies (a
        // `LOC+Z16` once `STS+7++xxx+ZW4` is there) only shows once the
        // message exists, so generation runs to a fixpoint over what the
        // validator reports: places and data elements it finds missing are
        // forced in; of the places it finds not permitted, one per round is
        // dropped — the last, so of two places that exclude each other
        // (`Marktlokation` / `Messlokation`) the first in MIG order stays.
        self.generate(None, af, parties)
    }

    /// `seed`, completed to the column of `af`: every segment of `seed` that
    /// fits a place of the Nachrichtenstruktur is kept where it sits, with its
    /// values, and the places and data elements the column requires and
    /// `seed` lacks are filled the way [`Profile::skeleton`] fills them.
    ///
    /// Completion only adds. What `seed` states stays — a place or value the
    /// column does not permit is the sender's to have chosen, and validation
    /// keeps reporting it — and a segment of `seed` that fits no place is the
    /// one thing left out, since nothing says where it belongs.
    ///
    /// This is the sender-side counterpart of validation: a builder states
    /// what the business case knows, and `complete` states the rest of the
    /// Prüfschablone.
    #[must_use]
    pub fn complete(
        &self,
        seed: &[OwnedSegment],
        af: &Anwendungsfall,
        parties: &SkeletonParties,
    ) -> Vec<OwnedSegment> {
        self.generate(Some(seed), af, parties)
    }

    fn generate(
        &self,
        seed: Option<&[OwnedSegment]>,
        af: &Anwendungsfall,
        parties: &SkeletonParties,
    ) -> Vec<OwnedSegment> {
        let resolution = seed.map(|segments| self.structure.resolve(segments));
        let seed = seed
            .zip(resolution.as_ref())
            .map(|(segments, res)| Seed { segments, res });
        let mut fix = Fixpoint::default();
        let mut out = Vec::new();
        for _ in 0..24 {
            out.clear();
            let mut generator = Generator {
                profile: self,
                af,
                parties,
                fix: &fix,
                seed,
                out: &mut out,
            };
            generator.walk(&self.structure.root, seed.map(|_| 0), 0);
            let count = out.len();
            if let Some(unt) = out.iter_mut().find(|s| s.tag == "UNT")
                && let Some(e) = unt.elements.get_mut(0)
            {
                *e = WireElement::of(&[count.to_string()]).into_owned();
            }
            let pid = af.pid.and_then(|p| crate::Pruefidentifikator::new(p).ok());
            let issues = self.validate(&out, pid);
            if !fix.learn(&issues) {
                break;
            }
        }
        out
    }

    /// [`Profile::skeleton`] as a complete interchange — `UNB` … `UNZ` with
    /// `parties` in the envelope — ready for [`crate::parse`].
    ///
    /// # Errors
    ///
    /// When the segments cannot be written (a separator-hostile value the
    /// writer cannot escape).
    pub fn skeleton_interchange(
        &self,
        af: &Anwendungsfall,
        parties: &SkeletonParties,
    ) -> Result<Vec<u8>, crate::Error> {
        let message = edifact_rs::segments_to_bytes(&self.skeleton(af, parties))
            .map_err(crate::Error::Parse)?;
        crate::builders::InterchangeBuilder::new(&parties.sender, &parties.receiver, "1")
            .transmission("261001", "0700")
            .message(message)
            .build()
    }
}

/// What earlier rounds of generation learnt from the validator.
#[derive(Default)]
struct Fixpoint {
    /// Places (by `Nr`) to emit whatever the column says.
    segments: HashSet<String>,
    /// Places (by `Nr`) to leave out.
    dropped: HashSet<String>,
    /// Data elements (`Nr`, DE) to fill.
    elements: HashSet<(String, String)>,
    /// Data elements (`Nr`, DE) to leave empty.
    dropped_elements: HashSet<(String, String)>,
    /// How many admitted codes to skip for a data element (`Nr`, DE) whose
    /// earlier choices the validator refused.
    skipped_codes: HashMap<(String, String), usize>,
}

impl Fixpoint {
    /// Take in a round's findings; `false` once nothing new was learnt.
    fn learn(&mut self, issues: &[edifact_rs::ValidationIssue]) -> bool {
        let mut changed = false;
        let mut drop_segment = None;
        for issue in issues {
            let Some(rule) = issue
                .rule_id()
                .filter(|r| r.starts_with("AHB-") || r.starts_with("MIG-"))
            else {
                continue;
            };
            let Some(nr) = issue.context_get("nr") else {
                continue;
            };
            let de = issue.context_get("de");
            if rule.ends_with("-MISSING")
                || (rule.starts_with("MIG-") && rule.ends_with("-REQUIRED"))
            {
                match de {
                    Some(de) => changed |= self.elements.insert((nr.to_owned(), de.to_owned())),
                    None if !self.dropped.contains(nr) => {
                        changed |= self.segments.insert(nr.to_owned());
                    }
                    None => {}
                }
            } else if rule.starts_with("MIG-") {
                // The MIG's other findings say nothing about what to emit.
            } else if rule.ends_with("-NOT-PERMITTED") {
                match de {
                    Some(de) => {
                        changed |= self.dropped_elements.insert((nr.to_owned(), de.to_owned()));
                    }
                    None => drop_segment = Some(nr.to_owned()),
                }
            } else if rule.ends_with("-CODE")
                && let Some(de) = de
            {
                *self
                    .skipped_codes
                    .entry((nr.to_owned(), de.to_owned()))
                    .or_default() += 1;
                changed = true;
            }
        }
        if let Some(nr) = drop_segment {
            self.segments.remove(&nr);
            changed |= self.dropped.insert(nr);
        }
        changed
    }
}

/// How many occurrences a MIG maximum leaves; `0` is „no maximum“.
fn room_for(max: u32) -> usize {
    if max == 0 { usize::MAX } else { max as usize }
}

/// A Qualifier/Code a Paketmerkmal asks for, and where it goes.
struct Pin {
    de: String,
    occurrence: u8,
    code: String,
}

/// The caller's segments, resolved against the structure.
#[derive(Clone, Copy)]
struct Seed<'a> {
    segments: &'a [OwnedSegment],
    res: &'a super::structure::Resolution,
}

struct Generator<'a> {
    profile: &'a Profile,
    af: &'a Anwendungsfall,
    parties: &'a SkeletonParties,
    /// What the previous rounds learnt from the validator.
    fix: &'a Fixpoint,
    /// The message being completed, if any.
    seed: Option<Seed<'a>>,
    out: &'a mut Vec<OwnedSegment>,
}

impl Generator<'_> {
    /// Emit the children of a group instance, in MIG order: the seed's
    /// segments where it has them (`instance` is the seed's instance of the
    /// group), the column's required ones where it has none.
    ///
    /// `round` selects, for a place whose Paketmerkmale ask for more
    /// Qualifier/Codes than one occurrence of it holds, which of them this
    /// repetition of the enclosing group carries.
    fn walk(&mut self, children: &[NodeId], instance: Option<usize>, round: usize) {
        let s = &self.profile.structure;
        for &child in children {
            let node = &s.nodes[child];
            match &node.kind {
                Kind::Segment { nr, .. } => {
                    let layout = s.layout(child).expect("segment node has a layout");
                    let seeded = self.seed_segments(instance, child);
                    if seeded.is_empty() {
                        if !self.fix.dropped.contains(nr)
                            && (self.fix.segments.contains(nr)
                                || self.required(self.af.segment_status(nr)))
                        {
                            // A Paketmerkmal `n..m` with `n ≥ 1` asks for its
                            // Qualifier/Code, so the place is repeated once
                            // per such code — within the MIG's maximum.
                            let pins = self.paket_pins(layout);
                            let room = room_for(node.max);
                            if pins.is_empty() {
                                let seg = self.segment(layout, None);
                                self.out.push(seg);
                            } else {
                                let from = round.saturating_mul(room).min(pins.len());
                                for pin in pins[from..].iter().take(room) {
                                    let seg = self.segment(layout, Some(pin));
                                    self.out.push(seg);
                                }
                            }
                        }
                    } else {
                        for i in seeded {
                            let seg = self.seeded_segment(i, layout);
                            self.out.push(seg);
                        }
                    }
                }
                Kind::Group { group } => {
                    let Some(trigger) = s.trigger(child) else {
                        continue;
                    };
                    let Some(trigger_nr) = s.nr(trigger) else {
                        continue;
                    };
                    let seeded = self.seed_instances(instance, child);
                    if seeded.is_empty() {
                        let statuses = self
                            .af
                            .group_status(group, trigger_nr)
                            .or_else(|| self.af.segment_status(trigger_nr));
                        if !self.fix.dropped.contains(trigger_nr)
                            && (self.fix.segments.contains(trigger_nr) || self.required(statuses))
                        {
                            let rounds = self.paket_rounds(child).min(room_for(node.max));
                            if rounds > 1 {
                                for r in 0..rounds {
                                    self.walk(&node.children, None, r);
                                }
                            } else {
                                // A group that repeats no further passes the
                                // round of the group above it down.
                                self.walk(&node.children, None, round);
                            }
                        }
                    } else {
                        for inst in seeded {
                            self.walk(&node.children, Some(inst), 0);
                        }
                    }
                }
            }
        }
    }

    /// The seed's segments assigned to `node` directly under `instance`.
    fn seed_segments(&self, instance: Option<usize>, node: NodeId) -> Vec<usize> {
        let (Some(seed), Some(instance)) = (self.seed, instance) else {
            return Vec::new();
        };
        seed.res.instances[instance]
            .segments
            .iter()
            .copied()
            .filter(|&i| seed.res.assigned[i].is_some_and(|a| a.node == node))
            .collect()
    }

    /// The seed's instances of group `node` nested in `instance`.
    fn seed_instances(&self, instance: Option<usize>, node: NodeId) -> Vec<usize> {
        let (Some(seed), Some(instance)) = (self.seed, instance) else {
            return Vec::new();
        };
        seed.res.instances[instance]
            .children
            .iter()
            .copied()
            .filter(|&c| seed.res.instances[c].node == Some(node))
            .collect()
    }

    /// The seed's segment `index`, with the data elements the validator found
    /// missing filled in; what the seed states is kept.
    fn seeded_segment(&self, index: usize, layout: &SegmentNode) -> OwnedSegment {
        let seed = self.seed.expect("a seeded segment has a seed");
        let src = &seed.segments[index];
        if !self.fix.elements.iter().any(|(nr, _)| *nr == layout.nr) {
            return src.clone();
        }
        let mut values: Vec<Vec<String>> = src
            .elements
            .iter()
            .map(|e| e.components().map(ToString::to_string).collect())
            .collect();
        let rules: Vec<&ElementRule> = self.af.element_rules(&layout.nr).collect();
        for (ei, el) in layout.elements.iter().enumerate() {
            let comps: Vec<&Element> = if el.components.is_empty() {
                vec![el]
            } else {
                el.components.iter().collect()
            };
            for (ci, comp) in comps.iter().enumerate() {
                let key = (layout.nr.clone(), comp.id.clone());
                let present = values
                    .get(ei)
                    .and_then(|c| c.get(ci))
                    .is_some_and(|v| !v.is_empty());
                if self.fix.elements.contains(&key) && !present {
                    let occurrence = occurrence_of(layout, comp.id.as_str(), ei, ci);
                    let rule = rules
                        .iter()
                        .find(|r| r.de == comp.id && r.occurrence == occurrence)
                        .copied();
                    while values.len() <= ei {
                        values.push(Vec::new());
                    }
                    while values[ei].len() <= ci {
                        values[ei].push(String::new());
                    }
                    let skip = self.fix.skipped_codes.get(&key).copied().unwrap_or(0);
                    let current = values[ei].clone();
                    values[ei][ci] = self.forced(layout, comp, rule, skip, &values, &current);
                }
            }
        }
        for comps in &mut values {
            while comps.last().is_some_and(String::is_empty) {
                comps.pop();
            }
        }
        while values.last().is_some_and(Vec::is_empty) {
            values.pop();
        }
        let elements: Vec<WireElement<'static>> = values
            .iter()
            .map(|comps| {
                if comps.is_empty() {
                    WireElement::of::<&str>(&[]).into_owned()
                } else {
                    WireElement::of(comps).into_owned()
                }
            })
            .collect();
        Segment::new(layout.tag.clone(), elements)
    }

    /// `Muss` without a Voraussetzung — the receiver-independent part of the
    /// column.
    fn required(&self, statuses: Option<&[String]>) -> bool {
        let Some(statuses) = statuses else {
            return false;
        };
        statuses.iter().any(|t| {
            Status::parse(t).is_some_and(|st| {
                st.kind.is_receiver_checkable()
                    && unconditioned(&self.profile.ahb, st.expr.as_ref())
            })
        })
    }

    /// How many instances of `group` its places need to carry every
    /// Qualifier/Code their Paketmerkmale ask for — the codes a nested place
    /// cannot fit in its own repetitions are carried by repeating the group
    /// around it.
    fn paket_rounds(&self, group: NodeId) -> usize {
        let s = &self.profile.structure;
        s.nodes[group]
            .children
            .iter()
            .map(|&child| {
                let room = room_for(s.nodes[child].max);
                match s.layout(child) {
                    Some(layout) => self.paket_pins(layout).len().div_ceil(room),
                    None => self.paket_rounds(child).div_ceil(room),
                }
            })
            .max()
            .unwrap_or(1)
            .max(1)
    }

    /// The Qualifier/Codes of `layout` a Paket asks for, in AHB order.
    ///
    /// Allgemeine Festlegungen 6.1d Kap. 6.9.2: a code marked `X [kPn..m]`
    /// with `n ≥ 1` „muss im Paket k … angegeben werden“. Only a Paket the
    /// skeleton knows applies — one whose Paketvoraussetzung is empty
    /// (Kap. 6.9.1) — is filled in.
    fn paket_pins(&self, layout: &SegmentNode) -> Vec<Pin> {
        let mut pins = Vec::new();
        for rule in self.af.element_rules(&layout.nr) {
            for op in &rule.operands {
                let (Some(code), Some(status)) = (&op.code, Status::parse(&op.operand)) else {
                    continue;
                };
                let Some(expr) = &status.expr else { continue };
                if !status.kind.is_receiver_checkable()
                    || !unconditioned(&self.profile.ahb, Some(expr))
                    || !expr.pakete().iter().any(|p| p.min >= 1)
                {
                    continue;
                }
                pins.push(Pin {
                    de: rule.de.clone(),
                    occurrence: rule.occurrence,
                    code: code.clone(),
                });
            }
        }
        pins
    }

    fn segment(&self, layout: &SegmentNode, pin: Option<&Pin>) -> OwnedSegment {
        let rules: Vec<&ElementRule> = self.af.element_rules(&layout.nr).collect();
        // The code chosen for each element, needed by dependent elements
        // (`DTM` DE 2380 follows DE 2379; `LOC` DE 3225 follows DE 3227).
        let mut chosen: Vec<Vec<String>> = Vec::new();
        for (ei, el) in layout.elements.iter().enumerate() {
            let comps: Vec<&Element> = if el.components.is_empty() {
                vec![el]
            } else {
                el.components.iter().collect()
            };
            let composite_used =
                el.components.is_empty() || matches!(el.status.as_str(), "M" | "R");
            let mut values: Vec<String> = Vec::new();
            for (ci, comp) in comps.iter().enumerate() {
                let occurrence = occurrence_of(layout, comp.id.as_str(), ei, ci);
                let rule = rules
                    .iter()
                    .find(|r| r.de == comp.id && r.occurrence == occurrence)
                    .copied();
                let key = (layout.nr.clone(), comp.id.clone());
                let skip = self.fix.skipped_codes.get(&key).copied().unwrap_or(0);
                let value = if pin.is_some_and(|p| p.de == comp.id && p.occurrence == occurrence) {
                    pin.map(|p| p.code.clone())
                } else if self.fix.dropped_elements.contains(&key) {
                    None
                } else if self.fix.elements.contains(&key) {
                    Some(self.forced(layout, comp, rule, skip, &chosen, &values))
                } else {
                    self.value(layout, comp, rule, composite_used, skip, &chosen, &values)
                };
                values.push(value.unwrap_or_default());
            }
            // A composite the column brings into use carries what the MIG
            // requires of it (`PIA` C212: DE 7143 beside the DE 1131 code).
            if !composite_used && values.iter().any(|v| !v.is_empty()) {
                for (ci, comp) in comps.iter().enumerate() {
                    let key = (layout.nr.clone(), comp.id.clone());
                    if values[ci].is_empty()
                        && matches!(comp.status.as_str(), "M" | "R")
                        && !self.fix.dropped_elements.contains(&key)
                    {
                        let occurrence = occurrence_of(layout, comp.id.as_str(), ei, ci);
                        let rule = rules
                            .iter()
                            .find(|r| r.de == comp.id && r.occurrence == occurrence)
                            .copied();
                        let current = values.clone();
                        let skip = self.fix.skipped_codes.get(&key).copied().unwrap_or(0);
                        values[ci] = self.forced(layout, comp, rule, skip, &chosen, &current);
                    }
                }
            }
            while values.last().is_some_and(String::is_empty) {
                values.pop();
            }
            chosen.push(values);
        }
        while chosen.last().is_some_and(Vec::is_empty) {
            chosen.pop();
        }
        let elements: Vec<WireElement<'static>> = chosen
            .iter()
            .map(|comps| {
                if comps.is_empty() {
                    WireElement::of::<&str>(&[]).into_owned()
                } else {
                    WireElement::of(comps).into_owned()
                }
            })
            .collect();
        Segment::new(layout.tag.clone(), elements)
    }

    /// The value of a data element the validator found missing: the column's
    /// first code, else the MIG's, else a synthetic value.
    fn forced(
        &self,
        layout: &SegmentNode,
        el: &Element,
        rule: Option<&ElementRule>,
        skip: usize,
        chosen: &[Vec<String>],
        current: &[String],
    ) -> String {
        let coded: Vec<String> = rule
            .map(|r| r.operands.iter().filter_map(|o| o.code.clone()).collect())
            .unwrap_or_default();
        nth_code(&coded, skip)
            .or_else(|| el.is_code_list().then(|| mig_code(el, skip)).flatten())
            .unwrap_or_else(|| self.synthetic(layout, el, chosen, current))
    }

    /// The value of one data element: the column's first admitted code, or
    /// a synthetic value when the element is used but not coded, or nothing.
    // The arguments are the segment's context; a struct for them would be
    // built and taken apart at the one call site.
    #[allow(clippy::too_many_arguments)]
    fn value(
        &self,
        layout: &SegmentNode,
        el: &Element,
        rule: Option<&ElementRule>,
        composite_used: bool,
        skip: usize,
        chosen: &[Vec<String>],
        current: &[String],
    ) -> Option<String> {
        let mig_required = composite_used && matches!(el.status.as_str(), "M" | "R");
        let Some(rule) = rule else {
            // Not in the column: only what the MIG itself requires, so a
            // contradiction between MIG and AHB shows up in validation.
            if !mig_required {
                return None;
            }
            if el.is_code_list() {
                return mig_code(el, skip);
            }
            return Some(self.synthetic(layout, el, chosen, current));
        };
        let used = |op: &str| {
            Status::parse(op).is_some_and(|st| {
                unconditioned(&self.profile.ahb, st.expr.as_ref())
                    && st.kind.is_receiver_checkable()
            })
        };
        // Codes whose operand applies without a Voraussetzung come first;
        // the rest follow for when the validator refuses those.
        let mut coded: Vec<String> = rule
            .operands
            .iter()
            .filter(|o| o.code.is_some() && used(&o.operand))
            .filter_map(|o| o.code.clone())
            .collect();
        let unconditional = !coded.is_empty();
        for o in &rule.operands {
            if let Some(c) = &o.code
                && !coded.contains(c)
            {
                coded.push(c.clone());
            }
        }
        if !coded.is_empty() {
            return if unconditional || mig_required {
                nth_code(&coded, skip)
            } else {
                None
            };
        }
        let value_used = rule
            .operands
            .iter()
            .any(|o| o.code.is_none() && used(&o.operand));
        if value_used || mig_required {
            // `X` on a coded element: the MIG's list applies.
            if el.is_code_list() {
                return mig_code(el, skip);
            }
            Some(self.synthetic(layout, el, chosen, current))
        } else {
            None
        }
    }

    /// A value of the right shape for `el`, given what the segment carries so
    /// far.
    // One table of data elements; splitting it would hide what it covers.
    #[allow(clippy::too_many_lines)]
    fn synthetic(
        &self,
        layout: &SegmentNode,
        el: &Element,
        chosen: &[Vec<String>],
        current: &[String],
    ) -> String {
        let qualifier = chosen
            .first()
            .and_then(|c| c.first())
            .map_or("", String::as_str);
        let first_in_element = current.first().map_or("", String::as_str);
        let pid = self
            .af
            .pid
            .map_or_else(|| "00000".into(), |p| p.to_string());
        let s = match el.id.as_str() {
            "0074" => "0".into(),
            "1004" => format!("DOK{pid}"),
            "1154" | "1156" => {
                if qualifier == "Z13" {
                    pid
                } else {
                    format!("REF{pid}")
                }
            }
            "7402" => "VORGANG0001".into(),
            "3039" => match qualifier {
                "MS" => self.parties.sender.clone(),
                "MR" => self.parties.receiver.clone(),
                _ => "9900357000004".into(),
            },
            "0004" => self.parties.sender.clone(),
            "0010" => self.parties.receiver.clone(),
            "3055" => {
                let id = current.first().map_or("", String::as_str);
                crate::AgencyCode::for_mp_id(id).as_str().to_owned()
            }
            "3225" => match qualifier {
                "Z17" | "172" | "Z04" => "DE00056266802AO6G56M11SN51G21M24S".into(),
                "237" | "Z15" => "11XBK-STD-----9".into(),
                "Z18" => "E0001234567890".into(),
                "Z19" => "C1234567890123".into(),
                "Z20" => "D0001234567890".into(),
                _ => "51238696781".into(),
            },
            "2380" => {
                // The format code sits after this component; the layout lists it.
                let format = layout
                    .leaves()
                    .find(|(_, _, e)| e.id == "2379")
                    .and_then(|(_, _, e)| {
                        self.first_code(&layout.nr, "2379")
                            .or_else(|| e.codes.first().map(|c| c.code.clone()))
                    })
                    .unwrap_or_else(|| "303".into());
                match format.as_str() {
                    "303" => "202610010000+00".into(),
                    "610" | "602" => "202610".into(),
                    "802" => "2026".into(),
                    "104" | "106" => "1001".into(),
                    "203" | "304" => "202610010000".into(),
                    "204" => "20261001000000".into(),
                    "401" => "0000".into(),
                    "501" | "502" => "00000000".into(),
                    _ => "20261001".into(),
                }
            }
            "0017" => "260901".into(),
            "0019" => "0000".into(),
            "0062" | "0020" | "1050" | "1082" | "1490" | "1222" | "7110" | "1229" | "1225" => {
                "1".into()
            }
            "7140" => match first_in_element {
                _ if layout.tag == "PIA" && self.profile.mig.message_type == "MSCONS" => {
                    "1-1:1.8.0".into()
                }
                _ => "9991000002082".into(),
            },
            "6060" | "6314" => "100".into(),
            "6411" => "KWH".into(),
            "5004" | "5025" | "5118" => "10.00".into(),
            "5482" | "5284" => "19.00".into(),
            "3036" | "3412" => "Mustermann".into(),
            "3042" => "Musterstr. 1".into(),
            "3164" => "Berlin".into(),
            "3251" => "10115".into(),
            "3207" => "DE".into(),
            "3148" => "0301234567".into(),
            "4440" | "4441" => "Text".into(),
            "1131" => "E_0001".into(),
            "6063" => "220".into(),
            "1001" => "E01".into(),
            "1373" => "11".into(),
            _ => match el.format.as_deref() {
                Some(f) if f.starts_with('n') => "1".into(),
                Some(f) if f.starts_with('a') && !f.starts_with("an") => "A".into(),
                _ => "X".into(),
            },
        };
        // A numeric representation takes digits whatever the element is for.
        let s = match el.format.as_deref() {
            Some(f) if f.starts_with('n') && !s.chars().all(|c| c.is_ascii_digit()) => {
                let digits: String = s.chars().filter(char::is_ascii_digit).collect();
                if digits.is_empty() {
                    "1".into()
                } else {
                    digits
                }
            }
            _ => s,
        };
        fit(&s, el.format.as_deref())
    }

    fn first_code(&self, nr: &str, de: &str) -> Option<String> {
        self.af
            .element_rules(nr)
            .find(|r| r.de == de)
            .and_then(|r| r.operands.iter().find_map(|o| o.code.clone()))
    }
}

/// The `skip`-th candidate, wrapping around.
fn nth_code(codes: &[String], skip: usize) -> Option<String> {
    (!codes.is_empty()).then(|| codes[skip % codes.len()].clone())
}

/// The `skip`-th code of the MIG's list.
fn mig_code(el: &Element, skip: usize) -> Option<String> {
    let codes: Vec<String> = el.codes.iter().map(|c| c.code.clone()).collect();
    nth_code(&codes, skip)
}

/// Which occurrence of `de` inside `layout` sits at `(ei, ci)`.
fn occurrence_of(layout: &SegmentNode, de: &str, ei: usize, ci: usize) -> u8 {
    let mut n = 0u8;
    for (i, el) in layout.elements.iter().enumerate() {
        if el.components.is_empty() {
            if el.id == de {
                if i == ei {
                    return n;
                }
                n += 1;
            }
        } else {
            for (j, comp) in el.components.iter().enumerate() {
                if comp.id == de {
                    if i == ei && j == ci {
                        return n;
                    }
                    n += 1;
                }
            }
        }
    }
    0
}

/// Whether a status expression holds whatever the message says — the
/// receiver-independent part of the column, which is what a skeleton fills.
///
/// A status with no expression is unconditioned; one that comes out
/// `Truth::Unknown`, or that does not evaluate at all, waits on a message the
/// skeleton is only about to build.
fn unconditioned(ahb: &super::model::AhbProfile, expr: Option<&super::conditions::Expr>) -> bool {
    let Some(expr) = expr else { return true };
    expr.eval(&mut |id| Ok(neutral_or_unknown(ahb, id)))
        .is_ok_and(|t| t != Truth::Unknown)
}

/// Bedingungen without a message: a Voraussetzung („Wenn …") is undecidable,
/// everything else — Hinweise, formats, repetition rules, constraints — is
/// neutral. A Paket stands for its Paketvoraussetzung, so an empty one — the
/// Standardpaket — is no condition at all (Allgemeine Festlegungen 6.1d
/// Kap. 6.9.1).
fn neutral_or_unknown(ahb: &super::model::AhbProfile, id: &str) -> Truth {
    match super::conditions::ConditionKind::of(id) {
        super::conditions::ConditionKind::Paket => {
            match super::conditions::Paket::parse(id).and_then(|p| ahb.packages.get(&p.id)) {
                Some(text) if text.trim().is_empty() => Truth::Neutral,
                _ => Truth::Unknown,
            }
        }
        super::conditions::ConditionKind::Voraussetzung => match ahb.conditions.get(id) {
            Some(text) if !super::conditions::is_precondition(text) => Truth::Neutral,
            _ => Truth::Unknown,
        },
        _ => Truth::Neutral,
    }
}

/// Trim or pad a synthetic value to a fixed representation.
fn fit(value: &str, format: Option<&str>) -> String {
    let Some(f) = format else {
        return value.to_owned();
    };
    let (variable, len) = match f
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .strip_prefix("..")
    {
        Some(l) => (true, l.parse::<usize>().ok()),
        None => (
            false,
            f.trim_start_matches(|c: char| c.is_ascii_alphabetic())
                .parse::<usize>()
                .ok(),
        ),
    };
    let Some(len) = len else {
        return value.to_owned();
    };
    let n = value.chars().count();
    if n > len {
        return value.chars().take(len).collect();
    }
    if !variable && n < len {
        let pad = if f.starts_with('n') { '0' } else { 'X' };
        let mut s = value.to_owned();
        while s.chars().count() < len {
            s.push(pad);
        }
        return s;
    }
    value.to_owned()
}
