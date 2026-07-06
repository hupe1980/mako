# mako-mdm

**Master data library for German energy market communication (MaKo/MDM).**

`mako-mdm` is the domain library for `Marktlokation` (MaLo), `Messlokation` (MeLo),
contract, subscription, trading-partner, and process-correlation management. It is the
foundation for [`mdmd`](../../services/mdmd), the production Master Data Manager daemon.

---

## Design principles

| Principle | Detail |
|---|---|
| **Stateless library** | No axum, no sqlx, no async runtime in this crate. All I/O lives in `services/mdmd`. |
| **Validated domain identifiers** | [`MaloId`], [`MeloId`], and [`Gln`] validate format and checksum at construction time — invalid IDs are rejected at the system boundary. |
| **Temporal role assignments** | `lokationszuordnung` entries carry `valid_from`/`valid_to` date ranges. Queries are always resolved against a reference date (German local time, CET/CEST). |
| **Generic `AppState`** | Six generic type parameters — one per repository trait — enable fully static dispatch with no `dyn Trait` overhead. |
| **AFIT** | All repository traits use `async fn in trait` (stable since Rust 1.75, MSRV 1.89). |

---

## Crate structure

```
mako_mdm
├── domain          Validated IDs (MaloId, MeloId, Gln), Sparte, ProcessStatus
├── repository      Repository traits + AppState + record types + PageResult
├── error           MdmError — RFC 7807-ready with status_u16, error_code, error_title
├── cloudevents     InboundMakoEvent, MdmEvent, HMAC-SHA256 signing/verification
├── makod_client    HTTP client for the makod admin API
└── testing         InMemory* test doubles (feature = "testing")
```

---

## Domain identifiers

### `MaloId` — 11-digit BDEW Marktlokations-ID

Validated with the BDEW alternating-weight check digit algorithm
(BDEW Identifikatoren AWH V1.2 §2.1):

```rust
use mako_mdm::domain::MaloId;

let id = MaloId::new("51238696780")?;   // validates checksum
println!("{id}");                        // "51238696780"
```

### `MeloId` — 33-character Messlokations-ID

```rust
use mako_mdm::domain::MeloId;

let id = MeloId::new("DE0001234567890123456789012345678")?;
```

### `Gln` — 13-digit BDEW/DVGW/GS1 Codenummer

Derives the NAD DE3055 agency code from the prefix:

```rust
use mako_mdm::domain::Gln;

let gln = Gln::new("9900357000004")?;
assert_eq!(gln.nad_agency_code(), "293");  // BDEW Strom
```

---

## Repository traits

All traits use AFIT and return `Result<_, MdmError>`. Every trait has two implementations:

| Implementation | Use |
|---|---|
| `Pg*Repository` in `services/mdmd/src/pg/` | Production — PostgreSQL via sqlx 0.8 |
| `InMemory*` in `testing.rs` (feature = `testing`) | Tests — in-process, no DB required |

### `MaloRepository`

```rust
pub trait MaloRepository: Send + Sync {
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: MaloPayload,
        lokationszuordnung: Vec<Lokationszuordnung>,
        if_match: Option<i64>,          // ETag optimistic-concurrency guard
    ) -> Result<i64, MdmError>;         // → new version

    async fn find(&self, malo_id: &MaloId, at: Date)
        -> Result<Option<MaloRecord>, MdmError>;

    async fn list(&self, filter: MaloFilter, at: Date)
        -> Result<PageResult<MaloRecord>, MdmError>;
}
```

The `at: Date` parameter is always the current German local date (CET/CEST) so that
`lokationszuordnung` validity is evaluated against the correct calendar date, not UTC.

### Temporal `Lokationszuordnung`

```rust
pub struct Lokationszuordnung {
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

Outbound events emitted by `mdmd` conform to **CloudEvents 1.0** structured-mode JSON
(`application/cloudevents+json`). They carry mako-specific extension attributes and
are HMAC-SHA256 signed for delivery to ERP subscribers.

```rust
use mako_mdm::cloudevents::{MdmEvent, EventExtensions, compute_signature};

let event = MdmEvent::new(
    "9900357000004",             // tenant GLN
    "de.mdm.malo.updated",       // CloudEvents type
    "51238696780",               // subject (MaLo-ID)
    serde_json::json!({ "_typ": "MARKTLOKATION", … }),
)
.with_extensions(EventExtensions {
    mdmmaloid: Some("51238696780".into()),
    mdmrole:   Some("NB".into()),
    ..Default::default()
});

let body   = serde_json::to_vec(&event)?;
let sig    = compute_signature(secret.as_bytes(), &body);
// send with `X-Mdm-Signature: {sig}` header
```

---

## Error handling

`MdmError` is a `thiserror`-derived enum. Every variant maps to a stable HTTP status,
machine-readable `error_code`, and a human-readable `error_title` for RFC 7807 Problem
Details responses:

| Variant | Status | Code |
|---|---|---|
| `InvalidMaloId` | 422 | `invalid_malo_id` |
| `InvalidMeloId` | 422 | `invalid_melo_id` |
| `InvalidGln` | 422 | `invalid_gln` |
| `NotFound` | 404 | `not_found` |
| `VersionConflict` | 412 | `version_conflict` |
| `Forbidden` | 403 | `forbidden` |
| `Unprocessable` | 422 | `unprocessable` |
| `Internal` | 500 | `internal_error` |

---

## Testing

Enable the `testing` feature to get `InMemory*` test doubles for every repository trait:

```toml
[dev-dependencies]
mako-mdm = { path = "../../crates/mako-mdm", features = ["testing"] }
```

```rust
use mako_mdm::{
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
| *(default)* | All domain types, traits, CloudEvents, makod client |
| `testing` | `InMemory*` test doubles — **never enable in production builds** |

---

## Relationship to `makod` and `mdmd`

```
┌─────────────────────────────────────────────────┐
│  services/mdmd  (binary)                        │
│  axum 0.8 · sqlx 0.8 · utoipa 5 · jiff 0.2     │
│  Pg*Repository  ←─── implements traits ─────┐  │
│  fanout worker  ←─── mpsc events channel    │  │
│  OIDC/JWT auth  ←─── Cedar not used here    │  │
└───────────────────────────┬─────────────────┘  │
                            │ uses               │
┌───────────────────────────▼─────────────────┐  │
│  crates/mako-mdm  (library — this crate)    │  │
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

`mako-mdm` depends on neither `axum` nor `sqlx`. Both are confined to `services/mdmd`.
This keeps the library independently testable with zero framework overhead.
