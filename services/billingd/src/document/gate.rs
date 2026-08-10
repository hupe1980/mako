//! Admission control: what a template must survive before it may be published.
//!
//! The store is append-only, so publishing is close to irreversible — the row
//! is there for eight years whether or not it works. And the moment to find out
//! that a template does not render is *not* the moment ten thousand invoices
//! are being generated at 02:00 on the first of the month. So the gate renders
//! it first.
//!
//! # What is proven
//!
//! For an invoice template, all of it:
//!
//! 1. It compiles, against a specimen deliberately chosen to be awkward — a
//!    real [`en16931::Invoice`], projected through [`DocumentView::of`] and
//!    rendered to real CII, so the gate exercises the production pipeline
//!    rather than a stand-in for it.
//! 2. It produces a **PDF/A-3** file under the standard it declares — this is
//!    `typst-pdf`'s own conformance enforcement, not a claim of ours.
//! 3. The Factur-X XMP stamps onto it.
//! 4. The finished document **reads back as an invoice**: byte-identical
//!    payload, no [`facturx::Divergence`], the XML re-parsed as CII, and the
//!    same BT-1 and BT-115 that went in.
//! 5. The **page** carries the § 14 Abs. 4 UStG terms a Rechnung cannot omit.
//!
//! (4) is the one that matters most, and it is done with the *counterparty's*
//! reader — `en16931-formats::zugferd::extract` — rather than with one of our
//! own. A ZUGFeRD invoice is a promise that somebody else's machine can read
//! the file; a check that only mako's extractor passes proves nothing about
//! that. Everything else here is a claim; this is a measurement.
//!
//! (5) exists because (1)–(4) are all about the machine-readable half, and the
//! two halves can be wrong separately. `#let render(invoice) = []` satisfies
//! every one of them — conformant PDF/A-3, perfectly extractable CII invoice,
//! blank page.
//!
//! # What is not proven, and why the store says so
//!
//! The Textform kinds — Mahnung, Preisanpassung — have no data contract in
//! `billingd`: the Mahnwesen lives in `accountingd` and the § 41 Abs. 5 EnWG
//! notice in `vertragd`. There is no specimen to render them against, so the
//! gate proves what it can — that the template parses and exports the contract
//! function — and the store records **which** proof was obtained
//! ([`Proof`]). A template that was only parsed is not a template that was
//! shown to work, and a column that says so is better than a comment that
//! implies otherwise.

use anyhow::{Context as _, Result, bail};

use super::facturx;
use super::render::{RenderRequest, render};
use super::view::DocumentView;
use crate::template_store::TemplateKind;

/// How thoroughly a stored template was proven before it was published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Proof {
    /// Rendered to a conformant PDF/A carrier whose embedded XML was extracted
    /// again and matched. The full proof, and the only one an invoice accepts.
    RenderedPdfa,
    /// Compiled far enough to show the template parses and exports `render`.
    /// No page was produced, because there is nothing yet to render it from.
    Parsed,
}

impl Proof {
    /// The `document_templates.proof` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RenderedPdfa => "RENDERED_PDFA",
            Self::Parsed => "PARSED",
        }
    }
}

/// What the gate learned about a template.
#[derive(Debug, Clone)]
pub struct Proven {
    pub proof: Proof,
    /// Typst warnings from the proving render — an operator should see these
    /// before rolling out, not after. A missing font family is a warning.
    pub warnings: Vec<String>,
    /// Pages the specimen came to. A layout that turns one invoice into three
    /// hundred pages compiles perfectly well.
    pub pages: usize,
}

/// The most pages a specimen invoice may occupy.
///
/// The specimen is a dozen lines. A template that spends more than this on it
/// has a layout fault — an unbounded box, a page break per line — that would
/// otherwise be discovered by a customer, or by the postage bill.
const MAX_SPECIMEN_PAGES: usize = 8;

/// The PDF/A level to prove against when the operator names none.
///
/// ZUGFeRD 2.3 requires PDF/A-3; `b` is the level the specification's own
/// examples use and the one every receiver accepts.
pub const DEFAULT_PDF_STANDARD: &str = "a-3b";

/// Prove a template, or refuse it.
///
/// # Errors
///
/// Any failure to render, to conform, or to get the XML back out — with the
/// message the operator needs to fix their template.
pub fn prove(kind: TemplateKind, source: &str, pdf_standard: Option<&str>) -> Result<Proven> {
    match kind {
        TemplateKind::Invoice => prove_invoice(source, pdf_standard),
        TemplateKind::Mahnung | TemplateKind::Preisanpassung => prove_parses(source),
    }
}

/// The full proof: render, conform, stamp, then read the result back as a
/// receiver would.
fn prove_invoice(source: &str, pdf_standard: Option<&str>) -> Result<Proven> {
    let model = specimen_invoice();
    let profile = facturx::profile_of(&model);

    // The payload must be a *valid* invoice before it is embedded in anything.
    // Not belt-and-braces: the specimen is what every operator template is
    // proven against, so an invalid one means the gate has been certifying
    // templates against a document a receiver would reject. It caught a real
    // defect the first time it ran — an exempt line with BT-152 absent, which
    // BR-E-05 requires to be zero — that nothing else here could see, because
    // the carrier round-trips an invalid payload exactly as faithfully as a
    // valid one.
    let fatal: Vec<String> = crate::einvoice::validate(&model)
        .fatal()
        .map(|f| format!("[{}] {} — {}", f.rule, f.path, f.message))
        .collect();
    if !fatal.is_empty() {
        bail!(
            "the gate specimen is not a valid EN 16931 invoice — a mako bug, not a \
             fault in the template being published:\n  {}",
            fatal.join("\n  "),
        );
    }

    // Real CII, produced by the same function production uses — not a stub.
    // A gate that embeds a placeholder proves the carrier moves *bytes*; it
    // cannot prove the bytes an invoice actually consists of survive it.
    let xml = crate::einvoice::render_cii(&model);

    let request = RenderRequest {
        template: source.to_owned(),
        // `DocumentView::of` is the production projection, so the gate exercises
        // it rather than a hand-written stand-in that could drift from it.
        data: Some(serde_json::to_string(&DocumentView::of(&model))?),
        attachment: Some(facturx::attachment(profile, xml.clone())?),
        standard: Some(pdf_standard.unwrap_or(DEFAULT_PDF_STANDARD).to_owned()),
        date: SPECIMEN_DATE,
        ident: "mako-publish-gate-specimen".to_owned(),
    };

    let rendered = render(&request)?;
    if rendered.pages > MAX_SPECIMEN_PAGES {
        bail!(
            "the specimen invoice came to {} pages; a layout that needs more than \
             {MAX_SPECIMEN_PAGES} for {} lines has a page-break fault",
            rendered.pages,
            model.lines.len(),
        );
    }

    let stamped = facturx::stamp(&rendered.pdf, profile)?;
    check_carrier(&stamped, profile, &xml, &model)?;
    the_page_is_an_invoice(&stamped, &model)?;

    Ok(Proven {
        proof: Proof::RenderedPdfa,
        warnings: rendered.warnings,
        pages: rendered.pages,
    })
}

/// Read the finished document back the way a receiver's software does.
///
/// Everything here goes through `en16931-formats`' own reader rather than
/// mako's: it walks the catalogue's `/Names` `/EmbeddedFiles` tree, resolves the
/// payload by the profile-preference filename order, parses it back through the
/// CII reader, and reports every disagreement between what the PDF *declares*
/// and what it *contains*. Using the counterparty's route rather than a private
/// one is the point — a check that only mako's own extractor passes proves
/// nothing about what a receiver will find.
fn check_carrier(
    pdf: &[u8],
    profile: facturx::Profile,
    sent: &str,
    model: &en16931::Invoice,
) -> Result<()> {
    let got = facturx::extract(pdf).context(
        "the finished document is not readable as a ZUGFeRD invoice — it looks like one \
         and no receiver could get an invoice out of it",
    )?;

    // 1. The bytes. Byte-identical, not merely equivalent: a receiver archives
    //    what it was sent, and a re-serialised payload is a different document.
    if got.xml != sent {
        bail!(
            "the embedded invoice came back out changed ({} bytes in, {} out); the visual \
             and machine representations of this document would disagree",
            sent.len(),
            got.xml.len(),
        );
    }

    // 2. Every disagreement the reader knows how to spot, at once — the XMP's
    //    profile against BT-24, its filename against the file actually attached,
    //    `/AFRelationship` against the profile, and the absence of XMP entirely.
    //    This replaced a hand-rolled comparison of two of those four.
    if !got.divergence.is_empty() {
        let found: Vec<String> = got.divergence.iter().map(ToString::to_string).collect();
        bail!(
            "the carrier metadata disagrees with the invoice inside it:\n  {}",
            found.join("\n  "),
        );
    }

    // 3. The payload must read back *as CII*, not merely as bytes. Byte equality
    //    would hold just as well for a payload no parser accepts.
    let Some(read) = got.invoice else {
        bail!(
            "the embedded invoice does not parse as CII: {}",
            got.syntax_findings.join("; "),
        );
    };
    if !got.syntax_findings.is_empty() {
        bail!(
            "the embedded invoice carries content outside the EN 16931 subset: {}",
            got.syntax_findings.join("; "),
        );
    }

    // 4. And it must be *this* invoice. The terms checked are the ones a
    //    receiver posts to a ledger; if any of them changed in transit through
    //    the carrier, the document says something the model does not.
    let sent_number = model.number.as_deref().unwrap_or_default();
    if read.number.as_deref().unwrap_or_default() != sent_number {
        bail!(
            "the invoice that came out is a different document: BT-1 `{}` in, `{}` out",
            sent_number,
            read.number.unwrap_or_default(),
        );
    }
    if read.totals.due != model.totals.due {
        bail!(
            "the amount due did not survive the carrier: BT-115 {} in, {} out",
            model.totals.due,
            read.totals.due,
        );
    }
    if got.profile != profile {
        bail!("the carrier declares {} but holds {profile}", got.profile);
    }
    Ok(())
}

/// The page must actually *say* the things an invoice has to say.
///
/// Everything above proves the machine-readable half. Without this, the
/// template `#let render(invoice) = []` passes every one of those checks: it
/// compiles, it is conformant PDF/A-3, and it carries a perfectly extractable
/// CII invoice — on a blank page. The two halves are separately capable of
/// being wrong, so both are measured.
///
/// The terms required here are the unambiguous ones § 14 Abs. 4 UStG makes
/// mandatory: the invoice number (Nr. 4) and the full names of both parties
/// (Nr. 1). They are not a matter of layout taste — a document without them is
/// not a Rechnung — so requiring them constrains nothing an operator may
/// legitimately want to do.
fn the_page_is_an_invoice(pdf: &[u8], model: &en16931::Invoice) -> Result<()> {
    let doc = lopdf::Document::load_mem(pdf).context("reading back the rendered document")?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc.extract_text(&pages).map_err(|e| {
        anyhow::anyhow!(
            "no text could be read off the rendered page ({e}) — an invoice must be \
             searchable and machine-readable, not an image of one"
        )
    })?;
    // Kerning and line breaks put arbitrary whitespace between glyphs, so the
    // comparison ignores it entirely rather than guessing where breaks fall.
    let flat: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    for (term, value) in [
        ("BT-1 Rechnungsnummer (§ 14 Abs. 4 Nr. 4)", &model.number),
        ("BT-27 seller name (§ 14 Abs. 4 Nr. 1)", &model.seller.name),
        ("BT-44 buyer name (§ 14 Abs. 4 Nr. 1)", &model.buyer.name),
    ] {
        let Some(value) = value else { continue };
        let needle: String = value.chars().filter(|c| !c.is_whitespace()).collect();
        if !flat.contains(&needle) {
            bail!(
                "the rendered page does not print {term}: `{value}` is nowhere on it. \
                 The invoice XML would be correct and the document a customer receives \
                 would not be a valid Rechnung"
            );
        }
    }
    Ok(())
}

/// The weaker proof: the template parses and exports the contract function.
///
/// Compiles a harness that imports `render` and never calls it, so the
/// template's top level is evaluated — catching a syntax error, a bad import or
/// a missing export — without needing data it has no specimen for.
fn prove_parses(source: &str) -> Result<Proven> {
    let rendered = render(&RenderRequest {
        template: source.to_owned(),
        // No view — so the harness imports `render` and does not call it.
        data: None,
        attachment: None,
        standard: None,
        date: SPECIMEN_DATE,
        ident: "mako-publish-gate-parse".to_owned(),
    })?;
    Ok(Proven {
        proof: Proof::Parsed,
        warnings: rendered.warnings,
        pages: rendered.pages,
    })
}

/// The date the specimen bears. Fixed, so the gate is deterministic.
pub const SPECIMEN_DATE: time::Date = time::macros::date!(2026 - 03 - 01);

/// The invoice a template is proven against.
///
/// A real [`en16931::Invoice`], reconciled by the crate that owns BG-23 and
/// BG-22 — not a hand-written [`DocumentView`]. That matters twice over: the
/// view the template renders comes from [`DocumentView::of`], the production
/// projection, so the gate exercises it instead of a stand-in that could drift
/// from it; and the payload embedded in the carrier is real CII from
/// [`crate::einvoice::render_cii`], so "the invoice survives the carrier" is a
/// statement about an invoice rather than about a placeholder.
///
/// It is chosen to be the awkward cases rather than the easy one, because a
/// template that only handles the easy one will meet the others in production:
///
/// - **two VAT rates**, so a breakdown loop that assumes one is caught;
/// - **an exempt position** with a BT-120 reason, which has a rate of `None`;
/// - **a credit line**, so a negative amount has somewhere to go;
/// - **a four-decimal unit price** beside two-decimal money, which is what
///   catches a template that formats every number the same way;
/// - **umlauts and a long item name**, which is what catches a fixed-width
///   column;
/// - **absent optional fields**, because `none` in Typst is not the empty
///   string and a template that assumes otherwise fails on the first customer
///   without a VAT ID.
///
/// # Panics
///
/// If the specimen itself is malformed — a literal in this file being wrong is
/// a mako bug, and failing loudly at the first render beats proving templates
/// against a broken document.
#[must_use]
pub fn specimen_invoice() -> en16931::Invoice {
    use en16931::invoice::{Code, Contact, InvoiceLine, Item, Party, PostalAddress, PriceDetails};
    use en16931::{Date, Invoice, InvoiceAmount, Percentage, Quantity, UnitPriceAmount};
    use energy_billing::en16931_map::EN16931_SPEC_ID;

    let party = |name: &str, vat: Option<&str>, line1: &str, plz: &str, city: &str| Party {
        name: Some(name.to_owned()),
        vat_identifier: vat.map(ToOwned::to_owned),
        address: PostalAddress {
            line1: Some(line1.to_owned()),
            post_code: Some(plz.to_owned()),
            city: Some(city.to_owned()),
            country: Some(Code::from("DE")),
            ..Default::default()
        },
        ..Default::default()
    };

    let line = |id: &str,
                name: &str,
                qty: &str,
                unit: &str,
                price: &str,
                net: &str,
                cat: &str,
                rate: Option<u32>| InvoiceLine {
        id: id.to_owned(),
        quantity: Quantity::from(
            qty.parse::<rust_decimal::Decimal>()
                .expect("specimen quantity"),
        ),
        unit_code: Code::from(unit),
        net_amount: InvoiceAmount::parse(net).expect("specimen line amount"),
        price: PriceDetails {
            net_price: UnitPriceAmount::from(
                price
                    .parse::<rust_decimal::Decimal>()
                    .expect("specimen unit price"),
            ),
            ..Default::default()
        },
        vat: en16931::invoice::LineVat {
            category: Code::from(cat),
            rate: rate.map(|r| Percentage::from(rust_decimal::Decimal::from(r))),
        },
        item: Item {
            name: Some(name.to_owned()),
            ..Default::default()
        },
        // `InvoiceLine` has no `Default`: every one of these is a term with a
        // meaning, and the crate makes you say you do not want it.
        note: None,
        order_line_reference: None,
        accounting_reference: None,
        object_identifier: None,
        period: None,
        allowances: Vec::new(),
        charges: Vec::new(),
    };

    let mut seller = party(
        "Stadtwerke Musterstadt GmbH",
        Some("DE123456789"),
        "Musterstraße 1",
        "12345",
        "Musterstadt",
    );
    seller.contact = Contact {
        name: Some("Kundenservice".to_owned()),
        phone: Some("0800 1234567".to_owned()),
        email: Some("service@stadtwerke-musterstadt.example".to_owned()),
    };
    // BT-34, through the same checked constructor production uses. The value is
    // a real GLN — check digit included — because `eas_checked` refuses one that
    // is not, which is the whole point of using it here rather than `eas`.
    seller.electronic_address =
        Some(en16931::Identifier::eas_checked("9900000000004", "0088").expect("a valid GLN"));

    let mut inv = Invoice::builder(
        EN16931_SPEC_ID,
        "R-2026-000042",
        Date::new(2026, 3, 1).expect("specimen issue date"),
        "380",
        "EUR",
    )
    .seller(seller)
    // No VAT ID on the buyer: a household has none, and a template must render
    // that rather than assume every party has one.
    .buyer(party(
        "Erika Mustermann-Übelacker",
        None,
        "Beispielweg 7",
        "10115",
        "Berlin",
    ))
    .due_in_days(14)
    .payment_terms("Zahlbar bis 15.03.2026 ohne Abzug.")
    .line(line(
        "1",
        "Arbeitspreis Strom (Grundversorgung)",
        "1250",
        "KWH",
        "0.3012",
        "376.50",
        "S",
        Some(19),
    ))
    .line(line(
        "2",
        "Grundpreis Strom",
        "1",
        "MON",
        "12.90",
        "12.90",
        "S",
        Some(19),
    ))
    .line(line(
        "3",
        "Fernwärme Arbeitspreis",
        "800",
        "KWH",
        "0.1050",
        "84.00",
        "S",
        Some(7),
    ))
    .line(line(
        "4",
        "Gutschrift Abschlagszahlung Februar",
        "-1",
        "C62",
        "150.00",
        "-150.00",
        "S",
        Some(19),
    ))
    // Category `E` with BT-152 = **0**, not absent: BR-E-05 requires the rate on
    // an exempt line to be zero. Absent is legal only for category `O` ("not
    // subject to VAT"), and BR-O-11/12 make `O` exclusive — an invoice carrying
    // it may hold no other category — so it cannot appear on a mixed-rate
    // document like this one.
    .line(line(
        "5",
        "Durchlaufender Posten Konzessionsabgabe",
        "1",
        "C62",
        "8.40",
        "8.40",
        "E",
        Some(0),
    ))
    .note("Ihre Marktlokation: 51238696781")
    .note("Der Rechnungsbetrag wird per SEPA-Lastschrift eingezogen.")
    .build();

    inv.invoicing_period = Some(en16931::invoice::Period {
        start: Some(Date::new(2026, 2, 1).expect("specimen period start")),
        end: Some(Date::new(2026, 2, 28).expect("specimen period end")),
    });

    // The terms `crate::einvoice::build` stamps on every production document.
    // Without them the specimen is not a document production could have
    // produced — and, concretely, cannot satisfy XRechnung, which requires all
    // three (PEPPOL-EN16931-R001, -R020 and BR-DE-1). A gate specimen that is
    // less complete than the real thing proves templates against a document
    // shape they will never meet.
    inv.business_process = Some(crate::einvoice::BUSINESS_PROCESS.to_owned());
    inv.payment = Some(en16931::invoice::PaymentInstructions {
        // UNCL 4461 code 58 — SEPA credit transfer.
        means_code: Some(Code::from("58")),
        means_text: Some("SEPA-Überweisung".to_owned()),
        remittance_information: inv.number.clone(),
        means: Some(en16931::invoice::PaymentMeans::CreditTransfer(vec![
            en16931::invoice::CreditTransfer {
                account_identifier: Some("DE89370400440532013000".to_owned()),
                account_name: inv.seller.name.clone(),
                provider_identifier: Some("COBADEFFXXX".to_owned()),
            },
        ])),
    });

    // BG-23 and BG-22 are derived from the lines rather than asserted, so the
    // specimen's totals cannot drift from its positions — which is exactly the
    // guarantee production relies on.
    en16931::reconcile::Reconciler::new()
        .exemption(
            "E",
            Some("Durchlaufender Posten, § 10 Abs. 1 Satz 6 UStG"),
            None,
        )
        .apply(&mut inv)
        .expect("the specimen reconciles");
    inv
}

/// The view a template is proven against — the production projection of
/// [`specimen_invoice`].
#[must_use]
pub fn specimen_view() -> DocumentView {
    DocumentView::of(&specimen_invoice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::REFERENCE_INVOICE_TEMPLATE;

    /// The reference template must survive its own gate.
    ///
    /// This is what keeps the example an operator copies from honest: it is not
    /// documentation that a template *could* be written this way, it is a
    /// template that passes the same admission control theirs will.
    #[test]
    fn the_reference_template_passes_the_gate() {
        let proven = prove(TemplateKind::Invoice, REFERENCE_INVOICE_TEMPLATE, None)
            .expect("the shipped reference template must pass its own gate");
        assert_eq!(proven.proof, Proof::RenderedPdfa);
        assert!(
            proven.warnings.is_empty(),
            "the reference template renders without warnings: {:?}",
            proven.warnings,
        );
        assert!(proven.pages >= 1 && proven.pages <= MAX_SPECIMEN_PAGES);
    }

    /// A template that cannot render an invoice is not published.
    #[test]
    fn a_template_that_does_not_compile_is_refused() {
        assert!(prove(TemplateKind::Invoice, "#let render(i) = [#i.nope]", None).is_err());
        assert!(prove(TemplateKind::Invoice, "#let nicht_render(i) = []", None).is_err());
        assert!(prove(TemplateKind::Invoice, "#let render(i) = [", None).is_err());
    }

    /// A template that renders a *blank page* is refused too.
    ///
    /// It compiles, it is conformant PDF/A-3, and it carries a perfectly
    /// extractable CII invoice. Every machine-side check passes. The document a
    /// customer receives is empty.
    #[test]
    fn a_template_that_prints_nothing_is_refused() {
        let err = prove(TemplateKind::Invoice, "#let render(invoice) = []", None)
            .expect_err("a blank invoice is not an invoice");
        assert!(
            err.to_string().contains("R-2026-000042"),
            "the refusal must name what is missing: {err}",
        );

        // Half an invoice is still refused: the parties are mandatory too.
        assert!(
            prove(
                TemplateKind::Invoice,
                "#let render(invoice) = [Rechnung #invoice.number]",
                None,
            )
            .is_err(),
            "a page with only the invoice number omits § 14 Abs. 4 Nr. 1",
        );
    }

    /// A standard that would silently drop the invoice XML is refused here too.
    #[test]
    fn a_carrier_that_cannot_hold_the_invoice_is_refused() {
        let err = prove(
            TemplateKind::Invoice,
            REFERENCE_INVOICE_TEMPLATE,
            Some("a-2b"),
        )
        .expect_err("PDF/A-2 cannot carry an embedded invoice");
        assert!(
            err.to_string().contains("a-3b"),
            "the refusal must say what to use instead: {err}",
        );
    }

    /// The Textform kinds get the weaker proof, and it is still a proof.
    #[test]
    fn a_textform_template_is_parsed_but_not_rendered() {
        let proven = prove(TemplateKind::Mahnung, "#let render(x) = [Mahnung]", None)
            .expect("a well-formed Textform template parses");
        assert_eq!(proven.proof, Proof::Parsed);

        assert!(
            prove(TemplateKind::Preisanpassung, "#let falsch(x) = []", None).is_err(),
            "a template without the contract function is refused for every kind",
        );
    }

    /// The specimen must exercise what production will.
    ///
    /// A gate that only proves the easy invoice is a gate that lets the
    /// mixed-rate one through untested — and mixed rates are the ordinary case
    /// for a supplier billing Strom and Fernwärme on one document.
    #[test]
    fn the_specimen_is_the_awkward_invoice() {
        let s = specimen_view();
        assert!(
            s.vat_breakdown.len() >= 3,
            "two rates plus an exempt category",
        );
        // The exempt position reaches the breakdown with a rate of `0`, not
        // `None`: `reconcile` fills BT-119 unconditionally because XRechnung's
        // BR-DE-14 requires it even for a category that levies nothing. The
        // *line* still carries no rate, so a template meets both shapes — and
        // must not print "zzgl. 0 % USt" for either.
        let exempt = s
            .vat_breakdown
            .iter()
            .find(|v| v.category == "E")
            .expect("an exempt breakdown entry");
        assert_eq!(exempt.rate.as_deref(), Some("0"));
        assert!(exempt.exemption_reason.is_some(), "BT-120 states why");
        // The exempt line carries BT-152 = 0 (BR-E-05), as does its breakdown
        // entry — so "0" is the shape a template actually meets for a position
        // that levies nothing, and it must not print "zzgl. 0 % USt" for it. A
        // genuinely absent BT-152 is legal only for category `O`, which
        // BR-O-11/12 make exclusive; the template still handles `none`
        // defensively because `DocumentView` types the field as optional.
        let exempt_line = s
            .lines
            .iter()
            .find(|l| l.vat_category == "E")
            .expect("an exempt line");
        assert_eq!(exempt_line.vat_rate.as_deref(), Some("0"));
        assert!(
            s.lines.iter().any(|l| l.net_amount.starts_with('-')),
            "a credit line",
        );
        assert!(
            s.lines
                .iter()
                .any(|l| l.unit_price.split('.').nth(1).is_some_and(|f| f.len() > 2)),
            "a unit price with more than two decimals",
        );
        assert!(s.buyer.vat_id.is_none(), "an absent optional field");
        assert!(
            s.buyer.name.as_deref().is_some_and(|n| !n.is_ascii()),
            "a name with an umlaut in it",
        );
    }
}
