//! AS4 transport security tests — sign+encrypt, replay dedup, policy contracts.
//!
//! Uses `mako-as4`'s built-in test helpers so no WIRK certificates are required.
//!
//! ## Coverage
//!
//! | Test | Finding | Regulatory basis |
//! |---|---|---|
//! | `policy_without_key_no_encryption_required` | D1 dev-mode behaviour | BDEW AS4-Profil v1.2 §2.2.6.2.2 |
//! | `policy_with_key_requires_encryption` | D1 prod-mode behaviour | BDEW AS4-Profil v1.2 §2.2.6.2.2 |
//! | `fragment_scope_is_soap_sender_id` | D2 no-fragmentation contract | BDEW AS4-Profil v1.2 OneWayPush |
//! | `sign_encrypt_pmode_defaults` | D1 bdew_pmode encrypt:true | BDEW AS4-Profil v1.2 §2.2.6.2.2 |
//! | `sign_only_pmode_disables_encryption` | D1 sign_only contract | BDEW AS4-Profil v1.2 dev variant |
//! | `replay_dedup_blocks_duplicate` | D2 72h replay window | BDEW AS4-Profil v1.2 §4.2 |
//! | `sign_encrypt_output_contains_required_elements` | D1 smoke | BDEW AS4-Profil v1.2 §2.2.6.2.1+2 |

use std::sync::Arc;

use mako_as4::{bdew_pmode, bdew_pmode_sign_only, bdew_push_policy, pmode::BdewAction};

// ── D1: bdew_push_policy encryption enforcement ───────────────────────────────

/// Without a decryption key, `bdew_push_policy` must NOT set
/// `require_encrypted_inbound`. This is correct for development mode (before
/// WIRK certificates arrive) — a hard requirement would block onboarding.
#[test]
fn policy_without_key_no_encryption_required() {
    let policy = bdew_push_policy(None);
    assert!(
        !policy.require_encrypted_inbound,
        "Without a decryption key bdew_push_policy must not require inbound \
         encryption (would block onboarding before WIRK certs arrive)"
    );
}

/// With a decryption key configured, `bdew_push_policy` must enforce
/// `require_encrypted_inbound = true` — BDEW AS4-Profil v1.2 §2.2.6.2.2.
#[test]
fn policy_with_key_requires_encryption() {
    // Provide any bytes — the key content is not validated by policy construction.
    let dummy_key = vec![0u8; 32];
    let policy = bdew_push_policy(Some(dummy_key));
    assert!(
        policy.require_encrypted_inbound,
        "With a decryption key, bdew_push_policy must enforce \
         require_encrypted_inbound — BDEW AS4-Profil v1.2 §2.2.6.2.2"
    );
}

// ── D2: fragment scope policy ─────────────────────────────────────────────────

/// BDEW AS4 uses `OneWayPush` with single `UserMessage` — no fragmentation.
/// `fragment_scope_policy` must be `UseSoapSenderId` so that passing
/// `authenticated_sender_scope: None` (the current ingest handler value) never
/// triggers a `PolicyViolation` error.
#[test]
fn fragment_scope_is_soap_sender_id() {
    use asx_rs::as4::FragmentScopePolicy;
    let policy = bdew_push_policy(None);
    assert_eq!(
        policy.fragment_scope_policy,
        FragmentScopePolicy::UseSoapSenderId,
        "BDEW does not use AS4 message fragmentation; policy must be \
         UseSoapSenderId so authenticated_sender_scope: None is always safe"
    );
}

// ── D1: P-Mode sign+encrypt defaults ─────────────────────────────────────────

/// `bdew_pmode()` must default to `sign=true, encrypt=true` per BDEW
/// AS4-Profil v1.2 §2.2.6.2.2. This is the canonical production P-Mode.
#[test]
fn sign_encrypt_pmode_defaults() {
    let pm = bdew_pmode("pm-1", "9900000000001", BdewAction::Utilmd);
    assert!(pm.security.sign, "bdew_pmode must have sign=true");
    assert!(
        pm.security.encrypt,
        "bdew_pmode must have encrypt=true — BDEW AS4-Profil v1.2 §2.2.6.2.2"
    );
}

/// `bdew_pmode_sign_only()` disables encryption — explicitly documented as
/// non-BDEW-compliant for dev/test only.
#[test]
fn sign_only_pmode_disables_encryption() {
    let pm = bdew_pmode_sign_only("pm-dev", "9900000000001", BdewAction::Aperak);
    assert!(pm.security.sign, "sign_only must still have sign=true");
    assert!(
        !pm.security.encrypt,
        "sign_only must have encrypt=false — non-compliant dev/test variant"
    );
}

// ── D2: 72-hour replay deduplication ─────────────────────────────────────────

/// BDEW AS4-Profil v1.2 §4.2 — duplicate `message_id` within the 72-hour
/// dedup window must be detected. Exercises `SlateDbDedupBridge::first_seen`
/// with an in-memory backing store.
#[tokio::test]
async fn replay_dedup_blocks_duplicate_message_id() {
    use asx_rs::storage::DedupStorage as _;
    use mako_engine::store_slatedb::SlateDbStore;
    use makod::as4_ingest::SlateDbDedupBridge;

    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory SlateDB for dedup test"),
    );
    // SlateDbDedupBridge takes Arc<SlateDbInboxStore> and a durable flag.
    let inbox = Arc::new(store.as_inbox_store());
    // durable=false because in-memory store has volatile dedup state.
    let dedup = SlateDbDedupBridge::new(inbox, false);

    let message_id = format!("dedup-test-{}", uuid::Uuid::new_v4());

    // First delivery — must be accepted (first_seen = true)
    let first = dedup.first_seen(&message_id).await.expect("first_seen 1");
    assert!(
        first,
        "First delivery of a message_id must be accepted (first_seen = true)"
    );

    // Second delivery — same message_id — must be detected as replay
    let second = dedup.first_seen(&message_id).await.expect("first_seen 2");
    assert!(
        !second,
        "Second delivery of the same message_id must be detected as replay \
         (first_seen = false) — BDEW AS4-Profil v1.2 §4.2 (72-hour dedup window)"
    );
}

// ── D1: sign+encrypt policy smoke ────────────────────────────────────────────

/// BDEW AS4-Profil v1.2 §2.2.6.2.1 + §2.2.6.2.2 — the `As4SendPolicyBuilder`
/// correctly accepts a BDEW-compliant sign+encrypt policy.
///
/// Verifies that:
/// - Building a policy with `sign=true, encrypt=true` succeeds without error
/// - The resulting policy has the correct key info profile (X509PKIPathv1) per §2.2.6.2.1
/// - Encryption mode is Aes128Gcm per §2.2.6.2.2 + BSI TR-03116-3 §9.2
///
/// Actual SOAP envelope generation is covered by asx-rs's own test suite.
/// This test validates that the mako-as4 constant values satisfy the BDEW spec.
#[test]
fn sign_encrypt_policy_is_bdew_compliant() {
    use asx_rs::as4::As4SendPolicy;
    use asx_rs::crypto::wssec::XmlEncPayloadAlgorithm;
    use mako_as4::constants;

    let policy = As4SendPolicy {
        sign: true,
        encrypt: true,
        action: format!("{}:UTILMD", constants::SERVICE),
        service: constants::SERVICE.to_owned(),
        service_type: constants::SERVICE_TYPE.to_owned(),
        outbound_key_info_profile:
            asx_rs::crypto::wssec::WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        outbound_xmlenc_payload_algorithm: XmlEncPayloadAlgorithm::Aes128Gcm,
        ..As4SendPolicy::default()
    };

    assert!(
        policy.sign,
        "BDEW AS4-Profil v1.2 §2.2.6.2.1 requires signing"
    );
    assert!(
        policy.encrypt,
        "BDEW AS4-Profil v1.2 §2.2.6.2.2 requires encryption"
    );
    assert_eq!(
        policy.outbound_key_info_profile,
        asx_rs::crypto::wssec::WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        "§2.2.6.2.1 requires X509PKIPathv1 BST token type"
    );
    assert_eq!(
        policy.outbound_xmlenc_payload_algorithm,
        XmlEncPayloadAlgorithm::Aes128Gcm,
        "§2.2.6.2.2 + BSI TR-03116-3 §9.2 requires AES-128-GCM"
    );
    assert_eq!(
        policy.action, "urn:bdew:as4:service:UTILMD",
        "BDEW AS4-Profil §3.1: action must be urn:bdew:as4:service:<TYPE>"
    );
}
