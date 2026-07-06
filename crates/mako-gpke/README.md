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

Implemented by `GpkeAbrechnungWorkflow`. Inbound INVOIC messages from the NB
spawn a new process; the `invoicd` daemon listens for
`de.mako.process.initiated` events and runs a plausibility check via
`invoic-checker`. It then calls `gpke.abrechnung.annehmen` (→ REMADV) or
`gpke.abrechnung.ablehnen` (→ COMDIS) on the Command API. Inbound REMADV and
COMDIS from the NB are handled via `ReceiveRemadv` and `ReceiveComdis` commands.

| PID   | Process name                                  | Status          |
|-------|-----------------------------------------------|-----------------|
| 31001 | Abschlagsrechnung (Netznutzung)               | ✅ Implemented  |
| 31002 | NN-Rechnung (Netznutzungsabrechnung)          | ✅ Implemented  |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)        | ✅ Implemented  |
| 31006 | MMM-Rechnung (selbst ausgestellt)             | ✅ Implemented  |

> PIDs 31007/31008 (Aggreg. MMM-Rechnung Gas, NB → MGV) belong to `mako-gabi-gas` (BK7-14-020).
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

### UTILMD Neuanlage Marktlokation (GPKE Teil 1)

Workflow `gpke-neuanlage` handles Neuanlage requests where the MaLo does not yet
exist in the grid operator's system.

| PID   | Process name (AHB)                               | Direction  | Status         |
|-------|--------------------------------------------------|------------|----------------|
| 55600 | Anmeldung neue verbrauchende MaLo (LF → NB)     | LF → NB    | ✅ Implemented |
| 55601 | Anmeldung neue erzeugende MaLo (LF → NB)        | LF → NB    | ✅ Implemented |
| 55602 | Bestätigung Anmeldung neue verb. MaLo (NB → LF) | NB → LF    | ↩ Derived from 55600 accept |
| 55603 | Bestätigung Anmeldung neue erz. MaLo (NB → LF)  | NB → LF    | ↩ Derived from 55601 accept |
| 55604 | Ablehnung Anmeldung neue verb. MaLo (NB → LF)   | NB → LF    | ↩ Derived from 55600 reject |
| 55605 | Ablehnung Anmeldung neue erz. MaLo (NB → LF)    | NB → LF    | ↩ Derived from 55601 reject |

> APERAK Frist: 24 h wall-clock (GPKE). PIDs 55602–55605 are derived response
> PIDs; they are never routed inbound — the NB emits them outbound.

### UTILMD Abmeldung LF (GPKE Teil 1)

Workflow `gpke-lf-abmeldung` handles LF-side tracking of an Abmeldung/Kündigung
initiated by the NB that the LF must acknowledge.

| PID   | Process name (AHB)                                    | Direction  | Status         |
|-------|-------------------------------------------------------|------------|----------------|
| 55007 | Kündigung Lieferung durch NB (NB → LF)                | NB → LF    | ✅ Implemented |
| 55008 | Bestätigung Kündigung durch NB (LF → NB)              | LF → NB    | ↩ Derived accept |
| 55009 | Ablehnung Kündigung durch NB (LF → NB)                | LF → NB    | ↩ Derived reject |

### MSCONS Messwerte Strom — Lieferant (GPKE Teil 2/4)

Workflow `gpke-messwerte` accepts inbound MSCONS messages carrying metered values
from the NB or MSB to the LF. These are read-only deliveries; no APERAK response
is required unless the message fails validation.

| PID   | Process name (AHB)                                        | Sender      |
|-------|-----------------------------------------------------------|-------------|
| 13005 | EEG-Überführungszeitreihe                                 | NB → LF     |
| 13006 | Stornierung von Messwerten                                | NB/MSB → LF |
| 13015 | Arbeit Leistungsmax. Kalenderj. vor Lieferbeginn          | NB → LF     |
| 13016 | Energiemenge u. Leistungsmax. Strom                       | NB/MSB → LF |
| 13017 | Zählerstand (Strom)                                       | MSB → LF    |
| 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation      | MSB → LF    |
| 13019 | Energiemenge (Strom)                                      | NB/MSB → LF |
| 13025 | Lastgang Marktlokation, Tranche                           | MSB → LF    |
| 13027 | Werte nach Typ 2 (WiM Strom Teil 2)                       | MSB → LF    |

> All MSCONS PIDs here carry metered data. They are stateless deliveries that
> write no outbox entries on success.

### ORDERS Datenabruf — Anfrage / Ablehnung (GPKE Teil 4)

Workflow `gpke-datenabruf` handles the LF-side of data-request processes: the LF
sends an ORDERS Anfrage to the NB or MSB and waits for a response or explicit
rejection within 24 h.

| PID   | Process name (AHB)                                 | Direction   | Status         |
|-------|----------------------------------------------------|-------------|----------------|
| 17004 | Anfrage Datenabruf (allgemein)                     | LF → NB/MSB | ✅ Implemented |
| 17102 | Anfrage Übermittlung Stammdaten Strom              | LF → NB/MSB | ✅ Implemented |
| 17113 | Anfrage Übermittlung Werte                         | LF → NB/MSB | ✅ Implemented |
| 19101 | Ablehnung Anfrage Datenabruf (NB → LF)             | NB → LF     | ↩ Derived      |
| 19102 | Ablehnung Anfrage Stammdaten (NB → LF)             | NB → LF     | ↩ Derived      |
| 19114 | Ablehnung Anfrage Werte (NB → LF)                  | NB → LF     | ↩ Derived      |

> The response deadline (24 h window `gpke-datenabruf-antwort-24h`) is enforced
> by a `DeadlineStore` entry registered on every outbound ORDERS Anfrage.

### ORDERS Allokationsliste — MSCONS 13014 (GPKE MSCONS Strom)

Workflow `gpke-allokationsliste` handles requests and rejections for the
Allokationsliste, exchanged between LF and NB via ORDERS and answered with MSCONS.

| PID   | Process name (AHB)                                      | Direction   | Status         |
|-------|---------------------------------------------------------|-------------|----------------|
| 17110 | Anfrage Allokationsliste (LF → NB)                      | LF → NB     | ✅ Implemented |
| 17114 | Anfrage Allokationsliste alternativ (LF → NB)           | LF → NB     | ✅ Implemented |
| 19110 | Ablehnung Anfrage Allokationsliste (NB → LF)            | NB → LF     | ↩ Derived      |
| 19115 | Ablehnung alternativ (NB → LF)                          | NB → LF     | ↩ Derived      |
| 13014 | Allokationsliste Strom (NB → LF, MSCONS)                | NB → LF     | ↩ Derived      |

> PIDs 17110/19110 here are Strom (GPKE). The same PID numbers also appear in
> `mako-gabi-gas` for the gas MMMA process (different commodity, different crate).

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

| Rust module                 | Workflow name                    | Contents                                                            |
|-----------------------------|----------------------------------|---------------------------------------------------------------------|
| `wechselprozesse`           | `gpke-supplier-change`           | PIDs 55001–55002, 55017, 55022–55024 (UTILMD supplier-switch + stornierung, NB role) |
| `lf_anmeldung`              | `gpke-lf-anmeldung`              | PIDs 55001/55002/55016/55077 (LF outbound) + 55003–55006/55017–55018/55078/55080 (LF-role receive NB ANTWORT) |
| `lf_abmeldung`              | `gpke-lf-abmeldung`              | PID 55007 (NB → LF Kündigung) + 55008/55009 derived           |
| `neuanlage`                 | `gpke-neuanlage`                 | PIDs 55600/55601 (Neuanlage MaLo, LF → NB) + 55602–55605 derived   |
| `messwerte`                 | `gpke-messwerte`                 | MSCONS PIDs 13005/13006/13015–13019/13025/13027 (Messwerte NB/MSB → LF) |
| `datenabruf`                | `gpke-datenabruf`                | ORDERS 17004/17102/17113 (Anfrage) + ORDRSP 19101/19102/19114 (Ablehnung) |
| `allokationsliste`          | `gpke-allokationsliste`          | ORDERS 17110/17114 + ORDRSP 19110/19115 + MSCONS 13014 (Allokationsliste Strom) |
| `anfrage_bestellung`        | `gpke-anfrage-bestellung`        | PID 55555 (Anfrage Daten der individuellen Bestellung, GPKE Teil 4)  |
| `abrechnung`                | `gpke-abrechnung`                | PIDs 31001/31002/31005/31006 (INVOIC Netznutzungsabrechnung)        |
| `konfiguration`             | `gpke-konfiguration`             | PIDs 17134/17135 (ORDERS outbound) + 19001/19002 (ORDRSP inbound) — GPKE Teil 4 |
| `konfiguration_aenderung`   | `gpke-konfiguration-aenderung`   | ORDERS/ORDRSP for configuration changes (NB role)                   |
| `sperrung`                  | `gpke-sperrung`                  | PIDs 17115–17117 (ORDERS Sperrung Strom, NB → MSB)                 |
| `sperrung_lf`               | `gpke-sperrung-lf`               | ORDRSP 19116/19117 + IFTSTA Sperrung (LF-side ANTWORT receiver)    |
| `ankuendigung_zuordnung_lf` | `gpke-ankuendigung-zuordnung-lf` | PIDs 55607–55609 (UTILMD Ankündigung Zuordnung LF)                 |
| `partin`                    | `gpke-partin`                    | PIDs 37000–37006 (PARTIN Strom Kommunikationsdaten)                |
| `utilts`                    | `gpke-utilts`                    | UTILTS PIDs 25001/25004–25010 (Netzzustandsdaten NB → LF)          |

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
