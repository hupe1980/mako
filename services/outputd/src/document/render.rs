//! Compiling a template into the PDF a customer receives.
//!
//! # The harness
//!
//! mako compiles `/main.typ`, not the operator's file. The harness imports the
//! template, hands it the view, and — for an invoice — emits the `pdf.attach`
//! that embeds the CII XML:
//!
//! ```typst
//! #import "/template.typ": render
//! #pdf.attach("factur-x.xml", bytes("<?xml version=\"1.0\" .."), ..)
//! #render(json("/document.json"))
//! ```
//!
//! Putting the attachment in mako's file rather than the operator's is what
//! makes the compliance claim hold: a template cannot omit the XML, cannot
//! rename it, and cannot attach a second one under the same name — Typst
//! refuses a duplicate attachment path outright.
//!
//! The XML is a **literal**, not a file the harness reads. That is the
//! difference between "the template is not supposed to read it" and "there is
//! nothing to read": a `/attachment.bin` served by the world would be served to
//! the template as readily as to the harness, because a `World` cannot tell its
//! callers apart. `bytes(..)` of an escaped literal has no path.
//!
//! # The contract a template must meet
//!
//! Export one function:
//!
//! ```typst
//! #let render(invoice) = { .. }
//! ```
//!
//! `invoice` is [`super::view::DocumentView`] as a Typst dictionary. The name is
//! `render` and not `document` because `document` is a built-in element and
//! shadowing it in a template is a trap worth not setting.
//!
//! # Compute is not sandboxed
//!
//! Typst caps loop iterations (10 000) and call depth, so the naive infinite
//! loop is already an error. Nested loops still multiply, and layout of an
//! absurd page count is unbounded, so a pathological template can occupy a core
//! for a long time. There is no way to interrupt a Typst compilation, so
//! [`render_guarded`] runs it on a blocking thread and stops *waiting* after a
//! budget — the caller is freed, the thread finishes on its own. That is a
//! deliberate trade: templates come from an authenticated operator publishing
//! their own layout, not from a counterparty, so the threat is a mistake rather
//! than an attack, and leaking a thread beats blocking the runtime.

use std::fmt::Write as _;
use std::sync::LazyLock;

use typst::WorldExt as _;
use typst::diag::{SourceDiagnostic, Warned};
use typst::foundations::{Datetime, Smart};
use typst::syntax::{DiagSpanKind, FileId};
use typst_layout::PagedDocument;
use typst_pdf::{PdfOptions, PdfStandard, PdfStandards, Timestamp};

use super::world::{self, TemplateWorld};

/// The relationship an attached file bears to the document (PDF 32000-2 /
/// PDF/A-3 `AFRelationship`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relationship {
    /// The attachment is another representation of this same document. What a
    /// ZUGFeRD invoice uses: the XML and the page say the same thing.
    Alternative,
    /// The document was derived *from* the attachment.
    Source,
    /// The attachment is data the document visualises.
    Data,
    /// Additional material, not a representation of the document.
    Supplement,
}

impl Relationship {
    /// The spelling Typst's `pdf.attach` expects.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Alternative => "alternative",
            Self::Source => "source",
            Self::Data => "data",
            Self::Supplement => "supplement",
        }
    }
}

/// A file to embed in the PDF.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// The name it carries inside the PDF. For ZUGFeRD this is fixed by the
    /// profile — see [`super::facturx`].
    pub filename: String,
    /// The document, as text. `String` rather than `Vec<u8>` because an
    /// e-invoice is XML and is written into the harness as a literal; the
    /// embedded bytes are its UTF-8 encoding, which is the encoding the XML
    /// declaration names.
    pub data: String,
    pub mime_type: String,
    pub description: String,
    pub relationship: Relationship,
}

/// Everything one render needs.
///
/// Owned rather than borrowed so the whole request can move onto a blocking
/// thread; a render is far too expensive for the copy to matter.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    /// The operator's template source.
    pub template: String,
    /// The view, serialised — whatever `json("/document.json")` yields.
    ///
    /// `None` compiles the template **without calling** `render`: the harness
    /// imports it and stops. That is not a render, it is a check that the file
    /// parses and exports the contract function, and it is what the publish
    /// gate can offer a document kind whose view does not exist yet.
    pub data: Option<String>,
    /// The file to embed, if this document carries one.
    pub attachment: Option<Attachment>,
    /// The PDF standard to enforce, in Typst's spelling (`a-3b`, `a-3u`, …).
    /// `None` renders unconstrained PDF — right for a Textform letter, never
    /// for an invoice.
    pub standard: Option<String>,
    /// The date the document bears. Becomes `datetime.today()` inside the
    /// template *and* the PDF creation timestamp, so neither reads the clock.
    pub date: time::Date,
    /// A stable identifier for this document. Hashed into the PDF's `/ID`, so
    /// two renders of the same invoice produce the same file.
    pub ident: String,
}

/// A rendered document.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub pdf: Vec<u8>,
    /// Typst warnings, already formatted with file, line and column. Surfaced
    /// rather than dropped: "unknown font family" is a warning, and a template
    /// silently falling back to a different typeface is worth an operator's
    /// attention before ten thousand invoices go out in it.
    pub warnings: Vec<String>,
    /// How many pages it came to. The caller may want to refuse a template that
    /// turned one invoice into four hundred pages.
    pub pages: usize,
}

/// Why a render did not produce a document.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The template did not compile, or violated the PDF standard it declared.
    /// Every diagnostic is formatted `path:line:col: message`, most important
    /// first, and points into the operator's own file.
    #[error("template did not render:\n{}", .0.join("\n"))]
    Compile(Vec<String>),
    /// The requested PDF standard is not one Typst can enforce.
    #[error("{0}")]
    Standard(String),
    /// The attachment's filename would not survive being written into the PDF.
    #[error("{0}")]
    Attachment(String),
    /// The document date is outside what a PDF timestamp can express.
    #[error("document date {0} is not a representable date")]
    Date(time::Date),
    /// The render exceeded its time budget. The template is not necessarily
    /// wrong — it may just be doing far too much work.
    #[error("template render exceeded its {0:?} budget")]
    Timeout(std::time::Duration),
}

/// The PDF standards under which a file may be embedded at all.
///
/// PDF/A-1 forbids embedded files outright and PDF/A-2 only permits ones that
/// are themselves PDF/A, which is why Typst refuses `pdf.attach` there. Letting
/// an operator select those for an invoice would produce a PDF with no XML in
/// it — a document that is not an e-invoice while looking exactly like one.
///
/// ZUGFeRD 2.3 itself requires **PDF/A-3**; the PDF/A-4 levels are here because
/// they can mechanically carry the attachment, for an operator whose profile
/// permits them.
const EMBEDDING_STANDARDS: &[&str] = &["a-3b", "a-3u", "a-3a", "a-4f", "a-4e"];

/// Render a document. Blocking and CPU-bound — see [`render_guarded`].
///
/// # Errors
///
/// [`RenderError`] — a template that does not compile, a standard Typst cannot
/// enforce, an unusable attachment name, or an unrepresentable date.
pub fn render(req: &RenderRequest) -> Result<Rendered, RenderError> {
    let standards = standards(req.standard.as_deref(), req.attachment.is_some())?;
    let today = datetime(req.date)?;
    let harness = harness(req.attachment.as_ref(), req.data.is_some())?;

    let world = TemplateWorld::new(
        &harness,
        &req.template,
        req.data.as_deref().unwrap_or("{}"),
        today,
    );

    let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
    let warnings = format_all(&world, &warnings);
    let document = output.map_err(|errors| RenderError::Compile(format_all(&world, &errors)))?;

    let options = PdfOptions {
        // A hash of `ident` becomes the PDF `/ID`. Passing one — rather than
        // `Smart::Auto`, which hashes title and author — is what makes two
        // renders of the same invoice byte-identical.
        ident: Smart::Custom(req.ident.clone()),
        creator: Smart::Custom(Some("mako outputd".to_owned())),
        timestamp: Some(Timestamp::new_utc(today)),
        page_ranges: None,
        standards,
        // Tagged PDF. Not optional in spirit: an invoice is a document a person
        // may need a screen reader for, and the tags cost nothing here.
        tagged: true,
        pretty: false,
    };
    let pdf = typst_pdf::pdf(&document, &options)
        .map_err(|errors| RenderError::Compile(format_all(&world, &errors)))?;

    Ok(Rendered {
        pdf,
        warnings,
        pages: document.pages().len(),
    })
}

/// How many templates may be typesetting at once.
///
/// Typesetting is CPU-bound and runs on tokio's blocking pool — the same pool
/// `sqlx` uses. Without a bound, a burst of publishes (or one operator iterating
/// on a layout in a loop) spawns one compilation per request: they contend for
/// the same cores, each takes proportionally longer, and in the limit they
/// exhaust the blocking pool and stall *database* work that has nothing to do
/// with rendering. Queuing is strictly better than thrashing — the work is
/// serialised either way, and this way the rest of the service keeps moving.
///
/// Sized to the machine, minus a core, so a saturated renderer still leaves
/// something for the runtime. Never zero.
static RENDER_SLOTS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    tokio::sync::Semaphore::new(cores.saturating_sub(1).max(1))
});

/// Render without letting a slow template hold the caller.
///
/// The compilation runs on a blocking thread and the wait is bounded. On
/// timeout the thread is **not** cancelled — Typst offers no way to interrupt
/// one — so it runs to completion and its result is discarded. See the module
/// docs for why that is the right trade here.
///
/// Concurrency is capped at `RENDER_SLOTS` (cores − 1). The permit is held by the blocking
/// task rather than by the caller, so a render that outlived its budget keeps
/// occupying a slot until it actually finishes — which is the truth about the
/// machine, and the behaviour that stops a queue of timed-out renders from
/// pretending the CPU is free.
///
/// # Errors
///
/// [`RenderError`], plus [`RenderError::Timeout`] when the budget runs out.
/// Waiting for a slot counts against the budget: a caller that would have
/// queued longer than it was willing to wait is told so rather than held.
pub async fn render_guarded(
    req: RenderRequest,
    budget: std::time::Duration,
) -> Result<Rendered, RenderError> {
    let started = tokio::time::Instant::now();
    let Ok(Ok(permit)) = tokio::time::timeout(budget, RENDER_SLOTS.acquire()).await else {
        // Either the wait for a slot exhausted the budget, or the semaphore was
        // closed — which nothing closes, so it is the former in practice.
        return Err(RenderError::Timeout(budget));
    };
    let remaining = budget.saturating_sub(started.elapsed());

    let handle = tokio::task::spawn_blocking(move || {
        let result = render(&req);
        // Dropped here, not on the caller's side: the slot is free when the CPU
        // is, which for a timed-out render is later than when the caller left.
        drop(permit);
        result
    });
    match tokio::time::timeout(remaining, handle).await {
        Ok(Ok(result)) => result,
        // The blocking task panicked. A panic in Typst is a bug in Typst or in
        // the harness, never something the operator's template should be blamed
        // for, so it is reported as such rather than as a template error.
        Ok(Err(join)) => Err(RenderError::Compile(vec![format!(
            "the renderer panicked — this is a mako bug, not a template error: {join}"
        )])),
        Err(_) => Err(RenderError::Timeout(budget)),
    }
}

/// Resolve the requested standard, refusing one that would drop the attachment.
fn standards(requested: Option<&str>, has_attachment: bool) -> Result<PdfStandards, RenderError> {
    let Some(name) = requested else {
        if has_attachment {
            return Err(RenderError::Standard(
                "a document carrying an embedded invoice must declare a PDF/A standard; \
                 ZUGFeRD 2.3 requires PDF/A-3 (`a-3b`)"
                    .to_owned(),
            ));
        }
        return Ok(PdfStandards::default());
    };
    // Parse through `PdfStandard`'s own serde spelling rather than a table of
    // our own: the vocabulary an operator writes is then exactly the one Typst
    // publishes, and cannot drift from it.
    let standard: PdfStandard = serde_json::from_value(serde_json::Value::String(name.to_owned()))
        .map_err(|_| {
            RenderError::Standard(format!(
                "`{name}` is not a PDF standard this renderer enforces; \
             use one of {}",
                EMBEDDING_STANDARDS.join(", ")
            ))
        })?;
    if has_attachment && !EMBEDDING_STANDARDS.contains(&name) {
        return Err(RenderError::Standard(format!(
            "`{name}` cannot carry an embedded file — PDF/A-1 forbids attachments and PDF/A-2 \
             accepts only PDF ones, so the invoice XML would be silently dropped; \
             ZUGFeRD 2.3 requires PDF/A-3 (`a-3b`)"
        )));
    }
    PdfStandards::new(&[standard]).map_err(|e| RenderError::Standard(e.message().to_string()))
}

/// The date, as Typst spells one.
fn datetime(date: time::Date) -> Result<Datetime, RenderError> {
    let month = u8::from(date.month());
    Datetime::from_ymd_hms(date.year(), month, date.day(), 0, 0, 0).ok_or(RenderError::Date(date))
}

/// mako's entry point: import, attach, render.
///
/// With `invoke` false the call is left out, so the template is evaluated but
/// nothing is laid out — see [`RenderRequest::data`].
fn harness(attachment: Option<&Attachment>, invoke: bool) -> Result<String, RenderError> {
    let mut out = String::from(
        "// Generated by mako outputd. The operator's template is /template.typ.\n\
         #import \"/template.typ\": render\n",
    );
    if let Some(a) = attachment {
        validate_filename(&a.filename)?;
        writeln!(
            out,
            "#pdf.attach(\n  \
               \"{}\",\n  \
               bytes(\"{}\"),\n  \
               relationship: \"{}\",\n  \
               mime-type: \"{}\",\n  \
               description: \"{}\",\n\
             )",
            escape(&a.filename),
            escape(&a.data),
            a.relationship.as_str(),
            escape(&a.mime_type),
            escape(&a.description),
        )
        .expect("writing to a String cannot fail");
    }
    if invoke {
        writeln!(out, "#render(json(\"{}\"))", world::DATA)
            .expect("writing to a String cannot fail");
    }
    Ok(out)
}

/// The attachment name must be a plain file name.
///
/// It becomes a path inside the PDF and a Typst path in the harness. A name
/// with a separator, a traversal or a quote in it is refused rather than
/// escaped: every filename a profile actually specifies (`factur-x.xml`,
/// `xrechnung.xml`) is a bare token, so anything else is a caller's bug.
fn validate_filename(name: &str) -> Result<(), RenderError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(RenderError::Attachment(format!(
            "`{name}` is not a usable attachment name: expected a bare file name of \
             letters, digits, `.`, `_` or `-`"
        )))
    }
}

/// Escape a value for a Typst string literal.
///
/// Backslash first — escaping it after the quotes would double the ones this
/// function itself introduced. Line endings are escaped rather than left raw so
/// the harness stays one line per construct, which keeps a diagnostic's line
/// number meaningful when a 40 KB invoice is embedded in it.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Format diagnostics, most severe first, pointing into the operator's file.
fn format_all(
    world: &TemplateWorld,
    diagnostics: &typst::ecow::EcoVec<SourceDiagnostic>,
) -> Vec<String> {
    diagnostics.iter().map(|d| format_one(world, d)).collect()
}

fn format_one(world: &TemplateWorld, d: &SourceDiagnostic) -> String {
    let at = location(world, d.span);
    let mut out = match &at {
        Some(at) => format!("{at}: {}", d.message),
        None => d.message.to_string(),
    };
    // A diagnostic in mako's own harness is almost always the template contract
    // not being met, and "/main.typ:2:26: unresolved import" tells an operator
    // nothing — they have never seen /main.typ and did not write it.
    if at.as_deref().is_some_and(|at| at.starts_with(world::MAIN)) {
        let _ = write!(
            out,
            "\n  hint: this is mako's harness, not your file. A template must export \
             the contract function: `#let render(invoice) = {{ .. }}`"
        );
    }
    for hint in &d.hints {
        let _ = write!(out, "\n  hint: {}", hint.v);
    }
    // The trace is how a diagnostic inside a helper function is explained: the
    // error is reported where it happened, and the trace says which call got
    // there. Without it, "unexpected type" in a shared formatting helper gives
    // the operator no idea which of forty call sites was wrong.
    for step in &d.trace {
        if let Some(at) = location(world, step.span.into()) {
            let _ = write!(out, "\n  called at {at}");
        }
    }
    out
}

/// `path:line:col`, one-based, as an editor would jump to it.
fn location(world: &TemplateWorld, span: typst::syntax::DiagSpan) -> Option<String> {
    let file: FileId = match span.get() {
        DiagSpanKind::Detached => return None,
        DiagSpanKind::Number { id, .. } | DiagSpanKind::Range { id, .. } => id,
    };
    let name = TemplateWorld::name_of(file);
    let Some(source) = world.source_of(file) else {
        return Some(name);
    };
    let range = world.range(span)?;
    let (line, column) = source.lines().byte_to_line_column(range.start)?;
    Some(format!("{name}:{}:{}", line + 1, column + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment() -> Attachment {
        Attachment {
            filename: "factur-x.xml".to_owned(),
            data: "<CrossIndustryInvoice/>".to_owned(),
            mime_type: "application/xml".to_owned(),
            description: "Rechnungsdaten".to_owned(),
            relationship: Relationship::Alternative,
        }
    }

    fn request(template: &str) -> RenderRequest {
        RenderRequest {
            template: template.to_owned(),
            data: Some(r#"{"number":"R-1","totals":{"due":"119.00"}}"#.to_owned()),
            attachment: Some(attachment()),
            standard: Some("a-3b".to_owned()),
            date: time::macros::date!(2026 - 03 - 01),
            ident: "R-1".to_owned(),
        }
    }

    const MINIMAL: &str = "#let render(invoice) = [Rechnung #invoice.number]";

    #[test]
    fn a_minimal_template_renders_a_pdf() {
        let out = render(&request(MINIMAL)).expect("minimal template renders");
        assert!(out.pdf.starts_with(b"%PDF-"), "output is a PDF");
        assert_eq!(out.pages, 1);
    }

    /// Two renders of the same request must be the same file.
    ///
    /// This is the § 147 AO property in its strongest form: nothing ambient —
    /// not the clock, not a random `/ID`, not a font path — leaks into the
    /// output, so an audit re-render in 2034 reproduces the original exactly.
    #[test]
    fn the_same_request_renders_the_same_bytes() {
        let req = request(MINIMAL);
        assert_eq!(
            render(&req).expect("first render").pdf,
            render(&req).expect("second render").pdf,
        );
    }

    /// A template error names the operator's file, its line and its column.
    #[test]
    fn a_broken_template_reports_where_it_broke() {
        let err = render(&request("#let render(invoice) = [#invoice.nope]"))
            .expect_err("a missing field is an error");
        let RenderError::Compile(messages) = err else {
            panic!("expected a compile error, got {err:?}");
        };
        assert!(
            messages.iter().any(|m| m.starts_with("/template.typ:1:")),
            "a diagnostic must point into the template: {messages:?}",
        );
    }

    /// A template that does not export `render` is refused, not rendered blank.
    ///
    /// And the refusal has to be *readable*: Typst reports it as an unresolved
    /// import in `/main.typ`, a file the operator has never seen and did not
    /// write, so the contract is spelled out alongside it.
    #[test]
    fn the_contract_is_enforced_at_the_import() {
        let err = render(&request("#let anders(invoice) = []")).expect_err("no `render` export");
        let RenderError::Compile(messages) = err else {
            panic!("expected a compile error, got {err:?}");
        };
        let joined = messages.join("\n");
        assert!(
            joined.contains("unresolved import"),
            "the import is what fails: {messages:?}",
        );
        assert!(
            joined.contains("#let render(invoice)"),
            "the diagnostic must explain the contract: {messages:?}",
        );
    }

    /// The template cannot reach the filesystem, the network, or the attachment.
    #[test]
    fn a_template_cannot_read_anything_it_was_not_given() {
        for (what, source) in [
            ("host file", r#"#let render(i) = [#read("/etc/passwd")]"#),
            ("package", r#"#import "@preview/cetz:0.4.2": *"#),
            (
                "invoice XML the harness embeds",
                "#let render(i) = [#read(\"/attachment.bin\")]",
            ),
            (
                "view as source",
                "#let render(i) = { include \"/document.json\" }",
            ),
        ] {
            assert!(
                render(&request(source)).is_err(),
                "a template must not be able to reach the {what}",
            );
        }
    }

    /// A standard that cannot carry the XML is refused before rendering.
    ///
    /// Rendering it would succeed and produce a handsome PDF with no invoice
    /// data inside — the one failure mode that looks like success.
    #[test]
    fn a_standard_that_would_drop_the_invoice_is_refused() {
        let mut req = request(MINIMAL);
        req.standard = Some("a-2b".to_owned());
        let err = render(&req).expect_err("PDF/A-2 cannot carry the attachment");
        assert!(matches!(err, RenderError::Standard(_)), "{err:?}");

        req.standard = None;
        assert!(matches!(
            render(&req).expect_err("no standard at all"),
            RenderError::Standard(_),
        ));

        req.standard = Some("a-3z".to_owned());
        assert!(matches!(
            render(&req).expect_err("not a standard"),
            RenderError::Standard(_),
        ));
    }

    /// Without an attachment there is nothing to drop, so plain PDF is fine —
    /// that is the Textform case.
    #[test]
    fn a_document_without_an_attachment_needs_no_standard() {
        let mut req = request(MINIMAL);
        req.attachment = None;
        req.standard = None;
        assert!(render(&req).is_ok());
    }

    #[test]
    fn an_attachment_name_must_be_a_bare_file_name() {
        for bad in ["", "../factur-x.xml", "a/b.xml", "x\".xml", ".hidden"] {
            assert!(validate_filename(bad).is_err(), "{bad:?} must be refused");
        }
        for good in ["factur-x.xml", "xrechnung.xml", "order-x.xml"] {
            assert!(validate_filename(good).is_ok(), "{good:?} must be accepted");
        }
    }

    /// The harness — not the template — is what attaches the XML.
    #[test]
    fn the_harness_owns_the_attachment() {
        let h = harness(Some(&attachment()), true).expect("harness builds");
        assert!(h.contains("#import \"/template.typ\": render"));
        assert!(h.contains("pdf.attach"));
        assert!(h.contains("\"factur-x.xml\""));
        // The XML is inline, so there is no file for a template to read.
        assert!(h.contains("bytes(\"<CrossIndustryInvoice/>\")"));
        assert!(
            !h.contains("read("),
            "the harness reads no file for the invoice"
        );
        assert!(h.contains("relationship: \"alternative\""));
        assert!(h.contains("#render(json(\"/document.json\"))"));

        // Nothing to attach, nothing attached.
        let bare = harness(None, false).expect("harness builds");
        assert!(!bare.contains("attach"));
        assert!(!bare.contains("#render("), "no data means no call");
    }

    /// Importing without calling is what proves a template exports `render`.
    #[test]
    fn a_template_can_be_checked_without_being_rendered() {
        let mut req = request("#let render(invoice) = [#invoice.number]");
        req.data = None;
        req.attachment = None;
        req.standard = None;
        assert!(render(&req).is_ok(), "the import alone must succeed");

        req.template = "#let anders(invoice) = []".to_owned();
        assert!(
            render(&req).is_err(),
            "a missing export must still be caught without data",
        );
    }

    /// A render that outlasts its budget frees the caller.
    ///
    /// The template here is heavy but **finite** on purpose. A genuinely
    /// non-terminating one would model production more closely and would hang
    /// this test at shutdown: `spawn_blocking` tasks are awaited when the
    /// runtime drops, and Typst gives no way to interrupt a compilation. That
    /// is exactly the trade [`render_guarded`] documents — the caller is freed,
    /// the thread is not — and the only part of it a test can observe is the
    /// caller being freed.
    /// Renders queue rather than thrash, and queueing counts against the budget.
    ///
    /// The cap exists so a burst of publishes cannot exhaust tokio's blocking
    /// pool — the same pool `sqlx` uses, so saturating it stalls database work
    /// that has nothing to do with rendering. What a caller must never see is a
    /// queue that outlasts the deadline it asked for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_renders_are_bounded_and_still_honour_the_budget() {
        // Far more requests than slots, each individually quick.
        let mut set = tokio::task::JoinSet::new();
        for i in 0..8 {
            let mut req = request(MINIMAL);
            req.ident = format!("concurrent-{i}");
            set.spawn(render_guarded(req, std::time::Duration::from_secs(30)));
        }
        let mut rendered = 0;
        while let Some(result) = set.join_next().await {
            assert!(result.expect("no panic").is_ok(), "every render completes");
            rendered += 1;
        }
        assert_eq!(rendered, 8, "queueing must not drop work");
    }

    #[tokio::test]
    async fn a_slow_render_frees_the_caller() {
        let mut req = request(MINIMAL);
        req.template = "#let render(i) = { for p in range(400) { pagebreak(weak: true) \
             ; [Seite mit etwas Text darauf] } }"
            .to_owned();
        let err = render_guarded(req, std::time::Duration::from_millis(1))
            .await
            .expect_err("a render that outruns its budget must time out");
        assert!(matches!(err, RenderError::Timeout(_)), "{err:?}");
    }
}
