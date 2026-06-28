# mako-mabis

**MABIS — Marktprozesse für Bilanzkreis- und Aggregationsverantwortliche**

Process engine workflows for the German electricity balance group settlement
processes. Implements PID 13003 (Bilanzkreisabrechnung Strom) from the BDEW
MABIS specification (BNetzA BK6-24-174).

## Process model (BKV perspective)

The **BIKO** (Bilanzkoordinator) is the central actor in MaBiS. It calculates
and sends the `Abrechnungssummenzeitreihe` (billing summary time series) to
each **BKV** (Bilanzkreisverantwortlicher). The BKV must respond with a
`Prüfmitteilung` (positive or negative) within **1 Werktag** (BK6-24-174 §13.8).

```
BIKO                               BKV (this crate)
────                               ────────────────
Abrechnungssummenzeitreihe ──────→ ReceiveSummenzeitreihe
                                       └─ register 1-WT deadline
                           ←──────── SendPruefmitteilungPositiv / Negativ (≤ 1 WT)
Datenstatus                ──────→ ReceiveDatastatus → Settled / Disputed
```

### Key difference from supplier-switch workflows

| Aspect | GPKE / WiM / GeLi Gas | MABIS (this crate) |
|---|---|---|
| Trigger | Single inbound EDIFACT | **Abrechnungssummenzeitreihe from BIKO** |
| Counterparty | NB / LFA | **BIKO (Bilanzkoordinator)** |
| Location scope | Single MeLo / MaLo | **Billing period aggregate** |
| Response Frist | 24 h / 5 Wkt / 10 Wkt | **1 Werktag (§13.8)** |
| Outbound message | APERAK / CONTRL | **Prüfmitteilung** |

## PID Inventory

| PID   | Process name                              | Direction    | Status     |
|-------|-------------------------------------------|--------------|------------|
| 13003 | Bilanzkreisabrechnung Strom (BIKO ↔ BKV)  | inbound BIKO | ✓ implemented |

> **PIDs 13002–13028 are NOT MABIS.** They are Messwerten-PIDs (MSCONS meter data
> exchange) in other domains. Never register 13002–13028 under `"mabis-billing"`.

## EDIFACT Format Versions

| Format version | Valid from | Notes |
|----------------|------------|-------|
| `FV2025-10-01` | 2025-10-01 | MSCONS 2.4c Summenzeitreihen |
| `FV2026-10-01` | 2026-10-01 | MSCONS 2.5 |

## Modules

| Rust module             | Contents                                            |
|-------------------------|-----------------------------------------------------|
| `bilanzkreisabrechnung` | PID 13003 workflow + `BillingProjection` read-model |

## Usage

```rust
use mako_mabis::{MabisBillingWorkflow, BillingCommand, BillingVersion};
use mako_engine::{
    builder::EngineBuilder,
    event_store::InMemoryEventStore,
    types::{BikoId, BillingPeriod, BkvId, MessageRef, Pruefidentifikator},
};

let process = ctx.spawn::<MabisBillingWorkflow>(tenant_id, workflow_id);

// Step 1: BIKO sent Abrechnungssummenzeitreihe
process.execute(BillingCommand::ReceiveSummenzeitreihe {
    pid: Pruefidentifikator::new(13003).unwrap(),
    billing_period: BillingPeriod::new("2025-09"),
    bkv_id: BkvId::new("4033872000022"),
    biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
    version: BillingVersion::Vorlaeufig,
    message_ref: MessageRef::new("MSCONS-BKA-2025-09-001"),
}).await?;

// Register 1-WT deadline (mabis-pruefmitteilung-1-werktag) in deadline store here.

// Step 2: BKV sends positive Prüfmitteilung
process.execute(BillingCommand::SendPruefmitteilungPositiv {
    message_ref: MessageRef::new("PRUEF-POS-2025-09-001"),
}).await?;

// Step 3: BIKO sends Datenstatus
process.execute(BillingCommand::ReceiveDatastatus {
    data_status: mako_mabis::DataStatus::AbgerechtneteDaten,
}).await?;
```

## Regulatory references

- BNetzA **BK6-24-174** — *Marktregeln für die Durchführung der Bilanzkreisabrechnung
  Strom (MaBiS)*, Anlage 3, §13 (Abrechnungsprozess), §13.8 (Prüfmitteilung Frist)
- EDI@Energy MSCONS AHB 2.4c / 2.5 (Summenzeitreihen)

