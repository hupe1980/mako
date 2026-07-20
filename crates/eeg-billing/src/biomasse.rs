//! Biomass-specific EEG settlement models — §§42–44 EEG 2023.
//!
//! Biomass plants have complex, fuel-type-dependent remuneration rules.
//! Key rules:
//! - §42 EEG 2023: biomass plants commissioned from 2023 are subject to
//!   substrate caps (max 40% Energiepflanzen vom Acker, §43 Abs. 1 Nr. 2).
//! - §44 EEG 2023: small manure-fed biogas plants (≤75 kW, ≥80% Gülle)
//!   receive a higher "Güllekleinanlage" bonus.
//! - §42a EEG 2023: Holzbiomasse (wood biomass) is restricted in its use.

use rust_decimal::Decimal;

// ── BiomassBrennstoff ─────────────────────────────────────────────────────────

/// Biomass fuel type — determines which §43/§44 EEG 2023 rules apply.
///
/// The fuel type affects:
/// - Whether the Güllekleinanlage bonus (§44 EEG 2023) is applicable.
/// - Whether Holzbiomasse restrictions apply (§42a EEG 2023).
/// - Whether substrate caps apply (§43 Abs. 1 Nr. 2: max 40% Energiepflanzen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum BiomassBrennstoff {
    /// Standard plant-based biomass (agricultural residues, dedicated crops).
    ///
    /// Subject to §43 Abs. 1 Nr. 2 substrate cap: max 40% Ackerpflanzen
    /// (energy crops from arable land).
    PflanzlicheBiomasse,

    /// Biomethane from biomass feedstocks — upgraded and fed into the gas grid.
    BiomethanAusBiomasse,

    /// Liquid manure (Gülle) and slurry — qualifies for Güllekleinanlage bonus.
    ///
    /// §44 EEG 2023 Güllekleinanlage rules:
    ///
    /// - Plant capacity ≤ 75 kW_el
    /// - ≥80% of energy input from liquid manure/slurry
    ///
    /// When both criteria are met, the Güllekleinanlage bonus rate applies.
    Guelle,

    /// Solid manure (Festmist) — also eligible for Güllekleinanlage if ≥80%.
    Festmist,

    /// Wood biomass — subject to §42a EEG 2023 restrictions.
    ///
    /// §42a prohibits new Holzbiomasse plants from using fresh wood for primary
    /// energy production from 2026. Only residual/recycled wood is permitted.
    Holzbiomasse,

    /// Sewage gas (Klärgas) from wastewater treatment.
    Klaegas,

    /// Landfill gas (Deponiegas).
    Deponiegas,

    /// Mine gas (Grubengas) from coal mines.
    Grubengas,

    /// Biogenic waste fractions (not covered by §43 substrate caps).
    BiogenicWaste,
}

impl BiomassBrennstoff {
    /// Returns `true` when this fuel type is eligible for the §44 Güllekleinanlage bonus.
    ///
    /// The bonus requires BOTH:
    /// - Plant capacity ≤ 75 kW_el (checked separately in [`BiomassSettlementData`])
    /// - ≥ 80% energy from Gülle/Festmist (this method confirms fuel type)
    #[must_use]
    pub fn guellebonusanlage_eligible(self) -> bool {
        matches!(self, Self::Guelle | Self::Festmist)
    }

    /// Returns `true` when §42a EEG 2023 Holzbiomasse restrictions apply.
    #[must_use]
    pub fn has_holzbiomasse_restriction(self) -> bool {
        self == Self::Holzbiomasse
    }
}

// ── BiomassSettlementData ─────────────────────────────────────────────────────

/// Biomass-specific data required for correct §42–§44 EEG 2023 settlement.
///
/// Add this to `SettleInput` via the `biomasse` field when settling biomass
/// or biogas plants.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BiomassSettlementData {
    /// Primary fuel type fed into this plant.
    pub hauptbrennstoff: BiomassBrennstoff,

    /// Fraction of energy input from liquid/solid manure (0.0–1.0).
    ///
    /// Used to determine §44 Güllekleinanlage eligibility:
    /// - ≥ 0.80 (80% manure) + plant ≤ 75 kW → eligible for bonus
    pub guelle_anteil: Decimal,

    /// Whether the plant qualifies as a §44 Güllekleinanlage (≤75 kW + ≥80% Gülle).
    ///
    /// Set by the billing system based on `guelle_anteil >= 0.80` AND
    /// `leistung_kwp <= 75`. When `true`, use the Güllekleinanlage tariff rate.
    pub ist_guellebonusanlage: bool,

    /// Fraction of energy input from Energiepflanzen vom Acker (0.0–1.0).
    ///
    /// §43 Abs. 1 Nr. 2 EEG 2023 substrate cap: must be ≤ 0.40 (40%).
    /// Exceeding this cap can result in loss of EEG support for the excess.
    pub energiepflanzen_anteil: Decimal,

    /// Whether the §43 substrate cap is met (`energiepflanzen_anteil <= 0.40`).
    pub substrate_cap_ok: bool,
}

impl BiomassSettlementData {
    /// Construct from fuel type and composition data.
    ///
    /// Automatically computes `ist_guellebonusanlage` and `substrate_cap_ok`.
    #[must_use]
    pub fn new(
        hauptbrennstoff: BiomassBrennstoff,
        guelle_anteil: Decimal,
        energiepflanzen_anteil: Decimal,
        leistung_kw: Decimal,
    ) -> Self {
        use rust_decimal::dec;
        let ist_guellebonusanlage = hauptbrennstoff.guellebonusanlage_eligible()
            && guelle_anteil >= dec!(0.80)
            && leistung_kw <= dec!(75);
        let substrate_cap_ok = energiepflanzen_anteil <= dec!(0.40);
        Self {
            hauptbrennstoff,
            guelle_anteil,
            ist_guellebonusanlage,
            energiepflanzen_anteil,
            substrate_cap_ok,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn guellebonusanlage_qualifies_when_criteria_met() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::Guelle,
            dec!(0.85), // 85% Gülle — above 80% threshold
            dec!(0.05), // minimal energy crops
            dec!(50),   // 50 kW — below 75 kW limit
        );
        assert!(data.ist_guellebonusanlage);
        assert!(data.substrate_cap_ok);
    }

    #[test]
    fn guellebonusanlage_disqualified_by_capacity() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::Guelle,
            dec!(0.90), // 90% Gülle — above threshold
            dec!(0.05),
            dec!(100), // 100 kW — ABOVE 75 kW limit
        );
        assert!(!data.ist_guellebonusanlage, "capacity > 75 kW → no bonus");
    }

    #[test]
    fn substrate_cap_exceeded() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::PflanzlicheBiomasse,
            dec!(0.0),
            dec!(0.50), // 50% energy crops — EXCEEDS 40% cap
            dec!(100),
        );
        assert!(!data.substrate_cap_ok);
    }
}
