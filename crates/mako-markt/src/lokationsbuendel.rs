//! The Lokationsbündel of a Marktlokation on a Stichtag, as `marktd` holds it
//! — the read model behind `GET /api/v1/malos/{malo_id}/lokationsbuendel`.
//!
//! The NB's answer to a Geschäftsdatenanfrage Stammdaten Strom (UTILMD 55035 /
//! 55095 / 55060, UTILMD AHB Strom 2.2, konsolidierte Lesefassung 29.06.2026,
//! Kap. 9.5, S. 599–665) transmits the
//! Lokationsbündel the Marktlokation belongs to: every Netz-, Markt- and
//! Messlokation, Technische and Steuerbare Ressource in `SG5 LOC`, the
//! Lokationsbündelstruktur in `SG8 SEQ+ZD5` and each object's position in it
//! in `SEQ+ZD6`, then the „Informative Daten" of each object. This module is
//! that bündel as facts, not as segments.
//!
//! - **Objects and their validity** come from the Lokationszuordnung graph:
//!   every edge valid on the Stichtag (half-open `[valid_from, valid_to)`,
//!   `valid_from` absent = since the beginning), followed in both directions
//!   from the Marktlokation, so a bündel whose Messlokation PUT wrote `MELO → MALO`
//!   and whose graph API wrote `MALO → NELO` is one bündel.
//! - **The Lokationsbündelstruktur** is the `lokationsbuendelcode` its edges
//!   carry, **each object's position** the BO4E `lokationsbuendelObjektcode`
//!   of that object (Codeliste der Lokationsbündelstrukturen 1.0, Kap. 2.3).
//! - **The NB's Angaben** — what the answer states and no BO4E object holds:
//!   the ÜNB, the Netznutzungsabrechnungspositionen, the Verwendungszwecke and
//!   Wertegranularität of each OBIS-Kennzahl, the technische Einrichtungen and
//!   the normierte Profil — are one dated row per Marktlokation.
//!
//! **A fact marktd does not hold is `None` (or an empty list), never a
//! default.** The answer builder refuses the command and names it.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::domain::Lokationstyp;

/// The Lokationsbündel of one Marktlokation on [`Self::stichtag`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaloLokationsbuendel {
    /// The Marktlokation the bündel is read from.
    pub malo_id: String,
    /// The date every edge and the Angaben row are resolved for.
    pub stichtag: Date,
    /// BO4E `Marktlokation.energierichtung` — `AUSSP` a verbrauchende,
    /// `EINSP` an erzeugende Marktlokation.
    pub energierichtung: Option<rubo4e::current::Energierichtung>,
    /// Every distinct `lokationsbuendelcode` the bündel's edges carry. One
    /// entry is a bündel with a Lokationsbündelstruktur; none is one that
    /// names none, and more than one is a graph that disagrees with itself.
    pub lokationsbuendelcodes: Vec<String>,
    /// Every object reached, the Marktlokation itself first.
    pub objekte: Vec<BuendelObjekt>,
    /// Every edge valid on the Stichtag that reached them.
    pub zuordnungen: Vec<BuendelZuordnung>,
    /// The NB's Angaben row covering the Stichtag.
    pub angaben: Option<BuendelAngaben>,
    /// The Zähler of the bündel's Messlokationen, with what the UTILMD
    /// Zähleinrichtungsdaten state of them beyond the Zählertyp.
    pub zaehler: Vec<BuendelZaehler>,
}

/// One object of the bündel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuendelObjekt {
    /// Which kind of object.
    pub typ: Lokationstyp,
    /// Its ID — MaLo-ID, Zählpunktbezeichnung, or the ID of the
    /// Netzlokation, Technische or Steuerbare Ressource.
    pub id: String,
    /// BO4E `lokationsbuendelObjektcode` — its position in the structure.
    pub objektcode: Option<String>,
}

/// One edge of the Lokationszuordnung graph, valid on the Stichtag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuendelZuordnung {
    /// Source object.
    pub von_id: String,
    /// Its type.
    pub von_typ: Lokationstyp,
    /// Target object.
    pub nach_id: String,
    /// Its type.
    pub nach_typ: Lokationstyp,
    /// First day the edge holds; `None` = since the beginning.
    pub valid_from: Option<Date>,
    /// First day it no longer holds; `None` = open.
    pub valid_to: Option<Date>,
}

/// One Zähler of a Messlokation of the bündel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuendelZaehler {
    /// The Messlokation it measures.
    pub melo_id: String,
    /// marktd's Zähler ID.
    pub zaehler_id: String,
    /// BO4E `Zaehler.zaehlertypSpezifikation` — EDL40 / EDL21 / sonstiger
    /// EHZ for an elektronischer Haushaltszähler, `MME_STANDARD` / `MME_MEDA` for a
    /// moderne Messeinrichtung.
    pub zaehlertyp_spezifikation: Option<rubo4e::current::ZaehlertypSpezifikation>,
}

/// The NB's Angaben of a Marktlokation's bündel — one dated row, half-open
/// `[valid_from, valid_to)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuendelAngaben {
    /// First day the row applies.
    pub valid_from: Date,
    /// First day it no longer applies; `None` = open.
    #[serde(default)]
    pub valid_to: Option<Date>,
    /// MP-ID of the Übertragungsnetzbetreiber of the Marktlokation —
    /// `SG10 CAV+Z90` in „Daten der Marktlokation".
    #[serde(default)]
    pub uenb_mp_id: Option<String>,
    /// Whether technische Einrichtungen are present at the Marktlokation —
    /// `SG10 CCI+Z63` `CAV+ZH7` / `ZH8`.
    #[serde(default)]
    pub technische_einrichtungen: Option<bool>,
    /// Code des normierten Profils — `SG10 CAV` under `CCI+Z02`
    /// Standardlastprofil.
    #[serde(default)]
    pub standardlastprofil: Option<String>,
    /// The Verfahren of the normierte Profil — `CCI+Z02` DE 7037.
    #[serde(default)]
    pub profilverfahren: Option<Profilverfahren>,
    /// The Netznutzungsabrechnungspositionen — one `SG8 SEQ+ZE1` each.
    #[serde(default)]
    pub nna_positionen: Vec<NnaPosition>,
    /// The OBIS-Kennzahlen of the Marktlokation with their Verwendungszwecke
    /// — one `SG8 SEQ+ZE3` each.
    #[serde(default)]
    pub obis: Vec<ObisAngabe>,
    /// The Wertegranularität of each Zähler register — `SG8 SEQ+ZF6`
    /// `CCI+++ZE4`.
    #[serde(default)]
    pub register: Vec<RegisterAngabe>,
}

/// The Verfahren of a normiertes Profil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Profilverfahren {
    /// Synthetisches Verfahren — `E01`.
    Synthetisch,
    /// Analytisches Verfahren — `Z10`.
    Analytisch,
}

impl Profilverfahren {
    /// Every wire value.
    pub const VARIANTS: &'static [&'static str] = &["SYNTHETISCH", "ANALYTISCH"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Synthetisch => "SYNTHETISCH",
            Self::Analytisch => "ANALYTISCH",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "SYNTHETISCH" => Some(Self::Synthetisch),
            "ANALYTISCH" => Some(Self::Analytisch),
            _ => None,
        }
    }
}

/// One Netznutzungsabrechnungsposition of the Marktlokation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NnaPosition {
    /// Gruppenartikel-ID or Artikel-ID (Codeliste der Artikelnummern und
    /// Artikel-ID) — `SG8 PIA+Z02`.
    pub artikel_id: String,
    /// Zuschlag (> 0) or Abschlag (< 0) in Prozent; `0` is „Kein Zu-/
    /// Abschlag" — `SG9 QTY+Z34` / `Z35` / `Z37`.
    pub zu_abschlag_prozent: Decimal,
}

/// One OBIS-Kennzahl of the Marktlokation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObisAngabe {
    /// The OBIS-Kennzahl.
    pub obis_kennzahl: String,
    /// Code der Codeliste der Verwendungszwecke for the NB.
    pub verwendungszweck_nb: Option<String>,
    /// Code for the LF.
    pub verwendungszweck_lf: Option<String>,
    /// Code for the ÜNB.
    pub verwendungszweck_uenb: Option<String>,
    /// Wertegranularität, where the OBIS-Kennzahl takes one.
    #[serde(default)]
    pub wertegranularitaet: Option<Wertegranularitaet>,
}

/// The Wertegranularität of one register of a Zähler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAngabe {
    /// marktd's Zähler ID.
    pub zaehler_id: String,
    /// The register's OBIS-Kennzahl.
    pub obis_kennzahl: String,
    /// Its Wertegranularität.
    pub wertegranularitaet: Wertegranularitaet,
}

/// Wertegranularität — `SG10 CCI+++ZE4` `CAV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Wertegranularitaet {
    /// Jährlich — `ZD9`.
    Jaehrlich,
    /// Halbjährlich — `ZE8`.
    Halbjaehrlich,
    /// Quartalsweise — `ZE9`.
    Quartalsweise,
    /// Monatlich — `ZB7`.
    Monatlich,
}

impl Wertegranularitaet {
    /// The UTILMD `CAV` DE 7111 code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Jaehrlich => "ZD9",
            Self::Halbjaehrlich => "ZE8",
            Self::Quartalsweise => "ZE9",
            Self::Monatlich => "ZB7",
        }
    }
}

/// One row of the Codeliste der Verwendungszwecke 1.0 (Kap. 2, S. 5–7) that
/// a Marktlokation's register may carry: the code, and for which Marktrolle
/// („Verwendbar für Marktrolle" NB / LF / ÜNB) it is admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verwendungszweck {
    /// The code — DE 7143 with DE 1131 `Z16`.
    pub code: &'static str,
    /// Verwendbar für Marktrolle NB.
    pub nb: bool,
    /// Verwendbar für Marktrolle LF.
    pub lf: bool,
    /// Verwendbar für Marktrolle ÜNB.
    pub uenb: bool,
    /// The Erläuterung „Nur möglich, wenn Lieferrichtung …": `Some(true)`
    /// Verbrauch, `Some(false)` Erzeugung, `None` no such restriction.
    pub nur_verbrauch: Option<bool>,
}

/// The codes of the Codeliste der Verwendungszwecke 1.0 whose „Lokation"
/// column marks `MALO` (Kap. 2, S. 5–7). `V12` and `V17` are Netzlokation
/// codes and absent.
pub const VERWENDUNGSZWECKE_MALO: &[Verwendungszweck] = &[
    vz("V01", true, true, false, Some(true)),
    vz("V02", false, true, false, Some(true)),
    vz("V03", true, true, false, Some(true)),
    vz("V04", false, true, false, Some(true)),
    vz("V05", true, true, false, Some(true)),
    vz("V06", false, true, false, Some(true)),
    vz("V07", false, true, false, None),
    vz("V08", true, false, false, Some(false)),
    vz("V09", true, false, false, Some(false)),
    vz("V10", false, true, false, None),
    vz("V11", false, false, true, None),
    vz("V13", false, true, false, Some(false)),
    vz("V14", true, true, false, None),
    vz("V15", true, true, true, None),
    vz("V16", false, false, true, None),
    vz("V18", true, true, false, None),
    vz("V33", false, true, false, None),
];

const fn vz(
    code: &'static str,
    nb: bool,
    lf: bool,
    uenb: bool,
    nur_verbrauch: Option<bool>,
) -> Verwendungszweck {
    Verwendungszweck {
        code,
        nb,
        lf,
        uenb,
        nur_verbrauch,
    }
}

/// The Marktlokation row of the Codeliste der Verwendungszwecke for `code`.
#[must_use]
pub fn verwendungszweck(code: &str) -> Option<&'static Verwendungszweck> {
    VERWENDUNGSZWECKE_MALO.iter().find(|v| v.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table holds every Marktlokation code of the Codeliste once.
    #[test]
    fn every_marktlokation_code_appears_once() {
        let mut codes: Vec<_> = VERWENDUNGSZWECKE_MALO.iter().map(|v| v.code).collect();
        let n = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), n);
        assert!(
            verwendungszweck("V12").is_none(),
            "V12 is a Netzlokation code"
        );
        assert!(verwendungszweck("V15").is_some_and(|v| v.nb && v.lf && v.uenb));
    }
}
