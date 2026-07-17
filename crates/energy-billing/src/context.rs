//! `BillingContext` — the immutable billing metadata passed to every provider.
//!
//! Separates *what we're billing* (quantities, products) from *how we're billing it*
//! (period, identifiers, invoice type, regulatory rates).

use rust_decimal::Decimal;

use crate::rates::RegulatoryRates;

// ── Verbrauchshistorie ───────────────────────────────────────────────────────────

/// §41 EnWG Abs. 1 Nr. 3 — Verbrauchshistorie (consumption history for invoice display).
///
/// German energy invoices must compare the billed period consumption against
/// the same period in the prior year and the national average for comparable
/// customers. This is an **invoice display requirement**, not a calculation input.
///
/// ## Legal basis
///
/// §41 Abs. 1 Nr. 3 EnWG: “der tatsächliche Energieverbrauch sowie — soweit technisch möglich
/// und sinnvoll — ein Vergleich des aktuellen Energieverbrauchs des Letztverbrauchers mit
/// seinem Verbrauch im gleichen Zeitraum des Vorjahres … und dem Verbrauch einer
/// Vergleichsgruppe von Letztverbrauchern.”
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Verbrauchshistorie {
    /// Consumption in the same period of the prior year (kWh). §41 Abs. 1 Nr. 3a EnWG.
    #[serde(default)]
    pub vorjahr_kwh: Option<Decimal>,
    /// National average consumption for comparable customers (kWh). §41 Abs. 1 Nr. 3b EnWG.
    #[serde(default)]
    pub bundesdurchschnitt_kwh: Option<Decimal>,
    /// Description of the comparable customer group (e.g. `"2-Personen-Haushalt"`).
    #[serde(default)]
    pub kundengruppe: Option<String>,
}

// ── InvoiceType ───────────────────────────────────────────────────────────────

/// Whether this is an initial invoice, a correction, a cancellation, or a final settlement.
///
/// German energy suppliers frequently perform:
/// ```text
/// Initial invoice  →  Correction (corrected meter reading)
///                  →  Cancellation (full reversal)
///                  →  Final (annual Schlussabrechnung)
/// ```
///
/// ## §22 MessZV compliance
///
/// Corrections must reference the original invoice ID for the 3-year audit trail.
/// Cancellations reverse the original to EUR 0.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InvoiceType {
    /// Standard billing run (Abschlagsrechnung, periodic invoice).
    Initial,

    /// Credit note (Gutschrift) — outgoing payment to a third party.
    ///
    /// Used for:
    /// - EEG feed-in settlement (payment to generator)
    /// - EINSPEISUNG Direktvermarktung settlement
    /// - Reverse-charge scenarios
    ///
    /// `rechnungsart` = `"GUTSCHRIFT"`
    CreditNote,

    /// Correction superseding an earlier invoice (§22 MessZV).
    ///
    /// The original invoice must be referenced in the accounting system.
    /// The net effect is: `original + correction = corrected total`.
    Correction {
        /// ID of the original invoice this corrects.
        original_invoice_id: String,
        /// Human-readable reason (for audit trail).
        reason: Option<String>,
    },

    /// Full reversal of an earlier invoice (Stornorechnung).
    ///
    /// All positions are sign-inverted to bring the original to EUR 0.
    Cancellation {
        /// ID of the original invoice being cancelled.
        original_invoice_id: String,
    },

    /// Annual final settlement (Schlussabrechnung / Jahresabrechnung).
    ///
    /// Reconciles advance payments against measured consumption.
    /// Include paid Abschläge in `BillingContext::abschlage` — they will be
    /// deducted from `Invoice::zahlbetrag_eur`.
    Final,

    /// Advance payment request (Abschlagsrechnung).
    ///
    /// Use this for **estimated** periodic billing where no final meter reading
    /// is available yet. The customer pays on account; the annual settlement
    /// (`InvoiceType::Final`) reconciles the difference.
    ///
    /// BO4E `rechnungsart` = `"ABSCHLAGSRECHNUNG"`
    ///
    /// ## Distinction from `Initial`
    ///
    /// `Initial` represents billing for **actual metered consumption** — it maps
    /// to `"RECHNUNG"`. `AdvancePayment` represents **estimated advance payments**
    /// that will be settled annually.
    AdvancePayment,

    /// Partial delivery invoice (Teilrechnung) for incomplete supply periods.
    ///
    /// Used when a customer switches supplier mid-period, moves in/out, or when a
    /// meter replacement creates a split period. The departing or arriving supplier
    /// issues a Teilrechnung for the exact days of actual supply.
    ///
    /// ## Legal basis
    ///
    /// §41 EnWG Abs. 1: the invoice must cover the actual supply period.
    /// StromGVV §17 / GasGVV §14: Lieferungsende is billed on the day of change.
    ///
    /// `rechnungsart` = `"TEILRECHNUNG"`
    PartialInvoice,
}

impl InvoiceType {
    /// BO4E Rechnungsart string for the rechnung_json field.
    #[must_use]
    pub fn rechnungsart(&self) -> &'static str {
        match self {
            Self::Initial => "RECHNUNG",
            Self::AdvancePayment => "ABSCHLAGSRECHNUNG",
            Self::CreditNote => "GUTSCHRIFT",
            Self::Correction { .. } => "KORREKTURRECHNUNG",
            Self::Cancellation { .. } => "STORNORECHNUNG",
            Self::Final => "SCHLUSSRECHNUNG",
            Self::PartialInvoice => "TEILRECHNUNG",
        }
    }

    /// Returns the original invoice ID for corrections and cancellations.
    #[must_use]
    pub fn original_invoice_id(&self) -> Option<&str> {
        match self {
            Self::Correction {
                original_invoice_id,
                ..
            }
            | Self::Cancellation {
                original_invoice_id,
            } => Some(original_invoice_id),
            _ => None,
        }
    }

    /// `true` when this invoice reverses all positions of the original.
    #[must_use]
    pub fn is_reversal(&self) -> bool {
        matches!(self, Self::Cancellation { .. })
    }
}

#[allow(clippy::derivable_impls)]
impl Default for InvoiceType {
    fn default() -> Self {
        Self::Initial
    }
}

// ── CustomerKategorie ─────────────────────────────────────────────────────────

/// Customer category for the delivery point.
///
/// Determines applicable tariff categories, regulatory exemptions, and invoice
/// disclosure requirements. Affects Stromsteuer (§9 Nr. 1 StromStG industrial
/// exemption threshold), Preisangabenverordnung, and §41 EnWG disclosure depth.
///
/// ## Legal basis
///
/// - §2 Nr. 4 StromStG — definition of "Unternehmen des produzierenden Gewerbes"
/// - §4 MessZV / §14 NAV — RLM metering thresholds
/// - §41 Abs. 1 EnWG — invoice disclosure requirements vary by customer type
/// - StromGVV vs. StromNZV — different contract law for B2C vs B2B
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CustomerKategorie {
    /// Household customer (Haushaltskunde, §2 Nr. 25 EnWG).
    ///
    /// B2C. StromGVV / GasGVV apply. §40 EnWG Kilowattstundenpreis mandatory.
    /// Invoice must include Verbrauchshistorie (§41 Abs. 1 Nr. 3 EnWG).
    #[default]
    Haushalt,

    /// Small commercial customer (Gewerbekunde, not a household but not RLM-obligated).
    ///
    /// B2B < 100 MWh/year. StromGVV / GasGVV still apply in most cases.
    /// May be on SLP or transitioning to iMSys.
    Gewerbe,

    /// Industrial / large commercial customer (Sonderkunde).
    ///
    /// B2B ≥ 100 MWh/year electricity (§4 MessZV), RLM mandatory.
    /// StromNZV applies (not StromGVV). Eligible for §9 Nr. 1–3 StromStG
    /// industrial exemption, KWKG Selbstbehaltsgrenze, and capacity pricing.
    Industrie,

    /// Agricultural customer (Landwirtschaft).
    ///
    /// Special BEHG/Energiesteuer treatment may apply for agricultural use.
    /// §2 Abs. 1 Nr. 4 UStG (7% reduced VAT on certain agricultural inputs).
    Landwirtschaft,

    /// Public authority / public transport (öffentliche Einrichtung).
    ///
    /// May qualify for Konzessionsabgabe exemption (§2 Abs. 7 KAV).
    OeffentlicheEinrichtung,
}

impl CustomerKategorie {
    /// Whether this customer category typically uses SLP billing.
    #[must_use]
    pub fn is_slp_customer(self) -> bool {
        matches!(self, Self::Haushalt | Self::Gewerbe)
    }

    /// Whether the annual Verbrauchshistorie (§41 Abs. 1 Nr. 3 EnWG) applies.
    ///
    /// Mandatory for household customers (B2C). Recommended for Gewerbe.
    /// Not required for industrial / RLM customers.
    #[must_use]
    pub fn requires_verbrauchshistorie(self) -> bool {
        matches!(self, Self::Haushalt)
    }

    /// Whether the §40a EnWG Kilowattstundenpreis must appear on the invoice.
    ///
    /// Mandatory for all non-RLM electricity customers.
    #[must_use]
    pub fn requires_kilowattstundenpreis(self) -> bool {
        !matches!(self, Self::Industrie)
    }
}

// ── AbschlagDeduction ─────────────────────────────────────────────────────────

/// An advance payment (Abschlag) previously collected from the customer.
///
/// Include these in `BillingContext::abschlage` for `InvoiceType::Final`
/// (Jahresabrechnung) to deduct prior payments from the final amount due.
///
/// ## §41 EnWG
///
/// The annual final settlement must show each advance payment date and amount
/// so the customer can verify the reconciliation.
///
/// ## Example
///
/// A customer paying EUR 120/month → 12 × EUR 120 = EUR 1 440 in advances.
/// If consumption bill = EUR 1 600, Zahlbetrag = EUR 160 (balance due).
/// If consumption bill = EUR 1 300, Zahlbetrag = EUR -140 (refund).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AbschlagDeduction {
    /// Payment date (shown on the invoice for §41 EnWG compliance).
    pub datum: time::Date,
    /// EUR amount already paid (positive = customer paid this amount).
    pub betrag_eur: Decimal,
    /// Optional description shown on invoice (e.g. `"Abschlag März 2026"`).
    #[serde(default)]
    pub beschreibung: Option<String>,
}

// ── BillingContext ────────────────────────────────────────────────────────────

/// Immutable billing metadata — the *context* for one invoice generation run.
///
/// Every [`BillingProvider`][crate::BillingProvider] receives a reference to
/// the same `BillingContext` so all positions share identical period, party IDs,
/// and regulatory rates.
///
/// ## New in this version
///
/// - `vertragsbeginn` / `vertragsende` — enables automatic pro-rata billing
///   when a contract starts or ends mid-period
/// - `zaehler_id` — §41 EnWG Zählernummer on invoice
/// - `abschlage` — advance payments deducted in `Invoice::zahlbetrag_eur`
///   (required for `InvoiceType::Final` / Jahresabrechnung)
///
/// ## Example
///
/// ```rust
/// use energy_billing::{BillingContext, InvoiceType, RegulatoryRates, AbschlagDeduction};
/// use time::macros::date;
/// use rust_decimal_macros::dec;
///
/// let ctx = BillingContext {
///     malo_id: "51238696781".to_owned(),
///     lf_mp_id: "9900000000001".to_owned(),
///     rechnungsnummer: "R2026-001".to_owned(),
///     period_from: date!(2026-01-01),
///     period_to: date!(2026-12-31),
///     invoice_type: InvoiceType::Final,
///     regulatory_rates: RegulatoryRates::default(),
///     contract_id: None,
///     abschlage: vec![
///         AbschlagDeduction {
///             datum: date!(2026-01-15),
///             betrag_eur: dec!(120.00),
///             beschreibung: Some("Abschlag Januar 2026".to_owned()),
///         },
///     ],
///     ..Default::default()
/// };
/// assert_eq!(ctx.total_abschlage_eur(), dec!(120.00));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BillingContext {
    /// 11-digit Marktlokations-ID of the delivery point.
    pub malo_id: String,

    /// BDEW/DVGW Codenummer of the Lieferant (invoice issuer).
    pub lf_mp_id: String,

    /// Invoice number (Rechnungsnummer) — unique per invoice.
    ///
    /// Operator's responsibility to ensure uniqueness. Recommended format:
    /// `{prefix}-{year}-{sequence}` (e.g. `"INV-2026-000001"`).
    pub rechnungsnummer: String,

    /// First day of the billing period (inclusive).
    pub period_from: time::Date,

    /// Last day of the billing period (inclusive).
    pub period_to: time::Date,

    /// Invoice type: initial, correction, cancellation, or final settlement.
    pub invoice_type: InvoiceType,

    /// Statutory levy rates (Stromsteuer, Energiesteuer, BEHG, MwSt).
    ///
    /// Sourced from `billingd.toml [rates]` — never hardcoded in the library.
    pub regulatory_rates: RegulatoryRates,

    /// Optional contract reference (for LF internal use / ERP routing).
    #[serde(default)]
    pub contract_id: Option<String>,

    /// Contract start date (§41 EnWG).
    ///
    /// When set AND `period_from < vertragsbeginn`, `billing_days_fraction()`
    /// returns a value < 1.0 for pro-rata first-month billing.
    #[serde(default)]
    pub vertragsbeginn: Option<time::Date>,

    /// Contract end date.
    ///
    /// When set AND `period_to > vertragsende`, `billing_days_fraction()`
    /// returns a value < 1.0 for pro-rata last-month billing.
    #[serde(default)]
    pub vertragsende: Option<time::Date>,

    /// Zählernummer (§41 EnWG — mandatory on electricity invoices).
    ///
    /// Appears on the invoice as an informational line item.
    #[serde(default)]
    pub zaehler_id: Option<String>,

    /// Advance payments to deduct from the final invoice (Jahresabrechnung).
    ///
    /// Used exclusively with `InvoiceType::Final`. Each entry produces an
    /// `Abschlag` deduction line in `Invoice::zahlbetrag_eur`.
    ///
    /// The German retail practice: monthly advance payments are collected
    /// throughout the year; the annual settlement debits/credits the difference.
    #[serde(default)]
    pub abschlage: Vec<AbschlagDeduction>,

    /// §41 EnWG Abs. 1 Nr. 3 — Verbrauchshistorie for invoice display.
    ///
    /// When set, appears as informational ZusatzAttribute in the Rechnung JSON
    /// showing the customer's consumption history vs. prior year and average.
    #[serde(default)]
    pub verbrauchshistorie: Option<Verbrauchshistorie>,

    /// §41 EnWG Abs. 1 Nr. 8 + §42 EnWG — Energiemix description.
    ///
    /// Must appear on electricity invoices. Can be the product's certified mix
    /// from a Herkunftsnachweis (HKN) or the national residual mix (Restmix).
    /// Injected as a `ZusatzAttribut` with name `"energiemix"`.
    ///
    /// ## Example
    /// `"100% Ernäuerbarer Energien (EE-Strom, HKN-zertifiziert, Österreich)"`
    #[serde(default)]
    pub energiemix: Option<String>,

    /// Minimum invoice amount (brutto) in EUR.
    ///
    /// When set and the computed `brutto_eur < minimum_invoice_eur_brutto`, the
    /// engine adds a `Mindestbetrag` position to reach the minimum.
    ///
    /// Set from `TariffInput.minimum_invoice_eur_brutto` by the service layer
    /// (`billingd`) when building the billing context.
    ///
    /// ## Use case
    ///
    /// B2B contracts with a minimum annual consumption commitment
    /// (Mindestabnahmeverpflichtung). The customer pays at least this amount
    /// per billing period regardless of actual consumption.
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,

    /// BDEW-Codenummer of the Netzbetreiber (§41 EnWG — mandatory on invoices).
    ///
    /// German energy invoices must identify the network operator who provides
    /// the grid infrastructure at the delivery point (§41 Abs. 1 Nr. 5 EnWG).
    /// This appears as `"netzbetreiber"."marktpartnercode"` in the Rechnung JSON.
    ///
    /// When `None`, the `netzbetreiber` field is omitted from the invoice JSON.
    /// For full §41 EnWG compliance on retail electricity/gas invoices, always set this.
    #[serde(default)]
    pub nb_mp_id: Option<String>,

    /// Unique billing run identifier for audit trail and duplicate detection.
    ///
    /// When set, propagated to `Invoice.billing_run_id` and included in the
    /// Rechnung JSON as a `ZusatzAttribut` under key `"billingRunId"`.
    ///
    /// Use a UUID v4 generated by `billingd` at invoice time to correlate the
    /// database record (`billing_records.id`) with calculation outputs.
    #[serde(default)]
    pub billing_run_id: Option<String>,

    /// Customer category — drives regulatory exemptions and invoice disclosure.
    ///
    /// | Category | SLP | Verbrauchshistorie | §40a kWh-Preis |
    /// |---|---|---|---|
    /// | `Haushalt` | ✅ | Mandatory | Mandatory |
    /// | `Gewerbe` | ✅ | Recommended | Mandatory |
    /// | `Industrie` | ❌ (RLM) | — | — |
    /// | `Landwirtschaft` | ✅ | Recommended | Mandatory |
    /// | `OeffentlicheEinrichtung` | ✅/❌ | — | Mandatory |
    ///
    /// Defaults to `Haushalt` — always set explicitly for B2B customers.
    #[serde(default)]
    pub kundenkategorie: CustomerKategorie,
}

impl Default for BillingContext {
    fn default() -> Self {
        Self {
            malo_id: String::new(),
            lf_mp_id: String::new(),
            rechnungsnummer: String::new(),
            period_from: time::Date::MIN,
            period_to: time::Date::MIN,
            invoice_type: InvoiceType::default(),
            regulatory_rates: RegulatoryRates::default(),
            contract_id: None,
            vertragsbeginn: None,
            vertragsende: None,
            zaehler_id: None,
            abschlage: Vec::new(),
            verbrauchshistorie: None,
            energiemix: None,
            minimum_invoice_eur_brutto: None,
            nb_mp_id: None,
            billing_run_id: None,
            kundenkategorie: CustomerKategorie::default(),
        }
    }
}

impl BillingContext {
    /// Number of calendar days in the billing period.
    ///
    /// Used for Grundpreis (daily rate × days) and pro-rata calculations.
    #[must_use]
    pub fn days(&self) -> i64 {
        let diff = self.period_to - self.period_from;
        diff.whole_days() + 1 // inclusive of both endpoints
    }

    /// Pro-rata fraction of the billing period actually billable.
    ///
    /// Returns `None` when the full period is billable (no pro-rata applies).
    /// Returns `Some(fraction)` where `0 < fraction < 1` when:
    /// - `vertragsbeginn` falls within the period (late contract start)
    /// - `vertragsende` falls within the period (early contract end)
    ///
    /// ## §41 EnWG — pro-rata billing
    ///
    /// First and last billing periods are prorated to the actual contract days.
    ///
    /// # Example
    ///
    /// ```rust
    /// use energy_billing::{BillingContext, InvoiceType, RegulatoryRates};
    /// use time::macros::date;
    ///
    /// let ctx = BillingContext {
    ///     period_from:    date!(2026-01-01),
    ///     period_to:      date!(2026-01-31),  // 31 days
    ///     vertragsbeginn: Some(date!(2026-01-16)), // contract started mid-month
    ///     ..Default::default()
    /// };
    /// let frac = ctx.billing_days_fraction().unwrap();
    /// // 16 billable days out of 31: ≈ 0.516
    /// assert!(frac > rust_decimal_macros::dec!(0.50) && frac < rust_decimal_macros::dec!(0.55));
    /// ```
    #[must_use]
    pub fn billing_days_fraction(&self) -> Option<Decimal> {
        let period_days = self.days();
        if period_days <= 0 {
            return None;
        }

        // Effective start: max(period_from, vertragsbeginn)
        let effective_from = match self.vertragsbeginn {
            Some(vb) if vb > self.period_from => vb,
            _ => self.period_from,
        };

        // Effective end: min(period_to, vertragsende)
        let effective_to = match self.vertragsende {
            Some(ve) if ve < self.period_to => ve,
            _ => self.period_to,
        };

        let billable = (effective_to - effective_from).whole_days() + 1;
        if billable <= 0 {
            return None;
        }
        if billable >= period_days {
            return None; // full period, no pro-rata
        }

        let frac = Decimal::from(billable) / Decimal::from(period_days);
        Some(frac.round_dp(6))
    }

    /// Total advance payments included in this context.
    ///
    /// For `InvoiceType::Final`, this equals the amount deducted from
    /// `Invoice::zahlbetrag_eur`.
    #[must_use]
    pub fn total_abschlage_eur(&self) -> Decimal {
        self.abschlage.iter().map(|a| a.betrag_eur).sum()
    }

    /// Return `(active_days, total_days)` for use with `billing::prorate` /
    /// `billing::prorate_amount`.
    ///
    /// - `total_days` = calendar days in the billing period (`days()`)
    /// - `active_days` = billable days after clipping to `vertragsbeginn` /
    ///   `vertragsende`
    ///
    /// When no pro-rata applies (full period billable), `active_days == total_days`.
    /// When the period would yield zero billable days, returns `(0, total_days)`.
    ///
    /// ## Example — Grundpreis pro-rata
    ///
    /// ```rust
    /// # use energy_billing::BillingContext;
    /// # use time::macros::date;
    /// let ctx = BillingContext {
    ///     period_from: date!(2026-01-01),
    ///     period_to:   date!(2026-01-31),
    ///     vertragsbeginn: Some(date!(2026-01-16)),
    ///     ..Default::default()
    /// };
    /// let (active, total) = ctx.prorate_days();
    /// assert_eq!(total, 31);
    /// assert_eq!(active, 16); // Jan 16–31
    /// ```
    #[must_use]
    pub fn prorate_days(&self) -> (u32, u32) {
        let total = self.days().max(0) as u32;
        if total == 0 {
            return (0, 1);
        }
        // Effective start: max(period_from, vertragsbeginn)
        let effective_from = self
            .vertragsbeginn
            .filter(|&vb| vb > self.period_from)
            .unwrap_or(self.period_from);
        // Effective end: min(period_to, vertragsende)
        let effective_to = self
            .vertragsende
            .filter(|&ve| ve < self.period_to)
            .unwrap_or(self.period_to);
        let active = ((effective_to - effective_from).whole_days() + 1).max(0) as u32;
        (active.min(total), total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use time::macros::date;

    fn base_ctx() -> BillingContext {
        BillingContext {
            period_from: date!(2026 - 01 - 01),
            period_to: date!(2026 - 01 - 31),
            ..Default::default()
        }
    }

    #[test]
    fn days_full_january() {
        assert_eq!(base_ctx().days(), 31);
    }

    #[test]
    fn billing_days_fraction_no_pro_rata_returns_none() {
        assert!(base_ctx().billing_days_fraction().is_none());
    }

    #[test]
    fn billing_days_fraction_mid_month_start() {
        let ctx = BillingContext {
            vertragsbeginn: Some(date!(2026 - 01 - 16)),
            ..base_ctx()
        };
        let frac = ctx.billing_days_fraction().unwrap();
        // billable: Jan 16..31 = 16 days out of 31
        let expected = Decimal::from(16) / Decimal::from(31);
        assert_eq!(frac, expected.round_dp(6));
    }

    #[test]
    fn billing_days_fraction_mid_month_end() {
        let ctx = BillingContext {
            vertragsende: Some(date!(2026 - 01 - 15)),
            ..base_ctx()
        };
        let frac = ctx.billing_days_fraction().unwrap();
        // billable: Jan 01..15 = 15 days out of 31
        let expected = Decimal::from(15) / Decimal::from(31);
        assert_eq!(frac, expected.round_dp(6));
    }

    #[test]
    fn total_abschlage_sums_correctly() {
        let ctx = BillingContext {
            abschlage: vec![
                AbschlagDeduction {
                    datum: date!(2026 - 01 - 15),
                    betrag_eur: dec!(100.00),
                    beschreibung: None,
                },
                AbschlagDeduction {
                    datum: date!(2026 - 02 - 15),
                    betrag_eur: dec!(120.00),
                    beschreibung: None,
                },
            ],
            ..base_ctx()
        };
        assert_eq!(ctx.total_abschlage_eur(), dec!(220.00));
    }
}
