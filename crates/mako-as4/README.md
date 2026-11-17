# mako-as4

[![Crates.io](https://img.shields.io/crates/v/mako-as4?label=mako-as4&color=f59e0b&logo=rust)](https://crates.io/crates/mako-as4)

The BDEW AS4-Profil 1.2 for German Strom and Gas market communication, on
[`asx-rs`](https://crates.io/crates/asx-rs) 0.19. `asx::Profile::bdew()` fixes
every security and wire setting of the profile and its `build()` holds the
certificate rules of the Regelungen zum Übertragungsweg für AS4 2.6; nothing
here can relax either. This crate holds what a deployment decides: one identity
and client per MP-ID, the partners, the AS4 service of a message, its part
properties, file name and message id, and the retry window.

## Example

```rust
use mako_as4::{BdewAction, MessageIdDomain, PartProperties, Service, bdew_action_from_str, uebertragungsdatei_name};

// Every EDIFACT Übertragungsdatei is sent under the Marktprozesse service.
let action = bdew_action_from_str("UTILMD");
assert_eq!(action, BdewAction::Utilmd);
assert_eq!(action.service(), Service::Marktprozesse);

// eb:MessageId — `<outbox id>@<domain>`, the same on every retry.
let domain = MessageIdDomain::parse("as4.stadtwerke.example").unwrap();
assert_eq!(domain.message_id("42"), "42@as4.stadtwerke.example");

// The payload part's name.
let name = uebertragungsdatei_name(
    "UTILMD", "", "9900000000003", "9900000000010", (2026, 10, 1), "DAR1",
);
assert_eq!(name, "UTILMD__9900000000003_9900000000010_20261001_DAR1.txt");

// The eb:PartProperties, read from the EDIFACT envelope.
let props = PartProperties::from_interchange(
    b"UNB+UNOC:3+9900000000003:500+9900000000010:500+261001:1200+DAR1'UNH+1+UTILMD:D:11A:UN:S2.2'",
)
.unwrap();
assert_eq!(props.document_type, "UTILMD");
assert_eq!(props.document_date, "2026-10-01");
```

A client per local MP-ID:

```rust,ignore
let builder = mako_as4::client_builder(&identity, sm_pki_root, ccrl_s_root, &partners);
let as4 = builder.build()?; // asx::ErrorKind::Config names the refused setting
```

## What it covers

| Item | Rule | Source |
|---|---|---|
| `LocalIdentity`, `client_builder` | signing, encryption and an EC TLS key pair per MP-ID | AS4-Profil 1.2 Kap. 2.2.6.1, S. 10 |
| `build()` (asx-rs) | three distinct key pairs, compared by key value; MP-ID in the subject OU and exactly one URI subject alternative name on every certificate of both triples | AS4-Profil 1.2 Kap. 2.3.4.4; RzÜ AS4 2.6 Kap. 5.2, S. 6 |
| `PartnerEntry` | endpoint optional — reached at the encryption certificate's address, never an IP address | RzÜ AS4 2.6 Kap. 2, S. 4; Kap. 3, S. 5 |
| `PartProperties` | `BDEWDocumentType`, `BDEWApplicationReference`, `BDEWDocumentDate`, `BDEWDocumentNo` from `UNB`/`UNH` | RzÜ AS4 2.6 Kap. 6, S. 7 |
| `BdewAction`, `Service` | 16 standard EDIFACT message types plus `Custom`; the five `eb:Service` URIs | AS4-Profil 1.2 Kap. 2.3.1.2.1, S. 14 |
| `constants` | `eb:PartyId` type per issuing agency, `PAYLOAD_MIME_TYPE`, `MAX_MESSAGE_BYTES`, `MAX_RETRY_DURATION_SECS` (72 h) | AS4-Profil 1.2 Kap. 2.3.1.1 |
| `uebertragungsdatei_name` | the Übertragungsdatei file name | Allgemeine Festlegungen V6.1d Kap. 2.12 |
| `MessageIdDomain` | RFC 2822 `eb:MessageId`, stable across retries | — |

Tests use `asx::testkit` (`asx-rs` feature `test-util`, dev-dependencies only);
`services/makod/tests/it/as4_security.rs` holds the end-to-end tests.

## Sources

- BDEW AS4-Profil 1.2
- Regelungen zum Übertragungsweg für AS4 2.6 (RzÜ), anzuwenden ab 01.10.2026
- BDEW Allgemeine Festlegungen V6.1d

Operator reference: [AS4 (BDEW)](https://hupe1980.github.io/mako/docs/reference/as4-bdew/) ·
API: <https://docs.rs/mako-as4>

License: MIT OR Apache-2.0
