# mako-markt

[![Crates.io](https://img.shields.io/crates/v/mako-markt?label=mako-markt&color=f59e0b&logo=rust)](https://crates.io/crates/mako-markt)

The master-data domain of the German energy market: validated identifiers
(MaLo-ID, MeLo-ID, MP-ID), the BO4E gate every payload crosses, the record
types behind [`marktd`](https://hupe1980.github.io/mako/docs/services/marktd/),
the Referenzreihen catalogue, and the typed `makod` command names. It holds no
storage and no server: the PostgreSQL repositories and HTTP handlers are
marktd's.

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
validation. `bo4e::decode` makes four decisions in one place:

| Stage | Refuses | `code` |
|---|---|---|
| 1. Discriminator | a `Zaehler` posted to the `Geraet` endpoint | `bo4e.discriminator` |
| 2. Schema | a value the type cannot hold | `bo4e.schema` |
| 3. Strict enums | `"sparte": "STROMM"`, at any depth, by JSON path | `bo4e.unknown_enum` |
| 4. BO4E rules | a document the standard's prose forbids | `bo4e.rule` |

`decode_received` reads a counterparty's document: stages 1–3 refuse, a rule
violation comes back beside the value to be disputed. `ensure_conformant` runs
stages 3 and 4 on a value mako built.

## What it covers

| Module | Contents |
|---|---|
| `domain` | `MaloId`, `MeloId`, `MarktpartnerId`, `EicCode`, … (from `rubo4e::identifiers`), `Sparte`, `nad_agency_code` |
| `bo4e` | `decode`, `decode_received`, `ensure_conformant`, shadow columns |
| `repository` | the record types of every marktd aggregate, incl. `ZuordnungsermaechtigungRecord` (keyed per ZRT, BG, BK and LF — MaBiS Kap. 10.2.1) |
| `referenzreihen` | `Reihe`, `ReiheInfo`, `Zeitraum`: the published figures billing reads (KAV, EEG Marktwerte, day-ahead) and the `referenzwert_fehlt` refusal |
| `lokationsbuendel`, `utilmd_stammdaten` | a Marktlokation's Lokationsbündel and UTILMD Stammdaten on a Stichtag |
| `cloudevents` | `InboundMakoEvent` (from makod) and `MarktEvent` (`de.markt.*`) |
| `commands` | the catalogued `makod` command names |
| `makod_client`, `marktd_client` | typed HTTP clients; `MarktdClient::referenzwert` returns a missing value as an error, never a zero |

## Features

| Feature | Enables |
|---|---|
| `makod-client` | `MakodClient`, the typed client for makod's admin and command APIs |
| `marktd-client` | `MarktdClient`, the typed client for marktd's APIs |

## Sources

- BK6-24-174 Anlage 3 (MaBiS) Kap. 10.2.1, S. 119 — the Zuordnungsermächtigung key

Service: [marktd](https://hupe1980.github.io/mako/docs/services/marktd/) ·
Model: [Domain model](https://hupe1980.github.io/mako/docs/architecture/domain-model/) ·
API: <https://docs.rs/mako-markt>

License: MIT OR Apache-2.0
