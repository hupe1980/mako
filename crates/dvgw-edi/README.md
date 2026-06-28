# dvgw-edi

**DVGW EDIFACT format parser and validator for the German gas market**

> **This crate is a name reservation. Implementation is pending.**

EDIFACT format parsing and validation for DVGW-governed message types used
in German gas network balancing (GaBi Gas): ALLOCAT, NOMINT, and NOMRES.

## Relationship to `edi-energy`

| Crate | Governs | Formats |
|---|---|---|
| `edi-energy` | BDEW EDI@Energy (electricity + gas market communication) | UTILMD, MSCONS, INVOIC, REMADV, APERAK, CONTRL, … |
| `dvgw-edi` | DVGW (gas balancing / GaBi Gas) | ALLOCAT, NOMINT, NOMRES |

Both crates follow the same architecture: stateless parse + validate API,
profiles loaded from AHB JSON definitions, Prüfidentifikator-based routing.

## Format family

| Message | UN/EDIFACT version | Description |
|---|---|---|
| `ALLOCAT` | D03A | Allokationsnachricht — gas quantity allocation |
| `NOMINT` | D01B | Nominierungsintegration — nomination integration |
| `NOMRES` | D01B | Nominierungsantwort — nomination response |
| `APERAK` | D01B | Application error and acknowledgement (shared with edi-energy) |
| `CONTRL` | D14A | Interchange control acknowledgement (shared with edi-energy) |

## Planned API

The planned API mirrors `edi-energy`:

```rust,ignore
use dvgw_edi::{DvgwPlatform, AnyDvgwMessage};

let platform = DvgwPlatform::with_all_profiles();
let msg = platform.parse(&raw_bytes)?;
let report = msg.validate()?;

let AnyDvgwMessage::Allocat(a) = &msg else { bail!("not ALLOCAT") };
let pid = msg.detect_pruefidentifikator()?;
// → route to mako-gabi-gas workflow
```

## Market roles

| Role | Abbrev. | Description |
|---|---|---|
| Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
| Verteilnetzbetreiber | VNB | Gas distribution system operator |
| Bilanzkreisverantwortlicher | BKV | Balance responsible party |
| Marktgebietsverantwortlicher | MGV | Market area manager |
| Großhändler / Produzent | GH | Gas wholesaler / producer |

## Regulatory references

- **GasNZV** — Gasnetzzugangsverordnung, statutory basis for gas network access
- **BNetzA BK7-14-020** — GaBi Gas 2.0 ruling (current)
- **DVGW G 685** — technical rules for gas metering and allocation
- DVGW AHBs and MIGs: <https://www.dvgw.de> / <https://www.bdew-mako.de>

## Workspace dependency

When implemented, `mako-gabi-gas` will depend on this crate:

```toml
[dependencies]
dvgw-edi = { path = "../dvgw-edi" }
mako-engine = { workspace = true }
```
