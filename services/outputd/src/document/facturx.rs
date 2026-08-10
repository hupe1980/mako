//! The ZUGFeRD / Factur-X carrier: making a PDF/A-3 file *be* an e-invoice.
//!
//! A PDF with the CII XML stapled inside it is not yet a ZUGFeRD document.
//! ZUGFeRD 2.3 (and Factur-X 1.0, the same specification under its French name)
//! requires four things of the carrier, and a receiver's validator checks all
//! four:
//!
//! 1. **PDF/A-3** conformance — enforced by the renderer, refused at publish.
//! 2. **The right filename** — `factur-x.xml`, or `xrechnung.xml` for the
//!    XRECHNUNG profile. Not a convention: a receiver looks the file up by name.
//! 3. **`AFRelationship /Alternative`** and membership of the catalogue's `/AF`
//!    array — written by Typst from the harness's `pdf.attach`.
//! 4. **XMP metadata** declaring the Factur-X schema — `fx:DocumentType`,
//!    `fx:DocumentFileName`, `fx:Version`, `fx:ConformanceLevel`, plus the
//!    PDF/A *extension schema description* that makes those four properties
//!    legal in a PDF/A file at all.
//!
//! Typst writes (1)–(3). It cannot write (4): `typst-pdf` exposes no hook for
//! custom XMP, and krilla's metadata surface has no extension-schema concept.
//! So [`stamp`] adds it — and without it, everything mako produced would be a
//! well-formed PDF/A-3 that no ZUGFeRD validator would accept as an invoice.
//!
//! # How the XMP is added
//!
//! By **incremental update**: the original bytes are never touched, and a new
//! definition of the existing metadata object is appended along with a
//! cross-reference section that points at it. This is the same mechanism a
//! digital signature uses.
//!
//! Rewriting the file instead — parse, mutate, re-serialise — would be less
//! code and considerably worse. A PDF/A file's conformance is a property of its
//! exact structure, and re-serialising it through a general-purpose PDF library
//! risks changing something the generator was careful about, silently, in a
//! document nobody re-validates. Appending cannot: every byte `typst-pdf`
//! produced is still there, in order.
//!
//! # Reading is not ours
//!
//! Only the *writing* half lives here. [`extract`] delegates to
//! `en16931-formats`' own reader, which walks the same catalogue route a
//! receiver does and additionally reports every [`Divergence`] between what a
//! PDF declares and what it contains. mako had its own name-tree walk and its
//! own XMP scan; both are gone, and the checks they could not make — profile
//! against BT-24, `/AFRelationship` against the profile, payload re-parsed as
//! CII — are now part of the publish gate.
//!
//! The [`Profile`] vocabulary is upstream's too. A private enum that knew about
//! two of six profiles is exactly how a MINIMUM document — which carries no
//! lines and is *not* an EN 16931 invoice — ends up wrapped in a carrier
//! claiming it is one.
//!
//! # Sources
//!
//! The extension schema description is the one PDFlib authored for the
//! Factur-X 1.0 info package and every reference implementation ships verbatim;
//! the conformance-level spellings (`EN 16931`, with the space) are
//! `Profile::as_str` upstream, corroborated against the Factur-X reference
//! implementation's profile table. See `EN16931_FEEDBACK.md` at the repository
//! root for the sources and for what was fed back to that crate's authors.

use anyhow::{Context as _, Result, bail};

use super::render::{Attachment, Relationship};

/// The XMP namespace URI of the Factur-X schema.
///
/// The mixed case in `CrossIndustryDocument` is required by the specification.
/// Some PDFs in circulation use an all-lowercase spelling; those are wrong, and
/// a validator that is strict about the URI rejects them.
const FX_NS: &str = "urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#";

/// `fx:Version` — the version of the *Factur-X XMP schema*, not of ZUGFeRD.
/// It has been `1.0` since Factur-X 1.0 and is not expected to move.
const FX_VERSION: &str = "1.0";

/// The ZUGFeRD profile vocabulary. Re-exported rather than redefined.
///
/// `en16931-formats` already models the profile matrix, including the trap that
/// **MINIMUM and BASIC WL are not EN 16931 invoices** — they carry no lines, so
/// they cannot satisfy BR-16. mako had its own two-variant enum before adopting
/// this one; a private enum that knows about two of six profiles is exactly how
/// a MINIMUM document ends up being validated as though it were an invoice.
pub use en16931_formats::zugferd::{Divergence, Extracted, IsInvoice, Profile, Xmp};

/// The profile a document declares in BT-24.
///
/// Not a configuration option. BT-24 is the document's own statement of which
/// specification it satisfies, and the carrier metadata must agree with it — a
/// PDF whose XMP claims a profile the XML does not satisfy is exactly the
/// mismatch a validator exists to find. Deriving the profile from BT-24 makes
/// disagreement unrepresentable.
#[must_use]
pub fn profile_of(model: &en16931::Invoice) -> Profile {
    model
        .specification_id
        .as_deref()
        .map_or(Profile::Unknown, Profile::parse)
}

/// The name the embedded invoice is filed under, for a profile mako *writes*.
///
/// `en16931-formats` deliberately takes no position on this for writing — it
/// knows the filenames a reader must accept ([`en16931_formats::zugferd::FILENAMES`])
/// but not which one a writer should choose. mako writes exactly two profiles
/// and the choice is unambiguous for both, so the decision lives here.
///
/// # Errors
///
/// A profile mako does not issue. `None` for MINIMUM and BASIC WL in
/// particular: they are not EN 16931 invoices, so wrapping one in a ZUGFeRD
/// carrier that claims to hold an invoice would be a false claim.
#[must_use]
pub fn filename_for(profile: Profile) -> Option<&'static str> {
    match profile {
        Profile::XRechnung => Some("xrechnung.xml"),
        Profile::En16931 | Profile::Basic | Profile::Extended => Some("factur-x.xml"),
        // MINIMUM and BASIC WL are not EN 16931 invoices; `Unknown` is a
        // profile mako does not recognise. `Profile` is `#[non_exhaustive]`, so
        // a variant added upstream lands here — refusing to name a file for a
        // profile we have not considered is the safe default.
        _ => None,
    }
}

/// The attachment as the renderer should embed it.
///
/// # Errors
///
/// A profile that cannot carry an invoice — see [`filename_for`].
pub fn attachment(profile: Profile, xml: String) -> Result<Attachment> {
    if let IsInvoice::No(why) = profile.is_en16931_invoice() {
        bail!("{profile} cannot be issued as a ZUGFeRD invoice: {why}");
    }
    let filename = filename_for(profile)
        .with_context(|| format!("mako does not issue the {profile} profile"))?;
    Ok(Attachment {
        filename: filename.to_owned(),
        data: xml,
        mime_type: "application/xml".to_owned(),
        // `Profile::as_str` *is* the `fx:ConformanceLevel` vocabulary.
        description: format!("Rechnungsdaten (ZUGFeRD, {profile})"),
        // The XML is another representation of this same document, which is
        // what `Alternative` means. `Data` would say the page visualises it and
        // `Source` that the page was derived from it; both are claims about
        // provenance that are not true here. `en16931-formats` refuses to pick
        // a default because published guidance disagrees for cross-border
        // Factur-X — mako's recipients are German, so the German reading
        // applies and the choice is made here rather than assumed there.
        relationship: Relationship::Alternative,
    })
}

/// The `rdf:Description` carrying the four `fx:` properties.
fn fx_properties(profile: Profile, filename: &str) -> String {
    format!(
        r#"  <rdf:Description rdf:about="" xmlns:fx="{FX_NS}">
    <fx:DocumentType>INVOICE</fx:DocumentType>
    <fx:DocumentFileName>{filename}</fx:DocumentFileName>
    <fx:Version>{FX_VERSION}</fx:Version>
    <fx:ConformanceLevel>{level}</fx:ConformanceLevel>
  </rdf:Description>
"#,
        level = profile,
    )
}

/// The fx schema's PDF/A extension-schema entry — one `rdf:li` for a
/// `pdfaExtension:schemas` bag.
///
/// PDF/A permits XMP properties outside the predefined schemas only if the file
/// *describes* them, so without this entry the four `fx:` properties make the
/// document non-conformant. It is emitted as a bag **item**, not as a complete
/// `rdf:Description`, because of an XMP data-model rule that veraPDF enforces
/// and XML well-formedness does not: a property — and `pdfaExtension:schemas`
/// is one — may appear **once** per packet. Typst/krilla already writes that
/// property to describe its own extension schemas, so mako's entry must join
/// the existing bag; a second `pdfaExtension:schemas` renders the whole packet
/// unparseable to Adobe-lineage XMP parsers, which veraPDF reports as
/// "serialized incorrectly", encoding null, and — because nothing can be read —
/// a missing PDF/A identification schema. Found by running veraPDF, and only
/// findable that way: roxmltree and expat both accept the duplicate.
fn fx_schema_entry() -> String {
    format!(
        r#"<rdf:li rdf:parseType="Resource"><pdfaSchema:schema>Factur-X PDFA Extension Schema</pdfaSchema:schema><pdfaSchema:namespaceURI>{FX_NS}</pdfaSchema:namespaceURI><pdfaSchema:prefix>fx</pdfaSchema:prefix><pdfaSchema:property><rdf:Seq><rdf:li rdf:parseType="Resource"><pdfaProperty:name>DocumentFileName</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>name of the embedded XML invoice file</pdfaProperty:description></rdf:li><rdf:li rdf:parseType="Resource"><pdfaProperty:name>DocumentType</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>INVOICE</pdfaProperty:description></rdf:li><rdf:li rdf:parseType="Resource"><pdfaProperty:name>Version</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>The actual version of the Factur-X XML schema</pdfaProperty:description></rdf:li><rdf:li rdf:parseType="Resource"><pdfaProperty:name>ConformanceLevel</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>The conformance level of the embedded Factur-X data</pdfaProperty:description></rdf:li></rdf:Seq></pdfaSchema:property></rdf:li>"#
    )
}

/// The fallback for a packet with **no** extension schemas at all: a complete
/// `rdf:Description` owning the (then unique) `pdfaExtension:schemas` property.
fn fx_schema_description() -> String {
    format!(
        r#"  <rdf:Description rdf:about=""
      xmlns:pdfaExtension="http://www.aiim.org/pdfa/ns/extension/"
      xmlns:pdfaSchema="http://www.aiim.org/pdfa/ns/schema#"
      xmlns:pdfaProperty="http://www.aiim.org/pdfa/ns/property#">
    <pdfaExtension:schemas>
      <rdf:Bag>
        {}
      </rdf:Bag>
    </pdfaExtension:schemas>
  </rdf:Description>
"#,
        fx_schema_entry()
    )
}

/// Add the Factur-X XMP to a rendered PDF/A-3 file.
///
/// Returns the same document with one object redefined by an appended
/// incremental update.
///
/// # Errors
///
/// When the input is not a PDF this can update in place — no catalogue, no
/// metadata stream, no `</rdf:RDF>` to extend — or when it has **already** been
/// stamped. All of those mean the renderer produced something unexpected, or a
/// caller called twice; none of them mean the operator did anything wrong.
pub fn stamp(pdf: &[u8], profile: Profile) -> Result<Vec<u8>> {
    // Resolved before anything is written: the filename goes into
    // `fx:DocumentFileName`, and guessing it for a profile mako does not issue
    // would put a name in the metadata that does not match the attachment —
    // which is `Divergence::Filename`, produced by us.
    let filename = filename_for(profile)
        .with_context(|| format!("mako does not issue the {profile} profile"))?;
    let doc = lopdf::Document::load_mem(pdf).context("the rendered PDF does not parse")?;

    let catalog = doc.catalog().context("the rendered PDF has no catalogue")?;
    let (number, generation) = match catalog.get(b"Metadata") {
        Ok(lopdf::Object::Reference(id)) => *id,
        _ => bail!("the rendered PDF has no /Metadata stream to extend"),
    };
    let stream = doc
        .get_object((number, generation))
        .and_then(lopdf::Object::as_stream)
        .context("the /Metadata entry does not point at a stream")?;
    // PDF/A requires the metadata stream to be unfiltered, which is also what
    // makes splicing into it safe. If it ever arrives compressed, refuse rather
    // than write a stream whose declared filter no longer describes it.
    if stream.dict.get(b"Filter").is_ok() {
        bail!("the /Metadata stream is filtered; PDF/A requires it to be plain XMP");
    }
    let xmp = std::str::from_utf8(&stream.content).context("the XMP packet is not UTF-8")?;
    // Stamping is not idempotent — a second pass would declare the schema twice
    // and leave two conflicting `fx:ConformanceLevel` values in one packet, which
    // is worse than either of them. Refuse rather than corrupt.
    if xmp.contains(FX_NS) {
        bail!(
            "this PDF already carries Factur-X metadata; stamping it again would declare the schema twice"
        );
    }

    // The fx properties become a sibling `rdf:Description` inside the same RDF
    // envelope; the extension-schema entry joins the generator's existing
    // `pdfaExtension:schemas` bag when there is one — see [`fx_schema_entry`]
    // for why joining is mandatory rather than tidy.
    let close = xmp
        .rfind("</rdf:RDF>")
        .context("the XMP packet has no </rdf:RDF> to extend")?;
    let mut updated = String::with_capacity(xmp.len() + 4096);
    updated.push_str(&xmp[..close]);
    updated.push_str(&fx_properties(profile, filename));
    updated.push_str(&xmp[close..]);

    const SCHEMAS_OPEN: &str = "<pdfaExtension:schemas>";
    updated = if let Some(at) = updated.find(SCHEMAS_OPEN) {
        let bag = updated[at..]
            .find("<rdf:Bag>")
            .map(|o| at + o + "<rdf:Bag>".len())
            .context("the pdfaExtension:schemas property carries no rdf:Bag")?;
        format!(
            "{}{}{}",
            &updated[..bag],
            fx_schema_entry(),
            &updated[bag..]
        )
    } else {
        // No extension schemas anywhere: the property is ours alone to write.
        let close = updated.rfind("</rdf:RDF>").expect("spliced above");
        format!(
            "{}{}{}",
            &updated[..close],
            fx_schema_description(),
            &updated[close..]
        )
    };

    let prev =
        last_startxref(pdf).context("the rendered PDF has no startxref to chain an update onto")?;
    Ok(append_object(
        pdf,
        &doc,
        prev,
        number,
        generation,
        updated.as_bytes(),
    ))
}

/// The byte offset the file's last `startxref` points at.
///
/// Read from the bytes rather than from the parsed document: it is the value
/// the appended trailer's `/Prev` must carry, and taking it from the file is
/// the only way to be sure it describes *this* file.
fn last_startxref(pdf: &[u8]) -> Option<usize> {
    let needle = b"startxref";
    let at = pdf
        .windows(needle.len())
        .rposition(|window| window == needle)?
        + needle.len();
    // Only the number follows, so a bounded lossy read is enough — and cannot be
    // derailed by whatever bytes come after the `%%EOF`.
    let end = (at + 40).min(pdf.len());
    String::from_utf8_lossy(&pdf[at..end])
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Append an incremental update redefining one stream object.
///
/// Redefining the *existing* object number is what keeps this small: the
/// catalogue still points at `number`, so nothing else in the file has to
/// change, and the appended cross-reference section needs exactly one entry.
fn append_object(
    original: &[u8],
    doc: &lopdf::Document,
    prev: usize,
    number: u32,
    generation: u16,
    content: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(original.len() + content.len() + 1024);
    out.extend_from_slice(original);
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }

    let offset = out.len();
    out.extend_from_slice(
        format!(
            "{number} {generation} obj\n\
             << /Type /Metadata /Subtype /XML /Length {} >>\nstream\n",
            content.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(content);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    // `/Size` is unchanged: the update redefines an object, it does not add one.
    // Copied from the original trailer rather than recomputed, so a file whose
    // numbering is sparser than its object count keeps the size it declared.
    let size = doc
        .trailer
        .get(b"Size")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or_else(|| i64::from(doc.max_id) + 1);

    let xref = out.len();
    out.extend_from_slice(
        format!(
            "xref\n{number} 1\n{offset:010} {generation:05} n \ntrailer\n\
             << /Size {size} /Root {root} /Prev {prev}",
            root = reference(doc.trailer.get(b"Root").ok()),
        )
        .as_bytes(),
    );
    // `/ID` must survive an update: it is the file's identity across revisions,
    // and PDF/A requires it to be present.
    if let Ok(id) = doc.trailer.get(b"ID") {
        out.extend_from_slice(b" /ID ");
        out.extend_from_slice(id_literal(id).as_bytes());
    }
    out.extend_from_slice(format!(" >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

/// `n g R`, or a reference to object 1 when the trailer is malformed — which
/// cannot happen for a file `typst-pdf` just produced.
fn reference(object: Option<&lopdf::Object>) -> String {
    match object {
        Some(lopdf::Object::Reference((n, g))) => format!("{n} {g} R"),
        _ => "1 0 R".to_owned(),
    }
}

/// Re-spell the trailer `/ID` array as PDF hex strings.
fn id_literal(object: &lopdf::Object) -> String {
    let Ok(items) = object.as_array() else {
        return String::new();
    };
    let mut out = String::from("[");
    for item in items {
        out.push('<');
        if let Ok(bytes) = item.as_str() {
            for b in bytes {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{b:02X}"));
            }
        }
        out.push('>');
    }
    out.push(']');
    out
}

/// Read the invoice back out of a finished ZUGFeRD PDF.
///
/// A thin wrapper over [`en16931_formats::zugferd::extract`], which walks the
/// catalogue's `/Names` `/EmbeddedFiles` tree exactly as a receiver's reader
/// does, resolves the filename by the profile-preference order, parses the
/// payload back into an [`en16931::Invoice`], reads the `/AFRelationship` and
/// the `fx:` XMP, and reports any [`Divergence`] between what the PDF *declares*
/// and what it *contains*.
///
/// mako had its own name-tree walk here. Deleting it removed about a hundred
/// lines and gained three checks it never made — see [`crate::document::gate`],
/// which now asserts the divergence set is empty rather than comparing a
/// filename and a conformance level by hand.
///
/// # Errors
///
/// Propagates [`en16931_formats::zugferd::Error`]: not a readable PDF, no
/// embedded invoice under any recognised name (naming what *was* attached), or
/// a payload that is not UTF-8.
pub fn extract(pdf: &[u8]) -> Result<Extracted, en16931_formats::zugferd::Error> {
    en16931_formats::zugferd::extract(pdf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BT-24 decides the profile, and the profile decides both the filename and
    /// the conformance level — they can never be set independently.
    #[test]
    fn the_profile_follows_what_the_document_declares() {
        let mut model = en16931::Invoice::default();
        model.specification_id = Some("urn:cen.eu:en16931:2017".to_owned());
        assert_eq!(profile_of(&model), Profile::En16931);
        assert_eq!(filename_for(profile_of(&model)), Some("factur-x.xml"));
        assert_eq!(profile_of(&model).as_str(), "EN 16931");

        model.specification_id = Some(
            "urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0".to_owned(),
        );
        assert_eq!(profile_of(&model), Profile::XRechnung);
        assert_eq!(filename_for(profile_of(&model)), Some("xrechnung.xml"));
        assert_eq!(profile_of(&model).as_str(), "XRECHNUNG");
    }

    /// A model with no BT-24 is `Unknown`, not silently the core profile.
    ///
    /// Defaulting an unrecognised profile to EN 16931 is the permissive answer
    /// and therefore the wrong one: it would wrap an unknown document in a
    /// carrier claiming it satisfies the core model.
    #[test]
    fn a_document_that_declares_nothing_is_not_assumed_to_be_an_invoice() {
        let model = en16931::Invoice::default();
        assert_eq!(profile_of(&model), Profile::Unknown);
        assert_eq!(filename_for(Profile::Unknown), None);
        assert!(attachment(Profile::Unknown, "<x/>".to_owned()).is_err());
        assert!(stamp(b"%PDF-1.7\n", Profile::Unknown).is_err());
    }

    /// A profile that is not an EN 16931 invoice cannot be issued as one.
    ///
    /// MINIMUM and BASIC WL carry no lines and cannot satisfy BR-16. mako never
    /// produces them — but the guard is what makes "every ZUGFeRD file mako
    /// issues holds an invoice" a property rather than a habit.
    #[test]
    fn a_profile_that_is_not_an_invoice_is_refused() {
        for profile in [Profile::Minimum, Profile::BasicWl] {
            assert!(
                matches!(profile.is_en16931_invoice(), IsInvoice::No(_)),
                "{profile} is not an EN 16931 invoice",
            );
            let err = attachment(profile, "<x/>".to_owned())
                .expect_err("must not be issued as an invoice");
            assert!(
                err.to_string().contains("BR-16"),
                "the refusal must say why: {err}",
            );
        }
    }

    /// The conformance level is `EN 16931` — with the space, in capitals.
    ///
    /// It is a fixed string from the Factur-X profile table, not a spelling of
    /// the standard's name, and a validator compares it literally.
    #[test]
    fn the_conformance_level_is_the_spelling_the_specification_uses() {
        let xmp = fx_properties(Profile::En16931, "factur-x.xml");
        assert!(xmp.contains("<fx:ConformanceLevel>EN 16931</fx:ConformanceLevel>"));
        assert!(xmp.contains("<fx:DocumentFileName>factur-x.xml</fx:DocumentFileName>"));
        assert!(xmp.contains("<fx:DocumentType>INVOICE</fx:DocumentType>"));
        assert!(xmp.contains("<fx:Version>1.0</fx:Version>"));
    }

    /// The extension schema description must declare all four properties.
    ///
    /// PDF/A rejects an undescribed property, so an incomplete description
    /// would make the document non-conformant in a way only a PDF/A validator
    /// reports — long after the invoices went out.
    #[test]
    fn every_property_is_described_for_pdf_a() {
        // The standalone fallback carries the same entry the merge inserts, so
        // asserting on it covers both paths' content.
        let xmp = fx_schema_description();
        for property in [
            "DocumentFileName",
            "DocumentType",
            "Version",
            "ConformanceLevel",
        ] {
            assert!(
                xmp.contains(&format!(
                    "<pdfaProperty:name>{property}</pdfaProperty:name>"
                )),
                "{property} is used but not described",
            );
        }
        assert!(xmp.contains(&format!(
            "<pdfaSchema:namespaceURI>{FX_NS}</pdfaSchema:namespaceURI>"
        )));
        assert!(xmp.contains("<pdfaSchema:prefix>fx</pdfaSchema:prefix>"));
    }

    /// The namespace URI keeps its mixed case.
    #[test]
    fn the_namespace_uri_is_the_specified_spelling() {
        assert_eq!(
            FX_NS,
            "urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#"
        );
        assert!(FX_NS.ends_with('#'), "the trailing # is part of the URI");
    }
}
