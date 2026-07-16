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
> and asx-rs v0.7 automatically uses ECDSA-SHA256. Supply an EC encryption certificate
> and asx-rs v0.7 automatically uses ECDH-ES + ConcatKDF + AES-128-GCM.

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
//    asx-rs v0.7 auto-selects ECDH-ES when an EC cert is supplied.
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

Enable the `testing` feature to generate BrainpoolP256r1 test keypairs in memory:

```toml
[dev-dependencies]
mako-as4 = { version = "0.11", features = ["testing"] }
```

```rust
use mako_as4::testing::{BdewTestPki, MockAs4Endpoint};

// All three keypairs (TLS, signing, encryption) on BrainpoolP256r1.
let pki = BdewTestPki::generate("Test NB 9900357000004");

// In-process mock AS4 server — no WIRK certs needed.
# async fn example() {
let endpoint = MockAs4Endpoint::bind("127.0.0.1:0").await.unwrap();
let url = endpoint.local_url(); // "http://127.0.0.1:PORT/as4/inbox"
# }
```

---

## Feature flags

| Flag | Enables | Extra deps |
|---|---|---|
| `server` | Axum AS4 inbound router (`bdew_router_config`) | `axum` |
| `testing` | `BdewTestPki`, `MockAs4Endpoint`, `generate_self_signed_bdew_keypair` | none (uses asx-rs testing) |

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
| `asx-rs` 0.7 | AS4/ebMS3 transport engine (ECDSA signing, ECDH-ES encrypt, dedup) |
| `makod` | Production daemon — assembles AS4 ingest, sender, and all 45+ BDEW workflows |

---

## BDEW AS4 requirements

| Requirement | Specification |
|---|---|
| Transport | HTTPS + mutual TLS (TLS 1.2 minimum) |
| SOAP version | 1.2 |
| MEP | One-Way/Push (mandatory) |
| Signing | RSA-SHA256 + Exclusive C14N + SHA-256 digest — **mandatory** |
| Encryption | AES-128-CBC or AES-256-GCM + RSA-OAEP — **optional** |
| Party ID | 13-digit GLN, type `urn:oasis:names:tc:ebcore:partyid-type:iso6523:0088` |
| Retry window | 72 hours, up to 5 attempts |
| Deduplication | Required (persistent `asx_rs::storage::TtlDedupStorage`) |

AS4 became mandatory for electricity market communication on **1 April 2024**
and for gas market communication on **1 April 2025**
(BNetzA orders BK6-22-024 / BK7-22-023).

---

## API overview

| Module | Contents |
|---|---|
| `constants` | BDEW-specific service URIs, algorithm identifiers, retry parameters |
| `pmode` | `BdewAction` enum + `bdew_pmode()` / `bdew_pmode_encrypted()` P-Mode factories |
| `profile` | `BdewAs4Profile` entry point + `bdew_mako_profile_stack()` |
| `partner_directory` | `PartnerDirectory` — GLN-to-endpoint resolution |
| `server` *(feature)* | Axum-based AS4 receive endpoint + router config |

---

## Quick start

```rust
use mako_as4::{BdewAs4Profile, BdewAction, bdew_pmode, constants};

// Build a profile and register bilateral P-Modes for each trading partner.
let mut profile = BdewAs4Profile::new();
profile
    .register_pmode(bdew_pmode("pm-utilmd-a", "9900000000001", BdewAction::Utilmd))
    .register_pmode(bdew_pmode("pm-aperak-a", "9900000000001", BdewAction::Aperak));

// Fail-fast at startup — verifies all security invariants.
profile.validate().expect("BDEW MaKo profile must satisfy all security invariants");

// Resolve a P-Mode at send time.
let pm = profile.resolve_pmode(
    "9900000000001",
    constants::SERVICE,
    &BdewAction::Utilmd.as_uri(),
);
assert!(pm.is_some());
```

---

## Feature flags

| Flag | Enables |
|---|---|
| `server` | Axum-based AS4 inbound receive endpoint (`bdew_router_config`) |

---

## Regulatory references

- **BDEW AS4 Kommunikationshandbuch** — transport and security profile specification
- **BNetzA BK6-22-024** — mandatory AS4 adoption for electricity (effective 2024-04-01)
- **BNetzA BK7-22-023** — mandatory AS4 adoption for gas (effective 2025-04-01)
- **OASIS ebMS3 / AS4 Profile** — messaging standard

---

## Related crates

| Crate | Role |
|---|---|
| `mako-as4` ← **this crate** | BDEW AS4 profile constants, P-Modes, security policy |
| `asx-rs` | AS4/ebMS3 transport engine (message signing, dedup, send/receive) |
| `makod` | Production daemon — assembles AS4 ingest and sender |
