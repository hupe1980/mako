# mako-as4

[![Crates.io](https://img.shields.io/crates/v/mako-as4?label=mako-as4&color=f59e0b&logo=rust)](https://crates.io/crates/mako-as4)

BDEW AS4-Profil 1.2 for German Strom and Gas market communication, on
[`asx-rs`](https://crates.io/crates/asx-rs) 0.17. `asx::Profile::bdew()` fixes
every security and wire setting of the profile — ECDSA on BrainpoolP256r1,
ECDH-ES with AES-128-GCM, sign then encrypt, gzip, the fixed Action, roles and
`AgreementRef`, CRL revocation, SM-PKI mutual TLS with SNI — and its `build()`
holds the certificate rules of Regelungen zum Übertragungsweg für AS4 2.6. Nothing
here can relax it. This crate holds what a deployment decides: one identity and
client per MP-ID, the partners, the AS4 service of a message, its part
properties, file name and message id, and the retry window.

## Example

```rust
use mako_as4::{BdewAction, MessageIdDomain, Service, bdew_action_from_str, uebertragungsdatei_name};

// Every EDIFACT Übertragungsdatei is sent under the Marktprozesse service
// (BDEW AS4-Profil 1.2 Kap. 2.3.1.2.1); the AS4 Action is the profile's fixed one.
let action = bdew_action_from_str("UTILMD");
assert_eq!(action, BdewAction::Utilmd);
assert_eq!(action.service(), Service::Marktprozesse);

// eb:MessageId — `<outbox id>@<domain>`, the same on every retry.
let domain = MessageIdDomain::parse("as4.stadtwerke.example").unwrap();
assert_eq!(domain.message_id("42"), "42@as4.stadtwerke.example");

// The payload part's name (Allgemeine Festlegungen V6.1d Kap. 2.12).
let name = uebertragungsdatei_name(
    "UTILMD", "", "9900000000003", "9900000000010", (2026, 10, 1), "DAR1",
);
assert_eq!(name, "UTILMD__9900000000003_9900000000010_20261001_DAR1.txt");
```

A client per local MP-ID:

```rust,ignore
let builder = mako_as4::client_builder(&identity, sm_pki_root, ccrl_s_root, &partners)?;
let as4 = builder.build()?; // the startup check: asx::ErrorKind::Config names the setting
```

`client_builder` refuses two of an identity's signing, encryption and TLS
certificates that certify one key pair (BDEW AS4-Profil 1.2 Kap. 2.3.4.4) — compared
by public key, which `build()` does not do — loads the TLS key pair as the client
identity and registers the CRL signer. `build()` refuses a certificate of the
local or a partner triple without the MP-ID in its subject OU or without exactly
one URI subject alternative name (Regelungen zum Übertragungsweg für AS4 2.6
Kap. 5.2), a partner endpoint other than the encryption certificate's address
(Kap. 2) or on an IP address (Kap. 3). A `PartnerEntry` without an endpoint is
reached at the address its encryption certificate names.

The `eb:PartProperties` of Kap. 6 come from the EDIFACT envelope:

```rust
use mako_as4::PartProperties;

let props = PartProperties::from_interchange(
    b"UNB+UNOC:3+9900000000003:500+9900000000010:500+261001:1200+DAR1'UNH+1+UTILMD:D:11A:UN:S2.2'",
)
.unwrap();
assert_eq!(props.document_type, "UTILMD");
assert_eq!(props.document_date, "2026-10-01");
assert_eq!(props.document_no, "DAR1");
```

## API

| Module | Contents |
|---|---|
| `identity` | `LocalIdentity` (MP-ID, signer, decrypter, TLS key pair) and `client_builder` |
| `partner_directory` | `PartnerDirectory` / `PartnerEntry` — one `asx::Partner` per counterparty |
| `action` | `BdewAction` — the 16 standard EDIFACT message types, `Custom` for any other; `service()` |
| `service` | `Service` — the five `eb:Service` URIs of §2.3.1.2.1 |
| `file_name` | `uebertragungsdatei_name` — Allgemeine Festlegungen V6.1d Kap. 2.12 |
| `message_id` | `MessageIdDomain` — RFC 2822 `eb:MessageId` values |
| `part_properties` | `PartProperties` — `BDEWDocumentType`, `BDEWApplicationReference`, `BDEWDocumentDate`, `BDEWDocumentNo` from `UNB`/`UNH` |
| `keys` | `subject_public_key_info`, `same_public_key` |
| `constants` | party types per issuing agency (§2.3.1.1), `PAYLOAD_MIME_TYPE`, `MAX_MESSAGE_BYTES`, `MAX_RETRY_DURATION_SECS` (72 h) |

Tests use `asx::testkit` (`asx-rs` feature `test-util`, dev-dependencies only):
a test SM-PKI root issuing each MP-ID separate BrainpoolP256r1 key pairs with the
MP-ID as subject OU and its AS4 address as URI subject alternative name, an
in-process loopback between two clients and a socket-bound TLS MSH.
`services/makod/tests/it/as4_security.rs` holds the end-to-end tests. API
documentation: <https://docs.rs/mako-as4>; operator reference:
[AS4 (BDEW)](https://hupe1980.github.io/mako/docs/reference/as4-bdew/).
