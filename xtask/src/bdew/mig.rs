//! The MIG: Nachrichtenstruktur (the tree of segment groups and segments,
//! keyed by the running segment number `Nr`) and Segmentlayout (per `Nr`, the
//! data elements with their BDEW status, format and admitted codes).

use std::collections::BTreeMap;

use regex::Regex;
use serde::Serialize;

use super::{char_pos, collapse, looks_like_code, tokens};

/// One node of the Nachrichtenstruktur.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Node {
    Group(GroupNode),
    Segment(SegmentNode),
}

/// A segment group occurrence in the structure — `SG4 Vorgangs-Identifikation`
/// and `SG4 Identifikation einer Liste` are two different groups that happen
/// to share a name and a trigger tag.
#[derive(Debug, Clone, Serialize)]
pub struct GroupNode {
    pub group: String,
    pub zaehler: String,
    pub status: String,
    pub max: u32,
    pub name: String,
    pub children: Vec<Node>,
}

/// A segment at one place in the structure, identified by its `Nr`.
#[derive(Debug, Clone, Serialize)]
pub struct SegmentNode {
    pub nr: String,
    pub tag: String,
    pub zaehler: String,
    pub status: String,
    pub max: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<Element>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

/// A data element of a segment layout — standalone or the component of a
/// composite. `position`/`component` are the wire coordinates.
#[derive(Debug, Clone, Serialize)]
pub struct Element {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Admitted codes with their description, in MIG order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<Code>,
    /// Components when this is a composite data element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Element>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Code {
    pub code: String,
    pub name: String,
}

/// The parsed document.
#[derive(Debug, Clone)]
pub struct MigDoc {
    pub structure: Vec<Node>,
    /// Layouts that are not part of the structure — the interchange envelope
    /// (`UNB`, `UNZ`) and the functional group.
    pub envelope: Vec<SegmentNode>,
}

impl MigDoc {
    /// Every segment node in document order.
    #[must_use]
    pub fn segments(&self) -> Vec<&SegmentNode> {
        fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a SegmentNode>) {
            for n in nodes {
                match n {
                    Node::Group(g) => walk(&g.children, out),
                    Node::Segment(s) => out.push(s),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.structure, &mut out);
        out
    }
}

struct StructureRow {
    zaehler: String,
    nr: Option<String>,
    bez: String,
    status: String,
    max: u32,
    level: u32,
    name: String,
}

/// Parse a MIG.
///
/// # Errors
///
/// When the Nachrichtenstruktur or the Segmentlayout section cannot be found.
/// `message_type` names the document (`UTILMD`): the `UNH` DE 0065 code
/// entry is printed at the note column, where a code is otherwise prose.
pub fn parse<S: AsRef<str>>(lines: &[S], message_type: &str) -> Result<MigDoc, String> {
    let rows = structure_rows(lines)?;
    let mut layouts = layouts(lines, message_type)?;
    let mut structure = build_tree(&rows, &mut layouts);
    // Some MIGs list the interchange envelope in the structure; the message
    // runs from UNH to UNT, the envelope is validated on its own.
    let mut envelope: Vec<SegmentNode> = layouts.into_values().collect();
    structure.retain(|n| match n {
        Node::Segment(s) if matches!(s.tag.as_str(), "UNB" | "UNZ" | "UNG" | "UNE") => {
            envelope.push(s.clone());
            false
        }
        _ => true,
    });
    envelope.sort_by(|a, b| a.nr.cmp(&b.nr));
    Ok(MigDoc {
        structure,
        envelope,
    })
}

// ── Nachrichtenstruktur ───────────────────────────────────────────────────────

fn structure_rows<S: AsRef<str>>(lines: &[S]) -> Result<Vec<StructureRow>, String> {
    // Zähler Nr? Bez Sta BDEW MaxStd MaxBdew Ebene Inhalt
    let row =
        Regex::new(r"^\s*(\d{4})\s+(\d{5})?\s*(SG\d+|[A-Z]{3})\s+([MC])\s+([MCRDNO])\s+(\d+)\s+(\d+)\s+(\d+)\s+(.*?)\s*$")
            .unwrap();
    let mut out: Vec<StructureRow> = Vec::new();
    let mut in_section = false;
    for line in lines {
        let line = line.as_ref();
        let collapsed = collapse(line);
        let t = collapsed.as_str();
        if t.contains("....") {
            continue; // table of contents
        }
        if t == "Nachrichtenstruktur" {
            in_section = true;
            continue;
        }
        if in_section && (t == "Diagramm" || t == "Segmentlayout") {
            break;
        }
        if !in_section {
            continue;
        }
        if let Some(c) = row.captures(line) {
            out.push(StructureRow {
                zaehler: c[1].to_owned(),
                nr: c.get(2).map(|m| m.as_str().to_owned()),
                bez: c[3].to_owned(),
                status: c[5].to_owned(),
                max: c[7].parse().unwrap_or(1),
                level: c[8].parse().unwrap_or(0),
                name: collapse(&c[9]),
            });
        } else if let Some(last) = out.last_mut() {
            // A wrapped name continues on a line holding text only in the
            // Inhalt column.
            let toks = tokens(line);
            if let Some(first) = toks.first()
                && first.x >= 60
                && !t.starts_with("Zähler")
                && !t.starts_with("Status")
            {
                last.name.push(' ');
                last.name.push_str(t);
            }
        }
    }
    if out.is_empty() {
        return Err("no Nachrichtenstruktur rows found".into());
    }
    Ok(out)
}

fn build_tree(rows: &[StructureRow], layouts: &mut BTreeMap<String, SegmentNode>) -> Vec<Node> {
    // A stack of open groups with the level their trigger segment sits on.
    struct Open {
        node: GroupNode,
        level: u32,
        trigger_seen: bool,
    }
    let mut root: Vec<Node> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();

    fn close_to(stack: &mut Vec<Open>, root: &mut Vec<Node>, level: u32) {
        while let Some(top) = stack.last() {
            if top.level >= level {
                let done = stack.pop().unwrap().node;
                match stack.last_mut() {
                    Some(parent) => parent.node.children.push(Node::Group(done)),
                    None => root.push(Node::Group(done)),
                }
            } else {
                break;
            }
        }
    }

    for r in rows {
        if r.bez.starts_with("SG") {
            // A sibling or outer group closes everything at or below its level;
            // a nested group (deeper level) stays inside.
            close_to(&mut stack, &mut root, r.level);
            stack.push(Open {
                node: GroupNode {
                    group: r.bez.clone(),
                    zaehler: r.zaehler.clone(),
                    status: r.status.clone(),
                    max: r.max,
                    name: r.name.clone(),
                    children: Vec::new(),
                },
                level: r.level,
                trigger_seen: false,
            });
            continue;
        }
        let Some(nr) = r.nr.clone() else { continue };
        // A segment on the trigger level that is not the trigger ends the
        // group — the structure lists a group's members one level deeper.
        if let Some(top) = stack.last() {
            if top.trigger_seen && r.level <= top.level {
                close_to(&mut stack, &mut root, r.level);
            }
        }
        let mut seg = layouts.remove(&nr).unwrap_or_else(|| SegmentNode {
            nr: nr.clone(),
            tag: r.bez.clone(),
            zaehler: String::new(),
            status: String::new(),
            max: 1,
            name: String::new(),
            elements: Vec::new(),
            example: None,
        });
        seg.tag = r.bez.clone();
        seg.zaehler = r.zaehler.clone();
        seg.status = r.status.clone();
        seg.max = r.max;
        seg.name = r.name.clone();
        match stack.last_mut() {
            Some(top) => {
                top.trigger_seen = true;
                top.node.children.push(Node::Segment(seg));
            }
            None => root.push(Node::Segment(seg)),
        }
    }
    close_to(&mut stack, &mut root, 0);
    root
}

// ── Segmentlayout ─────────────────────────────────────────────────────────────

/// Per `Nr`: the segment layout (elements, codes, example).
fn layouts<S: AsRef<str>>(
    lines: &[S],
    message_type: &str,
) -> Result<BTreeMap<String, SegmentNode>, String> {
    // Zähler Nr Bez St MaxWdh St MaxWdh Ebene Name
    // The segment tag of a layout header is typeset slightly above the row and
    // lands on the line before it; the element block repeats it anyway.
    let seg_head =
        Regex::new(r"^\s*(\d{4})\s+(\d{5})\s+(?:([A-Z]{3})\s+)?([MC])\s+(\d+)\s+([MCRDNO])\s+(\d+)\s+(\d+)\s+(.*?)\s*$")
            .unwrap();
    let group_head = Regex::new(
        r"^\s*(\d{4})\s+(?:(SG\d+)\s+)?([MC])\s+(\d+)\s+([MCRDNO])\s+(\d+)\s+(\d+)\s*(.*)$",
    )
    .unwrap();
    let bare_tag = Regex::new(r"^\s*([A-Z]{3}|SG\d+)\s*$").unwrap();
    // A long name can run up to one space before the status column, so the
    // name is delimited by the status letter and the format that follows it.
    let composite =
        Regex::new(r"^\s*([CS]\d{3})\s{2,}(.*?)\s+([MC])\s+([MCRDNO])(?:\s{2,}(.*))?$").unwrap();
    let de = Regex::new(
        r"^\s*(\d{4})\s{2,}(.*?)\s+([MC])\s+((?:an|a|n)\.?\.?\d+)\s+([MCRDNO])(?:\s+((?:an|a|n)\.?\.?\d+))?(?:\s{2,}(.*))?$",
    )
    .unwrap();
    let mut out: BTreeMap<String, SegmentNode> = BTreeMap::new();

    let mut in_section = false;
    let mut cur: Option<SegmentNode> = None;
    let mut anw_x: usize = 70;
    let mut name_x: usize = 12;
    let mut st_x: usize = 40;
    let mut pending_tag: Option<String> = None;
    // A composite sits in the `Bez` column, a standalone data element one cell
    // right of it and a component two or more cells right of it — the only
    // indentation scheme the MIGs share.
    let mut composite_x: Option<usize> = None;
    // Which element the running Anwendung / code lines belong to.
    let mut last_el: Option<(usize, Option<usize>)> = None; // (element idx, component idx)
    let mut last_code_x: Option<usize> = None;
    #[derive(PartialEq)]
    enum Mode {
        Elements,
        Remark,
        Example,
    }
    let mut mode = Mode::Elements;

    fn flush(cur: &mut Option<SegmentNode>, out: &mut BTreeMap<String, SegmentNode>) {
        if let Some(seg) = cur.take() {
            out.entry(seg.nr.clone()).or_insert(seg);
        }
    }

    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx].as_ref();
        idx += 1;
        let collapsed = collapse(line);
        let t = collapsed.as_str();
        if t.contains("....") {
            continue;
        }
        if t == "Segmentlayout" {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(c) = seg_head.captures(line) {
            flush(&mut cur, &mut out);
            let tag = c
                .get(3)
                .map(|m| m.as_str().to_owned())
                .or_else(|| pending_tag.take())
                .unwrap_or_default();
            cur = Some(SegmentNode {
                nr: c[2].to_owned(),
                tag,
                zaehler: c[1].to_owned(),
                status: c[6].to_owned(),
                max: c[7].parse().unwrap_or(1),
                name: collapse(&c[9]),
                elements: Vec::new(),
                example: None,
            });
            last_el = None;
            last_code_x = None;
            composite_x = None;
            mode = Mode::Elements;
            continue;
        }
        if group_head.is_match(line) {
            flush(&mut cur, &mut out);
            pending_tag = None;
            continue;
        }
        // A segment tag stands alone at the left edge; a three-letter word at
        // the right is a wrapped code description (`… ZE/ZRT`).
        if let Some(c) = bare_tag.captures(line)
            && tokens(line).first().is_some_and(|k| k.x + 20 < anw_x)
            && cur
                .as_ref()
                .is_none_or(|s| s.tag != c[1] || !s.elements.is_empty())
        {
            pending_tag = Some(c[1].to_owned());
            if cur.as_ref().is_some_and(|s| !s.elements.is_empty()) {
                flush(&mut cur, &mut out);
            }
            continue;
        }
        let Some(seg) = cur.as_mut() else { continue };
        if seg.tag.is_empty()
            && let Some(c) = bare_tag.captures(line)
        {
            seg.tag = c[1].to_owned();
            continue;
        }

        // The column header of a layout block (repeated after a page break).
        if t.starts_with("Bez") && t.contains("Name") && t.contains("Anwendung") {
            anw_x = char_pos(line, "Anwendung").unwrap_or(anw_x);
            name_x = char_pos(line, "Name").unwrap_or(name_x);
            st_x = char_pos(line, "St").unwrap_or(st_x);
            continue;
        }
        if t == "Standard BDEW" || t == "Standard" || t.starts_with("Zähler") {
            continue;
        }
        if t == seg.tag {
            continue;
        }
        if t.starts_with("Bemerkung:") {
            mode = Mode::Remark;
            continue;
        }
        if t.starts_with("Beispiel:") {
            mode = Mode::Example;
            continue;
        }
        match mode {
            Mode::Example => {
                // Every example line is a segment string and ends with the
                // segment terminator; prose that follows one does not.
                if t.is_empty() {
                    continue;
                }
                if !t.ends_with('\'') {
                    mode = Mode::Remark;
                    continue;
                }
                match &mut seg.example {
                    Some(e) => {
                        e.push('\n');
                        e.push_str(t);
                    }
                    None => seg.example = Some(t.to_owned()),
                }
                continue;
            }
            Mode::Remark => continue,
            Mode::Elements => {}
        }
        if t.is_empty() {
            continue;
        }
        if let Some(c) = composite.captures(line) {
            let x = line[..c.get(1).map_or(0, |m| m.start())].chars().count();
            composite_x = Some(x);
            seg.elements.push(Element {
                id: c[1].to_owned(),
                name: collapse(&c[2]),
                status: c[4].to_owned(),
                format: None,
                note: String::new(),
                codes: Vec::new(),
                components: Vec::new(),
            });
            let ei = seg.elements.len() - 1;
            last_el = Some((ei, None));
            last_code_x = None;
            if let Some(m) = c.get(5) {
                let x = line[..m.start()].chars().count();
                absorb_anwendung(
                    &mut seg.elements[ei],
                    line,
                    x,
                    anw_x,
                    message_type,
                    &mut last_code_x,
                );
            }
            continue;
        }
        if let Some(c) = de.captures(line) {
            let x = line[..c.get(1).map_or(0, |m| m.start())].chars().count();
            let is_component = composite_x.is_some_and(|cx| x >= cx + 2);
            let el = Element {
                id: c[1].to_owned(),
                name: collapse(&c[2]),
                status: c[5].to_owned(),
                format: Some(c.get(6).map_or(&c[4], |m| m.as_str()).to_owned()),
                note: String::new(),
                codes: Vec::new(),
                components: Vec::new(),
            };
            let attach_to_composite = is_component
                && seg
                    .elements
                    .last()
                    .is_some_and(|e| e.id.starts_with(['C', 'S']));
            let (ei, ci) = if attach_to_composite {
                let ei = seg.elements.len() - 1;
                let comp = &mut seg.elements[ei];
                comp.components.push(el);
                (ei, Some(comp.components.len() - 1))
            } else {
                seg.elements.push(el);
                (seg.elements.len() - 1, None)
            };
            last_el = Some((ei, ci));
            last_code_x = None;
            if let Some(m) = c.get(7) {
                let x = line[..m.start()].chars().count();
                let el = match ci {
                    Some(ci) => &mut seg.elements[ei].components[ci],
                    None => &mut seg.elements[ei],
                };
                absorb_anwendung(el, line, x, anw_x, message_type, &mut last_code_x);
            }
            continue;
        }
        // Continuation lines: a wrapped name (name column), a note or code
        // list (Anwendung column), or both on one line.
        let toks = tokens(line);
        let Some(first) = toks.first() else { continue };
        let Some((ei, ci)) = last_el else { continue };
        let el = match ci {
            Some(ci) => &mut seg.elements[ei].components[ci],
            None => &mut seg.elements[ei],
        };
        if first.x >= name_x.saturating_sub(1) && first.x < st_x {
            let name_part: Vec<&str> = toks.iter().filter(|k| k.x < st_x).map(|k| k.text).collect();
            el.name.push(' ');
            el.name.push_str(&name_part.join(" "));
        }
        if let Some(anw) = toks.iter().find(|k| k.x + 1 >= anw_x) {
            absorb_anwendung(el, line, anw.x, anw_x, message_type, &mut last_code_x);
        }
    }
    flush(&mut cur, &mut out);
    if out.is_empty() {
        return Err("no Segmentlayout blocks found".into());
    }
    Ok(out)
}

/// Classify the Anwendung / Bemerkung text starting at character `x`: a code
/// entry (indented past the note column, code token first), the wrapped
/// description of the previous code, or plain note text.
fn absorb_anwendung(
    el: &mut Element,
    line: &str,
    x: usize,
    anw_x: usize,
    message_type: &str,
    last_code_x: &mut Option<usize>,
) {
    let raw: String = line.chars().skip(x).collect();
    let text = collapse(&raw);
    if text.is_empty() {
        return;
    }
    let toks = tokens(&text);
    let first = toks[0].text;
    // A code entry sits in the code column, right of the note column — on
    // the grid the column jitters by a few cells around the first code's
    // position; a token further right is the wrapped tail of the previous
    // description. The message type itself (`UNH` DE 0065) is printed at
    // the note column in some MIGs.
    let is_code_line = (x >= anw_x + 2 || (el.id == "0065" && first == message_type))
        && looks_like_code(first)
        && last_code_x.is_none_or(|cx| x <= cx + 4);
    if is_code_line {
        let rest: String = toks
            .get(1)
            .map(|k| text.chars().skip(k.x).collect::<String>().trim().to_owned())
            .unwrap_or_default();
        el.codes.push(Code {
            code: first.to_owned(),
            name: rest,
        });
        last_code_x.get_or_insert(x);
    } else if let (Some(cx), Some(last)) = (*last_code_x, el.codes.last_mut())
        && x > cx + 4
    {
        last.name.push(' ');
        last.name.push_str(&text);
    } else if last_code_x.is_none() {
        if !el.note.is_empty() {
            el.note.push(' ');
        }
        el.note.push_str(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRUCTURE: &str = "\
 Nachrichtenstruktur
             0010 00003    UNH        M      M           1         1          0      Nachrichten-Kopfsegment
             0020 00004    BGM        M      M           1         1          0      Beginn der Nachricht
             0030 00005    DTM        M      M           9         1          1      Nachrichtendatum
             0100           SG2        C      R         99         1          1      MP-ID Absender
             0110 00008    NAD        M      M           1         1          1      MP-ID Absender
             0180           SG4        C      D        99999     99999        1      Vorgangs-Identifikation
             0190 00018     IDE       M      M           1         1          1      Vorgang
             0230 00020    DTM         C      D         99         1          2      Beginn zum
             0320           SG5        C      D        999999      1          2      Marktlokation
             0330 00047     LOC       M      M           1         1          2      Marktlokation
             0410           SG8        C      D        99999     99999        2      Daten der Marktlokation
             0420 00109     SEQ       M      M           1         1          2      Daten der Marktlokation
             0500          SG10        C      D         99         1          3      Zugeordnete Marktpartner
             0510 00117     CCI       M      M          1          1          3      Zugeordnete Marktpartner
             0520 00120     CAV        C      D         99         1          4      Messstellenbetreiber
             0180           SG4        C      D        99999     99999        1      Vorgangs-Identifikation
             0190 00500     IDE       M      M           1         1          1      Vorgang
             0900 00493    UNT        M      M           1         1          0      Nachrichten-Endesegment
 Diagramm
";

    const LAYOUT: &str = "\
 Segmentlayout
                                         Standard                 BDEW
 Zähler         Nr        Bez          St MaxWdh             St   MaxWdh           Ebene              Name

   0330     00047       LOC            M         1           M          1             2        Marktlokation

                                                Standard BDEW
 Bez        Name                             St Format       St Format         Anwendung / Bemerkung
 LOC
 3227       Ortsangabe, Qualifier            M an..3         M an..3           Mit dem Code Z16 wird die ID der Marktlokation
                                                                               beschrieben
                                                                                     Z16 Marktlokation
 C517       Ortsangabe                       C               R
   3225     Ortsangabe, Nummer               C an..35        R n11             ID der Marktlokation
   1131     Codeliste, Code                  C an..17        N                 Nicht benutzt
   3224     Ortsangabe                       C an..256       D n..6            Zeitraum-ID
Bemerkung:
Dieses Segment wird zur Angabe der ID der Marktlokation benutzt.
Beispiel:
LOC+Z16+20072281644:::2'
";

    #[test]
    fn structure_builds_the_tree_with_repeated_groups() {
        let lines: Vec<String> = STRUCTURE.lines().map(str::to_owned).collect();
        let rows = structure_rows(&lines).unwrap();
        let mut layouts = BTreeMap::new();
        let tree = build_tree(&rows, &mut layouts);
        let names: Vec<String> = tree
            .iter()
            .map(|n| match n {
                Node::Group(g) => format!("{}:{}", g.group, g.name),
                Node::Segment(s) => s.tag.clone(),
            })
            .collect();
        assert_eq!(
            names,
            [
                "UNH",
                "BGM",
                "DTM",
                "SG2:MP-ID Absender",
                "SG4:Vorgangs-Identifikation",
                "SG4:Vorgangs-Identifikation",
                "UNT"
            ]
        );
        let Node::Group(sg4) = &tree[4] else { panic!() };
        assert_eq!(sg4.children.len(), 4, "IDE, DTM, SG5, SG8");
        let Node::Group(sg8) = &sg4.children[3] else {
            panic!()
        };
        assert_eq!(sg8.children.len(), 2, "SEQ and SG10");
        let Node::Group(sg10) = &sg8.children[1] else {
            panic!()
        };
        assert_eq!(sg10.children.len(), 2, "CCI and CAV");
    }

    #[test]
    fn layout_reads_elements_formats_and_codes() {
        let lines: Vec<String> = LAYOUT.lines().map(str::to_owned).collect();
        let l = layouts(&lines, "UTILMD").unwrap();
        let loc = &l["00047"];
        assert_eq!(loc.tag, "LOC");
        assert_eq!(loc.elements.len(), 2);
        let q = &loc.elements[0];
        assert_eq!(q.id, "3227");
        assert_eq!(q.status, "M");
        assert_eq!(q.format.as_deref(), Some("an..3"));
        assert_eq!(q.codes.len(), 1);
        assert_eq!(q.codes[0].code, "Z16");
        assert!(q.note.starts_with("Mit dem Code Z16"));
        let c = &loc.elements[1];
        assert_eq!(c.id, "C517");
        assert_eq!(c.status, "R");
        assert_eq!(c.components.len(), 3);
        assert_eq!(c.components[0].format.as_deref(), Some("n11"));
        assert_eq!(c.components[1].status, "N");
        assert_eq!(loc.example.as_deref(), Some("LOC+Z16+20072281644:::2'"));
    }
}
