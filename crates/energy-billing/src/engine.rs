//! `BillingEngine` — the composition root for multi-product invoice generation.
//!
//! Register one `BillingProvider` per product/service. Call `bill()` to run all
//! providers in order and assemble the `Invoice`.
//!
//! ## Primary API — `Product::build_engine()`
//!
//! The recommended way to build an engine is via [`Product::build_engine()`](crate::Product::build_engine):
//!
//! ```rust
//! use energy_billing::{BillingContext, BillingPeriod, GridInput, InvoiceType, MeterInput, Product, Quantities, RegulatoryRates};
//! use rust_decimal::dec;
//! use time::macros::date;
//!
//! let product: Product = serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
//! let ctx = BillingContext {
//!     malo_id:         "51238696781".to_owned(),
//!     lf_mp_id:        "9900000000001".to_owned(),
//!     rechnungsnummer: "R2026-001".to_owned(),
//!     period: BillingPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
//!     invoice_type:     InvoiceType::Initial,
//!     regulatory_rates: RegulatoryRates::default(),
//!     ..Default::default()
//! };
//! let quantities = Quantities {
//!     electricity: Some(MeterInput { arbeitsmenge_kwh: dec!(500), ..Default::default() }),
//!     ..Default::default()
//! };
//! let invoice = product.build_engine(&GridInput::default(), &RegulatoryRates::default())
//!     .bill(ctx, &quantities).unwrap();
//! assert!(invoice.brutto_eur > invoice.netto_eur);
//! ```
//!
//! ## Manual engine construction
//!
//! For advanced use cases (e.g. combining multiple providers in one engine),
//! you can build the engine manually:
//!
//! ```rust,ignore
//! let invoice = BillingEngine::new()
//!     .add(ElectricityProvider::new(product, GridInput::default()))
//!     .add(MwStProvider::new(dec!(0.19)))
//!     .bill(ctx, &quantities).unwrap();
//! ```

use crate::context::BillingContext;
use crate::error::EngineError;
use crate::invoice::Invoice;
use crate::position::{BillingPosition, BillingWarning, WarningSeverity};
use crate::provider::BillingProvider;
use crate::quantities::Quantities;
use crate::rates::RoundMoney;

/// The composition root for multi-product invoice generation.
#[derive(Default)]
pub struct BillingEngine {
    providers: Vec<Box<dyn BillingProvider>>,
}

impl BillingEngine {
    /// Create an empty engine. Register providers with [`add`](Self::add).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a `BillingProvider`. Returns `self` for method chaining.
    ///
    /// Providers run in registration order. Register tax providers (e.g.
    /// `MwStProvider`) **last** — they will automatically run in a second pass.
    #[must_use]
    #[allow(clippy::should_implement_trait)] // `add` is idiomatic for builder APIs in Rust
    pub fn add<P: BillingProvider + 'static>(mut self, provider: P) -> Self {
        self.providers.push(Box::new(provider));
        self
    }

    /// Run all registered providers and collect regulatory compliance warnings.
    ///
    /// Does NOT generate positions or produce an invoice. Call this before `bill()`
    /// to check regulatory preconditions (e.g. §41b iMSys guard, missing tariff
    /// fields) without committing to billing.
    ///
    /// An `Error`-severity warning indicates a definite regulatory violation.
    /// The operator should resolve the issue before calling `bill()`.
    #[must_use]
    pub fn validate(&self, ctx: &BillingContext, quantities: &Quantities) -> Vec<BillingWarning> {
        self.providers
            .iter()
            .flat_map(|p| p.validate_warnings(ctx, quantities))
            .collect()
    }

    /// Bill multiple (context, quantities) pairs using this engine configuration.
    ///
    /// Reuses the same provider set for every item in the batch. Fails fast per item
    /// (each error is independent). For large portfolios, collect all results and
    /// handle errors individually.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// let results = engine.bill_batch(batch);
    /// let errors: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    /// ```
    pub fn bill_batch(
        &self,
        batch: Vec<(BillingContext, Quantities)>,
    ) -> Vec<Result<Invoice, EngineError>> {
        batch
            .into_iter()
            .map(|(ctx, quantities)| self.bill(ctx, &quantities))
            .collect()
    }

    /// Run all providers and assemble an `Invoice`.
    ///
    /// Three-pass execution:
    /// 1. Commodity + levy providers (all `is_tax_pass() == false`)
    /// 2. Tax providers (all `is_tax_pass() == true`)
    /// 3. Abschlag deductions from `ctx.abschlage` (for `InvoiceType::Final`)
    pub fn bill(
        &self,
        ctx: BillingContext,
        quantities: &Quantities,
    ) -> Result<Invoice, EngineError> {
        // ── Pass 0: collect regulatory warnings ───────────────────────────────
        // Run context-level checks and validate_warnings() on all providers.
        // If any Error-severity warning is found, fail before generating any
        // positions — the billing run is invalid and must not be dispatched.
        let mut warnings: Vec<BillingWarning> = context_warnings(&ctx);
        for provider in self.providers.iter() {
            warnings.extend(provider.validate_warnings(&ctx, quantities));
        }
        if warnings
            .iter()
            .any(|x| x.severity == WarningSeverity::Error)
        {
            // The error carries ALL warnings so the caller sees every violation.
            return Err(EngineError::ValidationBlocked { warnings });
        }

        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── Pass 1: commodity, grid, levy ─────────────────────────────────────
        for provider in self.providers.iter().filter(|p| !p.is_tax_pass()) {
            let new = provider.bill(&ctx, quantities, &positions)?;
            positions.extend(new);
        }

        // ── Pass 2: taxes (MwSt sees the full commodity/levy base) ─────────────
        let pre_tax_snap: Vec<BillingPosition> = positions.clone();
        for provider in self.providers.iter().filter(|p| p.is_tax_pass()) {
            let new = provider.bill(&ctx, quantities, &pre_tax_snap)?;
            positions.extend(new);
        }

        // ── Pass 3: Abschlag deductions (Final invoices only) ──────────────────
        // §41 EnWG: Jahresabrechnung must itemise each advance payment.
        // These positions do NOT affect netto_eur / mwst_eur — they reduce
        // zahlbetrag_eur only (already paid by customer, now being reconciled).
        for abschlag in &ctx.abschlage {
            let label = abschlag
                .beschreibung
                .clone()
                .unwrap_or_else(|| format!("Abschlag {}", abschlag.datum));
            positions.push(
                crate::position::BillingPosition::debit(
                    label,
                    rust_decimal::Decimal::ONE,
                    "EUR",
                    -abschlag.betrag_eur, // negative unit_price → deduction
                    crate::position::PositionCategory::Abschlag,
                )
                .with_legal_basis("§41 EnWG"),
            );
        }

        // ── Pass 4: Minimum invoice top-up ──────────────────────────────────────
        // When ctx.minimum_invoice_eur_brutto is set and the computed brutto_eur
        // is below the minimum, add a Mindestbetrag position and re-run the tax pass.
        if let Some(min_brutto) = ctx.minimum_invoice_eur_brutto {
            let current_invoice = Invoice::from_positions(ctx.clone(), positions.clone(), vec![]);
            let current_brutto = current_invoice.brutto_eur;
            if current_brutto < min_brutto {
                let gap_brutto = min_brutto - current_brutto;
                // Use the configured MwSt rate directly — deriving it from existing
                // positions is unreliable when netto is zero or all positions are
                // credits (P0 fix: use configured rate, not derived ratio).
                let mwst_rate = ctx.regulatory_rates.mwst_rate;
                let divisor = rust_decimal::Decimal::ONE + mwst_rate;
                let gap_netto = if divisor.is_zero() {
                    gap_brutto
                } else {
                    (gap_brutto / divisor).round_kfm(5)
                };

                // Strip old Tax positions and re-run tax pass with top-up included.
                let mut positions2: Vec<BillingPosition> = positions
                    .iter()
                    .filter(|p| p.category != crate::position::PositionCategory::Tax)
                    .cloned()
                    .collect();
                positions2.push(
                    crate::position::BillingPosition::debit(
                        format!("Mindestbetrag (Minimum {min_brutto:.2}\u{202f}EUR brutto)"),
                        rust_decimal::Decimal::ONE,
                        "EUR",
                        gap_netto,
                        crate::position::PositionCategory::Commodity,
                    )
                    .with_legal_basis("Vertraglich")
                    .with_tag("mindestbetrag"),
                );
                let pre_tax2: Vec<BillingPosition> = positions2.clone();
                for provider in self.providers.iter().filter(|p| p.is_tax_pass()) {
                    let new = provider.bill(&ctx, quantities, &pre_tax2)?;
                    positions2.extend(new);
                }
                // Re-add Abschlag (already correct, just copy them over).
                for p in positions
                    .iter()
                    .filter(|p| p.category == crate::position::PositionCategory::Abschlag)
                {
                    positions2.push(p.clone());
                }
                return Ok(Invoice::from_positions(ctx, positions2, warnings));
            }
        }

        // ── Pass 5: Cancellation (Storno) — negate all signs ──────────────────
        // §41 EnWG: A Stornorechnung reverses the original invoice to EUR 0.
        // All position signs are inverted so brutto_eur = -(original brutto_eur).
        if ctx.invoice_type.is_reversal() {
            negate_positions(&mut positions);
        }

        Ok(Invoice::from_positions(ctx, positions, warnings))
    }
}

// ── Context-level regulatory checks ───────────────────────────────────────────

/// Warnings derived from the context alone, independent of any provider.
///
/// Currently one check: §38 Abs. 2 S. 2 EnWG limits Ersatzversorgung to three
/// months — a longer Ersatzversorgung period describes a supply that cannot
/// legally exist, so it blocks the run (`Error` severity). Bill the first
/// three months as Ersatzversorgung and the remainder under the regime the
/// supply actually continued in.
fn context_warnings(ctx: &BillingContext) -> Vec<BillingWarning> {
    let mut warnings = Vec::new();
    if ctx.vertragsart == crate::context::Vertragsart::Ersatzversorgung {
        let from = ctx.period_from();
        // Three months after the first day; the Ersatzversorgung may run
        // through the day before.
        let limit = add_months(from, 3);
        if ctx.period_to() >= limit {
            warnings.push(BillingWarning {
                code: "ERSATZVERSORGUNG_UEBER_3_MONATE",
                severity: WarningSeverity::Error,
                message: format!(
                    "Ersatzversorgung endet spätestens drei Monate nach Beginn \
                     (§38 Abs. 2 S. 2 EnWG): Zeitraum {}..{} überschreitet die \
                     Grenze {limit}",
                    from,
                    ctx.period_to(),
                ),
            });
        }
    }
    warnings
}

/// `date` plus `months` calendar months, clamped to the last valid day.
fn add_months(date: time::Date, months: i32) -> time::Date {
    let total = date.month() as i32 - 1 + months;
    let year = date.year() + total.div_euclid(12);
    let month = time::Month::try_from((total.rem_euclid(12) + 1) as u8).expect("1..=12");
    let day = date.day().min(time::util::days_in_month(month, year));
    time::Date::from_calendar_date(year, month, day).expect("valid clamped date")
}

// ── Cancellation helpers ──────────────────────────────────────────────────────

/// Negate all position amounts for a Stornorechnung (Cancellation invoice).
///
/// Called internally by `BillingEngine::bill()` when `ctx.invoice_type.is_reversal()`.
/// All `net_eur` and `unit_price_eur` are sign-inverted so the Invoice's
/// `netto_eur`, `mwst_eur`, and `brutto_eur` equal `-(original)`.
fn negate_positions(positions: &mut [crate::position::BillingPosition]) {
    for p in positions.iter_mut() {
        p.net_eur = -p.net_eur;
        p.unit_price_eur = -p.unit_price_eur;
    }
}
