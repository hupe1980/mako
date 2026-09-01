//! The MaBiS Summenzeitreihen taxonomy — BK6-24-174 Anlage 3 Kap. 2, Tabelle 1.
//!
//! Nearly every rule in MaBiS is keyed on *which* Summenzeitreihe is in play:
//! who aggregates it, who receives it, whether it covers a month or a day,
//! whether it reaches the Bilanzkreisabrechnung at all, whether a Prüfmitteilung
//! and a Datenstatus exist for it, and which Frist from the Fristenkalender
//! applies. Modelling the settlement without that discriminator forces every
//! rule to be stated for "the Summenzeitreihe" in general, which is how a
//! day-scoped Kategorie-C series — for which BDEW sends *neither* a
//! Prüfmitteilung nor a Datenstatus (Kap. 3.8.3) — ends up carrying a
//! settlement obligation it does not have.
//!
//! ```text
//!                          ┌── Kategorie A — NB aggregates, month
//!  BG-SZR / BK-SZR / LF-SZR├── Kategorie B — ÜNB aggregates, month
//!                          └── Kategorie C — ÜNB aggregates, day
//!
//!  DZÜ · NZR · Abrechnungssummenzeitreihe — no Kategorie
//!
//!  AAÜZ · tägliche AAÜZ · LF-AASZR — Kap. 17, no Kategorie
//! ```
//!
//! # The Kapitel-17 series are on a clock
//!
//! Three Ausfallarbeit series come from MaBiS Anlage 1 **Kapitel 17**, which
//! BK6-23-241 Tenorziffer 5 repeals with the end of **30.09.2026**. Kap. 17.1
//! and 17.3 continue from 01.10.2026 as the „Anlage zur BilAReM"; Kap. **17.2**
//! — the tägliche AAÜZ — and Kap. **17.3.2.1** do not. [`Familie::endet_am`]
//! carries that date, so a deployment can refuse to open a settlement for a
//! series that will not exist when the month it covers is due.
//!
//! # Kategorie is not decoration
//!
//! The three Kategorien differ in **who is responsible** and **what period they
//! cover** (Kap. 2): A = NB/Monat, B = ÜNB/Monat, C = ÜNB/Tag. The BG-SZR has no
//! Kategorie A (a Bilanzierungsgebiet is always aggregated by the ÜNB) and the
//! LF-SZR has no Kategorie C. [`Zeitreihe::new`] refuses the combinations
//! Tabelle 1 does not define rather than defaulting to a neighbour.
//!
//! # Source
//!
//! BNetzA **BK6-24-174 Anlage 3 (MaBiS)** Kap. 2 „Zeitreihen, Aggregationen und
//! Kategorien", Tabelle 1 (S. 8–9), and Kap. 3.8.3 for the Prüfmitteilung /
//! Datenstatus carve-out.

use std::fmt;

// ── Kategorie ────────────────────────────────────────────────────────────────

/// Kategorie of a BG-/BK-/LF-Summenzeitreihe (Kap. 2).
///
/// The Kategorie fixes both the aggregating role and the Bezugszeitraum; the
/// two never vary independently.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum Kategorie {
    /// Aggregation durch NB, Bezugszeitraum Monat.
    A,
    /// Aggregation durch ÜNB, Bezugszeitraum Monat.
    B,
    /// Aggregation durch ÜNB, Bezugszeitraum Tag.
    C,
}

impl Kategorie {
    /// The role responsible for aggregation, dispatch and versioning.
    #[must_use]
    pub fn verantwortlich(self) -> Rolle {
        match self {
            Self::A => Rolle::Nb,
            Self::B | Self::C => Rolle::Uenb,
        }
    }

    /// Period one message of this Kategorie covers.
    #[must_use]
    pub fn bezugszeitraum(self) -> Bezugszeitraum {
        match self {
            Self::A | Self::B => Bezugszeitraum::Monat,
            Self::C => Bezugszeitraum::Tag,
        }
    }
}

impl fmt::Display for Kategorie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        })
    }
}

// ── Bezugszeitraum ───────────────────────────────────────────────────────────

/// Period one Summenzeitreihe message covers (Tabelle 1, column *Bezugszeitraum*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bezugszeitraum {
    /// A complete Bilanzierungsmonat.
    Monat,
    /// The complete previous day.
    Tag,
}

// ── Rolle ────────────────────────────────────────────────────────────────────

/// The MaBiS market roles (Kap. 1.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum Rolle {
    /// Bilanzkoordinator.
    Biko,
    /// Bilanzkreisverantwortlicher.
    Bkv,
    /// Lieferant.
    Lf,
    /// Netzbetreiber (Verteilernetzbetreiber).
    Nb,
    /// Übertragungsnetzbetreiber.
    Uenb,
}

impl fmt::Display for Rolle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Biko => "BIKO",
            Self::Bkv => "BKV",
            Self::Lf => "LF",
            Self::Nb => "NB",
            Self::Uenb => "ÜNB",
        })
    }
}

// ── Zeitreihenfamilie ────────────────────────────────────────────────────────

/// The six Summenzeitreihen families of Tabelle 1, before the Kategorie split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Familie {
    /// Bilanzierungsgebietssummenzeitreihe (BG-SZR) — Kategorie B or C.
    BgSzr,
    /// Bilanzkreissummenzeitreihe (BK-SZR) — Kategorie A, B or C.
    BkSzr,
    /// Lieferantensummenzeitreihe (LF-SZR) — Kategorie A or B.
    LfSzr,
    /// Deltazeitreihenübertrag (DZÜ) — ÜNB → NB, BIKO; monthly; no Kategorie.
    Dzue,
    /// Netzzeitreihe (NZR) — NB → NB, BIKO; monthly; no Kategorie.
    Nzr,
    /// Abrechnungssummenzeitreihe — BIKO → ÜNB, BKV, NB; monthly; no Kategorie.
    Abrechnungssummenzeitreihe,
    /// Monatliche Ausfallarbeitsüberführungszeitreihe (AAÜZ) — NB (ANB) → BIKO,
    /// weitergeleitet an den BKV. MaBiS Kap. 17.3.3 / 17.3.5.
    Aauez,
    /// Tägliche Ausfallarbeitsüberführungszeitreihe — NB (ANB) → ÜNB.
    ///
    /// MaBiS Kap. **17.2**, repealed with the end of 30.09.2026 and not
    /// republished as the Anlage zur BilAReM.
    TaeglicheAauez,
    /// Lieferantenausfallarbeitssummenzeitreihe (LF-AASZR) — NB (ANB) → LF.
    /// MaBiS Kap. 17.3.2.
    LfAaszr,
}

impl Familie {
    /// Whether Tabelle 1 splits this family by Kategorie.
    #[must_use]
    pub fn hat_kategorie(self) -> bool {
        matches!(self, Self::BgSzr | Self::BkSzr | Self::LfSzr)
    }

    /// The Kategorien Tabelle 1 defines for this family (empty if it has none).
    #[must_use]
    pub fn kategorien(self) -> &'static [Kategorie] {
        match self {
            Self::BgSzr => &[Kategorie::B, Kategorie::C],
            Self::BkSzr => &[Kategorie::A, Kategorie::B, Kategorie::C],
            Self::LfSzr => &[Kategorie::A, Kategorie::B],
            _ => &[],
        }
    }

    /// Canonical BDEW abbreviation.
    #[must_use]
    pub fn kuerzel(self) -> &'static str {
        match self {
            Self::BgSzr => "BG-SZR",
            Self::BkSzr => "BK-SZR",
            Self::LfSzr => "LF-SZR",
            Self::Dzue => "DZÜ",
            Self::Nzr => "NZR",
            Self::Abrechnungssummenzeitreihe => "Abrechnungssummenzeitreihe",
            Self::Aauez => "AAÜZ",
            Self::TaeglicheAauez => "tägliche AAÜZ",
            Self::LfAaszr => "LF-AASZR",
        }
    }

    /// Whether the family comes from MaBiS Anlage 1 Kapitel 17 (Redispatch
    /// Ausfallarbeit) rather than from Tabelle 1.
    #[must_use]
    pub fn stammt_aus_kapitel_17(self) -> bool {
        matches!(self, Self::Aauez | Self::TaeglicheAauez | Self::LfAaszr)
    }

    /// The last day this family exists, where a Festlegung ends it.
    ///
    /// Only the tägliche AAÜZ has one: BK6-23-241 Tenorziffer 5 repeals MaBiS
    /// Anlage 1 Kap. 17.2 with the end of 30.09.2026, and — unlike Kap. 17.1
    /// and 17.3 — it is not republished as the Anlage zur BilAReM.
    #[must_use]
    pub fn endet_am(self) -> Option<time::Date> {
        match self {
            Self::TaeglicheAauez => Some(KAPITEL_17_2_ENDE),
            _ => None,
        }
    }
}

/// Last day MaBiS Anlage 1 Kap. 17.2 (tägliche AAÜZ) exists — BK6-23-241
/// Tenorziffer 5.
pub const KAPITEL_17_2_ENDE: time::Date = time::macros::date!(2026 - 09 - 30);

// ── Zeitreihe ────────────────────────────────────────────────────────────────

/// A Summenzeitreihe of Tabelle 1, with its Kategorie where one exists.
///
/// Construct through [`Zeitreihe::new`], which refuses the family/Kategorie
/// combinations the table does not define.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Zeitreihe {
    familie: Familie,
    kategorie: Option<Kategorie>,
}

/// A family/Kategorie pair Tabelle 1 does not define.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{familie} hat keine Kategorie {kategorie:?} (BK6-24-174 Anlage 3 Tabelle 1)")]
pub struct UnbekannteKategorie {
    /// The family that was asked for.
    pub familie: &'static str,
    /// The Kategorie that does not exist for it (`None` = a Kategorie was
    /// omitted for a family that requires one, or supplied for one that has none).
    pub kategorie: Option<Kategorie>,
}

impl Zeitreihe {
    /// The **Entscheidungsbaum** that decides an inbound instance of this
    /// series, or `None` when checking it is not mako's obligation.
    ///
    /// Which tree applies follows from the series, never from the message that
    /// carried it: an `A02` drawn from `E_0062` and an `A02` drawn from
    /// `E_0041` are unrelated codes, and picking the tree by PID would let a
    /// BG-SZR be answered out of the LF-SZR Codeliste.
    ///
    /// `None` covers the Abrechnungssummenzeitreihe, whose prüfende Rolle is
    /// the BIKO, and the Kategorie C variants, for which BDEW publishes no
    /// tree.
    ///
    /// The **monatliche AAÜZ** is `None` for a different reason: BDEW
    /// publishes two trees for it, `E_0098` and `E_0099`, with identical
    /// titles and identical codes in two parallel chapter clusters, and the
    /// document does not say which leg each serves. Both are catalogued in
    /// `mako-pruefung`, so a received code still resolves; picking one here
    /// would be a guess.
    #[must_use]
    pub fn pruef_ebd(self) -> Option<&'static str> {
        use mako_pruefung::mabis::codes as ebd;
        match (self.familie, self.kategorie) {
            (Familie::LfSzr, Some(Kategorie::A)) => Some(ebd::EBD_LF_SZR_A),
            (Familie::LfSzr, Some(Kategorie::B)) => Some(ebd::EBD_LF_SZR_B),
            (Familie::LfAaszr, _) => Some(ebd::EBD_LF_AASZR),
            (Familie::Nzr, _) => Some(ebd::EBD_NZR),
            (Familie::BgSzr, Some(Kategorie::B)) => Some(ebd::EBD_BG_SZR_B),
            (Familie::BkSzr, Some(Kategorie::A)) => Some(ebd::EBD_BK_SZR_A),
            (Familie::BkSzr, Some(Kategorie::B)) => Some(ebd::EBD_BK_SZR_B),
            (Familie::Dzue, _) => Some(ebd::EBD_DZUE),
            _ => None,
        }
    }

    /// Build a Summenzeitreihe from its family and (where applicable) Kategorie.
    ///
    /// # Errors
    ///
    /// [`UnbekannteKategorie`] when the pair is not in Tabelle 1: a BG-SZR of
    /// Kategorie A, an LF-SZR of Kategorie C, a Kategorie on the DZÜ/NZR/
    /// Abrechnungssummenzeitreihe, or a missing Kategorie on BG-/BK-/LF-SZR.
    pub fn new(
        familie: Familie,
        kategorie: Option<Kategorie>,
    ) -> Result<Self, UnbekannteKategorie> {
        let ok = match (familie.hat_kategorie(), kategorie) {
            (true, Some(k)) => familie.kategorien().contains(&k),
            (false, None) => true,
            _ => false,
        };
        if ok {
            Ok(Self { familie, kategorie })
        } else {
            Err(UnbekannteKategorie {
                familie: familie.kuerzel(),
                kategorie,
            })
        }
    }

    /// The family.
    #[must_use]
    pub fn familie(self) -> Familie {
        self.familie
    }

    /// The Kategorie, where the family has one.
    #[must_use]
    pub fn kategorie(self) -> Option<Kategorie> {
        self.kategorie
    }

    /// Role responsible for aggregating, sending and versioning this series.
    #[must_use]
    pub fn verantwortlich(self) -> Rolle {
        match (self.familie, self.kategorie) {
            (_, Some(k)) => k.verantwortlich(),
            (Familie::Dzue, _) => Rolle::Uenb,
            (Familie::Nzr, _) => Rolle::Nb,
            (Familie::Abrechnungssummenzeitreihe, _) => Rolle::Biko,
            // The Anschlussnetzbetreiber determines the Ausfallarbeit.
            (Familie::Aauez | Familie::TaeglicheAauez | Familie::LfAaszr, _) => Rolle::Nb,
            // Unreachable: `new` rejects a Kategorie-less BG/BK/LF-SZR.
            (Familie::BgSzr | Familie::BkSzr | Familie::LfSzr, None) => Rolle::Uenb,
        }
    }

    /// Roles that receive this series (Tabelle 1, column *Empfänger*).
    #[must_use]
    pub fn empfaenger(self) -> &'static [Rolle] {
        use Familie as F;
        use Kategorie as K;
        match (self.familie, self.kategorie) {
            (F::BgSzr, Some(K::B)) => &[Rolle::Nb, Rolle::Biko],
            (F::BgSzr, _) => &[Rolle::Nb],
            (F::BkSzr, Some(K::A | K::B)) => &[Rolle::Bkv, Rolle::Biko],
            (F::BkSzr, _) => &[Rolle::Bkv],
            (F::LfSzr, _) => &[Rolle::Lf],
            (F::Dzue, _) => &[Rolle::Nb, Rolle::Biko],
            (F::Nzr, _) => &[Rolle::Nb, Rolle::Biko],
            (F::Abrechnungssummenzeitreihe, _) => &[Rolle::Uenb, Rolle::Bkv, Rolle::Nb],
            (F::Aauez, _) => &[Rolle::Biko, Rolle::Bkv],
            (F::TaeglicheAauez, _) => &[Rolle::Uenb],
            (F::LfAaszr, _) => &[Rolle::Lf],
        }
    }

    /// Period one message of this series covers.
    #[must_use]
    pub fn bezugszeitraum(self) -> Bezugszeitraum {
        self.kategorie
            .map_or(Bezugszeitraum::Monat, Kategorie::bezugszeitraum)
    }

    /// Whether the series is *für BKA abrechnungsrelevant* (Tabelle 1, last column).
    ///
    /// The LF-SZR is never settlement-relevant — it exists so the Lieferant can
    /// reconcile — and neither is any Kategorie-C series.
    #[must_use]
    pub fn abrechnungsrelevant(self) -> bool {
        use Familie as F;
        match self.familie {
            F::LfSzr => false,
            F::BgSzr | F::BkSzr => self.kategorie != Some(Kategorie::C),
            F::Dzue | F::Nzr | F::Abrechnungssummenzeitreihe | F::Aauez => true,
            // The tägliche AAÜZ feeds Bilanzkreismonitoring, not the BKA, and
            // the LF-AASZR is the LF's reconciliation copy.
            F::TaeglicheAauez | F::LfAaszr => false,
        }
    }

    /// Whether a Prüfmitteilung and a Datenstatus exist for this series.
    ///
    /// Kap. 3.8.3: „Für die BG-SZR (Kategorie C) und die BK-SZR (Kategorie C)
    /// werden keine Prüfmitteilung und kein Datenstatus versendet."
    ///
    /// The tägliche AAÜZ likewise has neither — Kap. 17.2 is Bilanzkreis-
    /// monitoring, one direction only.
    #[must_use]
    pub fn hat_pruefmitteilung_und_datenstatus(self) -> bool {
        !matches!(
            (self.familie, self.kategorie),
            (Familie::BgSzr | Familie::BkSzr, Some(Kategorie::C))
        ) && self.familie != Familie::TaeglicheAauez
    }

    /// The last day this Summenzeitreihe exists, where a Festlegung ends it.
    #[must_use]
    pub fn endet_am(self) -> Option<time::Date> {
        self.familie.endet_am()
    }

    /// Whether the series still exists on `date`.
    #[must_use]
    pub fn gilt_am(self, date: time::Date) -> bool {
        self.endet_am().is_none_or(|ende| date <= ende)
    }

    /// Tupelobjekte this series aggregates over (Tabelle 1, column *Aggregation*).
    ///
    /// Empty for the NZR and the Abrechnungssummenzeitreihe, which Tabelle 1
    /// marks „–".
    #[must_use]
    pub fn aggregation(self) -> &'static [&'static str] {
        use Familie as F;
        use Kategorie as K;
        match (self.familie, self.kategorie) {
            (F::BgSzr, _) => &["BG", "Spannungsebene", "ZRT"],
            (F::BkSzr, Some(K::A)) => &["BG", "BK", "ZRT"],
            (F::BkSzr, Some(K::B)) => &["BG/RZ", "BK", "ZRT"],
            (F::BkSzr, _) => &["RZ", "BK", "ZRT"],
            (F::LfSzr, Some(K::A)) => &["BG", "BK", "LF", "ZRT"],
            (F::LfSzr, _) => &["BG/RZ", "BK", "LF", "ZRT"],
            (F::Dzue, _) => &["BG"],
            (F::Aauez, _) => &["BK"],
            (F::TaeglicheAauez, _) => &["BG"],
            (F::LfAaszr, _) => &["BG", "BK", "LF"],
            (F::Nzr | F::Abrechnungssummenzeitreihe, _) => &[],
        }
    }

    /// Whether the BKV may choose the Aggregationsebene (BG or RZ) for this
    /// series — only the BK-SZR (Kategorie B) offers that choice (Kap. 3.8.3).
    ///
    /// Even when the RZ level is subscribed, only the **BG level** of the
    /// BK-SZR (Kategorie B) is settlement-relevant.
    #[must_use]
    pub fn hat_waehlbare_aggregationsebene(self) -> bool {
        self.familie == Familie::BkSzr && self.kategorie == Some(Kategorie::B)
    }
}

impl fmt::Display for Zeitreihe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kategorie {
            Some(k) => write!(f, "{} (Kategorie {k})", self.familie.kuerzel()),
            None => f.write_str(self.familie.kuerzel()),
        }
    }
}

/// Every Summenzeitreihe of Tabelle 1 and Kapitel 17, in table order.
///
/// The list is the single source of truth for exhaustiveness tests; adding a
/// row means adding it here.
///
/// # Panics
///
/// Never: every pair it builds comes from [`Familie::kategorien`], so
/// [`Zeitreihe::new`] cannot reject one. A panic here would mean the family
/// table and the constructor disagree, which is a bug rather than an input
/// error.
#[must_use]
pub fn alle() -> Vec<Zeitreihe> {
    let mut out = Vec::new();
    for familie in [
        Familie::BgSzr,
        Familie::BkSzr,
        Familie::LfSzr,
        Familie::Dzue,
        Familie::Nzr,
        Familie::Abrechnungssummenzeitreihe,
        Familie::Aauez,
        Familie::TaeglicheAauez,
        Familie::LfAaszr,
    ] {
        if familie.hat_kategorie() {
            for &k in familie.kategorien() {
                out.push(Zeitreihe::new(familie, Some(k)).expect("row from Familie::kategorien"));
            }
        } else {
            out.push(Zeitreihe::new(familie, None).expect("row from Familie::kategorien"));
        }
    }
    out
}

// ── Aggregationsebene ────────────────────────────────────────────────────────

/// Level a BK-SZR (Kategorie B) or LF-SZR (Kategorie B) is aggregated at.
///
/// The BKV chooses it per Bilanzkreis (Kap. 3.8.3) and it is carried on the
/// wire, because the two levels are different messages with different
/// Datenstatus behaviour — **only the BG level is settlement-relevant**,
/// „unabhängig von der gewählten Aggregationsebene".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregationsebene {
    /// Ebene Bilanzierungsgebiet — the level that settles.
    Bilanzierungsgebiet,
    /// Ebene Regelzone — subscribed by the BKV; a first negative Prüfmitteilung
    /// in a Bilanzierungsmonat drops that series back to the BG level for the
    /// rest of the month (Kap. 3.8.3).
    Regelzone,
}

// ── CAV codelist ─────────────────────────────────────────────────────────────

/// Which Summenzeitreihe a MaBiS UTILMD is about, as it appears on the wire.
///
/// `SG10 CCI+++ZB4` „Bezeichnung der Summenzeitreihe" with `CAV` DE 7111
/// carrying the code (UTILMD AHB Strom 2.2 Kap. 13.1, Anwendungsübersicht zur
/// Aktivierung/Deaktivierung von MaBiS-ZP).
///
/// **This is why 55062/55063 can be shared by eleven series without ambiguity.**
/// The PID says an activation happened; this code says what was activated. A
/// reader that skips it has to guess, and there are eleven wrong answers.
///
/// | Code | Summenzeitreihe |
/// |------|-----------------|
/// | `Z95` | BG-SZR (Kategorie B) |
/// | `Z96` | BG-SZR (Kategorie C) |
/// | `Z97` | BK-SZR (Kategorie A) |
/// | `Z98` | BK-SZR (Kategorie B) auf Ebene Regelzone |
/// | `Z99` | BK-SZR (Kategorie B) auf Ebene Bilanzierungsgebiet |
/// | `ZA0` | BK-SZR (Kategorie C) |
/// | `ZA1` | LF-SZR (Kategorie A) |
/// | `ZA2` | LF-SZR (Kategorie B) auf Ebene Regelzone |
/// | `ZA3` | LF-SZR (Kategorie B) auf Ebene Bilanzierungsgebiet |
/// | `ZA4` | Deltazeitreihenübertrag (DZÜ) |
/// | `ZA5` | Netzzeitreihe (NZR) |
/// | `ZA6` | Abrechnungssummenzeitreihe |
/// | `ZG7` | BK-SZR (eMob, täglich) |
///
/// Note that the Kategorie-B rows come in **pairs**: one code per
/// Aggregationsebene. The BG and RZ level of the same Bilanzkreis are distinct
/// MaBiS-Zählpunkte with distinct settlement behaviour, so they get distinct
/// codes rather than a shared one plus a flag.
///
/// Returns `None` for `ZG7`: the eMob BK-SZR comes from the AWH „Modell 2
/// ladevorgangsscharfe bilanzielle Energiezuordnung", not from Tabelle 1, and
/// giving it a Tabelle-1 row would put it in Fristen it does not have.
#[must_use]
pub fn zeitreihe_aus_cav(code: &str) -> Option<(Zeitreihe, Option<Aggregationsebene>)> {
    use Aggregationsebene as E;
    use Familie as F;
    use Kategorie as K;
    let (familie, kategorie, ebene) = match code {
        "Z95" => (F::BgSzr, Some(K::B), None),
        "Z96" => (F::BgSzr, Some(K::C), None),
        "Z97" => (F::BkSzr, Some(K::A), None),
        "Z98" => (F::BkSzr, Some(K::B), Some(E::Regelzone)),
        "Z99" => (F::BkSzr, Some(K::B), Some(E::Bilanzierungsgebiet)),
        "ZA0" => (F::BkSzr, Some(K::C), None),
        "ZA1" => (F::LfSzr, Some(K::A), None),
        "ZA2" => (F::LfSzr, Some(K::B), Some(E::Regelzone)),
        "ZA3" => (F::LfSzr, Some(K::B), Some(E::Bilanzierungsgebiet)),
        "ZA4" => (F::Dzue, None, None),
        "ZA5" => (F::Nzr, None, None),
        "ZA6" => (F::Abrechnungssummenzeitreihe, None, None),
        _ => return None,
    };
    Zeitreihe::new(familie, kategorie).ok().map(|z| (z, ebene))
}

/// The `SG10 CAV` DE 7111 code for a Summenzeitreihe.
///
/// `ebene` is required for the Kategorie-B rows of the BK-SZR and LF-SZR and
/// ignored elsewhere; without it those two have no code, because the AHB does
/// not define one that leaves the level open.
#[must_use]
pub fn cav_aus_zeitreihe(
    zeitreihe: Zeitreihe,
    ebene: Option<Aggregationsebene>,
) -> Option<&'static str> {
    use Aggregationsebene as E;
    use Familie as F;
    use Kategorie as K;
    Some(match (zeitreihe.familie(), zeitreihe.kategorie(), ebene) {
        (F::BgSzr, Some(K::B), _) => "Z95",
        (F::BgSzr, Some(K::C), _) => "Z96",
        (F::BkSzr, Some(K::A), _) => "Z97",
        (F::BkSzr, Some(K::B), Some(E::Regelzone)) => "Z98",
        (F::BkSzr, Some(K::B), Some(E::Bilanzierungsgebiet)) => "Z99",
        (F::BkSzr, Some(K::C), _) => "ZA0",
        (F::LfSzr, Some(K::A), _) => "ZA1",
        (F::LfSzr, Some(K::B), Some(E::Regelzone)) => "ZA2",
        (F::LfSzr, Some(K::B), Some(E::Bilanzierungsgebiet)) => "ZA3",
        (F::Dzue, _, _) => "ZA4",
        (F::Nzr, _, _) => "ZA5",
        (F::Abrechnungssummenzeitreihe, _, _) => "ZA6",
        _ => return None,
    })
}

/// `SG10 CCI+++ZB4` — DE 7037 for „Bezeichnung der Summenzeitreihe".
pub const CCI_BEZEICHNUNG_SUMMENZEITREIHE: &str = "ZB4";

/// `SG10 CCI+6` — DE 7059 Klassentyp for „Verantwortlicher".
pub const CCI_KLASSENTYP_VERANTWORTLICHER: &str = "6";

/// The `SG10 CCI+6` DE 7037 code naming the responsible role
/// (UTILMD AHB Strom 2.2 Kap. 13.1).
///
/// | Code | Rolle |
/// |------|-------|
/// | `ZA8` | NB |
/// | `ZA9` | ÜNB |
/// | `ZB7` | BIKO |
#[must_use]
pub fn rolle_aus_cci(code: &str) -> Option<Rolle> {
    Some(match code {
        "ZA8" => Rolle::Nb,
        "ZA9" => Rolle::Uenb,
        "ZB7" => Rolle::Biko,
        _ => return None,
    })
}

/// The `SG10 CCI+6` DE 7037 code for a responsible role.
#[must_use]
pub fn cci_aus_rolle(rolle: Rolle) -> Option<&'static str> {
    Some(match rolle {
        Rolle::Nb => "ZA8",
        Rolle::Uenb => "ZA9",
        Rolle::Biko => "ZB7",
        Rolle::Bkv | Rolle::Lf => return None,
    })
}

// ── Aggregationsverantwortung ────────────────────────────────────────────────

/// Metering equipment of the Messlokationen behind a Marktlokation, as far as
/// Kap. 3.9.1 distinguishes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Messtechnik {
    /// All Messlokationen carry an intelligentes Messsystem.
    NurIms,
    /// All carry a moderne Messeinrichtung.
    NurMme,
    /// All carry a konventionelle Messeinrichtung.
    NurKme,
    /// Mixed equipment across the Messlokationen of one Marktlokation.
    Gemischt,
    /// A pauschale Marktlokation (no metering at all).
    Pauschal,
}

/// Who aggregates the energy of one Marktlokation — Kap. 3.9.1.
///
/// The ÜNB is responsible only for the narrow case where **all three** hold:
/// every Messlokation has an iMS, the MaLo is billed on ¼-h values, and the NB
/// has already handed it over for aggregation. Every other combination stays
/// with the NB, including an iMS MaLo that is not yet ¼-h-billed and one the NB
/// has not yet handed over.
#[must_use]
pub fn aggregationsverantwortung(
    messtechnik: Messtechnik,
    viertelstunden_bilanziert: bool,
    an_uenb_uebertragen: bool,
) -> Rolle {
    if messtechnik == Messtechnik::NurIms && viertelstunden_bilanziert && an_uenb_uebertragen {
        Rolle::Uenb
    } else {
        Rolle::Nb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_has_thirteen_rows() {
        // Tabelle 1: BG-SZR B/C, BK-SZR A/B/C, LF-SZR A/B, DZÜ, NZR,
        // Abrechnungs-SZR = 10. Kapitel 17: AAÜZ, tägliche AAÜZ, LF-AASZR = 3.
        assert_eq!(alle().len(), 13);
        assert_eq!(
            alle()
                .iter()
                .filter(|z| z.familie().stammt_aus_kapitel_17())
                .count(),
            3
        );
    }

    #[test]
    fn only_the_taegliche_aauez_expires() {
        for z in alle() {
            let expected = z.familie() == Familie::TaeglicheAauez;
            assert_eq!(z.endet_am().is_some(), expected, "{z}");
        }
        assert!(
            Zeitreihe::new(Familie::TaeglicheAauez, None)
                .unwrap()
                .gilt_am(KAPITEL_17_2_ENDE)
        );
        assert!(
            !Zeitreihe::new(Familie::TaeglicheAauez, None)
                .unwrap()
                .gilt_am(KAPITEL_17_2_ENDE.next_day().unwrap())
        );
        // Kap. 17.1 and 17.3 continue as the Anlage zur BilAReM.
        assert!(
            Zeitreihe::new(Familie::Aauez, None)
                .unwrap()
                .gilt_am(KAPITEL_17_2_ENDE.next_day().unwrap())
        );
    }

    #[test]
    fn the_taegliche_aauez_carries_neither_pruefmitteilung_nor_datenstatus() {
        assert!(
            !Zeitreihe::new(Familie::TaeglicheAauez, None)
                .unwrap()
                .hat_pruefmitteilung_und_datenstatus()
        );
        assert!(
            Zeitreihe::new(Familie::Aauez, None)
                .unwrap()
                .hat_pruefmitteilung_und_datenstatus()
        );
    }

    #[test]
    fn bg_szr_has_no_kategorie_a() {
        assert!(Zeitreihe::new(Familie::BgSzr, Some(Kategorie::A)).is_err());
        assert!(Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B)).is_ok());
    }

    #[test]
    fn lf_szr_has_no_kategorie_c() {
        assert!(Zeitreihe::new(Familie::LfSzr, Some(Kategorie::C)).is_err());
    }

    #[test]
    fn kategorielose_familien_refuse_a_kategorie() {
        assert!(Zeitreihe::new(Familie::Nzr, Some(Kategorie::A)).is_err());
        assert!(Zeitreihe::new(Familie::Nzr, None).is_ok());
    }

    #[test]
    fn kategorie_szr_requires_a_kategorie() {
        assert!(Zeitreihe::new(Familie::BkSzr, None).is_err());
    }

    #[test]
    fn kategorie_c_is_daily_and_not_settlement_relevant() {
        for familie in [Familie::BgSzr, Familie::BkSzr] {
            let z = Zeitreihe::new(familie, Some(Kategorie::C)).unwrap();
            assert_eq!(z.bezugszeitraum(), Bezugszeitraum::Tag);
            assert!(!z.abrechnungsrelevant());
            // Kap. 3.8.3 — neither a Prüfmitteilung nor a Datenstatus.
            assert!(!z.hat_pruefmitteilung_und_datenstatus());
        }
    }

    #[test]
    fn lf_szr_is_never_settlement_relevant() {
        for &k in Familie::LfSzr.kategorien() {
            assert!(
                !Zeitreihe::new(Familie::LfSzr, Some(k))
                    .unwrap()
                    .abrechnungsrelevant()
            );
        }
    }

    #[test]
    fn verantwortlich_follows_the_kategorie() {
        let a = Zeitreihe::new(Familie::BkSzr, Some(Kategorie::A)).unwrap();
        let b = Zeitreihe::new(Familie::BkSzr, Some(Kategorie::B)).unwrap();
        assert_eq!(a.verantwortlich(), Rolle::Nb);
        assert_eq!(b.verantwortlich(), Rolle::Uenb);
    }

    #[test]
    fn only_bk_szr_kategorie_b_has_a_choosable_aggregationsebene() {
        for z in alle() {
            let expected = z.familie() == Familie::BkSzr && z.kategorie() == Some(Kategorie::B);
            assert_eq!(z.hat_waehlbare_aggregationsebene(), expected, "{z}");
        }
    }

    #[test]
    fn every_tabelle_1_row_round_trips_through_its_cav_code() {
        for z in alle() {
            if z.familie().stammt_aus_kapitel_17() {
                // Kapitel-17 series are activated by their own PIDs
                // (55197–55214), not by 55062/55063, so the AHB gives them no
                // CAV code in this codelist.
                continue;
            }
            let braucht_ebene = z.hat_waehlbare_aggregationsebene()
                || (z.familie() == Familie::LfSzr && z.kategorie() == Some(Kategorie::B));
            let ebenen: &[Option<Aggregationsebene>] = if braucht_ebene {
                &[
                    Some(Aggregationsebene::Bilanzierungsgebiet),
                    Some(Aggregationsebene::Regelzone),
                ]
            } else {
                &[None]
            };
            for &e in ebenen {
                let code = cav_aus_zeitreihe(z, e).unwrap_or_else(|| panic!("{z} / {e:?}"));
                assert_eq!(zeitreihe_aus_cav(code), Some((z, e)), "{code}");
            }
        }
    }

    #[test]
    fn the_kategorie_b_levels_are_distinct_codes() {
        // The BG and the RZ level of the same Bilanzkreis are different
        // MaBiS-Zählpunkte with different settlement behaviour.
        let bk_b = Zeitreihe::new(Familie::BkSzr, Some(Kategorie::B)).unwrap();
        assert_eq!(
            cav_aus_zeitreihe(bk_b, Some(Aggregationsebene::Regelzone)),
            Some("Z98")
        );
        assert_eq!(
            cav_aus_zeitreihe(bk_b, Some(Aggregationsebene::Bilanzierungsgebiet)),
            Some("Z99")
        );
        // Without a level there is no code — the AHB defines none.
        assert_eq!(cav_aus_zeitreihe(bk_b, None), None);
    }

    #[test]
    fn the_emob_bk_szr_is_not_a_tabelle_1_row() {
        // ZG7 comes from the AWH "Modell 2 ladevorgangsscharfe bilanzielle
        // Energiezuordnung"; giving it a Tabelle-1 row would put it in Fristen
        // it does not have.
        assert_eq!(zeitreihe_aus_cav("ZG7"), None);
        assert_eq!(zeitreihe_aus_cav("ZZZ"), None);
    }

    #[test]
    fn the_verantwortlicher_codes_round_trip() {
        for rolle in [Rolle::Nb, Rolle::Uenb, Rolle::Biko] {
            let code = cci_aus_rolle(rolle).expect("has a code");
            assert_eq!(rolle_aus_cci(code), Some(rolle));
        }
        // The BKV and the LF are never the Verantwortliche of a Summenzeitreihe.
        assert_eq!(cci_aus_rolle(Rolle::Bkv), None);
        assert_eq!(cci_aus_rolle(Rolle::Lf), None);
    }

    #[test]
    fn uenb_aggregation_needs_all_three_conditions() {
        assert_eq!(
            aggregationsverantwortung(Messtechnik::NurIms, true, true),
            Rolle::Uenb
        );
        // iMS and ¼-h, but not yet handed over → still the NB.
        assert_eq!(
            aggregationsverantwortung(Messtechnik::NurIms, true, false),
            Rolle::Nb
        );
        // iMS and handed over, but not ¼-h-billed → still the NB.
        assert_eq!(
            aggregationsverantwortung(Messtechnik::NurIms, false, true),
            Rolle::Nb
        );
        for mt in [
            Messtechnik::NurMme,
            Messtechnik::NurKme,
            Messtechnik::Gemischt,
            Messtechnik::Pauschal,
        ] {
            assert_eq!(aggregationsverantwortung(mt, true, true), Rolle::Nb);
        }
    }
}
