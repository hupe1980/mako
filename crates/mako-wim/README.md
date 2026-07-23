# mako-wim

**WiM — Wechselprozesse im Messwesen Strom**

Process engine workflows for the German electricity metering system change
processes. Implements the BDEW WiM specification and the BNetzA ruling
**BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025).

## APERAK Frist

WiM processes use **5 Werktage** (`fristen::add_werktage(5, BdewMaKo)`) for
the APERAK response deadline. Saturdays, Sundays and public holidays are not
Werktage; 24.12. and 31.12. count as holidays.

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
| `geraeteubernahme` | ORDERS 17001/17002/17009 → QUOTES 15001, ORDRSP 19001/19002 (Bestellbestätigung/Ablehnung), 19003/19004 (Fortführung), 19015/19016 (Gerätewechselabsicht) — WiM Teil 1 Kap. 3.2 |
| `stammdaten`       | PIDs 17102–17133, 17132 — Stammdaten Anforderung / Übermittlung           |
| `stornierung`      | PID 39002 — Stornierung der Bestellung von Werten (ORDCHG); answers ORDRSP 19013/19014 |
| `wertebestellung`  | PIDs 35002/15003/17007, ORDRSP 19011/19012 — **ESA Wertebestellung** (WiM Teil 2 Kap. 4): Anfrage → Angebot → Bestellung → Abbestellung, plus MSB-initiated termination. Fristen keyed on the positive AS4-Zustellquittung (ÜT). |
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

## Geräteübernahme (WiM Teil 1, Kapitel 3.2)

| Step | Direction | Message | PID | Frist |
|---|---|---|---|---|
| 1 Anforderung Geräteübernahmeangebot | MSBN → MSBA | REQOTE | — | — |
| 2 Geräteübernahmeangebot | MSBA → MSBN | QUOTES | 15001 | **4 WT** nach ÜT von Nr. 1 |
| 3 Bestellung | MSBN → MSBA | ORDERS | 17001 | **3 WT** nach ÜT von Nr. 2 |
| 4 Bestellbestätigung | MSBA → MSBN | ORDRSP | 19001 / 19002 | **2 WT** nach ÜT von Nr. 3 |
| 5 Zählerstand zur Geräteübernahme | MSBA → MSBN | MSCONS | — | 3 WT vor Ablauf des 28. T |

Adjacent processes sharing the workflow: ORDERS 17002 (Weiterverpflichtung MSBA)
answered by ORDRSP 19003/19004, and ORDERS 17009 (Ankündigung
Gerätewechselabsicht) answered by ORDRSP 19015/19016.

## ESA Wertebestellung (WiM Teil 2, Kapitel 4)

§34 Abs. 2 S. 2 Nr. 10 MsbG makes serving an Energieserviceanbieter a mandatory,
non-discriminatory Zusatzleistung, so an MSB must be able to process the order
that authorises value delivery and the one that stops it.

| UC step | Direction | Message | PID | Frist |
|---|---|---|---|---|
| 4.1 Nr. 1 Anfrage | ESA → MSB | REQOTE | 35002 | — |
| 4.1 Nr. 2 Angebot / Ablehnung | MSB → ESA | QUOTES | 15003 | **5 WT** nach ÜT der Anfrage |
| 4.1 Nr. 3 Bestellung | ESA → MSB | ORDERS | 17007 | bis Ablauf der **Bindungsfrist** |
| 4.1 Nr. 4 Antwort | MSB → ESA | ORDRSP | 19011 / 19012 | **2 WT** nach ÜT der Bestellung |
| 4.1 Nr. 5 Stornierung | ESA → MSB | ORDCHG | 39002 | unverzüglich |
| 4.1 Nr. 6 Antwort | MSB → ESA | ORDRSP | 19013 / 19014 | **2 WT** nach ÜT der Stornierung |
| 4.3 Nr. 1 Abbestellung | ESA → MSB | ORDERS | 17007 | unverzüglich |
| 4.3 Nr. 2 Antwort | MSB → ESA | ORDRSP | 19011 / 19012 | **2 WT** nach ÜT der Abbestellung |
| 4.4 Nr. 1 Beendigung durch MSB | MSB → ESA | — | — | unverzüglich |

**Bestellung and Abbestellung share PID 17007** ("Bestellung und Abbestellung von
Werten ESA"), and 19011/19012 answer both. They are separate commands because
they are admissible at different points in the lifecycle.

**Stornierung and Abbestellung are mutually exclusive.** UC 4.1 Nr. 5 admits a
Stornierung only while the einmalige Übermittlung has not happened or the
turnusmäßige has not begun; UC 4.3 Vorbedingung then states *"Eine Stornierung
der Bestellung ist nicht mehr möglich"*. `MarkLieferungBegonnen` flips the state
that enforces this, and a late Stornierung is refused with a pointer to UC 4.3.

### Routing: REQOTE 35002 is shared

No ESA-specific REQOTE Prüfidentifikator exists in any published format version,
so an ESA Werteanfrage and a Preisanfrage arrive under the **same PID 35002**.
WiM Teil 2 Kap. 4 resolves this at content level — footnote 5 requires *"die
entsprechenden Codes der zugehörigen Anwendungsfälle in der Codeliste der
Messprodukte"*.

`classify_reqote` uses two signals, strongest first:

1. **The sender's market role.** An ESA is registered via PARTIN 37006
   ("Kommunikationsdaten des ESA Strom"), so a REQOTE from a party in that role
   is decisively a Werteanfrage.
2. **A Messprodukt identifier in `PIA`.** A Werteanfrage names the product it
   wants delivered; a Preisanfrage asks for a price sheet and carries none.

With neither signal the message stays a Preisanfrage, preserving existing
routing. The function is parser-free: the caller extracts the `PIA` codes, so
`mako-wim` keeps no dependency on the EDIFACT reader.

The role signal needs the ESA counterparty market-partner IDs, since a NAD segment
carries only the party code, not the role. Supply them to `makod` with
`--esa-partner-mp-ids` (or `MAKOD_ESA_PARTNER_MP_IDS`); without them only the
`PIA` marker is active.

### Role-gated registration

| Deployment role | PIDs registered |
|---|---|
| `MSB` | ORDERS **17007** inbound — the order that authorises delivery and the one that stops it |
| `ESA` | QUOTES **15003**, ORDRSP **19011/19012/19013/19014** inbound — the answers the MSB sends |

The two sets are disjoint (pinned by a test), so an integrated deployment holding
both roles registers both without tripping the router's conflict guard.

### Fristen are keyed on the ÜT, not on parse time

GPKE Teil 1 defines the **ÜT** as *"Tag des Empfangs der Übertragungsdatei.
Dieser Tag ist aus der AS4-Zustellquittung zu entnehmen"*, and restricts it to a
**positive** acknowledgement: *"Für die Fristenberechnung ist der Tag nur
anwendbar, sofern es sich um eine positive Zustellquittung bzw. Response-Nachricht
handelt."*

`Zustellquittung` therefore carries the acknowledgement explicitly, and
`Zustellquittung::frist` refuses to compute a deadline from a negative one — a
Frist counted from an unacknowledged transmission is one the market partner is
not bound by.

## Regulatory references

- BDEW WiM Wechselprozesse im Messwesen Strom
- MsbG — Messstellenbetriebsgesetz
- BNetzA **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — Frist 5 Werktage für APERAK
- EDI@Energy UTILMD Strom AHB S2.2 (`FV2026-10-01`)
- EDI@Energy APERAK AHB 2.2 (`FV2026-10-01`)
