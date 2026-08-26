//! `Stammdaten` -- technical master data exchange (ANB/DV to VNB, VNB to UNB) for registered Redispatch resources.
use serde::{Deserialize, Serialize};

use crate::types::{Decimal3, DocumentId, MarketParticipantId, Mrid, UtcDateTime};

// ── German-localised coding scheme for Stammdaten ────────────────────────────

/// `Codierung` coding scheme used in `Stammdaten` (German-localised variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codierung {
    /// GS1 (GLN/GSRN).
    #[serde(rename = "A10")]
    Gs1,
    /// Germany National coding scheme (BDEW-Code).
    #[serde(rename = "NDE")]
    Nde,
}

/// Sender / receiver reference for `Stammdaten` documents.
///
/// The `Code` and `Codierung` attributes are German-language equivalents of
/// `v` and `codingScheme` used in ENTSO-E documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StammdatenParticipantRef {
    /// 13-digit market participant identifier.
    #[serde(rename = "@Code")]
    pub code: MarketParticipantId,
    /// Coding scheme for the identifier.
    #[serde(rename = "@Codierung")]
    pub codierung: Codierung,
}

// ── DocumentType ──────────────────────────────────────────────────────────────

/// `DocumentType` codes for `Stammdaten`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StammdatenDocType {
    /// Reduced master data (reduzierte Stammdaten).
    #[serde(rename = "Z02")]
    Reduced,
    /// Enriched master data (angereicherte Stammdaten).
    #[serde(rename = "Z03")]
    Enriched,
    /// Grid operator aggregate master data.
    #[serde(rename = "Z04")]
    NbAggregate,
    /// Balance responsible party master data.
    #[serde(rename = "Z14")]
    Bilanzkreis,
}

/// Sender market role (`Senderrolle`) in `Stammdaten`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StammdatenSenderRole {
    /// Grid operator (Netzbetreiber, A18).
    #[serde(rename = "A18")]
    GridOperator,
    /// Resource provider (Anlagenbetreiber / Direktvermarkter, A27).
    #[serde(rename = "A27")]
    ResourceProvider,
    /// Data provider (Datenprovider, A39).
    #[serde(rename = "A39")]
    DataProvider,
    /// Supplier (Lieferant, Z01).
    #[serde(rename = "Z01")]
    Supplier,
}

/// Receiver market role (`Empfaengerrolle`) in `Stammdaten`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StammdatenReceiverRole {
    /// Balance responsible party (Bilanzkreisverantwortlicher, A08).
    #[serde(rename = "A08")]
    BalanceResponsibleParty,
    /// Grid operator (Netzbetreiber, A18).
    #[serde(rename = "A18")]
    GridOperator,
    /// Data provider (Datenprovider, A39).
    #[serde(rename = "A39")]
    DataProvider,
    /// Supplier (Lieferant, Z01).
    #[serde(rename = "Z01")]
    Supplier,
}

/// Message status: indicates whether this is a creation, update, or deactivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Meldungsstatus {
    /// Initial creation of master data.
    #[serde(rename = "A14")]
    Creation,
    /// Update to existing master data.
    #[serde(rename = "A15")]
    Update,
    /// Deactivation of master data.
    #[serde(rename = "A16")]
    Deactivation,
}

/// German control zone (`Regelzone`) codes used in `Stammdaten`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Regelzone {
    /// TransnetBW control zone (EIC 10YDE-ENBW-----N).
    #[serde(rename = "10YDE-ENBW-----N")]
    TransnetBw,
    /// TenneT DE control zone (EIC 10YDE-EON------1).
    #[serde(rename = "10YDE-EON------1")]
    TennetDe,
    /// Amprion control zone (EIC 10YDE-RWENET---I).
    #[serde(rename = "10YDE-RWENET---I")]
    Amprion,
    /// 50Hertz control zone (EIC 10YDE-VE-------2).
    #[serde(rename = "10YDE-VE-------2")]
    FiftyHertz,
    /// Stadtwerke Flensburg control zone (EIC 10YFLENSBURG---3).
    #[serde(rename = "10YFLENSBURG---3")]
    Flensburg,
    /// DB Energie (Bahnstrom) control zone (EIC 11YRBAHNSTROM--P).
    #[serde(rename = "11YRBAHNSTROM--P")]
    Bahnstrom,
}

/// Energy carrier (`Energietraeger`) codes used in `SR_Objekt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Energietraeger {
    /// Natural gas (B01).
    #[serde(rename = "B01")]
    NaturalGas,
    /// Lignite (brown coal, B02).
    #[serde(rename = "B02")]
    LigniteCoal,
    /// Hard coal (B03).
    #[serde(rename = "B03")]
    HardCoal,
    /// Oil (B04).
    #[serde(rename = "B04")]
    Oil,
    /// Uranium / nuclear (B05).
    #[serde(rename = "B05")]
    Uranium,
    /// Biomass (B06).
    #[serde(rename = "B06")]
    Biomass,
    /// Wind energy (B07).
    #[serde(rename = "B07")]
    Wind,
    /// Solar / photovoltaic (B08).
    #[serde(rename = "B08")]
    Solar,
    /// Run-of-river hydro (B09).
    #[serde(rename = "B09")]
    RunOfRiver,
    /// Pumped-storage hydro (B10).
    #[serde(rename = "B10")]
    PumpedStorage,
    /// Geothermal (B11).
    #[serde(rename = "B11")]
    Geothermal,
    /// Waste-to-energy (B12).
    #[serde(rename = "B12")]
    WasteToEnergy,
    /// Other renewable energy source (B13).
    #[serde(rename = "B13")]
    OtherRenewable,
    /// Mixed energy carrier (B14).
    #[serde(rename = "B14")]
    Mixed,
    /// Pumped-storage hydro with natural inflow (B15).
    #[serde(rename = "B15")]
    PumpedStorageWithNaturalInflow,
    /// Other non-renewable energy source (B16).
    #[serde(rename = "B16")]
    OtherNonRenewable,
    /// Other storage technology (B17).
    #[serde(rename = "B17")]
    OtherStorage,
    /// Hydrogen (B18).
    #[serde(rename = "B18")]
    Hydrogen,
    /// Offshore wind energy (B19).
    #[serde(rename = "B19")]
    Offshore,
    /// Battery storage (B20).
    #[serde(rename = "B20")]
    Battery,
    /// EEG-remunerated renewable energy (Z01).
    #[serde(rename = "Z01")]
    Eeg,
    /// KWKG-remunerated combined heat and power (Z02).
    #[serde(rename = "Z02")]
    Kwkg,
}

/// Billing model (`Bilanzierungsmodell`) for a controllable resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bilanzierungsmodell {
    /// Plan value.
    #[serde(rename = "Z01")]
    Planwert,
    /// Forecast.
    #[serde(rename = "Z02")]
    Prognose,
    /// Forecast with planning data delivery.
    #[serde(rename = "Z03")]
    PrognoseWithPlanningData,
}

/// Call type (`Abrufart_Aufforderungsfall`) for a controllable resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AbrufartAufforderungsfall {
    /// Delta instruction (Deltaanweisung).
    #[serde(rename = "Z01")]
    Delta,
    /// Setpoint (Sollwert).
    #[serde(rename = "Z02")]
    Sollwert,
}

/// Tolerance case (`Status_Duldungsfall`) for a controllable resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusDuldungsfall {
    /// Tolerance case applies (A01).
    #[serde(rename = "A01")]
    Yes,
    /// Tolerance case does not apply (A02).
    #[serde(rename = "A02")]
    No,
}

/// Compensation type (`Verguetungsart`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Verguetungsart {
    /// EEG (Renewable Energy Act).
    #[serde(rename = "Z01")]
    Eeg,
    /// KWKG (CHP Act).
    #[serde(rename = "Z02")]
    Kwkg,
    /// Other.
    #[serde(rename = "Z03")]
    Other,
}

// ── Grid operator reference ───────────────────────────────────────────────────

/// Network operator reference used in `SR_Objekt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NbRef {
    /// 13-digit market participant identifier.
    #[serde(rename = "@Code")]
    pub code: MarketParticipantId,
    /// Coding scheme.
    #[serde(rename = "@Codierung")]
    pub codierung: Codierung,
}

/// Affected grid operator reference (includes cascade position 1–6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BetroffenerNb {
    /// 13-digit market participant identifier.
    #[serde(rename = "@Code")]
    pub code: MarketParticipantId,
    /// Coding scheme.
    #[serde(rename = "@Codierung")]
    pub codierung: Codierung,
    /// Position in the cascade (1–6).
    #[serde(rename = "@Pos")]
    pub pos: u8,
}

// ── Steuerbarkeit ─────────────────────────────────────────────────────────────

/// Measure unit for `Steuerbarkeit` steps / increments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SteuerbarkeitEinheit {
    /// Megawatt.
    #[serde(rename = "MAW")]
    Megawatt,
    /// Percent.
    #[serde(rename = "P1")]
    Percent,
}

/// Step-based controllability definition (`Stufen`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stufen {
    /// Individual step values (percentage of installed capacity).
    #[serde(rename = "Einzelstufe", default)]
    pub einzelstufen: Vec<Decimal3>,
    /// Unit (always `P1` — percent).
    #[serde(rename = "@Einheit", default, skip_serializing_if = "Option::is_none")]
    pub einheit: Option<String>,
}

/// Increment-based controllability definition (`Schritte`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schritte {
    /// Unit of the increment (`MAW` or `P1`).
    #[serde(rename = "Einheit")]
    pub einheit: SteuerbarkeitEinheit,
    /// Step size.
    #[serde(rename = "Schrittweite")]
    pub schrittweite: Decimal3,
    /// Minimum value.
    #[serde(rename = "Min")]
    pub min: Decimal3,
    /// Maximum value.
    #[serde(rename = "Max")]
    pub max: Decimal3,
}

/// Controllability definition of a steuerbare Ressource.
/// Either step-based (`Stufen`) or increment-based (`Schritte`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Steuerbarkeit {
    /// Step-based controllability (optional — exclusive with `Schritte`).
    #[serde(rename = "Stufen", default, skip_serializing_if = "Option::is_none")]
    pub stufen: Option<Stufen>,
    /// Increment-based controllability (optional — exclusive with `Stufen`).
    #[serde(rename = "Schritte", default, skip_serializing_if = "Option::is_none")]
    pub schritte: Option<Schritte>,
    /// Whether the controllability values are fixed (optional attribute).
    #[serde(
        rename = "@Fixierung",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub fixierung: Option<String>,
}

// ── Technische_Parameter ──────────────────────────────────────────────────────

/// Technical parameters of a **Steuerbare Ressource**.
///
/// The XSD declares `Technische_Parameter` three times with **three different
/// anonymous complexTypes** — once under `SR_Objekt` (this one, the dispatch
/// timings), once under `Enthaltene_TR` ([`TrTechnischeParameter`], the plant
/// nameplate) and once under `CR_Objekt` (the load gradients alone). Sharing
/// one Rust type across them silently drops whichever fields the other shapes
/// carry, which is what used to happen to the entire TR nameplate.
///
/// All fields are optional; only those relevant to the resource type are
/// populated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechnischeParameter {
    /// Minimum dispatchable generation (MW).
    #[serde(
        rename = "Fahrbare_Mindesterzeugungsleistung",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub fahrbare_mindesterzeugungsleistung: Option<Decimal3>,
    /// Minimum run time (minutes).
    #[serde(
        rename = "Mindestbetriebszeit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mindestbetriebszeit: Option<u32>,
    /// Minimum downtime (minutes).
    #[serde(
        rename = "Mindeststillstandszeit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mindeststillstandszeit: Option<u32>,
    /// Cold start time, i.e. after > 48h downtime (minutes).
    #[serde(
        rename = "Anfahrzeit_kalt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub anfahrzeit_kalt: Option<u32>,
    /// Warm start time, i.e. after ≤ 48h downtime (minutes).
    #[serde(
        rename = "Anfahrzeit_warm",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub anfahrzeit_warm: Option<u32>,
    /// Ramp-up time from cold start to synchronisation (minutes).
    #[serde(
        rename = "Hochfahrzeit_kalt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hochfahrzeit_kalt: Option<u32>,
    /// Ramp-up time from warm start to synchronisation (minutes).
    #[serde(
        rename = "Hochfahrzeit_warm",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hochfahrzeit_warm: Option<u32>,
    /// Ramp-down time to grid disconnection (minutes).
    #[serde(
        rename = "Abfahrzeit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub abfahrzeit: Option<u32>,
    /// Load gradient — upward ramp rate.
    #[serde(
        rename = "Lastgradient_Erhoehung",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub lastgradient_erhoehung: Option<Lastgradient>,
    /// Load gradient — downward ramp rate.
    #[serde(
        rename = "Lastgradient_Reduzierung",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub lastgradient_reduzierung: Option<Lastgradient>,
}

/// A load gradient and the capacity it is expressed relative to.
///
/// `Basisgroesse` is what makes `@Gradient` interpretable: a gradient given in
/// `%/min` is a percentage **of that base**, so a reader that drops it either
/// has to guess the reference or cannot use the figure at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lastgradient {
    /// The ramp rate.
    #[serde(rename = "@Gradient")]
    pub gradient: Decimal3,
    /// Unit of the ramp rate (`Z01` = %/min, or `MAW`).
    #[serde(rename = "@Einheit")]
    pub einheit: String,
    /// Capacity the gradient is relative to (MW).
    #[serde(
        rename = "Basisgroesse",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub basisgroesse: Option<Decimal3>,
}

// ── TR-level Technische_Parameter ────────────────────────────────────────────

/// Whether a 70 % Wirkleistungsbegrenzung applies (`Absenkung_70`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JaNein {
    /// `A01` — yes.
    #[serde(rename = "A01")]
    Ja,
    /// `A02` — no.
    #[serde(rename = "A02")]
    Nein,
}

/// Site coordinates of a Technische Ressource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Geokoordinaten {
    /// Longitude, degrees east.
    #[serde(rename = "@LaengeOst")]
    pub laenge_ost: f64,
    /// Latitude, degrees north.
    #[serde(rename = "@BreiteNord")]
    pub breite_nord: f64,
}

/// Technical parameters of a **Technische Ressource** — the plant nameplate.
///
/// A different complexType from [`TechnischeParameter`] despite sharing the
/// element name; see that type's docs. These are the figures the Ausfallarbeit
/// calculation reads: `BilAReM` Kap. 3.2.2.1 bounds `W_A` by `P_bean`, which is
/// „die installierte Leistung der TR" less any Nichtbeanspruchbarkeit, and the
/// Pauschal-Abrechnung of Kap. 3.2.2.3 multiplies an Anlagenfaktor by exactly
/// that installed capacity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrTechnischeParameter {
    /// Nettonennleistung, production direction (MW).
    #[serde(
        rename = "Nettonennleistung_Prod",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nettonennleistung_prod: Option<Decimal3>,
    /// Nettonennleistung, consumption direction (MW).
    #[serde(
        rename = "Nettonennleistung_Verb",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nettonennleistung_verb: Option<Decimal3>,
    /// Nettoengpassleistung, production direction (MW).
    #[serde(
        rename = "Nettoengpassleistung_Prod",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nettoengpassleistung_prod: Option<Decimal3>,
    /// Nettoengpassleistung, consumption direction (MW).
    #[serde(
        rename = "Nettoengpassleistung_Verb",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nettoengpassleistung_verb: Option<Decimal3>,
    /// Bruttonennleistung (MW).
    #[serde(
        rename = "Bruttonennleistung",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bruttonennleistung: Option<Decimal3>,
    /// Cumulated inverter capacity (MW).
    #[serde(
        rename = "Wechselrichterleistung_kumuliert",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub wechselrichterleistung_kumuliert: Option<Decimal3>,
    /// Whether the 70 % Wirkleistungsbegrenzung applies.
    #[serde(
        rename = "Absenkung_70",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub absenkung_70: Option<JaNein>,
    /// Plant type.
    #[serde(
        rename = "Anlagentyp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub anlagentyp: Option<String>,
    /// Hub height of a wind turbine (m).
    #[serde(
        rename = "Nabenhoehe",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nabenhoehe: Option<Decimal3>,
    /// Site coordinates.
    #[serde(
        rename = "Geokoordinaten",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub geokoordinaten: Option<Geokoordinaten>,
    /// Round-trip efficiency of a storage unit.
    #[serde(
        rename = "Wirkungsgrad_Speicher",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub wirkungsgrad_speicher: Option<Decimal3>,
    /// Usable energy content of a storage unit (MWh).
    #[serde(
        rename = "Nutzbarer_Energieinhalt_Speichers",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub nutzbarer_energieinhalt_speichers: Option<Decimal3>,
    /// Maximum charging power (MW).
    #[serde(
        rename = "Wirkleistung_Einspeichern_max",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub wirkleistung_einspeichern_max: Option<Decimal3>,
    /// Maximum discharging power (MW).
    #[serde(
        rename = "Wirkleistung_Ausspeichern_max",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub wirkleistung_ausspeichern_max: Option<Decimal3>,
}

// ── Abrechnungsmodell ────────────────────────────────────────────────────────

/// `Abrechnungsmodell` — the Ausfallarbeit settlement method elected for a
/// Technische Ressource.
///
/// The election is the Anlagenbetreiber's, made by 30 November for the
/// following calendar year (`BilAReM` Kap. 3.2.1); the ANB carries it in the
/// Stammdaten. It is **mandatory** on every TR, because without it the
/// Ausfallarbeit of a Redispatch-Maßnahme cannot be computed at all.
///
/// | Code | XSD label | `BilAReM` Kap. 3.2.1 |
/// |------|-----------|----------------------|
/// | `Z01` | `PAUSCHAL` | Pauschal-Abrechnung — grandfathered TR only, until 31.12.2028 |
/// | `Z02` | `SPITZ` | Spitzabrechnung — measured Wetterdaten at the TR |
/// | `Z03` | `SPITZLIGHT` | vereinfachte Spitzabrechnung — Referenzmesswerte or site weather data |
///
/// The XSD label `SPITZLIGHT` is the *vereinfachte* Spitzabrechnung, not a
/// lighter variant of the Spitzabrechnung: it is the one that applies when no
/// suitable weather data is measured **at** the TR, and it is the default when
/// the Anlagenbetreiber elects nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Abrechnungsmodell {
    /// `Z01` — Pauschal-Abrechnung (`BilAReM` Kap. 3.2.2.3 / 3.2.4.3).
    #[serde(rename = "Z01")]
    Pauschal,
    /// `Z02` — Spitzabrechnung (`BilAReM` Kap. 3.2.2.1 / 3.2.4.1).
    #[serde(rename = "Z02")]
    Spitz,
    /// `Z03` — vereinfachte Spitzabrechnung (`BilAReM` Kap. 3.2.2.2 / 3.2.4.2).
    #[serde(rename = "Z03")]
    SpitzLight,
}

/// `Betrieb` — the Stilllegungs-Status of a Technische Ressource on
/// `Gueltig_ab`.
///
/// A container, not a flag: the XSD distinguishes the **vorläufige** from the
/// **endgültige** Stilllegung, and the two have different consequences. A
/// vorläufig stillgelegte Anlage still exists for Redispatch purposes; an
/// endgültig stillgelegte one does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Betrieb {
    /// Whether the vorläufige Stilllegung is reached on `Gueltig_ab`.
    #[serde(
        rename = "Stilllegung_vorlaeufig_erreicht",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub stilllegung_vorlaeufig_erreicht: Option<JaNein>,
    /// Whether the endgültige Stilllegung is reached on `Gueltig_ab`.
    #[serde(
        rename = "Stilllegung_endgueltig_erreicht",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub stilllegung_endgueltig_erreicht: Option<JaNein>,
}

// ── Marktlokation and Tranche ────────────────────────────────────────────────

/// A Tranche of a Marktlokation, with the Bilanzkreis and Lieferant it belongs
/// to.
///
/// `BilAReM` Kap. 2.1.2: „Ist die Einspeisung mehreren Tranchen … zugeordnet,
/// wird der bilanzielle Ausgleich nach den für die Aufteilung der Einspeisung
/// in Tranchen jeweils geltenden Regeln aufgeteilt." The split therefore needs
/// both values, per Tranche.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tranche {
    /// Tranche identifier.
    #[serde(rename = "@Code")]
    pub code: String,
    /// Bilanzkreis of this Tranche (16-character EIC).
    #[serde(rename = "Bilanzkreis_Tranche")]
    pub bilanzkreis: String,
    /// Lieferant of this Tranche.
    #[serde(rename = "Lieferant_Tranche")]
    pub lieferant: StammdatenParticipantRef,
    /// Share of the Marktlokation this Tranche represents.
    ///
    /// `BilAReM` Kap. 2.1.2: the bilanzielle Ausgleich is split across Tranchen
    /// „nach den für die Aufteilung der Einspeisung in Tranchen jeweils
    /// geltenden Regeln", so the share is what the split is computed from.
    #[serde(rename = "Tranchengroesse")]
    pub tranchengroesse: Tranchengroesse,
}

/// The share of a Marktlokation one Tranche represents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tranchengroesse {
    /// Unit — always `P1` (percent).
    #[serde(rename = "@Einheit")]
    pub einheit: String,
    /// The share, in percent.
    #[serde(rename = "@Groesse")]
    pub groesse: Decimal3,
}

/// A coded reference carried entirely in a `@Code` attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRef {
    /// The code.
    #[serde(rename = "@Code")]
    pub code: String,
}

/// The Marktlokation a Technische Ressource is billed through, and the
/// **betroffener Bilanzkreis** the bilanzieller Ausgleich lands in.
///
/// A TR may name up to two (`maxOccurs="2"`) — a Stromspeichereinheit that both
/// charges and discharges is billed through one MaLo per direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marktlokation {
    /// The MaLo-ID.
    #[serde(rename = "@Code")]
    pub code: String,
    /// Energy flow direction of this Marktlokation.
    #[serde(rename = "@Lieferrichtung")]
    pub lieferrichtung: String,
    /// Bilanzkreis of the Marktlokation (16-character EIC), when the MaLo is
    /// not split into Tranchen.
    #[serde(
        rename = "Bilanzkreis_Marktlokation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bilanzkreis: Option<String>,
    /// Tranchen of the Marktlokation, when the Einspeisung is split.
    #[serde(rename = "Tranche", default, skip_serializing_if = "Vec::is_empty")]
    pub tranchen: Vec<Tranche>,
    /// Voltage level of the Marktlokation.
    #[serde(rename = "Spannungsebene_Marktlokation")]
    pub spannungsebene: CodeRef,
    /// Transformation level, when the MaLo sits at one.
    #[serde(
        rename = "Umspannung_Marktlokation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub umspannung: Option<CodeRef>,
    /// Messlokationen behind this Marktlokation (at least one).
    #[serde(rename = "Messlokation")]
    pub messlokationen: Vec<CodeRef>,
    /// Lieferant of the Marktlokation, when it is not split into Tranchen.
    #[serde(
        rename = "Lieferant_Marktlokation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub lieferant: Option<StammdatenParticipantRef>,
}

// ── Enthaltene_TR (contained technical resources) ────────────────────────────

/// A technical resource (Technische Ressource) contained within an
/// `SR_Objekt`.
///
/// `BilAReM` Kap. 6.1.5: „Eine SR setzt sich aus **mindestens einer** TR
/// zusammen" and „Jede TR ist genau einer SR zugeordnet."
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnthaltenesTr {
    /// MaStR number (Marktstammdatenregister).
    ///
    /// The XSD element is `MaStR-Nr`; the hyphen is preserved on the wire.
    #[serde(rename = "MaStR-Nr", default, skip_serializing_if = "Option::is_none")]
    pub ma_str_nr: Option<String>,
    /// Human-readable name.
    #[serde(rename = "Klarname", default, skip_serializing_if = "Option::is_none")]
    pub klarname: Option<String>,
    /// Resource type code.
    #[serde(rename = "Typ")]
    pub typ: String,
    /// Plant code (`Code_Kraftwerk`).
    #[serde(
        rename = "Code_Kraftwerk",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub code_kraftwerk: Option<String>,
    /// Storage units this TR is assigned to.
    #[serde(
        rename = "Zuordnung_Speicher",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub zuordnung_speicher: Vec<StammdatenParticipantRef>,
    /// The Marktlokationen this TR is billed through (at most two — one per
    /// direction for a Stromspeichereinheit).
    #[serde(
        rename = "Marktlokation",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub marktlokationen: Vec<Marktlokation>,
    /// EEG-Anlagenschlüssel of the plants behind this TR.
    #[serde(
        rename = "EEG_Anlagenschluessel",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub eeg_anlagenschluessel: Vec<String>,
    /// Ausfallarbeit settlement method — **mandatory**, because without it the
    /// Ausfallarbeit of a Redispatch-Maßnahme cannot be computed.
    #[serde(rename = "Abrechnungsmodell")]
    pub abrechnungsmodell: Abrechnungsmodell,
    /// Betreiber der technischen Ressource (BTR).
    #[serde(
        rename = "Betreiber_TR",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub betreiber_tr: Option<StammdatenParticipantRef>,
    /// Whether the Stilllegungszeitpunkt is reached on `Gueltig_ab`.
    #[serde(rename = "Betrieb", default, skip_serializing_if = "Option::is_none")]
    pub betrieb: Option<Betrieb>,
    /// Technical parameters of this TR — the plant nameplate, a **different**
    /// complexType from the SR-level one despite the shared element name.
    #[serde(
        rename = "Technische_Parameter",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub technische_parameter: Option<TrTechnischeParameter>,
}

// ── SR_Objekt ─────────────────────────────────────────────────────────────────

/// A steuerbare Ressource (controllable resource) object.
///
/// Each `SR_Objekt` describes one resource or cluster that participates in
/// the Redispatch 2.0 process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrObjekt {
    /// Human-readable name (optional, max 35 chars, `[A-Z0-9\-+_]*`).
    #[serde(rename = "Klarname", default, skip_serializing_if = "Option::is_none")]
    pub klarname: Option<String>,
    /// Network connection point operator.
    #[serde(rename = "Anschluss_Netzbetreiber")]
    pub anschluss_netzbetreiber: NbRef,
    /// Ordering grid operator (optional — absent when same as connection NB).
    #[serde(
        rename = "Anweisender_Netzbetreiber",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub anweisender_netzbetreiber: Option<NbRef>,
    /// Affected grid operators in cascade order (up to 6).
    #[serde(
        rename = "Betroffene_Netzbetreiber",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub betroffene_netzbetreiber: Vec<BetroffenerNb>,
    /// Additional affected grid operators beyond the cascade of 6.
    #[serde(
        rename = "Weitere_betroffene_Netzbetreiber",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub weitere_betroffene_netzbetreiber: Vec<NbRef>,
    /// Einsatzverantwortlicher (EIV) — the party responsible for deploying the
    /// SR.
    ///
    /// `BilAReM` Kap. 6.1.5: „Jede SR ist genau einem EIV zugeordnet", and
    /// Kap. 6.1.6 names the default: the LF of the betroffene Marktlokation,
    /// unless another company was designated. The element is optional in the
    /// XSD because a `Z02` reduzierte Stammdaten message omits it, not because
    /// an SR may lack one.
    #[serde(
        rename = "Einsatzverantwortlicher",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub einsatzverantwortlicher: Option<StammdatenParticipantRef>,
    /// Energy carrier.
    #[serde(
        rename = "Energietraeger",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub energietraeger: Option<Energietraeger>,
    /// Compensation type.
    #[serde(
        rename = "Verguetungsart",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub verguetungsart: Option<Verguetungsart>,
    /// Tolerance case status.
    #[serde(
        rename = "Status_Duldungsfall",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub status_duldungsfall: Option<StatusDuldungsfall>,
    /// Controllability definition.
    #[serde(
        rename = "Steuerbarkeit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub steuerbarkeit: Option<Steuerbarkeit>,
    /// Call type for demand requests.
    #[serde(
        rename = "Abrufart_Aufforderungsfall",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub abrufart_aufforderungsfall: Option<AbrufartAufforderungsfall>,
    /// Billing model.
    #[serde(rename = "Bilanzierungsmodell")]
    pub bilanzierungsmodell: Bilanzierungsmodell,
    /// Individual allocation quota percentages.
    #[serde(
        rename = "Individuelle_Quote",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub individuelle_quote: Option<IndividuelleQuote>,
    /// Bearbeitungszeit beim EIV in minutes — from an Aufforderung reaching the
    /// EIV to its implementation in the plant.
    #[serde(
        rename = "Bearbeitungszeit_EIV",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bearbeitungszeit_eiv: Option<Decimal3>,
    /// Control zone (Regelzone / TSO EIC code).
    #[serde(rename = "Regelzone")]
    pub regelzone: Regelzone,
    /// Technical parameters of the resource (optional).
    #[serde(
        rename = "Technische_Parameter",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub technische_parameter: Option<TechnischeParameter>,
    /// Contained technical resources.
    ///
    /// `minOccurs="1"` in the XSD, and `BilAReM` Kap. 6.1.5 says why: „Eine SR
    /// setzt sich aus mindestens einer TR zusammen." An SR with none is a
    /// resource nothing can be dispatched against.
    #[serde(rename = "Enthaltene_TR")]
    pub enthaltene_tr: Vec<EnthaltenesTr>,
}

/// One share of the bilanzieller Ausgleich, and where it is booked.
///
/// Each Quote names the **Redispatch-Bilanzkreis** the corresponding share of
/// the Ausgleichsfahrplan is scheduled against, and the Lieferant it belongs
/// to. `BilAReM` Kap. 2.1.2 makes both load-bearing: the Ausgleich runs „durch
/// die Anmeldung korrespondierender Fahrpläne", and „jeder Netzbetreiber
/// verwendet genau einen Bilanzkreis als Redispatch-Bilanzkreis".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    /// Unit of `wert` — always `P1` (percent) in the current XSD.
    #[serde(rename = "@Einheit")]
    pub einheit: String,
    /// The share, in percent.
    #[serde(rename = "@Wert")]
    pub wert: Decimal3,
    /// Bilanzkreis the Ausgleichsfahrplan for this share is scheduled against
    /// (16-character EIC).
    #[serde(rename = "Bilanzkreis_Ausgleichsfahrplan")]
    pub bilanzkreis_ausgleichsfahrplan: String,
    /// Lieferant this share belongs to.
    #[serde(rename = "Lieferant")]
    pub lieferant: StammdatenParticipantRef,
}

/// Individual allocation quota definition — up to twenty shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndividuelleQuote {
    /// The shares. `maxOccurs="20"` in the XSD.
    #[serde(rename = "Quote")]
    pub quoten: Vec<Quote>,
}

// ── Existenzende and the anfNB Redispatch-Bilanzkreis ────────────────────────

/// End of existence of one or more resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Existenzende {
    /// References of the resources whose existence is ending.
    #[serde(rename = "Objektreferenz")]
    pub objektreferenzen: Vec<StammdatenParticipantRef>,
}

/// The Redispatch-Bilanzkreis of one anfordernder Netzbetreiber.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnfordernderNetzbetreiber {
    /// The anfNB's Redispatch-Bilanzkreis (16-character EIC).
    #[serde(rename = "Bilanzkreis_anfNB")]
    pub bilanzkreis: String,
    /// The anfNB's Marktpartner-ID.
    #[serde(rename = "Marktpartner_ID")]
    pub marktpartner_id: StammdatenParticipantRef,
}

/// Which Redispatch-Bilanzkreis each anfordernder Netzbetreiber uses for one SR.
///
/// `BilAReM` Kap. 2.3.2 lists this among the three things a Planwertmodell
/// Zuordnungsmitteilung must contain: „die Bezeichnung der SR mit ihrer SR-ID,
/// das Datum der Wirksamkeit der Zuordnung und die **Nennung des
/// Redispatch-Bilanzkreises** des ANB." Without it the LF and the EIV know an
/// SR moved into the Planwertmodell but not where the Ausgleich will be booked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilanzkreisAusgleichsfahrplanAnfNb {
    /// The SR this applies to.
    #[serde(rename = "SR_Objekt_Referenz")]
    pub sr_objekt_referenz: StammdatenParticipantRef,
    /// One entry per anfordernder Netzbetreiber (up to twenty).
    #[serde(rename = "anfordernder_Netzbetreiber")]
    pub anfordernde_netzbetreiber: Vec<AnfordernderNetzbetreiber>,
}

// ── Stammdaten ────────────────────────────────────────────────────────────────

/// `Stammdaten` — master data for steuerbare Ressourcen in Redispatch 2.0.
///
/// XSD version: 1.4b (Fehlerkorrektur 2026-02-19)  
/// Namespace: `urn:kwep_stammdaten:1:0`
///
/// Contains the static attributes of controllable resources (generation plants,
/// storage, flexible loads) that participate in the Redispatch 2.0 process.
/// Submitted by resource providers (EIV) and DSOs (VNB) to TSOs (ÜNB).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Stammdaten")]
pub struct Stammdaten {
    /// Unique document identifier (max 35 chars).
    #[serde(rename = "DocumentIdentification")]
    pub document_identification: DocumentId,
    /// Document type.
    #[serde(rename = "DocumentType")]
    pub document_type: StammdatenDocType,
    /// Document creation timestamp (UTC, second precision).
    #[serde(rename = "Erstellungszeitpunkt")]
    pub erstellungszeitpunkt: UtcDateTime,
    /// Sender identification.
    #[serde(rename = "Sender")]
    pub sender: StammdatenParticipantRef,
    /// Sender's market role.
    #[serde(rename = "Senderrolle")]
    pub senderrolle: StammdatenSenderRole,
    /// Receiver identification.
    #[serde(rename = "Empfaenger")]
    pub empfaenger: StammdatenParticipantRef,
    /// Receiver's market role.
    #[serde(rename = "Empfaengerrolle")]
    pub empfaengerrolle: StammdatenReceiverRole,
    /// Reference document identification (optional; used for updates).
    #[serde(
        rename = "RefDokumentID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ref_dokument_id: Option<Mrid>,
    /// Original sender when forwarded via data provider (optional).
    #[serde(
        rename = "OriginalSender",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_sender: Option<StammdatenParticipantRef>,
    /// Original document identifier when forwarded (optional).
    #[serde(
        rename = "OriginalDokumentID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_dokument_id: Option<Mrid>,
    /// Original creation timestamp when forwarded (optional).
    #[serde(
        rename = "OriginalErstellungszeitpunkt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_erstellungszeitpunkt: Option<UtcDateTime>,
    /// Validity start timestamp (UTC; represents German local time midnight).
    #[serde(rename = "Gueltig_ab")]
    pub gueltig_ab: UtcDateTime,
    /// Message status: creation, update, or deactivation.
    #[serde(rename = "Meldungsstatus")]
    pub meldungsstatus: Meldungsstatus,
    /// Controllable resource objects described in this document.
    #[serde(rename = "SR_Objekt", default)]
    pub sr_objekte: Vec<SrObjekt>,
    /// End of existence of one or more resources.
    #[serde(
        rename = "Existenzende",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub existenzende: Option<Existenzende>,
    /// The Redispatch-Bilanzkreis each anfordernder Netzbetreiber uses for one
    /// SR — one of the three things a Planwertmodell Zuordnungsmitteilung must
    /// carry (`BilAReM` Kap. 2.3.2).
    #[serde(
        rename = "Bilanzkreis_Ausgleichsfahrplan_anfNB",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bilanzkreis_ausgleichsfahrplan_anf_nb: Option<BilanzkreisAusgleichsfahrplanAnfNb>,
}
