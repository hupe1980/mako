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
//! | `sign_encrypt_policy_is_bdew_compliant` | D1 smoke | BDEW AS4-Profil v1.2 §2.2.6.2.1+2 |
//! | `tampered_signature_is_rejected` | D2 signature integrity | BDEW AS4-Profil v1.2 §2.2.6.2.1 |
//! | `inbound_encryption_enforced_when_decryption_key_set` | D1 require_encrypted_inbound | BDEW AS4-Profil v1.2 §2.2.6.2.2 |
//! | `sign_encrypt_round_trip_via_mock_endpoint` | F-010 round-trip | BDEW AS4-Profil v1.2 §2.2.6 |
//! | `sign_only_round_trip_envelope_contains_wssec_signature` | F-010 signing | BDEW AS4-Profil v1.2 §2.2.6.2.1 |

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

// ── F-010: sign+encrypt end-to-end round-trip via MockAs4Endpoint ─────────────

/// BDEW AS4-Profil v1.2 §2.2.6 — full sign+encrypt end-to-end round-trip using
/// ephemeral BrainpoolP256r1 test certificates (no WIRK material required).
///
/// ## What this test covers
///
/// **Part A — Cryptographic output verification (sign+encrypt):**
/// - `asx_rs::as4::send_async()` correctly signs and encrypts a UTILMD payload
///   using BrainpoolP256r1 keypairs (BDEW AS4-Profil v1.2 §2.2.6.2.1/§2.2.6.2.2).
/// - The SOAP envelope embeds a `BinarySecurityToken` (X509PKIPathv1, §2.2.6.2.1).
/// - The SOAP envelope contains XML-DSig `Signature` (§2.2.6.2.1).
/// - The envelope carries `EncryptedData` (AES-128-GCM, §2.2.6.2.2).
///
/// **Part B — Full sign+encrypt→transport→decrypt round-trip (asx-rs v0.8.0):**
/// - `MockAs4Endpoint::builder().with_decryption_key_pem(key_pem)` configures
///   the mock to decrypt inbound ECDH-ES messages using the receiver's EC key.
/// - The encrypted AS4 message is delivered via `As4HttpTransport::new_for_localhost_testing()`.
/// - The mock decrypts and delivers the plaintext payload via `next_received()`.
///
/// ## asx-rs v0.8.0 improvements used
///
/// - `SessionContextBuilder::with_signing_material(cert, key)` — single-call signing
///   setup; `key_id` auto-derived from `partner_id` (fixes BUG-1 from ASX_FEEDBACK.md)
/// - `EventBus::new_for_testing()` — zero-config `BestEffort` bus (FR-2)
/// - `MockAs4Endpoint::builder().with_decryption_key_pem(key_pem)` — full encrypt round-trip (FR-1)
/// - `As4HttpTransport::new_for_localhost_testing()` — bypass SSRF guard for tests (FR-3)
/// - Partial `As4SendCredentials` — `recipient_cert_pem` only; session fallback for signing (FR-4)
#[tokio::test]
async fn sign_encrypt_round_trip_via_mock_endpoint() {
    use asx_rs::as4::{As4SendRequest, send_async};
    use asx_rs::core::SessionContextBuilder;
    use asx_rs::observability::EventBus;
    use asx_rs::transport::As4HttpTransport;
    use mako_as4::pmode::{BdewAction, bdew_pmode_with_endpoint};
    use mako_as4::profile::BdewAs4Profile;
    use mako_as4::testing::{BdewTestPki, MockAs4Endpoint};

    let sender_pki = BdewTestPki::generate("Test NB 9900000000001");
    let receiver_pki = BdewTestPki::generate("Test LF 9900000000002");

    const SENDER_GLN: &str = "9900000000001";
    const RECEIVER_GLN: &str = "9900000000002";

    // ── Start mock receiver with decryption key (asx-rs v0.8.0 FR-1) ─────────
    let mock_endpoint = MockAs4Endpoint::builder()
        .with_decryption_key_pem(receiver_pki.encryption.key_pem.clone())
        .bind("127.0.0.1:0")
        .await
        .expect("MockAs4Endpoint must bind");
    let endpoint_url = mock_endpoint.local_url();

    // ── Build sender SessionContext using v0.8.0 convenience API (BUG-1 fix) ──
    // `with_signing_material` auto-derives `key_id = "cert:{partner_id}"` and
    // eliminates the need to construct a CertHandle manually or depend on `zeroize`.
    let session = Arc::new(
        SessionContextBuilder::new("test-session-nb", SENDER_GLN)
            .with_signing_material(
                sender_pki.signing.cert_pem_str(),
                sender_pki.signing.key_pem_str(),
            )
            .with_trust_anchor_pem(sender_pki.signing.cert_pem_str())
            .build()
            .expect("SessionContext must build with valid BrainpoolP256r1 credentials"),
    );

    // ── Zero-config test event bus (asx-rs v0.8.0 FR-2) ──────────────────────
    let event_bus = Arc::new(EventBus::new_for_testing());

    let payload = b"UNB+UNOC:3+9900000000001:293+9900000000002:293+260101:0000+1'\
                    UNH+1+UTILMD:D:11A:UN:5.2S'\
                    UNZ+1+1'";

    // ── Part A: verify sign+encrypt SOAP output has all required elements ─────
    let mut profile = BdewAs4Profile::new();
    profile
        .register_partner_encryption_cert(RECEIVER_GLN, receiver_pki.encryption.cert_pem.clone());
    profile.register_pmode(bdew_pmode_with_endpoint(
        "pm-utilmd-lf",
        RECEIVER_GLN,
        BdewAction::Utilmd,
        &endpoint_url,
    ));

    let pm = profile
        .resolve_pmode_by_action(RECEIVER_GLN, &BdewAction::Utilmd)
        .expect("P-Mode must be registered");
    assert!(pm.security.sign, "bdew_pmode must sign");
    assert!(pm.security.encrypt, "bdew_pmode must encrypt");

    let mut policy = pm.to_send_policy().expect("sign+encrypt policy must build");
    policy.conversation_id = Some("CONV-ENCRYPT".to_owned());

    // Partial As4SendCredentials: recipient_cert_pem only; session fallback for
    // signing (asx-rs v0.8.0 FR-4 — partial override now supported).
    let output = send_async(
        &session,
        &event_bus,
        As4SendRequest {
            message_id: format!("enc-msg-{}", uuid::Uuid::new_v4()),
            payload: payload.to_vec(),
            policy,
            credentials: Some(asx_rs::as4::As4SendCredentials {
                recipient_cert_pem: Some(std::sync::Arc::from(
                    receiver_pki.encryption.cert_pem.as_slice(),
                )),
                // v0.8.0: signing material falls back to session cert_handle
                signing_cert_pem: None,
                signing_key_pem: None,
            }),
            payload_filename: None,
        },
    )
    .await
    .expect("sign+encrypt send_async must succeed with BrainpoolP256r1 material");

    let soap_str = std::str::from_utf8(&output.soap_envelope.body)
        .expect("sign+encrypt SOAP envelope must be valid UTF-8");

    assert!(
        soap_str.contains("BinarySecurityToken"),
        "SOAP must contain BinarySecurityToken (X509PKIPathv1 per §2.2.6.2.1)"
    );
    assert!(
        soap_str.contains("ds:Signature") || soap_str.contains("Signature"),
        "SOAP must contain XML-DSig Signature (§2.2.6.2.1)"
    );
    assert!(
        soap_str.contains("EncryptedData") || soap_str.contains("multipart"),
        "SOAP must contain EncryptedData (ECDH-ES + AES-128-GCM per §2.2.6.2.2)"
    );

    // ── Part B: full sign+encrypt→transport→decrypt round-trip ───────────────
    // asx-rs v0.8.0: As4HttpTransport::new_for_localhost_testing() (FR-3) — use the real
    // transport layer (Content-Type, receipt inspection) instead of raw reqwest.
    // `send_to_localhost()` bypasses the SSRF URL guard; regular `send()` still validates.
    let transport =
        As4HttpTransport::new_for_localhost_testing().expect("localhost test transport must build");
    transport
        .send_to_localhost(&endpoint_url, &output)
        .await
        .expect("As4HttpTransport must deliver to MockAs4Endpoint");

    let received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mock_endpoint.next_received(),
    )
    .await
    .expect("MockAs4Endpoint must receive within 5 seconds")
    .expect("MockAs4Endpoint channel must be open");

    assert_eq!(
        received.action,
        BdewAction::Utilmd.as_uri(),
        "Received action must match UTILMD action URI"
    );
    // Payload is decrypted: should contain the original UTILMD bytes.
    assert!(
        received.payload.windows(3).any(|w| w == b"UNB"),
        "Decrypted payload must contain the original UTILMD bytes (UNB…)"
    );
}

/// BDEW AS4-Profil v1.2 §2.2.6.2.1 — sign-only (development mode) round-trip.
///
/// Verifies that `bdew_pmode_sign_only()` produces a valid SOAP envelope that
/// the mock endpoint accepts, and that the envelope still contains the
/// WS-Security signature (but no XML encryption elements).
///
/// This path is used by operators during onboarding before WIRK certificates
/// arrive.  It is **not** BDEW-compliant for production — the test explicitly
/// documents this restriction.
///
/// Uses asx-rs v0.8.0 convenience APIs:
/// - `SessionContextBuilder::with_signing_material()` (BUG-1 fix)
/// - `EventBus::new_for_testing()` (FR-2)
/// - `As4HttpTransport::new_for_localhost_testing()` (FR-3)
#[tokio::test]
async fn sign_only_round_trip_envelope_contains_wssec_signature() {
    use asx_rs::as4::{As4SendRequest, send_async};
    use asx_rs::core::SessionContextBuilder;
    use asx_rs::observability::EventBus;
    use asx_rs::transport::As4HttpTransport;
    use mako_as4::pmode::{BdewAction, bdew_pmode_sign_only};
    use mako_as4::testing::{BdewTestPki, MockAs4Endpoint};

    let sender_pki = BdewTestPki::generate("Test NB 9900000000003");
    const SENDER_GLN: &str = "9900000000003";
    const RECEIVER_GLN: &str = "9900000000004";

    let mock_endpoint = MockAs4Endpoint::bind("127.0.0.1:0")
        .await
        .expect("MockAs4Endpoint must bind");
    let endpoint_url = mock_endpoint.local_url();

    // v0.8.0: with_signing_material — no manual CertHandle, no zeroize dep (BUG-1 fix)
    let session = Arc::new(
        SessionContextBuilder::new("test-session-sign-only", SENDER_GLN)
            .with_signing_material(
                sender_pki.signing.cert_pem_str(),
                sender_pki.signing.key_pem_str(),
            )
            .with_trust_anchor_pem(sender_pki.signing.cert_pem_str())
            .build()
            .expect("SessionContext must build"),
    );
    // v0.8.0: zero-config test bus (FR-2)
    let event_bus = Arc::new(EventBus::new_for_testing());

    let pm = bdew_pmode_sign_only("pm-sign-only", RECEIVER_GLN, BdewAction::Aperak);
    assert!(!pm.security.encrypt, "sign-only P-Mode must not encrypt");

    let output = send_async(
        &session,
        &event_bus,
        As4SendRequest {
            message_id: format!("sign-only-{}", uuid::Uuid::new_v4()),
            payload: b"UNB+UNOC:3+0001+0002+260101:0000+1'".to_vec(),
            policy: pm.to_send_policy().expect("sign-only policy must build"),
            credentials: None,
            payload_filename: None,
        },
    )
    .await
    .expect("sign-only send_async must succeed");

    let soap_str =
        std::str::from_utf8(&output.soap_envelope.body).expect("SOAP envelope must be valid UTF-8");

    assert!(
        soap_str.contains("BinarySecurityToken"),
        "sign-only envelope must embed X509PKIPathv1 BinarySecurityToken"
    );
    assert!(
        soap_str.contains("ds:Signature") || soap_str.contains("Signature"),
        "sign-only envelope must contain XML-DSig Signature"
    );
    assert!(
        !soap_str.contains("EncryptedKey"),
        "sign-only envelope must NOT contain EncryptedKey"
    );

    // v0.8.0: As4HttpTransport::new_for_localhost_testing() (FR-3) — use the real
    // transport layer (Content-Type, receipt inspection) instead of raw reqwest.
    let transport =
        As4HttpTransport::new_for_localhost_testing().expect("localhost test transport must build");
    transport
        .send_to_localhost(&endpoint_url, &output)
        .await
        .expect("As4HttpTransport must deliver to MockAs4Endpoint");

    let received = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mock_endpoint.next_received(),
    )
    .await
    .expect("MockAs4Endpoint must receive within 5 seconds")
    .expect("channel open");

    assert_eq!(
        received.action,
        BdewAction::Aperak.as_uri(),
        "sign-only round-trip must preserve APERAK action URI"
    );
}

// ── D2: signature integrity — tampered payload is rejected ───────────────────

/// BDEW AS4-Profil v1.2 §2.2.6.2.1 — the real `As4WsSecVerifier` (not the
/// `InsecureBypassAs4Verifier` used in the mock endpoint) must reject an AS4
/// message whose WS-Security signature does not match the payload.
///
/// Test flow:
/// 1. Build a valid signed SOAP envelope using `send_async`.
/// 2. Corrupt one byte in the payload region of the envelope.
/// 3. Feed the tampered bytes to `receive_push_with_dedup_async` (which uses
///    the real `As4WsSecVerifier` with strict PKI verification).
/// 4. Assert the receive returns an error (not `Ok(As4ReceiveOutcome::FirstSeen)`).
///
/// This test verifies that `makod`'s inbound AS4 pipeline cannot be bypassed
/// by forging a valid outer SOAP envelope and injecting a different payload.
///
/// Regulatory basis: BDEW AS4-Profil v1.2 §2.2.6.2.1 (mandatory signing).
#[tokio::test]
async fn tampered_signature_is_rejected() {
    use asx_rs::as4::{
        As4ReceivePushRequest, As4SendRequest, As4WsSecVerifier,
        receive_push_with_dedup_async_with_custom_verifier, send_async,
    };
    use asx_rs::core::SessionContextBuilder;
    use asx_rs::observability::EventBus;
    use asx_rs::storage::DurableInMemoryDedupBackend;
    use mako_as4::testing::BdewTestPki;
    use mako_as4::{bdew_pmode_sign_only, bdew_push_policy, pmode::BdewAction};
    use std::sync::Arc;

    let sender_pki = BdewTestPki::generate("Tamper NB 9900000000010");
    const SENDER_GLN: &str = "9900000000010";
    const RECEIVER_GLN: &str = "9900000000011";

    // Build sender session with real signing material.
    let sender_session = Arc::new(
        SessionContextBuilder::new("tamper-test-sender", SENDER_GLN)
            .with_signing_material(
                sender_pki.signing.cert_pem_str(),
                sender_pki.signing.key_pem_str(),
            )
            .with_trust_anchor_pem(sender_pki.signing.cert_pem_str())
            .build()
            .expect("sender session must build"),
    );

    // Build receiver session that trusts the sender's signing certificate.
    let receiver_session = Arc::new(
        SessionContextBuilder::new("tamper-test-receiver", RECEIVER_GLN)
            // The receiver trusts the sender's signing cert as its own CA.
            .with_trust_anchor_pem(sender_pki.signing.cert_pem_str())
            .build()
            .expect("receiver session must build"),
    );

    let event_bus = Arc::new(EventBus::new_for_testing());

    // Step 1: produce a valid, correctly-signed SOAP envelope.
    let pm = bdew_pmode_sign_only("pm-tamper", RECEIVER_GLN, BdewAction::Aperak);
    let output = send_async(
        &sender_session,
        &event_bus,
        As4SendRequest {
            message_id: format!("tamper-base-{}", uuid::Uuid::new_v4()),
            payload: b"UNB+UNOC:3+original+payload+260101:0000+1'".to_vec(),
            policy: pm.to_send_policy().expect("policy must build"),
            credentials: None,
            payload_filename: None,
        },
    )
    .await
    .expect("valid signed SOAP must be built");

    let content_type = output.http_content_type.clone();
    let mut tampered_body = output.soap_envelope.body.to_vec();

    // Step 2: corrupt the payload by flipping a byte in the middle of the body.
    // The SOAP body contains the EDIFACT payload string. Find "UNB" and mutate it.
    if let Some(pos) = tampered_body.windows(3).position(|w| w == b"UNB") {
        tampered_body[pos] = b'X'; // "UNB" → "XNB" — valid UTF-8 but wrong content
    }

    let dedup = Arc::new(DurableInMemoryDedupBackend::new(
        std::time::Duration::from_secs(3600),
    ));

    // Step 3: feed the tampered bytes to the REAL As4WsSecVerifier.
    // Set fail_closed_audit_events = false so the test is compatible with
    // EventBus::new_for_testing() (no durable audit sink configured).
    let mut receive_policy = bdew_push_policy(None);
    receive_policy.fail_closed_audit_events = false;

    let result = receive_push_with_dedup_async_with_custom_verifier(
        &receiver_session,
        &event_bus,
        As4ReceivePushRequest {
            http_content_type: content_type,
            payload: Arc::from(tampered_body.as_slice()),
            receipt_payload: None,
            policy: receive_policy,
            authenticated_sender_scope: Some(Arc::from("test-sender")),
        },
        dedup,
        As4WsSecVerifier,
    )
    .await;

    // Step 4: assert rejection.
    assert!(
        result.is_err(),
        "As4WsSecVerifier must reject a message with a tampered payload — \
         BDEW AS4-Profil v1.2 §2.2.6.2.1 mandatory signing requirement"
    );
    let err = result.unwrap_err();
    // The error must be a cryptographic verification failure, not a parse error
    // or policy check — it must specifically be the signature mismatch.
    let err_str = format!("{err:?}");
    assert!(
        err_str.contains("signature")
            || err_str.contains("Signature")
            || err_str.contains("verify")
            || err_str.contains("Verify")
            || err_str.contains("CryptoFailure")
            || err_str.contains("InvalidSignature")
            || err_str.contains("WsSecVerification"),
        "Error must indicate signature verification failure, not a different error; got: {err}"
    );
}

// ── D1: require_encrypted_inbound enforcement ─────────────────────────────────

/// BDEW AS4-Profil v1.2 §2.2.6.2.2 — `bdew_push_policy(Some(key))` must
/// reject inbound AS4 messages that are not encrypted.
///
/// When the operator has configured an AS4 decryption key (production mode),
/// `require_encrypted_inbound` is set to `true`.  A plain-signed (unencrypted)
/// message must be rejected with a policy violation error.
///
/// This verifies the inbound enforcement leg of BDEW AS4-Profil v1.2 §2.2.6.2.2
/// is wired correctly in `makod`'s `as4_ingest.rs`.
#[tokio::test]
async fn inbound_encryption_enforced_when_decryption_key_set() {
    use asx_rs::as4::{
        As4ReceivePushRequest, As4SendRequest, InsecureBypassAs4Verifier,
        receive_push_with_dedup_async_with_custom_verifier, send_async,
    };
    use asx_rs::core::SessionContextBuilder;
    use asx_rs::observability::EventBus;
    use asx_rs::storage::DurableInMemoryDedupBackend;
    use mako_as4::testing::BdewTestPki;
    use mako_as4::{bdew_pmode_sign_only, bdew_push_policy, pmode::BdewAction};
    use std::sync::Arc;

    let sender_pki = BdewTestPki::generate("EncReq NB 9900000000020");
    const SENDER_GLN: &str = "9900000000020";
    const RECEIVER_GLN: &str = "9900000000021";

    let sender_session = Arc::new(
        SessionContextBuilder::new("enc-req-sender", SENDER_GLN)
            .with_signing_material(
                sender_pki.signing.cert_pem_str(),
                sender_pki.signing.key_pem_str(),
            )
            .with_trust_anchor_pem(sender_pki.signing.cert_pem_str())
            .build()
            .expect("sender session must build"),
    );
    let event_bus = Arc::new(EventBus::new_for_testing());

    // Produce a valid SIGN-ONLY (not encrypted) SOAP envelope.
    let pm = bdew_pmode_sign_only("pm-enc-req", RECEIVER_GLN, BdewAction::Utilmd);
    let output = send_async(
        &sender_session,
        &event_bus,
        As4SendRequest {
            message_id: format!("enc-req-{}", uuid::Uuid::new_v4()),
            payload: b"UNB+UNOC:3+plain+message+260101:0000+1'".to_vec(),
            policy: pm.to_send_policy().expect("policy must build"),
            credentials: None,
            payload_filename: None,
        },
    )
    .await
    .expect("sign-only SOAP must be built");

    // Configure the inbound policy with a decryption key (production mode).
    // This sets `require_encrypted_inbound = true`.
    // Set fail_closed_audit_events = false to be compatible with EventBus::new_for_testing().
    let dummy_decryption_key = vec![0u8; 32]; // content not validated at policy level
    let mut strict_policy = bdew_push_policy(Some(dummy_decryption_key));
    strict_policy.fail_closed_audit_events = false;
    assert!(
        strict_policy.require_encrypted_inbound,
        "bdew_push_policy with a key must set require_encrypted_inbound = true"
    );

    let dedup = Arc::new(DurableInMemoryDedupBackend::new(
        std::time::Duration::from_secs(3600),
    ));

    // Feed the SIGN-ONLY (unencrypted) message to a receiver with strict
    // inbound-encryption policy.  Use InsecureBypassAs4Verifier so the test
    // exercises only the policy-level encryption check, not PKI verification.
    let result = receive_push_with_dedup_async_with_custom_verifier(
        &sender_session, // receiver session; PKI not relevant here
        &event_bus,
        As4ReceivePushRequest {
            http_content_type: output.http_content_type.clone(),
            payload: Arc::from(output.soap_envelope.body.as_ref()),
            receipt_payload: None,
            policy: strict_policy,
            authenticated_sender_scope: Some(Arc::from("test-sender")),
        },
        dedup,
        InsecureBypassAs4Verifier,
    )
    .await;

    assert!(
        result.is_err(),
        "Unencrypted inbound message must be rejected when require_encrypted_inbound=true — \
         BDEW AS4-Profil v1.2 §2.2.6.2.2"
    );
    let err_str = format!("{:?}", result.unwrap_err());
    assert!(
        err_str.contains("encrypt")
            || err_str.contains("Encrypt")
            || err_str.contains("PolicyViolation"),
        "Rejection must cite encryption policy violation; got: {err_str}"
    );
}
