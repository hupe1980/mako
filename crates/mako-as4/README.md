# mako-as4

**BDEW MaKo AS4 profile for German energy market communication.**

Encodes the **BDEW AS4 Kommunikationshandbuch** requirements on top of
[`asx-rs`](https://crates.io/crates/asx-rs), providing pre-configured
P-Modes, security policy constants, and a partner directory for bilateral
AS4 communication in the German electricity and gas markets.

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
