# mako-obs

**Business-process observability library — process projections, KPI computation, and BNetzA regulatory reports.**

`mako-obs` defines the domain types and repository trait used by the
[`obsd`](https://hupe1980.github.io/mako/docs/services/obsd/) daemon. The library itself has no I/O; persistence
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
    pub deadline_source: Option<String>,
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
| `de.mako.process.failed`     | `Failed`         |

`ProcessState::is_terminal()` returns `true` for `Completed`, `Rejected` and
`Failed` — states that will receive no further events. `AperakTimeout` is
deliberately **not** terminal: a counterparty that missed the acknowledgement
window can still answer the business message, and the process then completes
normally. The unsuccessful endings (`Rejected`, `Failed`) are
`is_unsuccessful_ending()`, which is the STP denominator.

The name is `Failed`, not `Cancelled`: nothing in mako emits a cancellation, and
that name would file unrecoverable failures in a bucket the STP rate reads as a
normal ending.

### `DeadlineRisk`

An enum classifying how close a live process is to its regulatory deadline,
computed by `DeadlineRisk::classify(deadline, now)`:

```rust
pub enum DeadlineRisk {
    Unknown, // no Antwortfrist is published for this PID, so no risk is stateable
    Green,   // more than AMBER_HOURS (24) before the deadline
    Amber,   // less than AMBER_HOURS before the deadline
    Red,     // the deadline has passed and the process is still open
}
```

A process with no `deadline_at` is `Unknown`, never `Green`: "we have not read
that Festlegung" and "there is time" are different statements.
`DeadlineRisk::classify_opt` is the `Option`-taking form. The 24 h in
`AMBER_HOURS` is an operating convention shared with obsd's sweep window — no
Festlegung states it.

`obsd` re-evaluates `DeadlineRisk` on its deadline sweep and emits a
`de.obs.deadline.approaching` CloudEvent for processes inside the warn window.

---

## `KpiReport`

Process KPIs for **one PID** over one calendar period, bucketed by `started_at`.

**The two deadline clocks are two fields.** `total_aperak_timeout` is the
*technical acknowledgement* (45 min Strom weekday; Gas next Werktag 12:00 or
3 Werktage); `total_frist_breached` is the *business Antwortfrist*. They differ
by orders of magnitude and fail for different reasons, so reporting one under the
other's name points an operator at the wrong problem.

```rust
pub struct KpiReport {
    pub pid: u32,
    pub period_from: Date,
    pub period_to: Date,
    pub total_initiated: u64,
    pub total_completed: u64,
    pub total_rejected: u64,
    pub total_failed: u64,
    /// The technical acknowledgement clock.
    pub total_aperak_timeout: u64,
    /// The business Antwortfrist.
    pub total_frist_breached: u64,
    /// Denominator of the rate below. Reported because a small one means the
    /// bucket is mostly *unmeasured*, which a rate near 1.0 would hide.
    pub total_with_frist: u64,
    /// `None` when nothing in the bucket carries a published Frist.
    pub frist_compliance_rate: Option<f64>,
    /// `None` until something in the bucket closes — never a measured 0 hours.
    pub avg_cycle_time_hours: Option<f64>,
    pub p95_cycle_time_hours: Option<f64>,
}
```

Every rate is `Option`, in the type rather than patched at the edge: a `null`
means nothing measurable in the bucket, and a `0.0` placeholder reads as
"we completed none of them".

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
`family`, `partner_mp_id`, `mdm_role`, `since`, `tenant`, and a `limit`
defaulting to 100).

Errors are reported through `ObsError` (`Database`, `NotFound`, `NoKpiData`,
`Internal`).

---

## § 7a Abs. 5 EnWG Gleichbehandlung parity

`ProcessProjection::initiator_is_affiliate` flags whether the Lieferant on a
process belongs to the operator's own vertically integrated undertaking. `obsd`
sets it over the processes the **network arm answers for a Lieferant** — the set
derived from the Antwortfrist table, not a literal PID list.

`ParityComparison` owns the comparison and its sign convention:
**`gap_pp = affiliate − third_party`, in percentage points; positive means the
affiliate fared better**, which is the concern. It is `None` when either group is
below `PARITY_MIN_SAMPLE` — an *unstatable* gap, not a zero one and not a
hundred-point one off a single process.

On its parity sweep `obsd` emits `de.obs.stp.parity.alert`
(`mako_events::obs::STP_PARITY_ALERT`) when the gap passes the configured
`parity_threshold_pp`. **That threshold is the operator's own escalation
policy.** The Bundesnetzagentur publishes no numeric parity limit for this
figure; § 7a Abs. 5 asks the Gleichbehandlungsbeauftragte to describe the
measures taken, and the report is filed by 31 March for the preceding calendar
year.

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

- **§ 20 Abs. 1 Satz 1 EnWG** — diskriminierungsfreier Netzzugang
- **§ 6a EnWG** — informatorische Entflechtung
- **§ 7a Abs. 5 EnWG** — Gleichbehandlungsprogramm and the Gleichbehandlungs­bericht
  the Gleichbehandlungsbeauftragte files by 31 March for the preceding calendar year
- **BK6-24-174** — GPKE Strom process framework and Fristen (Teil 2 states the
  answer windows as clock times on the 1. Werktag nach dem Übertragungstag)
- **BK6-22-024** — WiM (Messstellenbetrieb), Anlage 2a; the WiM Fristen for
  **both** Sparten, never BK6-24-174
- **BK7-24-01-009** — GeLi Gas 3.0 process framework and Fristen (WiM Gas adds
  AWH WiM Gas V2.0)

Deadlines themselves live in `mako-fristen`, not here: this crate holds the
read-model and the report shapes.

## Related crates

| Crate | Role |
|---|---|
| [`mako-obs`](https://docs.rs/mako-obs) ← **this crate** | `ProcessProjection`, `KpiReport`, the repository trait, the § 7a Abs. 5 EnWG report shapes |
| [`mako-events`](https://docs.rs/mako-events) | CloudEvents `type` catalog — the shared event vocabulary |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-engine`](https://docs.rs/mako-engine) | Event-sourced workflow runtime — `Workflow`, `Process`, `EventStore`, deadlines |
| [`obsd`](https://hupe1980.github.io/mako/docs/services/obsd/) | Production daemon — the PostgreSQL implementation of the repository trait |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
