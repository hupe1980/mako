//! Concrete `BillingProvider` implementations for all product types.
//!
//! Each provider corresponds to one product category. Build providers from
//! a `TariffInput` (the product definition from `tarifbd`) and register them
//! with `BillingEngine`.

use billing::{Currency, DynamicPricing, TariffBand, TariffSchedule, TimeOfUsePricing, TouBand};
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
    ControllableLoadProduct, EegProduct, EinspeisungProduct, ElectricityProduct, EmobilityProduct,
    GasProduct, HeatProduct, HemsProduct, ServiceProduct, SharingProduct, SolarProduct,
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

        // An estimated reading is billable (§17 Abs. 1 MessZV), but the caller
        // must know it happened: the customer can demand a corrected invoice
        // once a real reading arrives, so dispatch systems treat it differently.
        // This was an Info *position* only — visible on paper, invisible to code.
        if meter.is_some_and(|m| m.is_estimated) {
            w.push(BillingWarning {
                code: "ESTIMATED_READING",
                severity: WarningSeverity::Warning,
                message: "billed on an estimated reading (§17 Abs. 1 MessZV) — \
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
        w
    }

    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.electricity.as_ref().cloned().unwrap_or_default();
        let kwh = meter.arbeitsmenge_kwh;
        let days = ctx.days();
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
        // Self-consumed electricity is Stromsteuer-exempt (§9a Nr. 1 StromStG)
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
        if kwh > Decimal::ZERO {
            if let Some(tiers) = product.block_tiers.as_ref().filter(|t| !t.is_empty()) {
                // Delegate to billing::TariffSchedule for correct graduated pricing.
                // Replaces manual tier iteration — gains contiguous-band validation
                // and exact Amount<5> arithmetic. Legal basis: §41 EnWG.
                positions.extend(build_block_tariff_positions(tiers, kwh, &[])?);
            } else if let (Some(ht), Some(nt)) =
                (meter.arbeitsmenge_ht_kwh, meter.arbeitsmenge_nt_kwh)
            {
                // Zweitarif (HT/NT) — billing::TimeOfUsePricing for validated band arithmetic.
                // Negative quantities return Err; zero quantities are skipped silently.
                let mut bands = Vec::new();
                if let Some(ap_ht) = product.arbeitspreis_ht_ct_per_kwh {
                    let price = billing::Amount::<5>::try_from((ap_ht / dec!(100)).round_dp(5))
                        .map_err(|_| EngineError::PriceOutOfRange {
                            field: "arbeitspreis_ht_ct_per_kwh".to_owned(),
                            value: ap_ht,
                        })?;
                    bands.push(TouBand::new("HT", price));
                }
                if let Some(ap_nt) = product.arbeitspreis_nt_ct_per_kwh {
                    let price = billing::Amount::<5>::try_from((ap_nt / dec!(100)).round_dp(5))
                        .map_err(|_| EngineError::PriceOutOfRange {
                            field: "arbeitspreis_nt_ct_per_kwh".to_owned(),
                            value: ap_nt,
                        })?;
                    bands.push(TouBand::new("NT", price));
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
            } else if let Some(idx) = &product.indexed_price {
                // ── Indexed price (B2B, §41 Abs. 3 EnWG) ──────────────────────
                // Effective price = base + spread + index_value × factor.
                // When index_value is not available, no arbeitspreis position is added.
                if let Some(effective_ct) = idx.effective_ct_per_kwh() {
                    positions.push(
                        arbeitspreis_position(
                            idx.position_description(),
                            kwh,
                            effective_ct,
                            "kWh",
                            "§41 Abs. 3 EnWG",
                            &["strom", "indexed"],
                        )
                        .with_tag("strom")
                        .with_tag("indexed_price"),
                    );
                }
            }
        }

        // ── EEG-Gutschrift pass-through ────────────────────────────────────────
        if let Some(eeg_ct) = quantities.eeg_gutschrift_eur
            && eeg_ct != Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "EEG-Gutschrift (Photovoltaik)",
                    Decimal::ONE,
                    "EUR",
                    eeg_ct.abs(),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§38 EEG 2023")
                .with_tag("eeg_gutschrift")
                .with_tag("solar"),
            );
        }

        // ── Grid charges (NNE / KA) ────────────────────────────────────────────
        let kwh_for_grid = kwh;
        if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
            let daily = nne_gp / dec!(365);
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
            let months_frac = Decimal::from(days) / dec!(30.4375);
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Leistungspreis",
                    kw,
                    "kW",
                    nne_lp / dec!(12) * months_frac,
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
        // Billed on Spitzenleistung (peak demand, kW) for the billing period.
        // No pro-rating: the peak demand represents the contracted capacity
        // for the full period (§41 EnWG, standard C&I supply contracts).
        if let (Some(lp_ct_per_kw_month), Some(kw)) = (
            product.leistungspreis_strom_ct_per_kw_month,
            meter.spitzenleistung_kw.filter(|kw| *kw > Decimal::ZERO),
        ) {
            positions.push(
                BillingPosition::debit(
                    "Leistungspreis",
                    kw,
                    "kW",
                    lp_ct_per_kw_month / dec!(100),
                    PositionCategory::Commodity,
                )
                .with_legal_basis("§41 EnWG")
                .with_tag("leistungspreis")
                .with_tag("rlm"),
            );
        }

        // ── Stromsteuer ────────────────────────────────────────────────────────
        let st_rate = rates.effective_stromsteuer(product.stromsteuer_ct_per_kwh_override);
        // Resolve effective exemption: typed enum takes priority; boolean flag is legacy.
        let effective_befreiung = if product.stromsteuer_befreiung.is_exempt() {
            product.stromsteuer_befreiung
        } else if product.industrie_stromsteuer_befreiung {
            crate::tariff::StromsteuerBefreiung::IndustrieProduktionesGewerbe
        } else {
            crate::tariff::StromsteuerBefreiung::Keine
        };
        if effective_befreiung.is_exempt() {
            if kwh > Decimal::ZERO {
                positions.push(BillingPosition {
                    description: effective_befreiung.description().to_owned(),
                    legal_basis: Some(effective_befreiung.citation().to_owned()),
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
                        effective_befreiung.citation(),
                    ),
                });
            }
        } else if st_rate > Decimal::ZERO && kwh > Decimal::ZERO {
            positions.push(
                levy_position(
                    "Stromsteuer",
                    kwh,
                    "kWh",
                    st_rate,
                    "§3 StromStG",
                    "stromsteuer",
                )
                .with_tag("strom"),
            );
        }

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
            let months_frac = Decimal::from(days) / dec!(30.4375);
            let eur = crate::position::validated_eur(aa_month * months_frac);
            let (label, cat) = if aa_month < Decimal::ZERO {
                (
                    "Rabatt (monatlicher Festbetrag)",
                    PositionCategory::Discount,
                )
            } else {
                ("Aufschlag (monatlicher Festbetrag)", PositionCategory::Levy)
            };
            // Use debit to preserve sign (negative aa_month → negative net_eur)
            positions.push(BillingPosition {
                description: label.to_owned(),
                legal_basis: None,
                quantity: months_frac,
                unit: "Monat".to_owned(),
                unit_price_eur: aa_month,
                net_eur: eur
                    * if aa_month < Decimal::ZERO {
                        Decimal::NEGATIVE_ONE
                    } else {
                        Decimal::ONE
                    },
                category: cat,
                tags: vec!["auf_abschlag".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
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

        // ── Zählerstand info positions (§41 EnWG) ──────────────────────────────
        if meter.zaehlerstand_von.is_some() || meter.zaehlerstand_bis.is_some() {
            let label = format!(
                "Zählerstand: {} – {}",
                meter
                    .zaehlerstand_von
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                meter
                    .zaehlerstand_bis
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
            );
            let zid = meter
                .zaehlernummer
                .as_deref()
                .or(ctx.zaehler_id.as_deref())
                .unwrap_or("-");
            positions.push(BillingPosition {
                description: label,
                legal_basis: Some("§41 EnWG".to_owned()),
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

        // ── §41 EnWG Abs. 1 Nr. 3 — Verbrauchshistorie (consumption comparison) ──
        // Mandatory invoice display requirement: show prior-year and average.
        // These are informational positions (EUR 0) — they appear in the invoice
        // printout but do not affect the calculation.
        if let Some(vh) = &ctx.verbrauchshistorie {
            if let Some(vj_kwh) = vh.vorjahr_kwh {
                positions.push(BillingPosition {
                    description: format!("Verbrauch Vorjahreszeitraum: {vj_kwh:.0} kWh"),
                    legal_basis: Some("§41 Abs. 1 Nr. 3a EnWG".to_owned()),
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
                    legal_basis: Some("§41 Abs. 1 Nr. 3b EnWG".to_owned()),
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

        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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

        // ── §17 Abs. 1 MessZV — Estimated reading notice ──────────────────────
        if meter.is_estimated {
            positions.push(BillingPosition {
                description: "Abrechnungswert: Schätzung gemäß §17 Abs. 1 MessZV                               — Bestätigung innerhalb 8 Wochen"
                    .to_owned(),
                legal_basis: Some("§17 Abs. 1 MessZV".to_owned()),
                quantity: Decimal::ZERO,
                unit: String::new(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["schatzwert".to_owned(), "messZV".to_owned()],
                applicable_tax_rate: None,
                trace: crate::position::PositionTrace::default(),
            });
        }

        // ── Zählerwechsel notice ───────────────────────────────────────────────
        if meter.zaehler_replaced {
            positions.push(BillingPosition {
                description: "Zählerwechsel innerhalb des Abrechnungszeitraums".to_owned(),
                legal_basis: Some("§41 EnWG".to_owned()),
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
        let days = ctx.days();
        let mut positions: Vec<BillingPosition> = Vec::new();
        let grid_kwh = prosumer.grid_consumption_kwh;
        let self_kwh = prosumer.self_consumption_kwh;

        // Grundpreis on the full billing period (independent of consumption split)
        if let Some(gp_ct_day) = product.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis",
                    gp_ct_day / dec!(100),
                    days,
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
            // Stromsteuer on grid consumption only (§9a Nr. 1 StromStG: self-consumption exempt)
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
            let self_supply_pct = (prosumer.self_supply_ratio() * dec!(100)).round_dp(1);
            positions.push(BillingPosition {
                description: format!(
                    "Eigenverbrauch PV: {self_kwh:.3}\u{202f}kWh (Selbstversorgungsgrad {self_supply_pct:.1}\u{202f}%)",
                ),
                legal_basis: Some("\u{a7}9a Nr. 1 StromStG (Stromsteuerfreiheit Eigenverbrauch \u{2264}30\u{202f}kWp)".to_owned()),
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
/// appends §14a Steuerungsrabatt (Modul 1 NNE reduction + Modul 3 Entschädigung)
/// credit positions.
///
/// ## Legal basis
///
/// §14a Abs. 1 EnWG (BK6-22-024 §2.13): DSOs must offer controllable load
/// (Steuerbare Verbrauchseinrichtungen) customers a reduced NNE (Modul 1 or 3).
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

        // The Modul 2 bands *replace* the flat NNE Arbeitspreis. Both at once
        // bill the device's network usage twice.
        if self.product.sect14a_modul2_nne_ht_ct_per_kwh.is_some()
            && self.grid.nne_arbeitspreis_ct_per_kwh.is_some()
        {
            w.push(BillingWarning {
                code: "MODUL2_AND_FLAT_NNE",
                severity: WarningSeverity::Error,
                message: "§14a Modul 2 band rates are set alongside a flat NNE \
                          Arbeitspreis — the bands replace it; billing both charges \
                          the network usage twice"
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
        let days = ctx.days();
        let p = &self.product;

        // ── §14a Modul 2 — zeitvariables Netzentgelt (BK6-22-300 Anlage 2 §2) ─
        // Three Tarifstufen replace the flat NNE Arbeitspreis for the device.
        // A zero band still produces a position: a rate band silently omitted
        // from the invoice is indistinguishable from one that was never priced.
        if let (Some(ht), Some(st), Some(nt)) = (
            p.sect14a_modul2_nne_ht_ct_per_kwh,
            p.sect14a_modul2_nne_st_ct_per_kwh,
            p.sect14a_modul2_nne_nt_ct_per_kwh,
        ) {
            let verbrauch = quantities.sect14a_modul2.unwrap_or_default();
            for (label, band_kwh, rate_ct) in [
                ("Netzentgelt §14a Modul 2 HT", verbrauch.ht_kwh, ht),
                ("Netzentgelt §14a Modul 2 ST", verbrauch.st_kwh, st),
                ("Netzentgelt §14a Modul 2 NT", verbrauch.nt_kwh, nt),
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
                        .with_tag("modul2")
                        .with_tag("nne"),
                );
            }
        }

        // Modul 1 per-kWh NNE reduction (ct/kWh)
        if let Some(sect14a_m1_ct) = p.sect14a_modul1_nne_reduktion_ct_per_kwh
            && sect14a_m1_ct > Decimal::ZERO
            && kwh > Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "§14a EnWG Modul 1 — NNE Reduktion",
                    kwh,
                    "kWh",
                    sect14a_m1_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§14a EnWG")
                .with_tag("§14a")
                .with_tag("sect14a_modul1"),
            );
        }

        // Modul 1 annual capacity-based NNE reduction (EUR/kW/year)
        if let (Some(m1_year), Some(kw)) = (
            p.steuerungsrabatt_modul1_eur_per_kw_year,
            meter.spitzenleistung_kw,
        ) && m1_year > Decimal::ZERO
            && kw > Decimal::ZERO
        {
            let months_frac = Decimal::from(days) / dec!(30.4375);
            positions.push(
                BillingPosition::credit(
                    "§14a EnWG Modul 1 — Steuerungsrabatt NNE",
                    kw,
                    "kW",
                    (m1_year / dec!(12)) * months_frac,
                    PositionCategory::Credit,
                )
                .with_legal_basis("§14a EnWG")
                .with_tag("§14a")
                .with_tag("sect14a_modul1"),
            );
        }

        // Modul 3 annual capacity × steuerung hours (EUR/kW/year rate)
        if let (Some(m3_year), Some(kw), Some(steuerung_h)) = (
            p.steuerungsrabatt_modul3_eur_per_kw_year,
            meter.spitzenleistung_kw,
            meter.steuerung_stunden,
        ) && m3_year > Decimal::ZERO
            && kw > Decimal::ZERO
            && steuerung_h > Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "§14a EnWG Modul 3 — Steuerungsentschädigung",
                    kw,
                    "kW",
                    m3_year * (steuerung_h / dec!(8760)),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§14a EnWG")
                .with_tag("§14a")
                .with_tag("sect14a_modul3"),
            );
        }

        // Modul 3 per-kWh steuerung compensation
        if let (Some(modul3_ct), Some(steuerung_h)) = (
            p.sect14a_modul3_entschaedigung_ct_per_kwh,
            meter.steuerung_stunden,
        ) {
            let kw = meter.spitzenleistung_kw.unwrap_or(Decimal::ZERO);
            if modul3_ct > Decimal::ZERO && steuerung_h > Decimal::ZERO && kw > Decimal::ZERO {
                let steuerung_kwh = kw * steuerung_h;
                positions.push(
                    BillingPosition::credit(
                        "§14a EnWG Modul 3 — Steuerungsentschädigung",
                        steuerung_kwh,
                        "kWh",
                        modul3_ct / dec!(100),
                        PositionCategory::Credit,
                    )
                    .with_legal_basis("§14a EnWG")
                    .with_tag("§14a")
                    .with_tag("sect14a_modul3"),
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
            (meter.messung_qm3 * hs * z).round_dp(3)
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

        // ── Arbeitspreis ───────────────────────────────────────────────────────
        if kwh_hs > Decimal::ZERO {
            // Resolve effective gas price: gas_indexed_price > seasonal > direct.
            // gas_indexed_price (gas-specific TTF/NCG index) takes priority.
            // Falls back to legacy indexed_price for backward compat.
            let active_indexed = product
                .gas_indexed_price
                .as_ref()
                .or(product.gas_indexed_price.as_ref());
            let gas_ap_ct = if let Some(idx) = active_indexed {
                // Gas indexed price (TTF/NCG-linked, §41 Abs. 3 EnWG)
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
                        "§41 Abs. 3 EnWG",
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
            if let (Some(lp_ct_per_kw_month), Some(kw)) = (
                product.gas_leistungspreis_ct_per_kw_month,
                meter.spitzenleistung_kw.filter(|kw| *kw > Decimal::ZERO),
            ) {
                positions.push(
                    BillingPosition::debit(
                        "Leistungspreis Gas",
                        kw,
                        "kW",
                        lp_ct_per_kw_month / dec!(100),
                        PositionCategory::Commodity,
                    )
                    .with_legal_basis("§41 EnWG")
                    .with_tag("gas_leistungspreis")
                    .with_tag("gas")
                    .with_tag("rlm"),
                );
            }

            // ── Gas NNE ────────────────────────────────────────────────────────
            if let Some(nne_gp) = grid.gas_nne_grundpreis_eur_per_year {
                let daily = nne_gp / dec!(365);
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
            let est_rate =
                rates.effective_energiesteuer_gas(product.energiesteuer_gas_ct_per_kwh_override);
            if product.gas_energiesteuer_befreiung {
                // §54 EnergieStG — KWK / industrial exemption.
                // Plant operator holds formal exemption certificate (Bestimmungserklärung).
                // Operator must verify the certificate is current before enabling this flag.
                positions.push(BillingPosition {
                    description: "Energiesteuer Erdgas: befreit gemäß §54 EnergieStG".to_owned(),
                    legal_basis: Some("§54 EnergieStG".to_owned()),
                    quantity: kwh_hs,
                    unit: "kWh_Hs".to_owned(),
                    unit_price_eur: Decimal::ZERO,
                    net_eur: Decimal::ZERO,
                    category: PositionCategory::Info,
                    tags: vec!["energiesteuer_gas_befreiung".to_owned(), "gas".to_owned()],
                    applicable_tax_rate: None,
                    trace: crate::position::PositionTrace::default(),
                });
            } else if est_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "Energiesteuer Erdgas",
                        kwh_hs,
                        "kWh_Hs",
                        est_rate,
                        "§2 Nr. 3 EnergieStG",
                        "energiesteuer_gas",
                    )
                    .with_tag("gas"),
                );
            }

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
            }
        }

        // ── AufAbschlag / Rabatt (Gas) ─────────────────────────────────────────
        if let Some(aa_ct) = product
            .auf_abschlag_ct_per_kwh
            .filter(|v| *v != Decimal::ZERO)
        {
            let kwh_total = meter.kwh_hs.unwrap_or_else(|| {
                let bw = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.0));
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
            let days = ctx.days();
            let months_frac = Decimal::from(days) / dec!(30.4375);
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

        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let meter = quantities.heat.as_ref().cloned().unwrap_or_default();
        let days = ctx.days();
        let product = &self.product;
        let mut positions: Vec<BillingPosition> = Vec::new();
        let months = meter.months.unwrap_or(dec!(1));

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
            // Use meter.months directly when provided (more accurate than day-based proration)
            let billing_months = meter
                .months
                .unwrap_or_else(|| Decimal::from(days) / dec!(30.4375));
            positions.push(
                BillingPosition::debit(
                    "Leistungspreis Fernwärme",
                    kw,
                    "kW",
                    lp / dec!(12) * billing_months,
                    PositionCategory::Commodity,
                )
                .with_tag("commodity")
                .with_tag("waerme"),
            );
        }
        if let Some(ap_ct) = product.waerme_arbeitspreis_ct_per_kwh
            && meter.kwh_waerme > Decimal::ZERO
        {
            positions.push(
                arbeitspreis_position(
                    "Arbeitspreis Fernwärme",
                    meter.kwh_waerme,
                    ap_ct,
                    "kWh_th",
                    "§41 EnWG",
                    &["waerme"],
                )
                .with_tag("waerme"),
            );
        }
        // ── Auto-7% MwSt for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG) ──────
        // waerme_is_renewable = true → automatic 7% tax rate on heat positions.
        // mwst_rate_override still wins if explicitly set (for edge-case overrides).
        let heat_tax_rate = if product.mwst_rate_override.is_some() {
            product.mwst_rate_override
        } else if product.waerme_is_renewable {
            Some(dec!(0.07))
        } else {
            None
        };
        if let Some(rate) = heat_tax_rate {
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

// ── SolarProvider ─────────────────────────────────────────────────────────────

/// SOLAR (Eigenverbrauch / Mieterstrom §21 Abs. 3 / §42a GGV) billing provider.
pub struct SolarProvider {
    product: SolarProduct,
}

impl SolarProvider {
    pub fn new(product: SolarProduct) -> Self {
        Self { product }
    }
}

impl BillingProvider for SolarProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, EngineError> {
        let product = &self.product;
        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── §42b EEG 2023 (Solarpaket I) GGV hybrid billing ──────────────────
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
                            "\u{a7}42b EEG 2023",
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
                            "GGV-Rabatt Solarstrom (\u{a7}42b EEG 2023)",
                            pv_kwh,
                            "kWh",
                            rabatt_ct / dec!(100),
                            PositionCategory::Discount,
                        )
                        .with_legal_basis("\u{a7}42b EEG 2023 Abs.\u{202f}3")
                        .with_tag("gemeinschaft_rabatt")
                        .with_tag("solar")
                        .with_tag("ggv_pv"),
                    );
                }
                // Stromsteuer on PV portion (only when solar_include_stromsteuer)
                if product.solar_include_stromsteuer {
                    let st_rate = ctx.regulatory_rates.effective_stromsteuer(None);
                    if st_rate > Decimal::ZERO {
                        positions.push(
                            levy_position(
                                "Stromsteuer (GGV PV-Anteil)",
                                pv_kwh,
                                "kWh",
                                st_rate,
                                "\u{a7}3 StromStG",
                                "stromsteuer",
                            )
                            .with_tag("solar")
                            .with_tag("ggv_pv"),
                        );
                    }
                }
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
            let ratio_pct = (ggv.pv_coverage_ratio() * dec!(100)).round_dp(1);
            positions.push(BillingPosition {
                description: format!(
                    "GGV Solarstromanteil: {ratio_pct}\u{202f}% ({pv_kwh:.3}\u{202f}kWh von {:.3}\u{202f}kWh)",
                    ggv.actual_consumption_kwh
                ),
                legal_basis: Some("\u{a7}42b EEG 2023 (Solarpaket I)".to_owned()),
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
                    "\u{a7}42b EEG 2023",
                    &["solar"],
                )
                .with_tag("solar"),
            );
        }
        if let Some(ms_ct) = product.mieterstrom_aufschlag_ct_per_kwh {
            positions.push(
                arbeitspreis_position(
                    "Mieterstrom-Aufschlag (\u{a7}21 Abs. 3 EEG 2023)",
                    kwh,
                    ms_ct,
                    "kWh",
                    "\u{a7}21 Abs. 3 EEG 2023",
                    &["solar", "mieterstrom"],
                )
                .with_tag("mieterstrom_aufschlag"),
            );
        }
        if let Some(rabatt_ct) = product.gemeinschaft_rabatt_ct_per_kwh {
            positions.push(
                BillingPosition::credit(
                    "Rabatt Gemeinschaftliche Geb\u{e4}udeversorgung (\u{a7}42b EEG)",
                    kwh,
                    "kWh",
                    rabatt_ct / dec!(100),
                    PositionCategory::Discount,
                )
                .with_legal_basis("\u{a7}42b EEG 2023")
                .with_tag("gemeinschaft_rabatt")
                .with_tag("solar"),
            );
        }
        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
        if let Some(mp_ct) = product.eeg_marktpraemie_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "EEG Marktprämie",
                    kwh,
                    "kWh",
                    mp_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 EEG 2023")
                .with_tag("eeg_marktpraemie")
                .with_tag("eeg"),
            );
        }
        if let Some(mgp_ct) = product.eeg_managementpraemie_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Managementprämie Direktvermarktung",
                    kwh,
                    "kWh",
                    mgp_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 Abs. 3 EEG 2023")
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
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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

/// §41a EnWG dynamic electricity tariff — per-interval EPEX Spot pricing.
///
/// Accepts any `SpotPriceSource` implementation, not just EPEX.
/// Also includes NNE and Stromsteuer positions.
pub struct DynamicElectricityProvider {
    product: ElectricityProduct,
    grid: GridInput,
    spot_price_source: Box<dyn crate::provider::SpotPriceSource>,
}

impl DynamicElectricityProvider {
    pub fn new(
        product: ElectricityProduct,
        grid: GridInput,
        spot_source: impl crate::provider::SpotPriceSource + 'static,
    ) -> Self {
        Self {
            product,
            grid,
            spot_price_source: Box::new(spot_source),
        }
    }

    pub fn with_epex_map(
        product: ElectricityProduct,
        grid: GridInput,
        epex_prices: std::collections::HashMap<(i32, u8, u8, u8), Decimal>,
    ) -> Self {
        Self::new(
            product,
            grid,
            crate::provider::EpexSpotSource {
                prices: epex_prices,
            },
        )
    }
}

impl BillingProvider for DynamicElectricityProvider {
    fn validate_warnings(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
    ) -> Vec<BillingWarning> {
        // §41b Abs. 2 EnWG — dynamic tariffs require iMSys (Smart Meter Gateway).
        // If the metering mode is explicitly set to SLP or RLM, this is a definite
        // regulatory violation that must block the billing run.
        let is_non_imsys = quantities
            .electricity
            .as_ref()
            .is_some_and(|m| m.metering_mode != crate::quantities::MeteringMode::Imsys);
        if is_non_imsys {
            return vec![BillingWarning {
                code: "SECT41B_IMSYS_REQUIRED",
                severity: WarningSeverity::Error,
                message: "§41b Abs. 2 EnWG: dynamic tariffs (§41a) require an intelligent \
                     metering system (iMSys / Smart Meter Gateway). The meter point \
                     has MeteringMode::Slp or MeteringMode::Rlm. Update metering mode \
                     to MeteringMode::Imsys or switch the customer to a fixed-price product."
                    .to_owned(),
            }];
        }
        vec![]
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
        let days = ctx.days();
        let floor_ct = product.dynamic_epex_floor_ct_kwh;
        let source_name = self.spot_price_source.source_name().to_owned();
        let mut positions: Vec<BillingPosition> = Vec::new();

        // Grundpreis
        if let Some(gp_ct_day) = product.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis Strom (§41a)",
                    gp_ct_day / dec!(100),
                    days,
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
        let mut priced_pairs: Vec<(Decimal, billing::Amount<5>)> =
            Vec::with_capacity(quantities.dynamic_intervals.len());

        for interval in &quantities.dynamic_intervals {
            let price_ct = self
                .spot_price_source
                .price_ct_kwh(interval.timestamp_utc)
                .or_else(|| {
                    if quantities.dynamic_epex_prices.is_empty() {
                        return None;
                    }
                    use time_tz::{OffsetDateTimeExt, timezones};
                    let berlin = timezones::db::europe::BERLIN;
                    let local = interval.timestamp_utc.to_timezone(berlin);
                    let key = (local.year(), local.month() as u8, local.day(), local.hour());
                    quantities.dynamic_epex_prices.get(&key).copied()
                });

            let Some(price_ct) = price_ct else {
                missing_price_intervals += 1;
                continue;
            };

            let effective_ct = if let Some(floor) = floor_ct {
                price_ct.max(floor)
            } else {
                price_ct
            };

            // ct/kWh → EUR/kWh as Amount<5>.  round_dp(5) first ensures the
            // Decimal has at most 5 non-zero fractional digits before conversion.
            // EPEX prices are typically 2 dp in ct/kWh → 4 dp after /100, so this
            // never loses precision in practice.
            if let Ok(price_eur) =
                billing::Amount::<5>::try_from((effective_ct / dec!(100)).round_dp(5))
            {
                priced_pairs.push((interval.kwh, price_eur));
            }
        }

        if missing_price_intervals > 0 {
            tracing::warn!(
                missing_intervals = missing_price_intervals,
                total_intervals = quantities.dynamic_intervals.len(),
                "DynamicElectricityProvider: {missing_price_intervals} interval(s) skipped \
                 (no EPEX price data). Set quantities.dynamic_epex_prices from marktd."
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
            let avg_ct = if total_kwh.is_zero() {
                Decimal::ZERO
            } else {
                (total_eur * dec!(100) / total_kwh).round_dp(4)
            };
            positions.push(BillingPosition {
                description: format!("Arbeitspreis {source_name} (∅ {avg_ct:.4} ct/kWh)",),
                legal_basis: Some("§41a EnWG".to_owned()),
                quantity: total_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: avg_ct / dec!(100),
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

            // Stromsteuer
            let st_rate = rates.effective_stromsteuer(product.stromsteuer_ct_per_kwh_override);
            if st_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "Stromsteuer",
                        total_kwh,
                        "kWh",
                        st_rate,
                        "§3 StromStG",
                        "stromsteuer",
                    )
                    .with_tag("strom"),
                );
            }
        }

        // NNE Grundpreis
        if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
            let daily = nne_gp / dec!(365);
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

        // ── Wire per-position applicable_tax_rate from product.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
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
            let effective_rate = p.applicable_tax_rate.unwrap_or(self.rate);
            if effective_rate.is_zero() {
                continue; // zero rate \u2192 no tax position
            }
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
            let mwst_eur = validated_eur(net_base.abs() * rate);
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

/// Build block tariff `BillingPosition`s using [`billing::TariffSchedule`].
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
    let mut builder = TariffSchedule::graduated().unit("kWh");
    let mut prev: Option<Decimal> = None;

    for (idx, tier) in tiers.iter().enumerate() {
        let price_eur =
            billing::Amount::<5>::try_from((tier.preis_ct_per_kwh / dec!(100)).round_dp(5))
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
            (None, Some(upper)) => TariffBand::up_to(upper, price_eur),
            (Some(lower), Some(upper)) => TariffBand::between(lower, upper, price_eur),
            (lower, None) => TariffBand::over(lower.unwrap_or(Decimal::ZERO), price_eur),
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
/// §42c EnWG (Energiegemeinschaften, effective 01.01.2024): participants in a
/// registered Energiegemeinschaft may receive allocated shares of local generation.
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

        Ok(positions)
    }
}
