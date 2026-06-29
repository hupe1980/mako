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
    #[serde(rename = "A18")]
    GridOperator,
    #[serde(rename = "A27")]
    ResourceProvider,
    #[serde(rename = "A39")]
    DataProvider,
    #[serde(rename = "Z01")]
    Supplier,
}

/// Receiver market role (`Empfaengerrolle`) in `Stammdaten`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StammdatenReceiverRole {
    #[serde(rename = "A08")]
    BalanceResponsibleParty,
    #[serde(rename = "A18")]
    GridOperator,
    #[serde(rename = "A39")]
    DataProvider,
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
    #[serde(rename = "10YDE-ENBW-----N")]
    TransnetBw,
    #[serde(rename = "10YDE-EON------1")]
    TennetDe,
    #[serde(rename = "10YDE-RWENET---I")]
    Amprion,
    #[serde(rename = "10YDE-VE-------2")]
    FiftyHertz,
    #[serde(rename = "10YFLENSBURG---3")]
    Flensburg,
    #[serde(rename = "11YRBAHNSTROM--P")]
    Bahnstrom,
}

/// Energy carrier (`Energietraeger`) codes used in `SR_Objekt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Energietraeger {
    #[serde(rename = "B01")]
    NaturalGas,
    #[serde(rename = "B02")]
    LigniteCoal,
    #[serde(rename = "B03")]
    HardCoal,
    #[serde(rename = "B04")]
    Oil,
    #[serde(rename = "B05")]
    Uranium,
    #[serde(rename = "B06")]
    Biomass,
    #[serde(rename = "B07")]
    Wind,
    #[serde(rename = "B08")]
    Solar,
    #[serde(rename = "B09")]
    RunOfRiver,
    #[serde(rename = "B10")]
    PumpedStorage,
    #[serde(rename = "B11")]
    Geothermal,
    #[serde(rename = "B12")]
    WasteToEnergy,
    #[serde(rename = "B13")]
    OtherRenewable,
    #[serde(rename = "B14")]
    Mixed,
    #[serde(rename = "B15")]
    PumpedStorageWithNaturalInflow,
    #[serde(rename = "B16")]
    OtherNonRenewable,
    #[serde(rename = "B17")]
    OtherStorage,
    #[serde(rename = "B18")]
    Hydrogen,
    #[serde(rename = "B19")]
    Offshore,
    #[serde(rename = "B20")]
    Battery,
    #[serde(rename = "Z01")]
    Eeg,
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
    #[serde(rename = "A01")]
    Yes,
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

/// Technical parameters of a controllable resource.
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
    pub lastgradient_erhoehung: Option<Decimal3>,
    /// Load gradient — downward ramp rate.
    #[serde(
        rename = "Lastgradient_Reduzierung",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub lastgradient_reduzierung: Option<Decimal3>,
}

// ── Enthaltene_TR (contained technical resources) ────────────────────────────

/// A technical resource (Technische Ressource) contained within an
/// `SR_Objekt` (cluster resource or control group).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnthaltenesTr {
    /// MaStR number (Marktstammdatenregister).
    #[serde(rename = "MaStR_Nr", default, skip_serializing_if = "Option::is_none")]
    pub ma_str_nr: Option<String>,
    /// Human-readable name.
    #[serde(rename = "Klarname", default, skip_serializing_if = "Option::is_none")]
    pub klarname: Option<String>,
    /// Resource type code.
    #[serde(rename = "Typ", default, skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
    /// Plant code (`Code_Kraftwerk`).
    #[serde(
        rename = "Code_Kraftwerk",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub code_kraftwerk: Option<String>,
    /// Market location (Marktlokation).
    #[serde(
        rename = "Marktlokation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub marktlokation: Option<String>,
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
    /// Contained technical resources (for cluster / control group resources).
    #[serde(
        rename = "Enthaltene_TR",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub enthaltene_tr: Vec<EnthaltenesTr>,
}

/// Individual allocation quota definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndividuelleQuote {
    /// List of percentage quota values.
    #[serde(rename = "Quote", default)]
    pub quoten: Vec<Decimal3>,
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
}
