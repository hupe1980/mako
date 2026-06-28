//! BDEW MaKo AS4 P-Mode types and factory functions.
//!
//! In ebMS3, a **P-Mode** specifies the complete protocol configuration for a
//! trading relationship: MEP, security, service/action URIs, and payload packaging.
//!
//! Use [`bdew_pmode`] to build pre-configured [`PMode`]s with BDEW defaults,
//! then register them with a [`PModeRegistry`]:
//!
//! ```rust
//! use mako_as4::pmode::{bdew_pmode, BdewAction, PModeRegistry};
//!
//! let mut registry = PModeRegistry::new();
//! registry.register(bdew_pmode(
//!     "pm-utilmd-9900000000001",
//!     "9900000000001",             // counterparty GLN
//!     BdewAction::Utilmd,
//! ));
//! assert_eq!(registry.len(), 1);
//! ```

use crate::constants;

// Re-export `asx_rs` P-Mode types so consumers don't need a direct `asx_rs` dep
// for the common registry / P-Mode operations.
pub use asx_rs::as4::pmode::{MepType, PMode, PModeRegistry, PModeSecurity, PayloadPackagingMode};

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

/// Build a BDEW MaKo P-Mode with opinionated defaults (no encryption).
///
/// The returned [`PMode`] is pre-configured with:
/// - `mep`: [`MepType::OneWayPush`] — mandatory per BDEW AS4 spec
/// - `service`: [`constants::SERVICE`]
/// - `service_type`: [`constants::SERVICE_TYPE`] (empty)
/// - `security.sign = true` — mandatory
/// - `security.encrypt = false` — optional; use [`bdew_pmode_encrypted`] if needed
/// - `payload_packaging`: [`PayloadPackagingMode::MimeAttachment`]
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
    partner_gln: impl Into<String>,
    action: BdewAction,
) -> PMode {
    PMode {
        id: id.into(),
        partner_id: partner_gln.into(),
        service: constants::SERVICE.to_string(),
        service_type: constants::SERVICE_TYPE.to_string(),
        action: action.as_uri(),
        mep: MepType::OneWayPush,
        security: PModeSecurity {
            sign: true,
            encrypt: false,
            encrypt_soap_headers: false,
            compress: false,
        },
        payload_packaging: PayloadPackagingMode::MimeAttachment,
    }
}

/// Build a BDEW MaKo P-Mode with payload encryption enabled.
///
/// Same as [`bdew_pmode`] but with `security.encrypt = true`.
/// The caller must supply `recipient_cert_pem` in [`asx_rs::as4::As4SendCredentials`]
/// when building the send policy from this P-Mode.
///
/// # Example
///
/// ```rust
/// use mako_as4::pmode::{bdew_pmode_encrypted, BdewAction};
///
/// let pm = bdew_pmode_encrypted(
///     "pm-utilmd-encrypted-9900000000001",
///     "9900000000001",
///     BdewAction::Utilmd,
/// );
/// assert!(pm.security.encrypt);
/// ```
pub fn bdew_pmode_encrypted(
    id: impl Into<String>,
    partner_gln: impl Into<String>,
    action: BdewAction,
) -> PMode {
    PMode {
        security: PModeSecurity {
            sign: true,
            encrypt: true,
            encrypt_soap_headers: false,
            compress: false,
        },
        ..bdew_pmode(id, partner_gln, action)
    }
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
        assert!(!pm.security.encrypt);
        assert_eq!(pm.payload_packaging, PayloadPackagingMode::MimeAttachment);
    }

    #[test]
    fn bdew_pmode_encrypted_sets_encrypt() {
        let pm = bdew_pmode_encrypted("pm-enc", "9900000000001", BdewAction::Aperak);
        assert!(pm.security.sign);
        assert!(pm.security.encrypt);
        assert_eq!(pm.mep, MepType::OneWayPush);
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
