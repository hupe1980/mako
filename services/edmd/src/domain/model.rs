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

/// Parse a Sparte off the wire, or `None` when it names no known commodity.
///
/// [`metering::Sparte`] owns the vocabulary and its `FromStr` is the parser;
/// this adds exactly one thing — the umlaut spelling `WÄRME`, which German
/// callers type and which the canonical `WAERME` code does not cover.
///
/// The point of having one of these is that the doors agree. Five of them had
/// their own `match` and three of those ended in a catch-all arm that made an
/// unknown commodity silently `STROM` — so a mislabelled gas batch was stored,
/// scored and billed as electricity, in the electricity unit, with no error
/// anywhere. Returning `None` puts the refusal where the caller can see it.
#[must_use]
pub fn parse_sparte(raw: &str) -> Option<Sparte> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("WÄRME") {
        return Some(Sparte::Waerme);
    }
    trimmed.parse().ok()
}

/// MSCONS PIDs that `edmd` consumes from `marktd` webhook fan-out.
///
/// ## Messwesen PIDs
///
/// | PID   | Direction   | Anwendungsfall |
/// |-------|-------------|----------------|
/// | 13005 | BIKO → BKV / NB | EEG-Überführungszeitreihe |
/// | 13006 | NB → LF; MSB → NB/LF/ÜNB | **Messwert Storno** — see below |
/// | 13007 | NB → LF/NB (Gas) | Gasbeschaffenheit (Brennwert + Zustandszahl) |
/// | 13013 | NB → LF (Gas) | Marktlokationsscharfe Allokationsliste (MMMA) |
/// | 13015 | NB → LF | Arbeit + Leistungsmaximum im Kalenderjahr vor Lieferbeginn |
/// | 13016 | NB → LF | Energiemenge und Leistungsmaximum (Strom) |
/// | 13017 | NB → LF/RB | Zählerstand (Strom) |
/// | 13018 | NB → NB/ÜNB | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation |
/// | 13019 | NB → LF/RB | Energiemenge (Strom) |
/// | 13025 | NB → LF/RB | Lastgang Marktlokation, Tranche |
/// | 13027 | MSB → NB/LF/ESA | Werte nach Typ 2 (non-authoritative) |
///
/// **This table and [`mscons_pid_description`] render one source** — the BDEW
/// *Anwendungsübersicht der Prüfidentifikatoren* 4.0 — and
/// `pid_table_matches_descriptions` pins them together. They used to disagree on
/// five of eleven rows: 13005 was labelled "Lastgang Messwerte Strom", 13016
/// "Ausfallarbeit Strom", 13018 "korrigierte Werte", 13019 "Netzverluste Strom"
/// and 13025 a *Gas* Lastgang. Each names a different Anwendungsfall than the
/// PID carries and sends a reader to the wrong AHB section.
///
/// ## 13006 is a cancellation, not a reading
///
/// "Messwert Storno" withdraws values delivered earlier (GPKE Teil 2
/// Stornierung Lieferschein; WiM Strom Teil 2 Stornierung Werte vom MSB). Its
/// payload references what is being cancelled — it carries no new measurements.
/// The ingest handler records the receipt and refuses to store a `reads` array
/// under it, rather than booking withdrawn values as fresh readings. See
/// [`STORNO_PIDS`].
///
/// This is the subscription/accept filter, not the set that lands in
/// `meter_reads`: 13027 is included because `edmd` must **receive** ESA Typ-2
/// values, but they are routed to a separate, non-billing store — see
/// [`ESA_TYP2_PIDS`], [`Typ2Read`], and the handler fork.
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
/// Source: BDEW *Anwendungsübersicht der Prüfidentifikatoren* 4.0; MSCONS AHB.
pub const MSCONS_PIDS: &[u32] = &[
    13005, 13006, 13007, 13013, 13015, 13016, 13017, 13018, 13019, 13025, 13027,
];

/// MSCONS PIDs carrying **"Werte nach Typ 2"** (MSB → NB / LF / ESA).
///
/// These values are **non-authoritative** (Codeliste der Konfigurationen 1.4,
/// Kap. 4.6; WiM Strom Teil 2 §4): they have *no bearing* on Netznutzungs-,
/// Bilanzkreis- or Mehr-/Mindermengenabrechnung, and only Kapitel-2 (Typ-1)
/// values are relevant on divergence. `edmd` receives them (they are in
/// [`MSCONS_PIDS`]) but the ingest handler forks on this set and stores them in
/// a **separate** table ([`Typ2Read`] → `esa_typ2_reads`) that no billing query
/// can reach — the separation is a schema decision, not a runtime filter.
pub const ESA_TYP2_PIDS: &[u32] = &[13027];

/// MSCONS PIDs that **withdraw** previously delivered values rather than
/// carrying new ones.
///
/// PID 13006 "Messwert Storno" cancels an earlier delivery — GPKE Teil 2
/// (Stornierung Lieferschein) and WiM Strom Teil 2 (Stornierung Werte vom MSB).
/// Its payload identifies what is being cancelled; treating it as an ordinary
/// value delivery books the withdrawn quantities as if they had been measured,
/// which is the opposite of what the message says.
pub const STORNO_PIDS: &[u32] = &[13006];

/// MSCONS PIDs carrying Ausfallarbeit-related time series.
///
/// **This is a storage set, not a routing set.** `edmd` keeps the raw intervals
/// for OLAP and audit whoever routes the message, and the five PIDs below reach
/// three different process families:
///
/// | PID   | Inhalt | Routed by |
/// |-------|--------|-----------|
/// | 13020 | Ausfallarbeitsüberführungszeitreihe (AAÜZ) | `mako-mabis` `mabis-billing` |
/// | 13021 | meteorologische Daten (Ex-post) | `mako-redispatch` `redispatch-aktivierung` |
/// | 13022 | Einzelzeitreihe Ausfallarbeit (TR-scharf) | `mako-redispatch` `redispatch-aktivierung` |
/// | 13023 | Lieferantenausfallarbeitssummenzeitreihe (LF-AASZR) | `mako-mabis` `mabis-billing` |
/// | 13026 | EEG-Überführungszeitreihe aufgrund Ausfallarbeit | — (family not implemented) |
///
/// The name is historical: all five were once filed under Redispatch, and the
/// BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0* puts only 13021 and 13022
/// under „Kommunikationsprozesse Redispatch". What they still share is the
/// subject — the Ausfallarbeit of a Redispatch-Maßnahme — which is why `edmd`
/// keeps them together.
///
/// The invariant that matters is `mako_redispatch::aktivierung::MSCONS_PIDS` ⊆
/// this set, so a PID the workflow accepts always has its intervals stored.
/// `edmd` does not depend on `mako-redispatch`, so `redispatch_pids_are_accepted`
/// can only check the half that is local — that this set ⊆ `ALL_MSCONS_PIDS`.
///
/// Source: BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0* (01.04.2026);
/// MSCONS AHB 3.1g §5.
pub const REDISPATCH_MSCONS_PIDS: &[u32] = &[13_020, 13_021, 13_022, 13_023, 13_026];

/// All MSCONS PIDs that `edmd` accepts (Messwesen + Redispatch 2.0).
pub const ALL_MSCONS_PIDS: &[u32] = &[
    // Anything not listed falls through to the ignore branch, so a missing PID
    // means silently discarded readings rather than a visible error.
    13_002, 13_003, 13_005, 13_006, 13_007, 13_008, 13_009, 13_010, 13_011, 13_012, 13_013, 13_014,
    13_015, 13_016, 13_017, 13_018, 13_019, 13_020, 13_021, 13_022, 13_023, 13_025, 13_026, 13_027,
    13_028,
];

/// Human-readable description of each MSCONS PID.
///
/// Used in MCP tools and operator dashboards to explain what data a receipt contains.
pub const fn mscons_pid_description(pid: u32) -> &'static str {
    // Names are the AHB's own "Tabellenspalte" headings. An operator matching a
    // receipt against the AHB needs the same words the AHB uses.
    match pid {
        13002 => "Zählerstand (Gas)",
        13003 => "Summenzeitreihe (MaBiS)",
        13005 => "EEG-Überführungszeitreihe",
        13006 => "Messwert Storno",
        13007 => "Gasbeschaffenheit — Brennwert + Zustandszahl",
        13008 => "Lastgang (Gas)",
        13009 => "Energiemenge (Gas)",
        13010 => "Normiertes Profil",
        13011 => "Profilschar",
        13012 => "TEP vergleichbare Werte Referenzmessung",
        13013 => "Marktlokationsscharfe Allokationsliste Gas (MMMA)",
        13014 => "Marktlokationsscharfe bilanzierte Menge Strom/Gas (MMMA)",
        13015 => "Arbeit + Leistungsmaximum im Kalenderjahr vor Lieferbeginn",
        13016 => "Energiemenge und Leistungsmaximum",
        13017 => "Zählerstand (Strom)",
        13018 => "Lastgang Messlokation, Netzkoppelpunkt, Netzlokation",
        13019 => "Energiemenge (Strom)",
        13020 => "Ausfallarbeitsüberführungszeitreihe (Redispatch 2.0)",
        13021 => "Übermittlung von meteorologischen Daten (Redispatch 2.0)",
        13022 => "Redispatch 2.0 Einzelzeitreihe Ausfallarbeit",
        13023 => "Redispatch 2.0 Ausfallarbeitssummenzeitreihe",
        13025 => "Lastgang Marktlokation, Tranche",
        13026 => "EEG-Überführungszeitreihe aufgrund Ausfallarbeit",
        13027 => "Werte nach Typ 2",
        13028 => "Grundlage POG-Ermittlung",
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

/// Metering / balancing classification of a Marktlokation.
///
/// Re-exported from `metering` (`Slp` / `Rlm` / `IMsys`) — the single source of
/// truth for the Messtyp. `metering::Messtyp` has no `Display`/`FromStr` (the
/// orphan rule would forbid us adding them here anyway), so the DB-string
/// conversions live as the free helpers [`messtyp_as_str`] / [`messtyp_from_str`].
pub use metering::Messtyp;

/// The DB / wire string for a [`Messtyp`] (`SLP` / `RLM` / `IMSYS`).
#[must_use]
pub fn messtyp_as_str(m: Messtyp) -> &'static str {
    match m {
        Messtyp::Slp => "SLP",
        Messtyp::Rlm => "RLM",
        Messtyp::IMsys => "IMSYS",
    }
}

/// Parse a [`Messtyp`] from its DB / wire string; unknown values fall back to
/// `Slp` (the conservative Vorlauffrist / aggregation default).
#[must_use]
pub fn messtyp_from_str(s: &str) -> Messtyp {
    match s.to_ascii_uppercase().as_str() {
        "RLM" => Messtyp::Rlm,
        "IMSYS" | "IMS" => Messtyp::IMsys,
        _ => Messtyp::Slp,
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
/// Every interval must be traceable to its origin for § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) compliance.
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
    /// Automatic substitute value generated by `edmd` per § 60 Abs. 2 MsbG.
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
    /// Every variant. The ingestion source rides as a `meterstore` **attribute
    /// column** on the reading (`source`), not a column in edmd's own schema —
    /// `meter_reads` is a `meterstore` table, so there is no edmd-side
    /// `meter_reads.source` CHECK to pin this against. `quality_assessments.source`
    /// is a *different*, narrower "ingest family" vocabulary (see the CHECK there).
    pub const ALL: [Self; 9] = [
        Self::Mscons,
        Self::DirectPush,
        Self::DirectGas,
        Self::ApiImport,
        Self::AutoSubstitute,
        Self::Correction,
        Self::Manual,
        Self::Estimated,
        Self::IotPush,
    ];

    /// Whether edmd itself authored the value, rather than receiving it.
    ///
    /// The distinction decides how a write resolves against what is already
    /// stored. A *delivery* — MSCONS, direct push, IoT, a bulk import — is
    /// legitimately shadowed by a newer one, and forcing it to win would let a
    /// replayed original supersede the correction that fixed it. A value edmd
    /// authored is not a delivery: it is edmd asserting a figure about a slot
    /// whose current content it has just read and judged unusable — a § 60
    /// Abs. 2 MsbG Ersatzwert for a `FAULTY` reading, or an operator's explicit
    /// correction — and it has to take effect or be refused, never be silently
    /// outranked. See `store::MeterStoreTimeSeriesRepository::append_superseding`.
    ///
    /// `Manual` and `Estimated` are operator entries, so they are authored too.
    #[must_use]
    pub fn is_edmd_authored(self) -> bool {
        matches!(
            self,
            Self::AutoSubstitute | Self::Correction | Self::Manual | Self::Estimated
        )
    }

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

    /// Parse a DB / wire string, or `None` when it names no known source.
    ///
    /// Returning `None` rather than falling back is what stops a caller's
    /// provenance being quietly rewritten. `POST /api/v1/meter-reads/rlm/…`
    /// documents `"source": "SMGW"` and `"CLS_GATEWAY"` as examples; both fell
    /// through the old catch-all and were stored as `MSCONS`, so a reading that
    /// never touched EDIFACT claimed to have arrived by it. § 60 Abs. 1 MsbG
    /// attribution needs the door a value actually came in by.
    #[must_use]
    pub fn parse_db_str(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|source| source.as_str().eq_ignore_ascii_case(s))
    }

    /// Parse from a DB string value, falling back to `Mscons`.
    ///
    /// For **read-back only**, where the column is CHECK-constrained so an
    /// unknown value means enum and schema have diverged and there is nothing
    /// better to return. Ingest paths use [`Self::parse_db_str`] and refuse.
    #[must_use]
    pub fn from_db_str(s: &str) -> Self {
        Self::parse_db_str(s).unwrap_or(Self::Mscons)
    }
}

/// Read a delivered quality label into a [`QualityFlag`].
///
/// The vocabulary MSCONS `SG10 STS+Z34` decodes to, and the one every ingest
/// door speaks — the MSCONS fork, the SMGW push, the IoT batch. Shared rather
/// than copied: two doors filing the same label under different flags is a
/// difference no read-back can see, and `billable_qualities()` is what decides
/// whether a value settles.
///
/// An absent or unrecognised label is [`QualityFlag::Unknown`], never
/// `Measured`: „nothing was said" is not „it was measured", and the two differ
/// in whether the value may be billed.
#[must_use]
pub fn quality_from_label(label: Option<&str>) -> QualityFlag {
    match label.unwrap_or_default() {
        "MEASURED" => QualityFlag::Measured,
        "ESTIMATED" => QualityFlag::Estimated,
        "SUBSTITUTED" => QualityFlag::Substituted,
        "CALCULATED" => QualityFlag::Calculated,
        "CORRECTED" => QualityFlag::Corrected,
        "PRELIMINARY" => QualityFlag::Preliminary,
        "FAULTY" => QualityFlag::Faulty,
        _ => QualityFlag::Unknown,
    }
}

/// The transport an ESA Typ-2 value arrived on (Codeliste 1.4 Kap. 4.6).
///
/// Stored in `esa_typ2_reads.delivery_path`. Both paths carry the *same*
/// non-authoritative Typ-2 values; only the transport differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Typ2DeliveryPath {
    /// 4.6.1 — "Werte nach Typ 2 aus Backend": EDIFACT MSCONS from the MSB.
    #[default]
    MsconsBackend,
    /// 4.6.2 — "Werte nach Typ 2 aus SMGW": XML over SM-PKI, direct from the iMS.
    SmgwDirect,
}

impl Typ2DeliveryPath {
    /// Every variant — `schema_code_guard` pins the CHECK constraint against it.
    pub const ALL: [Self; 2] = [Self::MsconsBackend, Self::SmgwDirect];

    /// Returns the DB string value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MsconsBackend => "MSCONS_BACKEND",
            Self::SmgwDirect => "SMGW_DIRECT",
        }
    }

    /// Parse from a DB string value.
    #[must_use]
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "SMGW_DIRECT" => Self::SmgwDirect,
            _ => Self::MsconsBackend,
        }
    }
}

/// An ESA-delivered **"Werte nach Typ 2"** interval (MSCONS PID 13027).
///
/// Deliberately *not* a [`MeterRead`], and stored in a *separate* table
/// (`esa_typ2_reads`), because Typ-2 data is non-authoritative: Codeliste 1.4
/// Kap. 4.6 and WiM Strom Teil 2 §4 give it **no bearing** on Netznutzungs-,
/// Bilanzkreis- or Mehr-/Mindermengenabrechnung. The separation is structural —
/// there is no `source`/`pid` discriminator on the billing store that could
/// leak by omission, and this type carries **none** of `MeterRead`'s billing
/// machinery (no `allocation_version`, no correction/substitution provenance,
/// no billing-period participation): a Typ-2 value is stored as delivered and
/// never reconciled against, corrected, or substituted for a Typ-1 value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Typ2Read {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// 33-character Messlokations-ID, if available.
    pub melo_id: Option<String>,
    /// Interval start (UTC).
    pub dtm_from: OffsetDateTime,
    /// Interval end (UTC).
    pub dtm_to: OffsetDateTime,
    /// Energy quantity in kWh (or m³ for water/gas volume, per `sparte`).
    pub quantity_kwh: Decimal,
    /// Quality of the reading, as delivered by the MSB.
    pub quality: QualityFlag,
    /// Source PID — 13027 for the MSCONS backend path.
    pub pid: u32,
    /// Energy commodity.
    pub sparte: Sparte,
    /// OBIS-Kennzahl, when the delivery carried one.
    pub obis_code: Option<String>,
    /// Tenant data-isolation key.
    pub tenant: String,
    /// Which transport delivered this value (Codeliste 1.4 Kap. 4.6).
    #[serde(default)]
    pub delivery_path: Typ2DeliveryPath,
    /// MP-ID of the MSB that delivered the values.
    #[serde(default)]
    pub sender_mp_id: Option<String>,
    /// `SG1 RFF+AGI` on the delivering MSCONS 13027 — the Belegnummer of the
    /// ORDERS 17007 that ordered these values.
    ///
    /// MSCONS AHB 3.2 §11.2 hint `[574]`: „Wert aus BGM DE1004 der ORDERS mit
    /// der die Bestellung der Werte nach Typ 2 erfolgt ist", and the first hop
    /// of the PID overview's `EZ-03` routing. It is the **only** thing on a
    /// value delivery that names the subscription it belongs to — a Meldepunkt
    /// may carry several, since a subscription is the (Meldepunkt,
    /// Messprodukt) pair.
    ///
    /// `None` on a delivery from a counterparty that omitted the Muss, and on
    /// a 4.6.2 value that arrived over SM-PKI rather than as EDIFACT.
    #[serde(default)]
    pub bestellung_ref: Option<String>,
    /// When `edmd` received the value (database clock on write).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at: Option<OffsetDateTime>,
}

/// Default allocation version for `serde` deserialization — see `MeterRead.allocation_version`.
fn default_allocation_version() -> String {
    "INITIAL".to_owned()
}

/// One cumulative register reading (**Zählerstand**) at an instant.
///
/// What an intelligentes Messsystem actually measures. § 2 Satz 1 Nr. 27 MsbG
/// defines the Zählerstandsgang verbatim as *"die Messung einer Reihe
/// **viertelstündig ermittelter Zählerstände** von elektrischer Arbeit und
/// **stündlich ermittelter Zählerstände** von Gasmengen"* — two media, two
/// resolutions — and **BK6-24-174** („Datenübermittlung ZSG", Beschluss
/// 24.10.2024, wirksam 06.06.2025) puts the differencing at the
/// Messstellenbetreiber:
///
/// ```text
/// SMGW ──Zählerstandsgang──► MSB ──Lastgang──► NB, Lieferant
///                             └── edmd
/// ```
///
/// So a [`MeterRead`] is *derived* and this is the primary record. Both are
/// kept: § 146 Abs. 4 AO requires the original to stay recoverable, and a stored
/// difference cannot reproduce the register values it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterReading {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// The instant the register held this value (UTC).
    pub read_at: OffsetDateTime,
    /// The register value, **in the unit the register counts** — kWh for
    /// electricity and heat, m³ for gas and water.
    ///
    /// Deliberately unconverted. § 25 Nr. 4 MessEV converts the *difference*
    /// between two readings, not a register value, and a Zählerstand rewritten
    /// into kWh is no longer the number on the meter an operator reads off it.
    pub zaehlerstand: Decimal,
    /// Quality of this reading.
    pub quality: QualityFlag,
    /// Commodity — decides the register's unit and the expected cadence.
    pub sparte: Sparte,
    /// OBIS register, e.g. `1-0:1.8.0` (a Zählerstand: value group `D = 8`).
    pub obis_code: Option<String>,
    /// 33-character Messlokations-ID — the **meter** the register belongs to.
    ///
    /// Part of a reading's identity, not a label. A Marktlokation may be
    /// measured by several Messlokationen, and two meters carry the same OBIS
    /// register at the same instants: keyed on the Marktlokation alone, the
    /// second meter's Zählerstandsgang reads as a restatement of the first and
    /// silently overwrites it. A Lastgang is the other shape — one channel
    /// however many meters produce it — which is why the interval store is
    /// keyed on the Marktlokation and this is not.
    pub melo_id: Option<String>,
    /// Owning tenant.
    pub tenant: String,
    /// Which door the reading came in by.
    pub source: IngestionSource,
    /// BDEW Codenummer of the reporting MSB / network operator.
    pub sender_mp_id: Option<String>,
    /// Idempotency key of the delivering session, when there was one.
    pub push_session: Option<String>,
}

/// What the Zählerstandsgang → Lastgang conversion did across one span.
///
/// Written to `zsg_conversion_log`. Two different facts share the table because
/// they answer the same question — *what happened between these two readings,
/// and why* — which an auditor asks without knowing the answer:
///
/// - a reconstructed register **wrap**, where the interval exists and the
///   conversion added the register capacity to the difference on the strength of
///   a configured device width;
/// - an **anomaly**, where no honest difference could be taken and the interval
///   is therefore absent. It surfaces downstream as a V01 gap and is filled by
///   the § 60 Abs. 2 MsbG substitute path, which writes its own audit row.
///
/// Together the two logs say "this quarter-hour is an Ersatzwert *because* the
/// register went backwards here", which neither says alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZsgConversionEntry {
    pub tenant: String,
    pub malo_id: String,
    /// Canonical OBIS spelling, empty for an unlabelled register.
    pub obis_code_norm: String,
    pub span_from: OffsetDateTime,
    pub span_to: OffsetDateTime,
    /// [`ZSG_OUTCOME_ROLLOVER`], or the `AnomalyKind` that refused the
    /// difference.
    pub outcome: &'static str,
    pub previous_value: Decimal,
    pub current_value: Decimal,
    /// Reconstructed consumption across a wrap. `None` for an anomaly.
    pub delta: Option<Decimal>,
    /// The register capacity that explained a wrap. `None` for an anomaly.
    pub register_capacity: Option<Decimal>,
    pub session_id: Option<String>,
}

/// The `zsg_conversion_log.outcome` for a reconstructed register wrap.
pub const ZSG_OUTCOME_ROLLOVER: &str = "ROLLOVER";

/// The whole `zsg_conversion_log.outcome` vocabulary.
///
/// [`ZSG_OUTCOME_ROLLOVER`] plus every [`AnomalyKind`], in the crate's own
/// declaration order. `schema_code_guard` pins the DB `CHECK` against it, so a
/// kind added upstream fails the build rather than an insert at runtime — and
/// the insert is where an audit row would otherwise go missing with a warning.
///
/// [`AnomalyKind`]: metering::reading::AnomalyKind
#[must_use]
pub fn zsg_outcomes() -> Vec<&'static str> {
    std::iter::once(ZSG_OUTCOME_ROLLOVER)
        .chain(
            metering::reading::AnomalyKind::ALL
                .iter()
                .copied()
                .map(anomaly_outcome),
        )
        .collect()
}

/// The stored `outcome` label for an anomaly kind.
///
/// Spelled out rather than derived from `Debug`, because the column is
/// CHECK-constrained and a rename upstream would turn every audit insert into a
/// silent warning. `AnomalyKind` is `#[non_exhaustive]`, so a kind this does not
/// know falls back to the label that says a difference was refused without
/// claiming to know why.
#[must_use]
pub fn anomaly_outcome(kind: metering::reading::AnomalyKind) -> &'static str {
    use metering::reading::AnomalyKind as K;
    match kind {
        K::BackwardsWithoutRegisterWidth => "BACKWARDS_WITHOUT_REGISTER_WIDTH",
        K::ImplausibleRollover => "IMPLAUSIBLE_ROLLOVER",
        K::ImplausibleDelta => "IMPLAUSIBLE_DELTA",
        K::ZeroLengthSpan => "ZERO_LENGTH_SPAN",
        K::NonBillableEndpoint => "NON_BILLABLE_ENDPOINT",
        _ => "IMPLAUSIBLE_DELTA",
    }
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

    // ── Provenance tracking (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD)) ───────────────────────────────────────
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
    /// `None` = no warnings. Triggers `de.messwert.reading.quality.warning` CloudEvent.
    #[serde(default)]
    pub quality_warnings: Option<serde_json::Value>,

    // ── F-12: Extended provenance fields (migrations 0006–0007) ────────────────
    /// MP-ID of the MSB or system that delivered this reading.
    ///
    /// Populated from `meter_data_receipts.sender_mp_id` (MSCONS path) or from the
    /// direct-push API header. Required for § 60 Abs. 1 MsbG per-interval MSB attribution
    /// after an MSB switch (WiM PID 55039).
    #[serde(default)]
    pub sender_mp_id: Option<String>,

    /// Delivery label carried with the reading — provenance, not control flow.
    ///
    /// Ingest paths write `"INITIAL"`; the correction path writes `"CORRECTION"`;
    /// the ESA Typ-2 route carries its own labels (`"ESA-…"`). The column is an
    /// **open** string by design (`store.rs` declares it a nullable attribute
    /// column, not an enum), so it can record whatever a delivery called itself.
    ///
    /// It is deliberately *not* how preliminary is told from final. That
    /// question is answered by transaction time: `mabis-syncd` asks for the
    /// readings as they were known at the Erstaufschlag versus at the Clearing
    /// deadline via `repo.query_as_of` (see `valid_from_tx` below). A label
    /// would have to be maintained in lockstep with the real correction history
    /// and would drift; the `recorded_at` ceiling cannot.
    #[serde(default = "default_allocation_version")]
    pub allocation_version: String,

    /// Transaction time: when this row was written (database clock).
    ///
    /// It becomes the meterstore row's `recorded_at`, which is also the source of
    /// its `version` — so a correction is a new version at a later `recorded_at`,
    /// not an overwrite. "What did we know at time T?" is therefore answered
    /// directly by a transaction-time read (`repo.query_as_of` →
    /// `store.as_known_at(T)`): resolution under a `recorded_at ≤ T` ceiling
    /// returns the version in force then, and excludes intervals first stored
    /// after T. The `meter_read_corrections` table remains the human-readable
    /// audit log (who/when/why), not the reconstruction mechanism.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from_tx: Option<OffsetDateTime>,

    /// The MSCONS correction version this reading was delivered under.
    ///
    /// MSCONS assigns a numeric, monotonically ascending version per network
    /// operator per month, and **that** is what decides which of two deliveries
    /// for one interval wins — not the order they happened to arrive in.
    /// Without it, replaying an original after its correction landed gives the
    /// stale value the higher version and silently supersedes the correction.
    ///
    /// `None` when the delivery carried no version, in which case `store.rs`
    /// falls back to transaction time. Today that is every MSCONS delivery: the
    /// `process.completed` payload does not carry the version off the wire yet,
    /// so only callers that state one explicitly (direct push, Kafka batch) can
    /// populate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mscons_version: Option<u128>,
}

/// Query parameters for time-series reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesQuery {
    pub malo_id: String,
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
    pub sparte: Option<Sparte>,
    /// Tenant data-isolation key. Not optional: `store` scopes every read on it
    /// (`column_eq(TENANT_COL, …)`), and a query that could omit it would fold
    /// two tenants' readings for one MaLo into a single series.
    pub tenant: String,
}

/// Mehr-/Mindermengensaldo for one MaLo and one billing period.
///
/// ## The two halves, and which one edmd owns
///
/// The saldo compares a **measured** quantity against a **bilanzierte** one, and
/// edmd holds only the first. The bilanzierte Menge is what the balancing side
/// allocated to the Bilanzkreis from the load profile — a commercial figure in
/// the supplier's system, not a measurement — so it is an *input* to this report
/// (`bilanziert_kwh`), never something edmd can derive.
///
/// The previous shape had `lf_quantity_kwh` and `nb_quantity_kwh` and filled
/// both from the same measured total, so `delta_kwh` was structurally zero: an
/// imbalance report that could not report an imbalance. Naming the two halves for
/// what they are makes that unrepresentable.
///
/// ## Sign convention
///
/// Both quantities are named from the **network operator's** side, which inverts
/// the intuitive reading (GPKE Teil 1 Kap. 8.4 Nr. 3): a customer consuming
/// *less* than the profile leaves surplus energy the NB absorbed, and that
/// surplus is the **Mehrmenge**, which the NB credits. Consuming more is the
/// **Mindermenge**, which the NB invoices. Only one of the two is ever positive.
/// The arithmetic is [`metering::compute_imbalance`]'s, so the convention lives
/// in one place.
///
/// ## Legal basis
///
/// GPKE (BK6-24-174) Teil 1 Kap. 8.4 for Strom; GaBi Gas 2.1 (BK7-24-01-008)
/// Ziff. 3a for Gas. **Not** § 13 StromNZV / § 25 GasNZV — both ceased to be in
/// effect at the end of 31 December 2025, when the Bundesnetzagentur folded
/// their content into the Festlegungen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImbalanceReport {
    pub malo_id: String,
    /// Start of billing period (inclusive).
    pub period_from: Date,
    /// End of billing period (inclusive).
    pub period_to: Date,
    /// The commodity, which decides whether the period runs on calendar days or
    /// on the 06:00 Gastag.
    pub sparte: Sparte,
    /// Measured energy in the period (kWh) — billable qualities only. edmd's
    /// half of the comparison.
    pub gemessen_kwh: Decimal,
    /// Bilanzierte (profile-allocated) energy in the period (kWh), as supplied
    /// by the caller.
    pub bilanziert_kwh: Decimal,
    /// `max(0, bilanziert − gemessen)` — the NB credits the LF.
    pub mehrmenge_kwh: Decimal,
    /// `max(0, gemessen − bilanziert)` — the NB invoices the LF.
    pub mindermenge_kwh: Decimal,
    /// `gemessen − bilanziert`. Positive is a Mindermenge.
    pub delta_kwh: Decimal,
    /// The delta as a percentage of the bilanzierte quantity. `None` when that
    /// is zero — a ratio against nothing is not zero, it is undefined.
    pub delta_pct: Option<Decimal>,
    /// Worst quality flag across the reads that contributed.
    pub quality: QualityFlag,
    /// How many intervals the measured total is built from, so a caller can see
    /// whether the period is actually covered.
    pub interval_count: usize,
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
/// Source: GPKE (BK6-24-174) Teil 1; GeLi Gas 3.0 (BK7-24-01-009).
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
    /// Which commodity, because it decides where the day *starts*.
    ///
    /// A Strom period runs 00:00–00:00 Berlin; a Gas period runs the Gastag,
    /// 06:00–06:00 (GaBi Gas, Art. 3 Nr. 6 VO (EU) 312/2014). Aggregating gas
    /// over calendar days books the 00:00–06:00 draw into the neighbouring
    /// Bilanzierungstag — six hours, every day of the year, not only across a
    /// DST transition.
    pub sparte: Sparte,
}

/// One Gasbeschaffenheit delivery (MSCONS PID 13007, `QTY+Z08` / `QTY+Z10`).
///
/// Brennwert and Zustandszahl are only meaningful together with the period they
/// apply to: the gas grid operator publishes an Abrechnungsbrennwert per supply
/// area per month, and `kWh = m³ × Hs × Z` uses the one in force for the
/// consumption month. Storing the pair without its period — by patching
/// `meter_billing_periods` alone — leaves no record of *which* month's value was
/// applied.
///
/// Source: MSCONS AHB Gas; Allgemeine Festlegungen §6; § 25 Nr. 4 MessEV /
/// DVGW G 685.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasQualityRecord {
    /// Tenant data-isolation key.
    pub tenant: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// First day the values apply to (inclusive).
    pub period_from: Date,
    /// Last day the values apply to (inclusive).
    pub period_to: Date,
    /// Abrechnungsbrennwert Hs in kWh/m³ (`QTY+Z08`).
    pub brennwert_kwh_per_m3: Option<Decimal>,
    /// Zustandszahl, dimensionless (`QTY+Z10`).
    pub zustandszahl: Option<Decimal>,
    /// PID the values were delivered under (13007).
    pub source_pid: Option<u32>,
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
/// enabling full § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit-trail reconstruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionRecord {
    /// MaLo for the corrected interval.
    pub malo_id: String,
    /// OBIS register the correction applies to.
    ///
    /// Part of the reading's primary key. A MaLo may carry several registers at
    /// one timestamp (import and export, HT and NT), so a correction that does
    /// not name one cannot identify the reading it means to change.
    #[serde(default)]
    pub obis_code: Option<String>,
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

#[cfg(test)]
mod mscons_pid_tests {
    use super::{ALL_MSCONS_PIDS, MSCONS_PIDS, mscons_pid_description};

    /// Every PID the platform accepts must have a name taken from the AHB.
    ///
    /// A receipt labelled "Unbekannter MSCONS PID" tells an operator nothing,
    /// and a *wrong* label is worse — it sends them to the wrong AHB section.
    #[test]
    fn every_accepted_pid_is_named() {
        for &pid in ALL_MSCONS_PIDS {
            assert_ne!(
                mscons_pid_description(pid),
                "Unbekannter MSCONS PID",
                "PID {pid} is accepted but has no description"
            );
        }
    }

    /// Names pinned to the AHB 3.2 "Tabellenspalte" headings, so an edit
    /// cannot quietly reword them.
    #[test]
    fn names_match_the_ahb_tabellenspalte() {
        for (pid, expected) in [
            (13003, "Summenzeitreihe (MaBiS)"),
            (13005, "EEG-Überführungszeitreihe"),
            (13006, "Messwert Storno"),
            (
                13015,
                "Arbeit + Leistungsmaximum im Kalenderjahr vor Lieferbeginn",
            ),
            (13016, "Energiemenge und Leistungsmaximum"),
            (
                13018,
                "Lastgang Messlokation, Netzkoppelpunkt, Netzlokation",
            ),
            (13019, "Energiemenge (Strom)"),
            (13025, "Lastgang Marktlokation, Tranche"),
            (13026, "EEG-Überführungszeitreihe aufgrund Ausfallarbeit"),
            (13027, "Werte nach Typ 2"),
        ] {
            assert_eq!(mscons_pid_description(pid), expected, "PID {pid}");
        }
    }

    /// The doc table on `MSCONS_PIDS` and `mscons_pid_description` render one
    /// source, so they must not drift — and they had, on five of eleven rows.
    #[test]
    fn pid_table_matches_descriptions() {
        // The table as it appears in the `MSCONS_PIDS` doc comment.
        let table = include_str!("model.rs");
        let table = table
            .split("/// | PID   | Direction   | Anwendungsfall |")
            .nth(1)
            .expect("the PID table is in the MSCONS_PIDS doc comment")
            .split("///\n")
            .next()
            .expect("table block");

        for &pid in MSCONS_PIDS {
            assert_eq!(
                table.matches(&format!("| {pid} |")).count(),
                1,
                "PID {pid} is subscribed but not documented exactly once"
            );
        }

        // The Anwendungsfall column is the AHB heading, so the check is on the
        // distinguishing term rather than on string equality.
        for (pid, term) in [
            (13005u32, "EEG-Überführungszeitreihe"),
            (13006, "Messwert Storno"),
            (13007, "Gasbeschaffenheit"),
            (13013, "Allokationsliste"),
            (13015, "Leistungsmaximum im Kalenderjahr"),
            (13016, "Energiemenge und Leistungsmaximum"),
            (13017, "Zählerstand (Strom)"),
            (13018, "Lastgang Messlokation"),
            (13019, "Energiemenge (Strom)"),
            (13025, "Lastgang Marktlokation"),
            (13027, "Werte nach Typ 2"),
        ] {
            assert!(
                table.contains(term),
                "the table must name PID {pid} as {term:?}"
            );
            let described = mscons_pid_description(pid);
            assert!(
                described.contains(term) || term.contains(described),
                "PID {pid}: table says {term:?}, description says {described:?}"
            );
        }
    }

    /// A Storno withdraws values, so it must be received but never treated as a
    /// value delivery.
    #[test]
    fn storno_pids_are_subscribed_but_are_not_value_deliveries() {
        for &pid in super::STORNO_PIDS {
            assert!(
                MSCONS_PIDS.contains(&pid),
                "a Storno must still be received: {pid}"
            );
            assert!(
                ALL_MSCONS_PIDS.contains(&pid),
                "a Storno must be an accepted PID: {pid}"
            );
            assert!(
                !super::ESA_TYP2_PIDS.contains(&pid),
                "a Storno is not a Typ-2 delivery: {pid}"
            );
        }
    }

    /// The Redispatch subset must be a subset of what the platform accepts.
    #[test]
    fn redispatch_pids_are_accepted() {
        for &pid in super::REDISPATCH_MSCONS_PIDS {
            assert!(
                ALL_MSCONS_PIDS.contains(&pid),
                "Redispatch PID {pid} is not in ALL_MSCONS_PIDS, so it would be ignored"
            );
        }
    }
}
