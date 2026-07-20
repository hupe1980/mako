//! Fluent builder APIs for constructing EDI@Energy messages from scratch.
//!
//! # Overview
//!
//! Builders let you create well-formed EDIFACT messages without writing raw
//! EDIFACT bytes by hand. Each builder is configured with a release and
//! optional metadata, then produces a fully-parsed, typed message struct.
//!
//! # Type-state safety
//!
//! All builders enforce that `sender` and `receiver` are set before
//! `build` can be called — at **compile time**. The phantom type parameters
//! `S` (sender state) and `R` (receiver state) track whether each required
//! field has been provided. Calling `build` or `serialize` on a builder
//! where either field is missing produces a **compile error**, not a runtime
//! panic.
//!
//! ```text
//! UtilmdBuilder::new(release)          // Builder<Unset, Unset>
//!     .sender("9900987654321")         // Builder<Set,   Unset>
//!     .receiver("9900123456789")       // Builder<Set,   Set>
//!     .build()                         // ✓  only available here
//! ```
//!
//! The `Set` and `Unset` types are exported so callers can write generic
//! code over builder states when needed.
//!
//! # Example — UTILMD
//!
//! ```rust,no_run
//! # #[cfg(not(feature = "utilmd"))]
//! # fn main() {}
//! # #[cfg(feature = "utilmd")]
//! # fn main() -> Result<(), edi_energy::Error> {
//! use edi_energy::{Release, Pruefidentifikator};
//! use edi_energy::builders::UtilmdBuilder;
//!
//! let msg = UtilmdBuilder::new(Release::new("S2.1"))
//!     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
//!     .sender("9900987654321")
//!     .receiver("9900123456789")
//!     .build()?;
//!
//! assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("9900987654321"));
//! # Ok(())
//! # }
//! ```
//!
//! # Example — MSCONS with metering point
//!
//! ```rust,no_run
//! # #[cfg(not(feature = "mscons"))]
//! # fn main() {}
//! # #[cfg(feature = "mscons")]
//! # fn main() -> Result<(), edi_energy::Error> {
//! use edi_energy::{Release, Pruefidentifikator};
//! use edi_energy::builders::MsconsBuilder;
//!
//! let msg = MsconsBuilder::new(Release::new("2.4c"))
//!     .pruefidentifikator(Pruefidentifikator::new(21001).unwrap())
//!     .sender("9900111222333")
//!     .receiver("9900444555666")
//!     .metering_point("DE0001234567890")
//!         .location_id("12345678901")
//!         .quantity("220", "1000.500", "KWH")
//!     .done()
//!     .build()?;
//!
//! assert_eq!(msg.delivery_points().len(), 1);
//! # Ok(())
//! # }
//! ```

// ── Type-state marker types ───────────────────────────────────────────────────

/// Marker type: this required builder field **has been set**.
///
/// Used as a phantom type parameter on builder structs to track required-field
/// state at compile time. A builder whose type parameters include `Set` for
/// both sender and receiver may call [`build`](UtilmdBuilder::build).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Set;

/// Marker type: this required builder field **has not yet been set**.
///
/// Initial state for sender/receiver phantom type parameters on all builders.
/// A builder in `Unset` state for sender or receiver cannot call `build()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Unset;

// ── Shared helpers (pub(super) for child modules) ─────────────────────────────

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "iftsta",
    feature = "insrpt",
    feature = "invoic",
    feature = "orders",
    feature = "partin",
    feature = "reqote",
    feature = "remadv",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
pub(super) fn bytes_to_segments(
    bytes: &[u8],
) -> Result<Vec<edifact_rs::OwnedSegment>, crate::Error> {
    edifact_rs::from_bytes_owned(bytes)
        .collect::<Result<_, _>>()
        .map_err(crate::Error::Parse)
}

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "iftsta",
    feature = "insrpt",
    feature = "invoic",
    feature = "orders",
    feature = "partin",
    feature = "reqote",
    feature = "remadv",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "utilts",
))]
pub(super) fn format_dtm137(date: &str) -> String {
    format!("137:{date}:102")
}

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "iftsta",
    feature = "insrpt",
    feature = "invoic",
    feature = "orders",
    feature = "partin",
    feature = "reqote",
    feature = "remadv",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "utilts",
))]
pub(super) fn dtm_today() -> String {
    let today = time::OffsetDateTime::now_utc().date();
    let (y, m, d) = (today.year(), today.month() as u8, today.day());
    format!("137:{y:04}{m:02}{d:02}:102")
}

// ── Sub-modules ───────────────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
mod utilmd;
#[cfg(feature = "utilmd")]
pub use utilmd::{UtilmdBuilder, UtilmdTransactionBuilder};

#[cfg(feature = "mscons")]
mod mscons;
#[cfg(feature = "mscons")]
pub use mscons::{
    MSCONS_UNITS, MeteringPointBuilder, MsconsBuilder, QTY_ENERGIE_SUMMIERT, QTY_ERSATZWERT,
    QTY_WAHRER_WERT, is_valid_mscons_unit,
};

#[cfg(feature = "aperak")]
mod aperak;
#[cfg(feature = "aperak")]
pub use aperak::AperakBuilder;

#[cfg(feature = "contrl")]
mod contrl;
#[cfg(feature = "contrl")]
pub use contrl::ContrlBuilder;

#[cfg(feature = "iftsta")]
mod iftsta;
#[cfg(feature = "iftsta")]
pub use iftsta::IftstaBuilder;

#[cfg(feature = "insrpt")]
mod insrpt;
#[cfg(feature = "insrpt")]
pub use insrpt::InsrptBuilder;

#[cfg(feature = "invoic")]
mod invoic;
#[cfg(feature = "invoic")]
pub use invoic::InvoicBuilder;

#[cfg(feature = "orders")]
mod orders;
#[cfg(feature = "orders")]
pub use orders::OrdersBuilder;

#[cfg(feature = "partin")]
mod partin;
#[cfg(feature = "partin")]
pub use partin::PartinBuilder;

#[cfg(feature = "reqote")]
mod reqote;
#[cfg(feature = "reqote")]
pub use reqote::ReqoteBuilder;

#[cfg(feature = "remadv")]
mod remadv;
#[cfg(feature = "remadv")]
pub use remadv::RemadvBuilder;

#[cfg(feature = "ordchg")]
mod ordchg;
#[cfg(feature = "ordchg")]
pub use ordchg::OrdchgBuilder;

#[cfg(feature = "ordrsp")]
mod ordrsp;
#[cfg(feature = "ordrsp")]
pub use ordrsp::OrdrespBuilder;

#[cfg(feature = "quotes")]
mod quotes;
#[cfg(feature = "quotes")]
pub use quotes::QuotesBuilder;

#[cfg(feature = "comdis")]
mod comdis;
#[cfg(feature = "comdis")]
pub use comdis::ComdisBuilder;

#[cfg(feature = "pricat")]
mod pricat;
#[cfg(feature = "pricat")]
pub use pricat::{PricatBuilder, PricatLineItem, PricatPriceEntry, PricatPriceGroup};

#[cfg(feature = "utilts")]
mod utilts;
#[cfg(feature = "utilts")]
pub use utilts::{
    UtiltsBuilder, UtiltsCalcStep, UtiltsDefinitionBlock, UtiltsEnergyAmountRef, UtiltsUsagePeriod,
    UtiltsVorgang,
};
