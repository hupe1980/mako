//! Resolving a message against the Nachrichtenstruktur.
//!
//! The MIG numbers every place a segment can occur (`Nr`, e.g. `00047` is
//! `SG5 LOC` „Marktlokation", `00048` the `SG5 LOC` „Ruhende Marktlokation"
//! right after it). Which of two same-tag places a wire segment occupies is
//! decided by the codes the MIG admits there — `LOC+Z16` is `00047`,
//! `LOC+Z22` is `00048` — and by order: the structure is walked front to
//! back and a segment that fits no later place is out of order.
//!
//! [`Structure::resolve`] assigns every segment of a message to its `Nr` and
//! builds the tree of segment-group instances the AHB rules are evaluated
//! over.

use std::collections::BTreeMap;
use std::collections::HashMap;

use edifact_rs::Segment;

use super::model::{Element, MigProfile, Node, SegmentNode};

/// Index of a node in [`Structure::nodes`].
pub type NodeId = usize;
/// Index of a group instance in [`Resolution::instances`].
pub type InstanceId = usize;

/// A compiled node of the structure.
#[derive(Debug)]
pub struct SNode {
    /// The enclosing group node.
    pub parent: Option<NodeId>,
    /// Children in MIG order; empty for a segment.
    pub children: Vec<NodeId>,
    /// Group or segment.
    pub kind: Kind,
    /// BDEW status: `M`, `R`, `D`, `O`, `N`.
    pub status: String,
    /// Maximum repetitions.
    pub max: u32,
    /// The MIG's name for this place.
    pub name: String,
}

/// What a node is.
#[derive(Debug)]
pub enum Kind {
    /// A segment group.
    Group {
        /// `SG4`, `SG10`, …
        group: String,
    },
    /// A segment.
    Segment {
        /// The MIG's running segment number.
        nr: String,
        /// The segment tag.
        tag: String,
        /// Coded data elements that decide whether a wire segment sits here.
        discriminators: Vec<Discriminator>,
        /// Index into the flat layout list.
        layout: usize,
    },
}

/// A data element that tells this place apart from another place of the
/// same tag: the places differ here, and nowhere earlier on the wire.
///
/// `NAD` DE 3055 is coded at every `NAD` place, but `NAD+MS` and `NAD+MR`
/// are told apart by DE 3035 before it — a wrong 3055 is a code finding at
/// the place, not a reason to fit the segment elsewhere. `CCI+++ZB3` and
/// `CCI+Z20` differ at DE 7059 already: at the first, 7059 is not used.
#[derive(Debug)]
pub struct Discriminator {
    /// Element index on the wire.
    pub element: usize,
    /// Component index on the wire.
    pub component: usize,
    /// The codes the MIG admits at this place; empty when it admits any
    /// value here.
    pub codes: Vec<String>,
    /// The MIG does not use the element at this place (`N`): a value here
    /// belongs to another place.
    pub not_used: bool,
    /// Codes another place of the same tag admits here and this one does
    /// not: a value among them belongs to that other place. A value no place
    /// admits stays here as a code finding.
    pub foreign: Vec<String>,
    /// BDEW status `M`/`R`: an empty value cannot be this place.
    pub required: bool,
}

/// The compiled structure of one MIG.
#[derive(Debug)]
pub struct Structure {
    /// Every node, groups and segments.
    pub nodes: Vec<SNode>,
    /// Root children.
    pub root: Vec<NodeId>,
    /// Segment nodes by `Nr`.
    pub by_nr: HashMap<String, NodeId>,
    /// The segment layouts, by [`Kind::Segment::layout`].
    pub layouts: Vec<SegmentNode>,
}

impl Structure {
    /// Compile the structure of `mig`.
    #[must_use]
    pub fn compile(mig: &MigProfile) -> Self {
        let mut s = Structure {
            nodes: Vec::new(),
            root: Vec::new(),
            by_nr: HashMap::new(),
            layouts: Vec::new(),
        };
        let root: Vec<NodeId> = mig.structure.iter().map(|n| s.add(n, None)).collect();
        s.root = root;
        let Structure { nodes, layouts, .. } = &mut s;
        mark_identifying(nodes, layouts);
        s
    }

    fn add(&mut self, node: &Node, parent: Option<NodeId>) -> NodeId {
        match node {
            Node::Group(g) => {
                let id = self.nodes.len();
                self.nodes.push(SNode {
                    parent,
                    children: Vec::new(),
                    kind: Kind::Group {
                        group: g.group.clone(),
                    },
                    status: g.status.clone(),
                    max: g.max,
                    name: g.name.clone(),
                });
                let children: Vec<NodeId> =
                    g.children.iter().map(|c| self.add(c, Some(id))).collect();
                self.nodes[id].children = children;
                id
            }
            Node::Segment(seg) => {
                let id = self.nodes.len();
                let layout = self.layouts.len();
                self.layouts.push(seg.clone());
                self.by_nr.insert(seg.nr.clone(), id);
                self.nodes.push(SNode {
                    parent,
                    children: Vec::new(),
                    kind: Kind::Segment {
                        nr: seg.nr.clone(),
                        tag: seg.tag.clone(),
                        discriminators: discriminators(seg),
                        layout,
                    },
                    status: seg.status.clone(),
                    max: seg.max,
                    name: seg.name.clone(),
                });
                id
            }
        }
    }

    /// The layout of segment node `id`.
    #[must_use]
    pub fn layout(&self, id: NodeId) -> Option<&SegmentNode> {
        match &self.nodes[id].kind {
            Kind::Segment { layout, .. } => self.layouts.get(*layout),
            Kind::Group { .. } => None,
        }
    }

    /// The `Nr` of segment node `id`.
    #[must_use]
    pub fn nr(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            Kind::Segment { nr, .. } => Some(nr),
            Kind::Group { .. } => None,
        }
    }

    /// The tag of segment node `id`.
    #[must_use]
    pub fn tag(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            Kind::Segment { tag, .. } => Some(tag),
            Kind::Group { .. } => None,
        }
    }

    /// The group name of group node `id`.
    #[must_use]
    pub fn group(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            Kind::Group { group } => Some(group),
            Kind::Segment { .. } => None,
        }
    }

    /// The trigger segment of group node `id` — its first child.
    #[must_use]
    pub fn trigger(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].children.first().copied()
    }

    /// Groups from the root down to `id`, as `SGn` names.
    #[must_use]
    pub fn path(&self, id: NodeId) -> Vec<&str> {
        let mut out = Vec::new();
        let mut cur = self.nodes[id].parent;
        while let Some(p) = cur {
            if let Some(g) = self.group(p) {
                out.push(g);
            }
            cur = self.nodes[p].parent;
        }
        out.reverse();
        out
    }

    /// Whether `seg` opens `child`: the segment itself, or a group's trigger.
    fn matches_child(&self, child: NodeId, seg: &Segment<'_>) -> bool {
        match &self.nodes[child].kind {
            Kind::Segment { .. } => self.matches(child, seg),
            Kind::Group { .. } => self.trigger(child).is_some_and(|t| self.matches(t, seg)),
        }
    }

    fn matches(&self, id: NodeId, seg: &Segment<'_>) -> bool {
        let Kind::Segment {
            tag,
            discriminators,
            ..
        } = &self.nodes[id].kind
        else {
            return false;
        };
        if seg.tag != *tag {
            return false;
        }
        discriminators.iter().all(|d| {
            match seg.component_str(d.element, d.component) {
                Some(v) if !v.is_empty() => {
                    !d.not_used
                        && (d.codes.iter().any(|c| c == v) || !d.foreign.iter().any(|c| c == v))
                }
                // An absent optional qualifier cannot contradict the place; an
                // absent mandatory one rules it out (`CCI+++Z15` is not the
                // `CCI+Z18` place).
                _ => !d.required,
            }
        })
    }

    /// Assign every segment of `segments` (the message from `UNH` to `UNT`,
    /// envelope excluded) to its place in the structure.
    #[must_use]
    pub fn resolve(&self, segments: &[Segment<'_>]) -> Resolution {
        let mut res = Resolution {
            assigned: vec![None; segments.len()],
            instances: vec![Instance {
                node: None,
                parent: None,
                children: Vec::new(),
                segments: Vec::new(),
                first: 0,
                last: segments.len(),
            }],
            unresolved: Vec::new(),
        };
        let end = self.walk(&self.root, segments, 0, 0, &[], &mut res);
        for i in end..segments.len() {
            res.unresolved.push(i);
        }
        res.unresolved.sort_unstable();
        res
    }

    /// Match `children` in order against `segments[pos..]`, filling `res`,
    /// and return the position after the last segment consumed.
    ///
    /// `outer` holds, per ancestor level, the children still open there
    /// (the current one first, since a group may repeat). A segment that
    /// fits none of the remaining siblings and none of those is out of
    /// place: it is reported and skipped, so one stray segment does not
    /// leave the rest of the message unresolved.
    fn walk(
        &self,
        children: &[NodeId],
        segments: &[Segment<'_>],
        mut pos: usize,
        instance: InstanceId,
        outer: &[&[NodeId]],
        res: &mut Resolution,
    ) -> usize {
        for (i, &child) in children.iter().enumerate() {
            let mut count = 0u32;
            while let Some(seg) = segments.get(pos) {
                if !self.matches_child(child, seg) {
                    let placeable = children[i + 1..]
                        .iter()
                        .any(|&c| self.matches_child(c, seg))
                        || outer
                            .iter()
                            .any(|level| level.iter().any(|&c| self.matches_child(c, seg)));
                    if placeable {
                        break;
                    }
                    res.unresolved.push(pos);
                    pos += 1;
                    continue;
                }
                match &self.nodes[child].kind {
                    Kind::Segment { .. } => {
                        res.assigned[pos] = Some(Assigned {
                            node: child,
                            instance,
                            occurrence: count,
                        });
                        res.instances[instance].segments.push(pos);
                        pos += 1;
                        count += 1;
                    }
                    Kind::Group { .. } => {
                        let inst = res.instances.len();
                        res.instances.push(Instance {
                            node: Some(child),
                            parent: Some(instance),
                            children: Vec::new(),
                            segments: Vec::new(),
                            first: pos,
                            last: pos,
                        });
                        res.instances[instance].children.push(inst);
                        let mut levels: Vec<&[NodeId]> = Vec::with_capacity(outer.len() + 1);
                        levels.push(&children[i..]);
                        levels.extend_from_slice(outer);
                        pos = self.walk(
                            &self.nodes[child].children,
                            segments,
                            pos,
                            inst,
                            &levels,
                            res,
                        );
                        res.instances[inst].last = pos;
                        count += 1;
                    }
                }
                // Past the maximum the same segment may still belong here —
                // reported by the cardinality check rather than pushed into a
                // wrong place.
                if count >= self.nodes[child].max.max(1) && self.nodes[child].max != 0 {
                    // Keep consuming repeats of a segment (cardinality error),
                    // but do not open further group instances beyond max — and
                    // a repeated trigger opens the next instance of its group
                    // (`SG6 LOC` twice is two `SG6`, not one over its maximum).
                    if matches!(self.nodes[child].kind, Kind::Group { .. })
                        || (i == 0 && instance != 0)
                    {
                        break;
                    }
                }
            }
        }
        pos
    }
}

/// How a place reads one wire position: the element is not used, admits a
/// code list, or takes any value.
#[derive(PartialEq, Eq)]
enum Signature<'a> {
    NotUsed,
    Codes(&'a [String]),
    Free,
}

fn signature(ds: &[Discriminator], pos: (usize, usize)) -> Signature<'_> {
    ds.iter()
        .find(|d| (d.element, d.component) == pos)
        .map_or(Signature::Free, |d| {
            if d.not_used {
                Signature::NotUsed
            } else {
                Signature::Codes(&d.codes)
            }
        })
}

/// Keep, per place, the discriminators at the positions that tell the
/// places of its tag apart: for every pair of places, the first wire
/// position where they read differently.
fn mark_identifying(nodes: &mut [SNode], layouts: &[SegmentNode]) {
    let mut by_tag: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for (id, node) in nodes.iter().enumerate() {
        if let Kind::Segment { tag, .. } = &node.kind {
            by_tag.entry(tag.clone()).or_default().push(id);
        }
    }
    for ids in by_tag.values() {
        let places: Vec<&[Discriminator]> = ids
            .iter()
            .map(|&id| match &nodes[id].kind {
                Kind::Segment { discriminators, .. } => discriminators.as_slice(),
                Kind::Group { .. } => &[],
            })
            .collect();
        let mut positions: Vec<(usize, usize)> = places
            .iter()
            .flat_map(|ds| ds.iter().map(|d| (d.element, d.component)))
            .collect();
        positions.sort_unstable();
        positions.dedup();
        let mut identifying: Vec<(usize, usize)> = vec![(0, 0)];
        for (a, pa) in places.iter().enumerate() {
            for pb in places.iter().skip(a + 1) {
                if let Some(&pos) = positions
                    .iter()
                    .find(|&&pos| signature(pa, pos) != signature(pb, pos))
                    && !identifying.contains(&pos)
                {
                    identifying.push(pos);
                }
            }
        }
        // What the other places admit at each identifying position.
        let mut admitted_elsewhere: Vec<(NodeId, (usize, usize), Vec<String>)> = Vec::new();
        for (&id, ds) in ids.iter().zip(&places) {
            for d in ds
                .iter()
                .filter(|d| identifying.contains(&(d.element, d.component)))
            {
                admitted_elsewhere.push((id, (d.element, d.component), d.codes.clone()));
            }
        }
        for &id in ids {
            if let Kind::Segment {
                discriminators,
                layout,
                ..
            } = &mut nodes[id].kind
            {
                // A wire position the layout does not define at all is one
                // this place does not use: `LIN+1+Z67` is not a one-element
                // `LIN` place.
                let layout = &layouts[*layout];
                // A place published without a Segmentlayout defines nothing
                // and refuses nothing.
                let has_layout = !layout.elements.is_empty();
                for &(ei, ci) in &identifying {
                    if !has_layout {
                        break;
                    }
                    let defined = layout.elements.get(ei).is_some_and(|el| {
                        ci == 0 && el.components.is_empty() || el.components.get(ci).is_some()
                    });
                    if !defined
                        && !discriminators
                            .iter()
                            .any(|d| (d.element, d.component) == (ei, ci))
                    {
                        discriminators.push(Discriminator {
                            element: ei,
                            component: ci,
                            codes: Vec::new(),
                            not_used: true,
                            foreign: Vec::new(),
                            required: false,
                        });
                    }
                }
                discriminators.retain(|d| identifying.contains(&(d.element, d.component)));
                for d in discriminators.iter_mut() {
                    let mut foreign: Vec<String> = Vec::new();
                    for (other, pos, codes) in &admitted_elsewhere {
                        if *other != id && *pos == (d.element, d.component) {
                            for c in codes {
                                if !d.codes.contains(c) && !foreign.contains(c) {
                                    foreign.push(c.clone());
                                }
                            }
                        }
                    }
                    d.foreign = foreign;
                }
            }
        }
    }
}

/// Every position a place reads by code list, plus the elements the place
/// does not use; `mark_identifying` keeps the telling ones.
fn discriminators(seg: &SegmentNode) -> Vec<Discriminator> {
    let mut out = Vec::new();
    for (ei, el) in seg.elements.iter().enumerate() {
        let composite_used = matches!(el.status.as_str(), "M" | "R");
        let leaf = |e: &Element, ci: usize, in_composite: bool, out: &mut Vec<Discriminator>| {
            let leading = ei == 0 && ci == 0;
            if e.status == "N" {
                // A value where this place uses nothing belongs to a place
                // that does (`LIN+1+Z67` is the Messprodukt line, not the
                // first `SG27`) — where the places differ there at all.
                out.push(Discriminator {
                    element: ei,
                    component: ci,
                    codes: Vec::new(),
                    not_used: true,
                    foreign: Vec::new(),
                    required: false,
                });
            } else if e.is_code_list() {
                out.push(Discriminator {
                    element: ei,
                    component: ci,
                    codes: e.codes.iter().map(|c| c.code.clone()).collect(),
                    not_used: false,
                    foreign: Vec::new(),
                    required: leading
                        && matches!(e.status.as_str(), "M" | "R")
                        && (!in_composite || composite_used),
                });
            }
        };
        if el.components.is_empty() {
            leaf(el, 0, false, &mut out);
        } else {
            for (ci, comp) in el.components.iter().enumerate() {
                leaf(comp, ci, true, &mut out);
            }
        }
    }
    out
}

/// One segment's place.
#[derive(Debug, Clone, Copy)]
pub struct Assigned {
    /// The segment node.
    pub node: NodeId,
    /// The group instance the segment sits in (the root instance is `0`).
    pub instance: InstanceId,
    /// Zero-based repetition of this node inside its instance.
    pub occurrence: u32,
}

/// One occurrence of a segment group on the wire.
#[derive(Debug)]
pub struct Instance {
    /// The group node; `None` for the message root.
    pub node: Option<NodeId>,
    /// The enclosing instance.
    pub parent: Option<InstanceId>,
    /// Nested group instances, in wire order.
    pub children: Vec<InstanceId>,
    /// Segment indices assigned directly to this instance.
    pub segments: Vec<usize>,
    /// Span `[first, last)` over the segment slice, descendants included.
    pub first: usize,
    /// End of the span, exclusive.
    pub last: usize,
}

/// The result of [`Structure::resolve`].
#[derive(Debug)]
pub struct Resolution {
    /// Per segment, its place — `None` for an unresolved segment.
    pub assigned: Vec<Option<Assigned>>,
    /// Group instances; index `0` is the message root.
    pub instances: Vec<Instance>,
    /// Segments that fit no place from their position on.
    pub unresolved: Vec<usize>,
}

impl Resolution {
    /// How many direct segments of `instance` sit at `node`.
    #[must_use]
    pub fn count(&self, instance: InstanceId, node: NodeId) -> usize {
        self.instances[instance]
            .segments
            .iter()
            .filter(|&&i| self.assigned[i].is_some_and(|a| a.node == node))
            .count()
    }

    /// How many child instances of `instance` are occurrences of group `node`.
    #[must_use]
    pub fn group_count(&self, instance: InstanceId, node: NodeId) -> usize {
        self.instances[instance]
            .children
            .iter()
            .filter(|&&c| self.instances[c].node == Some(node))
            .count()
    }

    /// The nearest enclosing instance of `instance` (itself included) whose
    /// group is named `group`.
    #[must_use]
    pub fn enclosing(
        &self,
        structure: &Structure,
        instance: InstanceId,
        group: &str,
    ) -> Option<InstanceId> {
        let mut cur = Some(instance);
        while let Some(i) = cur {
            if let Some(n) = self.instances[i].node
                && structure.group(n) == Some(group)
            {
                return Some(i);
            }
            cur = self.instances[i].parent;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mig() -> MigProfile {
        let json = r#"{
          "schema_version": 2, "message_type": "UTILMD", "release": "S2.2", "valid_from": "2026-10-01",
          "ahb_version": "2.2", "source": {"file": "x"},
          "structure": [
            {"nr":"00003","tag":"UNH","status":"M","max":1,"elements":[]},
            {"nr":"00004","tag":"BGM","status":"M","max":1,"elements":[]},
            {"group":"SG2","status":"R","max":99,"children":[
              {"nr":"00008","tag":"NAD","status":"M","max":1,"elements":[{"id":"3035","status":"M","codes":[{"code":"MS"}]}]}
            ]},
            {"group":"SG2","status":"R","max":99,"children":[
              {"nr":"00009","tag":"NAD","status":"M","max":1,"elements":[{"id":"3035","status":"M","codes":[{"code":"MR"}]}]}
            ]},
            {"group":"SG4","status":"D","max":99999,"children":[
              {"nr":"00018","tag":"IDE","status":"M","max":1,"elements":[{"id":"7495","status":"M","codes":[{"code":"24"}]}]},
              {"nr":"00020","tag":"DTM","status":"D","max":1,"elements":[{"id":"C507","status":"M","components":[{"id":"2005","status":"M","codes":[{"code":"92"}]}]}]},
              {"nr":"00021","tag":"DTM","status":"D","max":1,"elements":[{"id":"C507","status":"M","components":[{"id":"2005","status":"M","codes":[{"code":"93"}]}]}]},
              {"group":"SG5","status":"D","max":999999,"children":[
                {"nr":"00047","tag":"LOC","status":"M","max":1,"elements":[{"id":"3227","status":"M","codes":[{"code":"Z16"}]}]}
              ]},
              {"group":"SG5","status":"D","max":999999,"children":[
                {"nr":"00048","tag":"LOC","status":"M","max":1,"elements":[{"id":"3227","status":"M","codes":[{"code":"Z22"}]}]}
              ]}
            ]},
            {"nr":"00493","tag":"UNT","status":"M","max":1,"elements":[]}
          ]
        }"#;
        serde_json::from_str(json).unwrap()
    }

    fn segs(edi: &str) -> Vec<edifact_rs::OwnedSegment> {
        edifact_rs::from_bytes(edi.as_bytes())
            .map(|s| s.map(edifact_rs::Segment::into_owned))
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn same_tag_places_are_told_apart_by_code() {
        let s = Structure::compile(&mig());
        let m = segs(
            "UNH+1+UTILMD:D:11A:UN:S2.2'BGM+E01+1'NAD+MS+1'NAD+MR+2'IDE+24+V1'DTM+93:20261001:303'LOC+Z22+1'LOC+Z16+2'UNT+8+1'",
        );
        let r = s.resolve(&m);
        let nrs: Vec<Option<&str>> = (0..m.len())
            .map(|i| r.assigned[i].map(|a| s.nr(a.node).unwrap()))
            .collect();
        assert_eq!(nrs[3], Some("00009"), "NAD+MR is the second SG2");
        assert_eq!(nrs[5], Some("00021"), "DTM+93 is Ende zum");
        assert_eq!(
            nrs[6],
            Some("00048"),
            "LOC+Z22 is the ruhende Marktlokation"
        );
        // LOC+Z16 after LOC+Z22 is out of MIG order: SG5 Marktlokation comes
        // before SG5 Ruhende Marktlokation. It is skipped, not the rest of
        // the message.
        assert_eq!(nrs[7], None);
        assert_eq!(r.unresolved, vec![7]);
        assert!(nrs[8].is_some(), "the UNT after it still finds its place");
    }

    #[test]
    fn instances_form_a_tree() {
        let s = Structure::compile(&mig());
        let m = segs(
            "UNH+1+UTILMD:D:11A:UN:S2.2'BGM+E01+1'NAD+MS+1'NAD+MR+2'IDE+24+V1'LOC+Z16+1'IDE+24+V2'DTM+92:20261001:303'UNT+8+1'",
        );
        let r = s.resolve(&m);
        assert!(r.unresolved.is_empty());
        let root = &r.instances[0];
        assert_eq!(root.children.len(), 4, "SG2, SG2, SG4, SG4");
        let first_sg4 = &r.instances[root.children[2]];
        assert_eq!(first_sg4.children.len(), 1, "one SG5");
        let second_sg4 = &r.instances[root.children[3]];
        assert_eq!(second_sg4.segments.len(), 2, "IDE and DTM");
        assert_eq!(r.count(root.children[3], s.by_nr["00020"]), 1);
        assert_eq!(r.group_count(0, s.root[4]), 2);
    }
}
