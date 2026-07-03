//! Common primitive value types for Redispatch 2.0 XML (identifiers, timestamps, decimals, market roles).
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::RedispatchXmlError;

// ── DocumentId ───────────────────────────────────────────────────────────────

/// A unique document identifier per sender and document type (max 35 chars,
/// case-sensitive). Used as `DocumentIdentification`, `OrderIdentification`,
/// `AllocationIdentification`, `mRID`, etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentId(String);

impl DocumentId {
    /// Create a new [`DocumentId`], returning an error if the value is empty or
    /// longer than 35 characters.
    #[must_use = "this returns the new DocumentId, discarding it is likely a mistake"]
    pub fn new(s: impl Into<String>) -> Result<Self, RedispatchXmlError> {
        let s = s.into();
        if s.is_empty() || s.len() > 35 {
            return Err(RedispatchXmlError::InvalidDocumentId(s));
        }
        Ok(Self(s))
    }

    /// Return the string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for DocumentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DocumentId {
    type Error = RedispatchXmlError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for DocumentId {
    type Error = RedispatchXmlError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for DocumentId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for DocumentId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

// ── Mrid (IEC 62325 style, same constraints as DocumentId) ───────────────────

/// An IEC 62325 message resource identifier (max 35 chars, case-sensitive).
/// Identifies a resource across its revision history; the current version is
/// the entry with the highest `revisionNumber` for this `mRID`.
pub type Mrid = DocumentId;

// ── DocumentVersion / RevisionNumber ─────────────────────────────────────────

/// A document version number (integer 1–999).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentVersion(u16);

impl DocumentVersion {
    /// Create a new [`DocumentVersion`], returning an error if outside 1–999.
    #[must_use = "this returns the new DocumentVersion, discarding it is likely a mistake"]
    pub fn new(v: u32) -> Result<Self, RedispatchXmlError> {
        if v == 0 || v > 999 {
            return Err(RedispatchXmlError::InvalidDocumentVersion(v));
        }
        Ok(Self(v as u16))
    }

    /// Return the numeric value.
    pub fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for DocumentVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for DocumentVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // XML encodes integer as a string token
        let v: u32 = Deserialize::deserialize(d)?;
        Self::new(v).map_err(serde::de::Error::custom)
    }
}

impl Serialize for DocumentVersion {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

/// An IEC 62325 revision number (integer 1–999).  Alias of [`DocumentVersion`].
pub type RevisionNumber = DocumentVersion;

// ── UtcDateTime (second-precision, yyyy-mm-ddThh:mm:ssZ) ─────────────────────

/// A UTC-only second-precision timestamp.
///
/// All BDEW Redispatch 2.0 datetime fields must end with `Z`. This type
/// rejects any offset other than UTC at deserialization time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcDateTime(OffsetDateTime);

impl UtcDateTime {
    /// Create from an [`OffsetDateTime`], returning an error if the offset is
    /// not UTC.
    #[must_use = "this returns the new UtcDateTime, discarding it is likely a mistake"]
    pub fn new(dt: OffsetDateTime) -> Result<Self, RedispatchXmlError> {
        if dt.offset() != time::UtcOffset::UTC {
            return Err(RedispatchXmlError::InvalidTimestamp(dt.to_string()));
        }
        Ok(Self(dt))
    }

    /// Return the inner [`OffsetDateTime`] (always UTC).
    pub fn inner(self) -> OffsetDateTime {
        self.0
    }
}

impl<'de> Deserialize<'de> for UtcDateTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let dt = OffsetDateTime::parse(&s, &Rfc3339)
            .map_err(|_| serde::de::Error::custom(format!("invalid UTC timestamp: {s:?}")))?;
        if dt.offset() != time::UtcOffset::UTC {
            return Err(serde::de::Error::custom(format!(
                "timestamp must use UTC (Z suffix): {s:?}"
            )));
        }
        Ok(UtcDateTime(dt))
    }
}

impl Serialize for UtcDateTime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Serialise back as yyyy-mm-ddThh:mm:ssZ
        self.0
            .format(&Rfc3339)
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
}

// ── UtcMinuteDateTime (minute-precision, yyyy-mm-ddThh:mmZ) ──────────────────

/// A UTC-only minute-precision timestamp used in time interval boundaries.
///
/// Format: `yyyy-mm-ddThh:mmZ` (no seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcMinuteDateTime(OffsetDateTime);

impl UtcMinuteDateTime {
    /// Create from an [`OffsetDateTime`], returning an error if not UTC.
    #[must_use = "this returns the new UtcMinuteDateTime, discarding it is likely a mistake"]
    pub fn new(dt: OffsetDateTime) -> Result<Self, RedispatchXmlError> {
        if dt.offset() != time::UtcOffset::UTC {
            return Err(RedispatchXmlError::InvalidTimestamp(dt.to_string()));
        }
        Ok(Self(dt))
    }

    /// Return the inner [`OffsetDateTime`] (always UTC).
    pub fn inner(self) -> OffsetDateTime {
        self.0
    }
}

const MINUTE_FMT: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]Z");

impl<'de> Deserialize<'de> for UtcMinuteDateTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let naive = time::PrimitiveDateTime::parse(&s, MINUTE_FMT).map_err(|_| {
            serde::de::Error::custom(format!("invalid UTC minute timestamp: {s:?}"))
        })?;
        Ok(UtcMinuteDateTime(naive.assume_offset(time::UtcOffset::UTC)))
    }
}

impl Serialize for UtcMinuteDateTime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0
            .format(MINUTE_FMT)
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
}

// ── TimeInterval (yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ) ───────────────────────

/// An ISO 8601 UTC time interval in the BDEW minute-precision format.
///
/// Format: `yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeInterval {
    /// Start of the interval (inclusive), always UTC.
    pub start: OffsetDateTime,
    /// End of the interval (exclusive), always UTC.
    pub end: OffsetDateTime,
}

impl TimeInterval {
    /// Create a new interval, validating that both timestamps are UTC and that
    /// start precedes end.
    #[must_use = "this returns the new TimeInterval, discarding it is likely a mistake"]
    pub fn new(start: OffsetDateTime, end: OffsetDateTime) -> Result<Self, RedispatchXmlError> {
        if start.offset() != time::UtcOffset::UTC || end.offset() != time::UtcOffset::UTC {
            return Err(RedispatchXmlError::InvalidTimeInterval(
                "timestamps must be UTC".into(),
            ));
        }
        if start >= end {
            return Err(RedispatchXmlError::InvalidTimeInterval(
                "start must be before end".into(),
            ));
        }
        Ok(Self { start, end })
    }
}

impl<'de> Deserialize<'de> for TimeInterval {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let (start_str, end_str) = s.split_once('/').ok_or_else(|| {
            serde::de::Error::custom(format!("invalid time interval {s:?}: missing '/'"))
        })?;
        let start_naive = time::PrimitiveDateTime::parse(start_str, MINUTE_FMT).map_err(|_| {
            serde::de::Error::custom(format!("invalid interval start {start_str:?}"))
        })?;
        let end_naive = time::PrimitiveDateTime::parse(end_str, MINUTE_FMT)
            .map_err(|_| serde::de::Error::custom(format!("invalid interval end {end_str:?}")))?;
        Ok(TimeInterval {
            start: start_naive.assume_offset(time::UtcOffset::UTC),
            end: end_naive.assume_offset(time::UtcOffset::UTC),
        })
    }
}

impl Serialize for TimeInterval {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let start = self
            .start
            .format(MINUTE_FMT)
            .map_err(serde::ser::Error::custom)?;
        let end = self
            .end
            .format(MINUTE_FMT)
            .map_err(serde::ser::Error::custom)?;
        format!("{start}/{end}").serialize(s)
    }
}

impl fmt::Display for TimeInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let start = self.start.format(MINUTE_FMT).map_err(|_| fmt::Error)?;
        let end = self.end.format(MINUTE_FMT).map_err(|_| fmt::Error)?;
        write!(f, "{start}/{end}")
    }
}

// ── MarketParticipantId (13 decimal digits) ───────────────────────────────────

/// A BDEW / GS1 market participant identifier (exactly 13 decimal digits).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MarketParticipantId(String);

impl MarketParticipantId {
    /// Create a new identifier, validating the 13-digit constraint.
    #[must_use = "this returns the new MarketParticipantId, discarding it is likely a mistake"]
    pub fn new(s: impl Into<String>) -> Result<Self, RedispatchXmlError> {
        let s = s.into();
        if s.len() != 13 || !s.chars().all(|c| c.is_ascii_digit()) {
            return Err(RedispatchXmlError::InvalidMarketParticipantId(s));
        }
        Ok(Self(s))
    }

    /// Return the string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for MarketParticipantId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MarketParticipantId {
    type Error = RedispatchXmlError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for MarketParticipantId {
    type Error = RedispatchXmlError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl fmt::Display for MarketParticipantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for MarketParticipantId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for MarketParticipantId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

// ── Decimal3 (0–999999.999, up to 3 fractional digits) ───────────────────────

/// A non-negative decimal with at most 3 fractional digits, used for power
/// quantities (0–999999.999 MW) and percentages (0–100.000 %).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Decimal3(f64);

impl Decimal3 {
    /// Create a new value.  Returns an error if `v` is negative.
    ///
    /// **Note**: due to binary floating-point representation, values with more
    /// than 3 fractional digits are accepted but will be rounded to 3 places
    /// on serialization. For exact decimal arithmetic use external rounding
    /// before construction.
    #[must_use = "this returns the new Decimal3, discarding it is likely a mistake"]
    pub fn new(v: f64) -> Result<Self, RedispatchXmlError> {
        if v < 0.0 {
            return Err(RedispatchXmlError::StructuralError(format!(
                "Decimal3 value {v} must be ≥ 0"
            )));
        }
        Ok(Self(v))
    }

    /// Return the raw `f64` value.
    pub fn value(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Decimal3 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // XML represents decimals as strings; serde will deserialise as f64
        let v: f64 = Deserialize::deserialize(d)?;
        Self::new(v).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Decimal3 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Serialise with exactly 3 decimal places.
        // E.g. 100.0 → "100.000", 50.5 → "50.500", 1.001 → "1.001"
        format!("{:.3}", self.0).serialize(s)
    }
}

impl fmt::Display for Decimal3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}", self.0)
    }
}

// ── CodingScheme ──────────────────────────────────────────────────────────────

/// Identifier coding scheme used for market participant IDs and object codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodingScheme {
    /// GS1 Global Location Number (GLN-13) or Global Service Relation Number
    /// (GSRN-18).
    #[serde(rename = "A10")]
    Gs1,
    /// German national coding scheme (BDEW-Code, 13-digit).
    #[serde(rename = "NDE")]
    Nde,
    /// Energy Identification Coding Scheme (EIC), maintained by ENTSO-E.
    #[serde(rename = "A01")]
    Eic,
}

// ── MeasureUnit ───────────────────────────────────────────────────────────────

/// Physical unit for quantity values in time series intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MeasureUnit {
    /// Megawatt (MW) — absolute power quantities.
    #[serde(rename = "MAW")]
    Megawatt,
    /// Percent (%) — relative to installed capacity (0–100).
    #[serde(rename = "P1")]
    Percent,
}

// ── Direction ─────────────────────────────────────────────────────────────────

/// Redispatch direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Direction {
    /// Upward redispatch: increase generation or decrease consumption.
    #[serde(rename = "A01")]
    Up,
    /// Downward redispatch: decrease generation or increase consumption.
    #[serde(rename = "A02")]
    Down,
}

// ── MarketRoleType ────────────────────────────────────────────────────────────

/// ENTSO-E harmonised market role codes used in sender/receiver role fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MarketRoleType {
    /// Balance responsible party (BKV).
    #[serde(rename = "A08")]
    BalanceResponsibleParty,
    /// Grid operator — TSO (ÜNB) or DSO (VNB).
    #[serde(rename = "A18")]
    GridOperator,
    /// Producer / generation asset owner.
    #[serde(rename = "A21")]
    Producer,
    /// Resource provider / Einspeiseverantwortlicher (EIV).
    #[serde(rename = "A27")]
    ResourceProvider,
    /// Data provider (e.g. metering point operator forwarding data).
    #[serde(rename = "A39")]
    DataProvider,
    /// Supplier (Lieferant).
    #[serde(rename = "Z01")]
    Supplier,
}

// ── ControlZone ───────────────────────────────────────────────────────────────

/// German TSO control zone EIC codes used in `ConnectingArea`,
/// `AcquiringArea`, and `biddingZone_Domain` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ControlZone {
    /// TransnetBW.
    #[serde(rename = "10YDE-ENBW-----N")]
    TransnetBw,
    /// TenneT DE.
    #[serde(rename = "10YDE-EON------1")]
    TennetDe,
    /// Amprion.
    #[serde(rename = "10YDE-RWENET---I")]
    Amprion,
    /// 50Hertz.
    #[serde(rename = "10YDE-VE-------2")]
    FiftyHertz,
    /// Schleswig-Holstein / Flensburg.
    #[serde(rename = "10YFLENSBURG---3")]
    Flensburg,
    /// DB Netz AG (railway grid).
    #[serde(rename = "11YRBAHNSTROM--P")]
    Bahnstrom,
}
