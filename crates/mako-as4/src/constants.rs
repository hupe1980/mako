//! BDEW MaKo AS4 values mako decides itself.
//!
//! Everything `Profile::bdew()` fixes — algorithms, token references, the
//! Action, roles, `AgreementRef`, compression — lives in `asx-rs` and is not
//! repeated here. What remains is what a deployment derives: the party type
//! of an MP-ID, the payload media type, the retry window.

// ── Party identifier types ────────────────────────────────────────────────────

/// `<eb:PartyId>/@type` for a GLN, per BDEW AS4-Profil 1.2 §2.3.1.1 (ebCore
/// ISO 6523, ICD 0088).
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

// ── Reliability ───────────────────────────────────────────────────────────────

/// How long an outbound AS4 message is retried before it is dead-lettered —
/// 72 hours.
///
/// Every retry resends the same message id and `eb:Timestamp`, so a receiver
/// that already holds the message answers it as a duplicate. `asx-rs` 0.16.2
/// accepts an `eb:Timestamp` up to 7 days old, which covers the whole window.
pub const MAX_RETRY_DURATION_SECS: u64 = 72 * 3600;

// ── Size ──────────────────────────────────────────────────────────────────────

/// The largest AS4 request an inbox reads, 64 MiB (`Profile::max_message_bytes`).
///
/// BDEW AS4-Profil 1.2 names no limit; 64 MiB is far above any Übertragungsdatei
/// and bounds what one request can make the receiver buffer.
pub const MAX_MESSAGE_BYTES: u64 = 64 << 20;

// ── Payload media type ────────────────────────────────────────────────────────

/// `<eb:PartInfo>/@MimeType` of the EDIFACT payload part.
///
/// BDEW AS4-Profil 1.2 §2.2.3.2: because compression is mandatory the payload
/// is binary in its own MIME part with Content-Type `application/octet-stream`;
/// `Profile::bdew()` refuses any other part type.
pub const PAYLOAD_MIME_TYPE: &str = "application/octet-stream";

#[cfg(test)]
mod tests {
    use super::*;

    /// §2.3.1.1 — one party type per issuing agency, quoted verbatim: a
    /// counterparty resolves the sender from these strings.
    #[test]
    fn party_types_are_the_profiles() {
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
        assert_eq!(party_id_type_for_agency("9"), PARTY_TYPE_GLN);
        assert_eq!(party_id_type_for_agency("14"), PARTY_TYPE_GLN);
        assert_eq!(party_id_type_for_agency("500"), PARTY_TYPE_GLN);
        assert_eq!(party_id_type_for_agency("ZEW"), PARTY_TYPE_BDEW);
    }

    /// §2.2.3.2 — the payload is binary in its own part, never inline XML.
    #[test]
    fn payload_media_type_is_binary() {
        assert_eq!(PAYLOAD_MIME_TYPE, "application/octet-stream");
    }
}
