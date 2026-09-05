# mako-markt

**Market data library for German energy market communication (MaKo).**

`mako-markt` is the domain library for `Marktlokation` (MaLo), `Messlokation` (MeLo),
`VersorgungsStatus`, NB network contracts, trading-partner, and process-correlation
management.  It is the foundation for [`marktd`](https://hupe1980.github.io/mako/docs/services/marktd/), the production
Market Data Hub.

Key design choices:
- **`mako_markt::bo4e` is the workspace's BO4E boundary** — the gate every
  payload crosses in either direction, the rules BO4E states but enforces
  nowhere, and the typed columns a stored document is indexed by. See
  [The BO4E gate](#the-bo4e-gate) below.
- **Writes take the typed BO, reads return the stored document.**
  `MaloRepository::upsert` takes `&rubo4e::current::Marktlokation` — never a
  `serde_json::Value` — so an unvalidated payload cannot reach storage and a
  shadow column cannot disagree with the document it shadows. The record types
  (`MaloRecord.data`, `MeloRecord.data`, `ZaehlerRecord.data`, …) hand the
  JSONB back verbatim, because a stored document must round-trip exactly as the
  counterparty sent it.
- **`NbContractRecord` carries full BO4E `Vertrag` JSONB** — `data: serde_json::Value`
  stores the canonical BO4E `Vertrag` payload alongside typed SQL columns
  (`netzebene`, `bilanzierungsmethode`, `billing_schedule`).  `vertragsart` and
  `vertragsstatus` are extracted as indexed columns for fast SQL filtering.
- **`rubo4e::current` is the BO vocabulary** — `Marktlokation`, `Messlokation`,
  `Zaehler`, `Geraet`, `Vertrag`, `Energiemenge`, `Lastgang`, `Rechnung` and the
  COMs and enums beneath them. BO4E schema version `202607.1.0`.

---

## The BO4E gate

BO4E's schema constrains almost nothing — of the 35 Geschäftsobjekte, two
declare a `required` field and none declares a `oneOf` — so "it deserialises" is
not validation. `mako_markt::bo4e::decode` is the four decisions that make up
accepting a BO4E document, in one place:

```rust
use mako_markt::bo4e;
use rubo4e::current::Marktlokation;

let malo: Marktlokation = bo4e::decode(payload)
    .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))?;
```

| Stage | Refuses | `code` |
|---|---|---|
| 1. Discriminator | a `Zaehler` posted to the `Geraet` endpoint | `bo4e.discriminator` |
| 2. Schema | a value the type cannot hold | `bo4e.schema` |
| 3. Strict enums | `"sparte": "STROMM"`, at any depth, by JSON-path | `bo4e.unknown_enum` |
| 4. BO4E rules | a document the standard's prose forbids | `bo4e.rule` |

Stage 3 carries most of the weight: `Unknown` serialises back as the literal
`"UNKNOWN"`, so at an endpoint that stores the canonical round-trip, skipping it
overwrites what the caller sent rather than merely accepting it.

| Variant | Use |
|---|---|
| `decode` | an endpoint. All four stages. |
| `decode_received` | a counterparty's document. Stages 1–3 refuse; a rule violation is returned alongside the value, to be disputed rather than dead-lettered. |
| `ensure_conformant` | outbound: stages 3 and 4 on a value mako built. |

Stage 4 is mostly `rubo4e`'s `.validate()`, which descends the whole tree;
`bo4e::conformance` adds the two rules BO4E states and `rubo4e` does not check.
Both report as `ValidationFailure { path, message }`. A rule belongs there
**only if BO4E asserts it** — endpoint requirements are a separate layer.

---

## Design principles

| Principle | Detail |
|---|---|
| **Stateless library** | No axum, no sqlx, no async runtime in this crate. All I/O lives in `services/marktd`. |
| **Validated domain identifiers** | [`MaloId`], [`MeloId`], and [`MarktpartnerId`] validate format and checksum at construction time — invalid IDs are rejected at the system boundary. |
| **Temporal role assignments** | `rollenzuordnung` entries carry `valid_from`/`valid_to` date ranges. Queries are always resolved against a reference date (German local time, CET/CEST). |
| **Generic `AppState`** | One generic type parameter per repository trait — fully static dispatch with no `dyn Trait` overhead. |
| **AFIT** | All repository traits use `async fn in trait` (stable since Rust 1.75, MSRV 1.94). |

---

## Crate structure

```
mako_markt
├── domain          Validated IDs (MaloId, MeloId, MarktpartnerId), Sparte, ProcessStatus
├── repository      Repository traits + AppState + record types + PageResult
│                   MaloRecord (data: Marktlokation JSONB, typed columns)
│                   MeloRecord (data: Messlokation JSONB, standorteigenschaften JSONB)
│                   NbContractRecord (data: Vertrag JSONB, vertragsart/vertragsstatus columns)
│                   ZaehlerRecord (data: Zaehler JSONB), GeraetRecord (data: Geraet JSONB)
│                   VersorgungsStatusRepository, LieferStatus, VersorgungsStatusRecord
│                   GrundversorgerRepository, GrundversorgerRecord (§36 Abs. 2 EnWG)
│                   PriCatRepository, PriCatVersion, PriCatDispatchState
├── error           MdmError — RFC 7807-ready with status_u16, error_code, error_title
├── bo4e            decode · decode_received · ensure_conformant · conformance rules
│                   shadow columns (MaloShadowColumns, …)
├── cloudevents     InboundMakoEvent, MarktEvent, EventExtensions
│                   Emitted: de.markt.malo.updated, de.markt.nb-contract.updated,
│                            de.markt.pricat.published, de.markt.versorgung.beliefert,
│                            and 17 more `de.markt.*` types
├── commands        typed makod command names/payloads
├── makod_client    HTTP client for the makod admin API (feature `makod-client`)
├── marktd_client   HTTP client for the marktd REST API (feature `marktd-client`)
└── testing         InMemory* test doubles (feature = "testing")
                    includes: InMemoryPriCatRepository
```

---

## Domain identifiers

### `MaloId` — 11-digit BDEW Marktlokations-ID

Validated with the BDEW alternating-weight check digit algorithm
(BDEW Identifikatoren AWH V1.2 §2.1):

```rust
use mako_markt::domain::MaloId;

let id = MaloId::new("51238696012")?;   // validates checksum
println!("{id}");                        // "51238696012"
```

### `MeloId` — 33-character Messlokations-ID

```rust
use mako_markt::domain::MeloId;

let id = MeloId::new("DE0001234567890123456789012345678")?;
```

### `MarktpartnerId` — 13-digit BDEW/DVGW/GS1 Codenummer

Derives the NAD DE3055 agency code from the prefix (`99…` → `293`, `98…` → `332`, other → `9`):

```rust
use mako_markt::domain::MarktpartnerId;

let mp_id = "9900357000004".parse::<MarktpartnerId>()?;
assert_eq!(mp_id.nad_agency_code(), "293");  // BDEW Strom
assert_eq!(mp_id.is_bdew(), true);
```

> **Note:** Only GS1-issued 13-digit codes are true GLNs (NAD DE3055 `9`).
> BDEW-Codenummern (`99…`, `293`) and DVGW-Codenummern (`98…`, `332`) are not GLNs.
> Use `MarktpartnerId` for all market-participant identifiers — never `String`.

---

## Repository traits

All traits use AFIT and return `Result<_, MdmError>`. Every trait has two implementations:

| Implementation | Use |
|---|---|
| `Pg*Repository` in `services/marktd/src/pg/` | Production — PostgreSQL via sqlx 0.8+ |
| `InMemory*` (behind the `testing` feature) | Unit tests — no PostgreSQL required |

### `VersorgungsStatusRepository`

Persists the current supply state for each MaLo.  Records are automatically derived
from `de.mako.process.completed` events by `marktd`'s EventBus pipeline.

```rust
pub trait VersorgungsStatusRepository: Send + Sync {
    /// Blind upsert (if_version = None) or optimistic update (if_version = Some(v)).
    /// Returns the new version on success.  Returns MdmError::VersionConflict
    /// if if_version is set and the stored version does not match.
    async fn upsert(
        &self,
        rec: VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError>;

    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError>;

    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: i64,
        size: i64,
    ) -> Result<PageResult<VersorgungsStatusRecord>, MdmError>;
}
```

**`LieferStatus` values:**

| Variant | Meaning |
|---|---|
| `Beliefert` | MaLo is actively supplied by a nominated LF |
| `Unbeliefert` | MaLo has no active supplier (Grundversorgungsfall) |
| `Grundversorgung` | §36 EnWG basic supply is active |
| `Ersatzversorgung` | §38 EnWG emergency supply is active (max 3 months) |
| `Ruhend` | Supply suspended, MaLo registered but dormant |
| `Stillgelegt` | MaLo decommissioned |

---

```rust
pub trait MaloRepository: Send + Sync {
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: &rubo4e::current::Marktlokation,   // the typed BO, never a Value
        rollenzuordnung: Vec<Rollenzuordnung>,
        if_match: Option<i64>,                   // ETag optimistic-concurrency guard
        bo4e_version: &str,
    ) -> Result<i64, MdmError>;                  // → new version

    async fn find(&self, malo_id: &MaloId, at: Date)
        -> Result<Option<MaloRecord>, MdmError>;

    async fn list(&self, filter: MaloFilter, at: Date)
        -> Result<PageResult<MaloRecord>, MdmError>;
}
```

The `at: Date` parameter is always the current German local date (CET/CEST) so that
`rollenzuordnung` validity is evaluated against the correct calendar date, not UTC.

### Temporal `Rollenzuordnung`

```rust
pub struct Rollenzuordnung {
    pub zuordnungstyp:    String,       // "NB", "LF", "MSB", …
    pub rollencodenummer: String,       // 13-digit GLN
    pub valid_from:       Date,
    pub valid_to:         Option<Date>, // None = currently valid
}
```

Each `MaloRecord` carries only the assignments valid at the requested reference date.
The storage layer uses a `LEFT JOIN … AND valid_from <= $at AND (valid_to IS NULL OR valid_to >= $at)`.

---

## CloudEvents

Outbound events emitted by `marktd` conform to **CloudEvents 1.0** structured-mode JSON
(`application/cloudevents+json`). They carry `markt*` extension attributes and are
HMAC-SHA256 signed for delivery to ERP subscribers.

HMAC-SHA256 signing and verification are **not** in this crate: they live in
`mako_service::webhook` (`sign` / `verify_hmac`), the one canonical
implementation shared by every emitter and verifier.

```rust
use mako_markt::cloudevents::{MarktEvent, EventExtensions};

let event = MarktEvent::new(
    "9900357000004",             // tenant GLN
    "de.markt.malo.updated",     // CloudEvents type
    "51238696012",               // subject (MaLo-ID)
    serde_json::json!({ "_typ": "MARKTLOKATION", … }),
)
.with_extensions(EventExtensions {
    marktmaloid: Some("51238696012".into()),
    marktrole:   Some("NB".into()),
    ..Default::default()
});

let body = serde_json::to_vec(&event)?;
// sign with mako_service::webhook::sign(secret, &body) and send the result
// in the `X-Markt-Signature` header
```

**Event source:** `urn:mako:marktd:tenant:{tenant}`

---

## Error handling

`MdmError` is a `thiserror`-derived enum. Every variant maps to a stable HTTP status,
machine-readable `error_code`, and a human-readable `error_title` for RFC 7807 Problem
Details responses:

| Variant | Status | Code |
|---|---|---|
| `InvalidMaloId` | 422 | `invalid_malo_id` |
| `InvalidMeloId` | 422 | `invalid_melo_id` |
| `InvalidMpId` | 422 | `invalid_gln` |
| `NotFound` | 404 | `not_found` |
| `VersionConflict` | 412 | `version_conflict` |
| `MakodConflict` | 409 | `makod_conflict` |
| `Forbidden` | 403 | `forbidden` |
| `Unprocessable` | 422 | `unprocessable` |
| `MakodSync` | 500 | `makod_sync_failed` |
| `WebhookDelivery` | 500 | `webhook_delivery_failed` |
| `Internal` | 500 | `internal_error` |

---

## Testing

Enable the `testing` feature to get `InMemory*` test doubles for every repository trait:

```toml
[dev-dependencies]
mako-markt = { path = "../../crates/mako-markt", features = ["testing"] }
```

```rust
use mako_markt::{
    domain::{MaloId, Sparte},
    repository::AppState,
    testing::{InMemoryMaloRepository, InMemoryMeloRepository, …},
};
use std::sync::Arc;

let state = Arc::new(AppState {
    malo_repo: InMemoryMaloRepository::default(),
    // … other repos …
});
```

---

## Feature flags

| Flag | Enables |
|---|---|
| *(default)* | All domain types, repository traits, CloudEvents |
| `testing` | `InMemory*` test doubles — **never enable in production builds** |
| `makod-client` | HTTP client for the makod admin API (`reqwest`) |
| `marktd-client` | HTTP client for the marktd REST API (`reqwest`) |

---

## `PriCatRepository` — PRICAT 27003 version history and dispatch

`PriCatRepository` stores versioned PRICAT snapshots and an audit log of every
outbound dispatch attempt.

Every `PUT /api/v1/preisblaetter/{nb_mp_id}` call in `marktd`:
1. Writes to `preisblaetter` (existing single-row store for `invoicd`)
2. Inserts a versioned snapshot in `pricat_versions`
3. Emits `de.markt.pricat.published` via the internal event channel

```rust
pub trait PriCatRepository: Send + Sync {
    async fn upsert_version(
        &self,
        nb_mp_id: &str,
        tenant: &str,
        valid_from: time::Date,
        valid_to: Option<time::Date>,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<uuid::Uuid, MdmError>;

    async fn find_latest(&self, nb_mp_id: &str, tenant: &str)
        -> Result<Option<PriCatVersion>, MdmError>;

    async fn list_versions(&self, nb_mp_id: &str, tenant: &str)
        -> Result<Vec<PriCatVersion>, MdmError>;

    async fn list_pending(&self, tenant: &str)
        -> Result<Vec<PriCatVersion>, MdmError>;

    async fn mark_queued(&self, id: uuid::Uuid) -> Result<(), MdmError>;
    async fn mark_done(&self, id: uuid::Uuid) -> Result<(), MdmError>;
    async fn mark_error(&self, id: uuid::Uuid, error: &str) -> Result<(), MdmError>;

    async fn log_dispatch(&self, entry: PriCatDispatchEntry) -> Result<(), MdmError>;
    async fn dispatch_log(&self, pricat_version_id: uuid::Uuid)
        -> Result<Vec<PriCatDispatchEntry>, MdmError>;
}
```

**Dispatch states:**

| State | Meaning |
|---|---|
| `Pending` | Stored; dispatch not yet started |
| `Queued` | Dispatch task picked this version up |
| `Done` | All active LF partners successfully reached |
| `Error` | Last attempt failed; retried on next background scan |

**Auto-dispatch on LF partner registration:** when a new LF partner is upserted
via `PUT /api/v1/partners/{mp_id}` in `marktd`, the latest PRICAT version for the
operator's NB GLN is automatically re-queued for dispatch to the new partner.

---

## Relationship to `makod` and `marktd`

```
┌─────────────────────────────────────────────────┐
│  services/marktd  (binary)                        │
│  axum 0.8+ · sqlx 0.8+ · utoipa 5 · jiff 0.2     │
│  Pg*Repository  ←─── implements traits ─────┐  │
│  fan-out worker ←─── event_log outbox +     │  │
│                      event_delivery ledger  │  │
│  OIDC/JWT auth  ←─── Cedar not used here    │  │
└───────────────────────────┬─────────────────┘  │
                            │ uses               │
┌───────────────────────────▼─────────────────┐  │
│  crates/mako-markt  (library — this crate)    │  │
│  domain · repository traits · AppState      │  │
│  error · cloudevents · makod_client         │  │
│  testing (feature = "testing")              │  │
└─────────────────────────────────────────────┘
            │ makes HTTP calls to
┌───────────▼─────────────────────────────────┐
│  services/makod  (production daemon)        │
│  EDIFACT · AS4 · event-sourced workflows    │
└─────────────────────────────────────────────┘
```

`mako-markt` depends on neither `axum` nor `sqlx`. Both are confined to `services/marktd`.
This keeps the library independently testable with zero framework overhead.

## Related crates

| Crate | Role |
|---|---|
| [`mako-markt`](https://docs.rs/mako-markt) ← **this crate** | Domain types, repository traits, the BO4E gate, CloudEvents emission |
| [`energy-api`](https://docs.rs/energy-api) | The REST/WebSocket API-Webdienste channel, which addresses partners by the same BDEW Codenummer |
| [`mako-events`](https://docs.rs/mako-events) | CloudEvents `type` catalog — the shared event vocabulary |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-pruefung`](https://docs.rs/mako-pruefung) | Reads Marktlokations- and Vertragsstammdaten to decide an Antwortcode |
| [`grid-billing`](https://docs.rs/grid-billing) · [`energy-billing`](https://docs.rs/energy-billing) · [`eeg-billing`](https://docs.rs/eeg-billing) | Bill against these stammdaten |
| [`marktd`](https://hupe1980.github.io/mako/docs/services/marktd/) | Production daemon — the PostgreSQL implementation of the repository traits |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
