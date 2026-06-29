//! `edi-energy` — EDI@Energy EDIFACT parser and validator for the German energy market.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use edi_energy::{Platform, AnyMessage, EdiEnergyMessage};
//!
//! let input: &[u8] = b"UNB+UNOA:3+SENDER+RECEIVER+200101:0900+1'UNH+1+UTILMD:D:11A:UN:S2.2'BGM+E01+11001+9'UNT+2+1'UNZ+1+1'";
//! let msg = Platform::with_all_profiles().parse(input)?;
//! let report = msg.validate()?;
//! assert!(report.is_valid());
//! # Ok::<(), edi_energy::Error>(())
//! ```
//!
//! # Supported releases
//!
//! This crate ships built-in profiles for the **S-track** (Strom) and
//! **G-track** (Gas) UTILMD releases introduced by the 2024 format split:
//! `S2.1`, `S2.2` (Strom) and `G1.1`, `G1.2` (Gas).
//!
//! **Classic UTILMD releases (5.5.x)** — messages with wire release codes such
//! as `5.5.3a`, `5.5.4a`, `5.5.5a`, `5.5.6a`, `5.5.7a`, or `5.5.8a` — can be
//! **parsed** (the EDIFACT segment structure is read and typed fields extracted)
//! but **cannot be validated**: [`AnyMessage::validate`] and
//! [`AnyMessage::validate_against`] return [`Error::ProfileNotFound`] for any
//! classic-track release because no 5.5.x MIG/AHB profiles are bundled.
//!
//! If you need validation support for classic-track archive messages, build
//! and register custom [`registry::Profile`] implementations for those releases.

#![deny(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
// `Error::Validation` intentionally carries a full `EdiEnergyReport` for rich diagnostics.
// Boxing it would change the public pattern-matching API and is not worth the churn.
#![allow(clippy::result_large_err)]
// Generated profile code uses patterns that trigger bulk lint noise.
// These are correct and idiomatic for machine-generated output.
#![allow(clippy::unnecessary_map_or)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::manual_contains)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::redundant_closure_for_method_calls)]
// Generated profile helper functions (e.g. `fn ahb_xxx_pack()`) have obvious
// must-use semantics; suppress this pedantic lint for the whole crate.
#![allow(clippy::must_use_candidate)]
// Builder methods that return `Self` all have obvious must-use semantics.
#![allow(clippy::return_self_not_must_use)]

mod agency_code;
mod any_message;
mod custom_rule_pack;
mod error;
mod interchange;
mod light_message;
mod message;
mod message_type;
mod object_type;
mod parse;
mod platform;
mod pruefidentifikator;
mod release;
mod report;

/// Fluent builder APIs for constructing EDI@Energy messages.
pub mod builders;
/// Concrete EDI@Energy message type structs, one sub-module per message type.
pub mod messages;
/// Profile registry mapping `(MessageType, Release)` pairs to validation rules.
pub mod registry;

#[doc(hidden)]
pub(crate) mod generated;

pub use agency_code::AgencyCode;
pub use any_message::AnyMessage;
pub use custom_rule_pack::CustomRulePack;
pub use error::{Error, ProfileError};
pub use interchange::{InterchangeHeader, MessageEnvelope, ParsedInterchange, ReceiptContext};
pub use light_message::LightMessage;
pub use message::EdiEnergyMessage;
pub use message_type::MessageType;
pub use object_type::ObjectType;
pub use parse::{
    DEFAULT_MAX_SEGMENT_BYTES, InterchangeIter, ParseConfig, Parser, parse, parse_envelope_only,
    parse_interchange,
};
pub use platform::Platform;
pub use pruefidentifikator::Pruefidentifikator;
pub use registry::{ProcessContext, ReleaseRegistry, TRANSITION_GRACE_DAYS, TransitionState};
pub use release::{Release, ReleaseKind, ReleaseTrack};
pub use report::EdiEnergyReport;

/// Well-known release identifiers for all registered profiles.
///
/// Use these instead of `Release::new("...")` to get a compile error when a
/// profile is removed or renamed after a BDEW format update.
pub mod releases {
    #[cfg(any(
        feature = "aperak",
        feature = "comdis",
        feature = "contrl",
        feature = "iftsta",
        feature = "insrpt",
        feature = "invoic",
        feature = "mscons",
        feature = "ordchg",
        feature = "orders",
        feature = "ordrsp",
        feature = "partin",
        feature = "pricat",
        feature = "quotes",
        feature = "remadv",
        feature = "reqote",
        feature = "utilmd",
        feature = "utilts",
    ))]
    pub use crate::generated::releases::*;
}

// Re-export edifact-rs types users may need
pub use edifact_rs::{
    EdifactDeserialize, EdifactSerialize, ReaderConfig, ValidationIssue, ValidationReport,
    ValidationSeverity,
};
// ValidationIssueSummary is unconditionally available.
// serde::Serialize is available on the type when the `serde` feature is enabled.
pub use report::ValidationIssueSummary;

// Re-export typed hierarchy structs (segment groups) for each message type.
#[cfg(feature = "aperak")]
pub use messages::aperak::AperakError;
#[cfg(feature = "contrl")]
pub use messages::contrl::{ContrlElementError, ContrlMessageResponse, ContrlSegmentError};
#[cfg(feature = "mscons")]
pub use messages::mscons::{
    MsconsDeliveryPoint, MsconsLineItem, MsconsQuantity, MsconsReference, MsconsTimeSeries,
};
#[cfg(feature = "utilmd")]
pub use messages::utilmd::{UtilmdReference, UtilmdTransaction};

/// Validate `msg` and additionally enforce that its Prüfidentifikator matches
/// `expected`.
///
/// This is the free-function replacement for the former
/// `EdiEnergyMessage::validate_pruefidentifikator` trait method.  Splitting the
/// concern out of the trait reduces boilerplate in every implementation and
/// keeps the trait surface minimal.
///
/// Behaviour:
/// - Calls [`EdiEnergyMessage::validate`] to obtain the standard validation
///   report (all layers L1–L5).
/// - If [`EdiEnergyMessage::detect_pruefidentifikator`] returns `Ok(pid)` and
///   `pid == expected`, the report is returned unchanged.
/// - If the detected PID does not match `expected`, or if no PID can be
///   detected, a rule-`EE-PID-001` error is appended to the report and the
///   (now-invalid) report is returned.
///
/// # Errors
///
/// Returns `Err` only when [`EdiEnergyMessage::validate`] itself fails (e.g.
/// parse failure, profile not registered).  A PID mismatch is always surfaced
/// as a validation issue inside the returned `Ok(EdiEnergyReport)`, not as an
/// `Err`.
#[must_use = "validation result must be checked for errors"]
pub fn validate_and_check_pid(
    msg: &impl EdiEnergyMessage,
    expected: Pruefidentifikator,
) -> Result<EdiEnergyReport, Error> {
    let report = msg.validate()?;
    match msg.detect_pruefidentifikator() {
        Ok(actual) if actual == expected => Ok(report),
        Ok(actual) => {
            let mut inner = report.into_inner();
            inner.add_error(
                edifact_rs::ValidationIssue::new(
                    edifact_rs::ValidationSeverity::Error,
                    format!("expected Pruefidentifikator {expected}, found {actual}"),
                )
                .with_rule_id("EE-PID-001")
                .with_segment("BGM"),
            );
            Ok(EdiEnergyReport::new(inner))
        }
        Err(_) => {
            let mut inner = report.into_inner();
            inner.add_error(
                edifact_rs::ValidationIssue::new(
                    edifact_rs::ValidationSeverity::Error,
                    format!("expected Pruefidentifikator {expected}, but none was found"),
                )
                .with_rule_id("EE-PID-001")
                .with_segment("BGM"),
            );
            Ok(EdiEnergyReport::new(inner))
        }
    }
}
