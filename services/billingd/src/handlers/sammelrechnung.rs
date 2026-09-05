//! B2B Sammelrechnung, B2G submission and UBL rendering.

use super::*;

// ── B2B Sammelrechnung ────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct SammelrechnungRequest {
    pub lf_mp_id: String,
    pub period_from: String,
    pub period_to: String,
    /// Override the consolidated document's number. Absent — the normal case —
    /// takes the next number of the tenant's `SR` series.
    #[serde(default)]
    pub rechnungsnummer: Option<String>,
}

/// One site of a Rahmenvertrag, priced and ready to be written.
struct SitePriced {
    malo_id: String,
    rechnungsnummer: String,
    product_code: String,
    category: String,
    invoice: Invoice,
    buyer: Option<crate::clients::Rechnungsempfaenger>,
}

/// `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}`
///
/// Consolidated B2B invoice for a `Rahmenvertrag` with `rechnungsstellung=SAMMEL`.
///
/// ## Pipeline
///
/// 1. Enumerate the Rahmenvertrag's active MaLos from `vertragd`.
/// 2. **Phase 1, no transaction:** price every site through the ordinary
///    calculation pipeline and resolve its BG-7 buyer. Nothing is written, so a
///    failure here leaves no trace.
/// 3. Consolidate the per-site invoices into one document — VAT recomputed once
///    over the combined base per rate.
/// 4. **Phase 2, one transaction:** the per-MaLo records, the consolidated
///    document, the links between them and the outbox event commit together.
///
/// Phase 2 is **one** transaction and not four independent writes. A site
/// failing after two of its siblings have committed would leave orphan invoices
/// occupying `br_unique_original`, so the retry cannot succeed either, and a
/// dropped link would leave the children counted alongside the bundle that
/// already contains them. The GGV endpoint has the same shape.
pub async fn post_sammelrechnung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(rahmenvertrag_id): Path<String>,
    Json(req): Json<SammelrechnungRequest>,
) -> BillingResult<impl IntoResponse> {
    let cfg = Arc::clone(&deps.cfg);
    let vertragd = &deps.vertragd;
    authorize(&cedar, &claims, "run-billing", &cfg.tenant)?;
    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;

    // Enumerate MaLos for this Rahmenvertrag.
    let sites = vertragd
        .get_rahmenvertrag_malos(&rahmenvertrag_id)
        .await
        .map_err(|e| BillingError::upstream("vertragd", e))?;
    if sites.malos.is_empty() {
        return Err(BillingError::unprocessable(
            "NO_SITES",
            format!("Rahmenvertrag {rahmenvertrag_id} has no active MaLos"),
        ));
    }

    // The document-level rates govern the bundle's own VAT pass. A Rahmenvertrag
    // is electricity in the overwhelming majority of cases; a mixed portfolio
    // keeps each site on its own commodity rates below, and `build_aggregate_invoice`
    // groups the combined base by each position's effective rate anyway.
    let doc_rates = cfg.try_regulatory_rates_for_period("STROM", period_from, period_to)?;

    // ── Phase 1: price every site, touching no transaction ────────────────────
    let mut priced: Vec<SitePriced> = Vec::with_capacity(sites.malos.len());
    let mut errors: Vec<serde_json::Value> = Vec::new();
    // One run: every per-MaLo record and the bundle share this id, so an ERP
    // can reconcile the whole Sammelrechnung against what it received.
    let run_id = Uuid::new_v4().to_string();

    for entry in &sites.malos {
        let site_req = CalculateRequest {
            lf_mp_id: req.lf_mp_id.clone(),
            period_from: req.period_from.clone(),
            period_to: req.period_to.clone(),
            ..Default::default()
        };

        // Each site is billed under **its own** product assignments and, per
        // leg, its own statutory rates. One product resolved as of the period's
        // last day charges a site that switched tariff mid-period the new price
        // for the whole of it, and one STROM rate set for the whole
        // Rahmenvertrag bills a gas site without its Energiesteuer/BEHG year
        // table and skips the §28 Abs. 5/6 UStG straddle refusal that only gas
        // and Fernwärme have.
        let legs = match resolve_legs(
            &site_req,
            deps.as_ref(),
            &entry.malo_id,
            period_from,
            period_to,
        )
        .await
        {
            Ok(l) => l,
            Err(e) => {
                errors.push(site_error(&entry.malo_id, &e));
                continue;
            }
        };
        let summary = LegSummary::of(&legs);

        // Each line of a Sammelrechnung is an invoice in its own right and takes
        // its own number from the `RE` series.
        let malo_nr =
            next_rechnungsnummer(&pool, &cfg.tenant, series::INVOICE, None, period_from).await?;

        let billed = match dispatch_invoice_multi(
            &deps,
            &legs,
            &site_req,
            &entry.malo_id,
            &malo_nr,
            RunId(Some(&run_id)),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                errors.push(site_error(&entry.malo_id, &e));
                continue;
            }
        };

        priced.push(SitePriced {
            // Each per-MaLo line of a Sammelrechnung bills that site's own supply
            // customer, so the buyer that came back with the priced invoice is
            // the right one.
            buyer: billed.buyer,
            malo_id: entry.malo_id.clone(),
            rechnungsnummer: malo_nr,
            product_code: summary.product_code,
            category: summary.category,
            invoice: billed.invoice,
        });
    }

    if priced.is_empty() {
        return Err(BillingError::unprocessable_with(
            "ALL_SITES_FAILED",
            "every MaLo calculation of this Rahmenvertrag failed",
            serde_json::json!({ "errors": errors }),
        ));
    }

    // Consolidated Sammelrechnung — through the engine: per-rate VAT over the
    // combined base, derived totals, deterministic rechnungsdatum. The per-MaLo
    // runs stay stored as calculation records linked below.
    let sammel_nr = next_rechnungsnummer(
        &pool,
        &cfg.tenant,
        series::CONSOLIDATED,
        req.rechnungsnummer.as_deref(),
        period_from,
    )
    .await?;
    let parts: Vec<(String, Invoice)> = priced
        .iter()
        .map(|p| (p.malo_id.clone(), p.invoice.clone()))
        .collect();
    let malos_count = parts.len();
    let (sammel_invoice, sammel_json) = build_aggregate_invoice(
        &rahmenvertrag_id,
        &req.lf_mp_id,
        sammel_nr.clone(),
        period_from,
        period_to,
        doc_rates.clone(),
        parts,
        vec![
            zusatz_attribut("mako:rahmenvertrag_id", serde_json::json!(rahmenvertrag_id)),
            zusatz_attribut(
                "mako:malos_count",
                serde_json::json!(malos_count.to_string()),
            ),
            zusatz_attribut("mako:billing_run_id", serde_json::json!(run_id)),
        ],
    )?;
    let (total_netto, total_brutto) = (sammel_invoice.netto_eur, sammel_invoice.brutto_eur);

    // The bundle is the document the counterparty receives, so it goes through
    // the same risk gate as any other invoice. Scoring only the standalone
    // paths meant the largest documents this service produces were the ones
    // nobody reviewed.
    let assessment = assess_risk(
        &pool,
        &cfg,
        &rahmenvertrag_id,
        &sammel_invoice,
        &doc_rates,
        period_from,
        period_to,
    )
    .await;
    let held = assessment
        .as_ref()
        .is_some_and(|a| cfg.risk.hold_dispatch && a.band == crate::risk::RiskBand::Held);

    // ── Phase 2: one transaction for the whole run ────────────────────────────
    let mut tx = pool.begin().await?;
    let mut per_malo_ids: Vec<Uuid> = Vec::with_capacity(priced.len());
    for site in &priced {
        let record_id = insert_billing_record(
            &mut *tx,
            &crate::pg::NewBillingRecord {
                tenant: &cfg.tenant,
                malo_id: &site.malo_id,
                lf_mp_id: &req.lf_mp_id,
                product_code: &site.product_code,
                category: &site.category,
                rechnungsnummer: &site.rechnungsnummer,
                period_from,
                period_to,
                rechnung_json: &site.invoice.to_rechnung_json(),
                total_netto_eur: site.invoice.netto_eur,
                total_brutto_eur: site.invoice.brutto_eur,
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
            &site.invoice,
            &cfg,
            &site.malo_id,
            site.buyer.as_ref(),
        )
        .await?;
        per_malo_ids.push(record_id);
    }

    let sammel_id = insert_sammelrechnung_record(
        &mut *tx,
        &cfg.tenant,
        &rahmenvertrag_id,
        &req.lf_mp_id,
        &sammel_nr,
        period_from,
        period_to,
        &sammel_json,
        total_netto,
        total_brutto,
    )
    .await
    .map_err(crate::error::BillingError::from)?;
    if held {
        tracing::warn!(
            %sammel_id, %rahmenvertrag_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: Sammelrechnung HELD by risk gate — issuance requires POST …/release"
        );
    } else {
        let ce = rechnung_erstellt_ce(
            sammel_id,
            &rahmenvertrag_id,
            &req.lf_mp_id,
            &sammel_json,
            false,
        );
        issue_record(&mut tx, &cfg, sammel_id, &ce).await?;
    }
    persist_risk(&mut *tx, sammel_id, assessment.as_ref()).await?;
    crate::einvoice::store(
        &mut *tx,
        sammel_id,
        &sammel_invoice,
        &cfg,
        &rahmenvertrag_id,
        // The bundled document bills the **Rahmenvertrag holder**, not any one
        // site's supply customer, so this is vertragd's holder projection rather
        // than the per-MaLo one used for the lines above.
        sites.rechnungsempfaenger.as_ref(),
    )
    .await?;
    // Inside the transaction: the risk baseline and the record listings treat
    // the per-MaLo rows as the bundle's children, and a child that committed
    // unlinked is counted alongside the bundle that already contains it.
    link_to_sammelrechnung(&mut *tx, &per_malo_ids, sammel_id).await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "sammelrechnung_id": sammel_id,
            "rahmenvertrag_id": rahmenvertrag_id,
            "billing_run_id": run_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "malos_billed": per_malo_ids.len(),
            "total_netto_eur": total_netto,
            "total_brutto_eur": total_brutto,
            "risk": assessment,
            "held": held,
            "errors": errors,
            "rechnungsnummer": sammel_nr,
        })),
    ))
}

/// One site's failure, as a structured entry in the run report.
///
/// Coded, not `"{malo}: {prose}"`: a caller distinguishes a missing product
/// from an unreachable productd without reading German.
fn site_error(malo_id: &str, e: &BillingError) -> serde_json::Value {
    serde_json::json!({
        "malo_id": malo_id,
        "code": e.code(),
        "message": e.to_string(),
    })
}

// ── XRechnung B2G submission pipeline ────────────────────────────────────────

/// Request body for `POST /api/v1/billing/{id}/submit-b2g`.
#[derive(Debug, serde::Deserialize)]
pub struct SubmitB2gRequest {
    /// Target portal identifier: `"ZRE"` (Zentraler Rechnungseingang) or `"OZG-RE"`.
    /// Defaults to `"ZRE"`.
    pub portal: Option<String>,
    /// BT-10 Leitweg-ID of the receiving public authority. Required for a ZRE/
    /// OZG-RE submission (`BR-DE-15`), so `reference` is what the portal routes on.
    pub reference: Option<String>,
    /// BG-7 buyer (the public authority) — supplied per submission because the
    /// recipient and its Rechnungsadresse are known to the caller, not the billing
    /// engine. Completes the model's placeholder buyer so the document is
    /// XRechnung-conformant.
    #[serde(default)]
    pub buyer: Option<crate::einvoice::B2gBuyer>,
}

/// `POST /api/v1/billing/{id}/submit-b2g`
///
/// Prepare an XRechnung 3.0 CII XML from the billing record and notify the
/// configured ERP webhook so the ERP's PEPPOL AS4 gateway can transmit it
/// to the ZRE / OZG-RE portal.
///
/// ## Why not send directly?
///
/// PEPPOL AS4 transport requires an accredited access-point operator
/// (Peppol AP) and a registered Peppol participant ID.  These are ERP /
/// platform operator responsibilities.  `billingd` generates the
/// EN 16931-conformant XML and hands it to the ERP via CloudEvent;
/// the ERP's AS4 gateway performs the actual network submission.
///
/// ## Regulatory
///
/// B2G e-invoicing to federal contracting authorities has been **mandatory
/// since 27.11.2020** — § 4a EGovG plus the E-Rechnungsverordnung (ERechV,
/// in force 27.11.2018), which transpose EU Directive 2014/55/EU; direct
/// orders up to EUR 1 000 are exempt (§ 3 Abs. 3 ERechV). The 2027/2028 dates
/// belong to the separate **B2B** mandate in § 14 UStG and have nothing to do
/// with this endpoint.
pub async fn post_submit_b2g(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
    Json(req): Json<SubmitB2gRequest>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "submit-b2g", &cfg.tenant)?;
    // Nothing can transmit the document without an ERP, so say so before doing
    // the work rather than after.
    if cfg.erp_webhook_url.is_none() {
        return Err(BillingError::Unavailable {
            code: "NO_ERP_WEBHOOK",
            message: "no erp_webhook_url configured — nothing can transmit the XRechnung"
                .to_owned(),
        });
    }
    let row = fetch_billing_record(&pool, &cfg.tenant, id)
        .await?
        .ok_or_else(|| BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"))?;

    // Render from the stored EN 16931 model and refuse to submit a document that
    // violates the rules — a B2G portal would reject it anyway.
    let model = row
        .en16931_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
        .ok_or_else(|| {
            BillingError::unprocessable(
                "MODEL_MISSING",
                "record has no EN 16931 model — re-run the billing calculation",
            )
        })?;
    // Complete the buyer (the receiving authority, known to the caller) and stamp
    // its Leitweg-ID (BT-10).
    let model = match &req.buyer {
        Some(b) => crate::einvoice::apply_b2g_buyer(model, b),
        None => model,
    };
    let model = match req.reference.as_deref() {
        Some(leitweg) => crate::einvoice::with_buyer_reference(model, leitweg),
        None => model,
    };
    // Validate against the XRechnung 3.0 profile and render in one step — a B2G
    // submission must be profile-valid or the ZRE/OZG-RE portal rejects it. On
    // failure, report exactly what is missing (usually the buyer BG-7 terms).
    let xml = crate::einvoice::render_xrechnung_cii(&model).map_err(|rules| {
        BillingError::unprocessable_with(
            "XRECHNUNG_NOT_CONFORMANT",
            "XRechnung 3.0 conformance failed — not submitting",
            serde_json::json!({
                "violated_rules": rules,
                "buyer_gaps": crate::einvoice::buyer_gaps(&model),
                "hint": "supply the recipient in `buyer` (name, address, contact) and \
                         `reference` (Leitweg-ID)",
            }),
        )
    })?;

    let portal = req.portal.as_deref().unwrap_or("ZRE");

    // Hand the XML to the ERP via the transactional outbox — the ERP's PEPPOL AS4
    // gateway transmits it. Posting the CloudEvent inline lost the submission
    // whenever the ERP was briefly down, while the caller was told "submitted";
    // the outbox row commits first and the dispatcher retries until it lands.
    let ce = mako_service::CloudEvent::new(
        mako_service::source("billingd", &cfg.tenant),
        mako_events::billing::XRECHNUNG_B2G_READY,
        id.to_string(),
        serde_json::json!({
            "billing_record_id": id,
            "malo_id": row.malo_id,
            "lf_mp_id": row.lf_mp_id,
            "portal": portal,
            "reference": req.reference,
            "xrechnung_xml": xml,
            "standard": "XRechnung 3.0 (EN 16931 CIUS)",
            "regulatory": "§4a EGovG i.V.m. ERechV — B2G e-invoicing mandatory since 27.11.2020",
        }),
    );
    let mut tx = pool.begin().await?;
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
    tx.commit().await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "billing_record_id": id,
            "portal": portal,
            "status": "queued",
            "message": "de.billing.xrechnung.b2g.ready enqueued in the outbox for the ERP webhook",
            "note": "ERP PEPPOL AS4 gateway is responsible for actual transmission to ZRE/OZG-RE",
            "regulatory": "§4a EGovG i.V.m. ERechV: B2G e-invoicing mandatory since 27.11.2020",
            "conformance": "XRechnung 3.0 (validated before dispatch)",
        })),
    ))
}

// ── PEPPOL BIS Billing 3.0 UBL export ────────────────────────────────────────

/// `GET /api/v1/billing/{id}/ubl`
///
/// Render the stored EN 16931 model as PEPPOL BIS Billing 3.0 (UBL 2.1).
///
/// UBL and CII are the **two permitted syntaxes** of EN 16931, not a hierarchy:
/// § 14 UStG requires conformance to the norm and accepts either, and so does
/// Directive 2014/55/EU. CII is what German receivers overwhelmingly expect
/// (XRechnung, ZUGFeRD); UBL is what a cross-border PEPPOL receiver usually
/// wants, which is the reason both endpoints exist.
pub async fn get_ubl(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    let row = fetch_billing_record(&pool, &cfg.tenant, id)
        .await?
        .ok_or_else(|| BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"))?;
    let model = row
        .en16931_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
        .ok_or_else(|| {
            BillingError::unprocessable(
                "MODEL_MISSING",
                "record has no EN 16931 model — re-run the billing calculation",
            )
        })?;
    Ok(super::records::xml_download(
        crate::einvoice::render_ubl(&model),
        format!("peppol-bis-{id}.xml"),
    ))
}
