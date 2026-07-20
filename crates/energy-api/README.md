# energy-api

**BDEW API-Webdienste Strom — REST/WebSocket client and Axum server bindings.**

Implements the German energy market **API-Webdienste Strom** specification
(BDEW/VKU/GEODE, current version 1.3), providing typed REST and WebSocket
clients for iMS grid control processes and a matching Axum server for hosting
the receiving endpoints.

---

## Scope

The BDEW API-Webdienste Strom defines a REST/JSON channel used primarily for
**intelligente Messsysteme (iMS)** processes:

| API | Parties | Purpose |
|---|---|---|
| `controlMeasures` | NB/LF ↔ MSB | Grid control commands (§ 14a EnWG) |
| `maloIdent` | LF ↔ NB | Marktlokations-Identifikation |
| `wimOrder` | MSB | iMS Universalbestellprozess (iMS Anmeldung, PIDs 11021–11023) |
| `directory` | All | `Verzeichnisdienst` — endpoint discovery via GLN |

---

## Module layout

```
energy_api
├── models/       OpenAPI/AsyncAPI types shared by all APIs
├── transport/    HTTP + mTLS builder, JWS sign/verify
├── directory/    Verzeichnisdienst — REST client, WebSocket client, server
├── client/       Electricity API clients  (feature = "client")
│   ├── control_measures   NB/LF and MSB send calls
│   └── malo_ident         LF and NB callback calls
└── server/       Electricity API servers  (feature = "server")
    ├── control_measures   MSB and NB/LF receive handlers + axum router
    ├── malo_ident         NB and LF receive handlers + axum router
    └── wim_order          MSB receive handler (iMS Anmeldung) + NB callbacks
```

---

## Feature flags

| Feature | Default | Enables |
|---|---|---|
| `client` | | HTTP clients for all APIs (reqwest + rustls) |
| `server` | | Axum router factories for server implementations |
| `websocket` | | WebSocket subscription client (tokio-tungstenite) |
| `crypto` | | JWS ECDSA-SHA256 sign/verify for directory records (p256) |

---

## Quick start

### Look up an endpoint via the Verzeichnisdienst

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

### Send a grid control command (§ 14a EnWG)

```rust,no_run
use energy_api::client::ControlMeasuresClient;
use energy_api::models::electricity::{
    CommandControl, LocationId, NeloId, MaximumPowerValue,
};
use url::Url;
use uuid::Uuid;

let client = ControlMeasuresClient::new(
    Url::parse("https://msb.example.de/")?,
    reqwest::Client::new(),
);
client.send_konfiguration(
    Uuid::new_v4(),
    "2025-06-01T10:00:00.000Z",
    &LocationId::NetworkLocation(NeloId("E1234848431".into())),
    &CommandControl {
        maximum_power_value: MaximumPowerValue("10.5".into()),
        execution_time_from: "2025-06-01T10:00:00Z".into(),
        execution_time_until: None,
    },
    None,
).await?;
```

### Mount the server in `makod` / Axum

```rust,no_run
use energy_api::server::{control_measures, wim_order};
use axum::Router;

let app = Router::new()
    .merge(control_measures::router(my_control_handler))
    .merge(wim_order::router(my_wim_handler));
```

---

## Identifiers

All BDEW identifiers are the validated types from `rubo4e::identifiers` —
`MaloId`, `MeloId`, `NeloId`, `SrId`, `TrId`, `MarktpartnerId` — not local
`String` newtypes.

`Deserialize` enforces the check digit, so a malformed identifier is rejected
**at the API boundary** rather than entering the identification path. MaLo-Ident
is the first binding API process in German MaKo (mandatory since 06.06.2025,
2-hour deadline) and a precondition for every supplier switch, so this is the
point where a bad ID would otherwise propagate into a switch.

`MarketPartnerId` is a string, not an `i64`: BDEW codes may carry leading zeros,
which an integer representation silently destroys.

Because the API layer and `mako-markt`'s domain layer now share these types, the
API→domain conversion in `makod`'s `api_bridge` is a variant remap with no
re-parsing.

### Wire contract

The `identificationParameterId` property names are pinned by a test against
`maloIdentV1.yaml` at tag `1.0.0`: `maloId`, `tranchenIds`, `meloIds`,
`meterNumbers`, `customerNumber`. Serde derives these from `rename_all =
"camelCase"`, and unknown properties are *ignored* on deserialization — so a
field rename in Rust would silently drop the value rather than error. Note
`tranchenIds` is mixed German/English: a tidier `tranche_ids` in Rust would
produce `trancheIds` and stop matching.

## Specification version

This crate implements **1.0.0**, the only tag in either spec repository.

Release **2.0.0** was put out for consultation by Mitteilung Nr. 55 for
01.10.2026, then **excluded** by Mitteilung Nr. 56: *"Die im Release 2.0.0 zur
Konsultation gestellten Anpassungen an den API-Webdiensten sind nicht Bestandteil
dieser Veröffentlichung."* Only API Guideline 1.0b binds on 01.10.2026. The 2.0.0
material exists only on the `2026-07-31-consultation` branch, which is still
moving; there is no `2.0.0` tag. See `spec_version::RELEASE_2_0_0_STATUS`.

Specs live in two **separate** repositories: `EDI-Energy/api-electricity` for the
electricity APIs and `EDI-Energy/api-directory-service` for the Verzeichnisdienst.

## Regulatory references

- **BDEW API-Webdienste Strom V1.3** — REST/JSON channel specification
- **API Guideline 1.0b** — binding from 01.10.2026
- **§ 14a EnWG** — statutory basis for controllable consumption devices (iMS grid control)
- **MsbG** — Messstellenbetriebsgesetz (smart meter rollout)
- **BNetzA BK6-24-174** — WiM process documentation (PIDs 11021–11023 via this channel)

---

## Related crates

| Crate | Role |
|---|---|
| `energy-api` ← **this crate** | REST/WebSocket client + Axum server |
| `mako-wim` | iMS process engine (WiM PIDs 11021–11023) |
| `mako-as4` | AS4 transport (parallel EDIFACT channel) |
| `makod` | Production daemon — mounts this crate's Axum routers |
| `edi-energy` | EDIFACT parsing (parallel EDIFACT channel) |
