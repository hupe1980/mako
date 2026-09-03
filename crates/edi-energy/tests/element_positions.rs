//! A data element sits at the same position in every message that carries it.
//!
//! Element position is fixed by the UN/EDIFACT directory — it is what a
//! counterparty writes on the wire. The BDEW MIGs list every element of a
//! segment (unused ones as `N`), so the imported layouts carry the directory
//! positions, and the hand-authored layouts in `messages::layouts` — the ones
//! the typed message readers address elements by — must agree with them.
//!
//! It had already gone wrong once: REQOTE's MIG was read as listing only
//! `4451` and `C108` for `FTX`, `C108` landed at position 2 instead of 4, and
//! mako rejected a correct `FTX+ACB+++Zusaetzliche Informationen`.

// The hand-authored layouts exist only when a message type is enabled.
#![cfg(any_message)]

use edi_energy::ReleaseRegistry;
use edi_energy::messages::layouts;
use edifact_rs::SegmentDefinition;

/// Every hand-authored layout, by tag.
fn hand_authored() -> Vec<(&'static str, &'static SegmentDefinition)> {
    vec![
        ("BGM", &layouts::BGM),
        ("DTM", &layouts::DTM),
        ("NAD", &layouts::NAD),
        ("RFF", &layouts::RFF),
        ("IDE", &layouts::IDE),
        ("LOC", &layouts::LOC),
        ("AJT", &layouts::AJT),
        ("ERC", &layouts::ERC),
        ("FTX", &layouts::FTX),
        ("QTY", &layouts::QTY),
        ("LIN", &layouts::LIN),
        ("PIA", &layouts::PIA),
        ("CCI", &layouts::CCI),
        ("CAV", &layouts::CAV),
        ("SEQ", &layouts::SEQ),
        ("STS", &layouts::STS),
        ("CTA", &layouts::CTA),
        ("COM", &layouts::COM),
    ]
}

#[test]
fn the_hand_authored_layouts_agree_with_every_mig() {
    let mut conflicts = Vec::new();
    let mut checked = 0usize;
    for profile in ReleaseRegistry::global().all_profiles() {
        for (tag, def) in hand_authored() {
            for layout in profile.structure.layouts.iter().filter(|l| l.tag == tag) {
                let mut seen_elements: Vec<&str> = Vec::new();
                for (ei, el) in layout.elements.iter().enumerate() {
                    // A composite repeated within one segment (`STS` C556 four
                    // times, `PIA` C212 three times) is addressed at its
                    // first occurrence.
                    if seen_elements.contains(&el.id.as_str()) {
                        continue;
                    }
                    seen_elements.push(&el.id);
                    if def.code_positions(&el.id) == 1 {
                        checked += 1;
                        let slot = def.element_slot(&el.id);
                        if slot != ei {
                            conflicts.push(format!(
                                "  {} {} {tag} (Nr {}): {} is element {ei} in the MIG, {slot} in messages::layouts",
                                profile.message_type(),
                                profile.release(),
                                layout.nr,
                                el.id
                            ));
                        }
                    }
                    let mut seen: Vec<&str> = Vec::new();
                    for (ci, comp) in el.components.iter().enumerate() {
                        // A component repeated inside one composite (`C080`
                        // carries 3036 five times) is addressed at its first
                        // occurrence.
                        if seen.contains(&comp.id.as_str()) {
                            continue;
                        }
                        seen.push(&comp.id);
                        if def.code_positions(&comp.id) == 1 {
                            checked += 1;
                            let slot = def.component_slot(&comp.id);
                            if slot != ci {
                                conflicts.push(format!(
                                    "  {} {} {tag} (Nr {}): {} is component {ci} of {} in the MIG, {slot} in messages::layouts",
                                    profile.message_type(),
                                    profile.release(),
                                    layout.nr,
                                    comp.id,
                                    el.id
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        checked > 500,
        "expected the profiles to exercise the layouts, checked {checked}"
    );
    assert!(
        conflicts.is_empty(),
        "a data element sits at one directory position; messages::layouts disagrees with the MIG:\n{}",
        conflicts.join("\n")
    );
}
