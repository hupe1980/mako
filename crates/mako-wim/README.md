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

### MSB-Wechsel — UTILMD (BK6-24-174)

| PID   | Process name                                    | EDIFACT       | Module           | Status                          |
|-------|-------------------------------------------------|---------------|------------------|---------------------------------|
| 55042 | Anmeldung MSB (MSBN → NB)                       | UTILMD S2.x   | `geraetewechsel` | ✅ Implemented · Antwort 55043/55044, **5 WT** |
| 55039 | Kündigung MSB (MSBN → **MSBA**)                 | UTILMD S2.x   | `geraetewechsel` | ✅ Implemented · Antwort 55040/55041, **3 WT** |
| 55051 | Ende MSB / Abmeldung (**MSBA → NB**)            | UTILMD S2.x   | `geraetewechsel` | ✅ Implemented · Antwort 55052/55053, **7 WT** |
| 55168 | Verpflichtungsanfrage / Aufforderung (NB → **gMSB**) | UTILMD S2.x | `geraetewechsel` | ✅ Implemented · Antwort 55169/55170, **1 WT** |

### Geräteübernahme — ORDERS / ORDRSP

| PID(s)       | Process name                                      | EDIFACT       | Module               | Status          |
|--------------|---------------------------------------------------|---------------|----------------------|-----------------|
| 17001–17011  | Geräteübernahme (Anfrage, Bestellung, Stornierung) | ORDERS 1.4b  | `geraeteubernahme`   | ✅ Implemented  |
| 19001, 19002 | ORDRSP Bestellbestätigung / Ablehnung (NB → nMSB) | ORDRSP 1.4c  | `geraeteubernahme`   | ✅ Registered (nMSB role only) |
| 19015, 19016 | ORDRSP Gerätewechselabsicht Best./Ablehnung       | ORDRSP 1.4c  | `geraeteubernahme`   | ✅ Registered (nMSB role only) |

> PIDs 19001/19002/19015/19016 are only registered when `DeploymentRoles` includes `Marktrolle::Nmsb`.
> On NB instances these PIDs belong to `mako-gpke` (GPKE Konfiguration). Never register both simultaneously.

### Stammdaten — ORDERS

| PID(s)        | Process name                                     | EDIFACT     | Module       | Status         |
|---------------|--------------------------------------------------|-------------|--------------|----------------|
| 17132         | Stammdaten Anforderung Strom (NB → MSB)          | ORDERS 1.4b | `stammdaten` | ✅ Implemented |
| 17102–17133   | Stammdatenübermittlung responses (MSB → NB)      | ORDERS 1.4b | `stammdaten` | ✅ Implemented |

### Weitere Prozesse

| PID(s)                 | Process name                          | EDIFACT         | Module             | Status         |
|------------------------|---------------------------------------|-----------------|--------------------|----------------|
| 39000                  | Stornierung (ORDCHG)                  | ORDCHG 1.1      | `stornierung`      | ✅ Implemented |
| 31009                  | MSB-Rechnung                          | INVOIC 2.8e     | `rechnung`         | ✅ Implemented (stub, settlement pending) |
| 35001–35005 (REQOTE)   | Preisanfrage — Anfrage (NB → MSB)     | REQOTE 1.3c     | `preisanfrage`     | ✅ Implemented |
| 15001–15005 (QUOTES)   | Preisanfrage — Antwort (MSB → NB)     | QUOTES 1.3c     | `preisanfrage`     | ✅ Implemented |
| 27001–27003            | Preisliste (PRICAT)                   | PRICAT 2.1      | `preisliste`       | ✅ Implemented |
| 23001, 23003, 23004, 23008 | Störungsmeldung (INSRPT, gemeinsam) | INSRPT 1.1a  | `insrpt`           | ✅ Implemented (5 WT Frist) |
| 23011, 23012           | Ergebnisbericht Strom-Variante        | INSRPT 1.1a     | `insrpt`           | ✅ Implemented |
| 11021–11023            | iMS Bestellung (Universalbestellprozess) | REST/JSON    | `steuerungsauftrag`| ✅ Implemented (API-Webdienste channel) |

> PIDs 23005 and 23009 (Gas-only INSRPT variants) always belong to `mako-wim-gas`
> `wim-gas-insrpt` with a 10-Werktage deadline. Never register them in `mako-wim`.

## EDIFACT Format Versions

| Format version | Valid from | Valid until | Profile status |
|----------------|------------|-------------|----------------|
| `FV2024-10-01` | 2024-10-01 | 2025-09-30  | ✓ available    |
| `FV2025-10-01` | 2025-10-01 | 2026-09-30  | ✓ available    |
| `FV2026-10-01` | 2026-10-01 | —           | ✓ available    |

## Modules

| Rust module        | Contents                                                                  |
|--------------------|---------------------------------------------------------------------------|
| `geraetewechsel`   | PIDs 55039, 55042, 55051, 55168 — MSB-Wechsel workflow + projection. Handles both directions and closes the loop: inbound UTILMD (`ReceiveUtilmd` → `Initiated` → APERAK) and ERP-initiated outbound orders (`InitiateDeviceChange` → `AuftragGesendet` → `ReceiveAntwort` → `AuftragBestaetigt`/`Rejected`). ERP command `wim.geraetewechsel.beauftragen`. Antwortfrist per process via `antwort_frist_werktage()`. |
| `geraeteubernahme` | PIDs 17001–17011, 19001/19002/19015/19016 — Geräteübernahme ORDERS/ORDRSP |
| `stammdaten`       | PIDs 17102–17133, 17132 — Stammdaten Anforderung / Übermittlung           |
| `stornierung`      | PID 39000 — Stornierung ORDCHG                                            |
| `rechnung`         | PID 31009 — MSB-Rechnung INVOIC (WiM Strom Teil 1, multi-domain; routed via `wim-rechnung`) |
| `preisanfrage`     | PIDs 35001–35005 (REQOTE), 15001–15005 (QUOTES) — Preisanfrage            |
| `preisliste`       | PIDs 27001–27003 — Preisliste PRICAT                                      |
| `steuerungsauftrag`| PIDs 11021–11023 — iMS Steuerungsauftrag (API-Webdienste REST channel)    |

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
    pid: 55042,  // Anmeldung MSB (nMSB → NB)
    // …
}).await?;
```

## Regulatory references

- BDEW WiM Wechselprozesse im Messwesen Strom
- MsbG — Messstellenbetriebsgesetz
- BNetzA **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — Frist 5 Werktage für APERAK
- EDI@Energy UTILMD Strom AHB S2.2 (`FV2026-10-01`)
- EDI@Energy APERAK AHB 2.2 (`FV2026-10-01`)
