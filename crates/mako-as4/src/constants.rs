//! BDEW MaKo AS4 protocol constants.
//!
//! All values are taken from the **BDEW AS4 Kommunikationshandbuch** (mandatory
//! for electricity since 1 April 2024, for gas since 1 April 2025).

// ── Service / action ─────────────────────────────────────────────────────────

/// BDEW MaKo AS4 service identifier used in `<eb:Service>`.
///
/// Identifies the BDEW market communication business service in the ebMS3
/// `<eb:CollaborationInfo>` element.
pub const SERVICE: &str = "urn:bdew:as4:service";

/// `type` attribute on `<eb:Service>` — empty string omits the attribute.
pub const SERVICE_TYPE: &str = "";

/// BDEW MaKo AS4 agreement reference (`<eb:AgreementRef>`).
///
/// Fixed value per BDEW AS4-Profil v1.2 §2.3.2, signalling that the profile's
/// dynamic sender/receiver model is in use. The `pmode` and `type` attributes
/// **must not** be emitted alongside it — pass `None` for the type.
pub const AGREEMENT_REF: &str = "https://www.bdew.de/as4/communication/agreement";

// ── Roles ─────────────────────────────────────────────────────────────────────

/// `<eb:From>/<eb:Role>` — fixed `PMode.Initiator.Role` per §2.3, Tabelle 1.
pub const ROLE_INITIATOR: &str =
    "http://docs.oasis-open.org/ebxml-msg/ebms/v3.0/ns/core/200704/initiator";

/// `<eb:To>/<eb:Role>` — fixed `PMode.Responder.Role` per §2.3, Tabelle 1.
pub const ROLE_RESPONDER: &str =
    "http://docs.oasis-open.org/ebxml-msg/ebms/v3.0/ns/core/200704/responder";

// ── Party identifier types ────────────────────────────────────────────────────

/// `<eb:PartyId>/@type` for a GLN, per §2.3.1.1 (ebCore ISO 6523, ICD 0088).
pub const PARTY_TYPE_GLN: &str = "urn:oasis:names:tc:ebcore:partyid-type:iso6523:0088";

/// `<eb:PartyId>/@type` for a BDEW-assigned MP-ID, per §2.3.1.1.
pub const PARTY_TYPE_BDEW: &str = "urn:oasis:names:tc:ebcore:partyid-type:unregistered:BDEW";

/// `<eb:PartyId>/@type` for a DVGW-assigned MP-ID, per §2.3.1.1.
pub const PARTY_TYPE_DVGW: &str = "urn:oasis:names:tc:ebcore:partyid-type:unregistered:DVGW";

/// `<eb:PartyId>/@type` for a DB (Bahnstromnetz) MP-ID, per §2.3.1.1.
pub const PARTY_TYPE_BAHN: &str = "urn:oasis:names:tc:ebcore:partyid-type:unregistered:BAHN";

/// Map a NAD DE3055 agency code to its ebCore `<eb:PartyId>/@type`.
///
/// §2.3.1.1 requires the attribute and derives it from the agency that issued
/// the MP-ID, so the two identifier vocabularies — EDIFACT's numeric agency
/// code and AS4's ebCore URI — must not drift apart. Unknown codes fall back
/// to the BDEW scheme, matching the registry's own default agency.
#[must_use]
pub const fn party_id_type_for_agency(agency: &str) -> &'static str {
    match agency.as_bytes() {
        b"9" | b"500" | b"14" => PARTY_TYPE_GLN,
        b"332" | b"502" => PARTY_TYPE_DVGW,
        _ => PARTY_TYPE_BDEW,
    }
}

// ── MPC ───────────────────────────────────────────────────────────────────────

/// ebMS3 default Message Partition Channel.
///
/// BDEW uses the standard default MPC; no custom partitioning is required.
pub const DEFAULT_MPC: &str =
    "http://docs.oasis-open.org/ebxml-msg/ebms/v3.0/ns/core/200704/defaultMPC";

// ── WS-Security signing algorithms ───────────────────────────────────────────

/// ECDSA-SHA256 signature algorithm.
///
/// Mandatory per BDEW AS4-Profil v1.2 §2.2.6.2.1 and BSI TR-03116-3 §9.1.
/// Use with a BrainpoolP256r1 EC signing key; the algorithm is auto-detected
/// from the key type — no explicit configuration needed.
pub const SIG_ALGO_ECDSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256";

/// SHA-256 digest algorithm (mandatory for all signed content).
pub const DIGEST_SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";

/// Exclusive C14N canonicalization algorithm (without comments).
///
/// Required by BDEW for WS-Security XMLDSig (BDEW AS4 Kommunikationshandbuch §5.5).
pub const C14N_EXCLUSIVE: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";

// ── XML Encryption algorithms ─────────────────────────────────────────────────

/// ECDH-ES key agreement algorithm.
///
/// Mandatory per BDEW AS4-Profil v1.2 §2.2.6.2.2 and BSI TR-03116-3 §9.2.
/// Automatically selected when the recipient certificate carries an EC public key.
pub const ENC_KEY_AGREEMENT_ECDH_ES: &str = "http://www.w3.org/2009/xmlenc11#ECDH-ES";

/// ConcatKDF key derivation algorithm (NIST SP 800-56A §5.8.1).
///
/// Used inside ECDH-ES key agreement to derive the key-encryption key (KEK).
pub const ENC_KEY_DERIVATION_CONCAT_KDF: &str = "http://www.w3.org/2009/xmlenc11#ConcatKDF";

/// AES-128 Key Wrap algorithm (RFC 3394).
///
/// Mandatory per BDEW AS4-Profil v1.2 §2.2.6.2.2: wraps the CEK with the
/// ECDH-ES-derived KEK.
pub const ENC_KEY_WRAP_AES128: &str = "http://www.w3.org/2001/04/xmlenc#kw-aes128";

/// AES-128-GCM content encryption algorithm.
///
/// Mandatory per BDEW AS4-Profil v1.2 §2.2.6.2.2.
pub const ENC_CONTENT_AES128_GCM: &str = "http://www.w3.org/2009/xmlenc11#aes128-gcm";

/// AES-256-GCM content encryption algorithm (alternative, not mandated by BDEW v1.2).
pub const ENC_CONTENT_AES256_GCM: &str = "http://www.w3.org/2009/xmlenc11#aes256-gcm";

// ── Reliability ───────────────────────────────────────────────────────────────

/// Maximum retry duration in seconds — 72 hours (BDEW AS4 Kommunikationshandbuch).
///
/// AS4 senders MUST retry unacknowledged messages for up to 72 hours before
/// permanently failing delivery.  This window also defines the deduplication
/// TTL: an [`asx_rs`] `TtlDedupStorage` should be configured with at least
/// this TTL (96 hours recommended for safety margin).
pub const MAX_RETRY_DURATION_SECS: u64 = 72 * 3600;

/// Timestamp freshness window in seconds.
///
/// Per eDelivery AS4 v1.15 §5.1.3, inbound `<eb:Timestamp>` values outside
/// ±5 minutes of the current time MUST be rejected.
pub const TIMESTAMP_FRESHNESS_WINDOW_SECS: u64 = 300;

// ── Payload media type ────────────────────────────────────────────────────────

/// `<eb:PartInfo>/@MimeType` for the EDIFACT payload part.
///
/// BDEW AS4-Profil v1.2 §2.2.3.2: because compression is mandatory in this
/// profile the payload is carried as binary in its own MIME part with
/// Content-Type `application/octet-stream`, never in the SOAP Body — which
/// this profile requires to be empty.
///
/// This must be stated explicitly on every send. `asx-rs` defaults an unset
/// media type to `application/xml` **whenever encryption is on**, and BDEW
/// encrypts unconditionally (§2.2.6.2.2), so leaving it unset would label an
/// EDIFACT interchange as XML on the wire.
pub const PAYLOAD_MIME_TYPE: &str = "application/octet-stream";

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The ebMS3 envelope vocabulary is quoted verbatim from BDEW AS4-Profil
    /// v1.2. A counterparty resolves its P-Mode from these strings, so a typo
    /// is a delivery failure the sender cannot see.
    #[test]
    fn envelope_vocabulary_matches_the_profile() {
        // §2.3.2 — Festwert; the pmode/type attributes must not accompany it.
        assert_eq!(
            AGREEMENT_REF,
            "https://www.bdew.de/as4/communication/agreement"
        );
        // §2.3 Tabelle 1 — PMode.Initiator.Role / PMode.Responder.Role.
        assert_eq!(
            ROLE_INITIATOR,
            "http://docs.oasis-open.org/ebxml-msg/ebms/v3.0/ns/core/200704/initiator"
        );
        assert_eq!(
            ROLE_RESPONDER,
            "http://docs.oasis-open.org/ebxml-msg/ebms/v3.0/ns/core/200704/responder"
        );
        // §2.3.1.1 — one party type per issuing agency.
        assert_eq!(
            PARTY_TYPE_GLN,
            "urn:oasis:names:tc:ebcore:partyid-type:iso6523:0088"
        );
        assert_eq!(
            PARTY_TYPE_BDEW,
            "urn:oasis:names:tc:ebcore:partyid-type:unregistered:BDEW"
        );
        assert_eq!(
            PARTY_TYPE_DVGW,
            "urn:oasis:names:tc:ebcore:partyid-type:unregistered:DVGW"
        );
        assert_eq!(
            PARTY_TYPE_BAHN,
            "urn:oasis:names:tc:ebcore:partyid-type:unregistered:BAHN"
        );
    }

    /// The AS4 party type and the EDIFACT NAD DE3055 agency describe the same
    /// fact in two vocabularies; they must not disagree for one MP-ID.
    #[test]
    fn party_type_follows_the_issuing_agency() {
        assert_eq!(party_id_type_for_agency("293"), PARTY_TYPE_BDEW);
        assert_eq!(party_id_type_for_agency("332"), PARTY_TYPE_DVGW);
        assert_eq!(party_id_type_for_agency("502"), PARTY_TYPE_DVGW);
        // GS1 appears as "9" (NAD DE3055) and "14"/"500" in the UNB vocabulary.
        assert_eq!(party_id_type_for_agency("9"), PARTY_TYPE_GLN);
        assert_eq!(party_id_type_for_agency("14"), PARTY_TYPE_GLN);
        assert_eq!(party_id_type_for_agency("500"), PARTY_TYPE_GLN);
        // Unknown agencies fall back to BDEW, matching the registry default.
        assert_eq!(party_id_type_for_agency("ZEW"), PARTY_TYPE_BDEW);
    }

    /// §2.2.3.2 — the payload is binary in its own part, never inline XML.
    #[test]
    fn payload_media_type_is_binary() {
        assert_eq!(PAYLOAD_MIME_TYPE, "application/octet-stream");
    }
}
