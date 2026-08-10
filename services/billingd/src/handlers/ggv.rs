//! §42b EnWG (Solarpaket I) GGV community solar billing and Tarifwechsel.

#[allow(unused_imports)]
use super::*;

// ── §42b EnWG (Solarpaket I) GGV Community Solar Multi-Tenant Billing ─────

/// Per-tenant input for the GGV proportional billing endpoint.
///
/// Each entry represents one tenant delivery point under the shared PV installation.
/// `consumption_kwh` is the metered actual consumption for the billing period from `edmd`.
#[derive(Debug, serde::Deserialize)]
pub struct GgvTenantInput {
    /// 11-digit MaLo-ID for this tenant's delivery point.
    pub malo_id: String,
    /// Metered actual consumption for the period (kWh) — from `edmd`.
    ///
    /// When `nutzungsplan` is set in `GgvBillingRequest`, billing is split into
    /// PV portion (allocated from plant generation) + residual grid portion.
    /// Without `nutzungsplan`, the full amount is billed as solar eigenverbrauch.
    pub consumption_kwh: rust_decimal::Decimal,
    /// Override product code; if absent, looked up from `tarifbd`.
    pub product_code: Option<String>,
    /// Supply price override (ct/kWh); if absent, looked up from `tarifbd`.
    pub arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Standard grid electricity rate for residual consumption (ct/kWh).
    ///
    /// Required when `pv_generation_kwh` is set and some tenants have consumption
    /// exceeding their PV allocation (grid fallback billing).
    pub grid_arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// GGV Rabatt on the PV portion (ct/kWh, §42b Abs. 3 EEG 2023).
    ///
    /// The discount reduces the net price of the PV portion below the standard
    /// electricity rate. Per §42b Abs. 3 EEG 2023 the LF must pass on savings from
    /// reduced grid charges for locally consumed PV electricity.
    pub gemeinschaft_rabatt_ct_per_kwh: Option<rust_decimal::Decimal>,
}

/// Request body for `POST /api/v1/billing/ggv/{ggv_id}`.
///
/// `ggv_id` is the operator-assigned ID of the Gemeinschaftliche Gebäudeversorgung
/// (typically the `tr_id` of the PV TechnischeRessource in `marktd`).
#[derive(Debug, serde::Deserialize)]
pub struct GgvBillingRequest {
    pub lf_mp_id: String,
    /// NB MP-ID for NNE pass-through (optional — supply in individual tenant rows if different).
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    pub period_from: String,
    pub period_to: String,
    /// Total PV generation of the GGV plant for the billing period (kWh).
    ///
    /// When supplied together with `nutzungsplan`, enables the full §42b billing model:
    /// - PV generation is allocated per tenant via the Nutzungsplan fractions
    /// - Each tenant invoice shows both the PV portion and the grid fallback portion
    ///
    /// When `None`, each tenant's full `consumption_kwh` is billed as solar eigenverbrauch
    /// (the previous simplified model — valid only when consumption ≤ plant output).
    pub pv_generation_kwh: Option<rust_decimal::Decimal>,
    /// GGV allocation plan: tenant fractions that sum to 1.0.
    ///
    /// Required when `pv_generation_kwh` is supplied. The fractions determine how much
    /// of the plant's generation each tenant is entitled to (§42b Abs. 2 EEG 2023).
    /// Entries must match the `malo_id` values in `tenants`.
    pub nutzungsplan: Option<Vec<NutzungsplanInput>>,
    /// All tenant delivery points belonging to this GGV installation.
    pub tenants: Vec<GgvTenantInput>,
}

/// One entry in the GGV Nutzungsplan submitted via the billing API.
#[derive(Debug, serde::Deserialize)]
pub struct NutzungsplanInput {
    pub malo_id: String,
    pub fraction: rust_decimal::Decimal,
}

/// Request body for `POST /api/v1/billing/{malo_id}/tarifwechsel`.
///
/// Calculates a combined invoice when a price change occurs within the billing
/// period. Uses `billing::merge_period_documents` semantics via `Invoice::merge()`.
#[derive(Debug, serde::Deserialize)]
pub struct TarifwechselRequest {
    /// Lieferant MP-ID.
    pub lf_mp_id: String,
    /// §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    /// Start of the billing period (inclusive, YYYY-MM-DD).
    pub period_from: String,
    /// End of the billing period (inclusive, YYYY-MM-DD).
    pub period_to: String,
    /// Date when the new tariff takes effect (YYYY-MM-DD, must be within the period).
    pub switch_date: String,
    /// Old tariff (applies from `period_from` to `switch_date - 1`).
    pub old_tariff: Product,
    /// New tariff (applies from `switch_date` to `period_to`).
    pub new_tariff: Product,
    /// Meter data for the old sub-period.
    #[serde(default)]
    pub old_meter: Option<MeterInput>,
    /// Meter data for the new sub-period.
    #[serde(default)]
    pub new_meter: Option<MeterInput>,
    /// Optional grid pass-through data.
    #[serde(default)]
    pub grid: Option<GridInput>,
}

/// `POST /api/v1/billing/{malo_id}/tarifwechsel`
///
/// Calculates a combined invoice for a billing period containing a price change
/// (Tarifwechsel). The period is split at `switch_date`:
///
/// - **Sub-period A**: `period_from` → `switch_date - 1` at `old_tariff`
/// - **Sub-period B**: `switch_date` → `period_to` at `new_tariff`
///
/// The two invoices are merged via [`Invoice::merge()`] using the same logic
/// as `billing::merge_period_documents`: positions are concatenated, totals
/// re-summed. Tax is applied **independently** per sub-period (correct for
/// mid-month rate changes per §41 EnWG).
///
/// ## Legal basis
///
/// §41 Abs. 1 Nr. 4 EnWG: every price change requires transparent itemisation
/// on the next invoice showing the old and new price with their respective
/// applicable periods.
pub async fn post_tarifwechsel(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(malo_id): Path<String>,
    Json(req): Json<TarifwechselRequest>,
) -> impl IntoResponse {
    // Parse all three date boundaries
    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("period: {e}")).into_response(),
    };
    let switch_date = match parse_period(&req.switch_date, &req.switch_date) {
        Ok((d, _)) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("switch_date: {e}")).into_response(),
    };
    if switch_date <= period_from || switch_date > period_to {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "switch_date {switch_date} must be strictly inside [{period_from}, {period_to}]"
            ),
        )
            .into_response();
    }

    let grid = req.grid.clone().unwrap_or_default();

    // Build the rechnungsnummer prefix — use timestamp for uniqueness
    let base_nr = format!("TW-{malo_id}-{period_from}",);

    // ── Sub-period A: period_from → switch_date - 1 ───────────────────────────
    // Each leg is billed under the statutory rates of *its own* dates and
    // commodity — that is the point of the split (§41 Abs. 5 EnWG price
    // change; a leg inside a VAT window carries that window's rate).
    let period_a_to = switch_date - time::Duration::days(1);
    let rates_a = match cfg.try_regulatory_rates_for_period(
        req.old_tariff.category_str(),
        period_from,
        period_a_to,
    ) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                super::straddle_error_body(&e),
            )
                .into_response();
        }
    };
    let run_id_a = Uuid::new_v4().to_string();
    let ctx_a = BillingContext {
        malo_id: malo_id.clone(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: format!("{base_nr}-A"),
        period: BillingPeriod::new(period_from, period_a_to)
            .expect("switch date is validated inside the period"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_a.clone(),
        nb_mp_id: req.nb_mp_id.clone(),
        billing_run_id: Some(run_id_a),
        ..Default::default()
    };
    let quantities_a = Quantities {
        electricity: req.old_meter.clone(),
        ..Default::default()
    };
    let engine_a = req.old_tariff.build_engine(&grid, &rates_a);
    let inv_a = match engine_a.bill(ctx_a, &quantities_a) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                engine_error_body("tarifwechsel period A", &e),
            )
                .into_response();
        }
    };

    // ── Sub-period B: switch_date → period_to ─────────────────────────────────
    let rates_b = match cfg.try_regulatory_rates_for_period(
        req.new_tariff.category_str(),
        switch_date,
        period_to,
    ) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                super::straddle_error_body(&e),
            )
                .into_response();
        }
    };
    let run_id_b = Uuid::new_v4().to_string();
    let ctx_b = BillingContext {
        malo_id: malo_id.clone(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: format!("{base_nr}-B"),
        period: BillingPeriod::new(switch_date, period_to)
            .expect("switch date is validated inside the period"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_b.clone(),
        nb_mp_id: req.nb_mp_id.clone(),
        billing_run_id: Some(run_id_b),
        ..Default::default()
    };
    let quantities_b = Quantities {
        electricity: req.new_meter.clone(),
        ..Default::default()
    };
    let engine_b = req.new_tariff.build_engine(&grid, &rates_b);
    let inv_b = match engine_b.bill(ctx_b, &quantities_b) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                engine_error_body("tarifwechsel period B", &e),
            )
                .into_response();
        }
    };

    // ── Merge via billing::merge_period_documents semantics ───────────────────
    let merged = inv_a.merge(inv_b);
    merged.assert_valid();

    let rechnung_json = merged.to_rechnung_json();
    let netto = merged.netto_eur;
    let brutto = merged.brutto_eur;

    let product_code = format!(
        "{}-{}",
        req.old_tariff.category_str(),
        req.new_tariff.category_str()
    );
    // Combined Tarifwechsel invoice + its dispatch event commit atomically.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let record_id = match insert_billing_record(
        &mut *tx,
        &cfg.tenant,
        &malo_id,
        &req.lf_mp_id,
        &product_code,
        "TARIFWECHSEL",
        period_from,
        period_to,
        &rechnung_json,
        netto,
        brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if cfg.erp_webhook_url.is_some() {
        let ce = rechnung_erstellt_ce(record_id, &malo_id, &req.lf_mp_id, &rechnung_json, false);
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        if let Err(e) = mark_dispatched_tx(&mut *tx, record_id).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "record_id": record_id,
            "malo_id": malo_id,
            "period_from": period_from.to_string(),
            "switch_date": switch_date.to_string(),
            "period_to": period_to.to_string(),
            "netto_eur": netto,
            "brutto_eur": brutto,
            "old_category": req.old_tariff.category_str(),
            "new_category": req.new_tariff.category_str(),
        })),
    )
        .into_response()
}

/// `POST /api/v1/billing/ggv/{ggv_id}` — §42b EnWG community solar (GGV) billing.
///
/// Allocates plant generation across tenants (§42b EnWG proportional
/// allocation) and emits one invoice per tenant. Rejects an empty `tenants` list
/// or a zero total-kWh input.
///
/// ## §42b EnWG (Solarpaket I) — two billing models
///
/// **Model A — Nutzungsplan-based (recommended)**: supply `pv_generation_kwh` +
/// `nutzungsplan`. Each tenant is allocated a proportional share of plant
/// generation. Per-tenant invoices show both the PV portion (at the GGV rate) and
/// the residual grid electricity (at the standard rate) — §42b Abs. 2 EEG 2023.
///
/// **Model B — Direct consumption (legacy)**: omit `pv_generation_kwh`. Each
/// tenant's full `consumption_kwh` is billed as solar Eigenverbrauch. Only valid
/// when consumption ≤ plant output for every tenant.
///
/// The GGV Rabatt (§42b Abs. 3 EEG 2023) reflects the savings from reduced network
/// charges for locally consumed PV electricity.
#[allow(clippy::too_many_arguments)]
pub async fn post_ggv_billing(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(vertragd): Extension<Arc<crate::clients::VertragdClient>>,
    Path(ggv_id): Path<String>,
    Json(req): Json<GgvBillingRequest>,
) -> impl IntoResponse {
    if req.tenants.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "GGV request must contain at least one tenant",
        )
            .into_response();
    }

    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let total_kwh: rust_decimal::Decimal = req.tenants.iter().map(|t| t.consumption_kwh).sum();
    if total_kwh <= rust_decimal::Decimal::ZERO {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "total GGV consumption must be > 0 kWh",
        )
            .into_response();
    }

    // ── §42b Model A: Nutzungsplan-based PV allocation ─────────────────────────
    // Build a MaloId → allocated_pv_kwh map when pv_generation_kwh is supplied.
    let pv_allocations: std::collections::HashMap<String, rust_decimal::Decimal> =
        if let (Some(pv_gen_kwh), Some(np)) = (req.pv_generation_kwh, req.nutzungsplan.as_ref()) {
            use energy_billing::{GgvNutzungsplan, GgvNutzungsplanEntry};
            let plan = GgvNutzungsplan(
                np.iter()
                    .map(|e| GgvNutzungsplanEntry {
                        malo_id: e.malo_id.clone(),
                        fraction: e.fraction,
                    })
                    .collect(),
            );
            if let Err(e) = plan.validate() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("nutzungsplan: {e}"),
                )
                    .into_response();
            }
            plan.allocate(pv_gen_kwh).into_iter().collect()
        } else {
            std::collections::HashMap::new()
        };

    let rates = match cfg.try_regulatory_rates_for_period("SOLAR", period_from, period_to) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                super::straddle_error_body(&e),
            )
                .into_response();
        }
    };
    let mut tenant_results: Vec<serde_json::Value> = Vec::with_capacity(req.tenants.len());
    let mut parts: Vec<(String, Invoice)> = Vec::with_capacity(req.tenants.len());
    let mut tenant_record_ids: Vec<Uuid> = Vec::with_capacity(req.tenants.len());

    for tenant in &req.tenants {
        // Build Product — prefer request overrides, fall back to tarifbd lookup.
        let tariff = match tarifbd
            .get_customer_product(&tenant.malo_id, &req.lf_mp_id)
            .await
        {
            Ok(Some(t)) => t,
            Ok(None) => {
                // No product in tarifbd — build minimal Product from request overrides.
                let map = serde_json::json!({
                    "category": "SOLAR",
                    "product_code": tenant.product_code,
                    "solar_arbeitspreis_ct_per_kwh": tenant.arbeitspreis_ct_per_kwh,
                    "gemeinschaft_rabatt_ct_per_kwh": tenant.gemeinschaft_rabatt_ct_per_kwh,
                    "arbeitspreis_ct_per_kwh": tenant.grid_arbeitspreis_ct_per_kwh,
                });
                match serde_json::from_value::<Product>(map) {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            format!("tariff build: {e}"),
                        )
                            .into_response();
                    }
                }
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("tarifbd: {e}")).into_response(),
        };

        // Per-request overrides take precedence over tarifbd product data.
        // Product is an enum — apply overrides by rebuilding the Solar/Sharing variant.
        let tariff = match tariff {
            Product::Solar(mut p) => {
                if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
                    p.solar_arbeitspreis_ct_per_kwh = Some(ap);
                }
                // solar_arbeitspreis is also used for grid remainder in SolarProvider
                if let Some(rabatt) = tenant.gemeinschaft_rabatt_ct_per_kwh {
                    if let Some(ap) = p.solar_arbeitspreis_ct_per_kwh {
                        let cap = ap * rust_decimal::dec!(0.10);
                        if rabatt > cap {
                            tracing::warn!(
                                malo_id = %tenant.malo_id,
                                ggv_id = %ggv_id,
                                rabatt_ct = %rabatt,
                                cap_diagnostic = %cap,
                                "billingd GGV: gemeinschaft_rabatt > 10% of Arbeitspreis — \
                                 verify §42b Abs. 3 EEG 2023 compliance against local Grundversorgungstarif"
                            );
                        }
                    }
                    p.gemeinschaft_rabatt_ct_per_kwh = Some(rabatt);
                }
                Product::Solar(p)
            }
            Product::Sharing(mut p) => {
                if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
                    p.electricity.solar_include_stromsteuer = false; // GGV shares are Stromsteuer-free
                    p.electricity.arbeitspreis_ct_per_kwh = Some(ap);
                }
                if let Some(rabatt) = tenant.gemeinschaft_rabatt_ct_per_kwh {
                    p.sharing_credit_ct_per_kwh = Some(rabatt);
                }
                Product::Sharing(p)
            }
            other => other,
        };

        // ── Build Quantities: Model A (GgvSolarInput) or Model B (SolarMeterInput) ──
        let quantities = if let Some(&pv_allocated) = pv_allocations.get(&tenant.malo_id) {
            // Model A: proportional allocation — hybrid PV + grid billing
            Quantities {
                ggv_solar: Some(energy_billing::GgvSolarInput {
                    pv_allocated_kwh: pv_allocated,
                    actual_consumption_kwh: tenant.consumption_kwh,
                }),
                ..Default::default()
            }
        } else {
            // Model B: direct consumption as solar eigenverbrauch
            Quantities {
                solar: Some(SolarMeterInput {
                    eigenverbrauch_kwh: tenant.consumption_kwh,
                }),
                ..Default::default()
            }
        };

        let rechnungsnummer = tenant
            .product_code
            .as_deref()
            .map(|p| format!("GGV-{ggv_id}-{p}-{period_from}"))
            .unwrap_or_else(|| format!("GGV-{ggv_id}-{}-{period_from}", tenant.malo_id));

        let ctx = BillingContext {
            malo_id: tenant.malo_id.clone(),
            lf_mp_id: req.lf_mp_id.clone(),
            rechnungsnummer: rechnungsnummer.clone(),
            period: BillingPeriod::new(period_from, period_to)
                .expect("parse_period guarantees from < to"),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: rates.clone(),
            contract_id: None,
            ..Default::default()
        };
        let engine = tariff.build_engine(&GridInput::default(), &rates);

        let result = match engine.bill(ctx, &quantities) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    engine_error_body(&format!("GGV tenant {}", tenant.malo_id), &e),
                )
                    .into_response();
            }
        };

        let record_id = match insert_billing_record(
            &pool,
            &cfg.tenant,
            &tenant.malo_id,
            &req.lf_mp_id,
            tariff.product_code().unwrap_or("SOLAR_GGV"),
            "SOLAR",
            period_from,
            period_to,
            &result.to_rechnung_json(),
            result.netto_eur,
            result.brutto_eur,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        // A GGV Teilnehmer under §42b is a Letztverbraucher with their own MaLo
        // and supply relationship, so the ordinary BG-7 lookup applies.
        let buyer = vertragd
            .get_vertrag_by_malo(&tenant.malo_id)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.rechnungsempfaenger);
        if let Err(e) = crate::einvoice::store(
            &pool,
            record_id,
            &result,
            &cfg,
            &tenant.malo_id,
            buyer.as_ref(),
        )
        .await
        {
            tracing::warn!(%record_id, error = %e, "billingd: attach en16931 model failed");
        }

        tenant_results.push(serde_json::json!({
            "record_id": record_id,
            "malo_id": tenant.malo_id,
            "consumption_kwh": tenant.consumption_kwh,
            "netto_eur": result.netto_eur,
            "brutto_eur": result.brutto_eur,
        }));
        tenant_record_ids.push(record_id);
        parts.push((tenant.malo_id.clone(), result));
    }

    // Consolidated SAMMEL document for the GGV installation — through the
    // engine, like every other invoice: derived totals, per-rate VAT over the
    // combined base, deterministic rechnungsdatum.
    let sammel_nr = format!("GGV-SAMMEL-{ggv_id}-{period_from}");
    let (sammel_invoice, sammel_rechnung) = match build_aggregate_invoice(
        &ggv_id,
        &req.lf_mp_id,
        sammel_nr,
        period_from,
        period_to,
        rates,
        parts,
        vec![
            zusatz_attribut("ggv_id", serde_json::json!(ggv_id)),
            zusatz_attribut(
                "tenant_count",
                serde_json::json!(tenant_results.len().to_string()),
            ),
            zusatz_attribut("total_kwh", serde_json::json!(total_kwh.to_string())),
        ],
    ) {
        Ok(x) => x,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let (sammel_netto, sammel_brutto) = (sammel_invoice.netto_eur, sammel_invoice.brutto_eur);

    // Consolidated GGV Sammelrechnung + its dispatch event commit atomically;
    // the per-tenant detail records above are separate bookkeeping writes.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let sammel_id = match insert_sammelrechnung_record(
        &mut *tx,
        &cfg.tenant,
        &ggv_id,
        &req.lf_mp_id,
        period_from,
        period_to,
        &sammel_rechnung,
        sammel_netto,
        sammel_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if cfg.erp_webhook_url.is_some() {
        let ce = rechnung_erstellt_ce(sammel_id, &ggv_id, &req.lf_mp_id, &sammel_rechnung, false);
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        if let Err(e) = mark_dispatched_tx(&mut *tx, sammel_id).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    if let Err(e) =
        // No BG-7 buyer, and none is reachable: this bills the GGV operator, and
        // the key is a GGV id rather than a MaLo. vertragd models Kunden behind
        // Versorgungs- and Rahmenverträge, not GGV bundles.
        crate::einvoice::store(&mut *tx, sammel_id, &sammel_invoice, &cfg, &ggv_id, None)
                .await
    {
        tracing::warn!(%sammel_id, error = %e, "billingd: attach en16931 model failed");
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Link the per-tenant detail records to the committed Sammelrechnung so the
    // risk baseline and record listings treat them as its children, not as
    // standalone invoices double-counted alongside the SAMMEL.
    if let Err(e) = link_to_sammelrechnung(&pool, &tenant_record_ids, sammel_id).await {
        tracing::warn!(error = %e, %sammel_id, "GGV: linking per-tenant records to Sammelrechnung failed");
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ggv_id": ggv_id,
            "sammel_id": sammel_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "total_kwh": total_kwh,
            "tenant_count": tenant_results.len(),
            "total_netto_eur": sammel_netto,
            "total_brutto_eur": sammel_brutto,
            "tenants": tenant_results,
        })),
    )
        .into_response()
}
