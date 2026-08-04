//! §24 EEG 2023 — Zahlungsansprüche für Strom aus mehreren Anlagen.
//!
//! Several plants can be deemed **one plant** for the purpose of determining the
//! claim under §19 Abs. 1 and the plant size under §21 Abs. 1 / §22. Because
//! tariff bands and the tender threshold are size-dependent, getting this wrong
//! moves a plant into the wrong band for its entire 20-year Förderdauer — it is
//! not a per-period error that a later correction washes out.
//!
//! # The decision is a conjunction plus four carve-outs
//!
//! Satz 1 fuses two plants only when **all four** conditions hold:
//!
//! 1. same Grundstück, Gebäude or Betriebsgelände, or otherwise in unmittelbarer
//!    räumlicher Nähe,
//! 2. they generate electricity from *gleichartige* erneuerbare Energien,
//! 3. the §19 Abs. 1 claim for their electricity depends on Bemessungsleistung or
//!    installierte Leistung, and
//! 4. they were commissioned within twelve consecutive calendar months.
//!
//! Then Sätze 2–5 override that result:
//!
//! - **Satz 2** — biogas (not biomethane) from *the same* Biogaserzeugungsanlage
//!   is fused regardless of Satz 1, so two such plants far apart still count as
//!   one.
//! - **Satz 3** — Freiflächenanlagen are never fused with solar on, in or at
//!   buildings and Lärmschutzwände.
//! - **Satz 4** — building/Lärmschutzwand solar behind *different*
//!   Netzverknüpfungspunkte does not count as one plant.
//! - **Satz 5** — Steckersolargeräte are disregarded entirely when their
//!   installed capacity is ≤ 2 kW in total, their inverter capacity ≤ 800 VA in
//!   total, and they sit behind a Letztverbraucher's Entnahmestelle.
//!
//! # Ownership is not a criterion
//!
//! Satz 1 opens with "**unabhängig von den Eigentumsverhältnissen**" and Satz 2
//! repeats it. Two plants with different operators are fused just the same, and
//! a model that keys the decision on operator identity will under-fuse — the
//! direction that overpays. Nothing in this module takes an operator.
//!
//! Legal text: EEG 2023 in der Fassung vom 18.12.2025, in Kraft ab 23.12.2025.

use crate::technology::ErzeugungsArt;
use time::Date;

/// Where a solar installation sits, for Sätze 3 and 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SolarMontage {
    /// On, in or at a building or a Lärmschutzwand (Satz 3 / Satz 4).
    AnGebaeudeOderLaermschutzwand,
    /// Freiflächenanlage (Satz 3).
    Freiflaeche,
    /// Not a solar installation, or the mounting does not matter here.
    #[default]
    Sonstige,
}

/// A Steckersolargerät small enough to be disregarded under Satz 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Steckersolar {
    /// Installed capacity in kW — Satz 5 Nr. 1 caps the total at 2 kW.
    pub installierte_leistung_kw: rust_decimal::Decimal,
    /// Inverter capacity in VA — Satz 5 Nr. 2 caps the total at 800 VA.
    pub wechselrichter_va: rust_decimal::Decimal,
    /// Satz 5 Nr. 3 — operated behind a Letztverbraucher's Entnahmestelle.
    pub hinter_entnahmestelle: bool,
}

impl Steckersolar {
    /// Whether Satz 5 disregards this device.
    ///
    /// All three conditions are cumulative; a device that misses any of them is
    /// an ordinary plant for §24 purposes.
    #[must_use]
    pub fn wird_nicht_beruecksichtigt(&self) -> bool {
        self.hinter_entnahmestelle
            && self.installierte_leistung_kw <= rust_decimal::dec!(2)
            && self.wechselrichter_va <= rust_decimal::dec!(800)
    }
}

/// One plant, described in the terms §24 actually asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AnlageFuerZusammenfassung {
    /// Commissioning date — Satz 1 Nr. 4.
    pub inbetriebnahme: Date,

    /// Technology — Satz 1 Nr. 2 ("gleichartige erneuerbare Energien").
    pub art: ErzeugungsArt,

    /// Identifier of the Grundstück, Gebäude or Betriebsgelände — Satz 1 Nr. 1.
    ///
    /// Two plants sharing this value are on the same site. When they do not but
    /// are nonetheless in unmittelbarer räumlicher Nähe, set
    /// `unmittelbare_raeumliche_naehe` on the query instead: proximity is a
    /// judgement about a *pair*, not a property of one plant.
    pub standort_id: String,

    /// Satz 1 Nr. 3 — the §19 Abs. 1 claim depends on Bemessungsleistung or
    /// installierte Leistung.
    ///
    /// False for plants whose claim is size-independent, which Satz 1 Nr. 3 then
    /// excludes from fusion.
    pub anspruch_leistungsabhaengig: bool,

    /// Mounting, for Sätze 3 and 4.
    pub montage: SolarMontage,

    /// Netzverknüpfungspunkt — Satz 4 separates building solar behind different
    /// ones. `None` means unknown, which is treated as "not proven different".
    pub netzverknuepfungspunkt: Option<String>,

    /// Satz 2 — the Biogaserzeugungsanlage the biogas comes from.
    ///
    /// Only meaningful for biogas other than biomethane.
    pub biogaserzeugungsanlage_id: Option<String>,

    /// Satz 5 — set when this plant is a Steckersolargerät.
    pub steckersolar: Option<Steckersolar>,
}

/// Why §24 reached its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ZusammenfassungGrund {
    /// Satz 5 — a Steckersolargerät is disregarded.
    Satz5SteckersolarUnberuecksichtigt,
    /// Satz 2 — biogas from the same Biogaserzeugungsanlage, fused regardless
    /// of Satz 1.
    Satz2GleicheBiogaserzeugungsanlage,
    /// Satz 3 — Freifläche is never fused with building/Lärmschutzwand solar.
    Satz3FreiflaecheNichtMitGebaeude,
    /// Satz 4 — building solar behind different Netzverknüpfungspunkte.
    Satz4VerschiedeneNetzverknuepfungspunkte,
    /// All four Satz 1 conditions hold and no carve-out applies.
    Satz1AlleVoraussetzungen,
    /// Satz 1 Nr. 1 — different site and no unmittelbare räumliche Nähe.
    Satz1Nr1StandortVerschieden,
    /// Satz 1 Nr. 2 — not gleichartige erneuerbare Energien.
    Satz1Nr2ArtVerschieden,
    /// Satz 1 Nr. 3 — the claim is not size-dependent for at least one plant.
    Satz1Nr3AnspruchNichtLeistungsabhaengig,
    /// Satz 1 Nr. 4 — commissioned more than twelve calendar months apart.
    Satz1Nr4AusserhalbZwoelfMonatsfenster,
}

/// The §24 answer for a pair of plants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZusammenfassungErgebnis {
    /// Whether the two count as one plant under §24 Abs. 1.
    pub gelten_als_eine_anlage: bool,
    /// The rule that decided it.
    pub grund: ZusammenfassungGrund,
}

/// Whether two commissioning dates fall inside one twelve-calendar-month window
/// (§24 Abs. 1 Satz 1 Nr. 4).
///
/// Twelve consecutive calendar months starting at month M span M..=M+11, so the
/// months must be at most eleven apart. Exactly twelve months later is outside.
#[must_use]
pub fn zusammenlegung_within_12_months(ibn_a: Date, ibn_b: Date) -> bool {
    let months = |d: Date| i32::from(d.year() as i16) * 12 + (u8::from(d.month()) as i32 - 1);
    (months(ibn_a) - months(ibn_b)).abs() < 12
}

/// Decide §24 Abs. 1 for a pair of plants.
///
/// `unmittelbare_raeumliche_naehe` supplies the second limb of Satz 1 Nr. 1 for
/// plants that do not share a `standort_id`. It is a parameter rather than a
/// plant field because proximity is a relation between two plants, and encoding
/// it on one of them invites a "near" flag that is true of nothing in particular.
#[must_use]
pub fn sind_eine_anlage(
    a: &AnlageFuerZusammenfassung,
    b: &AnlageFuerZusammenfassung,
    unmittelbare_raeumliche_naehe: bool,
) -> ZusammenfassungErgebnis {
    let nein = |grund| ZusammenfassungErgebnis {
        gelten_als_eine_anlage: false,
        grund,
    };
    let ja = |grund| ZusammenfassungErgebnis {
        gelten_als_eine_anlage: true,
        grund,
    };

    // ── Satz 5 — a qualifying Steckersolargerät is left out of the fiction ───
    // Checked first: a disregarded device cannot be fused by any other rule.
    if a.steckersolar
        .is_some_and(|s| s.wird_nicht_beruecksichtigt())
        || b.steckersolar
            .is_some_and(|s| s.wird_nicht_beruecksichtigt())
    {
        return nein(ZusammenfassungGrund::Satz5SteckersolarUnberuecksichtigt);
    }

    // ── Satz 2 — same Biogaserzeugungsanlage, "abweichend von Satz 1" ────────
    // Biomethane is excluded by the statute's own wording.
    // "wenn sie Strom aus Biogas mit Ausnahme von Biomethan erzeugen" — the two
    // are distinct `ErzeugungsArt` variants, so naming Biogas already excludes
    // Biomethan.
    let biogas_ohne_biomethan = |x: &AnlageFuerZusammenfassung| x.art == ErzeugungsArt::Biogas;
    if biogas_ohne_biomethan(a)
        && biogas_ohne_biomethan(b)
        && let (Some(ba), Some(bb)) = (&a.biogaserzeugungsanlage_id, &b.biogaserzeugungsanlage_id)
        && ba == bb
    {
        return ja(ZusammenfassungGrund::Satz2GleicheBiogaserzeugungsanlage);
    }

    // ── Satz 3 — Freifläche is never fused with building/Lärmschutzwand solar ─
    let ist_freiflaeche = |x: &AnlageFuerZusammenfassung| x.montage == SolarMontage::Freiflaeche;
    let ist_gebaeude =
        |x: &AnlageFuerZusammenfassung| x.montage == SolarMontage::AnGebaeudeOderLaermschutzwand;
    if (ist_freiflaeche(a) && ist_gebaeude(b)) || (ist_gebaeude(a) && ist_freiflaeche(b)) {
        return nein(ZusammenfassungGrund::Satz3FreiflaecheNichtMitGebaeude);
    }

    // ── Satz 4 — building solar behind different Netzverknüpfungspunkte ──────
    // Only a *known* difference separates them; two unknowns are not a proof.
    if ist_gebaeude(a)
        && ist_gebaeude(b)
        && let (Some(na), Some(nb)) = (&a.netzverknuepfungspunkt, &b.netzverknuepfungspunkt)
        && na != nb
    {
        return nein(ZusammenfassungGrund::Satz4VerschiedeneNetzverknuepfungspunkte);
    }

    // ── Satz 1 — all four conditions, in the statute's order ─────────────────
    if a.standort_id != b.standort_id && !unmittelbare_raeumliche_naehe {
        return nein(ZusammenfassungGrund::Satz1Nr1StandortVerschieden);
    }
    if !gleichartige_energien(a.art, b.art) {
        return nein(ZusammenfassungGrund::Satz1Nr2ArtVerschieden);
    }
    if !a.anspruch_leistungsabhaengig || !b.anspruch_leistungsabhaengig {
        return nein(ZusammenfassungGrund::Satz1Nr3AnspruchNichtLeistungsabhaengig);
    }
    if !zusammenlegung_within_12_months(a.inbetriebnahme, b.inbetriebnahme) {
        return nein(ZusammenfassungGrund::Satz1Nr4AusserhalbZwoelfMonatsfenster);
    }

    ja(ZusammenfassungGrund::Satz1AlleVoraussetzungen)
}

/// Satz 1 Nr. 2 — whether two technologies are *gleichartige* erneuerbare
/// Energien.
///
/// The statute groups by energy source, not by the finer classifications the
/// tariff tables use: two rooftop PV arrays are gleichartig, and so are a
/// rooftop and a ground-mounted array (Satz 3 then separates those on its own
/// terms, which is why the split belongs there and not here).
#[must_use]
pub fn gleichartige_energien(a: ErzeugungsArt, b: ErzeugungsArt) -> bool {
    if a == b {
        return true;
    }
    if a.is_solar() && b.is_solar() {
        return true;
    }
    if a.is_wind() && b.is_wind() {
        // Onshore and offshore are both Windenergie; no §24 case pairs them in
        // practice, but the classification is the statute's, not the tariff's.
        return true;
    }
    if a.is_biomasse_or_gas() && b.is_biomasse_or_gas() {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    fn anlage(ibn: Date, art: ErzeugungsArt, standort: &str) -> AnlageFuerZusammenfassung {
        AnlageFuerZusammenfassung {
            inbetriebnahme: ibn,
            art,
            standort_id: standort.to_owned(),
            anspruch_leistungsabhaengig: true,
            montage: SolarMontage::Sonstige,
            netzverknuepfungspunkt: None,
            biogaserzeugungsanlage_id: None,
            steckersolar: None,
        }
    }

    #[test]
    fn all_four_conditions_fuse_the_plants() {
        let a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let b = anlage(date!(2024 - 12 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let r = sind_eine_anlage(&a, &b, false);
        assert!(r.gelten_als_eine_anlage);
        assert_eq!(r.grund, ZusammenfassungGrund::Satz1AlleVoraussetzungen);
    }

    /// Satz 1 opens "unabhängig von den Eigentumsverhältnissen": this module
    /// takes no operator at all, so two plants of different owners fuse alike.
    #[test]
    fn twelve_month_window_is_exclusive_at_twelve() {
        assert!(zusammenlegung_within_12_months(
            date!(2024 - 01 - 15),
            date!(2024 - 12 - 15)
        ));
        assert!(!zusammenlegung_within_12_months(
            date!(2024 - 01 - 01),
            date!(2025 - 01 - 01)
        ));
        assert!(!zusammenlegung_within_12_months(
            date!(2024 - 01 - 01),
            date!(2025 - 02 - 01)
        ));
        assert!(zusammenlegung_within_12_months(
            date!(2024 - 12 - 01),
            date!(2025 - 11 - 01)
        ));
    }

    #[test]
    fn a_different_site_without_proximity_does_not_fuse() {
        let a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarAufdach, "FLST-2");
        assert_eq!(
            sind_eine_anlage(&a, &b, false).grund,
            ZusammenfassungGrund::Satz1Nr1StandortVerschieden
        );
        // …but unmittelbare räumliche Nähe supplies Nr. 1's second limb.
        assert!(sind_eine_anlage(&a, &b, true).gelten_als_eine_anlage);
    }

    #[test]
    fn different_energy_sources_do_not_fuse() {
        let a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::WindOnshore, "FLST-1");
        assert_eq!(
            sind_eine_anlage(&a, &b, false).grund,
            ZusammenfassungGrund::Satz1Nr2ArtVerschieden
        );
    }

    /// Satz 1 Nr. 3 — fusion serves the size-dependent claim; where the claim
    /// does not depend on size there is nothing to aggregate for.
    #[test]
    fn a_size_independent_claim_is_outside_the_fiction() {
        let a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        b.anspruch_leistungsabhaengig = false;
        assert_eq!(
            sind_eine_anlage(&a, &b, false).grund,
            ZusammenfassungGrund::Satz1Nr3AnspruchNichtLeistungsabhaengig
        );
    }

    /// Satz 2 fuses biogas from one Biogaserzeugungsanlage "abweichend von
    /// Satz 1" — so it wins even across sites and outside the 12-month window.
    #[test]
    fn same_biogas_plant_fuses_regardless_of_satz_1() {
        let mut a = anlage(date!(2018 - 01 - 15), ErzeugungsArt::Biogas, "FLST-1");
        let mut b = anlage(date!(2024 - 09 - 15), ErzeugungsArt::Biogas, "FLST-9");
        a.biogaserzeugungsanlage_id = Some("BGA-7".to_owned());
        b.biogaserzeugungsanlage_id = Some("BGA-7".to_owned());
        let r = sind_eine_anlage(&a, &b, false);
        assert!(r.gelten_als_eine_anlage);
        assert_eq!(
            r.grund,
            ZusammenfassungGrund::Satz2GleicheBiogaserzeugungsanlage
        );
    }

    #[test]
    fn different_biogas_plants_fall_back_to_satz_1() {
        let mut a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::Biogas, "FLST-1");
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::Biogas, "FLST-9");
        a.biogaserzeugungsanlage_id = Some("BGA-7".to_owned());
        b.biogaserzeugungsanlage_id = Some("BGA-8".to_owned());
        assert_eq!(
            sind_eine_anlage(&a, &b, false).grund,
            ZusammenfassungGrund::Satz1Nr1StandortVerschieden
        );
    }

    /// Satz 3 — a Freiflächenanlage is never fused with building solar, even on
    /// one Betriebsgelände within the window.
    #[test]
    fn freiflaeche_is_never_fused_with_building_solar() {
        let mut a = anlage(
            date!(2024 - 01 - 15),
            ErzeugungsArt::SolarFreiflaeche,
            "FLST-1",
        );
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        a.montage = SolarMontage::Freiflaeche;
        b.montage = SolarMontage::AnGebaeudeOderLaermschutzwand;
        let r = sind_eine_anlage(&a, &b, false);
        assert!(!r.gelten_als_eine_anlage);
        assert_eq!(
            r.grund,
            ZusammenfassungGrund::Satz3FreiflaecheNichtMitGebaeude
        );
    }

    /// Satz 4 — building solar behind different Netzverknüpfungspunkte.
    #[test]
    fn building_solar_behind_different_grid_points_does_not_fuse() {
        let mut a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        a.montage = SolarMontage::AnGebaeudeOderLaermschutzwand;
        b.montage = SolarMontage::AnGebaeudeOderLaermschutzwand;
        a.netzverknuepfungspunkt = Some("NVP-A".to_owned());
        b.netzverknuepfungspunkt = Some("NVP-B".to_owned());
        assert_eq!(
            sind_eine_anlage(&a, &b, false).grund,
            ZusammenfassungGrund::Satz4VerschiedeneNetzverknuepfungspunkte
        );
        // Behind the same point, Satz 1 decides normally.
        b.netzverknuepfungspunkt = Some("NVP-A".to_owned());
        assert!(sind_eine_anlage(&a, &b, false).gelten_als_eine_anlage);
    }

    /// Two unknown Netzverknüpfungspunkte are not proof of a difference —
    /// separating on ignorance would under-fuse, which overpays.
    #[test]
    fn unknown_grid_points_do_not_separate_building_solar() {
        let mut a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        a.montage = SolarMontage::AnGebaeudeOderLaermschutzwand;
        b.montage = SolarMontage::AnGebaeudeOderLaermschutzwand;
        assert!(sind_eine_anlage(&a, &b, false).gelten_als_eine_anlage);
    }

    /// Satz 5 — a small Steckersolargerät behind a Letztverbraucher's
    /// Entnahmestelle is disregarded.
    #[test]
    fn a_qualifying_steckersolargeraet_is_disregarded() {
        let a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarStecker, "FLST-1");
        b.steckersolar = Some(Steckersolar {
            installierte_leistung_kw: dec!(2),
            wechselrichter_va: dec!(800),
            hinter_entnahmestelle: true,
        });
        assert_eq!(
            sind_eine_anlage(&a, &b, false).grund,
            ZusammenfassungGrund::Satz5SteckersolarUnberuecksichtigt
        );
    }

    /// All three Satz 5 conditions are cumulative — a device over any limit, or
    /// not behind an Entnahmestelle, is an ordinary plant.
    #[test]
    fn an_oversized_steckersolargeraet_counts_normally() {
        let a = anlage(date!(2024 - 01 - 15), ErzeugungsArt::SolarAufdach, "FLST-1");
        let mut b = anlage(date!(2024 - 03 - 15), ErzeugungsArt::SolarStecker, "FLST-1");
        for over in [
            Steckersolar {
                installierte_leistung_kw: dec!(2.1),
                wechselrichter_va: dec!(800),
                hinter_entnahmestelle: true,
            },
            Steckersolar {
                installierte_leistung_kw: dec!(2),
                wechselrichter_va: dec!(801),
                hinter_entnahmestelle: true,
            },
            Steckersolar {
                installierte_leistung_kw: dec!(2),
                wechselrichter_va: dec!(800),
                hinter_entnahmestelle: false,
            },
        ] {
            b.steckersolar = Some(over);
            assert!(
                sind_eine_anlage(&a, &b, false).gelten_als_eine_anlage,
                "device outside Satz 5 must be fused normally: {over:?}"
            );
        }
    }
}
