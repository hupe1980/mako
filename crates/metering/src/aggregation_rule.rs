//! Virtual meter rules and aggregation specifications.
//!
//! ## Use cases
//!
//! | Rule type | German term | Application |
//! |---|---|---|
//! | `Sum` | Summenmessung | Aggregation of parallel MaLos (e.g. grid intake + local PV) |
//! | `Residual` | Residuallast | Grid withdrawal = total load − own generation (§42b EEG) |
//! | `PvSelfConsumption` | PV-Eigenverbrauch | Consumed from own PV plant before grid feed-in |
//! | `GgvAllocation` | GGV Nutzungsplan | §42b EEG community solar allocation to tenants |
//!
//! ## Relationship to other modules
//!
//! - Virtual meter rules are configured and stored in `marktd`
//! - `edmd` evaluates rules when computing aggregated Lastgang for a virtual MaLo
//! - The `metering::aggregation` module performs the actual kWh arithmetic
//!
//! ## Regulatory basis
//!
//! - **§42b EEG 2023 (Solarpaket I)** — Gemeinschaftliche Gebäudeversorgung (GGV)
//! - **§42a EEG** — residual load calculation for feed-in metering
//! - **BK6-22-024 (MaBiS)** — portfolio aggregation for BKV settlement

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// An aggregation rule defining how a virtual MaLo's time series is computed.
///
/// Virtual meters aggregate two or more physical MaLo time series. The result
/// is stored as a derived `meter_reads` entry with `source = "VIRTUAL"`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum AggregationRule {
    /// Sum of multiple MaLo time series — used for portfolio totals and
    /// Summenmessung (multiple parallel transformers, shared substations).
    ///
    /// `result[t] = Σ source_malo_ids[i][t]`
    Sum {
        /// MaLo IDs whose interval values are summed.
        source_malo_ids: Vec<String>,
    },

    /// Residual load: total minus one or more subtracted sources.
    ///
    /// `result[t] = total_malo_id[t] - Σ subtract_malo_ids[i][t]`
    ///
    /// Common application: grid feed-in metering at a building with local PV.
    /// The net grid intake = building load − PV generation.
    ///
    /// ## §42a EEG (residual load metering for feed-in)
    ///
    /// For feed-in compensation contracts: net feed-in = gross generation − own consumption.
    Residual {
        /// MaLo whose value is the minuend (total).
        total_malo_id: String,
        /// MaLo IDs whose values are subtracted.
        subtract_malo_ids: Vec<String>,
    },

    /// PV self-consumption allocation for a prosumer.
    ///
    /// `self_consumption[t] = min(generation[t], load[t])`
    /// `grid_feed_in[t] = max(0, generation[t] - load[t])`
    /// `grid_draw[t] = max(0, load[t] - generation[t])`
    ///
    /// The virtual MaLo represents the **net grid connection point**.
    PvSelfConsumption {
        /// MaLo for the total building load (from grid measurement point).
        grid_malo_id: String,
        /// MaLo for the PV generation (bidirectional meter or generation meter).
        generation_malo_id: String,
    },

    /// §42b EEG Gemeinschaftliche Gebäudeversorgung (GGV) — proportional allocation
    /// of shared PV generation to tenant delivery points.
    ///
    /// Each tenant receives `fraction * total_generation[t]` kWh for each interval.
    /// Fractions must sum to ≤ 1.0 (remainder is grid feed-in).
    ///
    /// ## §42b EEG 2023 (Solarpaket I)
    ///
    /// Tenant fractions are defined in the Nutzungsplan, updated annually.
    GgvAllocation {
        /// MaLo of the shared PV plant (generation measurement point).
        plant_malo_id: String,
        /// Allocated fraction for each tenant MaLo (0.0–1.0).
        tenant_fractions: Vec<(String, rust_decimal::Decimal)>,
    },
}

impl AggregationRule {
    /// All source MaLo IDs referenced by this rule.
    ///
    /// Used to enumerate which underlying MaLos must be queried from `edmd`
    /// before computing the virtual meter value.
    #[must_use]
    pub fn source_malo_ids(&self) -> Vec<&str> {
        match self {
            Self::Sum { source_malo_ids } => source_malo_ids.iter().map(String::as_str).collect(),
            Self::Residual {
                total_malo_id,
                subtract_malo_ids,
            } => {
                let mut ids = vec![total_malo_id.as_str()];
                ids.extend(subtract_malo_ids.iter().map(String::as_str));
                ids
            }
            Self::PvSelfConsumption {
                grid_malo_id,
                generation_malo_id,
            } => vec![grid_malo_id.as_str(), generation_malo_id.as_str()],
            Self::GgvAllocation {
                plant_malo_id,
                tenant_fractions,
            } => {
                let mut ids = vec![plant_malo_id.as_str()];
                ids.extend(tenant_fractions.iter().map(|(m, _)| m.as_str()));
                ids
            }
        }
    }

    /// Human-readable rule type name.
    #[must_use]
    pub fn rule_type(&self) -> &'static str {
        match self {
            Self::Sum { .. } => "Sum",
            Self::Residual { .. } => "Residual",
            Self::PvSelfConsumption { .. } => "PvSelfConsumption",
            Self::GgvAllocation { .. } => "GgvAllocation",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn sum_rule_lists_all_sources() {
        let rule = AggregationRule::Sum {
            source_malo_ids: vec!["MALO_A".to_owned(), "MALO_B".to_owned()],
        };
        let sources = rule.source_malo_ids();
        assert_eq!(sources, vec!["MALO_A", "MALO_B"]);
        assert_eq!(rule.rule_type(), "Sum");
    }

    #[test]
    fn residual_rule_includes_total_and_subtracts() {
        let rule = AggregationRule::Residual {
            total_malo_id: "TOTAL".to_owned(),
            subtract_malo_ids: vec!["PV".to_owned()],
        };
        let sources = rule.source_malo_ids();
        assert!(sources.contains(&"TOTAL"));
        assert!(sources.contains(&"PV"));
        assert_eq!(rule.rule_type(), "Residual");
    }

    #[test]
    fn ggv_lists_plant_and_tenants() {
        let rule = AggregationRule::GgvAllocation {
            plant_malo_id: "PLANT".to_owned(),
            tenant_fractions: vec![
                ("TENANT_A".to_owned(), dec!(0.4)),
                ("TENANT_B".to_owned(), dec!(0.3)),
            ],
        };
        let sources = rule.source_malo_ids();
        assert_eq!(sources.len(), 3);
        assert!(sources.contains(&"PLANT"));
        assert!(sources.contains(&"TENANT_A"));
    }
}
