//! The UTILMD Stammdaten of a Marktlokation on a Stichtag, as `marktd` holds
//! them — the read model behind `GET /api/v1/malos/{malo_id}/utilmd-stammdaten`.
//!
//! An Anmeldung `EoG` (UTILMD 55013 Strom, 44013 Gas) and the answers that
//! transmit master data carry the Marktlokation's record whole: its
//! Messlokationen, the Termine der Netznutzungsabrechnung, the `SG8` groups
//! and the Marktlokationsanschrift. This is that record as facts, not as
//! segments: which facts a Prüfidentifikator makes Muss, and how each one is
//! coded, is the AHB profile's and is decided where the message is built.
//!
//! **A fact marktd does not hold is `None` (or an empty list), never a
//! default.** The builder refuses a command whose message would need it and
//! names it; substituting a value would put a statement on the wire nobody
//! made.
//!
//! Every coded value is a BO4E wire value where BO4E models the fact; the
//! two facts BO4E has no type for — the Betriebszustand der Marktlokation and
//! the Netznutzungsabrechnungsvariante — are this module's own enums, each
//! with the `VARIANTS` the schema's `CHECK` lists are pinned to.

use rubo4e::current::{
    AbgabeArt, Fallgruppenzuordnung, Gasqualitaet, Netzebene, Profiltyp, Prognosegrundlage,
    Zaehlergroesse, Zaehlertyp,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::domain::Sparte;

/// The UTILMD Stammdaten of one Marktlokation on [`Self::stichtag`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaloUtilmdStammdaten {
    /// The Marktlokation.
    pub malo_id: String,
    /// Strom or Gas.
    pub sparte: Sparte,
    /// The date every dated fact below is resolved for.
    pub stichtag: Date,
    /// BO4E `Marktlokation.netzebene` — Spannungsebene (Strom) or
    /// Druckebene (Gas) of the Marktlokation.
    pub netzebene: Option<Netzebene>,
    /// BO4E `Marktlokation.gasqualitaet`.
    pub gasqualitaet: Option<Gasqualitaet>,
    /// BO4E `Marktlokation.marktgebiet` — the Gas Marktgebiet code.
    pub marktgebiet: Option<String>,
    /// BO4E `Marktlokation.lokationsadresse` — the Marktlokationsanschrift.
    pub anschrift: Option<Anschrift>,
    /// BO4E `Marktlokation.endkunde` — the Anschlussnutzer at the
    /// Marktlokation, as far as the NB knows him.
    pub kunde: Option<Kunde>,
    /// The OBIS-Kennzahlen of the Marktlokation (BO4E
    /// `Marktlokation.zaehlwerke`), each with its Konzessionsabgabe.
    pub zaehlwerke: Vec<MaloZaehlwerk>,
    /// The BO4E `Bilanzierung` effective on the Stichtag.
    pub bilanzierung: Option<BilanzierungStammdaten>,
    /// The NB's Netznutzungsdaten effective on the Stichtag.
    pub netznutzung: Option<Netznutzungsdaten>,
    /// Every Messlokation assigned to the Marktlokation on the Stichtag.
    pub messlokationen: Vec<MesslokationStammdaten>,
}

/// A postal address — BO4E `Adresse`, reduced to what a UTILMD `NAD` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anschrift {
    /// Straße.
    pub strasse: Option<String>,
    /// Hausnummer.
    pub hausnummer: Option<String>,
    /// Postleitzahl.
    pub postleitzahl: Option<String>,
    /// Ort.
    pub ort: Option<String>,
    /// Ortsteil.
    pub ortsteil: Option<String>,
    /// ISO 3166-1 Alpha-2 Ländercode (BO4E `Landescode` wire value).
    pub landescode: Option<String>,
}

/// The Anschlussnutzer of a Marktlokation — BO4E `Geschaeftspartner`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kunde {
    /// Nachname of a natural person.
    pub nachname: Option<String>,
    /// Vorname of a natural person.
    pub vorname: Option<String>,
    /// Firmenbezeichnung of an organisation.
    pub organisationsname: Option<String>,
    /// The Korrespondenzanschrift.
    pub anschrift: Option<Anschrift>,
}

impl Kunde {
    /// What a UTILMD `SG12 NAD` naming this party with its address lacks —
    /// empty when it can be sent: a Nachname or Organisationsname for `C080`,
    /// and Straße, Ort, Postleitzahl and Ländercode for `C059`, `3164`, `3251`
    /// and `3207` (UTILMD AHB Gas 1.2 Kap. 5.6, S. 64–65).
    #[must_use]
    pub fn anmeldung_maengel(&self) -> Vec<String> {
        fn leer(s: Option<&String>) -> bool {
            s.is_none_or(|s| s.trim().is_empty())
        }
        let mut m = Vec::new();
        if leer(self.nachname.as_ref()) && leer(self.organisationsname.as_ref()) {
            m.push("the name — nachname or organisationsname (NAD C080)".to_owned());
        }
        match &self.anschrift {
            None => m.push("the anschrift".to_owned()),
            Some(a) => {
                for (field, value) in [
                    ("strasse (NAD C059)", a.strasse.as_ref()),
                    ("ort (NAD 3164)", a.ort.as_ref()),
                    ("postleitzahl (NAD 3251)", a.postleitzahl.as_ref()),
                    ("landescode (NAD 3207)", a.landescode.as_ref()),
                ] {
                    if leer(value) {
                        m.push(format!("anschrift.{field}"));
                    }
                }
            }
        }
        m
    }
}

/// One Zählwerk of the Marktlokation — BO4E `Zaehlwerk` on the
/// `Marktlokation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaloZaehlwerk {
    /// The OBIS-Kennzahl.
    pub obis_kennzahl: String,
    /// BO4E `Konzessionsabgabe.satz`.
    pub konzessionsabgabe_satz: Option<AbgabeArt>,
    /// BO4E `Konzessionsabgabe.kosten` — EUR/kWh.
    pub konzessionsabgabe_eur_kwh: Option<Decimal>,
}

/// The balancing facts of the BO4E `Bilanzierung` effective on the Stichtag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BilanzierungStammdaten {
    /// BO4E `Bilanzierung.prognosegrundlage`.
    pub prognosegrundlage: Option<Prognosegrundlage>,
    /// BO4E `Bilanzierung.detailsPrognosegrundlage`.
    pub profiltypen: Vec<Profiltyp>,
    /// BO4E `Bilanzierung.jahresverbrauchsprognose`, in kWh.
    pub jahresverbrauchsprognose_kwh: Option<Decimal>,
    /// BO4E `Bilanzierung.fallgruppenzuordnung`.
    pub fallgruppe: Option<Fallgruppenzuordnung>,
}

/// The Netznutzungsdaten a Netzbetreiber holds for a Marktlokation — the
/// facts of the UTILMD record BO4E has no field for. Half-open validity
/// `[valid_from, valid_to)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Netznutzungsdaten {
    /// First day the row applies.
    pub valid_from: Date,
    /// First day it no longer applies; `None` = open.
    pub valid_to: Option<Date>,
    /// Gruppenzuordnung nach `EnWG`: whether the Letztverbraucher is a
    /// Haushaltskunde (§ 3 Nr. 22 `EnWG`).
    pub haushaltskunde: Option<bool>,
    /// Betriebszustand der Marktlokation.
    pub betriebszustand: Option<Betriebszustand>,
    /// Termin der Netznutzungsabrechnung, `MMDD`.
    pub abrechnungstermin: Option<String>,
    /// Jahr der nächsten Netznutzungsabrechnung, `CCYY`.
    pub naechste_abrechnung_jahr: Option<i32>,
    /// Netznutzungsabrechnungsintervall des NB in Monaten (1, 3, 6 or 12).
    pub abrechnungsintervall_monate: Option<i16>,
    /// Netznutzungsabrechnungsvariante.
    pub abrechnungsvariante: Option<Abrechnungsvariante>,
    /// Gemeinderabatt in Prozent (0 – 10, two decimals).
    pub gemeinderabatt_prozent: Option<Decimal>,
    /// Turnusablesung des NB — UTILMD `SG6 DTM+752`: `MMDD` (DE 2379 `106`)
    /// or `MMWWMMWW` (`104`, Woche `01`–`04` of the month). „Der Termin der
    /// Turnusablesung wird an der Marktlokation vorgegeben und muss an den
    /// zugordneten Messlokationen eingehalten werden" (UTILMD MIG Gas G1.2,
    /// S. 60), so it is held here and stated for each Messlokation.
    pub turnusablesung: Option<String>,
    /// Netznutzungsvertrag — UTILMD `SG10 CAV+Z74`.
    pub netznutzungsvertrag: Option<Netznutzungsvertrag>,
    /// Zahlung der Netznutzung — UTILMD `SG10 CAV+Z73`.
    pub netznutzung_zahler: Option<NetznutzungZahler>,
}

/// Netznutzungsvertrag der Marktlokation (UTILMD `CAV+Z74`, UTILMD AHB Gas 1.2
/// Kap. 5.12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Netznutzungsvertrag {
    /// `Z08` — Direkter Vertrag zwischen Kunden und NB.
    KundeNb,
    /// `Z09` — Vertrag zwischen Lieferanten und NB.
    LieferantNb,
}

impl Netznutzungsvertrag {
    /// Every wire value — the `CHECK` list of
    /// `malo_netznutzungsdaten.netznutzungsvertrag`.
    pub const VARIANTS: &'static [&'static str] = &["KUNDE_NB", "LIEFERANT_NB"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KundeNb => "KUNDE_NB",
            Self::LieferantNb => "LIEFERANT_NB",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "KUNDE_NB" => Some(Self::KundeNb),
            "LIEFERANT_NB" => Some(Self::LieferantNb),
            _ => None,
        }
    }

    /// The UTILMD `CAV+Z74` DE 7110 code.
    #[must_use]
    pub const fn utilmd_code(self) -> &'static str {
        match self {
            Self::KundeNb => "Z08",
            Self::LieferantNb => "Z09",
        }
    }
}

/// Who pays the Netznutzung (UTILMD `CAV+Z73`). `Kunde` is admitted only under
/// a [`Netznutzungsvertrag::KundeNb`]: `Z10` „Wenn `CCI+++Z88`
/// `CAV+Z74:::Z08` vorhanden" (UTILMD AHB Gas 1.2 Kap. 5.12.1, Bedingung
/// `[216]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NetznutzungZahler {
    /// `Z10` — Kunde.
    Kunde,
    /// `Z11` — Lieferant.
    Lieferant,
}

impl NetznutzungZahler {
    /// Every wire value — the `CHECK` list of
    /// `malo_netznutzungsdaten.netznutzung_zahler`.
    pub const VARIANTS: &'static [&'static str] = &["KUNDE", "LIEFERANT"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kunde => "KUNDE",
            Self::Lieferant => "LIEFERANT",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "KUNDE" => Some(Self::Kunde),
            "LIEFERANT" => Some(Self::Lieferant),
            _ => None,
        }
    }

    /// The UTILMD `CAV+Z73` DE 7110 code.
    #[must_use]
    pub const fn utilmd_code(self) -> &'static str {
        match self {
            Self::Kunde => "Z10",
            Self::Lieferant => "Z11",
        }
    }
}

/// Betriebszustand der Marktlokation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Betriebszustand {
    /// Marktlokation im Regelbetrieb.
    Regelbetrieb,
    /// Marktlokation gesperrt.
    Gesperrt,
}

impl Betriebszustand {
    /// Every wire value — the `CHECK` list of
    /// `malo_netznutzungsdaten.betriebszustand`.
    pub const VARIANTS: &'static [&'static str] = &["REGELBETRIEB", "GESPERRT"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regelbetrieb => "REGELBETRIEB",
            Self::Gesperrt => "GESPERRT",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "REGELBETRIEB" => Some(Self::Regelbetrieb),
            "GESPERRT" => Some(Self::Gesperrt),
            _ => None,
        }
    }
}

/// Netznutzungsabrechnungsvariante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Abrechnungsvariante {
    /// Arbeitspreis / Grundpreis.
    ArbeitspreisGrundpreis,
    /// Arbeitspreis / Leistungspreis.
    ArbeitspreisLeistungspreis,
}

impl Abrechnungsvariante {
    /// Every wire value — the `CHECK` list of
    /// `malo_netznutzungsdaten.abrechnungsvariante`.
    pub const VARIANTS: &'static [&'static str] =
        &["ARBEITSPREIS_GRUNDPREIS", "ARBEITSPREIS_LEISTUNGSPREIS"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArbeitspreisGrundpreis => "ARBEITSPREIS_GRUNDPREIS",
            Self::ArbeitspreisLeistungspreis => "ARBEITSPREIS_LEISTUNGSPREIS",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "ARBEITSPREIS_GRUNDPREIS" => Some(Self::ArbeitspreisGrundpreis),
            "ARBEITSPREIS_LEISTUNGSPREIS" => Some(Self::ArbeitspreisLeistungspreis),
            _ => None,
        }
    }
}

/// One Messlokation of the Marktlokation on the Stichtag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesslokationStammdaten {
    /// The Messlokations-ID (Zählpunktbezeichnung).
    pub melo_id: String,
    /// The Messstellenbetrieb assigned to it on the Stichtag.
    pub messstellenbetrieb: Option<Messstellenbetrieb>,
    /// Its Zähler.
    pub zaehler: Vec<ZaehlerStammdaten>,
}

/// The Messstellenbetreiber of a Messlokation on the Stichtag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Messstellenbetrieb {
    /// MP-ID of the acting MSB.
    pub msb_mp_id: String,
    /// Which kind of MSB it is.
    pub rolle: Option<MsbRolle>,
    /// On which footing it operates the Messstelle.
    pub grundlage: Option<MsbGrundlage>,
    /// MP-ID of the grundzuständiger MSB for the Messlokation.
    pub gmsb_mp_id: Option<String>,
}

/// Which kind of Messstellenbetreiber operates a Messlokation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MsbRolle {
    /// Grundzuständiger Messstellenbetreiber.
    Grundzustaendig,
    /// Wettbewerblicher Messstellenbetreiber.
    Wettbewerblich,
    /// Auffangmessstellenbetreiber.
    Auffang,
}

impl MsbRolle {
    /// Every wire value — the `CHECK` list of `melo_msb_zuordnungen.msb_rolle`.
    pub const VARIANTS: &'static [&'static str] = &["GRUNDZUSTAENDIG", "WETTBEWERBLICH", "AUFFANG"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Grundzustaendig => "GRUNDZUSTAENDIG",
            Self::Wettbewerblich => "WETTBEWERBLICH",
            Self::Auffang => "AUFFANG",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "GRUNDZUSTAENDIG" => Some(Self::Grundzustaendig),
            "WETTBEWERBLICH" => Some(Self::Wettbewerblich),
            "AUFFANG" => Some(Self::Auffang),
            _ => None,
        }
    }
}

/// The footing a Messstellenbetreiber operates a Messlokation on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MsbGrundlage {
    /// Auf vertraglicher Grundlage gegenüber Anschlussnutzer / Anschlussnehmer.
    Vertrag,
    /// In der Ausübung der Weiterverpflichtung durch den gMSB.
    Weiterverpflichtung,
}

impl MsbGrundlage {
    /// Every wire value — the `CHECK` list of `melo_msb_zuordnungen.grundlage`.
    pub const VARIANTS: &'static [&'static str] = &["VERTRAG", "WEITERVERPFLICHTUNG"];

    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vertrag => "VERTRAG",
            Self::Weiterverpflichtung => "WEITERVERPFLICHTUNG",
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "VERTRAG" => Some(Self::Vertrag),
            "WEITERVERPFLICHTUNG" => Some(Self::Weiterverpflichtung),
            _ => None,
        }
    }
}

/// One Zähler of a Messlokation — BO4E `Zaehler`, with the OBIS-Kennzahlen of
/// its registers active on the Stichtag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZaehlerStammdaten {
    /// marktd's key of the Zähler.
    pub zaehler_id: String,
    /// BO4E `Zaehler.zaehlernummer` — the Gerätenummer.
    pub zaehlernummer: Option<String>,
    /// BO4E `Zaehler.zaehlertyp`.
    pub zaehlertyp: Option<Zaehlertyp>,
    /// BO4E `Zaehler.zaehlergroesse`.
    pub zaehlergroesse: Option<Zaehlergroesse>,
    /// BO4E `Zaehler.istFernauslesbar`.
    pub fernauslesbar: Option<bool>,
    /// The OBIS-Kennzahlen of its registers active on the Stichtag.
    pub obis_kennzahlen: Vec<String>,
}
