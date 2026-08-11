//! Meter auto-enrichment (Gas, Strom, dynamic intervals) and NEHS market-price overlay.

#[allow(unused_imports)]
use super::*;

// ── Gas meter auto-enrichment ─────────────────────────────────────────────────

/// Normalize a raw `gasqualitaet` string to a canonical BO4E / BNetzA MaStR form.
///
/// ## Canonical values
///
/// | Canonical | Aliases accepted |
/// |---|---|
/// | `H_GAS` | `HGas`, `H-Gas`, `H-gas`, `HGAS`, `HIGH_CALORIFIC` |
/// | `L_GAS` | `LGas`, `L-Gas`, `L-gas`, `LGAS`, `LOW_CALORIFIC` |
/// | `H2_BLEND` | `H2Blend`, `H2-Blend`, `HYDROGEN_BLEND` |
/// | `BIOGAS` | `BioGas`, `Bio-Gas` |
/// | `FLUESSIGGAS` | `LPG`, `FlüssigGas` |
///
/// Unknown values are returned as-is (upper-case, underscores).
///
/// ## Why normalization matters
///
/// `marktd` stores `gasqualitaet` as extracted from the UTILMD G `STS+E01+Z12`
/// qualifier — typically `"HGas"` or `"LGas"` (legacy German abbreviations).
/// The BO4E schema (`rubo4e::GasQualitaet`) and BNetzA MaStR use `"H_GAS"` /
/// `"L_GAS"` / `"H2_BLEND"`.  Billing invoices, comparison portals, and AI agents
/// all benefit from a single canonical form.
pub(crate) fn normalize_gasqualitaet(raw: &str) -> String {
    // Normalize to UPPER_SNAKE_CASE first for uniform matching.
    let norm = raw.trim().to_uppercase().replace(['-', ' '], "_");
    match norm.as_str() {
        "HGAS" | "H_GAS" | "HIGH_CALORIFIC" | "HOCHKALORISCH" | "ERDGAS_H" => "H_GAS".to_owned(),
        "LGAS" | "L_GAS" | "LOW_CALORIFIC" | "NIEDERKALORISCH" | "ERDGAS_L" => "L_GAS".to_owned(),
        "H2_BLEND" | "H2BLEND" | "HYDROGEN_BLEND" | "HYDROGEN_GAS" | "H2_GAS" => {
            "H2_BLEND".to_owned()
        }
        "BIOGAS" | "BIO_GAS" | "BIOMETHANE" | "BIOMETHAN" => "BIOGAS".to_owned(),
        "FLUESSIGGAS" | "FLUSSIGGAS" | "LPG" | "LIQUID_GAS" => "FLUESSIGGAS".to_owned(),
        other => other.to_owned(),
    }
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
/// | `messung_qm3` | caller (`req.gas_meter`) | `edmd` billing-period | `0` (engine rejects) |
/// | `brennwert_kwh_per_qm3` | caller | edmd **gas-quality** (PID 13007) | edmd billing-period | `None` (engine applies default 10.55) |
/// | `zustandszahl` | caller | edmd gas-quality (PID 13007) | edmd billing-period | `None` (engine applies default 1.0) |
/// | `spitzenleistung_kw` | caller | edmd billing-period | `None` (no RLM demand charge) |
/// | `gasqualitaet` | caller | marktd MaLo fields | `None` (no audit annotation) |
///
/// ## Non-blocking
///
/// All external fetches are best-effort.  Failures are logged as `WARN` and
/// billing proceeds with the data available.  This prevents an edmd or marktd
/// outage from blocking all gas invoicing.
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
) {
    use crate::clients::{GasBillingPeriod, GasQualityRecord};

    // Track which fields were enriched for structured logging.
    let mut enriched_from_edmd_period = false;
    let mut enriched_bw_from_edmd_quality = false;
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
                tracing::debug!(malo_id, "billingd GAS: no billing period in edmd");
            }
            Err(e) => {
                tracing::warn!(
                    malo_id,
                    error = %e,
                    "billingd GAS: edmd billing-period fetch failed — proceeding without"
                );
            }
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
                        enriched_bw_from_edmd_quality = true;
                    }
                    if meter.zustandszahl.is_none() {
                        meter.zustandszahl = Some(q.zustandszahl);
                        enriched_bw_from_edmd_quality = true;
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
    // Informational only — billing always uses the measured Brennwert.
    // Annotated on the invoice as `ZusatzAttribut` for § 147 AO / GoBD audit trail
    // and for H2-blend detection in downstream AI agents (eeg-compliance-agent).
    if meter.gasqualitaet.is_none() {
        match marktd.get_malo(malo_id).await {
            Ok(Some(malo_fields)) => {
                if let Some(raw_gq) = malo_fields.gasqualitaet {
                    let canonical = normalize_gasqualitaet(&raw_gq);
                    meter.gasqualitaet = Some(canonical);
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
    if enriched_from_edmd_period || enriched_bw_from_edmd_quality || enriched_gq_from_marktd {
        tracing::debug!(
            malo_id,
            messung_qm3                   = %meter.messung_qm3,
            brennwert_kwh_per_qm3         = ?meter.brennwert_kwh_per_qm3,
            zustandszahl                  = ?meter.zustandszahl,
            spitzenleistung_kw            = ?meter.spitzenleistung_kw,
            gasqualitaet                  = ?meter.gasqualitaet,
            enriched_from_edmd_period,
            enriched_bw_from_edmd_quality,
            enriched_gq_from_marktd,
            "billingd GAS: meter enrichment complete"
        );
    }
}

pub(crate) async fn resolve_strom_meter(
    req: &CalculateRequest,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
) -> Result<MeterInput, (StatusCode, String)> {
    if let Some(m) = req.meter.clone() {
        return Ok(m);
    }
    match edmd
        .get_billing_period(malo_id, period_from, period_to)
        .await
    {
        Ok(Some(m)) => Ok(m),
        Ok(None) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("No meter data for MaLo {malo_id}"),
        )),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("edmd: {e}"))),
    }
}

pub(crate) async fn fetch_dynamic_intervals(
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
) -> Vec<DynamicInterval> {
    edmd.get_lastgang(malo_id, period_from, period_to)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(malo_id, error = %e, "billingd: Lastgang fetch failed");
            Vec::new()
        })
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
/// CO₂ component follows the supplier's dated acquisition prices in tarifbd's
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
    tarifbd: &TarifbdClient,
) {
    if !nehs_overlay_applies(category, cfg) {
        return;
    }
    match tarifbd.get_latest_nehs_price(period_from).await {
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

pub(crate) async fn fetch_epex_prices(
    period_from: time::Date,
    period_to: time::Date,
    tarifbd: &Arc<TarifbdClient>,
) -> std::collections::HashMap<time::OffsetDateTime, rust_decimal::Decimal> {
    // tarifbd owns the imported EPEX day-ahead series (15-min MTU). The map is
    // keyed on each MTU's UTC start instant, matching how `energy-billing`
    // floors a consumption interval to its quarter-hour.
    match tarifbd.get_epex_prices(period_from, period_to).await {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(error = %e, "billingd: EPEX price fetch failed; dynamic intervals will lack prices");
            std::collections::HashMap::new()
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod gas_enrichment_tests {
    use crate::handlers::{build_aggregate_invoice, engine_error_body, normalize_gasqualitaet};
    use energy_billing::{
        BillingContext, BillingPeriod, BillingPosition, BillingProvider as _, Invoice, InvoiceType,
        MwStProvider, Quantities, RegulatoryRates,
    };

    // ── normalize_gasqualitaet ────────────────────────────────────────────────

    #[test]
    fn normalize_hgas_variants() {
        // All aliases for H-Gas must map to "H_GAS"
        for raw in &[
            "HGas",
            "H-Gas",
            "H-gas",
            "HGAS",
            "H_GAS",
            "HIGH_CALORIFIC",
            "ERDGAS_H",
        ] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "H_GAS",
                "expected H_GAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_lgas_variants() {
        for raw in &[
            "LGas",
            "L-Gas",
            "L-gas",
            "LGAS",
            "L_GAS",
            "LOW_CALORIFIC",
            "ERDGAS_L",
        ] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "L_GAS",
                "expected L_GAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_h2_blend_variants() {
        for raw in &[
            "H2_BLEND",
            "H2Blend",
            "H2-Blend",
            "HYDROGEN_BLEND",
            "H2BLEND",
        ] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "H2_BLEND",
                "expected H2_BLEND for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_biogas_variants() {
        for raw in &["BIOGAS", "BioGas", "Bio-Gas", "BIOMETHANE", "BIOMETHAN"] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "BIOGAS",
                "expected BIOGAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_fluessiggas_variants() {
        for raw in &["FLUESSIGGAS", "LPG", "LIQUID_GAS"] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "FLUESSIGGAS",
                "expected FLUESSIGGAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_unknown_returns_uppercase_underscored() {
        // Unknown values are normalized to UPPER_SNAKE_CASE but preserved.
        assert_eq!(normalize_gasqualitaet("syngas"), "SYNGAS");
        assert_eq!(
            normalize_gasqualitaet("Compressed Natural Gas"),
            "COMPRESSED_NATURAL_GAS"
        );
    }

    #[test]
    fn normalize_already_canonical_is_idempotent() {
        for canonical in &["H_GAS", "L_GAS", "H2_BLEND", "BIOGAS", "FLUESSIGGAS"] {
            let result = normalize_gasqualitaet(canonical);
            assert_eq!(
                &result, canonical,
                "normalize_gasqualitaet should be idempotent on canonical value {canonical}"
            );
        }
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(normalize_gasqualitaet("  HGas  "), "H_GAS");
        assert_eq!(normalize_gasqualitaet("\tLGas\n"), "L_GAS");
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
        let parts: Vec<(String, Invoice)> = ["11111111111", "22222222222", "33333333333"]
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

    /// Every rendered position names the MaLo it came from; the document-level
    /// tax position names none.
    #[test]
    fn aggregate_annotates_positions_with_their_malo() {
        use rust_decimal::dec;
        let parts = vec![
            sub_invoice("11111111111", dec!(50)),
            sub_invoice("22222222222", dec!(70)),
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
        // 2 commodity positions annotated + 1 aggregate tax position without.
        assert_eq!(pos[0]["marktlokationsId"], "11111111111");
        assert_eq!(pos[1]["marktlokationsId"], "22222222222");
        let tax = pos
            .iter()
            .find(|p| p["kategorie"] == "Tax")
            .expect("aggregate tax position");
        assert!(tax.get("marktlokationsId").is_none());
        // Deterministic rechnungsdatum — no wall clock in the document.
        assert_eq!(json["rechnungsdatum"], "2026-01-31");
    }

    /// The engine-error body is machine-readable: code, context, warnings.
    #[test]
    fn engine_error_body_is_structured() {
        let e = energy_billing::EngineError::ValidationBlocked {
            warnings: vec![energy_billing::BillingWarning {
                code: "MODUL3_AND_FLAT_NNE",
                severity: energy_billing::WarningSeverity::Error,
                message: "both configured".to_owned(),
            }],
        };
        let body: serde_json::Value =
            serde_json::from_str(&engine_error_body("51238696781", &e)).unwrap();
        assert_eq!(body["error"]["code"], "VALIDATION_BLOCKED");
        assert_eq!(body["error"]["context"], "51238696781");
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
            "tarifbd_url": "http://127.0.0.1:1",
            "edmd_url": "http://127.0.0.1:1",
            "marktd_url": "http://127.0.0.1:1",
            "rates": rates,
        }))
        .expect("test config must deserialize")
    }

    /// Overlay precedence: an explicit `[rates] behg_gas_ct_per_kwh` override
    /// short-circuits the market lookup — `apply_nehs_market_price` returns
    /// before any tarifbd fetch, so the pinned rate stands untouched.
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
