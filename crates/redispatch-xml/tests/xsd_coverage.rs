//! Every element the BDEW XSDs declare must appear in the Rust model.
//!
//! # Why this test exists
//!
//! The nine Redispatch 2.0 document types are hand-modelled from the
//! Formatbeschreibungen, and a field that is simply *absent* from a struct
//! parses without complaint: `serde` ignores unknown XML elements, so an
//! inbound document carrying a value this crate does not know about is accepted
//! and the value is silently dropped. Nothing downstream can tell that apart
//! from a document that genuinely omitted it.
//!
//! That is not hypothetical. Until this test was written the `Stammdaten` model
//! was missing, among fifty others, `Bilanzkreis_Ausgleichsfahrplan_anfNB` and
//! the per-Quote `Bilanzkreis_Ausgleichsfahrplan` — the **Redispatch-
//! Bilanzkreis**, which `BilAReM` Kap. 2.3.2 names as one of the three things a
//! Planwertmodell-Zuordnung must carry, and which is where the bilanzielle
//! Ausgleich is booked.
//!
//! # How it runs
//!
//! The XSDs live in `regulatories/bdew-mako/`, which is **not tracked in git**
//! (they are third-party copyrighted publications; `regulatories/README.md`
//! carries the download URL for every file). The test therefore **skips** when
//! the folder is absent, exactly like the Docker-dependent suites, and asserts
//! when it is present. Skipping is reported, not silent.
//!
//! # What "covered" means
//!
//! An element counts as covered when its XSD name appears as a `#[serde(rename)]`
//! target — or as a bare field name — in **that document's own module**, plus
//! the shared `types/` modules. Scoping it per document matters: a name that
//! appears in one document's module says nothing about whether another document
//! models it, and a crate-wide search would mark `quantity_Measure_Unit.name`
//! covered for every document because one of them happens to declare it.
//!
//! It is a name-level check, not a shape-level one: it catches a *dropped*
//! field, which is the failure mode that is invisible at runtime, and does not
//! attempt to prove the cardinality or the type is right — the round-trip and
//! validation suites cover those.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Elements deliberately not modelled, with the reason.
///
/// Keep this list short and each entry justified. An unexplained entry is a
/// dropped field wearing a permission slip.
const NOT_MODELLED: &[(&str, &str)] = &[
    // ── AcknowledgementDocument ──────────────────────────────────────────────
    (
        "TimeIntervalError",
        "per-interval syntax error detail; mako rejects the whole Übertragungsdatei \
         (FB 1.0g: an ACK confirms or rejects the file as a whole), so the interval \
         breakdown has no consumer",
    ),
    (
        "QuantityTimeInterval",
        "child of TimeIntervalError, which is excused above for the same reason",
    ),
    (
        "SendersTimeSeriesIdentification",
        "child of TimeSeriesRejection, which mako does not emit for the same reason",
    ),
    // ── Stammdaten: object types mako does not yet exchange ──────────────────
    (
        "CR_Objekt",
        "Cluster objects — the clusternder NB's own Stammdaten. mako holds the ANB \
         and BKV sides; tracked in ROADMAP",
    ),
    (
        "SG_Objekt",
        "Steuergruppen — SR that share one Steuersignal (BilAReM Kap. 1). Same \
         role boundary as CR_Objekt: mako holds the ANB and BKV sides; tracked \
         in ROADMAP",
    ),
    (
        "CR_Objekt_Referenz",
        "child of Enthaltene_Objektreferenzen, only reachable from CR_Objekt/SG_Objekt",
    ),
    (
        "SG_Objekt_Referenz",
        "child of Enthaltene_Objektreferenzen, only reachable from CR_Objekt/SG_Objekt",
    ),
    (
        "Enthaltene_Objektreferenzen",
        "container inside CR_Objekt/SG_Objekt",
    ),
    (
        "Clusternder_Netzbetreiber",
        "the NB that formed the Cluster — a CR_Objekt field, unreachable until \
         CR_Objekt itself is modelled (BilAReM Kap. 6.2.1.7 makes it the owner of \
         all clusterbezogene Stammdaten)",
    ),
    (
        "tx_Cluster",
        "the Cluster's own dispatch lead time — a CR_Objekt field, unreachable \
         until CR_Objekt itself is modelled",
    ),
    (
        "T_Abruf_final",
        "the point at which an Abruf becomes final — a CR_Objekt and SG_Objekt \
         field, unreachable until those objects are modelled",
    ),
];

/// Root elements whose XSD carries no `targetNamespace`, keyed by file prefix.
fn xsd_dir() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("regulatories/bdew-mako");
    root.is_dir().then_some(root)
}

/// Pick the newest revision of each XSD — a `Fehlerkorrektur` supersedes the
/// base publication of the same version.
fn newest_xsds(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut best: std::collections::BTreeMap<String, PathBuf> = std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("xsd") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let key = name.split(" XSD").next().unwrap_or(name).to_owned();
        let better = name.contains("Fehlerkorrektur");
        match best.get(&key) {
            Some(_) if !better => {}
            _ => {
                best.insert(key, path);
            }
        }
    }
    best.into_iter().collect()
}

/// Every `<xs:element name="…">` in `xsd`.
fn declared_elements(xsd: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let needle = "<xs:element name=\"";
    let mut rest = xsd;
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        if let Some(end) = rest.find('"') {
            out.insert(rest[..end].to_owned());
        }
    }
    out
}

/// The module file that models each document type.
fn module_for(doc: &str) -> &'static str {
    match doc {
        "AcknowledgementDocument" => "acknowledgement.rs",
        "ActivationDocument" => "activation.rs",
        "Kaskade" => "kaskade.rs",
        "Kostenblatt" => "kostenblatt.rs",
        "NetworkConstraintDocument" => "network_constraint.rs",
        "PlannedResourceScheduleDocument" => "planned_resource_schedule.rs",
        "Stammdaten" => "stammdaten.rs",
        "StatusRequest_MarketDocument" => "status_request.rs",
        "Unavailability_MarketDocument" => "unavailability.rs",
        other => panic!("no module mapped for XSD {other}"),
    }
}

/// The source that may declare `doc`'s elements: its own module plus the shared
/// `types/` modules, which carry the reusable `Period`/`Interval`/`AttrV`
/// shapes several documents embed.
fn source_for(doc: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut buf = String::new();
    if let Ok(text) = std::fs::read_to_string(src.join("documents").join(module_for(doc))) {
        buf.push_str(&text);
    }
    if let Ok(entries) = std::fs::read_dir(src.join("types")) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("rs")
                && let Ok(text) = std::fs::read_to_string(entry.path())
            {
                buf.push_str(&text);
            }
        }
    }
    buf
}

#[test]
fn every_xsd_element_is_modelled_or_explained() {
    let Some(dir) = xsd_dir() else {
        eprintln!(
            "SKIP xsd_coverage: regulatories/bdew-mako/ is absent. The XSDs are \
             third-party publications and are not tracked in git; \
             regulatories/README.md carries the download URL for each one."
        );
        return;
    };
    let xsds = newest_xsds(&dir);
    assert!(
        !xsds.is_empty(),
        "regulatories/bdew-mako/ exists but holds no .xsd files"
    );

    let excused: BTreeSet<&str> = NOT_MODELLED.iter().map(|(name, _)| *name).collect();

    let mut missing: Vec<(String, String)> = Vec::new();
    let mut checked = 0usize;
    for (doc, path) in &xsds {
        let Ok(xsd) = std::fs::read_to_string(path) else {
            continue;
        };
        let source = source_for(doc);
        for element in declared_elements(&xsd) {
            checked += 1;
            if excused.contains(element.as_str()) {
                continue;
            }
            // Covered when the wire name appears as a `#[serde(rename)]`
            // target, or — for the elements whose Rust field name already
            // matches — as a bare field declaration.
            let renamed = source.contains(&format!("\"{element}\""));
            let bare_field = source.contains(&format!("pub {element}:"));
            if !renamed && !bare_field {
                missing.push((doc.clone(), element));
            }
        }
    }

    assert!(
        !missing.is_empty() || checked > 0,
        "no elements were checked — the XSD parser found nothing"
    );

    assert!(
        missing.is_empty(),
        "these XSD elements are declared by BDEW but appear nowhere in the model, so an \
         inbound document carrying one is parsed with the value silently dropped:\n{}\n\n\
         Either model the element, or add it to NOT_MODELLED with the reason.",
        missing
            .iter()
            .map(|(doc, el)| format!("  {doc}: {el}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_not_modelled_entry_carries_a_reason() {
    for (name, reason) in NOT_MODELLED {
        assert!(
            reason.len() > 20,
            "NOT_MODELLED entry {name:?} needs a real reason, not {reason:?}"
        );
    }
    let mut names: Vec<&str> = NOT_MODELLED.iter().map(|(n, _)| *n).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "duplicate entry in NOT_MODELLED");
}
