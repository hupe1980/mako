+++
title = "Deadline Compensation"
description = "How mako-engine decides a fired deadline: owned labels, on_deadline, the lapse outcomes, and the delivery windows the outbox retires."
weight = 12
+++
## Problem

An inbound message starts **two independent clocks**:

| Clock | Window | Source |
|---|---|---|
| **Technical acknowledgement** (APERAK) | **45 Minuten** for a UTILMD or ORDERS; Sonntag 12:00 for a Saturday arrival; nächster Werktag 12:00 for every other message type | APERAK AHB 1.1 § 2.4.1 |
| **Business answer** | per Prüfidentifikator — a clock time, an end-of-Werktag, or a Werktag count | the Festlegung for that process |

A queue sized by the looser of the two reports a lapsed Frist as still
running. The business windows are one table keyed by inbound PID
(`mako_fristen::antwort`), so `makod`, `processd` and `agentd` cannot
disagree about a deadline:

| Family | Shape | Examples |
|---|---|---|
| GPKE Strom | wall-clock time on the *n*-th Werktag after the ÜT | 11:00 (55001, 55077), 06:00 (55004), 05:00 (55007), 09:00 (55010), 15:00 **am ÜT** (55013, 55607) |
| WiM Strom | Werktage per PID | 3 (55039) / 5 (55042) / 7 (55051) / 1 (55168) |
| WiM Gas | the same four windows on the Gas twins | 3 (44039) / 5 (44042) / 7 (44051) / 1 (44168) |
| GeLi Gas | Ablauf des *n*-ten Werktags | 4 (44001), 3 (44004/44007/44010/44016), 2 (44013) |
| NZR-EMob (Modell 2) | Ablauf des *n*-ten Werktags | 7 (55238), 3 (55240, 55242) |

`Family` names exactly these five. MaBiS publishes no answer Frist — the
Prüfmitteilung's cell in Kap. 9.8.2 Nr. 1 is empty and the NB „kann" answer; the
Kap. 3.10 Tabelle 2 clearing window bounds it. The two 1-Werktag MaBiS
obligations belong to the BIKO (Kap. 9.8.2 Nr. 3, Kap. 9.9.2 Nr. 1) and live in
`mako_mabis::fristen`.

`mako_fristen::antwort::antwortfrist` returns `None` for a PID the Festlegungen
do not quantify — **unknown**, never *unbounded*. The GeLi Gas "10 Werktage" is
the supplier's Vorlauffrist before Lieferbeginn, not an answer window
(`TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST`).

Deadlines are never cancelled when a process moves on, so every registered
deadline fires — on the healthy path as much as on the failing one. Whether a
firing acts is decided in exactly one place: the workflow's own
`Workflow::on_deadline`, which reads the **label** to know which window fired
and the **state** to know whether that window is still open.

## Architecture

```
Deadline scheduler (makod/src/orchestrator/deadline_dispatch.rs)
  ├─ delivery window (fristen::is_delivery_window) → regulatory alert, no workflow
  ├─ label ∉ Workflow::DEADLINE_LABELS             → logged as unowned, nothing acts
  └─ Process::run_timeout(&deadline, RunOptions::new())
       └─ Workflow::on_deadline(deadline, state)   → None: the window is closed
            └─ Some(TimeoutExpired) → Workflow::handle → events + outbox, one write
```

### Key invariant: one guard

`Workflow::DEADLINE_LABELS` is the table registration and matching share. The
engine refuses to persist a deadline whose label is neither listed there nor a
delivery window (`EngineError::UnownedDeadlineLabel`), and
`Process::run_timeout` consults `on_deadline` only for a listed label. Each
workflow decides a listed label in one function — `on_deadline` and the
`TimeoutExpired` arm of `handle` both call it — so a window acts only while the
state it belongs to is current: an answer already sent, or received, closes it.

Every retry re-reads the state and asks `on_deadline` again: the write that
conflicted may be the very answer that closed the window.

### Key invariant: a lapse is not a rejection

When a window lapses with its answer still owed, the outcome is the one the
Festlegung gives that silence, and nothing else:

| The Festlegung says | The process |
|---|---|
| silence counts as consent — „Verstreicht die Frist, ohne dass eine Antwort beim NB eingeht, gilt dies als Bestätigung" (GPKE Teil 2), the Ersatz-/Grundversorger's silence on a 44013 (GeLi Gas 3.0 Kap. 3.3), the Altlieferant's on a 44010 (Kap. 3.2.3), a missing Rückmeldung (GPKE Teil 4 Kap. 1.4.2–1.4.5) | completes as if answered |
| the step fails — no Gesamtvorgang report by the 11. WT, „Der MSBA bleibt der einzelnen Messlokation zugeordnet" (WiM Teil 1 Kap. 2.3.2 Nr. 16) | ends with that outcome |
| nothing | `FristAbgelaufen` |

`FristAbgelaufen { label, offen }` wraps the state the process was waiting in.
It is a Frist breach for the operator — whether our own answer is late or the
counterparty's — and not an outcome: every command `offen` accepts is still
accepted, so a late answer still goes out or still lands, and the next event
moves the process on. The `DeadlineExpired` event records the lapse, and the
`TimeoutExpired` arm enqueues an `AperakTimeout` ERP notification where the
workflow carries one, in the same write.

### Key invariant: a delivery window watches a message that goes out

A **delivery window** waits on *us* (*did our APERAK go out within 45
minutes?*). `OutboxWorker` retires it the moment its message is delivered;
`fristen::discharges_delivery_window` maps each message type to the labels it
answers for:

| Message | Discharges | Obligation |
|---|---|---|
| `APERAK` | `aperak-strom-45min-window`, `aperak-gas-folgeprozess-…`, `aperak-gas-initialprozess-…` | APERAK AHB 1.1 §2.4.1 / §2.3.1 |
| `CONTRL` | `contrl-delivery-window` | CONTRL AHB 1.0 §2.3.1 / §2.4.1 |

A delivery window is registered only on a path that enqueues its message. In
der Sparte Gas the APERAK is a Verarbeitbarkeitsfehlermeldung only (APERAK AHB
1.1 §2.3), so a processable Gas message arms no APERAK window.

A delivery discharges only its own windows — an acknowledged CONTRL says nothing
about the APERAK. A delivery window that survives to its due time is an
undelivered message, and that alone is the violation the miss counters report.
`every_delivery_window_label_is_discharged_by_its_message` (in `mako-engine`)
pins that every delivery-window label is retired by its message.

## Workflow implementation pattern

```rust
impl Workflow for MyWorkflow {
    // 1. The labels this workflow registers and decides.
    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    // 2. on_deadline asks the one guard.
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| MyCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    // 3. handle(TimeoutExpired) asks it again, on the state before any lapse.
    //    MyCommand::TimeoutExpired { deadline_id, label } => {
    //        if !window_open(&label, lapsed) { return Ok(WorkflowOutput::events(vec![])); }
    //        Ok(vec![MyEvent::DeadlineExpired { deadline_id, label }].into())
    //    }
}

/// The window is open while the answer is owed.
fn window_open(label: &str, state: &MyState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, MyState::Eingegangen(_))
}
```

`services/makod/tests/it/deadline_labels.rs` holds the pattern: every label
constant is owned by a routed workflow, every owned label is matched by its
workflow's guard, every `TimeoutExpired` arm asks that guard, and no guard
decides a delivery window. `healthy_deadline_interleavings.rs` fires each
family's deadlines in random order after their answers and requires that no
healthy run ends lapsed or rejected.

## ERP delivery

`OutboxErpWorker` (`services/makod/src/core/erp_adapter.rs`) maps an
`AperakTimeout` outbox entry to `ErpEventType::AperakTimeout` and delivers it as
the CloudEvent `de.mako.aperak.timeout`, with the inbound PID in the `makopid`
extension. Envelope and receiver: [ERP Integration](@/docs/architecture/erp-integration.md).

## Adding a new workflow with compensation

1. Add `TimeoutExpired { deadline_id, label }` to the command enum.
2. Name the label as a `pub const …_LABEL`, list it in `DEADLINE_LABELS`, and
   return the deadline from `handle` as a `PendingDeadline` (or pass it through
   `RunOptions::deadlines` on spawn), its due time from `mako_fristen`.
3. Implement `window_open` and `on_deadline` as above.
4. In the `TimeoutExpired` arm, re-check `window_open`, then record the outcome
   the Festlegung gives the silence — or a `DeadlineExpired` event that `apply`
   folds into `FristAbgelaufen` — plus an `AperakTimeout` outbox entry where the
   ERP must hear of it.
5. Register the workflow in its module's `drivers()` with
   `WorkflowDriver::of::<W>(WORKFLOW_NAME)`; `EngineBuilder::build` refuses a
   routed workflow without one.

## `run_timeout`

`Process::run_timeout(&deadline, opts)` answers `None` for a label outside
`DEADLINE_LABELS`; otherwise it asks `on_deadline` on the current state and runs
the returned command exactly as `Process::run` would — one write, the same
version-conflict retry, `on_deadline` asked again on each attempt. The scheduler
reaches it through the workflow's `WorkflowDriver`; nothing outside the workflow
constructs `TimeoutExpired`.

**A label no `on_deadline` arm matches fires into the default `None`** —
no error, no event, the process left waiting. `deadline_labels.rs` therefore
requires every `pub const …_LABEL` in a `mako-*` crate to appear in some
`on_deadline` body unless it is a delivery window, every registration to take
the owning crate's constant rather than a literal, and no `on_deadline` to be a
catch-all that ignores `deadline.label()` — that would also swallow the APERAK
and CONTRL delivery windows running beside the business Frist.
