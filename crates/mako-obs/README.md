# mako-obs

**Business-process observability library — process projections, KPI computation, and BNetzA regulatory reports.**

`mako-obs` defines the domain types and repository trait used by the
[`obsd`](../../services/obsd/) daemon. The library itself has no I/O; persistence
is implemented in `obsd` via PostgreSQL.

---

## Core types

### `ProcessProjection`

A per-process read-model row, one per live or recently completed MaKo process,
updated on every `de.mako.*` event received by `obsd`:

```rust
pub struct ProcessProjection {
    pub process_id: Uuid,
    pub pid: u32,
    pub family: String,
    pub workflow_name: String,
    pub state: ProcessState,
    pub malo_id: Option<String>,
    pub partner_mp_id: Option<String>,
    pub mdm_role: Option<String>,
    pub deadline_at: Option<OffsetDateTime>,
    pub deadline_risk: DeadlineRisk,
    pub started_at: OffsetDateTime,
    pub last_event_at: OffsetDateTime,
    pub erc_code: Option<String>,
    pub initiator_is_affiliate: bool,
    pub tenant: String,
}
```

### `ProcessState`

Lifecycle state of a MaKo process, derived from the originating CloudEvent type
via `ProcessState::from_ce_type`:

| `ce_type`                    | `ProcessState`   |
|------------------------------|------------------|
| `de.mako.process.initiated`  | `Initiated`      |
| `de.mako.aperak.accepted`    | `Running`        |
| `de.mako.aperak.rejected`    | `Rejected` + ERC |
| `de.mako.aperak.timeout`     | `AperakTimeout`  |
| `de.mako.process.completed`  | `Completed`      |
| `de.mako.process.failed`     | `Cancelled`      |

`ProcessState::is_terminal()` returns `true` for `Completed`, `Rejected`, and
`Cancelled` — states that will receive no further events.

### `DeadlineRisk`

An enum classifying how close a live process is to its regulatory deadline,
computed by `DeadlineRisk::classify(deadline, now)`:

```rust
pub enum DeadlineRisk {
    Green, // more than 24 h before deadline
    Amber, // less than 24 h before deadline
    Red,   // deadline has passed and process is still open
}
```

`obsd` re-evaluates `DeadlineRisk` on its deadline sweep and emits a
`de.obs.deadline.approaching` CloudEvent for processes inside the warn window.

---

## `KpiReport`

Regulatory KPI report for **one PID** over one calendar period, suitable for
BNetzA voluntary reporting and §4a MsbG monitoring:

```rust
pub struct KpiReport {
    pub pid: u32,
    pub period_from: Date,
    pub period_to: Date,
    pub total_initiated: u64,
    pub total_completed: u64,
    pub total_rejected: u64,
    pub total_aperak_timeout: u64,
    pub total_cancelled: u64,
    pub aperak_compliance_rate: f64, // (total_initiated - total_aperak_timeout) / total_initiated
    pub avg_cycle_time_hours: f64,   // mean initiated → completed/rejected
    pub p95_cycle_time_hours: f64,   // 95th percentile cycle time
}
```

---

## Repository trait

### `ProcessProjectionRepository`

The trait uses `async fn` in traits directly (`#![allow(async_fn_in_trait)]`) —
there is no `#[async_trait]` macro:

```rust
pub trait ProcessProjectionRepository: Send + Sync + 'static {
    async fn upsert(&self, p: &ProcessProjection) -> Result<(), ObsError>;
    async fn query(&self, q: &ObsQuery) -> Result<Vec<ProcessProjection>, ObsError>;
    async fn get(&self, process_id: Uuid) -> Result<Option<ProcessProjection>, ObsError>;
    async fn kpi_report(
        &self,
        pid: u32,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<KpiReport, ObsError>;
    async fn overdue_processes(
        &self,
        now: OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<ProcessProjection>, ObsError>;
}
```

`upsert` is idempotent: re-applying the same event is safe, and updates only
advance a projection when the incoming event carries a later timestamp.
`kpi_report` and `overdue_processes` filter to the given operator `tenant`
(MP-ID / GLN). Queries are expressed with `ObsQuery` (filters on `state`, `pid`,
`partner_mp_id`, `mdm_role`, `since`, `tenant`, and a `limit` defaulting to 100).

Errors are reported through `ObsError` (`Database`, `NotFound`, `NoKpiData`,
`Internal`).

---

## §20 EnWG parity monitoring

`ProcessProjection::initiator_is_affiliate` flags whether the initiating LF
MP-ID equals the operator's own MP-ID (vertically integrated utility
deployment). `obsd` sets it on `de.mako.process.initiated` for Lieferbeginn PIDs
by comparing the event's new-supplier MP-ID to the configured `own_mp_id`.

On its parity sweep, `obsd` compares the completion-rate of affiliate-initiated
Anmeldungen against non-affiliate-initiated ones. When the **gap** exceeds the
configured `parity_threshold_pp` (default `5.0` percentage points; see
`services/obsd/src/config.rs`), it emits a `de.obs.stp.parity.alert` CloudEvent
(`mako_events::obs::STP_PARITY_ALERT`) for the BNetzA §20 EnWG
Diskriminierungsfreiheitspflicht audit trail. This is a gap-based comparison, not
an absolute rate threshold.

The deadline sweep emits `de.obs.deadline.approaching`
(`mako_events::obs::DEADLINE_APPROACHING`) per process inside the warn window.

---

## Testing feature

Enable `testing` to use the in-memory implementation:

```toml
[dev-dependencies]
mako-obs = { path = "../crates/mako-obs", features = ["testing"] }
```

```rust
use mako_obs::testing::InMemoryProcessProjectionRepository;
```

Never enable `testing` in production builds.

---

## Regulatory basis

- **§20 EnWG** — Nichtdiskriminierungsgebot (non-discrimination mandate)
- **BK6-24-174** — GPKE / WiM Strom process framework and deadlines
- **BK7-24-01-009** — GeLi Gas / WiM Gas process framework and deadlines
