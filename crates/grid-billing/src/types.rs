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
//! All monetary amounts use [`rust_decimal::Decimal`]. The `crate::EuroAmount`
//! newtype provides overflow-safe EUR arithmetic. No `f32`/`f64` appears anywhere
//! in settlement calculations.

use crate::rounding::RoundMoney;
use rust_decimal::Decimal;

// ── Sparte ────────────────────────────────────────────────────────────────────

/// Commodity — Strom (electricity) or Gas.
///
/// Controls which legal references are applied to each settlement position:
/// - `Strom` → `StromNEV`, BK6 Festlegungen
/// - `Gas` → `GasNEV`, BK7 Festlegungen
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// billed under: §2 Abs. 2 for a Tarifkunde, Abs. 3 for a
    /// Sondervertragskunde, Abs. 7 for a freigestellter Kunde.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sect14aModule {
    /// Modul 1 — **pauschale Reduzierung des Netzentgelts**.
    ///
    /// A flat reduction applied for the whole billing period, published by the
    /// NB either as an annual EUR amount or as a factor on the rate. It needs no
    /// additional metering, which is why it is the default where the connection
    /// holder makes no choice. It is the one base module Modul 3 may be added to.
    Modul1,
    /// Modul 2 — **prozentuale Reduzierung des Arbeitspreises**.
    ///
    /// The Arbeitspreis of the Netzentgelt is reduced by a percentage for the
    /// controllable device, which therefore needs its **own metering** — the
    /// reduction attaches to that device's energy, not to the whole connection.
    ///
    /// An **alternative to Modul 1**, not an addition to it, and it takes no
    /// Modul 3 (see [`Sect14aModule::combinable_with`]).
    Modul2,
    /// Modul 3 — **zeitvariable Netzentgelte**, available from 01.04.2025.
    ///
    /// Three Tarifstufen — Hochtarif, Standardtarif and Niedertarif — whose
    /// windows the NB publishes in the UTILTS Zählzeitdefinition. Requires an
    /// intelligent metering system. It may be combined with Modul 1 but **not**
    /// with Modul 2.
    Modul3,
}

impl Sect14aModule {
    /// Canonical BNetzA decision reference for this module.
    #[must_use]
    pub fn bnentza_reference(self) -> &'static str {
        "BK6-22-300"
    }

    /// Whether two **different** modules may be held at once.
    ///
    /// BK6-22-300 offers one base module and one optional addition. Modul 1 and
    /// Modul 2 are the two forms the base takes — a pauschale reduction needing
    /// no metering, or a percentage on the device's own Arbeitspreis — and the
    /// Anschlussnutzer picks one. Modul 3 re-prices the Arbeitspreis over time,
    /// so it composes with the pauschale Modul 1 and not with Modul 2, which
    /// would reduce the same Arbeitspreis twice.
    ///
    /// `Modul 1 + Modul 3` is therefore the only pair, in either order. A module
    /// paired with itself is not a combination and answers `false`.
    #[must_use]
    pub fn combinable_with(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Modul1, Self::Modul3) | (Self::Modul3, Self::Modul1)
        )
    }

    /// Display label for the module.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Modul1 => "§14a EnWG Modul 1 (pauschale Reduzierung)",
            Self::Modul2 => "§14a EnWG Modul 2 (prozentuale Arbeitspreisreduzierung)",
            Self::Modul3 => "§14a EnWG Modul 3 (zeitvariable Netzentgelte)",
        }
    }
}

// ── SettlementType ────────────────────────────────────────────────────────────

/// Which regulated settlement process produced this result.
///
/// Determines which BDEW PIDs are applicable and which regulatory references apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SettlementType {
    /// Abschlagsrechnung Netznutzung — PID 31001 (NB → LF).
    ///
    /// A payment on account, not a settled period: it prices no energy and
    /// carries **exactly one** Positionszeile (INVOIC AHB 1.0b, Änd-ID 26817 —
    /// "Eine Abschlagsrechnung kann und muss genau eine Positionszeile
    /// enthalten"). What settles it is the Abschlussrechnung that follows,
    /// which deducts it by invoice number.
    NneAbschlag,
    /// Netznutzungsentgelt (NNE) Strom — PID 31002 (NN-Rechnung, NB → LF).
    NneStrom,
    /// Netznutzungsentgelt (NNE) Gas — PID 31002 (NN-Rechnung, NB → LF, GasNEV).
    ///
    /// NNE Strom and Gas share the INVOIC Prüfidentifikator 31002 (NN-Rechnung);
    /// the Sparte is carried in the message content, not the PID. Keeping a
    /// separate variant preserves the correct legal references (StromNEV vs
    /// GasNEV) without conditional logic in call sites.
    NneGas,
    /// Mehr-/Mindermengen settlement Strom — PID 31005 (NB → LF, GPKE (BK6-24-174) Teil 1 Kap. 8.4).
    MmmStrom,
    /// Mehr-/Mindermengen settlement Gas — PID 31005 (NB → LF, GaBi Gas 2.1 (BK7-24-01-008)).
    ///
    /// Gas MMM settlement uses different legal references from Strom MMM:
    /// `GaBi Gas 2.1 (BK7-24-01-008)` and `GeLi Gas 3.0 (BK7-24-01-009)`. Using a separate variant
    /// ensures correct audit traces without conditional logic in call sites.
    MmmGas,
    /// Mehr-/Mindermengen Mehrmenge, selbst ausgestellte Rechnung (Lieferung) — PID 31006.
    ///
    /// Per INVOIC AHB §3.x, PID 31006 covers the Mehrmenge leg when the Mehr-/
    /// Mindermenge is treated as a „Lieferung“ and the invoice is self-issued.
    MmmSelbstausstellt,
    /// Messstellenbetrieb settlement — PID 31009 (MSB → NB / LF / ESA).
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
            Self::NneAbschlag => 31001,
            Self::NneStrom => 31002,
            Self::NneGas => 31002,
            Self::MmmStrom => 31005,
            Self::MmmGas => 31005,
            Self::MmmSelbstausstellt => 31006,
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
/// Settlements are never destroyed — a correction or cancellation produces a new
/// result and leaves the original intact.
///
/// **Which document supersedes which is not recorded here.** The invoice numbers
/// linking a correction to what it replaces live on
/// [`InvoiceDocument::correction_of`], because the same pair of settlements can
/// be presented under different invoice numbers. What *is* recorded here is
/// [`SettlementResult::korrektur_grund`] — why the recalculation happened, which
/// is a fact about the settlement rather than about the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SettlementStatus {
    /// Initial calculation — no prior settlement exists for this period.
    Initial,
    /// Correction of a prior settlement.
    Correction,
    /// Cancellation of a prior settlement — all positions are negated.
    Reversal,
    /// Final settlement — no further corrections expected.
    Final,
}

// ── KorrekturGrund ────────────────────────────────────────────────────────────

/// Why a settlement was recalculated.
///
/// A correction that cannot say why it happened is not an audit trail. The
/// invoice numbers alone answer *what* was replaced; they never answer whether
/// the meter was wrong, the tariff was wrong, or the law changed underneath —
/// and those have different consequences. A retroactive regulatory change is a
/// lawful recalculation; a Rechenfehler in the same period is a defect that
/// should be counted and investigated.
///
/// Carried on every non-`Initial` [`SettlementResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "bo4e", derive(serde::Deserialize))]
pub enum KorrekturGrund {
    /// Corrected metering — a replaced or re-read value (§ 60 Abs. 2 MsbG).
    Messwertkorrektur,
    /// The wrong tariff or price sheet version was applied.
    Tarifkorrektur,
    /// Master data was wrong — Netzebene, KA-Klasse, Konzessionsgemeinde.
    Stammdatenkorrektur,
    /// A regulatory change applies retroactively to a settled period.
    RegulatorischeAenderung,
    /// An arithmetic or logic error in the original settlement.
    Rechenfehler,
    /// A clearing result between the parties (Mehr-/Mindermengen, MaBiS).
    Clearing,
    /// Anything else — carry the detail in the settlement's warnings.
    Sonstiges,
}

impl KorrekturGrund {
    /// Stable machine-readable code for structured records and reporting.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Messwertkorrektur => "MESSWERTKORREKTUR",
            Self::Tarifkorrektur => "TARIFKORREKTUR",
            Self::Stammdatenkorrektur => "STAMMDATENKORREKTUR",
            Self::RegulatorischeAenderung => "REGULATORISCHE_AENDERUNG",
            Self::Rechenfehler => "RECHENFEHLER",
            Self::Clearing => "CLEARING",
            Self::Sonstiges => "SONSTIGES",
        }
    }

    /// Whether this reason indicates a defect in the original settlement rather
    /// than a lawful recalculation.
    ///
    /// Separating the two is the point of recording the reason: a rising count
    /// of `Rechenfehler` is an engineering signal, while a rising count of
    /// `RegulatorischeAenderung` is not.
    #[must_use]
    pub const fn indicates_defect(self) -> bool {
        matches!(self, Self::Rechenfehler | Self::Stammdatenkorrektur)
    }
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
    /// UStG — Umsatzsteuergesetz.
    ///
    /// Cited where the tax treatment is itself part of what the position claims:
    /// an Anzahlung under §14 Abs. 5, a reverse charge under §13b.
    Ustg {
        /// Paragraph citation, e.g. `"§14 Abs. 5"`.
        paragraph: &'static str,
    },
    /// §14a EnWG — Steuerbare Verbrauchseinrichtungen (controllable loads).
    ///
    /// Governs time-variable (ToU) NNE for heat pumps, EV chargers, etc.
    Sect14aEnwg {
        /// Module: Modul1 (flat reduction), Modul2 (HT/NT), or Modul3 (spot).
        module: Sect14aModule,
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
            Self::Ustg { paragraph } => format!("UStG {paragraph}"),
            Self::Kwkg { paragraph } => format!("KWKG {paragraph}"),
            Self::EnFG { paragraph } => format!("EnFG {paragraph}"),
            Self::Sect14aEnwg { module } => format!("§14a EnWG {}", module.label()),
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
/// | `NneArbeit` | `Wirkarbeit` | PID 31002 (NN-Rechnung) Arbeit |
/// | `NneArbeitHt` | `Wirkarbeit` | PID 31002 §14a Modul 3 HT |
/// | `NneArbeitSt` | `Wirkarbeit` | PID 31002 §14a Modul 3 ST |
/// | `NneArbeitNt` | `Wirkarbeit` | PID 31002 §14a Modul 3 NT |
/// | `NneArbeitModul2` | `Wirkarbeit` | PID 31002 §14a Modul 2 (rate reduced) |
/// | `NneArbeitModul1` | `Wirkarbeit` | PID 31002 §14a Modul 1 (rate reduced) |
/// | `NneLeistung` | `Leistung` | PID 31002 RLM kW charge |
/// | `NneGasGrundpreis` | `Grundpreis` | PID 31002 Gas monthly base fee |
/// | `Konzessionsabgabe` | `Konzessionsabgabe` | PID 31002 KAV §2 |
/// | `Mehrmenge` | `Mehrmenge` | PID 31005 positive imbalance |
/// | `Mindermenge` | `Mindermenge` | PID 31005 negative imbalance (credit) |
/// | `MsbGrundgebuehr` | `EntgeltEinbauBetriebWartungMesstechnik` | PID 31009 MSB monthly fee |
/// | `Messdienstleistung` | `EntgeltMessungAblesung` | PID 31009 reading service |
/// | `GasAwhSperrung` | `Sperrkosten` | PID 31011 AWH disconnection |
/// | `GasAwhEntsprrung` | `Entsperrkosten` | PID 31011 AWH reconnection |
/// | `GasAwhSonstige` | `EntgeltAbrechnung` | PID 31011 other AWH |
/// | `Blindmehrarbeit` | `Blindmehrarbeit` | Reactive energy excess |
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum BillingPositionKind {
    /// The single line of an Abschlagsrechnung — a payment on account.
    ///
    /// Carries an amount and nothing else: no quantity, no unit price, because
    /// an Abschlag prices no energy. What it is *for* is the delivery period on
    /// the settlement.
    NneAbschlag,
    /// Netznutzungsentgelt Arbeit — flat-rate active energy charge (kWh).
    /// SLP or Gas. → `BdewArtikelnummer::Wirkarbeit`
    NneArbeit,
    /// §14a Modul 3 Hochtarif (HT) Arbeit — zeitvariables Netzentgelt, high band.
    /// → `BdewArtikelnummer::Wirkarbeit`
    NneArbeitHt,
    /// §14a Modul 3 Standardtarif (ST) Arbeit — zeitvariables Netzentgelt, middle band.
    /// → `BdewArtikelnummer::Wirkarbeit`
    NneArbeitSt,
    /// §14a Modul 3 Niedertarif (NT) Arbeit — zeitvariables Netzentgelt, low band.
    /// → `BdewArtikelnummer::Wirkarbeit`
    NneArbeitNt,
    /// §14a Modul 1 Arbeit — pauschale Reduzierung applied to the Arbeitspreis.
    /// → `BdewArtikelnummer::Wirkarbeit` (same article, different rate)
    NneArbeitModul1,
    /// §14a Modul 2 Arbeit — prozentuale Reduzierung of the device's Arbeitspreis.
    /// → `BdewArtikelnummer::Wirkarbeit` (same article, different rate)
    NneArbeitModul2,
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
    /// PID 31005 GPKE (BK6-24-174) Teil 1 Kap. 8.4 / GaBi Gas 2.1 (BK7-24-01-008). → `BdewArtikelnummer::Mehrmenge`
    Mehrmenge,
    /// Mindermengen — negative imbalance credit note (actual < profiled).
    /// PID 31005. → `BdewArtikelnummer::Mindermenge`
    Mindermenge,
    /// MSB Grundgebühr Messstellenbetrieb — monthly metering base fee.
    /// MsbG §§6–7. → `BdewArtikelnummer::EntgeltEinbauBetriebWartungMesstechnik`
    MsbGrundgebuehr,
    /// Messdienstleistung — periodic reading service fee.
    /// MsbG §2. → `BdewArtikelnummer::EntgeltMessungAblesung`
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
    /// Blindmehrarbeit — reactive energy beyond the free share.
    ///
    /// Charged from the Netzbetreiber's published Preisblatt; StromNEV §17
    /// governs how those Netzentgelte are formed. **Not** §18, which is the
    /// Entgelt für dezentrale Erzeugung, and not §19, which is Sonderformen der
    /// Netznutzung. → `BdewArtikelnummer::Blindmehrarbeit`
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
/// Invariant: `net_eur == (quantity × unit_price_eur).round_kfm(5)`.
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
            // An Abschlag prices nothing, so it carries no Artikelnummer: the
            // codelist names charges, and a payment on account is not one.
            (K::NneAbschlag, _) => None,
            // Gas NNE keeps the classic codes — BK6-20-160 changed Strom only.
            (
                K::NneArbeit
                | K::NneArbeitHt
                | K::NneArbeitSt
                | K::NneArbeitNt
                | K::NneArbeitModul1
                | K::NneArbeitModul2
                | K::NneArbeitModul3,
                ST::NneGas,
            ) => Some("WIRKARBEIT"),
            (K::NneLeistung, ST::NneGas) => Some("LEISTUNG"),
            (K::NneGasGrundpreis, _) => Some("GRUNDPREIS"),
            // Strom NNE: the Artikel-ID replaces the Artikelnummer.
            (
                K::NneArbeit
                | K::NneArbeitHt
                | K::NneArbeitSt
                | K::NneArbeitNt
                | K::NneArbeitModul1
                | K::NneArbeitModul2
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

/// A §14a **Modul 2** reduction factor — the fraction of the published
/// Arbeitspreis actually paid.
///
/// Modul 2 is the *prozentuale* reduction; Modul 1 is the flat annual pauschale
/// and carries no factor at all (see [`ArbeitspreisModell::Modul1Pauschal`]).
///
/// A newtype because the range matters: `"0.85"` is a 15 % reduction, and a
/// value outside `(0, 1]` is not a reduction at all. It travels as a JSON
/// **string** like every other `Decimal` — see the architecture page's
/// *Quantities and money on the wire*. The unconstrained `Decimal` this
/// replaces was range-checked in the validator and *not* in the engine, so a
/// caller who skipped validation could multiply the tariff by 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Reduktionsfaktor(Decimal);

impl Reduktionsfaktor {
    /// A commonly published factor — 85 % of the tariff, i.e. a 15 % reduction.
    ///
    /// Not a statutory rate: BK8-22/010-A leaves the Modul-2 percentage to each
    /// Netzbetreiber's published Preisblatt, so this is a convenience default
    /// and never a substitute for the operator's own figure.
    pub const REGELFALL: Self = Self(rust_decimal::dec!(0.85));

    /// Build a factor.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::BillingError::InvalidInput`] outside `(0, 1]`.
    pub fn new(factor: Decimal) -> Result<Self, crate::error::BillingError> {
        if factor <= Decimal::ZERO || factor > Decimal::ONE {
            return Err(crate::error::BillingError::InvalidInput {
                reason: format!("§14a Modul 2 reduction factor must be in (0, 1], got {factor}"),
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

/// Deserialising goes through [`Reduktionsfaktor::new`], so a factor that
/// arrives over the wire is range-checked exactly like one built in process.
///
/// Deriving it would have reintroduced the unconstrained `Decimal` this newtype
/// exists to prevent: a request body carrying `5` would then multiply the
/// Arbeitspreis by five with no error anywhere.
impl<'de> serde::Deserialize<'de> for Reduktionsfaktor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <Decimal as serde::Deserialize>::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// A metered quantity priced at a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ArbeitspreisModell {
    /// A single rate for all metered energy.
    Einheitlich(MengePreis),

    /// **§14a Modul 1** — pauschale Reduzierung des Netzentgelts.
    ///
    /// A **flat annual amount** the Netzbetreiber publishes, credited pro rata
    /// for the settlement period. It does not scale with consumption — that is
    /// what makes it *pauschal*, and what distinguishes it from
    /// [`Self::Modul2ProzentualeReduzierung`], which reduces the Arbeitspreis by
    /// a percentage. The two were structurally identical here once; a factor on
    /// the Arbeitspreis is Modul 2's mechanism wearing Modul 1's name.
    ///
    /// Needs no additional metering, which is why it is the default where the
    /// connection holder makes no choice.
    Modul1Pauschal {
        /// The energy delivered in the period, at its published rate. Billed in
        /// full — Modul 1 does not touch the Arbeitspreis.
        basis: MengePreis,
        /// The Netzbetreiber's published annual pauschale, in EUR per year.
        pauschale_eur_pro_jahr: Decimal,
        /// The fraction of a year this settlement period covers, so the annual
        /// pauschale is credited pro rata.
        jahresanteil: Decimal,
    },

    /// **§14a Modul 2** — the Arbeitspreis reduced by a percentage.
    ///
    /// The reduction attaches to the controllable device's own metered energy,
    /// so `basis` carries that device's consumption rather than the whole
    /// connection's.
    Modul2ProzentualeReduzierung {
        /// The device's metered energy and its published rate, before reduction.
        basis: MengePreis,
        /// The fraction of that rate actually paid.
        reduktion: Reduktionsfaktor,
    },

    /// **§14a Modul 3** — zeitvariable Netzentgelte in three Tarifstufen.
    ///
    /// All three bands are required: BK6-22-300 defines Hochtarif, Standardtarif
    /// and Niedertarif, and permitting a subset would reintroduce the partial
    /// state this type exists to prevent. A band with no energy carries
    /// `menge_kwh = 0` rather than being omitted.
    Modul3ZeitVariabel {
        /// Hochtarif band.
        ht: MengePreis,
        /// Standardtarif band.
        st: MengePreis,
        /// Niedertarif band.
        nt: MengePreis,
    },

    /// A spot-derived NNE rate per dispatch interval.
    ///
    /// **Not a §14a module.** BK6-22-300 defines exactly three, none of which is
    /// spot-linked; this models a Netzentgelt whose rate follows the spot price
    /// under the NB's own `PreisblattNetznutzung` formula. The rates arrive
    /// already derived — this crate never queries a spot market.
    SpotpreisNetzentgelt {
        /// The dispatch intervals, each with its own rate.
        intervalle: Vec<SpotpreisInterval>,
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
            Self::Modul2ProzentualeReduzierung { basis, .. } => basis.menge_kwh,
            Self::Modul3ZeitVariabel { ht, st, nt } => ht.menge_kwh + st.menge_kwh + nt.menge_kwh,
            Self::SpotpreisNetzentgelt { intervalle } => {
                intervalle.iter().map(|i| i.menge_kwh).sum()
            }
        }
    }

    /// The §14a module in play, if any.
    #[must_use]
    pub const fn sect14a_modul(&self) -> Option<Sect14aModule> {
        match self {
            Self::Einheitlich(_) => None,
            Self::Modul1Pauschal { .. } => Some(Sect14aModule::Modul1),
            Self::Modul2ProzentualeReduzierung { .. } => Some(Sect14aModule::Modul2),
            Self::Modul3ZeitVariabel { .. } => Some(Sect14aModule::Modul3),
            // A spot-linked Netzentgelt is the NB's own price model, not one of
            // the three modules BK6-22-300 defines.
            Self::SpotpreisNetzentgelt { .. } => None,
        }
    }
}

// ── Blindarbeit ───────────────────────────────────────────────────────────────

/// Reactive energy and the terms on which its excess is charged.
///
/// A Netzbetreiber supplies a *free share* of reactive energy alongside the
/// active energy delivered, and charges only what exceeds it (Blindmehrarbeit).
/// The customary boundary is a power factor of cos φ 0,9 — reactive energy up to
/// **tan φ ≈ 0,4843** of the active energy — though many Preisblätter round that
/// to a flat 50 %, and some set different shares for inductive and capacitive
/// draw.
///
/// The share is therefore an **input**, not a constant: it is a term of the
/// Netzbetreiber's price sheet, and hard-coding one would bill some networks
/// wrongly. [`Blindarbeit::COS_PHI_0_9`] is the documented default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Blindarbeit {
    /// Reactive energy drawn in the period, in kvarh.
    pub blindarbeit_kvarh: Decimal,
    /// Free share of the active energy, as a fraction.
    ///
    /// `0.4843` for cos φ 0,9; `0.5` where the Preisblatt rounds it.
    pub freigrenze_anteil: Decimal,
    /// Price per excess kvarh, in ct/kvarh, from the Preisblatt.
    pub preis_ct_per_kvarh: Decimal,
}

impl Blindarbeit {
    /// tan φ at cos φ 0,9 — the customary free share, to 4 dp.
    pub const COS_PHI_0_9: Decimal = rust_decimal::dec!(0.4843);

    /// The chargeable excess in kvarh for the given active energy.
    ///
    /// Zero when the draw stays inside the free share; never negative, because
    /// an unused allowance is not a credit.
    #[must_use]
    pub fn mehrarbeit_kvarh(&self, wirkarbeit_kwh: Decimal) -> Decimal {
        let frei = wirkarbeit_kwh * self.freigrenze_anteil;
        (self.blindarbeit_kvarh - frei).max(Decimal::ZERO)
    }
}

// ── Paired inputs ─────────────────────────────────────────────────────────────

/// An RLM demand charge — peak demand and its rate.
///
/// A pair, because billing one without the other is meaningless — which two
/// independent `Option`s cannot express.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Leistungspreis {
    /// Peak demand in kW.
    pub spitzenleistung_kw: Decimal,
    /// Rate in EUR per kW.
    pub preis_eur_per_kw: Decimal,
}

/// A Gas NNE Grundpreis — monthly rate and the months billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Konzessionsabgabe {
    /// Published rate in ct/kWh.
    pub satz_ct_per_kwh: Decimal,
    /// The KAV §2 customer group, which fixes the ceiling.
    pub klasse: KaKundengruppe,
}

// ── SettlementPeriod ──────────────────────────────────────────────────────────

/// The delivery period a settlement covers.
///
/// A validated pair rather than two loose dates: constructing the type is the
/// ordering check, so no calculation carries its own copy of it.
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
    /// Why this settlement was recalculated.
    ///
    /// `None` for an `Initial` settlement and required for every other status —
    /// see [`SettlementResult::lineage_is_consistent`].
    pub korrektur_grund: Option<KorrekturGrund>,
    /// The delivery period.
    pub period: SettlementPeriod,
    /// The rules the calculation applied.
    pub regime: crate::regulatory::RegulatoryRegime,
    /// Commodity.
    pub sparte: Sparte,
    /// The metering location settled.
    pub malo_id: String,
    /// Sender MP-ID — the party issuing the invoice.
    ///
    /// The Netzbetreiber for NNE/MMM, and the **Messstellenbetreiber** for a
    /// MSB-Rechnung (PID 31009). Named for the role it plays, not for one of
    /// the roles that can fill it.
    pub sender_mp_id: String,
    /// Recipient MP-ID — the party being billed (LF, NB, MSB, MGV or ESA).
    pub recipient_mp_id: String,
    /// The positions, in calculation order.
    pub positions: Vec<SettlementPosition>,
    /// Net total in EUR, rounded to 2 decimal places.
    pub total_eur: Decimal,
    /// The Umsatzsteuer on that net total.
    ///
    /// §14 Abs. 4 Nr. 8 UStG requires the rate and the amount on every invoice,
    /// or a note saying why neither is stated. A settlement that carries only a
    /// net figure cannot be rendered as a lawful Rechnung, and the recipient
    /// gets no Vorsteuerabzug from one.
    pub steuer: crate::umsatzsteuer::Steuerausweis,
    /// What the engine could not do, or did with a caveat.
    pub warnings: Vec<SettlementWarning>,
}

impl SettlementResult {
    /// Whether the lifecycle status and the recorded reason agree.
    ///
    /// An `Initial` settlement corrects nothing and must carry no reason; every
    /// other status is a recalculation and must say why. A `Correction` with no
    /// reason is the state this check exists to catch — it looks like a complete
    /// settlement and answers none of the questions an audit asks of one.
    #[must_use]
    pub const fn lineage_is_consistent(&self) -> bool {
        match self.status {
            SettlementStatus::Initial => self.korrektur_grund.is_none(),
            _ => self.korrektur_grund.is_some(),
        }
    }

    /// Whether this settlement records a defect in an earlier one.
    ///
    /// Distinguishes an engineering signal from a lawful recalculation — see
    /// [`KorrekturGrund::indicates_defect`].
    #[must_use]
    pub fn corrects_a_defect(&self) -> bool {
        self.korrektur_grund
            .is_some_and(KorrekturGrund::indicates_defect)
    }
}

// ── Abschlagsverrechnung ──────────────────────────────────────────────────────

/// An Abschlagsrechnung a later invoice deducts.
///
/// The INVOIC AHB puts these in the Summenteil, not among the positions:
/// `SG50 MOA+113` carries the **gross** amount already paid, `SG51 RFF+AFL` the
/// invoice number it was billed under and `SG51 DTM+3` that invoice's date. They
/// therefore reduce what is *owed*, never the net or the tax — §14 Abs. 5 UStG
/// taxes the Anzahlung when it is received, so the Abschlussrechnung does not
/// tax it a second time.
///
/// Two rules from the AHB travel with this type:
///
/// - **\[526\]** — the amount stated must equal the referenced Abschlagsrechnung's
///   own Rechnungsbetrag. A deduction that does not match what was billed is a
///   deduction the counterparty will reject.
/// - **\[519\]** — a *stornierte* Abschlagsrechnung is not listed. It was
///   reversed, so nothing was paid on it, and deducting it would credit money
///   that never moved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Abschlagsverrechnung {
    /// The Abschlagsrechnung's invoice number (`SG51 RFF+AFL`).
    pub rechnungsnummer: String,
    /// That invoice's date (`SG51 DTM+3`).
    pub rechnungsdatum: time::Date,
    /// The **gross** amount already billed on it (`SG50 MOA+113`, inkl. USt.).
    pub betrag_brutto_eur: Decimal,
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
    /// The billing cadence — `IMD+7081` on the wire.
    ///
    /// A document fact, not a calculation one: an NNE settlement is the same
    /// arithmetic whether it is billed monthly, per Turnus or as the
    /// Abschlussrechnung that closes a year. `None` leaves the field unset
    /// rather than guessing a rhythm nothing supports.
    pub cadence: Option<Rechnungscharakter>,
    /// Abschlagsrechnungen this document settles, deducted from what is owed.
    ///
    /// Empty on an Abschlagsrechnung itself and on every document that has no
    /// payments on account to reconcile.
    pub abschlaege: Vec<Abschlagsverrechnung>,
}

/// The billing cadence of a Netznutzungsrechnung — `IMD+7081`.
///
/// Named for what the AHB calls it. The set is closed: these are the codes the
/// INVOIC AHB 1.0b permits for a Netznutzungsrechnung, and inventing a rhythm
/// outside them would put a claim on the wire the standard does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Rechnungscharakter {
    /// `ABS` — a payment on account (PID 31001).
    Abschlagsrechnung,
    /// `ABR` — the invoice that closes a period and settles its Abschläge.
    Abschlussrechnung,
    /// `JVR` — the periodic invoice of a billing cycle.
    Turnusrechnung,
    /// `MVR` — a monthly invoice.
    Monatsrechnung,
    /// `ZVR` — an invoice between two Turnus invoices.
    Zwischenrechnung,
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
            .round_kfm(2)
    }
}

// ── Input types ───────────────────────────────────────────────────────────────

/// Input for NNE (Netznutzungsentgelt) invoice calculation.
///
/// Covers:
/// - **PID 31002** (NN-Rechnung) — NNE Strom and Gas (NB → LF, monthly network
///   usage billing). The Sparte is carried in the message content, not the PID.
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
///   (edmd's `MeterBillingPeriod.arbeitsmenge_kwh` carries this converted value.)
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

    /// kWh already consumed at this Entnahmestelle earlier in the same calendar
    /// year, for the EnFG 1-GWh boundary.
    ///
    /// The B′/C′ rates are published for quantities *über* 1 000 000 kWh a year,
    /// so a settlement covering one period of that year cannot tell which side
    /// of the boundary its quantity falls on without knowing what came before.
    /// `None` is read as zero — the start of the year — which puts the first
    /// Gigawattstunde on the full rate. That is the direction that over-bills
    /// rather than under-bills, and the settlement says so in a warning.
    ///
    /// Ignored for groups A′ and Befreit, which have no boundary.
    pub enfg_jahresvorverbrauch_kwh: Option<Decimal>,

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

    /// Reactive energy and its Preisblatt terms.
    ///
    /// `None` = the network does not charge Blindmehrarbeit at this location, or
    /// the reactive energy was not metered.
    pub blindarbeit: Option<Blindarbeit>,

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
    /// **Empty (default)** when no spot-linked Netzentgelt applies to this MaLo.
    ///
    /// Selecting this model excludes every other `ArbeitspreisModell` by
    /// construction — the enum holds one at a time.
    ///
    /// Each interval generates one `InvoicePosition` with
    /// `kind = NneArbeitModul3` and `lastvariable_preisposition_json` populated.
    #[doc = "Spot-linked Netzentgelt per-interval input data."]
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

// ── SpotpreisInterval ─────────────────────────────────────────────────────

/// One dispatch interval for a spot-linked Netzentgelt (not a §14a module).
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
/// 1. Fetch the EPEX Spot day-ahead price for each 15-min interval from `productd`
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpotpreisInterval {
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
/// - **PID 31005** — MMM-Rechnung used for Mehr-/Mindermengen settlement between
///   NB and LF (Strom and Gas).
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
    /// Who holds §3g Wiederverkäufer status, evidenced by a *USt 1 TH*.
    ///
    /// A Mehr-/Mindermenge is a **Lieferung** of electricity or gas, not a
    /// network service, so §13b Abs. 2 Nr. 5 Buchst. b UStG can shift the tax to
    /// the recipient. The condition differs by Sparte — electricity needs both
    /// parties, gas needs the recipient — which is why this is a status rather
    /// than a `reverse_charge: bool` the caller has to reason out.
    pub wiederverkaeufer: crate::umsatzsteuer::Wiederverkaeuferstatus,
    /// The receiving party issues this invoice itself (Gutschriftverfahren).
    ///
    /// PID 31006 (Strom) / 31008 (Gas) is the Mehrmenge leg written by the
    /// party that would otherwise receive it, which the AHB marks as
    /// *Selbstausgestellt* rather than *Handelsrechnung*. That distinction is
    /// on the wire (`IMD+7081` and the Rechnungsart), so it has to come from
    /// the settlement rather than be stamped on the document afterwards:
    /// labelling an ordinary [`SettlementType::MmmStrom`] with PID 31006
    /// produces a message that states Handelsrechnung under a
    /// Selbstausstellung Prüfidentifikator.
    pub selbstausgestellt: bool,
}

// ── AbschlagInput ─────────────────────────────────────────────────────────────

/// Input for an Abschlagsrechnung Netznutzung (PID 31001).
///
/// A payment on account. There is no metered quantity and no Arbeitspreis: the
/// Netzbetreiber asks for an amount against a period it has not settled yet, and
/// the Abschlussrechnung that follows deducts it by invoice number.
///
/// How the amount is arrived at — a share of last year's Turnusrechnung, a
/// forecast from the Jahresarbeit, a figure agreed in the
/// Lieferantenrahmenvertrag — is the operator's judgement, not arithmetic this
/// crate can check. [`Self::grundlage`] records which of them it was, so an
/// auditor can see the basis rather than infer it from a bare number.
#[derive(Debug, Clone)]
pub struct AbschlagInput {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Invoice sender — the Netzbetreiber.
    pub nb_mp_id: String,
    /// Invoice recipient — the Lieferant.
    pub lf_mp_id: String,
    /// The period the payment is on account of.
    pub period: SettlementPeriod,
    /// Commodity.
    pub sparte: Sparte,
    /// The **net** amount requested, in EUR.
    ///
    /// Net rather than gross: the tax is stated separately on the invoice, as
    /// §14 Abs. 4 Nr. 8 UStG requires of an Anzahlungsrechnung like any other.
    pub betrag_netto_eur: Decimal,
    /// How the amount was arrived at.
    pub grundlage: AbschlagGrundlage,
}

/// How an Abschlag's amount was arrived at.
///
/// Recorded rather than computed. The engine cannot check a forecast, but an
/// audit can ask which basis was used, and an invoice that answers "a share of
/// the prior Turnusrechnung" is defensible where a bare figure is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AbschlagGrundlage {
    /// A share of the previous settled period's invoice.
    Vorjahresverbrauch,
    /// A forecast of the period being paid for.
    Prognose,
    /// A figure fixed in the Lieferantenrahmenvertrag.
    Vereinbarung,
}

impl AbschlagGrundlage {
    /// Short label for the position text and the trace.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vorjahresverbrauch => "auf Basis des Vorjahresverbrauchs",
            Self::Prognose => "auf Basis einer Verbrauchsprognose",
            Self::Vereinbarung => "gemäß Vereinbarung im Lieferantenrahmenvertrag",
        }
    }
}

// ── MsbInput ──────────────────────────────────────────────────────────────────

/// Input for MSB (Messstellenbetreiber) invoice calculation.
///
/// Covers:
/// - **PID 31009** — MSB-Rechnung (MSB → NB / LF / ESA, monthly metering
///   service settlement; Strom only)
///
/// The NB bills the MSB for the metering service period.  Positions:
/// 1. Grundgebühr Messstellenbetrieb — flat monthly base fee × billing months.
/// 2. Messdienstleistung — optional per-period measurement service fee.
#[derive(Debug, Clone)]
pub struct MsbInput {
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// Invoice **sender** — the Messstellenbetreiber.
    ///
    /// PID 31009 is issued *by* the MSB in all seven of its Anwendungsfälle; it
    /// is never sent to one. See [`MsbRechnungsempfaenger`].
    pub msb_mp_id: String,
    /// Invoice **recipient** — who is billed, and in which market role.
    pub empfaenger: MsbRechnungsempfaenger,
    /// The delivery period being settled.
    pub period: SettlementPeriod,
    /// The Sparte of the metering point.
    ///
    /// §30 MsbG prices metering, not energy, so this changes no arithmetic — but
    /// it is what the invoice states, and a service that stores one Sparte on
    /// the draft while the settlement carries another cannot answer which is
    /// right.
    pub sparte: Sparte,
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

/// Recipient of a MSB-Rechnung (PID 31009).
///
/// The *Anwendungsübersicht der Prüfidentifikatoren* 4.0 (01.04.2026) lists
/// seven Anwendungsfälle for 31009, and the sender is the **MSB** in every one:
///
/// | Prozessbeschreibung | von | an |
/// |---|---|---|
/// | GPKE Teil 3 | MSB | NB |
/// | GPKE Teil 3 | MSB | LF |
/// | WiM Strom Teil 1 | MSB (am Objekt Marktlokation) | LF |
/// | WiM Strom Teil 1 | MSB (am Objekt Marktlokation) | NB |
/// | WiM Strom Teil 2 | MSB | ESA |
/// | AWH Prozesse zur Änderung der Technik an Lokationen | MSB | NB |
/// | AWH Prozesse zur Änderung der Technik an Lokationen | MSB | LF |
///
/// So the recipient varies across three market roles while the sender does not,
/// which is why it is modelled as a role plus an MP-ID rather than a bare
/// `nb_mp_id`. 31009 is Strom-only — the overview marks Sparte Gas `--`.
///
/// Distinct from [`crate::msbg::Entgeltschuldner`], which selects the §30 MsbG
/// Preisobergrenze (whose *share* of the ceiling applies) rather than who
/// receives the invoice. The LF commonly receives an invoice for the
/// Letztverbraucher share under the Rechnungsabwicklung über den LF, so the two
/// axes do not coincide.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MsbRechnungsempfaenger {
    /// Which market role receives the invoice.
    pub rolle: MsbEmpfaengerRolle,
    /// The recipient's 13-digit MP-ID.
    pub mp_id: String,
}

/// Market role a MSB-Rechnung (PID 31009) may be addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MsbEmpfaengerRolle {
    /// Netzbetreiber — GPKE Teil 3, WiM Strom Teil 1, AWH Technikänderung.
    Netzbetreiber,
    /// Lieferant — GPKE Teil 3, WiM Strom Teil 1, AWH Technikänderung
    /// (Rechnungsabwicklung des MSB über den LF).
    Lieferant,
    /// Energieserviceanbieter — WiM Strom Teil 2.
    Energieserviceanbieter,
}

impl MsbEmpfaengerRolle {
    /// BDEW role code as it appears in the NAD segment.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Netzbetreiber => "NB",
            Self::Lieferant => "LF",
            Self::Energieserviceanbieter => "ESA",
        }
    }
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// A factor arriving over the wire is range-checked, not merely parsed.
    ///
    /// The whole point of the newtype is that an out-of-range value cannot
    /// exist; a derived `Deserialize` would have let one in through a request
    /// body and multiplied the Arbeitspreis by it.
    #[test]
    fn a_wire_reduktionsfaktor_is_range_checked() {
        // A `Decimal` is a JSON string on the wire, so the factor is too — a
        // float cannot carry 0.85 exactly and this one multiplies a tariff.
        let ok: Reduktionsfaktor = serde_json::from_str(r#""0.85""#).expect("in range");
        assert_eq!(ok.get(), dec!(0.85));

        for bad in [r#""0""#, r#""-0.5""#, r#""1.01""#, r#""5""#] {
            assert!(
                serde_json::from_str::<Reduktionsfaktor>(bad).is_err(),
                "{bad} must be refused"
            );
        }
        // The boundary is inclusive at 1 — no reduction is still a valid factor.
        assert!(serde_json::from_str::<Reduktionsfaktor>(r#""1""#).is_ok());
        // A bare number is refused before the range is even considered.
        assert!(serde_json::from_str::<Reduktionsfaktor>("0.85").is_err());
    }

    /// The Arbeitspreis model round-trips, so a settlement input can be stored
    /// and recomputed rather than a rendered document being edited in place.
    #[test]
    fn the_arbeitspreis_model_round_trips() {
        let model = ArbeitspreisModell::Modul3ZeitVariabel {
            ht: MengePreis {
                menge_kwh: dec!(600),
                preis_ct_per_kwh: dec!(4.2),
            },
            st: MengePreis {
                menge_kwh: dec!(100),
                preis_ct_per_kwh: dec!(3.0),
            },
            nt: MengePreis {
                menge_kwh: dec!(400),
                preis_ct_per_kwh: dec!(1.5),
            },
        };
        let json = serde_json::to_string(&model).expect("serialize");
        let back: ArbeitspreisModell = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, model);
    }

    use super::*;
    use rust_decimal::dec;

    /// A reduction factor outside `(0, 1]` cannot be built.
    ///
    /// The type is the check. A bare `Decimal` range-checked in a validator
    /// leaves `settle_nne` free to multiply the published tariff by 5 whenever
    /// the engine does not call it.
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
    /// The Konzessionsabgabe and the three network levies are all charged on it,
    /// so they read one figure rather than each deriving its own.
    #[test]
    fn every_model_reports_the_energy_it_priced() {
        let flat = ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: dec!(1000),
            preis_ct_per_kwh: dec!(3.5),
        });
        assert_eq!(flat.menge_kwh(), dec!(1000));

        let tou = ArbeitspreisModell::Modul3ZeitVariabel {
            ht: MengePreis {
                menge_kwh: dec!(600),
                preis_ct_per_kwh: dec!(4.0),
            },
            st: MengePreis {
                menge_kwh: dec!(0),
                preis_ct_per_kwh: dec!(0),
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
            pauschale_eur_pro_jahr: dec!(120),
            jahresanteil: dec!(1) / dec!(12),
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
                    pauschale_eur_pro_jahr: dec!(120),
                    jahresanteil: dec!(1) / dec!(12),
                },
                Some(M::Modul1),
            ),
            (
                ArbeitspreisModell::Modul3ZeitVariabel {
                    ht: MengePreis {
                        menge_kwh: dec!(1),
                        preis_ct_per_kwh: dec!(1),
                    },
                    st: MengePreis {
                        menge_kwh: dec!(0),
                        preis_ct_per_kwh: dec!(0),
                    },
                    nt: MengePreis {
                        menge_kwh: dec!(1),
                        preis_ct_per_kwh: dec!(1),
                    },
                },
                Some(M::Modul3),
            ),
            (
                // Not a §14a module at all — BK6-22-300 defines exactly three,
                // none of them spot-linked.
                ArbeitspreisModell::SpotpreisNetzentgelt { intervalle: vec![] },
                None,
            ),
        ];
        for (model, expected) in cases {
            assert_eq!(model.sect14a_modul(), expected);
        }
    }

    /// BK6-22-300 numbers the three modules in a specific way, and this project
    /// had them shuffled: the time-variable model was labelled Modul 2 and a
    /// spot-linked Netzentgelt was labelled Modul 3.
    ///
    /// Getting this wrong prints the wrong statutory module on a real invoice
    /// and makes the LF-side and NB-side engines disagree about the same
    /// connection, so it is pinned here rather than left to a doc comment.
    #[test]
    fn the_modules_are_numbered_as_bk6_22_300_defines_them() {
        use Sect14aModule as M;
        assert!(M::Modul1.label().contains("pauschale Reduzierung"));
        assert!(
            M::Modul2
                .label()
                .contains("prozentuale Arbeitspreisreduzierung")
        );
        assert!(M::Modul3.label().contains("zeitvariable Netzentgelte"));
    }

    /// `Modul 1 + Modul 3` is the only pair BK6-22-300 offers.
    ///
    /// Modul 1 and Modul 2 are the two forms of the *base* module and the
    /// Anschlussnutzer picks one; Modul 3 adds to the pauschale Modul 1 and not
    /// to Modul 2, which re-prices the same Arbeitspreis.
    #[test]
    fn modul_1_and_modul_3_are_the_only_combination() {
        use Sect14aModule as M;
        assert!(M::Modul1.combinable_with(M::Modul3));
        assert!(M::Modul3.combinable_with(M::Modul1));

        assert!(!M::Modul2.combinable_with(M::Modul3));
        assert!(!M::Modul3.combinable_with(M::Modul2));
        assert!(
            !M::Modul1.combinable_with(M::Modul2),
            "Modul 2 is an alternative to Modul 1, not an addition to it"
        );
        assert!(!M::Modul2.combinable_with(M::Modul1));

        for m in [M::Modul1, M::Modul2, M::Modul3] {
            assert!(!m.combinable_with(m), "{m:?} with itself is not a pair");
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

#[cfg(test)]
mod korrektur_grund_tests {
    use super::*;

    /// A correction that cannot say why it happened is not an audit trail.
    ///
    /// The invoice numbers answer *what* was replaced; only the reason
    /// distinguishes a lawful retroactive recalculation from a defect in the
    /// original settlement, and those have different consequences.
    #[test]
    fn a_reason_is_required_for_every_recalculation() {
        let mut r = sample_result(SettlementStatus::Initial, None);
        assert!(
            r.lineage_is_consistent(),
            "an initial settlement corrects nothing"
        );

        r.status = SettlementStatus::Correction;
        assert!(
            !r.lineage_is_consistent(),
            "a correction with no reason must be detectable"
        );

        r.korrektur_grund = Some(KorrekturGrund::Tarifkorrektur);
        assert!(r.lineage_is_consistent());
    }

    /// An initial settlement carrying a correction reason is equally inconsistent.
    #[test]
    fn an_initial_settlement_carries_no_reason() {
        let r = sample_result(
            SettlementStatus::Initial,
            Some(KorrekturGrund::Rechenfehler),
        );
        assert!(!r.lineage_is_consistent());
    }

    /// Separating defects from lawful recalculations is the point of the field:
    /// a rising Rechenfehler count is an engineering signal, a rising
    /// RegulatorischeAenderung count is not.
    #[test]
    fn only_some_reasons_indicate_a_defect() {
        assert!(KorrekturGrund::Rechenfehler.indicates_defect());
        assert!(KorrekturGrund::Stammdatenkorrektur.indicates_defect());
        assert!(!KorrekturGrund::RegulatorischeAenderung.indicates_defect());
        assert!(!KorrekturGrund::Messwertkorrektur.indicates_defect());
        assert!(!KorrekturGrund::Clearing.indicates_defect());
    }

    /// The codes are stable — they reach reporting and structured records.
    #[test]
    fn the_codes_are_stable() {
        assert_eq!(
            KorrekturGrund::Messwertkorrektur.code(),
            "MESSWERTKORREKTUR"
        );
        assert_eq!(
            KorrekturGrund::RegulatorischeAenderung.code(),
            "REGULATORISCHE_AENDERUNG"
        );
    }

    fn sample_result(
        status: SettlementStatus,
        korrektur_grund: Option<KorrekturGrund>,
    ) -> SettlementResult {
        SettlementResult {
            settlement_type: SettlementType::NneStrom,
            status,
            korrektur_grund,
            period: SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            regime: crate::regulatory::RegulatoryRegime::for_period(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            ),
            sparte: Sparte::Strom,
            malo_id: "51238696012".to_owned(),
            sender_mp_id: "9900000000001".to_owned(),
            recipient_mp_id: "9900000000002".to_owned(),
            positions: Vec::new(),
            total_eur: rust_decimal::Decimal::ZERO,
            steuer: crate::umsatzsteuer::Steuerausweis {
                kategorie: crate::umsatzsteuer::TaxCategory::Standard,
                satz_prozent: crate::umsatzsteuer::REGELSTEUERSATZ,
                bemessungsgrundlage_eur: rust_decimal::Decimal::ZERO,
                steuer_eur: rust_decimal::Decimal::ZERO,
                hinweis: None,
                rechtsgrundlage: "§12 Abs. 1 UStG",
            },
            warnings: Vec::new(),
        }
    }
}

#[cfg(test)]
mod blindarbeit_tests {
    use super::*;
    use rust_decimal::dec;

    /// cos φ 0,9 is the customary boundary: reactive energy up to tan φ ≈ 0,4843
    /// of the active energy travels with it and is not charged.
    #[test]
    fn draw_inside_the_free_share_costs_nothing() {
        let b = Blindarbeit {
            blindarbeit_kvarh: dec!(400),
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            preis_ct_per_kvarh: dec!(2.0),
        };
        // 1 000 kWh × 0,4843 = 484,3 kvarh free; 400 stays inside it.
        assert_eq!(b.mehrarbeit_kvarh(dec!(1000)), Decimal::ZERO);
    }

    /// Only the excess is chargeable.
    #[test]
    fn only_the_excess_is_charged() {
        let b = Blindarbeit {
            blindarbeit_kvarh: dec!(600),
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            preis_ct_per_kvarh: dec!(2.0),
        };
        assert_eq!(b.mehrarbeit_kvarh(dec!(1000)), dec!(115.7));
    }

    /// An unused allowance is not a credit — the excess floors at zero.
    #[test]
    fn an_unused_allowance_is_never_negative() {
        let b = Blindarbeit {
            blindarbeit_kvarh: dec!(10),
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            preis_ct_per_kvarh: dec!(2.0),
        };
        assert_eq!(b.mehrarbeit_kvarh(dec!(5000)), Decimal::ZERO);
    }

    /// The share is a term of the Preisblatt, not a constant: many networks
    /// round cos φ 0,9 to a flat 50 %, and billing them at 0,4843 overcharges.
    #[test]
    fn the_free_share_follows_the_preisblatt() {
        let rounded = Blindarbeit {
            blindarbeit_kvarh: dec!(600),
            freigrenze_anteil: dec!(0.5),
            preis_ct_per_kvarh: dec!(2.0),
        };
        assert_eq!(rounded.mehrarbeit_kvarh(dec!(1000)), dec!(100.0));

        let exact = Blindarbeit {
            freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
            ..rounded
        };
        assert!(
            exact.mehrarbeit_kvarh(dec!(1000)) > rounded.mehrarbeit_kvarh(dec!(1000)),
            "the tighter cos φ 0,9 share charges more than a rounded 50 %"
        );
    }

    /// Blindmehrarbeit rests on the Netzbetreiber's Preisblatt under StromNEV
    /// §17 — not §18 (dezentrale Erzeugung) and not §19 (Sonderformen).
    #[test]
    fn the_position_kind_maps_to_the_bdew_artikelnummer() {
        assert_eq!(
            BillingPositionKind::Blindmehrarbeit.artikelnummer(SettlementType::NneStrom),
            Some("BLINDMEHRARBEIT")
        );
    }
}
