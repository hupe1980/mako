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

> **PIDs 44002–44006, 44017–44018** are registered under
> `geli-gas-supplier-change` and share the same `GeliGasSupplierChangeWorkflow`
> as PID 44001. The state machine currently handles all registered PIDs via
> the same transition logic; separate state machines for Lieferende and
> Kündigung are planned but not yet implemented.
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
> **ORDERS PIDs 17115, 17116, 17117** serve two deployment roles:
> - **LF-Sicht** (`geli-gas-sperrung-lf`): PIDs 17115/17117 are **outbound** (LF
>   initiates the Sperrauftrag). The same PID numbers appear in GPKE (NB inbound);
>   routing is determined by market context and deployment role.
> - **GNB-Sicht** (`geli-gas-sperrung-nb`): PIDs 17115/17117 are **inbound** (GNB
>   receives the Sperrauftrag from LF). PID 17116 is **outbound** (GNB forwards the
>   Anfrage Sperrung to gMSB). This role is only active when the deployment operates
>   as a Gasnetzbetreiber.
>
> **ORDERS PIDs 17103, 17104** are the Gas Datenabruf processes
> (Abrechnungsbrennwert / Zustandszahl and MSB Gas → NB Strom). They are fully
> implemented in `GeliGasDatanabrufWorkflow` with corresponding rejection
> responses via ORDRSP 19103/19104.
>

## EDIFACT Format Versions

| Format version       | Valid from | Valid until | Profile status |
|----------------------|------------|-------------|----------------|
| `FV2024-10-01_gas`   | 2024-10-01 | 2025-09-30  | ✓ available    |
| `FV2025-10-01_gas`   | 2025-10-01 | 2026-09-30  | ✓ available    |
| `FV2026-10-01_gas`   | 2026-10-01 | —           | ✓ available    |

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
| `lieferbeginn` | `geli-gas-supplier-change`  | PIDs 44001–44021 Lieferantenwechsel workflow + projections                         |
| `stornierung`  | `geli-gas-stornierung`      | PID 44022 Nb-only (GNB receives Stornierungsanfrage inbound)                       |
| `lf_stornierung` | `geli-gas-stornierung-lf` | PIDs 44023/44024 Lf-only (LF receives GNB Stornierungsantwort inbound)             |
| `datenabruf`   | `geli-gas-datenabruf`       | PIDs 17103/17104 Gas Datenabruf (ORDERS) + ORDRSP 19103/19104                      |
| `sperrung_lf`  | `geli-gas-sperrung-lf`      | PIDs 17115/17117 Gas Sperrung LF-initiated; ORDRSP 19116/19117/19128/19129; ORDCHG 39000 |
| `sperrung_nb`  | `geli-gas-sperrung-nb`      | PIDs 17115/17116/17117 (GNB receives); ORDERS 17116 → gMSB; ORDRSP 19118/19119; ORDCHG 39000/39001 |
| `sperrprozesse_invoic` | `geli-gas-sperrprozesse-invoic` | PID 31011 (INVOIC AWH Sperrprozesse Gas, NB → LF)                      |
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
waits up to **10 Werktage** for the GNB's ORDRSP response.

```rust
use mako_geli_gas::{
    GeliGasSperrungLfWorkflow, GasSperrungLfCommand, GasSperrungAuftragData,
};
use mako_engine::ids::{MaloId, GlnId};

// Initiate a gas disconnection order (LF → GNB):
let cmd = GasSperrungLfCommand::InitiateSperrung {
    pid: Pruefidentifikator::new(17115).expect("Sperrauftrag"),
    gnb_gln: GlnId::parse("9900357000004").expect("valid GLN"),
    location_id: MaloId::parse("50123456789").expect("valid MaLo"),
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
or rejects execution to the LF. Deadline: **10 Werktage**.

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
// out.deadlines[0] registers the 10-WT response window.

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
use mako_geli_gas::{GeliGasDatanabrufWorkflow, DatanabrufCommand};

// Request billing combustion values (LF → NB/MSB, ORDERS 17103):
let cmd = DatanabrufCommand::InitiateAnfrage {
    pid: Pruefidentifikator::new(17103).expect("valid PID"),
    // …
};
```

## Regulatory references

- BDEW GeLi Gas Geschäftsprozesse Lieferantenwechsel Gas
- BNetzA **BK7-24-01-009** — GeLi Gas 3.0 (Beschluss 12.09.2025, g. 24.09.2025) — APERAK Frist 10 Werktage
- BNetzA BK7-19-001 — previous ruling (superseded)
- BNetzA BK7-06-067 — original GeLi Gas ruling 2007 (superseded)
- EDI@Energy UTILMD Gas AHB G2.x (`FV2026-10-01`)
- EDI@Energy ORDERS/ORDRSP/ORDCHG AHB 1.4b (`FV2026-10-01`)
- EDI@Energy APERAK AHB 2.2 (`FV2026-10-01`)
