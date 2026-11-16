# energy-api

[![Crates.io](https://img.shields.io/crates/v/energy-api?label=energy-api&color=f59e0b&logo=rust)](https://crates.io/crates/energy-api)

The BDEW API-Webdienste Strom — the REST/JSON channel beside EDIFACT/AS4 — as
typed clients and matching Axum routers: MaLo-Ident, § 14a Steuern
(`controlMeasures`), the iMS Universalbestellprozess and the Verzeichnisdienst.
It implements the specifications at version **1.0.0** and the Regelungen zum
Übertragungsweg für API-Webdienste 1.2; `makod` mounts the routers on its
`:8090` port. Process state and the decision what to answer stay with the
caller.

## Example

```rust,no_run
use energy_api::directory::DirectoryServiceClient;
use url::Url;

let base = Url::parse("https://verzeichnisdienst.example.de/")?;
let client = DirectoryServiceClient::new_insecure(base)?;
// The provider's EMT.API signature key, obtained from the issuing Sub-CA: the
// record is returned only when its JWS verifies against it.
let record = client
    .get_record("1234567890723", "controlMeasuresV1", 1, &[provider_key])
    .await?;
println!("{}", record.url);
```

## What it covers

| API | Parties | Purpose |
|---|---|---|
| `controlMeasures` | NB/LF ↔ MSB | Steuerbefehle (§ 14a EnWG) |
| `maloIdent` | LF ↔ NB | Marktlokations-Identifikation |
| `wimOrder` | NB ↔ MSB | iMS Universalbestellprozess, PIDs 11021–11023 |
| `directory` | all | Verzeichnisdienst — endpoint discovery by the provider's MP-ID |

- `server` — one `router(state)` per API (`control_measures`, `malo_ident`,
  `wim_order`, `directory::server`).
- `client` — the MaLo-Ident callbacks; `directory` — the Verzeichnisdienst
  client, every record verified (`directory::verify_record`).
- Content signing (TR-03116-3, RFC 8785 canonical JSON; RzÜ API-Webdienste 1.2
  Kap. 3.3) and JWS verification are always compiled: a request is signed
  exactly when the client has a signing key.
- Identifiers are the validated `rubo4e::identifiers` types (`MaloId`, `MeloId`,
  `NeloId`, `SrId`, `TrId`, `MarktpartnerId` as `MarketPartnerId`), so
  `Deserialize` refuses a wrong check digit at the API boundary.

BNetzA Mitteilung Nr. 58 keeps the channel at MaLo-Ident and Steuern — „Die
UTILTS bleibt damit erhalten". The `2.0.0` tag of `EDI-Energy/api-electricity`
(`locationBundle`, `calculationFormula`) is binding nowhere and not implemented.

## Features

| Feature | Enables |
|---|---|
| `client` | MaLo-Ident and directory HTTP clients (reqwest + rustls) |
| `server` | Axum router factories |
| `websocket` | Verzeichnisdienst subscriptions (tokio-tungstenite); a pushed record that does not verify arrives as an `Err` |

None is on by default: `cargo add energy-api --features client,server`.

## Sources

- EDI-Energy `api-electricity` and `api-directory-service`, version 1.0.0
- Regelungen zum Übertragungsweg für API-Webdienste 1.2
- BNetzA Mitteilung Nr. 58 (01.10.2026)

Architecture: [API-Webdienste](https://hupe1980.github.io/mako/docs/architecture/api-webdienste/) ·
API: <https://docs.rs/energy-api>

License: MIT OR Apache-2.0
