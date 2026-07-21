//! Core metering types: [`MeterInterval`], [`Sparte`], [`QualityFlag`].

use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Energy commodity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Sparte {
    /// Electricity.
    #[default]
    Strom,
    /// Natural gas.
    Gas,
    /// Heat (Fern-/Nahwärme, Wärmemengenzähler per EN 1434 / MID MI-004).
    ///
    /// A heat meter integrates flow against the supply/return temperature
    /// difference on-device, so its register holds thermal kWh directly.
    /// Governed by **HeizkostenV**, not MsbG.
    Waerme,
    /// Water (Kalt-/Warmwasser).
    ///
    /// Metered **and billed** in m³ — the only Sparte whose billing unit is a
    /// volume. Water has no calorific value, so the gas m³→kWh path does not
    /// apply to it. For the heat share of warm water see
    /// [`crate::warm_water_heat_kwh`] (HeizkostenV §9 Abs. 2).
    Wasser,
}

impl Sparte {
    /// The unit this Sparte's meter register advances in.
    ///
    /// A gas meter registers m³ of Betriebsvolumen; its energy content is
    /// derived from Brennwert and Zustandszahl. Electricity and heat meters
    /// register energy directly.
    #[must_use]
    pub const fn measured_unit(self) -> MeasurementUnit {
        match self {
            Self::Strom | Self::Waerme => MeasurementUnit::KiloWattHour,
            Self::Gas | Self::Wasser => MeasurementUnit::CubicMetre,
        }
    }

    /// The unit this Sparte is settled and invoiced in.
    ///
    /// Differs from [`Sparte::measured_unit`] only for gas, which is metered in
    /// m³ and billed in kWh.
    #[must_use]
    pub const fn billing_unit(self) -> MeasurementUnit {
        match self {
            Self::Strom | Self::Gas | Self::Waerme => MeasurementUnit::KiloWattHour,
            Self::Wasser => MeasurementUnit::CubicMetre,
        }
    }

    /// `true` when the measured unit differs from the billing unit, so a
    /// reading must be converted before it can be settled. Gas only.
    ///
    /// [`crate::gas_m3_to_kwh_hs`] performs the conversion. An ingest path uses
    /// this to require the conversion parameters up front.
    #[must_use]
    pub const fn requires_conversion(self) -> bool {
        matches!(
            (self.measured_unit(), self.billing_unit()),
            (MeasurementUnit::KiloWattHour, MeasurementUnit::CubicMetre)
                | (MeasurementUnit::CubicMetre, MeasurementUnit::KiloWattHour)
        )
    }

    /// Stable DB/wire label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strom => "STROM",
            Self::Gas => "GAS",
            Self::Waerme => "WAERME",
            Self::Wasser => "WASSER",
        }
    }
}

/// Unit a meter reading is expressed in.
///
/// Electricity, gas and heat settle in kWh; water settles in m³. Carrying the
/// unit alongside the value keeps the two dimensions distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MeasurementUnit {
    /// Kilowatt-hour. Electricity and heat as measured; gas only after
    /// conversion.
    #[default]
    KiloWattHour,
    /// Cubic metre. Water as measured and billed; gas as measured, before
    /// conversion.
    CubicMetre,
}

impl MeasurementUnit {
    /// Stable DB/wire label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KiloWattHour => "KWH",
            Self::CubicMetre => "M3",
        }
    }

    /// Parse a unit string that is already canonical.
    ///
    /// Accepts the superscript `m³` as well as `m3`. Units that need rescaling
    /// (MWh, GJ, litres) are rejected here; use
    /// [`MeasurementUnit::parse_scaled`] for those.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let scaled = Self::parse_scaled(s)?;
        scaled.is_canonical().then_some(scaled.unit)
    }

    /// Parse any accepted unit, returning the canonical unit plus the exact
    /// factor that converts a value into it.
    ///
    /// EN 1434-1 cl. 6.3.1 permits heat meters to register in Joules or
    /// Watt-hours and any decimal multiple, so kWh, MWh and GJ registers are all
    /// in service; water submeters commonly report litres. Callers rescale to
    /// the canonical unit before storing, keeping exactly two units in the
    /// persisted data.
    #[must_use]
    pub fn parse_scaled(s: &str) -> Option<UnitScale> {
        let (unit, num, den) = match s.trim().to_lowercase().as_str() {
            // ── Human/device unit symbols, as printed on a meter display ────
            "kwh" | "kwh_th" | "kwh_hs" => (Self::KiloWattHour, 1, 1),
            "wh" => (Self::KiloWattHour, 1, 1_000),
            "mwh" => (Self::KiloWattHour, 1_000, 1),
            "gwh" => (Self::KiloWattHour, 1_000_000, 1),
            // 1 GJ = 1e9 J and 1 kWh = 3.6e6 J, so the factor is 1000/3.6 —
            // a repeating decimal. Held as the exact rational 2500/9 so the
            // conversion does not lose precision on every single reading.
            "gj" => (Self::KiloWattHour, 2_500, 9),
            "mj" => (Self::KiloWattHour, 5, 18),
            "m3" | "m³" | "cbm" => (Self::CubicMetre, 1, 1),
            "l" | "ltr" | "liter" | "litre" => (Self::CubicMetre, 1, 1_000),

            // ── UN/ECE Recommendation 20 codes ──────────────────────────────
            // Used by UTILMD DE6411 and EN 16931/PEPPOL. The codes are not the
            // unit symbols: megajoule is `3B`, gigajoule is `GV`, cubic metre is
            // `MTQ`. Rec 20 also assigns `GJ` to gram per millilitre; the symbol
            // reading wins here because no Sparte modelled in this crate carries
            // a density. Callers emitting Rec 20 should send `GV`.
            "mtq" => (Self::CubicMetre, 1, 1),
            "whr" => (Self::KiloWattHour, 1, 1_000),
            "gv" => (Self::KiloWattHour, 2_500, 9),
            "3b" => (Self::KiloWattHour, 5, 18),
            "jou" => (Self::KiloWattHour, 1, 3_600_000),
            "kjo" => (Self::KiloWattHour, 1, 3_600),
            _ => return None,
        };
        Some(UnitScale { unit, num, den })
    }
}

/// A parsed unit together with the exact rational factor converting a value in
/// that unit into the canonical [`MeasurementUnit`].
///
/// The factor is kept as a numerator/denominator pair rather than a single
/// `Decimal` because the useful ones repeat: 1 GJ is 277.7… kWh. Multiplying
/// before dividing keeps the result exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitScale {
    /// The canonical unit the value converts into.
    pub unit: MeasurementUnit,
    /// Conversion numerator.
    pub num: i64,
    /// Conversion denominator.
    pub den: i64,
}

impl UnitScale {
    /// `true` when the source unit is already canonical (factor 1:1).
    #[must_use]
    pub const fn is_canonical(self) -> bool {
        self.num == self.den
    }

    /// Convert `value` into [`UnitScale::unit`].
    ///
    /// Multiplies before dividing so that a repeating factor such as GJ→kWh
    /// (2500/9) rounds once, at the end, rather than once per operand.
    #[must_use]
    pub fn apply(self, value: Decimal) -> Decimal {
        if self.is_canonical() {
            return value;
        }
        value * Decimal::from(self.num) / Decimal::from(self.den)
    }
}

/// BDEW / MSCONS quality flag.
///
/// Maps to the `MESSWERTSTATUS` field in MSCONS and
/// the BO4E `Messwertstatus` enum.
///
/// ## Billability (§ 60 Abs. 2 MsbG)
///
/// § 60 Abs. 2 MsbG requires substitute values (Ersatzwerte) when measurements are
/// absent. An estimated value (Prognosewert) IS a valid billing basis for
/// advance payments and Abschlagsrechnung:
///
/// | Flag | Billable | Notes |
/// |---|---|---|
/// | `Measured` | ✓ | Actual reading — highest confidence |
/// | `Estimated` | ✓ | Prognosewert — valid for advance billing (§ 60 Abs. 2 MsbG) |
/// | `Substituted` | ✓ | Ersatzwert — replacement by MSB when measurement failed |
/// | `Calculated` | ✓ | Derived from other measurements (e.g. Residuallast) |
/// | `Corrected` | ✓ | Nachbearbeitet — corrected from an earlier value |
/// | `Preliminary` | ✓ | Vorläufiger Wert — may be revised later |
/// | `Faulty` | ✗ | Fehlerhaft — measurement error, must not be billed |
/// | `Unknown` | ✗ | Quality not determinable — do not bill |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum QualityFlag {
    /// Reading as measured (Abgelesen / Messwert).
    Measured,
    /// Estimated value (Prognosewert) — valid for advance billing per § 60 Abs. 2 MsbG.
    ///
    /// Used in Abschlagsrechnung before final annual read.
    /// Also generated by SLP profiling for customers without interval metering.
    Estimated,
    /// Substituted / replaced value (Ersatzwert).
    ///
    /// Generated by the Messstellenbetreiber when measurement failed.
    /// § 60 Abs. 2 MsbG: must be used for billing when measurement unavailable.
    Substituted,
    /// Calculated / derived value (Vorlaeufiger Wert / Rechenwert).
    ///
    /// Derived from other meter readings (e.g. Residuallast = Bezug − Einspeisung).
    Calculated,
    /// Corrected value (Nachbearbeitungswert).
    ///
    /// Originally measured/estimated, subsequently corrected by MSB.
    /// Replaces a prior value — billable, supersedes earlier reading.
    Corrected,
    /// Preliminary value (Vorläufiger Wert).
    ///
    /// Valid for billing but may be revised. Record as provisional in invoice.
    Preliminary,
    /// Faulty measurement (Fehlerhaft / Unplausibel).
    ///
    /// Must NOT be used for billing. Requires substitute value generation.
    Faulty,
    /// Quality not known.
    #[default]
    Unknown,
}

impl QualityFlag {
    /// `true` when this flag indicates the value is reliable for billing.
    ///
    /// ## § 60 Abs. 2 MsbG note
    ///
    /// `Estimated` (Prognosewert) IS billable — it is the statutory mechanism
    /// for advance billing before the actual annual read is available.
    /// Excluding estimated values from billing would produce zero arbeitsmenge
    /// for SLP customers and during measurement outages, which is wrong.
    #[must_use]
    pub fn is_billable(&self) -> bool {
        matches!(
            self,
            QualityFlag::Measured
                | QualityFlag::Estimated
                | QualityFlag::Substituted
                | QualityFlag::Calculated
                | QualityFlag::Corrected
                | QualityFlag::Preliminary
        )
    }

    /// `true` when this value should be flagged as provisional in invoices.
    ///
    /// Preliminary values are billable but the invoice should note they may be revised.
    #[must_use]
    pub fn is_provisional(&self) -> bool {
        matches!(self, QualityFlag::Preliminary | QualityFlag::Estimated)
    }
}

/// A single metered interval: the fundamental unit of meter data.
///
/// All energy values are in **kWh** (Strom) or **kWh_Hs** (Gas after conversion).
/// Use [`crate::conversion::gas_m3_to_kwh_hs`] to convert Gas m³ readings before
/// creating `MeterInterval`s.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeterInterval {
    /// Interval start (UTC, inclusive).
    pub from: OffsetDateTime,
    /// Interval end (UTC, exclusive).
    pub to: OffsetDateTime,
    /// Energy quantity in kWh (or kWh_Hs for Gas).
    pub value_kwh: Decimal,
    /// Reading quality.
    pub quality: QualityFlag,
    /// OBIS-Kennzahl (e.g. `"1-0:1.8.0"`). `None` when not provided by MSCONS.
    pub obis_code: Option<String>,
}

impl MeterInterval {
    /// Duration in whole seconds.
    #[must_use]
    pub fn duration_secs(&self) -> i64 {
        (self.to - self.from).whole_seconds()
    }

    /// Duration in minutes.
    #[must_use]
    pub fn duration_minutes(&self) -> i64 {
        (self.to - self.from).whole_minutes()
    }

    /// Instantaneous demand in kW, computed as `kWh ÷ (duration_h)`.
    ///
    /// Only meaningful for RLM intervals (15-min or 60-min).
    /// For a 15-min interval carrying 2.5 kWh: demand = 2.5 × 4 = 10 kW.
    #[must_use]
    pub fn demand_kw(&self) -> Option<Decimal> {
        let h = Decimal::from(self.duration_secs()) / Decimal::from(3600u32);
        if h.is_zero() {
            None
        } else {
            Some(self.value_kwh / h)
        }
    }

    /// Parse the `obis_code` string into a typed [`crate::obis::ObisCode`].
    ///
    /// Returns `None` when `obis_code` is absent or cannot be parsed.
    /// The raw `obis_code: Option<String>` is preserved for EDIFACT round-trips.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use metering::{MeterInterval, QualityFlag};
    /// use metering::obis::ObisCode;
    /// use rust_decimal::dec;
    /// use time::macros::datetime;
    ///
    /// let iv = MeterInterval {
    ///     from: datetime!(2026-01-01 0:00 UTC),
    ///     to:   datetime!(2026-01-01 0:15 UTC),
    ///     value_kwh: dec!(2.5),
    ///     quality: QualityFlag::Measured,
    ///     obis_code: Some("1-0:1.8.0*255".to_owned()),
    /// };
    /// let code = iv.parsed_obis_code().unwrap();
    /// assert_eq!(code, ObisCode::STROM_BEZUG_TOTAL);
    /// ```
    #[must_use]
    pub fn parsed_obis_code(&self) -> Option<crate::obis::ObisCode> {
        self.obis_code.as_deref()?.parse().ok()
    }

    /// `true` when this interval carries forward / import energy (D = 8 in the OBIS code).
    ///
    /// Returns `None` when no OBIS code is set or the code is unparseable.
    #[must_use]
    pub fn is_import_energy(&self) -> Option<bool> {
        self.parsed_obis_code().map(|c| c.is_import())
    }

    /// `true` when this interval carries reverse / export (Einspeisung) energy (D = 9).
    #[must_use]
    pub fn is_export_energy(&self) -> Option<bool> {
        self.parsed_obis_code().map(|c| c.is_export())
    }

    /// Tariff register number from the OBIS code (`None` = total, `Some(1)` = HT, `Some(2)` = NT).
    #[must_use]
    pub fn tariff_register(&self) -> Option<u8> {
        self.parsed_obis_code()?.tariff_register()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::datetime;

    #[test]
    fn demand_kw_15min_interval() {
        let iv = MeterInterval {
            from: datetime!(2026-01-01 0:00 UTC),
            to: datetime!(2026-01-01 0:15 UTC),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        };
        // 2.5 kWh in 15 min = 10 kW
        assert_eq!(iv.demand_kw(), Some(dec!(10)));
    }

    #[test]
    fn demand_kw_hourly() {
        let iv = MeterInterval {
            from: datetime!(2026-01-01 0:00 UTC),
            to: datetime!(2026-01-01 1:00 UTC),
            value_kwh: dec!(5.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        };
        // 5.0 kWh in 60 min = 5 kW
        assert_eq!(iv.demand_kw(), Some(dec!(5)));
    }

    #[test]
    fn quality_flag_billable() {
        // § 60 Abs. 2 MsbG: Measured, Estimated, Substituted, Calculated, Corrected, Preliminary → all billable
        assert!(QualityFlag::Measured.is_billable());
        assert!(QualityFlag::Substituted.is_billable());
        // CRITICAL FIX: Estimated (Prognosewert) IS billable per § 60 Abs. 2 MsbG — used in Abschlagsrechnung
        assert!(
            QualityFlag::Estimated.is_billable(),
            "Estimated must be billable per § 60 Abs. 2 MsbG"
        );
        assert!(QualityFlag::Corrected.is_billable());
        assert!(QualityFlag::Preliminary.is_billable());
        // Only Faulty and Unknown block billing
        assert!(!QualityFlag::Faulty.is_billable());
        assert!(!QualityFlag::Unknown.is_billable());
    }

    #[test]
    fn quality_flag_provisional() {
        assert!(QualityFlag::Estimated.is_provisional());
        assert!(QualityFlag::Preliminary.is_provisional());
        assert!(!QualityFlag::Measured.is_provisional());
        assert!(!QualityFlag::Substituted.is_provisional());
    }
}

#[cfg(test)]
mod media_tests {
    use super::*;

    /// Water is m³ and heat is kWh_th — the distinction the `unit` column exists
    /// to preserve.
    #[test]
    fn measured_unit_is_what_the_register_counts() {
        // Gas and water meters register a volume.
        assert_eq!(Sparte::Gas.measured_unit(), MeasurementUnit::CubicMetre);
        assert_eq!(Sparte::Wasser.measured_unit(), MeasurementUnit::CubicMetre);
        // A heat meter integrates flow × ΔT on-device, so its register is kWh_th.
        assert_eq!(
            Sparte::Waerme.measured_unit(),
            MeasurementUnit::KiloWattHour
        );
        assert_eq!(Sparte::Strom.measured_unit(), MeasurementUnit::KiloWattHour);
    }

    #[test]
    fn billing_unit_diverges_from_measured_unit_only_for_gas() {
        for sparte in [Sparte::Strom, Sparte::Gas, Sparte::Waerme, Sparte::Wasser] {
            assert_eq!(
                sparte.requires_conversion(),
                sparte == Sparte::Gas,
                "{} conversion requirement",
                sparte.as_str()
            );
            assert_eq!(
                sparte.measured_unit() != sparte.billing_unit(),
                sparte.requires_conversion(),
                "{}: requires_conversion must track the unit divergence",
                sparte.as_str()
            );
        }

        // Water is the one Sparte billed in a volume.
        assert_eq!(Sparte::Wasser.billing_unit(), MeasurementUnit::CubicMetre);
        assert_eq!(Sparte::Gas.billing_unit(), MeasurementUnit::KiloWattHour);
    }

    /// Both spellings of the cubic metre are in use on the wire.
    #[test]
    fn unit_parse_accepts_superscript_cubic_metre() {
        assert_eq!(
            MeasurementUnit::parse("m³"),
            Some(MeasurementUnit::CubicMetre)
        );
        assert_eq!(
            MeasurementUnit::parse("m3"),
            Some(MeasurementUnit::CubicMetre)
        );
        assert_eq!(
            MeasurementUnit::parse(" M3 "),
            Some(MeasurementUnit::CubicMetre)
        );
        assert_eq!(
            MeasurementUnit::parse("kWh_th"),
            Some(MeasurementUnit::KiloWattHour)
        );
        assert_eq!(MeasurementUnit::parse("furlong"), None);
    }

    /// `parse` rejects any unit it would have to rescale, so a caller cannot
    /// read MWh as kWh.
    #[test]
    fn strict_parse_rejects_units_needing_rescaling() {
        assert_eq!(MeasurementUnit::parse("MWh"), None);
        assert_eq!(MeasurementUnit::parse("GJ"), None);
        assert_eq!(MeasurementUnit::parse("l"), None);
        // ...but the scaled parser accepts them.
        assert!(MeasurementUnit::parse_scaled("MWh").is_some());
        assert!(MeasurementUnit::parse_scaled("GJ").is_some());
    }

    /// GJ→kWh is 2500/9, a repeating decimal. Holding it as a rational and
    /// multiplying before dividing keeps the conversion exact.
    #[test]
    fn gigajoule_conversion_is_exact() {
        let gj = MeasurementUnit::parse_scaled("GJ").unwrap();
        assert_eq!(gj.unit, MeasurementUnit::KiloWattHour);
        assert!(!gj.is_canonical());

        // 3.6 GJ is exactly 1000 kWh — the identity that defines the factor.
        let kwh = gj.apply(Decimal::from_str_exact("3.6").unwrap());
        assert_eq!(
            kwh,
            Decimal::from(1000u32),
            "3.6 GJ must be exactly 1000 kWh"
        );

        // 9 GJ is exactly 2500 kWh — no residue from the repeating factor.
        assert_eq!(gj.apply(Decimal::from(9u32)), Decimal::from(2500u32));
    }

    /// Rec 20 codes are not the unit symbols, so each mapping is pinned.
    #[test]
    fn unece_rec20_codes_do_not_follow_the_obvious_mnemonic() {
        let kwh = MeasurementUnit::KiloWattHour;

        // Gigajoule is `GV`.
        let gv = MeasurementUnit::parse_scaled("GV").unwrap();
        assert_eq!((gv.unit, gv.num, gv.den), (kwh, 2_500, 9));

        // Megajoule is `3B`.
        let mj = MeasurementUnit::parse_scaled("3B").unwrap();
        assert_eq!((mj.unit, mj.num, mj.den), (kwh, 5, 18));
        assert_eq!(mj, MeasurementUnit::parse_scaled("MJ").unwrap());

        // Cubic metre is `MTQ`.
        assert_eq!(
            MeasurementUnit::parse_scaled("MTQ").unwrap().unit,
            MeasurementUnit::CubicMetre
        );

        // Joule round-trips exactly: 3.6e6 J is 1 kWh.
        let jou = MeasurementUnit::parse_scaled("JOU").unwrap();
        assert_eq!(jou.apply(Decimal::from(3_600_000u32)), Decimal::ONE);
    }

    #[test]
    fn scaled_units_cover_the_real_device_population() {
        let cases = [
            ("MWh", "0.5", "500"), // ista ultego III smart displays 0,01 MWh
            ("Wh", "2500", "2.5"),
            ("MJ", "18", "5"),    // 18 MJ = 5 kWh exactly
            ("l", "1500", "1.5"), // water submeters commonly report litres
            ("kWh", "42", "42"),  // canonical passes through untouched
        ];
        for (unit, input, expected) in cases {
            let scale =
                MeasurementUnit::parse_scaled(unit).unwrap_or_else(|| panic!("{unit} must parse"));
            assert_eq!(
                scale.apply(Decimal::from_str_exact(input).unwrap()),
                Decimal::from_str_exact(expected).unwrap(),
                "{input} {unit}"
            );
        }
    }

    /// Labels are the DB CHECK values.
    #[test]
    fn labels_match_the_db_check_values() {
        assert_eq!(Sparte::Waerme.as_str(), "WAERME");
        assert_eq!(Sparte::Wasser.as_str(), "WASSER");
        assert_eq!(MeasurementUnit::KiloWattHour.as_str(), "KWH");
        assert_eq!(MeasurementUnit::CubicMetre.as_str(), "M3");
    }
}
