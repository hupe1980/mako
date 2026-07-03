# mako-gpke

**GPKE — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität**

Process engine workflows for the German electricity market supplier-switch
and grid access billing processes. Implements the BDEW GPKE specification
and BNetzA rulings:
- **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — GPKE Teil 1–3 (Lieferantenwechsel, Zuordnungsprozesse)
- **BK6-22-024** (Beschluss 21.03.2024) — GPKE Teil 4 (Stammdatenprozesse, Konfigurationseinrichtung)

## APERAK Frist

GPKE processes use **24 wall-clock hours** (`fristen::add_hours(24)`) for
the APERAK response deadline — not Werktage. This is enforced by BK6-22-024.

## PID Inventory

### UTILMD supplier-switch and feed-in processes (S2.1/S2.2)

> **Legend:** ✅ Implemented — full state machine, AHB-validated, production-safe.
> ↩ Derived — emitted by workflow as outbound ANTWORT, not routed as inbound.
> ❌ Removed — existed pre-LFW24; router rejects with CONTRL.

| PID   | Process name (AHB)                                    | Direction   | Status      |
|-------|-------------------------------------------------------|-------------|-------------|
| 55001 | Anfrage Lieferbeginn Strom                            | LFN → NB    | ✅ Implemented |
| 55002 | Anfrage Lieferende Strom                              | LFN → NB    | ✅ Implemented |
| 55003 | Bestätigung Lieferbeginn                              | NB → LFN    | ↩ Derived from 55001 accept |
| 55004 | Ablehnung Lieferbeginn                                | NB → LFN    | ↩ Derived from 55001 reject |
| 55005 | Bestätigung Lieferende                                | NB → LFN    | ↩ Derived from 55002 accept |
| 55006 | Ablehnung Lieferende                                  | NB → LFN    | ↩ Derived from 55002 reject |
| 55007–55010 | (removed in LFW24 — not in AHB S2.x)           | —           | ❌ Removed |
| 55017 | Kündigung Lieferbeginn                                | LFN → LFA   | ✅ Implemented |
| 55018 | Bestätigung Kündigung Lieferbeginn                    | LFA → LFN   | ↩ Derived from 55017 always |
| 55555 | Anfrage Daten der individuellen Bestellung            | LFN → NB    | ✅ Implemented (GPKE Teil 4, BK6-24-174) |

### ORDERS/ORDRSP Konfigurationseinrichtung (GPKE Teil 4)

| PID   | Process name                                          | Direction   | Status         |
|-------|-------------------------------------------------------|-------------|----------------|
| 17134 | Einrichtung Konfiguration aufgrund Zuordnung LF (NB an MSB) | NB → MSB | ✅ Implemented |
| 17135 | Einrichtung Konfiguration aufgrund Zuordnung LF (MSB an MSB) | MSB → MSB | ✅ Implemented |
| 19001 | Bestellbestätigung (accept)                           | MSB → NB/MSB | ↩ Derived from 17134/17135 accept |
| 19002 | Ablehnung der Bestellung (reject)                     | MSB → NB/MSB | ↩ Derived from 17134/17135 reject |

### INVOIC billing processes (Netznutzungsabrechnung)

| PID   | Process name                                  | Status          |
|-------|-----------------------------------------------|-----------------|
| 31001 | Abschlagsrechnung (Netznutzung)               | ✅ Implemented  |
| 31002 | NN-Rechnung (Netznutzungsabrechnung)          | ✅ Implemented  |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)        | ✅ Implemented  |
| 31006 | MMM-Rechnung (selbst ausgestellt)             | ✅ Implemented  |
| 31007 | Aggregierte Mehr-/Mindermenge Rechnung        | ✅ Implemented  |
| 31008 | Aggregierte Mehr-/Mindermenge Rechnung (SA)   | ✅ Implemented  |

> PIDs 31003 (WiM-Rechnung) and 31009 (MSB-Rechnung) belong to the WiM domain.
> PID 31004 (Stornorechnung WiM Gas) belongs to `mako-wim-gas` (BK7-24-01-009).

### ORDERS Sperrung Strom — NB role (GPKE Teil 4, BK6-22-024)

> The gas Sperrung equivalents of these PIDs (same PID numbers, different Sparte) belong
> to `mako-geli-gas`. Never mix Strom and Gas Sperrung in the same deployment module.

| PID   | Process name                                              | Direction   | Status         |
|-------|-----------------------------------------------------------|-------------|----------------|
| 17115 | Anfrage Sperrung / Entsperrung (NB → gMSB/MSB)           | NB → MSB    | ✅ Implemented |
| 17116 | Antwort Sperrung / Entsperrung (gMSB/MSB → NB)           | MSB → NB    | ↩ Derived      |
| 17117 | Stornierung Sperrauftrag / Änderung (NB → gMSB/MSB)      | NB → MSB    | ✅ Implemented |

### UTILMD Stornierung Zuordnungsprozess (GPKE Teil 1)

| PID   | Process name                                          | Direction            | Status         |
|-------|-------------------------------------------------------|----------------------|----------------|
| 55022 | Anfrage Stornierung Zuordnungsprozess                 | LFN/NB → NB/LFN      | ✅ Implemented |
| 55023 | Bestätigung Stornierung Zuordnungsprozess             | NB/LFN → orig.       | ↩ Derived      |
| 55024 | Ablehnung Stornierung Zuordnungsprozess               | NB/LFN → orig.       | ↩ Derived      |

### UTILMD Ankündigung / Zuordnung LF (GPKE Teil 1)

| PID   | Process name                                          | Direction    | Status         |
|-------|-------------------------------------------------------|--------------|----------------|
| 55607 | Ankündigung Zuordnung LF (NB → LFN)                   | NB → LFN     | ✅ Implemented |
| 55608 | Bestätigung Ankündigung Zuordnung LF (LFN → NB)       | LFN → NB     | ↩ Derived      |
| 55609 | Ablehnung Ankündigung Zuordnung LF (LFN → NB)         | LFN → NB     | ↩ Derived      |

### PARTIN Strom — Kommunikationsdaten (PARTIN AHB 1.0f)

| PID       | Process name                                             | Status         |
|-----------|----------------------------------------------------------|----------------|
| 37000     | Übermittlung Kommunikationsdaten Strom (Stammdaten)      | ✅ Implemented |
| 37001     | Bestätigung Übermittlung Kommunikationsdaten Strom       | ↩ Derived      |
| 37002     | Ablehnung Übermittlung Kommunikationsdaten Strom         | ↩ Derived      |
| 37003     | Übermittlung Kommunikationsdaten Strom (Korrekturen)     | ✅ Implemented |
| 37004     | Bestätigung Korrektur                                    | ↩ Derived      |
| 37005     | Ablehnung Korrektur                                      | ↩ Derived      |
| 37006     | Übermittlung Kommunikationsdaten — weiterer Typ          | ✅ Implemented |

> PIDs 37008–37014 (PARTIN Gas Kommunikationsdaten) belong to `mako-geli-gas`.

## EDIFACT Format Versions

| Format version   | Valid from | Valid until | Profile status                   |
|------------------|------------|-------------|----------------------------------|
| `FV2025-06-06`   | 2025-06-06 | 2025-09-30  | ✓ available (UTILMD S1.2 — LFW24 cutover) |
| `FV2025-10-01`   | 2025-10-01 | 2026-09-30  | ✓ available (UTILMD S2.1 — current) |
| `FV2026-10-01`   | 2026-10-01 | —           | ✓ available (UTILMD S2.2 — upcoming) |
| `FV2026-04-01`   | 2026-04-01 | 2026-09-30  | ✓ available (INVOIC 2.8e, REMADV 2.9f, ORDERS 1.4b) |

> INVOIC (31001–31008) and ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002)
> use their own versioned profiles (`fv20260401`), independent of the UTILMD
> Strom release cycle.

## Modules

| Rust module         | Contents                                                |
|---------------------|---------------------------------------------------------|
| `wechselprozesse`   | PIDs 55001–55002, 55017 (UTILMD supplier-switch) |
| `lf_anmeldung`      | PIDs 55003–55006, 55018 (LF-role: receive NB ANTWORT)   |
| `anfrage_bestellung`| PID 55555 (Anfrage Daten der individuellen Bestellung, LFN → NB, GPKE Teil 4) |
| `abrechnung`        | PIDs 31001–31008 (INVOIC Netznutzungsabrechnung)        |
| `konfiguration`     | PIDs 17134/17135 (ORDERS outbound) + 19001/19002 (ORDRSP inbound) — GPKE Teil 4 |
| `sperrung`          | PIDs 17115–17117 (ORDERS Sperrung Strom, NB role)       |
| `stornierung`       | PIDs 55022–55024 (UTILMD Stornierung Zuordnungsprozess) |
| `ankuendigung_zuordnung_lf` | PIDs 55607–55609 (UTILMD Ankündigung Zuordnung LF) |
| `partin`            | PIDs 37000–37006 (PARTIN Strom Kommunikationsdaten)     |

## Usage

```rust
use mako_gpke::wechselprozesse::{GpkeSupplierChangeWorkflow, SupplierChangeCommand};
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

let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(tenant_id, workflow_id);
let events = process.execute(SupplierChangeCommand::ReceiveUtilmd {
    pid: 55001,
    // …
}).await?;
```

## Regulatory references

- BDEW GPKE Marktprozesse für die Belieferung mit Elektrizität
- BNetzA **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — GPKE Teil 1–3
- BNetzA **BK6-22-024** (Beschluss 21.03.2024) — GPKE Teil 4 + APERAK Frist 24 Stunden
- EDI@Energy UTILMD Strom AHB S2.2 (`FV2026-10-01`)
- EDI@Energy INVOIC AHB 2.8e / AHB 1.0 (`FV2025-10-01` onwards)
- EDI@Energy APERAK AHB 2.2 (`FV2026-10-01`)
