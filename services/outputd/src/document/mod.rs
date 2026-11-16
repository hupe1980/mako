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
//!   XML at all. The bytes arrive from the calling service (billingd renders its CII), and
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
//! `tests/it/zugferd_carrier.rs` asserts on the finished ZUGFeRD file, stamping
//! included.

pub mod facturx;
pub mod gate;
pub mod gebuehrenbescheid;
pub mod mahnung;
pub mod preisanpassung;
pub mod render;
pub mod vertragsbestaetigung;
pub mod vertragszusammenfassung;
pub mod view;
pub mod widerrufsbestaetigung;
pub mod world;

pub use render::{RenderError, RenderRequest, Rendered, render};
pub use view::{DocumentView, LineView, PartyView, TotalsView, VatView};

use crate::config::Anrede;

/// A Textform view as its template receives it: the producer's view plus
/// `anrede` (`"sie"` or `"du"`), the form of address the letter is written in.
///
/// outputd sets the field, never the producer — the form of address is the
/// operator's `[policy] anrede`, and a producer's copy would be a second source
/// for it. `None` when the view is not a JSON object.
#[must_use]
pub fn mit_anrede(view: &serde_json::Value, anrede: Anrede) -> Option<serde_json::Value> {
    let mut object = view.as_object()?.clone();
    object.insert("anrede".to_owned(), anrede.as_str().into());
    Some(serde_json::Value::Object(object))
}

/// A typed specimen, serialised the way production hands a view to its
/// template: with `anrede` set.
///
/// # Errors
///
/// A view that does not serialise to a JSON object.
pub fn specimen_data<T: serde::Serialize>(view: &T, anrede: Anrede) -> anyhow::Result<String> {
    let value = serde_json::to_value(view)?;
    let data = mit_anrede(&value, anrede)
        .ok_or_else(|| anyhow::anyhow!("a specimen view is a JSON object"))?;
    Ok(data.to_string())
}

/// The reference invoice layout, published as a tenant's template on request.
///
/// Shipping one is not decoration: without it the first thing an operator must
/// do is write a § 14 Abs. 4 UStG-complete Typst document from nothing. This is
/// that document, and it is compiled by the test suite on every run, so the
/// example an operator starts from is never stale.
pub const REFERENCE_INVOICE_TEMPLATE: &str = include_str!("templates/invoice.typ");

/// The reference Mahnung layout — same rationale, same gate (Textform proof).
pub const REFERENCE_MAHNUNG_TEMPLATE: &str = include_str!("templates/mahnung.typ");

/// The reference Preisanpassung layout — same rationale, same gate.
///
/// The one whose *content* is regulated rather than its form:
/// § 41 Abs. 5 EnWG fixes what a price-change notice must say, and the gate
/// checks the rendered page says it. Most of all the Satz 4
/// Sonderkündigungsrecht, which Satz 1 obliges the supplier to state in the
/// same notice and without which the Anzeige is invalid.
pub const REFERENCE_PREISANPASSUNG: &str = include_str!("templates/preisanpassung.typ");

/// The reference Gebührenbescheid layout — same rationale, same gate: the page
/// must print what § 157 Abs. 1 AO attaches to a Bescheid.
pub const REFERENCE_GEBUEHRENBESCHEID: &str = include_str!("templates/gebuehrenbescheid.typ");

/// The reference Vertragszusammenfassung layout — same rationale, same gate:
/// the page must print every item of § 41 Abs. 4 Satz 2 Nr. 1–6 EnWG.
pub const REFERENCE_VERTRAGSZUSAMMENFASSUNG: &str =
    include_str!("templates/vertragszusammenfassung.typ");

/// The reference Vertragsbestätigung layout — same rationale, same gate: the
/// page must print the contract's content and the Widerrufsbelehrung with the
/// Muster-Widerrufsformular and the Wertersatz notice.
pub const REFERENCE_VERTRAGSBESTAETIGUNG: &str = include_str!("templates/vertragsbestaetigung.typ");

/// The reference Widerrufsbestätigung layout — same rationale, same gate: the
/// page must print the declaration and the date and time of its receipt.
pub const REFERENCE_WIDERRUFSBESTAETIGUNG: &str =
    include_str!("templates/widerrufsbestaetigung.typ");
