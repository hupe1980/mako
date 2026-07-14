//! Funding termination and sanction lifecycle models.
//!
//! Tracks *why* EEG/KWKG support ends and the full lifecycle of §52 compliance
//! violations from onset through fulfillment or expiry.

use rust_decimal::Decimal;
use time::Date;

// ── FoerderendeGrund ──────────────────────────────────────────────────────────

/// Reason why EEG/KWKG support for a plant ends.
///
/// The billing system must track this to correctly transition the settlement
/// model after Förderungsende (e.g. from `FeedInTariff` to `PostEeg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum FoerderendeGrund {
    /// Normal: 20-year Förderdauer expired (§25 Abs. 1 EEG 2023).
    ///
    /// Plant transitions to `PostEeg` (§21 post-Förderung).
    /// `foerderendedatum` = 31 December of the 20th year for statutory plants.
    Expired20Years,

    /// §51a EEG 2023 extension expired: the §51a period extension was applied
    /// and the extended Förderdauer has now also expired.
    ///
    /// Tracking this separately allows the billing system to correctly compute
    /// and credit the §51a extension when reporting to the operator.
    Expired20YearsPlusSect51aExtension,

    /// BNetzA tender award expired (§33 EEG 2023): plant not built within deadline.
    ///
    /// The Ausschreibungsanlage loses its award. No PostEEG transition —
    /// the plant receives no payment unless it wins a new tender.
    AuctionAwardExpired,

    /// KWK hour-limit exhausted (§8 KWKG 2023 for plants >2 MW).
    ///
    /// Plant exhausted its full-load-hour cap (kwk_max_kwh). Status = FoerderungBeendet.
    KwkHourLimitExhausted,

    /// KWKG year-limit reached (§8 KWKG 2023 for plants ≤2 MW).
    KwkYearLimitReached,

    /// Voluntary termination: operator opts out of EEG support.
    VoluntaryTermination,

    /// BNetzA revocation: support revoked due to fraud, false registration, etc.
    Revoked,

    /// Permanent loss: plant destroyed, cannot resume operation.
    PermanentLoss,

    /// MaStR deregistration: plant removed from Marktstammdatenregister retroactively.
    ///
    /// Support is retroactively revoked for the deregistered period.
    MastrDeregistered,
}

impl FoerderendeGrund {
    /// Returns `true` when the plant transitions to post-EEG market value.
    #[must_use]
    pub fn transitions_to_post_eeg(self) -> bool {
        matches!(
            self,
            Self::Expired20Years | Self::Expired20YearsPlusSect51aExtension
        )
    }
}

// ── SanktionStatus ────────────────────────────────────────────────────────────

/// Lifecycle state of a §52 EEG compliance violation for a specific plant.
///
/// §52 violations are not instantaneous — they have a start date, may be
/// resolved (retroactively reducing the penalty), and may have partial months.
/// Tracking the full lifecycle is required for correct multi-month billing.
///
/// ## §52 Abs. 3 retroactive reduction
///
/// When the obligation is fulfilled (e.g. MaStR registration confirmed), the
/// penalty is retroactively reduced to €2/kW/month for eligible violation types
/// (Nr. 1 Fernsteuerbarkeit, Nr. 3 iMSys, Nr. 4 Direktvermarktung, Nr. 11 MaStR).
///
/// This means ALL previously billed months at €10/kW are re-settled at €2/kW.
/// The difference must be credited back to the operator.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SanktionStatus {
    /// Which compliance violation this tracks.
    pub typ: crate::model::SanktionsTyp,

    /// Date when the violation was first detected/reported.
    pub onset_date: Date,

    /// Whether the obligation has since been fulfilled.
    ///
    /// When `true`, §52 Abs. 3 reduces the penalty retroactively.
    pub erfuellt: bool,

    /// Date when the obligation was fulfilled (if `erfuellt = true`).
    pub erfuellungsdatum: Option<Date>,

    /// Number of months the violation was active (including partial months).
    pub monate_aktiv: u32,

    /// Current penalty rate in EUR/kW/month (10 or 2 depending on fulfillment).
    pub aktueller_satz_eur_kw: Decimal,

    /// Whether the plant has Bestandsschutz for THIS violation type.
    ///
    /// For example, pre-2024 Fernsteuerbarkeit installations have Bestandsschutz
    /// under §100 Abs. 3 EEG 2023 — the NB must grant a transition period.
    pub bestandsschutz: bool,
}

impl SanktionStatus {
    /// Compute the total penalty for this violation status.
    ///
    /// Returns the EUR amount the operator owes for `monate_aktiv` months.
    /// Does NOT apply the §52 Abs. 5 cap — the caller must cap when aggregating
    /// multiple violations.
    #[must_use]
    pub fn gesamtpflichtzahlung(&self, leistung_kw: Decimal) -> Decimal {
        crate::foerderdauer::calculate_pflichtzahlung(&crate::model::Pflichtverstoss {
            typ: self.typ,
            leistung_kw,
            monate_des_verstosses: self.monate_aktiv,
            nachtraeglich_erfuellt: self.erfuellt,
            technischer_defekt: false, // SanktionStatus tracks ongoing violations, not defects
        })
    }
}
