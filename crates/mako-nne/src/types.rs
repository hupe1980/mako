//! Input/output types for the billing calculation functions.

use rubo4e::current::Rechnung;
use rust_decimal::Decimal;

// ── NneInput ──────────────────────────────────────────────────────────────────

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
    pub arbeitsmenge_kwh: Decimal,
    /// Published NNE Arbeitspreis in **ct/kWh** (from `PreisblattNetznutzung`).
    pub arbeitspreis_ct_per_kwh: Decimal,
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
    /// Actual measured consumption in kWh (from MSCONS / `MeterBillingPeriod`).
    pub actual_kwh: Decimal,
    /// Standard load profile (SLP) forecast consumption in kWh.
    pub profil_kwh: Decimal,
    /// Mehrmengen price in **ct/kWh** (from `PreisblattNetznutzung` MMM position).
    pub mehr_preis_ct_per_kwh: Decimal,
    /// Mindermengen price in **ct/kWh** (from `PreisblattNetznutzung` MMM position).
    pub minder_preis_ct_per_kwh: Decimal,
}

// ── BillingResult ─────────────────────────────────────────────────────────────

/// Result of a billing calculation.
#[derive(Debug, Clone)]
pub struct BillingResult {
    /// Generated BO4E `Rechnung` — ready for INVOIC serialization and
    /// `invoic-checker` validation.
    pub rechnung: Rechnung,
    /// BDEW Prüfidentifikator for this invoice type.
    ///
    /// - `31001` — NNE Strom
    /// - `31002` — Mehr-/Mindermengen Strom
    /// - `31005` — NNE Gas
    /// - `31009` — MSB-Rechnung (NB → MSB metering service settlement)
    pub pid: u32,
    /// Total net amount in EUR (sum of all billing positions, rounded to 5 decimal places).
    pub total_eur: Decimal,
    /// Sender MP-ID (for `invoic-checker` tariff lookups).
    pub nb_mp_id: String,
    /// Number of billing positions generated.
    pub positions_count: usize,
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
