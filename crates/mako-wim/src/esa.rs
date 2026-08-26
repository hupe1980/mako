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

/// Whether the MSB must serve the product or may decline it — **as of a date**.
///
/// The Codeliste's „Pflicht / Optional" column is not a constant. Two rows
/// carry both values with a cut-over date („Optional ab 01.10.2023, Pflicht ab
/// 06.08.2024" — `9991 00000 077 1` and `078 9`), and every other Pflicht
/// product became mandatory on 06.08.2024 while having existed as an optional
/// product or not at all before.
///
/// For a role whose entire premise includes **Vergangenheitswerte** that
/// distinction is load-bearing: whether `E_0252` Prüfschritt 1 may skip the
/// MSB's commercial discretion depends on what the product was at the period
/// being requested, not on what it is today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verbindlichkeit {
    /// The MSB must offer it from `ab` — BNetzA *Mitteilung Nr. 3* (07.02.2024).
    /// Before that date the product may exist but is Optional.
    Pflicht {
        /// First day the Pflicht applies.
        ab: Date,
    },
    /// Optional for the product's whole life; the MSB may decline the Anfrage.
    Optional,
}

impl Verbindlichkeit {
    /// Whether the MSB is obliged to serve the product for a delivery on `am`.
    #[must_use]
    pub const fn ist_pflicht_am(self, am: Date) -> bool {
        match self {
            Self::Pflicht { ab } => am.to_julian_day() >= ab.to_julian_day(),
            Self::Optional => false,
        }
    }

    /// Whether the product is ever mandatory. Use [`Self::ist_pflicht_am`] to
    /// decide an actual order — a Pflicht that has not started yet is not one.
    #[must_use]
    pub const fn jemals_pflicht(self) -> bool {
        matches!(self, Self::Pflicht { .. })
    }
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
        $art:ident, $ri:ident, $rh:ident, $vb:expr, $nutzbar_ab:expr
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
            verbindlichkeit: $vb,
            nutzbar_ab: $nutzbar_ab,
            schwellwertgesteuert: false,
        }
    };
}

/// „Pflicht ab 06.08.2024" — the date BNetzA *Mitteilung Nr. 3* (07.02.2024)
/// set for every mandatory ESA Messprodukt in Codeliste Kapitel 4.6.
const PFLICHT: Verbindlichkeit = Verbindlichkeit::Pflicht {
    ab: date!(2024 - 08 - 06),
};

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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        PFLICHT,
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
        Verbindlichkeit::Optional,
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
        PFLICHT,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        PFLICHT,
        date!(2023 - 10 - 01)
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
        PFLICHT,
        date!(2023 - 10 - 01)
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        PFLICHT,
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
        Verbindlichkeit::Optional,
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
        PFLICHT,
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
        Verbindlichkeit::Optional,
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
        PFLICHT,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
        Verbindlichkeit::Optional,
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
///
/// „Must serve" is dated: use [`Messprodukt::ist_pflicht_am`] to decide an
/// actual order, since a historical Werteanfrage may reach back before the
/// Pflicht began.
pub fn pflichtprodukte() -> impl Iterator<Item = &'static Messprodukt> {
    BACKEND_PRODUKTE
        .iter()
        .chain(SMGW_PRODUKTE)
        .filter(|p| p.verbindlichkeit.jemals_pflicht())
}

/// The [`Lokationsebene`] a Messprodukt-Code is defined for.
///
/// **The product decides the level, not the identifier.** REQOTE AHB 1.2 §4.3
/// gives `LOC+172` DE 3225 four permitted shapes and lets the Marktlokations-ID
/// format (`[950]`) serve *both* the Marktlokation (`[502]`) and the Tranche
/// (`[504]`) — so an 11-digit identifier is provably ambiguous and no amount of
/// inspection of it can resolve the level. The `SG27 PIA+5` Messprodukt can,
/// and does so for every one of the four.
///
/// Returns `None` only for a code outside Kapitel 4.6, which is not orderable
/// by this Marktrolle at all.
#[must_use]
pub fn ebene_fuer_messprodukt(code: &str) -> Option<Lokationsebene> {
    messprodukt(code).map(|p| p.ebene)
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

impl Messprodukt {
    /// Whether the MSB is obliged to serve this product for a delivery on `am`.
    ///
    /// `E_0252` Prüfschritt 1 branches on exactly this, and it is what lets an
    /// MSB deployment answer a Werteanfrage without an operator.
    #[must_use]
    pub const fn ist_pflicht_am(&self, am: Date) -> bool {
        self.verbindlichkeit.ist_pflicht_am(am)
    }
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

// ── Antwort (ORDRSP SG2 AJT) ──────────────────────────────────────────────────

/// What an ORDRSP actually said — the published Antwortcode and its tree.
///
/// `SG2 AJT` is **Muss** on all four ESA answer PIDs (ORDRSP AHB 1.1b §4.15),
/// DE 4465 carrying the Code des Prüfschritts and DE 1082 the EBD that
/// publishes it. Those four use cases have **no free-text `FTX` segment at
/// all** — the only `FTX` a conformant 19011 may carry is `SG27 FTX+Z27`, the
/// MSB's IP address — so this is the entire content of a refusal.
///
/// Kept as a typed pair rather than a bare string because a code has no
/// meaning without its tree: `A01` is „Bindungsfrist abgelaufen" in `E_0256`,
/// „Bestellung nicht bestätigt" in `E_0257` and „war eine einmalige
/// Übermittlung" in `E_0254`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Antwort {
    /// `AJT` DE 4465 — the Code des Prüfschritts.
    pub antwortcode: String,
    /// `AJT` DE 1082 — the EBD the code was published in (`E_0254`, `E_0256`,
    /// `E_0257`). `None` only from a counterparty that omitted the Muss.
    #[serde(default)]
    pub ebd: Option<String>,
}

impl Antwort {
    /// Build one from the two `AJT` data elements.
    #[must_use]
    pub fn new(antwortcode: impl Into<String>, ebd: Option<String>) -> Self {
        Self {
            antwortcode: antwortcode.into(),
            ebd,
        }
    }

    /// Resolve the code against its tree and return the BDEW's own wording.
    ///
    /// `None` when the answer names no tree, or names a code that tree does
    /// not publish — either is a non-conformant answer, and reporting it as
    /// unresolved is honest where inventing a meaning is not.
    #[must_use]
    pub fn bedeutung(&self) -> Option<&'static str> {
        let tree = self.ebd.as_deref()?;
        mako_pruefung::codes::lookup(tree, &self.antwortcode).map(|c| c.bedeutung)
    }

    /// Whether the code sits in its tree's **Zustimmungs**-Cluster.
    ///
    /// ORDRSP AHB conditions `[17]`/`[18]` bind the cluster to the answer PID,
    /// so this and the PID must agree; [`Self::widerspricht_pid`] is that check.
    #[must_use]
    pub fn ist_zustimmung(&self) -> Option<bool> {
        let tree = self.ebd.as_deref()?;
        mako_pruefung::codes::lookup(tree, &self.antwortcode)
            .map(|c| c.cluster == mako_pruefung::codes::Cluster::Zustimmung)
    }

    /// `true` when the code's Cluster contradicts the PID that carried it.
    ///
    /// A 19011 (Bestätigung) whose `AJT` names an Ablehnungscode is not a
    /// confirmation with a note attached — it is a message whose two halves
    /// disagree, and acting on either half alone is guesswork. `false` when
    /// the answer cannot be resolved at all, which is a separate defect.
    #[must_use]
    pub fn widerspricht_pid(&self, pid_ist_zustimmung: bool) -> bool {
        self.ist_zustimmung()
            .is_some_and(|zustimmung| zustimmung != pid_ist_zustimmung)
    }

    /// One-line rendering for an operator queue or an audit log.
    #[must_use]
    pub fn beschreibung(&self) -> String {
        let tree = self.ebd.as_deref().unwrap_or("ohne EBD");
        self.bedeutung().map_or_else(
            || {
                format!(
                    "{} ({tree}, im Codelistenkatalog nicht geführt)",
                    self.antwortcode
                )
            },
            |b| format!("{} ({tree}): {b}", self.antwortcode),
        )
    }
}

// ── Angebot (QUOTES 15003) ────────────────────────────────────────────────────

/// One `SG31 PRI+CAL` price of the MSB's Angebot.
///
/// QUOTES AHB 1.1a §4.3: `SG31` is **Muss** and repeats once per `PIA+Z02`
/// Artikel-ID in the same `SG27 LIN`, up to three times. The Artikel-ID's last
/// two digits pick the price type — `01` Einrichtung, `02` Betrieb, `03`
/// Transaktion — which is what conditions `[83]`–`[85]` say.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preisposition {
    /// `SG27 PIA+Z02` DE 7140 — the Artikel-ID this price belongs to.
    pub artikel_id: String,
    /// `SG31 PRI` DE 5387 — `Z01` Einrichtungspreis, `Z02` Transaktionspreis,
    /// `Z03` Betriebspreis.
    pub preistyp: Preistyp,
    /// `SG31 PRI` DE 5118 — the amount, verbatim from the wire (up to six
    /// decimals). Kept as text so no rounding happens before the ESA's own
    /// ledger sees it.
    pub betrag: String,
    /// `SG31 PRI` DE 6411 — `H87` Stück (Einrichtung, Transaktion) or `DAY`
    /// Tag (Betrieb).
    pub einheit: String,
}

/// `SG31 PRI` DE 5387 — which of the three ESA price types this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preistyp {
    /// `Z01` — Einrichtungspreis, once per Stück.
    Einrichtung,
    /// `Z02` — Transaktionspreis, per Stück.
    Transaktion,
    /// `Z03` — Betriebspreis, per Tag.
    Betrieb,
}

impl Preistyp {
    /// `PRI` DE 5387 code.
    #[must_use]
    pub const fn pri_code(self) -> &'static str {
        match self {
            Self::Einrichtung => "Z01",
            Self::Transaktion => "Z02",
            Self::Betrieb => "Z03",
        }
    }

    /// Parse a `PRI` DE 5387 code.
    #[must_use]
    pub fn from_pri_code(code: &str) -> Option<Self> {
        match code {
            "Z01" => Some(Self::Einrichtung),
            "Z02" => Some(Self::Transaktion),
            "Z03" => Some(Self::Betrieb),
            _ => None,
        }
    }
}

/// The substance of the MSB's QUOTES 15003 Angebot.
///
/// UC 4.1.1 says the ESA „fragt die Übermittlung von Werten **und die damit
/// verbundenen Kosten**" and UC 4.1 Nr. 2 that the MSB states „wie hoch die
/// damit verbundenen Kosten sind". The offer is therefore what the ESA orders
/// against, what the MSB's later INVOIC 31009 is reconciled with, and what says
/// which registers the subscription will deliver.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Angebot {
    /// `SG4 CUX` DE 6345 — the currency the prices are in. **Muss**; `EUR` is
    /// the only published value, but it is read rather than assumed.
    #[serde(default)]
    pub waehrung: Option<String>,
    /// `SG31 PRI` — one entry per (Artikel-ID, Preistyp) the offer prices.
    #[serde(default)]
    pub preise: Vec<Preisposition>,
    /// `SG27 PIA+5 …:SRW` — the OBIS-Kennzahlen the subscription will deliver.
    ///
    /// **Muss**, one to 23 per `SG27 LIN` (QUOTES AHB 1.1a condition `[2073]`).
    /// This is the only place the ESA learns which registers to expect, and it
    /// is what a delivery-surveillance sweep has to compare against —
    /// `ZO-T21` of `EZ-03` routes an inbound MSCONS 13027 by exactly this.
    #[serde(default)]
    pub obis_kennzahlen: Vec<String>,
    /// `DTM+279` — „Erforderliche Zeitspanne zur Einrichtung der Übermittlung
    /// von Werten ab Bestellung", a **duration** (DE 2379 `802`/`803`/`804`),
    /// resolved against the day the Angebot arrived. `Kann`.
    #[serde(default)]
    pub einrichtung_bis: Option<time::OffsetDateTime>,
}

impl Angebot {
    /// `true` when the offer prices nothing.
    ///
    /// `SG31 PRI` is **Muss** inside a `SG27 LIN` position, and the position
    /// block is what a priced offer consists of — so a 15003 with no price is
    /// the MSB saying it will not deliver. The QUOTES AHB publishes no
    /// Ablehnung use case and `DTM+273` is Muss on the one it does publish, so
    /// the absence of a Bindungsfrist cannot be the discriminator.
    #[must_use]
    pub fn ist_leer(&self) -> bool {
        self.preise.is_empty() && self.obis_kennzahlen.is_empty()
    }

    /// The offered price of one type, if the offer names it.
    #[must_use]
    pub fn preis(&self, typ: Preistyp) -> Option<&Preisposition> {
        self.preise.iter().find(|p| p.preistyp == typ)
    }
}

/// Where the MSB will push SM-PKI values **from** (ORDRSP 19011, `SG27`).
///
/// ORDRSP AHB 1.1b §4.15: when the confirmed order named a Kapitel-4.6.2
/// product, the Bestätigung must carry either `FTX+Z27` (a single IP address,
/// condition `[77]`) or `FTX+Z28` (a range, condition `[76]`). The ESA has to
/// admit that source before the iMS can reach it, so dropping it left a
/// confirmed SMGW subscription that could never deliver.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "art", content = "wert")]
pub enum SmgwQuelle {
    /// `FTX+Z27` — one IP address.
    Adresse(String),
    /// `FTX+Z28` — the lower and upper bound of an IP range.
    Range {
        /// `FTX+Z28` DE 4440 #1 — untere Grenze.
        von: String,
        /// `FTX+Z28` DE 4440 #2 — obere Grenze.
        bis: String,
    },
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
    /// The ORDERS' Belegnummer, echoed as `RFF+AGI` — but in a different
    /// segment group per message: `ZG-T47` is `SG15 RFF+AGI` on the IFTSTA
    /// 21042, `ZG-T42` is `SG1 RFF+AGI` on the MSCONS 13027. Only the
    /// qualifier is shared, which is why this is one key and the lookup
    /// searches by qualifier rather than by position.
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
        // `RFF+AGI` qualifier as the IFTSTA (in `SG1` rather than `SG15`), and
        // the only thing on a value delivery that names the subscription it
        // belongs to. It is the first hop of the PID overview's `EZ-03`
        // (`ZG-T42` → `ZO-T20` Gerätenummer → `ZO-T21` OBIS-Kennzahl); the two
        // later hops assign the values to a Zählwerk and belong to `edmd`.
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
        assert!(p.ist_pflicht_am(time::macros::date!(2026 - 03 - 01)));
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

    /// Two different Messprodukte at one Marktlokation are two subscriptions.
    #[test]
    fn the_business_key_separates_products_at_one_location() {
        let lastgang = business_key("51238696012", "9991 00000 305 6");
        let energiemenge = business_key("51238696012", "9991000003147");
        assert_ne!(lastgang, energiemenge);
        // The spaced and bare forms of one code are the same subscription.
        assert_eq!(lastgang, business_key("51238696012", "9991000003056"));
    }

    /// The Zuordnungsschlüssel table of the PID overview 4.0, verbatim.
    ///
    /// The two easily-inverted ones: the answer to a Bestellung references
    /// `RFF+ON` (not `ACW`), and the Abbestellung itself carries `RFF+ACW`.
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

    /// The Codeliste dates the Pflicht — „Optional ab 01.10.2023, Pflicht ab
    /// 06.08.2024" for `077 1`/`078 9`. A Vergangenheitswerte-Anfrage reaching
    /// back before the cut-over asks for a product the MSB could still decline,
    /// which is what `E_0252` Prüfschritt 1 branches on.
    #[test]
    fn the_pflicht_is_dated_not_absolute() {
        let p = messprodukt("9991 00000 077 1").expect("Pflichtprodukt 077 1");
        assert!(!p.ist_pflicht_am(time::macros::date!(2024 - 08 - 05)));
        assert!(p.ist_pflicht_am(time::macros::date!(2024 - 08 - 06)));
        assert!(p.verbindlichkeit.jemals_pflicht());

        let optional = messprodukt("9991 00000 074 7").expect("optional 074 7");
        assert!(!optional.ist_pflicht_am(time::macros::date!(2026 - 01 - 01)));
        assert!(!optional.verbindlichkeit.jemals_pflicht());
    }

    /// …and the same two rows are usable from **01.10.2023**, a year before the
    /// Pflicht. Storing the Pflicht date as `nutzbar_ab` refused a legitimate
    /// historical Wunschtermin in between — for a role whose whole premise
    /// includes Vergangenheitswerte.
    #[test]
    fn nutzbar_ab_is_not_the_pflicht_date() {
        for code in ["9991000000771", "9991000000789"] {
            let p = messprodukt(code).expect("in the catalogue");
            assert_eq!(
                p.nutzbar_ab,
                time::macros::date!(2023 - 10 - 01),
                "{code}: Codeliste 1.4 Kap. 4.6.1 gives Nutzbar ab 01.10.2023"
            );
        }
    }

    /// The product, never the identifier, decides the level: REQOTE AHB 1.2
    /// §4.3 gives the Marktlokation (`[502]`) and the Tranche (`[504]`) the
    /// *same* `[950]` Marktlokations-ID format, so length inference cannot tell
    /// them apart — and the Tranche carries a Pflichtprodukt.
    #[test]
    fn the_product_resolves_the_level_where_the_identifier_cannot() {
        assert_eq!(
            ebene_fuer_messprodukt("9991 00000 305 6"),
            Some(Lokationsebene::Marktlokation)
        );
        assert_eq!(
            ebene_fuer_messprodukt("9991 00000 306 4"),
            Some(Lokationsebene::Tranche)
        );
        assert_eq!(ebene_fuer_messprodukt("9992000000011"), None);
        // Both are Pflicht, and both are addressed by an 11-digit identifier.
        assert!(pflichtprodukte().any(|p| p.ebene == Lokationsebene::Tranche));
    }

    /// An `AJT` code is meaningless without its tree, and the cluster it sits
    /// in has to agree with the PID that carried it.
    #[test]
    fn an_antwort_resolves_against_the_tree_it_names() {
        let ok = Antwort::new("A11", Some(EBD_ESA_BESTELLUNG.to_owned()));
        assert_eq!(ok.ist_zustimmung(), Some(true));
        assert!(ok.bedeutung().is_some());
        assert!(!ok.widerspricht_pid(true));
        // A Bestätigung PID carrying an Ablehnungscode is self-contradictory.
        assert!(ok.widerspricht_pid(false));

        let refusal = Antwort::new("A08", Some(EBD_ESA_BESTELLUNG.to_owned()));
        assert_eq!(refusal.ist_zustimmung(), Some(false));
        assert!(
            refusal
                .bedeutung()
                .expect("A08 is published")
                .contains("Einwilligung")
        );
    }

    /// `A01` means three different things across the three answer trees, so an
    /// answer that names no tree stays unresolved rather than being guessed.
    #[test]
    fn an_antwort_without_a_tree_is_not_guessed() {
        let bare = Antwort::new("A01", None);
        assert_eq!(bare.bedeutung(), None);
        assert_eq!(bare.ist_zustimmung(), None);
        assert!(
            !bare.widerspricht_pid(true),
            "unresolvable is not a conflict"
        );
        assert!(bare.beschreibung().contains("ohne EBD"));

        let per_tree: Vec<_> = [EBD_ESA_BESTELLUNG, EBD_ESA_STORNIERUNG, EBD_ESA_BEENDIGUNG]
            .into_iter()
            .map(|t| Antwort::new("A01", Some(t.to_owned())).bedeutung().unwrap())
            .collect();
        assert_eq!(
            per_tree
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "A01 must mean something different in each tree: {per_tree:?}"
        );
    }

    /// An Angebot with no price is the MSB declining. `DTM+273` cannot be the
    /// discriminator — QUOTES AHB 1.1a makes it Muss on the only published
    /// 15003 use case, so a refusal carries one too.
    #[test]
    fn an_offer_is_told_from_a_refusal_by_its_prices() {
        assert!(Angebot::default().ist_leer());
        let priced = Angebot {
            waehrung: Some("EUR".to_owned()),
            preise: vec![Preisposition {
                artikel_id: "9990001100002".to_owned(),
                preistyp: Preistyp::Betrieb,
                betrag: "0.004500".to_owned(),
                einheit: "DAY".to_owned(),
            }],
            obis_kennzahlen: vec!["1-1:1.29.0".to_owned()],
            einrichtung_bis: None,
        };
        assert!(!priced.ist_leer());
        assert_eq!(
            priced.preis(Preistyp::Betrieb).map(|p| p.betrag.as_str()),
            Some("0.004500")
        );
        assert_eq!(priced.preis(Preistyp::Einrichtung), None);
    }

    #[test]
    fn pri_codes_round_trip() {
        for t in [
            Preistyp::Einrichtung,
            Preistyp::Transaktion,
            Preistyp::Betrieb,
        ] {
            assert_eq!(Preistyp::from_pri_code(t.pri_code()), Some(t));
        }
        assert_eq!(Preistyp::from_pri_code("Z99"), None);
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
