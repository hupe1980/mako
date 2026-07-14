//! Input/output types for the billing calculation functions.

use rust_decimal::Decimal;

// ── QuantityUnit ──────────────────────────────────────────────────────────────────────────────

/// Unit of measure for an invoice position quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityUnit {
    /// Kilowatt-hours (energy).
    Kwh,
    /// Kilowatts (demand / peak load).
    Kw,
    /// Calendar months.
    Monat,
}

// ── InvoicePosition ───────────────────────────────────────────────────────────────────

/// One line item in a grid invoice.
///
/// Carries raw numbers for the service layer to map into the required format
/// (BO4E `Rechnungsposition`, EN16931 UBL, etc.).
/// Invariant: `net_eur == (quantity × unit_price_eur).round_dp(5)`.
#[derive(Debug, Clone)]
pub struct InvoicePosition {
    /// 1-based sequence number.
    pub number: u32,
    /// Human-readable position description.
    pub text: String,
    /// Metered or contracted quantity.
    pub quantity: Decimal,
    /// Unit of measure.
    pub unit: QuantityUnit,
    /// Unit price in EUR (already converted from ct where applicable).
    pub unit_price_eur: Decimal,
    /// Net amount in EUR, rounded to 5 decimal places.
    /// May be negative for credit positions (Mindermengen, Gutschriften).
    pub net_eur: Decimal,
}

// ── GridInvoice ──────────────────────────────────────────────────────────────────────────

/// Result of a grid invoice calculation — pure domain type, no BO4E coupling.
///
/// Call a local `into_rechnung()` helper in the service layer (netzbilanzd /
/// invoicd) to produce the `rubo4e::current::Rechnung` required for EDIFACT
/// serialization and `invoic-checker` validation.
///
/// # PID override
///
/// `pid` defaults to the primary PID for each function:
/// - `calculate_nne_invoice` → `31001` (caller sets `31005` for Gas, `31006` for selbstausstellt)
/// - `calculate_mmm_invoice` → `31002`
/// - `calculate_msb_invoice` → `31009`
/// - GeLi Gas AWH: caller overrides to `31011`
#[derive(Debug, Clone)]
pub struct GridInvoice {
    /// BDEW Prüfidentifikator — caller may override after construction.
    pub pid: u32,
    /// Unique invoice reference number.
    pub rechnungsnummer: String,
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
    /// Ordered billing positions.
    pub positions: Vec<InvoicePosition>,
    /// Net total in EUR, rounded to 2 decimal places.
    pub total_eur: Decimal,
}

impl GridInvoice {
    /// Number of billing positions.
    #[must_use]
    pub fn positions_count(&self) -> usize {
        self.positions.len()
    }
}

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
