//! §51 EEG — Verringerung des Zahlungsanspruchs bei negativen Preisen.
//!
//! §51 reduces the anzulegender Wert to null for the periods in which the
//! Spotmarktpreis is negative. Two things decide what a plant loses: **which
//! version of §51 governs it**, and how much of its feed-in fell into the
//! qualifying periods. Both live here.
//!
//! ## The regime is keyed on the Inbetriebnahmedatum, not on a law year
//!
//! §51 has been rewritten four times and the current cut — the
//! **Solarspitzengesetz** (Gesetz zur Änderung des EnWG, in force **25.02.2025**)
//! — took effect *mid-year*. A plant commissioned on 2025-02-01 and one
//! commissioned on 2025-03-01 are both "EEG 2023" plants and are governed by
//! entirely different rules: the first by the staged 4-3-2-1-hour rule with a
//! 400-kW exemption, the second by the first negative quarter-hour with a
//! 100-kW/iMSys exemption. A year bucket cannot express that, so
//! [`NegativpreisRegime`] is derived from the exact commissioning **date**.
//!
//! | Inbetriebnahme | §51 trigger | kW exemption | §51a extension |
//! |---|---|---|---|
//! | ≤ 2015-12-31 | never (§100 Abs. 1 Satz 4 EEG 2017) | — | — |
//! | 2016-01-01 – 2020-12-31 | ≥ 6 consecutive hours | Wind < 3 MW · sonstige < 500 kW | none |
//! | 2021-01-01 – 2022-12-31 | ≥ 4 consecutive hours | < 500 kW | ausschreibungspflichtige only |
//! | 2023-01-01 – 2025-02-24 | staged 4-3-2-1 h | < 400 kW | ausschreibungspflichtige only |
//! | ≥ 2025-02-25 | first negative quarter-hour | < 100 kW bis zum Ablauf des iMSys-Jahres · < 2 kW until §85 Abs. 2 Nr. 12 | all plants |
//!
//! Pilotwindenergieanlagen (§3 Nr. 37 EEG 2023) are exempt under every version.
//!
//! ### Sources
//! - §51 EEG i.d.F. des Solarspitzengesetzes (BGBl. 2025 I Nr. 55), in force 25.02.2025
//! - §51 Abs. 1 EEG 2023 i.d.F. vom 01.01.2023 (staged 4-3-2-1-hour rule)
//! - §51 Abs. 2 EEG 2023 i.d.F. vom 01.01.2023 (400 kW), §51 Abs. 2 EEG 2021 (500 kW)
//! - §51 Abs. 3 Nr. 1/2 EEG 2017 (Wind < 3 MW, sonstige < 500 kW)
//! - §100 Abs. 1 Satz 4 EEG 2017 (§51 first applies to plants from 01.01.2016)
//! - §51a Abs. 1 EEG (Verlängerung des Vergütungszeitraums)
//! - Clearingstelle EEG|KWKG, Häufige Rechtsfrage 264

use crate::technology::ErzeugungsArt;
use rust_decimal::{Decimal, dec};
use time::{Date, OffsetDateTime, macros::date};

/// The day the Solarspitzengesetz took effect.
pub const SOLARSPITZENGESETZ_INKRAFTTRETEN: Date = date!(2025 - 02 - 25);

/// Installed capacity below which §51 does not apply until the Bundesnetzagentur
/// has made its Festlegung under §85 Abs. 2 Nr. 12 EEG (§51 Abs. 2 Nr. 2).
///
/// No such Festlegung has been issued, so the exemption is unconditional today.
pub const SECT51_KLEINSTANLAGEN_GRENZE_KW: Decimal = dec!(2);

/// The uplift on the anzulegender Wert for a Bestandsanlage that opts into the
/// Solarspitzengesetz regime: **0,6 ct/kWh** (§100 EEG).
///
/// The operator declares in Textform to the Netzbetreiber that §§ 51 and 51a
/// shall apply, and the declaration takes effect at the earliest at the end of
/// the calendar year in which the plant is fitted with an intelligent metering
/// system. From then on the plant forgoes payment during negative prices and is
/// paid 0,6 ct/kWh more for everything else.
pub const SECT51_OPTIN_ZUSCHLAG_CT_KWH: Decimal = dec!(0.6);

/// When a §100 opt-in declared on `erklaert_am` takes effect.
///
/// The earliest possible date is the **end of the calendar year in which the
/// iMSys is installed** — 1 January of the following year, since a settlement
/// period either lies wholly inside the new regime or wholly outside it. A plant
/// with no iMSys has no effective date at all: the declaration is on file and
/// simply has not started running.
#[must_use]
pub fn optin_wirksam_ab(erklaert_am: Date, imesys_rollout: Option<Date>) -> Option<Date> {
    let imesys = imesys_rollout?;
    let nach_imesys_jahr =
        Date::from_calendar_date(imesys.year() + 1, time::Month::January, 1).ok()?;
    let nach_erklaerung =
        Date::from_calendar_date(erklaert_am.year() + 1, time::Month::January, 1).ok()?;
    Some(nach_imesys_jahr.max(nach_erklaerung))
}

/// § 51 Abs. 2 Nr. 1 EEG — the day the sub-100-kW exemption lapses.
///
/// The exemption covers „Zeiträume **vor dem Ablauf des Kalenderjahres**, in dem
/// die Anlage mit einem intelligenten Messsystem ausgestattet wird", so it runs
/// to the end of the installation year and lapses on 1 January of the year
/// after. A plant with no iMSys keeps it indefinitely and has no such day.
///
/// ```rust
/// use eeg_billing::negativpreis::imesys_befreiung_entfaellt_ab;
/// use time::macros::date;
///
/// // Fitted in March 2026 — exempt for the whole of 2026.
/// assert_eq!(
///     imesys_befreiung_entfaellt_ab(Some(date!(2026 - 03 - 15))),
///     Some(date!(2027 - 01 - 01))
/// );
/// assert_eq!(imesys_befreiung_entfaellt_ab(None), None);
/// ```
#[must_use]
pub fn imesys_befreiung_entfaellt_ab(imesys_rollout: Option<Date>) -> Option<Date> {
    let imesys = imesys_rollout?;
    Date::from_calendar_date(imesys.year() + 1, time::Month::January, 1).ok()
}

/// Whether the § 51 Abs. 2 Nr. 1 sub-100-kW exemption has lapsed for a
/// settlement period beginning on `periodenbeginn`.
///
/// This is what [`NegativpreisRegime::ist_befreit`] takes as `has_imesys`:
/// having an iMSys is not itself the trigger — the turn of the year after it was
/// fitted is. Answers `false` while either date is unknown, which keeps the
/// exemption rather than withholding a payment on a guess.
///
/// ```rust
/// use eeg_billing::negativpreis::imesys_befreiung_entfallen;
/// use time::macros::date;
///
/// let rollout = Some(date!(2026 - 03 - 15));
/// assert!(!imesys_befreiung_entfallen(rollout, Some(date!(2026 - 12 - 01))));
/// assert!(imesys_befreiung_entfallen(rollout, Some(date!(2027 - 01 - 01))));
/// ```
#[must_use]
pub fn imesys_befreiung_entfallen(
    imesys_rollout: Option<Date>,
    periodenbeginn: Option<Date>,
) -> bool {
    match (
        imesys_befreiung_entfaellt_ab(imesys_rollout),
        periodenbeginn,
    ) {
        (Some(ab), Some(periode)) => periode >= ab,
        _ => false,
    }
}

/// Which version of §51 governs a plant.
///
/// Derive with [`NegativpreisRegime::fuer_inbetriebnahme`] — never construct one
/// from a law year, because the Solarspitzengesetz boundary falls inside 2025.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum NegativpreisRegime {
    /// Inbetriebnahme ≤ 2015-12-31 — §51 never applies (§100 Abs. 1 Satz 4 EEG 2017).
    Keine,
    /// Inbetriebnahme 2016-01-01 – 2020-12-31 — §51 EEG 2017: six consecutive hours.
    Eeg2017,
    /// Inbetriebnahme 2021-01-01 – 2022-12-31 — §51 EEG 2021: four consecutive hours.
    Eeg2021,
    /// Inbetriebnahme 2023-01-01 – 2025-02-24 — §51 EEG 2023 in its original
    /// form: the staged 4-3-2-1-hour rule, stepping down by commissioning year.
    Eeg2023Gestaffelt {
        /// Consecutive negative hours required: 4, 3, 2 or 1.
        stunden: u8,
    },
    /// Inbetriebnahme ≥ 2025-02-25 — §51 i.d.F. des Solarspitzengesetzes: the
    /// anzulegender Wert falls to null from the **first** negative quarter-hour.
    Solarspitzen,
}

impl NegativpreisRegime {
    /// The §51 version governing a plant in a given billing period.
    ///
    /// `optin_wirksam_ab` is the effective date of a §100 opt-in declaration (see
    /// [`optin_wirksam_ab`]); from that date the plant is under the
    /// Solarspitzengesetz regime whatever its vintage, and earns the
    /// [`SECT51_OPTIN_ZUSCHLAG_CT_KWH`] uplift.
    #[must_use]
    pub fn fuer_periode(
        inbetriebnahme: Date,
        optin_wirksam_ab: Option<Date>,
        billing_date: Option<Date>,
    ) -> Self {
        if let (Some(ab), Some(bd)) = (optin_wirksam_ab, billing_date)
            && bd >= ab
        {
            return Self::Solarspitzen;
        }
        Self::fuer_inbetriebnahme(inbetriebnahme)
    }

    /// The §51 version governing a plant commissioned on `inbetriebnahme`.
    #[must_use]
    pub fn fuer_inbetriebnahme(inbetriebnahme: Date) -> Self {
        if inbetriebnahme >= SOLARSPITZENGESETZ_INKRAFTTRETEN {
            return Self::Solarspitzen;
        }
        match inbetriebnahme.year() {
            ..=2015 => Self::Keine,
            2016..=2020 => Self::Eeg2017,
            2021..=2022 => Self::Eeg2021,
            // §51 Abs. 1 EEG 2023 (Fassung 01.01.2023): four consecutive hours;
            // three for plants commissioned after 31.12.2023, two after
            // 31.12.2025, one after 31.12.2027. Only the 4 h and 3 h steps are
            // ever reachable — the Solarspitzengesetz superseded the rule on
            // 25.02.2025 — but the full ladder is modelled so the branch cannot
            // silently pick a wrong step if the cut-over date is ever revisited.
            2023 => Self::Eeg2023Gestaffelt { stunden: 4 },
            2024..=2025 => Self::Eeg2023Gestaffelt { stunden: 3 },
            2026..=2027 => Self::Eeg2023Gestaffelt { stunden: 2 },
            _ => Self::Eeg2023Gestaffelt { stunden: 1 },
        }
    }

    /// Minimum length of a negative-price run, in quarter-hours, for §51 to bite.
    ///
    /// `None` when §51 does not apply to the plant's vintage at all.
    #[must_use]
    pub fn mindest_lauflaenge_qh(self) -> Option<usize> {
        match self {
            Self::Keine => None,
            Self::Eeg2017 => Some(24),
            Self::Eeg2021 => Some(16),
            Self::Eeg2023Gestaffelt { stunden } => Some(usize::from(stunden) * 4),
            Self::Solarspitzen => Some(1),
        }
    }

    /// Installed capacity **below** which §51 does not apply.
    ///
    /// `None` when §51 never applies. Under [`Solarspitzen`](Self::Solarspitzen)
    /// the 100-kW figure is transitional — it lapses at the end of the calendar
    /// year in which an iMSys is fitted — which
    /// [`ist_befreit`](Self::ist_befreit) accounts for; the 2-kW floor below it
    /// does not.
    #[must_use]
    pub fn kw_grenze(self, art: Option<ErzeugungsArt>) -> Option<Decimal> {
        match self {
            Self::Keine => None,
            // §51 Abs. 3 Nr. 1 EEG 2017: Wind < 3 MW; Nr. 2: sonstige < 500 kW.
            Self::Eeg2017 => Some(if art.is_some_and(|a| a.is_wind()) {
                dec!(3000)
            } else {
                dec!(500)
            }),
            // EEG 2021 dropped the wind carve-out: 500 kW for everything.
            Self::Eeg2021 => Some(dec!(500)),
            Self::Eeg2023Gestaffelt { .. } => Some(dec!(400)),
            Self::Solarspitzen => Some(dec!(100)),
        }
    }

    /// Whether a plant is exempt from §51 on size / technology grounds.
    ///
    /// `leistung_kwp` must be the **aggregated** capacity: §51 Abs. 2 Satz 2
    /// applies §24 to the size test, so §24-linked blocks count as one plant.
    /// A `None` capacity is treated as "large" — an unknown size must not buy an
    /// exemption.
    #[must_use]
    pub fn ist_befreit(
        self,
        leistung_kwp: Option<Decimal>,
        art: Option<ErzeugungsArt>,
        has_imesys: bool,
        ist_pilotwindanlage: bool,
    ) -> bool {
        let Some(grenze) = self.kw_grenze(art) else {
            return true; // §51 does not exist for this vintage
        };
        // Pilotwindenergieanlagen are carved out of §51 in every Fassung.
        if ist_pilotwindanlage {
            return true;
        }
        let Some(kw) = leistung_kwp else {
            return false;
        };
        if self == Self::Solarspitzen {
            // §51 Abs. 2 Nr. 2: below 2 kW the rule is suspended until the
            // Bundesnetzagentur's §85 Abs. 2 Nr. 12 Festlegung, which does not
            // exist. An iMSys does not lift this one.
            if kw < SECT51_KLEINSTANLAGEN_GRENZE_KW {
                return true;
            }
            // §51 Abs. 2 Nr. 1: the sub-100-kW exemption covers „Zeiträume vor
            // dem Ablauf des Kalenderjahres, in dem die Anlage mit einem
            // intelligenten Messsystem ausgestattet wird" — it survives the
            // installation year and lapses at the turn of the year, which is
            // what `has_imesys` states (see `imesys_befreiung_entfallen`).
            return kw < grenze && !has_imesys;
        }
        kw < grenze
    }

    /// Whether §51a extends the Vergütungszeitraum for the lost quarter-hours.
    ///
    /// Before the Solarspitzengesetz the extension existed only for
    /// ausschreibungspflichtige Anlagen; it now covers every plant §51 reduces.
    #[must_use]
    pub fn verlaengerungsanspruch(self, ist_ausschreibungspflichtig: bool) -> bool {
        match self {
            Self::Keine | Self::Eeg2017 => false,
            Self::Eeg2021 | Self::Eeg2023Gestaffelt { .. } => ist_ausschreibungspflichtig,
            Self::Solarspitzen => true,
        }
    }

    /// Short label for audit positions and operator-facing explanations.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Keine => "§51 nicht anwendbar (Inbetriebnahme vor 2016)",
            Self::Eeg2017 => "§51 EEG 2017 (6-Stunden-Regel)",
            Self::Eeg2021 => "§51 EEG 2021 (4-Stunden-Regel)",
            Self::Eeg2023Gestaffelt { stunden: 4 } => "§51 EEG 2023 (4-Stunden-Regel)",
            Self::Eeg2023Gestaffelt { stunden: 3 } => "§51 EEG 2023 (3-Stunden-Regel)",
            Self::Eeg2023Gestaffelt { stunden: 2 } => "§51 EEG 2023 (2-Stunden-Regel)",
            Self::Eeg2023Gestaffelt { .. } => "§51 EEG 2023 (1-Stunden-Regel)",
            Self::Solarspitzen => "§51 EEG i.d.F. Solarspitzengesetz (ab erster Viertelstunde)",
        }
    }
}

/// One quarter-hour of feed-in overlaid with the sign of its spot price.
#[derive(Debug, Clone, Copy)]
pub struct NegativpreisInterval {
    /// Interval start on the quarter-hour grid.
    pub start: OffsetDateTime,
    /// Feed-in (Einspeisung) energy in this quarter-hour, kWh.
    pub feed_in_kwh: Decimal,
    /// Whether the Spotmarktpreis for this quarter-hour was negative.
    pub price_negative: bool,
}

/// Result of the §51 overlay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NegativpreisResult {
    /// Feed-in kWh in qualifying negative-price intervals — feeds
    /// `SettleInput.kwh_during_negative_epex` (§51 reduction).
    pub kwh_during_negative: Decimal,
    /// Count of qualifying quarter-hours — feeds
    /// `SettleInput.negative_price_quarter_hours` (§51a extension accrual).
    pub negative_quarter_hours: u64,
}

/// Derive the §51 negative-price feed-in from time-ordered quarter-hour intervals.
///
/// `intervals` must be sorted ascending by `start`. A negative-price *run* is a
/// maximal sequence of consecutive negative quarter-hours 15 minutes apart; it
/// counts only once its length reaches the regime's
/// [`mindest_lauflaenge_qh`](NegativpreisRegime::mindest_lauflaenge_qh). Negative
/// feed-in values (net consumption) are floored at zero.
///
/// This is the pure overlay — it answers *how much* fell into qualifying
/// periods. Whether the plant is exempt on size grounds is
/// [`ist_befreit`](NegativpreisRegime::ist_befreit) and is applied by the
/// settlement engine, so a caller that derives figures for an exempt plant still
/// gets a truthful count for reporting.
#[must_use]
pub fn derive_negativpreis(
    intervals: &[NegativpreisInterval],
    regime: NegativpreisRegime,
) -> NegativpreisResult {
    let Some(min_run_qh) = regime.mindest_lauflaenge_qh() else {
        return NegativpreisResult::default();
    };

    let mut result = NegativpreisResult::default();
    let mut i = 0;
    while i < intervals.len() {
        if !intervals[i].price_negative {
            i += 1;
            continue;
        }
        // Extend a maximal run of consecutive, time-adjacent negative quarter-hours.
        let run_start = i;
        let mut j = i + 1;
        while j < intervals.len()
            && intervals[j].price_negative
            && intervals[j].start == intervals[j - 1].start + time::Duration::minutes(15)
        {
            j += 1;
        }
        if j - run_start >= min_run_qh {
            for iv in &intervals[run_start..j] {
                result.kwh_during_negative += iv.feed_in_kwh.max(Decimal::ZERO);
                result.negative_quarter_hours += 1;
            }
        }
        i = j;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn qh(n: i64, kwh: &str, neg: bool) -> NegativpreisInterval {
        NegativpreisInterval {
            start: datetime!(2026-06-01 00:00 UTC) + time::Duration::minutes(15 * n),
            feed_in_kwh: kwh.parse::<Decimal>().unwrap(),
            price_negative: neg,
        }
    }

    /// The Solarspitzengesetz cut-over falls inside 2025, so the boundary has to
    /// be a date. Getting it wrong pays a February plant on March's rules.
    #[test]
    fn the_solarspitzengesetz_boundary_is_a_date_not_a_year() {
        assert_eq!(
            NegativpreisRegime::fuer_inbetriebnahme(date!(2025 - 02 - 24)),
            NegativpreisRegime::Eeg2023Gestaffelt { stunden: 3 }
        );
        assert_eq!(
            NegativpreisRegime::fuer_inbetriebnahme(date!(2025 - 02 - 25)),
            NegativpreisRegime::Solarspitzen
        );
    }

    #[test]
    fn regime_boundaries_follow_the_clearingstelle_table() {
        for (ibn, want) in [
            (date!(2015 - 12 - 31), NegativpreisRegime::Keine),
            (date!(2016 - 01 - 01), NegativpreisRegime::Eeg2017),
            (date!(2020 - 12 - 31), NegativpreisRegime::Eeg2017),
            (date!(2021 - 01 - 01), NegativpreisRegime::Eeg2021),
            (date!(2022 - 12 - 31), NegativpreisRegime::Eeg2021),
            (
                date!(2023 - 01 - 01),
                NegativpreisRegime::Eeg2023Gestaffelt { stunden: 4 },
            ),
            (
                date!(2024 - 01 - 01),
                NegativpreisRegime::Eeg2023Gestaffelt { stunden: 3 },
            ),
            (date!(2026 - 06 - 01), NegativpreisRegime::Solarspitzen),
        ] {
            assert_eq!(
                NegativpreisRegime::fuer_inbetriebnahme(ibn),
                want,
                "Inbetriebnahme {ibn}"
            );
        }
    }

    #[test]
    fn solarspitzen_counts_a_single_quarter_hour() {
        let ivs = [qh(0, "10", false), qh(1, "12", true), qh(2, "8", false)];
        let r = derive_negativpreis(&ivs, NegativpreisRegime::Solarspitzen);
        assert_eq!(r.kwh_during_negative, dec!(12));
        assert_eq!(r.negative_quarter_hours, 1);
    }

    /// A 2024 plant needs three consecutive negative hours. Applying the
    /// post-Solarspitzengesetz rule to it reduces a month §51 does not touch.
    #[test]
    fn a_2024_plant_needs_three_consecutive_hours() {
        let regime = NegativpreisRegime::fuer_inbetriebnahme(date!(2024 - 07 - 01));
        // 11 QH = 2¾ h — short.
        let short: Vec<_> = (0..11).map(|n| qh(n, "5", true)).collect();
        assert_eq!(
            derive_negativpreis(&short, regime),
            NegativpreisResult::default()
        );
        // 12 QH = 3 h — qualifies.
        let long: Vec<_> = (0..12).map(|n| qh(n, "5", true)).collect();
        let r = derive_negativpreis(&long, regime);
        assert_eq!(r.kwh_during_negative, dec!(60));
        assert_eq!(r.negative_quarter_hours, 12);
        // The same run under the Solarspitzen regime qualifies too.
        assert_eq!(
            derive_negativpreis(&short, NegativpreisRegime::Solarspitzen).negative_quarter_hours,
            11
        );
    }

    #[test]
    fn eeg2021_requires_four_consecutive_hours() {
        let short: Vec<_> = (0..8).map(|n| qh(n, "5", true)).collect();
        assert_eq!(
            derive_negativpreis(&short, NegativpreisRegime::Eeg2021),
            NegativpreisResult::default()
        );
        let long: Vec<_> = (0..16).map(|n| qh(n, "5", true)).collect();
        assert_eq!(
            derive_negativpreis(&long, NegativpreisRegime::Eeg2021).kwh_during_negative,
            dec!(80)
        );
    }

    #[test]
    fn a_gap_breaks_the_consecutive_run() {
        let mut ivs: Vec<_> = (0..15).map(|n| qh(n, "5", true)).collect();
        ivs.push(qh(15, "5", false));
        ivs.push(qh(16, "5", true));
        assert_eq!(
            derive_negativpreis(&ivs, NegativpreisRegime::Eeg2021),
            NegativpreisResult::default()
        );
    }

    #[test]
    fn negative_feed_in_is_floored() {
        let ivs = [qh(0, "-3", true)];
        assert_eq!(
            derive_negativpreis(&ivs, NegativpreisRegime::Solarspitzen).kwh_during_negative,
            dec!(0)
        );
    }

    /// The original EEG 2023 §51 Abs. 2 exempts at **400 kW**; the
    /// post-Solarspitzengesetz 100 kW would reduce plants the statute exempts.
    #[test]
    fn the_original_eeg2023_exemption_is_400_kw() {
        let regime = NegativpreisRegime::fuer_inbetriebnahme(date!(2024 - 03 - 01));
        assert!(regime.ist_befreit(Some(dec!(399)), None, true, false));
        assert!(!regime.ist_befreit(Some(dec!(400)), None, false, false));
    }

    #[test]
    fn solarspitzen_exemptions_are_layered() {
        let r = NegativpreisRegime::Solarspitzen;
        // < 2 kW: exempt until the §85 Abs. 2 Nr. 12 Festlegung, iMSys or not.
        assert!(r.ist_befreit(Some(dec!(1.5)), None, true, false));
        // < 100 kW without iMSys: exempt.
        assert!(r.ist_befreit(Some(dec!(30)), None, false, false));
        // < 100 kW with iMSys: the exemption lapses.
        assert!(!r.ist_befreit(Some(dec!(30)), None, true, false));
        // ≥ 100 kW: never exempt.
        assert!(!r.ist_befreit(Some(dec!(100)), None, false, false));
    }

    #[test]
    fn eeg2017_keeps_the_wind_carve_out() {
        let r = NegativpreisRegime::Eeg2017;
        assert!(r.ist_befreit(
            Some(dec!(2999)),
            Some(ErzeugungsArt::WindOnshore),
            false,
            false
        ));
        assert!(!r.ist_befreit(
            Some(dec!(600)),
            Some(ErzeugungsArt::SolarAufdach),
            false,
            false
        ));
    }

    #[test]
    fn a_pilotwindanlage_is_exempt_under_every_version() {
        for r in [
            NegativpreisRegime::Eeg2017,
            NegativpreisRegime::Eeg2021,
            NegativpreisRegime::Eeg2023Gestaffelt { stunden: 3 },
            NegativpreisRegime::Solarspitzen,
        ] {
            assert!(
                r.ist_befreit(
                    Some(dec!(5000)),
                    Some(ErzeugungsArt::WindOnshore),
                    true,
                    true
                ),
                "{r:?}"
            );
        }
    }

    /// An unknown capacity must not buy an exemption.
    #[test]
    fn unknown_capacity_is_not_exempt() {
        assert!(!NegativpreisRegime::Solarspitzen.ist_befreit(None, None, false, false));
    }

    /// §100 opt-in — a Bestandsanlage moves to the Solarspitzengesetz regime, but
    /// only from the turn of the year after its iMSys was installed.
    #[test]
    fn the_optin_takes_effect_at_the_year_end_after_the_imsys() {
        assert_eq!(
            optin_wirksam_ab(date!(2026 - 03 - 01), Some(date!(2026 - 09 - 01))),
            Some(date!(2027 - 01 - 01))
        );
        // Declared long after the iMSys: the declaration is the later of the two.
        assert_eq!(
            optin_wirksam_ab(date!(2028 - 05 - 01), Some(date!(2026 - 09 - 01))),
            Some(date!(2029 - 01 - 01))
        );
        // No iMSys, no effective date — the declaration is on file and idle.
        assert_eq!(optin_wirksam_ab(date!(2026 - 03 - 01), None), None);
    }

    #[test]
    fn an_effective_optin_moves_a_bestandsanlage_to_the_new_regime() {
        let ibn = date!(2019 - 06 - 01);
        let ab = Some(date!(2027 - 01 - 01));
        assert_eq!(
            NegativpreisRegime::fuer_periode(ibn, ab, Some(date!(2026 - 12 - 01))),
            NegativpreisRegime::Eeg2017,
            "before the effective date the old regime still governs"
        );
        assert_eq!(
            NegativpreisRegime::fuer_periode(ibn, ab, Some(date!(2027 - 01 - 01))),
            NegativpreisRegime::Solarspitzen
        );
        assert_eq!(
            NegativpreisRegime::fuer_periode(ibn, None, Some(date!(2030 - 01 - 01))),
            NegativpreisRegime::Eeg2017,
            "without a declaration the vintage governs for the whole Förderdauer"
        );
    }

    #[test]
    fn sect51a_extension_predates_solarspitzen_only_for_auction_plants() {
        let staffel = NegativpreisRegime::Eeg2023Gestaffelt { stunden: 3 };
        assert!(staffel.verlaengerungsanspruch(true));
        assert!(!staffel.verlaengerungsanspruch(false));
        assert!(NegativpreisRegime::Solarspitzen.verlaengerungsanspruch(false));
        assert!(!NegativpreisRegime::Eeg2017.verlaengerungsanspruch(true));
    }
}
