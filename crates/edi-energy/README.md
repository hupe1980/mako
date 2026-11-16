# edi-energy

[![Crates.io](https://img.shields.io/crates/v/edi-energy?label=edi-energy&color=f59e0b&logo=rust)](https://crates.io/crates/edi-energy)

BDEW EDI@Energy EDIFACT for the German energy market: parse, validate and build
the 17 MaKo message types against profiles generated from the BDEW MIG and AHB
documents and embedded at build time. Stateless — no async, no I/O — on
[`edifact`](https://crates.io/crates/edifact) 0.20. It is the format layer under
[`makod`](https://hupe1980.github.io/mako/docs/services/makod/) and works
standalone for gateways, converters and test harnesses; transport (AS4) and
process state are not here.

## Example

```rust,no_run
// `detect_*` are methods of the `EdiEnergyMessage` trait — it has to be in scope.
use edi_energy::{parse, EdiEnergyMessage, EdiEnergyReport};

let bytes = std::fs::read("message.edi")?;
let msg = parse(&bytes)?;

// A message whose Formatversion or Prüfidentifikator cannot be read is an
// error, not a `None`.
let pid = msg.detect_pruefidentifikator()?.as_u32();
let fv  = msg.detect_release()?.to_string();
println!("PID {pid}  FV {fv}");

let report: EdiEnergyReport = msg.validate()?; // AHB + MIG rules
for err in report.errors() {
    eprintln!("[{}] {}", err.rule_id.as_deref().unwrap_or("-"), err.message);
}
```

Building a 55001 Anmeldung:

```rust,no_run
use edi_energy::builders::UtilmdBuilder;
use edi_energy::utilmd_codes::{Produktpaket, Transaktionsgrund, dtm, transaktionsgrund};
use edi_energy::{Pruefidentifikator, Release};

let bytes = UtilmdBuilder::new(Release::new("S2.2"))
    .pruefidentifikator(Pruefidentifikator::new(55001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .document_date("202610010800")            // DTM+137, a UTC instant
    .transaction("VORGANG-0001")              // IDE+24 is a Vorgangsnummer
    .date(dtm::BEGINN_ZUM, "202610312300")    // 01.11.2026 00:00 MEZ in UTC
    .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(transaktionsgrund::WECHSEL))
    // SG8 SEQ+Z79 Produktpaket — Muss on an Anmeldung (UTILMD AHB Strom 2.2 Kap. 5.3).
    .produktpaket(Produktpaket::bilanzkreis("11XBK-STD------Y"))
    .marktlokation("51238696799")             // SG5 LOC+Z16
    .stammdaten("Z01").cci("", "Z15").done()
    .stammdaten("Z75").cci("Z61", "ZF9").cav("ZU5").done()
    .kunde_des_lf(["Mustermann".to_owned()], "Z01")
    .anschrift("Z04", ["Mustermann".to_owned()], "Z01", "Musterstr. 1", "Berlin", "10115", "DE")
    .done()
    .serialize()?;
```

The builder emits what it is given; `profile.pruefschablone(55001)` says what a
column needs, `profile.validate` names every place still missing, and
`profile.completion` lists in `unstated` every element it could only have filled
with a made-up value — the places to refuse rather than send.

## What it covers

- **Profiles** — one MIG + AHB per format version
  (`profiles/<message>/fv<yyyymmdd>/`), chosen by the `UNH` DE 0057 wire code
  and the date, so several format versions coexist; `profiles/sources.json`
  names every source PDF.
- **Findings name the place** — `AHB-55001-SG6-00057-MISSING`,
  `MIG-00050-LOC-3225-FORMAT`.
- **Envelope identity** — `parse` refuses a message whose `NAD+MS`/`NAD+MR`
  disagree with the `UNB` sender/receiver (Allgemeine Festlegungen V6.1d Kap. 2.13).
- **Decimal mark** — read from the interchange's `UNA`, `.` without one.
- **Limits** — `ParseConfig` bounds input, segment and message size
  (`max_message_bytes`); a writer refusal is `Error::Serialize`.
- **Isolation** — `parse` uses a global registry; `Platform::with_all_profiles()`
  gives an isolated one for tests and multi-tenant gateways.

## Features

Each message type is a feature; the defaults are `utilmd`, `mscons`, `aperak`
and `contrl`.

| Feature | Message type | MIG releases |
|---|---|---|
| `utilmd` | UTILMD | Strom S2.1 · S2.2, Gas G1.1 · G1.2 |
| `mscons` | MSCONS | 2.4c · 2.5 |
| `aperak` | APERAK | 2.1i · 2.2 |
| `contrl` | CONTRL | 2.0b |
| `invoic` | INVOIC | 2.8e |
| `remadv` | REMADV | 2.9e |
| `orders` / `ordrsp` | ORDERS / ORDRSP | 1.4b · 1.4c |
| `ordchg` | ORDCHG | 1.1 · 1.2 |
| `iftsta` | IFTSTA | 2.0g · 2.1 |
| `insrpt` | INSRPT | 1.1a |
| `reqote` | REQOTE | 1.3c |
| `quotes` | QUOTES | 1.3b · 1.3c |
| `pricat` | PRICAT | 2.0e · 2.1 |
| `comdis` | COMDIS | 1.0g |
| `partin` | PARTIN | 1.0f · 1.1 |
| `utilts` | UTILTS | 1.1e |

Optional: `serde` (serialisable reports), `diagnostics` (`miette`),
`tracing`. Seven runnable examples live in `examples/`
(`cargo run -p edi-energy --example 05_validate --all-features`).

## Sources

- BDEW MIGs and AHBs of each release, listed in `profiles/sources.json`
- BDEW Allgemeine Festlegungen V6.1d

Reference: [Parsing](https://hupe1980.github.io/mako/docs/reference/parsing/) ·
[Validation](https://hupe1980.github.io/mako/docs/reference/validation/) ·
[Builders](https://hupe1980.github.io/mako/docs/reference/builders/) ·
[Platform](https://hupe1980.github.io/mako/docs/reference/platform/) ·
[Release lifecycle](https://hupe1980.github.io/mako/docs/compliance/release-lifecycle/) ·
API: <https://docs.rs/edi-energy>

License: MIT OR Apache-2.0
