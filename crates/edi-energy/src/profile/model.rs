//! The profile files as written by `cargo xtask import-profiles`.
//!
//! `mig.json` is the Nachrichtenbeschreibung: the Nachrichtenstruktur as a
//! tree of segment groups and segments, each segment identified by the MIG's
//! running number `Nr` and carrying its Segmentlayout (data elements with the
//! BDEW status, format and admitted codes). `ahb.json` is the
//! Anwendungshandbuch: one Prüfschablone per Anwendungsfall, keyed by the same
//! `Nr`, with the Bedingungen the statuses cite.

use std::collections::BTreeMap;

use serde::Deserialize;

/// `mig.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct MigProfile {
    /// Profile format version; `2`.
    pub schema_version: u32,
    /// `UTILMD`, `MSCONS`, …
    pub message_type: String,
    /// The wire release code carried in `UNH` DE 0057.
    pub release: String,
    /// `Strom` or `Gas` for UTILMD, whose two MIGs share one message type.
    #[serde(default)]
    pub track: Option<String>,
    /// The Anwendungszeitpunkt (Allgemeine Festlegungen 6.1d §2.5).
    pub valid_from: String,
    /// The last day in force; absent while open-ended.
    #[serde(default)]
    pub valid_until: Option<String>,
    /// The Veröffentlichungszeitpunkt, six months before `valid_from`.
    #[serde(default)]
    pub publikationsdatum: Option<String>,
    /// The AHB version this Formatversion pairs the MIG with.
    pub ahb_version: String,
    /// `rff_z13` or `bgm_de1004`: where the Prüfidentifikator travels.
    #[serde(default)]
    pub pid_source: Option<String>,
    /// The message type is published without Prüfidentifikatoren.
    #[serde(default)]
    pub pid_exempt: bool,
    /// The MIG publication.
    pub source: Source,
    /// The Nachrichtenstruktur from `UNH` to `UNT`.
    pub structure: Vec<Node>,
    /// Layouts of the interchange envelope (`UNB`, `UNZ`), outside the message.
    #[serde(default)]
    pub envelope: Vec<SegmentNode>,
}

/// Which BDEW publication a file was extracted from.
#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    /// File name in the document mirror.
    pub file: String,
    /// The publication's title.
    #[serde(default)]
    pub title: Option<String>,
    /// Content hash of the file the profile was read from.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A node of the Nachrichtenstruktur.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Node {
    /// A segment group.
    Group(GroupNode),
    /// A segment.
    Segment(SegmentNode),
}

/// One segment group at one place in the structure. Two groups can share a
/// name and a trigger tag (`SG4 Identifikation einer Liste` and
/// `SG4 Vorgangs-Identifikation`); their trigger's codes tell them apart.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupNode {
    /// `SG4`, `SG10`, …
    pub group: String,
    /// The UN/EDIFACT position counter.
    #[serde(default)]
    pub zaehler: String,
    /// BDEW status: `M`, `R` (required), `D` (dependent), `O`, `N`.
    pub status: String,
    /// Maximum repetitions.
    pub max: u32,
    /// The MIG's name for this place.
    #[serde(default)]
    pub name: String,
    /// Trigger segment first, then the rest in MIG order.
    pub children: Vec<Node>,
}

/// One segment at one place in the structure, with its layout.
#[derive(Debug, Clone, Deserialize)]
pub struct SegmentNode {
    /// The MIG's running segment number, e.g. `00047`.
    pub nr: String,
    /// The segment tag.
    pub tag: String,
    /// The UN/EDIFACT position counter.
    #[serde(default)]
    pub zaehler: String,
    /// BDEW status: `M`, `R`, `D`, `O`, `N`.
    pub status: String,
    /// Maximum repetitions.
    pub max: u32,
    /// The MIG's name for this place.
    #[serde(default)]
    pub name: String,
    /// The Segmentlayout, in wire order.
    #[serde(default)]
    pub elements: Vec<Element>,
    /// The MIG's example segment.
    #[serde(default)]
    pub example: Option<String>,
}

/// A data element — standalone, a composite, or a composite's component.
#[derive(Debug, Clone, Deserialize)]
pub struct Element {
    /// The data element number (`3227`) or composite id (`C517`).
    pub id: String,
    /// The MIG's name.
    #[serde(default)]
    pub name: String,
    /// BDEW status: `M`, `R`, `D`, `O`, `N`.
    pub status: String,
    /// Representation, e.g. `an..35`, `n11`, `a1`.
    #[serde(default)]
    pub format: Option<String>,
    /// The MIG's Anwendung / Bemerkung text.
    #[serde(default)]
    pub note: String,
    /// Admitted codes, in MIG order.
    #[serde(default)]
    pub codes: Vec<Code>,
    /// The components when this is a composite.
    #[serde(default)]
    pub components: Vec<Element>,
}

/// An admitted code with the MIG's description.
#[derive(Debug, Clone, Deserialize)]
pub struct Code {
    /// The code value.
    pub code: String,
    /// The MIG's description.
    #[serde(default)]
    pub name: String,
}

/// `ahb.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct AhbProfile {
    /// Profile format version; `2`.
    pub schema_version: u32,
    /// `UTILMD`, `MSCONS`, …
    pub message_type: String,
    /// The wire release code the AHB belongs to.
    pub release: String,
    /// The AHB version.
    pub ahb_version: String,
    /// The AHB publication.
    pub source: Source,
    /// Bedingungen by number, text as printed: Voraussetzungen `1`–`499`,
    /// Hinweise `500`–`899`, Formatbedingungen `901`–`999`,
    /// Wiederholbarkeiten `2000`–`2499`, `UB1`–`UB3`.
    #[serde(default)]
    pub conditions: BTreeMap<String, String>,
    /// Pakete by id (`1P`) with their Paketvoraussetzung expression.
    #[serde(default)]
    pub packages: BTreeMap<String, String>,
    /// The Prüfschablonen, one per column of the AHB.
    pub anwendungsfaelle: Vec<Anwendungsfall>,
}

/// One column of the AHB — the Prüfschablone of one Anwendungsfall.
#[derive(Debug, Clone, Deserialize)]
pub struct Anwendungsfall {
    /// The Prüfidentifikator; absent for message types published without.
    #[serde(default)]
    pub pid: Option<u32>,
    /// The column title.
    pub name: String,
    /// „Kommunikation von“, e.g. `LF an NB`.
    #[serde(default)]
    pub communication: Option<String>,
    /// The AHB chapter.
    #[serde(default)]
    pub chapter: Option<String>,
    /// Segment and group statuses.
    pub rows: Vec<Row>,
    /// Data-element operands.
    #[serde(default)]
    pub elements: Vec<ElementRule>,
}

/// The AHB status of a segment (`nr`) or of a segment group (`group`,
/// printed before its trigger segment `before`).
#[derive(Debug, Clone, Deserialize)]
pub struct Row {
    /// The segment's `Nr`, for a segment row.
    #[serde(default)]
    pub nr: Option<String>,
    /// The group, for a group row.
    #[serde(default)]
    pub group: Option<String>,
    /// The trigger segment's `Nr` the group row precedes.
    #[serde(default)]
    pub before: Option<String>,
    /// `Muss`, `Muss [10]`, `Soll [3]` — several when the AHB states several.
    pub status: Vec<String>,
}

/// The operands of one data element of one segment.
#[derive(Debug, Clone, Deserialize)]
pub struct ElementRule {
    /// The segment's `Nr`.
    pub nr: String,
    /// The data element number.
    pub de: String,
    /// Which occurrence of `de` in the segment layout (`STS` repeats DE 9013).
    #[serde(default)]
    pub occurrence: u8,
    /// One entry per code the column marks, or one for the element.
    pub operands: Vec<Operand>,
}

/// `X`, `X [UB1]`, `M [7]`, `S [9P0..1]` … on a code or on the element.
#[derive(Debug, Clone, Deserialize)]
pub struct Operand {
    /// The code this operand is on; `None` for a value element.
    #[serde(default)]
    pub code: Option<String>,
    /// `X`, `X [UB1]`, `M [7]`, …
    pub operand: String,
}

impl SegmentNode {
    /// The wire coordinates `(element, component)` and the layout entry of
    /// the `occurrence`-th data element `de` in this segment.
    #[must_use]
    pub fn locate(&self, de: &str, occurrence: u8) -> Option<(usize, usize, &Element)> {
        let mut seen = 0u8;
        for (ei, el) in self.elements.iter().enumerate() {
            if el.id == de {
                if seen == occurrence {
                    return Some((ei, 0, el));
                }
                seen += 1;
            }
            for (ci, comp) in el.components.iter().enumerate() {
                if comp.id == de {
                    if seen == occurrence {
                        return Some((ei, ci, comp));
                    }
                    seen += 1;
                }
            }
        }
        None
    }

    /// Every data element with its wire coordinates, composites excluded.
    pub fn leaves(&self) -> impl Iterator<Item = (usize, usize, &Element)> {
        self.elements.iter().enumerate().flat_map(|(ei, el)| {
            if el.components.is_empty() {
                vec![(ei, 0, el)]
            } else {
                el.components
                    .iter()
                    .enumerate()
                    .map(|(ci, c)| (ei, ci, c))
                    .collect::<Vec<_>>()
            }
        })
    }
}

impl Element {
    /// Whether this element's code list is a genuine qualifier list — short
    /// codes — rather than sample values such as OBIS-Kennzahlen.
    #[must_use]
    pub fn is_code_list(&self) -> bool {
        !self.codes.is_empty()
            && self.codes.iter().all(|c| {
                c.code.chars().count() <= 8
                    && c.code
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_')
                    // `XYZ` / `xxx` stand for "any value" in a MIG layout.
                    && !c.code.eq_ignore_ascii_case("xyz")
                    && !c.code.eq_ignore_ascii_case("xxx")
            })
    }
}

impl Anwendungsfall {
    /// The statuses the column states for segment `nr`, if it lists it.
    #[must_use]
    pub fn segment_status(&self, nr: &str) -> Option<&[String]> {
        self.rows
            .iter()
            .find(|r| r.nr.as_deref() == Some(nr))
            .map(|r| r.status.as_slice())
    }

    /// The statuses the column states for the group opened before `trigger_nr`.
    #[must_use]
    pub fn group_status(&self, group: &str, trigger_nr: &str) -> Option<&[String]> {
        self.rows
            .iter()
            .find(|r| r.group.as_deref() == Some(group) && r.before.as_deref() == Some(trigger_nr))
            .map(|r| r.status.as_slice())
    }

    /// The element rules of segment `nr`.
    pub fn element_rules(&self, nr: &str) -> impl Iterator<Item = &ElementRule> {
        self.elements.iter().filter(move |e| e.nr == nr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_deserialise_untagged() {
        let json = r#"[
          {"nr":"00003","tag":"UNH","status":"M","max":1,"elements":[
            {"id":"0062","status":"M","format":"an..14"},
            {"id":"S009","status":"M","components":[{"id":"0065","status":"M","format":"an..6","codes":[{"code":"UTILMD","name":""}]}]}
          ]},
          {"group":"SG4","status":"D","max":99999,"children":[{"nr":"00018","tag":"IDE","status":"M","max":1}]}
        ]"#;
        let nodes: Vec<Node> = serde_json::from_str(json).unwrap();
        assert!(matches!(&nodes[0], Node::Segment(s) if s.tag == "UNH"));
        let Node::Segment(unh) = &nodes[0] else {
            panic!()
        };
        assert_eq!(unh.locate("0065", 0).map(|(e, c, _)| (e, c)), Some((1, 0)));
        assert!(matches!(&nodes[1], Node::Group(g) if g.group == "SG4" && g.children.len() == 1));
    }
}
