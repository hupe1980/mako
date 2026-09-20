# energy-api

**BDEW API-Webdienste Strom — REST/WebSocket client and Axum server bindings.**

Implements the German energy market **API-Webdienste Strom** — the REST/JSON
transport that runs alongside the EDIFACT/AS4 channel — providing typed REST and
WebSocket clients for iMS grid control processes and a matching Axum server for
hosting the receiving endpoints.

Two document families govern it and they version separately: the **OpenAPI /
AsyncAPI specs** (all at `1.0.0`, see [Specification version](#specification-version))
and the BDEW **API-Guideline** (`1.0a` since 06.06.2025, `1.0b` binding from
01.10.2026) with **Regelungen zum Übertragungsweg für API-Webdienste 1.2**.

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

TR-03116-3 content-layer signing (`DIGEST` / `SIGNATURE` over RFC 8785 canonical
JSON) and JWS verification are **not** features — they are always compiled.
Behind a flag they could be absent at the one moment they matter, and the
absence was silent: with the flag off `with_signing` did not exist, and a
request went out unsigned with no error and no warning. Whether a request is
signed is now one question with one answer — whether a signing key is configured
on the client.


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
    &LocationId::NetworkLocation(NeloId::new("E1234848431")?),
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
`MaloId`, `MeloId`, `NeloId`, `SrId`, `TrId` and `MarktpartnerId` (re-exported
here as `MarketPartnerId`) — not local `String` newtypes.

`Deserialize` enforces the check digit, so a malformed identifier is rejected
**at the API boundary** rather than entering the identification path. MaLo-Ident
is the first binding API process in German MaKo (mandatory since 06.06.2025 with
API-Guideline 1.0a, 2-hour deadline) and a precondition for every supplier
switch, so this is the
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

This crate implements **1.0.0**.

Release **2.0.0** was put out for consultation by Mitteilung Nr. 55 for
01.10.2026, then **excluded** by Mitteilung Nr. 56: *"Die im Release 2.0.0 zur
Konsultation gestellten Anpassungen an den API-Webdiensten sind nicht Bestandteil
dieser Veröffentlichung."* Only API Guideline 1.0b binds on 01.10.2026.

`EDI-Energy/api-electricity` carries a **`2.0.0` tag**, released 24.07.2026, and
its own Änderungshistorie applies it *"ab dem 01.10.2027 00:00 Uhr"* — one
format cycle after FV 2027-04, so it is not part of that import either. The
directory-service repository still has only `1.0.0`.
See `spec_version::RELEASE_2_0_0_STATUS`.

Specs live in two **separate** repositories: `EDI-Energy/api-electricity` for the
electricity APIs and `EDI-Energy/api-directory-service` for the Verzeichnisdienst.

## Regulatory references

- **API-Guideline 1.0a / 1.0b** — the BDEW rules for the REST/JSON channel;
  1.0a since 06.06.2025, 1.0b binding from 01.10.2026
- **Regelungen zum Übertragungsweg für API-Webdienste 1.2** — mTLS and the
  EMT.API certificate requirements
- **§ 14a EnWG** — statutory basis for controllable consumption devices (iMS grid control)
- **MsbG** — Messstellenbetriebsgesetz (smart meter rollout)
- **BNetzA BK6-22-024** — the Festlegung behind both processes this channel
  carries: MaLo-Ident for the 24-h Lieferantenwechsel, and WiM
  (Messstellenbetrieb) for the iMS Universalbestellprozess, PIDs 11021–11023

---

## Related crates

| Crate | Role |
|---|---|
| [`energy-api`](https://docs.rs/energy-api) ← **this crate** | REST/WebSocket client + Axum server for the API-Webdienste |
| [`mako-wim`](https://docs.rs/mako-wim) | The iMS process engine behind the WiM PIDs this API serves |
| [`mako-as4`](https://docs.rs/mako-as4) | The parallel EDIFACT channel's transport |
| [`edi-energy`](https://docs.rs/edi-energy) | The parallel EDIFACT channel's format layer |
| [`mako-markt`](https://docs.rs/mako-markt) | Marktstammdaten — Marktlokation, Messlokation, Marktpartner, Rollenzuordnung |
| [`makod`](https://hupe1980.github.io/mako/docs/services/makod/) | Production daemon — mounts this crate's Axum routers |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
