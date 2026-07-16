//! BDEW MaKo AS4 P-Mode types and factory functions.
//!
//! In ebMS3, a **P-Mode** specifies the complete protocol configuration for a
//! trading relationship: MEP, security, service/action URIs, payload packaging,
//! and the partner's HTTPS endpoint URL.
//!
//! Use [`bdew_pmode`] to build pre-configured [`PMode`]s with BDEW defaults,
//! then register them with a [`PModeRegistry`]:
//!
//! ```rust
//! use mako_as4::pmode::{bdew_pmode, bdew_pmode_with_endpoint, BdewAction, PModeRegistry};
//!
//! // Without endpoint (endpoint supplied separately at send time)
//! let mut registry = PModeRegistry::new();
//! registry.register(bdew_pmode(
//!     "pm-utilmd-9900000000001",
//!     "9900000000001",             // counterparty GLN
//!     BdewAction::Utilmd,
//! ));
//! assert_eq!(registry.len(), 1);
//!
//! // With endpoint baked into the P-Mode (recommended)
//! let pm = bdew_pmode_with_endpoint(
//!     "pm-aperak-9900000000001",
//!     "9900000000001",
//!     BdewAction::Aperak,
//!     "https://partner.example/as4/inbox",
//! );
//! assert_eq!(pm.endpoint_url.as_deref(), Some("https://partner.example/as4/inbox"));
//! ```

use crate::constants;

// Re-export `asx_rs` P-Mode types so consumers don't need a direct `asx_rs` dep
// for the common registry / P-Mode operations.
pub use asx_rs::as4::pmode::{MepType, PMode, PModeRegistry, PModeSecurity, PayloadPackagingMode};
pub use asx_rs::crypto::wssec::WsSecOutboundKeyInfoProfile;

/// EDIFACT message types used in BDEW German energy market communication.
///
/// Each variant maps to an `<eb:Action>` URI in the form
/// `{SERVICE}:{EDIFACT_TYPE}`.  The full URI is returned by [`as_uri`](Self::as_uri).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BdewAction {
    /// UTILMD — market location master data (GPKE, WiM, GeLi Gas).
    Utilmd,
    /// APERAK — application error acknowledgement (all processes).
    Aperak,
    /// CONTRL — syntax and content acknowledgement.
    Contrl,
    /// MSCONS — metered service consumption (metering data).
    Mscons,
    /// INVOIC — invoice (MABIS billing).
    Invoic,
    /// REMADV — remittance advice (MABIS billing).
    Remadv,
    /// IFTSTA — transport status (GPKE).
    Iftsta,
    /// ORDRSP — order response (GPKE).
    Ordrsp,
    /// ORDERS — orders (GPKE).
    Orders,
    /// ORDCHG — order change (GPKE).
    Ordchg,
    /// REQOTE — request for quotation.
    Reqote,
    /// INSRPT — inspection report (GPKE metering).
    Insrpt,
    /// PRICAT — price catalogue (MABIS tariff data).
    Pricat,
    /// QUOTES — quotes (bidding).
    Quotes,
    /// Custom action URI for non-standard or future EDIFACT message types.
    Custom(String),
}

impl BdewAction {
    /// Returns all standard (non-[`Custom`]) BDEW action variants.
    ///
    /// Useful for registering bilateral P-Modes for every known BDEW EDIFACT
    /// message type in a single call (see
    /// [`BdewAs4Profile::register_partner_all_actions`]).
    ///
    /// [`Custom`]: Self::Custom
    /// [`BdewAs4Profile::register_partner_all_actions`]: crate::profile::BdewAs4Profile::register_partner_all_actions
    pub fn all_standard() -> Vec<Self> {
        vec![
            Self::Utilmd,
            Self::Aperak,
            Self::Contrl,
            Self::Mscons,
            Self::Invoic,
            Self::Remadv,
            Self::Iftsta,
            Self::Ordrsp,
            Self::Orders,
            Self::Ordchg,
            Self::Reqote,
            Self::Insrpt,
            Self::Pricat,
            Self::Quotes,
        ]
    }

    /// Construct a [`BdewAction`] from an EDIFACT message-type string (e.g. `"APERAK"`).
    ///
    /// Matches all known BDEW message types case-sensitively.  Unknown types
    /// map to [`BdewAction::Custom`] with the full BDEW action URI so delivery
    /// is still attempted rather than silently failed.
    pub fn from_message_type_str(message_type: &str) -> Self {
        match message_type {
            "UTILMD" => Self::Utilmd,
            "APERAK" => Self::Aperak,
            "CONTRL" => Self::Contrl,
            "MSCONS" => Self::Mscons,
            "INVOIC" => Self::Invoic,
            "REMADV" => Self::Remadv,
            "IFTSTA" => Self::Iftsta,
            "ORDRSP" => Self::Ordrsp,
            "ORDERS" => Self::Orders,
            "ORDCHG" => Self::Ordchg,
            "REQOTE" => Self::Reqote,
            "INSRPT" => Self::Insrpt,
            "PRICAT" => Self::Pricat,
            "QUOTES" => Self::Quotes,
            other => Self::Custom(format!("{}:{}", crate::constants::SERVICE, other)),
        }
    }

    /// Returns the full BDEW AS4 action URI for this message type.
    ///
    /// Format: `{constants::SERVICE}:{EDIFACT_TYPE}`
    /// (e.g., `"urn:bdew:as4:service:UTILMD"`).
    pub fn as_uri(&self) -> String {
        let svc = constants::SERVICE;
        match self {
            Self::Utilmd => format!("{svc}:UTILMD"),
            Self::Aperak => format!("{svc}:APERAK"),
            Self::Contrl => format!("{svc}:CONTRL"),
            Self::Mscons => format!("{svc}:MSCONS"),
            Self::Invoic => format!("{svc}:INVOIC"),
            Self::Remadv => format!("{svc}:REMADV"),
            Self::Iftsta => format!("{svc}:IFTSTA"),
            Self::Ordrsp => format!("{svc}:ORDRSP"),
            Self::Orders => format!("{svc}:ORDERS"),
            Self::Ordchg => format!("{svc}:ORDCHG"),
            Self::Reqote => format!("{svc}:REQOTE"),
            Self::Insrpt => format!("{svc}:INSRPT"),
            Self::Pricat => format!("{svc}:PRICAT"),
            Self::Quotes => format!("{svc}:QUOTES"),
            Self::Custom(uri) => uri.clone(),
        }
    }
}

/// Build a BDEW MaKo P-Mode with BDEW-compliant security defaults.
///
/// The returned [`PMode`] is pre-configured with:
/// - `mep`: [`MepType::OneWayPush`] — mandatory per BDEW AS4-Profil §2.2
/// - `service`: [`constants::SERVICE`]
/// - `service_type`: [`constants::SERVICE_TYPE`] (empty)
/// - `security.sign = true` — mandatory (ECDSA-SHA256 + BrainpoolP256r1 per §2.2.6.2.1)
/// - `security.encrypt = true` — **mandatory** per BDEW AS4-Profil v1.2 §2.2.6.2.2
///   (ECDH-ES + ConcatKDF + AES-128-GCM with BrainpoolP256r1 per BSI TR-03116-3 §9.2)
/// - `security.outbound_key_info_profile`: [`WsSecOutboundKeyInfoProfile::X509PKIPathv1`]
///   — mandatory per BDEW AS4-Profil §2.2.6.2.1 (PKI path token type)
/// - `payload_packaging`: [`PayloadPackagingMode::MimeAttachment`]
/// - `endpoint_url`: `None` — use [`bdew_pmode_with_endpoint`] to include the URL
///
/// # Algorithm selection
///
/// asx-rs v0.6 selects the WS-Security algorithm automatically from the
/// signing key type:
/// - EC key (BrainpoolP256r1) → ECDSA-SHA256 (BDEW-compliant)
/// - RSA key → RSA-SHA256 (not BDEW-compliant; use only for testing)
///
/// For the encryption path, the key transport algorithm is selected from the
/// **recipient** certificate's key type at send time:
/// - EC certificate → ECDH-ES + ConcatKDF + AES-128-KW (BDEW-compliant)
/// - RSA certificate → RSA-OAEP (not BDEW-compliant)
///
/// For BDEW production deployments supply BrainpoolP256r1 EC credentials for
/// both signing and encryption. Use [`crate::testing::BdewTestPki`] for
/// ephemeral test keypairs.
///
/// Register the returned value in a [`PModeRegistry`].
///
/// # Example
///
/// ```rust
/// use mako_as4::pmode::{bdew_pmode, BdewAction, PModeRegistry};
///
/// let mut registry = PModeRegistry::new();
/// registry.register(bdew_pmode(
///     "pm-utilmd-9900000000001",
///     "9900000000001",
///     BdewAction::Utilmd,
/// ));
/// ```
pub fn bdew_pmode(
    id: impl Into<String>,
    partner_mp_id: impl Into<String>,
    action: BdewAction,
) -> PMode {
    PMode {
        id: id.into(),
        partner_id: partner_mp_id.into(),
        service: constants::SERVICE.to_string(),
        service_type: constants::SERVICE_TYPE.to_string(),
        action: action.as_uri(),
        mep: MepType::OneWayPush,
        security: PModeSecurity {
            sign: true,
            // Mandatory per BDEW AS4-Profil v1.2 §2.2.6.2.2.
            // Requires `recipient_cert_pem` in `As4SendCredentials` at send time.
            encrypt: true,
            encrypt_soap_headers: false,
            compress: false,
            // BDEW AS4-Profil §2.2.6.2.1 requires X509PKIPathv1 BST token type.
            outbound_key_info_profile: WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        },
        payload_packaging: PayloadPackagingMode::MimeAttachment,
        endpoint_url: None,
    }
}

/// Build a BDEW MaKo P-Mode with the partner's HTTPS AS4 endpoint baked in.
///
/// Same as [`bdew_pmode`] but populates `endpoint_url` so the P-Mode carries
/// everything needed for outbound delivery — no separate `PartnerDirectory`
/// lookup required.
///
/// Both signing and encryption are enabled per BDEW AS4-Profil v1.2 §2.2.6.
/// Supply `recipient_cert_pem` in [`asx_rs::as4::As4SendCredentials`] at send time.
///
/// # Example
///
/// ```rust
/// use mako_as4::pmode::{bdew_pmode_with_endpoint, BdewAction};
///
/// let pm = bdew_pmode_with_endpoint(
///     "pm-utilmd-9900000000001",
///     "9900000000001",
///     BdewAction::Utilmd,
///     "https://partner.example/as4/inbox",
/// );
/// assert_eq!(pm.endpoint_url.as_deref(), Some("https://partner.example/as4/inbox"));
/// assert!(pm.security.encrypt);
/// ```
pub fn bdew_pmode_with_endpoint(
    id: impl Into<String>,
    partner_mp_id: impl Into<String>,
    action: BdewAction,
    endpoint_url: impl Into<String>,
) -> PMode {
    PMode {
        endpoint_url: Some(endpoint_url.into()),
        ..bdew_pmode(id, partner_mp_id, action)
    }
}

/// Build a BDEW MaKo P-Mode with signing only (non-production / testing).
///
/// Same as [`bdew_pmode`] but with `security.encrypt = false`.
///
/// # ⚠ Non-BDEW-compliant
///
/// This produces sign-only messages. BDEW AS4-Profil v1.2 §2.2.6.2.2 **requires**
/// encryption in production. Use this variant only in:
/// - Local development without BDEW PKI certificates
/// - Tests that bypass AS4 transport entirely (`--allow-no-as4-signing`)
/// - Pre-production smoke testing with bilateral test agreements
///
/// For BDEW production deployments, use [`bdew_pmode`] (sign + encrypt).
///
/// # Example
///
/// ```rust
/// use mako_as4::pmode::{bdew_pmode_sign_only, BdewAction};
///
/// let pm = bdew_pmode_sign_only(
///     "pm-dev-9900000000001",
///     "9900000000001",
///     BdewAction::Utilmd,
/// );
/// assert!(pm.security.sign);
/// assert!(!pm.security.encrypt);
/// ```
pub fn bdew_pmode_sign_only(
    id: impl Into<String>,
    partner_mp_id: impl Into<String>,
    action: BdewAction,
) -> PMode {
    PMode {
        security: PModeSecurity {
            sign: true,
            encrypt: false,
            encrypt_soap_headers: false,
            compress: false,
            outbound_key_info_profile: WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        },
        ..bdew_pmode(id, partner_mp_id, action)
    }
}

/// Build a BDEW MaKo P-Mode with payload encryption enabled.
///
/// Alias for [`bdew_pmode`] — encryption is now the default.
/// Kept for source compatibility only.
#[inline]
pub fn bdew_pmode_encrypted(
    id: impl Into<String>,
    partner_mp_id: impl Into<String>,
    action: BdewAction,
) -> PMode {
    bdew_pmode(id, partner_mp_id, action)
}

/// Build a BDEW MaKo P-Mode with encryption enabled and endpoint baked in.
///
/// Alias for [`bdew_pmode_with_endpoint`] — encryption is now the default.
pub fn bdew_pmode_encrypted_with_endpoint(
    id: impl Into<String>,
    partner_mp_id: impl Into<String>,
    action: BdewAction,
    endpoint_url: impl Into<String>,
) -> PMode {
    bdew_pmode_with_endpoint(id, partner_mp_id, action, endpoint_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants;

    #[test]
    fn bdew_action_utilmd_uri() {
        assert_eq!(BdewAction::Utilmd.as_uri(), "urn:bdew:as4:service:UTILMD");
    }

    #[test]
    fn bdew_action_aperak_uri() {
        assert_eq!(BdewAction::Aperak.as_uri(), "urn:bdew:as4:service:APERAK");
    }

    #[test]
    fn bdew_action_custom_uri_passthrough() {
        let uri = "urn:custom:action:FOO";
        assert_eq!(BdewAction::Custom(uri.to_string()).as_uri(), uri);
    }

    #[test]
    fn bdew_pmode_defaults() {
        let pm = bdew_pmode("pm-1", "9900000000001", BdewAction::Utilmd);
        assert_eq!(pm.partner_id, "9900000000001");
        assert_eq!(pm.service, constants::SERVICE);
        assert_eq!(pm.service_type, "");
        assert_eq!(pm.action, BdewAction::Utilmd.as_uri());
        assert_eq!(pm.mep, MepType::OneWayPush);
        assert!(pm.security.sign);
        assert!(
            pm.security.encrypt,
            "BDEW AS4-Profil v1.2 §2.2.6.2.2 requires encryption"
        );
        assert_eq!(pm.payload_packaging, PayloadPackagingMode::MimeAttachment);
        assert!(
            pm.endpoint_url.is_none(),
            "bdew_pmode leaves endpoint_url unset"
        );
    }

    #[test]
    fn bdew_pmode_with_endpoint_sets_url() {
        let url = "https://partner.example/as4/inbox";
        let pm = bdew_pmode_with_endpoint("pm-1", "9900000000001", BdewAction::Utilmd, url);
        assert_eq!(pm.endpoint_url.as_deref(), Some(url));
        assert!(pm.security.sign);
        assert!(
            pm.security.encrypt,
            "bdew_pmode_with_endpoint must inherit encrypt:true from bdew_pmode"
        );
    }

    #[test]
    fn bdew_pmode_encrypted_sets_encrypt() {
        let pm = bdew_pmode_encrypted("pm-enc", "9900000000001", BdewAction::Aperak);
        assert!(pm.security.sign);
        assert!(pm.security.encrypt);
        assert_eq!(pm.mep, MepType::OneWayPush);
        assert!(pm.endpoint_url.is_none());
    }

    #[test]
    fn bdew_pmode_encrypted_with_endpoint_sets_both() {
        let url = "https://enc-partner.example/as4";
        let pm = bdew_pmode_encrypted_with_endpoint(
            "pm-enc-ep",
            "9900000000002",
            BdewAction::Mscons,
            url,
        );
        assert!(pm.security.encrypt);
        assert_eq!(pm.endpoint_url.as_deref(), Some(url));
    }

    #[test]
    fn pmode_registry_resolves_by_partner_and_action() {
        let mut registry = PModeRegistry::new();
        registry.register(bdew_pmode("pm-u", "9900000000001", BdewAction::Utilmd));
        registry.register(bdew_pmode("pm-a", "9900000000001", BdewAction::Aperak));

        let pm = registry.resolve(
            "9900000000001",
            constants::SERVICE,
            &BdewAction::Utilmd.as_uri(),
        );
        assert!(pm.is_some());
        assert_eq!(pm.unwrap().id, "pm-u");

        assert!(
            registry
                .resolve(
                    "9900000000002",
                    constants::SERVICE,
                    &BdewAction::Utilmd.as_uri()
                )
                .is_none()
        );
    }
}
