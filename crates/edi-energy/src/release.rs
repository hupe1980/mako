use std::{cmp::Ordering, fmt, str::FromStr};

use crate::Error;

/// Coarse track classification for a BDEW EDI@Energy release code.
///
/// Used for unambiguous multi-track dispatch in `ReleaseRegistry` without
/// relying on fragile string-prefix matching.
///
/// # Example
///
/// ```
/// use edi_energy::{Release, ReleaseTrack};
///
/// let s21: Release = "S2.1".parse().unwrap();
/// assert_eq!(s21.track(), ReleaseTrack::Strom);
///
/// let g11: Release = "G1.1".parse().unwrap();
/// assert_eq!(g11.track(), ReleaseTrack::Gas);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseTrack {
    /// UTILMD Strom track — release codes beginning with `S` (e.g. `S2.1`).
    Strom,
    /// UTILMD Gas track — release codes beginning with `G` (e.g. `G1.1`).
    Gas,
    /// Classic UTILMD / pre-2024 releases using `<major>.<minor>.<patch><letter>`
    /// (e.g. `5.5.3a`, `5.5.8a`).
    Classic,
    /// Short `<major>.<minor><letter>` releases (e.g. `2.4c`, `2.5a`).
    /// Used by MSCONS, APERAK, INVOIC, REMADV, and most non-UTILMD types.
    Short,
    /// Any release code that does not parse into a known BDEW pattern.
    ///
    /// Forward-compatible: new BDEW track conventions will initially appear
    /// here until this enum is extended.  Emit a `tracing::warn!` (when the
    /// `tracing` feature is enabled) when constructing an `Opaque` release so
    /// unknown patterns surface in production logs.
    Other,
}

impl fmt::Display for ReleaseTrack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReleaseTrack::Strom => f.write_str("Strom"),
            ReleaseTrack::Gas => f.write_str("Gas"),
            ReleaseTrack::Classic => f.write_str("Classic"),
            ReleaseTrack::Short => f.write_str("Short"),
            ReleaseTrack::Other => f.write_str("Other"),
        }
    }
}

/// Parsed structure of an EDI@Energy release code.
///
/// BDEW uses several different numbering conventions, which cannot be compared
/// correctly with plain lexicographic ordering:
///
/// | Pattern | Examples | Notes |
/// |---------|----------|-------|
/// | `<major>.<minor>.<patch><letter>` | `5.5.3a`, `5.5.4b` | UTILMD (classic) |
/// | `<major>.<minor><letter>` | `2.5a`, `2.4c` | MSCONS, APERAK, … |
/// | `S<major>.<minor>` | `S2.1`, `S2.2` | UTILMD Strom (new track) |
/// | `G<major>.<minor>` | `G1.1`, `G2.0` | UTILMD Gas |
///
/// The `ReleaseKind` variant carries the ordering rules for its track.
/// Releases from different tracks (e.g. `S2.1` vs `5.5.3a`) have no meaningful
/// order; [`Release::same_track_cmp`] reports that by returning `None`, while
/// [`Ord`] stays total so sorted containers behave.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReleaseKind {
    /// Classic three-component form: `<major>.<minor>.<patch><letter>`.
    ///
    /// Examples: `5.5.3a`, `5.5.4b`.
    Dotted {
        /// Major version component.
        major: u32,
        /// Minor version component.
        minor: u32,
        /// Patch version component.
        patch: u32,
        /// Trailing letter qualifier (`a`, `b`, …).
        letter: char,
    },
    /// Short two-component form: `<major>.<minor><letter>`.
    ///
    /// Examples: `2.5a`, `2.4c`, `1.0a`.
    Short {
        /// Major version component.
        major: u32,
        /// Minor version component.
        minor: u32,
        /// Trailing letter qualifier (`a`, `b`, …).
        letter: char,
    },
    /// Strom track: `S<major>.<minor>[letter]`.
    ///
    /// Examples: `S2.1`, `S2.2`, `S1.1a`.
    Strom {
        /// Major version component (after `S`).
        major: u32,
        /// Minor version component.
        minor: u32,
        /// Trailing corrigendum letter, `None` when the code carries none.
        letter: Option<char>,
    },
    /// Gas track: `G<major>.<minor>[letter]`.
    ///
    /// Examples: `G1.1`, `G2.0`, `G1.0a`.
    Gas {
        /// Major version component (after `G`).
        major: u32,
        /// Minor version component.
        minor: u32,
        /// Trailing corrigendum letter, `None` when the code carries none.
        letter: Option<char>,
    },
    /// Any release code that does not match any of the above patterns.
    ///
    /// Retained verbatim for forward-compatibility.  When a release is
    /// constructed with an unrecognised code, `tracing::warn!` is emitted
    /// (when the `tracing` feature is enabled) so unknown patterns surface in
    /// production observability backends.
    Opaque(Box<str>),
}

impl ReleaseKind {
    /// Returns the coarse [`ReleaseTrack`] for this release code.
    ///
    /// Use this for explicit, type-safe track dispatch instead of
    /// `release.as_str().starts_with("S")` string-prefix matching.
    #[must_use]
    pub fn track(&self) -> ReleaseTrack {
        match self {
            ReleaseKind::Strom { .. } => ReleaseTrack::Strom,
            ReleaseKind::Gas { .. } => ReleaseTrack::Gas,
            ReleaseKind::Dotted { .. } => ReleaseTrack::Classic,
            ReleaseKind::Short { .. } => ReleaseTrack::Short,
            ReleaseKind::Opaque(_) => ReleaseTrack::Other,
        }
    }

    /// Returns `true` if two `ReleaseKind` values belong to the same *track*
    /// and can therefore be meaningfully compared with `<` / `>`.
    #[must_use]
    pub fn same_track(&self, other: &ReleaseKind) -> bool {
        matches!(
            (self, other),
            (ReleaseKind::Dotted { .. }, ReleaseKind::Dotted { .. })
                | (ReleaseKind::Short { .. }, ReleaseKind::Short { .. })
                | (ReleaseKind::Strom { .. }, ReleaseKind::Strom { .. })
                | (ReleaseKind::Gas { .. }, ReleaseKind::Gas { .. })
                | (ReleaseKind::Opaque(_), ReleaseKind::Opaque(_))
        )
    }

    /// Parse a release string into a structured `ReleaseKind`.
    ///
    /// Returns `ReleaseKind::Opaque` when the string does not match any known
    /// pattern.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        // S<major>.<minor> or S<major>.<minor><letter>  e.g. "S2.1", "S1.1a"
        if let Some(rest) = s.strip_prefix('S')
            && let Some((major, minor, letter)) = split_two_numeric_with_suffix(rest)
        {
            return ReleaseKind::Strom {
                major,
                minor,
                letter,
            };
        }
        // G<major>.<minor> or G<major>.<minor><letter>  e.g. "G1.1", "G1.0a"
        if let Some(rest) = s.strip_prefix('G')
            && let Some((major, minor, letter)) = split_two_numeric_with_suffix(rest)
        {
            return ReleaseKind::Gas {
                major,
                minor,
                letter,
            };
        }
        // <major>.<minor>.<patch><letter>  e.g. "5.5.3a"
        if let Some(k) = parse_dotted(s) {
            return k;
        }
        // <major>.<minor><letter>  e.g. "2.5a"
        if let Some(k) = parse_short(s) {
            return k;
        }
        ReleaseKind::Opaque(s.into())
    }
}

/// Parse `"<major>.<minor>"` or `"<major>.<minor><letter>"`.
///
/// The corrigendum letter is part of the identity: `"S1.1a"` and `"S1.1"` are
/// different profiles, and dropping it would make them compare `Equal` while
/// remaining `!=` — the `Ord`/`Eq` disagreement every sorted container relies on
/// not happening.
fn split_two_numeric_with_suffix(s: &str) -> Option<(u32, u32, Option<char>)> {
    let (a, b) = s.split_once('.')?;
    let major: u32 = a.parse().ok()?;
    let minor_str = b.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    if minor_str.is_empty() {
        return None;
    }
    let minor: u32 = minor_str.parse().ok()?;
    let letter = b[minor_str.len()..].chars().next();
    // Only a single-letter suffix is a BDEW corrigendum marker.
    if b[minor_str.len()..].chars().count() > 1 {
        return None;
    }
    Some((major, minor, letter))
}

fn parse_dotted(s: &str) -> Option<ReleaseKind> {
    // Expect exactly two '.' in the string: "5.5.3a"
    let first = s.find('.')?;
    let rest = &s[first + 1..];
    let second = rest.find('.')?;
    let major: u32 = s[..first].parse().ok()?;
    let minor: u32 = rest[..second].parse().ok()?;
    let tail = &rest[second + 1..]; // "3a"
    let letter = tail.chars().last()?;
    if !letter.is_ascii_lowercase() {
        return None;
    }
    let patch: u32 = tail[..tail.len() - 1].parse().ok()?;
    Some(ReleaseKind::Dotted {
        major,
        minor,
        patch,
        letter,
    })
}

fn parse_short(s: &str) -> Option<ReleaseKind> {
    // Expect exactly one '.' and a trailing letter: "2.5a"
    let dot = s.find('.')?;
    if s.rfind('.') != Some(dot) {
        return None; // two or more dots → not short form
    }
    let major: u32 = s[..dot].parse().ok()?;
    let tail = &s[dot + 1..]; // "5a"
    let letter = tail.chars().last()?;
    if !letter.is_ascii_lowercase() {
        return None;
    }
    let minor: u32 = tail[..tail.len() - 1].parse().ok()?;
    Some(ReleaseKind::Short {
        major,
        minor,
        letter,
    })
}

// ── Release ───────────────────────────────────────────────────────────────────

/// The EDI@Energy release / association-code identifier extracted from the UNH segment.
///
/// Examples: `"5.5.3a"`, `"S4.0"`, `"5.2e"`.
///
/// The release code occupies DE 0057 (association assigned code) of the S009 composite
/// in the UNH segment (element index 1, component index 4).
///
/// # Ordering
///
/// `PartialOrd` implements *within-track* semantic ordering (numeric component
/// comparison).  `Ord` is defined for total ordering consistency but falls back
/// to byte ordering for cross-track comparisons, which are not meaningful.
/// Use [`Release::kind`] and [`ReleaseKind::same_track`] to guard cross-track
/// comparisons in application code.
///
/// # Construction
///
/// Use [`Release::new`] or the [`FromStr`] impl:
///
/// ```
/// use edi_energy::Release;
/// let r: Release = "5.5.3a".parse().unwrap();
/// assert_eq!(r.as_str(), "5.5.3a");
/// ```
///
/// # Memory layout
///
/// Internally uses `Arc<str>` so `Clone` is an atomic refcount increment with
/// no heap allocation — safe to clone on hot paths without measurable allocator
/// pressure.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Release(std::sync::Arc<str>);

impl Release {
    /// Create a `Release` from any string slice.
    ///
    /// The value is taken verbatim; no format validation is performed because
    /// BDEW occasionally introduces codes that do not follow a single fixed pattern.
    #[must_use]
    pub fn new(code: &str) -> Self {
        Self(code.into())
    }

    /// Create a `Release` from a string slice with basic validation.
    ///
    /// Rejects codes that are:
    /// - empty
    /// - longer than 35 characters (beyond any known BDEW convention)
    /// - non-ASCII
    ///
    /// For internal parsing where the wire value comes directly from an EDIFACT
    /// segment, use the infallible [`Release::new`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRelease`] if the code fails validation.
    ///
    /// # Examples
    ///
    /// ```
    /// use edi_energy::Release;
    ///
    /// assert!(Release::try_new("5.5.3a").is_ok());
    /// assert!(Release::try_new("S2.1").is_ok());
    /// assert!(Release::try_new("").is_err());
    /// assert!(Release::try_new(&"x".repeat(36)).is_err());
    /// ```
    pub fn try_new(code: &str) -> Result<Self, Error> {
        if code.is_empty() {
            return Err(Error::InvalidRelease("release code must not be empty"));
        }
        if code.len() > 35 {
            return Err(Error::InvalidRelease(
                "release code must not exceed 35 characters",
            ));
        }
        if !code.is_ascii() {
            return Err(Error::InvalidRelease(
                "release code must contain only ASCII characters",
            ));
        }
        Ok(Self(code.into()))
    }

    /// Returns the raw association code string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` when `release` is registered for `message_type` in `registry`.
    ///
    /// This is the fast-path check for builder code that wants to fail early before
    /// any serialization attempt.
    ///
    /// # Example
    ///
    /// ```rust
    /// use edi_energy::{Release, MessageType, ReleaseRegistry};
    ///
    /// # #[cfg(feature = "utilmd")]
    /// # {
    /// let registry = ReleaseRegistry::global();
    /// let r = Release::new("S2.2");
    /// assert!(r.is_registered(MessageType::Utilmd, registry));
    ///
    /// let bad = Release::new("X9.9");
    /// assert!(!bad.is_registered(MessageType::Utilmd, registry));
    /// # }
    /// ```
    #[must_use]
    pub fn is_registered(
        &self,
        message_type: crate::MessageType,
        registry: &crate::registry::ReleaseRegistry,
    ) -> bool {
        registry
            .profiles_for(message_type)
            .any(|p| p.release() == self)
    }

    /// Parse the release code into a structured [`ReleaseKind`].
    ///
    /// Returns `ReleaseKind::Opaque` when the string does not match any known
    /// BDEW release pattern.  When the `tracing` feature is enabled, an
    /// `Opaque` result emits a `tracing::warn!` so unknown patterns surface
    /// in production observability backends (resolves.
    #[must_use]
    pub fn kind(&self) -> ReleaseKind {
        let kind = ReleaseKind::parse(&self.0);
        #[cfg(feature = "tracing")]
        if matches!(kind, ReleaseKind::Opaque(_)) {
            tracing::warn!(
                release = %self.0,
                "unrecognised BDEW release code pattern — defaulting to Opaque track; \
                 update ReleaseKind::parse() if this is a new BDEW convention"
            );
        }
        kind
    }

    /// Return the coarse [`ReleaseTrack`] for this release code.
    ///
    /// Prefer this over `release.as_str().starts_with("S")` pattern matching
    /// for track dispatch — it is unambiguous and forward-compatible.
    ///
    /// ```
    /// use edi_energy::{Release, ReleaseTrack};
    ///
    /// assert_eq!("S2.1".parse::<Release>().unwrap().track(), ReleaseTrack::Strom);
    /// assert_eq!("G1.1".parse::<Release>().unwrap().track(), ReleaseTrack::Gas);
    /// assert_eq!("2.4c".parse::<Release>().unwrap().track(), ReleaseTrack::Short);
    /// assert_eq!("5.5.3a".parse::<Release>().unwrap().track(), ReleaseTrack::Classic);
    /// ```
    #[must_use]
    pub fn track(&self) -> ReleaseTrack {
        ReleaseKind::parse(&self.0).track()
    }

    /// Compare two releases within the same track, returning `None` for
    /// cross-track pairs.
    ///
    /// This is the semantically correct comparison for application code: unlike
    /// [`Ord::cmp`], which is total by construction, it says so when the two
    /// codes have no meaningful order relative to each other.
    ///
    /// ```
    /// use edi_energy::Release;
    /// use std::cmp::Ordering;
    ///
    /// let s21: Release = "S2.1".parse().unwrap();
    /// let s22: Release = "S2.2".parse().unwrap();
    /// let r24c: Release = "2.4c".parse().unwrap();
    ///
    /// assert_eq!(s21.same_track_cmp(&s22), Some(Ordering::Less));
    /// assert_eq!(s21.same_track_cmp(&r24c), None);  // cross-track → None
    /// ```
    #[must_use]
    pub fn same_track_cmp(&self, other: &Release) -> Option<Ordering> {
        let a = self.kind();
        let b = other.kind();
        a.same_track(&b).then(|| compare_kinds(&a, &b))
    }
}

impl PartialOrd for Release {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Release {
    /// A **total** order: track rank first, then the numeric components within
    /// a track, then the raw code.
    ///
    /// Total by construction: `partial_cmp(a, b) == Some(cmp(a, b))` must hold,
    /// or `sort`, `BTreeMap` and `binary_search` are free to produce arbitrary
    /// results — and `ReleaseRegistry::releases()` sorts through this impl.
    ///
    /// Cross-track order is deterministic but carries no BDEW meaning. Reach for
    /// [`Release::same_track_cmp`] when the answer must be meaningful; it says
    /// so by returning `None`.
    fn cmp(&self, other: &Self) -> Ordering {
        let a = self.kind();
        let b = other.kind();
        compare_kinds(&a, &b).then_with(|| self.0.cmp(&other.0))
    }
}

fn compare_kinds(a: &ReleaseKind, b: &ReleaseKind) -> Ordering {
    match (a, b) {
        (
            ReleaseKind::Dotted {
                major: ma,
                minor: mi,
                patch: pa,
                letter: la,
            },
            ReleaseKind::Dotted {
                major: mb,
                minor: mib,
                patch: pb,
                letter: lb,
            },
        ) => ma
            .cmp(mb)
            .then(mi.cmp(mib))
            .then(pa.cmp(pb))
            .then(la.cmp(lb)),
        (
            ReleaseKind::Short {
                major: ma,
                minor: mi,
                letter: la,
            },
            ReleaseKind::Short {
                major: mb,
                minor: mib,
                letter: lb,
            },
        ) => ma.cmp(mb).then(mi.cmp(mib)).then(la.cmp(lb)),
        (
            ReleaseKind::Strom {
                major: ma,
                minor: mi,
                letter: la,
            },
            ReleaseKind::Strom {
                major: mb,
                minor: mib,
                letter: lb,
            },
        )
        | (
            ReleaseKind::Gas {
                major: ma,
                minor: mi,
                letter: la,
            },
            ReleaseKind::Gas {
                major: mb,
                minor: mib,
                letter: lb,
            },
        ) => ma.cmp(mb).then(mi.cmp(mib)).then(la.cmp(lb)),
        (ReleaseKind::Opaque(a), ReleaseKind::Opaque(b)) => a.cmp(b),
        // Cross-track: rank by track so the order is total, then by the raw
        // code.  `same_track_cmp` is what callers use when the answer has to be
        // semantically meaningful; this arm only has to be consistent.
        _ => track_rank(a).cmp(&track_rank(b)),
    }
}

/// A stable rank per release kind, so cross-track comparison is deterministic.
fn track_rank(kind: &ReleaseKind) -> u8 {
    match kind {
        ReleaseKind::Dotted { .. } => 0,
        ReleaseKind::Short { .. } => 1,
        ReleaseKind::Strom { .. } => 2,
        ReleaseKind::Gas { .. } => 3,
        ReleaseKind::Opaque(_) => 4,
    }
}

impl fmt::Display for Release {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Release {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Infallible — every string is a valid (if potentially unrecognised) release code.
impl FromStr for Release {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

#[cfg(test)]
mod release_kind_tests {
    use super::*;

    #[test]
    fn parse_dotted_form() {
        let k = ReleaseKind::parse("5.5.3a");
        assert_eq!(
            k,
            ReleaseKind::Dotted {
                major: 5,
                minor: 5,
                patch: 3,
                letter: 'a'
            }
        );
    }

    #[test]
    fn parse_short_form() {
        let k = ReleaseKind::parse("2.5a");
        assert_eq!(
            k,
            ReleaseKind::Short {
                major: 2,
                minor: 5,
                letter: 'a'
            }
        );
    }

    #[test]
    fn parse_strom_track() {
        let k = ReleaseKind::parse("S2.1");
        assert_eq!(
            k,
            ReleaseKind::Strom {
                major: 2,
                minor: 1,
                letter: None
            }
        );
    }

    #[test]
    fn parse_gas_track() {
        let k = ReleaseKind::parse("G1.1");
        assert_eq!(
            k,
            ReleaseKind::Gas {
                major: 1,
                minor: 1,
                letter: None
            }
        );
    }

    #[test]
    fn parse_opaque_fallback() {
        match ReleaseKind::parse("UNKNOWN") {
            ReleaseKind::Opaque(_) => {}
            other => panic!("expected Opaque, got {other:?}"),
        }
    }

    #[test]
    fn semantic_ordering_dotted() {
        let a = Release::new("5.5.3a");
        let b = Release::new("5.5.10a"); // numeric: 10 > 3
        assert!(a < b, "5.5.3a should be less than 5.5.10a");
    }

    #[test]
    fn semantic_ordering_short() {
        let a = Release::new("2.9a");
        let b = Release::new("2.10a"); // numeric: 10 > 9
        assert!(a < b, "2.9a should be less than 2.10a (numeric, not lex)");
    }

    #[test]
    fn semantic_ordering_strom() {
        let a = Release::new("S2.9");
        let b = Release::new("S2.10");
        assert!(a < b, "S2.9 should be less than S2.10 (numeric)");
    }

    #[test]
    fn cross_track_comparison_is_reported_as_meaningless() {
        let a = Release::new("S2.1");
        let b = Release::new("5.5.3a");
        assert_eq!(
            a.same_track_cmp(&b),
            None,
            "no meaningful cross-track order"
        );
        // …but `Ord` still answers, and answers the same way every time.
        assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
    }

    /// `Ord` and `PartialOrd` must agree, or every sorted container is free to
    /// misbehave.
    #[test]
    fn ord_and_partial_ord_agree_on_every_pair() {
        let codes = [
            "S2.1", "S2.2", "S1.1a", "S1.1", "G1.0a", "G1.1", "5.5.3a", "5.5.10a", "2.4c", "2.9a",
            "2.10a", "UNKNOWN", "",
            // Two spellings that parse to the same kind: the raw-code tiebreak
            // has to keep `cmp` from calling them Equal while `Eq` says they
            // differ.
            "S02.1", "S2.01",
        ];
        for x in codes {
            for y in codes {
                let (a, b) = (Release::new(x), Release::new(y));
                assert_eq!(
                    a.partial_cmp(&b),
                    Some(a.cmp(&b)),
                    "partial_cmp disagrees with cmp for {x:?} vs {y:?}"
                );
                // Antisymmetry, and agreement with `Eq`.
                assert_eq!(a.cmp(&b), b.cmp(&a).reverse(), "{x:?} vs {y:?}");
                assert_eq!(
                    a.cmp(&b) == Ordering::Equal,
                    a == b,
                    "Ord and Eq disagree for {x:?} vs {y:?}"
                );
            }
        }
    }

    /// A corrigendum letter distinguishes two releases, so it must not be
    /// dropped: `S1.1a` and `S1.1` are different profiles.
    #[test]
    fn the_corrigendum_letter_is_not_discarded() {
        let plain = Release::new("S1.1");
        let corrected = Release::new("S1.1a");
        assert_ne!(plain, corrected);
        assert_eq!(plain.same_track_cmp(&corrected), Some(Ordering::Less));
        assert_eq!(plain.track(), ReleaseTrack::Strom);
        assert_eq!(corrected.track(), ReleaseTrack::Strom);
    }

    #[test]
    fn same_track_strom() {
        assert!(
            ReleaseKind::Strom {
                major: 2,
                minor: 1,
                letter: None
            }
            .same_track(&ReleaseKind::Strom {
                major: 2,
                minor: 2,
                letter: None
            })
        );
    }
}
