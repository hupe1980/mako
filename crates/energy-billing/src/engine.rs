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
//! let product: Product = serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0"}"#).unwrap();
//! let ctx = BillingContext {
//!     malo_id:         "51238696012".to_owned(),
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
    /// to check regulatory preconditions (e.g. §41a iMSys guard, missing tariff
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
    /// 3. Abschlag deductions from `ctx.abschlage` (on every settling document —
    ///    every [`InvoiceType`](crate::InvoiceType) except
    ///    [`AdvancePayment`](crate::InvoiceType::AdvancePayment))
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
        warnings.extend(zero_quantity_warning(&ctx, quantities));
        warnings.extend(coverage_warning(quantities));
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

        // ── §13b reverse charge (before the MwSt pass) ────────────────────────
        // Steuerschuldnerschaft des Leistungsempfängers (§13b Abs. 2 Nr. 5 lit. b
        // UStG): when the customer is a Stromwiederverkäufer the whole supply is
        // reverse-charged — the supplier invoices net, the recipient owes the VAT.
        // Mark every supply position (not Tax/Abschlag/Info) reverse-charge so the
        // MwStProvider computes 0 and `tax_subtotals_of` emits an `AE` subtotal.
        if ctx.reverse_charge {
            use crate::position::PositionCategory;
            positions = positions
                .into_iter()
                .map(|p| {
                    if matches!(
                        p.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    ) {
                        p
                    } else {
                        p.with_reverse_charge()
                    }
                })
                .collect();
        }

        // ── The rate every supply position is charged at ──────────────────────
        // Stamped before the tax pass, so the rate that produced `mwst_eur` is
        // the same rate the BG-23/`steuerbetraege` breakdown reads back off the
        // positions. The two must never be able to disagree: a § 19 UStG
        // Kleinunternehmer document that charges 0 and states 19 % is an
        // unrechtmäßiger Steuerausweis under § 14c Abs. 2 UStG.
        let charged_rate = self
            .providers
            .iter()
            .filter(|p| p.is_tax_pass())
            .find_map(|p| p.charged_tax_rate());
        if let Some(charged) = charged_rate {
            use crate::position::PositionCategory;
            for p in &mut positions {
                if p.applicable_tax_rate.is_none()
                    && !matches!(
                        p.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    p.applicable_tax_rate = Some(charged);
                }
            }
        }

        // ── Pass 2: taxes (MwSt sees the full commodity/levy base) ─────────────
        let pre_tax_snap: Vec<BillingPosition> = positions.clone();
        for provider in self.providers.iter().filter(|p| p.is_tax_pass()) {
            let new = provider.bill(&ctx, quantities, &pre_tax_snap)?;
            positions.extend(new);
        }

        // ── Pass 3: Abschlag deductions ────────────────────────────────────────
        // §40 Abs. 1 EnWG: the settling invoice must itemise each advance
        // payment it discharges. These positions do NOT affect netto_eur /
        // mwst_eur — they reduce zahlbetrag_eur only (already paid by the
        // customer, now being reconciled).
        //
        // An Abschlagsrechnung is the document that *collects* an advance, so
        // it discharges none: deducting the advances already paid there would
        // net them off the very request that asks for the next one.
        if ctx.invoice_type.settles_advances() {
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
                    .with_legal_basis("§40 EnWG"),
                );
            }
        }

        // ── Pass 4: Minimum invoice top-up ──────────────────────────────────────
        // When ctx.minimum_invoice_eur_brutto is set and the computed brutto_eur
        // is below the minimum, add a Mindestbetrag position and re-run the tax pass.
        if let Some(min_brutto) = ctx.minimum_invoice_eur_brutto {
            let current_invoice = Invoice::from_positions(ctx.clone(), positions.clone(), vec![]);
            let current_brutto = current_invoice.brutto_eur;
            if current_brutto < min_brutto {
                let gap_brutto = min_brutto - current_brutto;
                // The Mindestbetrag is a **contractual** charge, so the rate it
                // is agreed at is the contract's to state:
                // `ctx.minimum_invoice_mwst_rate`, falling back to the rate the
                // document charges. Deriving it from the position mix is
                // unreliable exactly where it matters — when the net is zero, or
                // every position is a credit — and using the standard rate
                // unconditionally left a mixed-rate invoice (7 % Trinkwasser
                // beside 19 % energy) short of the configured minimum by the
                // rate difference on the top-up. Under §13b reverse charge the
                // invoice carries no VAT, so the gap is net as-is.
                let mwst_rate = ctx
                    .minimum_invoice_mwst_rate
                    .or(charged_rate)
                    .unwrap_or(ctx.regulatory_rates.mwst_rate);
                let divisor = if ctx.reverse_charge {
                    rust_decimal::Decimal::ONE
                } else {
                    rust_decimal::Decimal::ONE + mwst_rate
                };
                let gap_netto = if divisor.is_zero() {
                    gap_brutto
                } else {
                    (gap_brutto / divisor).round_kfm(5)
                };

                // Only the Tax positions are recomputed: the top-up widens the
                // tax base, and nothing else about the invoice changes. Every
                // other position — the Abschlag deductions of Pass 3 included —
                // carries over untouched, so each advance stays deducted
                // exactly once whether or not the top-up fires.
                let mut positions2: Vec<BillingPosition> = positions
                    .iter()
                    .filter(|p| p.category != crate::position::PositionCategory::Tax)
                    .cloned()
                    .collect();
                let mut topup = crate::position::BillingPosition::debit(
                    format!("Mindestbetrag (Minimum {min_brutto:.2}\u{202f}EUR brutto)"),
                    rust_decimal::Decimal::ONE,
                    "EUR",
                    gap_netto,
                    crate::position::PositionCategory::Commodity,
                )
                .with_legal_basis("Vertraglich")
                .with_tag("mindestbetrag");
                // Stamp the rate the top-up is agreed at, so the MwSt pass and
                // the BG-23 breakdown put it in the right bucket instead of the
                // engine default.
                if let Some(rate) = ctx.minimum_invoice_mwst_rate.or(charged_rate) {
                    topup = topup.with_tax_rate(rate);
                }
                // The top-up is a supply position like any other: §13b covers it too.
                if ctx.reverse_charge {
                    topup = topup.with_reverse_charge();
                }
                positions2.push(topup);
                let pre_tax2: Vec<BillingPosition> = positions2.clone();
                for provider in self.providers.iter().filter(|p| p.is_tax_pass()) {
                    let new = provider.bill(&ctx, quantities, &pre_tax2)?;
                    positions2.extend(new);
                }
                // Fall through — a Stornorechnung of a topped-up invoice must
                // still be negated by Pass 5.
                positions = positions2;
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
/// One check: § 38 Abs. 4 EnWG ends the Ersatzversorgung „spätestens aber drei
/// Monate nach Beginn der Ersatzenergieversorgung", so a longer period
/// describes a supply that cannot legally exist and blocks the run (`Error`
/// severity). Bill the first three months as Ersatzversorgung and the remainder
/// under the regime the supply actually continued in.
///
/// The three months run from the day the **supply** began
/// ([`vertragsbeginn`](BillingContext::vertragsbeginn)), which is the day
/// § 38 Abs. 1 EnWG attaches the Ersatzversorgung to. Measuring from the
/// invoice period start makes every period its own beginning, so a
/// monthly-billed Ersatzversorgung would never reach the limit however long it
/// runs.
///
/// An Ersatzversorgung is by definition a supply with no assignable contract,
/// so the supply start is exactly the fact most often missing — and the period
/// being billed is itself the supply. Without a stated start the period's own
/// first day anchors the limit: it is never later than the true one, so the
/// three months can only be reported early, never missed. That the anchor was
/// assumed is reported alongside.
fn context_warnings(ctx: &BillingContext) -> Vec<BillingWarning> {
    let mut warnings = Vec::new();
    if ctx.vertragsart == crate::context::Vertragsart::Ersatzversorgung {
        let beginn = ctx.vertragsbeginn.unwrap_or_else(|| ctx.period_from());
        if ctx.vertragsbeginn.is_none() {
            warnings.push(BillingWarning {
                code: "ERSATZVERSORGUNG_BEGINN_FEHLT",
                severity: WarningSeverity::Warning,
                message: format!(
                    "Ersatzversorgung ohne Belieferungsbeginn im Kontext: die \
                     Drei-Monats-Grenze des § 38 Abs. 4 EnWG wird ab dem \
                     Zeitraumbeginn {beginn} gemessen — vertragsbeginn setzen, \
                     wenn die Belieferung früher begann"
                ),
            });
        }
        // Three months after the first day of supply; the Ersatzversorgung may
        // run through the day before.
        let limit = add_months(beginn, 3);
        if ctx.period_to() >= limit {
            warnings.push(BillingWarning {
                code: "ERSATZVERSORGUNG_UEBER_3_MONATE",
                severity: WarningSeverity::Error,
                message: format!(
                    "Ersatzversorgung endet spätestens drei Monate nach Beginn \
                     der Belieferung am {beginn} (§ 38 Abs. 4 EnWG): Zeitraum \
                     {}..{} überschreitet die Grenze {limit}",
                    ctx.period_from(),
                    ctx.period_to(),
                ),
            });
        }
    }
    warnings
}

/// `KEINE_MENGE` — every metered source of a multi-day period reads zero.
///
/// The quantity twin of the `KEIN_ARBEITSPREIS` family: a period longer than a
/// day that bills no commodity at all charges the standing charges and nothing
/// else, and the resulting invoice reads as ordinary. A single day can
/// legitimately be empty, and so can a vacant delivery point over a longer one —
/// which is why this is a finding and not a refusal. Whether a reading was
/// *missing* rather than genuinely zero is a question only the caller that
/// resolved it can answer, and `billingd` refuses there with `NO_METER_DATA`.
fn zero_quantity_warning(ctx: &BillingContext, quantities: &Quantities) -> Option<BillingWarning> {
    if ctx.days() <= 1 {
        return None;
    }
    let empty = quantities.empty_energy_sources();
    if empty.is_empty() {
        return None;
    }
    Some(BillingWarning {
        code: "KEINE_MENGE",
        severity: WarningSeverity::Warning,
        message: format!(
            "kein Verbrauch im Zeitraum {}..{}: {} liefer(n) 0 — die Rechnung stellt \
             nur Grund- und Leistungspreise. Fehlt die Ablesung, ist die Menge \
             nachzuliefern, bevor abgerechnet wird",
            ctx.period_from(),
            ctx.period_to(),
            empty.join(", "),
        ),
    })
}

/// `MENGE_UNVOLLSTAENDIG` — the period was not fully delivered.
///
/// A sum over the readings that arrived says nothing about the ones that did
/// not, and it says it invisibly: the Arbeitsmenge of a month delivered up to
/// the 3rd is a perfectly ordinary number.
///
/// § 40a Abs. 2 EnWG is what makes such a period billable at all — where the
/// supplier cannot determine the actual consumption for reasons it does not
/// answer for, the invoice „darf … auf einer Verbrauchsschätzung beruhen, die
/// unter angemessener Berücksichtigung der tatsächlichen Verhältnisse zu
/// erfolgen hat", and Satz 3 requires the estimate, the ground for it and the
/// factors behind it to be stated on the document „unter ausdrücklichem und
/// optisch besonders hervorgehobenem Hinweis". A finding rather than a refusal
/// for that reason: the gap is to be estimated and labelled, not left
/// uninvoiced while the § 40c Abs. 2 EnWG clock runs.
fn coverage_warning(quantities: &Quantities) -> Vec<BillingWarning> {
    let full = rust_decimal::Decimal::ONE_HUNDRED;
    [
        (
            "Strom",
            quantities.electricity.as_ref().and_then(|m| m.coverage_pct),
        ),
        ("Gas", quantities.gas.as_ref().and_then(|m| m.coverage_pct)),
    ]
    .into_iter()
    .filter_map(|(label, pct)| {
        let pct = pct?;
        (pct < full).then(|| BillingWarning {
            code: "MENGE_UNVOLLSTAENDIG",
            severity: WarningSeverity::Warning,
            message: format!(
                "{label}: nur {pct} % des Abrechnungszeitraums sind durch abrechenbare \
                 Messwerte gedeckt — die Lücke ist nach § 40a Abs. 2 EnWG zu schätzen \
                 und die Schätzung auf der Rechnung hervorgehoben auszuweisen"
            ),
        })
    })
    .collect()
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
