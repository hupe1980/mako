//! Biomass-specific EEG settlement models — §§ 42–44b EEG 2023.
//!
//! Biomass plants have fuel-type-dependent remuneration rules:
//!
//! - **§ 42** — the statutory anzulegender Wert for Biomasse: 12,67 ct/kWh up to
//!   a Bemessungsleistung of 150 kW, and Satz 2 excludes Biomethan. Above that,
//!   the value comes from a tender (§ 22 Abs. 4).
//! - **§ 43** — Vergärung von Bioabfällen: a separate, higher claim for plants
//!   running on ≥ 90 Masseprozent separately collected Bioabfälle.
//! - **§ 44** — Vergärung von Gülle: the Güllekleinanlage claim, ≤ 150 kW
//!   installed at the Biogaserzeugungsanlage's site with a minimum manure share.
//! - **§ 44b Abs. 1** — the 45 %-Bemessungsleistung annual quota for Biogas
//!   plants over 100 kW installed; § 44 Gülle plants and § 39 tendered plants
//!   are excluded by Satz 3.
//! - **§ 39i Abs. 1** — the Getreidekorn-und-Mais cap, which conditions a claim
//!   „für Strom aus **Biogas**" held by a plant with a tender award, and steps
//!   down by award year: 40 % (2023), 35 % (2024 – 24.02.2025), 30 %
//!   (25.02.2025 – 2025), 25 % (2026–2028). Deponiegas, Klärgas, Grubengas and
//!   feste Biomasse are outside it.
//!
//! There is no § 42a EEG, and the EEG 2023 imposes no Holzbiomasse restriction —
//! the sustainability rules for solid biomass sit outside it.

use crate::rounding::RoundMoney;
use rust_decimal::Decimal;

// ── BiomassBrennstoff ─────────────────────────────────────────────────────────

/// Biomass fuel type — determines which §43/§44 EEG 2023 rules apply.
///
/// The fuel type affects:
/// - Whether the § 44 Güllekleinanlage claim is applicable.
/// - Whether the § 39i Abs. 1 Getreidekorn/Mais cap applies (tendered plants).
/// - Which of §§ 42/43/44 sets the anzulegender Wert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum BiomassBrennstoff {
    /// Standard plant-based biomass (agricultural residues, dedicated crops).
    ///
    /// Where the plant holds a tender award, subject to the § 39i Abs. 1
    /// Getreidekorn-und-Mais cap, which steps down 40 → 35 → 30 → 25 % by the
    /// year of the award.
    PflanzlicheBiomasse,

    /// Biomethane from biomass feedstocks — upgraded and fed into the gas grid.
    BiomethanAusBiomasse,

    /// Liquid manure (Gülle) and slurry — qualifies for Güllekleinanlage bonus.
    ///
    /// § 44 Abs. 2 EEG 2023 conditions the Güllekleinanlage claim on
    /// installierte Leistung „am Standort der Biogaserzeugungsanlage insgesamt
    /// höchstens 150 Kilowatt" (Nr. 2) and an average Gülle share „von
    /// mindestens 80 Masseprozent" (Nr. 3). Abs. 1 then prices it 22 ct up to a
    /// Bemessungsleistung of 75 kW and 19 ct up to 150 kW — the 75 kW figure is
    /// a rate band, not the eligibility ceiling.
    Guelle,

    /// Solid manure (Festmist) — also eligible for Güllekleinanlage if ≥80%.
    Festmist,

    /// Wood biomass (feste Biomasse).
    ///
    /// The EEG 2023 sets no separate anzulegender Wert for it and imposes no
    /// fresh-wood restriction — the sustainability requirements for solid
    /// biomass sit outside the EEG. What the EEG does add for tendered plants
    /// running feste Biomasse is the § 39i Abs. 2 Höchstbemessungsleistung: 25 %
    /// below the awarded Gebotsmenge, above which the claim falls to zero (or to
    /// the Marktwert under an Einspeisevergütung).
    Holzbiomasse,

    /// Sewage gas (Klärgas) from wastewater treatment.
    Klaergas,

    /// Landfill gas (Deponiegas).
    Deponiegas,

    /// Mine gas (Grubengas) from coal mines.
    Grubengas,

    /// Biogenic waste fractions (not covered by §43 substrate caps).
    BiogenicWaste,
}

impl BiomassBrennstoff {
    /// Returns `true` when this fuel type can open the § 44 EEG 2023
    /// Güllekleinanlage claim.
    ///
    /// The claim needs all of § 44 Abs. 2: generation at the
    /// Biogaserzeugungsanlage's site, installierte Leistung „insgesamt höchstens
    /// 150 Kilowatt" (Nr. 2, checked in [`BiomassSettlementData`]) and „ein
    /// Anteil von Gülle […] von mindestens 80 Masseprozent" (Nr. 3). This method
    /// answers the fuel-type half.
    #[must_use]
    pub fn guellebonusanlage_eligible(self) -> bool {
        matches!(self, Self::Guelle | Self::Festmist)
    }

    /// Whether the plant runs on **Biogas** — § 3 Nr. 11 EEG 2023: „jedes Gas,
    /// das durch anaerobe Vergärung von Biomasse gewonnen wird".
    ///
    /// § 39i Abs. 1 conditions only a claim „für Strom aus Biogas", so this is
    /// the gate on that provision. Deponiegas, Klärgas and Grubengas are their
    /// own Energieträger with their own anzulegender Wert (§ 45), and feste
    /// Biomasse is not a gas at all — none of them is reached by Abs. 1.
    #[must_use]
    pub fn ist_biogas(self) -> bool {
        matches!(
            self,
            Self::PflanzlicheBiomasse
                | Self::BiomethanAusBiomasse
                | Self::Guelle
                | Self::Festmist
                | Self::BiogenicWaste
        )
    }
}

/// § 44 Abs. 2 Nr. 2 EEG 2023 — „die installierte Leistung am Standort der
/// Biogaserzeugungsanlage insgesamt höchstens 150 Kilowatt beträgt".
///
/// The eligibility ceiling of the Güllekleinanlage claim. The 75 kW in Abs. 1
/// Nr. 1 is the boundary between the 22-ct and the 19-ct rate band, not a
/// condition of the claim.
pub const SECT44_ABS2_NR2_GRENZE_KW: Decimal = rust_decimal::dec!(150);

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

    /// Share of the mass fed to the Biogaserzeugung that is Gülle (0.0–1.0).
    ///
    /// § 44 Abs. 2 Nr. 3 EEG 2023 requires „mindestens 80 Masseprozent",
    /// Geflügelmist und Geflügeltrockenkot excepted.
    pub guelle_anteil: Decimal,

    /// Whether the plant meets the § 44 Abs. 2 EEG 2023 conditions the fuel
    /// composition and capacity can answer: ≥ 80 Masseprozent Gülle (Nr. 3) and
    /// installierte Leistung ≤ 150 kW (Nr. 2).
    ///
    /// When `true`, the § 44 Abs. 1 Güllekleinanlage rates apply — 22 ct up to a
    /// Bemessungsleistung of 75 kW, 19 ct up to 150 kW.
    pub ist_guellebonusanlage: bool,

    /// § 39i Abs. 1 EEG 2023 — the share of Getreidekorn und Mais in the mass
    /// fed to the Biogaserzeugung over the calendar year (0.0–1.0).
    ///
    /// Satz 2 counts Ganzpflanzen, Maiskorn-Spindel-Gemisch, Körnermais and
    /// Lieschkolbenschrot as Mais. The cap is on those two feedstocks alone, not
    /// on Energiepflanzen generally.
    pub getreide_mais_anteil: Decimal,

    /// The Gebotstermin at which the plant received its Zuschlag.
    ///
    /// § 39i Abs. 1 conditions the claim only for a plant that holds one, and
    /// the permitted share steps down by the Gebotstermin. `None` — a plant
    /// whose anzulegender Wert is gesetzlich bestimmt — is outside Abs. 1
    /// entirely and has no Getreide-und-Mais cap.
    pub zuschlag_gebotstermin: Option<time::Date>,
}

impl BiomassSettlementData {
    /// Construct from fuel type and composition data.
    ///
    /// Computes `ist_guellebonusanlage` from the § 44 Abs. 2 EEG 2023 conditions
    /// the fuel composition and the installierte Leistung can answer.
    #[must_use]
    pub fn new(
        hauptbrennstoff: BiomassBrennstoff,
        guelle_anteil: Decimal,
        getreide_mais_anteil: Decimal,
        leistung_kw: Decimal,
        zuschlag_gebotstermin: Option<time::Date>,
    ) -> Self {
        use rust_decimal::dec;
        let ist_guellebonusanlage = hauptbrennstoff.guellebonusanlage_eligible()
            && guelle_anteil >= dec!(0.80)
            && leistung_kw <= SECT44_ABS2_NR2_GRENZE_KW;
        Self {
            hauptbrennstoff,
            guelle_anteil,
            ist_guellebonusanlage,
            getreide_mais_anteil,
            zuschlag_gebotstermin,
        }
    }

    /// The § 39i Abs. 1 EEG 2023 Höchstanteil this plant must stay within, and
    /// whether it does.
    ///
    /// `None` where Abs. 1 does not reach the plant: one that burns something
    /// other than Biogas, one without a Zuschlag, or one awarded outside the
    /// Gebotstermine the four Nummern name.
    #[must_use]
    pub fn sect39i_hoechstanteil(&self) -> Option<Decimal> {
        if !self.hauptbrennstoff.ist_biogas() {
            return None;
        }
        self.zuschlag_gebotstermin.and_then(sect39i_hoechstanteil)
    }

    /// Whether § 39i Abs. 1 EEG 2023 leaves the § 19 Abs. 1 claim standing.
    ///
    /// `true` wherever Abs. 1 does not reach the plant — the condition it states
    /// is the only thing it can fail.
    #[must_use]
    pub fn sect39i_eingehalten(&self) -> bool {
        self.sect39i_hoechstanteil()
            .is_none_or(|grenze| self.getreide_mais_anteil <= grenze)
    }
}

/// § 39i Abs. 1 EEG 2023 — the highest share of Getreidekorn und Mais a
/// bezuschlagte Biogasanlage may use in a calendar year, by Gebotstermin.
///
/// | Gebotstermin | Höchstanteil | Nummer |
/// |---|---|---|
/// | im Jahr 2023 | 40 % | Nr. 1 |
/// | nach dem 31.12.2023 und vor dem 25.02.2025 | 35 % | Nr. 2 |
/// | nach dem 25.02.2025 und vor dem 01.01.2026 | 30 % | Nr. 3 |
/// | in den Jahren 2026, 2027, 2028 | 25 % | Nr. 4 |
///
/// The share is a **Masseprozent** of the Getreidekorn und Mais fed to the
/// Biogaserzeugung, not of Energiepflanzen at large, and the ladder is keyed on
/// the Gebotstermin of the award — the plant's Inbetriebnahme and the settlement
/// year have no bearing on it.
///
/// `None` for a Gebotstermin no Nummer names: before 2023, on 25 February 2025
/// itself (Nr. 2 stops before it and Nr. 3 starts after it), or after 2028.
///
/// ```rust
/// use eeg_billing::biomasse::sect39i_hoechstanteil;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// assert_eq!(sect39i_hoechstanteil(date!(2023 - 04 - 01)), Some(dec!(0.40)));
/// assert_eq!(sect39i_hoechstanteil(date!(2024 - 10 - 01)), Some(dec!(0.35)));
/// assert_eq!(sect39i_hoechstanteil(date!(2025 - 10 - 01)), Some(dec!(0.30)));
/// assert_eq!(sect39i_hoechstanteil(date!(2026 - 04 - 01)), Some(dec!(0.25)));
/// assert_eq!(sect39i_hoechstanteil(date!(2022 - 04 - 01)), None);
/// ```
#[must_use]
pub fn sect39i_hoechstanteil(gebotstermin: time::Date) -> Option<Decimal> {
    use rust_decimal::dec;
    // Nr. 3's window opens *after* 25 February 2025 and Nr. 2's closes *before*
    // it, so the day itself belongs to neither.
    const NR3_STICHTAG: time::Date = time::macros::date!(2025 - 02 - 25);
    match gebotstermin.year() {
        2023 => Some(dec!(0.40)),
        2024 => Some(dec!(0.35)),
        2025 if gebotstermin < NR3_STICHTAG => Some(dec!(0.35)),
        2025 if gebotstermin > NR3_STICHTAG => Some(dec!(0.30)),
        2026..=2028 => Some(dec!(0.25)),
        _ => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    use time::macros::date;

    /// § 44 Abs. 2 EEG 2023 — the Güllekleinanlage claim needs both the manure
    /// share and the capacity ceiling.
    #[test]
    fn guellebonusanlage_qualifies_when_criteria_met() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::Guelle,
            dec!(0.85),
            dec!(0.05),
            dec!(50),
            None,
        );
        assert!(data.ist_guellebonusanlage);
    }

    /// **§ 44 Abs. 2 Nr. 2 EEG 2023 sets the ceiling at 150 kW, not 75.**
    ///
    /// The claim stands „wenn […] die installierte Leistung am Standort der
    /// Biogaserzeugungsanlage insgesamt höchstens 150 Kilowatt beträgt". 75 kW
    /// is where Abs. 1 steps the rate from 22 to 19 ct — a plant between the two
    /// figures is a Güllekleinanlage on the 19-ct band.
    #[test]
    fn guellebonusanlage_runs_to_the_sect44_abs2_nr2_ceiling() {
        let at = |kw| {
            BiomassSettlementData::new(BiomassBrennstoff::Guelle, dec!(0.90), dec!(0.05), kw, None)
        };
        assert!(
            at(dec!(100)).ist_guellebonusanlage,
            "100 kW is inside Nr. 2"
        );
        assert!(
            at(SECT44_ABS2_NR2_GRENZE_KW).ist_guellebonusanlage,
            "\u{201e}höchstens 150 Kilowatt\u{201c} includes 150"
        );
        assert!(
            !at(dec!(151)).ist_guellebonusanlage,
            "past 150 kW Nr. 2 is not met"
        );
    }

    /// **§ 39i Abs. 1 EEG 2023 reaches only Strom aus Biogas.**
    ///
    /// Abs. 1 conditions „ein durch einen Zuschlag erworbener Anspruch nach § 19
    /// Absatz 1 **für Strom aus Biogas**". Biogas is „jedes Gas, das durch
    /// anaerobe Vergärung von Biomasse gewonnen wird" (§ 3 Nr. 11); Holzbiomasse
    /// is no gas and Deponie-, Klär- und Grubengas are their own Energieträger,
    /// so a Getreide-und-Mais share cannot cost any of them their claim.
    #[test]
    fn sect39i_reaches_only_biogas() {
        use BiomassBrennstoff as B;
        let at = |brennstoff| {
            BiomassSettlementData::new(
                brennstoff,
                dec!(0.0),
                dec!(0.90),
                dec!(500),
                Some(date!(2026 - 04 - 01)),
            )
        };
        for ausserhalb in [B::Holzbiomasse, B::Klaergas, B::Deponiegas, B::Grubengas] {
            assert_eq!(
                at(ausserhalb).sect39i_hoechstanteil(),
                None,
                "{ausserhalb:?}"
            );
            assert!(at(ausserhalb).sect39i_eingehalten(), "{ausserhalb:?}");
        }
        for biogas in [
            B::PflanzlicheBiomasse,
            B::BiomethanAusBiomasse,
            B::Guelle,
            B::Festmist,
            B::BiogenicWaste,
        ] {
            assert_eq!(
                at(biogas).sect39i_hoechstanteil(),
                Some(dec!(0.25)),
                "{biogas:?}"
            );
            assert!(!at(biogas).sect39i_eingehalten(), "{biogas:?}");
        }
    }

    /// **§ 39i Abs. 1 EEG 2023 reaches only plants that hold a Zuschlag.**
    ///
    /// A plant whose anzulegender Wert is gesetzlich bestimmt (§ 42) has no
    /// Getreide-und-Mais cap at all, however much of either it uses.
    #[test]
    fn sect39i_does_not_reach_a_plant_without_a_zuschlag() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::PflanzlicheBiomasse,
            dec!(0.0),
            dec!(0.45),
            dec!(120),
            None,
        );
        assert_eq!(data.sect39i_hoechstanteil(), None);
        assert!(data.sect39i_eingehalten());
    }

    /// **§ 39i Abs. 1 steps down by Gebotstermin.** A 28 % share clears the
    /// 2023 and 2024 awards and fails a 2026 one, which is capped at 25 %.
    #[test]
    fn sect39i_steps_down_by_gebotstermin() {
        let at = |termin| {
            BiomassSettlementData::new(
                BiomassBrennstoff::PflanzlicheBiomasse,
                dec!(0.0),
                dec!(0.28),
                dec!(500),
                Some(termin),
            )
        };
        assert!(at(date!(2023 - 04 - 01)).sect39i_eingehalten());
        assert!(at(date!(2024 - 10 - 01)).sect39i_eingehalten());
        assert!(at(date!(2025 - 10 - 01)).sect39i_eingehalten());
        assert!(
            !at(date!(2026 - 04 - 01)).sect39i_eingehalten(),
            "Nr. 4 caps a 2026 award at 25 Masseprozent"
        );
    }
}

// ── §3 Nr. 6 EEG — Bemessungsleistung ─────────────────────────────────────────

/// The hours a Bemessungsleistung is measured against for one calendar year.
///
/// §3 Nr. 6 EEG defines the Bemessungsleistung as the annual kWh divided by
/// *„die Summe der vollen Zeitstunden des jeweiligen Kalenderjahres abzüglich der
/// vollen Stunden vor der erstmaligen Erzeugung"*. Two things follow that a flat
/// 8 760 gets wrong:
///
/// - a **leap year has 8 784 hours**, so the flat figure understates the quota by
///   24 h × the capacity share — real money for a plant on the §44b cap;
/// - a plant that **first generated during the year** is measured against the
///   hours since, not the whole year, so a flat figure hands it a full-year quota
///   the statute does not give it.
///
/// Returns `None` only for a year the calendar cannot represent.
#[must_use]
pub fn bemessungsleistung_stunden(
    kalenderjahr: i32,
    erstmalige_erzeugung: Option<time::Date>,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::Decimal;
    let jahresbeginn =
        time::Date::from_calendar_date(kalenderjahr, time::Month::January, 1).ok()?;
    let naechstes_jahr =
        time::Date::from_calendar_date(kalenderjahr + 1, time::Month::January, 1).ok()?;

    // Hours before the first generation are deducted; a plant that started in an
    // earlier year has none to deduct, one that starts later has no claim at all.
    let start = match erstmalige_erzeugung {
        Some(d) if d >= naechstes_jahr => return Some(Decimal::ZERO),
        Some(d) if d > jahresbeginn => d,
        _ => jahresbeginn,
    };
    let tage = (naechstes_jahr - start).whole_days();
    Some(Decimal::from(tage) * Decimal::from(24))
}

/// §44b Abs. 1 EEG 2023 — the annual kWh a Biogas plant may be paid the full rate for.
///
/// The share of a calendar year's generation whose Bemessungsleistung equals 45 %
/// of the installed capacity: `0,45 × P_inst ×` the §3 Nr. 6 hours.
#[must_use]
pub fn sect44b_jahreskontingent_kwh(
    leistung_kw: rust_decimal::Decimal,
    kalenderjahr: i32,
    erstmalige_erzeugung: Option<time::Date>,
) -> rust_decimal::Decimal {
    use rust_decimal::dec;
    let stunden =
        bemessungsleistung_stunden(kalenderjahr, erstmalige_erzeugung).unwrap_or(dec!(8760));
    (leistung_kw * dec!(0.45) * stunden).round_kfm(3)
}

#[cfg(test)]
mod bemessungsleistung_tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    /// A leap year has 8 784 hours. The flat 8 760 cost a 500 kW plant
    /// 24 × 0,45 × 500 = 5 400 kWh of full-rate quota every leap year.
    #[test]
    fn a_leap_year_has_more_hours() {
        assert_eq!(bemessungsleistung_stunden(2027, None), Some(dec!(8760)));
        assert_eq!(bemessungsleistung_stunden(2028, None), Some(dec!(8784)));
        assert_eq!(
            sect44b_jahreskontingent_kwh(dec!(500), 2028, None)
                - sect44b_jahreskontingent_kwh(dec!(500), 2027, None),
            dec!(5400)
        );
    }

    /// §3 Nr. 6 deducts the hours before the first generation, so a plant that
    /// started mid-year is measured against the rest of it — a flat figure would
    /// hand it a full-year quota.
    #[test]
    fn hours_before_the_first_generation_are_deducted() {
        // 1 July 2027 → 184 days remain (Jul 31 + Aug 31 + Sep 30 + Oct 31 + Nov 30 + Dec 31).
        assert_eq!(
            bemessungsleistung_stunden(2027, Some(date!(2027 - 07 - 01))),
            Some(dec!(4416))
        );
        // A plant commissioned in an earlier year is measured against the whole year.
        assert_eq!(
            bemessungsleistung_stunden(2027, Some(date!(2020 - 03 - 01))),
            Some(dec!(8760))
        );
        // One that starts after the year has no quota in it at all.
        assert_eq!(
            bemessungsleistung_stunden(2027, Some(date!(2028 - 01 - 01))),
            Some(dec!(0))
        );
    }
}
