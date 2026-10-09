//! The ZUGFeRD carrier, end to end: template → PDF/A-3 → embedded invoice.
//!
//! `document::gate` proves the same path as admission control. These tests
//! assert on the *artefact* instead — the structures a receiver's validator
//! looks for — because "the gate passed" and "the file is a ZUGFeRD invoice"
//! are only the same statement if the gate is checking the right things.

use outputd::document::facturx::{self, Profile};
use outputd::document::gate;
use outputd::document::render::{RenderRequest, render};
use outputd::document::{DocumentView, REFERENCE_INVOICE_TEMPLATE, RenderError};

/// The specimen invoice for a profile.
///
/// The profile is derived from what the document *says it is* (BT-24), never
/// configured, so producing an `xrechnung.xml` carrier means producing a
/// document that genuinely claims — and satisfies — the CIUS. Claiming it
/// without satisfying it would have this test exercising the carrier with a
/// document no B2G portal would accept, which proves less than it appears to.
///
/// So the XRechnung specimen is completed the way a caller completes one: the
/// receiving authority's BG-7 and the Leitweg-ID that BT-10 and BT-49 need.
/// (In production that completion happens in the issuing service — billingd's
/// `einvoice::apply_b2g_buyer` — because outputd never edits the model; this
/// test builds the same shape with the `en16931` types directly.)
fn specimen(profile: Profile) -> en16931::Invoice {
    use en16931::identifier::Identifier;
    use en16931::invoice::{Code, Contact, Party, PostalAddress};

    let mut model = gate::specimen_invoice();
    if profile == Profile::XRechnung {
        const LEITWEG: &str = "991-33333TEST-33";
        // BG-7: the receiving public authority.
        model.buyer = Party {
            name: Some("Bundesamt für Musterverwaltung".to_owned()),
            // BT-49 under EAS 0204 (German Leitweg-ID).
            electronic_address: Identifier::eas_checked(LEITWEG.to_owned(), "0204").ok(),
            address: PostalAddress {
                line1: Some("Behördenstraße 2".to_owned()),
                city: Some("Bonn".to_owned()),
                post_code: Some("53113".to_owned()),
                country: Some(Code::from("DE")),
                ..Default::default()
            },
            contact: Contact {
                name: Some("Rechnungseingang".to_owned()),
                phone: Some("+49 228 000".to_owned()),
                email: Some("re@bund.example".to_owned()),
            },
            ..Default::default()
        };
        // BT-10 is the last term XRechnung needs that a retail document cannot
        // have, so this is the point the document may honestly claim the CIUS.
        model.buyer_reference = Some(LEITWEG.to_owned());
        model.specification_id = Some(
            "urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0".to_owned(),
        );
    }
    assert_eq!(facturx::profile_of(&model), profile, "BT-24 decides");
    model
}

/// The CII payload the carrier is built around — real, not a placeholder.
///
/// A B2G document goes through `cii::to_string_for`, which **validates against
/// the full CIUS before writing** and refuses to produce a rejectable file — so
/// this returning at all is itself the proof that the XRechnung specimen is a
/// valid XRechnung document.
fn payload(profile: Profile) -> String {
    let model = specimen(profile);
    match profile {
        Profile::XRechnung => {
            en16931_formats::cii::to_string_for(&model, &en16931::profiles::XRECHNUNG)
                .unwrap_or_else(|e| {
                    panic!(
                        "the XRechnung specimen must satisfy the CIUS: {e}\n{}",
                        e.report()
                    )
                })
        }
        _ => en16931_formats::cii::to_string(&model),
    }
}

/// Render the reference template the way production does.
fn zugferd(profile: Profile) -> Vec<u8> {
    let model = specimen(profile);
    let request = RenderRequest {
        template: REFERENCE_INVOICE_TEMPLATE.to_owned(),
        data: Some(serde_json::to_string(&DocumentView::of(&model)).expect("the view serialises")),
        attachment: Some(
            facturx::attachment(profile, payload(profile))
                .expect("the specimen profile can carry an invoice"),
        ),
        standard: Some(gate::DEFAULT_PDF_STANDARD.to_owned()),
        date: gate::SPECIMEN_DATE,
        ident: "zugferd-carrier-test".to_owned(),
    };
    let rendered = render(&request).expect("the reference template renders");
    facturx::stamp(&rendered.pdf, profile).expect("the Factur-X metadata stamps on")
}

/// The stamped file must still be a PDF that parses, with its catalogue intact.
///
/// This is what the incremental update has to preserve. Appending a malformed
/// cross-reference section produces a file that looks fine to `starts_with` and
/// is unreadable to every actual PDF reader — so the check is to *be* a reader.
#[test]
fn the_stamped_file_is_still_a_readable_pdf() {
    let pdf = zugferd(Profile::En16931);
    assert!(pdf.starts_with(b"%PDF-"));

    let doc = lopdf::Document::load_mem(&pdf).expect("the stamped PDF parses");
    let catalog = doc.catalog().expect("the catalogue resolves");
    assert!(catalog.get(b"Pages").is_ok(), "the page tree is reachable");

    // And the update actually took effect: the metadata object the catalogue
    // points at is the *new* one, not the pre-stamp original.
    let metadata = catalog
        .get(b"Metadata")
        .and_then(|r| doc.dereference(r))
        .map(|(_, o)| o)
        .and_then(lopdf::Object::as_stream)
        .expect("the catalogue's /Metadata resolves after the update");
    let xmp = String::from_utf8(metadata.content.clone()).expect("the XMP is UTF-8");
    assert!(
        xmp.contains("<fx:ConformanceLevel>EN 16931</fx:ConformanceLevel>"),
        "the metadata a reader resolves is the stamped one",
    );
    assert!(
        xmp.matches("</rdf:RDF>").count() == 1 && xmp.contains("pdfaExtension:schemas"),
        "one RDF envelope, with the extension schema inside it",
    );
}

/// The file must still claim PDF/A-3, and the claim must be the generator's.
///
/// If stamping replaced or truncated the XMP, `pdfaid:part` would be the first
/// thing to disappear — and a ZUGFeRD file that is not PDF/A is not ZUGFeRD.
#[test]
fn the_pdf_a_claim_survives_stamping() {
    let pdf = zugferd(Profile::En16931);
    let text = String::from_utf8_lossy(&pdf);
    assert!(
        text.contains("pdfaid:part") && text.contains("pdfaid:conformance"),
        "the PDF/A identification is still in the metadata",
    );
    assert!(
        text.contains(">3<") || text.contains("pdfaid:part=\"3\""),
        "the document still identifies as PDF/A-3",
    );
}

/// The invoice comes back out of the finished file, unchanged, and re-parses.
///
/// Byte equality alone would hold for a payload no parser accepts, so this also
/// requires the extracted XML to read back as CII and to be *the same invoice*.
#[test]
fn a_receiver_can_read_the_invoice_back_out() {
    for profile in [Profile::En16931, Profile::XRechnung] {
        let pdf = zugferd(profile);
        let got = facturx::extract(&pdf).expect("the finished document is a ZUGFeRD invoice");

        assert_eq!(
            got.xml,
            payload(profile),
            "the embedded invoice must be byte-identical to what went in",
        );
        assert_eq!(
            got.filename,
            facturx::filename_for(profile).expect("an issuable profile"),
            "the payload is filed under the name its profile requires",
        );
        assert_eq!(got.profile, profile, "BT-24 in the payload agrees");

        let read = got.invoice.expect("the payload reads back as CII");
        assert_eq!(read.number, specimen(profile).number, "same document");
        assert_eq!(
            read.totals.due,
            specimen(profile).totals.due,
            "the amount due survived the carrier",
        );
        assert!(
            got.syntax_findings.is_empty(),
            "the payload is fully within the EN 16931 subset: {:?}",
            got.syntax_findings,
        );
    }
}

/// The carrier's metadata and its payload must not disagree about anything.
///
/// `Divergence` covers the four ways a hybrid invoice is wrong while still
/// opening cleanly: the XMP naming a different profile than BT-24, naming a
/// different filename than the one attached, an `/AFRelationship` that says the
/// XML is supplementary when it *is* the invoice, and no XMP at all. Each is a
/// document that validates and that some receiver processes differently from
/// some other receiver.
#[test]
fn the_finished_document_carries_no_divergence() {
    for profile in [Profile::En16931, Profile::XRechnung] {
        let got = facturx::extract(&zugferd(profile)).expect("readable");
        assert!(
            got.divergence.is_empty(),
            "{profile}: {:?}",
            got.divergence
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        );
    }
}

/// The attachment must be associated with the document, not merely present.
///
/// PDF/A-3 requires an embedded file to appear in the catalogue's `/AF` array
/// with an `/AFRelationship`; ZUGFeRD requires that relationship to be
/// `/Alternative`. A file listed only in the name tree is an ordinary
/// attachment, and a validator will not treat it as invoice data.
#[test]
fn the_invoice_is_an_associated_file_marked_alternative() {
    let pdf = zugferd(Profile::En16931);
    let doc = lopdf::Document::load_mem(&pdf).expect("parses");
    let catalog = doc.catalog().expect("catalogue");

    let af = catalog
        .get(b"AF")
        .and_then(|r| doc.dereference(r))
        .map(|(_, o)| o)
        .and_then(lopdf::Object::as_array)
        .expect("the catalogue carries an /AF array");
    assert!(!af.is_empty(), "/AF names at least the invoice");

    let spec = doc
        .dereference(&af[0])
        .expect("the file spec resolves")
        .1
        .as_dict()
        .expect("a file spec is a dictionary");
    assert_eq!(
        spec.get(b"AFRelationship")
            .and_then(lopdf::Object::as_name)
            .expect("the relationship is stated"),
        b"Alternative",
        "ZUGFeRD requires /Alternative: the XML is another representation of \
         this same document",
    );
}

/// The carrier metadata is derived from the document, so it cannot contradict it.
#[test]
fn the_xmp_agrees_with_the_profile_the_document_declares() {
    for profile in [Profile::En16931, Profile::XRechnung] {
        let got = facturx::extract(&zugferd(profile)).expect("readable");
        assert_eq!(
            got.xmp.conformance_level.as_deref(),
            Some(profile.as_str()),
            "fx:ConformanceLevel must name the profile the payload satisfies",
        );
        assert_eq!(
            got.xmp.document_filename.as_deref(),
            facturx::filename_for(profile),
            "fx:DocumentFileName must name the file actually attached",
        );
        assert_eq!(got.xmp.document_type.as_deref(), Some("INVOICE"));
        // The XMP schema version, which is `1.0` — not the ZUGFeRD version.
        assert_eq!(got.xmp.version.as_deref(), Some("1.0"));
    }
}

/// The whole pipeline is reproducible, stamping included.
///
/// § 147 AO keeps this document for eight years and GoBD requires it to be
/// unchanged. Re-rendering it must therefore produce the same file, not an
/// equivalent one — and that only holds if nothing ambient (a clock, a random
/// `/ID`, a hash-map iteration order) reaches the output.
#[test]
fn rendering_the_same_invoice_twice_produces_the_same_file() {
    assert_eq!(
        zugferd(Profile::En16931),
        zugferd(Profile::En16931),
        "an invoice re-rendered from the same inputs must be byte-identical",
    );
}

/// The operator's template cannot substitute, suppress or read the invoice XML.
///
/// This is the claim the whole layering rests on, so it is asserted directly
/// rather than inferred from the design: the harness owns `pdf.attach`, and the
/// XML is a literal inside it rather than a file anything can read.
#[test]
fn a_template_cannot_interfere_with_the_embedded_invoice() {
    let hostile = r#"
        #let render(invoice) = {
          // Try to read the invoice data that is about to be embedded.
          [#read("/attachment.bin")]
        }
    "#;
    let request = RenderRequest {
        template: hostile.to_owned(),
        data: Some(serde_json::to_string(&gate::specimen_view()).unwrap()),
        attachment: Some(
            facturx::attachment(Profile::En16931, payload(Profile::En16931)).expect("issuable"),
        ),
        standard: Some(gate::DEFAULT_PDF_STANDARD.to_owned()),
        date: gate::SPECIMEN_DATE,
        ident: "hostile".to_owned(),
    };
    assert!(
        matches!(render(&request), Err(RenderError::Compile(_))),
        "a template must not be able to read the invoice it is printed beside",
    );

    // And a template that attaches its own file cannot displace the real one:
    // the harness's attachment is emitted first and keeps the profile's name.
    let squatter = r#"
        #let render(invoice) = {
          pdf.attach("factur-x.xml", bytes("gefälscht"), relationship: "alternative")
          [Rechnung]
        }
    "#;
    let mut request = request;
    request.template = squatter.to_owned();
    let Err(RenderError::Compile(messages)) = render(&request) else {
        panic!("attaching a second file under the same name must fail");
    };
    assert!(
        messages.iter().any(|m| m.contains("twice")),
        "the duplicate attachment is what fails: {messages:?}",
    );
}

/// The § 14 Abs. 4 UStG Pflichtangaben are actually **on the page**.
///
/// Every other test here proves the machine-readable half. This one reads the
/// page the way a customer does, because the two halves are separately capable
/// of being wrong: a template that compiles, conforms and carries a perfect CII
/// invoice can still have dropped the seller's VAT-ID off the layout, and
/// nothing above would notice.
#[test]
fn the_page_carries_the_mandatory_invoice_content() {
    let pdf = zugferd(Profile::En16931);
    let doc = lopdf::Document::load_mem(&pdf).expect("parses");
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc
        .extract_text(&pages)
        .expect("the rendered page has extractable text");
    // Kerning and line breaks make spacing unreliable, so match on tokens.
    let flat: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    let specimen = gate::specimen_view();
    for (term, value) in [
        ("BT-1 invoice number", specimen.number.clone().unwrap()),
        ("BT-27 seller name", specimen.seller.name.clone().unwrap()),
        (
            "BT-31 seller VAT-ID",
            specimen.seller.vat_id.clone().unwrap(),
        ),
        ("BT-44 buyer name", specimen.buyer.name.clone().unwrap()),
        (
            "BT-53 buyer post code",
            specimen.buyer.post_code.clone().unwrap(),
        ),
    ] {
        let needle: String = value.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            flat.contains(&needle),
            "{term} ({value:?}) is missing from the rendered page",
        );
    }

    // § 14 Abs. 4 Nr. 6 — the period of supply, in German date order.
    assert!(
        flat.contains("01.02.2026") && flat.contains("28.02.2026"),
        "the billed period must be on the page",
    );
    // § 14 Abs. 4 Nr. 8 — an exemption must state its reason on the page, not
    // only in the XML.
    assert!(
        flat.contains("DurchlaufenderPosten"),
        "the BT-120 exemption reason must reach the page",
    );
    // The amount due is the model's, to the cent, in German notation — taken
    // from the specimen rather than pinned, so adding a term to the specimen
    // (a BG-20 allowance, say) does not turn this into a stale-literal failure
    // that says nothing about the page.
    let due_de = specimen.totals.due.replace('.', ",");
    assert!(
        flat.contains(&due_de),
        "the amount due ({due_de}) must be printed as a German decimal",
    );

    // BG-20 — the allowance and the base it leaves behind are both on the page.
    // Printing "Summe netto" and then a VAT breakdown on a smaller base, with
    // nothing between them, is a page that does not add up while the embedded
    // XML is correct — the one disagreement no rendering test above would see.
    assert!(!specimen.allowances.is_empty(), "the specimen carries one");
    for a in &specimen.allowances {
        let amount: String = a.amount.replace('.', ",");
        assert!(
            flat.contains(&amount),
            "the BG-20 allowance ({amount}) must reach the page",
        );
        let reason: String = a
            .reason
            .clone()
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(flat.contains(&reason), "and so must its BT-97 reason");
    }
    let taxable_de = specimen.totals.taxable_total.replace('.', ",");
    assert!(
        flat.contains(&taxable_de),
        "the base the VAT is computed on ({taxable_de}) must be on the page",
    );
    // Both VAT rates appear — the mixed-rate case the engine exists for.
    assert!(
        flat.contains("19%") && flat.contains("7%"),
        "every rate in the BG-23 breakdown must be visible",
    );
}

/// Stamping an already-stamped file is refused, not silently doubled.
///
/// Two `fx:ConformanceLevel` values in one XMP packet is worse than either of
/// them: a validator reading the first and a receiver reading the second would
/// disagree about what the document claims to be.
#[test]
fn a_document_is_stamped_exactly_once() {
    let once = zugferd(Profile::En16931);
    let err = facturx::stamp(&once, Profile::En16931).expect_err("a second stamp must be refused");
    assert!(
        err.to_string().contains("already"),
        "the refusal must say why: {err}",
    );
}

/// Write a specimen ZUGFeRD file for **external** validators.
///
/// Not an assertion — a build step, kept here because this is where the
/// pipeline that produces the artefact already lives. `just zugferd-specimen`
/// runs it and prints what to do with the output.
///
/// The two checks mako cannot make on its own need a file to work on:
///
/// - **veraPDF** proves PDF/A-3 conformance. Nothing in Rust does, and the
///   incremental update `document::facturx::stamp` appends is precisely the
///   part `typst-pdf`'s own enforcement cannot vouch for, because it happens
///   after the writer has finished.
/// - the **ZUGFeRD/Factur-X validator** proves the carrier metadata against the
///   specification rather than against a reference implementation.
///
/// `en16931 validate` (the `en16931-cli` crate) covers the payload and reports
/// this file **valid** — 227 rules, 0 findings — which is an independent
/// implementation reading what mako wrote, and is how the missing BT-152 on the
/// exempt line was found.
#[test]
#[ignore = "writes files for veraPDF / the ZUGFeRD validator"]
fn write_specimen_for_external_validators() {
    let out = std::env::var("MAKO_ZUGFERD_OUT")
        .unwrap_or_else(|_| "target/zugferd-specimen.pdf".to_owned());
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("create the output directory");
    }
    let stem = out.trim_end_matches(".pdf");

    // The stamped carriers, one per profile mako issues.
    std::fs::write(&out, zugferd(Profile::En16931)).expect("write the specimen");
    std::fs::write(format!("{stem}-xrechnung.pdf"), zugferd(Profile::XRechnung))
        .expect("write the XRechnung specimen");

    // And the renderer's own output *before* stamping. veraPDF findings on the
    // stamped file are only attributable to `facturx::stamp` if this file is
    // clean — it is the control, not a deliverable.
    let model = specimen(Profile::En16931);
    let unstamped = render(&RenderRequest {
        template: REFERENCE_INVOICE_TEMPLATE.to_owned(),
        data: Some(serde_json::to_string(&DocumentView::of(&model)).expect("serialises")),
        attachment: Some(
            facturx::attachment(Profile::En16931, payload(Profile::En16931)).expect("issuable"),
        ),
        standard: Some(gate::DEFAULT_PDF_STANDARD.to_owned()),
        date: gate::SPECIMEN_DATE,
        ident: "zugferd-carrier-test".to_owned(),
    })
    .expect("renders")
    .pdf;
    std::fs::write(format!("{stem}-unstamped.pdf"), unstamped).expect("write the control");
    println!("wrote {out}, {stem}-xrechnung.pdf, {stem}-unstamped.pdf");
}

/// The stamped XMP packet is well-formed XML, and says what it must.
///
/// `facturx::stamp` splices two `rdf:Description` blocks into the metadata
/// stream as a **string**. Every other test here checks it with `contains`,
/// which passes just as happily on a packet that is no longer parseable — an
/// unbalanced tag, a stray `&`, a broken namespace. veraPDF would catch that;
/// nothing in this repo would. So parse it.
///
/// This is the one PDF/A property we can check without veraPDF: the metadata
/// stream must be a well-formed XMP packet, and every `fx:` property must be
/// covered by the extension schema description or PDF/A rejects the file for
/// using an undeclared namespace.
#[test]
fn the_stamped_xmp_is_well_formed_and_self_describing() {
    let pdf = zugferd(Profile::En16931);
    let doc = lopdf::Document::load_mem(&pdf).expect("parses");
    let stream = doc
        .catalog()
        .expect("catalogue")
        .get(b"Metadata")
        .and_then(|r| doc.dereference(r))
        .map(|(_, o)| o)
        .and_then(lopdf::Object::as_stream)
        .expect("the metadata stream");
    // PDF/A requires the metadata stream to be unfiltered, which is also what
    // lets `stamp` splice into it at all.
    assert!(
        stream.dict.get(b"Filter").is_err(),
        "a PDF/A metadata stream must not be compressed",
    );
    let xmp = std::str::from_utf8(&stream.content).expect("the XMP is UTF-8");

    let parsed = roxmltree::Document::parse(xmp)
        .unwrap_or_else(|e| panic!("the stamped XMP is not well-formed XML: {e}"));

    // Every fx: property the packet asserts...
    let fx = "urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#";
    let asserted: Vec<String> = parsed
        .descendants()
        .filter(|n| n.tag_name().namespace() == Some(fx))
        .map(|n| n.tag_name().name().to_owned())
        .collect();
    assert!(
        asserted.contains(&"ConformanceLevel".to_owned()),
        "the fx: namespace must resolve to the specified URI, not just appear as a prefix: {asserted:?}",
    );

    // ...must be described by *the fx schema's own* extension-schema entry.
    // Scoped to that entry rather than to the whole packet: krilla writes its
    // own extension schema too, so a packet-wide search would let an fx property
    // be "described" by an unrelated schema — which PDF/A does not accept and
    // which would make this check meaningless.
    const SCHEMA_NS: &str = "http://www.aiim.org/pdfa/ns/schema#";
    const PROPERTY_NS: &str = "http://www.aiim.org/pdfa/ns/property#";
    let fx_schema = parsed
        .descendants()
        .find(|n| {
            n.children().any(|c| {
                c.tag_name().namespace() == Some(SCHEMA_NS)
                    && c.tag_name().name() == "namespaceURI"
                    && c.text() == Some(fx)
            })
        })
        .expect("an extension-schema entry declaring the fx namespace");
    assert_eq!(
        fx_schema
            .descendants()
            .find(|n| n.tag_name().namespace() == Some(SCHEMA_NS)
                && n.tag_name().name() == "prefix")
            .and_then(|n| n.text()),
        Some("fx"),
    );
    let described: Vec<String> = fx_schema
        .descendants()
        .filter(|n| n.tag_name().namespace() == Some(PROPERTY_NS) && n.tag_name().name() == "name")
        .filter_map(|n| n.text().map(ToOwned::to_owned))
        .collect();
    for property in &asserted {
        assert!(
            described.contains(property),
            "fx:{property} is asserted but not described in the fx extension \
             schema — PDF/A rejects an undeclared property. Described: {described:?}",
        );
    }
    assert_eq!(
        described.len(),
        4,
        "the fx schema describes exactly its four properties: {described:?}",
    );

    // The XMP data model allows each property **once** per packet, and
    // `pdfaExtension:schemas` is a property. Typst/krilla already writes it;
    // mako's schema entry must join that bag. A second occurrence is not a
    // style issue: Adobe-lineage XMP parsers reject the whole packet as
    // unparseable — veraPDF then reports "serialized incorrectly", a null
    // encoding, and a missing PDF/A identification, all three of which
    // happened. roxmltree and expat both accept the duplicate, which is why
    // only veraPDF could find it and why this pin exists.
    assert_eq!(
        xmp.matches("<pdfaExtension:schemas>").count(),
        1,
        "pdfaExtension:schemas must appear exactly once in the packet",
    );
}

/// Customer-controlled text cannot become Typst markup on the page.
///
/// The view reaches the template as JSON read via `json()`, and Typst renders a
/// string in content position as literal text — so a buyer named `#emph[x]` or
/// `= Überschrift` prints those characters rather than styling the invoice.
/// That is the design assumption; this pins it, because the failure mode is a
/// counterparty-controlled name silently reformatting (or, with `#read`,
/// probing) the document of every customer who shares a template.
#[test]
fn customer_text_is_content_not_markup() {
    let mut view = gate::specimen_view();
    view.buyer.name = Some("#emph[MARKUP] = Überschrift <tag> \\u{1F4A9}".to_owned());
    view.notes = vec!["#read(\"/document.json\")".to_owned()];

    let rendered = render(&RenderRequest {
        template: REFERENCE_INVOICE_TEMPLATE.to_owned(),
        data: Some(serde_json::to_string(&view).expect("serialises")),
        attachment: None,
        standard: None,
        date: gate::SPECIMEN_DATE,
        ident: "injection-probe".to_owned(),
    })
    .expect("hostile text must render, as text");

    let doc = lopdf::Document::load_mem(&rendered.pdf).expect("parses");
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc.extract_text(&pages).expect("text");
    let flat: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    // The markup characters appear literally on the page…
    assert!(
        flat.contains("#emph[MARKUP]"),
        "a function call in a name must print, not execute",
    );
    assert!(
        flat.contains(r#"#read("/document.json")"#.replace(' ', "").as_str()),
        "#read in a note must print, not read",
    );
    // …and did not execute: emphasised text would still contain the word, so
    // the discriminating assertion is on the *sigils* surviving, plus the
    // document not having grown a heading (a heading would re-run the page
    // counter’s expectations; cheap proxy: page count unchanged).
    assert_eq!(
        rendered.pages, 1,
        "hostile text must not restructure the page"
    );
}

/// Write the reference Mahnung render for visual inspection. Not an assertion.
#[test]
#[ignore = "writes a file for visual inspection (pdftoppm -r 150)"]
fn write_mahnung_specimen_for_visual_inspection() {
    let out = std::env::var("MAKO_ZUGFERD_OUT")
        .unwrap_or_else(|_| "target/mahnung-specimen.pdf".to_owned());
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("create dir");
    }
    let rendered = render(&RenderRequest {
        template: outputd::document::REFERENCE_MAHNUNG_TEMPLATE.to_owned(),
        data: Some(
            serde_json::to_string(&outputd::document::mahnung::specimen()).expect("serialises"),
        ),
        attachment: None,
        standard: None,
        date: gate::SPECIMEN_DATE,
        ident: "mahnung-visual".to_owned(),
    })
    .expect("renders");
    std::fs::write(&out, &rendered.pdf).expect("write");
    println!("wrote {out} ({} pages)", rendered.pages);
}

/// Write a **multi-page** render for visual inspection. Not an assertion.
///
/// Sammelrechnung and VPP documents carry dozens of lines, and nothing else in
/// the suite ever renders past page one — whether `table.header` repeats on the
/// break and the footer's `Seite n von m` counts correctly are properties only
/// eyes can check. Forty lines forces at least two pages; the gate's own page
/// cap does not apply here because this is not the gate specimen.
#[test]
#[ignore = "writes a file for visual inspection (pdftoppm -r 150)"]
fn write_multipage_specimen_for_visual_inspection() {
    use outputd::document::{LineView, VatView};

    let mut model = specimen(Profile::En16931);
    // Repeat the first line often enough to break the page. The amounts stop
    // reconciling with BG-22 — irrelevant here, nothing validates this file;
    // it exists to look at, not to send.
    let mut view = DocumentView::of(&model);
    let template_line = view.lines[0].clone();
    view.lines = (1..=40)
        .map(|i| LineView {
            id: i.to_string(),
            name: Some(format!("Arbeitspreis Strom Zone {i} (Grundversorgung)")),
            ..template_line.clone()
        })
        .collect();
    view.vat_breakdown = vec![VatView {
        category: "S".to_owned(),
        rate: Some("19".to_owned()),
        taxable_amount: "15060.00".to_owned(),
        tax_amount: "2861.40".to_owned(),
        exemption_reason: None,
    }];
    model.number = Some("R-2026-000099".to_owned());

    let out = std::env::var("MAKO_ZUGFERD_OUT")
        .unwrap_or_else(|_| "target/zugferd-multipage.pdf".to_owned());
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).expect("create dir");
    }
    let rendered = render(&RenderRequest {
        template: REFERENCE_INVOICE_TEMPLATE.to_owned(),
        data: Some(serde_json::to_string(&view).expect("serialises")),
        attachment: None,
        standard: None,
        date: gate::SPECIMEN_DATE,
        ident: "multipage-visual".to_owned(),
    })
    .expect("renders");
    assert!(rendered.pages >= 2, "40 lines must break the page");
    std::fs::write(&out, &rendered.pdf).expect("write");
    println!("wrote {out} ({} pages)", rendered.pages);
}

/// The incremental update changes **exactly one object** and nothing else.
///
/// This is the whole claim of `facturx::stamp`'s design, and until now it was
/// only argued in prose. veraPDF is what proves PDF/A conformance in general;
/// this proves the narrower thing mako is actually responsible for — that
/// appending the update did not disturb a document whose conformance the
/// generator had already established.
///
/// It walks every object in the pre-stamp file and requires the post-stamp file
/// to resolve each to identical content, with the single exception of the
/// `/Metadata` stream. Anything else — a shifted offset, a clobbered object
/// number, a broken `/Prev` chain — shows up here as a difference on an object
/// nobody meant to touch.
///
/// The trailer terms are checked by name because the update writes its own
/// trailer: `/ID` is a PDF/A requirement and the file's identity across
/// revisions, and `/Root` and `/Size` must carry over unchanged.
#[test]
fn stamping_disturbs_nothing_except_the_metadata_stream() {
    let model = specimen(Profile::En16931);
    let rendered = render(&RenderRequest {
        template: REFERENCE_INVOICE_TEMPLATE.to_owned(),
        data: Some(serde_json::to_string(&DocumentView::of(&model)).expect("serialises")),
        attachment: Some(
            facturx::attachment(Profile::En16931, payload(Profile::En16931)).expect("issuable"),
        ),
        standard: Some(gate::DEFAULT_PDF_STANDARD.to_owned()),
        date: gate::SPECIMEN_DATE,
        ident: "stamp-isolation".to_owned(),
    })
    .expect("renders");

    let before = lopdf::Document::load_mem(&rendered.pdf).expect("the unstamped PDF parses");
    let stamped = facturx::stamp(&rendered.pdf, Profile::En16931).expect("stamps");
    let after = lopdf::Document::load_mem(&stamped).expect("the stamped PDF parses");

    // The one object that is allowed to differ.
    let metadata_id = match before.catalog().expect("catalogue").get(b"Metadata") {
        Ok(lopdf::Object::Reference(id)) => *id,
        other => panic!("expected a /Metadata reference, got {other:?}"),
    };

    // The appended revision must be strictly additive: same objects, same ids.
    assert_eq!(
        before.objects.len(),
        after.objects.len(),
        "an incremental update that redefines one object adds no new ones",
    );

    let mut changed = Vec::new();
    for (id, original) in &before.objects {
        let updated = after
            .objects
            .get(id)
            .unwrap_or_else(|| panic!("object {id:?} disappeared from the stamped file"));
        if format!("{original:?}") != format!("{updated:?}") {
            changed.push(*id);
        }
    }
    assert_eq!(
        changed,
        vec![metadata_id],
        "exactly the /Metadata object may differ; these did: {changed:?}",
    );

    // The trailer the update writes by hand.
    for key in [&b"Root"[..], b"Size"] {
        let name = String::from_utf8_lossy(key).into_owned();
        let old = before
            .trailer
            .get(key)
            .unwrap_or_else(|_| panic!("the generator wrote /{name}"));
        let new = after
            .trailer
            .get(key)
            .unwrap_or_else(|_| panic!("/{name} must survive the incremental update"));
        assert_eq!(
            format!("{old:?}"),
            format!("{new:?}"),
            "/{name} must carry over unchanged",
        );
    }

    // `/ID` is compared by **value**, not by spelling. PDF has two string
    // syntaxes and they are interchangeable: krilla emits the identifiers as
    // literal strings `(..)` and `facturx::stamp` re-spells them as hex `<..>`,
    // which is the same bytes and is what every reader compares. Asserting on
    // the syntax would fail on a file that is correct — this test flagged
    // exactly that difference the first time it ran.
    let ids = |doc: &lopdf::Document| -> Vec<Vec<u8>> {
        doc.trailer
            .get(b"ID")
            .and_then(lopdf::Object::as_array)
            .expect("PDF/A requires a file identifier")
            .iter()
            .map(|o| o.as_str().expect("an identifier is a string").to_vec())
            .collect()
    };
    assert_eq!(
        ids(&before),
        ids(&after),
        "/ID is the file's identity across revisions and must carry over",
    );
    assert_eq!(ids(&after).len(), 2, "/ID is a two-element array");

    // And the page tree still renders the same document.
    assert_eq!(before.get_pages().len(), after.get_pages().len());
}
