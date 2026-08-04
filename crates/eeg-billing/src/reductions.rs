//! §52 EEG 2023 — Pflichtzahlungen and their netting against the disbursement.
//!
//! # Where each reduction belongs
//!
//! EEG settlement carries two structurally different kinds of reduction, and
//! putting them in one pipeline gets the arithmetic wrong:
//!
//! - **Reductions of the *anzulegender Wert*** — §§53, 53b, 53c, 54. These
//!   change the rate before any formula runs and live in
//!   [`crate::aw_reductions`] (§53 Abs. 1 itself is applied in the settlement
//!   formula from [`crate::rates::sect53_deduction`]). They must act on the AW
//!   because the gleitende Marktprämie floors at zero: a euro deduction taken
//!   afterwards would drive the settlement negative.
//! - **A separate monetary obligation** — §52 Pflichtzahlungen. These are not a
//!   rate change at all. The plant keeps its full Vergütung and owes the
//!   Pflichtzahlung alongside it; §52 Abs. 6 merely *permits* the
//!   Netzbetreiber to offset the two. That offsetting is what this module does.
//!
//! ```text
//! Einspeisemenge × anzulegender Wert / 100      ← AW already cut by §§53–54
//!   └─ §51 Negativpreisregel   (reduces the eligible kWh)
//!   └─ §52 SanktionAlt         (pre-2023 EEG: rate → 0 / Marktwert / ×0,8)
//!   └─ §52 Pflichtzahlung      (EEG 2023: separate obligation, Vergütung intact)
//!   └─ §52 Abs. 6 Netting      (optional NB offset — this module)
//! ─────────────────────────────────
//!   = disbursement + any residual Pflichtzahlung
//! ```
//!
//! Legal text: EEG 2023 in der Fassung vom 18.12.2025, in Kraft ab 23.12.2025.

use rust_decimal::Decimal;

// ── Sect52Netting ─────────────────────────────────────────────────────────────

/// Result of applying §52 Abs. 6 EEG 2023 netting.
///
/// The NB may deduct the monthly §52 Pflichtzahlung from the Vergütung before
/// disbursing. Any excess penalty (when Pflichtzahlung > Vergütung) becomes
/// a residual receivable of the NB.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NettingResult {
    /// Vergütung disbursed to the plant operator after netting.
    ///
    /// `max(0, vergütung_eur − pflichtzahlung_eur)`
    pub net_vergütung_eur: Decimal,

    /// Residual Pflichtzahlung still owed by the operator to the NB.
    ///
    /// Non-zero only when `pflichtzahlung_eur > vergütung_eur`.
    /// `max(0, pflichtzahlung_eur − vergütung_eur)`
    pub residual_pflichtzahlung_eur: Decimal,

    /// Whether any netting was performed (Pflichtzahlung > 0).
    pub netting_applied: bool,
}

/// Apply §52 Abs. 6 EEG 2023 netting of Pflichtzahlung against Vergütung.
///
/// The NB is permitted (but not required) to offset the monthly §52 obligation
/// against the Vergütung within the same calendar month.
///
/// ## Parameters
///
/// - `vergütung_eur`: Gross Vergütung amount before netting.
/// - `pflichtzahlung_eur`: Monthly §52 Pflichtzahlung (from `SettleOutput::pflichtzahlung_eur`).
///
/// ## Returns
///
/// A [`NettingResult`] with the net disbursement and any residual obligation.
///
/// # Example
///
/// ```rust
/// use eeg_billing::reductions::apply_sect52_netting;
/// use rust_decimal::dec;
///
/// // Vergütung: 42.55 EUR, Pflichtzahlung: 10.00 EUR
/// // Net disbursement = 32.55 EUR, residual = 0
/// let result = apply_sect52_netting(dec!(42.55), dec!(10.00));
/// assert_eq!(result.net_vergütung_eur, dec!(32.55));
/// assert_eq!(result.residual_pflichtzahlung_eur, dec!(0));
/// assert!(result.netting_applied);
/// ```
#[must_use]
pub fn apply_sect52_netting(vergütung_eur: Decimal, pflichtzahlung_eur: Decimal) -> NettingResult {
    if pflichtzahlung_eur.is_zero() {
        return NettingResult {
            net_vergütung_eur: vergütung_eur,
            residual_pflichtzahlung_eur: Decimal::ZERO,
            netting_applied: false,
        };
    }

    let net_vergütung = (vergütung_eur - pflichtzahlung_eur).max(Decimal::ZERO);
    let residual = (pflichtzahlung_eur - vergütung_eur).max(Decimal::ZERO);

    NettingResult {
        net_vergütung_eur: net_vergütung,
        residual_pflichtzahlung_eur: residual,
        netting_applied: true,
    }
}

// ── Full reduction pipeline ───────────────────────────────────────────────────

/// All applicable reductions for a single billing period.
///
/// Each field maps to a distinct legal mechanism. Reductions are applied in
/// order from §51 through §54 (see module-level documentation).
///
/// Not all reductions apply simultaneously — e.g. §53c is area-specific and
/// §54 applies only to Ausschreibungsanlagen. Fields are `Option` / `Vec`
/// to allow selective application.
///
/// ## Typical usage
///
/// ```rust
/// use eeg_billing::reductions::ReductionPipeline;
/// use rust_decimal::dec;
///
/// // Only Pflichtzahlung applies, with §52 Abs. 6 netting enabled
/// let pipeline = ReductionPipeline {
///     pflichtzahlung_eur: Some(dec!(10.00)),
///     apply_sect52_netting: true,
///     ..ReductionPipeline::none()
/// };
/// let result = pipeline.apply(dec!(42.55));
/// assert_eq!(result.net_vergütung_eur, dec!(32.55));
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReductionPipeline {
    /// §52 EEG 2023 Pflichtzahlung for the period.
    pub pflichtzahlung_eur: Option<Decimal>,

    /// Whether the Netzbetreiber exercises the §52 Abs. 6 offset.
    ///
    /// `true` = deduct the Pflichtzahlung from the disbursement.
    /// `false` = Vergütung and Pflichtzahlung are settled separately, and the
    /// whole Pflichtzahlung stays outstanding.
    pub apply_sect52_netting: bool,
}

/// Result after applying the full reduction pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReductionPipelineResult {
    /// Vergütung disbursed to operator (after all reductions and netting).
    pub net_vergütung_eur: Decimal,

    /// §52 residual Pflichtzahlung still owed to NB (after netting, if any).
    pub residual_pflichtzahlung_eur: Decimal,

    /// Sum of all reductions applied to the settlement (for audit trail).
    pub total_reductions_eur: Decimal,
}

impl ReductionPipeline {
    /// Empty pipeline — no reductions applied.
    #[must_use]
    pub fn none() -> Self {
        Self {
            pflichtzahlung_eur: None,
            apply_sect52_netting: false,
        }
    }

    /// Apply the §52 Abs. 6 offset, returning the disbursement and any residual.
    ///
    /// `gross_settlement_eur` is the output of `calculate_settlement`, whose AW
    /// has already been cut by §§53–54.
    #[must_use]
    pub fn apply(&self, gross_settlement_eur: Decimal) -> ReductionPipelineResult {
        let Some(pz) = self.pflichtzahlung_eur.filter(|p| *p > Decimal::ZERO) else {
            return ReductionPipelineResult {
                net_vergütung_eur: gross_settlement_eur,
                residual_pflichtzahlung_eur: Decimal::ZERO,
                total_reductions_eur: Decimal::ZERO,
            };
        };
        if !self.apply_sect52_netting {
            // §52 Abs. 6 is a permission, not a duty: without it the operator
            // receives the full Vergütung and still owes the whole obligation.
            return ReductionPipelineResult {
                net_vergütung_eur: gross_settlement_eur,
                residual_pflichtzahlung_eur: pz,
                total_reductions_eur: Decimal::ZERO,
            };
        }
        let netting = apply_sect52_netting(gross_settlement_eur, pz);
        ReductionPipelineResult {
            net_vergütung_eur: netting.net_vergütung_eur,
            residual_pflichtzahlung_eur: netting.residual_pflichtzahlung_eur,
            total_reductions_eur: pz - netting.residual_pflichtzahlung_eur,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn netting_normal_case() {
        let result = apply_sect52_netting(dec!(42.55), dec!(10.00));
        assert_eq!(result.net_vergütung_eur, dec!(32.55));
        assert_eq!(result.residual_pflichtzahlung_eur, dec!(0));
        assert!(result.netting_applied);
    }

    #[test]
    fn netting_penalty_exceeds_vergutung() {
        // Penalty > Vergütung: NB receives 0, operator still owes the residual
        let result = apply_sect52_netting(dec!(30.00), dec!(50.00));
        assert_eq!(result.net_vergütung_eur, dec!(0));
        assert_eq!(result.residual_pflichtzahlung_eur, dec!(20.00));
    }

    #[test]
    fn netting_zero_penalty_no_change() {
        let result = apply_sect52_netting(dec!(42.55), dec!(0));
        assert_eq!(result.net_vergütung_eur, dec!(42.55));
        assert!(!result.netting_applied);
    }

    #[test]
    fn pipeline_none_no_change() {
        let result = ReductionPipeline::none().apply(dec!(42.55));
        assert_eq!(result.net_vergütung_eur, dec!(42.55));
        assert_eq!(result.total_reductions_eur, dec!(0));
    }

    #[test]
    fn pipeline_sect52_netting() {
        let pipeline = ReductionPipeline {
            pflichtzahlung_eur: Some(dec!(10.00)),
            apply_sect52_netting: true,
        };
        let result = pipeline.apply(dec!(42.55));
        assert_eq!(result.net_vergütung_eur, dec!(32.55));
        assert_eq!(result.residual_pflichtzahlung_eur, dec!(0));
    }

    #[test]
    fn pipeline_netting_without_applying() {
        // Pflichtzahlung set but netting not applied → full Vergütung disbursed
        let pipeline = ReductionPipeline {
            pflichtzahlung_eur: Some(dec!(10.00)),
            apply_sect52_netting: false,
        };
        let result = pipeline.apply(dec!(42.55));
        assert_eq!(result.net_vergütung_eur, dec!(42.55));
        // Residual = full Pflichtzahlung (not netted)
        assert_eq!(result.residual_pflichtzahlung_eur, dec!(10.00));
    }
}
