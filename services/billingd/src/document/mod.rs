//! The customer-facing document: what an invoice *looks like*, and the carrier
//! that keeps its machine-readable twin inside it.
//!
//! # The three layers
//!
//! | Layer | Module | Owner |
//! |---|---|---|
//! | What may be printed | [`view`] | mako — a fixed projection of the EN 16931 model |
//! | How it is printed | [`mod@render`] | the operator — a Typst template, hash-pinned |
//! | What is embedded | [`facturx`] | mako — the CII XML, byte-identical, never the template's |
//!
//! The layering is the whole design. The operator owns the *layout* — logo,
//! Briefkopf, where the Pflichtangaben sit — because that is a branding decision
//! and not a compliance one. mako owns the *content*, both the visual projection
//! and the embedded XML, because that is a compliance decision and not a
//! branding one. A template is therefore code that mako runs, but it is code
//! that cannot reach the XML, cannot reach the network or the filesystem, and
//! cannot make the document say something the semantic model does not.
//!
//! # What that buys
//!
//! - No template can emit a non-conformant or mismatched XML — it never emits
//!   XML at all. The bytes come from `crate::einvoice::render_cii`, and
//!   [`gate`] proves they come back out of the finished PDF unchanged.
//! - A broken template is a rendering failure, never a compliance failure.
//! - The visual and machine layers cannot disagree: one model feeds both.
//!
//! # Reproducibility
//!
//! An invoice is a Buchungsbeleg kept for 8 years (§ 14b UStG / § 147 AO), so
//! "why did the 2027 invoice look like that" must be answerable in 2034. It is,
//! because rendering is a pure function of inputs that are all recorded:
//!
//! - the template — pinned by SHA-256 on the record (`template_hash`),
//! - the model — stored on the record (`en16931_json`),
//! - the date — taken from the document, never from the wall clock (see
//!   [`world::TemplateWorld`]),
//! - the fonts — bundled in the binary, never read from the host.
//!
//! Re-rendering an issued invoice therefore reproduces it byte-for-byte, which
//! `tests/zugferd_carrier.rs` asserts on the finished ZUGFeRD file, stamping
//! included.

pub mod facturx;
pub mod gate;
pub mod render;
pub mod view;
pub mod world;

pub use render::{RenderError, RenderRequest, Rendered, render};
pub use view::{DocumentView, LineView, PartyView, TotalsView, VatView};

/// The reference invoice layout, published as a tenant's template on request.
///
/// Shipping one is not decoration: without it the first thing an operator must
/// do is write a § 14 Abs. 4 UStG-complete Typst document from nothing. This is
/// that document, and it is compiled by the test suite on every run, so the
/// example an operator starts from is never stale.
pub const REFERENCE_INVOICE_TEMPLATE: &str = include_str!("templates/invoice.typ");
