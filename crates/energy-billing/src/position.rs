//! `BillingPosition` — the atomic unit of every energy invoice.
//!
//! Every charge, credit, levy, and tax on an invoice is one `BillingPosition`.
//! The `category` and `tags` fields enable downstream routing (accounting, ERP,
//! MwSt base computation, regulatory reporting).
//!
//! ## Explainability: `PositionTrace`
//!
//! Every `BillingPosition` now carries a `trace: PositionTrace` that answers
//! *"why does this amount appear on the invoice?"* — matching the audit depth of
//! `grid-billing::CalculationTrace`. Each trace records:
//! - the input quantity and unit price before rounding
//! - the formula used (human-readable)
//! - all applicable §-citations
//! - the tariff source (which product sheet supplied the rate)
//!
//! This makes every invoice amount reproducible and auditable without re-running
//! the calculation, satisfying BNetzA §20 EnWG audit requirements.

use crate::rates::RoundMoney;
use rust_decimal::Decimal;
use rust_decimal::dec;

// ── PositionTrace ─────────────────────────────────────────────────────────────

/// Full audit record for how one [`BillingPosition`] was computed.
///
/// Answers: *"Why does this charge/credit appear, and how was it calculated?"*
///
/// Every `BillingPosition` carries a `PositionTrace` so invoice auditors can
/// reconstruct any amount without re-running the billing engine.
///
/// ## Design note
///
/// Mirrors `grid_billing::CalculationTrace` — both billing engines should provide
/// the same depth of explainability.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PositionTrace {
    /// Human-readable formula, e.g.:
    /// `"500.000 kWh × 0.30000 EUR/kWh = 150.00000 EUR"`
    pub formula: String,

    /// Input quantity before rounding (same as `BillingPosition.quantity` for most positions;
    /// may differ when pro-rata fractions are applied).
    pub input_quantity: Decimal,

    /// Input unit price in EUR before rounding (converted from ct/kWh or EUR/month).
    pub input_unit_price_eur: Decimal,

    /// Gross amount before rounding (input_quantity × input_unit_price_eur).
    pub gross_eur: Decimal,

    /// Applicable regulatory citations (§-references).
    ///
    /// Examples: `["§41 EnWG"]`, `["§3 StromStG"]`, `["§40a EnWG"]`, `["§12 Abs. 3 UStG"]`
    pub regulatory_basis: Vec<String>,

    /// Tariff source reference (product sheet or contract).
    ///
    /// `None` for statutory positions (Stromsteuer, BEHG, MwSt).
    /// Set to tariff sheet ID for commodity positions from `tarifbd`.
    pub tariff_source: Option<String>,

    /// Any pro-rata fraction applied (0.0–1.0).
    ///
    /// `None` when billing covers the full period.
    /// `Some(0.5)` means only half the billing period is charged
    /// (e.g. contract start mid-month).
    pub pro_rata_fraction: Option<Decimal>,

    /// Human-readable note on rounding, if applicable.
    pub rounding_note: Option<String>,
}

impl PositionTrace {
    /// Build a simple commodity trace (quantity × price = net).
    #[must_use]
    pub fn commodity(
        quantity: Decimal,
        unit: &str,
        unit_price_eur: Decimal,
        regulatory_basis: impl Into<String>,
    ) -> Self {
        let gross = quantity * unit_price_eur;
        Self {
            formula: format!(
                "{quantity:.3} {unit} × {unit_price_eur:.5} EUR/{unit} = {:.5} EUR",
                gross.round_kfm(5)
            ),
            input_quantity: quantity,
            input_unit_price_eur: unit_price_eur,
            gross_eur: gross,
            regulatory_basis: vec![regulatory_basis.into()],
            tariff_source: None,
            pro_rata_fraction: None,
            rounding_note: None,
        }
    }

    /// Build a tax trace (rate × base = tax amount).
    #[must_use]
    pub fn tax(
        rate: Decimal,
        netto_base_eur: Decimal,
        regulatory_basis: impl Into<String>,
    ) -> Self {
        let gross = netto_base_eur * rate;
        Self {
            formula: format!(
                "{rate:.4} × {netto_base_eur:.5} EUR = {:.5} EUR",
                gross.round_kfm(5)
            ),
            input_quantity: netto_base_eur,
            input_unit_price_eur: rate,
            gross_eur: gross,
            regulatory_basis: vec![regulatory_basis.into()],
            tariff_source: None,
            pro_rata_fraction: None,
            rounding_note: None,
        }
    }

    /// Attach a tariff source reference (product sheet ID).
    #[must_use]
    pub fn with_tariff_source(mut self, source: impl Into<String>) -> Self {
        self.tariff_source = Some(source.into());
        self
    }

    /// Attach a pro-rata fraction.
    #[must_use]
    pub fn with_pro_rata(mut self, fraction: Decimal) -> Self {
        self.pro_rata_fraction = Some(fraction);
        self
    }

    /// Add an additional regulatory citation.
    #[must_use]
    pub fn with_basis(mut self, basis: impl Into<String>) -> Self {
        self.regulatory_basis.push(basis.into());
        self
    }
}

// ── BillingWarning ────────────────────────────────────────────────────────────

/// A non-fatal warning produced during billing calculation.
///
/// Warnings do not prevent invoice generation — they flag conditions that
/// the operator should review before dispatch. Examples:
/// - Estimated meter reading (§ 60 Abs. 2 MsbG) — labeled on invoice
/// - Preisgarantie expiring in ≤ 30 days — §41 Abs. 1 Nr. 4 EnWG notice
/// - §41a dynamic tariff offered but meter is not iMSys — §41b EnWG risk
/// - Consumption deviates > 50% from Vorjahresverbrauch — review reading
///
/// The service layer (`billingd`) should surface `Error`-severity warnings
/// to the operator and may block dispatch for high-severity issues.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BillingWarning {
    /// Machine-readable warning code for programmatic handling.
    pub code: &'static str,
    /// Severity level.
    pub severity: WarningSeverity,
    /// Human-readable description (shown in operator dashboard).
    pub message: String,
}

/// Severity level for [`BillingWarning`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum WarningSeverity {
    /// Informational — no action required.
    Info,
    /// Potential issue — review recommended before dispatch.
    Warning,
    /// Definite issue — operator must review before dispatch.
    Error,
}

// ── PositionCategory ──────────────────────────────────────────────────────────

/// High-level category for an invoice position.
///
/// Used by accounting systems and the MwSt engine to classify positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PositionCategory {
    /// Energy commodity charge (Arbeitspreis, Grundpreis).
    Commodity,
    /// Grid infrastructure charge (NNE, Konzessionsabgabe).
    GridCharge,
    /// Statutory per-unit levy (Stromsteuer, Energiesteuer, BEHG).
    Levy,
    /// Tax (MwSt / Umsatzsteuer).
    Tax,
    /// Credit note position (EEG Gutschrift, PV credit, §14a reduction).
    Credit,
    /// Commercial discount or reduction (rabatt, AufAbschlag).
    Discount,
    /// Non-commodity service fee (MSB, HEMS subscription, e-mobility roaming).
    Fee,
    /// Informational position (Brennwertkorrektur, §51 suspension info, Zählerstand).
    Info,
    /// Advance payment deduction on final invoice (Jahresabrechnung §41 EnWG).
    ///
    /// Does NOT affect `netto_eur` or `mwst_eur` — deducted separately in
    /// `Invoice::zahlbetrag_eur = brutto_eur - abschlag_total_eur`.
    Abschlag,

    /// Customer bonus (Willkommensbonus, Treuebonus, Wechselprämie).
    ///
    /// Semantically distinct from `Discount` (contractual price reduction) and
    /// from `Credit` (product-level credit note). Bonuses are one-time or
    /// conditional rewards that the customer earned by:
    /// - Switching to this supplier (Wechselprämie)
    /// - Staying with the supplier for N years (Treuebonus)
    /// - Signing up for a specific product (Willkommensbonus)
    ///
    /// MwSt treatment: same as `Discount` (reduces the MwSt base).
    Bonus,

    /// §42c EnWG Energy Sharing credit position.
    ///
    /// Community energy sharing (Energiegemeinschaft) allocation credit:
    /// the tenant's share of locally produced shared electricity.
    /// Reduces the grid consumption billed under `Commodity`.
    ///
    /// ## Legal basis
    ///
    /// §42c EnWG (Energiegemeinschaften, effective 01.01.2024):
    /// sharing communities may distribute locally generated electricity to
    /// participants within the same low-voltage grid area. The LF bills the
    /// full consumption and credits the sharing allocation separately.
    EnergyShare,
}

// ── BillingPosition ───────────────────────────────────────────────────────────

/// One line item on an energy invoice.
///
/// All monetary amounts are in **EUR** (not ct/kWh), stored as [`Decimal`]
/// with 5 decimal places precision (matching the internal `EuroAmount` type).
///
/// ## Sign convention
///
/// - `net_eur > 0` → debit (customer owes Lieferant)
/// - `net_eur < 0` → credit (Lieferant owes customer)
///
/// Credits (EEG feed-in, §14a reduction, rabatt) use negative `net_eur`.
///
/// ## Explainability via `PositionTrace`
///
/// Every position carries a `trace: PositionTrace` that records the formula,
/// inputs, regulatory citations, and tariff source. This enables full audit
/// reconstruction without re-running the billing engine — matching the depth
/// of `grid_billing::CalculationTrace`.
///
/// ## Tags
///
/// Tags are lower-case strings used for position filtering and routing:
/// - `"commodity"` — energy commodity (Stromsteuer base)
/// - `"nne"` — grid charge umbrella
/// - `"levy"` — statutory per-unit levy
/// - `"mwst"` — Umsatzsteuer position
/// - `"eeg"`, `"§14a"`, `"solar"` — product-specific
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BillingPosition {
    /// Human-readable description for the invoice.
    pub description: String,

    /// Legal basis (§-reference), shown on invoice if set.
    ///
    /// Short form for invoice display; the full citation tree is in `trace.regulatory_basis`.
    pub legal_basis: Option<String>,

    /// Quantity (kWh, m³, Tage, kW, or "1" for lump-sum).
    pub quantity: Decimal,

    /// Unit of measure (e.g. `"kWh"`, `"m³"`, `"Tage"`, `"kW"`, `"%"`).
    pub unit: String,

    /// Price per unit in EUR.
    ///
    /// For commodity: ct/kWh ÷ 100.
    /// For Grundpreis: EUR/year ÷ 365.
    /// For tax: the tax rate (fraction, e.g. 0.19).
    pub unit_price_eur: Decimal,

    /// Net amount in EUR = quantity × unit_price_eur (rounded to 5 dp).
    ///
    /// Negative for credits, positive for charges.
    pub net_eur: Decimal,

    /// Semantic category for accounting routing.
    pub category: PositionCategory,

    /// Free-form tags for downstream filtering.
    pub tags: Vec<String>,

    /// MwSt rate applicable to this position (fraction, e.g. `0.19`, `0.07`, `0.0`).
    ///
    /// When `Some`, the `MwStProvider` uses this rate for this position instead of the
    /// engine-wide default. Enables multi-rate VAT on a single invoice:
    /// - Standard electricity/gas: `None` (uses engine default, typically `0.19`)
    /// - Renewable Fernwärme: `Some(dec!(0.07))` (§12 Abs. 2 Nr. 1 UStG)
    /// - Solar PV ≤30 kWp since 01.01.2023: `Some(dec!(0.0))` (§12 Abs. 3 UStG Solarpaket I)
    ///
    /// Positions with category `Tax`, `Abschlag`, or `Info` are excluded from MwSt computation.
    #[serde(default)]
    pub applicable_tax_rate: Option<Decimal>,

    /// Full calculation audit trail for this position.
    ///
    /// Answers: *"Why does this amount appear, and how was it computed?"*
    /// Every position should have a non-default trace with at least one
    /// `regulatory_basis` citation.
    #[serde(default)]
    pub trace: PositionTrace,
}

impl BillingPosition {
    /// Construct a debit position (customer owes the amount).
    ///
    /// `net_eur` is automatically computed as `quantity × unit_price_eur`.
    #[must_use]
    pub fn debit(
        description: impl Into<String>,
        quantity: Decimal,
        unit: impl Into<String>,
        unit_price_eur: Decimal,
        category: PositionCategory,
    ) -> Self {
        let net_eur = validated_eur(quantity * unit_price_eur);
        let unit_str = unit.into();
        Self {
            description: description.into(),
            legal_basis: None,
            quantity,
            unit: unit_str,
            unit_price_eur,
            net_eur,
            category,
            tags: Vec::new(),
            applicable_tax_rate: None,
            trace: PositionTrace::default(),
        }
    }

    /// Construct a credit position (Lieferant owes the customer).
    ///
    /// `net_eur` is automatically negated from the absolute rate.
    #[must_use]
    pub fn credit(
        description: impl Into<String>,
        quantity: Decimal,
        unit: impl Into<String>,
        abs_rate_eur: Decimal,
        category: PositionCategory,
    ) -> Self {
        let net_eur = -validated_eur(quantity * abs_rate_eur);
        Self {
            description: description.into(),
            legal_basis: None,
            quantity,
            unit: unit.into(),
            unit_price_eur: -abs_rate_eur,
            net_eur,
            category,
            tags: Vec::new(),
            applicable_tax_rate: None,
            trace: PositionTrace::default(),
        }
    }

    /// Attach a legal basis reference (e.g. `"§3 StromStG"`).
    #[must_use]
    pub fn with_legal_basis(mut self, basis: impl Into<String>) -> Self {
        self.legal_basis = Some(basis.into());
        self
    }

    /// Add a routing tag.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set the MwSt rate for this position (§ UStG).
    ///
    /// Override the engine-wide default for this specific position.
    /// Use `dec!(0.07)` for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
    /// `dec!(0.0)` for solar PV ≤30 kWp (§12 Abs. 3 UStG),
    /// or omit to use the engine default (19%).
    #[must_use]
    pub fn with_tax_rate(mut self, rate: Decimal) -> Self {
        self.applicable_tax_rate = Some(rate);
        self
    }

    /// `true` when this position carries the given tag.
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Sum of `net_eur` across all positions.
    #[must_use]
    pub fn net_total(positions: &[BillingPosition]) -> Decimal {
        positions.iter().map(|p| p.net_eur).sum()
    }

    /// Sum of `net_eur` for positions carrying the given tag.
    #[must_use]
    pub fn total_by_tag(positions: &[BillingPosition], tag: &str) -> Decimal {
        positions
            .iter()
            .filter(|p| p.has_tag(tag))
            .map(|p| p.net_eur)
            .sum()
    }
}

/// Round and range-validate a monetary EUR amount to 5 decimal places.
///
/// Uses [`billing::EuroAmount`] internally to detect overflow (max ~92 M EUR).
/// Beyond the fixed-point range the amount is kept and rounded directly —
/// zeroing it (the old behaviour) silently erased the position's value,
/// which is exactly the silent-degradation failure a billing engine must
/// not have. An out-of-range line then fails loudly downstream in the
/// EN16931 total-reconciliation checks instead of vanishing.
pub(crate) fn validated_eur(amount: Decimal) -> Decimal {
    billing::EuroAmount::checked_from_decimal(amount)
        .map(billing::EuroAmount::into_decimal)
        .unwrap_or_else(|_| amount.round_kfm(5))
}

// ── Convenience constructors ──────────────────────────────────────────────────

/// Build a commodity Grundpreis position (daily rate × billing period days).
pub(crate) fn grundpreis_position(
    label: impl Into<String>,
    daily_rate_eur: Decimal,
    days: i64,
    legal_basis: &'static str,
    tags: &[&'static str],
) -> BillingPosition {
    let mut p = BillingPosition::debit(
        label,
        Decimal::from(days),
        "Tage",
        daily_rate_eur,
        PositionCategory::Commodity,
    )
    .with_legal_basis(legal_basis)
    .with_tag("commodity")
    .with_tag("grundpreis");
    // The trace is built here, where the inputs are, so every position going
    // through this helper explains itself without each caller remembering to.
    p.trace = PositionTrace::commodity(Decimal::from(days), "Tage", daily_rate_eur, legal_basis);
    for tag in tags {
        p = p.with_tag(*tag);
    }
    p
}

/// Build a commodity Arbeitspreis position (kWh × rate in ct/kWh).
pub(crate) fn arbeitspreis_position(
    label: impl Into<String>,
    kwh: Decimal,
    rate_ct_kwh: Decimal,
    unit: &'static str,
    legal_basis: &'static str,
    tags: &[&'static str],
) -> BillingPosition {
    let mut p = BillingPosition::debit(
        label,
        kwh,
        unit,
        rate_ct_kwh / dec!(100),
        PositionCategory::Commodity,
    )
    .with_legal_basis(legal_basis)
    .with_tag("commodity")
    .with_tag("arbeitspreis");
    p.trace = PositionTrace::commodity(kwh, unit, rate_ct_kwh / dec!(100), legal_basis);
    for tag in tags {
        p = p.with_tag(*tag);
    }
    p
}

/// Build a per-unit levy position (quantity × rate in ct/unit).
pub(crate) fn levy_position(
    label: impl Into<String>,
    quantity: Decimal,
    unit: &'static str,
    rate_ct: Decimal,
    legal_basis: &'static str,
    tag: &'static str,
) -> BillingPosition {
    let mut p = BillingPosition::debit(
        label,
        quantity,
        unit,
        rate_ct / dec!(100),
        PositionCategory::Levy,
    )
    .with_legal_basis(legal_basis)
    .with_tag("levy")
    .with_tag(tag);
    p.trace = PositionTrace::commodity(quantity, unit, rate_ct / dec!(100), legal_basis);
    p
}
