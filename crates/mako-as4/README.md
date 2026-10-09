# mako-as4

[![Crates.io](https://img.shields.io/crates/v/mako-as4?label=mako-as4&color=f59e0b&logo=rust)](https://crates.io/crates/mako-as4)

The BDEW AS4-Profil v1.2 on top of [`asx-rs`](https://crates.io/crates/asx-rs):
pre-configured P-Modes, the sign-and-encrypt security policy with
BrainpoolP256r1, a bilateral partner directory and test PKI helpers for German
Strom and Gas market communication.

## Example

```rust
use mako_as4::BdewAs4Profile;

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

`bdew_mako_profile_stack()` declares sign **and** encrypt as the stack's
security floor (§ 2.2.6.2.2), and `asx-rs` refuses any override layer — a
partner overlay included — that relaxes it (`SecurityFloorViolation`).
`validate()` runs at startup, so a downgraded profile never serves traffic.

## Profile requirements

| Requirement | Value | Spec |
|---|---|---|
| Transport | HTTPS + mTLS (TLS 1.2 minimum) | §2.2.1 |
| SOAP version | 1.2 | §2.2.3 |
| MEP | One-Way/Push (mandatory) | §2.2.5 |
| **Signing algorithm** | **ECDSA-SHA256 + BrainpoolP256r1** | §2.2.6.2.1 / BSI TR-03116-3 §9.1 |
| **Signing token type** | **`BinarySecurityToken` / X509PKIPathv1** | §2.2.6.2.1 |
| **Encryption** | **ECDH-ES + ConcatKDF + AES-128-GCM — mandatory** | §2.2.6.2.2 / BSI TR-03116-3 §9.2 |
| **Key reference** | **X509SKI** | §2.2.6.2.2 |
| Payload | gzip-compressed, carried as `application/octet-stream` in its own MIME part; the SOAP Body stays empty | §2.2.3.2 |
| Party ID | 13-digit GLN, ISO 6523 ICD 0088 | §2.2.4 |
| Retry window | 72 hours, up to 5 attempts | §2.2.7 |
| Deduplication | Required (persistent dedup store) | §4.2 |
| Synchronous receipt | Mandatory | §4.6.3 |

BDEW requires three separate BrainpoolP256r1 keypairs: mTLS, WS-Security
signing and XML encryption.

## API

| Module | Contents |
|---|---|
| `constants` | BDEW-specific service URIs, algorithm identifiers (`SIG_ALGO_ECDSA_SHA256`, `ENC_KEY_AGREEMENT_ECDH_ES`, …) |
| `pmode` | `BdewAction` enum + `bdew_pmode()` / `bdew_pmode_sign_only()` factory functions; `WsSecOutboundKeyInfoProfile` |
| `profile` | `BdewAs4Profile` entry point + `bdew_mako_profile_stack()` + `bdew_push_policy()` |
| `partner_directory` | `PartnerDirectory` — GLN-to-endpoint resolution |
| `server` *(feature)* | Axum AS4 inbound router + `bdew_router_config()` |
| `testing` *(feature)* | `BdewTestPki`, `generate_self_signed_bdew_keypair()`, `MockAs4Endpoint` |

| Feature | Enables |
|---|---|
| `server` | the Axum AS4 inbound router (`bdew_router_config`) |
| `testing` | `BdewTestPki` (all three keypairs in memory), `MockAs4Endpoint`, `generate_self_signed_bdew_keypair` |

`services/makod/tests/it/as4_security.rs` holds the end-to-end compliance tests.
API documentation: <https://docs.rs/mako-as4>; operator reference:
[AS4 (BDEW)](https://hupe1980.github.io/mako/docs/reference/as4-bdew/).
