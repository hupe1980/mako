# mako-geli-gas

**GeLi Gas — Geschäftsprozesse Lieferantenwechsel Gas**

Process engine workflows for the German gas market supplier-switch processes.
Implements the BDEW GeLi Gas specification:
- **GeLi Gas 3.0** — BNetzA **BK7-24-01-009** (Beschluss 12.09.2025, abgeschlossen 24.09.2025)

This supersedes BK7-19-001 and the original BK7-06-067 (2007).

## APERAK Frist

GeLi Gas processes use **10 Werktage** (`fristen::add_werktage(10, BdewMaKo)`)
for the APERAK response deadline. This is the longest Frist across all process
families. Saturday counts as a Werktag; Sunday and public holidays do not.

## Key difference from electricity processes

| Aspect          | GPKE (Strom)      | WiM (Strom)       | GeLi Gas          |
|-----------------|-------------------|-------------------|-------------------|
| Market          | Electricity       | Electricity       | **Gas**           |
| Location object | MeLo (Messlok.)   | MeLo (Messlok.)   | **MaLo (Marktlok.)** |
| Grid operator   | Netzbetreiber     | Netzbetreiber     | **Gasnetzbetreiber (GNB)** |
| APERAK Frist    | 24 h wall-clock   | 5 Werktage        | **10 Werktage**   |
| EDIFACT format  | UTILMD Strom S2.x | UTILMD Strom S2.x | **UTILMD Gas G2.x** |

## PID Inventory

> Legend: **✅ Implemented** — full state machine + AHB rule enforcement, production-safe.
> **⚠️ Registered** — PID routes to the workflow; partial handling in current code.
> **✗ Not registered** — PID is not in the router; inbound messages are dead-lettered.

| PID     | Process name                                        | EDIFACT       | Status                            |
|---------|-----------------------------------------------------|---------------|-----------------------------------|
| 44001   | Lieferbeginn Gas — Anfrage LFN → NB                 | UTILMD G1/G2  | ✅ Implemented                    |
| 44002   | Lieferende Gas — Anfrage LFN → NB                   | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44003   | Bestätigung Lieferbeginn Gas — NB → LFN             | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44004   | Ablehnung Lieferbeginn Gas — NB → LFN               | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44005   | Bestätigung Lieferende Gas — NB → LFN               | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44006   | Ablehnung Lieferende Gas — NB → LFN                 | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44017   | Kündigung Lieferbeginn Gas — LFN → LFA              | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44018   | Bestätigung Kündigung Lieferbeginn Gas — LFA → LFN  | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 17003   | Beauftragung Änderung Technik (MeLo Gas)            | ORDERS 1.4b   | ✗ Not registered          |
| 17101   | Anfrage Übermittlung Stammdaten Gas                 | ORDERS 1.4b   | ✗ Not registered          |
| 17103   | Anfrage Abrechnungsbrennwert und Zustandszahl       | ORDERS 1.4b   | ✗ Not registered          |
| 17104   | Anfrage MSB Gas an NB Strom                         | ORDERS 1.4b   | ✗ Not registered          |
| 39000   | Stornierung Sperr-/Entsperrauftrag                  | ORDCHG 1.1    | ✗ Not registered          |
| 39001   | Weiterleitung der Stornierung                       | ORDCHG 1.1    | ✗ Not registered          |
| 39002   | Stornierung der Bestellung von Werten               | ORDCHG 1.1    | ✗ Not registered          |

> **PIDs 44002–44006, 44017–44018** are registered under
> `geli-gas-supplier-change` and share the same `GeliGasSupplierChangeWorkflow`
> as PID 44001. The state machine currently handles all registered PIDs via
> the same transition logic; separate state machines for Lieferende and
> Kündigung are planned but not yet implemented.
>
> **ORDERS PIDs 17003, 17101, 17103, 17104** are Gas-specific Stammdaten and
> Zählpunktverwaltung Gas processes defined in ORDERS AHB 1.4b. They share the
> same 17xxx PID range with WiM Messwesen processes (which is exclusively
> Strom-oriented) but relate to Gas MeLo commissioning and master-data exchange.
> None are currently registered in `mako-geli-gas`; inbound messages are
> dead-lettered.
>
> **ORDCHG PIDs 39000–39002** are cancellation processes applicable to both Gas
> and Electricity (Stornierung). They are unregistered in all current domain
> crates.

## EDIFACT Format Versions

| Format version       | Valid from | Valid until | Profile status |
|----------------------|------------|-------------|----------------|
| `FV2024-10-01_gas`   | 2024-10-01 | 2025-09-30  | ✓ available    |
| `FV2025-10-01_gas`   | 2025-10-01 | 2026-09-30  | ✓ available    |
| `FV2026-10-01_gas`   | 2026-10-01 | —           | ✓ available    |

## Modules

| Rust module    | Contents                                      |
|----------------|-----------------------------------------------|
| `lieferbeginn` | PIDs 44001–44006, 44017–44018 workflow + proj |

## Usage

```rust
use mako_geli_gas::{GeliGasSupplierChangeWorkflow, GasSupplierChangeCommand};
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

let process = ctx.spawn::<GeliGasSupplierChangeWorkflow>(tenant_id, workflow_id);
let events = process.execute(GasSupplierChangeCommand::ReceiveUtilmd {
    pid: 44001,
    // …
}).await?;
```

## Regulatory references

- BDEW GeLi Gas Geschäftsprozesse Lieferantenwechsel Gas
- BNetzA **BK7-24-01-009** — GeLi Gas 3.0 (Beschluss 12.09.2025, g. 24.09.2025) — APERAK Frist 10 Werktage
- BNetzA BK7-19-001 — previous ruling (superseded)
- BNetzA BK7-06-067 — original GeLi Gas ruling 2007 (superseded)
- EDI@Energy UTILMD Gas AHB G2.x (`FV2026-10-01`)
- EDI@Energy APERAK AHB 2.2 (`FV2026-10-01`)
