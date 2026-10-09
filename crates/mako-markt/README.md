# mako-markt

[![Crates.io](https://img.shields.io/crates/v/mako-markt?label=mako-markt&color=f59e0b&logo=rust)](https://crates.io/crates/mako-markt)

Master-data domain for the German energy market: validated identifiers
(MaLo-ID, MeLo-ID, MP-ID), the BO4E gate every payload crosses, the repository
traits and record types behind [`marktd`](https://hupe1980.github.io/mako/docs/services/marktd/)
(Marktlokation, Messlokation, VersorgungsStatus, NB contracts, PRICAT), the
`de.markt.*` CloudEvents and the typed `makod` command names.

## Example

```rust
use mako_markt::bo4e;
use mako_markt::domain::{MaloId, MarktpartnerId};
use rubo4e::current::Marktlokation;

// Identifiers validate on construction — including the MaLo-ID check digit.
let malo: MaloId = "51238696012".parse()?;
let nb: MarktpartnerId = "9900357000004".parse()?;
assert_eq!(nb.nad_agency_code(), "293"); // BDEW-Codenummer → NAD DE3055 293
assert!("51238696782".parse::<MaloId>().is_err());

// An inbound BO4E document passes the gate or is refused with a stable code.
let payload = serde_json::json!({ "_typ": "MARKTLOKATION", "marktlokationsId": malo.to_string() });
let decoded: Result<Marktlokation, _> = bo4e::decode(payload);
if let Err(rejection) = decoded {
    println!("{}", rejection.to_json()); // 422 body
}
```

## The BO4E gate

BO4E's schema constrains almost nothing, so „it deserialises" is not
validation. `bo4e::decode` makes the four decisions in one place:

| Stage | Refuses | `code` |
|---|---|---|
| 1. Discriminator | a `Zaehler` posted to the `Geraet` endpoint | `bo4e.discriminator` |
| 2. Schema | a value the type cannot hold | `bo4e.schema` |
| 3. Strict enums | `"sparte": "STROMM"`, at any depth, by JSON-path | `bo4e.unknown_enum` |
| 4. BO4E rules | a document the standard's prose forbids | `bo4e.rule` |

| Variant | Use |
|---|---|
| `decode` | an endpoint. All four stages. |
| `decode_received` | a counterparty's document. Stages 1–3 refuse; a rule violation is returned alongside the value, to be disputed rather than dead-lettered. |
| `ensure_conformant` | outbound: stages 3 and 4 on a value mako built. |

A rule belongs in stage 4 only if BO4E itself asserts it. Writes take the typed
BO (`MaloRepository::upsert(&Marktlokation)`); reads hand the stored JSONB back
verbatim.

## Modules

| Module | Contents |
|---|---|
| `domain` | `MaloId`, `MeloId`, `MarktpartnerId`, `EicCode`, … (from `rubo4e::identifiers`), `Sparte` |
| `bo4e` | `decode`, `decode_received`, `ensure_conformant`, shadow columns |
| `repository` | AFIT repository traits and record types; PostgreSQL implementations live in `marktd` |
| `cloudevents` | `InboundMakoEvent`, `MarktEvent` and the `de.markt.*` emissions |
| `commands` | the catalogued `makod` command names |
| `makod_client`, `marktd_client` | HTTP clients (features `makod-client`, `marktd-client`) |
| `testing` | two in-memory doubles (feature `testing`, never in production) |

API documentation: <https://docs.rs/mako-markt>.
