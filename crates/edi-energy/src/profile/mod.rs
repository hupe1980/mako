//! A profile: one Formatversion of one message type — the MIG's
//! Nachrichtenstruktur and Segmentlayouts plus the AHB's Prüfschablonen — as
//! published by BDEW and extracted by `cargo xtask import-profiles`.
//!
//! The built-in profiles are embedded from `profiles/<type>/<fv>/{mig,ahb}.json`
//! and parsed on first use; [`Profile::from_json`] loads one from any source,
//! which is how a custom or corrected profile joins a [`ReleaseRegistry`].
//!
//! [`ReleaseRegistry`]: crate::ReleaseRegistry

pub mod conditions;
pub mod model;
pub mod skeleton;
pub mod structure;
pub mod validate;

use std::collections::HashMap;
use std::fmt;

use edifact_rs::{GroupDef, Segment, ValidationIssue};

use crate::registry::PidSource;
use crate::{MessageType, ProfileError, Pruefidentifikator, Release};
pub use model::{
    AhbProfile, Anwendungsfall, Element, ElementRule, MigProfile, Operand, Row, SegmentNode,
};
pub use skeleton::SkeletonParties;
pub use structure::{Resolution, Structure};

/// One loaded profile.
pub struct Profile {
    /// The Nachrichtenbeschreibung as extracted.
    pub mig: MigProfile,
    /// The Anwendungshandbuch as extracted.
    pub ahb: AhbProfile,
    /// The compiled Nachrichtenstruktur.
    pub structure: Structure,
    message_type: MessageType,
    release: Release,
    valid_from: Option<time::Date>,
    valid_until: Option<time::Date>,
    by_pid: HashMap<u32, usize>,
    group_schema: &'static [GroupDef<'static>],
}

impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("message_type", &self.message_type)
            .field("release", &self.release.as_str())
            .field("valid_from", &self.valid_from)
            .field("anwendungsfaelle", &self.ahb.anwendungsfaelle.len())
            .finish_non_exhaustive()
    }
}

impl Profile {
    /// Load a profile from the two JSON documents.
    ///
    /// # Errors
    ///
    /// When either document does not parse, they disagree on message type or
    /// release, or a date is malformed.
    pub fn from_json(mig: &str, ahb: &str) -> Result<Self, ProfileError> {
        let mig: MigProfile =
            serde_json::from_str(mig).map_err(|e| ProfileError::InvalidField {
                field: "mig.json",
                value: String::new(),
                reason: e.to_string(),
            })?;
        let ahb: AhbProfile =
            serde_json::from_str(ahb).map_err(|e| ProfileError::InvalidField {
                field: "ahb.json",
                value: String::new(),
                reason: e.to_string(),
            })?;
        Self::new(mig, ahb)
    }

    /// Build a profile from parsed documents.
    ///
    /// # Errors
    ///
    /// When the documents disagree on message type or release, or a date is
    /// malformed.
    pub fn new(mig: MigProfile, ahb: AhbProfile) -> Result<Self, ProfileError> {
        if mig.message_type != ahb.message_type || mig.release != ahb.release {
            return Err(ProfileError::InvalidField {
                field: "release",
                value: format!(
                    "{} {} / {} {}",
                    mig.message_type, mig.release, ahb.message_type, ahb.release
                ),
                reason: "mig.json and ahb.json describe different documents".into(),
            });
        }
        let message_type = MessageType::from_unh_code(&mig.message_type).ok_or_else(|| {
            ProfileError::InvalidField {
                field: "message_type",
                value: mig.message_type.clone(),
                reason: "not an EDI@Energy message type".into(),
            }
        })?;
        let date =
            |field: &'static str, s: &Option<String>| -> Result<Option<time::Date>, ProfileError> {
                match s {
                    None => Ok(None),
                    Some(s) => {
                        time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
                            .map(Some)
                            .map_err(|e| ProfileError::InvalidField {
                                field,
                                value: s.clone(),
                                reason: e.to_string(),
                            })
                    }
                }
            };
        let valid_from = date("valid_from", &Some(mig.valid_from.clone()))?;
        let valid_until = date("valid_until", &mig.valid_until)?;
        let structure = Structure::compile(&mig);
        let by_pid = ahb
            .anwendungsfaelle
            .iter()
            .enumerate()
            .filter_map(|(i, a)| a.pid.map(|p| (p, i)))
            .collect();
        let group_schema = leak_group_schema(&structure);
        Ok(Self {
            release: Release::new(&mig.release),
            message_type,
            valid_from,
            valid_until,
            by_pid,
            group_schema,
            structure,
            mig,
            ahb,
        })
    }

    /// The message type.
    #[must_use]
    pub fn message_type(&self) -> MessageType {
        self.message_type
    }

    /// The wire release code (`UNH` DE 0057).
    #[must_use]
    pub fn release(&self) -> &Release {
        &self.release
    }

    /// The Anwendungszeitpunkt (Allgemeine Festlegungen 6.1d §2.5).
    #[must_use]
    pub fn valid_from(&self) -> Option<time::Date> {
        self.valid_from
    }

    /// The last day the Formatversion is in force; `None` while open-ended.
    #[must_use]
    pub fn valid_until(&self) -> Option<time::Date> {
        self.valid_until
    }

    /// The AHB version, e.g. `2.2`, `3.2`, `1.1b` — a version line of its own
    /// for every message type but UTILMD.
    #[must_use]
    pub fn ahb_version(&self) -> &str {
        &self.ahb.ahb_version
    }

    /// The title of the AHB publication this profile was extracted from.
    #[must_use]
    pub fn source_document(&self) -> Option<&str> {
        self.ahb.source.title.as_deref()
    }

    /// Where messages of this type carry their Prüfidentifikator.
    #[must_use]
    pub fn pid_source(&self) -> PidSource {
        match self.mig.pid_source.as_deref() {
            Some("rff_z13") => PidSource::RffZ13,
            _ => PidSource::BgmDe1004,
        }
    }

    /// Whether the message type is published without Prüfidentifikatoren.
    #[must_use]
    pub fn pid_exempt(&self) -> bool {
        self.mig.pid_exempt || self.ahb.anwendungsfaelle.iter().all(|a| a.pid.is_none())
    }

    /// Every Anwendungsfall of the AHB.
    #[must_use]
    pub fn anwendungsfaelle(&self) -> &[Anwendungsfall] {
        &self.ahb.anwendungsfaelle
    }

    /// The Anwendungsfall of `pid`.
    #[must_use]
    pub fn anwendungsfall(&self, pid: u32) -> Option<&Anwendungsfall> {
        self.by_pid
            .get(&pid)
            .map(|&i| &self.ahb.anwendungsfaelle[i])
    }

    /// Whether the AHB carries a Prüfschablone for `pid`.
    #[must_use]
    pub fn has_anwendungsfall(&self, pid: Pruefidentifikator) -> bool {
        self.by_pid.contains_key(&pid.as_u32())
    }

    /// Every Prüfidentifikator the AHB defines, ascending.
    #[must_use]
    pub fn pruefidentifikatoren(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.by_pid.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// Run the MIG and AHB checks over a message (`UNH`…`UNT`, envelope
    /// excluded). `pid` selects the Prüfschablone; without one, a message
    /// type published without Prüfidentifikatoren is matched by its `BGM`.
    #[must_use]
    pub fn validate(
        &self,
        segments: &[Segment<'_>],
        pid: Option<Pruefidentifikator>,
    ) -> Vec<ValidationIssue> {
        validate::validate(self, segments, pid.map(Pruefidentifikator::as_u32))
    }

    /// Assign every segment of a message to its place in the
    /// Nachrichtenstruktur.
    #[must_use]
    pub fn resolve(&self, segments: &[Segment<'_>]) -> Resolution {
        self.structure.resolve(segments)
    }

    /// The Prüfschablone of `pid`, laid out in MIG order.
    #[must_use]
    pub fn pruefschablone(&self, pid: u32) -> Option<Pruefschablone<'_>> {
        let af = self.anwendungsfall(pid)?;
        Some(Pruefschablone::new(self, af))
    }

    /// Group nesting for `edifact_rs::group_segments_indexed`, used by the
    /// semantic and custom rule packs.
    #[must_use]
    pub fn group_schema(&self) -> &'static [GroupDef<'static>] {
        self.group_schema
    }
}

/// Derive the trigger-based group schema and leak it: profiles live for the
/// whole process, and edifact-rs wants `'static` definitions.
fn leak_group_schema(structure: &Structure) -> &'static [GroupDef<'static>] {
    fn build(structure: &Structure, ids: &[structure::NodeId]) -> &'static [GroupDef<'static>] {
        let mut out: Vec<GroupDef<'static>> = Vec::new();
        for &id in ids {
            let Some(group) = structure.group(id) else {
                continue;
            };
            let Some(trigger) = structure.trigger(id).and_then(|t| structure.tag(t)) else {
                continue;
            };
            if out.iter().any(|g| g.name == group && g.trigger == trigger) {
                continue;
            }
            let name: &'static str = Box::leak(group.to_owned().into_boxed_str());
            let trigger: &'static str = Box::leak(trigger.to_owned().into_boxed_str());
            let children = build(structure, &structure.nodes[id].children);
            out.push(if children.is_empty() {
                GroupDef::new(name, trigger)
            } else {
                GroupDef::with_children(name, trigger, children)
            });
        }
        Box::leak(out.into_boxed_slice())
    }
    build(structure, &structure.root)
}

/// One Anwendungsfall's Prüfschablone, laid out in MIG order — what a message
/// of that Prüfidentifikator has to contain.
#[derive(Debug)]
pub struct Pruefschablone<'p> {
    /// The Prüfidentifikator; `None` for a message type published without.
    pub pid: Option<u32>,
    /// The Anwendungsfall as the AHB titles it.
    pub name: &'p str,
    /// „Kommunikation von“, e.g. `LF an NB`.
    pub communication: Option<&'p str>,
    /// The AHB chapter the table is in.
    pub chapter: Option<&'p str>,
    /// The segments in MIG order.
    pub rows: Vec<SchablonenZeile<'p>>,
}

/// One segment of a Prüfschablone.
#[derive(Debug)]
pub struct SchablonenZeile<'p> {
    /// Groups from the root down, e.g. `["SG4", "SG8", "SG10"]`.
    pub path: Vec<&'p str>,
    /// The MIG's running segment number.
    pub nr: &'p str,
    /// The segment tag.
    pub tag: &'p str,
    /// The MIG's name for this place.
    pub name: &'p str,
    /// The group's own status when this segment opens a group.
    pub group_status: Option<&'p [String]>,
    /// The AHB statuses of the segment.
    pub status: &'p [String],
    /// The data elements the column lists.
    pub elements: Vec<ElementZeile<'p>>,
}

/// One data element of a Prüfschablone segment.
#[derive(Debug)]
pub struct ElementZeile<'p> {
    /// The data element number.
    pub de: &'p str,
    /// The MIG's name for it.
    pub name: &'p str,
    /// Which occurrence inside the segment.
    pub occurrence: u8,
    /// The operands per code, or the element's own.
    pub operands: &'p [Operand],
}

impl<'p> Pruefschablone<'p> {
    fn new(profile: &'p Profile, af: &'p Anwendungsfall) -> Self {
        let s = &profile.structure;
        let mut rows = Vec::new();
        for (id, node) in s.nodes.iter().enumerate() {
            let structure::Kind::Segment {
                nr, tag, layout, ..
            } = &node.kind
            else {
                continue;
            };
            let Some(status) = af.segment_status(nr) else {
                continue;
            };
            let seg = &s.layouts[*layout];
            let group_status = node
                .parent
                .and_then(|p| s.group(p).map(|g| (g, p)))
                .filter(|(_, p)| s.trigger(*p) == Some(id))
                .and_then(|(g, _)| af.group_status(g, nr));
            let elements = af
                .element_rules(nr)
                .filter_map(|r| {
                    seg.locate(&r.de, r.occurrence)
                        .map(|(_, _, el)| ElementZeile {
                            de: r.de.as_str(),
                            name: el.name.as_str(),
                            occurrence: r.occurrence,
                            operands: r.operands.as_slice(),
                        })
                })
                .collect();
            rows.push(SchablonenZeile {
                path: s.path(id),
                nr,
                tag,
                name: &node.name,
                group_status,
                status,
                elements,
            });
        }
        Self {
            pid: af.pid,
            name: &af.name,
            communication: af.communication.as_deref(),
            chapter: af.chapter.as_deref(),
            rows,
        }
    }
}

impl fmt::Display for Pruefschablone<'_> {
    /// Renders like the AHB column: one line per segment, indented by group.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.pid {
            Some(p) => writeln!(f, "{p} {}", self.name)?,
            None => writeln!(f, "{}", self.name)?,
        }
        if let Some(c) = self.communication {
            writeln!(f, "{c}")?;
        }
        for row in &self.rows {
            let indent = "  ".repeat(row.path.len());
            let group = row.path.last().map_or(String::new(), |g| format!("{g} "));
            if let Some(gs) = row.group_status {
                writeln!(f, "{indent}{group:<5}{:<32}{}", "", gs.join(" | "))?;
            }
            writeln!(
                f,
                "{indent}{group}{} {} {:<24} {}",
                row.tag,
                row.nr,
                truncate(row.name, 24),
                row.status.join(" | ")
            )?;
            for el in &row.elements {
                let ops: Vec<String> = el
                    .operands
                    .iter()
                    .map(|o| match &o.code {
                        Some(c) => format!("{c}={}", o.operand),
                        None => o.operand.clone(),
                    })
                    .collect();
                writeln!(
                    f,
                    "{indent}    {} {:<24} {}",
                    el.de,
                    truncate(el.name, 24),
                    ops.join(", ")
                )?;
            }
        }
        Ok(())
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}
