//! Gas quality types and normalization for the GeLi Gas / WiM Gas process family.
//!
//! ## Regulatory basis
//!
//! - **DVGW G 260** (2021): Technical rules for gas supply — defines H-Gas and L-Gas
//!   based on Wobbe index ranges. German grid operators publish the applicable quality
//!   type per gas network area.
//! - **DVGW G 685** (2008/2018): Gas billing — conversion from volume (m³) to energy
//!   (kWh_Hs). Requires Abrechnungsbrennwert × Zustandszahl.
//! - **BNetzA MaStR**: Gas network areas (Gasnetze) are typed as H-Gas or L-Gas.
//! - **BNetzA H2 pilots (2025–2028)**: GET H2 (H2-Wärmenetz Hamburg), GASCADE H2
//!   backbone. AHB extensions for H2-blend EDIFACT parameters expected 2026–2028.
//!   This module is designed to accept H2-blend values as soon as the AHBs are
//!   published without requiring a breaking schema change.
//!
//! ## Canonical string representation
//!
//! All `gasqualitaet` strings stored in `marktd.malo.gasqualitaet` and carried
//! in EDIFACT outbox payloads use the **BO4E / BNetzA MaStR canonical form**:
//!
//! | Canonical | Legacy aliases | DVGW designation |
//! |---|---|---|
//! | `H_GAS` | `HGas`, `H-Gas`, `HIGH_CALORIFIC` | Wobbe index 12.4–15.7 kWh/m³ |
//! | `L_GAS` | `LGas`, `L-Gas`, `LOW_CALORIFIC` | Wobbe index 10.5–13.0 kWh/m³ |
//! | `H2_BLEND` | `H2Blend`, `HYDROGEN_BLEND` | Future: H2 admixture ≤ 20 vol% |
//! | `BIOGAS` | `BioGas`, `BIOMETHANE` | Biomethane injection |
//! | `FLUESSIGGAS` | `LPG` | Liquified petroleum gas |
//!
//! ## Forward compatibility
//!
//! When DVGW G 260 and BNetzA publish H2-blend EDIFACT codes (expected 2026–2028),
//! the `GasQualitaet::H2Blend` variant is already present in this module with the
//! correct canonical string. Upstream adapters only need to add the new UTILMD G
//! segment code → `GasQualitaet::H2Blend` mapping.

/// Canonical gas quality classification for GeLi Gas / WiM Gas workflows.
///
/// ## Design rationale — why not use `rubo4e::Gasqualitaet` directly?
///
/// The BO4E v202607 `Gasqualitaet` enum only has `H_GAS`, `L_GAS`, and `Unknown`.
/// It does **not** yet include H2-blend variants, which are expected in the 2026–2028
/// DVGW/BNetzA AHB wave. This crate-local enum:
/// 1. Adds `H2Blend` (with sub-type) for H2-ready data models.
/// 2. Provides `from_raw` normalization from all real-world alias strings.
/// 3. Converts to/from the BO4E `Gasqualitaet` canonical string for storage.
///
/// When BO4E adds `H2_BLEND` to the standard enum, this type stays binary-compatible —
/// `as_canonical_str()` already produces `"H2_BLEND"` for the `H2Blend` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum GasQualitaet {
    /// Hochkalorisches Erdgas (H-Gas) per DVGW G 260 §3.2.
    ///
    /// Wobbe index: 12.4–15.7 kWh/m³ (dry, 0°C, 1.01325 bar).
    /// Most of the German gas grid operates on H-Gas.
    /// Canonical string: `"H_GAS"`.
    #[serde(rename = "H_GAS")]
    HGas,
    /// Niederkalorisches Erdgas (L-Gas) per DVGW G 260 §3.2.
    ///
    /// Wobbe index: 10.5–13.0 kWh/m³ (dry, 0°C, 1.01325 bar).
    /// Used in some regions of northern and north-western Germany.
    /// L-Gas networks are being converted to H-Gas by 2030 (BNetzA G-Umstellung).
    /// Canonical string: `"L_GAS"`.
    #[serde(rename = "L_GAS")]
    LGas,
    /// Hydrogen admixture in natural gas grid (H2-blend).
    ///
    /// DVGW G 260 (2021 draft) allows up to 20 vol% H2 in H-Gas grids.
    /// BNetzA H2 pilots: GET H2 Hamburg, GASCADE H2 backbone (2025–2028).
    ///
    /// ## EDIFACT note
    ///
    /// The BDEW/DVGW AHB EDIFACT qualifier for H2-blend in UTILMD G and MSCONS
    /// messages is expected to be standardized in the 2026–2028 AHB wave.
    /// This variant is pre-registered so adapters only need to add the
    /// segment-code → `GasQualitaet::H2Blend` mapping when the AHB is published.
    ///
    /// ## Billing note
    ///
    /// H2-blend Abrechnungsbrennwert is LOWER than pure H-Gas because H2 has
    /// lower calorific value (Hs ≈ 3.0 kWh/m³) than CH4 (Hs ≈ 10.5 kWh/m³).
    /// The DSO publishes the blended Brennwert via MSCONS PID 13007.
    /// Canonical string: `"H2_BLEND"`.
    #[serde(rename = "H2_BLEND")]
    H2Blend,
    /// Biomethane (upgraded biogas injected into gas grid).
    ///
    /// Per EEG 2023 §42 and BiogasRL: biomethane must meet DVGW G 260 H-Gas spec
    /// after upgrading. Usually tracked as H-Gas in billing; BIOGAS annotation
    /// enables GO (Guarantee of Origin) traceability.
    /// Canonical string: `"BIOGAS"`.
    #[serde(rename = "BIOGAS")]
    Biogas,
    /// Liquified petroleum gas (LPG / Flüssiggas).
    ///
    /// Not distributed via long-distance grid; used in off-grid areas.
    /// Canonical string: `"FLUESSIGGAS"`.
    #[serde(rename = "FLUESSIGGAS")]
    Fluessiggas,
    /// Unknown or future gas quality variant.
    ///
    /// Returned when `from_raw` receives a string not recognized by this version.
    /// Stored as-is (uppercased) to preserve forward compatibility with future AHBs.
    #[serde(other)]
    Unknown,
}

impl GasQualitaet {
    /// Parse a raw `gasqualitaet` string to the canonical enum variant.
    ///
    /// Accepts all real-world alias strings used by different systems:
    /// - `marktd` stored values (`"HGas"`, `"LGas"`)
    /// - DVGW notation (`"H-Gas"`, `"L-Gas"`)
    /// - BO4E form (`"H_GAS"`, `"L_GAS"`, `"H2_BLEND"`)
    /// - BNetzA MaStR form (`"H_GAS"`, `"L_GAS"`)
    /// - Case-insensitive and whitespace-trimmed
    ///
    /// Returns `GasQualitaet::Unknown` for unrecognized values.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        let norm = raw.trim().to_uppercase().replace(['-', ' '], "_");
        match norm.as_str() {
            "HGAS" | "H_GAS" | "HIGH_CALORIFIC" | "HOCHKALORISCH" | "ERDGAS_H" => Self::HGas,
            "LGAS" | "L_GAS" | "LOW_CALORIFIC" | "NIEDERKALORISCH" | "ERDGAS_L" => Self::LGas,
            "H2_BLEND"
            | "H2BLEND"
            | "HYDROGEN_BLEND"
            | "HYDROGEN_GAS"
            | "H2_GAS"
            | "H2_GEMISCH"
            | "WASSERSTOFF_BEIMISCHUNG" => Self::H2Blend,
            "BIOGAS" | "BIO_GAS" | "BIOMETHANE" | "BIOMETHAN" | "BIOERDGAS" => Self::Biogas,
            "FLUESSIGGAS" | "FLUSSIGGAS" | "LPG" | "LIQUID_GAS" | "PROPANGAS" => Self::Fluessiggas,
            _ => Self::Unknown,
        }
    }

    /// The canonical string representation (BO4E / BNetzA MaStR form).
    ///
    /// This is the value written to `marktd.malo.gasqualitaet`, EDIFACT outbox
    /// payloads, and `ZusatzAttribut` on billing invoices.
    #[must_use]
    pub fn as_canonical_str(&self) -> &'static str {
        match self {
            Self::HGas => "H_GAS",
            Self::LGas => "L_GAS",
            Self::H2Blend => "H2_BLEND",
            Self::Biogas => "BIOGAS",
            Self::Fluessiggas => "FLUESSIGGAS",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// `true` when this gas quality requires H2-specific billing handling.
    ///
    /// H2-blend Brennwert is lower than pure natural gas — the DSO-published
    /// Abrechnungsbrennwert (MSCONS PID 13007) already accounts for the blend,
    /// but invoice annotations and regulatory reporting differ.
    #[must_use]
    pub fn is_h2_blend(&self) -> bool {
        matches!(self, Self::H2Blend)
    }

    /// `true` when this gas quality is one of the standard pipeline natural gas
    /// types (H-Gas or L-Gas) per DVGW G 260.
    #[must_use]
    pub fn is_natural_gas(&self) -> bool {
        matches!(self, Self::HGas | Self::LGas)
    }

    /// DVGW G 260 reference Wobbe index range in kWh/m³ (dry, 0°C, 1.01325 bar).
    ///
    /// Returns `None` for variants without a standardized Wobbe range.
    /// Used for validation of Abrechnungsbrennwert values against known plausibility limits.
    #[must_use]
    pub fn wobbe_index_range_kwh_per_m3(&self) -> Option<(f64, f64)> {
        match self {
            // DVGW G 260 §3.2 Table 1
            Self::HGas => Some((12.4, 15.7)),
            Self::LGas => Some((10.5, 13.0)),
            // H2-blend: depends on blend ratio. 20 vol% H2 in H-Gas lowers Wobbe by ~3%.
            // No fixed range until DVGW finalizes the H2-blend standard.
            Self::H2Blend => Some((12.0, 15.7)),
            _ => None,
        }
    }
}

impl std::fmt::Display for GasQualitaet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_canonical_str())
    }
}

/// Normalize a raw `gasqualitaet` string to the canonical BO4E / BNetzA MaStR form.
///
/// This is a free-function wrapper around [`GasQualitaet::from_raw`] for use in
/// contexts where the full enum type is not needed — e.g. when normalizing strings
/// for database storage or EDIFACT outbox payloads.
///
/// Returns `"UNKNOWN"` for unrecognized inputs.
///
/// ## Examples
///
/// ```rust
/// use mako_geli_gas::gas_quality::normalize_gasqualitaet;
///
/// assert_eq!(normalize_gasqualitaet("HGas"), "H_GAS");
/// assert_eq!(normalize_gasqualitaet("L-Gas"), "L_GAS");
/// assert_eq!(normalize_gasqualitaet("H2Blend"), "H2_BLEND");
/// assert_eq!(normalize_gasqualitaet("  H_GAS  "), "H_GAS");  // idempotent
/// ```
#[must_use]
pub fn normalize_gasqualitaet(raw: &str) -> &'static str {
    GasQualitaet::from_raw(raw).as_canonical_str()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hgas_aliases_normalize_to_h_gas() {
        for raw in &[
            "HGas",
            "H-Gas",
            "H-gas",
            "HGAS",
            "H_GAS",
            "HIGH_CALORIFIC",
            "ERDGAS_H",
            "hochkalorisch",
        ] {
            assert_eq!(
                GasQualitaet::from_raw(raw),
                GasQualitaet::HGas,
                "expected H_GAS for {raw:?}"
            );
            assert_eq!(
                normalize_gasqualitaet(raw),
                "H_GAS",
                "canonical string wrong for {raw:?}"
            );
        }
    }

    #[test]
    fn lgas_aliases_normalize_to_l_gas() {
        for raw in &[
            "LGas",
            "L-Gas",
            "L_GAS",
            "LGAS",
            "LOW_CALORIFIC",
            "ERDGAS_L",
        ] {
            assert_eq!(
                GasQualitaet::from_raw(raw),
                GasQualitaet::LGas,
                "expected L_GAS for {raw:?}"
            );
        }
    }

    #[test]
    fn h2_blend_aliases_normalize() {
        for raw in &[
            "H2_BLEND",
            "H2Blend",
            "H2-Blend",
            "HYDROGEN_BLEND",
            "H2BLEND",
            "H2_GAS",
            "H2_GEMISCH",
            "WASSERSTOFF_BEIMISCHUNG",
        ] {
            assert_eq!(
                GasQualitaet::from_raw(raw),
                GasQualitaet::H2Blend,
                "expected H2_BLEND for {raw:?}"
            );
            assert_eq!(normalize_gasqualitaet(raw), "H2_BLEND");
        }
    }

    #[test]
    fn biogas_aliases_normalize() {
        for raw in &[
            "BIOGAS",
            "BioGas",
            "Bio-Gas",
            "BIOMETHANE",
            "BIOMETHAN",
            "BIOERDGAS",
        ] {
            assert_eq!(GasQualitaet::from_raw(raw), GasQualitaet::Biogas);
        }
    }

    #[test]
    fn fluessiggas_aliases_normalize() {
        for raw in &["FLUESSIGGAS", "LPG", "LIQUID_GAS", "PROPANGAS"] {
            assert_eq!(GasQualitaet::from_raw(raw), GasQualitaet::Fluessiggas);
        }
    }

    #[test]
    fn unknown_input_returns_unknown() {
        assert_eq!(GasQualitaet::from_raw("SYNGAS"), GasQualitaet::Unknown);
        assert_eq!(normalize_gasqualitaet("SYNGAS"), "UNKNOWN");
    }

    #[test]
    fn canonical_form_is_idempotent() {
        for (gq, canonical) in &[
            (GasQualitaet::HGas, "H_GAS"),
            (GasQualitaet::LGas, "L_GAS"),
            (GasQualitaet::H2Blend, "H2_BLEND"),
            (GasQualitaet::Biogas, "BIOGAS"),
            (GasQualitaet::Fluessiggas, "FLUESSIGGAS"),
        ] {
            assert_eq!(gq.as_canonical_str(), *canonical);
            // Normalizing the canonical form produces the same canonical form
            assert_eq!(normalize_gasqualitaet(canonical), *canonical);
        }
    }

    #[test]
    fn trims_whitespace_before_normalization() {
        assert_eq!(GasQualitaet::from_raw("  HGas  "), GasQualitaet::HGas);
        assert_eq!(GasQualitaet::from_raw("\tLGas\n"), GasQualitaet::LGas);
    }

    #[test]
    fn h2_blend_detection() {
        assert!(GasQualitaet::H2Blend.is_h2_blend());
        assert!(!GasQualitaet::HGas.is_h2_blend());
        assert!(!GasQualitaet::LGas.is_h2_blend());
    }

    #[test]
    fn natural_gas_detection() {
        assert!(GasQualitaet::HGas.is_natural_gas());
        assert!(GasQualitaet::LGas.is_natural_gas());
        assert!(!GasQualitaet::H2Blend.is_natural_gas());
        assert!(!GasQualitaet::Biogas.is_natural_gas());
    }

    #[test]
    fn wobbe_index_ranges_are_plausible() {
        let (lo, hi) = GasQualitaet::HGas.wobbe_index_range_kwh_per_m3().unwrap();
        assert!(lo < hi && lo > 10.0 && hi < 20.0);
        let (lo, hi) = GasQualitaet::LGas.wobbe_index_range_kwh_per_m3().unwrap();
        assert!(lo < hi && lo > 8.0 && hi < 15.0);
        // Non-grid gas has no Wobbe range
        assert!(
            GasQualitaet::Fluessiggas
                .wobbe_index_range_kwh_per_m3()
                .is_none()
        );
    }

    #[test]
    fn display_produces_canonical_string() {
        assert_eq!(GasQualitaet::HGas.to_string(), "H_GAS");
        assert_eq!(GasQualitaet::H2Blend.to_string(), "H2_BLEND");
    }

    #[test]
    fn serde_roundtrip_canonical_values() {
        for (raw, expected) in &[("H_GAS", GasQualitaet::HGas), ("L_GAS", GasQualitaet::LGas)] {
            let json = format!("\"{raw}\"");
            let deserialized: GasQualitaet = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, *expected);
            let serialized = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(serialized, json);
        }
    }
}
