//! §42c EnWG Energy-Sharing metering eligibility — pure decision logic.
//!
//! Energy Sharing has been in force since **1 June 2026**. Its binding practical
//! constraint is not the allocation engine — it is which delivery points can
//! produce quarter-hour values at all. §42c Abs. 1 admits a point only when both
//! consumption *and* generation are measured by:
//!
//! > „Zählerstandsgangmessung nach § 2 Satz 1 Nummer 27 des
//! > Messstellenbetriebsgesetzes **oder** durch eine viertelstündliche
//! > registrierende Leistungsmessung"
//!
//! The **`oder` is load-bearing**. Zählerstandsgangmessung and viertelstündliche
//! RLM are two independent qualifying bases, so a conventional RLM meter
//! installed before the iMSys rollout qualifies on its own. Treating
//! "Zählerstandsgangmessung" as a synonym for "iMSys" both over-restricts (it
//! excludes conforming RLM) and over-permits (an iMSys that is not configured for
//! Zählerstandsgangmessung produces no quarter-hour series).
//!
//! # The two dimensions
//!
//! Eligibility is not one question but two, answered from different stores:
//!
//! | Dimension | Question | Source |
//! |---|---|---|
//! | **Capability** | Can this point produce quarter-hour values? | Device master data |
//! | **Delivery** | Is it actually producing them? | Observed intervals |
//!
//! Keeping them apart is the point. A point with an iMSys installed but no
//! Zählerstandsgangmessung configured is *capable but not delivering* — it needs
//! a configuration order, not a meter rollout. Collapsing the two into a single
//! boolean hides exactly the distinction an operator has to act on.
//!
//! # Definitions
//!
//! - **§2 Satz 1 Nr. 27 MsbG — Zählerstandsgangmessung**: „die Messung einer
//!   Reihe viertelstündig ermittelter Zählerstände von elektrischer Arbeit und
//!   stündlich ermittelter Zählerstände von Gasmengen". Electricity is
//!   quarter-hourly; §42c is Strom-only, so the gas branch never applies here.
//! - **§2 Satz 1 Nr. 7 MsbG — intelligentes Messsystem (iMSys)**: a moderne
//!   Messeinrichtung *or* a Messeinrichtung zur registrierenden Leistungsmessung
//!   bound into a communication network via a Smart-Meter-Gateway.
//! - **§2 Satz 1 Nr. 15 MsbG — moderne Messeinrichtung (mME)**: reflects
//!   consumption and usage time. No gateway, no interval series — **not**
//!   sufficient for §42c on its own.

use time::OffsetDateTime;

use crate::classification::{IntervalLengthClass, Messtyp};

// ── Qualifying basis ──────────────────────────────────────────────────────────

/// The statutory basis on which a delivery point qualifies under §42c Abs. 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EligibilityBasis {
    /// Zählerstandsgangmessung per §2 Satz 1 Nr. 27 MsbG — the iMSys route.
    Zaehlerstandsgangmessung,
    /// Viertelstündliche registrierende Leistungsmessung — the RLM route.
    ///
    /// Independently sufficient; does not require a Smart-Meter-Gateway.
    RegistrierendeLeistungsmessung,
}

impl EligibilityBasis {
    /// Stable label for API responses and logs.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Zaehlerstandsgangmessung => "ZAEHLERSTANDSGANGMESSUNG",
            Self::RegistrierendeLeistungsmessung => "REGISTRIERENDE_LEISTUNGSMESSUNG",
        }
    }

    /// The statutory citation this basis rests on.
    #[must_use]
    pub const fn legal_basis(self) -> &'static str {
        match self {
            Self::Zaehlerstandsgangmessung => "§2 Satz 1 Nr. 27 MsbG",
            Self::RegistrierendeLeistungsmessung => "§42c Abs. 1 EnWG",
        }
    }
}

// ── Capability (master data) ──────────────────────────────────────────────────

/// Device master data for one delivery point.
///
/// All fields are `Option` because master data is routinely incomplete; the
/// assessment reports *why* it could not decide rather than guessing.
#[derive(Debug, Clone, Default)]
pub struct MeteringCapabilityInput {
    /// BO4E `Zaehlertyp` wire value, e.g. `INTELLIGENTES_MESSSYSTEM`,
    /// `MODERNE_MESSEINRICHTUNG`, `DREHSTROMZAEHLER`.
    pub zaehlertyp: Option<String>,
    /// BO4E `Zaehler.istFernauslesbar`. A meter that cannot be read remotely
    /// cannot supply a quarter-hour series regardless of its type.
    pub ist_fernauslesbar: Option<bool>,
    /// `Marktlokation.bilanzierungsmethode` — `RLM` | `SLP` | `IMS` | `TLP_*` | `PAUSCHAL`.
    pub bilanzierungsmethode: Option<String>,
    /// Whether an operational Smart-Meter-Gateway session exists for this point.
    pub smgw_operational: Option<bool>,
}

/// Outcome of the master-data capability assessment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Capability {
    /// Master data supports a §42c-qualifying measurement.
    Qualified(EligibilityBasis),
    /// Master data positively rules the point out.
    Disqualified,
    /// Master data is insufficient to decide.
    Unknown,
}

impl Capability {
    /// The qualifying basis, when one was established.
    #[must_use]
    pub const fn basis(self) -> Option<EligibilityBasis> {
        match self {
            Self::Qualified(b) => Some(b),
            _ => None,
        }
    }
}

/// Assess §42c capability from device master data.
///
/// Order matters. `bilanzierungsmethode = RLM` is checked before the meter type
/// because RLM qualifies on its own statutory limb and does not require a
/// gateway. `IMS` and an `INTELLIGENTES_MESSSYSTEM` meter qualify on the
/// Zählerstandsgangmessung limb, but only when the point is remotely readable —
/// an iMSys with no working gateway produces nothing.
///
/// A `MODERNE_MESSEINRICHTUNG` is explicitly disqualified: per §2 Satz 1 Nr. 15
/// MsbG an mME records consumption and usage time but has no gateway and no
/// interval series.
#[must_use]
pub fn assess_capability(input: &MeteringCapabilityInput) -> (Capability, Vec<String>) {
    let mut reasons = Vec::new();

    let methode = input.bilanzierungsmethode.as_deref().map(str::trim);
    let typ = input.zaehlertyp.as_deref().map(str::trim);

    // Limb 2 — viertelstündliche registrierende Leistungsmessung.
    if methode == Some("RLM") {
        return (
            Capability::Qualified(EligibilityBasis::RegistrierendeLeistungsmessung),
            reasons,
        );
    }

    // Limb 1 — Zählerstandsgangmessung via iMSys.
    let looks_imsys = methode == Some("IMS")
        || typ == Some("INTELLIGENTES_MESSSYSTEM")
        || input.smgw_operational == Some(true);

    if looks_imsys {
        // Remote readability is a precondition, not a nicety: without it there is
        // no series of viertelstündig ermittelte Zählerstände to transmit.
        if input.ist_fernauslesbar == Some(false) {
            reasons.push(
                "Zähler ist als nicht fernauslesbar gekennzeichnet — \
                 keine Zählerstandsgangmessung möglich"
                    .to_owned(),
            );
            return (Capability::Disqualified, reasons);
        }
        return (
            Capability::Qualified(EligibilityBasis::Zaehlerstandsgangmessung),
            reasons,
        );
    }

    match typ {
        Some("MODERNE_MESSEINRICHTUNG") => {
            reasons.push(
                "moderne Messeinrichtung ohne Smart-Meter-Gateway (§2 Satz 1 Nr. 15 MsbG) — \
                 iMSys-Rollout oder RLM erforderlich"
                    .to_owned(),
            );
            (Capability::Disqualified, reasons)
        }
        Some(_) if matches!(methode, Some("SLP") | Some("PAUSCHAL")) => {
            reasons.push(format!(
                "Bilanzierungsmethode {} liefert keine Viertelstundenwerte",
                methode.unwrap_or("?")
            ));
            (Capability::Disqualified, reasons)
        }
        _ => {
            if typ.is_none() {
                reasons.push("kein Zählertyp im Stammdatensatz hinterlegt".to_owned());
            }
            if methode.is_none() {
                reasons.push("keine Bilanzierungsmethode an der Marktlokation".to_owned());
            }
            if reasons.is_empty() {
                reasons.push(format!(
                    "Zählertyp {} ist weder iMSys noch RLM",
                    typ.unwrap_or("?")
                ));
                return (Capability::Disqualified, reasons);
            }
            (Capability::Unknown, reasons)
        }
    }
}

// ── Delivery (observed data) ──────────────────────────────────────────────────

/// Observed interval evidence for one delivery point.
#[derive(Debug, Clone, Default)]
pub struct DeliveryEvidenceInput {
    /// Detected interval length across the observation window.
    pub interval_class: Option<IntervalLengthClass>,
    /// Classification derived from the observed series.
    pub messtyp: Option<Messtyp>,
    /// Share of expected quarter-hour slots actually present, 0.0–100.0.
    pub coverage_pct: Option<f64>,
    /// Number of readings inspected.
    pub reading_count: u64,
    /// Most recent reading timestamp, if any.
    pub last_reading_at: Option<OffsetDateTime>,
}

/// Whether the point is in fact delivering a quarter-hour series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Delivery {
    /// Quarter-hour values observed at or above the coverage threshold.
    Delivering,
    /// Values observed, but not at quarter-hour resolution or below threshold.
    Insufficient,
    /// No readings in the observation window.
    Absent,
}

/// Minimum share of expected quarter-hour slots for a point to count as
/// delivering.
///
/// §42c fixes no coverage figure; this is an operational threshold for the
/// readiness report, deliberately strict because a sharing allocation cannot
/// close an interval it has no value for.
pub const DEFAULT_COVERAGE_THRESHOLD_PCT: f64 = 95.0;

/// Assess actual quarter-hour delivery from observed intervals.
#[must_use]
pub fn assess_delivery(
    input: &DeliveryEvidenceInput,
    coverage_threshold_pct: f64,
) -> (Delivery, Vec<String>) {
    let mut reasons = Vec::new();

    if input.reading_count == 0 {
        reasons.push("keine Messwerte im Betrachtungszeitraum".to_owned());
        return (Delivery::Absent, reasons);
    }

    if input.interval_class != Some(IntervalLengthClass::FifteenMin) {
        reasons.push(match input.interval_class {
            Some(c) => format!(
                "Messwerte liegen in {}-Sekunden-Intervallen vor, nicht viertelstündlich",
                c.seconds()
            ),
            None => "Intervalllänge konnte nicht bestimmt werden".to_owned(),
        });
        return (Delivery::Insufficient, reasons);
    }

    if let Some(cov) = input.coverage_pct
        && cov < coverage_threshold_pct
    {
        reasons.push(format!(
            "Abdeckung {cov:.1} % unter Schwelle {coverage_threshold_pct:.1} %"
        ));
        return (Delivery::Insufficient, reasons);
    }

    (Delivery::Delivering, reasons)
}

// ── Combined verdict ──────────────────────────────────────────────────────────

/// Overall §42c readiness for one delivery point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SharingReadiness {
    /// Capable and delivering — can join a sharing community today.
    Ready,
    /// Capable, but no conforming quarter-hour series is arriving.
    ///
    /// The actionable middle state: needs a Zählerstandsgangmessung
    /// configuration order, not a meter rollout.
    CapableNotDelivering,
    /// Master data rules the point out — an iMSys rollout or RLM is required.
    NotCapable,
    /// Insufficient master data to decide.
    Unknown,
}

impl SharingReadiness {
    /// Stable label for API responses.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::CapableNotDelivering => "CAPABLE_NOT_DELIVERING",
            Self::NotCapable => "NOT_CAPABLE",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// The operator action this verdict calls for.
    #[must_use]
    pub const fn required_action(self) -> &'static str {
        match self {
            Self::Ready => "keine",
            Self::CapableNotDelivering => "Zählerstandsgangmessung beauftragen",
            Self::NotCapable => "iMSys-Rollout oder RLM-Umbau beauftragen",
            Self::Unknown => "Stammdaten vervollständigen",
        }
    }
}

/// Combine a capability and a delivery assessment into one verdict.
///
/// Delivery alone never establishes eligibility: §42c is a statement about the
/// measurement installed at the point, so a conforming series from an
/// unidentifiable meter still leaves the master data to be fixed.
#[must_use]
pub const fn combine(capability: Capability, delivery: Delivery) -> SharingReadiness {
    match (capability, delivery) {
        (Capability::Qualified(_), Delivery::Delivering) => SharingReadiness::Ready,
        (Capability::Qualified(_), _) => SharingReadiness::CapableNotDelivering,
        (Capability::Disqualified, _) => SharingReadiness::NotCapable,
        (Capability::Unknown, _) => SharingReadiness::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(zt: Option<&str>, fern: Option<bool>, bm: Option<&str>) -> MeteringCapabilityInput {
        MeteringCapabilityInput {
            zaehlertyp: zt.map(str::to_owned),
            ist_fernauslesbar: fern,
            bilanzierungsmethode: bm.map(str::to_owned),
            smgw_operational: None,
        }
    }

    #[test]
    fn rlm_qualifies_without_a_gateway() {
        // §42c Abs. 1: "oder durch eine viertelstündliche registrierende
        // Leistungsmessung" — an independent limb.
        let (c, _) = assess_capability(&cap(Some("DREHSTROMZAEHLER"), None, Some("RLM")));
        assert_eq!(
            c,
            Capability::Qualified(EligibilityBasis::RegistrierendeLeistungsmessung)
        );
    }

    #[test]
    fn imsys_qualifies_on_the_zsg_limb() {
        let (c, _) = assess_capability(&cap(Some("INTELLIGENTES_MESSSYSTEM"), Some(true), None));
        assert_eq!(
            c,
            Capability::Qualified(EligibilityBasis::Zaehlerstandsgangmessung)
        );
    }

    #[test]
    fn imsys_that_cannot_be_read_remotely_is_disqualified() {
        let (c, reasons) = assess_capability(&cap(
            Some("INTELLIGENTES_MESSSYSTEM"),
            Some(false),
            Some("IMS"),
        ));
        assert_eq!(c, Capability::Disqualified);
        assert!(reasons[0].contains("fernauslesbar"), "{reasons:?}");
    }

    #[test]
    fn moderne_messeinrichtung_is_not_sufficient() {
        // §2 Satz 1 Nr. 15 MsbG — no gateway, no interval series.
        let (c, reasons) = assess_capability(&cap(Some("MODERNE_MESSEINRICHTUNG"), None, None));
        assert_eq!(c, Capability::Disqualified);
        assert!(reasons[0].contains("Gateway"), "{reasons:?}");
    }

    #[test]
    fn slp_is_disqualified() {
        let (c, _) = assess_capability(&cap(Some("DREHSTROMZAEHLER"), None, Some("SLP")));
        assert_eq!(c, Capability::Disqualified);
    }

    #[test]
    fn missing_master_data_is_unknown_not_a_guess() {
        let (c, reasons) = assess_capability(&cap(None, None, None));
        assert_eq!(c, Capability::Unknown);
        assert_eq!(reasons.len(), 2, "both gaps reported: {reasons:?}");
    }

    #[test]
    fn delivery_requires_quarter_hour_resolution() {
        let ev = DeliveryEvidenceInput {
            interval_class: Some(IntervalLengthClass::SixtyMin),
            reading_count: 100,
            ..Default::default()
        };
        let (d, reasons) = assess_delivery(&ev, DEFAULT_COVERAGE_THRESHOLD_PCT);
        assert_eq!(d, Delivery::Insufficient);
        assert!(reasons[0].contains("3600"), "{reasons:?}");
    }

    #[test]
    fn delivery_requires_coverage_above_threshold() {
        let ev = DeliveryEvidenceInput {
            interval_class: Some(IntervalLengthClass::FifteenMin),
            coverage_pct: Some(80.0),
            reading_count: 100,
            ..Default::default()
        };
        let (d, _) = assess_delivery(&ev, DEFAULT_COVERAGE_THRESHOLD_PCT);
        assert_eq!(d, Delivery::Insufficient);
    }

    #[test]
    fn capable_but_silent_is_its_own_verdict() {
        // The state the readiness report exists to surface.
        let verdict = combine(
            Capability::Qualified(EligibilityBasis::Zaehlerstandsgangmessung),
            Delivery::Absent,
        );
        assert_eq!(verdict, SharingReadiness::CapableNotDelivering);
        assert_eq!(
            verdict.required_action(),
            "Zählerstandsgangmessung beauftragen"
        );
    }

    #[test]
    fn delivery_alone_does_not_establish_eligibility() {
        assert_eq!(
            combine(Capability::Unknown, Delivery::Delivering),
            SharingReadiness::Unknown
        );
    }
}
