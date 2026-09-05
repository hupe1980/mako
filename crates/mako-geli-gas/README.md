# mako-geli-gas

**GeLi Gas — Geschäftsprozesse Lieferantenwechsel Gas**

Process engine workflows for the German gas market supplier-switch processes.
Implements the BDEW GeLi Gas specification:
- **GeLi Gas 3.0** — BNetzA **BK7-24-01-009** (Beschluss 12.09.2025, Tenor ab 01.01.2026)

This supersedes BK7-19-001 and the original BK7-06-067 (2007).

A **Prüfidentifikator** (PID) is the five-digit BDEW code every message in these
processes carries. It names the exact Anwendungsfall, and with it the rules, the
Frist and the answer tree that apply — so the PID, not the EDIFACT message type,
is what routes.

## APERAK

Gas knows only the **Verarbeitbarkeitsfehlermeldung**: „Die APERAK informiert den
Absender eines Geschäftsvorfalls ausschließlich darüber, dass … Fehler gefunden
wurden" (APERAK AHB 1.1 §2.3). A processable message is acknowledged by the
Frist lapsing in silence, and every APERAK is answered with a CONTRL.

| Prozessart | Frist | Helper |
|---|---|---|
| Folgeprozess | nächster Werktag 12:00 Uhr | `fristen::aperak_gas_folgeprozess_due_at` |
| Initialprozess (`ZO-F` in der PID-Übersicht: 44001, 44016) | **3 Werktage** | `fristen::aperak_gas_initialprozess_due_at` |

This is the *technical* clock. The business Antwortfristen are per
Prüfidentifikator in `mako_fristen::antwort`. Saturdays, Sundays and gesetzliche
Feiertage are not Werktage.

## Key differences from the electricity processes

Both Sparten switch suppliers at the **Marktlokation** and both are driven by
UTILMD. What actually differs:

| Aspect | GPKE (Strom) | GeLi Gas |
|---|---|---|
| Festlegung | BK6-24-174 (Teil 1–3), BK6-22-024 Anlage 1d (Teil 4) | **BK7-24-01-009** (GeLi Gas 3.0) |
| Antwortfrist shape | wall-clock instant on the 1. WT nach dem ÜT (07:00 / 09:00 / 11:00 / 12:00) | **Ablauf des 4. / 3. / 2. Werktags** nach Eingang |
| Zuordnungszeitpunkt | 00:00 Uhr | **06:00 Uhr** — the Gastag runs 06:00–06:00 |
| Vorlauffrist des LF | — | **10 WT** Anmeldung, **7 WT** Abmeldung, bei Lieferantenwechsel |
| Entscheidungsbäume | `E_06xx` | **`E_30xx`** |
| APERAK | Anerkennungs- *und* Verarbeitbarkeitsfehlermeldung; 45 min für UTILMD/ORDERS | **nur Verarbeitbarkeitsfehlermeldung**; nächster WT 12:00 / 3 WT |
| CONTRL | nur auf eine syntaktisch defekte APERAK | auf **jede** APERAK |
| EDIFACT profile | UTILMD Strom S2.1 / S2.2 | UTILMD Gas G1.1 / G1.2 |
| Grid operator | Netzbetreiber (NB) | Gasnetzbetreiber (GNB) |

### „10 Werktage" is the supplier's Vorlauffrist, not an answer window

„Bei Anmeldungen anlässlich eines Lieferantenwechsels erfolgt dies mindestens 10
Werktage vor Aufnahme der Belieferung" (GeLi Gas 3.0 Kap. 3.2.3) says how far
ahead the **LFN must send**. The GNB's answer window on the same message is
**4 Werktage**; the Abmeldung pairs a 7-Werktage lead time with a 3-Werktage
answer window. Sizing an answer queue with the lead time reports a lapsed Frist
as still running for six Werktage — see
`mako_fristen::antwort::TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST`.

The AWH GeLi Gas V1.2 Kap. 2.5.2 Nr. 5 refines the 4 Werktage further: where an
Abmeldeanfrage was sent, the GNB answers within **24 h of the LFA's reply**
(shifted to the next Werktag when that lands on a weekend), capped at the 4. WT.
`mako_fristen::antwort::gas_lieferbeginn_antwort_nach_abmeldeanfrage` computes
that sub-window; the PID-keyed table publishes the cap, which is all it can
state without knowing whether an Abmeldeanfrage went out.

### Meldepflichten — obligations with no answer

| PID | Message | NB → | Frist |
|---|---|---|---|
| 44036 | Informationsmeldung über existierende Zuordnung — **die Identität des LFA** | LFN | Ablauf des 4. WT nach Eingang |
| 44037 | Informationsmeldung zur Beendigung der Zuordnung | LFA | am selben Tag wie die Antwort |
| 44038 | Informationsmeldung zur Aufhebung einer zuk. Zuordnung | LFZ | am selben Tag wie die Antwort |

`geli-gas-zuordnungsmeldung` renders all three; `processd` issues 44036 and
44037 as part of the Anmeldung decision (`geli.zuordnung.informieren` /
`.beenden`) and 44038 is a command. The catalogue in `mako_fristen::meldung` is
cross-checked against the PID router by
`services/makod/tests/meldepflicht_coverage.rs` — the guard has to be a test,
because nothing waits for these and no timeout can fire.

**Two anchors, not one.** 44036 counts from the Eingang der Anmeldung like its
Strom twin; 44037 and 44038 are „am selben Tag wie in Prozessschritt 5, wenn die
Anmeldung bestätigt wurde" — anchored on the GNB's own Antwort, and owed only on
a confirmation. Resolving them against the Eingang gives a different day
whenever the GNB uses more than a few hours of its four Werktage.

**Where the wire differs from Strom.** All three are `BGM+E44`
Informationsmeldung (not `E01`/`E02`); every Lokation is `SG5 LOC+172`
Meldepunkt (not `Z16`/`Z21`); the Gründe are narrower — `ZC8` alone on the
Beendigung, no `ZG5` on the Aufhebung, so the Gas Aufhebung *always* names an
auslösenden Marktpartner in `SG12 NAD+VY`; and `SG4 DTM+159` Bilanzierungsende
is Soll on 44037/44038 „wenn eine Bilanzierung stattfindet", a slot Strom has
no counterpart for.

### The Sperrprozesse are not in GeLi Gas at all

GeLi Gas 3.0's chapters are Kündigung, Lieferende, Lieferbeginn,
Ersatz-/Grundversorgung and the Annexprozesse — **there is no Sperr- or
Entsperrprozess in it**. The Gas Sperrprozesse live in the BDEW AWH
„Unterbrechung / Wiederherstellung der Anschlussnutzung" (Gas-Entscheidungsbäume
`E_1000` / `E_1004`, against Strom's `E_0470` / `E_0497`). Because
17115 / 17117 / 19116 are Sparte-neutral ORDERS
Anwendungsfälle, `mako_fristen::antwort` resolves them from one row each — **1
Werktag**, sourced from BK6-24-174 GPKE Teil 2 § 3.5, the only text on hand that
quantifies them.

## PID Inventory

> Legend: **✅ Implemented** — full state machine + AHB rule enforcement, production-safe.
> **⚠️ Registered** — PID routes to the workflow; partial handling in current code.
> **✗ Not registered** — PID is not in the router; inbound messages are dead-lettered.

| PID     | Process name                                        | EDIFACT       | Status                            |
|---------|-----------------------------------------------------|---------------|-----------------------------------|
| 44001   | Lieferbeginn Gas — Anfrage LFN → NB                 | UTILMD G1/G2  | ✅ Implemented                    |
| 44004   | Abmeldung NN / Lieferende Gas — Anfrage LFN → NB     | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44002   | Bestätigung Anmeldung NN — NB → LF                  | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44003   | Ablehnung Anmeldung NN — NB → LF                    | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44005   | Bestätigung Abmeldung NN — NB → LF                  | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44006   | Ablehnung Abmeldung NN — NB → LF                    | UTILMD G1/G2  | ⚠️ Registered — partial handling |
| 44013   | Anmeldung / Zuordnung EOG (§36/§38 EnWG) — GNB → LF | UTILMD G1/G2  | ✅ Implemented (`EogAnmeldung` variant) |
| 44014   | Bestätigung EOG Anmeldung — LF → GNB                | UTILMD G1/G2  | ↩ Derived from 44013 accept |
| 44015   | Ablehnung EOG Anmeldung — LF → GNB                  | UTILMD G1/G2  | ↩ Derived from 44013 reject |
| 44016   | Kündigung Lieferbeginn Gas — LFN → LFA              | UTILMD G1/G2  | ✅ Sent (`geli.kuendigung.anmelden`) and answered |
| 44017   | Bestätigung Kündigung Lieferbeginn Gas — LFA → LFN  | UTILMD G1/G2  | ↩ Derived from 44016 accept |
| 44018   | Ablehnung Kündigung Lieferbeginn Gas — LFA → LFN    | UTILMD G1/G2  | ↩ Derived from 44016 reject |
| 17103   | Anfrage Abrechnungsbrennwert / Zustandszahl         | ORDERS 1.4b   | ✅ Implemented                    |
| 17104   | Anfrage MSB Gas an NB Strom                         | ORDERS 1.4b   | ✅ Implemented                    |
| 19103   | Ablehnung Anfrage Brennwert / Zustandszahl          | ORDRSP 1.4    | ✅ Implemented                    |
| 19104   | Ablehnung Anfrage vom MSB Gas                       | ORDRSP 1.4    | ✅ Implemented                    |
| 17115   | Gas-Sperrauftrag — outbound (LF → GNB) · inbound (GNB receives from LF) | ORDERS 1.4b   | ✅ Implemented (both roles)          |
| 17116   | Anfrage Sperrung (GNB → gMSB) — outbound GNB-side                         | ORDERS 1.4b   | ✅ Implemented                    |
| 17117   | Gas-Entsperrauftrag — outbound (LF → GNB) · inbound (GNB receives from LF) | ORDERS 1.4b   | ✅ Implemented (both roles)          |
| 19116   | Bestätigung Sperr-/Entsperrauftrag (GNB → LF)      | ORDRSP 1.4    | ✅ Implemented                    |
| 19117   | Ablehnung Sperr-/Entsperrauftrag (GNB → LF)        | ORDRSP 1.4    | ✅ Implemented                    |
| 19118   | Bestätigung Anfrage Sperrung (gMSB → GNB)           | ORDRSP 1.4    | ✅ Implemented                    |
| 19119   | Ablehnung Anfrage Sperrung (gMSB → GNB)             | ORDRSP 1.4    | ✅ Implemented                    |
| 19128   | Bestätigung Stornierung Sperr-/Entsperrauftrag      | ORDRSP 1.4    | ✅ Implemented                    |
| 19129   | Ablehnung Stornierung Sperr-/Entsperrauftrag        | ORDRSP 1.4    | ✅ Implemented                    |
| 39000   | Stornierung Sperr-/Entsperrauftrag (LF → GNB)      | ORDCHG 1.1    | ✅ Implemented                    |
| 39001   | Weiterleitung Stornierung (GNB → gMSB) — outbound  | ORDCHG 1.1    | ✅ Implemented                    |
| 37008   | Kommunikationsdaten des LF Gas                      | PARTIN 1.1    | ✅ Implemented                   |
| 37009   | Kommunikationsdaten des GNB Gas                     | PARTIN 1.1    | ✅ Implemented                   |
| 37010   | Kommunikationsdaten des gMSB Gas                    | PARTIN 1.1    | ✅ Implemented                   |
| 37011   | Kommunikationsdaten des MGV Gas                     | PARTIN 1.1    | ✅ Implemented                   |
| 37012   | Spartenübergreifende Kommunikationsdaten des GNB    | PARTIN 1.1    | ✅ Implemented                   |
| 37013   | Spartenübergreifende Kommunikationsdaten des gMSB   | PARTIN 1.1    | ✅ Implemented                   |
| 37014   | Spartenübergreifende Kommunikationsdaten des MSB Strom | PARTIN 1.1 | ✅ Implemented                   |
| 17003   | Beauftragung Änderung Technik (MeLo Gas)            | ORDERS 1.4b   | ✗ Not registered                 |
| 17101   | Anfrage Übermittlung Stammdaten Gas                 | ORDERS 1.4b   | ✗ Not registered                 |

> **The whole 44001–44021 band** (`lieferbeginn::UTILMD_PIDS`) is registered
> under `geli-gas-supplier-change` and shares one `GeliGasSupplierChangeWorkflow`.
> Beyond the rows above that means **44007–44009** (Abmeldung NN vom NB),
> **44010–44012** (Abmeldungsanfrage des NB) and **44019–44021** (Bestandsliste /
> Änderungsmeldung and its Antwort) — the eight Anfrage-PIDs of that band are
> `lieferbeginn::ANFRAGE_PIDS`, the rest are its Antworten.
>
> **PIDs 44036/44037/44038** are the Informationsmeldungen of the Meldepflichten
> section above and route to `geli-gas-zuordnungsmeldung`; nothing answers them.
>
> **ORDERS PIDs 17003, 17101** are Gas-specific Stammdaten and
> Zählpunktverwaltung Gas processes defined in ORDERS AHB 1.4b. None are
> currently registered in `mako-geli-gas`; inbound messages are dead-lettered.
>
> **ORDCHG PID 39002** (Stornierung der Bestellung von Werten) is not
> currently registered. PID 39001 (Weiterleitung Stornierung, GNB → gMSB)
> is now outbound from `geli-gas-sperrung-nb`.
>
> **ORDERS PIDs 17115, 17116, 17117** are fully dual-role:
> - **LF-Sicht** (`geli-gas-sperrung-lf`): LF sends 17115/17117 outbound; receives
>   ORDRSP 19116/19117 and Storno-ORDRSP 19128/19129 inbound.
> - **GNB-Sicht** (`geli-gas-sperrung-nb`): GNB receives 17115/17117 inbound from LF;
>   sends 17116 outbound to gMSB; receives ORDRSP 19118/19119 inbound from gMSB.
>   After gMSB confirms, GNB sends ORDRSP 19116/19117 back to LF via outbox.
> **ORDERS PIDs 17103, 17104** are the Gas Datenabruf processes
> (Abrechnungsbrennwert / Zustandszahl and MSB Gas → NB Strom). They are fully
> implemented in `GeliGasDatenabrufWorkflow` with corresponding rejection
> responses via ORDRSP 19103/19104.
>

## EDIFACT Format Versions

| Format version       | Valid from | Valid until | UTILMD Gas profile |
|----------------------|------------|-------------|--------------------|
| `FV2026-04-01_gas`   | 2026-04-01 | 2026-09-30  | AHB 1.1, MIG G1.1  |
| `FV2026-10-01_gas`   | 2026-10-01 | —           | AHB 1.2, MIG G1.2  |

The AHB and the MIG carry different version numbers for every message type
except UTILMD, where the AHB is `1.2` and the MIG release is `G1.2` — the same
document generation under two numbering schemes.

## MSCONS Messdaten Gas — GNB/gMSB to LFG

Workflow `geli-gas-mscons` receives inbound MSCONS messages that carry gas
metering values from the GNB or gMSB to the LF. These are read-only deliveries
on the retail gas side; no APERAK response is required unless validation fails.

| PID   | Process name (AHB)                                       | Sender        |
|-------|----------------------------------------------------------|---------------|
| 13002 | Energiemenge Gas (GNB → LF)                              | GNB → LF      |
| 13007 | Lastgang Gas (GNB / gMSB → LF)                           | GNB/gMSB → LF |
| 13008 | Tageslosmenge Gas (GNB → LF)                             | GNB → LF      |
| 13009 | Messwerte Gas (gMSB → LF)                                | gMSB → LF     |

> All four PIDs carry metered gas quantities under the GeLi Gas framework
> (BK7-24-01-009). They are routed to `geli-gas-mscons` on any deployment
> that includes the GeLi Gas module.

## Modules

| Rust module    | Workflow name               | Contents                                                                           |
|----------------|-----------------------------|------------------------------------------------------------------------------------|
| `lieferbeginn` | `geli-gas-supplier-change`  | PIDs 44001–44021 (`UTILMD_PIDS`) Lieferantenwechsel workflow + projections, GNB role |
| `lf_anmeldung` | `geli-gas-lf-anmeldung`     | LF role: 44001/44004/44016 outbound, 44002/44003 · 44005/44006 · 44017/44018 inbound |
| `zuordnungsmeldung` | `geli-gas-zuordnungsmeldung` | PIDs 44036/44037/44038 (Informationsmeldungen um den Lieferbeginn); one-way, no Antwortnachricht |
| `stornierung`  | `geli-gas-stornierung`      | PID 44022 Nb-only (GNB receives Stornierungsanfrage inbound)                       |
| `lf_stornierung` | `geli-gas-stornierung-lf` | PIDs 44023/44024 Lf-only (LF receives GNB Stornierungsantwort inbound)             |
| `datenabruf`   | `geli-gas-datenabruf`       | PIDs 17103/17104 Gas Datenabruf (ORDERS) + ORDRSP 19103/19104                      |
| `sperrung_lf`  | `geli-gas-sperrung-lf`      | PIDs 17115/17117 Gas Sperrung LF-initiated; ORDRSP 19116/19117/19128/19129; ORDCHG 39000 |
| `sperrung_nb`  | `geli-gas-sperrung-nb`      | PIDs 17115/17116/17117 (GNB receives); ORDERS 17116 → gMSB; ORDRSP 19118/19119; ORDCHG 39000/39001 |
| `invoic`       | `geli-gas-sperrprozesse-invoic` | PID 31011 (INVOIC AWH Sperrprozesse Gas). **Both roles:** the GNB issues one (`SendInvoic` → REMADV correlation), the LFG receives one (`ReceiveInvoic` → settle/dispute). The state machine is `mako-invoic`'s, shared with the GPKE, WiM and GaBi Gas billing families; this module declares only the family. |
| `stammdatenaenderung` | `geli-gas-stammdatenaenderung` | GeLi Gas Stammdatenänderung 44109–44182 — inbound MaLo change → Zustimmung (E15, apply) / Ablehnung (E13/E17); Monatserster rule for bilanzierungsrelevante changes; 10-WT Antwort-Frist |
| `mscons`       | `geli-gas-mscons`           | PIDs 13002/13007/13008/13009 (MSCONS Messdaten Gas, GNB/gMSB → LF)               |
| `partin`       | `geli-gas-partin`           | PIDs 37008–37014 Gas Kommunikationsdaten (LF, GNB, gMSB, MGV, ÜNB)               |

## Usage

### Lieferantenwechsel Gas

```rust
use mako_geli_gas::{GeliGasSupplierChangeWorkflow, GasSupplierChangeCommand};
use mako_engine::{builder::EngineBuilder, event_store::InMemoryEventStore};

// In production, explicitly provide all stores:
let ctx = EngineBuilder::with_stores(outbox, deadline, registry)
    .with_event_store(my_slatedb_store)
    .build();

let process = ctx.spawn::<GeliGasSupplierChangeWorkflow>(tenant_id, workflow_id);
let out = process.execute(GasSupplierChangeCommand::ReceiveUtilmd {
    pid: Pruefidentifikator::new(44001).expect("valid PID"),
    // …
}).await?;
```

### Gas Sperrung / Entsperrung (LF-initiated)

The `GeliGasSperrungLfWorkflow` models the LF-side of the gas disconnection /
reconnection process per BK7-24-01-009. The LF initiates the process by sending
an ORDERS 17115 (Sperrauftrag) or 17117 (Entsperrauftrag) to the GNB and then
waits for the GNB's ORDRSP — due „spätester ÜT ist der **1. WT** nach dem ÜT"
on the Sparte-neutral 17115 / 17117 row (`mako_fristen::antwort`).

```rust
use mako_geli_gas::{
    GeliGasSperrungLfWorkflow, GasSperrungLfCommand, GasSperrungAuftragData,
};
use mako_engine::{ids::MaloId, types::MarktpartnerCode};

// Initiate a gas disconnection order (LF → GNB):
let cmd = GasSperrungLfCommand::InitiateSperrung {
    pid: Pruefidentifikator::new(17115).expect("Sperrauftrag"),
    gnb_gln: MarktpartnerCode::new("9900357000004"),
    location_id: MaloId::parse("50123456721").expect("valid MaLo"),
    message_ref: MessageRef::from("MSG-2025-001"),
};
let out = process.execute(cmd).await?;
// out.outbox[0] carries the ORDERS 17115 message for AS4 dispatch.

// When the GNB confirms (ORDRSP 19116):
let confirmed = GasSperrungLfCommand::ReceiveOrdrsp {
    pid: Pruefidentifikator::new(19116).expect("Bestätigung"),
    is_confirmed: true,
    message_ref: MessageRef::from("MSG-GNB-001"),
};
let out = process.execute(confirmed).await?;
// Process transitions to OrdrspBestaetigt (terminal).
```

State transitions:

```
New ──InitiateSperrung──► AuftragGesendet ──ReceiveOrdrsp(confirm)──► OrdrspBestaetigt
                                          └──ReceiveOrdrsp(reject)──► OrdrspAbgelehnt
                                          └──SendStornierung──► StornierungGesendet ──ReceiveStornoOrdrsp(confirm)──► StornoBestaetigt
                                                                                     └──ReceiveStornoOrdrsp(reject)──► StornoAbgelehnt
                                          └──TimeoutExpired──► DeadlineExpired
```

### Gas Sperrung / Entsperrung (GNB-side)

The `GeliGasSperrungNbWorkflow` models the GNB-side of the gas disconnection /
reconnection process per BK7-24-01-009. The GNB receives the Anweisung from the
LF (ORDERS 17115/17117), optionally forwards a meter-access request to the gMSB
(ORDERS 17116), waits for the gMSB's ORDRSP (19118/19119), and then confirms
or rejects execution to the LF. Deadline: the ORDRSP by the **1. WT nach dem
ÜT**; the gMSB's answer to a 17116 Anfrage Sperrung by the **3. WT**, with
silence counting as consent.

```rust
use mako_geli_gas::{
    GeliGasSperrungNbWorkflow, GasSperrungNbCommand,
};

// AS4 adapter receives ORDERS 17115 from LF:
let cmd = GasSperrungNbCommand::ReceiveSperrung {
    pid: Pruefidentifikator::new(17115).expect("Sperrauftrag"),
    sender: MarktpartnerCode::new("4012345000023"),
    location_id: MaLo::new("DE00123456789012345678901234567890"),
    document_date: "20250601".to_owned(),
    message_ref: MessageRef::new("MSG-LF-001"),
    validation_passed: true,
    validation_errors: vec![],
};
let out = process.execute(cmd).await?;
// out.deadlines[0] registers the 1-WT ORDRSP response window.

// gMSB confirms access (ORDRSP 19118):
let msb = GasSperrungNbCommand::ReceiveMsbAntwort {
    pid: Pruefidentifikator::new(19118).expect("gMSB Bestätigung"),
    is_confirmed: true,
    message_ref: MessageRef::new("MSG-MSB-001"),
};
let _ = process.execute(msb).await?;

// GNB confirms execution to LF:
let confirm = GasSperrungNbCommand::BestaetigueSperrung {
    durchgefuehrt: true,
    reason: None,
};
let out = process.execute(confirm).await?;
// out.outbox carries the ORDRSP 19116 back to LF via AS4.
// Process transitions to Ausgefuehrt (terminal).
```

State transitions:

```
New ──ReceiveSperrung(valid)──► ValidationPassed ──BestaetigueSperrung──► Ausgefuehrt
                                                  └──ReceiveStornierung──► Storniert
                                                  └──TimeoutExpired──► (terminal)
    └──ReceiveSperrung(invalid)──► Rejected
```

### Gas Datenabruf (Brennwert / Zustandszahl)

```rust
use mako_geli_gas::{GeliGasDatenabrufWorkflow, DatenabrufCommand};

// Request billing combustion values (LF → NB/MSB, ORDERS 17103):
let cmd = DatenabrufCommand::InitiateAnfrage {
    pid: Pruefidentifikator::new(17103).expect("valid PID"),
    // …
};
```

## Regulatory references

- BDEW GeLi Gas Geschäftsprozesse Lieferantenwechsel Gas
- BNetzA **BK7-24-01-009** — GeLi Gas 3.0 (Beschluss 12.09.2025, Tenor ab 01.01.2026);
  Antwortfristen 4 / 3 / 2 WT, LF-Vorlauffristen 10 WT (Anmeldung) und 7 WT (Abmeldung).
  **Enthält keinen Sperrprozess** — der steht in der BDEW AWH „Unterbrechung /
  Wiederherstellung der Anschlussnutzung" (`E_1000` / `E_1004`)
- BDEW/VKU/GEODE/FNB Gas **AWH GeLi Gas V1.2** (26.03.2026, gültig ab 01.04.2026) —
  die Sequenzdiagramme, u. a. der Zwei-Zweig-Zuschnitt von Prozessschritt 5
- BNetzA BK7-19-001 — previous ruling (superseded)
- BNetzA BK7-06-067 — original GeLi Gas ruling 2007 (superseded)
- EDI@Energy UTILMD Gas **AHB 1.2** (MIG release G1.2, `FV2026-10-01`)
- EDI@Energy ORDERS/ORDRSP **AHB 1.1b** (MIG release 1.4c) and ORDCHG **AHB 1.1**
  (MIG release 1.2), `FV2026-10-01`
- EDI@Energy APERAK AHB 1.1 § 2.3.1 — Gas: nächster Werktag 12:00 (Folgeprozess),
  3 Werktage (Initialprozess)

## Related crates

The format layer and the domain packs meet in `makod`: a workflow crate knows the
`Pruefidentifikator` and its own domain types, never an EDIFACT message type.

| Crate | Role |
|---|---|
| [`mako-geli-gas`](https://docs.rs/mako-geli-gas) ← **this crate** | GeLi Gas workflows, PID routing, `GeliGasModule` |
| [`edi-energy`](https://docs.rs/edi-energy) | EDI@Energy EDIFACT — parse · validate · build (UTILMD, MSCONS, ORDERS, INVOIC, APERAK, …); joined to these workflows in `makod`, not depended on |
| [`mako-engine`](https://docs.rs/mako-engine) | Event-sourced workflow runtime — `Workflow`, `Process`, `EventStore`, deadlines |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-invoic`](https://docs.rs/mako-invoic) | The INVOIC settle/dispute state machine every billing family shares |
| [`mako-gpke`](https://docs.rs/mako-gpke) | The Strom counterpart — GPKE |
| [`mako-gabi-gas`](https://docs.rs/mako-gabi-gas) | The other half of the gas market: balancing, not supplier switching |
| [`makod`](https://hupe1980.github.io/mako/docs/services/makod/) | Production daemon — routes, adapts and renders these workflows |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
