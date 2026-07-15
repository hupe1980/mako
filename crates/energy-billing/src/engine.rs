//! `BillingEngine` — the composition root for multi-product invoice generation.
//!
//! Register one `BillingProvider` per product/service. Call `bill()` to run all
//! providers in order and assemble the `Invoice`.
//!
//! ## Execution model
//!
//! Providers run in two passes:
//!
//! **Pass 1 — commodity / levy providers** (`is_tax_pass() == false`):
//! Each receives the accumulated positions from all earlier providers.
//!
//! **Pass 2 — tax providers** (`is_tax_pass() == true`, typically `MwStProvider`):
//! Sees all commodity/levy positions as `prior`, computes tax on the net base.
//!
//! ## Example
//!
//! ```rust
//! use energy_billing::{
//!     BillingContext, BillingEngine, ElectricityProvider, GasProvider,
//!     InvoiceType, MwStProvider, Quantities, RegulatoryRates,
//!     TariffInput, GridInput, MeterInput, GasMeterInput,
//! };
//! use rust_decimal_macros::dec;
//! use time::macros::date;
//!
//! let rates = RegulatoryRates::default();
//! let ctx = BillingContext {
//!     malo_id:          "51238696781".to_owned(),
//!     lf_mp_id:         "9900000000001".to_owned(),
//!     rechnungsnummer:  "R2026-001".to_owned(),
//!     period_from:       date!(2026-01-01),
//!     period_to:         date!(2026-01-31),
//!     invoice_type:      InvoiceType::Initial,
//!     contract_id:       None,
//!     regulatory_rates:  rates.clone(),
//!     ..Default::default()
//! };
//! let quantities = Quantities {
//!     electricity: Some(MeterInput {
//!         arbeitsmenge_kwh: dec!(500),
//!         ..Default::default()
//!     }),
//!     ..Default::default()
//! };
//! let tariff: TariffInput = serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
//! let invoice = BillingEngine::new()
//!     .add(ElectricityProvider::from_tariff(&tariff, &GridInput::default()))
//!     .add(MwStProvider::new(dec!(0.19)))
//!     .bill(ctx, &quantities)
//!     .unwrap();
//! assert!(invoice.brutto_eur > invoice.netto_eur);
//! ```

use billing::BillingError;

use crate::context::BillingContext;
use crate::invoice::Invoice;
use crate::position::BillingPosition;
use crate::provider::BillingProvider;
use crate::quantities::Quantities;

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

    /// Run all providers and assemble an `Invoice`.
    ///
    /// Three-pass execution:
    /// 1. Commodity + levy providers (all `is_tax_pass() == false`)
    /// 2. Tax providers (all `is_tax_pass() == true`)
    /// 3. Abschlag deductions from `ctx.abschlage` (for `InvoiceType::Final`)
    pub fn bill(
        self,
        ctx: BillingContext,
        quantities: &Quantities,
    ) -> Result<Invoice, BillingError> {
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
            let current_invoice = Invoice::from_positions(ctx.clone(), positions.clone());
            let current_brutto = current_invoice.brutto_eur;
            if current_brutto < min_brutto {
                let gap_brutto = min_brutto - current_brutto;
                // Infer MwSt rate from existing Tax positions (netto-based).
                let netto_sum: rust_decimal::Decimal = positions
                    .iter()
                    .filter(|p| {
                        !matches!(
                            p.category,
                            crate::position::PositionCategory::Tax
                                | crate::position::PositionCategory::Abschlag
                                | crate::position::PositionCategory::Info
                        )
                    })
                    .map(|p| p.net_eur)
                    .sum();
                let tax_sum: rust_decimal::Decimal = positions
                    .iter()
                    .filter(|p| p.category == crate::position::PositionCategory::Tax)
                    .map(|p| p.net_eur)
                    .sum();
                let mwst_rate = if netto_sum.is_zero() {
                    ctx.regulatory_rates.mwst_rate
                } else {
                    (tax_sum / netto_sum).abs()
                };
                let divisor = rust_decimal::Decimal::ONE + mwst_rate;
                let gap_netto = if divisor.is_zero() {
                    gap_brutto
                } else {
                    (gap_brutto / divisor).round_dp(5)
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
                return Ok(Invoice::from_positions(ctx, positions2));
            }
        }

        // ── Pass 5: Cancellation (Storno) — negate all signs ──────────────────
        // §41 EnWG: A Stornorechnung reverses the original invoice to EUR 0.
        // All position signs are inverted so brutto_eur = -(original brutto_eur).
        if ctx.invoice_type.is_reversal() {
            negate_positions(&mut positions);
        }

        Ok(Invoice::from_positions(ctx, positions))
    }
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
