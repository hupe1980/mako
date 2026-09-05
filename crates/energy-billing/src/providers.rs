//! Concrete `BillingProvider` implementations for all product types.
//!
//! Each provider corresponds to one product category. Build providers from
//! a `TariffInput` (the product definition from `productd`) and register them
//! with `BillingEngine`.

use crate::rates::RoundMoney;
use billing::{Currency, DynamicPricing, RateBand, RateSchedule, TimeBand, TimeOfUsePricing};
use rust_decimal::Decimal;
use rust_decimal::dec;

use crate::context::BillingContext;
use crate::error::EngineError;
use crate::position::{
    BillingPosition, BillingWarning, PositionCategory, WarningSeverity, arbeitspreis_position,
    grundpreis_position, levy_position, validated_eur,
};
use crate::provider::BillingProvider;
use crate::quantities::{GridInput, Quantities};
use crate::tariff::{
    AbwasserRegime, ControllableLoadProduct, EegProduct, EinspeisungProduct, ElectricityProduct,
    EmobilityProduct, GasProduct, HeatProduct, HemsProduct, ServiceProduct, SharingProduct,
    SolarProduct, WaterProduct,
};

// ── ElectricityProvider ───────────────────────────────────────────────────────

/// STROM / WAERMEPUMPE / WALLBOX billing provider.
///
/// Produces commodity positions (Grundpreis, Arbeitspreis HT/NT, §14a credits).
/// Does NOT include MwSt — add `MwStProvider` to the engine.
/// Stromsteuer is included as a levy position.
pub struct ElectricityProvider {
    product: ElectricityProduct,
    grid: GridInput,
}

impl ElectricityProvider {
    #[must_use]
    pub fn new(product: ElectricityProduct, grid: GridInput) -> Self {
        Self { product, grid }
    }

    /// Construct from a [`Product`](crate::Product) by extracting the electricity variant.
    /// Accepts `Strom`, `Waermepumpe`, `Wallbox` (uses `.base`), and `Sharing` (uses `.electricity`).
    ///
    /// # Panics
    /// Panics when the `Product` variant is not electricity-compatible.
    #[must_use]
    pub fn from_product(product: &crate::tariff::Product, grid: GridInput) -> Self {
        use crate::tariff::Product;
        match product {
            Product::Strom(p) => Self::new(p.clone(), grid),
            Product::Waermepumpe(c) | Product::Wallbox(c) => Self::new(c.base.clone(), grid),
            Product::Sharing(s) => Self::new(s.electricity.clone(), grid),
            other => panic!(
                "ElectricityProvider::from_product: incompatible product category '{}'",
                other.category_str()
            ),
        }
    }
}

impl BillingProvider for ElectricityProvider {
    fn validate_warnings(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        let mut w = Vec::new();
        let meter = quantities.electricity.as_ref();

        // A commodity product must be able to price its commodity.
        //
        // Without this, a `StromProduct` carrying no Arbeitspreis at all — every
        // price field `None` — billed 1000 kWh for €20.50: the Stromsteuer, and
        // nothing for the electricity. No error, no warning, an invoice that
        // looks ordinary. That is not a hypothetical: the price fields are
        // populated by mapping `productd`'s `preistyp` strings onto struct
        // fields, and a renamed or missing position maps to `None` in silence.
        //
        // Error severity, so `bill()` refuses. A product that genuinely charges
        // no work price still states one (`0.0`); the missing case is a data
        // defect, and a zero is how an operator says they mean it.
        //
        // An `indexed_price` counts only when its index value has actually
        // arrived: `effective_ct_per_kwh()` returns `None` without one, the
        // provider then adds no Arbeitspreis position, and the invoice looks
        // exactly like the priceless-product case this guard exists to catch.
        // Counting the *presence* of the config let a stale index feed produce
        // a Grundpreis-only B2B invoice with a clean bill of health.
        let has_any_work_price = self.product.arbeitspreis_ct_per_kwh.is_some()
            || self.product.arbeitspreis_ht_ct_per_kwh.is_some()
            || self.product.arbeitspreis_nt_ct_per_kwh.is_some()
            || self.product.dynamic_epex
            || self
                .product
                .indexed_price
                .as_ref()
                .is_some_and(|i| i.effective_ct_per_kwh().is_some())
            || self
                .product
                .seasonal_prices
                .as_ref()
                .is_some_and(|s| !s.is_empty())
            || self
                .product
                .block_tiers
                .as_ref()
                .is_some_and(|t| !t.is_empty());
        w.extend(indexwert_warning(
            self.product.indexed_price.as_ref(),
            has_any_work_price,
        ));
        if !has_any_work_price {
            w.push(BillingWarning {
                code: "KEIN_ARBEITSPREIS",
                severity: WarningSeverity::Error,
                message: "the product carries no Arbeitspreis in any form (Eintarif, HT/NT, \
                          dynamic, indexed, seasonal or tiered) — the invoice would charge \
                          the Stromsteuer and nothing for the electricity. Check the \
                          productd product's price positions."
                    .to_owned(),
            });
        }

        // § 40 Abs. 2 Nr. 6 EnWG requires the invoice to state *how* the reading
        // was obtained. An unstated Ablesungsart leaves that sentence off the
        // page — a Pflichtangabe missing, not a cosmetic gap — so it is flagged
        // where a reading is actually being billed.
        if meter.is_some_and(|m| {
            (m.zaehlerstand_von.is_some() || m.zaehlerstand_bis.is_some())
                && m.ablesungsart == crate::quantities::Ablesungsart::Unbekannt
        }) {
            w.push(BillingWarning {
                code: "ABLESUNGSART_FEHLT",
                severity: WarningSeverity::Warning,
                message: "§ 40 Abs. 2 Nr. 6 EnWG verlangt die Angabe, wie der Zählerstand \
                          ermittelt wurde — `ablesungsart` ist nicht gesetzt, die Rechnung \
                          nennt sie daher nicht"
                    .to_owned(),
            });
        }

        // An estimated reading is billable (§ 40a Abs. 2 EnWG), but the caller
        // must know it happened: the customer can demand a corrected invoice
        // once a real reading arrives, so dispatch systems treat it differently.
        // A finding rather than an Info position alone, which paper shows and
        // code cannot see.
        if meter.is_some_and(|m| m.is_estimated) {
            w.push(BillingWarning {
                code: "ESTIMATED_READING",
                severity: WarningSeverity::Warning,
                message: "billed on an estimated reading (§ 40a Abs. 2 EnWG) — \
                          expect a correction when the real reading arrives"
                    .to_owned(),
            });
        }

        // A price guarantee that ends inside or within 30 days of the billed
        // period is something the operator wants to see before dispatch.
        if let Some(bis) = self.product.preisgarantie_bis
            && bis <= ctx.period_to() + time::Duration::days(30)
        {
            w.push(BillingWarning {
                code: "PREISGARANTIE_ENDET",
                severity: WarningSeverity::Warning,
                message: format!(
                    "the price guarantee ends {bis}, within 30 days of the billed \
                     period — verify the follow-on price was communicated"
                ),
            });
        }

        // A consumption deviation beyond 50 % of the prior year is the standard
        // plausibility threshold before an invoice goes out: it usually means a
        // meter fault, a reading transposition, or a tenant change nobody booked.
        if let (Some(m), Some(vh)) = (meter, ctx.verbrauchshistorie.as_ref())
            && let Some(vorjahr) = vh.vorjahr_kwh
            && vorjahr > Decimal::ZERO
        {
            let deviation = ((m.arbeitsmenge_kwh - vorjahr) / vorjahr).abs();
            if deviation > dec!(0.5) {
                w.push(BillingWarning {
                    code: "VERBRAUCH_ABWEICHUNG_50PCT",
                    severity: WarningSeverity::Warning,
                    message: format!(
                        "consumption {} kWh deviates {:.0}% from the prior year's \
                         {vorjahr} kWh — verify the reading before dispatch",
                        m.arbeitsmenge_kwh,
                        deviation * dec!(100)
                    ),
                });
            }
        }

        // A product must be able to price the quantities it is *given*, not just
        // carry a price field.
        //
        // A `Zweitarif` product prices HT and NT and nothing else. Handed a
        // meter that reports only a total — which is what `edmd` returns
        // whenever the register split did not arrive — the HT/NT branch does not
        // fire, no other branch matches, and the invoice bills 1000 kWh for
        // €20.50: the Stromsteuer, and nothing for the electricity. That is the
        // priceless-product defect exactly, reached from the other side, and
        // `KEIN_ARBEITSPREIS` waves it through because `arbeitspreis_ht…` is
        // populated.
        let p = &self.product;
        let prices_only_ht_nt = p.arbeitspreis_ct_per_kwh.is_none()
            && (p.arbeitspreis_ht_ct_per_kwh.is_some() || p.arbeitspreis_nt_ct_per_kwh.is_some())
            && !p.dynamic_epex
            && p.block_tiers.as_ref().is_none_or(|t| t.is_empty())
            && p.seasonal_prices.as_ref().is_none_or(|s| s.is_empty())
            && p.indexed_price.is_none();
        let has_split = meter
            .is_some_and(|m| m.arbeitsmenge_ht_kwh.is_some() && m.arbeitsmenge_nt_kwh.is_some());
        let has_quantity = meter.is_some_and(|m| m.billable_kwh() > Decimal::ZERO);
        if prices_only_ht_nt && !has_split && has_quantity {
            w.push(BillingWarning {
                code: "ZWEITARIF_OHNE_HT_NT_AUFTEILUNG",
                severity: WarningSeverity::Error,
                message: "the product prices only HT and NT, and the meter reports a single \
                          total with no HT/NT split — the consumption cannot be priced at \
                          all, and the invoice would carry the levies and nothing for the \
                          electricity. Supply arbeitsmenge_ht_kwh/arbeitsmenge_nt_kwh, or \
                          give the product an Eintarif Arbeitspreis."
                    .to_owned(),
            });
        }

        // Half a Zweitarif prices one band and not the other. There is no
        // sensible reading of that: billing the unpriced band at the other's
        // rate invents a price, and dropping it under-bills.
        let ht_priced = p.arbeitspreis_ht_ct_per_kwh.is_some();
        let nt_priced = p.arbeitspreis_nt_ct_per_kwh.is_some();
        if ht_priced != nt_priced {
            w.push(BillingWarning {
                code: "ZWEITARIF_UNVOLLSTAENDIG",
                severity: WarningSeverity::Error,
                message: format!(
                    "the product prices the {} band and not the {} one — a Zweitarif needs \
                     both, and neither inventing the missing price nor dropping the band is \
                     a lawful reading",
                    if ht_priced { "HT" } else { "NT" },
                    if ht_priced { "NT" } else { "HT" },
                ),
            });
        }

        // An HT/NT split that does not add up to the stated total prices one of
        // the two figures wrongly, and which one is not knowable here.
        if let Some(m) = meter
            && let (Some(ht), Some(nt)) = (m.arbeitsmenge_ht_kwh, m.arbeitsmenge_nt_kwh)
            && m.arbeitsmenge_kwh > Decimal::ZERO
        {
            let split = ht + nt;
            let gap = (split - m.arbeitsmenge_kwh).abs();
            // A tenth of a kWh over a billing period is measurement noise; more
            // is a register that was not reconciled.
            if gap > dec!(0.1) {
                w.push(BillingWarning {
                    code: "HT_NT_SUMME_WEICHT_AB",
                    severity: WarningSeverity::Error,
                    message: format!(
                        "HT ({ht}) + NT ({nt}) = {split} kWh does not match the stated total \
                         {} kWh — one of the registers is wrong and the invoice would bill \
                         the difference at whichever rate happens to apply",
                        m.arbeitsmenge_kwh
                    ),
                });
            }
        }

        // Electricity was 16 % in H2/2020 (§28 Abs. 1 UStG a.F.), 19 % otherwise.
        // A period straddling that boundary has no single correct rate — split at
        // the Stichtag and merge, the same discipline the gas/heat providers apply.
        if crate::rates::mwst_rate_for_period(ctx.period_from(), ctx.period_to()).is_none() {
            w.push(BillingWarning {
                code: "MWST_STICHTAG_IM_ZEITRAUM",
                severity: WarningSeverity::Warning,
                message: "Abrechnungszeitraum überschreitet eine USt-Satzgrenze für Strom \
                          (§28 UStG) — am Stichtag splitten und Teilrechnungen zusammenführen"
                    .to_owned(),
            });
        }
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.electricity.as_ref().cloned().unwrap_or_default();
        // The consumption to bill: the stated total, or the HT/NT registers when
        // the caller supplied only those. Everything downstream — NNE, KA,
        // Stromsteuer — is charged on the same figure the Arbeitspreis is.
        let kwh = meter.billable_kwh();
        let product = &self.product;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;
        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── Resolve seasonal arbeitspreis ──────────────────────────────────────
        // When seasonal_prices is set, the price for the billing month is looked up.
        // Uses ctx.period_from() month as the representative month for the period.
        let billing_month = ctx.period_from().month() as u8;
        let seasonal_arbeitspreis = product.seasonal_prices.as_ref().and_then(|seasons| {
            seasons
                .iter()
                .find(|s| s.contains_month(billing_month))
                .and_then(|s| s.arbeitspreis_ct_per_kwh)
        });

        // ── Prosumer billing path ──────────────────────────────────────────────
        // When prosumer meter data is provided, bill only grid_consumption.
        // Self-consumed electricity is Stromsteuer-exempt (§ 9 Abs. 1 Nr. 3 StromStG)
        // and does NOT attract NNE charges.
        if let Some(p) = &quantities.prosumer {
            return self.bill_prosumer(ctx, p, product, grid, rates, seasonal_arbeitspreis);
        }

        // ── Grundpreis ─────────────────────────────────────────────────────────
        if let Some(gp_ct_day) = product.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis",
                    gp_ct_day / dec!(100),
                    ctx.prorate_days().0 as i64,
                    "§41 EnWG",
                    &["strom"],
                )
                .with_tag("strom"),
            );
        }

        // ── Arbeitspreis ───────────────────────────────────────────────────────
        // Any billable quantity opens the block, not the total alone: a caller
        // that supplies the HT/NT registers and leaves `arbeitsmenge_kwh` at
        // zero has still delivered electricity, and gating on the total billed
        // them nothing for it.
        if meter.billable_kwh() > Decimal::ZERO {
            if let Some(tiers) = product.block_tiers.as_ref().filter(|t| !t.is_empty()) {
                // Delegate to billing::RateSchedule for correct graduated pricing.
                // Replaces manual tier iteration — gains contiguous-band validation
                // and exact Amount<5> arithmetic. Legal basis: §41 EnWG.
                positions.extend(build_block_tariff_positions(tiers, kwh, &[])?);
            } else if let (Some(ht), Some(nt), true) = (
                meter.arbeitsmenge_ht_kwh,
                meter.arbeitsmenge_nt_kwh,
                // …and the *product* prices **both** bands. Selecting the arm
                // on the meter alone would send a two-register meter on a
                // single-rate tariff down it, where there are no band prices to
                // build — leaving the electricity unbilled while the Stromsteuer
                // is charged. A half-priced Zweitarif is refused earlier, by
                // `validate_warnings`.
                product.arbeitspreis_ht_ct_per_kwh.is_some()
                    && product.arbeitspreis_nt_ct_per_kwh.is_some(),
            ) {
                // Zweitarif (HT/NT) — billing::TimeOfUsePricing for validated band arithmetic.
                // Negative quantities return Err; zero quantities are skipped silently.
                let mut bands = Vec::new();
                if let Some(ap_ht) = product.arbeitspreis_ht_ct_per_kwh {
                    let price = billing::Amount::<5>::try_from((ap_ht / dec!(100)).round_kfm(5))
                        .map_err(|_| EngineError::PriceOutOfRange {
                            field: "arbeitspreis_ht_ct_per_kwh".to_owned(),
                            value: ap_ht,
                        })?;
                    bands.push(TimeBand::new("HT", price));
                }
                if let Some(ap_nt) = product.arbeitspreis_nt_ct_per_kwh {
                    let price = billing::Amount::<5>::try_from((ap_nt / dec!(100)).round_kfm(5))
                        .map_err(|_| EngineError::PriceOutOfRange {
                            field: "arbeitspreis_nt_ct_per_kwh".to_owned(),
                            value: ap_nt,
                        })?;
                    bands.push(TimeBand::new("NT", price));
                }
                if !bands.is_empty() {
                    let items = TimeOfUsePricing::builder()
                        .bands(bands)
                        .unit("kWh")
                        .currency(Currency::EUR)
                        .build()?
                        .calculate(&[("HT", ht), ("NT", nt)])?;
                    for item in items {
                        let is_ht = item.has_tag("HT");
                        let label = if is_ht {
                            "Arbeitspreis Hochtarif (HT)"
                        } else {
                            "Arbeitspreis Niedertarif (NT)"
                        };
                        let band_tag = if is_ht { "ht" } else { "nt" };
                        let mut pos = billing_item_to_position(
                            item,
                            PositionCategory::Commodity,
                            "§41 EnWG",
                            &["strom", "arbeitspreis"],
                        );
                        pos.description = label.to_owned();
                        pos.tags.push(band_tag.to_owned());
                        positions.push(pos);
                    }
                }
            } else if let Some((effective_ct, idx)) = product
                .indexed_price
                .as_ref()
                .and_then(|idx| idx.effective_ct_per_kwh().map(|ct| (ct, idx)))
            {
                // ── Indexed price (B2B, §41 EnWG Sonderkundenvertrag) ─────────
                // Effective price = base + spread + index_value × factor.
                // Ahead of the static prices, not behind them: a product that
                // carries both agreed the indexed one, and resolving to the
                // static fallback would bill a price the contract does not
                // contain. When the index has not arrived, `validate_warnings`
                // has already refused the run (`INDEXWERT_FEHLT`) unless another
                // price is contracted alongside.
                positions.push(
                    arbeitspreis_position(
                        idx.position_description(),
                        kwh,
                        effective_ct,
                        "kWh",
                        "§41 EnWG",
                        &["strom", "indexed"],
                    )
                    .with_tag("strom")
                    .with_tag("indexed_price"),
                );
            } else if let Some(ap_ct) = seasonal_arbeitspreis.or(product.arbeitspreis_ct_per_kwh) {
                // Use seasonal price when available, otherwise base tariff price.
                let label = if seasonal_arbeitspreis.is_some() {
                    product
                        .seasonal_prices
                        .as_ref()
                        .and_then(|s| s.iter().find(|p| p.contains_month(billing_month)))
                        .and_then(|s| s.label.as_deref())
                        .map(|l| format!("Arbeitspreis Strom ({l})"))
                        .unwrap_or_else(|| "Arbeitspreis Strom (Saisontarif)".to_owned())
                } else {
                    "Arbeitspreis Strom".to_owned()
                };
                positions.push(
                    arbeitspreis_position(label, kwh, ap_ct, "kWh", "§41 EnWG", &["strom"])
                        .with_tag("strom"),
                );
            }
        }

        // ── EEG-Gutschrift pass-through ────────────────────────────────────────
        // The feed-in is a separate supply with its own USt status: for a §19
        // Kleinunternehmer operator it carries 0 %, so the credit must not net
        // against the standard-rate consumption base — that would understate the
        // supplier's own output VAT by the standard rate on the credit.
        if let Some(eeg_ct) = quantities.eeg_gutschrift_eur
            && eeg_ct != Decimal::ZERO
        {
            let mut p = BillingPosition::credit(
                "EEG-Gutschrift (Photovoltaik)",
                Decimal::ONE,
                "EUR",
                eeg_ct.abs(),
                PositionCategory::Credit,
            )
            // §19 Abs. 1 EEG 2023 is the Zahlungsanspruch itself. Which
            // Veräußerungsform it takes — §20 Marktprämie or §21 Abs. 1
            // Einspeisevergütung — is the plant's, and `einsd` decides it; this
            // position is the pass-through of whatever einsd computed, so the
            // anchor must be the entitlement rather than one of its two forms.
            // (It read "§38 EEG 2023", which is Zahlungsberechtigung für
            // Solaranlagen des ersten Segments — an auction provision that has
            // nothing to do with a rooftop feed-in credit.)
            .with_legal_basis("§19 Abs. 1 EEG 2023")
            .with_tag("eeg_gutschrift")
            .with_tag("solar");
            if product.eeg_gutschrift_kleinunternehmer_19_ustg {
                p = p.with_tax_rate(Decimal::ZERO);
            }
            positions.push(p);
        }

        // ── Grid charges (NNE / KA) ────────────────────────────────────────────
        let kwh_for_grid = kwh;
        if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
            // Leap-aware: an EUR/year rate divides by that year's actual days
            // (366 in 2024/2028), or the daily rate overstates the Grundpreis.
            let daily = nne_gp / Decimal::from(time::util::days_in_year(ctx.period_from().year()));
            // Active contract days, not the full billing period: the NNE
            // Grundpreis accrues only while the contract supplies the MaLo, the
            // same clipping the commodity Grundpreis applies. Billing the full
            // period over-charged every mid-period move-in and move-out.
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Grundpreis",
                    Decimal::from(ctx.prorate_days().0),
                    "Tage",
                    daily,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_grundpreis")
                .with_tag("nne"),
            );
        }
        if let Some(nne_ap_ct) = grid.nne_arbeitspreis_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Arbeitspreis",
                    kwh_for_grid,
                    "kWh",
                    nne_ap_ct / dec!(100),
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_arbeitspreis")
                .with_tag("nne"),
            );
        }
        if let (Some(nne_lp), Some(kw)) = (
            grid.nne_leistungspreis_eur_per_kw_year,
            meter.spitzenleistung_kw,
        ) {
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Leistungspreis",
                    kw,
                    "kW",
                    nne_lp * ctx.billed_years(),
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_leistungspreis")
                .with_tag("nne"),
            );
        }
        if let Some(ka_ct) = grid.ka_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Konzessionsabgabe",
                    kwh_for_grid,
                    "kWh",
                    ka_ct / dec!(100),
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("KAV §2")
                .with_tag("konzessionsabgabe")
                .with_tag("nne"),
            );
        }

        // ── RLM Leistungspreis (demand charge) ────────────────────────────────
        // For large commercial customers on RLM metering (≥100 MWh/year) with
        // a capacity-based Leistungspreis in the supply contract.
        //
        // Billed on Spitzenleistung (peak demand, kW). The rate is per kW *and
        // month*, so it scales with the billed period's month fraction — an
        // annual invoice owes twelve months of it, a half-month move-out half
        // of one. Every capacity rate in the crate prorates to the period it is
        // billed for; only the unit differs (the NNE Leistungspreis is per
        // kW-year and scales in years).
        if let (Some(lp_ct_per_kw_month), Some(kw)) = (
            product.leistungspreis_strom_ct_per_kw_month,
            meter.spitzenleistung_kw.filter(|kw| *kw > Decimal::ZERO),
        ) {
            positions.push(
                BillingPosition::debit(
                    "Leistungspreis",
                    kw,
                    "kW",
                    lp_ct_per_kw_month / dec!(100) * ctx.billed_months(),
                    PositionCategory::Commodity,
                )
                .with_legal_basis("§41 EnWG")
                .with_tag("leistungspreis")
                .with_tag("rlm"),
            );
        }

        // ── Stromsteuer ────────────────────────────────────────────────────────
        positions.extend(stromsteuer_positions(
            product.stromsteuer_tarif,
            kwh,
            rates.effective_stromsteuer(product.stromsteuer_ct_per_kwh_override),
            &["strom"],
        ));
        // A Steuerentlastung leaves the levy where it is and tells the customer
        // what to file — see `crate::steuer`.
        positions.extend(entlastungs_hinweise(
            &product.steuerentlastungen,
            &positions,
        ));

        // ── AufAbschlag / Rabatt ───────────────────────────────────────────────
        // Per-unit discount or surcharge applied after all commodity positions.
        // Negative value = customer discount; positive = surcharge.
        if let Some(aa_ct) = product
            .auf_abschlag_ct_per_kwh
            .filter(|v| *v != Decimal::ZERO)
            && kwh > Decimal::ZERO
        {
            let (label, cat) = if aa_ct < Decimal::ZERO {
                ("Rabatt (Arbeitspreis)", PositionCategory::Discount)
            } else {
                ("Aufschlag (Arbeitspreis)", PositionCategory::Levy)
            };
            positions.push(
                BillingPosition::debit(
                    label,
                    kwh,
                    "kWh",
                    aa_ct / dec!(100), // ct/kWh → EUR/kWh
                    cat,
                )
                .with_tag("auf_abschlag"),
            );
        }
        if let Some(aa_month) = product
            .auf_abschlag_eur_per_month
            .filter(|v| *v != Decimal::ZERO)
        {
            let months_frac = ctx.billed_months();
            let eur = crate::position::validated_eur(aa_month * months_frac);
            let (label, cat) = if aa_month < Decimal::ZERO {
                (
                    "Rabatt (monatlicher Festbetrag)",
                    PositionCategory::Discount,
                )
            } else {
                ("Aufschlag (monatlicher Festbetrag)", PositionCategory::Levy)
            };
            // `eur` already carries the sign of `aa_month`: a negative monthly
            // amount is a Rabatt and must stay negative. Re-negating it billed
            // every monthly discount as a surcharge of the same size — the
            // label said "Rabatt" while the line added money. The gas provider
            // never had the extra factor, which is why only electricity was hit.
            positions.push(BillingPosition {
                description: label.to_owned(),
                legal_basis: None,
                quantity: months_frac,
                unit: "Monat".to_owned(),
                unit_price_eur: aa_month,
                net_eur: eur,
                category: cat,
                tags: vec!["auf_abschlag".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        positions.extend(electricity_common_positions(ctx, product, &meter));

        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }

        Ok(positions)
    }
}

impl ElectricityProvider {
    /// Prosumer billing path — bills grid consumption only.
    ///
    /// Self-consumed energy is shown as an informational position (§41 EnWG transparency)
    /// but does NOT attract commodity charges, NNE, or Stromsteuer.
    fn bill_prosumer(
        &self,
        ctx: &BillingContext,
        prosumer: &crate::quantities::ProsumerMeterInput,
        product: &ElectricityProduct,
        grid: &GridInput,
        rates: &crate::rates::RegulatoryRates,
        seasonal_arbeitspreis: Option<Decimal>,
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let mut positions: Vec<BillingPosition> = Vec::new();
        let grid_kwh = prosumer.grid_consumption_kwh;
        let self_kwh = prosumer.self_consumption_kwh;

        // Grundpreis over the active contract days, independent of the
        // consumption split — the same clipping the non-prosumer path applies.
        if let Some(gp_ct_day) = product.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis",
                    gp_ct_day / dec!(100),
                    ctx.prorate_days().0 as i64,
                    "§41 EnWG",
                    &["strom"],
                )
                .with_tag("strom"),
            );
        }

        // Arbeitspreis on grid consumption only
        if grid_kwh > Decimal::ZERO {
            if let Some(ap_ct) = seasonal_arbeitspreis.or(product.arbeitspreis_ct_per_kwh) {
                let label = if seasonal_arbeitspreis.is_some() {
                    "Arbeitspreis Strom Netzbezug (Saisontarif)".to_owned()
                } else {
                    "Arbeitspreis Strom (Netzbezug)".to_owned()
                };
                positions.push(
                    arbeitspreis_position(label, grid_kwh, ap_ct, "kWh", "§41 EnWG", &["strom"])
                        .with_tag("strom"),
                );
            }
            // NNE on grid consumption only
            if let Some(nne_ap_ct) = grid.nne_arbeitspreis_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Netznutzungsentgelt Arbeitspreis (Netzbezug)",
                        grid_kwh,
                        "kWh",
                        nne_ap_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("StromNEV")
                    .with_tag("nne_arbeitspreis")
                    .with_tag("nne"),
                );
            }
            // Stromsteuer on grid consumption only (§ 9 Abs. 1 Nr. 3 StromStG:
            // self-consumption exempt)
            let st_rate = rates.effective_stromsteuer(product.stromsteuer_ct_per_kwh_override);
            if st_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "Stromsteuer (Netzbezug)",
                        grid_kwh,
                        "kWh",
                        st_rate,
                        "§3 StromStG",
                        "stromsteuer",
                    )
                    .with_tag("strom"),
                );
            }
        }

        // Informational: self-consumption and energy balance
        if self_kwh > Decimal::ZERO {
            let self_supply_pct = (prosumer.self_supply_ratio() * dec!(100)).round_kfm(1);
            positions.push(BillingPosition {
                description: format!(
                    "Eigenverbrauch PV: {self_kwh:.3}\u{202f}kWh (Selbstversorgungsgrad {self_supply_pct:.1}\u{202f}%)",
                ),
                // § 9 Abs. 1 Nr. 3 StromStG — an installation up to 2 MW,
                // consumed by the operator or drawn in the räumlicher
                // Zusammenhang. Not § 9a, which is a Steuerentlastung for
                // industrial processes.
                legal_basis: Some(
                    "\u{a7} 9 Abs. 1 Nr. 3 StromStG (Stromsteuerfreiheit Eigenverbrauch, \
                     Anlage bis 2\u{202f}MW)"
                        .to_owned(),
                ),
                quantity: self_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["eigenverbrauch".to_owned(), "prosumer".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }
        if let Some(export) = prosumer.export_kwh.filter(|&e| e > Decimal::ZERO) {
            positions.push(BillingPosition {
                description: format!("Netzeinspeisung PV: {export:.3}\u{202f}kWh (Abrechnung via EEG-Vergütung separat)"),
                legal_basis: Some("\u{a7}41 EnWG".to_owned()),
                quantity: export,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["einspeisung".to_owned(), "prosumer".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // Wire tax rate (same as normal path)
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── ControllableLoadProvider ──────────────────────────────────────────────────

/// §14a EnWG controllable load billing provider (WAERMEPUMPE / WALLBOX).
///
/// Delegates standard electricity billing to [`ElectricityProvider`] and then
/// appends the §14a credits (Modul 1 pauschale Reduzierung, Modul 2 Arbeitspreis-
/// reduzierung, Modul 3 zeitvariable Bänder, plus any Steuerungsentschädigung)
/// credit positions.
///
/// ## Legal basis
///
/// §14a Abs. 1 EnWG (BK6-22-024 §2.13): DSOs must offer controllable load
/// (Steuerbare Verbrauchseinrichtungen) customers a reduced NNE (Modul 1, 2 or 3).
/// The LF reflects this reduction as a credit on the retail invoice.
pub struct ControllableLoadProvider {
    product: ControllableLoadProduct,
    grid: GridInput,
}

impl ControllableLoadProvider {
    #[must_use]
    pub fn new(product: ControllableLoadProduct, grid: GridInput) -> Self {
        Self { product, grid }
    }
}

impl BillingProvider for ControllableLoadProvider {
    fn validate_warnings(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        // The base electricity checks apply to the underlying supply.
        let base = ElectricityProvider::new(self.product.base.clone(), self.grid.clone());
        let mut w = base.validate_warnings(ctx, quantities);

        // BK6-22-300 offers one base module and one optional addition. Modul 1
        // and Modul 2 are the two forms the base takes and the Anschlussnutzer
        // picks one; Modul 3 adds to Modul 1 alone. So `Modul 1 + Modul 3` is
        // the only pair, and the three other pairings each reduce the same
        // network usage twice.
        if self.product.sect14a_modul1_pauschale_eur_per_year.is_some()
            && self
                .product
                .sect14a_modul2_nne_reduktion_ct_per_kwh
                .is_some()
        {
            w.push(BillingWarning {
                code: "MODUL1_AND_MODUL2",
                severity: WarningSeverity::Error,
                message: "§14a EnWG Modul 1 (pauschale Reduzierung) and Modul 2 \
                          (prozentuale Arbeitspreisreduzierung) are both configured — \
                          BK6-22-300 offers them as alternative base modules, so the \
                          Anschlussnutzer holds one. Billing both grants the same \
                          Steuerbarkeit two reductions."
                    .to_owned(),
            });
        }

        if self
            .product
            .sect14a_modul2_nne_reduktion_ct_per_kwh
            .is_some()
            && self.product.sect14a_modul3_nne_ht_ct_per_kwh.is_some()
        {
            w.push(BillingWarning {
                code: "MODUL2_AND_MODUL3",
                severity: WarningSeverity::Error,
                message: "§14a EnWG Modul 2 (prozentuale Arbeitspreisreduzierung) and \
                          Modul 3 (zeitvariable Netzentgelte) are both configured — \
                          BK6-22-300 makes them mutually exclusive; both would reduce \
                          the same network usage twice"
                    .to_owned(),
            });
        }

        // The Modul 3 bands *replace* the flat NNE Arbeitspreis. Both at once
        // bill the device's network usage twice.
        if self.product.sect14a_modul3_nne_ht_ct_per_kwh.is_some()
            && self.grid.nne_arbeitspreis_ct_per_kwh.is_some()
        {
            w.push(BillingWarning {
                code: "MODUL3_AND_FLAT_NNE",
                severity: WarningSeverity::Error,
                message: "§14a Modul 3 band rates are set alongside a flat NNE \
                          Arbeitspreis — the bands replace it; billing both charges \
                          the network usage twice"
                    .to_owned(),
            });
        }

        // BK6-22-300: "Das Modul 3 kann nur in Kombination mit Modul 1
        // ausgewählt werden." Modul 3 alone is not an offer the NB makes, so a
        // product carrying only the bands prices a tariff that does not exist —
        // and the customer loses the Modul 1 reduction they are entitled to.
        if self.product.sect14a_modul3_nne_ht_ct_per_kwh.is_some()
            && self.product.sect14a_modul1_pauschale_eur_per_year.is_none()
        {
            w.push(BillingWarning {
                code: "MODUL3_OHNE_MODUL1",
                severity: WarningSeverity::Error,
                message: "§14a EnWG Modul 3 (zeitvariable Netzentgelte) is configured \
                          without Modul 1 — BK6-22-300 offers Modul 3 only in combination \
                          with Modul 1, so this prices a tariff the Netzbetreiber does \
                          not offer and drops the Modul 1 reduction the customer is due"
                    .to_owned(),
            });
        }

        // Modul 3 bills per time band, which needs a meter that resolves them:
        // BK6-22-300 makes an intelligentes Messsystem a precondition. The same
        // guard §41a carries, for the same reason — a band-priced invoice off an
        // SLP meter is priced against a profile, not against measurement.
        if self.product.sect14a_modul3_nne_ht_ct_per_kwh.is_some()
            && quantities
                .electricity
                .as_ref()
                .is_some_and(|m| m.metering_mode != crate::quantities::MeteringMode::Imsys)
        {
            w.push(BillingWarning {
                code: "MODUL3_IMSYS_REQUIRED",
                severity: WarningSeverity::Error,
                message: "§14a EnWG Modul 3 requires an intelligentes Messsystem \
                          (BK6-22-300) — the metering point reports SLP or RLM. The \
                          time bands cannot be measured, so the reduction cannot be \
                          billed against them."
                    .to_owned(),
            });
        }

        // One Steuerungsentschädigung, one rate basis. The per-kW-year and the
        // per-kWh variants describe the same compensation for the same dimming
        // hours; configured together they both fire and pay it twice.
        if self
            .product
            .sect14a_steuerungsentschaedigung_eur_per_kw_year
            .is_some()
            && self
                .product
                .sect14a_steuerungsentschaedigung_ct_per_kwh
                .is_some()
        {
            w.push(BillingWarning {
                code: "STEUERUNGSENTSCHAEDIGUNG_DOPPELT",
                severity: WarningSeverity::Error,
                message: "§14a EnWG Steuerungsentschädigung is configured both per \
                          kW/Jahr and per kWh — the two are alternative rate bases \
                          for the same compensation; billing both pays it twice"
                    .to_owned(),
            });
        }

        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        // ── Pass 1: standard electricity billing ─────────────────────────────
        let ep = ElectricityProvider::new(self.product.base.clone(), self.grid.clone());
        let mut positions = ep.bill(ctx, quantities, prior)?;

        // ── Pass 2: §14a credit positions ────────────────────────────────────
        let meter = quantities.electricity.as_ref().cloned().unwrap_or_default();
        let kwh = meter.arbeitsmenge_kwh;
        let p = &self.product;

        // ── §14a Modul 3 — zeitvariables Netzentgelt (BK6-22-300) ─────────────
        // Three Tarifstufen replace the flat NNE Arbeitspreis for the device.
        // A zero band still produces a position: a rate band silently omitted
        // from the invoice is indistinguishable from one that was never priced.
        if let (Some(ht), Some(st), Some(nt)) = (
            p.sect14a_modul3_nne_ht_ct_per_kwh,
            p.sect14a_modul3_nne_st_ct_per_kwh,
            p.sect14a_modul3_nne_nt_ct_per_kwh,
        ) {
            let verbrauch = quantities.sect14a_modul3.unwrap_or_default();
            for (label, band_kwh, rate_ct) in [
                ("Netzentgelt §14a Modul 3 HT", verbrauch.ht_kwh, ht),
                ("Netzentgelt §14a Modul 3 ST", verbrauch.st_kwh, st),
                ("Netzentgelt §14a Modul 3 NT", verbrauch.nt_kwh, nt),
            ] {
                let mut pos = BillingPosition::debit(
                    label,
                    band_kwh,
                    "kWh",
                    rate_ct / dec!(100),
                    PositionCategory::GridCharge,
                );
                pos.trace = crate::position::PositionTrace::commodity(
                    band_kwh,
                    "kWh",
                    rate_ct / dec!(100),
                    "§14a EnWG, BK6-22-300 Anlage 2 §2",
                );
                positions.push(
                    pos.with_legal_basis("§14a EnWG")
                        .with_tag("§14a")
                        .with_tag("modul3")
                        .with_tag("nne"),
                );
            }
        }

        // Modul 2 — prozentuale Arbeitspreisreduzierung, as a per-kWh credit
        if let Some(sect14a_m1_ct) = p.sect14a_modul2_nne_reduktion_ct_per_kwh
            && sect14a_m1_ct > Decimal::ZERO
            && kwh > Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "§14a EnWG Modul 2 — Arbeitspreisreduzierung",
                    kwh,
                    "kWh",
                    sect14a_m1_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§14a EnWG")
                .with_tag("§14a")
                .with_tag("sect14a_modul2"),
            );
        }

        // Modul 1 — a flat annual amount, prorated by the period. BK6-22-300
        // sets it as `80 EUR + 3 750 kWh × Arbeitspreis × 0,2`, so it carries no
        // per-kW component and needs no Spitzenleistung: that is what makes it
        // the module a household heat pump on an SLP meter can have at all.
        if let Some(m1_year) = p.sect14a_modul1_pauschale_eur_per_year
            && m1_year > Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "§14a EnWG Modul 1 — pauschale Reduzierung",
                    Decimal::ONE,
                    "Jahr",
                    m1_year * ctx.billed_years(),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§14a EnWG")
                .with_tag("§14a")
                .with_tag("sect14a_modul1"),
            );
        }

        // Steuerungsentschädigung — annual capacity rate × hours actually dimmed
        if let (Some(m3_year), Some(kw), Some(steuerung_h)) = (
            p.sect14a_steuerungsentschaedigung_eur_per_kw_year,
            meter.spitzenleistung_kw,
            meter.steuerung_stunden,
        ) && m3_year > Decimal::ZERO
            && kw > Decimal::ZERO
            && steuerung_h > Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "§14a EnWG Steuerungsentschädigung",
                    kw,
                    "kW",
                    m3_year * (steuerung_h / dec!(8760)),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§14a EnWG")
                .with_tag("§14a")
                .with_tag("sect14a_steuerungsentschaedigung"),
            );
        }

        // Steuerungsentschädigung — per kWh of dimmed energy
        if let (Some(modul3_ct), Some(steuerung_h)) = (
            p.sect14a_steuerungsentschaedigung_ct_per_kwh,
            meter.steuerung_stunden,
        ) {
            let kw = meter.spitzenleistung_kw.unwrap_or(Decimal::ZERO);
            if modul3_ct > Decimal::ZERO && steuerung_h > Decimal::ZERO && kw > Decimal::ZERO {
                let steuerung_kwh = kw * steuerung_h;
                positions.push(
                    BillingPosition::credit(
                        "§14a EnWG Steuerungsentschädigung",
                        steuerung_kwh,
                        "kWh",
                        modul3_ct / dec!(100),
                        PositionCategory::Credit,
                    )
                    .with_legal_basis("§14a EnWG")
                    .with_tag("§14a")
                    .with_tag("sect14a_steuerungsentschaedigung"),
                );
            }
        }

        Ok(positions)
    }
}

// ── GasProvider ───────────────────────────────────────────────────────────────

/// GAS billing provider.
///
/// Includes Brennwertkorrektur info, commodity positions, gas NNE,
/// Energiesteuer and BEHG CO₂ levy. Does NOT include MwSt.
pub struct GasProvider {
    product: GasProduct,
    grid: GridInput,
}

impl GasProvider {
    pub fn new(product: GasProduct, grid: GridInput) -> Self {
        Self { product, grid }
    }
    pub fn from_product(product: &crate::tariff::Product, grid: GridInput) -> Self {
        match product {
            crate::tariff::Product::Gas(p) => Self::new(p.clone(), grid),
            other => panic!(
                "GasProvider::from_product: got '{}', expected Gas",
                other.category_str()
            ),
        }
    }
}

impl BillingProvider for GasProvider {
    fn validate_warnings(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        let mut w = Vec::new();

        // Same invariant as electricity: a gas product must be able to price its
        // gas. A `GasProduct` with every work-price field `None` bills the
        // Energiesteuer and the BEHG levy and nothing for the gas itself.
        let has_gas_work_price = self.product.gas_arbeitspreis_ct_per_kwh_hs.is_some()
            || self
                .product
                .gas_indexed_price
                .as_ref()
                .is_some_and(|i| i.effective_ct_per_kwh().is_some())
            || self
                .product
                .seasonal_prices
                .as_ref()
                .is_some_and(|s| !s.is_empty());
        if !has_gas_work_price {
            w.push(BillingWarning {
                code: "KEIN_ARBEITSPREIS",
                severity: WarningSeverity::Error,
                message: "the gas product carries no Arbeitspreis in any form (kWh_Hs, \
                          indexed or seasonal) — the invoice would charge the Energiesteuer \
                          and the BEHG levy and nothing for the gas. Check the productd \
                          product's price positions."
                    .to_owned(),
            });
        }
        w.extend(indexwert_warning(
            self.product.gas_indexed_price.as_ref(),
            has_gas_work_price,
        ));

        // § 40a Abs. 2 EnWG: an estimated reading is billable but
        // the caller must know it happened — dispatch systems treat it
        // differently and the customer can demand a corrected invoice.
        if quantities.gas.as_ref().is_some_and(|m| m.is_estimated) {
            w.push(BillingWarning {
                code: "ESTIMATED_READING",
                severity: WarningSeverity::Warning,
                message: "billed on an estimated gas reading (§ 40a Abs. 2 EnWG) — \
                          expect a correction when the real reading arrives"
                    .to_owned(),
            });
        }
        // §25 Nr. 4 MessEV / DVGW G 685: the Zustandszahl converts Betriebs- to
        // Normkubikmeter and is never 1 in practice (typically ≈ 0.95). Billing
        // a volume reading without it overstates kWh_Hs by 3–5 %.
        if quantities
            .gas
            .as_ref()
            .is_some_and(|m| m.kwh_hs.is_none() && m.zustandszahl.is_none())
        {
            w.push(BillingWarning {
                code: "ZUSTANDSZAHL_FEHLT",
                severity: WarningSeverity::Warning,
                message: "keine Zustandszahl übergeben — die Mengenumwertung rechnet mit \
                          z = 1,0 (§25 Nr. 4 MessEV, DVGW G 685); reale Werte liegen bei \
                          etwa 0,95, die Abrechnung überschätzt kWh_Hs entsprechend"
                    .to_owned(),
            });
        }
        // Gas carried 7 % USt from 01.10.2022 to 31.03.2024 (§28 Abs. 5
        // UStG) and 16 % in H2/2020. A period straddling a window boundary
        // has no single correct rate — split at the Stichtag and merge.
        if crate::rates::mwst_rate_for_gas_waerme_period(ctx.period_from(), ctx.period_to())
            .is_none()
        {
            w.push(BillingWarning {
                code: "MWST_STICHTAG_IM_ZEITRAUM",
                severity: WarningSeverity::Warning,
                message: "Abrechnungszeitraum überschreitet eine USt-Satzgrenze für Gas \
                          (§28 Abs. 5 UStG) — am Stichtag splitten und Teilrechnungen \
                          zusammenführen"
                    .to_owned(),
            });
        }
        // The BEHG CO₂ price (§10 BEHG) steps at each calendar-year boundary. A
        // period spanning a year-end where the rate changes has no single correct
        // levy — split at 31.12./01.01. and bill each portion at its year's rate.
        if ctx.period_from().year() != ctx.period_to().year()
            && crate::rates::behg_ct_per_kwh_for_year(ctx.period_from().year())
                != crate::rates::behg_ct_per_kwh_for_year(ctx.period_to().year())
        {
            w.push(BillingWarning {
                code: "BEHG_JAHRESGRENZE_IM_ZEITRAUM",
                severity: WarningSeverity::Warning,
                message: "Abrechnungszeitraum überschreitet eine BEHG-Jahresgrenze \
                          (§10 BEHG, CO₂-Preis steigt zum Jahreswechsel) — am 31.12. \
                          splitten und je Teilzeitraum den Jahressatz anwenden"
                    .to_owned(),
            });
        }
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.gas.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;

        // ── Seasonal gas price lookup ──────────────────────────────────────────
        let billing_month = ctx.period_from().month() as u8;
        let seasonal_gas_ap = product.seasonal_prices.as_ref().and_then(|seasons| {
            seasons
                .iter()
                .find(|s| s.contains_month(billing_month))
                .and_then(|s| s.gas_arbeitspreis_ct_per_kwh_hs)
        });

        // Compute kWh_Hs
        let kwh_hs = if let Some(kwh) = meter.kwh_hs {
            kwh
        } else {
            let hs = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
            let z = meter.zustandszahl.unwrap_or(dec!(1.0));
            (meter.messung_qm3 * hs * z).round_kfm(3)
        };

        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── Brennwertkorrektur (info position) ────────────────────────────────
        if meter.kwh_hs.is_none() && meter.brennwert_kwh_per_qm3.is_some() {
            let hs = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
            let z = meter.zustandszahl.unwrap_or(dec!(1.0));
            positions.push(BillingPosition {
                description: format!(
                    "Brennwertkorrektur: {:.4} kWh/m³ × {:.4} = {:.3} kWh_Hs",
                    hs, z, kwh_hs
                ),
                legal_basis: Some("§25 Nr. 4 MessEV / DVGW G 685".to_owned()),
                quantity: meter.messung_qm3,
                unit: "m³".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["brennwertkorrektur".to_owned(), "info".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // ── Gas quality annotation (always added when set) ────────────────────
        // Carried as a tagged info position; to_rechnung_json() injects it as ZusatzAttribut.
        // Per DVGW G 260: the measured Brennwert already reflects the H2 blend —
        // this is a regulatory audit annotation, not a billing correction.
        if let Some(ref gq) = meter.gasqualitaet {
            positions.push(BillingPosition {
                description: format!("Gasqualität: {gq} (§ DVGW G 260)"),
                // Use legal_basis to carry the gasqualitaet value for to_rechnung_json()
                legal_basis: Some(gq.clone()),
                quantity: Decimal::ZERO,
                unit: "".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["gasqualitaet".to_owned(), "info".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // ── Grundpreis ─────────────────────────────────────────────────────────
        if let Some(gp_ct_day) = product.gas_grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis Gas",
                    gp_ct_day / dec!(100),
                    ctx.prorate_days().0 as i64,
                    "§41 EnWG",
                    &["gas"],
                )
                .with_tag("gas"),
            );
        }

        // ── Gas NNE Grundpreis ─────────────────────────────────────────────────
        // A standing charge accrues per day of supply, not per kWh drawn: the
        // supplier owes the Netzbetreiber the GasNEV Grundpreis for a MaLo that
        // consumed nothing, exactly as it owes the commodity Grundpreis above.
        // Both therefore sit outside the consumption guard, as the electricity
        // path's NNE Grundpreis does.
        if let Some(nne_gp) = grid.gas_nne_grundpreis_eur_per_year {
            // Leap-aware: an EUR/year rate divides by that year's actual days
            // (366 in 2024/2028), or the daily rate overstates the Grundpreis.
            let daily = nne_gp / Decimal::from(time::util::days_in_year(ctx.period_from().year()));
            // Active contract days — see the Strom NNE Grundpreis.
            positions.push(
                BillingPosition::debit(
                    "Gasnetznutzungsentgelt Grundpreis",
                    Decimal::from(ctx.prorate_days().0),
                    "Tage",
                    daily,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("GasNEV")
                .with_tag("gas_nne_grundpreis")
                .with_tag("nne"),
            );
        }

        // ── Arbeitspreis ───────────────────────────────────────────────────────
        if kwh_hs > Decimal::ZERO {
            // Resolve effective gas price: gas_indexed_price > seasonal > direct.
            let active_indexed = product.gas_indexed_price.as_ref();
            let gas_ap_ct = if let Some(idx) = active_indexed {
                // Gas indexed price (TTF/NCG-linked, §41 EnWG Sonderkundenvertrag)
                idx.effective_ct_per_kwh()
                    .or(seasonal_gas_ap)
                    .or(product.gas_arbeitspreis_ct_per_kwh_hs)
            } else {
                seasonal_gas_ap.or(product.gas_arbeitspreis_ct_per_kwh_hs)
            };
            if let Some(ap_ct) = gas_ap_ct {
                let (label, legal_basis) = if active_indexed.is_some() {
                    (
                        active_indexed
                            .and_then(|idx| {
                                if idx.index_value.is_some() {
                                    Some(idx.position_description())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "Arbeitspreis Gas".to_owned()),
                        "§41 EnWG",
                    )
                } else if seasonal_gas_ap.is_some() {
                    let season_label = product
                        .seasonal_prices
                        .as_ref()
                        .and_then(|s| s.iter().find(|p| p.contains_month(billing_month)))
                        .and_then(|s| s.label.as_deref())
                        .map(|l| format!("Arbeitspreis Gas ({l})"))
                        .unwrap_or_else(|| "Arbeitspreis Gas (Saisontarif)".to_owned());
                    (season_label, "§41 EnWG")
                } else {
                    ("Arbeitspreis Gas".to_owned(), "§41 EnWG")
                };
                positions.push(
                    arbeitspreis_position(label, kwh_hs, ap_ct, "kWh_Hs", legal_basis, &["gas"])
                        .with_tag("gas")
                        .with_tag(if active_indexed.is_some() {
                            "indexed_price"
                        } else if seasonal_gas_ap.is_some() {
                            "seasonal"
                        } else {
                            "gas"
                        }),
                );
            }

            // ── RLM Leistungspreis Gas (demand charge for large gas customers) ────
            // Applicable to RLM gas metering points with a capacity-based supply contract.
            // Triggered by gas_leistungspreis_ct_per_kw_month + GasMeterInput::spitzenleistung_kw.
            // The rate is per kW *and month*, so it scales with the billed
            // period's month fraction — the same treatment as the Strom and the
            // Fernwärme Leistungspreis.
            if let (Some(lp_ct_per_kw_month), Some(kw)) = (
                product.gas_leistungspreis_ct_per_kw_month,
                meter.spitzenleistung_kw.filter(|kw| *kw > Decimal::ZERO),
            ) {
                let months_frac = ctx.billed_months();
                positions.push(
                    BillingPosition::debit(
                        "Leistungspreis Gas",
                        kw,
                        "kW",
                        lp_ct_per_kw_month / dec!(100) * months_frac,
                        PositionCategory::Commodity,
                    )
                    .with_legal_basis("§41 EnWG")
                    .with_tag("gas_leistungspreis")
                    .with_tag("gas")
                    .with_tag("rlm"),
                );
            }

            // ── Gas NNE Arbeitspreis, Konzessionsabgabe, Bilanzierungsumlage ──
            // All three are per-kWh pass-throughs, so they belong under the
            // consumption guard; the GasNEV Grundpreis above does not.
            if let Some(nne_ap_ct) = grid.gas_nne_arbeitspreis_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Gasnetznutzungsentgelt Arbeitspreis",
                        kwh_hs,
                        "kWh_Hs",
                        nne_ap_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("GasNEV")
                    .with_tag("gas_nne_arbeitspreis")
                    .with_tag("nne"),
                );
            }
            if let Some(ka_ct) = grid.gas_ka_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Konzessionsabgabe Gas",
                        kwh_hs,
                        "kWh_Hs",
                        ka_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("KAV §2")
                    .with_tag("gas_konzessionsabgabe")
                    .with_tag("nne"),
                );
            }
            if let Some(bilu_ct) = grid.gas_bilanzierungsumlage_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Bilanzierungsumlage Gas",
                        kwh_hs,
                        "kWh_Hs",
                        bilu_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("GaBi Gas 2.1 (BK7-24-01-008)")
                    .with_tag("gas_bilanzierungsumlage")
                    .with_tag("nne"),
                );
            }

            // ── Energiesteuer ──────────────────────────────────────────────────
            positions.extend(energiesteuer_positions(
                product.energiesteuer_tarif,
                kwh_hs,
                rates.effective_energiesteuer_gas(product.energiesteuer_gas_ct_per_kwh_override),
            ));

            // ── BEHG CO₂ ───────────────────────────────────────────────────────
            let behg_rate = rates.effective_behg_gas(product.behg_gas_ct_per_kwh_override);
            if behg_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "CO₂-Abgabe BEHG",
                        kwh_hs,
                        "kWh_Hs",
                        behg_rate,
                        "BEHG",
                        "behg",
                    )
                    .with_tag("gas"),
                );
                // The levy line is § 3 Abs. 1 Nr. 2. The statute asks for five
                // more figures beside it; Nr. 6 is the Vermieter's building
                // fact and belongs to their Abrechnung, not to this supply.
                if let Some(faktor) =
                    crate::rates::erdgas_emissionsfaktor_kg_per_kwh(ctx.period_from().year())
                {
                    positions.extend(crate::position::co2kostaufg_disclosures(
                        kwh_hs, "kWh_Hs", faktor, "gas",
                    ));
                }
            }
        }

        // ── AufAbschlag / Rabatt (Gas) ─────────────────────────────────────────
        if let Some(aa_ct) = product
            .auf_abschlag_ct_per_kwh
            .filter(|v| *v != Decimal::ZERO)
        {
            let kwh_total = meter.kwh_hs.unwrap_or_else(|| {
                // Same default as the main kWh_Hs conversion — a diverging
                // fallback made the AufAbschlag quantity base inconsistent.
                let bw = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
                let zz = meter.zustandszahl.unwrap_or(dec!(1.0));
                meter.messung_qm3 * bw * zz
            });
            if kwh_total > Decimal::ZERO {
                let (label, cat) = if aa_ct < Decimal::ZERO {
                    ("Rabatt Gas (Arbeitspreis)", PositionCategory::Discount)
                } else {
                    ("Aufschlag Gas (Arbeitspreis)", PositionCategory::Levy)
                };
                positions.push(
                    BillingPosition::debit(label, kwh_total, "kWh", aa_ct / dec!(100), cat)
                        .with_tag("auf_abschlag")
                        .with_tag("gas"),
                );
            }
        }
        if let Some(aa_month) = product
            .auf_abschlag_eur_per_month
            .filter(|v| *v != Decimal::ZERO)
        {
            let months_frac = ctx.billed_months();
            let (label, cat) = if aa_month < Decimal::ZERO {
                ("Rabatt Gas (Festbetrag)", PositionCategory::Discount)
            } else {
                ("Aufschlag Gas (Festbetrag)", PositionCategory::Levy)
            };
            positions.push(BillingPosition {
                description: label.to_owned(),
                legal_basis: None,
                quantity: months_frac,
                unit: "Monat".to_owned(),
                unit_price_eur: aa_month,
                net_eur: crate::position::validated_eur(aa_month * months_frac),
                category: cat,
                tags: vec!["auf_abschlag".to_owned(), "gas".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // ── Zählerstand info position (§40 Abs. 2 Nr. 6 EnWG) ─────────────────
        // Meter identity + start/end readings in m³ — same display duty as
        // the electricity provider fulfils for kWh registers.
        if meter.zaehlerstand_von.is_some() || meter.zaehlerstand_bis.is_some() {
            // § 40 Abs. 2 Nr. 6 EnWG — the readings *and* how they were obtained.
            let label = format!(
                "Zählerstand: {} – {} m³{}",
                meter
                    .zaehlerstand_von
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                meter
                    .zaehlerstand_bis
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                meter
                    .ablesungsart
                    .label()
                    .map(|l| format!(" ({l})"))
                    .unwrap_or_default(),
            );
            let zid = meter
                .zaehlernummer
                .as_deref()
                .or(ctx.zaehler_id.as_deref())
                .unwrap_or("-");
            positions.push(BillingPosition {
                description: label,
                legal_basis: Some("§40 Abs. 2 Nr. 6 EnWG".to_owned()),
                quantity: Decimal::ZERO,
                unit: "m³".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["zaehlerstand".to_owned(), zid.to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // ── § 40a Abs. 2 EnWG — estimated reading notice ─────────────────────
        // The estimation basis must carry an explicit, prominently marked hint.
        if meter.is_estimated {
            positions.push(BillingPosition {
                description: "Abrechnungswert: Schätzung gemäß § 40a Abs. 2 EnWG — \
                              auf Wunsch Korrektur nach realer Ablesung"
                    .to_owned(),
                legal_basis: Some("§40a EnWG".to_owned()),
                quantity: Decimal::ZERO,
                unit: String::new(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["schatzwert".to_owned(), "ersatzwert".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // A Steuerentlastung leaves the levy where it is and tells the customer
        // what to file — see `crate::steuer`.
        let hinweise = entlastungs_hinweise(&product.steuerentlastungen, &positions);
        positions.extend(hinweise);

        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── HeatProvider ──────────────────────────────────────────────────────────────

/// WAERME (Fernwärme) billing provider.
pub struct HeatProvider {
    product: HeatProduct,
}

impl HeatProvider {
    pub fn new(product: HeatProduct) -> Self {
        Self { product }
    }
    pub fn from_product(product: &crate::tariff::Product) -> Self {
        match product {
            crate::tariff::Product::Waerme(p) => Self::new(p.clone()),
            other => panic!(
                "HeatProvider::from_product: got '{}', expected Waerme",
                other.category_str()
            ),
        }
    }
}

impl BillingProvider for HeatProvider {
    fn validate_warnings(
        &self,
        ctx: &BillingContext,
        _quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        let mut w = Vec::new();

        // Same invariant as electricity and gas: heat must be priced. A
        // Fernwärme product with no Arbeitspreis bills the Grundpreis and the
        // Leistungspreis and nothing for the delivered heat.
        let has_heat_work_price = self.product.waerme_arbeitspreis_ct_per_kwh.is_some()
            || self
                .product
                .waerme_indexed_price
                .as_ref()
                .is_some_and(|i| i.effective_ct_per_kwh().is_some());
        if !has_heat_work_price {
            w.push(BillingWarning {
                code: "KEIN_ARBEITSPREIS",
                severity: WarningSeverity::Error,
                message: "the Fernwärme product carries no Arbeitspreis — the invoice would \
                          bill the standing and demand charges and nothing for the delivered \
                          heat. Check the productd product's price positions."
                    .to_owned(),
            });
        }
        // AVBFernwärmeV § 24 Abs. 4: the Preisgleitklausel *is* the agreed price.
        // Falling back to the static Arbeitspreis when the index is missing bills
        // a figure the contract does not contain, so it is flagged rather than
        // substituted in silence.
        w.extend(indexwert_warning(
            self.product.waerme_indexed_price.as_ref(),
            self.product.waerme_arbeitspreis_ct_per_kwh.is_some(),
        ));

        // Fernwärme carried 7 % USt from 01.10.2022 to 31.03.2024 (§28
        // Abs. 6 UStG) and 16 % in H2/2020 — same split discipline as gas.
        if crate::rates::mwst_rate_for_gas_waerme_period(ctx.period_from(), ctx.period_to())
            .is_none()
        {
            w.push(BillingWarning {
                code: "MWST_STICHTAG_IM_ZEITRAUM",
                severity: WarningSeverity::Warning,
                message: "Abrechnungszeitraum überschreitet eine USt-Satzgrenze für \
                          Fernwärme (§28 Abs. 6 UStG) — am Stichtag splitten und \
                          Teilrechnungen zusammenführen"
                    .to_owned(),
            });
        }
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.heat.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let mut positions: Vec<BillingPosition> = Vec::new();
        // The billed period decides the month count. `unwrap_or(1)` charged a
        // whole year of Fernwärme one month of Grundpreis whenever the caller
        // did not state `months` — silently, because a plausible amount came
        // out. An explicit `months` still wins: an operator billing on
        // Abrechnungsmonate rather than calendar days states them.
        let months = meter.months.unwrap_or_else(|| ctx.billed_months());

        if let Some(gp) = product.waerme_grundpreis_eur_per_month {
            positions.push(
                BillingPosition::debit(
                    "Grundpreis Fernwärme",
                    months,
                    "Monate",
                    gp,
                    PositionCategory::Commodity,
                )
                .with_tag("commodity")
                .with_tag("waerme"),
            );
        }
        if let (Some(lp), Some(kw)) = (
            product.waerme_leistungspreis_eur_per_kw_year.or_else(|| {
                product
                    .waerme_leistungspreis_eur_per_kw_month
                    .map(|m| m * dec!(12))
            }),
            meter.spitzenleistung_kw,
        ) {
            positions.push(
                BillingPosition::debit(
                    "Leistungspreis Fernwärme",
                    kw,
                    "kW",
                    lp / dec!(12) * months,
                    PositionCategory::Commodity,
                )
                .with_tag("commodity")
                .with_tag("waerme"),
            );
        }
        // AVBFernwärmeV §24 Abs. 4 Preisänderungsklausel: an index-linked
        // Arbeitspreis resolves the effective ct/kWh and overrides the static one.
        let (waerme_ap_ct, ap_basis) = match product
            .waerme_indexed_price
            .as_ref()
            .and_then(|idx| idx.effective_ct_per_kwh())
        {
            Some(idx_ct) => (Some(idx_ct), "AVBFernwärmeV §24 Abs. 4"),
            None => (product.waerme_arbeitspreis_ct_per_kwh, "§41 EnWG"),
        };
        if let Some(ap_ct) = waerme_ap_ct
            && meter.kwh_waerme > Decimal::ZERO
        {
            positions.push(
                arbeitspreis_position(
                    "Arbeitspreis Fernwärme",
                    meter.kwh_waerme,
                    ap_ct,
                    "kWh_th",
                    ap_basis,
                    &["waerme"],
                )
                .with_tag("waerme"),
            );
        }
        // ── CO₂-Kosten (BEHG / CO2KostAufG § 3) ────────────────────────────────
        // A Wärmelieferung carries the CO₂ cost of the fuel burned to produce
        // it, and **CO2KostAufG § 3** obliges the supplier to state the cost it
        // actually bore. The rate is the heat product's own — the generator's
        // fuel mix and conversion losses sit between the gas BEHG rate and the
        // delivered kWh_th, so reusing the gas rate would be wrong in both
        // directions.
        if let Some(co2_ct) = product.waerme_co2_kosten_ct_per_kwh
            && meter.kwh_waerme > Decimal::ZERO
            && co2_ct > Decimal::ZERO
        {
            positions.push(
                levy_position(
                    "CO₂-Kosten (BEHG)",
                    meter.kwh_waerme,
                    "kWh_th",
                    co2_ct,
                    "CO2KostAufG § 3",
                    "behg",
                )
                .with_tag("waerme"),
            );
        }
        // § 3 Abs. 1 CO2KostAufG — the five figures that accompany the cost.
        //
        // The product states the Emissionsfaktor in g/kWh, which is how heat
        // networks publish it; Nr. 3 asks for kg CO₂/kWh, so it is converted
        // for the statement rather than restated in the wrong unit.
        //
        // Emitted even where the cost is zero: a fully renewable network still
        // owes its customers the statement.
        if let Some(g_per_kwh) = product.waerme_co2_emission_g_per_kwh {
            positions.extend(crate::position::co2kostaufg_disclosures(
                meter.kwh_waerme,
                "kWh_th",
                g_per_kwh / dec!(1000),
                "waerme",
            ));
        }
        // § 14 WPG — the renewable share of the delivered heat.
        if let Some(pct) = product.waerme_erneuerbar_anteil_pct {
            positions.push(BillingPosition {
                description: format!(
                    "Anteil erneuerbarer Energien an der Wärmelieferung: {pct}\u{202f}%"
                ),
                legal_basis: Some("§ 14 WPG".to_owned()),
                quantity: pct,
                unit: "%".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["erneuerbar_anteil".to_owned(), "waerme".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // District heating is standard-rated. There is NO permanent reduced rate
        // (§12 Abs. 2 Nr. 1 UStG covers Anlage-2 goods, not heat); the 7 % on
        // gas/Fernwärme was the temporary §28 Abs. 5/6 UStG window and is expressed
        // via `mwst_rate_override`. When an override is set, stamp it on the heat
        // positions so a bundled multi-commodity invoice yields a separate tax
        // bucket; otherwise leave them for the engine's period-aware default rate.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── WaterProvider ─────────────────────────────────────────────────────────────

/// WASSER billing provider — Trinkwasser + gesplittete Abwassergebühr.
///
/// Positions and their USt treatment:
///
/// | Position | Base | USt |
/// |---|---|---|
/// | Grundpreis Trinkwasser | months × EUR/month | 7 % (§12 Abs. 2 Nr. 1 UStG, Anlage 2 Nr. 34) |
/// | Mengenpreis Trinkwasser | frischwasser m³ × EUR/m³ | 7 % |
/// | Schmutzwassergebühr | (frischwasser − Absetzungen) m³ × EUR/m³ | none (public-law fee) or 19 % (private charge) |
/// | Niederschlagswassergebühr | versiegelte Fläche m² × EUR/m²/a, pro-rated | same as Schmutzwasser |
///
/// Absetzungen (Gartenwasser, Schleppwasser, Verdunstung, …) reduce only the
/// Schmutzwasser volume — the drinking water that fed them was delivered and
/// stays billed. Each Absetzung is shown as a 0-EUR Info position so the
/// deduction is auditable on the invoice.
pub struct WaterProvider {
    product: WaterProduct,
}

impl WaterProvider {
    pub fn new(product: WaterProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for WaterProvider {
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        let mut w = Vec::new();
        let meter = quantities.wasser.clone().unwrap_or_default();
        let p = &self.product;

        // The `KEIN_ARBEITSPREIS` invariant, on the water side. A tariff that
        // prices only the Abwasser side still reaches `bill()`: the
        // Schmutzwassergebühr is charged on the Frischwassermaßstab, so the
        // invoice comes out with a full Gebühr and not one cent for the
        // drinking water that was actually delivered — and it looks complete,
        // because a plausible amount is on the page.
        if meter.frischwasser_m3 > Decimal::ZERO
            && p.wasser_mengenpreis_eur_per_m3.is_none()
            && p.wasser_grundpreis_eur_per_month.is_none()
        {
            w.push(BillingWarning {
                code: "KEIN_TRINKWASSERPREIS",
                severity: WarningSeverity::Error,
                message: format!(
                    "der Wassertarif nennt weder Grund- noch Mengenpreis für Trinkwasser, \
                     es wurden aber {} m³ geliefert — die Rechnung enthielte allein die \
                     Abwassergebühr und nichts für das Wasser. Preispositionen des \
                     productd-Produkts prüfen.",
                    meter.frischwasser_m3
                ),
            });
        }

        if !meter.absetzungen.is_empty() && p.schmutzwasser_eur_per_m3.is_none() {
            w.push(BillingWarning {
                code: "ABSETZUNG_OHNE_SCHMUTZWASSERPREIS",
                severity: WarningSeverity::Warning,
                message: "Absetzungen übermittelt, aber kein Schmutzwasserpreis im Tarif — \
                          die Absetzung hat keine Wirkung"
                    .to_owned(),
            });
        }
        if p.niederschlagswasser_eur_per_m2_year.is_some() && meter.versiegelte_flaeche_m2.is_none()
        {
            w.push(BillingWarning {
                code: "NIEDERSCHLAGSWASSER_OHNE_FLAECHE",
                severity: WarningSeverity::Warning,
                message: "Niederschlagswasserpreis im Tarif, aber keine versiegelte Fläche \
                          übermittelt — gesplittete Abwassergebühr unvollständig"
                    .to_owned(),
            });
        }
        // EN 16931 BR-O-11 … BR-O-14: a document carrying a "not subject to
        // VAT" line may carry nothing else. An öffentlich-rechtliche
        // Abwassergebühr is exactly that, and over 90 % of municipalities levy
        // one — so a combined Trinkwasser-plus-Abwasser invoice is not a valid
        // e-invoice, and the platform issued them silently. Municipalities do
        // not in fact combine them: the Gebühr goes out as a Bescheid.
        let has_public_law_fee = p.abwasser_regime == AbwasserRegime::PublicLawFee
            && (p.schmutzwasser_eur_per_m3.is_some()
                || p.niederschlagswasser_eur_per_m2_year.is_some());
        let has_taxable_supply = p.wasser_grundpreis_eur_per_month.is_some()
            || p.wasser_mengenpreis_eur_per_m3.is_some();
        if has_public_law_fee && has_taxable_supply {
            w.push(BillingWarning {
                code: "GEBUEHR_UND_ENTGELT_AUF_EINEM_BELEG",
                // A warning, not a refusal: a combined paper
                // Jahresverbrauchsabrechnung is lawful and common. What is not
                // possible is rendering it as an e-invoice, and that is refused
                // where it actually bites — `Invoice::to_en16931`.
                severity: WarningSeverity::Warning,
                message: "die öffentlich-rechtliche Abwassergebühr ist nicht steuerbar \
                          (EN 16931 Kategorie O) und darf nach BR-O-11 ff. nicht mit \
                          umsatzsteuerpflichtigen Trinkwasserpositionen auf einem Beleg \
                          stehen — Gebührenbescheid und Trinkwasserrechnung getrennt \
                          erstellen, oder abwasser_regime auf PRIVATE_LAW_CHARGE setzen, \
                          wenn privatrechtlich abgerechnet wird"
                    .to_owned(),
            });
        }

        if meter.absetzung_total_m3() > meter.frischwasser_m3 {
            w.push(BillingWarning {
                code: "ABSETZUNG_UEBERSTEIGT_FRISCHWASSER",
                severity: WarningSeverity::Error,
                message: format!(
                    "Absetzungen ({} m³) übersteigen den Frischwasserbezug ({} m³) — \
                     Zählerstände prüfen",
                    meter.absetzung_total_m3(),
                    meter.frischwasser_m3
                ),
            });
        }
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.wasser.clone().unwrap_or_default();
        let p = &self.product;
        let mut positions: Vec<BillingPosition> = Vec::new();
        // Same reasoning as Fernwärme: an unstated month count is the billed
        // period, not one month.
        let months = meter.months.unwrap_or_else(|| ctx.billed_months());

        let absetzung_m3 = meter.absetzung_total_m3();
        if absetzung_m3 > meter.frischwasser_m3 {
            return Err(EngineError::ValidationBlocked {
                warnings: self.validate_warnings(ctx, quantities),
            });
        }

        // § 12 Abs. 2 Nr. 1 UStG i. V. m. Anlage 2 Nr. 34 — Wasser is reduced-
        // rated. The reduced rate itself comes from the period-aware
        // `RegulatoryRates`, not a literal, so a statutory change reaches water
        // billing the same way it reaches everything else.
        let trinkwasser_rate = p
            .mwst_rate_override
            .unwrap_or(ctx.regulatory_rates.mwst_rate_reduced);
        // A public-law Gebühr is hoheitlich — outside the scope of the UStG, so
        // EN 16931 category `O`, not `Z`. Zero-rating asserts a taxable supply
        // at 0 %, which a Gebührenbescheid is not, and the two carry different
        // business rules on the receiving side.
        let public_law = p.abwasser_regime == AbwasserRegime::PublicLawFee;
        let abwasser_rate = if public_law {
            Decimal::ZERO
        } else {
            ctx.regulatory_rates.mwst_rate
        };

        if let Some(gp) = p.wasser_grundpreis_eur_per_month {
            let mut pos = BillingPosition::debit(
                "Grundpreis Trinkwasser",
                months,
                "Monate",
                gp,
                PositionCategory::Commodity,
            )
            .with_legal_basis("AVBWasserV")
            .with_tag("wasser");
            pos.applicable_tax_rate = Some(trinkwasser_rate);
            positions.push(pos);
        }

        if let Some(mp) = p.wasser_mengenpreis_eur_per_m3
            && meter.frischwasser_m3 > Decimal::ZERO
        {
            let mut pos = BillingPosition::debit(
                "Mengenpreis Trinkwasser",
                meter.frischwasser_m3,
                "m³",
                mp,
                PositionCategory::Commodity,
            )
            .with_legal_basis("§12 Abs. 2 Nr. 1 UStG i. V. m. Anlage 2 Nr. 34 (7 % USt)")
            .with_tag("wasser");
            pos.applicable_tax_rate = Some(trinkwasser_rate);
            positions.push(pos);
        }

        if let Some(sw) = p.schmutzwasser_eur_per_m3 {
            let schmutzwasser_m3 = meter.frischwasser_m3 - absetzung_m3;
            if schmutzwasser_m3 > Decimal::ZERO {
                let mut pos = BillingPosition::debit(
                    "Schmutzwassergebühr",
                    schmutzwasser_m3,
                    "m³",
                    sw,
                    PositionCategory::Fee,
                )
                .with_legal_basis("Gesplittete Abwassergebühr (KAG-Satzung, Frischwassermaßstab)")
                .with_tag("wasser")
                .with_tag("abwasser");
                pos.applicable_tax_rate = Some(abwasser_rate);
                if public_law {
                    pos = pos.with_out_of_scope();
                }
                pos.trace.formula = format!(
                    "({} m³ Frischwasser − {} m³ Absetzungen) × {} EUR/m³",
                    meter.frischwasser_m3, absetzung_m3, sw
                );
                positions.push(pos);
            }

            // One auditable 0-EUR Info position per Absetzung.
            for a in &meter.absetzungen {
                let mut pos = BillingPosition::debit(
                    format!("Absetzung {} (nicht eingeleitet)", a.grund.label()),
                    a.m3,
                    "m³",
                    Decimal::ZERO,
                    PositionCategory::Info,
                )
                .with_legal_basis("Absetzung nicht eingeleiteter Wassermengen (KAG-Satzung)")
                .with_tag("wasser")
                .with_tag("abwasser");
                pos.applicable_tax_rate = Some(Decimal::ZERO);
                positions.push(pos);
            }
        }

        if let (Some(nsw), Some(flaeche)) = (
            p.niederschlagswasser_eur_per_m2_year,
            meter.versiegelte_flaeche_m2,
        ) && flaeche > Decimal::ZERO
        {
            let mut pos = BillingPosition::debit(
                "Niederschlagswassergebühr",
                flaeche,
                "m²",
                nsw / dec!(12) * months,
                PositionCategory::Fee,
            )
            .with_legal_basis("Gesplittete Abwassergebühr (KAG-Satzung, Flächenmaßstab)")
            .with_tag("wasser")
            .with_tag("abwasser");
            pos.applicable_tax_rate = Some(abwasser_rate);
            if public_law {
                pos = pos.with_out_of_scope();
            }
            pos.trace.formula =
                format!("{flaeche} m² versiegelte Fläche × {nsw} EUR/m²/a × {months}/12 Monate");
            positions.push(pos);
        }

        Ok(positions)
    }
}

// ── SolarProvider ─────────────────────────────────────────────────────────────

/// SOLAR (Eigenverbrauch / Mieterstrom §21 Abs. 3 / §42b EnWG GGV) billing provider.
pub struct SolarProvider {
    product: SolarProduct,
}

impl SolarProvider {
    pub fn new(product: SolarProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for SolarProvider {
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        _quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        let mut w = Vec::new();
        let p = &self.product;

        // A commodity product must be able to price its commodity — the same
        // invariant electricity, gas and heat carry. Without it a solar product
        // billed the Stromsteuer and nothing for the kWh.
        if p.solar_arbeitspreis_ct_per_kwh.is_none() && p.arbeitspreis_ct_per_kwh.is_none() {
            w.push(BillingWarning {
                code: "KEIN_ARBEITSPREIS",
                severity: WarningSeverity::Error,
                message: "the solar product carries neither solar_arbeitspreis_ct_per_kwh nor \
                          arbeitspreis_ct_per_kwh — the invoice would price no electricity at \
                          all. Check the productd product's price positions."
                    .to_owned(),
            });
        }

        // \u{a7} 42a Abs. 4 EnWG caps a Mieterstrompreis at 90\u{202f}% of the local
        // Grundversorgungstarif. It is a statutory ceiling, so exceeding it does
        // not produce a payable invoice — it blocks the run.
        if let (Some(gv_ct), Some(ms_ct)) = (
            p.grundversorgung_arbeitspreis_ct_per_kwh,
            p.solar_arbeitspreis_ct_per_kwh,
        ) {
            let cap = (gv_ct * dec!(0.9)).round_kfm(4);
            if ms_ct > cap {
                w.push(BillingWarning {
                    code: "MIETERSTROM_UEBER_90PCT_GRUNDVERSORGUNG",
                    severity: WarningSeverity::Error,
                    message: format!(
                        "\u{a7} 42a Abs. 4 EnWG: der Mieterstrom-Arbeitspreis {ms_ct} ct/kWh \
                         \u{fc}berschreitet 90\u{202f}% des Grundversorgungstarifs \
                         ({gv_ct} ct/kWh \u{2192} {cap} ct/kWh)"
                    ),
                });
            }
        }
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let product = &self.product;
        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── §42b EnWG (Solarpaket I) GGV hybrid billing ──────────────────
        // When GgvSolarInput is present, billing is split into two portions:
        // 1. PV portion: min(consumption, allocated_pv) at community solar rate
        // 2. Grid portion: max(0, consumption − allocated_pv) at electricity rate
        if let Some(ggv) = &quantities.ggv_solar {
            let pv_kwh = ggv.pv_delivered_kwh();
            let grid_kwh = ggv.grid_kwh();

            // ── PV portion ──────────────────────────────────────────────────────
            if pv_kwh > Decimal::ZERO {
                if let Some(ap_ct) = product.solar_arbeitspreis_ct_per_kwh {
                    positions.push(
                        arbeitspreis_position(
                            format!("Arbeitspreis Solarstrom GGV ({pv_kwh:.3}\u{202f}kWh)"),
                            pv_kwh,
                            ap_ct,
                            "kWh",
                            "\u{a7}42b EnWG",
                            &["solar", "ggv_pv"],
                        )
                        .with_tag("solar")
                        .with_tag("ggv_pv"),
                    );
                }
                // GGV Rabatt applies to the PV portion only
                if let Some(rabatt_ct) = product.gemeinschaft_rabatt_ct_per_kwh {
                    positions.push(
                        BillingPosition::credit(
                            "GGV-Rabatt Solarstrom (\u{a7}42b EnWG)",
                            pv_kwh,
                            "kWh",
                            rabatt_ct / dec!(100),
                            PositionCategory::Discount,
                        )
                        .with_legal_basis("\u{a7}42b EnWG Abs.\u{202f}3")
                        .with_tag("gemeinschaft_rabatt")
                        .with_tag("solar")
                        .with_tag("ggv_pv"),
                    );
                }
                // Stromsteuer on the PV portion, through the same § 9 StromStG
                // resolution the electricity provider uses — so a Befreiung is
                // *stated* on the page with its ground and citation instead of
                // the line merely being absent, which is all a bare
                // `solar_include_stromsteuer = false` produced.
                positions.extend(stromsteuer_positions(
                    product.stromsteuer_tarif,
                    pv_kwh,
                    ctx.regulatory_rates.effective_stromsteuer(None),
                    &["solar", "ggv_pv"],
                ));
            }

            // ── Grid portion ────────────────────────────────────────────────────
            // Billed at the grid remainder rate (arbeitspreis_ct_per_kwh).
            // Falls back to solar_arbeitspreis_ct_per_kwh if not separately configured.
            // Stromsteuer always applies to grid electricity (§3 StromStG).
            if grid_kwh > Decimal::ZERO {
                let grid_rate = product
                    .arbeitspreis_ct_per_kwh
                    .or(product.solar_arbeitspreis_ct_per_kwh);
                if let Some(ap_ct) = grid_rate {
                    positions.push(
                        arbeitspreis_position(
                            format!("Arbeitspreis Reststrom Netz ({grid_kwh:.3}\u{202f}kWh)"),
                            grid_kwh,
                            ap_ct,
                            "kWh",
                            "\u{a7}41 EnWG",
                            &["strom", "ggv_grid"],
                        )
                        .with_tag("strom")
                        .with_tag("ggv_grid"),
                    );
                }
                // Stromsteuer on grid portion
                let st_rate = ctx.regulatory_rates.effective_stromsteuer(None);
                if st_rate > Decimal::ZERO {
                    positions.push(
                        levy_position(
                            "Stromsteuer (Reststrom Netz)",
                            grid_kwh,
                            "kWh",
                            st_rate,
                            "\u{a7}3 StromStG",
                            "stromsteuer",
                        )
                        .with_tag("strom")
                        .with_tag("ggv_grid"),
                    );
                }
            }

            // Info position: PV coverage ratio (useful for \u00a740a Kilowattstundenpreis reporting)
            let ratio_pct = (ggv.pv_coverage_ratio() * dec!(100)).round_kfm(1);
            positions.push(BillingPosition {
                description: format!(
                    "GGV Solarstromanteil: {ratio_pct}\u{202f}% ({pv_kwh:.3}\u{202f}kWh von {:.3}\u{202f}kWh)",
                    ggv.actual_consumption_kwh
                ),
                legal_basis: Some("\u{a7}42b EnWG (Solarpaket I)".to_owned()),
                quantity: ggv.pv_coverage_ratio(),
                unit: "%".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["ggv_coverage".to_owned(), "solar".to_owned()],
                        applicable_tax_rate: None,
                        trace: crate::position::PositionTrace::default(),
            });

            // Wire tax rate for GGV hybrid positions too
            if let Some(rate) = product.mwst_rate_override {
                for pos in &mut positions {
                    if pos.applicable_tax_rate.is_none()
                        && !matches!(
                            pos.category,
                            PositionCategory::Tax
                                | PositionCategory::Abschlag
                                | PositionCategory::Info
                        )
                    {
                        pos.applicable_tax_rate = Some(rate);
                    }
                }
            }
            return Ok(positions);
        }

        // ── Standard solar / Mieterstrom / simple GGV path ────────────────────
        let meter = quantities.solar.as_ref().cloned().unwrap_or_default();
        let kwh = meter.eigenverbrauch_kwh;

        if let Some(ap_ct) = product.solar_arbeitspreis_ct_per_kwh {
            positions.push(
                arbeitspreis_position(
                    "Arbeitspreis Solarstrom (Eigenverbrauch)",
                    kwh,
                    ap_ct,
                    "kWh",
                    "\u{a7}42b EnWG",
                    &["solar"],
                )
                .with_tag("solar"),
            );
        }
        // The Mieterstromzuschlag (\u{a7} 21 Abs. 3 EEG 2023) is deliberately absent
        // here: it is the Anlagenbetreiber's claim against the Netzbetreiber,
        // settled through `eeg-billing`'s `TenantElectricity` scheme. Billing it
        // as a surcharge on the tenant's invoice would charge the tenant for a
        // payment somebody else owes the landlord.
        if let Some(gv_ct) = product.grundversorgung_arbeitspreis_ct_per_kwh {
            positions.push(BillingPosition {
                description: format!(
                    "Mieterstrom-Preisobergrenze (\u{a7} 42a Abs. 4 EnWG): 90\u{202f}% von                      {gv_ct:.4}\u{202f}ct/kWh = {:.4}\u{202f}ct/kWh",
                    gv_ct * dec!(0.9)
                ),
                legal_basis: Some("\u{a7} 42a Abs. 4 EnWG".to_owned()),
                quantity: Decimal::ZERO,
                unit: "ct/kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["mieterstrom".to_owned(), "preisobergrenze".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }
        if let Some(rabatt_ct) = product.gemeinschaft_rabatt_ct_per_kwh {
            positions.push(
                BillingPosition::credit(
                    "Rabatt Gemeinschaftliche Geb\u{e4}udeversorgung (\u{a7}42b EnWG)",
                    kwh,
                    "kWh",
                    rabatt_ct / dec!(100),
                    PositionCategory::Discount,
                )
                .with_legal_basis("\u{a7}42b EnWG")
                .with_tag("gemeinschaft_rabatt")
                .with_tag("solar"),
            );
        }
        // Mieterstrom and Eigenverbrauch are supplies like any other: either the
        // Stromsteuer is owed on them, or a ground exempts them and the invoice
        // says which. This path billed neither — no levy and no notice.
        positions.extend(stromsteuer_positions(
            product.stromsteuer_tarif,
            kwh,
            ctx.regulatory_rates.effective_stromsteuer(None),
            &["solar"],
        ));
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── EegProvider ───────────────────────────────────────────────────────────────

/// EEG feed-in settlement billing provider.
///
/// **Preferred path**: when `quantities.eeg_full` is set, delegates to
/// `eeg_billing::calculate_settlement()` for version-aware §51/§52/§44b rules.
///
/// **Fallback path**: when only `quantities.eeg` is set, uses the simplified
/// EEG credit note formula (Vergütung, Marktprämie, Managementprämie, KWKG).
/// This is suitable for LF-side Gutschrift documents where plant-specific
/// regulatory details (§52 sanctions, §44b biogas quota) are not relevant.
///
/// ## Recommended usage
///
/// - **NB-side settlement** (plant registry, MaStR compliance): use `einsd` + `eeg-billing`
/// - **LF-side credit notes** (monthly Gutschrift to generator): use `EegProvider`
///   with `eeg_full` when plant parameters are available, `eeg` otherwise
pub struct EegProvider {
    product: EegProduct,
}

impl EegProvider {
    pub fn new(product: EegProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for EegProvider {
    // `ctx` is consumed only by the eeg-feature path below.
    #[cfg_attr(not(feature = "eeg"), allow(unused_variables))]
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        // ── Preferred path: delegate to eeg-billing for full regulatory accuracy ──
        // Only available when the `eeg` feature is enabled.
        #[cfg(feature = "eeg")]
        if let Some(eeg_full) = &quantities.eeg_full {
            return bill_eeg_full(eeg_full, ctx);
        }

        // ── Fallback: simplified EEG credit note ──────────────────────────────
        let meter = quantities.eeg.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let kwh = meter.einspeisung_kwh;

        let billable_kwh = meter
            .kwh_during_negative_epex
            .map(|neg| (kwh - neg).max(Decimal::ZERO))
            .unwrap_or(kwh);

        let suspended_kwh = kwh - billable_kwh;
        let mut positions: Vec<BillingPosition> = Vec::new();

        if suspended_kwh > Decimal::ZERO {
            positions.push(BillingPosition {
                description: "Keine Vergütung (§51 EEG Negativpreisregel)".to_owned(),
                legal_basis: Some("§51 EEG 2023".to_owned()),
                quantity: suspended_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["eeg_negativpreis_suspension".to_owned(), "info".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }
        if let Some(vg_ct) = product.eeg_verguetungssatz_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "EEG Einspeisevergütung",
                    billable_kwh,
                    "kWh",
                    vg_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§21 EEG 2023")
                .with_tag("eeg_verguetung")
                .with_tag("eeg"),
            );
        }
        // The Marktprämie is computed from the anzulegende Wert (§ 20 iVm
        // Anlage 1 EEG 2023), and § 51 Abs. 1 EEG 2023 reduces that value to
        // zero for the hours it applies to. So the suspension governs the
        // Marktprämie exactly as it governs the Einspeisevergütung: both are
        // paid on `billable_kwh`. Credited on the raw `kwh`, the Marktprämie
        // pays for the very hours the invoice prints as unremunerated.
        if let Some(mp_ct) = product.eeg_marktpraemie_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "EEG Marktprämie",
                    billable_kwh,
                    "kWh",
                    mp_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 EEG 2023")
                .with_tag("eeg_marktpraemie")
                .with_tag("eeg"),
            );
        }
        // A **contractual** Direktvermarktungsentgelt, not a statutory premium:
        // EEG 2023 knows no standalone Managementprämie — the management cost is
        // part of the anzulegende Wert the Marktprämie above is derived from.
        // Being contractual, it is owed on every delivered kWh and § 51 Abs. 1
        // EEG 2023, which reaches only the anzulegende Wert, does not touch it.
        if let Some(mgp_ct) = product.eeg_managementpraemie_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Managementprämie Direktvermarktung",
                    kwh,
                    "kWh",
                    mgp_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("Direktvermarktungsvertrag")
                .with_tag("eeg_managementpraemie")
                .with_tag("eeg"),
            );
        }
        if let Some(kwkg_ct) = product.kwkg_zuschlag_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "KWKG Zuschlag",
                    kwh,
                    "kWh",
                    kwkg_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§7 KWKG 2023")
                .with_tag("kwkg_zuschlag")
                .with_tag("kwkg"),
            );
        }
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

/// Bridge from eeg-billing SettleOutput → Vec<BillingPosition>.
///
/// EEG settlements are positive values — the generator receives this amount.
///
/// Only compiled when the `eeg` feature is enabled.
#[cfg(feature = "eeg")]
fn bill_eeg_full(
    settle_input: &eeg_billing::SettleInput,
    _ctx: &BillingContext,
) -> Result<Vec<BillingPosition>, EngineError> {
    let output = eeg_billing::calculate_settlement(settle_input);
    let positions = output
        .positions
        .into_iter()
        .map(|p| BillingPosition {
            description: p.description,
            legal_basis: Some(p.legal_basis),
            quantity: p.kwh,
            unit: "kWh".to_owned(),
            unit_price_eur: p.rate_ct_kwh / dec!(100),
            // Positive: generator receives payment (credit note perspective)
            net_eur: validated_eur(p.eur),
            category: PositionCategory::Credit,
            tags: vec!["eeg".to_owned(), "eeg_full".to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::default(),
        })
        .collect();
    Ok(positions)
}

// ── EinspeisungProvider ───────────────────────────────────────────────────────

/// Non-EEG Direktvermarktung feed-in settlement (EINSPEISUNG).
pub struct EinspeisungProvider {
    product: EinspeisungProduct,
}

impl EinspeisungProvider {
    pub fn new(product: EinspeisungProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for EinspeisungProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.einspeisung.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let kwh = meter.einspeisung_kwh;
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(mv_ct) = product.marktwert_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Marktwert Strom (EPEX Spot Monatsmarktwert)",
                    kwh,
                    "kWh",
                    mv_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 EEG 2023")
                .with_tag("marktwert")
                .with_tag("einspeisung"),
            );
        }
        if let Some(vm_ct) = product.vermarktungsgebuehr_ct_per_kwh {
            // Vermarktungsgebühr is a cost for the generator (reduces net payment)
            positions.push(
                BillingPosition::debit(
                    "Vermarktungsgebühr Direktvermarktung",
                    kwh,
                    "kWh",
                    -(vm_ct / dec!(100)), // negative: cost deducted from settlement
                    PositionCategory::Fee,
                )
                .with_tag("vermarktungsgebuehr")
                .with_tag("einspeisung"),
            );
        }
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── HemsProvider ──────────────────────────────────────────────────────────────

/// HEMS subscription + event billing provider.
pub struct HemsProvider {
    product: HemsProduct,
}

impl HemsProvider {
    pub fn new(product: HemsProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for HemsProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let usage = quantities.hems.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        let sub_eur = product.hems_subscription_eur_per_month;

        if let Some(sub_eur) = sub_eur {
            positions.push(
                BillingPosition::debit(
                    "HEMS Grundgebühr",
                    months,
                    "Monate",
                    sub_eur,
                    PositionCategory::Fee,
                )
                .with_tag("hems_subscription")
                .with_tag("hems"),
            );
        }
        if let (Some(events), Some(event_eur)) = (
            usage.optimization_events,
            product.hems_optimization_event_eur,
        ) && events > 0
        {
            positions.push(
                BillingPosition::debit(
                    "HEMS Optimierungsereignisse",
                    Decimal::from(events),
                    "Ereignisse",
                    event_eur,
                    PositionCategory::Fee,
                )
                .with_tag("hems_events")
                .with_tag("hems"),
            );
        }
        if let (Some(reads), Some(read_eur)) =
            (usage.readout_events, product.hems_readout_event_eur)
            && reads > 0
        {
            positions.push(
                BillingPosition::debit(
                    "HEMS Smart Meter Ablesungen",
                    Decimal::from(reads),
                    "Ablesungen",
                    read_eur,
                    PositionCategory::Fee,
                )
                .with_tag("hems_readouts")
                .with_tag("hems"),
            );
        }
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── EmobilityProvider ─────────────────────────────────────────────────────────

/// E-Mobility CPO/EMSP billing provider.
pub struct EmobilityProvider {
    product: EmobilityProduct,
}

impl EmobilityProvider {
    pub fn new(product: EmobilityProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for EmobilityProvider {
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        // The `KEIN_ARBEITSPREIS` invariant on the charging side. Energy that
        // was demonstrably delivered — `kwh_charged` is a measured figure off
        // the charge point — against a product with no per-kWh price bills the
        // monthly Servicegebühr and nothing for the electricity.
        //
        // An EMSP whose tariff genuinely bundles charging into the flat fee
        // says so with a `0.0`, the same way every other product in this crate
        // distinguishes a decision from missing data.
        let charged = quantities
            .emobility
            .as_ref()
            .and_then(|u| u.kwh_charged)
            .unwrap_or(Decimal::ZERO);
        if charged > Decimal::ZERO && self.product.emobility_kwh_price_ct.is_none() {
            return vec![BillingWarning {
                code: "KEIN_LADEPREIS",
                severity: WarningSeverity::Error,
                message: format!(
                    "es wurden {charged} kWh geladen, das Produkt nennt aber keinen \
                     Arbeitspreis (emobility_kwh_price_ct) — die Rechnung enthielte allein \
                     die Service- und Sessiongebühren und nichts für die Ladeenergie. Preis \
                     hinterlegen, oder 0.0 setzen, wenn das Laden in der Grundgebühr \
                     enthalten ist."
                ),
            }];
        }
        Vec::new()
    }

    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let usage = quantities.emobility.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        let svc_eur = product.emobility_service_fee_eur;
        let kwh_price = product.emobility_kwh_price_ct;

        if let Some(svc_eur) = svc_eur {
            positions.push(
                BillingPosition::debit(
                    "E-Mobility Servicegebühr",
                    months,
                    "Monate",
                    svc_eur,
                    PositionCategory::Fee,
                )
                .with_tag("emobility_service")
                .with_tag("emobility"),
            );
        }
        if let (Some(kwh), Some(kwh_price_ct)) = (usage.kwh_charged, kwh_price)
            && kwh > Decimal::ZERO
        {
            positions.push(
                arbeitspreis_position(
                    "E-Mobility Ladeenergie",
                    kwh,
                    kwh_price_ct,
                    "kWh",
                    "§41a EnWG",
                    &["emobility"],
                )
                .with_tag("emobility"),
            );
        }
        if let (Some(sessions), Some(session_eur)) =
            (usage.sessions, product.emobility_session_fee_eur)
            && sessions > 0
        {
            positions.push(
                BillingPosition::debit(
                    "E-Mobility Ladesessionsgebühr",
                    Decimal::from(sessions),
                    "Sessionen",
                    session_eur,
                    PositionCategory::Fee,
                )
                .with_tag("emobility_sessions")
                .with_tag("emobility"),
            );
        }
        if let (Some(roaming), Some(roaming_eur)) =
            (usage.roaming_sessions, product.emobility_roaming_fee_eur)
            && roaming > 0
        {
            positions.push(
                BillingPosition::debit(
                    "E-Mobility Roaming-Gebühr",
                    Decimal::from(roaming),
                    "Sessionen",
                    roaming_eur,
                    PositionCategory::Fee,
                )
                .with_tag("emobility_roaming")
                .with_tag("emobility"),
            );
        }
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── ServiceProvider ───────────────────────────────────────────────────────────

/// Energiedienstleistung (MSB, EMS, maintenance) billing provider.
pub struct ServiceProvider {
    product: ServiceProduct,
}

impl ServiceProvider {
    pub fn new(product: ServiceProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for ServiceProvider {
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        // Billable events were counted and neither the usage nor the product
        // prices them, so they fall off the invoice silently.
        //
        // A Warning rather than an Error: unlike delivered energy, an event
        // count is also a legitimate *informational* figure — an operator may
        // report how many Störungseinsätze a maintenance flat rate covered
        // without charging per event. The count alone does not settle which was
        // meant, so this names the ambiguity instead of refusing the run.
        let events = quantities
            .service
            .as_ref()
            .and_then(|u| u.event_count)
            .unwrap_or(0);
        let priced = quantities
            .service
            .as_ref()
            .and_then(|u| u.event_price_eur)
            .or(self.product.service_event_price_eur)
            .is_some();
        if events > 0 && !priced {
            return vec![BillingWarning {
                code: "KEIN_EREIGNISPREIS",
                severity: WarningSeverity::Warning,
                message: format!(
                    "{events} abrechenbare Ereignisse übermittelt, aber weder \
                     `event_price_eur` noch `service_event_price_eur` gesetzt — sie \
                     erscheinen nicht auf der Rechnung. Preis hinterlegen, oder 0.0 \
                     setzen, wenn die Ereignisse durch die Grundgebühr abgegolten sind."
                ),
            }];
        }
        Vec::new()
    }

    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let usage = quantities.service.as_ref().cloned().unwrap_or_default();
        let product = &self.product;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(fee_eur) = product.service_fee_eur {
            positions.push(
                BillingPosition::debit(
                    "Energiedienstleistung Grundgebühr",
                    months,
                    "Monate",
                    fee_eur,
                    PositionCategory::Fee,
                )
                .with_tag("service"),
            );
        }
        let event_price = usage.event_price_eur.or(product.service_event_price_eur);
        if let (Some(events), Some(event_eur)) = (usage.event_count, event_price)
            && events > 0
        {
            positions.push(
                BillingPosition::debit(
                    "Energiedienstleistung Ereignisgebühr",
                    Decimal::from(events),
                    "Ereignisse",
                    event_eur,
                    PositionCategory::Fee,
                )
                .with_tag("service_events")
                .with_tag("service"),
            );
        }
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        Ok(positions)
    }
}

// ── DynamicElectricityProvider ────────────────────────────────────────────────

/// §41a EnWG dynamic electricity tariff — per-interval spot pricing.
///
/// Prices come from [`Quantities::dynamic_epex_prices`], keyed on each market
/// time unit's UTC start ([`mtu_start`](crate::mtu_start)). Also emits NNE,
/// Konzessionsabgabe, Stromsteuer and the § 40 Abs. 2 EnWG display positions.
pub struct DynamicElectricityProvider {
    product: ElectricityProduct,
    grid: GridInput,
}

impl DynamicElectricityProvider {
    #[must_use]
    pub fn new(product: ElectricityProduct, grid: GridInput) -> Self {
        Self { product, grid }
    }
}

impl BillingProvider for DynamicElectricityProvider {
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        // §41a Abs. 1 EnWG — dynamic tariffs require iMSys (Smart Meter Gateway).
        // If the metering mode is explicitly set to SLP or RLM, this is a definite
        // regulatory violation that must block the billing run.
        let is_non_imsys = quantities
            .electricity
            .as_ref()
            .is_some_and(|m| m.metering_mode != crate::quantities::MeteringMode::Imsys);
        if is_non_imsys {
            return vec![BillingWarning {
                code: "SECT41A_IMSYS_REQUIRED",
                severity: WarningSeverity::Error,
                message: "§41a Abs. 1 EnWG: dynamic tariffs require an intelligent \
                     metering system (iMSys / Smart Meter Gateway). The meter point \
                     has MeteringMode::Slp or MeteringMode::Rlm. Update metering mode \
                     to MeteringMode::Imsys or switch the customer to a fixed-price product."
                    .to_owned(),
            }];
        }

        let mut w = Vec::new();
        // On this path the **interval series is the quantity**: every amount —
        // Arbeitspreis, NNE Arbeitspreis, Konzessionsabgabe, Stromsteuer — is
        // charged on the sum of the priced intervals, and nothing else reads
        // the meter total. So an absent or short series does not bill less
        // energy at the right price; it bills a Grundpreis-only invoice and
        // states no consumption at all, silently.
        //
        // The meter total is the independent witness that says whether that
        // happened. It is only a witness when it was supplied — a caller that
        // sends intervals alone is not making a claim to contradict.
        let stated_kwh = quantities
            .electricity
            .as_ref()
            .map_or(Decimal::ZERO, crate::quantities::MeterInput::billable_kwh);
        if stated_kwh > Decimal::ZERO {
            let interval_kwh: Decimal = quantities.dynamic_intervals.iter().map(|i| i.kwh).sum();
            if quantities.dynamic_intervals.is_empty() {
                w.push(BillingWarning {
                    code: "SECT41A_KEINE_INTERVALLE",
                    severity: WarningSeverity::Error,
                    message: format!(
                        "§ 41a EnWG: der Zähler meldet {stated_kwh} kWh, es liegt aber keine \
                         Viertelstunden-Zeitreihe vor. Auf dem dynamischen Pfad ist die \
                         Zeitreihe die Abrechnungsmenge — ohne sie entstünde eine Rechnung \
                         über den Grundpreis und keine einzige kWh. Zeitreihe aus edmd \
                         nachladen oder den Kunden über einen Festpreistarif abrechnen."
                    ),
                });
            } else {
                // Interval sums and register differences never agree to the
                // last digit — the series is per-quarter-hour rounded, the
                // total is a difference of two readings. Half a percent (with a
                // 1 kWh floor for small accounts) separates that from a series
                // that is genuinely missing days.
                let tolerance = (stated_kwh * dec!(0.005)).max(Decimal::ONE);
                let gap = (interval_kwh - stated_kwh).abs();
                if gap > tolerance {
                    w.push(BillingWarning {
                        code: "SECT41A_INTERVALLSUMME_WEICHT_AB",
                        severity: WarningSeverity::Error,
                        message: format!(
                            "§ 41a EnWG: die Summe der Viertelstundenwerte ({interval_kwh} kWh) \
                             weicht um {gap} kWh vom gemeldeten Zählerverbrauch \
                             ({stated_kwh} kWh) ab (Toleranz {tolerance} kWh). Abgerechnet \
                             würde die Zeitreihe — Arbeitspreis, Netzentgelt und Stromsteuer \
                             also auf der niedrigeren Menge. Zeitreihe vervollständigen oder \
                             den Zählerstand korrigieren."
                        ),
                    });
                }
            }
        }
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let product = &self.product;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;
        let floor_ct = product.dynamic_epex_floor_ct_kwh;
        let cap_ct = product.dynamic_epex_cap_ct_kwh;
        // §41a EnWG: the customer's per-kWh price is the market spot price plus
        // the Lieferant's fixed Arbeitspreis-Aufschlag (margin). The floor caps
        // the spot component from below (protecting the Lieferant against negative
        // prices); an optional cap limits it from above (consumer protection); the
        // Aufschlag is then added on top.
        // The § 41a margin has its own field: sharing
        // `auf_abschlag_ct_per_kwh` with the static path's Rabatt/Aufschlag line
        // would make one number mean "margin on the spot price" here and
        // "discount off the work price" three providers away.
        let aufschlag_ct = product
            .dynamic_aufschlag_ct_per_kwh
            .unwrap_or(Decimal::ZERO);
        let source_name = product
            .dynamic_price_source
            .clone()
            .unwrap_or_else(|| "EPEX Spot Day-Ahead".to_owned());
        let mut positions: Vec<BillingPosition> = Vec::new();

        // Grundpreis — active contract days, like every other Grundpreis in this
        // crate: a mid-period move-in must not be charged the full period.
        if let Some(gp_ct_day) = product.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis Strom (§41a)",
                    gp_ct_day / dec!(100),
                    ctx.prorate_days().0 as i64,
                    "§41a EnWG",
                    &["strom"],
                )
                .with_tag("strom"),
            );
        }

        // Per-interval EPEX pricing via `billing::DynamicPricing`.
        //
        // `DynamicPricing` computes the weighted-average unit price using
        // `Amount<5>` arithmetic throughout — no intermediate Decimal accumulation.
        // We pass it (kwh, eur_per_kwh) pairs; it returns a single `LineItem` from
        // which we extract `net_amount` and `quantity_value` to build our own
        // `BillingPosition` (with energy-billing tags and legal basis).
        //
        // Primary price source: `self.spot_price_source` (live API / Tibber / NordPool).
        // Fallback: `quantities.dynamic_epex_prices` (pre-fetched map from billingd /
        // marktd). This is the typical production path when `build_engine()` creates
        // the provider before prices are known.
        let mut missing_price_intervals: u32 = 0;
        let mut missing_price_kwh = Decimal::ZERO;
        let mut priced_pairs: Vec<(Decimal, billing::Amount<5>)> =
            Vec::with_capacity(quantities.dynamic_intervals.len());

        for interval in &quantities.dynamic_intervals {
            // Floor the interval start to its 15-min MTU (DST-safe, UTC).
            let key = crate::provider::mtu_start(interval.timestamp_utc);
            let price_ct = quantities.dynamic_epex_prices.get(&key).copied();

            let Some(price_ct) = price_ct else {
                missing_price_intervals += 1;
                // Consumption in an unpriced interval cannot be billed at all —
                // track it so an incomplete price series hard-blocks below
                // rather than silently under-billing.
                missing_price_kwh += interval.kwh;
                continue;
            };

            let mut spot_ct = price_ct;
            if let Some(floor) = floor_ct {
                spot_ct = spot_ct.max(floor);
            }
            if let Some(cap) = cap_ct {
                spot_ct = spot_ct.min(cap);
            }
            // §41a: market spot clamped into [floor, cap] + fixed Arbeitspreis-Aufschlag.
            let effective_ct = spot_ct + aufschlag_ct;

            // ct/kWh → EUR/kWh as Amount<5>. Rounding kaufmännisch to 5 dp first
            // ensures the Decimal fits the target precision before conversion.
            // EPEX prices are typically 2 dp in ct/kWh → 4 dp after /100, so this
            // never loses precision in practice.
            //
            // A conversion that does not fit is an error, not a skip: silently
            // dropping the interval is the same under-bill the missing-price
            // guard below refuses, arrived at by a different route.
            let price_eur = billing::Amount::<5>::try_from((effective_ct / dec!(100)).round_kfm(5))
                .map_err(|_| EngineError::PriceOutOfRange {
                    field: format!("spotpreis@{}", interval.timestamp_utc),
                    value: effective_ct,
                })?;
            priced_pairs.push((interval.kwh, price_eur));
        }

        // §41a EnWG requires a dynamic tariff to be billed on verifiable market
        // prices for the consumed energy. Consumption in an interval with no
        // EPEX price cannot be billed at all — dropping it would silently
        // under-bill and produce an unverifiable invoice. Any missing-price
        // interval that carries consumption hard-blocks the run, exactly like
        // the §41a iMSys guard, rather than degrading to a partial bill.
        if missing_price_kwh > Decimal::ZERO {
            return Err(EngineError::ValidationBlocked {
                warnings: vec![BillingWarning {
                    code: "SECT41A_MISSING_EPEX_PRICES",
                    severity: WarningSeverity::Error,
                    message: format!(
                        "§41a EnWG: {missing_price_intervals} interval(s) totalling \
                         {missing_price_kwh} kWh have no EPEX Spot price. A dynamic \
                         tariff cannot be billed on an incomplete price series — import \
                         the missing prices (PUT /api/v1/epex-prices/{{date}}) or the \
                         invoice would silently under-bill."
                    ),
                }],
            });
        }
        // Missing prices only in zero-consumption intervals are harmless (no
        // money is at stake); note them for observability and continue.
        if missing_price_intervals > 0 {
            tracing::warn!(
                missing_intervals = missing_price_intervals,
                total_intervals = quantities.dynamic_intervals.len(),
                "DynamicElectricityProvider: {missing_price_intervals} zero-consumption \
                 interval(s) had no EPEX price."
            );
        }

        if !priced_pairs.is_empty() {
            let item = DynamicPricing::builder()
                .intervals(priced_pairs)
                .unit("kWh")
                .currency(Currency::EUR)
                .build()
                .and_then(|dp| dp.calculate())?;

            let total_kwh = item.quantity_value().unwrap_or_default();
            let total_eur = item.net_amount.into_decimal();
            // The weighted-average unit price is the crate's, not ours:
            // `DynamicPricing::calculate` computes it while it sums and hands it
            // back as the `LineItem`'s `unit_price` in EUR/kWh. Re-deriving it
            // as `net ÷ quantity` would be a second division on the figure that
            // has to agree with the amount beside it.
            //
            // It is carried at **full precision**, and only the description
            // rounds. A weighted average is rarely representable, so a price
            // rounded for the page no longer multiplies out to its own line:
            // **PEPPOL-EN16931-R120** allows ±0.02 between `price × quantity`
            // and the amount, and four decimal places in ct drift past that at
            // industrial volumes — €2.42 over a spot-linked year at 2 MW. The
            // reader gets the rounded average in the description; the machine
            // field stays exact.
            let avg_eur = item.unit_price.as_ref().map_or(Decimal::ZERO, |p| p.value);
            let avg_ct = (avg_eur * dec!(100)).round_kfm(4);
            positions.push(BillingPosition {
                description: format!("Arbeitspreis {source_name} (∅ {avg_ct:.4} ct/kWh)",),
                legal_basis: Some("§41a EnWG".to_owned()),
                quantity: total_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: avg_eur,
                net_eur: total_eur,
                category: PositionCategory::Commodity,
                tags: vec![
                    "commodity".to_owned(),
                    "arbeitspreis".to_owned(),
                    "strom".to_owned(),
                    "§41a".to_owned(),
                ],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });

            // NNE + KA
            if let Some(nne_ap_ct) = grid.nne_arbeitspreis_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Netznutzungsentgelt Arbeitspreis",
                        total_kwh,
                        "kWh",
                        nne_ap_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("StromNEV")
                    .with_tag("nne_arbeitspreis")
                    .with_tag("nne"),
                );
            }
            if let Some(ka_ct) = grid.ka_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Konzessionsabgabe",
                        total_kwh,
                        "kWh",
                        ka_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("KAV §2")
                    .with_tag("konzessionsabgabe")
                    .with_tag("nne"),
                );
            }

            // Stromsteuer — through the same § 9 StromStG resolution the static
            // path uses, so a Befreiung or an ermäßigter Satz cannot apply to
            // one kind of electricity tariff and not the other.
            positions.extend(stromsteuer_positions(
                product.stromsteuer_tarif,
                total_kwh,
                rates.effective_stromsteuer(product.stromsteuer_ct_per_kwh_override),
                &["strom"],
            ));
        }

        // NNE Grundpreis
        if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
            // Leap-aware: an EUR/year rate divides by that year's actual days
            // (366 in 2024/2028), or the daily rate overstates the Grundpreis.
            let daily = nne_gp / Decimal::from(time::util::days_in_year(ctx.period_from().year()));
            // Active contract days, not the full billing period: the NNE
            // Grundpreis accrues only while the contract supplies the MaLo, the
            // same clipping the commodity Grundpreis applies. Billing the full
            // period over-charged every mid-period move-in and move-out.
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Grundpreis",
                    Decimal::from(ctx.prorate_days().0),
                    "Tage",
                    daily,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_grundpreis")
                .with_tag("nne"),
            );
        }

        // The MSB fee, the bonuses and the § 40 Abs. 2 EnWG display duties are
        // the same on a dynamic invoice as on a static one.
        let meter = quantities.electricity.as_ref().cloned().unwrap_or_default();
        positions.extend(electricity_common_positions(ctx, product, &meter));

        // A Steuerentlastung leaves the levy where it is — see `crate::steuer`.
        let hinweise = entlastungs_hinweise(&product.steuerentlastungen, &positions);
        positions.extend(hinweise);

        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: e.g. 7% Trinkwasser (§12 Abs. 2 Nr. 1 UStG, Anlage 2),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = product.mwst_rate_override {
            for pos in &mut positions {
                if pos.applicable_tax_rate.is_none()
                    && !matches!(
                        pos.category,
                        PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
                    )
                {
                    pos.applicable_tax_rate = Some(rate);
                }
            }
        }
        // ── §41a Abs. 6 EnWG — Annual savings comparison
        if let Some(comp) = &quantities.sect41a_annual_comparison {
            let sign = if comp.savings_eur >= Decimal::ZERO {
                "Ersparnis"
            } else {
                "Mehrkosten"
            };
            positions.push(BillingPosition {
                description: format!(
                    "§41a Abs. 6 EnWG Jahresvergleich: {:.2} EUR (Dynamisch) vs. {:.2} EUR (Festpreis {:.4} ct/kWh) -> {} {:.2} EUR",
                    comp.actual_eur_brutto, comp.reference_eur_brutto,
                    comp.reference_price_ct_per_kwh, sign, comp.savings_eur.abs(),
                ),
                legal_basis: Some("§41a Abs. 6 EnWG".to_owned()),
                quantity: comp.actual_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["sect41a_annual_comparison".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        Ok(positions)
    }
}

// ── MwStProvider ──────────────────────────────────────────────────────────────

/// MwSt (Mehrwertsteuer / Umsatzsteuer) provider — supports **multi-rate VAT**.
///
/// **Must be registered last** — computes tax on the sum of ALL prior positions.
///
/// ## Multi-rate VAT (\u00a712 UStG)
///
/// The provider groups prior positions by their `applicable_tax_rate`:
/// - `None` \u2192 uses the engine-wide default rate (passed to `new()`)
/// - `Some(dec!(0.19))` \u2192 standard rate
/// - `Some(dec!(0.07))` \u2192 reduced rate (\u00a712 Abs. 2 Nr. 1 UStG for renewable Fernw\u00e4rme)
/// - `Some(dec!(0.0))` \u2192 zero rate (\u00a712 Abs. 3 UStG for solar PV \u226430 kWp since 01.01.2023)
///
/// One `Tax` position is generated per distinct rate group.
/// Groups with `rate = 0` produce no Tax position.
pub struct MwStProvider {
    /// Default MwSt rate for positions without an explicit `applicable_tax_rate`.
    rate: Decimal,
}

impl MwStProvider {
    /// Construct with the engine-wide default MwSt rate (e.g. `dec!(0.19)`).
    #[must_use]
    pub fn new(rate: Decimal) -> Self {
        Self { rate }
    }
}

impl BillingProvider for MwStProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        _quantities: &Quantities,
        prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        use std::collections::BTreeMap;

        // Group taxable positions by their effective MwSt rate.
        // Tax, Abschlag, Info positions are excluded from the tax base.
        let mut rate_buckets: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();

        for p in prior {
            if matches!(
                p.category,
                PositionCategory::Tax | PositionCategory::Abschlag | PositionCategory::Info
            ) {
                continue;
            }
            let effective_rate = p.applicable_tax_rate.unwrap_or(self.rate).normalize();
            if effective_rate.is_zero() {
                continue; // zero rate \u2192 no tax position
            }
            // Normalised, exactly as `tax_subtotals_of` groups the BG-23
            // breakdown: `0.19` and `0.190` are one rate, and bucketing them
            // apart rounded each half on its own, so `gesamtsteuer` could land
            // a cent away from `\u03a3 steuerbetraege`.
            let key = effective_rate.to_string();
            let entry = rate_buckets
                .entry(key)
                .or_insert((effective_rate, Decimal::ZERO));
            entry.1 += p.net_eur;
        }

        if rate_buckets.is_empty() {
            return Ok(vec![]);
        }

        let mut tax_positions: Vec<BillingPosition> = Vec::with_capacity(rate_buckets.len());
        for (_key, (rate, net_base)) in rate_buckets {
            if net_base.is_zero() {
                continue;
            }
            // Rounded to the cent per rate, not carried at 5 dp: the Steuerbetrag
            // is a document amount (§14 Abs. 4 Nr. 8 UStG) and the BO4E/EN 16931
            // per-rate breakdown states it to the cent. Summing 5-dp tax layers
            // and rounding once at the end can land a cent away from the sum of
            // the stated Steuerbeträge, which breaks the documented invariant
            // "Σ steuerbetraege == gesamtsteuer" (19 % 1.995 + 7 % 0.525 →
            // 2.00 + 0.53 = 2.53, not round2(2.52)).
            let mwst_eur = validated_eur((net_base.abs() * rate).round_kfm(2));
            // Sign follows the net base (credit invoices \u2192 negative MwSt)
            let mwst_eur = if net_base < Decimal::ZERO {
                -mwst_eur
            } else {
                mwst_eur
            };
            let pct = (rate * dec!(100)).normalize();
            tax_positions.push(BillingPosition {
                description: format!("Mehrwertsteuer {pct}\u{202f}%"),
                legal_basis: Some("\u{a7}12 UStG".to_owned()),
                quantity: Decimal::ONE,
                unit: "%".to_owned(),
                unit_price_eur: mwst_eur,
                net_eur: mwst_eur,
                category: PositionCategory::Tax,
                tags: vec!["mwst".to_owned(), "tax".to_owned()],
                applicable_tax_rate: Some(rate),
                trace: crate::position::PositionTrace::tax(rate, net_base, "§12 UStG"),
            });
        }

        Ok(tax_positions)
    }

    fn is_tax_pass(&self) -> bool {
        true
    }

    fn charged_tax_rate(&self) -> Option<Decimal> {
        Some(self.rate)
    }
}

/// The positions every electricity invoice carries regardless of how the
/// Arbeitspreis was formed — the MSB fee, the contractual bonuses and the
/// § 40 Abs. 2 EnWG display duties.
///
/// Shared by the static and the § 41a dynamic providers. The dynamic path used
/// to emit none of them: a dynamic-tariff invoice went out with no Zählerstand
/// (Nr. 6), no Vorjahresvergleich (Nr. 7), no Vergleichsgruppe (Nr. 8) and no
/// estimation notice, while the identically-regulated static invoice carried
/// all four.
fn electricity_common_positions(
    ctx: &BillingContext,
    product: &ElectricityProduct,
    meter: &crate::quantities::MeterInput,
) -> Vec<BillingPosition> {
    let mut positions: Vec<BillingPosition> = Vec::new();
    let days = ctx.prorate_days().0 as i64;
    // ── Boni (Neukunden-/Sofort-/Treuebonus) ───────────────────────────────
    // A contractual bonus is a Preisnachlass (§17 UStG Entgeltminderung): it
    // rides as a negative Bonus position that reduces the taxable base, so the
    // MwSt is computed on the net after the bonus (not a gross gift on top).
    if let Some(bonus) = product.sofortbonus_eur.filter(|v| *v > Decimal::ZERO) {
        positions.push(
            BillingPosition::debit(
                "Sofortbonus / Neukundenbonus",
                Decimal::ONE,
                "Bonus",
                -bonus,
                PositionCategory::Bonus,
            )
            .with_legal_basis("Vertraglich (§17 UStG Entgeltminderung)")
            .with_tag("bonus")
            .with_tag("sofortbonus"),
        );
    }
    if let Some(treue) = product
        .treuebonus_eur_per_year
        .filter(|v| *v > Decimal::ZERO)
    {
        // Pro-rate the annual loyalty bonus to the billed contract days.
        let frac = ctx.billed_years().round_kfm(4);
        positions.push(
            BillingPosition::debit(
                "Treuebonus (anteilig)",
                frac,
                "Jahr",
                -treue,
                PositionCategory::Bonus,
            )
            .with_legal_basis("Vertraglich (§17 UStG Entgeltminderung)")
            .with_tag("bonus")
            .with_tag("treuebonus"),
        );
    }
    // ── MSB Grundgebühr ────────────────────────────────────────────────────
    // Messstellenbetreiber fee bundled into the retail invoice (MsbG 2016).
    // Itemised separately per §41 EnWG.
    if let Some(msb_ct_day) = product
        .msb_gebuehr_ct_per_day
        .filter(|v| *v > Decimal::ZERO)
    {
        positions.push(
            BillingPosition::debit(
                "Messstellenbetrieb Grundgebühr",
                Decimal::from(days),
                "Tage",
                msb_ct_day / dec!(100),
                PositionCategory::Fee,
            )
            .with_legal_basis("MsbG")
            .with_tag("msb_gebuehr"),
        );
    }

    // ── Zählerstand info positions (§40 Abs. 2 Nr. 6 EnWG) ─────────────────
    if meter.zaehlerstand_von.is_some() || meter.zaehlerstand_bis.is_some() {
        // § 40 Abs. 2 Nr. 6 EnWG names three things: the opening and closing
        // readings, the consumption derived from them, **and how the reading
        // was obtained**. The third is its own duty — a customer can act on a
        // self-reported figure differently from a remote read-out — and it was
        // absent from the page unless the reading happened to be an estimate.
        let label = format!(
            "Zählerstand: {} – {}{}",
            meter
                .zaehlerstand_von
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            meter
                .zaehlerstand_bis
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            meter
                .ablesungsart
                .label()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default(),
        );
        let zid = meter
            .zaehlernummer
            .as_deref()
            .or(ctx.zaehler_id.as_deref())
            .unwrap_or("-");
        positions.push(BillingPosition {
            description: label,
            legal_basis: Some("§40 Abs. 2 Nr. 6 EnWG".to_owned()),
            quantity: Decimal::ZERO,
            unit: "kWh".to_owned(),
            unit_price_eur: Decimal::ZERO,
            net_eur: Decimal::ZERO,
            category: PositionCategory::Info,
            tags: vec!["zaehlerstand".to_owned(), zid.to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::default(),
        });
    }

    // ── §40 Abs. 2 EnWG — Verbrauchshistorie (consumption comparison) ──
    // Mandatory invoice display requirement: show prior-year and average.
    // These are informational positions (EUR 0) — they appear in the invoice
    // printout but do not affect the calculation.
    if let Some(vh) = &ctx.verbrauchshistorie {
        if let Some(vj_kwh) = vh.vorjahr_kwh {
            positions.push(BillingPosition {
                description: format!("Verbrauch Vorjahreszeitraum: {vj_kwh:.0} kWh"),
                legal_basis: Some("§40 Abs. 2 Nr. 7 EnWG".to_owned()),
                quantity: vj_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["verbrauchshistorie".to_owned(), "vorjahr".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }
        if let Some(avg_kwh) = vh.bundesdurchschnitt_kwh {
            let kundengruppe = vh.kundengruppe.as_deref().unwrap_or("Vergleichsgruppe");
            positions.push(BillingPosition {
                description: format!("Bundesdurchschnitt {kundengruppe}: {avg_kwh:.0} kWh"),
                legal_basis: Some("§40 Abs. 2 Nr. 8 EnWG".to_owned()),
                quantity: avg_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec![
                    "verbrauchshistorie".to_owned(),
                    "bundesdurchschnitt".to_owned(),
                ],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }
    }

    // ── § 40a Abs. 2 EnWG — estimated reading notice ─────────────────────
    // Satz 3 requires the estimate, the ground that makes it admissible and
    // the factors behind it to be stated „unter ausdrücklichem und optisch
    // besonders hervorgehobenem Hinweis", and Satz 1 measures it against the
    // customer's own prior period or a comparable customer.
    if meter.is_estimated {
        positions.push(BillingPosition {
            description: "Abrechnungswert: Schätzung gemäß § 40a Abs. 2 EnWG — \
                          auf Wunsch Korrektur nach realer Ablesung"
                .to_owned(),
            legal_basis: Some("§ 40a Abs. 2 EnWG".to_owned()),
            quantity: Decimal::ZERO,
            unit: String::new(),
            unit_price_eur: Decimal::ZERO,
            net_eur: Decimal::ZERO,
            category: PositionCategory::Info,
            tags: vec!["schatzwert".to_owned(), "ersatzwert".to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::default(),
        });
    }

    // ── Zählerwechsel notice ───────────────────────────────────────────────
    if meter.zaehler_replaced {
        positions.push(BillingPosition {
            description: "Zählerwechsel innerhalb des Abrechnungszeitraums".to_owned(),
            legal_basis: Some("§40 Abs. 2 Nr. 6 EnWG".to_owned()),
            quantity: Decimal::ZERO,
            unit: String::new(),
            unit_price_eur: Decimal::ZERO,
            net_eur: Decimal::ZERO,
            category: PositionCategory::Info,
            tags: vec!["zaehlerwechsel".to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::default(),
        });
    }

    // ── Preisgarantie notice (§41 Abs. 1 Nr. 4 EnWG) ─────────────────────
    if let Some(pg_bis) = product.preisgarantie_bis.filter(|d| *d >= ctx.period_to()) {
        positions.push(BillingPosition {
            description: format!("Preisgarantie gültig bis {pg_bis}"),
            legal_basis: Some("§41 Abs. 1 Nr. 4 EnWG".to_owned()),
            quantity: Decimal::ZERO,
            unit: String::new(),
            unit_price_eur: Decimal::ZERO,
            net_eur: Decimal::ZERO,
            category: PositionCategory::Info,
            tags: vec!["preisgarantie".to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::default(),
        });
    }

    positions
}

/// The `INDEXWERT_FEHLT` warning for an index-linked price whose index value
/// has not arrived.
///
/// `Error` when nothing else can price the commodity — an unresolvable index
/// otherwise produces an invoice with a standing charge and no work price, the
/// same silent zero `KEIN_ARBEITSPREIS` exists to refuse. `Warning` when a
/// static price can carry the invoice, because the operator still contracted an
/// indexed one and is about to bill a different number.
fn indexwert_warning(
    idx: Option<&crate::tariff::IndexedPriceConfig>,
    has_other_price: bool,
) -> Option<BillingWarning> {
    let idx = idx.filter(|i| i.index_value.is_none())?;
    Some(BillingWarning {
        code: "INDEXWERT_FEHLT",
        severity: if has_other_price {
            WarningSeverity::Warning
        } else {
            WarningSeverity::Error
        },
        message: format!(
            "der Indexwert für '{}' fehlt — der vertraglich vereinbarte Arbeitspreis \
             kann nicht bestimmt werden",
            idx.index_name
        ),
    })
}

/// A 0-EUR informational position — a statement the invoice must carry that is
/// not an amount.
fn info_position(
    description: impl Into<String>,
    legal_basis: &'static str,
    tags: &[&'static str],
) -> BillingPosition {
    BillingPosition {
        description: description.into(),
        legal_basis: Some(legal_basis.to_owned()),
        quantity: Decimal::ZERO,
        unit: String::new(),
        unit_price_eur: Decimal::ZERO,
        net_eur: Decimal::ZERO,
        category: PositionCategory::Info,
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        applicable_tax_rate: None,
        trace: crate::position::PositionTrace::default(),
    }
}

// ── Verbrauchsteuer helpers ───────────────────────────────────────────────────

/// The Stromsteuer line for a supply — or the exemption notice standing in for it.
///
/// One place decides, so a Befreiung, an Ermäßigung and the Regelsatz cannot
/// diverge between the static, controllable-load and dynamic paths.
fn stromsteuer_positions(
    tarif: crate::steuer::StromsteuerTarif,
    kwh: Decimal,
    regelsatz_ct: Decimal,
    extra_tags: &[&'static str],
) -> Vec<BillingPosition> {
    use crate::steuer::StromsteuerTarif;
    if kwh <= Decimal::ZERO {
        return Vec::new();
    }
    match tarif {
        StromsteuerTarif::Befreiung { grund } => vec![BillingPosition {
            description: grund.description().to_owned(),
            legal_basis: Some(grund.citation().to_owned()),
            quantity: kwh,
            unit: "kWh".to_owned(),
            unit_price_eur: Decimal::ZERO,
            net_eur: Decimal::ZERO,
            category: PositionCategory::Info,
            tags: vec!["stromsteuer_befreiung".to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::commodity(
                kwh,
                "kWh",
                Decimal::ZERO,
                grund.citation(),
            ),
        }],
        StromsteuerTarif::Ermaessigung { grund } => {
            // A reduction is still a levy line. Dropping it — the shape the old
            // `StromsteuerBefreiung::Bahnstrom` variant produced — left 1,142
            // ct/kWh of Fahrstrom tax off every invoice.
            let mut p = levy_position(
                grund.label(),
                kwh,
                "kWh",
                grund.rate_ct_per_kwh(),
                grund.citation(),
                "stromsteuer",
            );
            for t in extra_tags {
                p = p.with_tag(*t);
            }
            vec![p.with_tag("stromsteuer_ermaessigt")]
        }
        StromsteuerTarif::Regel => {
            if regelsatz_ct <= Decimal::ZERO {
                return Vec::new();
            }
            let mut p = levy_position(
                "Stromsteuer",
                kwh,
                "kWh",
                regelsatz_ct,
                "§ 3 StromStG",
                "stromsteuer",
            );
            for t in extra_tags {
                p = p.with_tag(*t);
            }
            vec![p]
        }
    }
}

/// The Energiesteuer line for a gas supply — or the exemption notice.
fn energiesteuer_positions(
    tarif: crate::steuer::EnergiesteuerTarif,
    kwh_hs: Decimal,
    regelsatz_ct: Decimal,
) -> Vec<BillingPosition> {
    use crate::steuer::EnergiesteuerTarif;
    if kwh_hs <= Decimal::ZERO {
        return Vec::new();
    }
    match tarif {
        EnergiesteuerTarif::Befreiung { grund } => vec![BillingPosition {
            description: grund.description().to_owned(),
            legal_basis: Some(grund.citation().to_owned()),
            quantity: kwh_hs,
            unit: "kWh_Hs".to_owned(),
            unit_price_eur: Decimal::ZERO,
            net_eur: Decimal::ZERO,
            category: PositionCategory::Info,
            tags: vec!["energiesteuer_gas_befreiung".to_owned(), "gas".to_owned()],
            applicable_tax_rate: None,
            trace: crate::position::PositionTrace::commodity(
                kwh_hs,
                "kWh_Hs",
                Decimal::ZERO,
                grund.citation(),
            ),
        }],
        EnergiesteuerTarif::Regel => {
            if regelsatz_ct <= Decimal::ZERO {
                return Vec::new();
            }
            vec![
                levy_position(
                    "Energiesteuer Erdgas",
                    kwh_hs,
                    "kWh_Hs",
                    regelsatz_ct,
                    // § 2 Abs. 3 Satz 1 Nr. 4 is the Erdgas-als-Heizstoff rate.
                    // The old citation "§2 Nr. 3" names no provision at all.
                    "§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG",
                    "energiesteuer_gas",
                )
                .with_tag("gas"),
            ]
        }
    }
}

/// One informational note per [`Steuerentlastung`](crate::steuer::Steuerentlastung),
/// quantifying the levy it may be claimed against.
///
/// Never an amount: an Entlastung is the customer's filing, and the supply on
/// this invoice was taxed in full. The note exists because the customer cannot
/// file without knowing the figure.
fn entlastungs_hinweise(
    entlastungen: &[crate::steuer::Steuerentlastung],
    prior: &[BillingPosition],
) -> Vec<BillingPosition> {
    entlastungen
        .iter()
        .map(|e| {
            let levied = BillingPosition::total_by_tag(prior, e.levy_tag());
            BillingPosition {
                description: format!("{} (ausgewiesen: {levied:.2} EUR)", e.hinweis()),
                legal_basis: Some(e.citation().to_owned()),
                quantity: Decimal::ZERO,
                unit: String::new(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["steuerentlastung".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            }
        })
        .collect()
}

// ── billing crate bridge helpers ──────────────────────────────────────────────

/// Convert a [`billing::LineItem`] to a [`BillingPosition`].
///
/// The `billing` crate is domain-agnostic; this adapter attaches energy-domain
/// metadata (`category`, `legal_basis`, `tags`) to the generic `LineItem`.
/// Used by [`build_block_tariff_positions`] and any other paths that delegate
/// to billing-crate primitives.
#[inline]
fn billing_item_to_position(
    item: billing::LineItem,
    category: PositionCategory,
    legal_basis: &str,
    tags: &[&str],
) -> BillingPosition {
    BillingPosition {
        description: item.description,
        legal_basis: Some(legal_basis.to_owned()),
        quantity: item.quantity.as_ref().map(|q| q.value).unwrap_or_default(),
        unit: item
            .quantity
            .as_ref()
            .map(|q| q.unit.clone())
            .unwrap_or_default(),
        unit_price_eur: item
            .unit_price
            .as_ref()
            .map(|p| p.value)
            .unwrap_or_default(),
        net_eur: item.net_amount.into_decimal(),
        category,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        applicable_tax_rate: None,
        trace: crate::position::PositionTrace::default(),
    }
}

/// Build block tariff `BillingPosition`s using [`billing::RateSchedule`].
///
/// Replaces the manual tier-iteration loop with the well-tested graduated
/// schedule from the `billing` crate, gaining:
/// - Contiguous-band validation on construction (catches misconfigured tiers)
/// - Correct open-ended last-tier handling
/// - Exact `Amount<5>` arithmetic (no intermediate float money)
///
/// ## Legal basis
///
/// §41 EnWG — block tariffs (Blocktarif / Staffelpreis) are permissible for
/// electricity and gas supply contracts.
fn build_block_tariff_positions(
    tiers: &[crate::tariff::BlockTierInput],
    kwh: Decimal,
    extra_tags: &[&str],
) -> Result<Vec<BillingPosition>, EngineError> {
    let mut builder = RateSchedule::graduated().unit("kWh");
    let mut prev: Option<Decimal> = None;

    for (idx, tier) in tiers.iter().enumerate() {
        let price_eur =
            billing::Amount::<5>::try_from((tier.preis_ct_per_kwh / dec!(100)).round_kfm(5))
                .map_err(|_| EngineError::PriceOutOfRange {
                    field: format!("blocktarif_stufe_{}_preis_ct_per_kwh", idx + 1),
                    value: tier.preis_ct_per_kwh,
                })?;
        let desc = match tier.bis_kwh {
            Some(upper) => format!(
                "Arbeitspreis Strom Stufe {} (bis {upper}\u{202f}kWh)",
                idx + 1
            ),
            None => format!("Arbeitspreis Strom Stufe {}", idx + 1),
        };
        let band = match (prev, tier.bis_kwh) {
            (None, Some(upper)) => RateBand::up_to(upper, price_eur),
            (Some(lower), Some(upper)) => RateBand::between(lower, upper, price_eur),
            (lower, None) => RateBand::over(lower.unwrap_or(Decimal::ZERO), price_eur),
        }
        .with_description(desc);
        builder = builder.band(band);
        prev = tier.bis_kwh;
    }

    let items = builder.build().and_then(|s| s.split(kwh))?;
    let mut tags: Vec<&str> = vec!["strom", "arbeitspreis", "block_tier"];
    tags.extend_from_slice(extra_tags);
    Ok(items
        .into_iter()
        .map(|item| billing_item_to_position(item, PositionCategory::Commodity, "§41 EnWG", &tags))
        .collect())
}

// ── EnergyShareProvider ───────────────────────────────────────────────────────

/// §42c EnWG Energy Sharing — community energy allocation credit provider.
///
/// Generates a credit position for the customer's share of locally produced
/// electricity from the community energy pool (Energiegemeinschaft). The credit
/// reduces the effective energy cost without affecting the grid-consumption billing
/// (which is handled by the `ElectricityProvider` in the same engine).
///
/// ## Legal basis
///
/// §42c EnWG (Energy Sharing, EnWG-Novelle BGBl. 2025 I Nr. 347; obligatory within
/// a single Bilanzkreis from 01.06.2026, extended to adjacent Bilanzkreise in the
/// same Regelzone from 01.06.2028): participants in a registered Energiegemeinschaft
/// may receive allocated shares of local generation.
/// The Lieferant bills full grid consumption (§41 EnWG) and separately credits the
/// sharing allocation at the contracted sharing rate.
///
/// ## §41a intersection
///
/// If the sharing tariff is combined with a dynamic tariff (`STROM` + dynamic EPEX
/// overlay), the credit is applied as a flat per-kWh reduction on the allocated amount.
/// For interval-resolved sharing under §42c, use `DynamicElectricityProvider` instead.
///
/// ## Integration
///
/// ```text
/// ElectricityProvider → full grid consumption (Arbeitspreis + Grundpreis + Stromsteuer)
/// EnergyShareProvider → credit for sharing allocation (negative net_eur)
/// MwStProvider        → MwSt on netto sum (sharing credit reduces the MwSt base)
/// ```
pub struct EnergyShareProvider {
    product: SharingProduct,
}

impl EnergyShareProvider {
    pub fn new(product: SharingProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for EnergyShareProvider {
    fn bill(
        &self,
        _ctx: &crate::context::BillingContext,
        quantities: &crate::quantities::Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let product = &self.product;
        let mut positions: Vec<BillingPosition> = Vec::new();

        // Sharing credit rate from tariff sheet.
        let credit_rate_ct = product.sharing_credit_ct_per_kwh.unwrap_or(Decimal::ZERO);
        if credit_rate_ct.is_zero() {
            return Ok(positions);
        }
        let credit_rate_eur = credit_rate_ct / dec!(100);

        // Allocated kWh from quantities.
        let allocated_kwh = quantities
            .energy_share
            .as_ref()
            .map(|s| s.allocated_kwh)
            .unwrap_or(Decimal::ZERO);
        if allocated_kwh <= Decimal::ZERO {
            return Ok(positions);
        }

        let description = product
            .sharing_description
            .clone()
            .unwrap_or_else(|| "Energiegemeinschaft Gutschrift (§42c EnWG)".to_owned());

        let mut pos = BillingPosition::credit(
            description,
            allocated_kwh,
            "kWh",
            credit_rate_eur,
            PositionCategory::EnergyShare,
        )
        .with_legal_basis("§42c EnWG")
        .with_tag("sharing")
        .with_tag("strom");
        pos.trace = crate::position::PositionTrace::commodity(
            allocated_kwh,
            "kWh",
            -credit_rate_eur,
            "§42c EnWG",
        )
        .with_basis("Energiegemeinschaft-Liefervertrag");
        positions.push(pos);

        // The three transparency terms the input carries. A participant cannot
        // check an allocation without the total it came out of and the fraction
        // it was taken at — the two figures whose product is the credited
        // quantity — so all three reach the invoice, not `allocated_kwh` alone.
        let share = quantities.energy_share.as_ref();
        if let Some(id) = share.and_then(|s| s.gemeinschaft_id.as_deref()) {
            positions.push(info_position(
                format!("Energiegemeinschaft: {id}"),
                "§42c EnWG",
                &["sharing", "gemeinschaft"],
            ));
        }
        if let (Some(total), Some(fraction)) = (
            share.and_then(|s| s.total_plant_generation_kwh),
            share.and_then(|s| s.allocation_fraction),
        ) {
            positions.push(info_position(
                format!(
                    "Zuteilung: {:.1}\u{202f}% von {total:.3}\u{202f}kWh                      Gemeinschaftserzeugung = {allocated_kwh:.3}\u{202f}kWh",
                    fraction * dec!(100)
                ),
                "§42c EnWG",
                &["sharing", "zuteilung"],
            ));
        }

        Ok(positions)
    }
}
