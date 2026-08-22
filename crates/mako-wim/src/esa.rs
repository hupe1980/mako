//! ESA vocabulary — what an Energieserviceanbieter may order, how the messages
//! of the ordering handshake correlate, and which EBD answers which step.
//!
//! This module holds the parts of **WiM Strom Teil 2, Kapitel 4** that both
//! sides of the handshake need to agree on, so the MSB-side
//! ([`crate::wertebestellung`]) and the ESA-side
//! ([`crate::esa_wertebestellung`]) aggregates cannot drift apart:
//!
//! - the **Messprodukt catalogue** ([`Messprodukt`]) from the *Codeliste der
//!   Konfigurationen* 1.4, Kapitel 4.6 — the only products the role may order;
//! - the **Abonnement** mode ([`Abonnement`]) that `IMD+7081` carries and that
//!   decides whether a Stornierung targets a one-shot or a running series;
//! - the **Bestellgegenstand** ([`Bestellgegenstand`]) — the ordered product,
//!   the Wunschtermin and the Abo mode, i.e. *what* the process is about;
//! - the **correlation keys** ([`Korrelation`]) the BDEW
//!   *Anwendungsübersicht der Prüfidentifikatoren* 4.0 assigns per PID;
//! - the **EBD** (`AntwortEbd`) an ORDRSP must name in `SG2 AJT`.
//!
//! # Why the catalogue is data, not a free-form string
//!
//! REQOTE AHB 1.2 §4.3 condition `[41]` and ORDERS AHB 1.1b §4.15 restrict
//! `SG27 PIA+5 DE7140` to the codes of *Codeliste der Konfigurationen* Kapitel
//! 4.6.1/4.6.2. A Messprodukt-Code is therefore a closed set, and the level it
//! is defined for (`Marktlokation`, `Messlokation`, `Netzlokation`, `Tranche`)
//! must match the `LOC+172` identifier the Werteanfrage carries — a constraint
//! only a typed catalogue can enforce.

use time::{Date, macros::date};

// ── Lokationsebene ────────────────────────────────────────────────────────────

/// Which level of location an ESA order addresses.
///
/// UC 4.1.1 requires the request to reach the MSB assigned to *that exact*
/// location, and the identifier differs per level. `LOC+172` DE 3225 is
/// polymorphic across all four (REQOTE AHB 1.2 §4.3, hints `[502]`–`[510]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lokationsebene {
    /// Marktlokation — 11-digit MaLo-ID.
    Marktlokation,
    /// Messlokation — 33-character Zählpunktbezeichnung.
    Messlokation,
    /// Netzlokation — NeLo-ID.
    Netzlokation,
    /// Tranche — Tranchen-ID. Permitted by `LOC+172` hint `[504]` and the only
    /// level `9991 00000 306 4` (a Pflicht product) is defined for.
    Tranche,
}

impl Lokationsebene {
    /// Stable label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Marktlokation => "Marktlokation",
            Self::Messlokation => "Messlokation",
            Self::Netzlokation => "Netzlokation",
            Self::Tranche => "Tranche",
        }
    }
}

// ── Übertragungsweg ───────────────────────────────────────────────────────────

/// How the ordered values reach the ESA.
///
/// The two paths of *Codeliste der Konfigurationen* 1.4 Kapitel 4.6 are not
/// interchangeable: 4.6.1 delivers MSCONS 13027 over AS4 from the MSB
/// back-end, 4.6.2 delivers XML straight from the iMS over SM-PKI and needs
/// the `SG27 FTX+Z17/Z23/Z24` target address and certificate body in the
/// Werteanfrage (REQOTE AHB 1.2 §4.3, condition `[512]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Uebertragungsweg {
    /// Kapitel 4.6.1 — „Werte nach Typ 2 aus Backend“, EDIFACT (MSCONS 13027).
    Backend,
    /// Kapitel 4.6.2 — „Werte nach Typ 2 aus SMGW“, XML direct from the iMS.
    Smgw,
}

impl Uebertragungsweg {
    /// `SG27 LIN` DE 1229 code that introduces this path's product line.
    ///
    /// `Z67` = „Erforderliches Messprodukt für Werte nach Typ 2 aus Backend“,
    /// `Z68` = „Erforderliches Produkt Konfigurationserlaubnis für Werte nach
    /// Typ 2 aus SMGW“ (REQOTE AHB 1.2 §4.3).
    #[must_use]
    pub const fn lin_code(self) -> &'static str {
        match self {
            Self::Backend => "Z67",
            Self::Smgw => "Z68",
        }
    }
}

// ── Messgrößen ────────────────────────────────────────────────────────────────

/// Measured quantity of a Messprodukt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wert {
    /// Wirkarbeit.
    Wirkarbeit,
    /// Blindarbeit.
    Blindarbeit,
    /// Momentanwerte (Ist-Einspeisung, Netzzustandsdaten, Mehrwertdienste) —
    /// the SMGW products whose Art der Werte is a Messprodukt-Position-Code
    /// list in Kapitel 4.7 rather than a single quantity.
    Momentanwerte,
}

/// Shape of the delivered series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Werteart {
    /// Lastgang.
    Lastgang,
    /// Zählerstandsgang.
    Zaehlerstandsgang,
    /// Energiemenge / Arbeitsmenge over a Zeitintervall.
    Arbeitsmenge,
    /// Momentanwerte from the SMGW.
    Momentanwerte,
}

/// Direction of energy flow (and, for Blindarbeit, the quadrant pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Energieflussrichtung {
    /// Verbrauch.
    Verbrauch,
    /// Erzeugung.
    Erzeugung,
    /// Verbrauch *and* Erzeugung in one product.
    VerbrauchUndErzeugung,
    /// Blindarbeit quadrants Q1/Q4 (the Verbrauch pair).
    Q1Q4,
    /// Blindarbeit quadrants Q2/Q3 (the Erzeugung pair).
    Q2Q3,
}

/// How often the MSB transmits and by when.
///
/// The Rohdaten products carry an explicit clock (`unverzüglich, spätestens
/// 9:30 Uhr`); the aufbereitete-Daten products defer to *WiM Teil 2 Kapitel
/// 2.5.5*, whose windows depend on the Werteart and the Messlokation's
/// equipment — so mako must not invent a single blanket deadline for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lieferrhythmus {
    /// Daily, `unverzüglich, jedoch spätestens bis 9:30 Uhr` (Rohdaten).
    TaeglichBis0930,
    /// Per *WiM Teil 2 Kapitel 2.5.5* — depends on Werteart and equipment.
    WimKapitel255,
    /// Direktverbindung iMS → ESA; the interval is the product's own
    /// Erfassungsintervall (SMGW Mehrwertdienste / Ist-Einspeisung).
    Direktverbindung,
}

/// „unverzüglich, jedoch spätestens bis 9:30 Uhr" — the delivery deadline the
/// Rohdaten Messprodukte publish.
pub const ROHDATEN_FRIST: time::Time = time::macros::time!(9:30);

impl Lieferrhythmus {
    /// The hard clock time a daily delivery must arrive by, when the product
    /// states one.
    ///
    /// `None` means the product defers to *WiM Teil 2 Kapitel 2.5.5* or to a
    /// direct iMS connection: there is no single published wall-clock deadline
    /// to monitor against, and asserting one would raise false alarms.
    #[must_use]
    pub const fn taegliche_frist(self) -> Option<time::Time> {
        match self {
            Self::TaeglichBis0930 => Some(ROHDATEN_FRIST),
            Self::WimKapitel255 | Self::Direktverbindung => None,
        }
    }
}

/// Whether the MSB must serve the product or may decline it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verbindlichkeit {
    /// The MSB must offer it — BNetzA *Mitteilung Nr. 3* (07.02.2024).
    Pflicht,
    /// Optional; the MSB may reject the Anfrage for this product.
    Optional,
}

// ── Messprodukt ───────────────────────────────────────────────────────────────

/// One row of *Codeliste der Konfigurationen* 1.4, Kapitel 4.6.
///
/// Kept as data rather than a bare string so a Werteanfrage can be checked
/// against the level it addresses before it goes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Messprodukt {
    /// 13-digit Messprodukt-Code, digits only (no grouping spaces).
    pub code: &'static str,
    /// Bezeichnung as published.
    pub bezeichnung: &'static str,
    /// Delivery path — decides EDIFACT vs SM-PKI.
    pub weg: Uebertragungsweg,
    /// Location level the product is defined for.
    pub ebene: Lokationsebene,
    /// Measured quantity.
    pub wert: Wert,
    /// Series shape.
    pub werteart: Werteart,
    /// Flow direction / quadrants.
    pub richtung: Energieflussrichtung,
    /// Transmission cadence and its deadline.
    pub rhythmus: Lieferrhythmus,
    /// Pflicht or Optional.
    pub verbindlichkeit: Verbindlichkeit,
    /// „Nutzbar ab“ — the product may not be ordered before this date.
    pub nutzbar_ab: Date,
    /// `true` when the product's Auslöser is „Bei Schwellwertunter- /
    /// -überschreitung“, which makes `SG28 CCI+Z60` mandatory in the
    /// Werteanfrage (REQOTE AHB 1.2 §4.3, conditions `[43]`/`[2066]`).
    pub schwellwertgesteuert: bool,
}

/// Build one catalogue row. Schwellwert-triggered products are written out
/// longhand instead, so this shorthand always yields `schwellwertgesteuert:
/// false` and cannot silently drop the `SG28 CCI+Z60` requirement.
macro_rules! produkt {
    (
        $code:literal, $bez:literal, $weg:ident, $ebene:ident, $wert:ident,
        $art:ident, $ri:ident, $rh:ident, $vb:ident, $nutzbar_ab:expr
    ) => {
        Messprodukt {
            code: $code,
            bezeichnung: $bez,
            weg: Uebertragungsweg::$weg,
            ebene: Lokationsebene::$ebene,
            wert: Wert::$wert,
            werteart: Werteart::$art,
            richtung: Energieflussrichtung::$ri,
            rhythmus: Lieferrhythmus::$rh,
            verbindlichkeit: Verbindlichkeit::$vb,
            nutzbar_ab: $nutzbar_ab,
            schwellwertgesteuert: false,
        }
    };
}

/// *Codeliste der Konfigurationen* 1.4, Kapitel **4.6.1** — „Werte nach Typ 2
/// aus Backend“, delivered as MSCONS 13027 over AS4.
pub const BACKEND_PRODUKTE: &[Messprodukt] = &[
    produkt!(
        "9991000000416",
        "ESA, Messlokation Wirkarbeit Lastgang Verbrauch 1/4 stündlich, Rohdaten",
        Backend,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Verbrauch,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000000424",
        "ESA, Messlokation Wirkarbeit Lastgang Erzeugung 1/4 stündlich, Rohdaten",
        Backend,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Erzeugung,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000000458",
        "ESA, Messlokation Blindarbeit Lastgang Verbrauch 1/4 stündlich, Rohdaten",
        Backend,
        Messlokation,
        Blindarbeit,
        Lastgang,
        Q1Q4,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000000466",
        "ESA, Messlokation Blindarbeit Lastgang Erzeugung 1/4 stündlich, Rohdaten",
        Backend,
        Messlokation,
        Blindarbeit,
        Lastgang,
        Q2Q3,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000000747",
        "ESA, Marktlokation Wirkarbeit Lastgang Verbrauch oder Erzeugung 1/4 stündlich, aufbereitete Daten",
        Backend,
        Marktlokation,
        Wirkarbeit,
        Lastgang,
        VerbrauchUndErzeugung,
        WimKapitel255,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000003056",
        "ESA, Marktlokation Wirkarbeit Lastgang Verbrauch oder Erzeugung 1/4 stündlich, aufbereitete Daten",
        Backend,
        Marktlokation,
        Wirkarbeit,
        Lastgang,
        VerbrauchUndErzeugung,
        WimKapitel255,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
    produkt!(
        "9991000000755",
        "ESA, Tranche Wirkarbeit Lastgang Erzeugung 1/4 stündlich, aufbereitete Daten",
        Backend,
        Tranche,
        Wirkarbeit,
        Lastgang,
        Erzeugung,
        WimKapitel255,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000003064",
        "ESA, Tranche Wirkarbeit Lastgang Erzeugung 1/4 stündlich, aufbereitete Daten",
        Backend,
        Tranche,
        Wirkarbeit,
        Lastgang,
        Erzeugung,
        WimKapitel255,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
    produkt!(
        "9991000001539",
        "ESA, Marktlokation Blindarbeit Lastgang 1/4 stündlich, aufbereitete Daten",
        Backend,
        Marktlokation,
        Blindarbeit,
        Lastgang,
        VerbrauchUndErzeugung,
        WimKapitel255,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000000763",
        "ESA, Netzlokation Blindarbeit Lastgang 1/4 stündlich, aufbereitete Daten",
        Backend,
        Netzlokation,
        Blindarbeit,
        Lastgang,
        VerbrauchUndErzeugung,
        WimKapitel255,
        Optional,
        date!(2024 - 01 - 01)
    ),
    produkt!(
        "9991000000771",
        "ESA, Messlokation Wirkarbeit Lastgang Verbrauch 1/4 stündlich, aufbereitete Daten",
        Backend,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Verbrauch,
        WimKapitel255,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
    produkt!(
        "9991000000789",
        "ESA, Messlokation Wirkarbeit Lastgang Erzeugung 1/4 stündlich, aufbereitete Daten",
        Backend,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Erzeugung,
        WimKapitel255,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
    produkt!(
        "9991000000797",
        "ESA, Messlokation Blindarbeit Lastgang Verbrauch 1/4 stündlich, aufbereitete Daten",
        Backend,
        Messlokation,
        Blindarbeit,
        Lastgang,
        Q1Q4,
        WimKapitel255,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000000804",
        "ESA, Messlokation Blindarbeit Lastgang Erzeugung 1/4 stündlich, aufbereitete Daten",
        Backend,
        Messlokation,
        Blindarbeit,
        Lastgang,
        Q2Q3,
        WimKapitel255,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000001505",
        "ESA, Messlokation Zählerstandsgang Erzeugung 1/4 stündlich, Rohdaten",
        Backend,
        Messlokation,
        Wirkarbeit,
        Zaehlerstandsgang,
        Erzeugung,
        TaeglichBis0930,
        Optional,
        date!(2023 - 08 - 01)
    ),
    produkt!(
        "9991000001513",
        "ESA, Marktlokation Zählerstandsgang Verbrauch / Erzeugung 1/4 stündlich, Rohdaten",
        Backend,
        Marktlokation,
        Wirkarbeit,
        Zaehlerstandsgang,
        VerbrauchUndErzeugung,
        TaeglichBis0930,
        Optional,
        date!(2023 - 08 - 01)
    ),
    produkt!(
        "9991000001521",
        "ESA, Messlokation Zählerstandsgang Verbrauch 1/4 stündlich, Rohdaten",
        Backend,
        Messlokation,
        Wirkarbeit,
        Zaehlerstandsgang,
        Verbrauch,
        TaeglichBis0930,
        Optional,
        date!(2023 - 08 - 01)
    ),
    produkt!(
        "9991000003147",
        "ESA, Marktlokation, Energiemenge, aufbereitete Daten",
        Backend,
        Marktlokation,
        Wirkarbeit,
        Arbeitsmenge,
        VerbrauchUndErzeugung,
        WimKapitel255,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
];

/// *Codeliste der Konfigurationen* 1.4, Kapitel **4.6.2** — „Werte nach Typ 2
/// aus SMGW“, delivered as XML straight from the iMS over SM-PKI.
///
/// Ordering any of these makes the `SG27 FTX+Z17` Zieladresse (IPv4 *and*
/// IPv6) and the `FTX+Z23`/`FTX+Z24` certificate bodies mandatory in the
/// Werteanfrage; the Schwellwert-triggered ones additionally require
/// `SG28 CCI+Z60` (REQOTE AHB 1.2 §4.3).
pub const SMGW_PRODUKTE: &[Messprodukt] = &[
    produkt!(
        "9991000000432",
        "ESA, Messlokation Wirkarbeit Lastgang Verbrauch 1/4 stündlich aus dem SMGW",
        Smgw,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Verbrauch,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000003121",
        "ESA, Messlokation Wirkarbeit Lastgang Verbrauch 1/4 stündlich aus dem SMGW, Rohdaten",
        Smgw,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Verbrauch,
        TaeglichBis0930,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
    produkt!(
        "9991000000440",
        "ESA, Messlokation Wirkarbeit Lastgang Erzeugung 1/4 stündlich aus dem SMGW",
        Smgw,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Erzeugung,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000003139",
        "ESA, Messlokation Wirkarbeit Lastgang Erzeugung 1/4 stündlich aus dem SMGW, Rohdaten",
        Smgw,
        Messlokation,
        Wirkarbeit,
        Lastgang,
        Erzeugung,
        TaeglichBis0930,
        Pflicht,
        date!(2024 - 08 - 06)
    ),
    produkt!(
        "9991000000474",
        "ESA, Messlokation Blindarbeit Lastgang Verbrauch 1/4 stündlich aus dem SMGW",
        Smgw,
        Messlokation,
        Blindarbeit,
        Lastgang,
        Q1Q4,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000000482",
        "ESA, Messlokation Blindarbeit Lastgang Erzeugung 1/4 stündlich aus dem SMGW",
        Smgw,
        Messlokation,
        Blindarbeit,
        Lastgang,
        Q2Q3,
        TaeglichBis0930,
        Optional,
        date!(2022 - 04 - 01)
    ),
    produkt!(
        "9991000001183",
        "Messlokation, Ist-Einspeisung, 1 Min.",
        Smgw,
        Messlokation,
        Momentanwerte,
        Momentanwerte,
        Erzeugung,
        Direktverbindung,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000001191",
        "Messlokation, Ist-Einspeisung, 15 Min.",
        Smgw,
        Messlokation,
        Momentanwerte,
        Momentanwerte,
        Erzeugung,
        Direktverbindung,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000001208",
        "Messlokation, Ist-Einspeisung, zur einmaligen Übermittlung",
        Smgw,
        Messlokation,
        Momentanwerte,
        Momentanwerte,
        Erzeugung,
        Direktverbindung,
        Optional,
        date!(2023 - 10 - 01)
    ),
    Messprodukt {
        code: "9991000001216",
        bezeichnung: "Messlokation, Ist-Einspeisung, Schwellwert",
        weg: Uebertragungsweg::Smgw,
        ebene: Lokationsebene::Messlokation,
        wert: Wert::Momentanwerte,
        werteart: Werteart::Momentanwerte,
        richtung: Energieflussrichtung::Erzeugung,
        rhythmus: Lieferrhythmus::Direktverbindung,
        verbindlichkeit: Verbindlichkeit::Optional,
        nutzbar_ab: date!(2023 - 10 - 01),
        schwellwertgesteuert: true,
    },
    produkt!(
        "9991000001224",
        "Messlokation, Mehrwertdienste, 1 Min.",
        Smgw,
        Messlokation,
        Momentanwerte,
        Momentanwerte,
        VerbrauchUndErzeugung,
        Direktverbindung,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000001232",
        "Messlokation, Mehrwertdienste, 15 Min.",
        Smgw,
        Messlokation,
        Momentanwerte,
        Momentanwerte,
        VerbrauchUndErzeugung,
        Direktverbindung,
        Optional,
        date!(2023 - 10 - 01)
    ),
    produkt!(
        "9991000001240",
        "Messlokation, Mehrwertdienste, zur einmaligen Übermittlung",
        Smgw,
        Messlokation,
        Momentanwerte,
        Momentanwerte,
        VerbrauchUndErzeugung,
        Direktverbindung,
        Optional,
        date!(2023 - 10 - 01)
    ),
    Messprodukt {
        code: "9991000001258",
        bezeichnung: "Messlokation, Mehrwertdienste, Schwellwert",
        weg: Uebertragungsweg::Smgw,
        ebene: Lokationsebene::Messlokation,
        wert: Wert::Momentanwerte,
        werteart: Werteart::Momentanwerte,
        richtung: Energieflussrichtung::VerbrauchUndErzeugung,
        rhythmus: Lieferrhythmus::Direktverbindung,
        verbindlichkeit: Verbindlichkeit::Optional,
        nutzbar_ab: date!(2023 - 10 - 01),
        schwellwertgesteuert: true,
    },
];

/// Normalise a Messprodukt-Code: strip the grouping spaces the Codeliste
/// prints it with (`"9991 00000 305 6"` → `"9991000000305 6"`… → digits only).
#[must_use]
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

/// Look up a Messprodukt by its code, accepting the spaced published form.
///
/// Returns `None` for any code outside Kapitel 4.6 — including the Kapitel 2
/// Typ-1 Standard-Messprodukte, which the ESA role may never order.
#[must_use]
pub fn messprodukt(code: &str) -> Option<&'static Messprodukt> {
    let normalized = normalize_code(code);
    BACKEND_PRODUKTE
        .iter()
        .chain(SMGW_PRODUKTE)
        .find(|p| p.code == normalized)
}

/// Every Messprodukt an MSB must serve on request (BNetzA *Mitteilung Nr. 3*).
pub fn pflichtprodukte() -> impl Iterator<Item = &'static Messprodukt> {
    BACKEND_PRODUKTE
        .iter()
        .chain(SMGW_PRODUKTE)
        .filter(|p| p.verbindlichkeit == Verbindlichkeit::Pflicht)
}

/// Why a [`Bestellgegenstand`] is not orderable as stated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProduktFehler {
    /// The code is not in Kapitel 4.6 at all.
    UnbekanntesMessprodukt {
        /// The rejected code, as supplied.
        code: String,
    },
    /// The product is defined for a different Lokationsebene than the request
    /// addresses (UC 4.1.1: MaLo-level asks name a MaLo-ID, MeLo-level a ZPB…).
    EbeneStimmtNicht {
        /// Level the product is defined for.
        produkt: Lokationsebene,
        /// Level the Werteanfrage addresses.
        anfrage: Lokationsebene,
    },
    /// The Wunschtermin precedes the product's „Nutzbar ab“ date.
    NochNichtNutzbar {
        /// Published „Nutzbar ab“.
        nutzbar_ab: Date,
        /// Requested first delivery.
        wunschtermin: Date,
    },
    /// A 4.6.2 product was ordered without the SM-PKI delivery target the
    /// REQOTE AHB makes mandatory for it (condition `[512]`).
    SmgwZielFehlt,
    /// A Schwellwert-triggered product was ordered without any threshold.
    SchwellwertFehlt,
    /// An SM-PKI delivery target was supplied for a 4.6.1 Backend product,
    /// which has no `SG27 LIN+Z68` line to carry it.
    SmgwZielUnzulaessig,
}

impl std::fmt::Display for ProduktFehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnbekanntesMessprodukt { code } => write!(
                f,
                "Messprodukt {code} steht nicht in der Codeliste der Konfigurationen 1.4 \
                 Kapitel 4.6 — nur diese Produkte darf die Marktrolle ESA bestellen"
            ),
            Self::EbeneStimmtNicht { produkt, anfrage } => write!(
                f,
                "Messprodukt ist für die Ebene {} definiert, die Werteanfrage adressiert \
                 aber eine {}",
                produkt.as_str(),
                anfrage.as_str()
            ),
            Self::NochNichtNutzbar {
                nutzbar_ab,
                wunschtermin,
            } => write!(
                f,
                "Messprodukt ist erst ab {nutzbar_ab} nutzbar, Wunschtermin ist {wunschtermin}"
            ),
            Self::SmgwZielFehlt => f.write_str(
                "Messprodukt aus Kapitel 4.6.2 (Werte aus SMGW) verlangt Zieladresse (IPv4 und \
                 IPv6) sowie Zertifikatsaussteller und -nutzer",
            ),
            Self::SchwellwertFehlt => f.write_str(
                "schwellwertgesteuertes Messprodukt verlangt mindestens einen \
                 Messprodukt-Position-Code mit oberem und unterem Schwellwert",
            ),
            Self::SmgwZielUnzulaessig => f.write_str(
                "Zieladresse/Zertifikate gehören zu Kapitel 4.6.2; ein Backend-Messprodukt \
                 (4.6.1) trägt sie nicht",
            ),
        }
    }
}

impl std::error::Error for ProduktFehler {}

// ── Abonnement (IMD 7081) ─────────────────────────────────────────────────────

/// Whether the order starts, ends or forgoes a running series.
///
/// `IMD+7081` is **Muss** on ORDERS 17007/17008 and on ORDRSP 19011/19012
/// (ORDERS AHB 1.1b §4.15, ORDRSP AHB 1.1b §4.15). It is also what decides
/// which EBD the answer cites and which of the two termination paths applies:
/// a one-shot ([`Self::OhneAbo`]) is stopped with the Stornierung, a running
/// series ([`Self::StartAbo`]) with the Abbestellung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Abonnement {
    /// `Z01` — Start Abo: turnusmäßige/regelmäßige Übermittlung.
    StartAbo,
    /// `Z02` — Ende Abo: carried by the Abbestellung (17008).
    EndeAbo,
    /// `Z03` — ohne Abo: einmalige Übermittlung (e.g. Vergangenheitswerte).
    OhneAbo,
}

impl Abonnement {
    /// `IMD` DE 7081 code.
    #[must_use]
    pub const fn imd_code(self) -> &'static str {
        match self {
            Self::StartAbo => "Z01",
            Self::EndeAbo => "Z02",
            Self::OhneAbo => "Z03",
        }
    }

    /// Parse an `IMD` DE 7081 code.
    #[must_use]
    pub fn from_imd_code(code: &str) -> Option<Self> {
        match code {
            "Z01" => Some(Self::StartAbo),
            "Z02" => Some(Self::EndeAbo),
            "Z03" => Some(Self::OhneAbo),
            _ => None,
        }
    }

    /// `true` when the order establishes a recurring delivery.
    ///
    /// UC 4.3's Vorbedingung („Es findet eine turnusmäßige/regelmäßige
    /// Übermittlung von Werten statt“) makes the Abbestellung meaningful only
    /// for these.
    #[must_use]
    pub const fn ist_abo(self) -> bool {
        matches!(self, Self::StartAbo)
    }
}

// ── SM-PKI delivery target (Kapitel 4.6.2) ────────────────────────────────────

/// One `SG28 CCI+Z60` threshold pair for a Schwellwert-triggered SMGW product.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schwellwert {
    /// Messprodukt-Position-Code from *Codeliste der Konfigurationen* 1.4
    /// Kapitel 4.7 (`CCI` DE 7037).
    pub position_code: String,
    /// Upper threshold (`CCI` DE 7036, first occurrence).
    pub oberer: String,
    /// Lower threshold (`CCI` DE 7036, second occurrence).
    pub unterer: String,
}

/// Where the iMS delivers, and under which certificates.
///
/// Mandatory content of the Werteanfrage for every Kapitel 4.6.2 product
/// (REQOTE AHB 1.2 §4.3, `SG27 FTX+Z17/Z24/Z23`, condition `[512]`). Both an
/// IPv4 and an IPv6 URI are required — the AHB lists two `FTX+Z17` DE 4440
/// occurrences with hints `[515]` and `[516]`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmgwZiel {
    /// `FTX+Z17` DE 4440 #1 — IPv4 URI.
    pub uri_ipv4: String,
    /// `FTX+Z17` DE 4440 #2 — IPv6 URI.
    pub uri_ipv6: String,
    /// `FTX+Z24` — Zertifikatsaussteller (X.509 per BSI TR-03109-4).
    pub zertifikat_aussteller: String,
    /// `FTX+Z23` — Zertifikatsnutzer (X.509 per BSI TR-03109-4).
    pub zertifikat_nutzer: String,
    /// `SG28 CCI+Z60` thresholds, for Schwellwert-triggered products.
    #[serde(default)]
    pub schwellwerte: Vec<Schwellwert>,
}

// ── Bestellgegenstand ─────────────────────────────────────────────────────────

/// What an ESA Wertebestellung is actually about.
///
/// Threaded through both aggregates so the process state answers "what was
/// ordered, for when, as a subscription or a one-shot" without re-reading the
/// wire — which the MSB side needs to fulfil the order and the ESA side needs
/// to notice a delivery that never arrived.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bestellgegenstand {
    /// Messprodukt-Code (`SG27 PIA+5` DE 7140), digits only.
    pub messprodukt: String,
    /// `DTM+76` „Datum zum geplanten Leistungsbeginn“ — the ESA's Wunschtermin
    /// for the first delivery (WiM Teil 2 UC 4.1.2 Nr. 1: *"Der ESA gibt u. a.
    /// seinen Wunschtermin für die erstmalige Übermittlung von Werten mit"*).
    pub wunschtermin: Date,
    /// End of the period the values are requested for. `None` for an open
    /// subscription; set for a bounded historical request. UC 4.1.1 bounds it
    /// to the span the Anschlussnutzer was assigned to the location.
    #[serde(default)]
    pub zeitraum_bis: Option<Date>,
    /// `IMD+7081` — subscription or one-shot.
    pub abonnement: Abonnement,
    /// SM-PKI delivery target; present exactly for Kapitel 4.6.2 products.
    #[serde(default)]
    pub smgw: Option<SmgwZiel>,
}

impl Bestellgegenstand {
    /// Resolve the catalogue entry.
    ///
    /// # Errors
    ///
    /// [`ProduktFehler::UnbekanntesMessprodukt`] when the code is outside
    /// *Codeliste der Konfigurationen* Kapitel 4.6.
    pub fn produkt(&self) -> Result<&'static Messprodukt, ProduktFehler> {
        messprodukt(&self.messprodukt).ok_or_else(|| ProduktFehler::UnbekanntesMessprodukt {
            code: self.messprodukt.clone(),
        })
    }

    /// Check the order against the catalogue and the level it addresses.
    ///
    /// # Errors
    ///
    /// A [`ProduktFehler`] naming the first violated constraint.
    pub fn validate(&self, anfrage_ebene: Lokationsebene) -> Result<(), ProduktFehler> {
        let p = self.produkt()?;
        if p.ebene != anfrage_ebene {
            return Err(ProduktFehler::EbeneStimmtNicht {
                produkt: p.ebene,
                anfrage: anfrage_ebene,
            });
        }
        if self.wunschtermin < p.nutzbar_ab {
            return Err(ProduktFehler::NochNichtNutzbar {
                nutzbar_ab: p.nutzbar_ab,
                wunschtermin: self.wunschtermin,
            });
        }
        match (p.weg, self.smgw.as_ref()) {
            (Uebertragungsweg::Smgw, None) => return Err(ProduktFehler::SmgwZielFehlt),
            (Uebertragungsweg::Backend, Some(_)) => {
                return Err(ProduktFehler::SmgwZielUnzulaessig);
            }
            (Uebertragungsweg::Smgw, Some(ziel)) => {
                if ziel.uri_ipv4.is_empty()
                    || ziel.uri_ipv6.is_empty()
                    || ziel.zertifikat_aussteller.is_empty()
                    || ziel.zertifikat_nutzer.is_empty()
                {
                    return Err(ProduktFehler::SmgwZielFehlt);
                }
                if p.schwellwertgesteuert && ziel.schwellwerte.is_empty() {
                    return Err(ProduktFehler::SchwellwertFehlt);
                }
            }
            (Uebertragungsweg::Backend, None) => {}
        }
        Ok(())
    }
}

/// The business key one ESA subscription occupies: the location **and** the
/// Messprodukt.
///
/// A location alone is too coarse. The Kapitel-4.6 catalogue offers several
/// products for the same Marktlokation — `9991 00000 305 6` (Wirkarbeit
/// Lastgang ¼ h) and `9991 00000 314 7` (Energiemenge über ein Zeitintervall)
/// among them — and nothing in WiM Teil 2 says an ESA may hold only one at a
/// time. Keying the duplicate guard on the location would refuse the second
/// order as a duplicate of the first.
#[must_use]
pub fn business_key(lokations_id: &str, messprodukt: &str) -> String {
    format!("{lokations_id}#{}", normalize_code(messprodukt))
}

// ── Correlation (Anwendungsübersicht der Prüfidentifikatoren 4.0) ──────────────

/// How a message of the ESA handshake is matched to its running process.
///
/// The BDEW *Anwendungsübersicht der Prüfidentifikatoren* 4.0 publishes one
/// Zuordnungsschlüssel per Prüfidentifikator. Only the opening REQOTE is keyed
/// on the location; every later step is keyed on a **Belegnummer** the message
/// echoes — and a conformant ORDERS, ORDCHG, ORDRSP or IFTSTA of this process
/// carries **no `LOC`** at all, so location keying cannot work for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Korrelation {
    /// `ZO-T17` — `SG11 LOC+172` DE 3225, the Meldepunkt. REQOTE 35003 only.
    Meldepunkt,
    /// `ZG-T16` — `SG1 RFF+AAV` DE 1154, the REQOTE's Belegnummer. QUOTES 15003.
    AnfrageNummer,
    /// `ZG-T24` — `SG1 RFF+AAG` DE 1154, the QUOTES Angebot's Belegnummer.
    /// ORDERS 17007.
    AngebotsNummer,
    /// `ZG-T14` / `ZG-T51` — `SG1 RFF+ON` DE 1154, the ORDERS' Belegnummer.
    /// ORDRSP 19011/19012 and ORDCHG 39002.
    AuftragsNummer,
    /// `ZG-T41` / `ZG-T50` — `SG1 RFF+ACW` DE 1154. On ORDERS 17008 it is the
    /// 17007's Belegnummer; on ORDRSP 19013/19014 the ORDCHG's.
    VorherigeNachricht,
    /// `ZG-T47` — `SG15 RFF+AGI` DE 1154, the ORDERS' Belegnummer. IFTSTA 21042.
    BeantragungsNummer,
}

impl Korrelation {
    /// The `RFF` DE 1153 qualifier that carries this key, if it is a reference.
    ///
    /// `None` for [`Self::Meldepunkt`], which is a `LOC` rather than an `RFF`.
    #[must_use]
    pub const fn rff_qualifier(self) -> Option<&'static str> {
        match self {
            Self::Meldepunkt => None,
            Self::AnfrageNummer => Some("AAV"),
            Self::AngebotsNummer => Some("AAG"),
            Self::AuftragsNummer => Some("ON"),
            Self::VorherigeNachricht => Some("ACW"),
            Self::BeantragungsNummer => Some("AGI"),
        }
    }
}

/// The published Zuordnungsschlüssel of an ESA-Wertebestellung PID.
///
/// `None` for any PID outside WiM Strom Teil 2 Kapitel 4.
#[must_use]
pub const fn korrelation(pid: u32) -> Option<Korrelation> {
    match pid {
        35003 => Some(Korrelation::Meldepunkt),
        15003 => Some(Korrelation::AnfrageNummer),
        17007 => Some(Korrelation::AngebotsNummer),
        19011 | 19012 | 39002 => Some(Korrelation::AuftragsNummer),
        17008 | 19013 | 19014 => Some(Korrelation::VorherigeNachricht),
        // MSCONS AHB 3.2 §11.2 hint `[574]`: „Wert aus BGM DE1004 der ORDERS
        // mit der die Bestellung der Werte nach Typ 2 erfolgt ist" — the same
        // `RFF+AGI` shape as the IFTSTA, and the only thing on a value
        // delivery that names the subscription it belongs to.
        21042 | 13027 => Some(Korrelation::BeantragungsNummer),
        _ => None,
    }
}

// ── EBD (ORDRSP SG2 AJT) ──────────────────────────────────────────────────────
//
// The three answer trees of this process — `E_0256` Bestellung, `E_0257`
// Stornierung, `E_0254` Beendigung — live in [`mako_pruefung::msb::esa`]
// together with their Codelisten and executable Prüfschritte, for the same
// reason every other answer tree does: an Antwortcode has no meaning without
// naming its tree (`A01` means three different things across the three), and
// the code's **Cluster** — not an `accept: bool` passed alongside — is what
// decides whether the ORDRSP rides 19011 or 19012.
//
// [`mako_pruefung::msb::esa::ebd_fuer_antwort`] resolves which tree an
// ORDRSP 19011/19012 belongs to from the `IMD+7081` it carries, since those
// two PIDs answer both the Bestellung and the Beendigung.

pub use mako_pruefung::codes::{EBD_ESA_BEENDIGUNG, EBD_ESA_BESTELLUNG, EBD_ESA_STORNIERUNG};
pub use mako_pruefung::msb::esa::{
    Bestellart, EsaBeendigung, EsaBestellung, EsaStornierung, ebd_fuer_antwort, pruefe_beendigung,
    pruefe_bestellung, pruefe_stornierung,
};

impl Abonnement {
    /// The [`Bestellart`] this Abo mode denotes, for the `mako-pruefung` walks.
    ///
    /// `Z02` (Ende Abo) can only appear on an Abbestellung, which by
    /// definition ends a running series.
    #[must_use]
    pub const fn bestellart(self) -> Bestellart {
        match self {
            Self::StartAbo | Self::EndeAbo => Bestellart::Abo,
            Self::OhneAbo => Bestellart::Einmalig,
        }
    }

    /// The EBD an ORDRSP answering an order with this `IMD+7081` must cite.
    #[must_use]
    pub const fn antwort_ebd(self) -> &'static str {
        match self {
            Self::EndeAbo => EBD_ESA_BEENDIGUNG,
            Self::StartAbo | Self::OhneAbo => EBD_ESA_BESTELLUNG,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue is keyed by code; a duplicate would silently shadow a row.
    #[test]
    fn messprodukt_codes_are_unique_and_thirteen_digits() {
        let mut seen = std::collections::HashSet::new();
        for p in BACKEND_PRODUKTE.iter().chain(SMGW_PRODUKTE) {
            assert!(seen.insert(p.code), "duplicate Messprodukt-Code {}", p.code);
            assert_eq!(p.code.len(), 13, "{} is not 13 digits", p.code);
            assert!(p.code.chars().all(|c| c.is_ascii_digit()), "{}", p.code);
        }
    }

    /// The published form carries grouping spaces; lookup must accept both.
    #[test]
    fn lookup_accepts_the_spaced_published_form() {
        let p = messprodukt("9991 00000 305 6").expect("Pflichtprodukt 305 6");
        assert_eq!(p.code, "9991000003056");
        assert_eq!(p.ebene, Lokationsebene::Marktlokation);
        assert_eq!(p.verbindlichkeit, Verbindlichkeit::Pflicht);
        assert_eq!(messprodukt("9991000003056").map(|q| q.code), Some(p.code));
    }

    /// A Typ-1 Standard-Messprodukt is not orderable by the ESA role.
    #[test]
    fn unknown_code_is_rejected() {
        assert!(messprodukt("9992000000011").is_none());
    }

    /// BNetzA Mitteilung Nr. 3 makes seven products mandatory — five EDIFACT
    /// (305 6, 306 4, 314 7, 077 1, 078 9) and two SMGW (312 1, 313 9).
    #[test]
    fn seven_products_are_pflicht() {
        let codes: Vec<_> = pflichtprodukte().map(|p| p.code).collect();
        assert_eq!(codes.len(), 7, "{codes:?}");
        for expected in [
            "9991000003056",
            "9991000003064",
            "9991000003147",
            "9991000000771",
            "9991000000789",
            "9991000003121",
            "9991000003139",
        ] {
            assert!(
                codes.contains(&expected),
                "{expected} missing from {codes:?}"
            );
        }
    }

    fn backend_order() -> Bestellgegenstand {
        Bestellgegenstand {
            messprodukt: "9991000003056".to_owned(),
            wunschtermin: time::macros::date!(2026 - 03 - 01),
            zeitraum_bis: None,
            abonnement: Abonnement::StartAbo,
            smgw: None,
        }
    }

    #[test]
    fn backend_order_on_its_own_level_validates() {
        assert_eq!(
            backend_order().validate(Lokationsebene::Marktlokation),
            Ok(())
        );
    }

    /// 305 6 is a Marktlokation product; asking for it with a ZPB is a
    /// mis-addressed request the MSB would have to reject.
    #[test]
    fn level_mismatch_is_refused() {
        assert_eq!(
            backend_order().validate(Lokationsebene::Messlokation),
            Err(ProduktFehler::EbeneStimmtNicht {
                produkt: Lokationsebene::Marktlokation,
                anfrage: Lokationsebene::Messlokation,
            })
        );
    }

    /// The Pflicht products are usable from 06.08.2024; a Wunschtermin before
    /// that asks for a product that did not exist yet.
    #[test]
    fn wunschtermin_before_nutzbar_ab_is_refused() {
        let mut o = backend_order();
        o.wunschtermin = time::macros::date!(2024 - 01 - 01);
        assert_eq!(
            o.validate(Lokationsebene::Marktlokation),
            Err(ProduktFehler::NochNichtNutzbar {
                nutzbar_ab: time::macros::date!(2024 - 08 - 06),
                wunschtermin: time::macros::date!(2024 - 01 - 01),
            })
        );
    }

    /// REQOTE AHB condition `[512]`: a 4.6.2 product needs the SM-PKI target.
    #[test]
    fn smgw_product_without_target_is_refused() {
        let o = Bestellgegenstand {
            messprodukt: "9991000003121".to_owned(),
            ..backend_order()
        };
        assert_eq!(
            o.validate(Lokationsebene::Messlokation),
            Err(ProduktFehler::SmgwZielFehlt)
        );
    }

    /// …and a Backend product has no `LIN+Z68` line to carry one.
    #[test]
    fn backend_product_with_smgw_target_is_refused() {
        let o = Bestellgegenstand {
            smgw: Some(SmgwZiel {
                uri_ipv4: "https://192.0.2.1/esa".to_owned(),
                uri_ipv6: "https://[2001:db8::1]/esa".to_owned(),
                zertifikat_aussteller: "CN=Test-CA".to_owned(),
                zertifikat_nutzer: "CN=Test-ESA".to_owned(),
                schwellwerte: Vec::new(),
            }),
            ..backend_order()
        };
        assert_eq!(
            o.validate(Lokationsebene::Marktlokation),
            Err(ProduktFehler::SmgwZielUnzulaessig)
        );
    }

    /// Conditions `[43]`/`[2066]`: a Schwellwert-triggered product needs thresholds.
    #[test]
    fn schwellwert_product_without_thresholds_is_refused() {
        let o = Bestellgegenstand {
            messprodukt: "9991000001216".to_owned(),
            smgw: Some(SmgwZiel {
                uri_ipv4: "https://192.0.2.1/esa".to_owned(),
                uri_ipv6: "https://[2001:db8::1]/esa".to_owned(),
                zertifikat_aussteller: "CN=Test-CA".to_owned(),
                zertifikat_nutzer: "CN=Test-ESA".to_owned(),
                schwellwerte: Vec::new(),
            }),
            ..backend_order()
        };
        assert_eq!(
            o.validate(Lokationsebene::Messlokation),
            Err(ProduktFehler::SchwellwertFehlt)
        );
    }

    /// The Zuordnungsschlüssel table of the PID overview 4.0, verbatim.
    ///
    /// The two easily-inverted ones: the answer to a Bestellung references
    /// `RFF+ON` (not `ACW`), and the Abbestellung itself carries `RFF+ACW`.
    /// Two different Messprodukte at one Marktlokation are two subscriptions.
    #[test]
    fn the_business_key_separates_products_at_one_location() {
        let lastgang = business_key("51238696012", "9991 00000 305 6");
        let energiemenge = business_key("51238696012", "9991000003147");
        assert_ne!(lastgang, energiemenge);
        // The spaced and bare forms of one code are the same subscription.
        assert_eq!(lastgang, business_key("51238696012", "9991000003056"));
    }

    #[test]
    fn correlation_keys_match_the_pid_overview() {
        for (pid, expected, qual) in [
            (35003, Korrelation::Meldepunkt, None),
            (15003, Korrelation::AnfrageNummer, Some("AAV")),
            (17007, Korrelation::AngebotsNummer, Some("AAG")),
            (17008, Korrelation::VorherigeNachricht, Some("ACW")),
            (39002, Korrelation::AuftragsNummer, Some("ON")),
            (19011, Korrelation::AuftragsNummer, Some("ON")),
            (19012, Korrelation::AuftragsNummer, Some("ON")),
            (19013, Korrelation::VorherigeNachricht, Some("ACW")),
            (19014, Korrelation::VorherigeNachricht, Some("ACW")),
            (21042, Korrelation::BeantragungsNummer, Some("AGI")),
            (13027, Korrelation::BeantragungsNummer, Some("AGI")),
        ] {
            assert_eq!(korrelation(pid), Some(expected), "PID {pid}");
            assert_eq!(expected.rff_qualifier(), qual, "PID {pid}");
        }
        assert_eq!(korrelation(31009), None, "billing is a different process");
    }

    /// ORDRSP AHB conditions `[21]`–`[23]`, resolved against the `mako-pruefung`
    /// catalogue rather than a local copy of the tree ids.
    #[test]
    fn ebd_follows_the_abonnement() {
        assert_eq!(Abonnement::StartAbo.antwort_ebd(), EBD_ESA_BESTELLUNG);
        assert_eq!(Abonnement::OhneAbo.antwort_ebd(), EBD_ESA_BESTELLUNG);
        assert_eq!(Abonnement::EndeAbo.antwort_ebd(), EBD_ESA_BEENDIGUNG);
        assert_eq!(
            ebd_fuer_antwort(Abonnement::EndeAbo.imd_code()),
            Some(EBD_ESA_BEENDIGUNG)
        );
    }

    /// A one-shot order is `Einmalig` on both sides of the crate boundary —
    /// `E_0257` refuses a started one-shot with `A03`, an Abo with `A02`.
    #[test]
    fn the_abo_mode_maps_onto_the_pruefung_bestellart() {
        assert_eq!(Abonnement::StartAbo.bestellart(), Bestellart::Abo);
        assert_eq!(Abonnement::EndeAbo.bestellart(), Bestellart::Abo);
        assert_eq!(Abonnement::OhneAbo.bestellart(), Bestellart::Einmalig);
    }

    #[test]
    fn imd_codes_round_trip() {
        for a in [
            Abonnement::StartAbo,
            Abonnement::EndeAbo,
            Abonnement::OhneAbo,
        ] {
            assert_eq!(Abonnement::from_imd_code(a.imd_code()), Some(a));
        }
        assert_eq!(Abonnement::from_imd_code("Z99"), None);
    }

    /// Rohdaten products publish a wall-clock deadline; the aufbereitete-Daten
    /// products defer to WiM Kapitel 2.5.5 and must not be given a fake one.
    #[test]
    fn only_rohdaten_products_publish_a_daily_clock() {
        let roh = messprodukt("9991000000416").unwrap();
        assert_eq!(roh.rhythmus.taegliche_frist(), Some(ROHDATEN_FRIST));
        let aufbereitet = messprodukt("9991000003056").unwrap();
        assert_eq!(aufbereitet.rhythmus.taegliche_frist(), None);
    }
}
