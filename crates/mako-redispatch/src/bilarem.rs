//! `BilAReM` — Bilanzieller Ausgleich von Redispatch-Maßnahmen (BK6-23-241).
//!
//! Festlegung BK6-23-241 (Beschluss 07.05.2026) consolidates Redispatch 2.0:
//! Tenor Ziff. 1 puts the ÜNB bilanzieller Ausgleich per §13a Abs. 1a `EnWG` on
//! the `BilAReM` rules **from 01.07.2026**; Ziff. 3 revokes BK6-20-061, Ziff. 4
//! revokes BK6-20-060 (Beschluss S. 3); Ziff. 5 revokes `MaBiS` Anlage 1
//! Kap. 17 with the end of 30.09.2026, its 17.1/17.3 continuing as the „Anlage
//! zur `BilAReM`" from 01.10.2026. That date is `mako-mabis`'s
//! `KAPITEL_17_2_ENDE`, which ends the tägliche AAÜZ — the one Kap.-17 series
//! the Anlage does not carry on.
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
//!   stays with the BKV, who receives Aufwendungsersatz from the NB per §14
//!   Abs. 1b `EnWG` (amount not standardised in `BilAReM`). Until 31.12.2031 a
//!   VNB performs the Ausgleich only as a Festlegung provides (§ 14 Abs. 1 S. 3
//!   `EnWG`; Beschluss Gründe 3.2.2.2, S. 31 f.), and Tenor Ziff. 2 is that
//!   Festlegung: § 13a Abs. 1a S. 1 und 2 `EnWG` applies entsprechend to a VNB
//!   under Kapitel 2.3 — the Planwertmodell.
//!
//! Migration Prognose → Planwert is one-way, per SR, effective only at
//! quarter boundaries with ≥6 months notice (Zuordnungsmitteilung ANB →
//! LF/EIV/BTR). Soll-target: transmission-grid-relevant SR migrated by
//! 01.01.2031; the statutory Prognosemodell window ends 31.12.2031.
//!
//! The EDI@Energy wire formats for `BilAReM` are published by the expert group
//! on **relative** deadlines (no calendar date in the Tenor); this module is
//! deliberately wire-format-free — the seam the formats will plug into.

use time::{Date, Month, macros::date};

/// `BilAReM` rules apply to the ÜNB bilanzieller Ausgleich from this day
/// (BK6-23-241 Tenor Ziff. 1).
pub const BILAREM_WIRKSAM: Date = date!(2026 - 07 - 01);

/// Last day a TR in Pauschal-Abrechnung at the Bekanntmachung of BK6-23-241
/// may stay in it (`BilAReM` Kap. 3.2.1, S. 12: „Diese TR können bis zum
/// 31.12.2028 in der Pauschal-Abrechnung … verbleiben"). The Gründe date the
/// Abschaffung „mit Ablauf des 31.12.2028" (Beschluss Gründe 3.2.3.3.1, S. 49).
///
/// The date ends Pauschal only for a fluctuating TR whose Einspeisung is
/// viertelstundenscharf erfasst: a TR without such a measurement keeps it
/// (Kap. 3.2.1, S. 12), and a nicht-fluktuierende TR in the Prognosemodell
/// keeps it without a date (Kap. 3.3, S. 20). [`abrechnungsverfahren`] applies
/// all three.
pub const PAUSCHAL_ABRECHNUNG_ENDE: Date = date!(2028 - 12 - 31);

/// The Zuordnungsfrist for 2029, the first year without the grandfathered
/// Pauschal-Abrechnung: „wenn nicht der Anlagenbetreiber spätestens bis zum
/// 30.11.2028 die Spitzabrechnung festlegt" (`BilAReM` Kap. 3.2.1, S. 12).
///
/// It is one instance of the yearly rule of the same chapter — a Festlegung
/// „bis zum 30.11. eines Jahres für das folgende Kalenderjahr" (S. 11) — and
/// [`Verfahrensfestlegung::wirksam_ab`] derives every year's Frist from it, so the two
/// cannot come apart.
pub const SPITZ_WAHL_FRIST: Date = date!(2028 - 11 - 30);

/// Soll-target: SR that improve transmission-grid congestion-relief efficiency
/// are to be in the Planwertmodell by this day.
pub const MIGRATION_SOLL_ZIEL: Date = date!(2031 - 01 - 01);

/// Last day of the statutory window in which a VNB performs the bilanzieller
/// Ausgleich only as a Festlegung provides (§ 14 Abs. 1 S. 3 `EnWG`, read by
/// Beschluss Gründe 3.2.2.2, S. 31 f.). Tenor Ziff. 2 makes § 13a Abs. 1a S. 1
/// und 2 `EnWG` „bis zum Ablauf des 31.12.2031" entsprechend applicable to a
/// VNB under Kapitel 2.3 (Beschluss S. 3). What follows the window the
/// Beschluss does not decide: the Beschlusskammer re-assesses the question when
/// the Übergangsfrist ends (Gründe, S. 43).
pub const PROGNOSEMODELL_ENDE: Date = date!(2031 - 12 - 31);

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
    /// Vereinfachte Spitzabrechnung — fluctuating TR only (Kap. 3.2.1).
    VereinfachteSpitz,
    /// Pauschal-Abrechnung (Kap. 3.2.1, Kap. 3.3).
    Pauschal,
}

/// Whether a TR's generation follows the weather (`BilAReM` Kap. 3.2) or not
/// (Kap. 3.3). The two chapters admit different Abrechnungsvarianten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Erzeugungsart {
    /// Wind, Solar — Kap. 3.2: Spitz, vereinfachte Spitz, Pauschal.
    Fluktuierend,
    /// Kap. 3.3: Spitz and Pauschal; there is no vereinfachte Spitzabrechnung.
    NichtFluktuierend,
}

/// Whether a fluctuating TR's Einspeisung is erfasst viertelstundenscharf in
/// the sense of `BilAReM` Kap. 3.2.1 (S. 12): measured so, **or** obliged to be
/// equipped with a viertelstundenscharfe Messung or a technische Einrichtung
/// with which the NB can call up the Ist-Einspeisung.
///
/// Without either the Korrekturfaktor cannot be formed, so Spitz and
/// vereinfachte Spitzabrechnung are impossible and the Pauschal-Abrechnung
/// applies „bis eine viertelstundenscharfe Messung vorgenommen wird"
/// (Beschluss Gründe 3.2.3.3.1, S. 49).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Viertelstundenerfassung {
    /// Neither measured nor obliged to be.
    Keine,
    /// Since this day — the first day on which the TR no longer met the
    /// Pauschal condition.
    Seit(Date),
}

/// The facts `BilAReM` Kap. 3.2.1 and 3.3 decide a TR's Abrechnungsvariante
/// from. Every one is an input; none has a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrMerkmale {
    /// Fluctuating or not.
    pub erzeugung: Erzeugungsart,
    /// The SR's Bilanzierungsmodell.
    pub modell: Bilanzierungsmodell,
    /// The TR was in Pauschal-Abrechnung (BK6-20-059 Anlage 1 Kap. 3.2.1) at
    /// the Bekanntmachung of BK6-23-241 — the only TR that may choose it
    /// while viertelstundenscharf erfasst (Kap. 3.2.1, S. 12).
    pub pauschal_bei_bekanntmachung: bool,
    /// Viertelstundenscharfe Erfassung; read for fluctuating TR only.
    pub viertelstundenerfassung: Viertelstundenerfassung,
    /// Geeignete Wetterdaten are measured at the TR. „Die Anwendung der
    /// vereinfachten Spitzabrechnung setzt voraus, dass an der TR keine
    /// geeigneten Wetterdaten gemessen werden" (Kap. 3.2.1, S. 12), so such a
    /// TR is settled by Spitzabrechnung. Read for fluctuating TR only.
    pub wetterdaten_an_der_tr_gemessen: bool,
}

/// The Anlagenbetreiber's Festlegung of an Abrechnungsvariante for one TR
/// (`BilAReM` Kap. 3.2.1, S. 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "art", deny_unknown_fields)]
pub enum Verfahrensfestlegung {
    /// The yearly Festlegung „bis zum 30.11. eines Jahres für das folgende
    /// Kalenderjahr".
    Jaehrlich {
        /// The chosen variant.
        verfahren: Abrechnungsverfahren,
        /// The day the Anlagenbetreiber made it.
        festgelegt_am: Date,
    },
    /// The „initiale Zuordnung zum Monatsbeginn" of a new or wesentlich
    /// geänderte TR, or on a change of Anlagenbetreiber or BKV.
    Initial {
        /// The chosen variant.
        verfahren: Abrechnungsverfahren,
        /// The Monatsbeginn it takes effect on.
        ab: Date,
    },
}

impl Verfahrensfestlegung {
    /// The chosen variant.
    #[must_use]
    pub const fn verfahren(self) -> Abrechnungsverfahren {
        match self {
            Self::Jaehrlich { verfahren, .. } | Self::Initial { verfahren, .. } => verfahren,
        }
    }

    /// The first day the Festlegung governs.
    ///
    /// A yearly one made by the 30.11. applies from the next 01.01.; one made
    /// later misses that year and applies from the 01.01. after it.
    ///
    /// # Errors
    ///
    /// [`VerfahrenError::KeinMonatsbeginn`] for an initial Zuordnung that is
    /// not dated on the first of a month.
    pub fn wirksam_ab(self) -> Result<Date, VerfahrenError> {
        match self {
            Self::Jaehrlich { festgelegt_am, .. } => {
                let jahr = festgelegt_am.year();
                let folgejahr = if festgelegt_am <= zuordnungsfrist(jahr + 1) {
                    jahr + 1
                } else {
                    jahr + 2
                };
                Ok(neujahr(folgejahr))
            }
            Self::Initial { ab, .. } if ab.day() == 1 => Ok(ab),
            Self::Initial { ab, .. } => Err(VerfahrenError::KeinMonatsbeginn(ab)),
        }
    }
}

/// The last day a yearly Festlegung for `jahr` may be made: the 30.11. of the
/// year before, the day [`SPITZ_WAHL_FRIST`] names for 2029.
fn zuordnungsfrist(jahr: i32) -> Date {
    Date::from_calendar_date(jahr - 1, SPITZ_WAHL_FRIST.month(), SPITZ_WAHL_FRIST.day())
        .unwrap_or(SPITZ_WAHL_FRIST)
}

fn neujahr(jahr: i32) -> Date {
    Date::from_calendar_date(jahr, Month::January, 1).unwrap_or(Date::MAX)
}

/// Last day a fluctuating TR stays in the Pauschal-Abrechnung after its
/// Einspeisung became viertelstundenscharf erfasst on `seit`.
///
/// `BilAReM` Kap. 3.2.1, S. 12: „Bei Wegfall dieser Voraussetzungen wechselt
/// die TR mit einer Frist von drei Monaten zum Ablauf des nächsten 31.12. in
/// die vereinfachte Spitzabrechnung". The three months
/// ([`crate::ausfallarbeit::PAUSCHAL_WECHSEL_FRIST_MONATE`]) count from `seit`,
/// that day included, so a TR erfasst from 01.10. switches at the end of the
/// same 31.12. and one erfasst from 02.10. a year later.
#[must_use]
pub fn pauschal_wechsel_ende(seit: Date) -> Date {
    let fristende = mako_fristen::add_months(
        seit,
        u32::from(crate::ausfallarbeit::PAUSCHAL_WECHSEL_FRIST_MONATE),
    )
    .previous_day()
    .unwrap_or(seit);
    Date::from_calendar_date(fristende.year(), Month::December, 31).unwrap_or(Date::MAX)
}

/// Why no Abrechnungsvariante can be decided for a TR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerfahrenError {
    /// The Maßnahme lies before Kapitel 3 of the `BilAReM` applies: Tenor
    /// Ziff. 1 takes effect „ab dem 01.07.2026" and lifts BK6-20-059 Tenorziffer
    /// 1 with the end of 30.06.2026 (Beschluss S. 3), so an earlier Maßnahme
    /// settles under BK6-20-059, which this module does not implement.
    #[error("Maßnahme am {0} liegt vor dem 01.07.2026 — BK6-20-059, nicht BilAReM Kap. 3")]
    VorBilarem(Date),
    /// An initial Zuordnung takes effect „zum Monatsbeginn" only.
    #[error("initiale Zuordnung zum {0} ist kein Monatsbeginn (BilAReM Kap. 3.2.1)")]
    KeinMonatsbeginn(Date),
    /// The Anlagenbetreiber's Festlegung names a variant the TR may not use.
    #[error("{verfahren:?} ist für diese TR am {tag} nicht zulässig (BilAReM Kap. 3.2.1 / 3.3)")]
    NichtZulaessig {
        /// The variant festgelegt.
        verfahren: Abrechnungsverfahren,
        /// The day it would apply.
        tag: Date,
    },
    /// No Festlegung stands and the default cannot apply: Kap. 3.2.1 sends a
    /// TR without one to the vereinfachte Spitzabrechnung, which a TR with
    /// geeigneten Wetterdaten measured at it may not use. The Spitzabrechnung
    /// it can use is the Anlagenbetreiber's to festlegen.
    #[error(
        "keine Abrechnungsvariante am {tag}: ohne Festlegung gilt die vereinfachte \
         Spitzabrechnung, die an der TR gemessene Wetterdaten ausschließen — die \
         Spitzabrechnung ist festzulegen (BilAReM Kap. 3.2.1)"
    )]
    KeineZuordnung {
        /// The day settled.
        tag: Date,
    },
    /// A fluctuating TR in the Planwertmodell without viertelstundenscharfe
    /// Erfassung: Kap. 3.2.1 puts a Planwertmodell TR into Spitz or vereinfachte
    /// Spitzabrechnung, and neither can be formed without the Korrekturfaktor
    /// such an Erfassung supplies (Beschluss Gründe 3.2.3.3.1, S. 49).
    #[error(
        "TR im Planwertmodell ohne viertelstundenscharfe Erfassung: weder Spitz- noch \
         vereinfachte Spitzabrechnung bestimmbar (BilAReM Kap. 3.2.1)"
    )]
    PlanwertOhneViertelstundenerfassung,
}

impl Abrechnungsverfahren {
    /// Whether this variant may apply to a TR with `merkmale` on `tag`.
    ///
    /// - **Nicht-fluktuierend** (Kap. 3.3, S. 20): Spitz always; Pauschal in
    ///   the Prognosemodell; never the vereinfachte Spitzabrechnung.
    /// - **Fluktuierend, not viertelstundenscharf erfasst** (Kap. 3.2.1, S. 12,
    ///   until [`pauschal_wechsel_ende`]): Pauschal alone, in the
    ///   Prognosemodell.
    /// - **Fluktuierend, erfasst**: Spitz; vereinfachte Spitz only where no
    ///   geeigneten Wetterdaten are measured at the TR; Pauschal only for a TR
    ///   in it at the Bekanntmachung, in the Prognosemodell, up to
    ///   [`PAUSCHAL_ABRECHNUNG_ENDE`].
    #[must_use]
    pub fn admissible(self, tag: Date, merkmale: &TrMerkmale) -> bool {
        let prognose = merkmale.modell == Bilanzierungsmodell::Prognosemodell;
        match merkmale.erzeugung {
            Erzeugungsart::NichtFluktuierend => match self {
                Self::Spitz => true,
                Self::Pauschal => prognose,
                Self::VereinfachteSpitz => false,
            },
            Erzeugungsart::Fluktuierend if !viertelstundenscharf(*merkmale, tag) => {
                self == Self::Pauschal && prognose
            }
            Erzeugungsart::Fluktuierend => match self {
                Self::Spitz => true,
                Self::VereinfachteSpitz => !merkmale.wetterdaten_an_der_tr_gemessen,
                Self::Pauschal => {
                    merkmale.pauschal_bei_bekanntmachung
                        && tag <= PAUSCHAL_ABRECHNUNG_ENDE
                        && prognose
                }
            },
        }
    }

    /// The variant that applies when no Festlegung stands: the vereinfachte
    /// Spitzabrechnung (Kap. 3.2.1, S. 11 f.) — for a TR whose grandfathered
    /// Pauschal-Abrechnung has ended, one that left the metering Pauschal, and
    /// one that never chose.
    #[must_use]
    pub const fn post_pauschal_default() -> Self {
        Self::VereinfachteSpitz
    }
}

/// Whether a fluctuating TR counts as viertelstundenscharf erfasst on `tag` —
/// after the switch its Erfassung triggers, not on the day it began.
fn viertelstundenscharf(merkmale: TrMerkmale, tag: Date) -> bool {
    match merkmale.viertelstundenerfassung {
        Viertelstundenerfassung::Keine => false,
        Viertelstundenerfassung::Seit(seit) => tag > pauschal_wechsel_ende(seit),
    }
}

/// The Abrechnungsvariante of a TR on `tag` (`BilAReM` Kap. 3.2.1, S. 11 f.;
/// Kap. 3.3, S. 20).
///
/// `festlegung` is the Anlagenbetreiber's latest Festlegung, if any; it
/// governs from [`Verfahrensfestlegung::wirksam_ab`]. Before that, and without one, the
/// Festlegung's own defaults apply: the metering Pauschal for a fluctuating TR
/// not viertelstundenscharf erfasst, the grandfathered Pauschal up to
/// [`PAUSCHAL_ABRECHNUNG_ENDE`], then [`Abrechnungsverfahren::post_pauschal_default`];
/// for a nicht-fluktuierende TR Pauschal in the Prognosemodell and Spitz in the
/// Planwertmodell.
///
/// # Errors
///
/// - [`VerfahrenError::VorBilarem`] before [`BILAREM_WIRKSAM`].
/// - [`VerfahrenError::NichtZulaessig`] when the Festlegung in force names a
///   variant [`Abrechnungsverfahren::admissible`] refuses.
/// - [`VerfahrenError::PlanwertOhneViertelstundenerfassung`] for a fluctuating
///   Planwertmodell TR no variant can be formed for.
/// - [`VerfahrenError::KeineZuordnung`] for a fluctuating TR with Wetterdaten
///   measured at it and no Spitz Festlegung, once Pauschal is closed to it.
/// - [`VerfahrenError::KeinMonatsbeginn`] from the Festlegung.
pub fn abrechnungsverfahren(
    merkmale: &TrMerkmale,
    festlegung: Option<Verfahrensfestlegung>,
    tag: Date,
) -> Result<Abrechnungsverfahren, VerfahrenError> {
    use Abrechnungsverfahren as A;
    if tag < BILAREM_WIRKSAM {
        return Err(VerfahrenError::VorBilarem(tag));
    }
    let prognose = merkmale.modell == Bilanzierungsmodell::Prognosemodell;
    if merkmale.erzeugung == Erzeugungsart::Fluktuierend && !viertelstundenscharf(*merkmale, tag) {
        // Kap. 3.2.1: the Pauschal-Abrechnung „findet … Anwendung" — no choice.
        return if prognose {
            Ok(A::Pauschal)
        } else {
            Err(VerfahrenError::PlanwertOhneViertelstundenerfassung)
        };
    }
    let in_kraft = match festlegung {
        Some(f) if f.wirksam_ab()? <= tag => Some(f.verfahren()),
        _ => None,
    };
    if let Some(verfahren) = in_kraft {
        return if verfahren.admissible(tag, merkmale) {
            Ok(verfahren)
        } else {
            Err(VerfahrenError::NichtZulaessig { verfahren, tag })
        };
    }
    let default = A::post_pauschal_default();
    if merkmale.erzeugung == Erzeugungsart::Fluktuierend
        && !A::Pauschal.admissible(tag, merkmale)
        && !default.admissible(tag, merkmale)
    {
        return Err(VerfahrenError::KeineZuordnung { tag });
    }
    Ok(match merkmale.erzeugung {
        Erzeugungsart::NichtFluktuierend if prognose => A::Pauschal,
        Erzeugungsart::NichtFluktuierend => A::Spitz,
        Erzeugungsart::Fluktuierend if A::Pauschal.admissible(tag, merkmale) => A::Pauschal,
        Erzeugungsart::Fluktuierend => default,
    })
}

/// Errors validating a model migration (Zuordnungsmitteilung).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// Effective date is not a quarter boundary (01.01/01.04/01.07/01.10).
    #[error("Wirksamkeitsdatum {0} ist kein Quartalsbeginn (01.01/01.04/01.07/01.10)")]
    KeinQuartalsbeginn(Date),
    /// Less than [`crate::fristen::PLANWERT_UEBERFUEHRUNG_VORLAUF_MONATE`]
    /// months between notice and effective date.
    #[error(
        "Ankündigungsfrist unterschritten: {notice} → {effective} \
         (< {} Monate, BilAReM Kap. 2.3.2)",
        crate::fristen::PLANWERT_UEBERFUEHRUNG_VORLAUF_MONATE
    )]
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
    pub fn validate(&self, current: Bilanzierungsmodell) -> Result<(), MigrationError> {
        if current == Bilanzierungsmodell::Planwertmodell {
            return Err(MigrationError::KeinWegZurueck);
        }
        let d = self.wirksam_ab;
        if !crate::fristen::ist_wirksamkeitstermin(d) {
            return Err(MigrationError::KeinQuartalsbeginn(d));
        }
        // Kap. 2.3.2: „spätestens sechs Monate vor der Wirksamkeit".
        let earliest = mako_fristen::add_months(
            self.mitteilungsdatum,
            crate::fristen::PLANWERT_UEBERFUEHRUNG_VORLAUF_MONATE,
        );
        if d < earliest {
            return Err(MigrationError::FristUnterschritten {
                notice: self.mitteilungsdatum,
                effective: d,
            });
        }
        Ok(())
    }
}

// ── Abstimmung der Ausfallarbeit (Kap. 6.4.3) ───────────────────────────────

/// Why an Ausfallarbeits-Abstimmung may not be started or continued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AbstimmungError {
    /// The window closed. Kap. 6.4.3: „Danach dürfen die Prozesse zur
    /// Abstimmung der Ausfallarbeit **nicht erneut gestartet** werden."
    #[error(
        "Abstimmungsfenster geschlossen: Maßnahme endete {massnahme_ende}, \
         Frist lief am {frist} ab (BilAReM Kap. 6.4.3)"
    )]
    FensterGeschlossen {
        /// Day the Redispatch-Maßnahme ended.
        massnahme_ende: Date,
        /// Last day the Abstimmung could run.
        frist: Date,
    },
}

/// Whether the Ausfallarbeit of a Maßnahme ending on `massnahme_ende` may still
/// be adjusted on `heute` (`BilAReM` Kap. 6.4.3).
///
/// The window is a **hard stop**, not a target: once the end of the third
/// following month has passed, the figure that stands is either the agreed one
/// or the formally established Dissens, and neither side may reopen it. A
/// system that keeps accepting corrections afterwards produces settlements the
/// counterparty is entitled to refuse.
///
/// # Errors
///
/// [`AbstimmungError::FensterGeschlossen`], naming both dates.
pub fn abstimmung_zulaessig(massnahme_ende: Date, heute: Date) -> Result<(), AbstimmungError> {
    let frist = crate::fristen::ausfallarbeit_endet_am(massnahme_ende);
    if heute > frist {
        return Err(AbstimmungError::FensterGeschlossen {
            massnahme_ende,
            frist,
        });
    }
    Ok(())
}

// ── Zuordnung einer neu eingerichteten SR (Kap. 2.3.2) ──────────────────────

/// Latest day the ANB may notify the Bilanzierungsmodell of a **newly created**
/// SR (`BilAReM` Kap. 2.3.2).
///
/// Two cases, and the second is the one that is easy to miss:
///
/// - The BTR or EIV gave the ANB everything it needed at least ten Werktage
///   before the planned Inbetriebnahme → the notice is due **five Werktage
///   before** that date.
/// - They did not → the notice is due **five Werktage after** the information
///   was complete, which can fall *after* the Inbetriebnahme. Late information
///   moves the ANB's deadline; it does not remove it.
///
/// The Zuordnung takes effect with the Inbetriebnahme of the first TR assigned
/// to the SR, regardless of which case applied.
#[must_use]
pub fn neue_sr_mitteilung_spaetestens(
    geplante_inbetriebnahme: Date,
    information_vollstaendig_am: Date,
    kalender: mako_fristen::HolidayCalendar,
) -> Date {
    let rechtzeitig = mako_fristen::sub_werktage(
        geplante_inbetriebnahme,
        crate::fristen::PLANWERT_NEUE_SR_INFORMATION_WERKTAGE,
        kalender,
    );
    if information_vollstaendig_am <= rechtzeitig {
        mako_fristen::sub_werktage(
            geplante_inbetriebnahme,
            crate::fristen::PLANWERT_NEUE_SR_MITTEILUNG_WERKTAGE,
            kalender,
        )
    } else {
        mako_fristen::add_werktage(
            information_vollstaendig_am,
            crate::fristen::PLANWERT_NEUE_SR_MITTEILUNG_WERKTAGE,
            kalender,
        )
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

    fn tr(
        erzeugung: Erzeugungsart,
        modell: Bilanzierungsmodell,
        bestand: bool,
        erfassung: Viertelstundenerfassung,
    ) -> TrMerkmale {
        TrMerkmale {
            erzeugung,
            modell,
            pauschal_bei_bekanntmachung: bestand,
            viertelstundenerfassung: erfassung,
            wetterdaten_an_der_tr_gemessen: false,
        }
    }

    const ERFASST: Viertelstundenerfassung = Viertelstundenerfassung::Seit(date!(2020 - 01 - 01));

    /// Kap. 3.2.1: the 31.12.2028 ends Pauschal for a fluctuating TR that is
    /// viertelstundenscharf erfasst — the gate is that pair, not the date.
    #[test]
    fn the_pauschal_end_reads_fluctuation_and_quarter_hour_metering() {
        use Abrechnungsverfahren as A;
        use Bilanzierungsmodell::Prognosemodell as P;
        let fluktuierend = tr(Erzeugungsart::Fluktuierend, P, true, ERFASST);
        assert!(A::Pauschal.admissible(PAUSCHAL_ABRECHNUNG_ENDE, &fluktuierend));
        assert!(!A::Pauschal.admissible(date!(2029 - 01 - 01), &fluktuierend));

        // Kap. 3.3: nicht-fluktuierend keeps Pauschal in the Prognosemodell.
        let konventionell = tr(Erzeugungsart::NichtFluktuierend, P, true, ERFASST);
        assert!(A::Pauschal.admissible(date!(2029 - 01 - 01), &konventionell));
        assert_eq!(
            abrechnungsverfahren(&konventionell, None, date!(2029 - 01 - 01)),
            Ok(A::Pauschal)
        );

        // Kap. 3.2.1: without viertelstundenscharfe Erfassung Pauschal stays,
        // grandfathered or not, and nothing else is possible.
        let ohne = tr(
            Erzeugungsart::Fluktuierend,
            P,
            false,
            Viertelstundenerfassung::Keine,
        );
        assert!(A::Pauschal.admissible(date!(2031 - 06 - 01), &ohne));
        assert!(!A::VereinfachteSpitz.admissible(date!(2031 - 06 - 01), &ohne));
        assert_eq!(
            abrechnungsverfahren(&ohne, None, date!(2031 - 06 - 01)),
            Ok(A::Pauschal)
        );
    }

    #[test]
    fn pauschal_is_closed_to_new_planwert_and_unmetered_planwert_tr() {
        use Abrechnungsverfahren as A;
        use Bilanzierungsmodell::{Planwertmodell, Prognosemodell};
        let neu = tr(Erzeugungsart::Fluktuierend, Prognosemodell, false, ERFASST);
        assert!(!A::Pauschal.admissible(date!(2027 - 01 - 01), &neu));
        let planwert = tr(Erzeugungsart::Fluktuierend, Planwertmodell, true, ERFASST);
        assert!(!A::Pauschal.admissible(date!(2027 - 01 - 01), &planwert));
        // Kap. 3.3: no vereinfachte Spitzabrechnung for nicht-fluktuierende TR,
        // and the Planwertmodell puts them into Spitz.
        let konv = tr(
            Erzeugungsart::NichtFluktuierend,
            Planwertmodell,
            false,
            ERFASST,
        );
        assert!(!A::VereinfachteSpitz.admissible(date!(2027 - 01 - 01), &konv));
        assert_eq!(
            abrechnungsverfahren(&konv, None, date!(2027 - 01 - 01)),
            Ok(A::Spitz)
        );
        // A fluctuating Planwertmodell TR without Erfassung has no variant.
        let ohne = tr(
            Erzeugungsart::Fluktuierend,
            Planwertmodell,
            false,
            Viertelstundenerfassung::Keine,
        );
        assert_eq!(
            abrechnungsverfahren(&ohne, None, date!(2027 - 01 - 01)),
            Err(VerfahrenError::PlanwertOhneViertelstundenerfassung)
        );
    }

    /// Kap. 3.2.1: no Spitz Festlegung by 30.11.2028 → vereinfachte
    /// Spitzabrechnung from 01.01.2029; one made on that day → Spitz.
    #[test]
    fn the_2029_default_reads_the_election_deadline() {
        use Abrechnungsverfahren as A;
        let bestand = tr(
            Erzeugungsart::Fluktuierend,
            Bilanzierungsmodell::Prognosemodell,
            true,
            ERFASST,
        );
        let neujahr = date!(2029 - 01 - 01);
        assert_eq!(
            abrechnungsverfahren(&bestand, None, PAUSCHAL_ABRECHNUNG_ENDE),
            Ok(A::Pauschal)
        );
        assert_eq!(
            abrechnungsverfahren(&bestand, None, neujahr),
            Ok(A::VereinfachteSpitz)
        );
        assert_eq!(A::post_pauschal_default(), A::VereinfachteSpitz);

        let rechtzeitig = Verfahrensfestlegung::Jaehrlich {
            verfahren: A::Spitz,
            festgelegt_am: SPITZ_WAHL_FRIST,
        };
        assert_eq!(
            abrechnungsverfahren(&bestand, Some(rechtzeitig), neujahr),
            Ok(A::Spitz)
        );
        // Still Pauschal until it takes effect.
        assert_eq!(
            abrechnungsverfahren(&bestand, Some(rechtzeitig), PAUSCHAL_ABRECHNUNG_ENDE),
            Ok(A::Pauschal)
        );

        let verspaetet = Verfahrensfestlegung::Jaehrlich {
            verfahren: A::Spitz,
            festgelegt_am: date!(2028 - 12 - 01),
        };
        assert_eq!(verspaetet.wirksam_ab(), Ok(date!(2030 - 01 - 01)));
        assert_eq!(
            abrechnungsverfahren(&bestand, Some(verspaetet), neujahr),
            Ok(A::VereinfachteSpitz)
        );
        assert_eq!(
            abrechnungsverfahren(&bestand, Some(verspaetet), date!(2030 - 01 - 01)),
            Ok(A::Spitz)
        );

        // A Pauschal Festlegung cannot outlive the grandfathering.
        let pauschal = Verfahrensfestlegung::Jaehrlich {
            verfahren: A::Pauschal,
            festgelegt_am: date!(2028 - 06 - 01),
        };
        assert_eq!(
            abrechnungsverfahren(&bestand, Some(pauschal), neujahr),
            Err(VerfahrenError::NichtZulaessig {
                verfahren: A::Pauschal,
                tag: neujahr
            })
        );
    }

    /// Kap. 3.2.1: a TR newly erfasst switches „mit einer Frist von drei
    /// Monaten zum Ablauf des nächsten 31.12."; under three months moves the
    /// switch to the 31.12. after.
    #[test]
    fn the_metering_switch_takes_three_months_to_the_next_new_year() {
        use Abrechnungsverfahren as A;
        assert_eq!(
            pauschal_wechsel_ende(date!(2029 - 10 - 01)),
            date!(2029 - 12 - 31)
        );
        assert_eq!(
            pauschal_wechsel_ende(date!(2029 - 10 - 02)),
            date!(2030 - 12 - 31)
        );

        let neu_erfasst = tr(
            Erzeugungsart::Fluktuierend,
            Bilanzierungsmodell::Prognosemodell,
            false,
            Viertelstundenerfassung::Seit(date!(2029 - 10 - 15)),
        );
        assert_eq!(
            abrechnungsverfahren(&neu_erfasst, None, date!(2030 - 12 - 31)),
            Ok(A::Pauschal)
        );
        assert_eq!(
            abrechnungsverfahren(&neu_erfasst, None, date!(2031 - 01 - 01)),
            Ok(A::VereinfachteSpitz)
        );
        let spitz = Verfahrensfestlegung::Jaehrlich {
            verfahren: A::Spitz,
            festgelegt_am: date!(2030 - 11 - 30),
        };
        assert_eq!(
            abrechnungsverfahren(&neu_erfasst, Some(spitz), date!(2031 - 01 - 01)),
            Ok(A::Spitz)
        );
    }

    /// Kap. 3.2.1: the vereinfachte Spitzabrechnung presupposes that no
    /// geeigneten Wetterdaten are measured at the TR.
    #[test]
    fn measured_weather_at_the_tr_closes_the_vereinfachte_spitzabrechnung() {
        use Abrechnungsverfahren as A;
        let mut m = tr(
            Erzeugungsart::Fluktuierend,
            Bilanzierungsmodell::Prognosemodell,
            false,
            ERFASST,
        );
        m.wetterdaten_an_der_tr_gemessen = true;
        let tag = date!(2027 - 06 - 01);
        assert!(!A::VereinfachteSpitz.admissible(tag, &m));
        assert!(A::Spitz.admissible(tag, &m));
        assert_eq!(
            abrechnungsverfahren(&m, None, tag),
            Err(VerfahrenError::KeineZuordnung { tag })
        );
        let spitz = Verfahrensfestlegung::Initial {
            verfahren: A::Spitz,
            ab: date!(2027 - 01 - 01),
        };
        assert_eq!(abrechnungsverfahren(&m, Some(spitz), tag), Ok(A::Spitz));
        // A grandfathered TR keeps Pauschal to 31.12.2028 regardless.
        m.pauschal_bei_bekanntmachung = true;
        assert_eq!(abrechnungsverfahren(&m, None, tag), Ok(A::Pauschal));
    }

    /// Tenor Ziff. 1: Kapitel 3 settles Maßnahmen from 01.07.2026; the day
    /// before is BK6-20-059's.
    #[test]
    fn kapitel_3_applies_from_the_bilarem_start_day() {
        let m = tr(
            Erzeugungsart::NichtFluktuierend,
            Bilanzierungsmodell::Prognosemodell,
            false,
            ERFASST,
        );
        assert_eq!(
            abrechnungsverfahren(&m, None, BILAREM_WIRKSAM),
            Ok(Abrechnungsverfahren::Pauschal)
        );
        assert_eq!(
            abrechnungsverfahren(&m, None, date!(2026 - 06 - 30)),
            Err(VerfahrenError::VorBilarem(date!(2026 - 06 - 30)))
        );
    }

    #[test]
    fn an_initial_zuordnung_takes_effect_on_a_monatsbeginn_only() {
        use Abrechnungsverfahren as A;
        let ok = Verfahrensfestlegung::Initial {
            verfahren: A::Spitz,
            ab: date!(2027 - 03 - 01),
        };
        assert_eq!(ok.wirksam_ab(), Ok(date!(2027 - 03 - 01)));
        let falsch = Verfahrensfestlegung::Initial {
            verfahren: A::Spitz,
            ab: date!(2027 - 03 - 02),
        };
        assert_eq!(
            falsch.wirksam_ab(),
            Err(VerfahrenError::KeinMonatsbeginn(date!(2027 - 03 - 02)))
        );
    }

    #[test]
    fn the_ausfallarbeit_window_is_a_hard_stop() {
        // Kap. 6.4.3 — a Maßnahme ending in January closes at the end of April.
        let ende = date!(2026 - 01 - 20);
        assert!(abstimmung_zulaessig(ende, date!(2026 - 04 - 30)).is_ok());
        assert!(matches!(
            abstimmung_zulaessig(ende, date!(2026 - 05 - 01)),
            Err(AbstimmungError::FensterGeschlossen { .. })
        ));
    }

    #[test]
    fn late_information_moves_the_new_sr_deadline_instead_of_removing_it() {
        use mako_fristen::HolidayCalendar::BdewMaKo;
        let ibn = date!(2027 - 03 - 15);
        // Information complete well ahead → five Werktage before the IBN.
        let rechtzeitig = neue_sr_mitteilung_spaetestens(ibn, date!(2027 - 01 - 04), BdewMaKo);
        assert_eq!(
            rechtzeitig,
            mako_fristen::sub_werktage(ibn, 5, BdewMaKo),
            "the ordinary case is anchored on the Inbetriebnahme"
        );
        assert!(rechtzeitig < ibn);

        // Information complete only two days before → five Werktage after that,
        // which lands *after* the Inbetriebnahme. The obligation survives.
        let spaet = neue_sr_mitteilung_spaetestens(ibn, date!(2027 - 03 - 13), BdewMaKo);
        assert_eq!(
            spaet,
            mako_fristen::add_werktage(date!(2027 - 03 - 13), 5, BdewMaKo)
        );
        assert!(spaet > ibn);
    }
}
