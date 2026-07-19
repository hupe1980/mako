# mako-as4

**BDEW MaKo AS4 profile — sign + encrypt with BrainpoolP256r1.**

Encodes the **BDEW AS4-Profil v1.2** requirements on top of
[`asx-rs`](https://crates.io/crates/asx-rs), providing pre-configured
P-Modes, security policy constants, a bilateral partner directory, and
test helpers for the German electricity and gas AS4 network.

---

## BDEW AS4-Profil v1.2 requirements

| Requirement | Value | Spec |
|---|---|---|
| Transport | HTTPS + mTLS (TLS 1.2 minimum) | §2.2.1 |
| SOAP version | 1.2 | §2.2.3 |
| MEP | One-Way/Push (mandatory) | §2.2.5 |
| **Signing algorithm** | **ECDSA-SHA256 + BrainpoolP256r1** | §2.2.6.2.1 / BSI TR-03116-3 §9.1 |
| **Signing token type** | **`BinarySecurityToken` / X509PKIPathv1** | §2.2.6.2.1 |
| **Encryption** | **ECDH-ES + ConcatKDF + AES-128-GCM — mandatory** | §2.2.6.2.2 / BSI TR-03116-3 §9.2 |
| **Key reference** | **X509SKI** | §2.2.6.2.2 |
| Party ID | 13-digit GLN, ISO 6523 ICD 0088 | §2.2.4 |
| Retry window | 72 hours, up to 5 attempts | §2.2.7 |
| Deduplication | Required (persistent dedup store) | §4.2 |
| Synchronous receipt | Mandatory | §4.6.3 |

> **All algorithms are auto-detected** — supply an EC (BrainpoolP256r1) signing key
> and asx-rs automatically uses ECDSA-SHA256. Supply an EC encryption certificate
> and asx-rs automatically uses ECDH-ES + ConcatKDF + AES-128-GCM.

AS4 became mandatory for electricity on **1 April 2024** (BK6-22-024) and
for gas on **1 April 2025** (BK7-22-023).

---

## Certificate triplet

BDEW requires **three separate X.509 keypairs**, all using BrainpoolP256r1:

```
mTLS certificate         WS-Security signing cert    XML Encryption cert
KeyUsage: digitalSig     KeyUsage: digitalSignature  KeyUsage: keyAgreement
        ↕ mTLS                  ↕ sign payload              ↕ ECDH-ES
   HTTPS transport          outbound messages           recipient's pubkey
```

---

## API overview

| Module | Contents |
|---|---|
| `constants` | BDEW-specific service URIs, algorithm identifiers (`SIG_ALGO_ECDSA_SHA256`, `ENC_KEY_AGREEMENT_ECDH_ES`, …) |
| `pmode` | `BdewAction` enum + `bdew_pmode()` / `bdew_pmode_sign_only()` factory functions; `WsSecOutboundKeyInfoProfile` |
| `profile` | `BdewAs4Profile` entry point + `bdew_mako_profile_stack()` + `bdew_push_policy()` |
| `partner_directory` | `PartnerDirectory` — GLN-to-endpoint resolution |
| `server` *(feature)* | Axum AS4 inbound router + `bdew_router_config()` |
| `testing` *(feature)* | `BdewTestPki`, `generate_self_signed_bdew_keypair()`, `MockAs4Endpoint` |

---

## Quick start

```rust
use mako_as4::{BdewAs4Profile, BdewAction, bdew_pmode, constants};

// 1. Register bilateral P-Modes for each trading partner.
let mut profile = BdewAs4Profile::new();
profile.register_partner_all_actions(
    "9900000000001",
    "https://partner.example/as4/inbox",
);

// 2. Register the partner's EC encryption certificate (BrainpoolP256r1).
//    asx-rs auto-selects ECDH-ES when an EC cert is supplied.
let partner_encrypt_cert_pem: Vec<u8> = std::fs::read("/etc/partner/encrypt.pem").unwrap_or_default();
profile.register_partner_encryption_cert("9900000000001", partner_encrypt_cert_pem);

// 3. Fail-fast at startup.
profile.validate().expect("BDEW MaKo profile must satisfy all security invariants");

// 4. Build inbound push policy with own decryption key.
let own_decrypt_key_pem: Vec<u8> = std::fs::read("/etc/certs/as4-encrypt.key.pem").unwrap_or_default();
let push_policy = mako_as4::bdew_push_policy(Some(own_decrypt_key_pem));
// push_policy.require_encrypted_inbound == true (rejects unencrypted inbound)
```

---

## Testing without WIRK certificates

Enable the `testing` feature to generate BrainpoolP256r1 test keypairs in memory.
The test helpers leverage the asx-rs v0.8.0 testing API:

```toml
[dev-dependencies]
mako-as4 = { version = "0.12", features = ["testing"] }
```

### Full sign+encrypt round-trip test

```rust
use mako_as4::testing::{BdewTestPki, MockAs4Endpoint};
use asx_rs::core::SessionContextBuilder;
use asx_rs::observability::EventBus;
use asx_rs::transport::As4HttpTransport;

#[tokio::test]
async fn test_as4_sign_encrypt_round_trip() {
    let sender_pki = BdewTestPki::generate("Test NB 9900357000004");
    let receiver_pki = BdewTestPki::generate("Test LF 9900357000005");

    // Build a mock endpoint configured to decrypt ECDH-ES messages.
    let mock = MockAs4Endpoint::builder()
        .with_decryption_key_pem(receiver_pki.encryption.key_pem.clone())
        .bind("127.0.0.1:0")
        .await
        .unwrap();

    // Build sender session — with_signing_material() atomically sets cert + key
    // and auto-derives key_id = "cert:{partner_id}" (no manual CertHandle needed).
    let session = std::sync::Arc::new(
        SessionContextBuilder::new("sess-test", "9900357000004")
            .with_signing_material(
                sender_pki.signing.cert_pem_str(),
                sender_pki.signing.key_pem_str(),
            )
            .with_trust_anchor_pem(sender_pki.signing.cert_pem_str())
            .build()
            .unwrap(),
    );

    // EventBus::new_for_testing() — BestEffort mode, no durable audit sink needed.
    let event_bus = std::sync::Arc::new(EventBus::new_for_testing());

    // ... send a sign+encrypt message using As4SendRequest ...

    // As4HttpTransport::new_for_localhost_testing() — SSRF guard disabled for tests.
    // Use send_to_localhost() instead of send() to bypass the URL validator.
    let transport = As4HttpTransport::new_for_localhost_testing().unwrap();
    // transport.send_to_localhost(&mock.local_url(), &output).await.unwrap();

    // Mock delivers the decrypted payload via next_received().
    // let received = mock.next_received().await.unwrap();
    // assert!(received.payload.starts_with(b"UNB"));
}
```

### BdewTestPki — three-keypair bundle

```rust
use mako_as4::testing::BdewTestPki;

// All three keypairs (TLS, signing, encryption) on BrainpoolP256r1.
let pki = BdewTestPki::generate("Test NB 9900357000004");

println!("Signing cert (PEM): {}", pki.signing.cert_pem_str());
println!("Encryption key (PEM): {}", pki.encryption.key_pem_str());
// pki.tls, pki.signing, pki.encryption — each has cert_pem + key_pem
```

---

## Feature flags

| Flag | Enables | Extra deps |
|---|---|---|
| `server` | Axum AS4 inbound router (`bdew_router_config`) | `axum` |
| `testing` | `BdewTestPki`, `MockAs4Endpoint`, `generate_self_signed_bdew_keypair` | none (uses asx-rs testing) |

---

## Security test coverage in makod

`services/makod/tests/as4_security.rs` ships 11 tests that verify BDEW AS4 compliance:

| Test | Verifies |
|---|---|
| `sign_encrypt_pmode_defaults` | `bdew_pmode()` defaults to `sign=true, encrypt=true` per §2.2.6.2.2 |
| `sign_only_pmode_disables_encryption` | `bdew_pmode_sign_only()` disables encryption (dev/test only) |
| `policy_with_key_requires_encryption` | `bdew_push_policy(Some(key))` enforces `require_encrypted_inbound` |
| `policy_without_key_no_encryption_required` | Dev-mode without decryption key does not block onboarding |
| `fragment_scope_is_soap_sender_id` | OneWayPush never triggers fragment scope `PolicyViolation` |
| `sign_encrypt_policy_is_bdew_compliant` | SOAP policy constants satisfy §2.2.6.2.1 + §2.2.6.2.2 |
| `replay_dedup_blocks_duplicate_message_id` | 72-hour dedup window (§4.2) |
| `tampered_signature_is_rejected` | Real `As4WsSecVerifier` rejects payload tampering |
| `inbound_encryption_enforced_when_decryption_key_set` | `require_encrypted_inbound` rejects unencrypted messages |
| `sign_encrypt_round_trip_via_mock_endpoint` | Full sign+encrypt→transport→decrypt round-trip |
| `sign_only_round_trip_envelope_contains_wssec_signature` | Sign-only round-trip preserves WS-Security elements |

---

## Regulatory references

| Document | Scope |
|---|---|
| **BDEW AS4-Profil v1.2** (01.04.2026) | Complete AS4 transport specification |
| **BSI TR-03116-3** | Cryptographic algorithms (ECDSA §9.1, ECDH-ES §9.2) |
| **BNetzA BK6-22-024** | Mandatory AS4 for Strom (2024-04-01) |
| **BNetzA BK7-22-023** | Mandatory AS4 for Gas (2025-04-01) |
| **RFC 6090** | EC cryptography interoperability (referenced by BDEW §2.2.6.2.1/2) |

---

## Related crates

| Crate | Role |
|---|---|
| `mako-as4` ← **this crate** | BDEW AS4 profile (P-Modes, constants, policy, test helpers) |
| `asx-rs` 0.8 | AS4/ebMS3 transport engine (ECDSA signing, ECDH-ES encrypt, dedup, testing helpers) |
| `makod` | Production daemon — assembles AS4 ingest, sender, and all 45+ BDEW workflows |

