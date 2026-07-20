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

// ── Vertragsinformationen ─────────────────────────────────────────────────────

/// §40 Abs. 1 EnWG — contract facts the invoice must state.
///
/// Vertragsdauer, Kündigungsfrist, the next possible Kündigungstermin and the
/// next Abrechnungstermin are invoice *contents*, not calculation inputs: they
/// change no amount, but an electricity or gas invoice without them is
/// incomplete under §40. Typed here so billingd can source them from vertragd
/// and the engine can emit them without either side inventing prose.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Vertragsinformationen {
    /// Contract term as displayed, e.g. `"24 Monate"` or `"unbefristet"`.
    #[serde(default)]
    pub vertragsdauer: Option<String>,
    /// Notice period as displayed, e.g. `"6 Wochen zum Vertragsende"`.
    #[serde(default)]
    pub kuendigungsfrist: Option<String>,
    /// Next date the customer could terminate to.
    #[serde(default)]
    pub naechstmoeglicher_kuendigungstermin: Option<time::Date>,
    /// Next scheduled Abrechnungstermin.
    #[serde(default)]
    pub naechster_abrechnungstermin: Option<time::Date>,
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
/// - Grundversorgung (StromGVV) vs. Sondervertrag — different contract law
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
    /// Sondervertrag, not Grundversorgung. Eligible for §9 Nr. 1–3 StromStG
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
/// ## §14 Abs. 5 Satz 2 UStG
///
/// An Endrechnung must deduct the advances **and the tax attributable to them**
/// ("die vereinnahmten Teilentgelte und die auf sie entfallenden Steuerbeträge"),
/// so each advance carries the rate it was invoiced at. A gross total alone
/// cannot express that, which is why [`ust_satz`](Self::ust_satz) is not
/// optional: an advance collected at 19 % and one collected at 7 % deduct
/// different amounts of tax from the same gross sum.
///
/// [`betrag_eur`](Self::betrag_eur) is the **gross** amount the customer paid;
/// the net and the tax it contains are derived from it by
/// [`netto_eur`](Self::netto_eur) and [`ust_eur`](Self::ust_eur).
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
    /// Gross EUR amount already paid (positive = customer paid this amount).
    pub betrag_eur: Decimal,
    /// VAT rate contained in `betrag_eur`, as a fraction — `0.19` for 19 %.
    ///
    /// This is the rate the *advance* was invoiced at, which is not necessarily
    /// the rate on the final invoice: a rate change mid-year leaves earlier
    /// advances at the old rate.
    pub ust_satz: Decimal,
    /// Optional description shown on invoice (e.g. `"Abschlag März 2026"`).
    #[serde(default)]
    pub beschreibung: Option<String>,
}

impl AbschlagDeduction {
    /// The net amount contained in the gross payment (Herausrechnung).
    ///
    /// `betrag_eur / (1 + ust_satz)`, rounded to cents. Returns the gross
    /// unchanged when the rate is zero, so a zero-rated advance needs no
    /// special-casing at the call site.
    #[must_use]
    pub fn netto_eur(&self) -> Decimal {
        if self.ust_satz.is_zero() {
            return self.betrag_eur;
        }
        (self.betrag_eur / (Decimal::ONE + self.ust_satz)).round_dp(2)
    }

    /// The tax contained in the gross payment.
    ///
    /// Derived as `betrag_eur - netto_eur` rather than `netto × rate`, so that
    /// net and tax always re-sum to the gross the customer actually paid.
    #[must_use]
    pub fn ust_eur(&self) -> Decimal {
        self.betrag_eur - self.netto_eur()
    }

    /// Project into a [`billing::AdvancePayment`] carrying this advance's own tax.
    ///
    /// This is the structure EN 16931's flat BT-113 cannot hold and that
    /// §14 Abs. 5 Satz 2 UStG requires on a settling invoice. It mirrors the
    /// ZUGFeRD / Factur-X EXTENDED group `SpecifiedAdvancePayment` (BG-X-45).
    ///
    /// The category is derived from the rate: a positive rate is a standard-rated
    /// advance, a zero rate a zero-rated one. An advance under reverse charge
    /// (§13b UStG) is not expressible this way and is not produced here — such a
    /// supply carries no advance tax to deduct.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Arithmetic`](crate::EngineError::Arithmetic) if
    /// the amounts overflow [`billing::EuroAmount`].
    pub fn to_advance_payment(&self) -> Result<billing::AdvancePayment, crate::EngineError> {
        let category = if self.ust_satz.is_zero() {
            billing::TaxCategory::ZeroRated
        } else {
            billing::TaxCategory::Standard
        };
        let entry = billing::TaxBreakdownEntry::new(
            category,
            self.ust_satz,
            billing::EuroAmount::checked_from_decimal(self.netto_eur())?,
            billing::EuroAmount::checked_from_decimal(self.ust_eur())?,
        );
        let advance =
            billing::AdvancePayment::new(vec![entry])?.with_received_on(self.datum.to_string());
        Ok(match &self.beschreibung {
            Some(r) => advance.with_reference(r.clone()),
            None => advance,
        })
    }
}

// ── SettlementForm ────────────────────────────────────────────────────────────

/// How a settling invoice accounts for advances the customer already paid.
///
/// Both shapes are lawful and both are in use; they differ in what the document
/// shows, not in what the customer ends up paying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SettlementForm {
    /// **Endrechnung** — invoice the whole supply, then deduct the advances and
    /// the tax contained in them (§14 Abs. 5 Satz 2 UStG).
    ///
    /// Totals and the VAT breakdown describe the full period; only the amount
    /// payable shrinks. Deducting the advances but *not* their tax is the failure
    /// this form has to avoid: under UStAE 14.8 Abs. 10 the issuer then owes the
    /// tax shown plus the advance-related portion again under §14c Abs. 1 — the
    /// same tax twice.
    #[default]
    Endrechnung,

    /// **Restrechnung** — invoice only the remainder; the advances are not listed.
    ///
    /// Structurally simpler, and what the BMF recommends for e-invoices (Schreiben
    /// v. 15.10.2024, Rn. 48), because EN 16931's core profiles have nowhere to
    /// carry per-advance tax. The taxable base is the residual per rate rather
    /// than the full supply.
    Restrechnung,
}

// ── BillingPeriod ─────────────────────────────────────────────────────────────

/// A validated billing period — first and last day, both inclusive.
///
/// The constructor refuses `from > to`, so an inverted period is
/// unrepresentable everywhere downstream: no provider, no pro-rata helper,
/// no JSON assembly ever needs to re-check the ordering.
///
/// Deserialization runs through the same validation
/// (`#[serde(try_from = …)]`), so a period arriving over the wire holds the
/// same invariant as one built in code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "PeriodEndpoints", into = "PeriodEndpoints")]
pub struct BillingPeriod {
    from: time::Date,
    to: time::Date,
}

/// Serde carrier for [`BillingPeriod`] — validation happens in `TryFrom`.
#[derive(serde::Serialize, serde::Deserialize)]
struct PeriodEndpoints {
    from: time::Date,
    to: time::Date,
}

impl TryFrom<PeriodEndpoints> for BillingPeriod {
    type Error = crate::EngineError;
    fn try_from(p: PeriodEndpoints) -> Result<Self, Self::Error> {
        Self::new(p.from, p.to)
    }
}

impl From<BillingPeriod> for PeriodEndpoints {
    fn from(p: BillingPeriod) -> Self {
        Self {
            from: p.from,
            to: p.to,
        }
    }
}

impl BillingPeriod {
    /// Build a period from first and last day (both inclusive).
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidPeriod`](crate::EngineError::InvalidPeriod) when `from > to`.
    pub fn new(from: time::Date, to: time::Date) -> Result<Self, crate::EngineError> {
        if from > to {
            return Err(crate::EngineError::InvalidPeriod { from, to });
        }
        Ok(Self { from, to })
    }

    /// First day of the period (inclusive).
    #[must_use]
    pub const fn from(self) -> time::Date {
        self.from
    }

    /// Last day of the period (inclusive).
    #[must_use]
    pub const fn to(self) -> time::Date {
        self.to
    }

    /// Number of calendar days, inclusive of both endpoints. Always ≥ 1.
    #[must_use]
    pub fn days(self) -> i64 {
        (self.to - self.from).whole_days() + 1
    }

    /// Whether the given date falls inside the period.
    #[must_use]
    pub fn contains(self, date: time::Date) -> bool {
        self.from <= date && date <= self.to
    }
}

impl Default for BillingPeriod {
    /// Placeholder single-day period at `Date::MIN` — used only by
    /// `BillingContext::default()`. Always set an explicit period before
    /// billing.
    fn default() -> Self {
        Self {
            from: time::Date::MIN,
            to: time::Date::MIN,
        }
    }
}

impl std::fmt::Display for BillingPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.from, self.to)
    }
}

// ── Vertragsart ───────────────────────────────────────────────────────────────

/// The contractual regime the delivery runs under — drives which invoice
/// disclosures and period limits apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Vertragsart {
    /// Freely negotiated supply contract (§41 EnWG). The default.
    #[default]
    Sondervertrag,

    /// Grundversorgung (§36 EnWG, StromGVV/GasGVV): the published Allgemeine
    /// Preise apply, termination per §20 StromGVV/GasGVV is two weeks.
    /// Emitted as the `vertragsart` ZusatzAttribut so the invoice states the
    /// regime the prices come from.
    Grundversorgung,

    /// Ersatzversorgung (§38 EnWG): the fallback supply when energy is drawn
    /// without an assignable contract. Ends by law after **three months** at
    /// the latest (§38 Abs. 2 S. 2 EnWG) — the engine refuses to bill a
    /// longer Ersatzversorgung period, because such a supply cannot exist.
    Ersatzversorgung,
}

impl Vertragsart {
    /// The label emitted as the `vertragsart` ZusatzAttribut.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sondervertrag => "SONDERVERTRAG",
            Self::Grundversorgung => "GRUNDVERSORGUNG",
            Self::Ersatzversorgung => "ERSATZVERSORGUNG",
        }
    }
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
/// use energy_billing::{AbschlagDeduction, BillingContext, BillingPeriod, InvoiceType, RegulatoryRates};
/// use time::macros::date;
/// use rust_decimal::dec;
///
/// let ctx = BillingContext {
///     malo_id: "51238696781".to_owned(),
///     lf_mp_id: "9900000000001".to_owned(),
///     rechnungsnummer: "R2026-001".to_owned(),
///     period: BillingPeriod::new(date!(2026-01-01), date!(2026-12-31)).unwrap(),
///     invoice_type: InvoiceType::Final,
///     regulatory_rates: RegulatoryRates::default(),
///     contract_id: None,
///     abschlage: vec![
///         AbschlagDeduction {
///             datum: date!(2026-01-15),
///             betrag_eur: dec!(120.00),
///             ust_satz: dec!(0.19),
///             beschreibung: Some("Abschlag Januar 2026".to_owned()),
///         },
///     ],
///     ..Default::default()
/// };
/// assert_eq!(ctx.total_abschlage_eur(), dec!(120.00));
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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

    /// The billing period — validated, `from > to` unrepresentable.
    pub period: BillingPeriod,

    /// Invoice type: initial, correction, cancellation, or final settlement.
    pub invoice_type: InvoiceType,

    /// The contractual regime — Sondervertrag, Grundversorgung, or
    /// Ersatzversorgung. Emitted as the `vertragsart` ZusatzAttribut; an
    /// Ersatzversorgung period longer than three months blocks the run
    /// (§38 Abs. 2 S. 2 EnWG).
    #[serde(default)]
    pub vertragsart: Vertragsart,

    /// How advances are accounted for on a settling invoice.
    ///
    /// Only consulted when `abschlage` is non-empty. See [`SettlementForm`].
    #[serde(default)]
    pub settlement_form: SettlementForm,

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

    /// §40 Abs. 1 EnWG contract facts, emitted as ZusatzAttribute.
    #[serde(default)]
    pub vertragsinformationen: Option<Vertragsinformationen>,

    /// §42 EnWG — Stromkennzeichnung, structured.
    ///
    /// Fuel-mix percentages, the specific CO₂ emissions (§42 Abs. 2 Nr. 2 —
    /// mandatory on every electricity invoice), and HKN certification. Emitted
    /// as the `stromkennzeichnung` ZusatzAttribut with the structure intact;
    /// prose belongs in [`crate::tariff::EnergieQuellen::beschreibung`].
    ///
    /// This replaces a free-text `energiemix` string that could not carry the
    /// CO₂ figure the law names explicitly — the structured type existed on the
    /// product all along and never reached the invoice.
    #[serde(default)]
    pub energiequellen: Option<crate::tariff::EnergieQuellen>,

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

impl BillingContext {
    /// First day of the billing period (inclusive).
    #[must_use]
    pub const fn period_from(&self) -> time::Date {
        self.period.from()
    }

    /// Last day of the billing period (inclusive).
    #[must_use]
    pub const fn period_to(&self) -> time::Date {
        self.period.to()
    }

    /// Number of calendar days in the billing period.
    ///
    /// Used for Grundpreis (daily rate × days) and pro-rata calculations.
    #[must_use]
    pub fn days(&self) -> i64 {
        self.period.days()
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
    /// use energy_billing::{BillingContext, BillingPeriod, InvoiceType, RegulatoryRates};
    /// use time::macros::date;
    ///
    /// let ctx = BillingContext {
    ///     period: BillingPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
    ///     vertragsbeginn: Some(date!(2026-01-16)), // contract started mid-month
    ///     ..Default::default()
    /// };
    /// let frac = ctx.billing_days_fraction().unwrap();
    /// // 16 billable days out of 31: ≈ 0.516
    /// assert!(frac > rust_decimal::dec!(0.50) && frac < rust_decimal::dec!(0.55));
    /// ```
    #[must_use]
    pub fn billing_days_fraction(&self) -> Option<Decimal> {
        let period_days = self.days();
        if period_days <= 0 {
            return None;
        }

        // Effective start: max(period_from, vertragsbeginn)
        let effective_from = match self.vertragsbeginn {
            Some(vb) if vb > self.period_from() => vb,
            _ => self.period_from(),
        };

        // Effective end: min(period_to, vertragsende)
        let effective_to = match self.vertragsende {
            Some(ve) if ve < self.period_to() => ve,
            _ => self.period_to(),
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
    /// # use energy_billing::{BillingContext, BillingPeriod};
    /// # use time::macros::date;
    /// let ctx = BillingContext {
    ///     period: BillingPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
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
            .filter(|&vb| vb > self.period_from())
            .unwrap_or(self.period_from());
        // Effective end: min(period_to, vertragsende)
        let effective_to = self
            .vertragsende
            .filter(|&ve| ve < self.period_to())
            .unwrap_or(self.period_to());
        let active = ((effective_to - effective_from).whole_days() + 1).max(0) as u32;
        (active.min(total), total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    fn base_ctx() -> BillingContext {
        BillingContext {
            period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
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
                    ust_satz: dec!(0.19),
                    beschreibung: None,
                },
                AbschlagDeduction {
                    datum: date!(2026 - 02 - 15),
                    betrag_eur: dec!(120.00),
                    ust_satz: dec!(0.19),
                    beschreibung: None,
                },
            ],
            ..base_ctx()
        };
        assert_eq!(ctx.total_abschlage_eur(), dec!(220.00));
    }
}
