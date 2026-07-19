//! GaBi Gas portfolio balancing domain types.
//!
//! A **Bilanzkreis portfolio** is the aggregated view of all nominations,
//! allocations, and imbalances for a BKV across one or more gas days.
//! This module provides the domain types for:
//!
//! - [`GasMarketRole`] — typed market role classification per BDEW Rollenmodell
//! - [`GasPortfolioBalance`] — aggregated BKV portfolio position for a gas day
//! - [`PortfolioPosition`] — individual balance group position (nominated vs. allocated)
//!
//! ## Regulatory basis
//!
//! - **GasNZV §24**: Balance group accounting obligations for BKVs
//! - **Kooperationsvereinbarung Gas (KoV) §3**: Nomination requirements
//! - **KoV §6**: Allocation and imbalance settlement
//! - **BNetzA BK7-14-020**: GaBi Gas 2.0 ruling

use rust_decimal::Decimal;

use crate::domain::{GasDay, GasImbalanceSaldo, ImbalanceDirection};

// ── GasMarketRole ─────────────────────────────────────────────────────────────

/// Market role in the German gas market per BDEW Rollenmodell V2.2.
///
/// Used to classify which entity is responsible for a message, process,
/// or domain object. Enables correct responsibility separation per
/// §9 EnWG Informatorisches Unbundling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GasMarketRole {
    /// Lieferant (LF) — gas supplier to end customers.
    ///
    /// Responsibilities: customer supply, nominations, portfolio balancing.
    Lf,

    /// Netzbetreiber (NB) — gas grid operator (distribution).
    ///
    /// Responsibilities: allocation, measurement data, balancing processes.
    Nb,

    /// Fernleitungsnetzbetreiber (FNB) — gas transmission system operator.
    ///
    /// Responsibilities: transport scheduling, daily allocations (ALOCAT 90001).
    Fnb,

    /// Verteilnetzbetreiber (VNB) — gas distribution network operator.
    ///
    /// Responsibilities: sub-daily allocations (ALOCAT 90003), SLP allocation.
    Vnb,

    /// Bilanzkreisverantwortlicher (BKV) — balance group manager.
    ///
    /// Responsibilities: balancing group management, nominations (NOMINT),
    /// receiving allocations (ALOCAT), managing deviations.
    Bkv,

    /// Marktgebietsverantwortlicher (MGV) — market area responsible party.
    ///
    /// Responsibilities: monthly allocations (ALOCAT 90002), imbalance settlement
    /// (IMBNOT), Ausgleichsenergie pricing.
    Mgv,

    /// Messstellenbetreiber (MSB) — metering point operator.
    ///
    /// Responsibilities: meter operation, measurement data delivery (MSCONS),
    /// RLM gas profiles.
    Msb,

    /// Händler / Trader — gas trading party.
    ///
    /// Responsibilities: portfolio management, intraday trading,
    /// re-nomination within correction windows.
    Haendler,

    /// Transportnetzbetreiber — generic transport grid operator.
    Transportnetzbetreiber,
}

impl GasMarketRole {
    /// BDEW abbreviation for this role.
    #[must_use]
    pub fn abbreviation(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Nb => "NB",
            Self::Fnb => "FNB",
            Self::Vnb => "VNB",
            Self::Bkv => "BKV",
            Self::Mgv => "MGV",
            Self::Msb => "MSB",
            Self::Haendler => "GH",
            Self::Transportnetzbetreiber => "TNB",
        }
    }

    /// `true` when this role submits nominations (NOMINT).
    #[must_use]
    pub fn submits_nominations(self) -> bool {
        matches!(self, Self::Bkv | Self::Haendler)
    }

    /// `true` when this role receives allocation messages (ALOCAT).
    #[must_use]
    pub fn receives_allocations(self) -> bool {
        matches!(self, Self::Bkv | Self::Fnb)
    }

    /// `true` when this role is subject to imbalance settlement (IMBNOT).
    #[must_use]
    pub fn has_imbalance_obligation(self) -> bool {
        matches!(self, Self::Bkv)
    }
}

// ── PortfolioPosition ─────────────────────────────────────────────────────────

/// The energy position for a single Bilanzkreis on a gas day.
///
/// Captures the nominated and allocated quantities so the BKV can compute
/// their imbalance before the final settlement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortfolioPosition {
    /// EIC code of the Bilanzkreis.
    pub bilanzkreis_eic: String,

    /// Gas day for this position.
    pub gas_day: GasDay,

    /// Total nominated quantity for this gas day (from the most recent NOMINT).
    ///
    /// `None` when no nomination has been submitted yet.
    pub nominated_kwh: Option<Decimal>,

    /// Total allocated quantity received via ALOCAT.
    ///
    /// `None` when no allocation has been received yet (initial allocation pending).
    pub allocated_kwh: Option<Decimal>,

    /// Imbalance (nominated − allocated), computed when both are available.
    ///
    /// Positive = Mehr-Energie (BKV over-nominated).
    /// Negative = Minder-Energie (BKV under-nominated).
    pub imbalance_kwh: Option<Decimal>,

    /// `true` when the final allocation has been received (no further corrections).
    pub is_final: bool,
}

impl PortfolioPosition {
    /// Compute the imbalance for this position.
    ///
    /// Returns `None` when either nominated or allocated quantity is unknown.
    #[must_use]
    pub fn compute_imbalance_saldo(&self, bkv_eic: &str) -> Option<GasImbalanceSaldo> {
        let nominated = self.nominated_kwh?;
        let allocated = self.allocated_kwh?;
        Some(GasImbalanceSaldo::calculate(
            self.gas_day,
            bkv_eic,
            &self.bilanzkreis_eic,
            nominated,
            allocated,
        ))
    }

    /// `true` when the nomination exactly matches the allocation (balanced position).
    ///
    /// Uses 1 kWh tolerance per GasNZV §24.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        match (self.nominated_kwh, self.allocated_kwh) {
            (Some(n), Some(a)) => (n - a).abs() <= Decimal::ONE,
            _ => false,
        }
    }
}

// ── GasPortfolioBalance ───────────────────────────────────────────────────────

/// Aggregated gas portfolio balance for a BKV across all Bilanzkreise on a gas day.
///
/// The portfolio balance summarises the BKV's total position:
/// - Total nominated energy (sum across all Bilanzkreise)
/// - Total allocated energy
/// - Net imbalance exposure
/// - Per-Bilanzkreis breakdown
///
/// This is the primary input for:
/// - Intraday re-nomination decisions (if nominated ≠ forecast)
/// - End-of-day imbalance settlement via the MGV
/// - BNetzA regulatory reporting
///
/// ## Regulatory basis
///
/// - **GasNZV §24**: BKV must balance nominations with actual offtake.
/// - **KoV §6.3**: Imbalance settlement deadlines and Ausgleichsenergie pricing.
/// - **BNetzA BK7-14-020 §7**: GaBi Gas 2.0 imbalance reporting format.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GasPortfolioBalance {
    /// EIC code of the BKV.
    pub bkv_eic: String,

    /// Gas day for this portfolio snapshot.
    pub gas_day: GasDay,

    /// Per-Bilanzkreis positions.
    pub positions: Vec<PortfolioPosition>,

    /// Timestamp when this snapshot was computed (UTC).
    pub computed_at: time::OffsetDateTime,
}

impl GasPortfolioBalance {
    /// Total nominated energy across all Bilanzkreise (kWh_Hs).
    #[must_use]
    pub fn total_nominated_kwh(&self) -> Decimal {
        self.positions.iter().filter_map(|p| p.nominated_kwh).sum()
    }

    /// Total allocated energy across all Bilanzkreise (kWh_Hs).
    #[must_use]
    pub fn total_allocated_kwh(&self) -> Decimal {
        self.positions.iter().filter_map(|p| p.allocated_kwh).sum()
    }

    /// Net portfolio imbalance (nominated − allocated) in kWh_Hs.
    ///
    /// Positive = portfolio over-nominated (net Mehr-Energie).
    /// Negative = portfolio under-nominated (net Minder-Energie).
    #[must_use]
    pub fn net_imbalance_kwh(&self) -> Decimal {
        self.total_nominated_kwh() - self.total_allocated_kwh()
    }

    /// Net imbalance direction for the whole portfolio.
    #[must_use]
    pub fn portfolio_direction(&self) -> ImbalanceDirection {
        let net = self.net_imbalance_kwh();
        match net.cmp(&Decimal::ZERO) {
            std::cmp::Ordering::Greater => ImbalanceDirection::Mehr,
            std::cmp::Ordering::Less => ImbalanceDirection::Minder,
            std::cmp::Ordering::Equal => ImbalanceDirection::Balanced,
        }
    }

    /// `true` when all Bilanzkreise have received their final allocations.
    #[must_use]
    pub fn is_fully_settled(&self) -> bool {
        !self.positions.is_empty() && self.positions.iter().all(|p| p.is_final)
    }

    /// Number of Bilanzkreise with open imbalance (|imbalance| > 1 kWh).
    #[must_use]
    pub fn open_imbalance_count(&self) -> usize {
        self.positions.iter().filter(|p| !p.is_balanced()).count()
    }

    /// Check energy conservation for this portfolio snapshot.
    ///
    /// Per KoV and DVGW G 685 / GasNZV §24, the total allocated quantity
    /// across all BKVs in a VNB exit zone must equal the VNB's total measured
    /// offtake. This method verifies that the BKV's own portfolio sums are
    /// internally consistent.
    ///
    /// ## Return value
    ///
    /// Returns `Ok(total_allocated_kwh)` when all positions have allocations
    /// and the total matches `expected_total_kwh` within `tolerance_kwh`.
    ///
    /// Returns `Err(ConservationViolation)` when:
    /// - Any position lacks an allocation (incomplete data)
    /// - The total deviates from `expected_total_kwh` beyond `tolerance_kwh`
    ///
    /// ## Usage
    ///
    /// ```rust,no_run
    /// # use mako_gabi_gas::portfolio::*;
    /// # use mako_gabi_gas::domain::GasDay;
    /// # use rust_decimal_macros::dec;
    /// # use time::macros::date;
    /// # let balance = GasPortfolioBalance { bkv_eic: "E".to_owned(), gas_day: GasDay::new(date!(2026-01-01)), positions: vec![], computed_at: time::OffsetDateTime::now_utc() };
    /// let expected_kwh = dec!(5000.0); // from VNB measurement data
    /// let tolerance_kwh = dec!(1.0);   // 1 kWh tolerance per GasNZV §24
    /// match balance.conservation_check(expected_kwh, tolerance_kwh) {
    ///     Ok(total) => println!("Conservation OK: {} kWh allocated", total),
    ///     Err(e) => println!("Conservation violation: {e}"),
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ConservationViolation::IncompleteAllocations`] when any position lacks
    /// an `allocated_kwh` value, or [`ConservationViolation::EnergyImbalance`] when the
    /// sum of allocations differs from `expected_total_kwh` by more than `tolerance_kwh`.
    pub fn conservation_check(
        &self,
        expected_total_kwh: Decimal,
        tolerance_kwh: Decimal,
    ) -> Result<Decimal, ConservationViolation> {
        let positions_without_allocation: Vec<&str> = self
            .positions
            .iter()
            .filter(|p| p.allocated_kwh.is_none())
            .map(|p| p.bilanzkreis_eic.as_str())
            .collect();

        if !positions_without_allocation.is_empty() {
            return Err(ConservationViolation::IncompleteAllocations {
                missing_bilanzkreise: positions_without_allocation
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            });
        }

        let actual_total = self.total_allocated_kwh();
        let deviation = (actual_total - expected_total_kwh).abs();
        if deviation > tolerance_kwh {
            Err(ConservationViolation::EnergyImbalance {
                expected_kwh: expected_total_kwh,
                actual_kwh: actual_total,
                deviation_kwh: deviation,
                tolerance_kwh,
            })
        } else {
            Ok(actual_total)
        }
    }
}

/// Error returned by [`GasPortfolioBalance::conservation_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConservationViolation {
    /// One or more Bilanzkreise have no allocation data yet.
    IncompleteAllocations {
        /// EIC codes of Bilanzkreise without an allocation.
        missing_bilanzkreise: Vec<String>,
    },
    /// Total allocated energy deviates from expected by more than the tolerance.
    EnergyImbalance {
        /// Expected total (from VNB measurement).
        expected_kwh: Decimal,
        /// Actual total (sum of all BKV allocations).
        actual_kwh: Decimal,
        /// Absolute deviation.
        deviation_kwh: Decimal,
        /// Accepted tolerance (per GasNZV §24).
        tolerance_kwh: Decimal,
    },
}

impl std::fmt::Display for ConservationViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompleteAllocations { missing_bilanzkreise } => write!(
                f,
                "Conservation check requires complete allocations; \
                 missing for Bilanzkreise: {}",
                missing_bilanzkreise.join(", ")
            ),
            Self::EnergyImbalance {
                expected_kwh,
                actual_kwh,
                deviation_kwh,
                tolerance_kwh,
            } => write!(
                f,
                "Conservation violation: expected {expected_kwh} kWh_Hs, \
                 got {actual_kwh} kWh_Hs, deviation {deviation_kwh} > tolerance {tolerance_kwh}"
            ),
        }
    }
}

impl std::error::Error for ConservationViolation {}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use time::macros::date;

    fn gas_day() -> GasDay {
        GasDay::new(date!(2026 - 01 - 15))
    }

    #[test]
    fn gas_market_role_abbreviations() {
        assert_eq!(GasMarketRole::Lf.abbreviation(), "LF");
        assert_eq!(GasMarketRole::Fnb.abbreviation(), "FNB");
        assert_eq!(GasMarketRole::Mgv.abbreviation(), "MGV");
        assert_eq!(GasMarketRole::Bkv.abbreviation(), "BKV");
    }

    #[test]
    fn bkv_submits_nominations_has_imbalance() {
        assert!(GasMarketRole::Bkv.submits_nominations());
        assert!(GasMarketRole::Bkv.has_imbalance_obligation());
        assert!(GasMarketRole::Bkv.receives_allocations());
    }

    #[test]
    fn lf_does_not_receive_allocations_directly() {
        assert!(!GasMarketRole::Lf.receives_allocations());
        assert!(!GasMarketRole::Lf.has_imbalance_obligation());
    }

    #[test]
    fn portfolio_net_imbalance_mehr() {
        let balance = GasPortfolioBalance {
            bkv_eic: "EIC_BKV".to_owned(),
            gas_day: gas_day(),
            positions: vec![
                PortfolioPosition {
                    bilanzkreis_eic: "BK1".to_owned(),
                    gas_day: gas_day(),
                    nominated_kwh: Some(dec!(1000.0)),
                    allocated_kwh: Some(dec!(900.0)),
                    imbalance_kwh: Some(dec!(100.0)),
                    is_final: true,
                },
                PortfolioPosition {
                    bilanzkreis_eic: "BK2".to_owned(),
                    gas_day: gas_day(),
                    nominated_kwh: Some(dec!(500.0)),
                    allocated_kwh: Some(dec!(500.0)),
                    imbalance_kwh: Some(dec!(0.0)),
                    is_final: true,
                },
            ],
            computed_at: time::OffsetDateTime::now_utc(),
        };
        assert_eq!(balance.net_imbalance_kwh(), dec!(100.0));
        assert_eq!(balance.portfolio_direction(), ImbalanceDirection::Mehr);
        assert!(balance.is_fully_settled());
        assert_eq!(balance.open_imbalance_count(), 1);
    }

    #[test]
    fn portfolio_balanced_position_returns_zero_imbalance() {
        let pos = PortfolioPosition {
            bilanzkreis_eic: "BK1".to_owned(),
            gas_day: gas_day(),
            nominated_kwh: Some(dec!(1000.0)),
            allocated_kwh: Some(dec!(1000.0)),
            imbalance_kwh: Some(dec!(0.0)),
            is_final: true,
        };
        assert!(pos.is_balanced());
    }

    #[test]
    fn portfolio_position_computes_saldo() {
        let pos = PortfolioPosition {
            bilanzkreis_eic: "BK1".to_owned(),
            gas_day: gas_day(),
            nominated_kwh: Some(dec!(1000.0)),
            allocated_kwh: Some(dec!(800.0)),
            imbalance_kwh: Some(dec!(200.0)),
            is_final: false,
        };
        let saldo = pos.compute_imbalance_saldo("EIC_BKV").unwrap();
        assert_eq!(saldo.imbalance_kwh, dec!(200.0));
        assert_eq!(saldo.direction(), ImbalanceDirection::Mehr);
    }

    #[test]
    fn allocation_version_is_revision() {
        use crate::allocation::AllocationVersion;
        assert!(!AllocationVersion::Initial.is_revision());
        assert!(AllocationVersion::Correction(1).is_revision());
        assert!(AllocationVersion::Final.is_revision());
    }

    // ── GasPortfolioBalance::conservation_check ────────────────────────────────

    #[test]
    fn conservation_check_passes_within_tolerance() {
        let balance = GasPortfolioBalance {
            bkv_eic: "EIC_BKV".to_owned(),
            gas_day: gas_day(),
            positions: vec![
                PortfolioPosition {
                    bilanzkreis_eic: "BK1".to_owned(),
                    gas_day: gas_day(),
                    nominated_kwh: Some(dec!(1000.0)),
                    allocated_kwh: Some(dec!(600.0)),
                    imbalance_kwh: Some(dec!(400.0)),
                    is_final: true,
                },
                PortfolioPosition {
                    bilanzkreis_eic: "BK2".to_owned(),
                    gas_day: gas_day(),
                    nominated_kwh: Some(dec!(400.0)),
                    allocated_kwh: Some(dec!(400.5)), // slight measurement deviation
                    imbalance_kwh: Some(dec!(-0.5)),
                    is_final: true,
                },
            ],
            computed_at: time::OffsetDateTime::now_utc(),
        };
        // Expected total from VNB: 1000 kWh; actual = 600 + 400.5 = 1000.5
        let result = balance.conservation_check(dec!(1000.0), dec!(1.0));
        assert!(result.is_ok(), "deviation 0.5 < tolerance 1.0");
        assert_eq!(result.unwrap(), dec!(1000.5));
    }

    #[test]
    fn conservation_check_fails_when_deviation_exceeds_tolerance() {
        let balance = GasPortfolioBalance {
            bkv_eic: "EIC_BKV".to_owned(),
            gas_day: gas_day(),
            positions: vec![PortfolioPosition {
                bilanzkreis_eic: "BK1".to_owned(),
                gas_day: gas_day(),
                nominated_kwh: Some(dec!(1000.0)),
                allocated_kwh: Some(dec!(990.0)), // 10 kWh short
                imbalance_kwh: Some(dec!(10.0)),
                is_final: true,
            }],
            computed_at: time::OffsetDateTime::now_utc(),
        };
        let result = balance.conservation_check(dec!(1000.0), dec!(1.0));
        assert!(result.is_err(), "deviation 10 > tolerance 1.0");
        match result.unwrap_err() {
            ConservationViolation::EnergyImbalance { deviation_kwh, .. } => {
                assert_eq!(deviation_kwh, dec!(10.0));
            }
            other => panic!("unexpected violation type: {other:?}"),
        }
    }

    #[test]
    fn conservation_check_fails_when_allocation_missing() {
        let balance = GasPortfolioBalance {
            bkv_eic: "EIC_BKV".to_owned(),
            gas_day: gas_day(),
            positions: vec![PortfolioPosition {
                bilanzkreis_eic: "BK_MISSING".to_owned(),
                gas_day: gas_day(),
                nominated_kwh: Some(dec!(500.0)),
                allocated_kwh: None, // initial allocation not yet received
                imbalance_kwh: None,
                is_final: false,
            }],
            computed_at: time::OffsetDateTime::now_utc(),
        };
        let result = balance.conservation_check(dec!(500.0), dec!(1.0));
        assert!(result.is_err());
        match result.unwrap_err() {
            ConservationViolation::IncompleteAllocations { missing_bilanzkreise } => {
                assert_eq!(missing_bilanzkreise, vec!["BK_MISSING"]);
            }
            other => panic!("unexpected violation type: {other:?}"),
        }
    }
}
