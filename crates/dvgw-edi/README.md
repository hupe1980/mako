# dvgw-edi

**DVGW EDIFACT format parser for the German gas transport and balancing market**

Implements parsing of DVGW-governed EDIFACT formats used in GaBi Gas 2.1
(BNetzA BK7-24-01-008). The crate is the DVGW counterpart to `edi-energy`, which
covers BDEW EDI@Energy (UTILMD, MSCONS, INVOIC, APERAK, …).

## Supported formats

| Message | Version | Valid from | UN/EDIFACT base | Description |
|---|---|---|---|---|
| `ALOCAT` | 5.11a | 2024-10-01 | D03A | Allokationsnachricht — gas quantity allocation |
| `NOMINT` | 4.6 FK | 2026-02-01 | D01B | Nominierungsintegration — nomination submission |
| `NOMRES` | 4.7 FK | 2026-02-01 | D01B | Nominierungsantwort — nomination response |
| `SCHEDL` | G685/G2000 | — | D03A | Schedulingnachricht — transport schedule (FNB → BKV) |
| `IMBNOT` | G685/G2000 | — | D03A | Imbalance Notification — intraday imbalance (FNB/MGV → BKV) |
| `TRANOT` | G685/G2000 | — | D03A | Transport Notification — capacity restriction or event (FNB/VNB → BKV/GH/MGV) |
| `DELORD` | G685/G2000 | — | D03A | Delivery Order — delivery nomination (BKV → FNB) |
| `DELRES` | G685/G2000 | — | D03A | Delivery Response — FNB confirmation/rejection of DELORD (FNB → BKV) |

**FK** = Fehlerkorrektur — editorial correction only; no structural change.

## Quick start

```rust
use dvgw_edi::{DvgwPlatform, AnyDvgwMessage, DvgwMessage};

let platform = DvgwPlatform::default();
let msg = platform.parse(edi_bytes)?;

if let AnyDvgwMessage::Nomint(nomint) = msg {
    // nomination_ref from BGM document number — use to correlate the NOMRES
    println!("nomination ref:  {:?}", nomint.nomination_ref);
    println!("sender EIC:      {:?}", nomint.sender_eic());
    for qty in &nomint.quantities {
        println!("  {} {} {}", qty.location_code, qty.quantity,
                 qty.unit.as_deref().unwrap_or("?"));
    }
}

// For ALOCAT — always use quantity_decimal() for gas billing precision
if let AnyDvgwMessage::Alocat(alocat) = msg {
    for qty in &alocat.quantities {
        // Preferred: Decimal arithmetic per DVGW G 685 §7 (≥ 3 dp required)
        if let Some(kwh) = qty.quantity_decimal() {
            println!("  {} kWh_Hs (Decimal)", kwh);
        }
        // Avoid: quantity_f64() loses precision on large gas quantities
    }
}
```

## Message routing

DVGW messages carry no BGM Prüfidentifikator. Routing uses the combination of
message type and the direction qualifier extracted from the NAD+MS/MR role codes.

Use `AnyDvgwMessage::detect_pid(role_qualifier)` to obtain the synthetic PID for
registration with the `mako-engine` PID router:

```rust
// After parsing — BKV → FNB nomination
let pid = msg.detect_pid(Some("Z01")); // → Some(90011)
```

### Synthetic PID table (range `90000–90999`)

| PID   | Message | Direction |
|-------|---------|-----------|
| 90001 | ALOCAT  | FNB → BKV (daily allocation) |
| 90002 | ALOCAT  | MGV → BKV (monthly allocation) |
| 90003 | ALOCAT  | VNB → FNB (sub-daily allocation) |
| 90011 | NOMINT  | BKV → FNB (nomination) |
| 90012 | NOMINT  | BKV → MGV (nomination) |
| 90021 | NOMRES  | FNB → BKV (nomination response) |
| 90022 | NOMRES  | MGV → BKV (nomination response) |
| 90031 | SCHEDL  | FNB → BKV (schedule) |
| 90041 | IMBNOT  | MGV → BKV (intraday imbalance) |
| 90051 | TRANOT  | FNB → BKV (transport notification) |
| 90061 | DELORD  | BKV → FNB (delivery order) |
| 90062 | DELRES  | FNB → BKV (delivery response) |

The range `90000–90999` is reserved exclusively for DVGW synthetic PIDs and will
never collide with BDEW PIDs (10000–99999, documented in PID 3.3 / PID 4.0).

## NOMINT/NOMRES correlation

1. BKV sends **NOMINT** — `nomination_ref` holds the BGM document number.
2. FNB/MGV responds with **NOMRES** — `nomination_ref` holds the `RFF+Z13` value
   that back-references the originating NOMINT.

Match `nomres.nomination_ref == nomint.nomination_ref` to correlate the response
to the outbound nomination workflow.

## DELORD/DELRES correlation

1. BKV sends **DELORD** — `order_ref` holds the BGM document number.
2. FNB responds with **DELRES** — `order_ref` holds the `RFF+Z13` value that
   back-references the originating DELORD.

Match `delres.order_ref == delord.order_ref` to correlate the delivery response
to the outbound delivery order workflow.

`delres.status` carries the overall disposition (`Accepted`, `Modified`, or
`Rejected`). Per-location detail is in `delres.lines`.

## Feature flags

| Feature   | Default | Description |
|-----------|---------|-------------|
| `alocat`  | ✅ on   | Enable `AlocatMessage` and ALOCAT parsing |
| `nomint`  | ✅ on   | Enable `NomintMessage` and NOMINT parsing |
| `nomres`  | ✅ on   | Enable `NomresMessage` and NOMRES parsing |
| `schedl`  | ✅ on   | Enable `SchedlMessage` and SCHEDL parsing |
| `imbnot`  | ✅ on   | Enable `ImbalanceMessage` and IMBNOT parsing |
| `tranot`  | ✅ on   | Enable `TransportNotificationMessage` and TRANOT parsing |
| `delord`  | ✅ on   | Enable `DeliveryOrderMessage` and DELORD parsing |
| `delres`  | ✅ on   | Enable `DeliveryResponseMessage` and DELRES parsing |
| `decimal` | ✅ on   | Add `AlocatQuantity::quantity_decimal()` returning `rust_decimal::Decimal` |
| `serde`   | ❌ off  | Add `serde::Serialize` / `Deserialize` to all public value types |
| `tracing` | ❌ off  | Emit structured tracing spans during parse dispatch |

> **`quantity_decimal()` vs `quantity_f64()`** — Always prefer `quantity_decimal()`
> for gas energy values. DVGW G 685 §7 requires ≥ 3 decimal places of precision;
> `f64` cannot represent all gas quantities exactly. `quantity_f64()` is retained
> for legacy/diagnostic use only.

## Market roles

| Role | Abbrev. | Description |
|---|---|---|
| Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
| Verteilnetzbetreiber | VNB | Gas distribution system operator |
| Bilanzkreisverantwortlicher | BKV | Balance responsible party |
| Marktgebietsverantwortlicher | MGV | Market area manager |

## Acknowledgements (CONTRL / APERAK)

`dvgw-edi` does not reimplement CONTRL or APERAK. These are handled by the
`edi-energy` crate and shared across all message families. See
"Ergänzungsblatt zur APERAK und CONTRL für die Nutzung in GaBi Prozessen"
on [edi-energy.de](http://www.edi-energy.de/).

## Relationship to other crates

| Crate | Layer |
|---|---|
| `dvgw-edi` | EDIFACT parsing (ALOCAT, NOMINT, NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD, DELRES) — **this crate** |
| `mako-gabi-gas` | GaBi Gas process engine (INVOIC billing + all DVGW transport workflows) |
| `edi-energy` | BDEW EDI@Energy (UTILMD, MSCONS, INVOIC, APERAK, CONTRL, …) |
| `mako-engine` | Event-sourced workflow runtime |

## Regulatory basis

| Document | Scope |
|---|---|
| **GasNZV** | Statutory basis for gas network access and balancing (§ 20 Abs. 1b EnWG) |
| **BNetzA BK7-24-01-008** | GaBi Gas 2.1 ruling — current production version |
| **Kooperationsvereinbarung Gas** (KoV) | Industry agreement mandating DVGW EDIFACT formats |
| **DVGW G 685** | Technical standard for gas metering and allocation calculations |

DVGW AHBs and MIGs are published by DVGW S&C:
<https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas>
