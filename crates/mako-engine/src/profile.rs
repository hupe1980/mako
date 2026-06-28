//! Profile requirements — decouple domain crates from `edi-energy`.
//!
//! Domain crates (`mako-gpke`, `mako-wim`, …) declare which EDIFACT message
//! types they depend on by implementing
//! [`EngineModule::profile_requirements`].  The engine builder validates these
//! requirements against the injected `ReleaseRegistry` at startup — without
//! requiring domain crates to import `edi-energy` in their production
//! `[dependencies]`.
//!
//! # Example (domain crate)
//!
//! ```rust,ignore
//! use mako_engine::builder::EngineModule;
//! use mako_engine::profile::ProfileRequirement;
//!
//! pub struct GpkeModule;
//!
//! impl EngineModule for GpkeModule {
//!     fn name(&self) -> &'static str { "gpke" }
//!
//!     fn profile_requirements(&self) -> &'static [ProfileRequirement] {
//!         &[
//!             ProfileRequirement { message_type: "UTILMD", label: "UTILMD Strom" },
//!             ProfileRequirement { message_type: "INVOIC", label: "INVOIC Abrechnung" },
//!         ]
//!     }
//! }
//! ```
//!
//! # Validation
//!
//! [`EngineBuilder::build`] calls every registered module's
//! [`EngineModule::profile_requirements`] method.  For each requirement it checks that the
//! caller-supplied validation function (see
//! [`EngineBuilder::with_profile_validator`]) confirms at least one active
//! profile exists.  If not, `build` panics with an actionable error message —
//! exactly like `configure()` used to, but without the `edi-energy` import.
//!
//! [`EngineModule::profile_requirements`]: crate::builder::EngineModule::profile_requirements
//! [`EngineBuilder::build`]: crate::builder::EngineBuilder::build
//! [`EngineBuilder::with_profile_validator`]: crate::builder::EngineBuilder::with_profile_validator

/// A single profile requirement declared by a domain module.
///
/// The engine builder validates that at least one active profile satisfying
/// this requirement exists in the `edi-energy` registry.
///
/// Domain crates return a `&'static [ProfileRequirement]` slice from
/// [`EngineModule::profile_requirements`] — no `edi-energy` import required.
///
/// [`EngineModule::profile_requirements`]: crate::builder::EngineModule::profile_requirements
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileRequirement {
    /// EDIFACT message type identifier, e.g. `"UTILMD"`, `"APERAK"`, `"MSCONS"`.
    ///
    /// Must match the value that `edi_energy::MessageType::as_str()` (or
    /// equivalent) returns for the relevant profile.
    pub message_type: &'static str,

    /// Human-readable label used in error messages.
    ///
    /// Example: `"UTILMD Strom (GPKE)"`.
    pub label: &'static str,
}
