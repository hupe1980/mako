//! BDEW MaKo AS4 profile stack and `BdewAs4Profile` entry point.
//!
//! [`bdew_mako_profile_stack`] returns an [`asx_rs`] [`ProfileStack`] pre-configured
//! for BDEW AS4 strict compliance.  [`BdewAs4Profile`] combines the profile stack
//! with a [`PModeRegistry`] for a single, startup-time entry point.

pub use asx_rs::as4::As4PushPolicy;
pub use asx_rs::as4::FragmentScopePolicy;
use asx_rs::core::InteropMode;
use asx_rs::interop::{
    As2ValidationPolicy, BaseProfile, CanonicalizationPolicy, ProfileStack,
    ProfileValidationReport, SecurityPolicy, ValidationPolicy,
};

use crate::{
    constants,
    pmode::{BdewAction, PMode, PModeRegistry, bdew_pmode_with_endpoint},
};

// ── Per-partner encryption certificate store ──────────────────────────────────
/// PEM-encoded X.509 certificate for encrypting outbound AS4 messages to a
/// specific trading partner.
///
/// Stored as `Arc<[u8]>` (byte slice) to allow cheap cloning across the
/// `BdewAs4Sender` and the `BdewAs4Profile`.
type EncryptionCertPem = std::sync::Arc<[u8]>;

/// Short identifier for the BDEW MaKo AS4 profile.
pub const PROFILE_NAME: &str = "bdew_mako_as4";

/// Profile version string (mirrors the AS4 Kommunikationshandbuch edition).
pub const PROFILE_VERSION: &str = "2.0.0";

/// Build an [`As4PushPolicy`] preset for BDEW AS4-Profil v1.2 inbound receive.
///
/// Equivalent to [`As4PushPolicy::regulated()`] with the operator's own
/// decryption private key wired in, enabling decryption of inbound encrypted
/// AS4 messages.
///
/// # BDEW AS4-Profil v1.2 §2.2.6.2.2
///
/// BDEW requires every inbound message to be encrypted with the operator's
/// EC (BrainpoolP256r1) public key. Supply the corresponding private key here
/// to decrypt them. The key must be in PEM format (PKCS#8 or SEC1 encoding).
///
/// # Parameters
///
/// - `decryption_key_pem` — `None` for sign-only mode (testing / before certs arrive).
///   `Some(pem_bytes)` for production with inbound decryption enabled.
///
/// # Example
///
/// ```rust
/// use mako_as4::profile::bdew_push_policy;
///
/// // Sign-only (development / no BDEW PKI certs yet)
/// let policy = bdew_push_policy(None);
///
/// // Production with inbound decryption
/// let key_pem: Vec<u8> = std::fs::read("/etc/certs/as4-decrypt.key.pem").unwrap_or_default();
/// let policy = bdew_push_policy(Some(key_pem));
/// ```
pub fn bdew_push_policy(decryption_key_pem: Option<Vec<u8>>) -> As4PushPolicy {
    match decryption_key_pem {
        // BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every inbound message to be
        // encrypted. `regulated_with_decryption_key` (asx-rs 0.11) sets the key and
        // `require_encrypted_inbound` together, so the invariant can no longer be
        // split across two calls.
        Some(key) => As4PushPolicy::regulated_with_decryption_key(key),
        // Sign-only (development / before the BDEW PKI certs arrive).
        None => As4PushPolicy::regulated(),
    }
    // No `fragment_scope_policy` override: BDEW MaKo sends single-message
    // `UserMessage` only, and `RequireAuthenticatedScope` (the strict default) is
    // consulted for *fragmented* messages only — a `None` scope is already safe,
    // and switching to `UseSoapSenderId` would only weaken the policy if a fragment
    // ever arrived (asx-rs 0.11 clarified this).
}

/// Creates a [`ProfileStack`] pre-configured for BDEW MaKo AS4 compliance.
///
/// The base profile enforces:
///
/// | Policy | Value | Source |
/// |---|---|---|
/// | Interop mode | `Strict` | BDEW requires full AS4 conformance |
/// | Canonicalization | Exclusive C14N, no comments | BDEW KH §5.5 |
/// | Signing required | `true` | AS4-Profil v1.2 §2.2.6.2.1 |
/// | Encryption required | `true` | AS4-Profil v1.2 §2.2.6.2.2 |
/// | Security floor | sign **and** encrypt | §2.2.6.2.2 — no override may relax it |
/// | Payload limits enforced | `true` | defense-in-depth |
///
/// §2.2.6.2.2 is unambiguous that encryption is a *MUSS*, and fixes the
/// algorithms: the key reference MUSS be `X509SKI`, the content algorithm MUSS
/// be `http://www.w3.org/2009/xmlenc11#aes128-gcm`, and key transport follows
/// BSI [TR-03116-3] §9.2. This table previously read "Encryption required:
/// `false` — BDEW KH §5.6 (optional)", which contradicted both the statute and
/// the code beneath it.
///
/// (Since asx-rs 0.11 the AS2 MIC knob lives in a separate `As2ValidationPolicy`,
/// not the shared AS4 `ValidationPolicy` — an AS4 profile no longer carries a field
/// that cannot apply to it.)
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
                // Mandatory per BDEW AS4-Profil v1.2 §2.2.6.2.2.
                // asx-rs implements ECDH-ES + ConcatKDF + AES-128-KW with
                // BrainpoolP256r1 (BSI TR-03116-3 §9.2) automatically when the
                // recipient certificate has an EC public key.
                require_encryption: true,
            },
            validation: ValidationPolicy {
                reject_ambiguous_headers: true,
                enforce_payload_limits: true,
            },
            // The floor no override may relax. asx-rs enforces this across the
            // base and every override layer during `validate()`, rejecting a
            // downgrade with `SecurityFloorViolation`.
            //
            // It matters because the generic AS4 invariant only rejects
            // disabling *both* signature and encryption. BDEW AS4-Profil v1.2
            // §2.2.6.2.2 requires both, so a layer that keeps signing and turns
            // encryption off satisfies the generic rule while breaking the
            // mandate — and every message to that partner would go out in the
            // clear.
            security_floor: SecurityPolicy::SIGN_AND_ENCRYPT,
            // AS2-only concern, modelled separately from the AS4 policy.
            as2_validation: As2ValidationPolicy { require_mic: false },
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
    /// Per-partner encryption certificates: `partner_mp_id → PEM cert bytes`.
    ///
    /// Used by the outbound send path to populate `As4SendCredentials::recipient_cert_pem`.
    /// BDEW AS4-Profil v1.2 §2.2.6.2.2 requires each message to be encrypted with the
    /// **recipient's** encryption certificate.
    encryption_certs: std::collections::HashMap<String, EncryptionCertPem>,
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
            encryption_certs: std::collections::HashMap::new(),
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
    /// [`bdew_pmode_with_endpoint`] and [`register_pmode`](Self::register_pmode) instead.
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

    /// Register the encryption certificate for a trading partner.
    ///
    /// `cert_pem` is the partner's X.509 certificate (PEM-encoded) used to encrypt
    /// outbound AS4 messages. Per BDEW AS4-Profil v1.2 §2.2.6.2.2 the recipient's
    /// encryption certificate is required for every outbound message when
    /// `security.encrypt = true`.
    ///
    /// The certificate is stored keyed by `partner_mp_id` (13-digit GLN).
    /// It is returned by [`get_partner_encryption_cert`](Self::get_partner_encryption_cert)
    /// for injection into `As4SendCredentials::recipient_cert_pem` at send time.
    ///
    /// # Note
    ///
    /// BDEW uses **separate** signing and encryption keypairs. The encryption certificate
    /// corresponds to the partner's EC keypair (BrainpoolP256r1). Do not use the signing
    /// certificate here.
    pub fn register_partner_encryption_cert(
        &mut self,
        partner_mp_id: impl Into<String>,
        cert_pem: impl Into<Vec<u8>>,
    ) -> &mut Self {
        self.encryption_certs
            .insert(partner_mp_id.into(), cert_pem.into().into());
        self
    }

    /// Return the encryption certificate PEM for a trading partner, if registered.
    ///
    /// Used by the outbound send path to populate `As4SendCredentials::recipient_cert_pem`.
    /// Returns `None` when no encryption certificate has been registered for this partner.
    pub fn get_partner_encryption_cert(&self, partner_mp_id: &str) -> Option<&[u8]> {
        self.encryption_certs
            .get(partner_mp_id)
            .map(|arc| arc.as_ref())
    }

    /// Returns `true` if at least one partner has an encryption certificate registered.
    pub fn has_any_encryption_certs(&self) -> bool {
        !self.encryption_certs.is_empty()
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

    /// Validate the profile stack against the BDEW MaKo mandate.
    ///
    /// Runs `asx-rs`'s generic AS4 validation and then the stricter BDEW rule on
    /// top of it.
    ///
    /// # Errors
    ///
    /// [`BdewProfileError`] when any AS4 invariant fails, including a layer that
    /// relaxes the sign-and-encrypt floor declared in [`bdew_mako_profile_stack`]
    /// (`ProfileValidationCode::SecurityFloorViolation`).
    ///
    /// Call this at startup before serving traffic, so a misconfigured partner
    /// overlay fails the process rather than silently downgrading its traffic.
    pub fn validate(&self) -> Result<ProfileValidationReport, BdewProfileError> {
        Ok(self.stack.validate()?)
    }
}

/// Startup validation of the BDEW MaKo AS4 profile failed.
#[derive(Debug, thiserror::Error)]
pub enum BdewProfileError {
    /// An AS4 profile invariant was violated.
    ///
    /// Includes the BDEW sign-and-encrypt floor: `asx-rs` reports a relaxing
    /// layer as `ProfileValidationCode::SecurityFloorViolation`, so the check
    /// mako used to perform itself now lives in the layer that owns policy
    /// resolution.
    #[error(transparent)]
    Validation(#[from] asx_rs::interop::ProfileValidationFailure),
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
            stack.base.security.require_encryption,
            "encryption must be required — BDEW AS4-Profil v1.2 §2.2.6.2.2"
        );
    }

    /// BDEW AS4-Profil v1.2 §2.2.6.2.2 requires MaKo messages to be signed **and**
    /// encrypted. `asx-rs` only rejects a layer that disables *both*, so a partner
    /// overlay that keeps signing and turns encryption off passes its check —
    /// and every message to that partner would go out unencrypted.
    ///
    /// `partner_overrides` is a public field, so this is reachable by
    /// configuration, not just in theory.
    #[test]
    fn a_partner_overlay_cannot_turn_off_encryption() {
        use asx_rs::interop::{
            PartnerProfileOverlay, ProfilePolicyOverrides, ProfileValidationCode,
        };

        let mut profile = BdewAs4Profile::new();
        profile.stack.partner_overrides.push(PartnerProfileOverlay {
            name: "legacy-partner".to_owned(),
            partner_id: "9900000000001".to_owned(),
            overrides: ProfilePolicyOverrides {
                security: Some(SecurityPolicy {
                    require_signature: true,
                    require_encryption: false,
                }),
                ..Default::default()
            },
        });

        // Signing stays on, so the generic "at least one of the two" invariant is
        // satisfied. Only the declared floor rejects this.
        let failure = profile
            .stack
            .validate()
            .expect_err("the sign-and-encrypt floor must refuse the downgrade");
        assert!(
            failure.has_code(ProfileValidationCode::SecurityFloorViolation),
            "expected a floor violation, got {failure}"
        );

        let err = profile
            .validate()
            .expect_err("and BdewAs4Profile::validate must surface it");
        assert!(
            err.to_string().contains("floor") || err.to_string().contains("encryption"),
            "the refusal should name what was relaxed: {err}"
        );
    }

    /// Signing may not be dropped either.
    #[test]
    fn a_layer_cannot_turn_off_signing() {
        use asx_rs::interop::{ProfileOverride, ProfilePolicyOverrides, ProfileValidationCode};

        let mut profile = BdewAs4Profile::new();
        profile.stack.overrides.push(ProfileOverride {
            name: "no-signing".to_owned(),
            overrides: ProfilePolicyOverrides {
                security: Some(SecurityPolicy {
                    require_signature: false,
                    require_encryption: true,
                }),
                ..Default::default()
            },
        });
        let failure = profile.stack.validate().expect_err("must refuse");
        assert!(
            failure.has_code(ProfileValidationCode::SecurityFloorViolation),
            "expected a floor violation, got {failure}"
        );
        assert!(profile.validate().is_err());
    }

    /// The floor is what makes the two tests above fail — assert it explicitly
    /// so a stack built without it cannot pass unnoticed.
    #[test]
    fn the_stack_declares_the_bdew_sign_and_encrypt_floor() {
        let stack = bdew_mako_profile_stack();
        assert_eq!(
            stack.base.security_floor,
            SecurityPolicy::SIGN_AND_ENCRYPT,
            "BDEW AS4-Profil v1.2 §2.2.6.2.2 requires signing AND encryption"
        );
    }

    /// The unmodified BDEW profile still validates.
    #[test]
    fn the_base_profile_passes_the_bdew_check() {
        let profile = BdewAs4Profile::new();
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn profile_stack_mode_is_strict() {
        let stack = bdew_mako_profile_stack();
        assert_eq!(stack.base.mode, InteropMode::Strict);
    }

    #[test]
    fn profile_stack_no_as2_mic() {
        let stack = bdew_mako_profile_stack();
        // Since asx-rs 0.11 the AS2 MIC knob lives in a separate As2ValidationPolicy,
        // not the AS4 ValidationPolicy; an AS4 profile keeps it disabled.
        assert!(
            !stack.base.as2_validation.require_mic,
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
