# mako-wim

**WiM — Wechselprozesse im Messwesen Strom**

Process engine workflows for the German electricity metering system change
processes. Implements the BDEW WiM specification and the BNetzA ruling
**BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025).

## APERAK Frist

WiM processes use **5 Werktage** (`fristen::add_werktage(5, BdewMaKo)`) for
the APERAK response deadline. Saturday counts as a Werktag; Sunday and public
holidays do not.

## PID Inventory

> Legend: **✅ Implemented** — full state machine + AHB rule enforcement, production-safe.
> **⚠️ Registered** — PID routes to the workflow; `handle()` returns
> `WorkflowError::NotImplemented` for unhandled commands (no silent data loss).
> **✗ Not registered** — PID is not in the router; inbound messages are dead-lettered.

| PID   | Process name                                   | EDIFACT     | Status                             |
|-------|------------------------------------------------|-------------|------------------------------------|
| 11001 | Gerätewechsel — Anmeldung nMSB                 | UTILMD S2.x | ✅ Implemented                     |
| 11002 | Gerätewechsel — Abmeldung aMSB                 | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11003 | Gerätewechsel — Bestätigung                    | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11004 | Gerätewechsel — Ablehnung                      | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11005 | Gerätewechsel — Fristgerecht                   | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11006 | Gerätewechsel — Kündigung                      | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11011 | Entstörung/Unterbrechung — Anmeldung           | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11012 | Entstörung/Unterbrechung — Bestätigung         | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11013 | Entstörung/Unterbrechung — Ablehnung           | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11021 | iMSys — Anmeldung (Universalbestellprozess)    | REST/JSON   | ✅ Implemented (REST channel)      |
| 11022 | iMSys — Bestätigung                            | REST/JSON   | ✅ Implemented (REST channel)      |
| 11023 | iMSys — Ablehnung                              | REST/JSON   | ✅ Implemented (REST channel)      |
| 11031 | Zählpunkt — Anmeldung                          | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11032 | Zählpunkt — Abmeldung                          | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 11041 | Messdatenübermittlung — Anmeldung              | MSCONS 2.x  | ✗ Not registered                   |
| 11042 | Messdatenübermittlung — Ablehnung              | MSCONS 2.x  | ✗ Not registered                   |
| 11051 | Prüfidentifikator-Prüfung (WiM) — Reserve     | UTILMD S2.x | ⚠️ Registered — not implemented   |
| 17001–17211 | WiM ORDERS (Zählpunktverwaltung)         | ORDERS      | ✗ Not registered           |
| 39000–39002 | WiM ORDCHG (Änderungsbestellungen)       | ORDCHG      | ✗ Not registered           |

> **PIDs 11002–11099:** All 99 PIDs are registered under `wim-device-change`.
> Only 11001 (Anmeldung nMSB) returns a real response; others return
> `WorkflowError::NotImplemented` and the message is APERAK-rejected
> without crashing the engine.

## EDIFACT Format Versions

| Format version | Valid from | Valid until | Profile status |
|----------------|------------|-------------|----------------|
| `FV2024-10-01` | 2024-10-01 | 2025-09-30  | ✓ available    |
| `FV2025-10-01` | 2025-10-01 | 2026-09-30  | ✓ available    |
| `FV2026-10-01` | 2026-10-01 | —           | ✓ available    |

## Modules

| Rust module        | Contents                                                          |
|--------------------|-------------------------------------------------------------------|
| `geraetewechsel`   | PID 11001 (nMSB Anmeldung) + 11021–11023 (iMS REST) workflow + projection |
| `geraeteubernahme` | Gerätübernahme — ANFRAGE/BESTELLUNG/STORNIERUNG workflows         |
| `stammdaten`       | Stammdatenanforderung and Stammdatenübermittlung workflow         |
| `steuerungsauftrag`| Steuerungsauftrag (iMS Steuerbefehl) workflow                    |
| `stornierung`      | Stornierung — cancellation workflow                              |

## Usage

```rust
use mako_wim::{WimDeviceChangeWorkflow, DeviceChangeCommand};
use mako_engine::{builder::EngineBuilder, event_store::InMemoryEventStore};

// In tests (requires `testing` feature or `#[cfg(test)]`):
#[cfg(test)]
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .build();

// In production, explicitly provide all stores:
// let ctx = EngineBuilder::with_stores(outbox, deadline, registry)
//     .with_event_store(my_slatedb_store)
//     .build();

let process = ctx.spawn::<WimDeviceChangeWorkflow>(tenant_id, workflow_id);
let events = process.execute(DeviceChangeCommand::ReceiveUtilmd {
    pid: 11001,
    // …
}).await?;
```

## Regulatory references

- BDEW WiM Wechselprozesse im Messwesen Strom
- MsbG — Messstellenbetriebsgesetz
- BNetzA **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — Frist 5 Werktage für APERAK
- EDI@Energy UTILMD Strom AHB S2.2 (`FV2026-10-01`)
- EDI@Energy APERAK AHB 2.2 (`FV2026-10-01`)
