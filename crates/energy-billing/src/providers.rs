//! Concrete `BillingProvider` implementations for all product types.
//!
//! Each provider corresponds to one product category. Build providers from
//! a `TariffInput` (the product definition from `tarifbd`) and register them
//! with `BillingEngine`.

use billing::{
    BillingError, DynamicPricing, TariffBand, TariffSchedule, TimeOfUsePricing, TouBand,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::context::BillingContext;
use crate::position::{
    BillingPosition, PositionCategory, arbeitspreis_position, grundpreis_position, levy_position,
    validated_eur,
};
use crate::provider::BillingProvider;
use crate::quantities::{GridInput, Quantities};
use crate::tariff::TariffInput;

// ── ElectricityProvider ───────────────────────────────────────────────────────

/// STROM / WAERMEPUMPE / WALLBOX billing provider.
///
/// Produces commodity positions (Grundpreis, Arbeitspreis HT/NT, §14a credits).
/// Does NOT include MwSt — add `MwStProvider` to the engine.
/// Stromsteuer is included as a levy position.
pub struct ElectricityProvider {
    tariff: TariffInput,
    grid: GridInput,
}

impl ElectricityProvider {
    /// Build from a `TariffInput` product definition.
    #[must_use]
    pub fn new(tariff: TariffInput, grid: GridInput) -> Self {
        Self { tariff, grid }
    }

    pub fn from_tariff(tariff: &TariffInput, grid: &GridInput) -> Self {
        Self {
            tariff: tariff.clone(),
            grid: grid.clone(),
        }
    }
}

impl BillingProvider for ElectricityProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.electricity.as_ref().cloned().unwrap_or_default();
        let kwh = meter.arbeitsmenge_kwh;
        let days = ctx.days();
        let tariff = &self.tariff;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;
        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── Resolve seasonal arbeitspreis ──────────────────────────────────────
        // When seasonal_prices is set, the price for the billing month is looked up.
        // Uses ctx.period_from month as the representative month for the period.
        let billing_month = ctx.period_from.month() as u8;
        let seasonal_arbeitspreis = tariff.seasonal_prices.as_ref().and_then(|seasons| {
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
            return self.bill_prosumer(ctx, p, tariff, grid, rates, seasonal_arbeitspreis);
        }

        // ── Grundpreis ─────────────────────────────────────────────────────────
        if let Some(gp_ct_day) = tariff.grundpreis_ct_per_day {
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
            if let Some(tiers) = tariff.block_tiers.as_ref().filter(|t| !t.is_empty()) {
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
                if let Some(ap_ht) = tariff.arbeitspreis_ht_ct_per_kwh {
                    let price = billing::Amount::<5>::try_from((ap_ht / dec!(100)).round_dp(5))
                        .map_err(|_| BillingError::InvalidInput {
                            reason: "HT price out of range".into(),
                        })?;
                    bands.push(TouBand::new("HT", price));
                }
                if let Some(ap_nt) = tariff.arbeitspreis_nt_ct_per_kwh {
                    let price = billing::Amount::<5>::try_from((ap_nt / dec!(100)).round_dp(5))
                        .map_err(|_| BillingError::InvalidInput {
                            reason: "NT price out of range".into(),
                        })?;
                    bands.push(TouBand::new("NT", price));
                }
                if !bands.is_empty() {
                    let items = TimeOfUsePricing::new(bands)
                        .with_unit("kWh")
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
            } else if let Some(ap_ct) = seasonal_arbeitspreis.or(tariff.arbeitspreis_ct_per_kwh) {
                // Use seasonal price when available, otherwise base tariff price.
                let label = if seasonal_arbeitspreis.is_some() {
                    tariff
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
            } else if let Some(idx) = &tariff.indexed_price {
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
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Grundpreis",
                    Decimal::from(days),
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
                .with_legal_basis("KAV")
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
            tariff.leistungspreis_strom_ct_per_kw_month,
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
        let st_rate = rates.effective_stromsteuer(tariff);
        if tariff.industrie_stromsteuer_befreiung {
            // §9 Abs. 1 Nr. 4 StromStG — industrial customer full exemption.
            // Applies to "Unternehmen des produzierenden Gewerbes" (§2 Nr. 4 StromStG)
            // consuming > 2 GWh/year. Operator must verify the exemption certificate.
            positions.push(BillingPosition {
                description: "Stromsteuer: befreit gemäß §9 Abs. 1 Nr. 4 StromStG".to_owned(),
                legal_basis: Some("§9 Abs. 1 Nr. 4 StromStG".to_owned()),
                quantity: kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["stromsteuer_befreiung".to_owned()],
                applicable_tax_rate: None,
            });
        } else if st_rate > Decimal::ZERO && kwh > Decimal::ZERO && tariff.category != "SOLAR" {
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

        // ── §14a EnWG — Steuerbare Verbrauchseinrichtungen ─────────────────────
        // WAERMEPUMPE and WALLBOX: Modul 1 (capacity reduction) + Modul 3 (EV compensation)
        if matches!(tariff.category.as_str(), "WAERMEPUMPE" | "WALLBOX") {
            if let Some(sect14a_m1_ct) = tariff.sect14a_modul1_nne_reduktion_ct_per_kwh
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
            // Annual capacity-based NNE reduction (steuerungsrabatt_modul1_eur_per_kw_year)
            if let (Some(m1_year), Some(kw)) = (
                tariff.steuerungsrabatt_modul1_eur_per_kw_year,
                meter.spitzenleistung_kw,
            ) && m1_year > Decimal::ZERO
                && kw > Decimal::ZERO
            {
                let months_frac = Decimal::from(days) / dec!(30.4375);
                let monthly_per_kw = m1_year / dec!(12);
                positions.push(
                    BillingPosition::credit(
                        "§14a EnWG Modul 1 — Steuerungsrabatt NNE",
                        kw,
                        "kW",
                        monthly_per_kw * months_frac,
                        PositionCategory::Credit,
                    )
                    .with_legal_basis("§14a EnWG")
                    .with_tag("§14a")
                    .with_tag("sect14a_modul1"),
                );
            }
            // Annual capacity-based Modul 3 Entschädigung (steuerungsrabatt_modul3_eur_per_kw_year)
            // Compensation for each kW-hour of remote steuerung
            if let (Some(m3_year), Some(kw), Some(steuerung_h)) = (
                tariff.steuerungsrabatt_modul3_eur_per_kw_year,
                meter.spitzenleistung_kw,
                meter.steuerung_stunden,
            ) && m3_year > Decimal::ZERO
                && kw > Decimal::ZERO
                && steuerung_h > Decimal::ZERO
            {
                // compensation = rate × kW × (steuerung_h / 8760h per year)
                let rate = m3_year * kw * (steuerung_h / dec!(8760));
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
                let _ = rate;
            }
            if let (Some(modul3_ct), Some(steuerung_h)) = (
                tariff.sect14a_modul3_entschaedigung_ct_per_kwh,
                meter.steuerung_stunden,
            ) {
                let kw = meter.spitzenleistung_kw.unwrap_or(dec!(0));
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
        }

        // ── AufAbschlag / Rabatt ───────────────────────────────────────────────
        // Per-unit discount or surcharge applied after all commodity positions.
        // Negative value = customer discount; positive = surcharge.
        if let Some(aa_ct) = tariff
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
        if let Some(aa_month) = tariff
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
            });
        }

        // ── MSB Grundgebühr ────────────────────────────────────────────────────
        // Messstellenbetreiber fee bundled into the retail invoice (MsbG 2016).
        // Itemised separately per §41 EnWG.
        if let Some(msb_ct_day) = tariff.msb_gebuehr_ct_per_day.filter(|v| *v > Decimal::ZERO) {
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
                });
            }
        }

        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
            });
        }

        // ── Preisgarantie notice (§41 Abs. 1 Nr. 4 EnWG) ─────────────────────
        if let Some(pg_bis) = tariff.preisgarantie_bis.filter(|d| *d >= ctx.period_to) {
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
        tariff: &TariffInput,
        grid: &GridInput,
        rates: &crate::rates::RegulatoryRates,
        seasonal_arbeitspreis: Option<Decimal>,
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let days = ctx.days();
        let mut positions: Vec<BillingPosition> = Vec::new();
        let grid_kwh = prosumer.grid_consumption_kwh;
        let self_kwh = prosumer.self_consumption_kwh;

        // Grundpreis on the full billing period (independent of consumption split)
        if let Some(gp_ct_day) = tariff.grundpreis_ct_per_day {
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
            if let Some(ap_ct) = seasonal_arbeitspreis.or(tariff.arbeitspreis_ct_per_kwh) {
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
            let st_rate = rates.effective_stromsteuer(tariff);
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
            });
        }

        // Wire tax rate (same as normal path)
        if let Some(rate) = tariff.mwst_rate_override {
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

// ── GasProvider ───────────────────────────────────────────────────────────────

/// GAS billing provider.
///
/// Includes Brennwertkorrektur info, commodity positions, gas NNE,
/// Energiesteuer and BEHG CO₂ levy. Does NOT include MwSt.
pub struct GasProvider {
    tariff: TariffInput,
    grid: GridInput,
}

impl GasProvider {
    pub fn from_tariff(tariff: &TariffInput, grid: &GridInput) -> Self {
        Self {
            tariff: tariff.clone(),
            grid: grid.clone(),
        }
    }
}

impl BillingProvider for GasProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.gas.as_ref().cloned().unwrap_or_default();
        let days = ctx.days();
        let tariff = &self.tariff;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;

        // ── Seasonal gas price lookup ──────────────────────────────────────────
        let billing_month = ctx.period_from.month() as u8;
        let seasonal_gas_ap = tariff.seasonal_prices.as_ref().and_then(|seasons| {
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
                legal_basis: Some("§24 GasGVV / DVGW G 685".to_owned()),
                quantity: meter.messung_qm3,
                unit: "m³".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["brennwertkorrektur".to_owned(), "info".to_owned()],
                applicable_tax_rate: None,
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
            });
        }

        // ── Grundpreis ─────────────────────────────────────────────────────────
        if let Some(gp_ct_day) = tariff.gas_grundpreis_ct_per_day {
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
            // Resolve effective gas price: indexed > seasonal > direct
            let gas_ap_ct = if let Some(idx) = &tariff.indexed_price {
                // Indexed gas price (TTF/TTF-linked, §41 Abs. 3 EnWG)
                idx.effective_ct_per_kwh()
                    .or(seasonal_gas_ap)
                    .or(tariff.gas_arbeitspreis_ct_per_kwh_hs)
            } else {
                seasonal_gas_ap.or(tariff.gas_arbeitspreis_ct_per_kwh_hs)
            };
            if let Some(ap_ct) = gas_ap_ct {
                let (label, legal_basis) = if tariff.indexed_price.is_some() {
                    (
                        tariff
                            .indexed_price
                            .as_ref()
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
                    let season_label = tariff
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
                        .with_tag(if tariff.indexed_price.is_some() {
                            "indexed_price"
                        } else if seasonal_gas_ap.is_some() {
                            "seasonal"
                        } else {
                            "gas"
                        }),
                );
            }

            // ── Gas NNE ────────────────────────────────────────────────────────
            if let Some(nne_gp) = grid.gas_nne_grundpreis_eur_per_year {
                let daily = nne_gp / dec!(365);
                positions.push(
                    BillingPosition::debit(
                        "Gasnetznutzungsentgelt Grundpreis",
                        Decimal::from(days),
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
                    .with_legal_basis("KAV")
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
                    .with_legal_basis("GasNZV")
                    .with_tag("gas_bilanzierungsumlage")
                    .with_tag("nne"),
                );
            }

            // ── Energiesteuer ──────────────────────────────────────────────────
            let est_rate = rates.effective_energiesteuer_gas(tariff);
            if tariff.gas_energiesteuer_befreiung {
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
            let behg_rate = rates.effective_behg_gas(tariff);
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
        if let Some(aa_ct) = tariff
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
        if let Some(aa_month) = tariff
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
            });
        }

        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    tariff: TariffInput,
}

impl HeatProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for HeatProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.heat.as_ref().cloned().unwrap_or_default();
        let days = ctx.days();
        let tariff = &self.tariff;
        let mut positions: Vec<BillingPosition> = Vec::new();
        let months = meter.months.unwrap_or(dec!(1));

        if let Some(gp) = tariff.waerme_grundpreis_eur_per_month {
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
            tariff.waerme_leistungspreis_eur_per_kw_year.or_else(|| {
                tariff
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
        if let Some(ap_ct) = tariff.waerme_arbeitspreis_ct_per_kwh
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
        let heat_tax_rate = if tariff.mwst_rate_override.is_some() {
            tariff.mwst_rate_override
        } else if tariff.waerme_is_renewable {
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

/// SOLAR (Eigenverbrauch / Mieterstrom §38a / §42a GGV) billing provider.
pub struct SolarProvider {
    tariff: TariffInput,
}

impl SolarProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for SolarProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let tariff = &self.tariff;
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
                if let Some(ap_ct) = tariff.solar_arbeitspreis_ct_per_kwh {
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
                if let Some(rabatt_ct) = tariff.gemeinschaft_rabatt_ct_per_kwh {
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
                if tariff.solar_include_stromsteuer {
                    let st_rate = ctx.regulatory_rates.effective_stromsteuer(tariff);
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
            // Billed at the standard electricity rate (arbeitspreis_ct_per_kwh).
            // Stromsteuer always applies to grid electricity.
            if grid_kwh > Decimal::ZERO {
                if let Some(ap_ct) = tariff.arbeitspreis_ct_per_kwh {
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
                let st_rate = ctx.regulatory_rates.effective_stromsteuer(tariff);
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
            });

            // Wire tax rate for GGV hybrid positions too
            if let Some(rate) = tariff.mwst_rate_override {
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

        if let Some(ap_ct) = tariff.solar_arbeitspreis_ct_per_kwh {
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
        if let Some(ms_ct) = tariff.mieterstrom_aufschlag_ct_per_kwh {
            positions.push(
                arbeitspreis_position(
                    "Mieterstrom-Aufschlag (\u{a7}38a EEG 2023)",
                    kwh,
                    ms_ct,
                    "kWh",
                    "\u{a7}38a EEG 2023",
                    &["solar", "mieterstrom"],
                )
                .with_tag("mieterstrom_aufschlag"),
            );
        }
        if let Some(rabatt_ct) = tariff.gemeinschaft_rabatt_ct_per_kwh {
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
        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    tariff: TariffInput,
}

impl EegProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for EegProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        // ── Preferred path: delegate to eeg-billing for full regulatory accuracy ──
        // Only available when the `eeg` feature is enabled.
        #[cfg(feature = "eeg")]
        if let Some(eeg_full) = &quantities.eeg_full {
            return bill_eeg_full(eeg_full, ctx);
        }

        // ── Fallback: simplified EEG credit note ──────────────────────────────
        let meter = quantities.eeg.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
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
            });
        }
        if let Some(vg_ct) = tariff.eeg_verguetungssatz_ct_per_kwh {
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
        if let Some(mp_ct) = tariff.eeg_marktpraemie_ct_per_kwh {
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
        if let Some(mgp_ct) = tariff.eeg_managementpraemie_ct_per_kwh {
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
        if let Some(kwkg_ct) = tariff.kwkg_zuschlag_ct_per_kwh {
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
        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
) -> Result<Vec<BillingPosition>, BillingError> {
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
        })
        .collect();
    Ok(positions)
}

// ── EinspeisungProvider ───────────────────────────────────────────────────────

/// Non-EEG Direktvermarktung feed-in settlement (EINSPEISUNG).
pub struct EinspeisungProvider {
    tariff: TariffInput,
}

impl EinspeisungProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for EinspeisungProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.einspeisung.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let kwh = meter.einspeisung_kwh;
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(mv_ct) = tariff.marktwert_ct_per_kwh {
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
        if let Some(vm_ct) = tariff.vermarktungsgebuehr_ct_per_kwh {
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
        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    tariff: TariffInput,
}

impl HemsProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for HemsProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let usage = quantities.hems.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        let sub_eur = tariff.hems_subscription_eur_per_month;

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
            tariff.hems_optimization_event_eur,
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
        if let (Some(reads), Some(read_eur)) = (usage.readout_events, tariff.hems_readout_event_eur)
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
        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    tariff: TariffInput,
}

impl EmobilityProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for EmobilityProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let usage = quantities.emobility.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        let svc_eur = tariff.emobility_service_fee_eur;
        let kwh_price = tariff.emobility_kwh_price_ct;

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
            (usage.sessions, tariff.emobility_session_fee_eur)
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
            (usage.roaming_sessions, tariff.emobility_roaming_fee_eur)
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
        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    tariff: TariffInput,
}

impl ServiceProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for ServiceProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let usage = quantities.service.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(fee_eur) = tariff.service_fee_eur {
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
        let event_price = usage.event_price_eur.or(tariff.service_event_price_eur);
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
        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    tariff: TariffInput,
    grid: GridInput,
    spot_price_source: Box<dyn crate::provider::SpotPriceSource>,
}

impl DynamicElectricityProvider {
    pub fn new(
        tariff: TariffInput,
        grid: GridInput,
        spot_source: impl crate::provider::SpotPriceSource + 'static,
    ) -> Self {
        Self {
            tariff,
            grid,
            spot_price_source: Box::new(spot_source),
        }
    }

    pub fn with_epex_map(
        tariff: TariffInput,
        grid: GridInput,
        epex_prices: std::collections::HashMap<(i32, u8, u8, u8), Decimal>,
    ) -> Self {
        Self::new(
            tariff,
            grid,
            crate::provider::EpexSpotSource {
                prices: epex_prices,
            },
        )
    }
}

impl BillingProvider for DynamicElectricityProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let tariff = &self.tariff;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;
        let days = ctx.days();
        let floor_ct = tariff.dynamic_epex_floor_ct_kwh;
        let source_name = self.spot_price_source.source_name().to_owned();
        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── §41a Abs. 4 EnWG — iMSys requirement ──────────────────────────────
        // Dynamic tariffs require an intelligent metering system (Smart Meter
        // Gateway / iMSys). Billing is not blocked — the operator is responsible
        // for verifying metering compliance — but a regulatory notice is appended.
        if quantities
            .electricity
            .as_ref()
            .is_some_and(|m| m.metering_mode != crate::quantities::MeteringMode::Imsys)
        {
            positions.push(BillingPosition {
                description: "Hinweis: §41a Abs. 4 EnWG erfordert iMSys (Smart Meter) \
                     für dynamische Tarife. Bitte Messsystem prüfen."
                    .to_owned(),
                legal_basis: Some("§41a Abs. 4 EnWG".to_owned()),
                quantity: Decimal::ZERO,
                unit: String::new(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["sect41a_imsys_warning".to_owned()],
                applicable_tax_rate: None,
            });
        }

        // Grundpreis
        if let Some(gp_ct_day) = tariff.grundpreis_ct_per_day {
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
            let item = DynamicPricing::from_intervals(priced_pairs)
                .and_then(|dp| dp.with_unit("kWh").calculate())
                .map_err(|e| BillingError::InvalidInput {
                    reason: format!("§41a DynamicPricing: {e}"),
                })?;

            let total_kwh = item.quantity_value().unwrap_or_default();
            let total_eur = item.net_amount.to_decimal();
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
                    .with_legal_basis("KAV")
                    .with_tag("konzessionsabgabe")
                    .with_tag("nne"),
                );
            }

            // Stromsteuer
            let st_rate = rates.effective_stromsteuer(tariff);
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
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Grundpreis",
                    Decimal::from(days),
                    "Tage",
                    daily,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_grundpreis")
                .with_tag("nne"),
            );
        }

        // ── Wire per-position applicable_tax_rate from tariff.mwst_rate_override ──
        // Enables multi-rate MwSt: 7% for renewable Fernwärme (§12 Abs. 2 Nr. 1 UStG),
        // 0% for solar PV ≤30 kWp (§12 Abs. 3 UStG), etc.
        if let Some(rate) = tariff.mwst_rate_override {
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
    ) -> Result<Vec<BillingPosition>, BillingError> {
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
) -> Result<Vec<BillingPosition>, BillingError> {
    let mut builder = TariffSchedule::graduated().unit("kWh");
    let mut prev: Option<Decimal> = None;

    for (idx, tier) in tiers.iter().enumerate() {
        let price_eur =
            billing::Amount::<5>::try_from((tier.preis_ct_per_kwh / dec!(100)).round_dp(5))
                .map_err(|_| BillingError::InvalidInput {
                    reason: format!("block tier {} price out of range", idx + 1),
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
