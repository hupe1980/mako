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

use crate::{
    constants,
    pmode::{BdewAction, PMode, PModeRegistry, bdew_pmode_with_endpoint},
};

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

    /// Register P-Modes for all standard BDEW EDIFACT message types with one call.
    ///
    /// For each [`BdewAction::all_standard()`] variant, creates a P-Mode with:
    /// - `endpoint_url = Some(endpoint_url)` (HTTPS validated at send time)
    /// - `security.sign = true`, `security.encrypt = false` (BDEW defaults)
    /// - `mep = OneWayPush`
    ///
    /// This is the recommended way to register a trading partner at startup
    /// when you know their single AS4 inbox URL and use BDEW default security
    /// settings (signing required, encryption optional).
    ///
    /// For per-action encryption overrides, register individual P-Modes via
    /// [`bdew_pmode_encrypted_with_endpoint`](crate::pmode::bdew_pmode_encrypted_with_endpoint)
    /// and [`register_pmode`](Self::register_pmode) instead.
    ///
    /// # Example
    ///
    /// ```rust
    /// use mako_as4::profile::BdewAs4Profile;
    /// use mako_as4::pmode::BdewAction;
    ///
    /// let mut profile = BdewAs4Profile::new();
    /// profile.register_partner_all_actions(
    ///     "9900000000001",
    ///     "https://partner.example/as4/inbox",
    /// );
    /// // One P-Mode per standard BDEW action variant
    /// assert_eq!(profile.registry().len(), BdewAction::all_standard().len());
    /// ```
    pub fn register_partner_all_actions(
        &mut self,
        partner_mp_id: impl Into<String>,
        endpoint_url: impl Into<String>,
    ) -> &mut Self {
        let mp_id: String = partner_mp_id.into();
        let url: String = endpoint_url.into();
        for action in BdewAction::all_standard() {
            let action_short = action
                .as_uri()
                .strip_prefix(constants::SERVICE)
                .and_then(|s| s.strip_prefix(':'))
                .unwrap_or("unknown")
                .to_ascii_lowercase();
            let id = format!("pm-{mp_id}-{action_short}");
            self.registry
                .register(bdew_pmode_with_endpoint(id, &mp_id, action, &url));
        }
        self
    }

    /// Resolve the first P-Mode for `partner_mp_id` matching this BDEW [`BdewAction`].
    ///
    /// Uses [`PModeRegistry::resolve_by_action`] against the BDEW action URI.
    /// Unlike [`resolve_pmode`](Self::resolve_pmode), the BDEW service URI
    /// ([`constants::SERVICE`]) does not need to match — only the partner GLN
    /// and action URI are compared.  In BDEW deployments this is the correct
    /// strategy since there is only one service URI.
    ///
    /// Returns `None` when no P-Mode is registered for `(partner_mp_id, action)`.
    pub fn resolve_pmode_by_action(
        &self,
        partner_mp_id: &str,
        action: &BdewAction,
    ) -> Option<&PMode> {
        self.registry
            .resolve_by_action(partner_mp_id, &action.as_uri())
    }

    /// All registered P-Modes.
    ///
    /// Useful for startup-validation logging (e.g. warn when a P-Mode has
    /// `endpoint_url = None`) and auditing the registry state.
    pub fn all_pmodes(&self) -> &[PMode] {
        self.registry.all()
    }

    /// Resolve the HTTPS endpoint URL for the first P-Mode matching `partner_mp_id`,
    /// `service`, and `action`.
    ///
    /// Returns `Some(&str)` when a matching P-Mode is registered **and** its
    /// [`PMode::endpoint_url`] field is populated.  Returns `None` when no P-Mode
    /// matches or when the matched P-Mode has `endpoint_url = None`.
    ///
    /// Use this as an alternative to a separate `PartnerDirectory` when endpoint
    /// URLs are baked into P-Mode registrations via [`bdew_pmode_with_endpoint`].
    ///
    /// [`bdew_pmode_with_endpoint`]: crate::pmode::bdew_pmode_with_endpoint
    pub fn resolve_endpoint(
        &self,
        partner_mp_id: &str,
        service: &str,
        action: &str,
    ) -> Option<&str> {
        self.registry
            .resolve(partner_mp_id, service, action)
            .and_then(|pm| pm.endpoint_url.as_deref())
    }

    /// Resolve a P-Mode by partner GLN, service URI, and action URI.
    ///
    /// Returns `None` when no matching P-Mode is registered.
    pub fn resolve_pmode(
        &self,
        partner_mp_id: &str,
        service: &str,
        action: &str,
    ) -> Option<&PMode> {
        self.registry.resolve(partner_mp_id, service, action)
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

    #[test]
    fn resolve_endpoint_returns_url_when_baked_in() {
        use crate::pmode::bdew_pmode_with_endpoint;
        let mut profile = BdewAs4Profile::new();
        profile.register_pmode(bdew_pmode_with_endpoint(
            "pm-u",
            "9900000000001",
            BdewAction::Utilmd,
            "https://partner.example/as4",
        ));
        let url = profile.resolve_endpoint(
            "9900000000001",
            constants::SERVICE,
            &BdewAction::Utilmd.as_uri(),
        );
        assert_eq!(url, Some("https://partner.example/as4"));
    }

    #[test]
    fn resolve_endpoint_returns_none_when_not_set() {
        let mut profile = BdewAs4Profile::new();
        profile.register_pmode(bdew_pmode("pm-u", "9900000000001", BdewAction::Utilmd));
        assert!(
            profile
                .resolve_endpoint(
                    "9900000000001",
                    constants::SERVICE,
                    &BdewAction::Utilmd.as_uri()
                )
                .is_none()
        );
    }

    #[test]
    fn register_partner_all_actions_creates_one_pmode_per_standard_action() {
        use crate::pmode::BdewAction;
        let mut profile = BdewAs4Profile::new();
        profile.register_partner_all_actions("9900000000001", "https://partner.example/as4/inbox");
        assert_eq!(profile.registry().len(), BdewAction::all_standard().len());
        // Every P-Mode must carry the endpoint
        for pm in profile.all_pmodes() {
            assert_eq!(
                pm.endpoint_url.as_deref(),
                Some("https://partner.example/as4/inbox"),
            );
        }
    }

    #[test]
    fn register_partner_all_actions_chaining() {
        let mut profile = BdewAs4Profile::new();
        profile
            .register_partner_all_actions("9900000000001", "https://a.example/as4")
            .register_partner_all_actions("9900000000002", "https://b.example/as4");
        use crate::pmode::BdewAction;
        assert_eq!(
            profile.registry().len(),
            2 * BdewAction::all_standard().len()
        );
    }

    #[test]
    fn resolve_pmode_by_action_finds_registered_pmode() {
        use crate::pmode::BdewAction;
        let mut profile = BdewAs4Profile::new();
        profile.register_partner_all_actions("9900000000001", "https://partner.example/as4/inbox");
        let pm = profile.resolve_pmode_by_action("9900000000001", &BdewAction::Utilmd);
        assert!(pm.is_some());
        assert_eq!(pm.unwrap().partner_id, "9900000000001");
        assert_eq!(pm.unwrap().action, BdewAction::Utilmd.as_uri());
    }

    #[test]
    fn resolve_pmode_by_action_returns_none_for_unknown_partner() {
        use crate::pmode::BdewAction;
        let mut profile = BdewAs4Profile::new();
        profile.register_partner_all_actions("9900000000001", "https://partner.example/as4");
        assert!(
            profile
                .resolve_pmode_by_action("9999999999999", &BdewAction::Utilmd)
                .is_none()
        );
    }

    #[test]
    fn all_pmodes_reflects_registered_pmode_count() {
        use crate::pmode::BdewAction;
        let mut profile = BdewAs4Profile::new();
        assert!(profile.all_pmodes().is_empty());
        profile.register_partner_all_actions("9900000000001", "https://a.example/as4");
        assert_eq!(profile.all_pmodes().len(), BdewAction::all_standard().len());
    }
}
