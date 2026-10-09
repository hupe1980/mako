# mako-obs

[![Crates.io](https://img.shields.io/crates/v/mako-obs?label=mako-obs&color=f59e0b&logo=rust)](https://crates.io/crates/mako-obs)

Business-process observability for German market communication: the
`ProcessProjection` read-model, deadline risk, per-PID KPI reports and the
§ 7a Abs. 5 EnWG Gleichbehandlung parity comparison. No I/O — the repository
trait is implemented over PostgreSQL by
[`obsd`](https://hupe1980.github.io/mako/docs/services/obsd/). Deadlines
themselves come from `mako-fristen`.

## Example

```rust
use mako_obs::domain::{DeadlineRisk, ProcessState};
use time::macros::datetime;

let state = ProcessState::from_ce_type("de.mako.aperak.timeout").expect("projected type");
assert!(!state.is_terminal()); // a missed APERAK still owes the business answer

let now = datetime!(2026-10-09 10:00 UTC);
assert_eq!(DeadlineRisk::classify(datetime!(2026-10-09 20:00 UTC), now), DeadlineRisk::Amber);
assert_eq!(DeadlineRisk::classify_opt(None, now), DeadlineRisk::Unknown); // never Green
```

## Key types

| Item | Purpose |
|---|---|
| `ProcessProjection` | one row per process: PID, family, state, MaLo, partner, `deadline_at` + `deadline_source`, `deadline_risk`, `initiator_is_affiliate` |
| `ProcessState` | derived from the CloudEvent type (below); `is_terminal()`, `is_unsuccessful_ending()` |
| `DeadlineRisk` | `Unknown` (no published Frist) · `Green` · `Amber` (< `AMBER_HOURS`, an operating convention) · `Red` |
| `KpiReport` | one PID over one period: `total_aperak_timeout` and `total_frist_breached` as separate clocks; every rate is `Option` |
| `ParityComparison` | `gap_pp = affiliate − third_party`; `None` below `PARITY_MIN_SAMPLE`. No BNetzA threshold exists |
| `ProcessProjectionRepository` | AFIT trait: `upsert` (idempotent), `query`, `get`, `kpi_report`, `overdue_processes` |
| feature `testing` | `testing::InMemoryProcessProjectionRepository` — never in production |

| `ce_type`                    | `ProcessState`   |
|------------------------------|------------------|
| `de.mako.process.initiated`  | `Initiated`      |
| `de.mako.aperak.accepted`    | `Running`        |
| `de.mako.aperak.rejected`    | `Rejected` + ERC |
| `de.mako.aperak.timeout`     | `AperakTimeout`  |
| `de.mako.process.completed`  | `Completed`      |
| `de.mako.process.failed`     | `Failed`         |

API documentation: <https://docs.rs/mako-obs>.
