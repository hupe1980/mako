//! The sandbox an operator template compiles in.
//!
//! A Typst template is *code*, and an operator publishes it over HTTP. The
//! [`World`] is the only interface that code has to anything outside itself —
//! Typst has no ambient I/O, so whatever this type refuses to hand over simply
//! does not exist during a render. That makes the trait implementation the
//! security boundary, and it is deliberately the smallest one that can still
//! typeset an invoice:
//!
//! | Capability | Here |
//! |---|---|
//! | Host filesystem | none — [`TemplateWorld::file`] serves three in-memory files and nothing else |
//! | Network / `@preview` packages | refused, with a message saying so |
//! | Fonts | the bundled `typst-assets` set; never the host's, never the operator's |
//! | Wall clock | none — [`TemplateWorld::today`] returns the *document's* date |
//!
//! The embedded invoice XML is deliberately **not** among those files. It is
//! written into the harness as a literal instead, so there is no path a
//! template could `read` it from — see [`super::render`](mod@super::render).
//!
//! # Why the clock is not the clock
//!
//! `datetime.today()` in a template must not mean "when the PDF was made".
//! A re-render of a 2027 invoice in 2034 has to produce the same bytes, and an
//! ambient clock would make every render of the same invoice a different
//! document. Returning the document date makes rendering a pure function of
//! recorded inputs, which is what § 147 AO reproducibility requires in practice.
//!
//! # What is *not* bounded here
//!
//! Compute. Typst caps loop iterations and call depth, but nested loops still
//! multiply, so a pathological template can burn a core. That is a scheduling
//! concern rather than a sandboxing one and belongs to the caller —
//! [`super::render`](mod@super::render) documents how it is handled.

use std::collections::HashMap;
use std::sync::LazyLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

/// The entry point the harness compiles.
pub const MAIN: &str = "/main.typ";
/// The operator's template, imported by the harness.
pub const TEMPLATE: &str = "/template.typ";
/// The [`super::view::DocumentView`] the template renders, as JSON.
pub const DATA: &str = "/document.json";

/// The bundled fonts, parsed once for the life of the process.
///
/// Parsing the seventeen `typst-assets` faces takes long enough that doing it
/// per render would dominate the render. They never change, so they are built
/// once and shared; `Font` is reference-counted, so sharing is free.
struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let mut fonts = Vec::new();
    for data in typst_assets::fonts() {
        fonts.extend(Font::iter(Bytes::new(data)));
    }
    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

/// The Typst standard library, built once. Immutable and shared.
static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));

/// One file in the world.
enum Entry {
    /// A `.typ` file, pre-parsed so a diagnostic can resolve to line and column.
    Source(Source),
    /// Data a template reads but must not `include` — the view's JSON.
    Data(Bytes),
}

/// The compilation environment for one render.
///
/// Construct it with [`TemplateWorld::new`]; every file the compiler can reach
/// is fixed at that moment, so a template's inputs are exactly what the caller
/// decided they are.
pub struct TemplateWorld {
    main: FileId,
    files: HashMap<FileId, Entry>,
    today: Datetime,
}

/// Resolve a virtual path to the id Typst uses for it.
///
/// Always rooted in the project: a file in this world is never part of a
/// package, because there are no packages here.
///
/// # Panics
///
/// On a path Typst cannot virtualise. Every caller passes one of the four
/// constants above, so reaching that is a bug in mako rather than in a template.
fn id(path: &str) -> FileId {
    let vpath = VirtualPath::new(path).expect("a virtual path mako itself wrote");
    FileId::new(RootedPath::new(VirtualRoot::Project, vpath))
}

impl TemplateWorld {
    /// Assemble the world for one render.
    ///
    /// `harness` is mako's entry point, `template` the operator's source, and
    /// `data` the JSON the template reads. `today` is the date the document
    /// bears — see the module docs for why it is not the current date.
    #[must_use]
    pub fn new(harness: &str, template: &str, data: &str, today: Datetime) -> Self {
        let main = id(MAIN);
        let mut files = HashMap::new();
        files.insert(main, Entry::Source(Source::new(main, harness.to_owned())));
        let tid = id(TEMPLATE);
        files.insert(tid, Entry::Source(Source::new(tid, template.to_owned())));
        // The view is served as a file rather than as a `sys.inputs` value so a
        // template reads it with plain `json("/document.json")` — the ordinary
        // Typst idiom, which means operator documentation and examples from
        // outside mako apply unchanged.
        files.insert(id(DATA), Entry::Data(Bytes::from_string(data.to_owned())));
        Self { main, files, today }
    }

    /// The parsed source of a file, for turning a span into a line and column.
    #[must_use]
    pub fn source_of(&self, file: FileId) -> Option<&Source> {
        match self.files.get(&file) {
            Some(Entry::Source(s)) => Some(s),
            _ => None,
        }
    }

    /// The virtual path of a file, as a diagnostic should name it.
    #[must_use]
    pub fn name_of(file: FileId) -> String {
        file.vpath().get_with_slash().to_owned()
    }
}

impl World for TemplateWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, file: FileId) -> FileResult<Source> {
        match self.files.get(&file) {
            Some(Entry::Source(s)) => Ok(s.clone()),
            // The view is data, not code: `include "/document.json"` would put
            // the raw JSON on the page instead of raising.
            Some(Entry::Data(_)) => Err(FileError::NotSource),
            None => Err(self.missing(file)),
        }
    }

    fn file(&self, file: FileId) -> FileResult<Bytes> {
        match self.files.get(&file) {
            Some(Entry::Source(s)) => Ok(Bytes::from_string(s.text().to_owned())),
            Some(Entry::Data(b)) => Ok(b.clone()),
            None => Err(self.missing(file)),
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        // The index may come from an outdated font book during incremental
        // validation, so it is not guaranteed to be in bounds — `get`, not `[]`.
        FONTS.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // The offset is ignored rather than honoured: there is no "now" to shift
        // into another timezone. The document has one date, and every render of
        // it — today's and 2034's — must see that same date.
        Some(self.today)
    }
}

impl TemplateWorld {
    /// The error for a file the template asked for and will not get.
    ///
    /// Worth spelling out: "file not found" on `@preview/cetz:0.4.2` sends an
    /// operator looking for a typo instead of telling them the answer, which is
    /// that mako renders offline and always will.
    fn missing(&self, file: FileId) -> FileError {
        let name = Self::name_of(file);
        if let VirtualRoot::Package(package) = file.root() {
            return FileError::Other(Some(
                format!(
                    "template packages are not available: `{package}` would have to be fetched \
                     from the network, and a document that must still render in 2034 may not \
                     depend on a registry that might not exist then"
                )
                .into(),
            ));
        }
        FileError::NotFound(std::path::PathBuf::from(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> TemplateWorld {
        TemplateWorld::new(
            "#import \"/template.typ\": render",
            "#let render(x) = []",
            "{}",
            Datetime::from_ymd(2026, 3, 1).unwrap(),
        )
    }

    #[test]
    fn only_the_three_files_the_caller_supplied_exist() {
        let w = world();
        assert!(w.source(id(MAIN)).is_ok());
        assert!(w.source(id(TEMPLATE)).is_ok());
        assert!(w.file(id(DATA)).is_ok());

        // Anything else is absent, including paths that exist on the host.
        for path in [
            "/etc/passwd",
            "/logo.png",
            "/main.rs",
            "/assets/../logo.svg",
        ] {
            assert!(
                matches!(w.file(id(path)), Err(FileError::NotFound(_))),
                "{path} must not resolve",
            );
        }

        // A path that would escape the root cannot even be *named*: Typst
        // refuses to virtualise it, so traversal is not something this world
        // has to defend against.
        for escape in ["/../../secrets", "/../template.typ"] {
            assert!(VirtualPath::new(escape).is_err(), "{escape} must not exist");
        }
    }

    /// A `@preview` import must fail with an explanation, not a bare not-found.
    #[test]
    fn packages_are_refused_with_a_reason() {
        let w = world();
        let pkg = FileId::new(RootedPath::new(
            VirtualRoot::Package("@preview/cetz:0.4.2".parse().expect("valid package spec")),
            VirtualPath::new("/lib.typ").expect("valid virtual path"),
        ));
        let Err(FileError::Other(Some(msg))) = w.file(pkg) else {
            panic!("a package file must be refused with a message");
        };
        assert!(msg.contains("cetz"), "the message names the package: {msg}");
    }

    /// The view is data, not code.
    #[test]
    fn the_view_cannot_be_included_as_source() {
        assert!(matches!(
            world().source(id(DATA)),
            Err(FileError::NotSource)
        ));
    }

    /// There is no path the embedded invoice could be read from.
    ///
    /// It is a literal inside the harness, not a file. Serving it as
    /// `/attachment.bin` so the harness could `read` it would mean a template
    /// could read it too.
    #[test]
    fn the_embedded_invoice_is_not_a_file_at_all() {
        let w = world();
        for path in ["/attachment.bin", "/factur-x.xml", "/xrechnung.xml"] {
            assert!(
                matches!(w.file(id(path)), Err(FileError::NotFound(_))),
                "{path} must not exist",
            );
        }
    }

    /// `datetime.today()` is the document's date, in every timezone, always.
    #[test]
    fn the_clock_is_the_document_date() {
        let w = world();
        let expected = Datetime::from_ymd(2026, 3, 1);
        assert_eq!(w.today(None), expected);
        for hours in [0, 12, -12] {
            assert_eq!(
                w.today(Some(Duration::from(time::Duration::hours(hours)))),
                expected,
                "an offset of {hours}h cannot move the document's date",
            );
        }
    }

    /// The bundled font set must actually be there — a distroless image has no
    /// system fonts, so an empty book would make every render fall back to
    /// nothing and silently produce blank glyphs.
    #[test]
    fn the_bundled_fonts_are_present() {
        let w = world();
        assert!(w.book().families().count() > 0, "no bundled font families");
        assert!(w.font(0).is_some());
        assert!(
            w.font(usize::MAX).is_none(),
            "an out-of-range index is None"
        );
    }
}
