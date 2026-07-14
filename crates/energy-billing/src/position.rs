//! `BillingPosition` — the atomic unit of every energy invoice.
//!
//! Every charge, credit, levy, and tax on an invoice is one `BillingPosition`.
//! The `category` and `tags` fields enable downstream routing (accounting, ERP,
//! MwSt base computation, regulatory reporting).

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

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
    /// Informational position (Brennwertkorrektur, §51 suspension info).
    Info,
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
        Self {
            description: description.into(),
            legal_basis: None,
            quantity,
            unit: unit.into(),
            unit_price_eur,
            net_eur,
            category,
            tags: Vec::new(),
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

/// Round a monetary EUR amount to 5 decimal places (matching `EuroAmount` precision).
pub(crate) fn validated_eur(amount: Decimal) -> Decimal {
    amount.round_dp(5)
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
    BillingPosition::debit(
        label,
        quantity,
        unit,
        rate_ct / dec!(100),
        PositionCategory::Levy,
    )
    .with_legal_basis(legal_basis)
    .with_tag("levy")
    .with_tag(tag)
}
