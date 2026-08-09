//! Guards the hand-authored positional element/component indices in
//! `messages::segments` against the BDEW MIG segment layouts.
//!
//! Every defect this file exists to prevent had the same shape: an index was
//! authored by hand, a fixture was written to match it, and the pair agreed
//! with each other while disagreeing with the MIG. `FTX` free text was read one
//! element past `C108`; `UCS` grew a component the standard does not define;
//! `STS` read the composite the MIG marks *nicht benutzt*.
//!
//! A test cannot read the MIG PDFs, so it does the next best thing: it pins the
//! layouts those PDFs state, in one table, and checks that a fixture drawn from
//! the corpus is consistent with them. Changing an index now means changing
//! this table too — which is the point at which someone has to go and read the
//! MIG.
//!
//! Gated on `utilmd` because `messages::layouts` ships with the message types
//! that use it; with no message type enabled there is no layout to check.
#![cfg(feature = "utilmd")]

/// One BDEW segment layout: element index → the composite's components in
/// order, as the MIG lists them.
///
/// Sources: `UTILMD MIG Strom S2.2`, `MSCONS MIG 2.5`, `APERAK MIG 2.2`,
/// `ORDERS MIG 1.4c`, `INVOIC MIG 2.8e` (BDEW EDI@Energy).
/// Components of one data element, in the order the MIG lists them.
type ElementLayout = (usize, &'static [&'static str]);
/// One segment's layout: its tag and the elements the MIG defines.
type SegmentLayout = (&'static str, &'static [ElementLayout]);

const MIG_LAYOUTS: &[SegmentLayout] = &[
    ("BGM", &[(0, &["1001"]), (1, &["1004"]), (2, &["1225"])]),
    ("DTM", &[(0, &["2005", "2380", "2379"])]),
    (
        "NAD",
        &[
            (0, &["3035"]),
            (1, &["3039", "1131", "3055"]),
            (3, &["3036"]),
        ],
    ),
    ("RFF", &[(0, &["1153", "1154"])]),
    ("IDE", &[(0, &["7495"]), (1, &["7402"])]),
    ("LOC", &[(0, &["3227"]), (1, &["3225", "1131", "3055"])]),
    // C901 carries only 9321 — no code list, no agency.
    ("ERC", &[(0, &["9321"])]),
    // Element 1 is DE 4453 (Text function, coded) and element 2 is C107; the
    // free text is C108 at element 3. `FTX+AAO+++Text'`.
    (
        "FTX",
        &[
            (0, &["4451"]),
            (1, &["4453"]),
            (2, &["4441"]),
            (3, &["4440"]),
        ],
    ),
    ("QTY", &[(0, &["6063", "6060", "6411"])]),
    ("LIN", &[(0, &["1082"]), (1, &["1229"])]),
    ("PIA", &[(0, &["4347"]), (1, &["7140", "7143"])]),
    // C502 is *nicht benutzt* but keeps element 1: `CCI+15++BI1'`.
    ("CCI", &[(0, &["7059"]), (1, &["6313"]), (2, &["7037"])]),
    // Polymorphic in DE 9015: C555 carries the value for Z18/10, C556 for 7/Z33.
    ("STS", &[(0, &["9015"]), (1, &["4405"]), (2, &["9013"])]),
    ("CTA", &[(0, &["3139"]), (1, &["3413", "3412"])]),
    ("COM", &[(0, &["3148", "3155"])]),
];

/// Every data element the accessors in `messages::segments` address, as
/// `(segment, code)`.
///
/// The derive resolves each of these against `messages::layouts` in a `const`
/// context, so a code that does not exist there is already a build error. What
/// this list adds is the other direction: that the *layout* agrees with the
/// MIG table above, which is the one thing still authored by hand.
const ADDRESSED_CODES: &[(&str, &str)] = &[
    ("BGM", "1001"),
    ("BGM", "1004"),
    ("BGM", "1225"),
    ("DTM", "2005"),
    ("DTM", "2380"),
    ("DTM", "2379"),
    ("NAD", "3035"),
    ("NAD", "3039"),
    ("NAD", "3055"),
    ("NAD", "3036"),
    ("RFF", "1153"),
    ("RFF", "1154"),
    ("IDE", "7495"),
    ("IDE", "7402"),
    ("LOC", "3227"),
    ("LOC", "3225"),
    ("ERC", "9321"),
    ("FTX", "4451"),
    ("FTX", "4440"),
    ("QTY", "6063"),
    ("QTY", "6060"),
    ("QTY", "6411"),
    ("LIN", "1082"),
    ("PIA", "4347"),
    ("PIA", "7140"),
    ("PIA", "7143"),
    ("CCI", "7059"),
    ("CCI", "7037"),
    ("STS", "9015"),
    ("STS", "4405"),
    ("STS", "9013"),
    ("CTA", "3139"),
    ("CTA", "3413"),
    ("COM", "3148"),
    ("COM", "3155"),
];

/// The shipped layout must place every addressed code where the MIG does.
///
/// `messages::layouts` is hand-authored from the BDEW MIG PDFs, and the derive
/// trusts it completely: `element = "9013"` resolves to whatever slot the
/// layout says, silently. This checks that slot against [`MIG_LAYOUTS`], so a
/// layout edit that moves a field has to move the MIG table too — which is
/// where someone has to go and re-read the PDF.
#[test]
fn the_shipped_layout_agrees_with_the_mig() {
    use edi_energy::messages::layouts;

    fn definition(tag: &str) -> Option<&'static edifact_rs::SegmentDefinition> {
        Some(match tag {
            "BGM" => &layouts::BGM,
            "DTM" => &layouts::DTM,
            "NAD" => &layouts::NAD,
            "RFF" => &layouts::RFF,
            "IDE" => &layouts::IDE,
            "LOC" => &layouts::LOC,
            "ERC" => &layouts::ERC,
            "FTX" => &layouts::FTX,
            "QTY" => &layouts::QTY,
            "LIN" => &layouts::LIN,
            "PIA" => &layouts::PIA,
            "CCI" => &layouts::CCI,
            "STS" => &layouts::STS,
            "CTA" => &layouts::CTA,
            "COM" => &layouts::COM,
            _ => return None,
        })
    }

    let mut offenders = Vec::new();
    for (seg, code) in ADDRESSED_CODES {
        let Some(def) = definition(seg) else {
            offenders.push(format!("{seg}: no layout exposed for {seg}"));
            continue;
        };
        if def.code_positions(code) != 1 {
            offenders.push(format!(
                "{seg}/{code}: resolves to {} positions in the shipped layout, expected exactly 1",
                def.code_positions(code)
            ));
            continue;
        }
        let (element, component) = (def.element_slot(code), def.component_slot(code));

        let Some((_, mig)) = MIG_LAYOUTS.iter().find(|(t, _)| t == seg) else {
            offenders.push(format!("{seg}: no MIG layout recorded"));
            continue;
        };
        let found = mig
            .iter()
            .find(|(idx, comps)| *idx == element && comps.get(component) == Some(code));
        if found.is_none() {
            let mig_slot = mig
                .iter()
                .find_map(|(idx, comps)| comps.iter().position(|c| c == code).map(|c| (*idx, c)));
            offenders.push(match mig_slot {
                Some((mi, mc)) => format!(
                    "{seg}/{code}: layout puts it at element {element} component {component},                      the MIG at element {mi} component {mc}"
                ),
                None => format!("{seg}/{code}: not present in the MIG layout at all"),
            });
        }
    }

    assert!(
        offenders.is_empty(),
        "the shipped segment layouts disagree with the BDEW MIG:\n  {}",
        offenders.join("\n  ")
    );
}

/// The MIG table itself must be well-formed: no duplicate segment, and no
/// element index recorded twice for one segment.
#[test]
fn the_mig_layout_table_is_consistent() {
    let mut seen = std::collections::HashSet::new();
    for (seg, layout) in MIG_LAYOUTS {
        assert!(seen.insert(*seg), "{seg} appears twice in MIG_LAYOUTS");
        let mut elems = std::collections::HashSet::new();
        for (idx, comps) in *layout {
            assert!(
                elems.insert(*idx),
                "{seg} element {idx} appears twice in MIG_LAYOUTS"
            );
            assert!(
                !comps.is_empty(),
                "{seg} element {idx} declares no components"
            );
        }
    }
}
