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
    /// Rendered against the kind's specimen and the page carried its mandatory
    /// content — the full proof for a Textform document, which has no PDF/A to
    /// meet and nothing to embed. What MAHNUNG and PREISANPASSUNG require.
    ///
    /// There is no weaker level: every kind has a specimen, so every template
    /// is proven against one.
    RenderedTextform,
}

impl Proof {
    /// The `document_templates.proof` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RenderedPdfa => "RENDERED_PDFA",
            Self::RenderedTextform => "RENDERED_TEXTFORM",
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

/// PEPPOL BIS Billing 3.0 business process (BT-23) — stamped on the specimen so
/// it matches what billingd's production mapping stamps. One constant, two
/// services: the value is a published PEPPOL identifier, not shared state.
const BUSINESS_PROCESS: &str = "urn:fdc:peppol.eu:2017:poacc:billing:01:1.0";

/// Prove a template, or refuse it.
///
/// # Errors
///
/// Any failure to render, to conform, or to get the XML back out — with the
/// message the operator needs to fix their template.
pub fn prove(kind: TemplateKind, source: &str, pdf_standard: Option<&str>) -> Result<Proven> {
    match kind {
        TemplateKind::Invoice => prove_invoice(source, pdf_standard),
        TemplateKind::Mahnung => prove_mahnung(source),
        TemplateKind::Preisanpassung => prove_preisanpassung(source),
    }
}

/// The Textform proof for a **Preisanpassung**: render the mixed-change
/// specimen, then read the page back.
///
/// § 41 Abs. 5 EnWG makes this letter's *content* a form requirement, so the
/// gate checks the page prints it:
///
/// * the **declarant** (§ 126b BGB), without which it is not Textform;
/// * the **Wirksamkeit**, the date the new prices apply (Satz 1);
/// * **both** changed prices, including the one that goes down — a template
///   printing only the first position, or assuming every price rises, is
///   refused (Satz 1, *Umfang*);
/// * the **Sonderkündigungsrecht** date (Satz 4), which Satz 1 obliges the
///   supplier to state in the same notice. A page that announces the price and
///   omits the right is not a valid Preisänderungsanzeige.
fn prove_preisanpassung(source: &str) -> Result<Proven> {
    let view = super::preisanpassung::specimen();
    let rendered = render(&RenderRequest {
        template: source.to_owned(),
        data: Some(serde_json::to_string(&view)?),
        attachment: None,
        standard: None,
        date: SPECIMEN_DATE,
        ident: "mako-publish-gate-preisanpassung".to_owned(),
    })?;
    if rendered.pages > 3 {
        bail!(
            "the specimen Preisanpassung came to {} pages; a price-change notice over three \
             pages has a layout fault",
            rendered.pages,
        );
    }

    let doc = lopdf::Document::load_mem(&rendered.pdf).context("reading back the render")?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc
        .extract_text(&pages)
        .context("a Textform document must carry extractable text")?;
    let flat = page_text(&text);

    let declarant = view.absender.name.clone().unwrap_or_default();
    if declarant.is_empty() {
        bail!(
            "the Preisanpassung specimen carries no declarant name — the § 126b check cannot run"
        );
    }
    // German customer-facing renderings, as in `prove_mahnung`: the view
    // carries ISO values and the reference template prints them the way a
    // German letter reads.
    let required: [(&str, String); 5] = [
        ("§ 126b declarant", declarant),
        ("Wirksamkeit der Preisänderung", "01.05.2026".to_owned()),
        ("neuer Arbeitspreis", "37,20".to_owned()),
        // The line that falls. A template printing only the first position, or
        // one that hard-codes an increase, fails here.
        ("neuer Grundpreis", "131,40".to_owned()),
        (
            "§ 41 Abs. 5 Satz 4 Sonderkündigungsrecht",
            "01.05.2026".to_owned(),
        ),
    ];
    for (term, value) in required {
        let needle = needle_text(&value);
        if !contains_standalone(&flat, &needle) {
            bail!(
                "the rendered Preisanpassung does not print the {term} (`{value}`, in its \
                 German customer-facing form) — § 41 Abs. 5 EnWG makes it part of the notice, \
                 so a template that omits it produces an invalid Preisänderungsanzeige"
            );
        }
    }

    Ok(Proven {
        proof: Proof::RenderedTextform,
        pages: rendered.pages,
        warnings: rendered.warnings,
    })
}

/// The Textform proof: render the Stufe-3 specimen, then read the page back.
///
/// No carrier, no PDF/A — a Mahnung is § 126b Textform. What the page must
/// say instead: the **declarant** (§ 126b names it as a requirement of the
/// form), the **Gesamtforderung** (a demand without an amount demands
/// nothing), the **Zahlungsfrist**, and — because the specimen is Stufe 3 —
/// the § 41f Sperrtermin, so a template cannot silently drop the one block
/// with statutory form requirements attached.
fn prove_mahnung(source: &str) -> Result<Proven> {
    let view = super::mahnung::specimen();
    let rendered = render(&RenderRequest {
        template: source.to_owned(),
        data: Some(serde_json::to_string(&view)?),
        attachment: None,
        standard: None,
        date: SPECIMEN_DATE,
        ident: "mako-publish-gate-mahnung".to_owned(),
    })?;
    if rendered.pages > 3 {
        bail!(
            "the specimen Mahnung came to {} pages; a dunning letter over three pages has a layout fault",
            rendered.pages,
        );
    }

    let doc = lopdf::Document::load_mem(&rendered.pdf).context("reading back the render")?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc
        .extract_text(&pages)
        .context("a Textform document must carry extractable text")?;
    let flat = page_text(&text);

    // German customer-facing renderings, deliberately: the view carries ISO
    // values (`523.40`, `2026-03-15`) and the reference template formats them
    // the way a German Mahnung reads (`523,40`, `15.03.2026`) — a template
    // printing the raw ISO forms fails here, and the message says what was
    // expected. The declarant needle comes from the view, and an empty needle
    // would make its check vacuously true (`contains("")` always holds), so it
    // is refused rather than skipped.
    let declarant = view.absender.name.clone().unwrap_or_default();
    if declarant.is_empty() {
        bail!("the Mahnung specimen carries no declarant name — the § 126b check cannot run");
    }
    let required: [(&str, String); 4] = [
        ("§ 126b declarant", declarant),
        ("Gesamtforderung", "523,40".to_owned()),
        ("Zahlungsfrist", "15.03.2026".to_owned()),
        ("§ 41f Sperrtermin", "01.04.2026".to_owned()),
    ];
    for (term, value) in required {
        let needle = needle_text(&value);
        if !contains_standalone(&flat, &needle) {
            bail!(
                "the rendered Mahnung does not print the {term} (`{value}`, in its German \
                 customer-facing format) — a Textform document without it does not meet its form"
            );
        }
    }

    Ok(Proven {
        proof: Proof::RenderedTextform,
        warnings: rendered.warnings,
        pages: rendered.pages,
    })
}

/// The page's text, normalised for the content checks.
///
/// Whitespace runs collapse to **one space** rather than vanishing: removing it
/// entirely runs adjacent table cells together, so an ordinary two-column price
/// table (`34,90` | `37,20`) flattens to `34,9037,20` and
/// [`contains_standalone`] reads the second amount as embedded in the first.
/// One space still absorbs the line breaks and inter-cell padding a PDF text
/// extraction produces.
fn page_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            in_space = true;
        } else {
            if in_space && !out.is_empty() {
                out.push(' ');
            }
            in_space = false;
            out.push(c);
        }
    }
    out
}

/// A needle in the same normalisation as [`page_text`].
fn needle_text(value: &str) -> String {
    page_text(value)
}

/// Whether `haystack` contains `needle` *not embedded in a larger number* —
/// no digit or thousands separator directly before it, no digit directly
/// after. Plain `contains` would accept a Mahnung that misprints the
/// Gesamtforderung `523,40` as `1.523,40`: the wrong amount contains the right
/// one as a suffix, and the whole point of the check is that the printed
/// number is the demanded number.
fn contains_standalone(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(needle) {
        let at = from + pos;
        let ok_before = haystack[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_digit() && c != '.' && c != ',');
        let ok_after = haystack[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_digit());
        if ok_before && ok_after {
            return true;
        }
        from = at + 1;
    }
    false
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
    // Against plain EN 16931, which is what the specimen declares —
    // `specimen_invoice` stamps `EN16931_SPEC_ID` unconditionally, so there is
    // no BT-24 to dispatch on. Dispatching anyway would be a third copy of a
    // decision that already lives in billingd's `einvoice::validate`, guarding
    // a branch the specimen can never take. A document billingd renders is
    // validated by billingd, where the profile is not a constant.
    let fatal: Vec<String> = en16931::validation::validate(&model)
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
    let xml = en16931_formats::cii::to_string(&model);

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
/// mandatory: the invoice number (Nr. 4), the full names of both parties
/// (Nr. 1), and the seller's tax identifier (Nr. 2). They are not a matter of
/// layout taste — a document without them is not a Rechnung — so requiring them
/// constrains nothing an operator may legitimately want to do.
///
/// Nr. 2 is checked as the **disjunction** the statute actually writes: the
/// USt-IdNr. *or* the Steuernummer, either one satisfying it. A template is
/// free to print whichever it prefers, and free to print both.
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
    let flat = page_text(&text);

    for (term, value) in [
        ("BT-1 Rechnungsnummer (§ 14 Abs. 4 Nr. 4)", &model.number),
        ("BT-27 seller name (§ 14 Abs. 4 Nr. 1)", &model.seller.name),
        ("BT-44 buyer name (§ 14 Abs. 4 Nr. 1)", &model.buyer.name),
    ] {
        let Some(value) = value else { continue };
        let needle = needle_text(value);
        if !flat.contains(&needle) {
            bail!(
                "the rendered page does not print {term}: `{value}` is nowhere on it. \
                 The invoice XML would be correct and the document a customer receives \
                 would not be a valid Rechnung"
            );
        }
    }

    // § 14 Abs. 4 Nr. 2 UStG is a disjunction — the USt-IdNr. *or* the
    // Steuernummer — so it is checked as one, and only when the model supplies
    // at least one of them.
    let tax_ids: Vec<&String> = [
        model.seller.vat_identifier.as_ref(),
        model.seller.tax_registration.as_ref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !tax_ids.is_empty() {
        let printed = tax_ids.iter().any(|value| {
            let needle = needle_text(value);
            flat.contains(&needle)
        });
        if !printed {
            bail!(
                "the rendered page prints neither the seller's USt-IdNr. (BT-31) nor their \
                 Steuernummer (BT-32) — § 14 Abs. 4 Nr. 2 UStG requires one of them. \
                 The model offered {tax_ids:?} and the page carries none of it"
            );
        }
    }
    Ok(())
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
/// `en16931_formats::cii::to_string`, so "the invoice survives the carrier" is a
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
    /// BT-24 of the core specimen — the EN 16931 spec id, not any CIUS.
    /// (Matches `energy_billing::en16931_map::EN16931_SPEC_ID`; inlined so the
    /// renderer does not depend on a billing-domain crate.)
    const EN16931_SPEC_ID: &str = "urn:cen.eu:en16931:2017";

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
    // BT-32 alongside BT-31: § 14 Abs. 4 Nr. 2 UStG requires one of the two, and
    // the specimen carries both so a template may print either.
    seller.tax_registration = Some("123/456/78901".to_owned());
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
    //
    // The exempt position is a *genuine* durchlaufender Posten: under the WiM
    // Rechnungsabwicklung des MSB über den LF (QUOTES 15002 / ORDERS 17005),
    // the LF collects the MSB's Messentgelt in the MSB's name and for the
    // MSB's account — exactly § 10 Abs. 1 Satz 4 UStG. A Konzessionsabgabe is
    // not one: it is collected in the supplier's own name as part of the
    // Entgelt and is subject to VAT.
    .line(line(
        "5",
        "Durchlaufender Posten: Messentgelt MSB (in fremdem Namen vereinnahmt)",
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

    // The terms billingd's `einvoice::build` stamps on every production document.
    // Without them the specimen is not a document production could have
    // produced — and, concretely, cannot satisfy XRechnung, which requires all
    // three (PEPPOL-EN16931-R001, -R020 and BR-DE-1). A gate specimen that is
    // less complete than the real thing proves templates against a document
    // shape they will never meet.
    inv.business_process = Some(BUSINESS_PROCESS.to_owned());
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
            Some("Durchlaufender Posten, § 10 Abs. 1 Satz 4 UStG"),
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

    /// The Gesamtforderung check must not accept the right digits inside the
    /// wrong number.
    ///
    /// `"1.523,40".contains("523,40")` is true — so plain substring matching
    /// would pass a template that misprints the demanded amount, and the whole
    /// point of the check is that the printed number is the demanded number.
    #[test]
    fn an_amount_inside_a_larger_number_does_not_count_as_printed() {
        assert!(contains_standalone("Gesamtforderung:523,40EUR", "523,40"));
        assert!(!contains_standalone(
            "Gesamtforderung:1.523,40EUR",
            "523,40"
        ));
        assert!(!contains_standalone("Gesamtforderung:8523,40EUR", "523,40"));
        // Trailing digits extend the number too: 523,401 is not 523,40.
        assert!(!contains_standalone("Betrag523,401EUR", "523,40"));
        // A later clean occurrence still counts even after an embedded one.
        assert!(contains_standalone("alt:1.523,40neu:523,40", "523,40"));
        // Dates keep working: dotted dates match only as themselves.
        assert!(contains_standalone("zahlbarbis15.03.2026.", "15.03.2026"));
        assert!(!contains_standalone("bis115.03.2026", "15.03.2026"));
    }

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

    /// A page that names both parties and the number but no tax identifier is
    /// refused: § 14 Abs. 4 Nr. 2 UStG is mandatory too.
    #[test]
    fn a_page_without_a_seller_tax_identifier_is_refused() {
        let err = prove(
            TemplateKind::Invoice,
            "#let render(invoice) = [Rechnung #invoice.number \
             #invoice.seller.name #invoice.buyer.name]",
            None,
        )
        .expect_err("§ 14 Abs. 4 Nr. 2 UStG is not optional");
        assert!(
            err.to_string().contains("Nr. 2"),
            "the refusal must name the term it is missing: {err}",
        );
    }

    /// Either half of the disjunction satisfies it, on its own — a seller with
    /// only a Steuernummer prints a lawful page.
    #[test]
    fn either_tax_identifier_alone_satisfies_nr_2() {
        let head = "#let render(invoice) = [Rechnung #invoice.number \
                    #invoice.seller.name #invoice.buyer.name ";
        for (label, field) in [
            ("USt-IdNr.", "#invoice.seller.vat_id"),
            ("Steuernummer", "#invoice.seller.tax_number"),
        ] {
            prove(TemplateKind::Invoice, &format!("{head}{field}]"), None)
                .unwrap_or_else(|e| panic!("{label} alone satisfies § 14 Abs. 4 Nr. 2: {e}"));
        }
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

    /// The reference Mahnung passes its own gate, at the Textform proof.
    #[test]
    fn the_reference_mahnung_passes_the_gate() {
        let proven = prove(
            TemplateKind::Mahnung,
            crate::document::REFERENCE_MAHNUNG_TEMPLATE,
            None,
        )
        .expect("the shipped Mahnung template must pass its own gate");
        assert_eq!(proven.proof, Proof::RenderedTextform);
        assert!(
            proven.warnings.is_empty(),
            "no warnings: {:?}",
            proven.warnings
        );
    }

    /// A Mahnung that renders but omits its mandatory content is refused.
    ///
    /// `#let render(x) = [Mahnung]` parses, and a parse proof would accept it.
    /// The page-content check is what refuses it: a dunning letter naming
    /// neither declarant, amount, deadline nor the § 41f Sperrtermin is not a
    /// Mahnung in any form the statute recognises.
    #[test]
    fn a_mahnung_that_prints_nothing_is_refused() {
        let err = prove(TemplateKind::Mahnung, "#let render(x) = [Mahnung]", None)
            .expect_err("an empty dunning letter is not a Mahnung");
        assert!(
            err.to_string().contains("declarant") || err.to_string().contains("Gesamtforderung"),
            "the refusal names what is missing: {err}",
        );
    }

    /// A PREISANPASSUNG template that compiles but prints no § 41 Abs. 5 EnWG
    /// content is refused: rolled out, it would make every price-change notice
    /// the operator sends invalid.
    #[test]
    fn a_preisanpassung_that_prints_nothing_is_refused() {
        let err = prove(
            TemplateKind::Preisanpassung,
            "#let render(x) = [Wir passen unsere Preise an.]",
            None,
        )
        .expect_err("a page with no statutory content is not a Preisänderungsanzeige")
        .to_string();
        assert!(
            err.contains("§ 126b declarant"),
            "the refusal names what is missing: {err}"
        );
        assert!(
            prove(TemplateKind::Preisanpassung, "#let falsch(x) = []", None).is_err(),
            "a template without the contract function is refused for every kind",
        );
    }

    /// Two amounts in neighbouring table cells are two amounts, not one —
    /// while the "not embedded in a larger number" guard still refuses a
    /// misprinted figure.
    #[test]
    fn neighbouring_cells_are_not_one_number() {
        let page = page_text("Arbeitspreis ct/kWh   34,90\n   37,20\n");
        assert!(contains_standalone(&page, &needle_text("37,20")));
        assert!(contains_standalone(&page, &needle_text("34,90")));
        // …and the guard it exists for still holds: a misprinted
        // `1.523,40` does not satisfy a demand for `523,40`.
        let wrong = page_text("Gesamtforderung 1.523,40 EUR");
        assert!(!contains_standalone(&wrong, &needle_text("523,40")));
    }

    /// The reference layout mako ships satisfies its own gate — including the
    /// price line that goes **down**, which a template assuming every price
    /// rises renders wrong.
    #[test]
    fn the_reference_preisanpassung_passes_its_own_gate() {
        let proven = prove(
            TemplateKind::Preisanpassung,
            crate::document::REFERENCE_PREISANPASSUNG,
            None,
        )
        .expect("the shipped Preisanpassung layout must pass the gate");
        assert_eq!(proven.proof, Proof::RenderedTextform);
    }

    /// The specimen's stamped terms match what billingd's production stamps.
    ///
    /// The specimen is hand-built, so it can drift from `billingd::einvoice::build`
    /// — and it had: it was once missing BT-23, BT-34 and BG-16, proving
    /// templates against a document shape production never emits. The old
    /// in-process equality check died with the extraction; the tripwire is now
    /// two-sided: billingd's `einvoice_render.rs::production_stamps_the_terms_the_gate_specimen_proves_templates_against`
    /// pins production to these same expected values.
    #[test]
    fn the_specimen_carries_the_terms_production_stamps() {
        let specimen = specimen_invoice();
        assert_eq!(
            specimen.business_process.as_deref(),
            Some("urn:fdc:peppol.eu:2017:poacc:billing:01:1.0"),
            "BT-23 business process",
        );
        assert_eq!(
            specimen
                .seller
                .electronic_address
                .as_ref()
                .and_then(|i| i.scheme()),
            Some("0088"),
            "BT-34 seller electronic address, EAS 0088 (GLN)",
        );
        assert_eq!(
            specimen
                .payment
                .as_ref()
                .and_then(|p| p.means_code.as_ref().map(en16931::invoice::Code::as_str)),
            Some("58"),
            "BG-16 payment instructions with the SEPA means code (UNCL 4461 58)",
        );
        assert!(specimen.invoicing_period.is_some(), "BG-14 billing period");
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
