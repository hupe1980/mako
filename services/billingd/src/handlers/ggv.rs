//! §42b EnWG (Solarpaket I) GGV community solar billing and Tarifwechsel.

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
    /// Override product code; if absent, looked up from `productd`.
    pub product_code: Option<String>,
    /// Supply price override (ct/kWh); if absent, looked up from `productd`.
    pub arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Standard grid electricity rate for residual consumption (ct/kWh).
    ///
    /// Required when `pv_generation_kwh` is set and some tenants have consumption
    /// exceeding their PV allocation (grid fallback billing).
    pub grid_arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Price advantage on the PV portion (ct/kWh).
    ///
    /// Purely **contractual**: §42b Abs. 2 Nr. 2 EnWG requires the
    /// Gebäudestromnutzungsvertrag to state a Vergütung in ct/kWh for the
    /// shared electricity, and this is the discount against the residual grid
    /// rate that the parties agreed. There is no statutory pass-through duty
    /// for saved Netzentgelte — §42b EnWG does not contain one — so nothing
    /// here may be presented as a legal requirement.
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
    /// Required when `pv_generation_kwh` is supplied. The fractions are the
    /// Aufteilungsschlüssel the Gebäudestromnutzungsvertrag must state
    /// (§42b Abs. 2 Nr. 1 EnWG). Entries must match the `malo_id` values in
    /// `tenants`.
    ///
    /// **§42b Abs. 5 EnWG caps what may be allocated at all**: only the
    /// electricity generated *and* consumed inside the same 15-minute interval
    /// is shareable. A period total cannot express that cap, so a caller
    /// supplying `pv_generation_kwh` must already have applied the
    /// quarter-hour matching — `edmd`'s virtual-meter allocation does. Handing
    /// in a raw meter total over-allocates.
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
    /// Override the combined invoice's number. Absent — the normal case — takes
    /// the next number of the tenant's `RE` series.
    #[serde(default)]
    pub rechnungsnummer: Option<String>,
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
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(malo_id): Path<String>,
    Json(req): Json<TarifwechselRequest>,
) -> BillingResult<impl IntoResponse> {
    let cfg = Arc::clone(&deps.cfg);
    authorize(&cedar, &claims, "run-billing", &cfg.tenant)?;
    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;
    // A single date, parsed as a single date — not through
    // `parse_period(switch_date, switch_date)`, which pairs a date with itself
    // and reports any refusal as a period error.
    let switch_date = parse_date("switch_date", &req.switch_date)?;
    if switch_date <= period_from || switch_date > period_to {
        return Err(BillingError::bad_request(
            "SWITCH_DATE_OUTSIDE_PERIOD",
            format!(
                "switch_date {switch_date} must fall strictly inside \
                 ({period_from}, {period_to}]"
            ),
        ));
    }

    // The combined document's own number, from the ordinary invoice series; the
    // two legs carry `/A` and `/B` suffixes for the trace, but only the merged
    // invoice is issued, so only it consumes a number.
    let base_nr = next_rechnungsnummer(
        &pool,
        &cfg.tenant,
        series::INVOICE,
        req.rechnungsnummer.as_deref(),
        period_from,
    )
    .await?;

    // Both legs go through the same pipeline every other invoice uses, with
    // the readings the caller split by hand. Driving the engine directly here
    // — which is what this did — produced a Tarifwechsel invoice with none of
    // the § 40 content the law requires of it: no contract facts, no
    // Zählernummer, no consumption comparison, no BG-7 buyer and no § 13b
    // reverse-charge derivation. It looked like an invoice and was missing half
    // of one.
    let legs = vec![
        TariffLeg {
            tariff: req.old_tariff.clone(),
            from: period_from,
            to: switch_date - time::Duration::days(1),
            meter: req.old_meter.clone(),
        },
        TariffLeg {
            tariff: req.new_tariff.clone(),
            from: switch_date,
            to: period_to,
            meter: req.new_meter.clone(),
        },
    ];
    // A leg whose reading the caller supplied is billed as one span; one read
    // from edmd is split further at any statutory boundary inside it.
    let legs = split_on_rate_boundaries(cfg.as_ref(), legs);
    let leg_req = CalculateRequest {
        lf_mp_id: req.lf_mp_id.clone(),
        nb_mp_id: req.nb_mp_id.clone(),
        period_from: req.period_from.clone(),
        period_to: req.period_to.clone(),
        grid: req.grid.clone(),
        ..Default::default()
    };
    let billed =
        dispatch_invoice_multi(&deps, &legs, &leg_req, &malo_id, &base_nr, RunId(None)).await?;
    let merged = billed.invoice;
    let buyer = billed.buyer;

    let rechnung_json = merged.to_rechnung_json();
    let netto = merged.netto_eur;
    let brutto = merged.brutto_eur;
    let summary = LegSummary::of(&legs);
    let product_code = summary.product_code;

    // The risk gate scores the document against the rates in force at its end.
    let rates_b =
        cfg.try_regulatory_rates_for_period(req.new_tariff.category_str(), switch_date, period_to)?;

    // A Tarifwechsel invoice is an invoice: it goes through the same risk gate
    // and stores the same EN 16931 model as every other document, or the
    // render endpoints answer 422 for it and no analyst ever sees it.
    let assessment = assess_risk(
        &pool,
        &cfg,
        &malo_id,
        &merged,
        &rates_b,
        period_from,
        period_to,
    )
    .await;
    let held = assessment
        .as_ref()
        .is_some_and(|a| cfg.risk.hold_dispatch && a.band == crate::risk::RiskBand::Held);
    // Combined Tarifwechsel invoice + its dispatch event commit atomically.
    let mut tx = pool.begin().await?;
    let record_id = insert_billing_record(
        &mut *tx,
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &malo_id,
            lf_mp_id: &req.lf_mp_id,
            product_code: &product_code,
            category: &summary.category,
            rechnungsnummer: &base_nr,
            period_from,
            period_to,
            rechnung_json: &rechnung_json,
            total_netto_eur: netto,
            total_brutto_eur: brutto,
        },
    )
    .await;
    let record_id = match record_id {
        Ok(id) => id,
        Err(e) => return Err(period_conflict(&pool, &cfg.tenant, e).await),
    };
    if held {
        tracing::warn!(
            %record_id, %malo_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: Tarifwechsel invoice HELD by risk gate"
        );
    } else {
        let ce = rechnung_erstellt_ce(record_id, &malo_id, &req.lf_mp_id, &rechnung_json, false);
        issue_record(&mut tx, &cfg, record_id, &ce).await?;
    }
    persist_risk(&mut *tx, record_id, assessment.as_ref()).await?;
    crate::einvoice::store(&mut *tx, record_id, &merged, &cfg, &malo_id, buyer.as_ref()).await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "record_id": record_id,
            "malo_id": malo_id,
            "rechnungsnummer": base_nr,
            "period_from": period_from.to_string(),
            "switch_date": switch_date.to_string(),
            "period_to": period_to.to_string(),
            "netto_eur": netto,
            "brutto_eur": brutto,
            "old_category": req.old_tariff.category_str(),
            "new_category": req.new_tariff.category_str(),
            "risk": assessment,
            "held": held,
        })),
    ))
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
/// `nutzungsplan`. Each participant is allocated their Aufteilungsschlüssel
/// share of the shareable generation (§42b Abs. 2 Nr. 1 EnWG); the invoice shows
/// that portion at the agreed Gebäudestrom rate and the remainder as residual
/// grid supply, which §42b Abs. 3 EnWG explicitly contemplates because the
/// operator owes no Vollversorgung.
///
/// **Model B — Direct consumption**: omit `pv_generation_kwh`. Each
/// participant's full `consumption_kwh` is billed as Gebäudestrom. Only valid
/// when the plant covered every participant for the whole period.
///
/// The price advantage on the shared portion is contractual (§42b Abs. 2 Nr. 2
/// EnWG names the Vergütung as a contract term), not a statutory rebate.
#[allow(clippy::too_many_arguments)]
pub async fn post_ggv_billing(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(ggv_id): Path<String>,
    Json(req): Json<GgvBillingRequest>,
) -> BillingResult<impl IntoResponse> {
    let cfg = Arc::clone(&deps.cfg);
    let vertragd = &deps.vertragd;
    authorize(&cedar, &claims, "run-billing", &cfg.tenant)?;
    if req.tenants.is_empty() {
        return Err(BillingError::bad_request(
            "NO_PARTICIPANTS",
            "a GGV request must contain at least one participant",
        ));
    }

    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;

    let total_kwh: rust_decimal::Decimal = req.tenants.iter().map(|t| t.consumption_kwh).sum();
    if total_kwh <= rust_decimal::Decimal::ZERO {
        return Err(BillingError::unprocessable(
            "NO_CONSUMPTION",
            "total GGV consumption must be > 0 kWh",
        ));
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
            plan.validate().map_err(|e| {
                BillingError::unprocessable("NUTZUNGSPLAN_INVALID", format!("nutzungsplan: {e}"))
            })?;
            // A participant absent from the plan gets no allocation and falls
            // through to Model B below — their entire consumption billed as
            // Solar-Eigenverbrauch, with no grid residual and no Stromsteuer.
            // Refuse instead of silently mis-billing them.
            plan.validate_covers(req.tenants.iter().map(|t| t.malo_id.as_str()))
                .map_err(|e| {
                    BillingError::unprocessable(
                        "NUTZUNGSPLAN_INCOMPLETE",
                        format!("nutzungsplan: {e}"),
                    )
                })?;
            plan.allocate(pv_gen_kwh).into_iter().collect()
        } else {
            std::collections::HashMap::new()
        };

    let rates = cfg.try_regulatory_rates_for_period("SOLAR", period_from, period_to)?;

    // ── Phase 1: calculate everything, touching no transaction ────────────────
    // Resolving a tariff and a buyer per participant is a round-trip to productd
    // and vertragd each; doing that inside the write transaction would hold a
    // pool connection open across the whole fan-out. Everything below is pure
    // reads plus the engine, so a failure here has written nothing.
    struct Priced {
        malo_id: String,
        rechnungsnummer: String,
        product_code: String,
        category: &'static str,
        invoice: Invoice,
        buyer: Option<crate::clients::Rechnungsempfaenger>,
    }
    let mut priced: Vec<Priced> = Vec::with_capacity(req.tenants.len());
    // Every document this request produces — the participant records and the
    // bundle — belongs to one run, so they carry one `billingRunId`.
    let run_id = Uuid::new_v4().to_string();

    for tenant in &req.tenants {
        // The assigned product, when the participant has a contract. A § 42b
        // participant may be billed purely from the request — the community's
        // own terms — so an unassigned MaLo is not an error here.
        let assigned = resolve_tariff(
            &CalculateRequest {
                lf_mp_id: req.lf_mp_id.clone(),
                ..Default::default()
            },
            &deps,
            &tenant.malo_id,
            period_to,
        )
        .await
        .ok();
        let tariff = match assigned {
            Some(t) => t,
            // No product in productd — build a minimal Product from the request.
            None => {
                let map = serde_json::json!({
                    "category": "SOLAR",
                    "product_code": tenant.product_code,
                    "solar_arbeitspreis_ct_per_kwh": tenant.arbeitspreis_ct_per_kwh,
                    "gemeinschaft_rabatt_ct_per_kwh": tenant.gemeinschaft_rabatt_ct_per_kwh,
                    "arbeitspreis_ct_per_kwh": tenant.grid_arbeitspreis_ct_per_kwh,
                });
                serde_json::from_value::<Product>(map).map_err(|e| {
                    BillingError::unprocessable(
                        "PRODUCT_INVALID",
                        format!("MaLo {}: tariff build: {e}", tenant.malo_id),
                    )
                })?
            }
        };

        // Per-request overrides take precedence over productd product data.
        // Product is an enum — apply overrides by rebuilding the Solar/Sharing variant.
        let tariff = match tariff {
            Product::Solar(mut p) => {
                if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
                    p.solar_arbeitspreis_ct_per_kwh = Some(ap);
                }
                // solar_arbeitspreis is also used for the grid remainder in SolarProvider.
                if let Some(rabatt) = tenant.gemeinschaft_rabatt_ct_per_kwh {
                    // A discount exceeding the price it discounts turns the
                    // supply position negative — the participant would be paid
                    // for consuming. That is an input error, not a policy
                    // question, so it is refused rather than warned about.
                    if let Some(ap) = p.solar_arbeitspreis_ct_per_kwh
                        && rabatt > ap
                    {
                        return Err(BillingError::unprocessable(
                            "RABATT_EXCEEDS_ARBEITSPREIS",
                            format!(
                                "MaLo {}: gemeinschaft_rabatt {rabatt} ct/kWh exceeds the \
                                 Gebäudestrom Arbeitspreis {ap} ct/kWh — the supply position \
                                 would be negative",
                                tenant.malo_id
                            ),
                        ));
                    }
                    p.gemeinschaft_rabatt_ct_per_kwh = Some(rabatt);
                }
                Product::Solar(p)
            }
            Product::Sharing(mut p) => {
                // Stromsteuer is a *product* property, not an endpoint one: the
                // §9 Nr. 3 / §9a StromStG exemption depends on plant size and
                // spatial proximity, and `productd` carries it as the typed
                // `stromsteuer_befreiung`. Forcing it off here — and only when
                // an Arbeitspreis override happens to be present — would decide
                // a tax question from a price field.
                if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
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

        // A GGV participant's invoice is an ordinary Rechnung and takes its
        // number from the `RE` series. Deriving it from the community id, the
        // MaLo and the period would repeat exactly when the period is re-billed
        // after a Storno.
        let rechnungsnummer =
            next_rechnungsnummer(&pool, &cfg.tenant, series::INVOICE, None, period_from).await?;

        let ctx = BillingContext {
            malo_id: tenant.malo_id.clone(),
            lf_mp_id: req.lf_mp_id.clone(),
            rechnungsnummer: rechnungsnummer.clone(),
            period: BillingPeriod::new(period_from, period_to)
                .expect("parse_period guarantees from <= to"),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: rates.clone(),
            contract_id: None,
            billing_run_id: Some(run_id.clone()),
            ..Default::default()
        };
        let engine = tariff.build_engine(&GridInput::default(), &rates);
        let result = engine.bill(ctx, &quantities)?;

        priced.push(Priced {
            // A GGV Teilnehmer under §42b is a Letztverbraucher with their own
            // MaLo and supply relationship, so the ordinary BG-7 lookup applies.
            // This path drives the engine directly rather than through
            // `dispatch_invoice`, so it resolves the buyer itself.
            buyer: vertragd
                .get_vertrag_by_malo(&tenant.malo_id)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(malo_id = %tenant.malo_id, error = %e, "GGV: BG-7 buyer lookup failed");
                    None
                })
                .and_then(|v| v.rechnungsempfaenger),
            malo_id: tenant.malo_id.clone(),
            rechnungsnummer,
            product_code: tariff.product_code().unwrap_or("SOLAR_GGV").to_owned(),
            category: tariff.category_str(),
            invoice: result,
        });
    }

    // Consolidated SAMMEL document for the GGV installation — through the
    // engine, like every other invoice: derived totals, per-rate VAT over the
    // combined base, deterministic rechnungsdatum.
    let parts: Vec<(String, Invoice)> = priced
        .iter()
        .map(|t| (t.malo_id.clone(), t.invoice.clone()))
        .collect();
    let sammel_nr =
        next_rechnungsnummer(&pool, &cfg.tenant, series::CONSOLIDATED, None, period_from).await?;
    let (sammel_invoice, sammel_rechnung) = build_aggregate_invoice(
        &ggv_id,
        &req.lf_mp_id,
        sammel_nr.clone(),
        period_from,
        period_to,
        rates.clone(),
        parts,
        vec![
            zusatz_attribut("mako:ggv_id", serde_json::json!(ggv_id)),
            zusatz_attribut(
                "mako:tenant_count",
                serde_json::json!(priced.len().to_string()),
            ),
            zusatz_attribut("mako:total_kwh", serde_json::json!(total_kwh.to_string())),
            zusatz_attribut("mako:billing_run_id", serde_json::json!(run_id)),
        ],
    )?;
    let (sammel_netto, sammel_brutto) = (sammel_invoice.netto_eur, sammel_invoice.brutto_eur);

    // The bundle bills the § 42b GGV operator — a Kunde in vertragd, resolved
    // by the community id (`ggv_betreiber`), the same buyer master every other
    // e-invoice path uses. Best-effort like the per-MaLo lookups: an
    // unconfigured Betreiber ships the document with its buyer findings rather
    // than failing the billing run.
    let sammel_buyer = vertragd
        .get_ggv_betreiber(&ggv_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(%ggv_id, error = %e, "GGV: Betreiber lookup failed");
            None
        });

    // The bundle is a document the operator receives, so it is scored like any
    // other invoice.
    let assessment = assess_risk(
        &pool,
        &cfg,
        &ggv_id,
        &sammel_invoice,
        &rates,
        period_from,
        period_to,
    )
    .await;
    let held = assessment
        .as_ref()
        .is_some_and(|a| cfg.risk.hold_dispatch && a.band == crate::risk::RiskBand::Held);

    // ── Phase 2: one transaction for the whole run ────────────────────────────
    // The participant records, the consolidated document and the links between
    // them commit together. Writing the participants outside a transaction left
    // orphan invoices behind whenever a later one failed, and those orphans then
    // occupied `br_unique_original` so the retry could not succeed either.
    let mut tx = pool.begin().await?;
    let mut tenant_record_ids: Vec<Uuid> = Vec::with_capacity(priced.len());
    let mut tenant_results: Vec<serde_json::Value> = Vec::with_capacity(priced.len());
    for t in &priced {
        let record_id = insert_billing_record(
            &mut *tx,
            &crate::pg::NewBillingRecord {
                tenant: &cfg.tenant,
                malo_id: &t.malo_id,
                lf_mp_id: &req.lf_mp_id,
                product_code: &t.product_code,
                category: t.category,
                rechnungsnummer: &t.rechnungsnummer,
                period_from,
                period_to,
                rechnung_json: &t.invoice.to_rechnung_json(),
                total_netto_eur: t.invoice.netto_eur,
                total_brutto_eur: t.invoice.brutto_eur,
            },
        )
        .await;
        let record_id = match record_id {
            Ok(id) => id,
            Err(e) => return Err(period_conflict(&pool, &cfg.tenant, e).await),
        };
        crate::einvoice::store(
            &mut *tx,
            record_id,
            &t.invoice,
            &cfg,
            &t.malo_id,
            t.buyer.as_ref(),
        )
        .await?;
        tenant_results.push(serde_json::json!({
            "record_id": record_id,
            "malo_id": t.malo_id,
            "rechnungsnummer": t.rechnungsnummer,
            "netto_eur": t.invoice.netto_eur,
            "brutto_eur": t.invoice.brutto_eur,
        }));
        tenant_record_ids.push(record_id);
    }

    let sammel_id = insert_sammelrechnung_record(
        &mut *tx,
        &cfg.tenant,
        &ggv_id,
        &req.lf_mp_id,
        &sammel_nr,
        period_from,
        period_to,
        &sammel_rechnung,
        sammel_netto,
        sammel_brutto,
    )
    .await
    .map_err(crate::error::BillingError::from)?;
    if held {
        tracing::warn!(
            %sammel_id, %ggv_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: GGV bundle HELD by risk gate — issuance requires POST …/release"
        );
    } else {
        let ce = rechnung_erstellt_ce(sammel_id, &ggv_id, &req.lf_mp_id, &sammel_rechnung, false);
        issue_record(&mut tx, &cfg, sammel_id, &ce).await?;
    }
    persist_risk(&mut *tx, sammel_id, assessment.as_ref()).await?;
    crate::einvoice::store(
        &mut *tx,
        sammel_id,
        &sammel_invoice,
        &cfg,
        &ggv_id,
        sammel_buyer.as_ref(),
    )
    .await?;
    // Link the participant records to the bundle inside the same transaction:
    // the risk baseline and the record listings treat them as its children, and
    // a child that committed unlinked would be double-counted alongside the
    // SAMMEL it belongs to.
    link_to_sammelrechnung(&mut *tx, &tenant_record_ids, sammel_id).await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ggv_id": ggv_id,
            "sammel_id": sammel_id,
            "rechnungsnummer": sammel_nr,
            "billing_run_id": run_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "total_kwh": total_kwh,
            "tenant_count": tenant_results.len(),
            "total_netto_eur": sammel_netto,
            "total_brutto_eur": sammel_brutto,
            "risk": assessment,
            "held": held,
            "tenants": tenant_results,
        })),
    ))
}
