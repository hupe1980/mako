//! A segment addressed by its MIG place.
//!
//! A sender knows what it wants to say — „this is the Positionsnettobetrag" —
//! and the MIG knows how that place is told apart from every other place of
//! its tag (`MOA+203` against `MOA+77`, `MOA+9`, `MOA+125`, …).
//! [`Profile::seed_segment`] joins the two: the caller names the place by its
//! `Nr` and supplies values, and the place's identifying code comes from the
//! profile. A builder written this way cannot disagree with the MIG on which
//! qualifier a place carries, because it never spells one.

use edifact_rs::{Element as WireElement, OwnedSegment, Segment};

use super::Profile;
use super::structure::Kind;

/// Why [`Profile::seed_segment`] could not make a segment for a place.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SeedError {
    /// The MIG numbers no place `nr`.
    #[error("the MIG numbers no place {nr}")]
    UnknownPlace {
        /// The `Nr` asked for.
        nr: String,
    },
    /// The place's Segmentlayout has no such data element.
    #[error("place {nr} ({tag}) has no data element {de} at occurrence {occurrence}")]
    UnknownElement {
        /// The place.
        nr: String,
        /// Its tag.
        tag: String,
        /// The data element asked for.
        de: String,
        /// Which occurrence of it, counted from `0`.
        occurrence: u8,
    },
    /// The place admits a code list at this data element and the value is not
    /// on it.
    #[error("place {nr} ({tag}) admits {admitted:?} at DE {de}, not {value:?}")]
    NotAdmitted {
        /// The place.
        nr: String,
        /// Its tag.
        tag: String,
        /// The data element.
        de: String,
        /// The value supplied.
        value: String,
        /// What the MIG admits there.
        admitted: Vec<String>,
    },
    /// The place is told apart from its siblings at a data element that
    /// admits more than one code, and the caller named none of them.
    #[error("place {nr} ({tag}) is told apart at DE {de}, which admits {admitted:?} — state which")]
    Undecided {
        /// The place.
        nr: String,
        /// Its tag.
        tag: String,
        /// The identifying data element.
        de: String,
        /// The codes it admits.
        admitted: Vec<String>,
    },
}

impl Profile {
    /// The segment of MIG place `nr`, carrying `values` and the place's
    /// identifying code(s).
    ///
    /// `values` are `(data element, value)` pairs; a data element named twice
    /// fills its next occurrence in the Segmentlayout (`NAD` DE 3036 repeats
    /// five times). Where the structure tells this place apart from the other
    /// places of its tag by a data element that admits **one** code here —
    /// `MOA` DE 5025 `203` at `00041` — that code is written without being
    /// asked for. Where it admits several (`BGM` DE 1001), the caller states
    /// which, and a value the MIG does not list at a coded element is refused
    /// rather than written somewhere it would place the segment elsewhere.
    ///
    /// Everything else the Prüfschablone requires is
    /// [`Profile::complete`]'s to add.
    ///
    /// # Errors
    ///
    /// [`SeedError`] for an unknown place or data element, a value off the
    /// MIG's code list, and an identifying element whose code the caller left
    /// open.
    pub fn seed_segment(
        &self,
        nr: &str,
        values: &[(&str, &str)],
    ) -> Result<OwnedSegment, SeedError> {
        let s = &self.structure;
        let Some(&node) = s.by_nr.get(nr) else {
            return Err(SeedError::UnknownPlace { nr: nr.to_owned() });
        };
        let (Some(layout), Kind::Segment { discriminators, .. }) =
            (s.layout(node), &s.nodes[node].kind)
        else {
            return Err(SeedError::UnknownPlace { nr: nr.to_owned() });
        };
        let mut grid: Vec<Vec<String>> = layout
            .elements
            .iter()
            .map(|el| vec![String::new(); el.components.len().max(1)])
            .collect();
        let element_id = |ei: usize, ci: usize| -> String {
            let el = &layout.elements[ei];
            el.components
                .get(ci)
                .map_or_else(|| el.id.clone(), |c| c.id.clone())
        };

        let mut seen: Vec<(&str, u8)> = Vec::new();
        for &(de, value) in values {
            let occurrence = if let Some((_, n)) = seen.iter_mut().find(|(d, _)| *d == de) {
                *n += 1;
                *n
            } else {
                seen.push((de, 0));
                0
            };
            if value.is_empty() {
                continue;
            }
            let Some((ei, ci, el)) = layout.locate(de, occurrence) else {
                return Err(SeedError::UnknownElement {
                    nr: nr.to_owned(),
                    tag: layout.tag.clone(),
                    de: de.to_owned(),
                    occurrence,
                });
            };
            if el.is_code_list() && !el.codes.iter().any(|c| c.code == value) {
                return Err(SeedError::NotAdmitted {
                    nr: nr.to_owned(),
                    tag: layout.tag.clone(),
                    de: de.to_owned(),
                    value: value.to_owned(),
                    admitted: el.codes.iter().map(|c| c.code.clone()).collect(),
                });
            }
            value.clone_into(&mut grid[ei][ci]);
        }

        for d in discriminators
            .iter()
            .filter(|d| !d.not_used && !d.codes.is_empty())
        {
            let slot = &mut grid[d.element][d.component];
            if !slot.is_empty() {
                continue;
            }
            match d.codes.as_slice() {
                [only] => slot.clone_from(only),
                admitted => {
                    return Err(SeedError::Undecided {
                        nr: nr.to_owned(),
                        tag: layout.tag.clone(),
                        de: element_id(d.element, d.component),
                        admitted: admitted.to_vec(),
                    });
                }
            }
        }

        for comps in &mut grid {
            while comps.last().is_some_and(String::is_empty) {
                comps.pop();
            }
        }
        while grid.last().is_some_and(Vec::is_empty) {
            grid.pop();
        }
        let elements: Vec<WireElement<'static>> = grid
            .iter()
            .map(|comps| {
                if comps.is_empty() {
                    WireElement::of::<&str>(&[]).into_owned()
                } else {
                    WireElement::of(comps).into_owned()
                }
            })
            .collect();
        Ok(Segment::new(layout.tag.clone(), elements))
    }
}

#[cfg(all(test, feature = "invoic"))]
mod tests {
    use super::*;
    use crate::{MessageType, ReleaseRegistry};

    fn invoic() -> &'static Profile {
        ReleaseRegistry::global()
            .profiles_for(MessageType::Invoic)
            .find(|p| p.release().as_str() == "2.8e" && p.ahb_version() == "1.0b")
            .expect("INVOIC MIG 2.8e / AHB 1.0b is compiled in")
    }

    /// A seeded `MOA` 00041 carries the Positionsnettobetrag qualifier the
    /// profile gives that place, and fits that place and no other `MOA`
    /// place of the structure.
    #[test]
    fn a_seeded_moa_00041_fits_place_00041_and_no_other() {
        let p = invoic();
        let seg = p
            .seed_segment("00041", &[("5004", "80.00")])
            .expect("00041 seeds");
        assert_eq!(seg.component_str(0, 0), Some("203"));
        assert_eq!(seg.component_str(0, 1), Some("80.00"));
        let s = &p.structure;
        let fits: Vec<&str> = s
            .by_nr
            .iter()
            .filter(|&(_, &node)| s.tag(node) == Some("MOA") && s.matches(node, &seg))
            .map(|(nr, _)| nr.as_str())
            .collect();
        assert_eq!(fits, vec!["00041"]);
    }

    /// Every `MOA` place seeds to a segment that fits no place with another
    /// qualifier — the property does not hold by luck for one place. Two
    /// places the MIG gives the same qualifier (`MOA+Z01` at 00048 in `SG42`
    /// and at 00056 in `SG50`) are told apart by their group, not their code.
    #[test]
    fn every_moa_place_seeds_to_itself() {
        let p = invoic();
        let s = &p.structure;
        let moa: Vec<&String> = s
            .by_nr
            .iter()
            .filter(|&(_, &node)| s.tag(node) == Some("MOA"))
            .map(|(nr, _)| nr)
            .collect();
        assert!(moa.len() > 5, "INVOIC has many MOA places: {moa:?}");
        let qualifier = |nr: &str| {
            p.mig
                .segment(nr)
                .and_then(|l| l.locate("5025", 0))
                .map(|(_, _, e)| e.codes.iter().map(|c| c.code.clone()).collect::<Vec<_>>())
        };
        for nr in moa {
            let seg = p.seed_segment(nr, &[("5004", "1")]).expect("seeds");
            for (other, &node) in &s.by_nr {
                if s.tag(node) == Some("MOA") && other != nr && s.matches(node, &seg) {
                    assert_eq!(
                        qualifier(other),
                        qualifier(nr),
                        "{nr} also fits {other}, which the MIG qualifies differently"
                    );
                }
            }
        }
    }

    /// `BGM` DE 1001 admits four codes: the profile cannot choose for the
    /// caller, and a code off the list is refused at the seed.
    #[test]
    fn an_undecided_or_unlisted_code_is_refused() {
        let p = invoic();
        assert!(matches!(
            p.seed_segment("00004", &[("1004", "R-1")]),
            Err(SeedError::Undecided { de, .. }) if de == "1001"
        ));
        assert!(matches!(
            p.seed_segment("00004", &[("1001", "999"), ("1004", "R-1")]),
            Err(SeedError::NotAdmitted { .. })
        ));
        assert!(matches!(
            p.seed_segment("99999", &[]),
            Err(SeedError::UnknownPlace { .. })
        ));
    }

    /// A data element named twice fills its next occurrence.
    #[test]
    fn a_repeated_data_element_fills_its_next_occurrence() {
        let p = invoic();
        let seg = p
            .seed_segment(
                "00022",
                &[("3039", "9900357000004"), ("3036", "A"), ("3036", "B")],
            )
            .expect("seeds");
        assert_eq!(seg.component_str(0, 0), Some("MS"));
        assert_eq!(seg.component_str(3, 0), Some("A"));
        assert_eq!(seg.component_str(3, 1), Some("B"));
    }
}
