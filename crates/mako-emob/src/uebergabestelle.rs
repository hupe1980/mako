//! The **Übergabestelle** — one physical Marktlokation whose flows the VNB
//! treats as an exchange with the LPB's Bilanzierungsgebiet.
//!
//! This is where the two access paths meet. Anlage 6 grants the model to
//! öffentlich zugängliche Ladepunkte on demand; Beschluss **BK6-24-267**
//! (15.05.2025, bestandskräftig) grants it *entsprechend* to any Netznutzer
//! whose goal is qualitatively unreachable in the standard model, on the
//! individual network-access claim of § 20 Abs. 1, 1a EnWG. Both end at the
//! same place — a MaLo in Modell 2 — so both are the same type here, with the
//! basis recorded because the obligations either side of it differ.

use serde::{Deserialize, Serialize};
use time::Date;

use mako_mabis::MabisZaehlpunktId;
use rubo4e::identifiers::{MaloId, MarktpartnerId};

use crate::bg::{VirtualBalancingArea, ist_monatserster};
use crate::error::EmobError;

/// Which model a Marktlokation is balanced in.
///
/// On the wire this is `SG10 CCI+ZA2` with `CAV`-less DE 7037 `ZE9`/`ZF0` —
/// see [`crate::wire`], which insists on reading both data elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Abwicklungsmodell {
    /// „Bilanzierung an der Marktlokation" — the ordinary case.
    Modell1,
    /// „Bilanzierung im Bilanzierungsgebiet (BG) des LPB".
    Modell2,
}

impl Abwicklungsmodell {
    /// The `SG10 CCI` DE 7037 Merkmal, under Klassentyp `ZA2`.
    #[must_use]
    pub const fn merkmal(self) -> &'static str {
        match self {
            Self::Modell1 => crate::wire::MERKMAL_MODELL_1,
            Self::Modell2 => crate::wire::MERKMAL_MODELL_2,
        }
    }

    /// The BO4E `Abwicklungsmodell` this is, for
    /// `Bilanzierung.abwicklungsmodell`.
    ///
    /// The wire code and the Business Object are different alphabets — `ZF0` on
    /// the EDIFACT side, `MODELL_2` in BO4E — and this is the only place they
    /// meet. Mapping to the `rubo4e` enum rather than to a string means the
    /// value a Stammdatenänderung persists is one BO4E validation accepts by
    /// construction.
    #[must_use]
    pub const fn bo4e(self) -> rubo4e::current::Abwicklungsmodell {
        match self {
            Self::Modell1 => rubo4e::current::Abwicklungsmodell::Modell1,
            Self::Modell2 => rubo4e::current::Abwicklungsmodell::Modell2,
        }
    }

    /// The BO4E wire value — `"MODELL_1"` / `"MODELL_2"`.
    #[must_use]
    pub fn bo4e_wire(self) -> &'static str {
        // `IntoStaticStr` is a `strum` derive and `strum` is deliberately off
        // (its `FromStr` accepts the `UNKNOWN` catch-all), so the wire value
        // comes from the serde representation the enum itself defines.
        match self.bo4e() {
            rubo4e::current::Abwicklungsmodell::Modell1 => "MODELL_1",
            _ => "MODELL_2",
        }
    }
}

/// On what legal basis this Übergabestelle is in Modell 2.
///
/// Not decoration. The two differ in scope and in what a refusal must show,
/// and one of them has an explicit gap that is an onboarding blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccessBasis {
    /// Anlage 6 zum Beschluss BK6-20-160 — a CPO's öffentlich zugänglicher
    /// Ladepunkt (LSV § 2 Nr. 9). In force since 01.06.2021, with MaKo
    /// processes since 01.10.2023.
    NzrEmobOeffentlich,
    /// § 20 Abs. 1, 1a EnWG as decided in **BK6-24-267** — an individual
    /// network-access claim, e.g. a Hausanschluss with a non-public wallbox.
    ///
    /// The Beschluss is explicit that its scope excludes Kundenanlagen with
    /// EEG-geförderte Anlagen or otherwise complex structures, which
    /// [`Uebergabestelle::anmelden`] turns into a refusal rather than a
    /// warning.
    IndividuellerNetzzugang,
}

/// How the Übergabestelle is metered.
///
/// Anlage 6 §III.1 requires quarter-hour measurement. Nothing else can carry
/// the model: an SLP-billed Marktlokation has no ¼-h truth to allocate against,
/// so the Netzgangzeitreihe would be a profile and the Bilanzkreis-Zuordnung a
/// fiction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MeteringMode {
    /// Zählerstandsgang from an intelligentes Messsystem.
    Zaehlerstandsgang,
    /// Registrierende Leistungsmessung.
    Rlm,
    /// Anything else — refused.
    Sonstiges,
}

impl MeteringMode {
    /// `true` when the mode delivers quarter-hour values.
    #[must_use]
    pub const fn ist_viertelstundenscharf(self) -> bool {
        matches!(self, Self::Zaehlerstandsgang | Self::Rlm)
    }
}

/// The requested move of one Marktlokation between the two models.
///
/// AHB Bedingung `[317]` makes `DTM+92` (Vertragsbeginn) and `DTM+158`
/// (Bilanzierungsbeginn) carry the *same* value, so this type holds one date.
/// Two fields would let a caller render a message the receiving AHB layer
/// rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modellwechsel {
    /// The day the new model takes effect — always a Monatserster.
    pub termin: Date,
}

impl Modellwechsel {
    /// A Modellwechsel to `termin`.
    ///
    /// # Errors
    ///
    /// [`EmobError::NotFirstOfMonth`] unless `termin` is the first of a month.
    /// An-/Abmeldung is „zum Beginn eines Monats mit einer Frist von einem
    /// Monat in die Zukunft möglich" (AWH Kap. 2.1.2 Nr. 1 / 2.2.2 Nr. 1).
    pub fn neu(termin: Date) -> Result<Self, EmobError> {
        if !ist_monatserster(termin) {
            return Err(EmobError::NotFirstOfMonth {
                was: "the Modellwechseltermin",
                date: termin,
            });
        }
        Ok(Self { termin })
    }
}

/// A Marktlokation registered as an exchange point with the LPB's BG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uebergabestelle {
    /// The physical Marktlokation the VNB knows.
    ///
    /// Under [`AccessBasis::IndividuellerNetzzugang`] it is „ruhend gestellt"
    /// for balancing but kept for the Netznutzungsabrechnung, and § 14a control
    /// stays with the physical Netzbetreiber (BK6-24-267 S. 4, S. 22–23).
    pub malo: MaloId,
    /// The Verteilnetzbetreiber the Anmeldung goes to.
    pub vnb: MarktpartnerId,
    /// Which door the Übergabestelle came through.
    pub basis: AccessBasis,
    /// Which model it is balanced in today.
    pub modell: Abwicklungsmodell,
    /// The Zählpunkt der Netzgangzeitreihe, once the VNB has named it.
    ///
    /// `None` until the 55239 Bestätigung arrives: AHB Bedingung `[663]` makes
    /// the VNB return „die ID der Marktlokation und die ZPB des ZP der NGZ",
    /// so before the answer the LPB does not have it.
    pub zp_ngz: Option<MabisZaehlpunktId>,
    /// How it is metered.
    pub metering: MeteringMode,
    /// `true` when the Kundenanlage carries an EEG-geförderte Anlage.
    ///
    /// BK6-24-267 leaves that case open, so it blocks onboarding under
    /// [`AccessBasis::IndividuellerNetzzugang`] rather than being decided here.
    pub eeg_anlage_vorhanden: bool,
}

impl Uebergabestelle {
    /// Check that this Übergabestelle may enter Modell 2 at `wechsel`.
    ///
    /// Enforces the three preconditions that are checkable from the operator's
    /// own records:
    ///
    /// 1. **¼-h metering** — Anlage 6 §III.1.
    /// 2. **The BG must be valid on the day the Anmeldung takes effect** —
    ///    „Das Anmeldedatum darf nicht vor dem Gültigkeitsbeginn des BG
    ///    liegen" (AWH Kap. 2.1.2 Nr. 1) is the lower half; the upper half is
    ///    implied, because a Marktlokation balanced into a BG that has already
    ///    been beendet has no Bilanzkreis-Zuordnung on the day it moves.
    /// 3. **No EEG plant on an individueller-Netzzugang Kundenanlage** —
    ///    BK6-24-267 S. 28 does not decide that case.
    ///
    /// It deliberately does **not** check the one-month lead time: that is a
    /// property of when the message is sent, not of the Übergabestelle, and it
    /// belongs to the caller that stamps the Übertragungszeitpunkt.
    /// [`crate::fristen::fruehester_modellwechsel`] is that check.
    ///
    /// # Errors
    ///
    /// One of [`EmobError::KeineViertelstundenmessung`],
    /// [`EmobError::AnmeldungVorBgBeginn`], [`EmobError::AnmeldungNachBgEnde`]
    /// or [`EmobError::EegAnlageAusserhalbDesBeschlusses`].
    pub fn anmelden(
        &self,
        bg: &VirtualBalancingArea,
        wechsel: Modellwechsel,
    ) -> Result<(), EmobError> {
        if !self.metering.ist_viertelstundenscharf() {
            return Err(EmobError::KeineViertelstundenmessung);
        }
        if wechsel.termin < bg.valid_from {
            return Err(EmobError::AnmeldungVorBgBeginn {
                anmeldung: wechsel.termin,
                bg_start: bg.valid_from,
            });
        }
        if let Some(ende) = bg.valid_to.filter(|&to| wechsel.termin >= to) {
            return Err(EmobError::AnmeldungNachBgEnde {
                anmeldung: wechsel.termin,
                bg_ende: ende,
            });
        }
        if self.basis == AccessBasis::IndividuellerNetzzugang && self.eeg_anlage_vorhanden {
            return Err(EmobError::EegAnlageAusserhalbDesBeschlusses);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bg::Regelzone;
    use mako_mabis::{BilanzierungsgebietId, BilanzkreisId};
    use time::Month;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    fn bg() -> VirtualBalancingArea {
        VirtualBalancingArea::neu(
            BilanzierungsgebietId::new("11YN-0000-0001-Q").unwrap(),
            Regelzone::TenneT,
            BilanzkreisId::new("11XSUEDWESTSTRO8").unwrap(),
            d(2026, 9, 1),
        )
        .unwrap()
    }

    fn us(basis: AccessBasis, metering: MeteringMode, eeg: bool) -> Uebergabestelle {
        Uebergabestelle {
            malo: MaloId::new("51238696781").expect("valid MaLo id"),
            vnb: MarktpartnerId::new("9903030000001").expect("valid MP id"),
            basis,
            modell: Abwicklungsmodell::Modell1,
            zp_ngz: None,
            metering,
            eeg_anlage_vorhanden: eeg,
        }
    }

    #[test]
    fn a_modellwechsel_lands_on_the_first() {
        assert!(Modellwechsel::neu(d(2026, 11, 1)).is_ok());
        assert!(Modellwechsel::neu(d(2026, 11, 2)).is_err());
    }

    #[test]
    fn slp_metering_cannot_carry_the_model() {
        let e = us(
            AccessBasis::NzrEmobOeffentlich,
            MeteringMode::Sonstiges,
            false,
        )
        .anmelden(&bg(), Modellwechsel::neu(d(2026, 11, 1)).unwrap())
        .unwrap_err();
        assert_eq!(e, EmobError::KeineViertelstundenmessung);
    }

    #[test]
    fn the_anmeldung_cannot_precede_the_bg() {
        let e = us(AccessBasis::NzrEmobOeffentlich, MeteringMode::Rlm, false)
            .anmelden(&bg(), Modellwechsel::neu(d(2026, 8, 1)).unwrap())
            .unwrap_err();
        assert!(matches!(e, EmobError::AnmeldungVorBgBeginn { .. }));
    }

    /// The EEG gap is a BK6-24-267 gap, so it blocks only that path.
    #[test]
    fn an_eeg_plant_blocks_only_the_individual_access_path() {
        let wechsel = Modellwechsel::neu(d(2026, 11, 1)).unwrap();
        assert_eq!(
            us(
                AccessBasis::IndividuellerNetzzugang,
                MeteringMode::Zaehlerstandsgang,
                true
            )
            .anmelden(&bg(), wechsel)
            .unwrap_err(),
            EmobError::EegAnlageAusserhalbDesBeschlusses
        );
        assert!(
            us(
                AccessBasis::NzrEmobOeffentlich,
                MeteringMode::Zaehlerstandsgang,
                true
            )
            .anmelden(&bg(), wechsel)
            .is_ok()
        );
    }

    #[test]
    fn both_metering_modes_are_accepted() {
        let wechsel = Modellwechsel::neu(d(2026, 11, 1)).unwrap();
        for m in [MeteringMode::Zaehlerstandsgang, MeteringMode::Rlm] {
            assert!(
                us(AccessBasis::NzrEmobOeffentlich, m, false)
                    .anmelden(&bg(), wechsel)
                    .is_ok()
            );
        }
    }

    /// A BG that has been beendet is not one a Marktlokation can move into.
    #[test]
    fn the_anmeldung_cannot_land_after_the_bg_has_ended() {
        let mut b = bg();
        b.beenden(d(2026, 12, 31)).unwrap();
        let us = us(AccessBasis::NzrEmobOeffentlich, MeteringMode::Rlm, false);

        assert!(
            us.anmelden(&b, Modellwechsel::neu(d(2026, 12, 1)).unwrap())
                .is_ok(),
            "the last month inside the BG still works"
        );
        let e = us
            .anmelden(&b, Modellwechsel::neu(d(2027, 1, 1)).unwrap())
            .unwrap_err();
        assert!(matches!(e, EmobError::AnmeldungNachBgEnde { .. }), "{e:?}");
    }

    #[test]
    fn the_merkmal_matches_the_wire_module() {
        assert_eq!(Abwicklungsmodell::Modell2.merkmal(), "ZF0");
        assert_eq!(Abwicklungsmodell::Modell1.merkmal(), "ZE9");
        assert_eq!(Abwicklungsmodell::Modell1.merkmal(), "ZE9");
    }

    /// The BO4E wire strings are hand-written (`strum` is off), so they are
    /// pinned against the enum's own serde representation. A `rubo4e` rename
    /// fails here rather than silently writing an unrecognised
    /// `Bilanzierung.abwicklungsmodell` into `marktd`.
    #[test]
    fn the_bo4e_wire_value_matches_what_rubo4e_serialises() {
        for m in [Abwicklungsmodell::Modell1, Abwicklungsmodell::Modell2] {
            let json = serde_json::to_string(&m.bo4e()).expect("the BO4E enum serialises");
            assert_eq!(
                json.trim_matches('"'),
                m.bo4e_wire(),
                "{m:?}: bo4e_wire disagrees with rubo4e's serde value"
            );
        }
    }
}
