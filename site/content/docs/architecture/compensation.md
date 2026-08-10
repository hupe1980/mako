+++
title = "Deadline Compensation"
description = "Saga pattern for MaKo regulatory deadlines. How mako-engine compensates for failed outbox delivery and enforces APERAK Fristen."
weight = 12
+++
# Deadline Compensation / Saga Pattern

## Problem

MaKo regulatory processes have hard SLA windows enforced by BNetzA rulings:

| Domain        | Window          | Helper                               | Ruling       |
|---------------|-----------------|--------------------------------------|--------------|
| GPKE (Strom)  | **24 h wall-clock** | `fristen::add_hours(t, 24)`      | BK6-24-174   |
| WiM (Strom)   | **3 / 5 / 7 / 1 WT**, per PID | `antwort_frist_werktage(pid)` | BK6-24-174 Teil 1 |
| GeLi Gas      | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` | BK7-24-01-009 |
| WiM Gas       | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` | BK7-24-01-009 |
| MABIS         | **1 Werktag** (Prüfmitteilung) | `fristen::add_werktage(d, 1, BdewMaKo)` | BK6-24-174 |

When a process does not receive an APERAK within the window, the engine fires a
`DeadlineExpired` event and must automatically enqueue an `AperakTimeout` ERP
outbox message so the ERP/operator can act on the missed SLA.

## Architecture

The compensation path flows through three layers:

```
Deadline scheduler (makod/src/deadline_dispatch.rs)
  └─ Process::execute_and_enqueue_with_retry(TimeoutExpired, 3)
       └─ Workflow::handle(TimeoutExpired, state)
            ├─ emit: DeadlineExpired event
            └─ outbox: AperakTimeout → OutboxErpWorker → ERP webhook
```

### Key invariant: atomicity

`execute_and_enqueue_with_retry` routes through
`execute_command_atomic` → `SlateDbStore::append_with_outbox`, which writes the
`DeadlineExpired` event **and** the `AperakTimeout` outbox entry in a single
`WriteBatch`. There is no window where:
- the event is persisted but the ERP notification is lost, or
- the ERP notification is sent but the event is missing from the audit log.

### Key invariant: a deadline is discharged when its obligation is met

Deadlines fall into two kinds, and they are retired differently.

A **process-response window** waits on the counterparty (*did they answer within
24 h?*). It is meant to fire; `Workflow::on_deadline` inspects process state and
returns `None` when the answer already arrived, which is why deadline dispatch
should route through `Process::execute_timeout_with_retry` rather than
constructing a `TimeoutExpired` command directly.

A **delivery window** waits on *us* (*did our APERAK go out within 45 minutes?*).
It must never fire on the happy path, so `OutboxWorker` retires it the moment the
message it watches is delivered — `fristen::discharges_delivery_window` maps each
message type to the labels its delivery answers for:

| Message | Discharges | Obligation |
|---|---|---|
| `APERAK` | `aperak-strom-45min-window`, `aperak-gas-folgeprozess-…`, `aperak-gas-initialprozess-…` | APERAK AHB 1.0 §2.4.1 / §2.3.1 |
| `CONTRL` | `contrl-6h-delivery-window` | CONTRL AHB 1.0 §1.2 |

A delivery discharges only its own windows — an acknowledged CONTRL says nothing
about whether the application-level APERAK went out, and a deadline that merely
shares the stream is left alone.

**This discharge is what gives the miss counters meaning.** The scheduler selects
deadlines on `due_at <= now`, so "fired after its due time" is true of every
deadline it ever hands out and proves nothing on its own. A delivery window that
survives to its due time is an undelivered message — that, and only that, is the
violation. A new delivery window that `discharges_delivery_window` does not
recognise is never retired, so it alerts on every process; the
`every_delivery_window_label_is_discharged_by_its_message` test pins that.

### Retry on conflict

Deadline workers use `execute_and_enqueue_with_retry(..., 3)` so that a
`VersionConflict` (concurrent event append by another task) is retried up to
3 times before bubbling to the scheduler, which re-fires the deadline later.

## Workflow implementation pattern

Every workflow that registers a regulatory deadline MUST implement
`Workflow::on_deadline` AND add compensation outbox entries in the
`TimeoutExpired` handler:

```rust
// 1. on_deadline — map label → command (pure, no I/O)
fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
    match (deadline.label(), state) {
        ("aperak-window", SupplierChangeState::Initiated(_))
        | ("aperak-window", SupplierChangeState::ValidationPassed(_)) => {
            Some(SupplierChangeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label:       deadline.label().into(),
            })
        }
        // Terminal or unrecognised states → no-op (idempotent)
        _ => None,
    }
}

// 2. handle(TimeoutExpired) — emit event + compensation outbox atomically
SupplierChangeCommand::TimeoutExpired { deadline_id, label } => {
    // Absorb silently on terminal states (late-firing deadline).
    if matches!(state, SupplierChangeState::Active(_) | SupplierChangeState::Rejected { .. }) {
        return Ok(WorkflowOutput::events(vec![]));
    }
    let mut outbox = vec![];
    if let Some(data) = state.initiated_data() {
        outbox.push(PendingOutbox::new(
            "AperakTimeout",
            data.new_supplier.as_str(),
            serde_json::json!({
                "pid":          data.pruefidentifikator.as_u32(),
                "malo":         data.location_id.as_str(),
                "new_supplier": data.new_supplier.as_str(),
                "deadline_label": label.as_ref(),
            }),
        ));
    }
    let event = SupplierChangeEvent::DeadlineExpired { deadline_id, label };
    if outbox.is_empty() {
        Ok(vec![event].into())
    } else {
        Ok(WorkflowOutput::with_outbox(vec![event], outbox))
    }
}
```

## ERP delivery

The `OutboxErpWorker` in `makod/src/erp_adapter.rs` picks up `AperakTimeout`
messages and maps them to `ErpEventType::AperakTimeout`:

```rust
"AperakTimeout" => ErpEventType::AperakTimeout,
```

The ERP webhook receives a **CloudEvents 1.0** message with:

```json
{
  "specversion": "1.0",
  "type": "de.mako.aperak.timeout",
  "source": "urn:mako:makod:tenant:9900357000004",
  "id": "...",
  "time": "...",
  "makopid": 55001,
  "data": {
    "malo": "DE0004...",
    "new_supplier": "9900000000001",
    "deadline_label": "aperak-window"
  }
}
```

## Adding a new workflow with compensation

1. Add `TimeoutExpired { deadline_id, label }` to your `XxxCommand` enum.
2. Register the deadline in your workflow's `Initiate` handler via
   `PendingOutbox` with `deliver_after` derived from the regulatory Frist.
3. Implement `on_deadline` to return `Some(TimeoutExpired)` for active states.
4. Implement the `TimeoutExpired` arm in `handle` with:
   - `DeadlineExpired` event (for audit log).
   - `AperakTimeout` outbox entry (for ERP notification).
   - Early return `WorkflowOutput::events(vec![])` for terminal states.
5. Add a `match` arm to `deadline_dispatch::dispatch_deadline` calling
   `execute_and_enqueue_with_retry(..., 3)`.

## `execute_timeout` / `execute_timeout_with_retry`

`Process::execute_timeout` and `Process::execute_timeout_with_retry` are
convenience wrappers that call `on_deadline` and route the returned command
through `execute_and_enqueue` / `execute_and_enqueue_with_retry`. Prefer these
in custom deadline workers over manually calling `on_deadline` + `execute`.
Note that `deadline_dispatch.rs` calls the command directly for clarity.
