//! The one place a Prüfidentifikator is read out of a segment list.
//!
//! The PID lives in `BGM` DE 1004 for most message types and in `SG1 RFF+Z13`
//! for the rest, and which one a profile declares is only a hint: reading just
//! the declared location makes a conformant partner's message undetectable, and
//! an undetectable message is dropped without an APERAK.
//!
//! So both are read, the declared one first — and both demand a *plausible*
//! code, because `BGM` DE 1004 legitimately holds a Dokumentennummer, and a
//! numeric one would otherwise beat the real PID in `RFF+Z13`.
//!
//! Every path that reads a PID goes through here — the full parse, the
//! envelope-only routing path, and typed deserialization — so a routing decision
//! can never resolve a different code from the parse of the same bytes.

use edifact_rs::Segment;

use crate::{Pruefidentifikator, registry::PidSource};

/// A five-digit code in `10000..=99999`, or nothing.
fn plausible(raw: &str) -> Option<u32> {
    raw.parse::<u32>()
        .ok()
        .filter(|v| (Pruefidentifikator::MIN..=Pruefidentifikator::MAX).contains(v))
}

/// Segment access shared by the borrowed and owned segment types.
pub(crate) trait PidSegment {
    fn tag_str(&self) -> &str;
    fn element(&self, index: usize) -> Option<&str>;
    fn component(&self, element: usize, component: usize) -> Option<&str>;
}

// `OwnedSegment` is an alias for `Segment<'static>`, so this one impl serves
// both the borrowed slice path and the owned stream path.
impl PidSegment for Segment<'_> {
    fn tag_str(&self) -> &str {
        &self.tag
    }
    fn element(&self, index: usize) -> Option<&str> {
        self.element_str(index)
    }
    fn component(&self, element: usize, component: usize) -> Option<&str> {
        self.component_str(element, component)
    }
}

/// `BGM` element 1 (DE 1004).
fn from_bgm<S: PidSegment>(segments: &[S]) -> Option<u32> {
    segments
        .iter()
        .find(|s| s.tag_str() == "BGM")
        .and_then(|bgm| bgm.element(1))
        .and_then(plausible)
}

/// The first top-level `RFF+Z13`, C506 component 1 (DE 1154).
fn from_rff_z13<S: PidSegment>(segments: &[S]) -> Option<u32> {
    segments
        .iter()
        .filter(|s| s.tag_str() == "RFF" && s.element(0) == Some("Z13"))
        .find_map(|rff| rff.component(0, 1).and_then(plausible))
}

/// Read the Prüfidentifikator, trying `source` first and the other location second.
pub(crate) fn detect<S: PidSegment>(segments: &[S], source: PidSource) -> Option<u32> {
    match source {
        PidSource::RffZ13 => from_rff_z13(segments).or_else(|| from_bgm(segments)),
        PidSource::BgmDe1004 => from_bgm(segments).or_else(|| from_rff_z13(segments)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edifact_rs::{Element, OwnedSegment};

    fn seg(tag: &str, elements: &[&[&str]]) -> OwnedSegment {
        Segment::new(
            tag,
            elements.iter().map(|comps| Element::of(comps)).collect(),
        )
        .into_owned()
    }

    /// A Dokumentennummer that happens to be numeric must not outrank the real
    /// PID in `RFF+Z13`.
    #[test]
    fn a_numeric_document_number_does_not_beat_the_real_pid() {
        let segments = vec![
            seg("BGM", &[&["E01"], &["7"]]),
            seg("RFF", &[&["Z13", "13002"]]),
        ];
        assert_eq!(detect(&segments, PidSource::RffZ13), Some(13002));
        // Even when the profile declares BGM, the implausible 7 is skipped.
        assert_eq!(detect(&segments, PidSource::BgmDe1004), Some(13002));
    }

    /// Both locations are read regardless of which the profile declares.
    #[test]
    fn either_location_is_found_from_either_declared_source() {
        let in_bgm = vec![seg("BGM", &[&["E01"], &["55001"]])];
        let in_rff = vec![seg("RFF", &[&["Z13", "55001"]])];
        for source in [PidSource::BgmDe1004, PidSource::RffZ13] {
            assert_eq!(detect(&in_bgm, source), Some(55001));
            assert_eq!(detect(&in_rff, source), Some(55001));
        }
    }

    /// The declared source wins when both carry a plausible code.
    #[test]
    fn the_declared_source_wins_a_genuine_conflict() {
        let segments = vec![
            seg("BGM", &[&["E01"], &["11001"]]),
            seg("RFF", &[&["Z13", "13002"]]),
        ];
        assert_eq!(detect(&segments, PidSource::BgmDe1004), Some(11001));
        assert_eq!(detect(&segments, PidSource::RffZ13), Some(13002));
    }

    /// A non-`Z13` reference is not a Prüfidentifikator.
    #[test]
    fn other_rff_qualifiers_are_ignored() {
        let segments = vec![seg("RFF", &[&["ACW", "55001"]])];
        assert_eq!(detect(&segments, PidSource::RffZ13), None);
    }

    #[test]
    fn out_of_range_and_non_numeric_values_are_rejected() {
        for raw in ["9999", "100000", "", "ABCDE", "-55001"] {
            let segments = vec![seg("BGM", &[&["E01"], &[raw]])];
            assert_eq!(detect(&segments, PidSource::BgmDe1004), None, "{raw:?}");
        }
    }
}
