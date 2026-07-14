//! §§52–54 EEG 2023 — settlement reductions and netting pipeline.
//!
//! German EEG settlement involves a series of **orthogonal, composable reductions**
//! applied after the gross entitlement is computed. These reductions are governed by
//! distinct paragraphs and must be applied in the correct order:
//!
//! ```text
//! Gross Einspeisemenge × Anzulegender Wert / 100
//!   └─ §51 Negativpreisregel          (reduces eligible kWh)
//!   └─ §52 SanktionAlt               (old EEG: reduces rate to 0 / EPEX / ×0.8)
//!   └─ §52 Pflichtzahlungen          (EEG 2023: separate penalty, Vergütung unchanged)
//!   └─ §52 Abs. 6 Netting            (NB may net penalty against Vergütung)
//!   └─ §53 Vergütungsabzug           (−0.4/−0.2 ct/kWh, applied at rate level)
//!   └─ §53b Regionale Reduzierung    (BNetzA-certified area reduction)
//!   └─ §53c Energiesteuerabzug       (electricity tax on self-consumed Eigenstromerzeugung)
//!   └─ §54 Ausschreibungsreduzierung (auction-specific AW reduction)
//! ─────────────────────────────────
//!   = NET Einspeisevergütung / Marktprämie
//! ```
//!
//! ## §52 Abs. 6 Netting
//!
//! The NB may offset the §52 Pflichtzahlung against the Vergütung payment within
//! the same calendar month (§52 Abs. 6 EEG 2023). This produces:
//! - `net_vergütung = max(0, vergütung_eur − pflichtzahlung_eur)`
//! - `residual_pflichtzahlung = max(0, pflichtzahlung_eur − vergütung_eur)`
//!
//! ## §53c Energiesteuer (electricity tax on self-consumption)
//!
//! Under §9 Abs. 1 Nr. 1 EnergieStG: electricity generated and self-consumed
//! by an operator from their own plant is **exempt** from electricity tax
//! (Energiesteuerbefreiung) when:
//! - Plant ≤ 2 MW and operator is not an electricity supplier
//! - The self-consumed electricity is not fed into the grid
//!
//! For EEG billing: §53c EEG 2023 introduces a reduction when the BNetzA certifies
//! that an area has a structural oversupply. This is a forward-compatibility field;
//! BNetzA methodology is still under development as of 2026.
//!
//! ## §54 Ausschreibungsreduzierung
//!
//! For auction-awarded plants (§22 EEG 2023), the awarded `anzulegender Wert`
//! may be reduced when the actual Einspeisemenge significantly underperforms
//! the auction projection. This is enforced via separate BNetzA notification.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

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
/// use rust_decimal_macros::dec;
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

// ── Sect53c ───────────────────────────────────────────────────────────────────

/// §53c EEG 2023 — reduction for Eigenversorgung in structurally oversupplied areas.
///
/// When BNetzA certifies a grid area as structurally oversupplied (Strukturelle
/// Überangebotssituation), operators in that area who practice Eigenversorgung
/// (self-consumption from their own EEG plant) may have their Vergütung reduced.
///
/// **Status (2026):** BNetzA methodology is under development. No certificates
/// have been issued yet. This struct is for forward compatibility only.
///
/// ## §9 Abs. 1 Nr. 1 EnergieStG exemption vs. §53c EEG
///
/// These are separate legal instruments:
/// - **EnergieStG §9**: electricity tax exemption for self-consumed power from
///   own plant ≤2 MW (no EEG relevance, handled by tax authorities)
/// - **§53c EEG**: feed-in tariff reduction when area has structural oversupply
///   (EEG-specific, not yet implemented by BNetzA as of 2026)
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sect53cReduction {
    /// BNetzA-certified regional reduction factor (0.00–1.00).
    ///
    /// `0.10` = 10% reduction in Vergütung for this area/period.
    /// Applied as: `reduced_vergütung = vergütung × (1 − factor)`
    pub regional_factor: Decimal,

    /// BNetzA certificate reference (e.g. `"BNetzA-53c-2026-REG-BW-001"`).
    pub certificate_reference: String,
}

impl Sect53cReduction {
    /// Compute the §53c reduction amount for the given settlement.
    ///
    /// # Example
    ///
    /// ```rust
    /// use eeg_billing::reductions::Sect53cReduction;
    /// use rust_decimal_macros::dec;
    ///
    /// let reduction = Sect53cReduction {
    ///     regional_factor: dec!(0.10),
    ///     certificate_reference: "BNetzA-53c-2026-TEST".into(),
    /// };
    /// // Settlement = 100 EUR → 10% reduction = 10 EUR deducted
    /// let amount = reduction.compute_reduction(dec!(100.00));
    /// assert_eq!(amount, dec!(10.00));
    /// ```
    #[must_use]
    pub fn compute_reduction(&self, vergütung_eur: Decimal) -> Decimal {
        (vergütung_eur * self.regional_factor).round_dp(5)
    }
}

// ── Sect54Reduction ───────────────────────────────────────────────────────────

/// §54 EEG 2023 — Ausschreibungsreduzierung (auction AW reduction).
///
/// Under §54 EEG 2023, the BNetzA may reduce the awarded `anzulegender Wert`
/// for auction plants if:
/// - The plant repeatedly underperforms vs. auction projection
/// - The operator fails to meet commissioning deadlines (§36d EEG 2023)
/// - The operator violates §37a (iMSys Nachrüstung) conditions
///
/// The reduction is applied as a flat deduction from the AW in ct/kWh.
///
/// **Important**: §54 reductions are announced by BNetzA notice and become
/// effective from the next billing period. Store them against the specific
/// auction round and plant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sect54Reduction {
    /// Reduction amount in ct/kWh to deduct from the awarded AW.
    ///
    /// Applied as: `effective_aw = awarded_aw − sect54_deduction_ct`
    pub deduction_ct_kwh: Decimal,

    /// BNetzA notification reference.
    pub bnetza_notification_ref: String,

    /// Effective from date (first billing period affected).
    pub effective_from: time::Date,
}

impl Sect54Reduction {
    /// Compute the effective AW after §54 deduction.
    ///
    /// Result is floored at zero (AW cannot be negative under §54).
    ///
    /// # Example
    ///
    /// ```rust
    /// use eeg_billing::reductions::Sect54Reduction;
    /// use rust_decimal_macros::dec;
    /// use time::macros::date;
    ///
    /// let reduction = Sect54Reduction {
    ///     deduction_ct_kwh: dec!(0.5),
    ///     bnetza_notification_ref: "BNetzA-54-2026-TEST".into(),
    ///     effective_from: date!(2026-01-01),
    /// };
    /// // Awarded AW = 6.50 ct/kWh → effective = 6.00 ct/kWh
    /// let effective = reduction.effective_aw(dec!(6.50));
    /// assert_eq!(effective, dec!(6.00));
    /// ```
    #[must_use]
    pub fn effective_aw(&self, awarded_aw_ct: Decimal) -> Decimal {
        (awarded_aw_ct - self.deduction_ct_kwh).max(Decimal::ZERO)
    }

    /// Whether this reduction is active on a given billing date.
    #[must_use]
    pub fn is_active_on(&self, billing_date: time::Date) -> bool {
        billing_date >= self.effective_from
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
/// use rust_decimal_macros::dec;
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
    /// §52 EEG 2023 Pflichtzahlung to net against Vergütung (when `apply_sect52_netting` is true).
    pub pflichtzahlung_eur: Option<Decimal>,

    /// Whether the NB exercises §52 Abs. 6 netting (optional — NB's decision).
    ///
    /// `true` = deduct Pflichtzahlung from Vergütung disbursement.
    /// `false` = Vergütung and Pflichtzahlung are settled separately.
    pub apply_sect52_netting: bool,

    /// §53b regional Grünstromkennzeichnung reduction in ct/kWh.
    ///
    /// Applied to the Einspeisemenge: `reduction_eur = kwh × sect53b_ct / 100`.
    pub sect53b_ct_kwh: Option<Decimal>,

    /// §53c area reduction (BNetzA certificate required).
    pub sect53c: Option<Sect53cReduction>,

    /// §54 Ausschreibungsreduzierung.
    pub sect54: Option<Sect54Reduction>,
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
            sect53b_ct_kwh: None,
            sect53c: None,
            sect54: None,
        }
    }

    /// Apply all reductions in order, returning the final net Vergütung.
    ///
    /// `gross_settlement_eur` is the result of `calculate_settlement` BEFORE netting.
    /// `eligible_kwh` is needed to compute §53b / §53c volume-based reductions.
    #[must_use]
    pub fn apply_with_kwh(
        &self,
        gross_settlement_eur: Decimal,
        eligible_kwh: Decimal,
    ) -> ReductionPipelineResult {
        let mut remaining = gross_settlement_eur;
        let mut total_reductions = Decimal::ZERO;

        // ── §53b regional reduction (ct/kWh) ─────────────────────────────────
        if let Some(sect53b_ct) = self.sect53b_ct_kwh.filter(|ct| *ct > Decimal::ZERO) {
            let reduction = (eligible_kwh * sect53b_ct / dec!(100)).round_dp(5);
            remaining -= reduction;
            total_reductions += reduction;
        }

        // ── §53c area reduction (factor on Vergütung) ────────────────────────
        if let Some(ref sect53c) = self.sect53c {
            let reduction = sect53c.compute_reduction(remaining);
            remaining -= reduction;
            total_reductions += reduction;
        }

        // ── §52 Abs. 6 netting ────────────────────────────────────────────────
        let (net, residual) = if self.apply_sect52_netting {
            if let Some(pz) = self.pflichtzahlung_eur.filter(|p| *p > Decimal::ZERO) {
                let netting = apply_sect52_netting(remaining, pz);
                total_reductions += pz - netting.residual_pflichtzahlung_eur;
                (
                    netting.net_vergütung_eur,
                    netting.residual_pflichtzahlung_eur,
                )
            } else {
                (remaining, Decimal::ZERO)
            }
        } else {
            (remaining, self.pflichtzahlung_eur.unwrap_or(Decimal::ZERO))
        };

        ReductionPipelineResult {
            net_vergütung_eur: net,
            residual_pflichtzahlung_eur: residual,
            total_reductions_eur: total_reductions,
        }
    }

    /// Convenience wrapper when `eligible_kwh` is not available (omits volume-based §53b).
    #[must_use]
    pub fn apply(&self, gross_settlement_eur: Decimal) -> ReductionPipelineResult {
        self.apply_with_kwh(gross_settlement_eur, Decimal::ZERO)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

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
    fn sect53c_compute_reduction() {
        let r = Sect53cReduction {
            regional_factor: dec!(0.10),
            certificate_reference: "TEST-001".into(),
        };
        assert_eq!(r.compute_reduction(dec!(100.00)), dec!(10.00000));
    }

    #[test]
    fn sect54_effective_aw_not_negative() {
        let r = Sect54Reduction {
            deduction_ct_kwh: dec!(7.00),
            bnetza_notification_ref: "TEST-001".into(),
            effective_from: date!(2026 - 01 - 01),
        };
        // AW = 5 ct, deduction = 7 ct → floor at 0
        assert_eq!(r.effective_aw(dec!(5.00)), dec!(0));
        assert_eq!(r.effective_aw(dec!(6.50)), dec!(0));
        assert_eq!(r.effective_aw(dec!(8.00)), dec!(1.00));
    }

    #[test]
    fn sect54_active_on_check() {
        let r = Sect54Reduction {
            deduction_ct_kwh: dec!(0.5),
            bnetza_notification_ref: "TEST-001".into(),
            effective_from: date!(2026 - 03 - 01),
        };
        assert!(!r.is_active_on(date!(2026 - 02 - 28)));
        assert!(r.is_active_on(date!(2026 - 03 - 01)));
        assert!(r.is_active_on(date!(2027 - 01 - 01)));
    }

    #[test]
    fn pipeline_none_no_change() {
        let result = ReductionPipeline::none().apply_with_kwh(dec!(42.55), dec!(500));
        assert_eq!(result.net_vergütung_eur, dec!(42.55));
        assert_eq!(result.total_reductions_eur, dec!(0));
    }

    #[test]
    fn pipeline_sect52_netting() {
        let pipeline = ReductionPipeline {
            pflichtzahlung_eur: Some(dec!(10.00)),
            apply_sect52_netting: true,
            ..ReductionPipeline::none()
        };
        let result = pipeline.apply(dec!(42.55));
        assert_eq!(result.net_vergütung_eur, dec!(32.55));
        assert_eq!(result.residual_pflichtzahlung_eur, dec!(0));
    }

    #[test]
    fn pipeline_sect53b_plus_netting() {
        let pipeline = ReductionPipeline {
            pflichtzahlung_eur: Some(dec!(5.00)),
            apply_sect52_netting: true,
            sect53b_ct_kwh: Some(dec!(0.5)),
            ..ReductionPipeline::none()
        };
        // 500 kWh × 0.5 ct/kWh / 100 = 2.50 EUR §53b reduction
        // gross = 42.55, after §53b = 40.05, after netting = 35.05
        let result = pipeline.apply_with_kwh(dec!(42.55), dec!(500));
        assert_eq!(result.net_vergütung_eur, dec!(35.05));
        assert!(result.total_reductions_eur > dec!(0));
    }

    #[test]
    fn pipeline_netting_without_applying() {
        // Pflichtzahlung set but netting not applied → full Vergütung disbursed
        let pipeline = ReductionPipeline {
            pflichtzahlung_eur: Some(dec!(10.00)),
            apply_sect52_netting: false,
            ..ReductionPipeline::none()
        };
        let result = pipeline.apply(dec!(42.55));
        assert_eq!(result.net_vergütung_eur, dec!(42.55));
        // Residual = full Pflichtzahlung (not netted)
        assert_eq!(result.residual_pflichtzahlung_eur, dec!(10.00));
    }
}
