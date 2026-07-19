use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ── Canonical types re-exported from `metering` ───────────────────────────────
//
// `metering` is the single source of truth for `QualityFlag` and `Sparte`.
// Re-exporting here eliminates the duplicate definitions that previously required
// an 8-arm identity match (`map_quality_flag`) in every service that used both crates.
pub use metering::{QualityFlag, Sparte};

/// MSCONS PIDs that `edmd` consumes from `marktd` webhook fan-out.
///
/// ## Messwesen PIDs (MSCONS AHB, BDEW BK6-24-174 / BK7-24-01-009 / BK7-24-01-008)
///
/// | PID   | Direction        | Content |
/// |-------|------------------|---------|
/// | 13005 | NB → LF (Strom)  | Lastgang Messwerte Strom |
/// | 13006 | NB → LF (Strom)  | Zählerstand / Ersatzwert Strom |
/// | 13007 | NB → LF (Gas)    | Gasbeschaffenheitsdaten (Brennwert + Zustandszahl) |
/// | 13013 | NB → LF (Gas)    | Allokationsliste Gas MMMA (GaBi Gas 2.1) |
/// | 13015 | NB → LF (Strom)  | Lastgang Summenzeitreihe (SLP-Abrechnung) |
/// | 13016 | NB → LF (Strom)  | Ausfallarbeit Strom |
/// | 13017 | NB → LF (Strom)  | Zählerstand Strom (Ablese-Übermittlung) |
/// | 13018 | NB → LF (Strom)  | Messwerte Strom — korrigierte Werte |
/// | 13019 | NB → LF (Strom)  | Netzverluste Strom |
/// | 13025 | NB → LF (Gas)    | Lastgang Gas (Zustandsmengen / Energiemengen) |
/// | 13027 | NB → LF (Gas)    | Zählerstand Gas |
///
/// ## Note on PIDs 13002–13028
///
/// These are **Messwesen-PIDs** (meter data exchange), distinct from PID 13003
/// (MABIS Bilanzkreisabrechnung). They must not be registered under any MABIS
/// workflow in `mako-mabis`. They belong exclusively to `edmd` as meter-data receipts.
///
/// **Exception**: PID 13013 (Gas MMMA Allokationsliste) is also routed in
/// `mako-gabi-gas` `gabi-gas-mmma` for workflow state tracking, but the raw
/// meter-data receipts and interval values are stored here in `edmd`.
///
/// Source: MSCONS AHB 3.1g; BDEW BK6-24-174 Anlage 1; BK7-24-01-008.
pub const MSCONS_PIDS: &[u32] = &[
    13005, 13006, 13007, 13013, 13015, 13016, 13017, 13018, 13019, 13025, 13027,
];

/// MSCONS PIDs for Redispatch 2.0 time-series data delivery.
///
/// These PIDs carry Ausfallarbeit, meteorological data, and Redispatch 2.0
/// time-series. Handled by `mako-redispatch` for workflow routing; raw
/// intervals are also stored in `edmd` for OLAP and audit.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 13020 | Ausfallarbeitsüberführungszeitreihe (NB → ÜNB) |
/// | 13021 | Redispatch meteorologische Daten |
/// | 13022 | Redispatch Einzelzeitreihe Ausfallarbeit |
/// | 13023 | Redispatch Ausfallarbeitssummen |
/// | 13026 | Redispatch Summenzeitreihe (ÜNB/VNB) |
///
/// Source: MSCONS AHB 3.1g §5; BNetzA BK6-20-059; `mako-redispatch`.
pub const REDISPATCH_MSCONS_PIDS: &[u32] = &[13_020, 13_021, 13_022, 13_023, 13_026];

/// All MSCONS PIDs that `edmd` accepts (Messwesen + Redispatch 2.0).
pub const ALL_MSCONS_PIDS: &[u32] = &[
    13005, 13006, 13007, 13013, 13015, 13016, 13017, 13018, 13019, 13025, 13027, 13_020, 13_021,
    13_022, 13_023, 13_026,
];

/// Human-readable description of each MSCONS PID.
///
/// Used in MCP tools and operator dashboards to explain what data a receipt contains.
pub const fn mscons_pid_description(pid: u32) -> &'static str {
    match pid {
        13005 => "Lastgang Messwerte Strom (NB → LF)",
        13006 => "Zählerstand / Ersatzwert Strom (NB → LF)",
        13007 => "Gasbeschaffenheitsdaten — Brennwert + Zustandszahl (NB → LF)",
        13013 => {
            "Allokationsliste Gas MMMA — Mehr-/Mindermengen Gas (NB → LF, GaBi Gas BK7-24-01-008)"
        }
        13015 => "Lastgang Summenzeitreihe SLP Strom (NB → LF)",
        13016 => "Ausfallarbeit Strom (NB → LF)",
        13017 => "Zählerstand Strom — Ablese-Übermittlung (NB → LF)",
        13018 => "Messwerte Strom — korrigierte Werte (NB → LF)",
        13019 => "Netzverluste Strom (NB → LF)",
        13020 => "Ausfallarbeitsüberführungszeitreihe (Redispatch 2.0, NB → ÜNB)",
        13021 => "Redispatch meteorologische Daten (Redispatch 2.0)",
        13022 => "Redispatch Einzelzeitreihe Ausfallarbeit (Redispatch 2.0)",
        13023 => "Redispatch Ausfallarbeitssummen (Redispatch 2.0)",
        13025 => "Lastgang Gas — Zustandsmengen / Energiemengen (NB → LF)",
        13026 => "Redispatch Summenzeitreihe (Redispatch 2.0, ÜNB/VNB)",
        13027 => "Zählerstand Gas (NB → LF)",
        _ => "Unbekannter MSCONS PID",
    }
}

/// MSCONS PIDs that carry Gas quality data (Brennwert + Zustandszahl).
///
/// PID 13007 = Gasbeschaffenheitsdaten (NB → LF): contains Abrechnungsbrennwert
/// (`QTY+Z08`, kWh/m³) and Zustandszahl (`QTY+Z10`, dimensionless).
///
/// Source: MSCONS AHB Gas 1.x; Allgemeine Festlegungen V6.1d §6.
pub const GAS_QUALITY_PIDS: &[u32] = &[13007];

/// MSCONS PIDs that carry Gas Allokation (Mehr-/Mindermengen) data.
///
/// PID 13013 = Marktlokationsscharfe Allokationsliste Gas (MMMA, NB → LF).
/// Used by `mako-gabi-gas` `gabi-gas-mmma` for balance group accounting.
///
/// Source: BK7-24-01-008 GaBi Gas 2.1; MSCONS AHB Gas 1.x.
pub const GAS_MMMA_PIDS: &[u32] = &[13013];

/// Metering / balancing classification of a Marktlokation.
///
/// Determines the applicable Mindestvorlauffrist and billing-period aggregation
/// rules.
///
/// | Variant | Description | Vorlauffrist |
/// |---------|-------------|--------------|
/// | `Slp` | Standard load profile — synthetic, grid-area-based | Next Arbeitstag (15:00 cutoff) |
/// | `Rlm` | Registrierte Lastgangmessung — interval meter | 2 Werktage minimum |
/// | `Imsys` | Intelligentes Messsystem — smart meter | Treated as SLP for Vorlauffrist |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Messtyp {
    /// Standardlastprofil metering.
    Slp,
    /// Registrierende Lastgangmessung (interval metering, typically 15-min).
    Rlm,
    /// Intelligentes Messsystem (smart meter).
    Imsys,
}

impl std::fmt::Display for Messtyp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slp => write!(f, "SLP"),
            Self::Rlm => write!(f, "RLM"),
            Self::Imsys => write!(f, "IMSYS"),
        }
    }
}

impl std::str::FromStr for Messtyp {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "SLP" => Ok(Self::Slp),
            "RLM" => Ok(Self::Rlm),
            "IMSYS" => Ok(Self::Imsys),
            other => Err(format!("unknown Messtyp: {other:?}")),
        }
    }
}

/// A delivery receipt: confirms that MSCONS meter data was received for a MaLo.
///
/// Stored by `edmd` when a `de.mako.process.completed` event arrives for an
/// MSCONS PID. The actual kWh values are stored separately as [`MeterRead`]
/// records once the domain crates emit typed meter reads in the payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterDataReceipt {
    /// Process ID in `makod` (UUID v4).
    pub process_id: Uuid,
    /// MSCONS Prüfidentifikator.
    pub pid: u32,
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// GLN of the sending NB/MSB.
    pub sender_mp_id: String,
    /// EDIFACT message reference.
    pub message_ref: Option<String>,
    /// UTC timestamp of the `de.mako.process.completed` event.
    pub received_at: OffsetDateTime,
    /// Data-isolation key — operator's BDEW/DVGW Codenummer or GLN.
    ///
    /// Mandatory; every receipt is scoped to exactly one tenant.
    /// Matches `meter_reads.tenant` and all other `edmd` table tenant columns.
    pub tenant: String,
}

/// How a `MeterRead` entered the system.
///
/// Stored in the `source` column of `meter_reads` for provenance tracking.
/// Every interval must be traceable to its origin for §22 MessZV compliance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IngestionSource {
    /// Received via EDIFACT MSCONS → makod → marktd → edmd webhook pipeline.
    #[default]
    Mscons,
    /// iMSys / SMGW direct push via `POST /api/v1/meter-reads/rlm/{malo_id}`.
    DirectPush,
    /// Gas direct push via `POST /api/v1/meter-reads/gas/{malo_id}`.
    DirectGas,
    /// Bulk import via ERP REST API.
    ApiImport,
    /// Automatic substitute value generated by `edmd` per §17 MessZV.
    AutoSubstitute,
    /// Retroactive correction applied by `POST /api/v1/corrections/{malo_id}`.
    Correction,
    /// Manual entry by an operator.
    Manual,
    /// Estimated value entered by an operator.
    Estimated,
    /// IoT push via `POST /api/v1/meter-reads/iot/{malo_id}` — LoRaWAN network
    /// server, M-Bus/wM-Bus concentrator, or a REST heat meter.
    ///
    /// Distinct from `DirectPush`, which is the iMSys/SMGW path: an IoT reading
    /// arrives outside the MsbG regime (heat and water submetering is governed by
    /// **HeizkostenV**) and carries no Smart-Meter-Gateway provenance.
    IotPush,
}

impl IngestionSource {
    /// Returns the DB string value for this source.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mscons => "MSCONS",
            Self::DirectPush => "DIRECT_PUSH",
            Self::DirectGas => "DIRECT_GAS",
            Self::ApiImport => "API_IMPORT",
            Self::AutoSubstitute => "AUTO_SUBSTITUTE",
            Self::Correction => "CORRECTION",
            Self::Manual => "MANUAL",
            Self::Estimated => "ESTIMATED",
            Self::IotPush => "IOT_PUSH",
        }
    }

    /// Parse from a DB string value.
    #[must_use]
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "DIRECT_PUSH" => Self::DirectPush,
            "DIRECT_GAS" => Self::DirectGas,
            "API_IMPORT" => Self::ApiImport,
            "AUTO_SUBSTITUTE" => Self::AutoSubstitute,
            "CORRECTION" => Self::Correction,
            "MANUAL" => Self::Manual,
            "ESTIMATED" => Self::Estimated,
            "IOT_PUSH" => Self::IotPush,
            // Lossy by design: the column is CHECK-constrained, so an unknown
            // value means enum and schema have diverged. `schema_code_guard`
            // pins the two together.
            _ => Self::Mscons,
        }
    }
}

/// Default allocation version for `serde` deserialization — see `MeterRead.allocation_version`.
fn default_allocation_version() -> String {
    "INITIAL".to_owned()
}

/// A single metered interval read sourced from an MSCONS message.
///
/// Populated when domain crates emit typed read payloads in `ProcessCompleted`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterRead {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// 33-character Messlokations-ID, if available.
    pub melo_id: Option<String>,
    /// Interval start (UTC).
    pub dtm_from: OffsetDateTime,
    /// Interval end (UTC).
    pub dtm_to: OffsetDateTime,
    /// Energy quantity in kWh.
    pub quantity_kwh: Decimal,
    /// Quality of the reading.
    pub quality: QualityFlag,
    /// Source PID (e.g. 13005).
    pub pid: u32,
    /// Energy commodity.
    pub sparte: Sparte,
    /// OBIS-Kennzahl (e.g. `"1-1:1.29.0"` for active energy, `"7-20:3.0.0"` for Gas volume).
    ///
    /// `None` when the MSCONS source did not include a PIA segment.
    pub obis_code: Option<String>,
    /// Tenant data-isolation key. Matches `meter_reads.tenant`.
    pub tenant: String,

    // ── Provenance tracking (§22 MessZV) ───────────────────────────────────────
    /// Origin of this interval — which ingestion path was used.
    ///
    /// Stored in `meter_reads.source`. Default: `Mscons`.
    #[serde(default)]
    pub source: IngestionSource,

    /// Idempotency key from the direct-push caller.
    ///
    /// Present for `DirectPush` and `DirectGas` sources. Used by `edmd` to
    /// deduplicate re-submitted batches. `None` for MSCONS-ingested reads.
    #[serde(default)]
    pub push_session: Option<String>,

    /// Automated quality warnings produced at ingest time (Hampel filter, gap detection).
    ///
    /// Schema: `{ "gaps_detected": N, "zero_run_length": N, "outlier_factor": 0.0 }`.
    /// `None` = no warnings. Triggers `de.edmd.reading.quality.warning` CloudEvent.
    #[serde(default)]
    pub quality_warnings: Option<serde_json::Value>,

    // ── F-12: Extended provenance fields (migrations 0006–0007) ────────────────
    /// MP-ID of the MSB or system that delivered this reading.
    ///
    /// Populated from `meter_data_receipts.sender_mp_id` (MSCONS path) or from the
    /// direct-push API header. Required for §22 MessZV per-interval MSB attribution
    /// after an MSB switch (WiM PID 55039).
    #[serde(default)]
    pub sender_mp_id: Option<String>,

    /// MSCONS data-delivery version per BK6-22-024 §6.4 (MaBiS AllocationVersion).
    ///
    /// `"INITIAL"` = vorläufig (day-3); `"FINAL"` = endgültig (day-8);
    /// `"CORRECTION"` = Nachbearbeitungswert.
    /// Used by `mabis-syncd` to distinguish preliminary from final Summenzeitreihen.
    #[serde(default = "default_allocation_version")]
    pub allocation_version: String,

    /// Transaction time: when this row was first inserted (database clock).
    ///
    /// Combined with `meter_read_corrections.corrected_at` this gives a full
    /// bitemporal model: "what did we know at time T?" (`valid_from_tx` ≤ T).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from_tx: Option<OffsetDateTime>,
}

/// Query parameters for time-series reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesQuery {
    pub malo_id: String,
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
    pub sparte: Option<Sparte>,
    /// Tenant data-isolation key.  **Required for all production queries** —
    /// omitting this field causes `pg/timeseries.rs::query()` to reject the call.
    /// Previously `tenant_id: Option<Uuid>` allowed NULL which leaked cross-tenant data.
    pub tenant: String,
}

/// Mehr-/Mindermengen imbalance report for one MaLo and one billing period.
///
/// Computed from [`MeterRead`] records by comparing LF-expected quantities
/// against NB-reported quantities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImbalanceReport {
    pub malo_id: String,
    /// Start of billing period (inclusive).
    pub period_from: Date,
    /// End of billing period (inclusive).
    pub period_to: Date,
    /// Total LF quantity (kWh) in period.
    pub lf_quantity_kwh: Decimal,
    /// Total NB reported quantity (kWh) in period.
    pub nb_quantity_kwh: Decimal,
    /// Delta = lf − nb.
    pub delta_kwh: Decimal,
    /// Delta as percentage of nb quantity. Zero when nb_quantity is zero.
    pub delta_pct: Decimal,
    /// Worst quality flag across all reads in the period.
    pub quality: QualityFlag,
}

/// Aggregated billing period summary for one MaLo.
///
/// Consumed by `invoicd` for INVOIC plausibility checks and by `netzbilanzd`
/// for NNE invoice generation.  Covers both SLP and RLM metering.
///
/// ## M15 requirement
///
/// This struct provides the inputs for all NNE billing positions:
/// - SLP: `arbeitsmenge_kwh` (total energy quantity)
/// - RLM Strom: `spitzenleistung_kw` (peak demand — Leistungspreisanteil = `Leistungspreis × spitzenleistung_kw`)
/// - Gas: `brennwert_kwh_per_m3` × `zustandszahl` → energy content from volume (m³ → kWh)
///
/// Lastgang (15-min intervals) is **NOT** inlined here — fetch separately via
/// `GET /api/v1/timeseries/{malo_id}` to avoid transferring 35 k rows per MaLo
/// in a billing-period summary response.
///
/// Source: GPKE BK6-22-024; GeLi Gas 3.0 (BK7-24-01-009); Allgemeine Festlegungen §6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterBillingPeriod {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Start of billing period (German local date, inclusive).
    pub period_from: Date,
    /// End of billing period (German local date, inclusive).
    pub period_to: Date,
    /// Metering classification: SLP / RLM / iMSys.
    pub messtyp: Messtyp,
    /// Energy commodity.
    pub sparte: Sparte,
    /// Total energy quantity in kWh (HT + NT combined for dual-tariff meters).
    pub arbeitsmenge_kwh: Decimal,
    /// High-tariff (Hochtarif, HT) quantity — `None` for single-tariff SLP.
    pub arbeitsmenge_ht_kwh: Option<Decimal>,
    /// Low-tariff (Niedertarif, NT) quantity — `None` for single-tariff SLP.
    pub arbeitsmenge_nt_kwh: Option<Decimal>,
    /// Peak demand in kW (Spitzenleistung).
    ///
    /// **RLM Strom only.** The 15-min interval with the highest average kW
    /// reading in the billing period.  Used to compute the Leistungspreisanteil:
    /// `Leistungspreis_EUR_per_kW × spitzenleistung_kw`.
    ///
    /// `None` for SLP, iMSys, and Gas MaLos.
    pub spitzenleistung_kw: Option<Decimal>,
    /// Abrechnungsbrennwert in kWh/m³ (Gas only).
    ///
    /// Supplied by the gas grid operator in PID 13007 or 17103.
    /// Used to convert volume (m³) to energy (kWh):
    /// `kWh = m³ × brennwert_kwh_per_m3 × zustandszahl`.
    ///
    /// `None` for Strom MaLos.
    pub brennwert_kwh_per_m3: Option<Decimal>,
    /// Zustandszahl (Gas only) — dimensionless compressibility factor.
    ///
    /// Accounts for temperature and pressure corrections.  **Not** a tariff
    /// zone — it is a physical gas Beschaffenheit factor.  Typically 0.95–1.05.
    ///
    /// `None` for Strom MaLos.
    pub zustandszahl: Option<Decimal>,
    /// Meter start reading (Zählerstand Anfang) — optional.
    pub zaehlerstand_anfang: Option<Decimal>,
    /// Meter end reading (Zählerstand Ende) — optional.
    pub zaehlerstand_ende: Option<Decimal>,
    /// Worst quality flag across all reads contributing to this summary.
    pub quality: QualityFlag,
    /// **SLP only** — standardised load profile designation.
    ///
    /// Set by the NB from the UTILMD `LIN+1` / `IMD` segment during supply-start
    /// registration.  Standard values:
    /// - `H0` — household (Haushalt)
    /// - `G0` – `G6` — commercial (Gewerbe, 0 = generic)
    /// - `L0` / `L1` / `L2` — agricultural (Landwirtschaft)
    /// - `P0` — pumping station / agriculture
    ///
    /// `None` for RLM and iMSys MaLos (metered individually).
    pub lastprofil: Option<String>,
    /// BO4E `ProfilTyp` for this MaLo.
    ///
    /// Populated from the UTILMD `TS+Z09`/`TS+Z10` qualifier or from the
    /// `bilanzierungsmethode` field in `marktd`.  Valid values per BO4E schema:
    /// - `"STANDARDLASTPROFIL"` — synthetic SLP  
    /// - `"ANALYTISCHES_VERFAHREN"` — analytically profiled (used for some Gas SLPs)
    ///
    /// `None` when unspecified (backwards-compatible — treat as SLP for existing records).
    pub profil_typ: Option<String>,
}

/// Query parameters for a billing-period summary request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingPeriodQuery {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Start of requested billing period (inclusive).
    pub period_from: Date,
    /// End of requested billing period (inclusive).
    pub period_to: Date,
    /// Tenant scope — mandatory; mirrors `TimeSeriesQuery`.
    pub tenant: String,
}

// ── Correction domain types ───────────────────────────────────────────────────

/// Source category for a meter read correction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CorrectionSource {
    /// Correction driven by a new MSCONS message from the NB/MSB.
    MsconsUpdate,
    /// Manual correction entered by an operator.
    Operator,
    /// Automatic correction by a quality/substitution algorithm.
    AutoSubstitute,
    /// Correction from an iMSys direct push (SMGW re-read).
    ImsysDirectPush,
    /// Other / unclassified source.
    Other,
}

/// A retroactive correction to a previously stored meter interval.
///
/// Stored in `meter_read_corrections` without modifying the original row —
/// enabling full §22 MessZV audit trail reconstruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionRecord {
    /// MaLo for the corrected interval.
    pub malo_id: String,
    /// Interval start (UTC).
    pub dtm_from: OffsetDateTime,
    /// Interval end (UTC).
    pub dtm_to: OffsetDateTime,
    /// Energy value BEFORE the correction (kWh).
    pub original_kwh: Decimal,
    /// Quality flag BEFORE the correction.
    pub original_quality: QualityFlag,
    /// Corrected energy value (kWh).
    pub corrected_kwh: Decimal,
    /// Quality flag for the corrected value.
    pub corrected_quality: QualityFlag,
    /// Mandatory audit trail: why was this corrected?
    pub reason: String,
    /// What triggered this correction (MSCONS, operator, algorithm).
    pub source: CorrectionSource,
    /// Operator name or system ID.
    pub corrected_by: Option<String>,
    /// MSCONS process ID that triggered this correction (if applicable).
    pub process_id: Option<Uuid>,
    /// MSCONS PID (if applicable).
    pub pid: Option<u32>,
    /// Tenant data-isolation key.
    pub tenant: String,
}

/// Gas quality data received via MSCONS PID 13007 (Gasbeschaffenheitsdaten).
///
/// Contains the Abrechnungsbrennwert and Zustandszahl required to convert gas
/// volume (m³) to energy (kWh_Hs) per §25 Nr. 4 MessEV and DVGW G 685.
///
/// ## Formula
///
/// ```text
/// kWh_Hs = m³ × brennwert_kwh_per_m3 × zustandszahl
/// ```
///
/// Source: MSCONS AHB Gas 1.x; Allgemeine Festlegungen V6.1d §6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasQualityData {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Billing period start (inclusive).
    pub period_from: time::Date,
    /// Billing period end (inclusive).
    pub period_to: time::Date,
    /// Abrechnungsbrennwert in kWh/m³ (MSCONS QTY+Z08).
    ///
    /// Typically 9.5–11.5 kWh/m³ for natural gas in Germany.
    pub brennwert_kwh_per_m3: Decimal,
    /// Zustandszahl (dimensionless, MSCONS QTY+Z10).
    ///
    /// Compressibility and temperature correction factor. Typically 0.95–1.05.
    pub zustandszahl: Decimal,
    /// Source PID (always 13007 for Gasbeschaffenheitsdaten).
    pub pid: u32,
    /// Tenant data-isolation key.
    pub tenant: String,
}

impl GasQualityData {
    /// Convert gas volume (m³) to energy (kWh_Hs).
    ///
    /// Applies Brennwert and Zustandszahl per §25 Nr. 4 MessEV.
    #[must_use]
    pub fn to_kwh(&self, volume_m3: Decimal) -> Decimal {
        volume_m3 * self.brennwert_kwh_per_m3 * self.zustandszahl
    }
}

/// A request to correct one or more meter read intervals.
///
/// Used by `POST /api/v1/corrections/{malo_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionRequest {
    /// All corrections to apply atomically.
    pub corrections: Vec<CorrectionRecord>,
}

/// Response from `POST /api/v1/corrections/{malo_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionResponse {
    /// Number of intervals corrected.
    pub corrected_count: usize,
    /// UUIDs of the created correction records.
    pub correction_ids: Vec<Uuid>,
}

// ── Bilanzierungsgebiet / Bilanzkreis topology ────────────────────────────────
//
// These types model the balance-group topology from BK6-22-024 (MaBiS) and
// allow `marktd` to store which MaLos belong to which Bilanzierungsgebiet and
// Bilanzkreis. `edmd` does not own this data — it lives in `marktd` — but the
// types are defined here so both crates share the same domain vocabulary.

/// A Bilanzierungsgebiet (settlement zone) within the German electricity grid.
///
/// Each ÜNB / NB operates one or more Bilanzierungsgebiete. All MaLos within
/// a Bilanzierungsgebiet belong to the same settlement pool for MaBiS.
///
/// ## Source
///
/// BK6-22-024 (MaBiS) — Bilanzierungsgebiet definitions; BDEW code list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BilanzierungsgebietId(pub String);

impl std::fmt::Display for BilanzierungsgebietId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Bilanzkreis (balance group) within a Bilanzierungsgebiet.
///
/// A BKV (Bilanzkreisverantwortlicher) holds one or more Bilanzkreise.
/// Each MaLo is assigned to exactly one Bilanzkreis within its Bilanzierungsgebiet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BilanzkreisId(pub String);

impl std::fmt::Display for BilanzkreisId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The balance-group assignment of one Marktlokation.
///
/// Stored in `marktd` as part of the MaLo record. Queried by `mabis-syncd`
/// (when built) to aggregate per-MaLo Lastgänge into Summenzeitreihen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BilanzzuordnungRecord {
    /// The Marktlokation being assigned.
    pub malo_id: String,
    /// Settlement zone the MaLo belongs to.
    pub bilanzierungsgebiet_id: BilanzierungsgebietId,
    /// Balance group the MaLo belongs to (None = not yet assigned).
    pub bilanzkreis_id: Option<BilanzkreisId>,
    /// MP-ID of the BKV responsible for the Bilanzkreis.
    pub bkv_mp_id: Option<String>,
    /// Effective from (inclusive, UTC date).
    pub valid_from: Date,
    /// Effective until (exclusive). `None` = open-ended (currently active).
    pub valid_to: Option<Date>,
    /// Data-isolation key — operator's BDEW/DVGW Codenummer or GLN.
    pub tenant: String,
}
