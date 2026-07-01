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
| 17115   | Gas-Sperrauftrag (LF → GNB) — outbound             | ORDERS 1.4b   | ✅ Implemented                    |
| 17117   | Gas-Entsperrauftrag (LF → GNB) — outbound          | ORDERS 1.4b   | ✅ Implemented                    |
| 19116   | Bestätigung Sperr-/Entsperrauftrag (GNB → LF)      | ORDRSP 1.4    | ✅ Implemented                    |
| 19117   | Ablehnung Sperr-/Entsperrauftrag (GNB → LF)        | ORDRSP 1.4    | ✅ Implemented                    |
| 19128   | Bestätigung Stornierung Sperr-/Entsperrauftrag      | ORDRSP 1.4    | ✅ Implemented                    |
| 19129   | Ablehnung Stornierung Sperr-/Entsperrauftrag        | ORDRSP 1.4    | ✅ Implemented                    |
| 39000   | Stornierung Sperr-/Entsperrauftrag (LF → GNB)      | ORDCHG 1.1    | ✅ Implemented — outbound          |
| 37008   | Kommunikationsdaten des LF Gas                      | PARTIN 1.1    | ✅ Implemented                   |
| 37009   | Kommunikationsdaten des GNB Gas                     | PARTIN 1.1    | ✅ Implemented                   |
| 37010   | Kommunikationsdaten des gMSB Gas                    | PARTIN 1.1    | ✅ Implemented                   |
| 37011   | Kommunikationsdaten des MGV Gas                     | PARTIN 1.1    | ✅ Implemented                   |
| 37012   | Spartenübergreifende Kommunikationsdaten des GNB    | PARTIN 1.1    | ✅ Implemented                   |
| 37013   | Spartenübergreifende Kommunikationsdaten des gMSB   | PARTIN 1.1    | ✅ Implemented                   |
| 37014   | Spartenübergreifende Kommunikationsdaten des MSB Strom | PARTIN 1.1 | ✅ Implemented                   |
| 17003   | Beauftragung Änderung Technik (MeLo Gas)            | ORDERS 1.4b   | ✗ Not registered                 |
| 17101   | Anfrage Übermittlung Stammdaten Gas                 | ORDERS 1.4b   | ✗ Not registered                 |
| 39001   | Weiterleitung der Stornierung                       | ORDCHG 1.1    | ✗ Not registered                 |
| 39002   | Stornierung der Bestellung von Werten               | ORDCHG 1.1    | ✗ Not registered                 |

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
> **ORDERS PIDs 17103, 17104** are the Gas Datenabruf processes
> (Abrechnungsbrennwert / Zustandszahl and MSB Gas → NB Strom). They are fully
> implemented in `GeliGasDatanabrufWorkflow` with corresponding rejection
> responses via ORDRSP 19103/19104.
>
> **ORDERS PIDs 17115, 17117** are the outbound Gas Sperrung / Entsperrung
> requests (LF → GNB). They are initiated by `GeliGasSperrungLfWorkflow` and
> NOT registered in the inbound PID router (the LF never receives these).
> The same PID numbers are used for the analogous Strom process in GPKE
> (NB-role inbound); routing is determined by market context and deployment
> role.
>
> **ORDRSP PIDs 39001, 39002** are cancellation processes (Weiterleitung,
> Bestellung) applicable to other Gas processes and are unregistered.

## EDIFACT Format Versions

| Format version       | Valid from | Valid until | Profile status |
|----------------------|------------|-------------|----------------|
| `FV2024-10-01_gas`   | 2024-10-01 | 2025-09-30  | ✓ available    |
| `FV2025-10-01_gas`   | 2025-10-01 | 2026-09-30  | ✓ available    |
| `FV2026-10-01_gas`   | 2026-10-01 | —           | ✓ available    |

## Modules

| Rust module    | Contents                                                                  |
|----------------|-----------------------------------------------------------------------|
| `lieferbeginn` | PIDs 44001–44006, 44007–44021 Lieferantenwechsel workflow + projections  |
| `datenabruf`   | PIDs 17103, 17104 Gas Datenabruf (ORDERS) + ORDRSP 19103, 19104           |
| `sperrung_lf`  | PIDs 17115, 17117 Gas Sperrung LF-initiated; ORDRSP 19116, 19117, 19128, 19129; ORDCHG 39000 (outbound Stornierung) |
| `partin`       | PIDs 37008–37014 Gas Kommunikationsdaten (LF, GNB, gMSB, MGV, ÜNB) — auto-upsert into `PartnerStore` |

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
                                          └──SendStornierung──► StornierungGesendet ──ReceiveOrdrspStorno(confirm)──► StornoBestaetigt
                                                                                     └──ReceiveOrdrspStorno(reject)──► StornoAbgelehnt
                                          └──TimeoutExpired──► DeadlineExpired
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
