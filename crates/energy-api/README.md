# energy-api

[![Crates.io](https://img.shields.io/crates/v/energy-api?label=energy-api&color=f59e0b&logo=rust)](https://crates.io/crates/energy-api)

The BDEW API-Webdienste Strom — the REST/JSON channel beside EDIFACT/AS4 — as
typed REST and WebSocket clients and matching Axum server routers: MaLo-Ident,
§ 14a Steuern (`controlMeasures`), the iMS Universalbestellprozess and the
Verzeichnisdienst. It implements specification **1.0.0** under API-Guideline
1.0a / 1.0b and the Regelungen zum Übertragungsweg 1.2; `makod` mounts it on
its `:8090` port.

```bash
cargo add energy-api --features client,server
```

## Example

```rust,no_run
use energy_api::directory::DirectoryServiceClient;
use url::Url;

let base = Url::parse("https://verzeichnisdienst.example.de/")?;
let client = DirectoryServiceClient::new_insecure(base)?;
let (record, _cert, _sig) = client
    .get_record("1234567890123", "controlMeasuresV1", 1)
    .await?;
println!("{}", record.url);
```

## Scope

| API | Parties | Purpose |
|---|---|---|
| `controlMeasures` | NB/LF ↔ MSB | Grid control commands (§ 14a EnWG) |
| `maloIdent` | LF ↔ NB | Marktlokations-Identifikation |
| `wimOrder` | MSB | iMS Universalbestellprozess (iMS Anmeldung, PIDs 11021–11023) |
| `directory` | All | `Verzeichnisdienst` — endpoint discovery via GLN |

The `server` module carries one `router(state)` per API (`control_measures`,
`malo_ident`, `wim_order`, `directory::server`); `client` carries the MaLo-Ident
callbacks and the directory client.

| Feature | Default | Enables |
|---|---|---|
| `client` | | MaLo-Ident and directory HTTP clients (reqwest + rustls) |
| `server` | | Axum router factories for server implementations |
| `websocket` | | WebSocket subscription client (tokio-tungstenite) |

TR-03116-3 content signing (RFC 8785 canonical JSON) and JWS verification are
always compiled: a request is signed exactly when the client has a signing key.

## Identifiers and wire contract

Identifiers are the validated `rubo4e::identifiers` types (`MaloId`, `MeloId`,
`NeloId`, `SrId`, `TrId`, `MarktpartnerId` as `MarketPartnerId`), so
`Deserialize` refuses a wrong check digit at the API boundary.
`identificationParameterId` names (`maloId`, `tranchenIds`, `meloIds`,
`meterNumbers`, `customerNumber`) are pinned by a test against `maloIdentV1.yaml`
at tag `1.0.0`.

BNetzA Mitteilung Nr. 58 keeps the channel at its productive scope —
MaLo-Ident and Steuern; „Die UTILTS bleibt damit erhalten". The `2.0.0` tag of
`EDI-Energy/api-electricity` (`locationBundle`, `calculationFormula`) is binding
nowhere and not implemented.

API documentation: <https://docs.rs/energy-api>; architecture:
[API-Webdienste](https://hupe1980.github.io/mako/docs/architecture/api-webdienste/).
