//! BDEW MaKo AS4 profile stack and `BdewAs4Profile` entry point.
//!
//! [`bdew_mako_profile_stack`] returns an [`asx_rs`] [`ProfileStack`] pre-configured
//! for BDEW AS4 strict compliance.  [`BdewAs4Profile`] combines the profile stack
//! with a [`PModeRegistry`] for a single, startup-time entry point.

use asx_rs::core::InteropMode;
use asx_rs::interop::{
    BaseProfile, CanonicalizationPolicy, ProfileStack, ProfileValidationReport,
    ProfileValidationResult, SecurityPolicy, ValidationPolicy,
};

use crate::pmode::{PMode, PModeRegistry};

/// Short identifier for the BDEW MaKo AS4 profile.
pub const PROFILE_NAME: &str = "bdew_mako_as4";

/// Profile version string (mirrors the AS4 Kommunikationshandbuch edition).
pub const PROFILE_VERSION: &str = "2.0.0";

/// Creates a [`ProfileStack`] pre-configured for BDEW MaKo AS4 compliance.
///
/// The base profile enforces:
///
/// | Policy | Value | Source |
/// |---|---|---|
/// | Interop mode | `Strict` | BDEW requires full AS4 conformance |
/// | Canonicalization | Exclusive C14N, no comments | BDEW KH §5.5 |
/// | Signing required | `true` | BDEW KH §5.5 (mandatory) |
/// | Encryption required | `false` | BDEW KH §5.6 (optional) |
/// | Payload limits enforced | `true` | defense-in-depth |
/// | AS2 MIC required | `false` | not an AS4 concept |
///
/// Add partner-specific overrides via `ProfileStack::partner_overrides` if needed.
///
/// # Panics
///
/// Never panics — the returned profile always satisfies its own invariants.
///
/// # Example
///
/// ```rust
/// use mako_as4::profile::bdew_mako_profile_stack;
///
/// let stack = bdew_mako_profile_stack();
/// stack.validate().expect("BDEW MaKo base profile must pass all invariants");
/// ```
pub fn bdew_mako_profile_stack() -> ProfileStack {
    ProfileStack {
        base: BaseProfile {
            name: PROFILE_NAME.to_string(),
            version: PROFILE_VERSION.to_string(),
            mode: InteropMode::Strict,
            // Exclusive C14N without comments — BDEW AS4 Kommunikationshandbuch §5.5
            canonicalization: CanonicalizationPolicy::default(),
            security: SecurityPolicy {
                require_signature: true,
                // Encryption is optional per BDEW AS4 Kommunikationshandbuch §5.6
                require_encryption: false,
            },
            validation: ValidationPolicy {
                reject_ambiguous_headers: true,
                enforce_payload_limits: true,
                // AS4 does not use AS2 MIC (AS2-specific concept)
                require_as2_mic: false,
            },
        },
        extensions: Vec::new(),
        overrides: Vec::new(),
        partner_overrides: Vec::new(),
    }
}

/// BDEW MaKo AS4 profile — combines a [`ProfileStack`] with a [`PModeRegistry`].
///
/// `BdewAs4Profile` is the main startup entry point.  Build it once, register
/// all bilateral P-Modes, call [`validate`](Self::validate) to fail-fast on
/// misconfiguration, then share the profile (e.g., via `Arc`) across send/receive paths.
///
/// # Example
///
/// ```rust
/// use mako_as4::profile::BdewAs4Profile;
/// use mako_as4::pmode::{bdew_pmode, BdewAction};
///
/// let mut profile = BdewAs4Profile::new();
/// profile
///     .register_pmode(bdew_pmode("pm-utilmd-a", "9900000000001", BdewAction::Utilmd))
///     .register_pmode(bdew_pmode("pm-aperak-a", "9900000000001", BdewAction::Aperak));
///
/// profile.validate().expect("profile must satisfy all security invariants");
/// assert_eq!(profile.registry().len(), 2);
/// ```
#[derive(Debug)]
pub struct BdewAs4Profile {
    stack: ProfileStack,
    registry: PModeRegistry,
}

impl Default for BdewAs4Profile {
    fn default() -> Self {
        Self::new()
    }
}

impl BdewAs4Profile {
    /// Creates a new profile with the BDEW MaKo base stack and an empty P-Mode registry.
    pub fn new() -> Self {
        Self {
            stack: bdew_mako_profile_stack(),
            registry: PModeRegistry::new(),
        }
    }

    /// Returns the BDEW MaKo [`ProfileStack`].
    pub fn profile_stack(&self) -> &ProfileStack {
        &self.stack
    }

    /// Returns the P-Mode registry.
    pub fn registry(&self) -> &PModeRegistry {
        &self.registry
    }

    /// Register a [`PMode`] for a bilateral trading-partner channel.
    ///
    /// Returns `&mut self` for chaining.
    pub fn register_pmode(&mut self, pmode: PMode) -> &mut Self {
        self.registry.register(pmode);
        self
    }

    /// Resolve a P-Mode by partner GLN, service URI, and action URI.
    ///
    /// Returns `None` when no matching P-Mode is registered.
    pub fn resolve_pmode(&self, partner_gln: &str, service: &str, action: &str) -> Option<&PMode> {
        self.registry.resolve(partner_gln, service, action)
    }

    /// Validate the profile stack.
    ///
    /// Returns an error if any critical security invariant is violated (e.g.,
    /// both `require_signature` and `require_encryption` are `false`).
    ///
    /// Call this at startup before serving traffic to catch misconfiguration early.
    pub fn validate(&self) -> ProfileValidationResult<ProfileValidationReport> {
        self.stack.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants;
    use crate::pmode::{BdewAction, bdew_pmode};

    #[test]
    fn profile_stack_validates_without_errors() {
        let stack = bdew_mako_profile_stack();
        let report = stack
            .validate()
            .expect("BDEW base profile must pass validation");
        assert!(
            report.lints.is_empty(),
            "no redundant-override lints expected"
        );
    }

    #[test]
    fn profile_stack_name_and_version() {
        let stack = bdew_mako_profile_stack();
        assert_eq!(stack.base.name, PROFILE_NAME);
        assert_eq!(stack.base.version, PROFILE_VERSION);
    }

    #[test]
    fn profile_stack_security_policy() {
        let stack = bdew_mako_profile_stack();
        assert!(
            stack.base.security.require_signature,
            "signing must be required"
        );
        assert!(
            !stack.base.security.require_encryption,
            "encryption must be optional (not required)"
        );
    }

    #[test]
    fn profile_stack_mode_is_strict() {
        let stack = bdew_mako_profile_stack();
        assert_eq!(stack.base.mode, InteropMode::Strict);
    }

    #[test]
    fn profile_stack_no_as2_mic() {
        let stack = bdew_mako_profile_stack();
        assert!(
            !stack.base.validation.require_as2_mic,
            "AS2 MIC must not be required in an AS4 profile"
        );
    }

    #[test]
    fn bdew_as4_profile_register_and_resolve() {
        let mut profile = BdewAs4Profile::new();
        profile
            .register_pmode(bdew_pmode("pm-u", "9900000000001", BdewAction::Utilmd))
            .register_pmode(bdew_pmode("pm-a", "9900000000001", BdewAction::Aperak));

        assert_eq!(profile.registry().len(), 2);

        let pm = profile.resolve_pmode(
            "9900000000001",
            constants::SERVICE,
            &BdewAction::Utilmd.as_uri(),
        );
        assert!(pm.is_some());
        assert_eq!(pm.unwrap().id, "pm-u");

        assert!(
            profile
                .resolve_pmode(
                    "9999999999999",
                    constants::SERVICE,
                    &BdewAction::Utilmd.as_uri()
                )
                .is_none()
        );
    }

    #[test]
    fn bdew_as4_profile_validates() {
        let mut profile = BdewAs4Profile::new();
        profile.register_pmode(bdew_pmode("pm-u", "9900000000001", BdewAction::Utilmd));
        profile
            .validate()
            .expect("profile with registered P-Mode must validate");
    }

    #[test]
    fn bdew_as4_profile_default_equals_new() {
        let a = BdewAs4Profile::new();
        let b = BdewAs4Profile::default();
        assert_eq!(a.registry().len(), b.registry().len());
        assert_eq!(a.profile_stack().base.name, b.profile_stack().base.name);
    }
}
