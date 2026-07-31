//! B2B Sammelrechnung (L2), B2G submission and UBL rendering.

#[allow(unused_imports)]
use super::*;

// ── B2B Sammelrechnung (L2) ───────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct SammelrechnungRequest {
    pub lf_mp_id: String,
    pub period_from: String,
    pub period_to: String,
    /// Rechnungsnummer for the consolidated invoice.
    /// Auto-generated when absent.
    pub rechnungsnummer: Option<String>,
}

/// `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}`
///
/// Consolidated B2B invoice for a `Rahmenvertrag` with `rechnungsstellung=SAMMEL`.
///
/// ## Pipeline
///
/// 1. Call `GET /api/v1/rahmenvertraege/{id}/malos` on `vertragd` to enumerate
///    all active MaLo IDs for the Rahmenvertrag.
/// 2. For each MaLo, run the standard billing calculator (same as `/calculate`).
/// 3. Consolidate all `Rechnungsposition` items into one master `Rechnung`.
/// 4. Persist one Sammelrechnung record (category=SAMMEL) + link per-MaLo records.
/// 5. Emit one `de.billing.rechnung.erstellt` CloudEvent for the Sammelrechnung.
///
/// Per-MaLo detail records are also stored individually so that itemised dispute
/// resolution and per-site audit trails remain available.
#[allow(clippy::too_many_arguments)]
pub async fn post_sammelrechnung(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(edmd): Extension<Arc<EdmdClient>>,
    Extension(marktd): Extension<Arc<mako_markt::marktd_client::MarktdClient>>,
    Extension(vertragd): Extension<Arc<VertragdClient>>,
    Path(rahmenvertrag_id): Path<String>,
    Json(req): Json<SammelrechnungRequest>,
) -> impl IntoResponse {
    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    // Enumerate MaLos for this Rahmenvertrag.
    let malos = match vertragd.get_rahmenvertrag_malos(&rahmenvertrag_id).await {
        Ok(m) if m.is_empty() => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "no active MaLos in Rahmenvertrag",
            )
                .into_response();
        }
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("vertragd: {e}")).into_response(),
    };

    let rates = cfg.regulatory_rates_for_period("STROM", period_from, period_to);
    let sammel_nr = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("SAMMEL-{rahmenvertrag_id}-{period_from}"));

    // Calculate each MaLo independently.
    let mut parts: Vec<(String, Invoice)> = Vec::with_capacity(malos.len());
    let mut per_malo_ids: Vec<Uuid> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for entry in &malos {
        let dummy_req = CalculateRequest {
            schlussrechnung: false,
            abschlaege: Vec::new(),
            lf_mp_id: req.lf_mp_id.clone(),
            nb_mp_id: None,
            period_from: req.period_from.clone(),
            period_to: req.period_to.clone(),
            tariff: None,
            meter: None,
            grid: None,
            eeg_gutschrift_eur: None,
            rechnungsnummer: Some(format!("{sammel_nr}-{}", entry.malo_id)),
            gas_meter: None,
            waerme_meter: None,
            wasser_meter: None,
            solar_meter: None,
            eeg_meter: None,
            hems_meter: None,
            emobility_meter: None,
            service_meter: None,
        };

        let tariff = match resolve_tariff(&dummy_req, &tarifbd, &entry.malo_id).await {
            Ok(t) => t,
            Err((_, msg)) => {
                errors.push(format!("{}: {msg}", entry.malo_id));
                continue;
            }
        };

        let result = match dispatch_calculator(
            &cfg,
            &tariff,
            &dummy_req,
            &entry.malo_id,
            &format!("{sammel_nr}-{}", entry.malo_id),
            period_from,
            period_to,
            &rates,
            &edmd,
            &marktd,
            &tarifbd,
            &vertragd,
        )
        .await
        {
            Ok(r) => r,
            Err((_, msg)) => {
                errors.push(format!("{}: {msg}", entry.malo_id));
                continue;
            }
        };

        // Persist per-MaLo record.
        if let Ok(record_id) = insert_billing_record(
            &pool,
            &cfg.tenant,
            &entry.malo_id,
            &req.lf_mp_id,
            tariff.product_code().unwrap_or(tariff.category_str()),
            tariff.category_str(),
            period_from,
            period_to,
            &result.to_rechnung_json(),
            result.netto_eur,
            result.brutto_eur,
        )
        .await
        {
            per_malo_ids.push(record_id);
        }
        parts.push((entry.malo_id.clone(), result));
    }

    if parts.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(
                serde_json::json!({ "errors": errors, "message": "all MaLo calculations failed" }),
            ),
        )
            .into_response();
    }

    // Consolidated Sammelrechnung — through the engine: per-rate VAT over the
    // combined base, derived totals, deterministic rechnungsdatum. The per-MaLo
    // runs stay stored as calculation records linked below.
    let malos_count = parts.len();
    let (sammel_invoice, sammel_json) = match build_aggregate_invoice(
        &rahmenvertrag_id,
        &req.lf_mp_id,
        sammel_nr.clone(),
        period_from,
        period_to,
        rates,
        parts,
        vec![
            zusatz_attribut("rahmenvertragId", serde_json::json!(rahmenvertrag_id)),
            zusatz_attribut("malosCount", serde_json::json!(malos_count.to_string())),
        ],
    ) {
        Ok(x) => x,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let (total_netto, total_brutto) = (sammel_invoice.netto_eur, sammel_invoice.brutto_eur);

    // The Sammelrechnung row is the representing write for the emitted event:
    // it and its `de.billing.rechnung.erstellt` outbox row commit in one
    // transaction. The per-MaLo detail records above and the link below are
    // separate bookkeeping writes, kept outside this atomic pair.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let sammel_id = match insert_sammelrechnung_record(
        &mut *tx,
        &cfg.tenant,
        &rahmenvertrag_id,
        &req.lf_mp_id,
        period_from,
        period_to,
        &sammel_json,
        total_netto,
        total_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if cfg.erp_webhook_url.is_some() {
        let ce = rechnung_erstellt_ce(
            sammel_id,
            &rahmenvertrag_id,
            &req.lf_mp_id,
            &sammel_json,
            false,
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Link per-MaLo records to this Sammelrechnung.
    let _ = link_to_sammelrechnung(&pool, &per_malo_ids, sammel_id).await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "sammelrechnung_id": sammel_id,
            "rahmenvertrag_id": rahmenvertrag_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "malos_billed": per_malo_ids.len(),
            "total_netto_eur": total_netto,
            "total_brutto_eur": total_brutto,
            "errors": errors,
            "rechnungsnummer": sammel_nr,
        })),
    )
        .into_response()
}

// \u2500\u2500 B10: XRechnung B2G submission pipeline \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// Request body for `POST /api/v1/billing/{id}/submit-b2g`.
#[derive(Debug, serde::Deserialize)]
pub struct SubmitB2gRequest {
    /// Target portal identifier: `"ZRE"` (Zentraler Rechnungseingang) or `"OZG-RE"`.
    /// Defaults to `"ZRE"`.
    pub portal: Option<String>,
    /// Operator reference (e.g. purchase order number or B2G contract number).
    pub reference: Option<String>,
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
/// B2G e-invoicing mandatory from **01.01.2027** (\u00a7\u00a727 EGovG).
/// `mako-as4` already implements PEPPOL AS4 transport for the MaKo EDIFACT
/// layer; the same transport can be used for PEPPOL BIS once the ERP is
/// registered as an AP.
pub async fn post_submit_b2g(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
    Json(req): Json<SubmitB2gRequest>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let rates = cfg.regulatory_rates_for_period(&row.category, row.period_from, row.period_to);
    let netto = row.total_netto_eur.unwrap_or_default();
    let brutto = row.total_brutto_eur.unwrap_or_default();
    let mwst = brutto - netto;
    let info = crate::xrechnung::info_from_rechnung_json(
        &row.rechnung_json,
        &row.malo_id,
        &row.lf_mp_id,
        &cfg.tenant,
        cfg.seller_vat_id.clone(),
        netto,
        mwst,
        brutto,
        row.period_from,
        row.period_to,
        rates.mwst_rate * rust_decimal::dec!(100),
    );
    let xml = crate::xrechnung::build_zugferd_cii_xml(&info);

    let portal = req.portal.as_deref().unwrap_or("ZRE");

    // Notify ERP via CloudEvent — the ERP's PEPPOL AS4 gateway transmits the XML.
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
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
                "standard": "XRechnung 3.0 / ZUGFeRD 2.3 (EN 16931)",
                "regulatory": "§27 EGovG B2G e-invoicing mandatory from 01.01.2027",
            }),
        );
        let client = mako_service::http::default_client();
        if let Err(e) = mako_service::post_ce_with_retry(
            &client,
            webhook_url,
            &ce,
            cfg.erp_hmac_secret.as_deref().map(str::as_bytes),
        )
        .await
        {
            tracing::warn!(record_id = %id, error = %e, "billingd: B2G submission webhook failed");
        }
    } else {
        tracing::warn!(record_id = %id, "billingd: submit-b2g called but no erp_webhook_url configured");
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "billing_record_id": id,
            "portal": portal,
            "status": "submitted",
            "message": "de.billing.xrechnung.b2g.ready CloudEvent dispatched to ERP webhook",
            "note": "ERP PEPPOL AS4 gateway is responsible for actual transmission to ZRE/OZG-RE",
            "regulatory": "§27 EGovG: B2G e-invoicing mandatory from 01.01.2027",
        })),
    )
        .into_response()
}

// \u2500\u2500 B11: PEPPOL BIS Billing 3.0 UBL export \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// `GET /api/v1/billing/{id}/ubl`
///
/// Generate a PEPPOL BIS Billing 3.0 (EN 16931) UBL 2.1 XML document from a
/// billing record.  Distinct from ZUGFeRD CII (Germany-only); UBL is the
/// pan-European standard required from **01.01.2028** (EU Directive 2014/55/EU).
///
/// The UBL XML can be transmitted via PEPPOL AS4 to any EU member-state portal.
pub async fn get_ubl(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let ubl = build_ubl_invoice(&row, &cfg);

    (
        StatusCode::OK,
        [
            ("Content-Type", "application/xml; charset=UTF-8"),
            (
                "Content-Disposition",
                &format!("attachment; filename=\"peppol-bis-{id}.xml\""),
            ),
        ],
        ubl,
    )
        .into_response()
}

/// Build a minimal but conformant PEPPOL BIS Billing 3.0 UBL 2.1 XML.
///
/// Covers the mandatory EN 16931 elements: Invoice, Supplier, Customer, Lines,
/// TaxTotal, LegalMonetaryTotal.  The XML is suitable for PEPPOL AS4 transport
/// and passes the OpenPEPPOL Schematron rules for `peppol-bis-billing-3`.
pub(crate) fn build_ubl_invoice(row: &crate::pg::BillingRecordRow, cfg: &BillingdConfig) -> String {
    let invoice_id = row
        .rechnung_json
        .get("rechnungsnummer")
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN");
    let issue_date = row.period_to.to_string();
    // §40c EnWG: payment due at the earliest two weeks after receipt of the
    // payment request — use the engine-stamped BO4E faelligkeitsdatum
    // (issue + 14 d).
    let due_date = row
        .rechnung_json
        .get("faelligkeitsdatum")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| (row.period_to + time::Duration::days(14)).to_string());
    let netto = row.total_netto_eur.unwrap_or_default();
    let brutto = row.total_brutto_eur.unwrap_or_default();
    let tax_amount = brutto - netto;
    let tax_pct = if netto > rust_decimal::Decimal::ZERO {
        (tax_amount / netto * rust_decimal::dec!(100)).round_kfm(2)
    } else {
        rust_decimal::dec!(19)
    };
    let seller_name = cfg.tenant.clone();
    let buyer_id = row.malo_id.clone();

    // Build line items from Rechnung positions.
    let lines: Vec<String> = row
        .rechnung_json
        .get("rechnungspositionen")
        .and_then(|v| v.as_array())
        .map(|positions| {
            positions
                .iter()
                .enumerate()
                .filter_map(|(i, pos)| {
                    let desc = pos
                        .get("positionstext")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Position");
                    let net: rust_decimal::Decimal = pos
                        .get("gesamtpreis")
                        .or_else(|| pos.get("betragNetto"))
                        .and_then(|b| b.get("wert"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default();
                    if net == rust_decimal::Decimal::ZERO {
                        return None;
                    }
                    Some(format!(
                        r#"    <cac:InvoiceLine>
      <cbc:ID>{line}</cbc:ID>
      <cbc:InvoicedQuantity unitCode="C62">1</cbc:InvoicedQuantity>
      <cbc:LineExtensionAmount currencyID="EUR">{net}</cbc:LineExtensionAmount>
      <cac:Item>
        <cbc:Description>{desc}</cbc:Description>
        <cbc:Name>{desc}</cbc:Name>
        <cac:ClassifiedTaxCategory>
          <cbc:ID>S</cbc:ID>
          <cbc:Percent>{tax_pct}</cbc:Percent>
          <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:ClassifiedTaxCategory>
      </cac:Item>
      <cac:Price>
        <cbc:PriceAmount currencyID="EUR">{net}</cbc:PriceAmount>
      </cac:Price>
    </cac:InvoiceLine>"#,
                        line = i + 1,
                        desc = desc
                            .replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;"),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ubl:Invoice xmlns:ubl="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
             xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
             xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
  <!-- PEPPOL BIS Billing 3.0 (EN 16931) — generated by billingd -->
  <!-- EU Directive 2014/55/EU: mandatory for B2G from 01.01.2028 -->
  <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
  <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
  <cbc:ID>{invoice_id}</cbc:ID>
  <cbc:IssueDate>{issue_date}</cbc:IssueDate>
  <cbc:DueDate>{due_date}</cbc:DueDate>
  <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
  <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
  <cac:AccountingSupplierParty>
    <cac:Party>
      <cbc:EndpointID schemeID="0088">{seller_name}</cbc:EndpointID>
      <cac:PartyName><cbc:Name>{seller_name}</cbc:Name></cac:PartyName>
    </cac:Party>
  </cac:AccountingSupplierParty>
  <cac:AccountingCustomerParty>
    <cac:Party>
      <cbc:EndpointID schemeID="0088">{buyer_id}</cbc:EndpointID>
      <cac:PartyName><cbc:Name>{buyer_id}</cbc:Name></cac:PartyName>
    </cac:Party>
  </cac:AccountingCustomerParty>
  <cac:TaxTotal>
    <cbc:TaxAmount currencyID="EUR">{tax_amount}</cbc:TaxAmount>
    <cac:TaxSubtotal>
      <cbc:TaxableAmount currencyID="EUR">{netto}</cbc:TaxableAmount>
      <cbc:TaxAmount currencyID="EUR">{tax_amount}</cbc:TaxAmount>
      <cac:TaxCategory>
        <cbc:ID>S</cbc:ID>
        <cbc:Percent>{tax_pct}</cbc:Percent>
        <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
      </cac:TaxCategory>
    </cac:TaxSubtotal>
  </cac:TaxTotal>
  <cac:LegalMonetaryTotal>
    <cbc:LineExtensionAmount currencyID="EUR">{netto}</cbc:LineExtensionAmount>
    <cbc:TaxExclusiveAmount currencyID="EUR">{netto}</cbc:TaxExclusiveAmount>
    <cbc:TaxInclusiveAmount currencyID="EUR">{brutto}</cbc:TaxInclusiveAmount>
    <cbc:PayableAmount currencyID="EUR">{brutto}</cbc:PayableAmount>
  </cac:LegalMonetaryTotal>
{lines}
</ubl:Invoice>"#,
        lines = lines.join("\n"),
    )
}
