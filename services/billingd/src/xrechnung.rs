//! XRechnung / ZUGFeRD 2.3 structured invoice generation for `billingd`.
//!
#![allow(clippy::too_many_arguments)]
//! Generates **Cross Industry Invoice (CII)** XML conforming to:
//! - **EN16931:2017** (European e-invoicing standard)
//! - **XRechnung 3.0** (German CIUS — `urn:cen.eu:en16931:2017#compliant#urn:xoev-de:kosit:standard:xrechnung_3.0`)
//! - **ZUGFeRD 2.3 Extended** for embedding in PDF/A-3 (Factur-X)
//!
//! ## Legal mandate
//!
//! - **B2G (business-to-government):** mandatory from **01.01.2027** per §§27 EGovG,
//!   4 E-Rechnungsverordnung (EU Directive 2014/55/EU transposed).
//! - EEG plant operators who are municipalities or public-law bodies require XRechnung
//!   for all incoming service invoices.
//! - B2B: mandatory from **01.01.2028** per §14 UStG n.F. (E-Rechnungspflicht).
//!
//! ## Format profile identifier
//!
//! `urn:cen.eu:en16931:2017#compliant#urn:xoev-de:kosit:standard:xrechnung_3.0`
//!
//! ## Architecture
//!
//! The `build_zugferd_cii_xml()` function is **pure** (no I/O). It takes
//! `XRechnungInfo` — a flat DTO assembled from the stored `billing_records.rechnung_json`
//! — and returns a well-formed XML `String`.

use rust_decimal::Decimal;

// ── Input types ───────────────────────────────────────────────────────────────

/// Flat DTO for XRechnung/ZUGFeRD XML generation.
///
/// Assembled by the handler from `billing_records.rechnung_json` + config.
#[derive(Debug, Clone)]
pub struct XRechnungInfo {
    /// BT-1 Invoice identifier.
    pub invoice_number: String,
    /// BT-2 Invoice issue date.
    pub issue_date: time::Date,
    /// BT-9 Payment due date (optional).
    pub due_date: Option<time::Date>,
    /// Billing period start (BT-73 / BillingSpecifiedPeriod).
    pub period_from: time::Date,
    /// Billing period end (BT-74).
    pub period_to: time::Date,

    /// BG-4 Seller — MP-ID (BDEW-Codenummer `99…`).
    pub seller_mp_id: String,
    /// BG-4 Seller — human-readable name.
    pub seller_name: String,
    /// BT-31 Seller VAT registration number (Umsatzsteuer-ID, e.g. `DE123456789`).
    pub seller_vat_id: Option<String>,
    /// Seller street + city, used in BG-5.
    pub seller_address: Option<String>,

    /// BG-7 Buyer — MP-ID or customer account number.
    pub buyer_id: String,
    /// BG-7 Buyer — human-readable name.
    pub buyer_name: String,

    /// MaLo reference (BT-22 note / BT-10 buyer reference).
    pub malo_id: String,

    /// Invoice lines.
    pub positions: Vec<XRechnungPosition>,

    /// BT-109 Invoice total amount exclusive of VAT.
    pub netto_eur: Decimal,
    /// BT-111 Invoice total VAT amount.
    pub mwst_eur: Decimal,
    /// EN16931 BG-23 VAT breakdown, one entry per category and rate.
    ///
    /// Empty falls back to a single standard-rate block over the whole net.
    pub tax_subtotals: Vec<energy_billing::invoice::TaxSubtotal>,
    /// BT-112 Invoice total amount inclusive of VAT.
    pub brutto_eur: Decimal,

    /// Standard VAT rate applied (e.g. `19` for Germany 19%).
    pub vat_rate_pct: Decimal,
}

/// One CII supply chain trade line item.
#[derive(Debug, Clone)]
pub struct XRechnungPosition {
    /// BT-126 Invoice line identifier.
    pub id: u32,
    /// BT-153 Item name.
    pub description: String,
    /// BT-129 Billed quantity.
    pub quantity: Decimal,
    /// BT-130 Unit of measure code (UN/ECE rec20).
    pub unit_code: String,
    /// BT-146 Item net price per unit.
    pub net_price_eur: Decimal,
    /// BT-131 Invoice line net amount.
    pub net_total_eur: Decimal,
    /// BT-152 Item VAT category code (`S` = standard, `Z` = zero, `E` = exempt).
    pub vat_category: String,
    /// BT-152 applicable VAT rate.
    pub vat_rate_pct: Decimal,
}

// ── Unit code mapping ─────────────────────────────────────────────────────────

/// Map billingd unit strings to UN/ECE Rec 20 codes used in CII.
fn unit_to_unece(unit: &str) -> &str {
    match unit.to_lowercase().as_str() {
        "kwh" => "KWH", // kilowatt hour
        "kw" => "KWT",  // kilowatt
        "tage" | "day" | "days" => "DAY",
        "monat" | "monate" | "month" | "months" => "MON",
        "ereignis" | "event" | "events" => "C62", // one (dimensionless unit)
        "gb" => "E34",                            // gigabyte
        "minuten" | "min" | "minutes" => "MIN",
        "pauschal" | "flat" => "C62",
        _ => "C62",
    }
}

// ── XML builder ───────────────────────────────────────────────────────────────

/// Generate ZUGFeRD 2.3 / XRechnung 3.0 compliant CII XML.
///
/// The returned `String` is UTF-8 encoded, starts with an XML declaration,
/// and can be embedded in a PDF/A-3 as `/EmbeddedFiles/factur-x.xml` or served
/// as `Content-Type: application/xml` on `GET /api/v1/billing/{id}/xrechnung`.
///
/// # Standard conformance
///
/// - CIUS: XRechnung 3.0 (`urn:cen.eu:en16931:2017#compliant#urn:xoev-de:kosit:standard:xrechnung_3.0`)
/// - Namespaces: rsm, ram, udt (CII D16B)
/// - TypeCode: `380` (commercial invoice)
/// - DocumentCurrencyCode: `EUR`
/// - VAT: single-rate (all positions at `info.vat_rate_pct`); extend for mixed-rate
pub fn build_zugferd_cii_xml(info: &XRechnungInfo) -> String {
    let issue_date_fmt = format_date_yyyymmdd(info.issue_date);
    let period_from_fmt = format_date_yyyymmdd(info.period_from);
    let period_to_fmt = format_date_yyyymmdd(info.period_to);
    let due_date_fmt = info
        .due_date
        .map(format_date_yyyymmdd)
        .unwrap_or_else(|| format_date_yyyymmdd(info.issue_date));

    let _vat_rate = info.vat_rate_pct.round_dp(2);

    // Build line items.
    let lines: String = info
        .positions
        .iter()
        .map(build_line_item)
        .collect::<Vec<_>>()
        .join("\n");

    // Seller VAT element (optional).
    let seller_vat_xml = match &info.seller_vat_id {
        Some(vat) => format!(
            r#"<ram:SpecifiedTaxRegistration>
          <ram:ID schemeID="VA">{}</ram:ID>
        </ram:SpecifiedTaxRegistration>"#,
            xml_escape(vat)
        ),
        None => String::new(),
    };

    // Seller address (optional, recommended for EN16931).
    let seller_addr_xml = match &info.seller_address {
        Some(addr) => format!(
            r#"<ram:PostalTradeAddress>
          <ram:LineOne>{}</ram:LineOne>
          <ram:CountryID>DE</ram:CountryID>
        </ram:PostalTradeAddress>"#,
            xml_escape(addr)
        ),
        None => r#"<ram:PostalTradeAddress>
          <ram:CountryID>DE</ram:CountryID>
        </ram:PostalTradeAddress>"#
            .to_owned(),
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Generated by mako billingd — XRechnung 3.0 / ZUGFeRD 2.3 Extended CII -->
<rsm:CrossIndustryInvoice
  xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100"
  xmlns:ram="urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100"
  xmlns:udt="urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100"
  xmlns:qdt="urn:un:unece:uncefact:data:standard:QualifiedDataType:100">

  <!-- BT-24 Specification identifier (XRechnung 3.0 CIUS) -->
  <rsm:ExchangedDocumentContext>
    <ram:GuidelineSpecifiedDocumentContextParameter>
      <ram:ID>urn:cen.eu:en16931:2017#compliant#urn:xoev-de:kosit:standard:xrechnung_3.0</ram:ID>
    </ram:GuidelineSpecifiedDocumentContextParameter>
  </rsm:ExchangedDocumentContext>

  <!-- BT-1 Invoice number, BT-3 TypeCode=380, BT-2 Issue date -->
  <rsm:ExchangedDocument>
    <ram:ID>{invoice_number}</ram:ID>
    <ram:TypeCode>380</ram:TypeCode>
    <ram:IssueDateTime>
      <udt:DateTimeString format="102">{issue_date}</udt:DateTimeString>
    </ram:IssueDateTime>
    <!-- BT-22 Note: MaLo reference -->
    <ram:IncludedNote>
      <ram:Content>Marktlokation: {malo_id}</ram:Content>
    </ram:IncludedNote>
  </rsm:ExchangedDocument>

  <rsm:SupplyChainTradeTransaction>

    <!-- BG-25 Invoice lines -->
{lines}

    <!-- BG-4 Seller, BG-7 Buyer -->
    <ram:ApplicableHeaderTradeAgreement>
      <!-- BT-10 Buyer reference (MaLo-ID) -->
      <ram:BuyerReference>{malo_id}</ram:BuyerReference>

      <!-- BG-4 Seller -->
      <ram:SellerTradeParty>
        <ram:ID>{seller_mp_id}</ram:ID>
        <ram:Name>{seller_name}</ram:Name>
        {seller_addr_xml}
        {seller_vat_xml}
      </ram:SellerTradeParty>

      <!-- BG-7 Buyer -->
      <ram:BuyerTradeParty>
        <ram:ID>{buyer_id}</ram:ID>
        <ram:Name>{buyer_name}</ram:Name>
      </ram:BuyerTradeParty>
    </ram:ApplicableHeaderTradeAgreement>

    <!-- BG-13 Delivery / supply period -->
    <ram:ApplicableHeaderTradeDelivery>
      <ram:ActualDeliverySupplyChainEvent>
        <ram:OccurrenceSpecifiedPeriod>
          <ram:StartDateTime>
            <udt:DateTimeString format="102">{period_from}</udt:DateTimeString>
          </ram:StartDateTime>
          <ram:EndDateTime>
            <udt:DateTimeString format="102">{period_to}</udt:DateTimeString>
          </ram:EndDateTime>
        </ram:OccurrenceSpecifiedPeriod>
      </ram:ActualDeliverySupplyChainEvent>
    </ram:ApplicableHeaderTradeDelivery>

    <!-- BG-22 Settlement -->
    <ram:ApplicableHeaderTradeSettlement>
      <!-- BT-5 Invoice currency -->
      <ram:InvoiceCurrencyCode>EUR</ram:InvoiceCurrencyCode>

      <!-- BG-23 VAT breakdown — one entry per category and rate -->
{tax_breakdown}

      <!-- BG-14 Billing period -->
      <ram:BillingSpecifiedPeriod>
        <ram:StartDateTime>
          <udt:DateTimeString format="102">{period_from}</udt:DateTimeString>
        </ram:StartDateTime>
        <ram:EndDateTime>
          <udt:DateTimeString format="102">{period_to}</udt:DateTimeString>
        </ram:EndDateTime>
      </ram:BillingSpecifiedPeriod>

      <!-- BT-20 Payment terms / BT-9 Due date -->
      <ram:SpecifiedTradePaymentTerms>
        <ram:DueDateDateTime>
          <udt:DateTimeString format="102">{due_date}</udt:DateTimeString>
        </ram:DueDateDateTime>
      </ram:SpecifiedTradePaymentTerms>

      <!-- BG-22 Document monetary summary -->
      <ram:SpecifiedTradeSettlementHeaderMonetarySummation>
        <ram:LineTotalAmount>{netto_eur}</ram:LineTotalAmount>
        <ram:TaxBasisTotalAmount>{netto_eur}</ram:TaxBasisTotalAmount>
        <ram:TaxTotalAmount currencyID="EUR">{mwst_eur}</ram:TaxTotalAmount>
        <ram:GrandTotalAmount>{brutto_eur}</ram:GrandTotalAmount>
        <ram:DuePayableAmount>{brutto_eur}</ram:DuePayableAmount>
      </ram:SpecifiedTradeSettlementHeaderMonetarySummation>
    </ram:ApplicableHeaderTradeSettlement>

  </rsm:SupplyChainTradeTransaction>
</rsm:CrossIndustryInvoice>"#,
        invoice_number = xml_escape(&info.invoice_number),
        issue_date = issue_date_fmt,
        malo_id = xml_escape(&info.malo_id),
        lines = lines,
        seller_mp_id = xml_escape(&info.seller_mp_id),
        seller_name = xml_escape(&info.seller_name),
        seller_addr_xml = seller_addr_xml,
        seller_vat_xml = seller_vat_xml,
        buyer_id = xml_escape(&info.buyer_id),
        buyer_name = xml_escape(&info.buyer_name),
        period_from = period_from_fmt,
        period_to = period_to_fmt,
        netto_eur = info.netto_eur.round_dp(2),
        mwst_eur = info.mwst_eur.round_dp(2),
        brutto_eur = info.brutto_eur.round_dp(2),
        tax_breakdown = render_tax_breakdown(info),
        due_date = due_date_fmt,
    )
}

fn build_line_item(pos: &XRechnungPosition) -> String {
    let unit_code = unit_to_unece(&pos.unit_code);
    format!(
        r#"    <!-- BG-25 Invoice line {id} -->
    <ram:IncludedSupplyChainTradeLineItem>
      <ram:AssociatedDocumentLineDocument>
        <ram:LineID>{id}</ram:LineID>
      </ram:AssociatedDocumentLineDocument>
      <ram:SpecifiedTradeProduct>
        <ram:Name>{description}</ram:Name>
      </ram:SpecifiedTradeProduct>
      <ram:SpecifiedLineTradeAgreement>
        <ram:NetPriceProductTradePrice>
          <ram:ChargeAmount>{net_price}</ram:ChargeAmount>
        </ram:NetPriceProductTradePrice>
      </ram:SpecifiedLineTradeAgreement>
      <ram:SpecifiedLineTradeDelivery>
        <ram:BilledQuantity unitCode="{unit_code}">{quantity}</ram:BilledQuantity>
      </ram:SpecifiedLineTradeDelivery>
      <ram:SpecifiedLineTradeSettlement>
        <ram:ApplicableTradeTax>
          <ram:TypeCode>VAT</ram:TypeCode>
          <ram:CategoryCode>{vat_cat}</ram:CategoryCode>
          <ram:RateApplicablePercent>{vat_rate}</ram:RateApplicablePercent>
        </ram:ApplicableTradeTax>
        <ram:SpecifiedTradeSettlementLineMonetarySummation>
          <ram:LineTotalAmount>{net_total}</ram:LineTotalAmount>
        </ram:SpecifiedTradeSettlementLineMonetarySummation>
      </ram:SpecifiedLineTradeSettlement>
    </ram:IncludedSupplyChainTradeLineItem>"#,
        id = pos.id,
        description = xml_escape(&pos.description),
        net_price = pos.net_price_eur.round_dp(4),
        unit_code = unit_code,
        quantity = pos.quantity.round_dp(3),
        vat_cat = xml_escape(&pos.vat_category),
        vat_rate = pos.vat_rate_pct.round_dp(2),
        net_total = pos.net_total_eur.round_dp(2),
    )
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn format_date_yyyymmdd(d: time::Date) -> String {
    format!("{:04}{:02}{:02}", d.year(), d.month() as u8, d.day())
}

/// Minimal XML character escaping (EN16931 data is ASCII + German Umlauts).
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render the EN16931 BG-23 VAT breakdown.
///
/// One `ApplicableTradeTax` per distinct category and rate. A single aggregate
/// block cannot represent an invoice that mixes rates — 19 % commodity with 7 %
/// Fernwärme (§12 Abs. 2 Nr. 1 UStG) or 0 % Solar (§12 Abs. 3 UStG) — and the
/// EN16931 total-reconciliation rules compare the sum of the taxable bases
/// against the invoice net, so a collapsed breakdown fails validation.
///
/// Zero-rated bases are emitted with category `Z`; omitting them would leave the
/// bases short of the net.
fn render_tax_breakdown(info: &XRechnungInfo) -> String {
    if info.tax_subtotals.is_empty() {
        // No structured breakdown supplied: fall back to a single standard-rate
        // block over the whole net, which is correct for a single-rate invoice.
        return format!(
            "      <ram:ApplicableTradeTax>\n\
             \x20       <ram:CalculatedAmount>{mwst}</ram:CalculatedAmount>\n\
             \x20       <ram:TypeCode>VAT</ram:TypeCode>\n\
             \x20       <ram:BasisAmount>{netto}</ram:BasisAmount>\n\
             \x20       <ram:CategoryCode>S</ram:CategoryCode>\n\
             \x20       <ram:RateApplicablePercent>{rate}</ram:RateApplicablePercent>\n\
             \x20     </ram:ApplicableTradeTax>",
            mwst = info.mwst_eur.round_dp(2),
            netto = info.netto_eur.round_dp(2),
            rate = info.vat_rate_pct.normalize(),
        );
    }
    info.tax_subtotals
        .iter()
        .map(|t| {
            format!(
                "      <ram:ApplicableTradeTax>\n\
                 \x20       <ram:CalculatedAmount>{tax}</ram:CalculatedAmount>\n\
                 \x20       <ram:TypeCode>VAT</ram:TypeCode>\n\
                 \x20       <ram:BasisAmount>{base}</ram:BasisAmount>\n\
                 \x20       <ram:CategoryCode>{cat}</ram:CategoryCode>\n\
                 \x20       <ram:RateApplicablePercent>{rate}</ram:RateApplicablePercent>\n\
                 \x20     </ram:ApplicableTradeTax>",
                tax = t.tax_amount_eur.round_dp(2),
                base = t.taxable_base_eur.round_dp(2),
                cat = t.category.code(),
                rate = t.rate_percent.normalize(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── BillingResult → XRechnungInfo conversion ──────────────────────────────────

/// Build `XRechnungInfo` from the stored `rechnung_json` JSONB + billing record metadata.
///
/// Called by `GET /api/v1/billing/{id}/xrechnung` after loading the record.
pub fn info_from_rechnung_json(
    rechnung_json: &serde_json::Value,
    malo_id: &str,
    lf_mp_id: &str,
    seller_name: &str,
    seller_vat_id: Option<String>,
    netto_eur: rust_decimal::Decimal,
    mwst_eur: rust_decimal::Decimal,
    brutto_eur: rust_decimal::Decimal,
    period_from: time::Date,
    period_to: time::Date,
    vat_rate_pct: rust_decimal::Decimal,
) -> XRechnungInfo {
    let invoice_number = rechnung_json
        .get("rechnungsnummer")
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN")
        .to_owned();

    let issue_date = rechnung_json
        .get("rechnungsdatum")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).ok()
        })
        .unwrap_or(period_to);

    // Build positions from rechnung_json.rechnungspositionen.
    let positions = rechnung_json
        .get("rechnungspositionen")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(i, pos)| {
                    let description = pos
                        .get("positionstext")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Position")
                        .to_owned();
                    let net_total = pos
                        .get("gesamtpreis")
                        .and_then(|v| v.get("wert"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                        .unwrap_or(rust_decimal::Decimal::ZERO);
                    let quantity = pos
                        .get("positionsMenge")
                        .and_then(|v| v.get("wert"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                        .unwrap_or(rust_decimal::Decimal::ONE);
                    let unit = pos
                        .get("positionsMenge")
                        .and_then(|v| v.get("einheit"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("C62")
                        .to_owned();
                    let net_price = pos
                        .get("einzelpreis")
                        .and_then(|v| v.get("wert"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                        .unwrap_or(net_total);

                    // Skip the MwSt position (it's the summary VAT, not a line item VAT)
                    if description.contains("Mehrwertsteuer") || description.contains("MwSt") {
                        return None;
                    }

                    Some(XRechnungPosition {
                        id: (i + 1) as u32,
                        description,
                        quantity,
                        unit_code: unit,
                        net_price_eur: net_price,
                        net_total_eur: net_total,
                        vat_category: "S".to_owned(),
                        vat_rate_pct,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    XRechnungInfo {
        invoice_number,
        issue_date,
        due_date: None, // Operators should configure Zahlungsziel
        period_from,
        period_to,
        seller_mp_id: lf_mp_id.to_owned(),
        seller_name: seller_name.to_owned(),
        seller_vat_id,
        seller_address: None,
        buyer_id: malo_id.to_owned(),
        buyer_name: format!("Kunde {malo_id}"),
        malo_id: malo_id.to_owned(),
        positions,
        netto_eur,
        mwst_eur,
        brutto_eur,
        // Derived from the stored positions so the breakdown cannot drift from
        // what was billed. Empty when the JSON carries no positions, which the
        // renderer handles by falling back to a single standard-rate block.
        tax_subtotals: subtotals_from_rechnung_json(rechnung_json, vat_rate_pct),
        vat_rate_pct,
    }
}

/// Recover the EN16931 VAT breakdown from the stored `rechnung_json`.
///
/// The stored document keeps each position's `applicable_tax_rate`, so the
/// breakdown is re-derived rather than persisted separately — a stored copy
/// could disagree with the positions it summarises.
fn subtotals_from_rechnung_json(
    rechnung_json: &serde_json::Value,
    vat_rate_pct: rust_decimal::Decimal,
) -> Vec<energy_billing::invoice::TaxSubtotal> {
    let Some(positions) = rechnung_json.get("positions") else {
        return Vec::new();
    };
    let Ok(parsed) =
        serde_json::from_value::<Vec<energy_billing::position::BillingPosition>>(positions.clone())
    else {
        return Vec::new();
    };
    let default_rate = vat_rate_pct / rust_decimal::Decimal::ONE_HUNDRED;
    energy_billing::invoice::tax_subtotals_of(&parsed, default_rate)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn sample_info() -> XRechnungInfo {
        XRechnungInfo {
            invoice_number: "BILL-2026-001".to_owned(),
            issue_date: time::macros::date!(2026 - 07 - 01),
            due_date: Some(time::macros::date!(2026 - 07 - 31)),
            period_from: time::macros::date!(2026 - 06 - 01),
            period_to: time::macros::date!(2026 - 06 - 30),
            seller_mp_id: "9910000000002".to_owned(),
            seller_name: "Musterstrom GmbH".to_owned(),
            seller_vat_id: Some("DE123456789".to_owned()),
            seller_address: Some("Musterstraße 1, 10115 Berlin".to_owned()),
            buyer_id: "51238696781".to_owned(),
            buyer_name: "Kunde 51238696781".to_owned(),
            malo_id: "51238696781".to_owned(),
            positions: vec![
                XRechnungPosition {
                    id: 1,
                    description: "Grundpreis Strom".to_owned(),
                    quantity: dec!(30),
                    unit_code: "Tage".to_owned(),
                    net_price_eur: dec!(0.20),
                    net_total_eur: dec!(6.00),
                    vat_category: "S".to_owned(),
                    vat_rate_pct: dec!(19),
                },
                XRechnungPosition {
                    id: 2,
                    description: "Arbeitspreis Strom".to_owned(),
                    quantity: dec!(312.5),
                    unit_code: "kWh".to_owned(),
                    net_price_eur: dec!(0.32),
                    net_total_eur: dec!(100.00),
                    vat_category: "S".to_owned(),
                    vat_rate_pct: dec!(19),
                },
            ],
            netto_eur: dec!(106.00),
            mwst_eur: dec!(20.14),
            tax_subtotals: Vec::new(),
            brutto_eur: dec!(126.14),
            vat_rate_pct: dec!(19),
        }
    }

    #[test]
    fn generates_valid_xml_structure() {
        let xml = build_zugferd_cii_xml(&sample_info());
        assert!(
            xml.contains("CrossIndustryInvoice"),
            "must contain root element"
        );
        assert!(xml.contains("BILL-2026-001"), "must contain invoice number");
        assert!(xml.contains("51238696781"), "must contain MaLo ID");
        assert!(xml.contains("Grundpreis Strom"), "must contain position 1");
        assert!(
            xml.contains("Arbeitspreis Strom"),
            "must contain position 2"
        );
        assert!(
            xml.contains("xoev-de:kosit:standard:xrechnung_3.0"),
            "must have XRechnung CIUS ID"
        );
        assert!(
            xml.contains("<ram:TypeCode>380</ram:TypeCode>"),
            "TypeCode 380 = commercial invoice"
        );
        assert!(xml.contains("DE123456789"), "must include VAT ID");
    }

    #[test]
    fn xml_escape_works() {
        assert_eq!(
            xml_escape("Müller & Söhne <GmbH>"),
            "Müller &amp; Söhne &lt;GmbH&gt;"
        );
    }

    #[test]
    fn date_format_yyyymmdd() {
        let d = time::macros::date!(2026 - 06 - 01);
        assert_eq!(format_date_yyyymmdd(d), "20260601");
    }

    #[test]
    fn unit_code_kwh_maps_to_unece() {
        assert_eq!(unit_to_unece("kWh"), "KWH");
        assert_eq!(unit_to_unece("Tage"), "DAY");
        assert_eq!(unit_to_unece("Monat"), "MON");
    }
}
