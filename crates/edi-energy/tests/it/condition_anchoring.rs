//! Every segment pattern a bundled AHB's Voraussetzungen name is read in the
//! data element the MIG lists its codes under.
//!
//! A Bedingung writes a segment's codes positionally and drops elements it
//! leaves empty: ORDERS AHB 1.1b writes „Wenn IMD+Z01 vorhanden" where the MIG
//! 1.4c (S. 20) puts the Abonnement in DE 7081 (`IMD++Z01`). Read by position,
//! the pattern asks DE 7077 — „Nicht benutzt" — for a code it never carries,
//! and the place the Bedingung gates is never owed: the gate refused every
//! 17202/17203 that sent its `DTM+203`.

use edi_energy::ReleaseRegistry;
use edi_energy::profile::conditions::{SegmentPattern, Voraussetzung, is_precondition};
use edi_energy::profile::model::SegmentNode;

/// Where `pattern` puts a code no layout of its tag admits at that place.
fn misplaced(pattern: &SegmentPattern, layouts: &[SegmentNode]) -> Vec<String> {
    let tagged: Vec<&SegmentNode> = layouts.iter().filter(|l| l.tag == pattern.tag).collect();
    if tagged.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (ei, comps) in pattern.elements.iter().enumerate() {
        for (ci, alternatives) in comps.iter().enumerate() {
            let Some(alts) = alternatives else { continue };
            // A code the MIG lists nowhere in this segment (an EBD's
            // Antwortcode, a value the MIG leaves free) has no place to be
            // misread from; one listed at another place is misread here.
            let listed_elsewhere = tagged.iter().any(|l| {
                l.leaves().any(|(e, c, el)| {
                    (e, c) != (ei, ci) && alts.iter().any(|a| el.codes.iter().any(|k| &k.code == a))
                })
            });
            if !listed_elsewhere {
                continue;
            }
            let admitted = tagged.iter().any(|l| {
                l.leaves().any(|(e, c, el)| {
                    e == ei
                        && c == ci
                        && el.status != "N"
                        && (el.codes.is_empty()
                            || alts.iter().any(|a| el.codes.iter().any(|k| &k.code == a)))
                })
            });
            if !admitted {
                out.push(format!("{}@{ei}:{ci}={}", pattern.tag, alts.join("/")));
            }
        }
    }
    out
}

fn patterns(v: &Voraussetzung) -> Vec<&SegmentPattern> {
    match v {
        Voraussetzung::Present { patterns, .. } => patterns.iter().collect(),
        Voraussetzung::ElementShape { pattern, .. } | Voraussetzung::Count { pattern, .. } => {
            vec![pattern]
        }
        _ => Vec::new(),
    }
}

/// Anchored in the MIG, no Voraussetzung of any bundled profile asks an
/// element for a code the MIG lists nowhere there — and the ORDERS
/// Abonnement is among those it moved.
#[test]
fn every_bundled_condition_reads_its_codes_where_the_mig_lists_them() {
    let mut findings = Vec::new();
    let mut moved_the_abonnement = false;
    for profile in ReleaseRegistry::global().all_profiles() {
        let layouts = &profile.structure.layouts;
        for (id, text) in &profile.ahb.conditions {
            if !is_precondition(text) {
                continue;
            }
            let Some(v) = Voraussetzung::parse(text) else {
                continue;
            };
            for p in patterns(&v) {
                let anchored = p.anchored(layouts);
                if &anchored != p && p.tag == "IMD" && text.contains("IMD+Z01") {
                    moved_the_abonnement = true;
                }
                for m in misplaced(&anchored, layouts) {
                    findings.push(format!(
                        "{:?} {} [{id}] {text:?}: {m}",
                        profile.message_type(),
                        profile.release().as_str()
                    ));
                }
            }
        }
    }
    assert!(
        moved_the_abonnement,
        "the ORDERS IMD+Z01 was not moved to DE 7081"
    );
    assert!(
        findings.is_empty(),
        "{} misread:\n{}",
        findings.len(),
        findings.join("\n")
    );
}
