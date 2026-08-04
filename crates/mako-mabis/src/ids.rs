//! Balance-group topology identifiers (MaBiS).
//!
//! `BilanzierungsgebietId` and `BilanzkreisId` model the balance-group topology
//! from BK6-22-024 (MaBiS). MABIS Summenzeitreihen key on them, which is why
//! they live in `mako-mabis`.

use serde::{Deserialize, Serialize};

/// A Bilanzierungsgebiet (settlement zone) within the German electricity grid.
///
/// Each ÜNB / NB operates one or more Bilanzierungsgebiete. All MaLos within a
/// Bilanzierungsgebiet belong to the same settlement pool for MaBiS.
///
/// Source: BK6-22-024 (MaBiS) — Bilanzierungsgebiet definitions; BDEW code list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BilanzierungsgebietId(pub String);

impl std::fmt::Display for BilanzierungsgebietId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Bilanzkreis (balance group) within a Bilanzierungsgebiet.
///
/// A BKV (Bilanzkreisverantwortlicher) holds one or more Bilanzkreise. Each MaLo
/// is assigned to exactly one Bilanzkreis within its Bilanzierungsgebiet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BilanzkreisId(pub String);

impl std::fmt::Display for BilanzkreisId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── MabisZaehlpunktId ─────────────────────────────────────────────────────────

/// A MaBiS-Zählpunkt — the Meldepunkt a Summenzeitreihe is filed under.
///
/// # Why this is a type and not a `String`
///
/// MSCONS SG6 carries three `LOC` qualifiers whose values are all free text at
/// the MIG level: `172` the Meldepunkt, `107` the Bilanzierungsgebiet, `237` the
/// Bilanzkreis. A message that puts the territory EIC in `LOC+172` parses,
/// validates and is **accepted by the BIKO**, which then files the series
/// against the wrong Meldepunkt. Nothing downstream can tell that apart from a
/// correct submission.
///
/// [`BilanzierungsgebietId`] was already a newtype while this stayed a bare
/// `String`, so exactly one half of the dangerous pair was protected. With both
/// typed, passing one where the other belongs is a compile error rather than a
/// settlement filed against the wrong point.
///
/// # Format
///
/// A Zählpunktbezeichnung is **33 characters**. A Bilanzierungsgebiet EIC is 16,
/// so the length alone separates them — which is why the constructor enforces it
/// and why `Deserialize` goes through the same check rather than around it.
///
/// # Where this type is *not* used
///
/// Inbound commands keep a plain `String` — see
/// [`ZpLifecycleCommand::ReceiveAnfrage`](crate::zp_lifecycle::ZpLifecycleCommand).
/// Making a counterparty's malformed Meldepunkt unconstructible would leave the
/// workflow unable to record what actually arrived, and therefore unable to
/// reject it properly. Parsing into a type belongs on values this system
/// *produces*; a value it *receives* has to be representable before it can be
/// refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct MabisZaehlpunktId(String);

impl MabisZaehlpunktId {
    /// The statutory length of a Zählpunktbezeichnung.
    pub const LENGTH: usize = 33;

    /// Parse a Zählpunktbezeichnung.
    ///
    /// # Errors
    ///
    /// [`InvalidMabisZaehlpunkt`] when the value is not 33 characters. A
    /// 16-character EIC — the shape of a Bilanzierungsgebiet — fails here rather
    /// than reaching the wire as a Meldepunkt.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidMabisZaehlpunkt> {
        let value = value.into();
        let len = value.chars().count();
        if len != Self::LENGTH {
            return Err(InvalidMabisZaehlpunkt { value, len });
        }
        Ok(Self(value))
    }

    /// The underlying Zählpunktbezeichnung.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MabisZaehlpunktId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Deserialization goes through [`MabisZaehlpunktId::new`].
///
/// Deriving it would let a JSON payload construct a value the constructor
/// rejects, which is the whole hole this type closes — the identifier usually
/// *arrives* as JSON from marktd or a command API.
impl<'de> Deserialize<'de> for MabisZaehlpunktId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// A value offered as a MaBiS-Zählpunkt that is not a Zählpunktbezeichnung.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{value}` is {len} characters — a MaBiS-Zählpunkt (Zählpunktbezeichnung) is 33. \
     A 16-character value is a Bilanzierungsgebiet EIC; submitting it as the \
     Meldepunkt (MSCONS SG6 LOC+172) files the Summenzeitreihe against the wrong \
     point, and the BIKO accepts it"
)]
pub struct InvalidMabisZaehlpunkt {
    /// The offered value.
    pub value: String,
    /// Its character count.
    pub len: usize,
}

#[cfg(test)]
mod zaehlpunkt_tests {
    use super::*;

    /// The length check is what separates a Meldepunkt from a territory EIC.
    #[test]
    fn a_bilanzierungsgebiet_eic_is_not_a_meldepunkt() {
        let err = MabisZaehlpunktId::new("11XSWISSGRIDBGX1").expect_err("must be refused");
        assert_eq!(err.len, 16);
        assert!(err.to_string().contains("33"), "{err}");
    }

    #[test]
    fn a_thirty_three_character_zaehlpunkt_is_accepted() {
        let zp = MabisZaehlpunktId::new("DE0004030099000000000000000012345").expect("valid");
        assert_eq!(zp.as_str().chars().count(), 33);
    }

    #[test]
    fn the_empty_string_is_not_a_meldepunkt() {
        assert!(MabisZaehlpunktId::new("").is_err());
    }

    /// The identifier usually arrives as JSON, so `Deserialize` must run the
    /// same check — otherwise serde is a way around the constructor.
    #[test]
    fn deserialization_cannot_bypass_the_constructor() {
        let ok: Result<MabisZaehlpunktId, _> =
            serde_json::from_str("\"DE0004030099000000000000000012345\"");
        assert!(ok.is_ok());

        let bad: Result<MabisZaehlpunktId, _> = serde_json::from_str("\"11XSWISSGRIDBGX1\"");
        let err = bad.expect_err("a territory EIC must not deserialize into a Meldepunkt");
        assert!(err.to_string().contains("33"), "{err}");
    }

    /// Round-trips through JSON as a plain string, so the wire format is
    /// unchanged by the type.
    #[test]
    fn it_serializes_transparently() {
        let zp = MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap();
        assert_eq!(
            serde_json::to_string(&zp).unwrap(),
            "\"DE0004030099000000000000000012345\""
        );
    }
}
