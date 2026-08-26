//! Meter auto-enrichment (Gas, Strom, dynamic intervals) and NEHS market-price overlay.

use super::*;

// ── Gas meter auto-enrichment ─────────────────────────────────────────────────

/// Normalize a raw `gasqualitaet` string to its BO4E `Gasqualitaet` wire value.
///
/// Delegates to [`mako_geli_gas::gas_quality::normalize_gasqualitaet`] — the one
/// implementation — and returns `None` for anything BO4E does not define.
///
/// One implementation, because the value is annotated onto an invoice: an
/// unrecognised string would be a `ZusatzAttribut` claiming to be a gas
/// quality.
pub(crate) fn normalize_gasqualitaet(raw: &str) -> Option<&'static str> {
    mako_geli_gas::gas_quality::normalize_gasqualitaet(raw)
}

/// Auto-enrich a `GasMeterInput` with data from `edmd` and `marktd`.
///
/// This is the **Gas billing data pipeline** for `billingd`.  It fills in
/// missing fields using the priority order below, without overriding anything
/// the caller already supplied.
///
/// ## Priority order (highest to lowest)
///
/// | Field | 1st source | 2nd source | Fallback |
/// |---|---|---|---|
/// | `kwh_hs` | caller (`req.gas_meter`) | edmd billing-period | `None` — the caller's `messung_qm3` × factors |
/// | `messung_qm3` | caller only | — | `0` (the engine rejects a volume-less bill) |
/// | `brennwert_kwh_per_qm3` | caller | edmd billing-period | edmd **gas-quality** (PID 13007) → `None` (engine default 10.55) |
/// | `zustandszahl` | caller | edmd billing-period | edmd gas-quality (PID 13007) → `None` (engine default 1.0) |
/// | `spitzenleistung_kw` | caller | edmd billing-period | `None` (no RLM demand charge) |
/// | `zaehlerstand_von/-bis` | caller | edmd billing-period | `None` (§40 Abs. 2 Nr. 6 reading omitted) |
/// | `gasqualitaet` | caller | marktd MaLo fields | `None` (no audit annotation) |
///
/// `edmd` reports gas as energy (`arbeitsmenge_kwh` = kWh_Hs, the DSO's MSCONS
/// conversion already applied) and never as a raw volume, which is why nothing
/// here can fill `messung_qm3` — the m³ path exists for callers holding a
/// customer-read meter value.
///
/// ## Failure handling
///
/// The **annotations** (gas quality, conversion factors) are best-effort: a
/// failure is logged and billing proceeds on the engine's DVGW defaults.
///
/// The **quantity** is not. When the caller supplied neither `kwh_hs` nor a
/// volume, an `edmd` failure leaves both at zero and the invoice comes back
/// carrying the Grundpreis and nothing for the gas — no Arbeitspreis, no
/// Energiesteuer, no BEHG — and looks entirely ordinary. That is the gas twin
/// of the §41a `SECT41A_NO_LASTGANG` defect, so it is returned as an error
/// (`502` for an outage, `422` when edmd simply has no period) rather than
/// billed. The electricity path has always done this
/// ([`resolve_strom_meter`]); gas was the one door left open.
///
/// ## DVGW G 685 / §25 Nr. 4 MessEV compliance
///
/// `brennwert_kwh_per_qm3` × `zustandszahl` converts m³ → kWh_Hs.  The
/// energy-billing engine applies DVGW defaults when both are absent:
/// - brennwert: 10.55 kWh/m³ (German-average H-Gas per DVGW G 685 §5.3)
/// - zustandszahl: 1.0 (pressure/temperature ≈ reference conditions)
///
/// To suppress the engine default and ensure the DSO-published values are
/// always used, operators should verify that MSCONS PID 13007 data is flowing
/// into `edmd` before running billing.
pub(crate) async fn enrich_gas_meter(
    meter: &mut GasMeterInput,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
) -> BillingResult<()> {
    use crate::clients::{GasBillingPeriod, GasQualityRecord};

    // Track which fields were enriched for structured logging.
    let mut enriched_from_edmd_period = false;
    let mut enriched_from_edmd_quality = false;
    let mut enriched_gq_from_marktd = false;

    // ── Step 1: Energy + conversion factors from edmd billing period ──────────
    // Fetch only when the caller supplied neither a volume nor an energy value.
    // edmd reports gas energy as kWh_Hs (`arbeitsmenge_kwh`, Brennwert already
    // applied by the DSO's MSCONS data) plus the applied conversion factors and
    // the §40 Abs. 2 Nr. 6 register readings.
    if meter.messung_qm3 == rust_decimal::Decimal::ZERO && meter.kwh_hs.is_none() {
        match edmd
            .get_gas_billing_period(malo_id, period_from, period_to)
            .await
        {
            Ok(Some(GasBillingPeriod {
                kwh_hs,
                brennwert_kwh_per_qm3,
                zustandszahl,
                spitzenleistung_kw,
                zaehlerstand_von,
                zaehlerstand_bis,
                is_estimated,
            })) => {
                meter.kwh_hs = kwh_hs;
                if meter.brennwert_kwh_per_qm3.is_none() {
                    meter.brennwert_kwh_per_qm3 = brennwert_kwh_per_qm3;
                }
                if meter.zustandszahl.is_none() {
                    meter.zustandszahl = zustandszahl;
                }
                if meter.spitzenleistung_kw.is_none() {
                    meter.spitzenleistung_kw = spitzenleistung_kw;
                }
                if meter.zaehlerstand_von.is_none() {
                    meter.zaehlerstand_von = zaehlerstand_von;
                }
                if meter.zaehlerstand_bis.is_none() {
                    meter.zaehlerstand_bis = zaehlerstand_bis;
                }
                meter.is_estimated |= is_estimated;
                enriched_from_edmd_period = true;
            }
            Ok(None) => {
                return Err(BillingError::unprocessable(
                    "NO_METER_DATA",
                    format!(
                        "edmd has no gas billing period for MaLo {malo_id} in \
                         {period_from}..{period_to}, and the request supplied neither \
                         kwh_hs nor a volume — the invoice would bill the Grundpreis and \
                         no gas at all"
                    ),
                ));
            }
            Err(e) => return Err(BillingError::upstream("edmd", e)),
        }
    }

    // ── Step 2: Abrechnungsbrennwert + Zustandszahl from edmd gas-quality ─────
    // MSCONS PID 13007 (Gasbeschaffenheitsdaten) carries the DSO-published
    // monthly Brennwert and Zustandszahl — more precise than the billing-period
    // summary because it covers the exact billing window.
    // Only fetch when at least one conversion factor is still missing.
    if meter.brennwert_kwh_per_qm3.is_none() || meter.zustandszahl.is_none() {
        match edmd.get_gas_quality(malo_id).await {
            Ok(Some(records)) => {
                // Find the record whose period best covers the billing period.
                // "Best" = latest period_from that still starts ≤ billing period end,
                // ensuring we pick the most recent DSO-published Brennwert.
                let best: Option<&GasQualityRecord> = records
                    .iter()
                    .filter(|q| q.period_from <= period_to && q.period_to >= period_from)
                    .max_by_key(|q| q.period_from);

                if let Some(q) = best {
                    if meter.brennwert_kwh_per_qm3.is_none() {
                        meter.brennwert_kwh_per_qm3 = Some(q.brennwert_kwh_per_m3);
                        enriched_from_edmd_quality = true;
                    }
                    if meter.zustandszahl.is_none() {
                        meter.zustandszahl = Some(q.zustandszahl);
                        enriched_from_edmd_quality = true;
                    }
                } else if !records.is_empty() {
                    tracing::debug!(
                        malo_id,
                        period_from = %period_from,
                        period_to   = %period_to,
                        "billingd GAS: edmd gas-quality records exist but none cover billing period"
                    );
                }
            }
            Ok(None) => {
                tracing::debug!(
                    malo_id,
                    "billingd GAS: no gas-quality data in edmd (PID 13007 not yet received)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    malo_id,
                    error = %e,
                    "billingd GAS: edmd gas-quality fetch failed — proceeding without"
                );
            }
        }
    }

    // ── Step 3: gasqualitaet annotation from marktd MaLo ──────────────────────
    // Informational only — billing always uses the measured Brennwert. Annotated
    // on the invoice as a `ZusatzAttribut` for the § 147 AO / GoBD audit trail.
    //
    // A value the BO4E schema does not define is dropped rather than passed
    // through: the annotation claims to be a gas quality, so writing an
    // unrecognised string onto an invoice would be an audit trail that asserts
    // something the standard does not.
    if meter.gasqualitaet.is_none() {
        match marktd.get_malo(malo_id).await {
            Ok(Some(malo_fields)) => {
                if let Some(canonical) = malo_fields
                    .gasqualitaet
                    .as_deref()
                    .and_then(normalize_gasqualitaet)
                {
                    meter.gasqualitaet = Some(canonical.to_owned());
                    enriched_gq_from_marktd = true;
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    malo_id,
                    error = %e,
                    "billingd GAS: marktd get_malo failed — proceeding without gasqualitaet"
                );
            }
        }
    }

    // ── Structured enrichment summary ─────────────────────────────────────────
    // Logged at DEBUG level so billing operators can verify auto-enrichment
    // without flooding production logs.
    if enriched_from_edmd_period || enriched_from_edmd_quality || enriched_gq_from_marktd {
        tracing::debug!(
            malo_id,
            messung_qm3                   = %meter.messung_qm3,
            brennwert_kwh_per_qm3         = ?meter.brennwert_kwh_per_qm3,
            zustandszahl                  = ?meter.zustandszahl,
            spitzenleistung_kw            = ?meter.spitzenleistung_kw,
            gasqualitaet                  = ?meter.gasqualitaet,
            enriched_from_edmd_period,
            enriched_from_edmd_quality,
            enriched_gq_from_marktd,
            "billingd GAS: meter enrichment complete"
        );
    }
    Ok(())
}

/// The metered quantities for the period: the request override, else edmd.
///
/// # Errors
///
/// `422` when edmd has no billing period for the MaLo, `502` when it cannot be
/// reached.
pub(crate) async fn resolve_strom_meter(
    req: &CalculateRequest,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
) -> BillingResult<MeterInput> {
    if let Some(m) = req.meter.clone() {
        return Ok(m);
    }
    match edmd
        .get_billing_period(malo_id, period_from, period_to)
        .await
    {
        Ok(Some(m)) => Ok(m),
        Ok(None) => Err(BillingError::unprocessable(
            "NO_METER_DATA",
            format!("edmd has no billing period for MaLo {malo_id} in {period_from}..{period_to}"),
        )),
        Err(e) => Err(BillingError::upstream("edmd", e)),
    }
}

/// The 15-minute Lastgang a §41a dynamic tariff is priced from.
///
/// A failed fetch must not degrade to an empty Vec: an empty Vec bills a
/// dynamic customer their Grundpreis and nothing else, silently. An outage of
/// edmd is an outage, not a zero-energy month.
///
/// # Errors
///
/// `502` when edmd cannot be reached.
pub(crate) async fn fetch_dynamic_intervals(
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
) -> BillingResult<Vec<DynamicInterval>> {
    edmd.get_lastgang(malo_id, period_from, period_to)
        .await
        .map_err(|e| BillingError::upstream("edmd", format!("Lastgang: {e}")))
}

/// Whether the nEHS market overlay applies: Gas/Wärme categories only, and
/// only when the operator has **not** pinned an explicit `[rates]` BEHG
/// override — an explicit override always wins, so the market fetch is
/// skipped entirely.
pub(crate) fn nehs_overlay_applies(category: &str, cfg: &BillingdConfig) -> bool {
    matches!(category, "GAS" | "WAERME") && cfg.behg_override().is_none()
}

/// Overlay the nEHS market price onto the period rates (CO2KostAufG §3).
///
/// Since 2026 the nEHS certificate price is auction-formed (§10 Abs. 1 BEHG:
/// weekly EEX auctions from 01.07.2026, Verkaufsphase 68 EUR/t), so the Gas
/// CO₂ component follows the supplier's dated acquisition prices in productd's
/// `nehs_prices` series. Resolution: explicit `[rates]` override > market
/// series > year table (fallback inside `regulatory_rates_for_period`).
///
/// The lookup date is the **period start**: a straddling period takes the
/// start-of-period regime, the same basis `regulatory_rates_for_period` uses
/// (`period_from.year()`) for the year-table fallback — so market overlay and
/// fallback cannot diverge on which side of a boundary they bill.
///
/// The EUR/t → ct/kWh conversion uses the configured CO₂ factor
/// (`[rates] behg_co2_factor_kg_per_kwh`, e.g. 0.20140 for an L-Gas
/// deployment); unset means the H-Gas default 0.20160.
pub(crate) async fn apply_nehs_market_price(
    rates: &mut energy_billing::RegulatoryRates,
    category: &str,
    period_from: time::Date,
    cfg: &BillingdConfig,
    productd: &ProductdClient,
) {
    if !nehs_overlay_applies(category, cfg) {
        return;
    }
    match productd.get_latest_nehs_price(period_from).await {
        Ok(Some(eur_per_t)) => {
            rates.behg_gas_ct_per_kwh =
                energy_billing::behg_ct_per_kwh_from_price(eur_per_t, cfg.behg_co2_factor());
        }
        Ok(None) => {} // no series data — year-table fallback stands
        Err(e) => {
            tracing::warn!(error = %e, "billingd: nEHS price fetch failed; using year-table BEHG rate");
        }
    }
}

/// The day-ahead price series a §41a tariff is billed against.
///
/// `productd` owns the imported EPEX series (15-min MTU). The map is keyed on
/// each MTU's UTC start instant, matching how `energy-billing` floors a
/// consumption interval to its quarter-hour.
///
/// # Errors
///
/// `502` when productd cannot be reached. Swallowing the error and returning an
/// empty map made every unreachable-productd invoice fail downstream as
/// `SECT41A_MISSING_EPEX_PRICES`, telling the operator to import prices that
/// were already imported — a 502 reported as a data problem, pointing at the
/// wrong fix.
pub(crate) async fn fetch_epex_prices(
    period_from: time::Date,
    period_to: time::Date,
    productd: &Arc<ProductdClient>,
) -> BillingResult<std::collections::HashMap<time::OffsetDateTime, rust_decimal::Decimal>> {
    productd
        .get_epex_prices(period_from, period_to)
        .await
        .map_err(|e| BillingError::upstream("productd", format!("EPEX Spot: {e}")))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod gas_enrichment_tests {
    use crate::handlers::{build_aggregate_invoice, normalize_gasqualitaet};
    use energy_billing::{
        BillingContext, BillingPeriod, BillingPosition, BillingProvider as _, Invoice, InvoiceType,
        MwStProvider, Quantities, RegulatoryRates,
    };

    // ── normalize_gasqualitaet ────────────────────────────────────────────────

    /// The alias table itself lives in `mako-geli-gas` and is tested there.
    /// What this asserts is the *boundary*: billingd annotates an invoice with
    /// the value, so only a value BO4E defines may come out.
    #[test]
    fn only_bo4e_gas_qualities_reach_an_invoice_annotation() {
        for raw in ["HGas", "H-Gas", "  H_GAS  ", "HIGH_CALORIFIC", "ERDGAS_H"] {
            assert_eq!(normalize_gasqualitaet(raw), Some("H_GAS"), "for {raw:?}");
        }
        for raw in ["LGas", "L-Gas", "\tLGas\n", "LOW_CALORIFIC", "ERDGAS_L"] {
            assert_eq!(normalize_gasqualitaet(raw), Some("L_GAS"), "for {raw:?}");
        }

        // Everything else is dropped, not passed through. The previous version
        // returned an upper-snake-cased copy of whatever it was given, so
        // `"syngas"` became a `ZusatzAttribut` on a real invoice asserting a gas
        // quality no standard defines — as did the speculative `H2_BLEND`,
        // `BIOGAS` and `FLUESSIGGAS` values it claimed as canonical.
        for raw in [
            "syngas",
            "Compressed Natural Gas",
            "H2_BLEND",
            "HYDROGEN_BLEND",
            "BIOGAS",
            "FLUESSIGGAS",
            "LPG",
            "",
        ] {
            assert_eq!(normalize_gasqualitaet(raw), None, "for {raw:?}");
        }
    }

    // ── build_aggregate_invoice ───────────────────────────────────────────────

    fn sub_invoice(malo: &str, netto_ct: rust_decimal::Decimal) -> (String, Invoice) {
        use energy_billing::PositionCategory;
        let ctx = BillingContext {
            malo_id: malo.to_owned(),
            lf_mp_id: "9900000000001".to_owned(),
            rechnungsnummer: format!("SUB-{malo}"),
            period: BillingPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .unwrap(),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: RegulatoryRates::default(),
            ..Default::default()
        };
        let base = vec![BillingPosition::debit(
            "Arbeitspreis".to_owned(),
            rust_decimal::Decimal::ONE,
            "kWh",
            netto_ct,
            PositionCategory::Commodity,
        )];
        let mut all = base.clone();
        all.extend(
            MwStProvider::new(rust_decimal::dec!(0.19))
                .bill(&ctx, &Quantities::default(), &base)
                .unwrap(),
        );
        (malo.to_owned(), Invoice::from_positions(ctx, all, vec![]))
    }

    /// The consolidated document strips the sub-invoices' tax positions and
    /// recomputes VAT once over the combined base per rate — steuerbetraege
    /// and totals agree by construction, not by hoping the parts add up.
    #[test]
    fn aggregate_recomputes_vat_over_the_combined_base() {
        use rust_decimal::dec;
        let parts: Vec<(String, Invoice)> = ["11111111115", "22222222220", "33333333333"]
            .iter()
            .map(|m| sub_invoice(m, dec!(10.01)))
            .collect();
        let (agg, json) = build_aggregate_invoice(
            "RV-1",
            "9900000000001",
            "SAMMEL-RV-1".to_owned(),
            time::macros::date!(2026 - 01 - 01),
            time::macros::date!(2026 - 01 - 31),
            RegulatoryRates::default(),
            parts,
            vec![],
        )
        .unwrap();
        assert_eq!(agg.netto_eur, dec!(30.03));
        // 30.03 × 0.19 = 5.7057, stated to the cent per rate as every
        // Steuerbetrag is — so the total and the BG-23 breakdown are the same
        // number, not two that round apart.
        assert_eq!(agg.mwst_eur, dec!(5.71), "VAT over the combined base");
        assert_eq!(agg.brutto_eur, dec!(35.74));
        // The BG-23 breakdown over the combined base per rate. Had the aggregate
        // summed the per-MaLo breakdowns instead, it would show 3 × 1.90.
        let steuer: rust_decimal::Decimal = json["steuerbetraege"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                s["steuerwert"]
                    .as_str()
                    .map(|v| v.parse::<rust_decimal::Decimal>().unwrap())
                    .unwrap_or_else(|| {
                        rust_decimal::Decimal::try_from(s["steuerwert"].as_f64().unwrap()).unwrap()
                    })
            })
            .sum();
        assert_eq!(steuer, dec!(5.71));
    }

    /// Every emitted position names the MaLo it came from, and the tax is
    /// stated once — in `steuerbetraege`, not as a position.
    ///
    /// A BO4E `Rechnungsposition` is a net supply line: `gesamtnetto` is „Die
    /// Summe der Nettobeträge der Rechnungsteile", so a tax line among the
    /// positions is the same amount stated twice and makes the document's own
    /// totals irreconcilable. That also makes the annotation total: there is no
    /// longer a position with no MaLo to account for.
    #[test]
    fn aggregate_annotates_positions_with_their_malo() {
        use rust_decimal::dec;
        let parts = vec![
            sub_invoice("11111111115", dec!(50)),
            sub_invoice("22222222220", dec!(70)),
        ];
        let (_, json) = build_aggregate_invoice(
            "RV-2",
            "9900000000001",
            "SAMMEL-RV-2".to_owned(),
            time::macros::date!(2026 - 01 - 01),
            time::macros::date!(2026 - 01 - 31),
            RegulatoryRates::default(),
            parts,
            vec![],
        )
        .unwrap();
        let pos = json["rechnungspositionen"].as_array().unwrap();
        assert_eq!(pos[0]["marktlokationsId"], "11111111115");
        assert_eq!(pos[1]["marktlokationsId"], "22222222220");
        assert!(
            !pos.iter().any(|p| p["kategorie"] == "Tax"),
            "tax belongs in steuerbetraege, not among the positions"
        );
        assert!(
            pos.iter().all(|p| p.get("marktlokationsId").is_some()),
            "every emitted position is a supply line, so every one names its MaLo"
        );
        // …and the tax the positions no longer state is still on the document.
        let steuer: rust_decimal::Decimal = json["steuerbetraege"]
            .as_array()
            .expect("steuerbetraege")
            .iter()
            .filter_map(|e| e["steuerwert"].as_str())
            .filter_map(|v| v.parse::<rust_decimal::Decimal>().ok())
            .sum();
        assert_eq!(
            steuer,
            json["gesamtsteuer"]["wert"]
                .as_str()
                .and_then(|v| v.parse::<rust_decimal::Decimal>().ok())
                .expect("gesamtsteuer"),
        );
        // Deterministic rechnungsdatum — no wall clock in the document.
        //
        // BO4E declares `rechnungsdatum` as `format: date-time`, so the wire
        // value is a timestamp, not a bare date. Midnight UTC: the calendar
        // date survives the promotion and any later offset normalisation.
        assert_eq!(json["rechnungsdatum"], "2026-01-31T00:00:00Z");
    }

    /// A blocked engine validation reaches the wire as a 422 whose code and
    /// warnings a caller can act on — not a prose string it has to parse.
    #[test]
    fn a_blocked_validation_is_a_machine_readable_422() {
        let e = energy_billing::EngineError::ValidationBlocked {
            warnings: vec![energy_billing::BillingWarning {
                code: "MODUL3_AND_FLAT_NNE",
                severity: energy_billing::WarningSeverity::Error,
                message: "both configured".to_owned(),
            }],
        };
        let err: crate::error::BillingError = e.into();
        assert_eq!(err.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        let body = err.body();
        assert_eq!(body["error"]["code"], "VALIDATION_BLOCKED");
        assert_eq!(body["error"]["warnings"][0]["code"], "MODUL3_AND_FLAT_NNE");
    }
}

#[cfg(test)]
mod nehs_overlay_tests {
    use super::nehs_overlay_applies;
    use crate::config::BillingdConfig;
    use rust_decimal::dec;

    /// Minimal config; `rates` is spliced in as-is (`null` = no `[rates]`).
    fn cfg(rates: serde_json::Value) -> BillingdConfig {
        serde_json::from_value(serde_json::json!({
            "database": { "url": "postgres://unused" },
            "tenant": "9900000000001",
            "productd_url": "http://127.0.0.1:1",
            "edmd_url": "http://127.0.0.1:1",
            "marktd_url": "http://127.0.0.1:1",
            "rates": rates,
        }))
        .expect("test config must deserialize")
    }

    /// Overlay precedence: an explicit `[rates] behg_gas_ct_per_kwh` override
    /// short-circuits the market lookup — `apply_nehs_market_price` returns
    /// before any productd fetch, so the pinned rate stands untouched.
    #[test]
    fn explicit_behg_override_skips_the_market_fetch() {
        let pinned = cfg(serde_json::json!({ "behg_gas_ct_per_kwh": "1.25" }));
        assert_eq!(pinned.behg_override(), Some(dec!(1.25)));
        assert!(
            !nehs_overlay_applies("GAS", &pinned),
            "explicit [rates] override must win over the nEHS market series"
        );
        assert!(!nehs_overlay_applies("WAERME", &pinned));
    }

    /// Without an override, Gas and Wärme periods consult the market series.
    #[test]
    fn gas_and_waerme_without_override_use_the_market_series() {
        let open = cfg(serde_json::Value::Null);
        assert!(nehs_overlay_applies("GAS", &open));
        assert!(nehs_overlay_applies("WAERME", &open));
        // Other [rates] keys do not pin BEHG.
        let other = cfg(serde_json::json!({ "stromsteuer_ct_per_kwh": "2.05" }));
        assert!(nehs_overlay_applies("GAS", &other));
    }

    /// BEHG is a fuel-emissions levy — non-Gas/Wärme categories never overlay.
    #[test]
    fn non_gas_categories_never_overlay() {
        let open = cfg(serde_json::Value::Null);
        for category in ["STROM", "EEG", "SOLAR", "SHARING", "WASSER"] {
            assert!(!nehs_overlay_applies(category, &open), "{category}");
        }
    }

    /// The configured CO₂ factor reaches the EUR/t → ct/kWh conversion: an
    /// L-Gas deployment (0.20140) prices the same certificate lower than the
    /// H-Gas default (0.20160).
    #[test]
    fn configured_l_gas_factor_threads_into_the_conversion() {
        let l_gas = cfg(serde_json::json!({ "behg_co2_factor_kg_per_kwh": "0.20140" }));
        assert_eq!(l_gas.behg_co2_factor(), Some(dec!(0.20140)));
        let ct = energy_billing::behg_ct_per_kwh_from_price(dec!(65), l_gas.behg_co2_factor());
        assert_eq!(ct, dec!(65) * dec!(0.20140) / dec!(10));

        // Unset factor → None → H-Gas default inside the conversion.
        let default = cfg(serde_json::Value::Null);
        assert_eq!(default.behg_co2_factor(), None);
        let ct_h = energy_billing::behg_ct_per_kwh_from_price(dec!(65), default.behg_co2_factor());
        assert_eq!(ct_h, dec!(1.3104));
        assert!(
            ct < ct_h,
            "L-Gas factor must yield a lower ct/kWh than H-Gas"
        );
    }
}
