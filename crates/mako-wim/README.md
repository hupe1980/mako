# mako-wim

**WiM — Wechselprozesse im Messwesen, Strom und Gas**

Process engine workflows for the German metering-point-operator change
processes in **both Sparten**: **BK6-22-024 Anlagen 2a/2b** (WiM Strom Teil 1
and Teil 2) and the **AWH WiM Gas 2.0** (gültig ab 01.10.2026), plus the
EDI@Energy AHBs that carry them.

## One engine, two Sparten

AWH WiM Gas 2.0 restates WiM Strom Teil 1 use-case for use-case, Frist for
Frist. Only the UTILMD PID namespace splits (55xxx / 44xxx); ORDERS, ORDRSP,
REQOTE, QUOTES, IFTSTA and INSRPT are Sparte-neutral AHBs carrying both. The
Sparte comes from the **recipient MP-ID** (BDEW Allgemeine Festlegungen §2.13)
and decides four things:

| | Strom | Gas |
|---|---|---|
| Antwort-Codeliste | `S_00xx` | `G_00xx` |
| Zuordnungszeitpunkt | 00:00 Uhr | **06:00 Uhr** (Gastag) |
| APERAK | positiv **und** negativ, 45 min für UTILMD/ORDERS | **nur negativ**; nächster WT 12:00 (Folgeprozess) / 3 WT (Initialprozess) |
| Störungs-Fristen | nach Messtechnik | flach |

## Fristen

Three clocks run on an inbound MSB-Wechsel order, and they are separate
messages with separate commands:

| Clock | Window | Message | Source |
|---|---|---|---|
| **APERAK** — technical acknowledgement | Strom **45 Minuten** (UTILMD/ORDERS am Werktag); Gas nächster WT 12:00 bzw. **3 WT** auf einem Initialprozess | APERAK BGM+312/313 (Gas: nur 313) | APERAK AHB 1.1 §2.3.1/§2.4.1 |
| **Antwort** — business Bestätigung/Ablehnung | **per PID**, in beiden Sparten gleich: 55039·44039 → 3 WT, 55042·44042 → 5 WT, 55051·44051 → 7 WT, 55168·44168 → 1 WT | the Antwort-PID's UTILMD | WiM Teil 1 Kap. 2.2.2 / 2.3.2 / 2.4.2 · AWH WiM Gas 2.0 Kap. 3.3.2 / 3.5.2 / 3.6.2 |
| **Vorlauffrist** — was the requested date admissible? | 15 / 7 WT (Anmeldung), 20 WT (Abmeldung), ±9 WT Realisierungskorridor | the date inside the message | WiM Teil 1 Kap. 2.3.2 Nr. 1 / 2.4.2 Nr. 1 |

Only the second discharges the Antwortfrist. The business window comes from
`antwort_frist_werktage(pid)` and the Vorlauffristen from
`mako_fristen::vorlauf` — never a flat value. **Saturday is not a Werktag**
(GPKE Teil 1 Kap. 1.7, which WiM Teil 1 §1.7 defers to), and 24.12. and 31.12.
count as holidays.

## Antwortcodes

Every answer carries `SG4 STS+E01` (UTILMD) or `SG2 AJT` (ORDRSP) with a code
from the process's own Entscheidungsbaum. The catalogue and the executable
Prüfschritte are `mako-pruefung` (`role-msb`); this crate resolves against it
before anything reaches the outbox, so a code from the wrong tree is refused at
the command rather than sent.

**DE 1131 / DE 1082 names the Codeliste, not the Entscheidungsbaum.** The AHB
column reads „Codeliste Strom Nr. `S_0090`" for every WiM MSB-Wechsel answer,
and the *cluster* picks which of the pair. `AntwortCode::wire_codeliste()`
returns the wire value.

| Process | EBD Strom / Gas | Bestätigung / Ablehnung | Codeliste Strom | Gas |
|---|---|---|---|---|
| Kündigung MSB | `E_0200` / `E_2000` | `E15` `Z01` `Z44` / `E11` `Z12` `Z29` `Z34` (+`ZC9` Strom) | `S_0090` / `S_0054` | `G_0052` / `G_0051` |
| Anmeldung MSB | `E_0201` / `E_2002` | `E15` `Z01` `Z44` / `E11` `E17` `Z09` `Z29` `ZB6` (+`ZC9` Strom) | `S_0055` / `S_0056` | `G_0054` / `G_0053` |
| Ende MSB | `E_0202` / `E_2005` | `E15` `Z01` / `E17` `Z09` | `S_0059` / `S_0060` | `G_0058` / `G_0057` |
| Verpflichtungsanfrage | `E_0240` / `E_2006` | `E15` `Z01` `Z44` / `E17` `Z07` `Z09` `ZB6` | `S_0063` / `S_0064` | `G_0070` / `G_0071` |
| Gesamtvorgang | `E_0232` / `E_2003` | — / `Z66` | `S_0057` | `G_0055` |
| Weiterverpflichtung | `E_0203` / `E_2004` | `Z13` `Z14` / `Z22` | `S_0061` / `S_0062` | `G_0072` / `G_0073` |
| Gerätewechselabsicht | `E_0204` / `E_2007` | `ZB4` / `ZB5`; `E17` `Z07` in beiden | `S_0065` / `S_0066` | `G_0059` / `G_0060` |
| Bestellung Geräteübernahme | `E_0247` / `E_2011` | `Z13` / `5` `Z32` | `S_0067` / `S_0068` | `G_0061` / `G_0074` |
| Messlokationsänderung, direkt beauftragt | `E_0249` (NB) / `E_0250` (LF) | `A02` / `A01` (+ `A03` `A04`) | `E_0249` / `E_0250` | — |
| Anfrage Änderung der Technik (35005) | `E_0278` (NB) / `E_0281` (LF) | — / `A01`–`A04` `A99` resp. `A01`–`A06` `A99` | `E_0278` / `E_0281` | — |
| Bestellung Änderung der Technik nach Angebot | `E_0279` (NB) / `E_0283` (LF) | `A06` / `A01`–`A05` `A99` | `E_0279` / `E_0283` | — |
| Messlokationsänderung durchführen | `E_0286` | — / `A01` `A02` `A03` `A99` | `E_0286` | — |
| Ersteinbau iMS (21029 → 21030/21031) | `E_0233` | `A03` / `A01` `A02` `A04` | `E_0233` | — |

None of these alphabets is a GPKE one — `A02` and `A05` appear in no
MSB-Wechsel tree — and the Gas lists are not the Strom lists.

## PID Inventory

> Legend: **✅ Implemented** — full state machine + AHB rule enforcement, production-safe.
> **⚠️ Registered** — PID routes to the workflow; `handle()` returns
> `WorkflowError::NotImplemented` for unhandled commands (no silent data loss).
> **✗ Not registered** — PID is not in the router; inbound messages are dead-lettered.

### MSB-Wechsel — UTILMD (WiM Strom Teil 1 Kap. 2)

| PID   | Process name                                    | EDIFACT       | Module           | Status                          |
|-------|-------------------------------------------------|---------------|------------------|---------------------------------|
| 55042 | Anmeldung MSB (MSBN → NB)                       | UTILMD S2.x   | `geraetewechsel` | ✅ Implemented · Antwort 55043/55044, **5 WT** (*vorläufig*) |
| 55039 | Kündigung MSB (MSBN → **MSBA**)                 | UTILMD S2.x   | `geraetewechsel` | ✅ Implemented · Antwort 55040/55041, **3 WT** |
| 55051 | Ende MSB / Abmeldung (**MSBA → NB**)            | UTILMD S2.x   | `geraetewechsel` | ✅ Implemented · Antwort 55052/55053, **7 WT** |
| 55168 | Verpflichtungsanfrage / Aufforderung (NB → **gMSB**) | UTILMD S2.x | `geraetewechsel` | ✅ Implemented · Antwort 55169/55170, **1 WT** |
| 44039 | Kündigung MSB Gas (MSBN → **MSBA**)             | UTILMD G1.x   | `geraetewechsel` | ✅ Implemented · Antwort 44040/44041, **3 WT** |
| 44042 | Anmeldung MSB Gas (MSBN → NB)                   | UTILMD G1.x   | `geraetewechsel` | ✅ Implemented · Antwort 44043/44044, **5 WT** (*vorläufig*) |
| 44051 | Ende MSB Gas (**MSBA → NB**)                    | UTILMD G1.x   | `geraetewechsel` | ✅ Implemented · Antwort 44052/44053, **7 WT** |
| 44168 | Verpflichtungsanfrage Gas (NB → **gMSB**)       | UTILMD G1.x   | `geraetewechsel` | ✅ Implemented · Antwort **44169 only** — no Ablehnungs-PID exists, **1 WT** |
| 44183 | Ende MSB von NB (Stilllegung, NB → MSB)         | UTILMD G1.x   | `geraetewechsel` | ✅ Implemented · informational, no answer |

### Mitteilung über Gesamtvorgang — IFTSTA

The Anmeldebestätigung 55043 is *vorläufig*. WiM Teil 1 Kap. 2.1.1: the NB assigns
the MSBN „zu dem Tag des vom MSBN mitgeteilten Termins des erfolgreichen Abschlusses
des Gesamtvorgangs … mit dem Zeitpunkt 00:00 Uhr", and the MSBA's assignment ends at
the same instant. AWH WiM Gas 2.0 Kap. 3.1.1 says the same with **06:00 Uhr** —
the Gastag boundary. This leg is what makes the Wechsel constitutive.

| PID   | Process name                                        | Von → An            | Frist |
|-------|-----------------------------------------------------|---------------------|-------|
| 21010 | Statusmeldung (**erfolgreich**), `DTM+2380`         | MSBN → NB           | 10 WT nach dem bestätigten Zuordnungsbeginn |
| 21009 | Statusmeldung (**gescheitert**)                     | MSBN → NB           | — |
| 21012 | Statusmeldung (erfolgreich) — die Zuordnung         | NB → MSBN           | 1 WT |
| 21011 | Statusmeldung (MSB-Scheitermeldung, `Z66`)          | NB → MSBN/MSBA/LF   | 1 WT |
| 21013 | Statusmeldung (gescheitert) — keine Meldung eingegangen | NB → MSBN/MSBA/LF | 11 WT |

> The numeric order is the reverse of the reading order: **21009 is the failure and
> 21010 the success** (IFTSTA AHB 2.1 § 6.2).
>
> Every failure path leaves the MSBA assigned. `marktd` derives the per-Messlokation
> MSB timeline from 21012 alone.

### Geräteübernahme — ORDERS / ORDRSP

| PID(s)       | Process name                                      | EDIFACT       | Module               | Status          |
|--------------|---------------------------------------------------|---------------|----------------------|-----------------|
| 17001        | Bestellung Geräteübernahmeangebot (MSBN → MSBA)   | ORDERS 1.4b   | `geraeteubernahme`   | ✅ Implemented · Antwort 19001/19002, **2 WT** |
| 17009        | Anzeige Gerätewechselabsicht (MSBN → MSBA)        | ORDERS 1.4b   | `geraeteubernahme`   | ✅ Implemented · Antwort 19015/19016, **2 WT vor dem Wechseltermin** |
| 19001, 19002 | ORDRSP Bestellbestätigung / Ablehnung (MSBA → MSBN) | ORDRSP 1.4c | `geraeteubernahme`   | ✅ Registered (nMSB role only) |
| 19015, 19016 | ORDRSP Eigenausbau ja/nein (MSBA → MSBN)          | ORDRSP 1.4c   | `geraeteubernahme`   | ✅ Registered (nMSB role only) |
| 17002 → 19003/19004 | Weiterverpflichtung des MSB (**NB → MSBA**) | ORDERS 1.4b   | `weiterverpflichtung`| ✅ Implemented · **1 WT**, `E_0203` |

> The **Anforderung** eines Geräteübernahmeangebots is REQOTE 35001, answered by
> QUOTES 15001 within **4 WT** — `preisanfrage` owns that leg.
>
> 19016 is named „Ablehnung Gerätewechselabsicht" but carries `ZB5` „Kein
> Eigenausbau des MSBA": it settles who removes the old device, not whether the
> Gerätewechsel happens.

> PIDs 19001/19002/19015/19016 are only registered when `DeploymentRoles` includes `Marktrolle::Nmsb`.
> On NB instances these PIDs belong to `mako-gpke` (GPKE Konfiguration). Never register both simultaneously.
>
> All six ORDERS/ORDRSP PIDs carry the Strom **and** the Gas Use-Case; the
> recipient MP-ID's Sparte picks `E_0247`/`E_0204` against `E_2011`/`E_2007`.

### Stammdaten — ORDERS

| PID(s)        | Process name                                     | EDIFACT     | Module       | Status         |
|---------------|--------------------------------------------------|-------------|--------------|----------------|
| 17132         | Stammdaten Anforderung Strom (NB → MSB)          | ORDERS 1.4b | `stammdaten` | ✅ Implemented |
| 17102–17133   | Stammdatenübermittlung responses (MSB → NB)      | ORDERS 1.4b | `stammdaten` | ✅ Implemented |

### Weitere Prozesse

| PID(s)                 | Process name                          | EDIFACT         | Module             | Status         |
|------------------------|---------------------------------------|-----------------|--------------------|----------------|
| 39002                  | ESA Stornierung der Bestellung von Werten (ORDCHG) | ORDCHG 1.1 | `wertebestellung`  | ✅ Implemented |
| 31009                  | MSB-Rechnung (MSB → NB/LF/ESA)        | INVOIC 2.8e     | `invoic`           | ✅ Implemented (send + receive) · Antwort **zum Zahlungsziel**, an den NB **4. WT davor** |
| 33001–33004 (REMADV)   | Zahlungsavis / itemized Abweisung     | REMADV 1.0a     | `invoic`           | ✅ Implemented (33003/34 = Strom Kopf+Summe / Position) |
| 29001 (COMDIS)         | Ablehnung REMADV                      | COMDIS 1.0      | `invoic`           | ✅ Implemented · **2. WT vor dem Zahlungsziel** |
| 35001 → 15001 (REQOTE/QUOTES) | Anforderung Geräteübernahmeangebot (MSBN → MSBA) | REQOTE 1.3c | `preisanfrage` | ✅ Implemented · **4 WT** |
| 35002 → 15002 | Anfrage Rechnungsabwicklung über den LF (LF → MSB) | REQOTE 1.3c | `preisanfrage` | ✅ Implemented · **5 WT** |
| 35004 → 15004 | Anfrage einer Konfiguration (GPKE Teil 3, NB/LF → MSB) | REQOTE 1.3c | `preisanfrage` | ✅ Implemented · **2 WT** |
| 35005 → 15005 | Anfrage Angebot Änderung Technik (NB/LF → MSB) | REQOTE 1.3c | `preisanfrage` | ✅ Implemented · **10 WT** |
| 17005/17006 → 19009/19010 | Rechnungsabwicklung MSB über LF | ORDERS/ORDRSP | `rechnungsabwicklung` | ✅ Implemented · **8 WT** |
| 27001–27003            | Preisliste (PRICAT)                   | PRICAT 2.1      | `preisliste`       | ✅ Implemented · Preisblatt an LF **3 Monate** vor Wirksamwerden, an NB initial **3 WT** / bei Änderung **20 WT** vor Inkrafttreten |
| 23001, 23003, 23004, 23008 | Störungsmeldung / Antwort / Ergebnisbericht | INSRPT 1.1a | `insrpt`     | ✅ Implemented, **beide Seiten** · Antwort Strom **3 WT** (kME ohne RLM, mME) / **1 WT** (kME mit RLM, iMS), Gas flach **3 WT**; Ergebnis Strom **7/4/2 WT** nach Messtechnik und Spannungsebene, Gas flach **7 WT** |
| 23005, 23009           | Informationsmeldungen an den NB (Gas) | INSRPT 1.1a     | `insrpt`           | ✅ Implemented |
| 23011, 23012           | Weiterleitung an betroffene Marktlokationen (Strom) | INSRPT 1.1a | `insrpt`     | ✅ Implemented · **1 WT**, und das Fenster überlebt den Ergebnisbericht — die 23012 ist danach fällig |
| 31003 → 33001/33002    | WiM-Rechnung (MSBA → NB / MSBN), beide Sparten | INVOIC 2.8e | `invoic`        | ✅ Implemented · Rechnung **20. WT** nach Leistungsende, Antwort **zum Zahlungsziel** |
| 31004 → 33001/33002    | Stornorechnung, Sparte-neutral        | INVOIC 2.8e     | `invoic`           | ✅ Implemented |
| 11021–11023            | iMS Bestellung (Universalbestellprozess) | REST/JSON    | `steuerungsauftrag`| ✅ Implemented (API-Webdienste channel) |

## EDIFACT Format Versions

| Format version | Valid from | Valid until | Profile status |
|----------------|------------|-------------|----------------|
| `FV2024-10-01` | 2024-10-01 | 2025-09-30  | ✓ available    |
| `FV2025-10-01` | 2025-10-01 | 2026-09-30  | ✓ available    |
| `FV2026-10-01` | 2026-10-01 | —           | ✓ available    |

## Modules

| Rust module        | Contents                                                                  |
|--------------------|---------------------------------------------------------------------------|
| `geraetewechsel`   | PIDs 55039/55042/55051/55168 and their Gas twins 44039/44042/44051/44168, plus 44183 and the IFTSTA Gesamtvorgang leg 21009–21013 — MSB-Wechsel workflow + projection. Handles both directions: inbound UTILMD (`ReceiveUtilmd` → APERAK → `DispatchAntwort` → `ReceiveGesamtvorgang` → `DispatchZuordnung`) and ERP-initiated outbound orders (`InitiateDeviceChange` → `ReceiveAntwort` → `MeldeGesamtvorgang` → `ReceiveZuordnungsantwort`). Antwortfrist per process via `antwort_frist_werktage()`; the Realisierungskorridor is enforced on the Gesamtvorgang date. |
| `geraeteubernahme` | ORDERS 17001 → ORDRSP 19001/19002 (Bestellbestätigung/Ablehnung) and ORDERS 17009 → 19015/19016 (Eigenausbau ja/nein) — WiM Teil 1 Kap. 3.1/3.2 |
| `weiterverpflichtung` | ORDERS 17002 → ORDRSP 19003/19004 — the NB keeping the abgebender MSB on the Messlokation while the gMSB prepares to take over (Kap. 2.4.2 Nr. 5/6, `E_0203`) |
| `technik_aenderung` | REQOTE 35005 → QUOTES 15005 / IFTSTA 21033, ORDERS 17011/17118 → ORDRSP 19005/19006, IFTSTA 21025/21027 — Messlokationsänderung auf **beiden** Wegen; **10 WT** Antwort, **20 WT** Vorlauffrist nur auf der direkten Beauftragung (Kap. 3.3 / AWH Änd. Technik) |
| `ersteinbau` | IFTSTA 21029 → 21030/21031 — Ersteinbau eines iMS in eine bestehende Messlokation, **3 WT** Antwort aus `E_0233` (Kap. 3.5, Strom only) |
| `stammdaten`       | PIDs 17102–17133, 17132 — Stammdaten Anforderung / Übermittlung           |
| `wertebestellung`  | PIDs 35003/15003/17007/17008, ORDCHG 39002 (Stornierung, answered by ORDRSP 19013/19014), ORDRSP 19011/19012, IFTSTA 21042 — **ESA Wertebestellung** (WiM Teil 2 Kap. 4): Anfrage → Angebot → Bestellung → Stornierung/Abbestellung, plus MSB-initiated termination. Fristen keyed on the positive AS4-Zustellquittung (ÜT); answers carry an `E_0254`/`E_0256`/`E_0257` Antwortcode. |
| `invoic`           | PIDs 31009 (MSB-Rechnung, Strom) · 31003 (Abrechnung von Dienstleistungen, beide Sparten) · 31004 (Stornorechnung, Sparte-neutral). Both sides: **MSB** sends via `SendInvoic` (invoicer, awaits REMADV); **NB/LF/ESA** ingests via `ReceiveInvoic` then settles/disputes. Inbound REMADV 33001–33004 (incl. the Strom itemized Abweisungen 33003/34) + COMDIS 29001. Routed via `wim-invoic`; replies use conversation-ID correlation (RFF+Z13 → 31009 ref) so they resume this family even when the shared REMADV PID statically resolves to GPKE. The state machine is `mako-invoic`'s, shared with the GPKE, GaBi Gas and GeLi Gas billing families; this module declares only the family and the Gas Ablehnungs-Entscheidungsbaum. |
| `preisanfrage`     | PIDs 35001/35002/35004/35005 (REQOTE), 15001/15002/15004/15005 (QUOTES) — Preisanfrage            |
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
that authorises value delivery and the one that stops it. Both sides are
modelled: `wertebestellung` (MSB) and `esa_wertebestellung` (ESA), over disjoint
PID sets so one deployment may hold both roles.

| UC step | Direction | Message | PID | Frist | EBD |
|---|---|---|---|---|---|
| 4.1 Nr. 1 Anfrage | ESA → MSB | REQOTE | 35003 | — | `E_0252` |
| 4.1 Nr. 2 Angebot / Ablehnung | MSB → ESA | QUOTES | 15003 | **5 WT** nach ÜT der Anfrage | — |
| 4.1 Nr. 3 Bestellung | ESA → MSB | ORDERS | 17007 | bis Ablauf der **Bindungsfrist** | — |
| 4.1 Nr. 4 Antwort | MSB → ESA | ORDRSP | 19011 / 19012 | **2 WT** nach ÜT der Bestellung | `E_0256` |
| 4.1 Nr. 5 Stornierung | ESA → MSB | ORDCHG | 39002 | unverzüglich | — |
| 4.1 Nr. 6 Antwort | MSB → ESA | ORDRSP | 19013 / 19014 | **2 WT** nach ÜT der Stornierung | `E_0257` |
| 4.2 Werteübermittlung | MSB → ESA | MSCONS | 13027 | per Messprodukt | — |
| 4.3 Nr. 1 Abbestellung | ESA → MSB | ORDERS | 17008 | unverzüglich | — |
| 4.3 Nr. 2 Antwort | MSB → ESA | ORDRSP | 19011 / 19012 | **2 WT** nach ÜT der Abbestellung | `E_0254` |
| 4.4 Nr. 1 Beendigung durch MSB | MSB → ESA | IFTSTA | 21042 (`STS+Z21` 4405 = 105) | unverzüglich | — |

### What is ordered

The [`esa`](src/esa.rs) module holds the *Codeliste der Konfigurationen* 1.4
Kapitel 4.6 catalogue — the only Messprodukte the role may order — as data:
delivery path (4.6.1 EDIFACT back-end vs 4.6.2 SM-PKI from the iMS),
Lokationsebene, Werteart, Energieflussrichtung, cadence, and whether BNetzA
*Mitteilung Nr. 3* makes the product mandatory. A [`Bestellgegenstand`] pairs a
Messprodukt-Code with the `DTM+76` Wunschtermin and the `IMD+7081` Abonnement
mode, and is carried through both aggregates: without it the process could not
say what a confirmed delivery is supposed to contain.

A subscription is the **(Meldepunkt, Messprodukt) pair** (`esa::business_key`) —
one Marktlokation can carry several products at once, so every follow-up message
and command has to say which one it means.

An order is validated against the catalogue before it leaves the system — a
product outside Kapitel 4.6, one defined for a different Lokationsebene than the
request addresses, or a 4.6.2 product without its SM-PKI target is refused.

**The product decides the Lokationsebene** (`esa::ebene_fuer_messprodukt`).
REQOTE AHB 1.2 §4.3 gives `LOC+172` DE 3225 four permitted shapes and lets the
Marktlokations-ID format (`[950]`) serve both the Marktlokation (hint `[502]`)
and the **Tranche** (hint `[504]`), so an identifier cannot resolve the level —
and the Tranche carries a Pflichtprodukt (`9991 00000 306 4`).

**„Pflicht" and „nutzbar ab" are two different dates**, and both are dates rather
than flags: `9991 00000 077 1` and `078 9` read „Optional ab 01.10.2023, Pflicht
ab 06.08.2024" while being usable from 01.10.2023. `Verbindlichkeit::Pflicht { ab }`
carries its start and `Messprodukt::ist_pflicht_am` is what `E_0252` Prüfschritt 1
asks — a Vergangenheitswerte-Anfrage may reach back before the cut-over.

### The Angebot is a priced offer

UC 4.1.1 has the ESA asking for „die Übermittlung von Werten **und die damit
verbundenen Kosten**". QUOTES AHB 1.1a §4.3 makes the substance Muss — `SG4 CUX`,
one to three `SG27 PIA+Z02` Artikel-IDs, one `SG31 PRI+CAL` each (`Z01`
Einrichtungs- / `Z02` Transaktions- / `Z03` Betriebspreis) and one to 23
`PIA+5 … :SRW` OBIS-Kennzahlen — so [`esa::Angebot`] carries all of it and the
process keeps it past the Bestellung: it is what the INVOIC 31009 is checked
against and what a delivery-surveillance sweep expects to arrive.

It is also the **discriminator**. `DTM+273` is Muss on the only published 15003
use case, so a refusal carries a Bindungsfrist too; the priced position is what
tells an Angebot from an Ablehnung.

### The Prüfidentifikator is not in BGM

`BGM` DE 1004 is a **Dokumentennummer** throughout these handbooks; the PID
travels in `SG1 RFF+Z13`. DE 1001 carries a BDEW document code: `Z57` on the
order handshake, `Z83` on the MSCONS 13027 delivery, `Z09` on the IFTSTA 21042.

### Correlation

Only the opening REQOTE is keyed on a location. A conformant ORDERS, ORDCHG,
ORDRSP or IFTSTA of Kapitel 4 carries **no `LOC` at all** and correlates by a
Belegnummer, under the Zuordnungsschlüssel the BDEW *Anwendungsübersicht der
Prüfidentifikatoren* 4.0 publishes per PID:

| PID | Schlüssel | Segment | Points at |
|---|---|---|---|
| 35003 | `ZO-T17` | `SG11 LOC+172` | the Meldepunkt |
| 15003 | `ZG-T16` | `SG1 RFF+AAV` | the REQOTE |
| 17007 | `ZG-T24` | `SG1 RFF+AAG` | the QUOTES Angebot |
| 17008 | `ZG-T41` | `SG1 RFF+ACW` | the ORDERS Bestellung |
| 39002 | `ZG-T51` | `SG1 RFF+ON` | the ORDERS Bestellung |
| 19011 / 19012 | `ZG-T14` | `SG1 RFF+ON` | the ORDERS answered |
| 19013 / 19014 | `ZG-T50` | `SG1 RFF+ACW` | the ORDCHG |
| 21042 | `ZG-T47` | `SG15 RFF+AGI` | the ORDERS Bestellung |
| 13027 | `ZG-T42` (of `EZ-03`) | `SG1 RFF+AGI` | the ORDERS Bestellung |

`esa::korrelation` is that table; the renderer and the ingest dispatcher both
read it, so the qualifier they emit and the one they look for cannot drift. Note
that the same `AGI` qualifier sits in a different segment group per message —
`SG15` on the IFTSTA, `SG1` on the MSCONS — which is why the lookup searches by
qualifier rather than by position. `EZ-03` continues past it to `ZO-T20`
(Gerätenummer) and `ZO-T21` (OBIS-Kennzahl); those two assign the values to a
Zählwerk and belong to `edmd`.

### Answers are Antwortcodes, not booleans

`SG2 AJT` is Muss on all four answer PIDs (ORDRSP AHB 1.1b §4.15) and carries the
Prüfschritt code in DE 4465 with its EBD in DE 1082. Conditions [17]/[18] require
the code to sit in that tree's Zustimmungs- resp. Ablehnungs-Cluster, so **the
cluster selects the answer PID**. The MSB commands therefore take an
`antwort_code` resolved against [`mako_pruefung::esa::wertebestellung`], never an `accept`
flag alongside it.

**And it is the whole content of a refusal on the receiving side too.** Those
four use cases publish **no free-text segment at all**: the only `FTX` a
conformant 19011 may carry is `SG27 FTX+Z27` — the MSB's IP address, Muss when
the confirmed order named a Kapitel-4.6.2 SMGW product (`FTX+Z28` for a range),
and the source the ESA has to admit before the iMS can reach it. So the ESA side
records [`esa::Antwort`], the typed `(Antwortcode, EBD)` pair, rather than prose:
that is what lets it tell `A08` (Einwilligung abgelaufen — renew and re-order)
from `A10` (Lokationsbündel — split the request) from `A09` (Gerätetechnik —
nothing to retry). `Antwort::widerspricht_pid` flags a code whose Cluster
contradicts the PID that carried it; such an answer is recorded as a conflict and
resolved **by the PID**, since resolving it by the code would silently turn a
confirmation into a refusal.

### Every inbound step notifies

The MSB-side workflow emits a `ProcessInitiated` outbox entry on each of
35003/17007/39002/17008. That notification is the entire input to `processd`'s
ESA module — the four `mako-pruefung` walks, the operator queue and its Fristen —
and its payload is a contract: every field is a Prüfschritt input, so an omitted
one does not fail, it escalates a decision that could have been answered. In
particular `abo_beginn` (the confirmed `DTM+203` of the Bestellung) is what
`E_0254` Prüfschritt 2 compares a requested end against.

### What a subscription remembers

Both aggregates keep `lieferung_begonnen` in their process **data**, not inside
the state variant that happens to be current. It is a fact about the
subscription: it has to survive a Storno or Abbestellung round trip, and a first
delivery landing while a 39002 is in flight is exactly the case `E_0257` `A02`
exists for. The MSB side additionally remembers its own Bindungsfrist (`E_0256`
Prüfschritt 1 asks about it one state later), the Abo start, the reach of its
latest delivery and any date it already ended on — the three inputs `E_0254`
Prüfschritte 2–4 need.

An Angebot whose Bindungsfrist lapses is **terminal** on the ESA side: UC 4.1
Nr. 3 admits no Bestellung after it, so the process releases its (Meldepunkt,
Messprodukt) business key rather than blocking every replacement order.

19011/19012 answer both the Bestellung and the Beendigung; the `IMD+7081` on the
answer is what says which tree its code came from.

### Stornierung and Abbestellung are not interchangeable

UC 4.1 Nr. 5 admits a Stornierung only while the einmalige Übermittlung has not
happened or the turnusmäßige has not begun; UC 4.3's Vorbedingung then states
*"Eine Stornierung der Bestellung ist nicht mehr möglich"*.
`MarkLieferungBegonnen` flips the state that enforces this. On the MSB side the
two trees make the boundary explicit: `E_0254` `A01` refuses a Beendigung of a
one-shot order, and `E_0257` refuses a Stornierung of a started delivery with
**different codes** per Abo mode (`A02` Abo, `A03` einmalig).

## Regulatory references

- BDEW WiM Wechselprozesse im Messwesen
- MsbG — Messstellenbetriebsgesetz
- BNetzA **BK6-22-024** Anlagen 2a/2b (WiM Strom Teil 1 und Teil 2)
- BDEW/VKU/GEODE/FNBGas **AWH WiM Gas 2.0** (gültig ab 01.10.2026)
- EDI@Energy UTILMD Strom AHB S2.2 (`FV2026-10-01`)
- EDI@Energy **APERAK AHB 1.1** (`FV2026-10-01`) — § 2.4.1 Strom, § 2.3.1 Gas.
  2.2 is the APERAK **MIG** revision; AHB and MIG carry different version numbers
  for every message type except UTILMD
