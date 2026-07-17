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
//! // Without endpoint (endpoint supplied at send time)
//! let mut registry = PModeRegistry::new();
//! registry.register(bdew_pmode(
//!     "pm-utilmd-9900000000001",
//!     "9900000000001",             // counterparty GLN
//!     BdewAction::Utilmd,
//! ));
//! assert_eq!(registry.len(), 1);
//!
//! // With endpoint baked into the P-Mode (recommended for static configurations)
//! let pm = bdew_pmode_with_endpoint(
//!     "pm-aperak-9900000000001",
//!     "9900000000001",
//!     BdewAction::Aperak,
//!     "https://partner.example/as4/inbox",
//! );
//! assert_eq!(pm.endpoint_url.as_deref(), Some("https://partner.example/as4/inbox"));
//! ```

use std::fmt;
use std::str::FromStr;

use crate::constants;

// Re-export `asx_rs` P-Mode types so consumers don't need a direct `asx_rs` dep
// for the common registry / P-Mode operations.
pub use asx_rs::as4::pmode::{MepType, PMode, PModeRegistry, PModeSecurity, PayloadPackagingMode};
pub use asx_rs::crypto::wssec::WsSecOutboundKeyInfoProfile;

// ── BdewAction ────────────────────────────────────────────────────────────────

/// EDIFACT message types used in BDEW German energy market communication.
///
/// Each variant maps to an AS4 `<eb:Action>` URI in the form
/// `{SERVICE}:{EDIFACT_TYPE}` where `SERVICE = "urn:bdew:as4:service"`.
///
/// ## Conversion methods
///
/// | Method | Returns | Example |
/// |---|---|---|
/// | [`as_edifact_type()`] | `&str` — EDIFACT type name | `"UTILMD"` |
/// | [`as_uri()`] | `String` — full AS4 action URI | `"urn:bdew:as4:service:UTILMD"` |
/// | [`fmt::Display`] | EDIFACT type name | `"UTILMD"` |
/// | [`FromStr`] | parse from EDIFACT type string | `"APERAK".parse()` |
///
/// ## Coverage
///
/// All 16 standard BDEW EDIFACT message types (UTILMD, APERAK, CONTRL, MSCONS,
/// INVOIC, REMADV, IFTSTA, ORDRSP, ORDERS, ORDCHG, REQOTE, INSRPT, PRICAT,
/// QUOTES, PARTIN, UTILTS) are covered by named variants. Any other type maps
/// to [`Custom`], which stores the full action URI verbatim.
///
/// [`as_edifact_type()`]: Self::as_edifact_type
/// [`as_uri()`]: Self::as_uri
/// [`Custom`]: Self::Custom
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BdewAction {
    /// UTILMD — market location master data (GPKE, WiM, GeLi Gas, WiM Gas).
    Utilmd,
    /// APERAK — application error acknowledgement (all processes).
    Aperak,
    /// CONTRL — interchange/message syntax acknowledgement (all processes).
    Contrl,
    /// MSCONS — metered service consumption; meter readings / time series data.
    Mscons,
    /// INVOIC — invoice (GPKE NNE, WiM MSB, GeLi Gas AWH, MABIS).
    Invoic,
    /// REMADV — remittance advice; payment confirmation (GPKE, MABIS billing).
    Remadv,
    /// IFTSTA — interchange transport status; GPKE Sperrung / Entsperrung confirmation.
    Iftsta,
    /// ORDRSP — order response (GPKE Konfiguration, WiM Geräteübernahme).
    Ordrsp,
    /// ORDERS — order / commissioning request (WiM Geräteübernahme, GPKE Sperrung).
    Orders,
    /// ORDCHG — order change (WiM Stornierung, GeLi Gas AWH Stornierung).
    Ordchg,
    /// REQOTE — request for quotation (WiM Preisanfrage MSB).
    Reqote,
    /// INSRPT — inspection report (WiM Ablesesteuerung / Gerätebefund).
    Insrpt,
    /// PRICAT — price catalogue (WiM Preisliste MSB).
    Pricat,
    /// QUOTES — quote response (WiM Preisanfrage MSB response).
    Quotes,
    /// PARTIN — party information; BDEW Kommunikationsdaten exchange
    /// (GPKE PIDs 37000–37006, GeLi Gas PIDs 37008–37014).
    Partin,
    /// UTILTS — utility time series; GPKE UTILTS Konfigurationsdaten and
    /// MaBiS Summenzeitreihen exchange (ÜNB ↔ BIKO).
    Utilts,
    /// Custom action URI for non-standard or future EDIFACT message types.
    ///
    /// Stores the **full** AS4 action URI (e.g. `"urn:bdew:as4:service:SLSFCT"`),
    /// not just the type name. Use [`BdewAction::custom`] to build it from a
    /// type name string, or supply the full URI directly.
    Custom(String),
}

impl BdewAction {
    /// Build a [`Custom`] action from an EDIFACT type name string.
    ///
    /// Composes `{SERVICE}:{type_name}` (e.g. `"SLSFCT"` →
    /// `"urn:bdew:as4:service:SLSFCT"`).
    ///
    /// Use when the type name is not covered by a named variant. For known
    /// types prefer the named variant (e.g. [`BdewAction::Utilmd`]).
    ///
    /// [`Custom`]: Self::Custom
    #[must_use]
    pub fn custom(type_name: impl Into<String>) -> Self {
        Self::Custom(format!(
            "{}:{}",
            crate::constants::SERVICE,
            type_name.into()
        ))
    }

    /// Returns all 16 standard (non-[`Custom`]) BDEW action variants.
    ///
    /// Useful for registering bilateral P-Modes for every known BDEW EDIFACT
    /// message type in one call:
    ///
    /// ```rust
    /// use mako_as4::pmode::{bdew_pmode, BdewAction, PModeRegistry};
    ///
    /// let mut registry = PModeRegistry::new();
    /// for action in BdewAction::all_standard() {
    ///     registry.register(bdew_pmode(
    ///         format!("pm-{}-partner-a", action),
    ///         "9900000000001",
    ///         action,
    ///     ));
    /// }
    /// assert_eq!(registry.len(), 16);
    /// ```
    ///
    /// See also [`BdewAs4Profile::register_partner_all_actions`] which wraps
    /// this in a single call.
    ///
    /// [`Custom`]: Self::Custom
    /// [`BdewAs4Profile::register_partner_all_actions`]: crate::profile::BdewAs4Profile::register_partner_all_actions
    #[must_use]
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
            Self::Partin,
            Self::Utilts,
        ]
    }

    /// Returns the EDIFACT message-type name for standard variants.
    ///
    /// For `Custom` variants the stored URI string is returned unchanged (it
    /// may be a full URI or a bare type name depending on how the value was
    /// constructed).
    ///
    /// This method avoids a heap allocation compared to [`as_uri()`] when only
    /// the type name is needed (e.g. for logging or EDIFACT header construction).
    ///
    /// ```rust
    /// use mako_as4::pmode::BdewAction;
    ///
    /// assert_eq!(BdewAction::Utilmd.as_edifact_type(), "UTILMD");
    /// assert_eq!(BdewAction::Partin.as_edifact_type(), "PARTIN");
    /// assert_eq!(BdewAction::Utilts.as_edifact_type(), "UTILTS");
    /// ```
    ///
    /// [`as_uri()`]: Self::as_uri
    #[must_use]
    pub fn as_edifact_type(&self) -> &str {
        match self {
            Self::Utilmd => "UTILMD",
            Self::Aperak => "APERAK",
            Self::Contrl => "CONTRL",
            Self::Mscons => "MSCONS",
            Self::Invoic => "INVOIC",
            Self::Remadv => "REMADV",
            Self::Iftsta => "IFTSTA",
            Self::Ordrsp => "ORDRSP",
            Self::Orders => "ORDERS",
            Self::Ordchg => "ORDCHG",
            Self::Reqote => "REQOTE",
            Self::Insrpt => "INSRPT",
            Self::Pricat => "PRICAT",
            Self::Quotes => "QUOTES",
            Self::Partin => "PARTIN",
            Self::Utilts => "UTILTS",
            // Custom stores the full URI; return as-is.
            Self::Custom(uri) => uri.as_str(),
        }
    }

    /// Returns the full BDEW AS4 action URI.
    ///
    /// Format: `"urn:bdew:as4:service:{EDIFACT_TYPE}"`
    ///
    /// For `Custom` variants the stored URI is returned as-is.
    ///
    /// ```rust
    /// use mako_as4::pmode::BdewAction;
    ///
    /// assert_eq!(BdewAction::Utilmd.as_uri(), "urn:bdew:as4:service:UTILMD");
    /// assert_eq!(BdewAction::Partin.as_uri(), "urn:bdew:as4:service:PARTIN");
    /// assert_eq!(BdewAction::Utilts.as_uri(), "urn:bdew:as4:service:UTILTS");
    ///
    /// let uri = "urn:custom:action:FOO";
    /// assert_eq!(BdewAction::Custom(uri.to_string()).as_uri(), uri);
    /// ```
    #[must_use]
    pub fn as_uri(&self) -> String {
        match self {
            Self::Custom(uri) => uri.clone(),
            _ => format!("{}:{}", constants::SERVICE, self.as_edifact_type()),
        }
    }
}

// ── fmt::Display ─────────────────────────────────────────────────────────────

/// Formats the EDIFACT type name (e.g. `"UTILMD"`, `"APERAK"`).
///
/// For `Custom` variants the stored URI string is displayed unchanged.
impl fmt::Display for BdewAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_edifact_type())
    }
}

// ── FromStr ───────────────────────────────────────────────────────────────────

/// Parse a [`BdewAction`] from an EDIFACT message-type name string.
///
/// Matches all 16 standard BDEW type names case-sensitively. Any unrecognised
/// string maps to [`BdewAction::custom`] — the parse never fails, so delivery
/// is attempted rather than being rejected at parse time.
///
/// [`ParseBdewActionError`] is an uninhabited enum provided for API completeness.
///
/// ```rust
/// use mako_as4::pmode::BdewAction;
///
/// assert_eq!("UTILMD".parse::<BdewAction>().unwrap(), BdewAction::Utilmd);
/// assert_eq!("PARTIN".parse::<BdewAction>().unwrap(), BdewAction::Partin);
/// assert_eq!("UTILTS".parse::<BdewAction>().unwrap(), BdewAction::Utilts);
///
/// // Unknown type — maps to Custom (never an Err)
/// let action: BdewAction = "SLSFCT".parse().unwrap();
/// assert!(matches!(action, BdewAction::Custom(_)));
/// ```
impl FromStr for BdewAction {
    type Err = ParseBdewActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let action = match s {
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
            "PARTIN" => Self::Partin,
            "UTILTS" => Self::Utilts,
            other => Self::custom(other),
        };
        Ok(action)
    }
}

/// Uninhabited error type for [`BdewAction`]'s [`FromStr`] implementation.
///
/// `BdewAction::from_str` never returns `Err` — unrecognised strings fall
/// through to [`BdewAction::Custom`].  This type exists purely to satisfy the
/// [`FromStr`] associated-type constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseBdewActionError {}

impl fmt::Display for ParseBdewActionError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Uninhabited — never reached.
        unreachable!()
    }
}

impl std::error::Error for ParseBdewActionError {}

// ── Convenience free function ─────────────────────────────────────────────────

/// Construct a [`BdewAction`] from an EDIFACT message-type name string.
///
/// Equivalent to `message_type.parse::<BdewAction>().unwrap()`.
///
/// Provided as a free function for call sites (e.g. the outbox delivery loop)
/// that need to convert a raw EDIFACT type string into an action without
/// importing the [`FromStr`] trait.
#[must_use]
#[inline]
pub fn bdew_action_from_str(message_type: &str) -> BdewAction {
    // Safety: FromStr for BdewAction is infallible.
    message_type
        .parse::<BdewAction>()
        .unwrap_or_else(|_| BdewAction::custom(message_type))
}

// ── P-Mode factory functions ──────────────────────────────────────────────────

/// Build a BDEW MaKo P-Mode with BDEW AS4-Profil v1.2 compliant security defaults.
///
/// The returned [`PMode`] is pre-configured with:
///
/// | Field | Value | BDEW source |
/// |---|---|---|
/// | `mep` | [`MepType::OneWayPush`] | AS4-Profil §2.2 |
/// | `service` | [`constants::SERVICE`] | AS4-Profil §3.1 |
/// | `service_type` | `""` (omitted) | AS4-Profil §3.1 |
/// | `security.sign` | `true` — ECDSA-SHA256 + BrainpoolP256r1 | §2.2.6.2.1, BSI TR-03116-3 §9.1 |
/// | `security.encrypt` | `true` — ECDH-ES + ConcatKDF + AES-128-GCM | §2.2.6.2.2, BSI TR-03116-3 §9.2 |
/// | `security.outbound_key_info_profile` | [`X509PKIPathv1`] | §2.2.6.2.1 |
/// | `payload_packaging` | [`MimeAttachment`] | AS4-Profil §3.3 |
/// | `endpoint_url` | `None` | use [`bdew_pmode_with_endpoint`] |
///
/// ## Algorithm auto-detection (asx-rs v0.7)
///
/// The WS-Security algorithm is selected automatically from the key material
/// supplied to the `asx_rs` `SessionContext`:
/// - EC key (BrainpoolP256r1) → ECDSA-SHA256 (**BDEW-compliant**)
/// - RSA key → RSA-SHA256 (*not* BDEW-compliant — use only for local testing)
///
/// The key-agreement algorithm is selected from the **recipient** certificate
/// at send time:
/// - EC certificate → ECDH-ES + ConcatKDF + AES-128-GCM (**BDEW-compliant**)
/// - RSA certificate → RSA-OAEP (*not* BDEW-compliant)
///
/// Supply BrainpoolP256r1 credentials for production. Use
/// [`crate::testing::BdewTestPki`] to generate ephemeral test keypairs.
///
/// [`X509PKIPathv1`]: WsSecOutboundKeyInfoProfile::X509PKIPathv1
/// [`MimeAttachment`]: PayloadPackagingMode::MimeAttachment
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
#[must_use]
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
            // `recipient_cert_pem` must be set in `As4SendCredentials` at send time.
            encrypt: true,
            encrypt_soap_headers: false,
            compress: false,
            // X509PKIPathv1 BST token type — mandatory per §2.2.6.2.1.
            outbound_key_info_profile: WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        },
        payload_packaging: PayloadPackagingMode::MimeAttachment,
        endpoint_url: None,
    }
}

/// Build a BDEW MaKo P-Mode with the partner's HTTPS AS4 endpoint baked in.
///
/// Identical to [`bdew_pmode`] but sets `endpoint_url` so the P-Mode is
/// self-contained for outbound delivery — no separate `PartnerDirectory`
/// lookup is required.
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
/// assert!(pm.security.sign);
/// assert!(pm.security.encrypt);
/// ```
#[must_use]
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

/// Build a sign-only BDEW MaKo P-Mode (development / testing only).
///
/// Identical to [`bdew_pmode`] but sets `security.encrypt = false`.
///
/// ## ⚠ Non-BDEW-compliant — do not use in production
///
/// BDEW AS4-Profil v1.2 §2.2.6.2.2 **requires** encryption for every production
/// message. Use this variant only in:
///
/// - Local development without WIRK certificates
/// - Tests that mock the AS4 transport entirely
/// - Pre-production bilateral test agreements where both parties opt out
///
/// For production deployments use [`bdew_pmode`] (sign + encrypt).
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
/// assert!(!pm.security.encrypt, "sign-only: not BDEW-compliant, dev/test only");
/// ```
#[must_use]
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants;

    // ── BdewAction::as_edifact_type ───────────────────────────────────────────

    #[test]
    fn as_edifact_type_all_standard() {
        assert_eq!(BdewAction::Utilmd.as_edifact_type(), "UTILMD");
        assert_eq!(BdewAction::Aperak.as_edifact_type(), "APERAK");
        assert_eq!(BdewAction::Contrl.as_edifact_type(), "CONTRL");
        assert_eq!(BdewAction::Mscons.as_edifact_type(), "MSCONS");
        assert_eq!(BdewAction::Invoic.as_edifact_type(), "INVOIC");
        assert_eq!(BdewAction::Remadv.as_edifact_type(), "REMADV");
        assert_eq!(BdewAction::Iftsta.as_edifact_type(), "IFTSTA");
        assert_eq!(BdewAction::Ordrsp.as_edifact_type(), "ORDRSP");
        assert_eq!(BdewAction::Orders.as_edifact_type(), "ORDERS");
        assert_eq!(BdewAction::Ordchg.as_edifact_type(), "ORDCHG");
        assert_eq!(BdewAction::Reqote.as_edifact_type(), "REQOTE");
        assert_eq!(BdewAction::Insrpt.as_edifact_type(), "INSRPT");
        assert_eq!(BdewAction::Pricat.as_edifact_type(), "PRICAT");
        assert_eq!(BdewAction::Quotes.as_edifact_type(), "QUOTES");
        assert_eq!(BdewAction::Partin.as_edifact_type(), "PARTIN");
        assert_eq!(BdewAction::Utilts.as_edifact_type(), "UTILTS");
    }

    // ── BdewAction::as_uri ────────────────────────────────────────────────────

    #[test]
    fn as_uri_utilmd() {
        assert_eq!(BdewAction::Utilmd.as_uri(), "urn:bdew:as4:service:UTILMD");
    }

    #[test]
    fn as_uri_aperak() {
        assert_eq!(BdewAction::Aperak.as_uri(), "urn:bdew:as4:service:APERAK");
    }

    #[test]
    fn as_uri_partin() {
        assert_eq!(BdewAction::Partin.as_uri(), "urn:bdew:as4:service:PARTIN");
    }

    #[test]
    fn as_uri_utilts() {
        assert_eq!(BdewAction::Utilts.as_uri(), "urn:bdew:as4:service:UTILTS");
    }

    #[test]
    fn as_uri_custom_passthrough() {
        let uri = "urn:custom:action:FOO";
        assert_eq!(BdewAction::Custom(uri.to_string()).as_uri(), uri);
    }

    // ── BdewAction::all_standard ──────────────────────────────────────────────

    #[test]
    fn all_standard_has_16_variants() {
        assert_eq!(BdewAction::all_standard().len(), 16);
    }

    #[test]
    fn all_standard_no_duplicates() {
        let v = BdewAction::all_standard();
        let uris: std::collections::HashSet<String> = v.iter().map(|a| a.as_uri()).collect();
        assert_eq!(
            uris.len(),
            v.len(),
            "all_standard() must not contain duplicate URIs"
        );
    }

    #[test]
    fn all_standard_contains_partin_and_utilts() {
        let v = BdewAction::all_standard();
        assert!(
            v.contains(&BdewAction::Partin),
            "all_standard must include Partin"
        );
        assert!(
            v.contains(&BdewAction::Utilts),
            "all_standard must include Utilts"
        );
    }

    // ── BdewAction::custom ────────────────────────────────────────────────────

    #[test]
    fn custom_builds_full_uri() {
        let action = BdewAction::custom("SLSFCT");
        assert_eq!(action.as_uri(), "urn:bdew:as4:service:SLSFCT");
    }

    // ── fmt::Display ──────────────────────────────────────────────────────────

    #[test]
    fn display_shows_edifact_type_name() {
        assert_eq!(BdewAction::Utilmd.to_string(), "UTILMD");
        assert_eq!(BdewAction::Partin.to_string(), "PARTIN");
        assert_eq!(BdewAction::Utilts.to_string(), "UTILTS");
    }

    // ── FromStr ───────────────────────────────────────────────────────────────

    #[test]
    fn from_str_all_standard_roundtrip() {
        for action in BdewAction::all_standard() {
            let type_name = action.as_edifact_type();
            let parsed: BdewAction = type_name.parse().unwrap();
            assert_eq!(
                parsed, action,
                "from_str({type_name}) did not round-trip through as_edifact_type"
            );
        }
    }

    #[test]
    fn from_str_partin() {
        let a: BdewAction = "PARTIN".parse().unwrap();
        assert_eq!(a, BdewAction::Partin);
    }

    #[test]
    fn from_str_utilts() {
        let a: BdewAction = "UTILTS".parse().unwrap();
        assert_eq!(a, BdewAction::Utilts);
    }

    #[test]
    fn from_str_unknown_maps_to_custom_not_error() {
        let a: BdewAction = "SLSFCT".parse().unwrap();
        assert!(
            matches!(a, BdewAction::Custom(_)),
            "Unknown type must map to Custom, not Err"
        );
        assert_eq!(a.as_uri(), "urn:bdew:as4:service:SLSFCT");
    }

    // ── bdew_action_from_str free function ────────────────────────────────────

    #[test]
    fn bdew_action_from_str_helper() {
        assert_eq!(bdew_action_from_str("UTILMD"), BdewAction::Utilmd);
        assert_eq!(bdew_action_from_str("PARTIN"), BdewAction::Partin);
        assert_eq!(bdew_action_from_str("UTILTS"), BdewAction::Utilts);
        assert!(matches!(
            bdew_action_from_str("UNKNOWN"),
            BdewAction::Custom(_)
        ));
    }

    // ── bdew_pmode factory ────────────────────────────────────────────────────

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
            "bdew_pmode_with_endpoint inherits encrypt:true from bdew_pmode"
        );
    }

    #[test]
    fn bdew_pmode_sign_only_disables_encrypt() {
        let pm = bdew_pmode_sign_only("pm-dev", "9900000000001", BdewAction::Utilmd);
        assert!(pm.security.sign);
        assert!(!pm.security.encrypt, "sign_only must have encrypt:false");
        assert_eq!(pm.mep, MepType::OneWayPush);
        assert!(pm.endpoint_url.is_none());
    }

    #[test]
    fn bdew_pmode_partin_action() {
        let pm = bdew_pmode("pm-partin", "9900000000001", BdewAction::Partin);
        assert_eq!(pm.action, "urn:bdew:as4:service:PARTIN");
    }

    #[test]
    fn bdew_pmode_utilts_action() {
        let pm = bdew_pmode("pm-utilts", "9900000000001", BdewAction::Utilts);
        assert_eq!(pm.action, "urn:bdew:as4:service:UTILTS");
    }

    // ── PModeRegistry resolution ──────────────────────────────────────────────

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

        // Different partner — not found
        assert!(
            registry
                .resolve(
                    "9900000000002",
                    constants::SERVICE,
                    &BdewAction::Utilmd.as_uri(),
                )
                .is_none()
        );
    }

    #[test]
    fn pmode_registry_for_all_standard_actions() {
        let mut registry = PModeRegistry::new();
        for action in BdewAction::all_standard() {
            let id = format!("pm-{}-partner", action);
            registry.register(bdew_pmode(id, "9900000000001", action.clone()));
        }
        // Spot-check newly added variants
        assert!(
            registry
                .resolve(
                    "9900000000001",
                    constants::SERVICE,
                    &BdewAction::Partin.as_uri()
                )
                .is_some(),
            "PARTIN P-Mode must resolve"
        );
        assert!(
            registry
                .resolve(
                    "9900000000001",
                    constants::SERVICE,
                    &BdewAction::Utilts.as_uri()
                )
                .is_some(),
            "UTILTS P-Mode must resolve"
        );
    }
}
