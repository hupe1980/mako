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
use energy_billing::en16931_map::{EN16931_SPEC_ID, XRECHNUNG_SPEC_ID};

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

/// BT-34, the seller's own electronic address — but only if it is really a GLN.
///
/// A BDEW-Codenummer is issued through GS1 and *is* a GLN, so declaring the
/// operator's MP-ID under EAS `0088` is true for a correctly configured tenant.
/// It is false for a mistyped one, and `Identifier::eas` cannot tell the
/// difference: it validates the *scheme* code and accepts any content. That is
/// the same defect this crate already fixed on the buyer side — a MaLo-ID
/// dressed up as a GLN — and it was still here on the seller side, unnoticed,
/// because nothing checks a claim no rule can test.
///
/// `eas_checked` verifies the GS1 check digit and length. On failure the term is
/// **omitted**: BT-34 is optional in EN 16931 core, so a retail invoice stays
/// valid, and omitting is the honest encoding of "we do not have one". A
/// misconfigured operator therefore ships documents without BT-34 rather than
/// documents asserting a GLN that no receiver can resolve — and finds out at the
/// B2G path, where XRechnung requires the term and `render_xrechnung_cii`
/// refuses to write a rejectable file.
fn seller_electronic_address(tenant: &str) -> Option<Identifier> {
    match Identifier::eas_checked(tenant, "0088") {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                tenant,
                error = %e,
                "seller MP-ID is not a valid GS1 GLN — BT-34 omitted rather than \
                 claiming a GLN this identifier is not. Check the `tenant` setting \
                 against the operator's BDEW-Codenummer",
            );
            None
        }
    }
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
        // BT-34 electronic address: the LF's own MP-ID under EAS 0088 (GLN).
        // A BDEW-Codenummer *is* a GLN — BDEW issues them through GS1 — so the
        // claim is true for a correctly configured operator, and `eas_checked`
        // is what makes "correctly configured" checkable rather than assumed.
        electronic_address: seller_electronic_address(&cfg.tenant),
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

/// BG-7 BUYER — from `vertragd`'s Kunde when one is on file, else a stub.
///
/// `billingd` holds no customer master, so the real name, postal address and
/// VAT-ID come from `vertragd.kunden` via `GET /vertraege/by-malo/{id}`. With
/// them the document satisfies BR-DE-8 (city) and BR-DE-9 (post code); without
/// them the fallback names the supply site and those two findings stand. The
/// fallback is deliberate — a vertragd outage must degrade the invoice, not fail
/// the billing run.
///
/// # Why there is no BT-49 electronic address
///
/// A MaLo-ID is an **11-digit BDEW Marktlokations-ID**. It is not a GS1 GLN, and
/// the EAS code list has no entry for it. This function used to emit it under
/// EAS `0088` (GLN), which is a false claim about the identifier's registry:
/// syntactically valid — `Identifier::eas` only checks that the *scheme code*
/// exists, and BR-CL-25 only checks the same — but semantically wrong, and
/// unresolvable for any receiver that takes it at face value.
///
/// `Identifier::eas_checked` now catches exactly this, and
/// [`seller_electronic_address`] uses it on the seller side where the same
/// mistake had survived. It refuses an 11-digit value for scheme `0088` with
/// the reason: *"a GS1 GLN is exactly 13 digits, and this is 11"*.
///
/// Omitting it is the honest encoding, and it is not a data gap that master data
/// closes: a household has no Peppol endpoint (BT-49) and no Leitweg-ID (BT-10).
/// Those two findings are what a retail invoice legitimately carries; a B2G
/// recipient supplies both through [`apply_b2g_buyer`] / [`with_buyer_reference`].
fn buyer_party(malo_id: &str, buyer: Option<&crate::clients::Rechnungsempfaenger>) -> Party {
    let Some(b) = buyer else {
        return Party {
            name: Some(format!("Marktlokation {malo_id}")),
            address: PostalAddress {
                country: Some(Code::from("DE")),
                ..Default::default()
            },
            ..Default::default()
        };
    };
    Party {
        // Fall back to the MaLo label rather than an empty BT-44: a nameless
        // party is a worse document than one naming the supply site.
        name: b
            .name
            .clone()
            .or_else(|| Some(format!("Marktlokation {malo_id}"))),
        vat_identifier: b.vat_id.clone(),
        address: PostalAddress {
            line1: b.line1.clone(),
            city: b.city.clone(),
            post_code: b.post_code.clone(),
            country: Some(Code::from(b.country.as_deref().unwrap_or("DE"))),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Findings against the profile the document actually declares in BT-24.
///
/// A document is held to what it claims, not to a fixed profile — so a retail
/// invoice (plain EN 16931) is checked against the core rules and a B2G
/// submission (XRechnung, set by [`with_buyer_reference`]) against the CIUS.
/// Validating retail against XRechnung would report BR-DE findings the document
/// never claimed to satisfy.
///
/// Reports rather than rejects. The B2G path is separately *proven* before
/// writing — [`render_xrechnung_cii`] refuses to emit a rejectable file.
#[must_use]
pub fn validate(model: &en16931::Invoice) -> en16931::validation::ValidationReport {
    if model.specification_id.as_deref() == Some(XRECHNUNG_SPEC_ID) {
        en16931::profiles::XRECHNUNG.validate(model)
    } else {
        en16931::validation::validate(model)
    }
}

/// Log any fatal conformance finding against the declared profile.
fn report_conformance(model: &en16931::Invoice, invoice_number: &str) {
    let report = validate(model);
    let fatal: Vec<String> = report
        .fatal()
        .map(|f| format!("{} at {}", f.rule, f.path))
        .collect();
    if !fatal.is_empty() {
        tracing::warn!(
            invoice_number,
            findings = %fatal.join(", "),
            "e-invoice: the stored model does not satisfy the profile it declares \
             in BT-24 — a receiving validator will reject it",
        );
    }
}

/// PEPPOL BIS Billing 3.0 business process (BT-23), required by XRechnung/Peppol.
pub const BUSINESS_PROCESS: &str = "urn:fdc:peppol.eu:2017:poacc:billing:01:1.0";

/// Build the EN 16931 semantic model for a freshly-billed invoice.
///
/// BT-24 declares **plain EN 16931**, not XRechnung. XRechnung is the German
/// *B2G* CIUS: it requires a Leitweg-ID (BT-10) and a Peppol endpoint (BT-49),
/// neither of which a household supply customer has. §14 UStG requires an
/// e-invoice to conform to **EN 16931** — XRechnung and ZUGFeRD are examples,
/// not the requirement — so core is both sufficient and true here. Claiming a
/// CIUS the document cannot satisfy is the same class of defect as a fabricated
/// identifier scheme. [`with_buyer_reference`] upgrades BT-24 once a B2G caller
/// supplies the missing terms.
///
/// Also stamps BT-23 business process and the BG-16 SEPA payment instruction
/// (BT-81 means code 58 + BT-84 seller IBAN).
#[must_use]
pub fn build(
    invoice: &Invoice,
    cfg: &BillingdConfig,
    malo_id: &str,
    buyer: Option<&crate::clients::Rechnungsempfaenger>,
) -> en16931::Invoice {
    let mut model = invoice.to_en16931(
        EN16931_SPEC_ID,
        seller_party(cfg),
        buyer_party(malo_id, buyer),
    );
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
    report_conformance(&model, model.number.as_deref().unwrap_or("<no number>"));
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
    buyer: Option<&crate::clients::Rechnungsempfaenger>,
) -> anyhow::Result<()> {
    let model = build(invoice, cfg, malo_id, buyer);
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
    // BT-10 is the last term XRechnung needs that a retail document cannot have,
    // so this is the point the document may honestly claim the CIUS. Everything
    // before it declares plain EN 16931 — see `build`.
    model.specification_id = Some(XRECHNUNG_SPEC_ID.to_owned());
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
        // `eas_checked` rather than `eas`: 0204 has no published shape check
        // today, so the two behave identically — but the day the crate learns
        // one, this call picks it up instead of continuing to accept anything.
        electronic_address: eas.and_then(|a| Identifier::eas_checked(a, "0204").ok()),
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
