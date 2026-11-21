//! The Messkonzept of a Netzanschluss — what a set of devices requires, what
//! its owner decides, and which of the Netzbetreiber's Messkonzepte fit.
//!
//! | Step | Type | Answers |
//! |---|---|---|
//! | Describe | [`Netzanschluss`] | the household, its devices, the figures known |
//! | Require | [`Netzanschluss::lokationsbuendel`] | the Lokationen each [`Lokationsbuendel`] must hold, each with its source |
//! | Decide | [`Netzanschluss::decisions`] | § 14a module, feed-in mode, iMSys, a further household — with euros where a source fixes them |
//! | Compare | [`Change::between`] | what an extension adds and removes |
//! | Match | [`Catalogue::matching`], [`structures`] | the operator's Messkonzepte, and the BDEW Codeliste structures, that hold a bundle |
//!
//! No layout is derived from devices: a device adds only the Lokationen a
//! published source requires (see [`Device`]); every further Messlokation is
//! the Messkonzept's choice, so matching compares Marktlokationen and
//! Technische Ressourcen alone. A Messkonzept is named only from the
//! operator's [`Catalogue`].
//!
//! Pure: no I/O and no clock — the day a decision is taken is an input.

#![deny(unsafe_code)]

mod catalogue;
mod decision;
mod netzanschluss;

pub use catalogue::{Bound, Catalogue, Lokationsbuendelstruktur, Match, Messkonzept, structures};
pub use decision::{Choice, Decision, Topic, volleinspeisung_from};
pub use netzanschluss::{
    Change, Device, FeedIn, Lokation, LokationKind, Lokationsbuendel, Modul14a, Netzanschluss,
    Preisblatt,
};
