# mako-obs

**Business-process observability library — process projections, KPI computation, and BNetzA regulatory reports.**

`mako-obs` defines the domain types and repository traits used by the
[`obsd`](../../services/obsd/) daemon. The library itself has no I/O; persistence
is implemented in `obsd` via PostgreSQL.

---

## Core types

### `ProcessProjection`

A read-model snapshot of one running MaKo process:

```rust
pub struct ProcessProjection {
    pub process_id: ProcessId,
    pub workflow_name: String,
    pub pid: u32,
    pub tenant: String,
    pub state: ProcessState,
    pub started_at: OffsetDateTime,
    pub deadline_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub last_event_at: OffsetDateTime,
    pub event_count: u32,
}
```

`ProcessState` variants: `Running`, `WaitingForAperak`, `WaitingForResponse`,
`Completed`, `Cancelled`, `Escalated`, `TimedOut`.

### `DeadlineRisk`

Identifies processes at risk of missing their regulatory deadline:

```rust
pub struct DeadlineRisk {
    pub process_id: ProcessId,
    pub workflow_name: String,
    pub pid: u32,
    pub deadline_at: OffsetDateTime,
    pub risk_level: RiskLevel,   // Critical | High | Medium
    pub minutes_remaining: i64,
}
```

`obsd` evaluates `DeadlineRisk` on a schedule and pushes alerts to Alertmanager.

---

## `KpiReport`

Monthly BNetzA KPI report aggregated across all process types:

```rust
pub struct KpiReport {
    pub period: YearMonth,
    pub pid_stats: Vec<PidKpiStats>,
    pub stp_rate: Decimal,             // § 20 EnWG parity
    pub aperak_p95_minutes: Decimal,   // 45-min compliance
    pub escalation_count: u32,
}
```

`PidKpiStats` breaks down acceptance rate, median processing time, and
escalation count per `pid`.

---

## Repository traits

### `ProcessProjectionRepository`

```rust
#[async_trait]
pub trait ProcessProjectionRepository: Send + Sync {
    async fn upsert(&self, proj: &ProcessProjection) -> Result<(), ObsError>;
    async fn get(&self, process_id: &ProcessId)
        -> Result<Option<ProcessProjection>, ObsError>;
    async fn list_at_risk(&self, now: OffsetDateTime, lookahead_minutes: u64)
        -> Result<Vec<DeadlineRisk>, ObsError>;
    async fn compute_kpi_report(&self, period: YearMonth)
        -> Result<KpiReport, ObsError>;
}
```

---

## §20 EnWG parity monitoring

`stp_rate` in `KpiReport` is the fraction of Anmeldung processes that received
an automatic STP decision (no manual override). The BNetzA expects this to be
≥ 95 % for NB operators subject to §20 EnWG non-discrimination obligations.
`obsd` emits a `de.obs.stp.parity.alert` CloudEvent when it drops below threshold.

---

## Testing feature

Enable `testing` to use in-memory implementations:

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
- **BK6-24-174 §7** — BNetzA KPI reporting obligations for GPKE/WiM
- **APERAK AHB 1.0 §2.4.1** — 45-minute APERAK deadline (Strom UTILMD/ORDERS)
