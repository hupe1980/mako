//! `BilAReM` — Bilanzieller Ausgleich von Redispatch-Maßnahmen (BK6-23-241).
//!
//! Festlegung BK6-23-241 (Beschluss 07.05.2026) consolidates Redispatch 2.0:
//! Tenor Ziff. 1 puts the ÜNB bilanzieller Ausgleich per §13a Abs. 1a `EnWG` on
//! the `BilAReM` rules **from 01.07.2026**; Ziff. 3/4 revoke BK6-20-060/-061;
//! Ziff. 5 revokes `MaBiS` Anlage 1 Kap. 17 effective 30.09.2026 (the surviving
//! 17.1/17.3 content continues as "Anlage zur `BilAReM`" from 01.10.2026).
//!
//! Two settlement models coexist per Steuerbare Ressource (SR):
//!
//! - **Planwertmodell** — the anweisende Netzbetreiber performs the
//!   bilanzielle Ausgleich against the BKV, sized from the geplante Fahrweise
//!   (last ex-ante planning data before the Abruf), executed via
//!   korrespondierende Fahrpläne between the NB's dedicated
//!   Redispatch-Bilanzkreis and the betroffener Bilanzkreis. For fluctuating
//!   plants the residual between actual Ausfallarbeit and the plan-based
//!   Ausgleich is settled **financially only** (see
//!   [`grid_billing`-side `bilarem_finanzielle_korrektur`]).
//! - **Prognosemodell** — no NB-side bilanzieller Ausgleich; the imbalance
//!   stays with the BKV (§14 Abs. 1 S. 3 `EnWG`, befristet bis 31.12.2031), who
//!   receives Aufwendungsersatz from the NB per §14 Abs. 1b `EnWG` (amount not
//!   standardised in `BilAReM`).
//!
//! Migration Prognose → Planwert is one-way, per SR, effective only at
//! quarter boundaries with ≥6 months notice (Zuordnungsmitteilung ANB →
//! LF/EIV/BTR). Soll-target: transmission-grid-relevant SR migrated by
//! 01.01.2031; the statutory Prognosemodell window ends 31.12.2031.
//!
//! The EDI@Energy wire formats for `BilAReM` are published by the expert group
//! on **relative** deadlines (no calendar date in the Tenor); this module is
//! deliberately wire-format-free — the seam the formats will plug into.

use time::{Date, Month};

/// `BilAReM` rules apply to the ÜNB bilanzieller Ausgleich from this day
/// (BK6-23-241 Tenor Ziff. 1).
pub const BILAREM_WIRKSAM: Date = match Date::from_calendar_date(2026, Month::July, 1) {
    Ok(d) => d,
    Err(_) => panic!("valid date"),
};

/// `MaBiS` Anlage 1 Kap. 17 is revoked with the end of this day (Tenor Ziff. 5);
/// surviving content continues as "Anlage zur `BilAReM`" from 01.10.2026.
pub const MABIS_ANLAGE1_KAP17_ENDE: Date =
    match Date::from_calendar_date(2026, Month::September, 30) {
        Ok(d) => d,
        Err(_) => panic!("valid date"),
    };

/// Grandfathered Pauschal-Abrechnung ends with this day; from 01.01.2029 those
/// TR fall into vereinfachte Spitzabrechnung unless Spitzabrechnung was
/// elected by 30.11.2028 (`BilAReM` Kap. 3.2.1).
pub const PAUSCHAL_ABRECHNUNG_ENDE: Date = match Date::from_calendar_date(2028, Month::December, 31)
{
    Ok(d) => d,
    Err(_) => panic!("valid date"),
};

/// Election deadline for grandfathered TR choosing Spitzabrechnung over the
/// vereinfachte Spitzabrechnung default.
pub const SPITZ_WAHL_FRIST: Date = match Date::from_calendar_date(2028, Month::November, 30) {
    Ok(d) => d,
    Err(_) => panic!("valid date"),
};

/// Soll-target: SR that improve transmission-grid congestion-relief efficiency
/// are to be in the Planwertmodell by this day.
pub const MIGRATION_SOLL_ZIEL: Date = match Date::from_calendar_date(2031, Month::January, 1) {
    Ok(d) => d,
    Err(_) => panic!("valid date"),
};

/// End of the statutory Prognosemodell/BKV window (§14 Abs. 1 S. 3 `EnWG`,
/// befristet bis 31.12.2031). From 2032 the NB performs the full Ausgleich.
pub const PROGNOSEMODELL_ENDE: Date = match Date::from_calendar_date(2031, Month::December, 31) {
    Ok(d) => d,
    Err(_) => panic!("valid date"),
};

/// Settlement model of a Steuerbare Ressource for the bilanzieller Ausgleich.
///
/// Each SR is in exactly one model; clusters must be model-pure. Migration is
/// one-way (Prognose → Planwert only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bilanzierungsmodell {
    /// NB-side Ausgleich via korrespondierende Fahrpläne (`BilAReM` Kap. 2.1).
    Planwertmodell,
    /// BKV keeps the imbalance; NB owes Aufwendungsersatz (§14 Abs. 1b `EnWG`).
    Prognosemodell,
}

/// Ausfallarbeit settlement method of a Technische Ressource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Abrechnungsverfahren {
    /// Spitzabrechnung (measured; KF/Wind-Bin variants per `BilAReM` Kap. 3).
    Spitz,
    /// Vereinfachte Spitzabrechnung.
    VereinfachteSpitz,
    /// Pauschal — grandfathered TR only, until 31.12.2028.
    Pauschal,
}

impl Abrechnungsverfahren {
    /// Whether this method is admissible for a TR on `date`.
    ///
    /// `grandfathered` = the TR was in Pauschal-Abrechnung at the
    /// Bekanntmachung of BK6-23-241. New TR can never elect Pauschal, and TR
    /// in the Planwertmodell must use Spitz-/vereinfachte Spitzabrechnung.
    #[must_use]
    pub fn admissible(self, date: Date, grandfathered: bool, modell: Bilanzierungsmodell) -> bool {
        match self {
            Self::Spitz | Self::VereinfachteSpitz => true,
            Self::Pauschal => {
                grandfathered
                    && date <= PAUSCHAL_ABRECHNUNG_ENDE
                    && modell == Bilanzierungsmodell::Prognosemodell
            }
        }
    }

    /// The default method a grandfathered Pauschal TR falls into from
    /// 01.01.2029 when no Spitzabrechnung election was made by 30.11.2028.
    #[must_use]
    pub const fn post_pauschal_default() -> Self {
        Self::VereinfachteSpitz
    }
}

/// Errors validating a model migration (Zuordnungsmitteilung).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// Effective date is not a quarter boundary (01.01/01.04/01.07/01.10).
    #[error("Wirksamkeitsdatum {0} ist kein Quartalsbeginn (01.01/01.04/01.07/01.10)")]
    KeinQuartalsbeginn(Date),
    /// Less than 6 months between notice and effective date.
    #[error("Ankündigungsfrist unterschritten: {notice} → {effective} (< 6 Monate)")]
    FristUnterschritten {
        /// Notice date.
        notice: Date,
        /// Requested effective date.
        effective: Date,
    },
    /// Planwert → Prognose is not permitted (one-way migration).
    #[error("Rückkehr vom Planwert- ins Prognosemodell ist unzulässig")]
    KeinWegZurueck,
}

/// Zuordnungsmitteilung: the ANB announces an SR's migration into the
/// Planwertmodell to LF/EIV/BTR (`BilAReM` Kap. 2.3).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Zuordnungsmitteilung {
    /// The Steuerbare Ressource being migrated.
    pub sr_id: String,
    /// The NB's dedicated Redispatch-Bilanzkreis (exactly one per NB).
    pub redispatch_bilanzkreis: String,
    /// Day the notice was issued.
    pub mitteilungsdatum: Date,
    /// Requested effective date (quarter boundary, ≥6 months out).
    pub wirksam_ab: Date,
}

impl Zuordnungsmitteilung {
    /// Validate the migration per `BilAReM` Kap. 2.3: quarter-boundary
    /// effectiveness, ≥6 months notice, one-way only.
    ///
    /// # Errors
    ///
    /// Returns the first violated [`MigrationError`].
    ///
    /// # Panics
    ///
    /// Never in practice: the six-month arithmetic keeps the month in
    /// `1..=12` and the clamped day always exists in the target month.
    pub fn validate(&self, current: Bilanzierungsmodell) -> Result<(), MigrationError> {
        if current == Bilanzierungsmodell::Planwertmodell {
            return Err(MigrationError::KeinWegZurueck);
        }
        let d = self.wirksam_ab;
        let is_quarter_start = d.day() == 1
            && matches!(
                d.month(),
                Month::January | Month::April | Month::July | Month::October
            );
        if !is_quarter_start {
            return Err(MigrationError::KeinQuartalsbeginn(d));
        }
        // ≥ 6 months notice: effective date must be on/after notice + 6 months.
        let mut y = self.mitteilungsdatum.year();
        let mut m = self.mitteilungsdatum.month() as u8 + 6;
        if m > 12 {
            m -= 12;
            y += 1;
        }
        let month = Month::try_from(m).expect("1..=12");
        let day = self.mitteilungsdatum.day().min(month.length(y));
        let earliest = Date::from_calendar_date(y, month, day).expect("valid date");
        if d < earliest {
            return Err(MigrationError::FristUnterschritten {
                notice: self.mitteilungsdatum,
                effective: d,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn mitteilung(notice: Date, effective: Date) -> Zuordnungsmitteilung {
        Zuordnungsmitteilung {
            sr_id: "SR-1".into(),
            redispatch_bilanzkreis: "11XRD-NB-00001-L".into(),
            mitteilungsdatum: notice,
            wirksam_ab: effective,
        }
    }

    #[test]
    fn migration_requires_quarter_boundary() {
        let m = mitteilung(date!(2026 - 08 - 01), date!(2027 - 05 - 01));
        assert_eq!(
            m.validate(Bilanzierungsmodell::Prognosemodell),
            Err(MigrationError::KeinQuartalsbeginn(date!(2027 - 05 - 01)))
        );
    }

    #[test]
    fn migration_requires_six_months_notice() {
        let m = mitteilung(date!(2026 - 08 - 15), date!(2027 - 01 - 01));
        assert!(matches!(
            m.validate(Bilanzierungsmodell::Prognosemodell),
            Err(MigrationError::FristUnterschritten { .. })
        ));
        let ok = mitteilung(date!(2026 - 08 - 15), date!(2027 - 04 - 01));
        assert_eq!(ok.validate(Bilanzierungsmodell::Prognosemodell), Ok(()));
    }

    #[test]
    fn migration_is_one_way() {
        let m = mitteilung(date!(2026 - 08 - 01), date!(2027 - 04 - 01));
        assert_eq!(
            m.validate(Bilanzierungsmodell::Planwertmodell),
            Err(MigrationError::KeinWegZurueck)
        );
    }

    #[test]
    fn pauschal_only_for_grandfathered_prognose_tr_until_2028() {
        use Abrechnungsverfahren as A;
        use Bilanzierungsmodell as B;
        // Grandfathered TR in the Prognosemodell: Pauschal admissible until end-2028.
        assert!(A::Pauschal.admissible(date!(2028 - 12 - 31), true, B::Prognosemodell));
        // From 2029: no longer admissible.
        assert!(!A::Pauschal.admissible(date!(2029 - 01 - 01), true, B::Prognosemodell));
        // New TR: never.
        assert!(!A::Pauschal.admissible(date!(2027 - 01 - 01), false, B::Prognosemodell));
        // Planwertmodell TR: never Pauschal.
        assert!(!A::Pauschal.admissible(date!(2027 - 01 - 01), true, B::Planwertmodell));
        // Spitz variants always admissible.
        assert!(A::Spitz.admissible(date!(2032 - 01 - 01), false, B::Planwertmodell));
        assert!(A::VereinfachteSpitz.admissible(date!(2029 - 01 - 01), true, B::Prognosemodell));
        assert_eq!(A::post_pauschal_default(), A::VereinfachteSpitz);
    }
}
