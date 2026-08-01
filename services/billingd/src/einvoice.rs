//! EN 16931 e-invoicing — the semantic model and the syntaxes rendered from it.
//!
//! The engine's [`energy_billing::Invoice`] maps to an [`en16931::Invoice`] (the
//! syntax-neutral semantic model) via `to_en16931`, at the layer that still has
//! per-position VAT — so a mixed-rate invoice keeps a correct per-line rate that
//! reconciles with the BG-23 breakdown. `en16931-formats` then renders it to
//! XRechnung/CII (B2G) and PEPPOL UBL. BO4E stays the accounting representation;
//! this is the e-invoicing one. The model is stored (`billing_records.en16931_json`)
//! so any syntax can be produced from it long after the calculation.

use en16931::identifier::Identifier;
use en16931::invoice::{Code, Contact, Party, PostalAddress};
use energy_billing::Invoice;
use energy_billing::en16931_map::XRECHNUNG_SPEC_ID;

use crate::config::BillingdConfig;

/// Split `"Straße 1, 12345 Ort"` into (line1, post_code, city) — best effort.
fn parse_address(addr: &str) -> (Option<String>, Option<String>, Option<String>) {
    match addr.rsplit_once(',') {
        Some((street, rest)) => {
            let rest = rest.trim();
            let (plz, city) = rest.split_once(' ').unwrap_or((rest, ""));
            (
                Some(street.trim().to_owned()),
                (!plz.is_empty()).then(|| plz.to_owned()),
                (!city.is_empty()).then(|| city.trim().to_owned()),
            )
        }
        None => (Some(addr.trim().to_owned()), None, None),
    }
}

/// Pull (phone, email) out of a free-form `seller_contact` string.
fn parse_contact(contact: &str) -> (Option<String>, Option<String>) {
    let email = contact
        .split_whitespace()
        .find(|t| t.contains('@'))
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.')
                .to_owned()
        });
    let phone = contact
        .split(',')
        .find(|t| t.chars().any(|c| c.is_ascii_digit()) && !t.contains('@'))
        .map(|t| t.trim().to_owned());
    (phone, email)
}

/// BG-4 SELLER, from the operator's `[seller]` configuration. Fills BG-6 contact
/// and the split BG-5 address so the seller side satisfies XRechnung's BR-DE-2..7.
fn seller_party(cfg: &BillingdConfig) -> Party {
    let name = cfg
        .seller_name
        .clone()
        .unwrap_or_else(|| cfg.tenant.clone());
    let (line1, post_code, city) = cfg
        .seller_address
        .as_deref()
        .map_or((None, None, None), parse_address);
    let (phone, email) = cfg
        .seller_contact
        .as_deref()
        .map_or((None, None), parse_contact);
    Party {
        name: Some(name.clone()),
        vat_identifier: cfg.seller_vat_id.clone(),
        // BT-49 electronic address: the LF's MP-ID under EAS 0088 (GLN — a
        // BDEW MP-ID is GLN-format). A strict-XRechnung buyer Leitweg-ID (BT-10)
        // is set per document where a B2G recipient supplies one.
        electronic_address: Identifier::eas(cfg.tenant.clone(), "0088").ok(),
        address: PostalAddress {
            line1,
            city,
            post_code,
            country: Some(Code::from("DE")),
            ..Default::default()
        },
        contact: Contact {
            name: Some(name),
            phone,
            email,
        },
        ..Default::default()
    }
}

/// BG-7 BUYER, from the billed MaLo.
fn buyer_party(malo_id: &str) -> Party {
    Party {
        name: Some(format!("Marktlokation {malo_id}")),
        electronic_address: Identifier::eas(malo_id.to_owned(), "0088").ok(),
        address: PostalAddress {
            country: Some(Code::from("DE")),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// PEPPOL BIS Billing 3.0 business process (BT-23), required by XRechnung/Peppol.
const BUSINESS_PROCESS: &str = "urn:fdc:peppol.eu:2017:poacc:billing:01:1.0";

/// Build the EN 16931 semantic model for a freshly-billed invoice, and stamp the
/// operator-level terms XRechnung needs on every document: BT-23 business process
/// and the BG-16 SEPA payment instruction (BT-81 means code 58 + BT-84 seller IBAN).
#[must_use]
pub fn build(invoice: &Invoice, cfg: &BillingdConfig, malo_id: &str) -> en16931::Invoice {
    let mut model = invoice.to_en16931(XRECHNUNG_SPEC_ID, seller_party(cfg), buyer_party(malo_id));
    model.business_process = Some(BUSINESS_PROCESS.to_owned());
    if let Some(iban) = cfg.seller_iban.clone() {
        model.payment = Some(en16931::invoice::PaymentInstructions {
            // UNCL 4461 code 58 — SEPA credit transfer.
            means_code: Some(Code::from("58")),
            means_text: Some("SEPA-Überweisung".to_owned()),
            remittance_information: model.number.clone(),
            means: Some(en16931::invoice::PaymentMeans::CreditTransfer(vec![
                en16931::invoice::CreditTransfer {
                    account_identifier: Some(iban),
                    account_name: cfg.seller_name.clone(),
                    provider_identifier: cfg.seller_bic.clone(),
                },
            ])),
        });
    }
    model
}

/// Map and persist the EN 16931 model for a record, in the caller's transaction.
/// Every invoice-producing path calls this so the render endpoints can require a
/// stored model rather than re-parsing BO4E.
pub async fn store(
    exec: impl sqlx::PgExecutor<'_>,
    record_id: uuid::Uuid,
    invoice: &Invoice,
    cfg: &BillingdConfig,
    malo_id: &str,
) -> anyhow::Result<()> {
    let model = build(invoice, cfg, malo_id);
    crate::pg::attach_en16931(exec, record_id, &serde_json::to_value(&model)?).await
}

/// Persist an already-built model (used by the correction path, which credits an
/// existing record's model rather than re-billing).
pub async fn store_model(
    exec: impl sqlx::PgExecutor<'_>,
    record_id: uuid::Uuid,
    model: &en16931::Invoice,
) -> anyhow::Result<()> {
    crate::pg::attach_en16931(exec, record_id, &serde_json::to_value(model)?).await
}

/// Turn a stored model into its Stornorechnung/credit note: EN 16931 credit notes
/// carry positive amounts and convey the reversal through the document kind (381),
/// so only the number and the type change.
#[must_use]
pub fn to_credit_note(mut original: en16931::Invoice, new_number: &str) -> en16931::Invoice {
    original.number = Some(new_number.to_owned());
    original.type_code = Some(en16931::invoice::Code::from("381"));
    original.kind = en16931::invoice::DocumentKind::CreditNote;
    original
}

/// Render CII (XRechnung 3.0 / ZUGFeRD CII) XML from the stored model.
#[must_use]
pub fn render_cii(model: &en16931::Invoice) -> String {
    en16931_formats::cii::to_string(model)
}

/// Render PEPPOL BIS 3.0 UBL XML from the stored model.
#[must_use]
pub fn render_ubl(model: &en16931::Invoice) -> String {
    en16931_formats::ubl::to_string(model)
}

/// Validate against the **XRechnung 3.0** profile and render CII in one step.
///
/// `en16931-formats::cii::to_string_for` proves the document against the profile
/// before writing, so a B2G submission cannot ship a rejectable file — on failure
/// it returns the findings (`[BR-DE-…] BG-…/BT-… — …`) the ZRE/OZG-RE portal would
/// raise, most-severe first.
///
/// # Errors
/// The list of violated XRechnung rules when the model is not profile-valid.
pub fn render_xrechnung_cii(model: &en16931::Invoice) -> Result<String, Vec<String>> {
    en16931_formats::cii::to_string_for(model, &en16931::profiles::XRECHNUNG).map_err(|nv| {
        nv.report()
            .findings()
            .iter()
            .map(|f| format!("[{}] {} — {}", f.rule, f.path, f.message))
            .collect()
    })
}

/// The XRechnung terms the **buyer** party is still missing — the customer master
/// that lives in `vertragd` (BG-7 address/contact). Precise per-term answers
/// (`[BR-DE-3] BG-7/BT-52 — Buyer city`) via `Party::missing_for`, so the operator
/// or an enrichment step knows exactly what to supply.
#[must_use]
pub fn buyer_gaps(model: &en16931::Invoice) -> Vec<String> {
    model
        .buyer
        .missing_for(
            &en16931::profiles::XRECHNUNG,
            en16931::invoice::PartyRole::Buyer,
        )
        .iter()
        .map(|m| m.to_string())
        .collect()
}

/// Stamp the buyer's Leitweg-ID (BT-10) for a B2G submission.
#[must_use]
pub fn with_buyer_reference(mut model: en16931::Invoice, leitweg_id: &str) -> en16931::Invoice {
    model.buyer_reference = Some(leitweg_id.to_owned());
    model
}

/// BG-7 buyer supplied on a B2G submission — the receiving public authority.
#[derive(Debug, serde::Deserialize)]
pub struct B2gBuyer {
    /// BT-44 buyer name.
    pub name: String,
    /// BT-50 address line.
    pub line1: Option<String>,
    /// BT-53 post code.
    pub post_code: Option<String>,
    /// BT-52 city.
    pub city: Option<String>,
    /// BT-55 country (ISO-3166 alpha-2); defaults to `"DE"`.
    pub country: Option<String>,
    /// BT-56/57/58 contact.
    pub contact_name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    /// BT-48 buyer VAT identifier (B2B recipients).
    pub vat_id: Option<String>,
    /// BT-49 buyer electronic address (Leitweg-ID under EAS 0204, or a GLN).
    pub electronic_address: Option<String>,
}

/// Complete the model's placeholder buyer with the B2G recipient's details, so
/// the document satisfies XRechnung's BG-7 `BR-DE-*` rules.
#[must_use]
pub fn apply_b2g_buyer(mut model: en16931::Invoice, buyer: &B2gBuyer) -> en16931::Invoice {
    let eas = buyer.electronic_address.clone();
    model.buyer = Party {
        name: Some(buyer.name.clone()),
        vat_identifier: buyer.vat_id.clone(),
        // BT-49 under EAS 0204 (German Leitweg-ID); omitted if not supplied.
        electronic_address: eas.and_then(|a| Identifier::eas(a, "0204").ok()),
        address: PostalAddress {
            line1: buyer.line1.clone(),
            city: buyer.city.clone(),
            post_code: buyer.post_code.clone(),
            country: Some(Code::from(buyer.country.as_deref().unwrap_or("DE"))),
            ..Default::default()
        },
        contact: Contact {
            name: buyer
                .contact_name
                .clone()
                .or_else(|| Some(buyer.name.clone())),
            phone: buyer.phone.clone(),
            email: buyer.email.clone(),
        },
        ..Default::default()
    };
    model
}
