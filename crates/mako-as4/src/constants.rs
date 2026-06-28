//! BDEW MaKo AS4 protocol constants.
//!
//! All values are taken from the **BDEW AS4 Kommunikationshandbuch** (mandatory
//! for electricity since 1 April 2024, for gas since 1 April 2025).

// ── Party identification ──────────────────────────────────────────────────────

/// BDEW party ID type: GLN (Global Location Number, ISO 6523 ICD 0088).
///
/// All BDEW market participants are identified by their 13-digit GLN.
/// Used as the `type` attribute on `<eb:PartyId>` elements.
pub const PARTY_ID_TYPE: &str = "urn:oasis:names:tc:ebcore:partyid-type:iso6523:0088";

// ── Service / action ─────────────────────────────────────────────────────────

/// BDEW MaKo AS4 service identifier used in `<eb:Service>`.
///
/// Identifies the BDEW market communication business service in the ebMS3
/// `<eb:CollaborationInfo>` element.
pub const SERVICE: &str = "urn:bdew:as4:service";

/// `type` attribute on `<eb:Service>` — empty string omits the attribute.
pub const SERVICE_TYPE: &str = "";

/// BDEW MaKo AS4 agreement reference name (`<eb:AgreementRef>`).
pub const AGREEMENT_REF: &str = "urn:bdew:as4:agreement";

/// `type` attribute on `<eb:AgreementRef>`.
pub const AGREEMENT_TYPE: &str = "bdew:as4";

// ── MPC ───────────────────────────────────────────────────────────────────────

/// ebMS3 default Message Partition Channel.
///
/// BDEW uses the standard default MPC; no custom partitioning is required.
pub const DEFAULT_MPC: &str =
    "http://docs.oasis-open.org/ebxml-msg/ebms/v3.0/ns/core/200704/defaultMPC";

// ── WS-Security algorithms ────────────────────────────────────────────────────

/// RSA-SHA256 signature algorithm (mandatory, BDEW AS4 Kommunikationshandbuch §5.5).
pub const SIG_ALGO_RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";

/// SHA-256 digest algorithm (mandatory for all signed content).
pub const DIGEST_SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";

/// Exclusive C14N canonicalization algorithm (without comments).
///
/// Required by BDEW for WS-Security XMLDSig (BDEW AS4 Kommunikationshandbuch §5.5).
pub const C14N_EXCLUSIVE: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";

// ── Optional encryption algorithms ────────────────────────────────────────────

/// RSA-OAEP key transport algorithm for AS4 payload encryption (optional).
///
/// Used to wrap the symmetric content-encryption key.
pub const ENC_KEY_TRANSPORT_RSA_OAEP: &str = "http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p";

/// AES-128-CBC content encryption algorithm (optional, BDEW AS4 Kommunikationshandbuch §5.6).
pub const ENC_CONTENT_AES128_CBC: &str = "http://www.w3.org/2001/04/xmlenc#aes128-cbc";

/// AES-256-GCM content encryption algorithm (alternative optional cipher).
pub const ENC_CONTENT_AES256_GCM: &str = "http://www.w3.org/2009/xmlenc11#aes256-gcm";

// ── Reliability ───────────────────────────────────────────────────────────────

/// Maximum retry duration in seconds — 72 hours (BDEW AS4 Kommunikationshandbuch).
///
/// AS4 senders MUST retry unacknowledged messages for up to 72 hours before
/// permanently failing delivery.  This window also defines the deduplication
/// TTL: an [`asx_rs`] `TtlDedupStorage` should be configured with at least
/// this TTL (96 hours recommended for safety margin).
pub const MAX_RETRY_DURATION_SECS: u64 = 72 * 3600;

/// Maximum number of delivery attempts (BDEW AS4 Kommunikationshandbuch).
pub const MAX_RETRY_COUNT: u32 = 5;

/// Timestamp freshness window in seconds.
///
/// Per eDelivery AS4 v1.15 §5.1.3, inbound `<eb:Timestamp>` values outside
/// ±5 minutes of the current time MUST be rejected.
pub const TIMESTAMP_FRESHNESS_WINDOW_SECS: u64 = 300;

// ── EDIFACT content type ──────────────────────────────────────────────────────

/// MIME content type for EDIFACT UN/EDIFACT payloads in AS4 attachments.
pub const EDIFACT_CONTENT_TYPE: &str = "application/edifact";
