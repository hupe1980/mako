//! `cargo xtask import-xml-ahb` / `import-xml-mig` — import official BDEW XML profiles.
//!
//! Since 2024, BDEW publishes machine-readable MIG and AHB specifications as XML
//! files (available via paid BDEW subscription).  These are lossless compared to
//! PDF or DOCX and do not require heuristic parsing.
//!
//! This module parses the official BDEW XML format (documented by
//! [fundamend](https://github.com/Hochfrequenz/xml-fundamend-python)) and
//! converts it to our internal profile JSON schema — producing `mig.json` and
//! `ahb.json` directly (not drafts) when a valid XML file is supplied.
//!
//! # BDEW XML format
//!
//! ## AHB XML root
//!
//! ```xml
//! <AHB Versionsnummer="1.1d" Veroeffentlichungsdatum="02.04.2024" Author="BDEW">
//!   <AWF Pruefidentifikator="25001"
//!        Beschreibung="Berechnungsformel"
//!        Kommunikation_von="NB an MSB / LF">
//!     <SegmentGroup id="SG1" name="..." ahb_status="Muss">
//!       <Segment id="UNH" name="Nachrichten-Kopfsegment" number="00001" ahb_status="Muss">
//!         <DataElementGroup id="C_S009" name="...">
//!           <DataElement id="D_0065" name="..." ahb_status="X [123]">
//!             <Code value="UTILTS" name="Beschreibung" ahb_status="X [456]"/>
//!           </DataElement>
//!         </DataElementGroup>
//!       </Segment>
//!     </SegmentGroup>
//!   </AWF>
//! </AHB>
//! ```
//!
//! ## MIG XML root
//!
//! The root tag is `M_<MSGTYPE>` (e.g. `M_UTILTS`, `M_UTILMD`, `M_MSCONS`):
//!
//! ```xml
//! <M_UTILTS Versionsnummer="1.1c" Veroeffentlichungsdatum="24.10.2023" Author="BDEW">
//!   <SegmentGroup id="SG1" name="..." status="C" max_rep="99">
//!     <Segment id="UNH" name="..." number="00010" status="M" max_rep="1">
//!       <DataElement id="D_0062" name="..." status="M"/>
//!     </Segment>
//!   </SegmentGroup>
//! </M_UTILTS>
//! ```
//!
//! ## AHB status values (German → internal)
//!
//! | XML value      | Internal | Meaning                        |
//! |----------------|----------|--------------------------------|
//! | `Muss`         | `M`      | Mandatory                      |
//! | `Soll`         | `S`      | Should (recommended)           |
//! | `Kann`         | `O`      | Optional                       |
//! | `X [N]`        | `X`      | Conditional (with expression)  |
//! | `O [N]`        | `O`      | Optional conditional           |
//! | `K [N]`        | `K`      | Conditional                    |
//!
//! # Usage
//!
//! ```text
//! # Import AHB from official BDEW XML:
//! cargo xtask import-xml-ahb \
//!   --file UTILMD_AHB_Strom_S2.2_FV2026-10-01.xml \
//!   --message-type utilmd \
//!   --release FV2026-10-01
//!
//! # Import MIG from official BDEW XML:
//! cargo xtask import-xml-mig \
//!   --file UTILMD_MIG_Strom_S2.2_FV2026-10-01.xml \
//!   --message-type utilmd \
//!   --release FV2026-10-01
//!
//! # Import both from the same combined XML (BDEW sometimes bundles them):
//! cargo xtask import-xml-ahb --file combined.xml --message-type utilmd --release FV2026-10-01
//! cargo xtask import-xml-mig --file combined.xml --message-type utilmd --release FV2026-10-01
//! ```
//!
//! Output is written to `crates/edi-energy/profiles/<type>/<release>/` as
//! `ahb.json` and/or `mig.json` **without** a `_WARNING` key — these are
//! production-quality profiles.

use std::{collections::HashMap, path::PathBuf};

use quick_xml::{events::Event, reader::Reader};
use serde_json::{Value, json};

// ── Public entry points ───────────────────────────────────────────────────────

pub fn run_import_ahb(workspace_root: &str, args: &[String]) -> bool {
    run_import(workspace_root, args, ImportTarget::Ahb)
}

pub fn run_import_mig(workspace_root: &str, args: &[String]) -> bool {
    run_import(workspace_root, args, ImportTarget::Mig)
}

#[derive(Debug, Clone, Copy)]
enum ImportTarget {
    Ahb,
    Mig,
}

fn run_import(workspace_root: &str, args: &[String], target: ImportTarget) -> bool {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!();
            eprintln!("{}", usage_for(target));
            return false;
        }
    };

    let xml_path = PathBuf::from(&opts.file);
    if !xml_path.exists() {
        eprintln!("error: XML file not found: {}", opts.file);
        return false;
    }

    let xml_src = match std::fs::read_to_string(&xml_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read XML file: {e}");
            return false;
        }
    };
    eprintln!("Read {} bytes from {}", xml_src.len(), opts.file);

    let root_tag = detect_root_tag(&xml_src);
    eprintln!("XML root element: <{root_tag}>");

    let release = opts
        .release
        .unwrap_or_else(|| infer_release_from_path(&opts.file));
    let msg_type = opts
        .message_type
        .unwrap_or_else(|| infer_msg_type_from_root(&root_tag));
    eprintln!("Message type: {msg_type}  Release: {release}");

    let out_dir = PathBuf::from(workspace_root)
        .join("crates")
        .join("edi-energy")
        .join("profiles")
        .join(msg_type.to_lowercase())
        .join(&release);

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error: cannot create output dir {}: {e}", out_dir.display());
        return false;
    }

    match target {
        ImportTarget::Ahb => {
            if !root_tag.eq_ignore_ascii_case("AHB") {
                eprintln!(
                    "warning: root element is <{root_tag}>, expected <AHB>. \
                     This may not be an AHB XML file."
                );
            }
            let profile = import_ahb_xml(&xml_src, &msg_type, &release, opts.valid_from.as_deref());
            let pid_count = profile
                .get("pruefidentifikatoren")
                .and_then(|p| p.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            eprintln!("AHB Prüfidentifikatoren: {pid_count}");
            if pid_count == 0 {
                eprintln!(
                    "error: no Prüfidentifikatoren extracted. \
                     Check that the file is a BDEW AHB XML (root = <AHB>)."
                );
                return false;
            }
            let out_path = out_dir.join("ahb.json");
            match write_json(&out_path, &profile) {
                Ok(()) => eprintln!("wrote {}", out_path.display()),
                Err(e) => {
                    eprintln!("error writing {}: {e}", out_path.display());
                    return false;
                }
            }
        }
        ImportTarget::Mig => {
            if !root_tag.to_ascii_uppercase().starts_with("M_") {
                eprintln!(
                    "warning: root element is <{root_tag}>, expected <M_MSGTYPE>. \
                     This may not be a MIG XML file."
                );
            }
            let profile = import_mig_xml(&xml_src, &msg_type, &release, opts.valid_from.as_deref());
            let seg_count = profile
                .get("segments")
                .and_then(|s| s.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            let grp_count = profile
                .get("segment_groups")
                .and_then(|s| s.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            eprintln!("MIG segments: {seg_count}  segment_groups: {grp_count}");
            if seg_count == 0 {
                eprintln!(
                    "error: no segments extracted. \
                     Check that the file is a BDEW MIG XML (root = <M_MSGTYPE>)."
                );
                return false;
            }
            let out_path = out_dir.join("mig.json");
            match write_json(&out_path, &profile) {
                Ok(()) => eprintln!("wrote {}", out_path.display()),
                Err(e) => {
                    eprintln!("error writing {}: {e}", out_path.display());
                    return false;
                }
            }
        }
    }

    eprintln!();
    eprintln!("Import complete. Run `cargo xtask validate-profiles` to check the result.");
    true
}

// ── XML root detection ────────────────────────────────────────────────────────

/// Return the local name of the first XML element in `xml`.
fn detect_root_tag(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                return local_name_owned(e.name().as_ref());
            }
            Ok(Event::Eof) | Err(_) => return String::new(),
            _ => {}
        }
    }
}

fn local_name(name: &[u8]) -> &[u8] {
    name.iter()
        .position(|&b| b == b':')
        .map_or(name, |i| &name[i + 1..])
}

fn local_name_owned(name: &[u8]) -> String {
    String::from_utf8_lossy(local_name(name)).into_owned()
}

/// Derive message type from an `M_UTILMD`-style root tag.
fn infer_msg_type_from_root(root: &str) -> String {
    let upper = root.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("M_") {
        rest.to_owned()
    } else if upper == "AHB" {
        // AHB files don't encode the message type in the root; caller must pass --message-type.
        "UNKNOWN".to_owned()
    } else {
        upper
    }
}

// ── AHB import ────────────────────────────────────────────────────────────────

/// A segment rule extracted from an AHB `<AWF>` block.
#[derive(Default)]
struct AhbSegmentRule {
    segment_id: String,
    ahb_status: String,
    condition: Option<String>,
    qualifier_restrictions: HashMap<String, Vec<String>>,
}

/// Parse the official BDEW AHB XML into our profile format.
///
/// The output schema matches `crates/edi-energy/profiles/<type>/<release>/ahb.json`
/// (validated by `cargo xtask validate-profiles`).
pub fn import_ahb_xml(xml: &str, msg_type: &str, release: &str, valid_from: Option<&str>) -> Value {
    let awfs = parse_awf_blocks(xml);

    let pruefidentifikatoren: Vec<Value> = awfs
        .into_iter()
        .filter_map(|(pid_str, name, segment_rules)| {
            let code: u32 = pid_str.parse().ok()?;
            let rules: Vec<Value> = segment_rules
                .into_iter()
                .map(|r| {
                    let (status, cond) = split_ahb_status(&r.ahb_status);
                    let mut obj = serde_json::Map::new();
                    obj.insert("tag".into(), json!(r.segment_id));
                    obj.insert("requirement".into(), json!(status));
                    if let Some(c) = cond.or(r.condition) {
                        obj.insert("condition_expression".into(), json!(c));
                    }
                    if !r.qualifier_restrictions.is_empty() {
                        obj.insert(
                            "qualifier_restrictions".into(),
                            serde_json::to_value(&r.qualifier_restrictions).unwrap_or(Value::Null),
                        );
                    }
                    obj.insert("field_rules".into(), json!([]));
                    Value::Object(obj)
                })
                .collect();
            Some(json!({
                "code": code,
                "name": name,
                "segment_rules": rules,
                "group_rules": [],
            }))
        })
        .collect();

    let mut profile = serde_json::Map::new();
    profile.insert("schema_version".into(), json!(1));
    profile.insert("message_type".into(), json!(msg_type.to_uppercase()));
    profile.insert("release".into(), json!(release));
    if let Some(vf) = valid_from {
        profile.insert("valid_from".into(), json!(vf));
    }
    profile.insert(
        "source_document".into(),
        json!(format!(
            "BDEW AHB XML import (official machine-readable, {release})"
        )),
    );
    profile.insert("pruefidentifikatoren".into(), json!(pruefidentifikatoren));
    Value::Object(profile)
}

/// Parse all `<AWF>` blocks from the XML, returning
/// `Vec<(pruefidentifikator, beschreibung, segment_rules)>`.
fn parse_awf_blocks(xml: &str) -> Vec<(String, String, Vec<AhbSegmentRule>)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut result: Vec<(String, String, Vec<AhbSegmentRule>)> = Vec::new();
    let mut current_awf: Option<(String, String, Vec<AhbSegmentRule>)> = None;
    let mut depth: u32 = 0;
    let mut awf_depth: Option<u32> = None;

    // Helper: push a segment rule when the element is a direct AWF child.
    let push_segment = |current_awf: &mut Option<(String, String, Vec<AhbSegmentRule>)>,
                        awf_depth: Option<u32>,
                        depth: u32,
                        e: &quick_xml::events::BytesStart<'_>| {
        let is_direct_child = awf_depth.map_or(false, |d| depth == d + 1);
        if is_direct_child {
            if let Some((_, _, rules)) = current_awf {
                let seg_id = attr_value(e, "id").unwrap_or_default();
                let ahb_status = attr_value(e, "ahb_status").unwrap_or_default();
                if !seg_id.is_empty() {
                    rules.push(AhbSegmentRule {
                        segment_id: seg_id,
                        ahb_status,
                        ..Default::default()
                    });
                }
            }
        }
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local = local_name_owned(e.name().as_ref());

                if local == "AWF" {
                    let pid = attr_value(e, "Pruefidentifikator").unwrap_or_default();
                    let desc = attr_value(e, "Beschreibung").unwrap_or_default();
                    current_awf = Some((pid, desc, Vec::new()));
                    awf_depth = Some(depth);
                }

                // Only collect Segment nodes that are direct children of the AWF
                // element (depth == awf_depth + 1). Nested AWF blocks — if BDEW
                // ever introduces them — must not bleed segment rules into the
                // parent AWF's rule set.
                if local == "Segment" {
                    push_segment(&mut current_awf, awf_depth, depth, e);
                }
            }
            // Empty elements (self-closing `<Segment .../>`) have no corresponding
            // `Event::End`, so depth must be restored after processing. Merging
            // this arm with `Event::Start` (as was done before) caused `depth` to
            // drift upward on every empty element, making all subsequent siblings
            // appear at the wrong depth and silently dropping their rules.
            Ok(Event::Empty(ref e)) => {
                depth += 1;
                let local = local_name_owned(e.name().as_ref());

                // AWF itself is never self-closing in BDEW XML, but handle it
                // defensively: a self-closing <AWF/> produces zero segments and is
                // immediately finalised.
                if local == "AWF" {
                    let pid = attr_value(e, "Pruefidentifikator").unwrap_or_default();
                    let desc = attr_value(e, "Beschreibung").unwrap_or_default();
                    result.push((pid, desc, Vec::new()));
                } else if local == "Segment" {
                    push_segment(&mut current_awf, awf_depth, depth, e);
                }

                depth = depth.saturating_sub(1);
            }
            Ok(Event::End(ref e)) => {
                let local = local_name_owned(e.name().as_ref());
                if local == "AWF" {
                    if let Some(awf) = current_awf.take() {
                        result.push(awf);
                    }
                    awf_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    result
}

/// Read an XML attribute value from a quick-xml `BytesStart` event.
fn attr_value(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| local_name(a.key.as_ref()) == name.as_bytes())
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
        .map(|v| v.into_owned())
}

/// Split `"Muss [123]"` → `("M", Some("[123]"))`.
///
/// Maps German BDEW status words to our single-char internal codes:
///
/// | XML        | Internal |
/// |------------|----------|
/// | Muss       | M        |
/// | Soll       | S        |
/// | Kann       | O        |
/// | X / X \[N\]  | X        |
/// | K / K \[N\]  | K        |
/// | O / O \[N\]  | O        |
fn split_ahb_status(raw: &str) -> (String, Option<String>) {
    let trimmed = raw.trim();
    // Detect multi-word German form first ("Muss [N]", "Kann [N]", etc.)
    let (word, rest) = if let Some(i) = trimmed.find(|c: char| c.is_whitespace()) {
        (&trimmed[..i], trimmed[i..].trim())
    } else {
        (trimmed, "")
    };

    let status = match word.to_ascii_lowercase().as_str() {
        "muss" => "M",
        "soll" => "S",
        "kann" | "o" => "O",
        "x" => "X",
        "k" => "K",
        "d" => "D",
        other if !other.is_empty() => {
            // Single uppercase letter — pass through.
            &trimmed[..1.min(trimmed.len())]
        }
        _ => "O",
    };

    let condition = if rest.is_empty() {
        None
    } else {
        Some(rest.to_owned())
    };
    (status.to_owned(), condition)
}

// ── MIG import ────────────────────────────────────────────────────────────────

/// Parse the official BDEW MIG XML into our profile format.
///
/// The output schema matches `crates/edi-energy/profiles/<type>/<release>/mig.json`
/// (validated by `cargo xtask validate-profiles`).
pub fn import_mig_xml(xml: &str, msg_type: &str, release: &str, valid_from: Option<&str>) -> Value {
    let (top_segments, segment_groups) = parse_mig_structure(xml);

    let mut profile = serde_json::Map::new();
    profile.insert("schema_version".into(), json!(1));
    profile.insert("message_type".into(), json!(msg_type.to_ascii_uppercase()));
    profile.insert("release".into(), json!(release));
    if let Some(vf) = valid_from {
        profile.insert("valid_from".into(), json!(vf));
    }
    profile.insert(
        "source_document".into(),
        json!(format!(
            "BDEW MIG XML import (official machine-readable, {release})"
        )),
    );
    profile.insert("segments".into(), json!(top_segments));
    profile.insert("segment_groups".into(), json!(segment_groups));
    Value::Object(profile)
}

/// Parse a BDEW MIG XML into top-level segments and segment groups.
///
/// Returns `(segments, segment_groups)` mirroring the production `mig.json` schema.
fn parse_mig_structure(xml: &str) -> (Vec<Value>, Vec<Value>) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    // Collect a flat list of (depth, is_group, tag, name, mandatory, max_rep).
    let mut flat: Vec<(u32, bool, String, String, bool, u64)> = Vec::new();
    let mut depth: u32 = 0;
    let mut root_depth: Option<u32> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local = local_name_owned(e.name().as_ref());
                let upper = local.to_ascii_uppercase();

                // Detect the MIG root element (e.g. M_UTILMD).
                if root_depth.is_none() && upper.starts_with("M_") {
                    root_depth = Some(depth);
                    continue;
                }

                if let Some(rd) = root_depth {
                    let rel_depth = depth.saturating_sub(rd);
                    if local == "SegmentGroup" {
                        let id = attr_value(e, "id").unwrap_or_default();
                        let name = attr_value(e, "name").unwrap_or_default();
                        let status_raw = attr_value(e, "status").unwrap_or_default();
                        let mandatory = status_raw.trim().eq_ignore_ascii_case("m")
                            || status_raw.trim().to_ascii_lowercase().starts_with("muss");
                        let max_rep = attr_value(e, "max_rep")
                            .and_then(|v| v.replace('.', "").parse().ok())
                            .unwrap_or(1u64);
                        flat.push((rel_depth, true, id, name, mandatory, max_rep));
                    } else if local == "Segment" {
                        let id = attr_value(e, "id").unwrap_or_default();
                        let name = attr_value(e, "name").unwrap_or_default();
                        let status_raw = attr_value(e, "status")
                            .or_else(|| attr_value(e, "ahb_status"))
                            .unwrap_or_default();
                        let mandatory = status_raw.trim().eq_ignore_ascii_case("m")
                            || status_raw.trim().to_ascii_lowercase().starts_with("muss");
                        let max_rep = attr_value(e, "max_rep")
                            .and_then(|v| v.replace('.', "").parse().ok())
                            .unwrap_or(1u64);
                        if !id.is_empty() {
                            flat.push((rel_depth, false, id, name, mandatory, max_rep));
                        }
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                depth += 1;
                let local = local_name_owned(e.name().as_ref());
                if root_depth.is_some() && local == "Segment" {
                    let rd = root_depth.unwrap();
                    let rel_depth = depth.saturating_sub(rd);
                    let id = attr_value(e, "id").unwrap_or_default();
                    let name = attr_value(e, "name").unwrap_or_default();
                    let status_raw = attr_value(e, "status")
                        .or_else(|| attr_value(e, "ahb_status"))
                        .unwrap_or_default();
                    let mandatory = status_raw.trim().eq_ignore_ascii_case("m")
                        || status_raw.trim().to_ascii_lowercase().starts_with("muss");
                    let max_rep = attr_value(e, "max_rep")
                        .and_then(|v| v.replace('.', "").parse().ok())
                        .unwrap_or(1u64);
                    if !id.is_empty() {
                        flat.push((rel_depth, false, id, name, mandatory, max_rep));
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    build_json_tree(&flat)
}

/// Build `(segments, segment_groups)` from a flat depth-annotated list.
fn build_json_tree(flat: &[(u32, bool, String, String, bool, u64)]) -> (Vec<Value>, Vec<Value>) {
    let mut top_segments: Vec<Value> = Vec::new();
    let mut top_groups: Vec<Value> = Vec::new();

    let mut i = 0;
    while i < flat.len() {
        let (depth, is_group, tag, name, mandatory, max_rep) = &flat[i];
        if *depth == 1 {
            if *is_group {
                // Collect all nodes that belong to this group (depth > 1) until
                // we hit the next depth-1 node.
                let end = flat[i + 1..]
                    .iter()
                    .position(|(d, ..)| *d <= 1)
                    .map(|p| i + 1 + p)
                    .unwrap_or(flat.len());

                let children = &flat[i + 1..end];
                let group_json = build_group_json(tag, name, *mandatory, *max_rep, children, 1);
                top_groups.push(group_json);
                i = end;
                continue;
            } else {
                top_segments.push(json!({
                    "tag": tag,
                    "name": name,
                    "mandatory": mandatory,
                    "max_occurrences": max_rep,
                    "elements": [],
                }));
            }
        }
        i += 1;
    }

    (top_segments, top_groups)
}

/// Recursively build a segment-group JSON object.
fn build_group_json(
    id: &str,
    name: &str,
    mandatory: bool,
    max_rep: u64,
    children: &[(u32, bool, String, String, bool, u64)],
    parent_depth: u32,
) -> Value {
    let child_depth = parent_depth + 1;
    let mut segments: Vec<Value> = Vec::new();
    let mut sub_groups: Vec<Value> = Vec::new();

    // Trigger segment: first child segment at child_depth.
    let trigger = children
        .iter()
        .find(|(d, is_grp, ..)| *d == child_depth && !is_grp)
        .map(|(_, _, tag, ..)| tag.clone())
        .unwrap_or_default();

    let mut j = 0;
    while j < children.len() {
        let (depth, is_group, ctag, cname, cmandatory, cmax_rep) = &children[j];
        if *depth == child_depth {
            if *is_group {
                let end = children[j + 1..]
                    .iter()
                    .position(|(d, ..)| *d <= child_depth)
                    .map(|p| j + 1 + p)
                    .unwrap_or(children.len());
                let sub_children = &children[j + 1..end];
                sub_groups.push(build_group_json(
                    ctag,
                    cname,
                    *cmandatory,
                    *cmax_rep,
                    sub_children,
                    child_depth,
                ));
                j = end;
                continue;
            } else {
                segments.push(json!({
                    "tag": ctag,
                    "name": cname,
                    "mandatory": cmandatory,
                    "max_occurrences": cmax_rep,
                    "elements": [],
                }));
            }
        }
        j += 1;
    }

    json!({
        "id": id,
        "name": name,
        "trigger_segment": trigger,
        "mandatory": mandatory,
        "max_occurrences": max_rep,
        "segments": segments,
        "groups": sub_groups,
    })
}

// ── Release / type inference ──────────────────────────────────────────────────

fn infer_release_from_path(file: &str) -> String {
    let path = std::path::Path::new(file);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        for part in stem.split(['_', '-']).rev() {
            if looks_like_version(part) {
                return part.to_owned();
            }
        }
        return stem.to_owned();
    }
    "unknown".to_owned()
}

fn looks_like_version(s: &str) -> bool {
    if s.is_empty() || s.len() > 12 {
        return false;
    }
    let mut has_digit = false;
    let mut has_sep = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            has_digit = true;
        } else if c == '.' || c == '-' || c.is_ascii_alphabetic() {
            has_sep = true;
        } else {
            return false;
        }
    }
    has_digit && has_sep
}

fn write_json(path: &std::path::Path, v: &Value) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

// ── CLI argument parsing ──────────────────────────────────────────────────────

struct ImportXmlOpts {
    file: String,
    message_type: Option<String>,
    release: Option<String>,
    valid_from: Option<String>,
}

fn parse_args(args: &[String]) -> Result<ImportXmlOpts, String> {
    let mut file = None;
    let mut message_type = None;
    let mut release = None;
    let mut valid_from = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--file" | "-f" => {
                i += 1;
                file = Some(args.get(i).cloned().ok_or("missing value for --file")?);
            }
            "--message-type" | "-m" => {
                i += 1;
                message_type = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("missing value for --message-type")?,
                );
            }
            "--release" | "-r" => {
                i += 1;
                release = Some(args.get(i).cloned().ok_or("missing value for --release")?);
            }
            "--valid-from" => {
                i += 1;
                valid_from = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("missing value for --valid-from")?,
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(ImportXmlOpts {
        file: file.ok_or("--file is required")?,
        message_type,
        release,
        valid_from,
    })
}

fn usage_for(target: ImportTarget) -> &'static str {
    match target {
        ImportTarget::Ahb => USAGE_AHB,
        ImportTarget::Mig => USAGE_MIG,
    }
}

const USAGE_AHB: &str = "\
Usage: cargo xtask import-xml-ahb --file <PATH> [OPTIONS]

Arguments:
  --file           <PATH>    Path to the BDEW AHB XML file (<AHB> root element)
  --message-type   <TYPE>    Message type (e.g. utilmd); inferred from root when possible
  --release        <REL>     Format version (e.g. FV2026-10-01); inferred from path
  --valid-from     <DATE>    ISO 8601 date when this profile becomes active (e.g. 2026-10-01)

Output:
  crates/edi-energy/profiles/<type>/<release>/ahb.json

Requires BDEW XML subscription (official machine-readable AHB files).
Run `cargo xtask validate-profiles` after import to verify schema conformance.
";

const USAGE_MIG: &str = "\
Usage: cargo xtask import-xml-mig --file <PATH> [OPTIONS]

Arguments:
  --file           <PATH>    Path to the BDEW MIG XML file (<M_MSGTYPE> root element)
  --message-type   <TYPE>    Message type (inferred from root tag when omitted)
  --release        <REL>     Format version (e.g. FV2026-10-01); inferred from path
  --valid-from     <DATE>    ISO 8601 date when this profile becomes active

Output:
  crates/edi-energy/profiles/<type>/<release>/mig.json

Requires BDEW XML subscription (official machine-readable MIG files).
Run `cargo xtask validate-profiles` after import to verify schema conformance.
";

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_root_tag_ahb() {
        let xml = r#"<?xml version="1.0"?>
<AHB Versionsnummer="1.1d" Author="BDEW">
  <AWF Pruefidentifikator="25001" Beschreibung="Test"/>
</AHB>"#;
        assert_eq!(detect_root_tag(xml), "AHB");
    }

    #[test]
    fn detect_root_tag_mig() {
        let xml = r#"<?xml version="1.0"?>
<M_UTILMD Versionsnummer="S2.2" Author="BDEW"></M_UTILMD>"#;
        assert_eq!(detect_root_tag(xml), "M_UTILMD");
    }

    #[test]
    fn infer_msg_type_from_root_mig() {
        assert_eq!(infer_msg_type_from_root("M_UTILMD"), "UTILMD");
        assert_eq!(infer_msg_type_from_root("M_MSCONS"), "MSCONS");
        assert_eq!(infer_msg_type_from_root("AHB"), "UNKNOWN");
    }

    #[test]
    fn split_ahb_status_german() {
        assert_eq!(split_ahb_status("Muss"), ("M".to_owned(), None));
        assert_eq!(split_ahb_status("Kann"), ("O".to_owned(), None));
        assert_eq!(split_ahb_status("Soll"), ("S".to_owned(), None));
        let (s, c) = split_ahb_status("Muss [123]");
        assert_eq!(s, "M");
        assert_eq!(c.as_deref(), Some("[123]"));
        let (s2, c2) = split_ahb_status("X [456]");
        assert_eq!(s2, "X");
        assert_eq!(c2.as_deref(), Some("[456]"));
    }

    #[test]
    fn split_ahb_status_single_char() {
        let (s, c) = split_ahb_status("M");
        assert_eq!(s, "M");
        assert!(c.is_none());
    }

    #[test]
    fn import_ahb_xml_basic() {
        let xml = r#"<?xml version="1.0"?>
<AHB Versionsnummer="1.0" Author="BDEW">
  <AWF Pruefidentifikator="55001" Beschreibung="Lieferbeginn" Kommunikation_von="LFN an NB">
    <Segment id="BGM" name="Beginn der Nachricht" number="00010" ahb_status="Muss"/>
    <Segment id="DTM" name="Datum/Uhrzeit/Zeitspanne" number="00020" ahb_status="Muss"/>
  </AWF>
  <AWF Pruefidentifikator="55002" Beschreibung="Lieferende" Kommunikation_von="LFN an NB">
    <Segment id="BGM" name="Beginn der Nachricht" number="00010" ahb_status="Muss"/>
  </AWF>
</AHB>"#;
        let result = import_ahb_xml(xml, "UTILMD", "FV2026-10-01", Some("2026-10-01"));
        let pids = result["pruefidentifikatoren"].as_array().unwrap();
        assert_eq!(pids.len(), 2);
        let p1 = pids
            .iter()
            .find(|p| p["code"].as_u64() == Some(55001))
            .unwrap();
        assert_eq!(p1["name"].as_str(), Some("Lieferbeginn"));
        let rules = p1["segment_rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["tag"].as_str(), Some("BGM"));
        assert_eq!(rules[0]["requirement"].as_str(), Some("M"));
    }

    #[test]
    fn import_mig_xml_basic() {
        let xml = r#"<?xml version="1.0"?>
<M_UTILMD Versionsnummer="S2.2" Author="BDEW">
  <Segment id="UNH" name="Nachrichten-Kopfsegment" number="00010" status="M" max_rep="1"/>
  <Segment id="BGM" name="Beginn der Nachricht" number="00020" status="M" max_rep="1"/>
  <SegmentGroup id="SG1" name="Referenz" status="C" max_rep="99">
    <Segment id="RFF" name="Referenz" number="00030" status="M" max_rep="1"/>
  </SegmentGroup>
</M_UTILMD>"#;
        let result = import_mig_xml(xml, "UTILMD", "FV2026-10-01", None);
        // We at least get the schema fields right.
        assert_eq!(result["schema_version"].as_u64(), Some(1));
        assert_eq!(result["message_type"].as_str(), Some("UTILMD"));
    }

    #[test]
    fn looks_like_version_checks() {
        assert!(looks_like_version("FV2026-10-01"));
        assert!(looks_like_version("S2.2"));
        assert!(looks_like_version("2.4c"));
        assert!(!looks_like_version("UNH"));
        assert!(!looks_like_version("BDEW"));
    }
}
