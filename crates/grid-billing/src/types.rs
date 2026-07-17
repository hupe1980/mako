//! Input/output types for the settlement calculation engine.
//!
//! ## Architecture
//!
//! The preferred calculation flow is:
//!
//! ```text
//! Input → Validation → Settlement Engine → GridSettlement → Invoice Adapter → BO4E → EDIFACT
//! ```
//!
//! [`GridSettlement`] is the canonical output. It carries every billing position
//! alongside its [`CalculationTrace`], applicable [`LegalReference`]s, the
//! [`TariffSource`] that justified each rate, and any [`SettlementWarning`]s.
//!
//! The service layer (`netzbilanzd`, `invoicd`) adapts `GridSettlement` into
//! `rubo4e::current::Rechnung` via a local `into_rechnung()` helper — keeping
//! BO4E as a purely rendering concern outside this crate.
//!
//! ## No float money
//!
//! All monetary amounts use [`rust_decimal::Decimal`]. The `billing::EuroAmount`
//! newtype provides overflow-safe EUR arithmetic. No `f32`/`f64` appears anywhere
//! in settlement calculations.

use rust_decimal::Decimal;

// ── Sparte ────────────────────────────────────────────────────────────────────

/// Commodity — Strom (electricity) or Gas.
///
/// Controls which legal references are applied to each settlement position:
/// - `Strom` → `StromNEV`, `StromNZV`, BK6 decisions
/// - `Gas` → `GasNEV`, `GasNZV`, BK7 decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sparte {
    /// Electricity (Strom). Default.
    #[default]
    Strom,
    /// Natural gas (Gas).
    Gas,
}

// ── KaKlasse ──────────────────────────────────────────────────────────────────

/// KAV §2 concession fee rate class.
///
/// Different annual consumption bands attract different KAV rates per
/// KAV §2 Abs. 2. Providing the class makes each position's audit trace
/// self-explanatory: auditors can verify the rate matches the class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KaKlasse {
    /// Tarifkunde ≤ 25 MWh/a — residential (highest rate tier).
    TarifkundeLow,
    /// Tarifkunde > 25 MWh/a and ≤ 150 MWh/a — commercial.
    TarifkundeMedium,
    /// Sonderkunde / Industriekunde > 150 MWh/a.
    SonderkundeHigh,
    /// Exempt from KA (hospitals, water utilities, §2 Abs. 7 KAV).
    Exempt,
}

// ── QuantityUnit ──────────────────────────────────────────────────────────────

/// Unit of measure for a settlement position quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityUnit {
    /// Kilowatt-hours (active energy).
    Kwh,
    /// Kilowatts (demand / peak load).
    Kw,
    /// Reactive energy (Blindarbeit) — kilovolt-ampere reactive hours.
    ///
    /// Used for reactive energy settlement positions per StromNEV §18.
    Kvarh,
    /// Reactive power (Blindleistung) — kilovolt-ampere reactive.
    Kvar,
    /// Calendar months.
    Monat,
}

// ── Sect14aModule ─────────────────────────────────────────────────────────────

/// §14a EnWG module for steuerbare Verbrauchseinrichtungen (controllable loads).
///
/// Source: BNetzA BK6-22-300 (Beschluss 27.11.2023, in force 01.01.2024).
///
/// All three modules are **mandatory** for eligible controllable loads (heat pumps,
/// EV chargers, battery storage ≥ 4.2 kW) registered with the NB. The LF/NB
/// must offer at least Modul 1 to all eligible customers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sect14aModule {
    /// Modul 1 — pauschale Reduzierung (flat reduction).
    ///
    /// The NB applies a fixed percentage reduction to the Arbeitspreis (or
    /// Arbeitspreis + Leistungspreis) for the entire billing period.
    /// Reduction factor = 85 % (i.e. customer pays 85 % of full rate) per BK6-22-300
    /// Anlage 2. The NB may set a different approved rate in their tariff sheet.
    ///
    /// Equivalent UTILTS segment: `CCI+ZG6 CAV+Z28:::0.85` (multiplier).
    Modul1,
    /// Modul 2 — variable Netzentgelte (time-variable, HT/NT split).
    ///
    /// Two Arbeitspreis tiers: Hochlast (HT, higher price) and Niedertarif (NT,
    /// lower price). Periods are defined in the UTILTS Zählzeitdefinition published
    /// by the NB. Required for iMSys meters with quarter-hour metering.
    Modul2,
    /// Modul 3 — Spotpreis-Netzentgelt (dynamic, spot-price linked).
    ///
    /// NNE follows the intraday or day-ahead electricity spot price. The calculation
    /// basis is the `PreisblattNetznutzung.spotpreisNetzentgelt` formula defined by
    /// the NB. Requires smart meter (iMSys) with 15-min resolution.
    ///
    /// Note: Modul 3 rates are not yet calculable from static inputs alone —
    /// populate `regulatory_reduction_factor` in the trace with the effective
    /// period-average rate when using this module.
    Modul3,
}

impl Sect14aModule {
    /// Canonical BNetzA decision reference for this module.
    #[must_use]
    pub fn bnentza_reference(self) -> &'static str {
        "BK6-22-300"
    }

    /// Display label for the module.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Modul1 => "§14a EnWG Modul 1 (pauschale Reduzierung)",
            Self::Modul2 => "§14a EnWG Modul 2 (HT/NT variable)",
            Self::Modul3 => "§14a EnWG Modul 3 (Spotpreis)",
        }
    }
}

// ── SettlementType ────────────────────────────────────────────────────────────

/// Which regulated settlement process produced this result.
///
/// Determines which BDEW PIDs are applicable and which regulatory references apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementType {
    /// Netznutzungsentgelt (NNE) Strom — PID 31001 (NB → LF).
    NneStrom,
    /// Netznutzungsentgelt (NNE) Gas — PID 31005 (NB → LF, GasNEV).
    NneGas,
    /// NNE selbst ausgestellt (NB + LF = same entity) — PID 31006.
    NneSelbstausstellt,
    /// Mehr-/Mindermengen settlement Strom — PID 31002 (NB → LF, StromNZV §15).
    MmmStrom,
    /// Mehr-/Mindermengen settlement Gas — PID 31002 (NB → LF, GasNZV §14).
    ///
    /// Gas MMM settlement uses different legal references from Strom MMM:
    /// `GasNZV §14` and `GeLi Gas BK7-24-01-009`. Using a separate variant
    /// ensures correct audit traces without conditional logic in call sites.
    MmmGas,
    /// Messstellenbetrieb settlement — PID 31009 (NB → MSB).
    MsbRechnung,
    /// GaBi Gas AWH Sperrprozesse settlement — PID 31011 (NB → LF, BK7-24-01-009 §5.4).
    ///
    /// Rechnung sonstige Leistung: bills the LF (LFG/LFA) for abrechnungswürdige
    /// Handlungen (AWH) performed by the GNB/VNB during Sperrung/Entsperrung.
    GasAwhSperrung,
    /// Redispatch 2.0 Einsatzkosten (NB → ÜNB, BK6-20-061).
    RedispatchKostenblatt,
}

impl SettlementType {
    /// Default BDEW PID for this settlement type.
    ///
    /// Callers may override the PID after construction if needed.
    #[must_use]
    pub fn default_pid(self) -> u32 {
        match self {
            Self::NneStrom => 31001,
            Self::NneGas => 31005,
            Self::NneSelbstausstellt => 31006,
            Self::MmmStrom => 31002,
            Self::MmmGas => 31002,
            Self::MsbRechnung => 31009,
            Self::GasAwhSperrung => 31011,
            Self::RedispatchKostenblatt => 0, // no standard PID
        }
    }
}

// ── SettlementStatus ──────────────────────────────────────────────────────────

/// Lifecycle status of a settlement result.
///
/// Settlements are never destroyed — every correction or cancellation creates
/// a new result that references the original. This ensures an immutable audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementStatus {
    /// Initial calculation — no prior settlement exists for this period.
    Initial,
    /// Correction of a prior settlement (references `correction_of`).
    Correction,
    /// Cancellation of a prior settlement — all positions are negated.
    Reversal,
    /// Final settlement — no further corrections expected.
    Final,
}

// ── LegalReference ────────────────────────────────────────────────────────────

/// Regulatory citation that justifies a billing position or rate.
///
/// Every [`InvoicePosition`] should carry at least one `LegalReference`.
/// This enables full auditability: any operator or regulator can trace
/// exactly which paragraph, ruling, and version authorised each charge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegalReference {
    /// StromNEV — Stromnetzentgeltverordnung (grid usage charges, Strom).
    ///
    /// Example: `StromNev { paragraph: "§17" }` for Leistungspreise.
    StromNev {
        /// Paragraph reference, e.g. `"§17"`, `"§21"`.
        paragraph: &'static str,
    },
    /// GasNEV — Gasnetzentgeltverordnung (grid usage charges, Gas).
    GasNev {
        /// Paragraph reference, e.g. `"§14"`.
        paragraph: &'static str,
    },
    /// KAV — Konzessionsabgabenverordnung (municipal concession fee).
    ///
    /// Example: `Kav { paragraph: "§2 Abs. 2" }`.
    Kav {
        /// Paragraph reference, e.g. `"§2 Abs. 2"`.
        paragraph: &'static str,
    },
    /// §14a EnWG — Steuerbare Verbrauchseinrichtungen (controllable loads).
    ///
    /// Governs time-variable (ToU) NNE for heat pumps, EV chargers, etc.
    Sect14aEnwg {
        /// Module: Modul1 (flat reduction), Modul2 (HT/NT), or Modul3 (spot).
        module: Sect14aModule,
    },
    /// MessZV — Messzugangsverordnung (metering access).
    MessZv {
        /// Paragraph citation, e.g. `"§2"`, `"§17"`.
        paragraph: &'static str,
    },
    /// MsbG — Messstellenbetriebsgesetz (metering point operation).
    MsbG {
        /// Paragraph citation, e.g. `"§§6–7"`.
        paragraph: &'static str,
    },
    /// BNetzA decision (Beschluss).
    ///
    /// Example: `BnetzaDecision { reference: "BK6-22-300" }`.
    BnetzaDecision {
        /// Decision reference, e.g. `"BK6-22-300"`, `"BK6-24-174"`.
        reference: &'static str,
    },
    /// BDEW application handbook (Anwendungshandbuch).
    BdewAhb {
        /// AHB reference, e.g. `"GPKE BK6-22-024"`.
        reference: &'static str,
    },
    /// StromNZV — Stromnetzzugangsverordnung (grid access, Strom).
    StromNzv {
        /// Paragraph citation, e.g. `"§15"`.
        paragraph: &'static str,
    },
    /// GasNZV — Gasnetzzugangsverordnung (grid access, Gas).
    GasNzv {
        /// Paragraph citation, e.g. `"§15"`.
        paragraph: &'static str,
    },
    /// EnWG — Energiewirtschaftsgesetz (general energy law).
    Enwg {
        /// Paragraph citation, e.g. `"§14a"`.
        paragraph: &'static str,
    },
    /// ARegV — Anreizregulierungsverordnung (incentive regulation).
    ///
    /// ARegV §§17–21 define the allowed NNE revenue caps and efficiency targets.
    /// Relevant when documenting why a specific regulated tariff level was approved.
    ARegV {
        /// Paragraph citation, e.g. `"§17"`, `"§21"`.
        paragraph: &'static str,
    },
}

impl LegalReference {
    /// Short human-readable citation string (German).
    #[must_use]
    pub fn citation(&self) -> String {
        match self {
            Self::StromNev { paragraph } => format!("StromNEV {paragraph}"),
            Self::GasNev { paragraph } => format!("GasNEV {paragraph}"),
            Self::Kav { paragraph } => format!("KAV {paragraph}"),
            Self::Sect14aEnwg { module } => format!("§14a EnWG {}", module.label()),
            Self::MessZv { paragraph } => format!("MessZV {paragraph}"),
            Self::MsbG { paragraph } => format!("MsbG {paragraph}"),
            Self::BnetzaDecision { reference } => format!("BNetzA {reference}"),
            Self::BdewAhb { reference } => format!("BDEW {reference}"),
            Self::StromNzv { paragraph } => format!("StromNZV {paragraph}"),
            Self::GasNzv { paragraph } => format!("GasNZV {paragraph}"),
            Self::Enwg { paragraph } => format!("EnWG {paragraph}"),
            Self::ARegV { paragraph } => format!("ARegV {paragraph}"),
        }
    }
}

// ── TariffSource ──────────────────────────────────────────────────────────────

/// Origin of the tariff rate applied in a settlement position.
///
/// Every rate used in a billing position must be traceable to a `TariffSource`.
/// This enables operators and auditors to answer: *"Why was this rate used?"*
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TariffSource {
    /// Rate from the published and approved `PreisblattNetznutzung` tariff sheet.
    PublishedTariffSheet {
        /// Tariff sheet identifier or version, e.g. `"Preisblatt 2025 Q1"`.
        sheet_id: String,
    },
    /// Rate from a historical tariff (retroactive billing or correction).
    HistoricalTariff {
        /// Original valid_from date of the tariff.
        valid_from: time::Date,
    },
    /// Regulatory rate mandated by a BNetzA decision.
    RegulatoryTariff {
        /// BNetzA decision reference.
        decision_ref: &'static str,
    },
    /// Contract-specific rate negotiated between NB and customer.
    ContractTariff {
        /// Contract reference.
        contract_ref: String,
    },
    /// Manual override by operator (requires documentation).
    ManualOverride {
        /// Reason for the override.
        reason: String,
    },
}

// ── CalculationTrace ──────────────────────────────────────────────────────────

/// Full audit record for how one [`InvoicePosition`] was computed.
///
/// Answers the question: *"Why is this amount on the invoice?"*
///
/// Every `CalculationTrace` carries the input values, the applied legal rules,
/// intermediate results, and the tariff source. This enables:
/// - Regulator audits (BNetzA §20 EnWG)
/// - Operator review
/// - LF dispute resolution
/// - AI-assisted invoice explainability (MCP tools)
#[derive(Debug, Clone)]
pub struct CalculationTrace {
    /// Human-readable explanation of this position.
    ///
    /// Example: `"Arbeit 1500 kWh × 3.5 ct/kWh = 52.50 EUR"`
    pub explanation: String,
    /// Input quantity used (before rounding).
    pub input_quantity: Decimal,
    /// Input unit price in EUR (before rounding, already converted from ct).
    pub input_unit_price_eur: Decimal,
    /// Intermediate result before rounding (qty × price).
    pub gross_eur: Decimal,
    /// Applied legal references (at least one required).
    pub legal_refs: Vec<LegalReference>,
    /// Source of the tariff rate.
    pub tariff_source: Option<TariffSource>,
    /// Any §14a reductions applied, expressed as a fraction (0.0–1.0).
    ///
    /// `None` when no regulatory reduction applies.
    /// Example: `Some(Decimal::new(85, 2))` = 85% of full rate (15% reduction).
    pub regulatory_reduction_factor: Option<Decimal>,
    /// Notes on rounding applied.
    ///
    /// Example: `"rounded to 5 dp per StromNEV §17"`.
    pub rounding_note: Option<&'static str>,
}

// ── SettlementWarning ─────────────────────────────────────────────────────────

/// A non-blocking validation issue found during settlement calculation.
///
/// Warnings do not prevent the invoice from being generated but should be
/// reviewed before dispatch. The service layer may choose to block dispatch
/// on `Severity::Error` warnings.
#[derive(Debug, Clone)]
pub struct SettlementWarning {
    /// Severity: informational, warning, or error.
    pub severity: WarningSeverity,
    /// Machine-readable warning code.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
}

/// Severity level for [`SettlementWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WarningSeverity {
    /// Informational — no action required.
    Info,
    /// Potential issue — review recommended before dispatch.
    Warning,
    /// Definite issue — should be resolved before dispatch.
    Error,
}

// ── InvoicePosition ───────────────────────────────────────────────────────────

/// Semantic kind of a billing position — used by the service layer to derive
/// the correct `BdewArtikelnummer` for the BO4E `Rechnungsposition`.
///
/// `grid-billing` has no `rubo4e` dependency, so this enum is the bridge:
/// the service layer maps `BillingPositionKind` → `BdewArtikelnummer` in
/// `into_rechnung()`. Every position in every `GridSettlement` must carry
/// a `kind` so the INVOIC `Rechnungsposition.artikelnummer` is never missing.
///
/// ## BDEW INVOIC AHB requirement
///
/// BDEW INVOIC AHBs (FV2025-10-01) mandate `artikelnummer` in every
/// `SG28 PIA` line item. Missing or wrong Artikelnummern cause counterparty
/// APERAK rejection. The `invoic-checker` checks 6 plausibility rules;
/// Artikelnummer matching is part of the tariff-found rule (check 5).
///
/// ## Mapping to `BdewArtikelnummer`
///
/// | `BillingPositionKind` | `BdewArtikelnummer` | INVOIC AHB ref |
/// |---|---|---|
/// | `NneArbeit` | `Wirkarbeit` | PID 31001/31005/31006 Arbeit |
/// | `NneArbeitHt` | `Wirkarbeit` | PID 31001 §14a Modul 2 HT |
/// | `NneArbeitNt` | `Wirkarbeit` | PID 31001 §14a Modul 2 NT |
/// | `NneArbeitModul1` | `Wirkarbeit` | PID 31001 §14a Modul 1 (rate reduced) |
/// | `NneLeistung` | `Leistung` | PID 31001/31005 RLM kW charge |
/// | `NneGasGrundpreis` | `Grundpreis` | PID 31005 monthly base fee |
/// | `Konzessionsabgabe` | `Konzessionsabgabe` | PID 31001/31006 KAV §2 |
/// | `Mehrmenge` | `Mehrmenge` | PID 31002 positive imbalance |
/// | `Mindermenge` | `Mindermenge` | PID 31002 negative imbalance (credit) |
/// | `MsbGrundgebuehr` | `EntgeltEinbauBetriebWartungMesstechnik` | PID 31009 MSB monthly fee |
/// | `Messdienstleistung` | `EntgeltMessungAblesung` | PID 31009 reading service |
/// | `GasAwhSperrung` | `Sperrkosten` | PID 31011 AWH disconnection |
/// | `GasAwhEntsprrung` | `Entsperrkosten` | PID 31011 AWH reconnection |
/// | `GasAwhSonstige` | `EntgeltAbrechnung` | PID 31011 other AWH |
/// | `Blindmehrarbeit` | `Blindmehrarbeit` | Reactive energy excess |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingPositionKind {
    /// Netznutzungsentgelt Arbeit — flat-rate active energy charge (kWh).
    /// SLP or Gas. → `BdewArtikelnummer::Wirkarbeit`
    NneArbeit,
    /// §14a Modul 2 Hochlast (HT) Arbeit — time-variable higher-price band.
    /// → `BdewArtikelnummer::Wirkarbeit`
    NneArbeitHt,
    /// §14a Modul 2 Niedertarif (NT) Arbeit — time-variable lower-price band.
    /// → `BdewArtikelnummer::Wirkarbeit`
    NneArbeitNt,
    /// §14a Modul 1 Arbeit — flat percentage reduction applied to Arbeitspreis.
    /// → `BdewArtikelnummer::Wirkarbeit` (same article, different rate)
    NneArbeitModul1,
    /// §14a Modul 3 Spotpreis-NNE — per-dispatch-interval variable rate position.
    ///
    /// One `InvoicePosition` is generated per dispatch interval from
    /// `NneInput::sect14a_modul3_intervals`. Each carries a
    /// `lastvariable_preisposition_json` with the BO4E `LastvariablePreisposition`
    /// COM data (pricing formula parameters) for ERP-side validation and portal
    /// display of the per-interval tariff breakdown.
    ///
    /// Regulatory basis: BNetzA BK6-22-300 Anlage 2 §3 — Spotpreis-Netzentgelt.
    /// → `BdewArtikelnummer::Wirkarbeit`
    NneArbeitModul3,
    /// Netznutzungsentgelt Leistung — RLM peak demand charge (kW).
    /// → `BdewArtikelnummer::Leistung`
    NneLeistung,
    /// Gas NNE monthly base fee (Grundpreis / Verrechnungspreis).
    /// GasNEV §14. → `BdewArtikelnummer::Grundpreis`
    NneGasGrundpreis,
    /// Konzessionsabgabe — KAV §2 municipal concession fee.
    /// → `BdewArtikelnummer::Konzessionsabgabe`
    Konzessionsabgabe,
    /// Mehrmengen — positive imbalance (actual > profiled).
    /// PID 31002 StromNZV §15 / GasNZV §14. → `BdewArtikelnummer::Mehrmenge`
    Mehrmenge,
    /// Mindermengen — negative imbalance credit note (actual < profiled).
    /// PID 31002. → `BdewArtikelnummer::Mindermenge`
    Mindermenge,
    /// MSB Grundgebühr Messstellenbetrieb — monthly metering base fee.
    /// MsbG §§6–7. → `BdewArtikelnummer::EntgeltEinbauBetriebWartungMesstechnik`
    MsbGrundgebuehr,
    /// Messdienstleistung — periodic reading service fee.
    /// MessZV §2. → `BdewArtikelnummer::EntgeltMessungAblesung`
    Messdienstleistung,
    /// Gas AWH Sperrung — abrechnungswürdige Handlung disconnection.
    /// BK7-24-01-009 §5.4. → `BdewArtikelnummer::Sperrkosten`
    GasAwhSperrung,
    /// Gas AWH Entsperrung — abrechnungswürdige Handlung reconnection.
    /// BK7-24-01-009 §5.4. → `BdewArtikelnummer::Entsperrkosten`
    GasAwhEntsprrung,
    /// Gas AWH sonstige — other abrechnungswürdige Handlung.
    /// BK7-24-01-009 §5.4. → `BdewArtikelnummer::EntgeltAbrechnung`
    GasAwhSonstige,
    /// Blindmehrarbeit — reactive energy excess charge.
    /// StromNEV §18. → `BdewArtikelnummer::Blindmehrarbeit`
    Blindmehrarbeit,
}

/// One line item in a grid settlement.
///
/// Carries raw numbers for the service layer to map into the required format
/// (BO4E `Rechnungsposition`, EN16931 UBL, etc.).
///
/// Invariant: `net_eur == (quantity × unit_price_eur).round_dp(5)`.
#[derive(Debug, Clone)]
pub struct InvoicePosition {
    /// 1-based sequence number.
    pub number: u32,
    /// Human-readable position description.
    pub text: String,
    /// Semantic position kind — used by the service layer to set
    /// `Rechnungsposition.artikelnummer` (BDEW INVOIC AHB requirement).
    pub kind: BillingPositionKind,
    /// Metered or contracted quantity.
    pub quantity: Decimal,
    /// Unit of measure.
    pub unit: QuantityUnit,
    /// Unit price in EUR (already converted from ct where applicable).
    pub unit_price_eur: Decimal,
    /// Net amount in EUR, rounded to 5 decimal places.
    ///
    /// May be negative for credit positions (Mindermengen, Gutschriften).
    pub net_eur: Decimal,
    /// BDEW Artikel-ID for positions that use the new format (post-BK6-20-160).
    ///
    /// Used for:
    /// - **NNE Strom** (GPKE PIDs 31001/31006): BNetzA Netznutzungspreisblatt article IDs
    ///   (e.g. `"1-02-5-001"` for NS Grundpreis-/Arbeitspreissystem Arbeitspreis).
    ///   The service layer populates this from the `PreisblattNetznutzung` Artikel-ID.
    /// - **AWH Gas Sperrprozesse** (PID 31011): `"2-01-7-001"` (Sperrung) / `"2-01-7-002"` (Entsperrung)
    ///   populated automatically by `calculate_gas_awh_invoice()`.
    /// - **MSB** (PID 31009): `"4-02-0-xxx"` format from section 3.5 of the codelist,
    ///   populated by the service layer from the product/Konfigurationsprodukt code.
    ///
    /// For Gas NNE, MMM, Konzessionsabgabe and Blindmehrarbeit, the classic
    /// `artikelnummer` (via `BillingPositionKind`) takes precedence; this field is `None`.
    ///
    /// Source: BDEW Codeliste Artikelnummern und Artikel-ID v5.6 (valid 01.09.2025).
    pub artikel_id: Option<String>,
    /// §14a Modul 3: serialized `LastvariablePreisposition` BO4E COM for this position.
    ///
    /// Set when `kind == NneArbeitModul3`. Carries the pricing formula parameters
    /// (SPOTPREIS method, `preisreferenz`, `preisBezugseinheit`) from the
    /// `PreisblattNetznutzung.lastvariablePreispositionen` used to derive `unit_price_eur`.
    ///
    /// Stored as raw `serde_json::Value` so that `grid-billing` remains rubo4e-free.
    /// The service layer (`netzbilanzd`, `billingd`) can deserialize to
    /// `rubo4e::current::LastvariablePreisposition` for `Rechnungsposition.zusatzAttribute`
    /// embedding, enabling ERP-side validation and portal display of the per-dispatch
    /// tariff breakdown.
    ///
    /// `None` for all other position kinds.
    pub lastvariable_preisposition_json: Option<serde_json::Value>,
    /// Full audit trace for this position.
    ///
    /// Answers "why is this amount here?" and carries all legal references.
    pub trace: CalculationTrace,
}

// ── GridSettlement ────────────────────────────────────────────────────────────

/// Result of a grid settlement calculation — pure domain type, no BO4E coupling.
///
/// This is the canonical output of all calculation functions in `grid-billing`.
/// The service layer (`netzbilanzd`, `invoicd`) converts it to `rubo4e::current::Rechnung`
/// via a local `into_rechnung()` adapter.
///
/// ## Explainability
///
/// Every settlement carries a full [`CalculationTrace`] per position, applied
/// [`LegalReference`]s, and the [`TariffSource`] for each rate. The `warnings`
/// field surfaces any non-blocking validation issues.
///
/// ## Immutable correction chain
///
/// When correcting a prior settlement, set `status = SettlementStatus::Correction`
/// and populate `correction_of` with the original settlement's `rechnungsnummer`.
/// The original settlement is never mutated.
///
/// ## PID override
///
/// `pid` defaults to `SettlementType::default_pid()`. Override after construction:
/// - `31005` for Gas NNE (NneStrom → NneGas)
/// - `31006` for selbstausstellt NNE
/// - `31011` for GeLi Gas AWH Sperrprozesse
#[derive(Debug, Clone)]
pub struct GridSettlement {
    /// BDEW Prüfidentifikator — caller may override after construction.
    pub pid: u32,
    /// Settlement type.
    pub settlement_type: SettlementType,
    /// Lifecycle status.
    pub status: SettlementStatus,
    /// Unique invoice reference number.
    pub rechnungsnummer: String,
    /// If this is a correction, the `rechnungsnummer` of the original settlement.
    pub correction_of: Option<String>,
    /// Invoice issue date.
    pub invoice_date: time::Date,
    /// Payment due date (Zahlungsziel, §271 BGB).
    pub due_date: time::Date,
    /// Start of billing period (inclusive).
    pub period_from: time::Date,
    /// End of billing period (inclusive).
    pub period_to: time::Date,
    /// Sender MP-ID — Netzbetreiber (or MSB for PID 31009).
    pub nb_mp_id: String,
    /// Recipient MP-ID — Lieferant (NNE/MMM), MSB (PID 31009), or MGV (GaBi Gas).
    ///
    /// Maps to `rechnungsempfaenger` in the BO4E `Rechnung` built by the service layer.
    /// Previously omitted, causing service-layer code to pass recipient IDs separately.
    pub counterparty_mp_id: String,
    /// Ordered billing positions (each with full calculation trace).
    pub positions: Vec<InvoicePosition>,
    /// Net total in EUR, rounded to 2 decimal places.
    pub total_eur: Decimal,
    /// Non-blocking validation warnings.
    ///
    /// Empty when the settlement is clean. The service layer should review
    /// `Warning` and `Error` severity items before dispatch.
    pub warnings: Vec<SettlementWarning>,
}

/// Backward-compatible alias so callers using the old name continue to compile.
///
/// `GridInvoice` was the original output type. New code should use [`GridSettlement`]
/// which carries full calculation traces, legal references, and settlement metadata.
pub type GridInvoice = GridSettlement;

impl GridSettlement {
    /// Number of billing positions.
    #[must_use]
    pub fn positions_count(&self) -> usize {
        self.positions.len()
    }

    /// `true` when the settlement has no warnings at `Warning` or `Error` severity.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        !self
            .warnings
            .iter()
            .any(|w| w.severity >= WarningSeverity::Warning)
    }

    /// All legal references cited across all positions (deduplicated by citation string).
    #[must_use]
    pub fn all_legal_refs(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.positions
            .iter()
            .flat_map(|p| p.trace.legal_refs.iter().map(|r| r.citation()))
            .filter(|c| seen.insert(c.clone()))
            .collect()
    }

    /// Net total as computed from positions (re-summed for verification).
    ///
    /// Should equal `total_eur`. A mismatch indicates a calculation bug.
    #[must_use]
    pub fn recomputed_total(&self) -> Decimal {
        self.positions
            .iter()
            .map(|p| p.net_eur)
            .sum::<Decimal>()
            .round_dp(2)
    }
}

// ── Input types ───────────────────────────────────────────────────────────────

/// Input for NNE (Netznutzungsentgelt) invoice calculation.
///
/// Covers:
/// - **PID 31001** — NNE Strom (NB → LF, monthly network usage billing)
/// - **PID 31005** — NNE Gas (NB → LF, monthly gas network usage billing)
///
/// For **RLM** (Leistungsmessung) meters:
/// - Set `spitzenleistung_kw` to the peak demand in kW.
/// - Set `leistungspreis_eur_per_kw` to the published tariff.
///
/// For **SLP** meters:
/// - Leave both fields as `None` (Arbeitspreisanteil only).
///
/// For **§14a Modul 2 time-variable NNE** (BNetzA BK6-22-300):
/// - Set `arbeitsmenge_ht_kwh` + `arbeitspreis_ht_ct_per_kwh` for Hochlast periods.
/// - Set `arbeitsmenge_nt_kwh` + `arbeitspreis_nt_ct_per_kwh` for Niedertarif periods.
/// - Leave `arbeitsmenge_kwh` / `arbeitspreis_ct_per_kwh` as the base fallback.
///
/// For Gas:
/// - The `arbeitsmenge_kwh` should already be converted from m³ using
///   `brennwert × zustandszahl` before being supplied here.
///   (`mako-edm` `MeterBillingPeriod.arbeitsmenge_kwh` carries this converted value.)
#[derive(Debug, Clone)]
pub struct NneInput {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Invoice sender — Netzbetreiber or Gasnetzbetreiber MP-ID.
    pub nb_mp_id: String,
    /// Invoice recipient — Lieferant MP-ID.
    pub lf_mp_id: String,
    /// Unique invoice number (operator-generated).
    pub rechnungsnummer: String,
    /// Start of billing period (inclusive, German local date).
    pub period_from: time::Date,
    /// End of billing period (inclusive, German local date).
    pub period_to: time::Date,
    /// Invoice issue date.
    pub invoice_date: time::Date,
    /// Payment due date (Zahlungsziel).
    pub due_date: time::Date,
    /// Total energy consumption in kWh for the billing period.
    ///
    /// For Gas: already converted from m³ (brennwert × zustandszahl × volume).
    /// Used when HT/NT split is not available (SLP, Gas, or pre-§14a deployments).
    pub arbeitsmenge_kwh: Decimal,
    /// Published NNE Arbeitspreis in **ct/kWh** (from `PreisblattNetznutzung`).
    /// Used as the single Arbeit rate when HT/NT split is absent.
    pub arbeitspreis_ct_per_kwh: Decimal,

    // ── §14a Modul 2 time-variable (ToU) NNE ─────────────────────────────────
    // BNetzA BK6-22-300: mandatory for all controllable loads since 01.01.2024.
    // When both fields below are non-None, the billing engine generates two
    // separate Arbeit positions (HT + NT) instead of a single blended position.
    // Source: `edmd` MeterBillingPeriod.arbeitsmenge_ht_kwh / .arbeitsmenge_nt_kwh.
    /// Hochlast (HT) consumption in kWh — §14a Modul 2 periods (higher-price band).
    /// `None` when ToU metering is not configured for this MaLo.
    pub arbeitsmenge_ht_kwh: Option<Decimal>,
    /// HT Arbeitspreis in ct/kWh (from `PreisblattNetznutzung.zeitvariablePreispositionen`).
    /// Required when `arbeitsmenge_ht_kwh` is set.
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    /// Niedertarif (NT) consumption in kWh — §14a Modul 2 off-peak periods.
    /// `None` when ToU metering is not configured for this MaLo.
    pub arbeitsmenge_nt_kwh: Option<Decimal>,
    /// NT Arbeitspreis in ct/kWh (from `PreisblattNetznutzung.zeitvariablePreispositionen`).
    /// Required when `arbeitsmenge_nt_kwh` is set.
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,

    // ── RLM demand charge ─────────────────────────────────────────────────────
    /// Peak demand in **kW** (`spitzenleistung_kw` from `MeterBillingPeriod`).
    ///
    /// `None` for SLP meters and Gas MaLos.
    pub spitzenleistung_kw: Option<Decimal>,
    /// Published NNE Leistungspreis in **EUR/kW** (from `PreisblattNetznutzung`).
    ///
    /// `None` when `spitzenleistung_kw` is `None`.
    pub leistungspreis_eur_per_kw: Option<Decimal>,
    /// Published Konzessionsabgabe rate in **ct/kWh** (from `PreisblattKonzessionsabgabe`).
    ///
    /// `None` when KA does not apply (Gas or exempt customer class).
    pub ka_satz_ct_per_kwh: Option<Decimal>,

    // ── §14a Modul 1 flat reduction ───────────────────────────────────────────
    /// §14a Modul 1 (BNetzA BK6-22-300): rate multiplier applied to the Arbeit
    /// positions (and Leistung if applicable) before billing.
    ///
    /// This is the fraction of the full rate that the customer pays — a reduction
    /// factor of `0.85` means the customer pays 85 % of the published tariff
    /// (15 % reduction). The regulatory default per BK6-22-300 Anlage 2 is 0.85;
    /// the NB may publish a different approved value in the `PreisblattNetznutzung`.
    ///
    /// `None` when §14a Modul 1 does not apply to this MaLo. Must not be set
    /// together with ToU HT/NT fields (Modul 2) — the validator rejects the
    /// combination.
    ///
    /// # Example
    ///
    /// Full rate: 3.5 ct/kWh, reduction factor: 0.85 → billed rate: 2.975 ct/kWh
    pub sect14a_modul1_reduction_factor: Option<Decimal>,

    // ── Gas NNE monthly base fee ──────────────────────────────────────────────
    /// Gas NNE monthly base fee (Grundpreis / Verrechnungspreis) in **EUR/month**.
    ///
    /// Per GasNEV, gas network charges may include a fixed monthly standing charge
    /// in addition to the commodity-linked Arbeitspreis.  Supply the approved
    /// `PreisblattNetznutzung.grundpreis_eur_per_month` value here.
    ///
    /// `None` for Strom NNE (Strom does not have a separate Grundpreis).
    /// `None` when the Gas tariff is purely commodity-based (no Grundpreis).
    pub nne_grundpreis_eur_per_month: Option<Decimal>,
    /// Number of months to apply the `nne_grundpreis_eur_per_month` to.
    ///
    /// Defaults to `None` (= 0 months, no Grundpreis position generated).
    /// Required when `nne_grundpreis_eur_per_month` is set.
    pub nne_grundpreis_months: Option<u32>,

    /// Optional tariff sheet identifier for audit tracing.
    ///
    /// When set, each position's `trace.tariff_source` references this sheet.
    pub tariff_sheet_id: Option<String>,
    /// Commodity — drives legal references (StromNEV vs GasNEV) and `SettlementType`.
    ///
    /// - `Sparte::Strom` (default) → `StromNEV §21` Arbeit, `StromNEV §17` Leistung,
    ///   `SettlementType::NneStrom`
    /// - `Sparte::Gas` → `GasNEV §14`, `SettlementType::NneGas`
    pub sparte: Sparte,
    /// KAV rate class applied to this metering point.
    ///
    /// Included in the calculation trace for KA positions so auditors can verify
    /// the rate matches the correct KAV §2 tier. `None` when KA is absent.
    pub ka_klasse: Option<KaKlasse>,

    // ── §14a Modul 3 Spotpreis-NNE per-interval dispatch data ────────────────
    /// §14a Modul 3 (BNetzA BK6-22-300 Anlage 2 §3) per-dispatch-interval positions.
    ///
    /// Each entry represents one 15-min interval during which a spot-price-linked
    /// NNE rate applies. The caller fetches the EPEX Spot day-ahead price for each
    /// interval and applies the formula from `PreisblattNetznutzung.lastvariablePreispositionen`
    /// to derive `nne_rate_ct_per_kwh`. `grid-billing` receives pre-calculated rates —
    /// it never queries EPEX directly.
    ///
    /// **Empty (default)** when §14a Modul 3 does not apply to this MaLo.
    ///
    /// **Cannot be combined with `sect14a_modul1_reduction_factor`** — the validator
    /// returns `InvalidInput` when both are set.
    ///
    /// Each interval generates one `InvoicePosition` with
    /// `kind = NneArbeitModul3` and `lastvariable_preisposition_json` populated.
    #[doc = "§14a Modul 3 per-interval input data."]
    pub sect14a_modul3_intervals: Vec<Sect14aModul3Interval>,
}

// ── Sect14aModul3Interval ─────────────────────────────────────────────────────

/// One controlled dispatch interval for §14a Modul 3 (Spotpreis-Netzentgelt).
///
/// Each interval represents a 15-min period during which the DSO exercised load
/// control and the NNE rate is derived from the day-ahead spot price via the
/// formula published in `PreisblattNetznutzung.lastvariablePreispositionen`.
///
/// ## Calculation
///
/// `Einsatzkosten = menge_kwh × nne_rate_ct_per_kwh / 100`
///
/// The NB computes one `InvoicePosition` per interval, allowing the LF (and their
/// customers) to see the exact tariff breakdown for each dispatch event.
///
/// ## Caller responsibility
///
/// The caller (service layer) must:
/// 1. Fetch the EPEX Spot day-ahead price for each 15-min interval from `tarifbd`
///    or the `PreisblattNetznutzung` formula.
/// 2. Apply the formula from `lastvariablePreispositionen` to derive `nne_rate_ct_per_kwh`.
/// 3. Fetch `menge_kwh` from `edmd Lastgang` for the interval.
///
/// `grid-billing` receives pre-calculated rates — it does NOT query EPEX or `edmd`.
///
/// ## Regulatory basis
///
/// BNetzA BK6-22-300 Anlage 2 §3 — Modul 3: Spotpreis-Netzentgelt.
/// The NNE varies per 15-min interval based on the spot market price.
/// All controllable loads ≥ 3.7 kW registered under §14a must have Modul 1 at minimum;
/// Modul 3 is the opt-in premium variant (lower NNE when spot prices are low).
#[derive(Debug, Clone)]
pub struct Sect14aModul3Interval {
    /// UTC start of this controlled dispatch interval (ISO-8601).
    ///
    /// Typically the start of a 15-min settlement slot.
    pub period_from: time::OffsetDateTime,
    /// UTC end of this controlled dispatch interval (ISO-8601).
    ///
    /// Typically `period_from + 15 min`.
    pub period_to: time::OffsetDateTime,
    /// Energy consumption (or reduction) during this interval in kWh.
    ///
    /// Sourced from `edmd Lastgang` for the MaLo during the interval window.
    pub menge_kwh: Decimal,
    /// Effective NNE rate in **ct/kWh** for this interval.
    ///
    /// Derived from the `LastvariablePreisposition` formula applied to the
    /// applicable EPEX Spot day-ahead price. Pre-calculated by the caller.
    pub nne_rate_ct_per_kwh: Decimal,
    /// EPEX Spot day-ahead price in ct/kWh used to derive `nne_rate_ct_per_kwh`.
    ///
    /// Stored in the `CalculationTrace.explanation` for audit transparency.
    /// `None` when the rate was determined by a fixed formula without market reference.
    pub epex_spot_ct_per_kwh: Option<Decimal>,
}

// ── MmmInput ──────────────────────────────────────────────────────────────────

/// Input for Mehr-/Mindermengen (MMM) settlement invoice calculation.
///
/// Covers:
/// - **PID 31002** — `MMM-Stornorechnung NNE Strom` used for Mehr-/Mindermengen
///   settlement between NB and LF.
///
/// Mehr-/Mindermengen settle the difference between the LF's forecast profile
/// (SLP standard load profile) and the actual measured consumption.
///
/// - **Mehrmengen** (positive deviation): actual > profil → LF owes NB
/// - **Mindermengen** (negative deviation): actual < profil → NB owes LF
///
/// The settlement amount is the algebraic sum of both positions.  It can be
/// negative (i.e. a credit note from NB to LF) when Mindermengen dominate.
#[derive(Debug, Clone)]
pub struct MmmInput {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Invoice sender — Netzbetreiber MP-ID.
    pub nb_mp_id: String,
    /// Invoice recipient — Lieferant MP-ID.
    pub lf_mp_id: String,
    /// Unique invoice number.
    pub rechnungsnummer: String,
    /// Start of billing period.
    pub period_from: time::Date,
    /// End of billing period.
    pub period_to: time::Date,
    /// Invoice issue date.
    pub invoice_date: time::Date,
    /// Payment due date.
    pub due_date: time::Date,
    /// Commodity — determines legal references (StromNZV vs GasNZV).
    ///
    /// - `Sparte::Strom` → `StromNZV §15`, `GPKE BK6-22-024`
    /// - `Sparte::Gas` → `GasNZV §14`, `GeLi Gas BK7-24-01-009`
    pub sparte: Sparte,
    /// Actual measured consumption in kWh (from MSCONS / `MeterBillingPeriod`).
    pub actual_kwh: Decimal,
    /// Standard load profile (SLP) forecast consumption in kWh.
    pub profil_kwh: Decimal,
    /// Mehrmengen price in **ct/kWh** (from `PreisblattNetznutzung` MMM position).
    pub mehr_preis_ct_per_kwh: Decimal,
    /// Mindermengen price in **ct/kWh** (from `PreisblattNetznutzung` MMM position).
    pub minder_preis_ct_per_kwh: Decimal,
}

// ── MsbInput ──────────────────────────────────────────────────────────────────

/// Input for MSB (Messstellenbetreiber) invoice calculation.
///
/// Covers:
/// - **PID 31009** — MSB-Rechnung (NB → MSB, monthly metering service settlement)
///
/// The NB bills the MSB for the metering service period.  Positions:
/// 1. Grundgebühr Messstellenbetrieb — flat monthly base fee × billing months.
/// 2. Messdienstleistung — optional per-period measurement service fee.
#[derive(Debug, Clone)]
pub struct MsbInput {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Invoice sender — Netzbetreiber MP-ID.
    pub nb_mp_id: String,
    /// Invoice recipient — Messstellenbetreiber MP-ID.
    pub msb_mp_id: String,
    /// Unique invoice number.
    pub rechnungsnummer: String,
    /// Start of billing period (inclusive, German local date).
    pub period_from: time::Date,
    /// End of billing period (inclusive, German local date).
    pub period_to: time::Date,
    /// Invoice issue date.
    pub invoice_date: time::Date,
    /// Payment due date.
    pub due_date: time::Date,
    /// Grundgebühr Messstellenbetrieb in **EUR/month** (from `PreisblattMessung`).
    pub grundgebuehr_eur_per_month: Decimal,
    /// Number of full calendar months in the billing period.
    pub billing_months: u32,
    /// Optional Messdienstleistung flat fee in **EUR** for the full period.
    ///
    /// `None` when the MSB provides only the meter, not a separate measurement service.
    pub messdienstleistung_eur: Option<Decimal>,
}

// ── GasAwhInput ───────────────────────────────────────────────────────────────

/// Input for GeLi Gas AWH Sperrprozesse settlement (PID 31011).
///
/// **PID 31011 — Rechnung sonstige Leistung (NB → LF)**
///
/// Bills the Lieferant (LFG/LFA) for abrechnungswürdige Handlungen (AWH)
/// performed by the GNB/VNB during the Sperrung/Entsperrung process.
/// Governed by BK7-24-01-009 §5.4 (GeLi Gas 3.0).
///
/// ## What counts as AWH
///
/// AWH are chargeable actions not included in the network tariff, triggered by
/// the LF through the Sperrung process. Typical AWH:
/// - `Sperrung` (disconnection)
/// - `Entsperrung` (reconnection)
/// - `Teilsperrung` (partial disconnection)
/// - `Unterbrechung Verfahren` (process interruption)
///
/// Each action type has a fixed price published in the `PreisblattNetznutzung`.
#[derive(Debug, Clone)]
pub struct GasAwhInput {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Invoice sender — Gasnetzbetreiber (GNB/VNB) MP-ID.
    pub nb_mp_id: String,
    /// Invoice recipient — Lieferant Gas (LFG or LFA) MP-ID.
    pub lf_mp_id: String,
    /// Unique invoice number (operator-generated).
    pub rechnungsnummer: String,
    /// Start of billing period (inclusive, German local date).
    pub period_from: time::Date,
    /// End of billing period (inclusive, German local date).
    pub period_to: time::Date,
    /// Invoice issue date.
    pub invoice_date: time::Date,
    /// Payment due date (Zahlungsziel, §271 BGB).
    pub due_date: time::Date,
    /// Optional tariff sheet identifier for audit tracing.
    pub tariff_sheet_id: Option<String>,
    /// AWH line items: each chargeable action with count and unit price.
    ///
    /// At least one position is required.
    pub awh_positionen: Vec<AwhPositionInput>,
}

/// One AWH action line item for [`GasAwhInput`].
///
/// ## Examples
///
/// ```rust
/// # use grid_billing::AwhPositionInput;
/// # use rust_decimal_macros::dec;
/// let sperrung = AwhPositionInput {
///     beschreibung: "Sperrung Gaszähler".to_owned(),
///     anzahl: 1,
///     preis_eur: dec!(45.00),
///     artikel_id: Some("2-01-7-001".to_owned()),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct AwhPositionInput {
    /// Human-readable action description, e.g. `"Sperrung Gaszähler"`.
    pub beschreibung: String,
    /// Number of executions of this action.
    pub anzahl: u32,
    /// Price per execution in **EUR** (from `PreisblattNetznutzung`).
    pub preis_eur: Decimal,
    /// BDEW Artikel-ID from section 3.2 of the Codeliste Artikelnummern v5.6.
    ///
    /// Standard values for Gas AWH Sperrprozesse (BK7-24-01-009 §5.4):
    /// - `"2-01-7-001"` — Unterbrechung der Anschlussnutzung (reguläre AZ)
    /// - `"2-01-7-002"` — Wiederherstellung der Anschlussnutzung (reguläre AZ)
    /// - `"2-01-7-003"` — Erfolglose Unterbrechung
    /// - `"2-01-7-004"` — Stornierung Unterbrechungsauftrag (bis Vortag)
    /// - `"2-01-7-005"` — Stornierung Unterbrechungsauftrag (am Sperrtag)
    /// - `"2-01-7-006"` — Wiederherstellung außerhalb regulärer AZ
    ///
    /// `None` for custom / non-standard AWH positions.
    pub artikel_id: Option<String>,
}

// ── ValidationResult ─────────────────────────────────────────────────────────

/// Result of pre-calculation input validation.
///
/// Validation runs **before** the calculation begins. A `ValidationResult`
/// with `is_valid = false` should prevent calling `calculate_*` to avoid
/// partial or incorrect results.
///
/// Use [`validate_nne_input`], [`validate_mmm_input`], or [`validate_msb_input`]
/// to obtain a `ValidationResult` for your input.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the input passed all validation checks.
    pub is_valid: bool,
    /// All warnings and errors found. May contain [`WarningSeverity::Info`] items
    /// even when `is_valid = true`.
    pub warnings: Vec<SettlementWarning>,
}

impl ValidationResult {
    /// Returns a clean (valid, no warnings) result.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            is_valid: true,
            warnings: Vec::new(),
        }
    }

    /// Appends a warning. `WarningSeverity::Error` marks the result invalid.
    pub fn push(&mut self, w: SettlementWarning) {
        if w.severity == WarningSeverity::Error {
            self.is_valid = false;
        }
        self.warnings.push(w);
    }
}

/// Validate a [`NneInput`] before calling [`crate::calculate_nne_invoice`].
///
/// The calculation functions also validate hard constraints and return
/// `Err(BillingError)`. This function additionally surfaces soft warnings
/// (e.g. suspiciously negative prices) that would not prevent calculation
/// but should be reviewed before dispatch.
#[must_use]
pub fn validate_nne_input(input: &NneInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period_from >= input.period_to {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "INVALID_PERIOD",
            message: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.arbeitsmenge_kwh < Decimal::ZERO {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "NEGATIVE_CONSUMPTION",
            message: format!("arbeitsmenge_kwh is negative: {}", input.arbeitsmenge_kwh),
        });
    }
    if input.arbeitspreis_ct_per_kwh < Decimal::ZERO {
        r.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "NEGATIVE_ARBEITSPREIS",
            message: format!(
                "arbeitspreis_ct_per_kwh is negative: {}",
                input.arbeitspreis_ct_per_kwh
            ),
        });
    }
    if input.spitzenleistung_kw.is_some() != input.leistungspreis_eur_per_kw.is_some() {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "MISMATCHED_RLM_FIELDS",
            message:
                "spitzenleistung_kw and leistungspreis_eur_per_kw must both be set or both absent"
                    .to_owned(),
        });
    }
    if input.sparte == Sparte::Gas && input.spitzenleistung_kw.is_some() {
        r.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "GAS_WITH_LEISTUNG",
            message: "Gas NNE typically does not use Leistungspreis — verify tariff configuration"
                .to_owned(),
        });
    }
    // Partial HT/NT field consistency: all four must be set or all four absent.
    let tou_count = [
        input.arbeitsmenge_ht_kwh.is_some(),
        input.arbeitspreis_ht_ct_per_kwh.is_some(),
        input.arbeitsmenge_nt_kwh.is_some(),
        input.arbeitspreis_nt_ct_per_kwh.is_some(),
    ]
    .iter()
    .filter(|&&b| b)
    .count();
    if tou_count > 0 && tou_count < 4 {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "PARTIAL_TOU_FIELDS",
            message: format!(
                "§14a Modul 2 ToU: {tou_count} of 4 HT/NT fields set — all four must be provided or all absent"
            ),
        });
    }
    // §14a Modul 1 and Modul 2 are mutually exclusive.
    if input.sect14a_modul1_reduction_factor.is_some() && tou_count == 4 {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "MODUL1_AND_MODUL2_CONFLICT",
            message:
                "sect14a_modul1_reduction_factor (Modul 1) cannot be combined with HT/NT ToU fields (Modul 2)"
                    .to_owned(),
        });
    }
    // §14a Modul 1 and Modul 3 are mutually exclusive.
    if input.sect14a_modul1_reduction_factor.is_some() && !input.sect14a_modul3_intervals.is_empty()
    {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "MODUL1_AND_MODUL3_CONFLICT",
            message:
                "sect14a_modul1_reduction_factor (Modul 1) cannot be combined with sect14a_modul3_intervals (Modul 3)"
                    .to_owned(),
        });
    }
    // §14a Modul 3 interval sanity checks.
    for (i, interval) in input.sect14a_modul3_intervals.iter().enumerate() {
        if interval.period_from >= interval.period_to {
            r.push(SettlementWarning {
                severity: WarningSeverity::Error,
                code: "MODUL3_INTERVAL_PERIOD_INVALID",
                message: format!(
                    "sect14a_modul3_intervals[{i}]: period_from must be before period_to"
                ),
            });
        }
        if interval.menge_kwh < Decimal::ZERO {
            r.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "MODUL3_INTERVAL_NEGATIVE_MENGE",
                message: format!(
                    "sect14a_modul3_intervals[{i}]: menge_kwh is negative ({}) — verify Lastgang data",
                    interval.menge_kwh
                ),
            });
        }
        if interval.nne_rate_ct_per_kwh < Decimal::ZERO {
            r.push(SettlementWarning {
                severity: WarningSeverity::Warning,
                code: "MODUL3_INTERVAL_NEGATIVE_RATE",
                message: format!(
                    "sect14a_modul3_intervals[{i}]: nne_rate_ct_per_kwh is negative ({}) — \
                     Modul 3 rates should be ≥ 0; verify spot formula",
                    interval.nne_rate_ct_per_kwh
                ),
            });
        }
    }
    if let Some(factor) = input.sect14a_modul1_reduction_factor
        && (factor <= Decimal::ZERO || factor > Decimal::ONE)
    {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "INVALID_MODUL1_FACTOR",
            message: format!("sect14a_modul1_reduction_factor must be in (0, 1], got {factor}"),
        });
    }
    // Gas Grundpreis: both fields must be set together.
    if input.nne_grundpreis_eur_per_month.is_some() != input.nne_grundpreis_months.is_some() {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "GRUNDPREIS_MONTHS_MISMATCH",
            message:
                "nne_grundpreis_eur_per_month and nne_grundpreis_months must both be set or both absent"
                    .to_owned(),
        });
    }
    r
}

/// Validate a [`MmmInput`] before calling [`crate::calculate_mmm_invoice`].
#[must_use]
pub fn validate_mmm_input(input: &MmmInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period_from >= input.period_to {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "INVALID_PERIOD",
            message: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.mehr_preis_ct_per_kwh < Decimal::ZERO {
        r.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "NEGATIVE_MEHR_PREIS",
            message: format!(
                "mehr_preis_ct_per_kwh is negative: {}",
                input.mehr_preis_ct_per_kwh
            ),
        });
    }
    if input.minder_preis_ct_per_kwh < Decimal::ZERO {
        r.push(SettlementWarning {
            severity: WarningSeverity::Warning,
            code: "NEGATIVE_MINDER_PREIS",
            message: format!(
                "minder_preis_ct_per_kwh is negative: {}",
                input.minder_preis_ct_per_kwh
            ),
        });
    }
    r
}

/// Validate a [`MsbInput`] before calling [`crate::calculate_msb_invoice`].
#[must_use]
pub fn validate_msb_input(input: &MsbInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period_from >= input.period_to {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "INVALID_PERIOD",
            message: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.grundgebuehr_eur_per_month < Decimal::ZERO {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "NEGATIVE_GRUNDGEBUEHR",
            message: format!(
                "grundgebuehr_eur_per_month is negative: {}",
                input.grundgebuehr_eur_per_month
            ),
        });
    }
    if input.billing_months == 0 {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "ZERO_BILLING_MONTHS",
            message: "billing_months must be at least 1".to_owned(),
        });
    }
    r
}

/// Validate a [`GasAwhInput`] before calling [`crate::calculate_gas_awh_invoice`].
///
/// Checks that:
/// - `period_from < period_to`
/// - `awh_positionen` is non-empty
/// - All positions have `anzahl ≥ 1` and `preis_eur ≥ 0`
#[must_use]
pub fn validate_gas_awh_input(input: &GasAwhInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period_from >= input.period_to {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "INVALID_PERIOD",
            message: "period_from must be strictly before period_to".to_owned(),
        });
    }
    if input.awh_positionen.is_empty() {
        r.push(SettlementWarning {
            severity: WarningSeverity::Error,
            code: "EMPTY_AWH_POSITIONEN",
            message: "awh_positionen must contain at least one position".to_owned(),
        });
    }
    for (i, awh) in input.awh_positionen.iter().enumerate() {
        if awh.anzahl == 0 {
            r.push(SettlementWarning {
                severity: WarningSeverity::Error,
                code: "ZERO_AWH_ANZAHL",
                message: format!("awh_positionen[{i}].anzahl must be ≥ 1"),
            });
        }
        if awh.preis_eur < Decimal::ZERO {
            r.push(SettlementWarning {
                severity: WarningSeverity::Error,
                code: "NEGATIVE_AWH_PREIS",
                message: format!(
                    "awh_positionen[{i}].preis_eur must be non-negative, got {}",
                    awh.preis_eur
                ),
            });
        }
    }
    r
}
