//! Input/output types for the settlement calculation engine.
//!
//! ## Architecture
//!
//! The preferred calculation flow is:
//!
//! ```text
//! Input → Validation → Settlement Engine → SettlementResult → InvoiceDocument → BO4E → EDIFACT
//! ```
//!
//! [`SettlementResult`] is the canonical output. It carries every position
//! alongside its [`CalculationTrace`], applicable [`LegalReference`]s, the
//! [`TariffSource`] that justified each rate, and any [`SettlementWarning`]s.
//!
//! The service layer (`netzbilanzd`, `invoicd`) adapts `SettlementResult` into
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
/// - `Strom` → `StromNEV`, BK6 Festlegungen
/// - `Gas` → `GasNEV`, BK7 Festlegungen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum Sparte {
    /// Electricity (Strom). Default.
    #[default]
    Strom,
    /// Natural gas (Gas).
    Gas,
}

// ── Konzessionsabgabe (KAV §2) ────────────────────────────────────────────────

/// Municipality size band for Konzessionsabgabe, per **KAV §2 Abs. 2**.
///
/// KAV bands Tarifkunden rates by the municipality's **inhabitant count**, not by
/// the customer's annual consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum GemeindeGroesse {
    /// bis 25 000 Einwohner.
    Bis25k,
    /// bis 100 000 Einwohner.
    Bis100k,
    /// bis 500 000 Einwohner.
    Bis500k,
    /// über 500 000 Einwohner.
    Ueber500k,
}

/// Konzessionsabgabe customer group per **KAV §2**.
///
/// The Tarifkunde/Sondervertragskunde split is a **contract-type** test, not a
/// consumption threshold: KAV §2 Abs. 3 applies to Sondervertragskunden whatever
/// they consume, and Abs. 2 bands Tarifkunden by municipality size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum KaKundengruppe {
    /// Tarifkunde — KAV §2 Abs. 2. Rate depends on [`GemeindeGroesse`].
    ///
    /// For gas, `nur_kochen_warmwasser` selects between the two Abs. 2 columns:
    /// supply limited to cooking and hot water, or all other Tariflieferungen.
    Tarifkunde {
        /// Municipality size band.
        gemeinde: GemeindeGroesse,
        /// Gas only: supply limited to cooking/hot water. Ignored for Strom.
        nur_kochen_warmwasser: bool,
    },
    /// Schwachlaststrom — KAV §2 Abs. 2. **Strom only**; gas has no such tier.
    Schwachlast,
    /// Sondervertragskunde — KAV §2 Abs. 3. Flat, independent of municipality size.
    Sondervertragskunde,
    /// Freigestellt nach KAV §2 Abs. 7.
    Exempt,
}

impl KaKundengruppe {
    /// The KAV §2 **Höchstbetrag** in ct/kWh for this group and Sparte.
    ///
    /// Returns `None` for [`KaKundengruppe::Exempt`], and for
    /// [`KaKundengruppe::Schwachlast`] on gas, which KAV does not provide.
    ///
    /// These are statutory **maxima**, not the agreed rate — a concession contract
    /// may set anything up to them.
    #[must_use]
    pub fn hoechstsatz_ct_per_kwh(self, sparte: Sparte) -> Option<Decimal> {
        let pick = |a: &str| Decimal::from_str_exact(a).ok();
        match (self, sparte) {
            (Self::Exempt, _) => None,
            (Self::Schwachlast, Sparte::Strom) => pick("0.61"),
            (Self::Schwachlast, Sparte::Gas) => None,
            (Self::Sondervertragskunde, Sparte::Strom) => pick("0.11"),
            (Self::Sondervertragskunde, Sparte::Gas) => pick("0.03"),
            (Self::Tarifkunde { gemeinde, .. }, Sparte::Strom) => pick(match gemeinde {
                GemeindeGroesse::Bis25k => "1.32",
                GemeindeGroesse::Bis100k => "1.59",
                GemeindeGroesse::Bis500k => "1.99",
                GemeindeGroesse::Ueber500k => "2.39",
            }),
            (
                Self::Tarifkunde {
                    gemeinde,
                    nur_kochen_warmwasser: true,
                },
                Sparte::Gas,
            ) => pick(match gemeinde {
                GemeindeGroesse::Bis25k => "0.51",
                GemeindeGroesse::Bis100k => "0.61",
                GemeindeGroesse::Bis500k => "0.77",
                GemeindeGroesse::Ueber500k => "0.93",
            }),
            (
                Self::Tarifkunde {
                    gemeinde,
                    nur_kochen_warmwasser: false,
                },
                Sparte::Gas,
            ) => pick(match gemeinde {
                GemeindeGroesse::Bis25k => "0.22",
                GemeindeGroesse::Bis100k => "0.27",
                GemeindeGroesse::Bis500k => "0.33",
                GemeindeGroesse::Ueber500k => "0.40",
            }),
        }
    }

    /// Short label for the invoice position text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tarifkunde { .. } => "KAV §2 Abs. 2 Tarifkunde",
            Self::Schwachlast => "KAV §2 Abs. 2 Schwachlast",
            Self::Sondervertragskunde => "KAV §2 Abs. 3 Sondervertragskunde",
            Self::Exempt => "KAV §2 Abs. 7 — freigestellt",
        }
    }

    /// The KAV paragraph that fixes this group's Höchstbetrag.
    ///
    /// Cited on the position, so the invoice states the rule it was actually
    /// billed under. Every position used to cite §2 Abs. 2 regardless — wrong
    /// for a Sondervertragskunde, whose ceiling is Abs. 3, and wrong again for a
    /// customer freigestellt under Abs. 7.
    #[must_use]
    pub const fn kav_paragraph(self) -> &'static str {
        match self {
            Self::Tarifkunde { .. } | Self::Schwachlast => "§2 Abs. 2",
            Self::Sondervertragskunde => "§2 Abs. 3",
            Self::Exempt => "§2 Abs. 7",
        }
    }
}

// ── QuantityUnit ──────────────────────────────────────────────────────────────

/// Unit of measure for a settlement position quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SettlementType {
    /// Netznutzungsentgelt (NNE) Strom — PID 31001 (NB → LF).
    NneStrom,
    /// Netznutzungsentgelt (NNE) Gas — PID 31005 (NB → LF, GasNEV).
    NneGas,
    /// NNE selbst ausgestellt (NB + LF = same entity) — PID 31006.
    NneSelbstausstellt,
    /// Mehr-/Mindermengen settlement Strom — PID 31002 (NB → LF, GPKE (BK6-24-174) Teil 1 Kap. 8.4).
    MmmStrom,
    /// Mehr-/Mindermengen settlement Gas — PID 31002 (NB → LF, GaBi Gas 2.1 (BK7-24-01-008)).
    ///
    /// Gas MMM settlement uses different legal references from Strom MMM:
    /// `GaBi Gas 2.1 (BK7-24-01-008)` and `GeLi Gas 3.0 (BK7-24-01-009)`. Using a separate variant
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
    /// Entgelt für dezentrale Erzeugung — §18 StromNEV, NB → Anlagenbetreiber.
    ///
    /// A bilateral payment relationship, not an EDIFACT market process: it has
    /// no Prüfidentifikator and is rendered as an ordinary commercial credit.
    DezentraleEinspeisung,
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
            // Bilateral NB → Anlagenbetreiber payment; not an EDIFACT process.
            Self::DezentraleEinspeisung => 0,
        }
    }
}

// ── SettlementStatus ──────────────────────────────────────────────────────────

/// Lifecycle status of a settlement result.
///
/// Settlements are never destroyed — every correction or cancellation creates
/// a new result that references the original. This ensures an immutable audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
/// Every [`SettlementPosition`] should carry at least one `LegalReference`.
/// This enables full auditability: any operator or regulator can trace
/// exactly which paragraph, ruling, and version authorised each charge.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    /// KWKG — Kraft-Wärme-Kopplungsgesetz.
    ///
    /// Example: `Kwkg { paragraph: "§26" }` for the KWKG-Umlage.
    Kwkg {
        /// Paragraph citation, e.g. `"§26"`.
        paragraph: &'static str,
    },
    /// EnFG — Energiefinanzierungsgesetz.
    ///
    /// Governs which Letztverbrauchergruppe an Entnahmestelle falls into and so
    /// which rate of a network levy applies.
    EnFG {
        /// Paragraph citation, e.g. `"§§21 ff."`.
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
    /// StromNZV — Stromnetzzugangsverordnung.
    ///
    /// **Außer Kraft mit Ablauf des 31.12.2025** (Art. 15 Abs. 4 des Gesetzes
    /// v. 22.12.2023, BGBl. 2023 I Nr. 405). Valid only for Lieferzeiträume up
    /// to that date; the successor competence is §20 Abs. 3 EnWG, exercised
    /// through the BK6 Festlegungen. [`LegalReference::citation`] appends the
    /// expiry so an archived invoice stays self-explanatory.
    StromNzv {
        /// Paragraph citation, e.g. `"§13 Abs. 3"`.
        paragraph: &'static str,
    },
    /// GasNZV — Gasnetzzugangsverordnung 2010.
    ///
    /// **Außer Kraft mit Ablauf des 31.12.2025** (Art. 15 Abs. 6 des Gesetzes
    /// v. 22.12.2023, BGBl. 2023 I Nr. 405). Succeeded by KARLA Gas 2.0
    /// (BK7-24-01-007), GaBi Gas 2.1 (BK7-24-01-008), GeLi Gas 3.0
    /// (BK7-24-01-009) and ZuBio (BK7-24-01-010), all in force 01.01.2026.
    GasNzv {
        /// Paragraph citation, e.g. `"§25"`.
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
            Self::Kwkg { paragraph } => format!("KWKG {paragraph}"),
            Self::EnFG { paragraph } => format!("EnFG {paragraph}"),
            Self::Sect14aEnwg { module } => format!("§14a EnWG {}", module.label()),
            Self::MessZv { paragraph } => format!("MessZV {paragraph}"),
            Self::MsbG { paragraph } => format!("MsbG {paragraph}"),
            Self::BnetzaDecision { reference } => format!("BNetzA {reference}"),
            Self::BdewAhb { reference } => format!("BDEW {reference}"),
            Self::StromNzv { paragraph } => {
                format!("StromNZV {paragraph} (außer Kraft seit 01.01.2026)")
            }
            Self::GasNzv { paragraph } => {
                format!("GasNZV {paragraph} (außer Kraft seit 01.01.2026)")
            }
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

/// Full audit record for how one [`SettlementPosition`] was computed.
///
/// Answers the question: *"Why is this amount on the invoice?"*
///
/// Every `CalculationTrace` carries the input values, the applied legal rules,
/// intermediate results, and the tariff source. This enables:
/// - Regulator audits (BNetzA §20 EnWG)
/// - Operator review
/// - LF dispute resolution
/// - AI-assisted invoice explainability (MCP tools)
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettlementWarning {
    /// Severity: informational, warning, or error.
    pub severity: WarningSeverity,
    /// Machine-readable warning code.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
}

/// Severity level for [`SettlementWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
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
/// `into_rechnung()`. Every position in every `SettlementResult` must carry
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    /// PID 31002 GPKE (BK6-24-174) Teil 1 Kap. 8.4 / GaBi Gas 2.1 (BK7-24-01-008). → `BdewArtikelnummer::Mehrmenge`
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
    /// Aufschlag für besondere Netznutzung (§19 StromNEV-Umlage).
    ///
    /// Funds the reduced individual network charges granted under §19 Abs. 2
    /// StromNEV. Rate depends on the Letztverbrauchergruppe (EnFG).
    Sect19StromNevUmlage,
    /// Offshore-Netzumlage (§17f EnWG).
    ///
    /// Funds offshore connection cost and the compensation owed to offshore
    /// wind farms for unavailable connections.
    OffshoreNetzumlage,
    /// KWKG-Umlage (§26 KWKG).
    ///
    /// Funds the KWK-Zuschlag paid to CHP operators.
    KwkgUmlage,
    /// Entgelt für dezentrale Erzeugung — §18 StromNEV, under Abschmelzung
    /// (GBK-25-02-1#1). A payment out, so its `net_eur` is negative.
    DezentraleEinspeisung,
    /// §19 Abs. 2 StromNEV individual-charge reduction over the Netzentgelt.
    /// Negative: it takes the published charge down to the agreed fraction.
    Sect19IndividuellesEntgelt,
    /// Gas Kapazitätsentgelt — booked capacity at the price sheet's annual
    /// rate, pro-rated over the period. §15 GasNEV.
    GasKapazitaetsentgelt,
}

/// One line item in a grid settlement.
///
/// Carries raw numbers for the service layer to map into the required format
/// (BO4E `Rechnungsposition`, EN16931 UBL, etc.).
///
/// Invariant: `net_eur == (quantity × unit_price_eur).round_dp(5)`.
/// The pricing formula behind a §14a Modul 3 spot-priced position.
///
/// Modelled as a value object rather than a serialised BO4E document. The engine
/// states *what the formula was*; translating that into
/// `LastvariablePreisposition` — or into any other representation — is the
/// adapter's job. Carrying BO4E JSON here would put schema knowledge inside the
/// calculation, untyped and unvalidated, which is the coupling the crate exists
/// to avoid.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SpotPriceFormula {
    /// What the price refers to — for Modul 3 always the metered energy.
    pub reference: PriceReference,
    /// The unit the price is expressed per.
    pub unit: QuantityUnit,
    /// How the rate was derived.
    pub method: TariffCalculationMethod,
    /// The rate steps that applied, in order.
    pub steps: Vec<PriceStep>,
}

/// What a price refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PriceReference {
    /// The metered energy quantity.
    Energiemenge,
    /// Contracted or metered capacity.
    Leistung,
}

/// How a rate was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum TariffCalculationMethod {
    /// A published fixed rate.
    Festpreis,
    /// Derived from a spot-market price — §14a Modul 3, BK6-22-300 Anlage 2 §3.
    Spotpreis,
}

/// One step of a rate schedule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PriceStep {
    /// Lower bound of the step, inclusive.
    pub from: Decimal,
    /// Upper bound, exclusive; `None` for the open top step.
    pub to: Option<Decimal>,
    /// The rate in EUR per [`SpotPriceFormula::unit`].
    pub unit_price_eur: Decimal,
}

/// One line of a settlement.
///
/// Carries no position number and no BDEW Artikel-ID: both are properties of the
/// *document* that presents the settlement, not of the calculation. An adapter
/// numbers the positions it renders and resolves article identifiers from the
/// price sheet.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettlementPosition {
    /// Human-readable description.
    pub text: String,
    /// Semantic kind — what was charged, independent of how it is coded.
    pub kind: BillingPositionKind,
    /// Metered or contracted quantity.
    pub quantity: Decimal,
    /// Unit of measure.
    pub unit: QuantityUnit,
    /// Unit price in EUR.
    pub unit_price_eur: Decimal,
    /// Net amount in EUR, rounded to 5 decimal places.
    ///
    /// May be negative for credit positions (Mindermengen, Gutschriften).
    pub net_eur: Decimal,
    /// The formula behind the rate, where one applied.
    pub spot_price_formula: Option<SpotPriceFormula>,
    /// Why this amount is what it is.
    pub trace: CalculationTrace,
}

impl BillingPositionKind {
    /// The BDEW Artikelnummer that codes this position, as its codelist name.
    ///
    /// Which article number applies depends on both what was charged and what
    /// kind of settlement it appears in — Gas NNE keeps the classic `WIRKARBEIT`
    /// code, while Strom NNE moved to Artikel-IDs under BK6-20-160 and carries
    /// no Artikelnummer at all.
    ///
    /// Returned as the codelist *name* rather than a BO4E enum so that this
    /// crate stays free of BO4E types. A consumer parses it into whatever it
    /// renders — `rubo4e::current::BdewArtikelnummer` implements `FromStr` over
    /// exactly these names.
    ///
    /// `None` means the position carries an Artikel-ID instead, resolved from
    /// the price sheet by the renderer.
    ///
    /// Source: BDEW Codeliste der Artikelnummern und Artikel-IDs v5.6.
    #[must_use]
    pub fn artikelnummer(self, settlement_type: SettlementType) -> Option<&'static str> {
        use BillingPositionKind as K;
        use SettlementType as ST;
        match (self, settlement_type) {
            // Gas NNE keeps the classic codes — BK6-20-160 changed Strom only.
            (
                K::NneArbeit
                | K::NneArbeitHt
                | K::NneArbeitNt
                | K::NneArbeitModul1
                | K::NneArbeitModul3,
                ST::NneGas,
            ) => Some("WIRKARBEIT"),
            (K::NneLeistung, ST::NneGas) => Some("LEISTUNG"),
            (K::NneGasGrundpreis, _) => Some("GRUNDPREIS"),
            // Strom NNE: the Artikel-ID replaces the Artikelnummer.
            (
                K::NneArbeit
                | K::NneArbeitHt
                | K::NneArbeitNt
                | K::NneArbeitModul1
                | K::NneArbeitModul3
                | K::NneLeistung,
                _,
            ) => None,
            (K::Konzessionsabgabe, _) => Some("KONZESSIONSABGABE"),
            (K::Mehrmenge, _) => Some("MEHRMENGE"),
            (K::Mindermenge, _) => Some("MINDERMENGE"),
            (K::MsbGrundgebuehr, _) => Some("ENTGELT_EINBAU_BETRIEB_WARTUNG_MESSTECHNIK"),
            (K::Messdienstleistung, _) => Some("ENTGELT_MESSUNG_ABLESUNG"),
            // AWH Gas positions carry a 2-01-7-xxx Artikel-ID from the input.
            (K::GasAwhSperrung | K::GasAwhEntsprrung | K::GasAwhSonstige, _) => None,
            (K::Blindmehrarbeit, _) => Some("BLINDMEHRARBEIT"),
            // Netzseitige Umlagen (EnFG). `OFFSHORE_HAFTUNGSUMLAGE` is the code's
            // legacy name — the levy was renamed Offshore-Netzumlage, the article
            // number was not.
            (K::Sect19StromNevUmlage, _) => Some("PARAGRAF_19_STROM_NEV_UMLAGE"),
            // Bilateral payment outside the INVOIC market processes — the
            // codelist has no article number for it.
            (K::DezentraleEinspeisung, _) => None,
            // A reduction over Strom NNE positions, which carry Artikel-IDs.
            (K::Sect19IndividuellesEntgelt, _) => None,
            // Capacity is the gas Leistung analogue and keeps the classic code.
            (K::GasKapazitaetsentgelt, ST::NneGas) => Some("LEISTUNG"),
            (K::GasKapazitaetsentgelt, _) => None,
            (K::OffshoreNetzumlage, _) => Some("OFFSHORE_HAFTUNGSUMLAGE"),
            (K::KwkgUmlage, _) => Some("ABGABE_KWKG"),
        }
    }
}

// ── Arbeitspreis model ────────────────────────────────────────────────────────

/// A §14a Modul 1 reduction factor — the fraction of the published rate paid.
///
/// A newtype because the range matters: `0.85` is a 15 % reduction, and a value
/// outside `(0, 1]` is not a reduction at all. The unconstrained `Decimal` this
/// replaces was range-checked in the validator and *not* in the engine, so a
/// caller who skipped validation could multiply the tariff by 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Reduktionsfaktor(Decimal);

impl Reduktionsfaktor {
    /// The regulatory default, BNetzA BK6-22-300 Anlage 2 — 85 % of the tariff.
    pub const REGELFALL: Self = Self(rust_decimal::dec!(0.85));

    /// Build a factor.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::BillingError::InvalidInput`] outside `(0, 1]`.
    pub fn new(factor: Decimal) -> Result<Self, crate::error::BillingError> {
        if factor <= Decimal::ZERO || factor > Decimal::ONE {
            return Err(crate::error::BillingError::InvalidInput {
                reason: format!("§14a Modul 1 reduction factor must be in (0, 1], got {factor}"),
            });
        }
        Ok(Self(factor))
    }

    /// The factor as a fraction.
    #[must_use]
    pub const fn get(self) -> Decimal {
        self.0
    }
}

/// A metered quantity priced at a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MengePreis {
    /// Metered energy in kWh.
    pub menge_kwh: Decimal,
    /// Rate in ct/kWh.
    pub preis_ct_per_kwh: Decimal,
}

/// How the Arbeitspreis is structured, and whether §14a applies.
///
/// One enum rather than three independent field groups. The four variants are
/// mutually exclusive **by construction**, which removes a whole class of defect:
///
/// - The four HT/NT fields were 2⁴ states of which two were valid. Setting three
///   of them fell through to flat billing with no error — the invoice looked
///   right and was billed on the wrong basis.
/// - Modul 1 and Modul 3 could both be set. The engine applied the flat
///   reduction *and* the per-interval rates, double-billing the same energy.
/// - Modul 1 and Modul 2 could both be set; the engine silently preferred
///   Modul 2 rather than rejecting the conflict.
///
/// Those were runtime warnings in a validator the engine never called. They are
/// now unrepresentable.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum ArbeitspreisModell {
    /// A single rate for all metered energy.
    Einheitlich(MengePreis),

    /// **§14a Modul 1** — the published rate reduced by a flat factor.
    ///
    /// BNetzA BK6-22-300 Anlage 2.
    Modul1Pauschal {
        /// The metered energy and its published rate, before reduction.
        basis: MengePreis,
        /// The fraction of that rate actually paid.
        reduktion: Reduktionsfaktor,
    },

    /// **§14a Modul 2** — time-variable rates in a Hoch-/Niedertarif split.
    ///
    /// Both bands are required: a Modul 2 tariff has both, and permitting one
    /// would reintroduce the partial state this type exists to prevent.
    Modul2ZeitVariabel {
        /// Hochtarif band.
        ht: MengePreis,
        /// Niedertarif band.
        nt: MengePreis,
    },

    /// **§14a Modul 3** — a spot-derived rate per dispatch interval.
    ///
    /// BNetzA BK6-22-300 Anlage 2 §3. The rates arrive already derived; this
    /// crate never queries a spot market.
    Modul3Spotpreis {
        /// The dispatch intervals, each with its own rate.
        intervalle: Vec<Sect14aModul3Interval>,
    },
}

impl ArbeitspreisModell {
    /// Total metered energy across the model, in kWh.
    ///
    /// This is the base the Konzessionsabgabe and the network levies are charged
    /// on, so it is derived here once rather than recomputed per levy.
    #[must_use]
    pub fn menge_kwh(&self) -> Decimal {
        match self {
            Self::Einheitlich(mp) | Self::Modul1Pauschal { basis: mp, .. } => mp.menge_kwh,
            Self::Modul2ZeitVariabel { ht, nt } => ht.menge_kwh + nt.menge_kwh,
            Self::Modul3Spotpreis { intervalle } => intervalle.iter().map(|i| i.menge_kwh).sum(),
        }
    }

    /// The §14a module in play, if any.
    #[must_use]
    pub const fn sect14a_modul(&self) -> Option<Sect14aModule> {
        match self {
            Self::Einheitlich(_) => None,
            Self::Modul1Pauschal { .. } => Some(Sect14aModule::Modul1),
            Self::Modul2ZeitVariabel { .. } => Some(Sect14aModule::Modul2),
            Self::Modul3Spotpreis { .. } => Some(Sect14aModule::Modul3),
        }
    }
}

// ── Paired inputs ─────────────────────────────────────────────────────────────

/// An RLM demand charge — peak demand and its rate.
///
/// A pair, because billing one without the other is meaningless. The two used to
/// be independent `Option`s checked at runtime in two separate places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Leistungspreis {
    /// Peak demand in kW.
    pub spitzenleistung_kw: Decimal,
    /// Rate in EUR per kW.
    pub preis_eur_per_kw: Decimal,
}

/// A Gas NNE Grundpreis — monthly rate and the months billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Grundpreis {
    /// Rate in EUR per month.
    pub eur_per_month: Decimal,
    /// Months in the billing period.
    pub months: Decimal,
}

/// A Konzessionsabgabe — the rate together with the customer group it applies to.
///
/// Paired so the KAV §2 Höchstbetrag check can always run. They were independent
/// `Option`s, and the ceiling check was skipped entirely when the group was
/// absent — which is exactly when an over-charge is most likely to go unnoticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Konzessionsabgabe {
    /// Published rate in ct/kWh.
    pub satz_ct_per_kwh: Decimal,
    /// The KAV §2 customer group, which fixes the ceiling.
    pub klasse: KaKundengruppe,
}

// ── SettlementPeriod ──────────────────────────────────────────────────────────

/// The delivery period a settlement covers.
///
/// A validated pair rather than two loose dates. Every input struct previously
/// carried `period_from` and `period_to` independently, and every calculation
/// re-checked their ordering — five copies of the same guard, each able to be
/// forgotten. Constructing this type is the check.
///
/// Both bounds are inclusive: a monthly period runs from the 1st to the last day
/// of the month, matching how Netzentgelte are published and billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SettlementPeriod {
    from: time::Date,
    to: time::Date,
}

impl SettlementPeriod {
    /// Build a period.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::BillingError::InvalidInput`] when `from` is after `to`. A
    /// zero-length period (`from == to`) is a valid single day.
    pub fn new(from: time::Date, to: time::Date) -> Result<Self, crate::error::BillingError> {
        if from > to {
            return Err(crate::error::BillingError::InvalidInput {
                reason: format!("period start {from} is after its end {to}"),
            });
        }
        Ok(Self { from, to })
    }

    /// Start of the period, inclusive.
    #[must_use]
    pub const fn from(&self) -> time::Date {
        self.from
    }

    /// End of the period, inclusive.
    #[must_use]
    pub const fn to(&self) -> time::Date {
        self.to
    }

    /// Number of days covered, both bounds inclusive.
    #[must_use]
    pub fn days(&self) -> i64 {
        (self.to - self.from).whole_days() + 1
    }
}

// ── SettlementResult ──────────────────────────────────────────────────────────

/// What a settlement calculation produced.
///
/// This is the canonical output of every calculation in this crate. It answers
/// *what is owed and why*, and deliberately not *what the invoice looks like*:
/// invoice numbers, issue and due dates, Prüfidentifikatoren and position
/// numbering live on [`InvoiceDocument`], which an adapter builds around this.
///
/// The separation is what makes a settlement recomputable. The same period can
/// be settled twice — for a correction, a dispute, or an audit — and the two
/// results compared, without inventing a document each time.
///
/// ## Explainability
///
/// Every position carries a [`CalculationTrace`]; [`Self::all_legal_refs`]
/// collects the paragraphs the settlement rests on. `warnings` records what the
/// engine could not do, which is as much part of the result as the amounts.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettlementResult {
    /// What was settled.
    pub settlement_type: SettlementType,
    /// Where this settlement sits in the correction lifecycle.
    pub status: SettlementStatus,
    /// The delivery period.
    pub period: SettlementPeriod,
    /// The rules the calculation applied.
    pub regime: crate::regulatory::RegulatoryRegime,
    /// Commodity.
    pub sparte: Sparte,
    /// The metering location settled.
    pub malo_id: String,
    /// Sender MP-ID — Netzbetreiber, or MSB for a metering settlement.
    pub nb_mp_id: String,
    /// Recipient MP-ID — Lieferant, MSB, or MGV.
    pub counterparty_mp_id: String,
    /// The positions, in calculation order.
    pub positions: Vec<SettlementPosition>,
    /// Net total in EUR, rounded to 2 decimal places.
    pub total_eur: Decimal,
    /// What the engine could not do, or did with a caveat.
    pub warnings: Vec<SettlementWarning>,
}

// ── InvoiceDocument ───────────────────────────────────────────────────────────

/// A settlement presented as an invoice.
///
/// Everything here is a property of the document rather than of the calculation:
/// an invoice number, the dates it was issued and falls due, the
/// Prüfidentifikator that routes it, and the reference to whatever it corrects.
/// None of it affects what is owed.
///
/// Built by an adapter around a [`SettlementResult`]; the engine never produces
/// one, which is why the engine can be run without inventing an invoice number.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvoiceDocument {
    /// What the document presents.
    pub settlement: SettlementResult,
    /// BDEW Prüfidentifikator.
    pub pid: u32,
    /// Unique invoice reference.
    pub rechnungsnummer: String,
    /// The `rechnungsnummer` this corrects, if any.
    pub correction_of: Option<String>,
    /// Issue date.
    pub invoice_date: time::Date,
    /// Payment due date (Zahlungsziel, §271 BGB).
    pub due_date: time::Date,
}

impl InvoiceDocument {
    /// Positions paired with their 1-based document numbers.
    ///
    /// Numbering is assigned here, at rendering time, rather than carried through
    /// the calculation as mutable state.
    pub fn numbered_positions(&self) -> impl Iterator<Item = (u32, &SettlementPosition)> {
        self.settlement
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| (u32::try_from(i + 1).unwrap_or(u32::MAX), p))
    }
}

impl SettlementResult {
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
    /// The delivery period being settled.
    pub period: SettlementPeriod,

    /// Letztverbrauchergruppe for the network levies (EnFG §§21 ff.).
    ///
    /// Determines which rate of the §19 StromNEV-, Offshore- and KWKG-Umlage
    /// applies at this Entnahmestelle.
    pub letztverbrauchergruppe: crate::umlagen::Letztverbrauchergruppe,

    /// §19 StromNEV-Umlage in ct/kWh, overriding the tabled rate.
    ///
    /// `None` uses the statutory rate for the delivery year and group. Set it
    /// where an EnFG decision grants a rate the published schedule does not
    /// express.
    pub sect19_umlage_ct_per_kwh: Option<Decimal>,
    /// Offshore-Netzumlage in ct/kWh, overriding the tabled rate.
    pub offshore_umlage_ct_per_kwh: Option<Decimal>,
    /// KWKG-Umlage in ct/kWh, overriding the tabled rate.
    pub kwkg_umlage_ct_per_kwh: Option<Decimal>,

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
    ///
    /// One value rather than twelve loose fields: the four shapes are mutually
    /// exclusive by construction.
    pub arbeitspreis: ArbeitspreisModell,

    /// RLM demand charge — peak demand and its rate, or neither.
    pub leistungspreis: Option<Leistungspreis>,

    /// Gas NNE Grundpreis. `None` for Strom, which has no separate Grundpreis.
    pub grundpreis: Option<Grundpreis>,

    /// Konzessionsabgabe — rate and customer group together, so the KAV §2
    /// ceiling can always be checked.
    pub konzessionsabgabe: Option<Konzessionsabgabe>,

    /// The Netzebene this metering point takes supply from.
    ///
    /// Netzentgelte are published per level, so the level is what makes a rate
    /// checkable against a price sheet. Recorded on the settlement and in the
    /// trace; it does not itself select a rate — this crate is given the rates.
    pub netzebene: Option<crate::netzebene::Netzebene>,

    /// Annual peak demand in kW, where the metering point has one.
    ///
    /// Used with the annual energy to record the Benutzungsstundenzahl in the
    /// trace. This is the *annual* peak, which is not the same as the peak in
    /// the billing period — a monthly settlement carries the annual figure so
    /// the utilisation can be checked against the price sheet that priced it.
    pub jahreshoechstleistung_kw: Option<Decimal>,

    /// Annual energy in kWh, where known.
    ///
    /// Pairs with `jahreshoechstleistung_kw` for the Benutzungsstundenzahl, and
    /// decides whether §17 Abs. 6 permits an Arbeitspreis-only tariff.
    pub jahresarbeit_kwh: Option<Decimal>,

    /// An agreed §19 Abs. 2 StromNEV individual charge, where one exists.
    ///
    /// Applied as a reduction over the Arbeits- and Leistungspreis positions,
    /// with the statutory Mindestentgelt floor checked against the utilisation
    /// data above. The Konzessionsabgabe and the network levies are unaffected —
    /// the Netzbetreiber's lost revenue is compensated through the
    /// §19 StromNEV-Umlage, billed separately.
    pub sect19: Option<crate::sect19::Sect19Vereinbarung>,

    /// A booked gas capacity, billed alongside the commodity charge.
    ///
    /// Gas only; §15 GasNEV. The annual rate is pro-rated over the settlement
    /// period by calendar days.
    pub gas_kapazitaet: Option<crate::gas::GasKapazitaet>,
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
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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
    /// The delivery period being settled.
    pub period: SettlementPeriod,
    /// Commodity — determines which Festlegung the legal references cite.
    ///
    /// - `Sparte::Strom` → `GPKE (BK6-24-174) Teil 1 Kap. 8.4`, `GPKE BK6-22-024`
    /// - `Sparte::Gas` → `GaBi Gas 2.1 (BK7-24-01-008)`, `GeLi Gas 3.0 (BK7-24-01-009)`
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
    /// The delivery period being settled.
    pub period: SettlementPeriod,
    /// Grundgebühr Messstellenbetrieb in **EUR/month** (from `PreisblattMessung`).
    pub grundgebuehr_eur_per_month: Decimal,
    /// Number of full calendar months in the billing period.
    pub billing_months: u32,
    /// Optional Messdienstleistung flat fee in **EUR** for the full period.
    ///
    /// `None` when the MSB provides only the meter, not a separate measurement service.
    pub messdienstleistung_eur: Option<Decimal>,

    /// Which §30 MsbG case this metering point falls under.
    ///
    /// Fixes the Preisobergrenze the charge is checked against. `None` skips the
    /// check, which should be rare: a metering charge above the POG is an amount
    /// the customer is entitled to have refunded.
    pub messstellen_kategorie: Option<crate::msbg::MessstellenKategorie>,

    /// Whose share of the metering charge this settlement bills.
    ///
    /// §30 MsbG splits the ceiling between the Netzbetreiber and the
    /// Letztverbraucher, so the applicable cap depends on who is being billed.
    pub entgeltschuldner: Option<crate::msbg::Entgeltschuldner>,
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
    /// The delivery period being settled.
    pub period: SettlementPeriod,
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
/// # use rust_decimal::dec;
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
/// For NNE there is no separate validator: the invariants that mattered are
/// either unrepresentable — an inverted [`SettlementPeriod`], a half-set
/// [`Leistungspreis`], two §14a modules at once — or enforced inside
/// [`crate::settle_nne`] itself. A validator the engine did not call was how a
/// caller who skipped it got billed on the wrong basis with no error.
///
/// [`validate_mmm_input`], [`validate_msb_input`] and [`validate_gas_awh_input`]
/// remain for inputs whose engines accept looser shapes.
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

/// Validate a [`MmmInput`] before calling [`crate::settle_mmm`].
#[must_use]
pub fn validate_mmm_input(input: &MmmInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period.from() >= input.period.to() {
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

/// Validate a [`MsbInput`] before calling [`crate::settle_msb`].
#[must_use]
pub fn validate_msb_input(input: &MsbInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period.from() >= input.period.to() {
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

/// Validate a [`GasAwhInput`] before calling [`crate::settle_gas_awh`].
///
/// Checks that:
/// - `period_from < period_to`
/// - `awh_positionen` is non-empty
/// - All positions have `anzahl ≥ 1` and `preis_eur ≥ 0`
#[must_use]
pub fn validate_gas_awh_input(input: &GasAwhInput) -> ValidationResult {
    let mut r = ValidationResult::ok();
    if input.period.from() >= input.period.to() {
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

#[cfg(test)]
mod input_model_tests {
    use super::*;
    use rust_decimal::dec;

    /// A reduction factor outside `(0, 1]` cannot be built.
    ///
    /// It used to be a bare `Decimal`, range-checked in a validator the engine
    /// did not call — so `settle_nne` would happily multiply the published
    /// tariff by 5.
    #[test]
    fn a_reduction_factor_must_actually_reduce() {
        assert!(Reduktionsfaktor::new(dec!(0.85)).is_ok());
        assert!(
            Reduktionsfaktor::new(dec!(1)).is_ok(),
            "no reduction is still valid"
        );
        assert!(
            Reduktionsfaktor::new(dec!(0)).is_err(),
            "zero is not a reduction"
        );
        assert!(Reduktionsfaktor::new(dec!(-0.5)).is_err());
        assert!(
            Reduktionsfaktor::new(dec!(5)).is_err(),
            "5x is not a reduction"
        );
        assert_eq!(Reduktionsfaktor::REGELFALL.get(), dec!(0.85));
    }

    /// The charged energy is the same figure whichever model priced it.
    ///
    /// The Konzessionsabgabe and the three network levies are charged on it, and
    /// each used to recompute the base with its own `if has_tou` branch.
    #[test]
    fn every_model_reports_the_energy_it_priced() {
        let flat = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: dec!(1000),
            preis_ct_per_kwh: dec!(3.5),
        });
        assert_eq!(flat.menge_kwh(), dec!(1000));

        let tou = ArbeitspreisModell::Modul2ZeitVariabel {
            ht: MengePreis {
                menge_kwh: dec!(600),
                preis_ct_per_kwh: dec!(4.0),
            },
            nt: MengePreis {
                menge_kwh: dec!(400),
                preis_ct_per_kwh: dec!(1.5),
            },
        };
        assert_eq!(tou.menge_kwh(), dec!(1000), "HT + NT, not one of them");

        let modul1 = ArbeitspreisModell::Modul1Pauschal {
            basis: MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(3.5),
            },
            reduktion: Reduktionsfaktor::REGELFALL,
        };
        assert_eq!(
            modul1.menge_kwh(),
            dec!(1000),
            "the reduction changes the rate, not the energy"
        );
    }

    /// Each model names its §14a module, and only one can be in play.
    #[test]
    fn a_model_carries_at_most_one_sect14a_module() {
        use Sect14aModule as M;
        let cases = [
            (
                ArbeitspreisModell::Einheitlich(MengePreis {
                    menge_kwh: dec!(1),
                    preis_ct_per_kwh: dec!(1),
                }),
                None,
            ),
            (
                ArbeitspreisModell::Modul1Pauschal {
                    basis: MengePreis {
                        menge_kwh: dec!(1),
                        preis_ct_per_kwh: dec!(1),
                    },
                    reduktion: Reduktionsfaktor::REGELFALL,
                },
                Some(M::Modul1),
            ),
            (
                ArbeitspreisModell::Modul2ZeitVariabel {
                    ht: MengePreis {
                        menge_kwh: dec!(1),
                        preis_ct_per_kwh: dec!(1),
                    },
                    nt: MengePreis {
                        menge_kwh: dec!(1),
                        preis_ct_per_kwh: dec!(1),
                    },
                },
                Some(M::Modul2),
            ),
            (
                ArbeitspreisModell::Modul3Spotpreis { intervalle: vec![] },
                Some(M::Modul3),
            ),
        ];
        for (model, expected) in cases {
            assert_eq!(model.sect14a_modul(), expected);
        }
    }

    /// A period is ordered by construction; a single day is valid.
    #[test]
    fn a_period_cannot_be_inverted() {
        use time::macros::date;
        assert!(SettlementPeriod::new(date!(2026 - 01 - 31), date!(2026 - 01 - 01)).is_err());
        let one_day = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 01))
            .expect("a single day is a period");
        assert_eq!(one_day.days(), 1);
        let january =
            SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).expect("valid");
        assert_eq!(january.days(), 31, "both bounds are inclusive");
    }
}
