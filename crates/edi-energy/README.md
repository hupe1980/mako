# edi-energy

**EDIFACT parse · validate · build — stateless German energy market library**

`edi-energy` is the low-level EDIFACT processing layer for the German energy
market (EDI@Energy / BDEW MaKo). It is a **purely stateless library**: no async,
no I/O, no runtime dependencies. All parsing, validation, and message building
happen in-process without allocating threads or network connections.

This crate is the foundation for [`makod`] and the `mako-*` domain crates, but
it is also useful standalone for:
- AS4 gateway pre-processing
- Regulatory compliance checking pipelines
- ERP import/export converters
- Testing harnesses for MaKo messages

## Message Types

| Feature flag | Message type | BDEW abbreviation | Profiles |
|---|---|---|---|
| `utilmd` | Utility Master Data | UTILMD | Strom S2.1/S2.2 · Gas G1.1/G1.2 |
| `mscons` | Metered Services Consumption | MSCONS | 2.4c · 2.5 |
| `aperak` | Application Error and Acknowledgement | APERAK | 2.1i · 2.2 |
| `contrl` | Interchange Control Response | CONTRL | 2.0b |
| `invoic` | Invoice | INVOIC | 2.8e |
| `remadv` | Remittance Advice | REMADV | 2.9e |
| `orders` | Purchase Order | ORDERS | 1.4b · 1.4c |
| `ordrsp` | Purchase Order Response | ORDRSP | 1.4b · 1.4c |
| `ordchg` | Purchase Order Change | ORDCHG | 1.1 · 1.2 |
| `iftsta` | International Multimodal Status Report | IFTSTA | 2.0g · 2.1 |
| `insrpt` | Inspection Report | INSRPT | 1.1a |
| `reqote` | Request for Quotation | REQOTE | 1.3c |
| `quotes` | Quotation | QUOTES | 1.3b · 1.3c |
| `pricat` | Price/Sales Catalogue | PRICAT | 2.0e · 2.1 |
| `comdis` | Commercial Dispute | COMDIS | 1.0g |
| `partin` | Party Information | PARTIN | 1.0f · 1.1 |
| `utilts` | Utility Time Series | UTILTS | 1.1e |

The default feature set enables `utilmd`, `mscons`, `aperak`, and `contrl` —
the four message types every MaKo deployment needs. Enable the other flags
explicitly (or `--all-features`) for the full set of 17.

## Quick Start

Add to `Cargo.toml`:

```bash
cargo add edi-energy
```

### Parse and validate

```rust,no_run
// `detect_*` are methods of the `EdiEnergyMessage` trait — it has to be in scope.
use edi_energy::{parse, EdiEnergyMessage, EdiEnergyReport};

let bytes = std::fs::read("message.edi")?;
let msg = parse(&bytes)?;

// Detect what arrived. Both return `Result`: a message whose Formatversion or
// Prüfidentifikator cannot be read is a finding, not a `None`.
let pid = msg.detect_pruefidentifikator()?.as_u32();
let fv  = msg.detect_release()?.to_string();
println!("PID {pid}  FV {fv}");

// Run AHB + MIG rule enforcement
let report: EdiEnergyReport = msg.validate()?;
if !report.is_valid() {
    for err in report.errors() {
        eprintln!("[{}] {}", err.rule_id.as_deref().unwrap_or("-"), err.message);
    }
} else {
    println!("valid ✓");
}
```

### Parse as hard error

```rust,no_run
use edi_energy::parse;

let msg = parse(&bytes)?;
msg.validate()?.into_error_result()?;  // returns Err if any validation error
```

### Interchange stream (multiple messages)

```rust,no_run
use edi_energy::{Platform, parse_interchange};
use std::io::BufReader;

let reader = BufReader::new(std::fs::File::open("bulk.edi")?);
for result in parse_interchange(reader) {
    let msg = result?;
    println!("{}", msg.try_message_type().map_or("?", |t| t.as_str()));
}
```

### Build a UTILMD message

```rust,no_run
use edi_energy::builders::UtilmdBuilder;
use edi_energy::utilmd_codes::{Produktpaket, Transaktionsgrund, dtm, transaktionsgrund};
use edi_energy::{Pruefidentifikator, Release};

let bytes = UtilmdBuilder::new(Release::new("S2.2"))
    .pruefidentifikator(Pruefidentifikator::new(55001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .document_date("20261001")
    // `IDE+24` DE 7402 is a **Vorgangsnummer**, never a Lokations-ID.
    .transaction("VORGANG-0001")
    // `SG4 DTM+92` „Datum Vertragsbeginn" — the Anmeldung's process date.
    .date(dtm::BEGINN_ZUM, "20261101")
    .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(transaktionsgrund::WECHSEL))
    // `SG8 SEQ+Z79` Produktpaket — Muss on an Anmeldung: without a Bilanzkreis
    // the NB cannot assign the Marktlokation (UTILMD AHB Strom 2.2 Kap. 5.3).
    .produktpaket(Produktpaket::bilanzkreis("11XBK-STD-----9"))
    // The Lokations-ID lives in `SG5 LOC+Z16`.
    .marktlokation("51238696799")
    // The rest of the 55001 column: the Marktlokation's and the Kunde's
    // Stammdaten blocks and the Kunde with a Korrespondenzanschrift.
    .stammdaten("Z01").cci("", "Z15").done()
    .stammdaten("Z75").cci("Z61", "ZF9").cav("ZU5").done()
    .kunde_des_lf(["Mustermann".to_owned()], "Z01")
    .anschrift("Z04", ["Mustermann".to_owned()], "Z01", "Musterstr. 1", "Berlin", "10115", "DE")
    .done()
    .serialize()?;

println!("{}", String::from_utf8_lossy(&bytes));
```

The builder emits what it is given; the Prüfschablone says what a column
needs (`profile.pruefschablone(55001)`), and `profile.validate` on the result
names every place still missing.

See the [builder guide][builders] for the full builder API and all message types.

## The Produktpaket an Anmeldung must carry

UTILMD AHB Strom 2.2 Kap. 5.3 makes `SG8 SEQ+Z79` „Bestandteil eines
Produktpakets" and `SG8 SEQ+ZH0` „Priorisierung erforderliches Produktpaket"
Muss on **55001, 55077, 55600, 55601, 55014 and 55608**, and the Codeliste der
Konfigurationen 1.4 Kap. 6.1.1 makes one product unconditional inside it:

> `9991000002082` **Bilanzkreis** — „Dieses Produkt ist je Produktpaket-ID in
> der UTILMD zwingend anzugeben."

[`Produktpaket::bilanzkreis`] builds that package; the emitted shape is

```text
SEQ+Z79+1
PIA+5+9991000002082:Z11
CCI+Z66
CAV+ZV4:::11XBK-STD-----9
SEQ+ZH0+1
CCI+Z65+++Z01
```

The Bilanzkreis is *not* an `FTX+ACB` remark: on a 55608 the AHB admits that
segment under Bedingung [48] („Wenn … STS+E01++A99 vorhanden"), i.e. on the
Ablehnung only.

**GeLi Gas has no Produktpaket.** UTILMD AHB Gas 1.2 marks `SG10 CCI+Z19` with
the Bilanzkreis in DE 7037 Muss on a 44001 — one segment, no `SEQ`. Build it
with `.merkmal(produkt::CCI_BILANZKREIS_GAS, "…")`; the two shapes carry the
same fact and neither is sendable on the other Sparte.

**Reading it back.** `UtilmdTransaction::sequences` nests each `CAV` under the
`CCI` above it — the only thing that says which Merkmal a value belongs to — and
`UtilmdTransaction::bilanzkreis()` accepts either Sparte's shape. `SG12 NAD`
lands in `UtilmdTransaction::parties` with the full five-component `C080` name.

[`Produktpaket::bilanzkreis`]: https://docs.rs/edi-energy/latest/edi_energy/utilmd_codes/struct.Produktpaket.html

## Interchange party identity (§2.13)

`parse` and `parse_interchange` reject a message whose `NAD` parties disagree
with the interchange envelope:

```
interchange party mismatch: UNB DE0004 is "9900555000005" but NAD+MS is
"9900111000002" (message 0) — BDEW Allgemeine Festlegungen §2.13 requires them
to be identical
```

> "Die im UNB- und NAD-Segment für den Absender / Empfänger verwendeten MP-ID
> sind identisch."
> — Allgemeine Festlegungen V6.1d §2.13

This is an authorisation boundary, not a formatting rule. AS4 authenticates the
**envelope** sender, while consuming services read `NAD+MS` for consent gates,
partner lookup and role resolution. Tolerating a mismatch would let an
authenticated partner attribute a message to a different market participant.

A party absent from either side is not a mismatch — some profiles omit one, and
whether that is legal is an AHB question. `AnyMessage::nad_sender()` /
`nad_receiver()` expose the message-level parties uniformly; CONTRL has none,
being an interchange-level acknowledgement.

## Profiles — read from the BDEW documents

A profile is one MIG and one AHB of one format version, generated from the
BDEW PDFs by `cargo xtask import-profiles` and embedded at build time:

- `mig.json` — the Nachrichtenstruktur as a tree, every segment with the MIG's
  running number `Nr` and its Segmentlayout (BDEW status, format, codes);
- `ahb.json` — one **Prüfschablone** per Anwendungsfall, keyed by the same
  `Nr`: the status of every segment and group (`Muss`, `Muss [10]`,
  `Soll [3] ∧ [4]`), the operands on every data element and code, and every
  Bedingung as printed.

`Profile::validate` resolves each segment of a message to its place in the
structure (`SG5 LOC+Z16` Marktlokation is not `SG5 LOC+Z17` Messlokation),
applies the MIG's checks and the column's Prüfschablone, and evaluates
Voraussetzungen against the message. A finding names the place:

```text
[AHB-55001-SG6-00057-MISSING]  segment group SG6 „Prüfidentifikator" (Nr 00057) is Muss for 55001 in SG4 but missing
[MIG-00050-LOC-3225-FORMAT]    LOC (Nr 00050): DE 3225 „Marktlokations-ID" is "BADID", the MIG says n11
```

The same profile answers what a sender has to fill:

```rust,no_run
use edi_energy::{MessageType, ReleaseRegistry};
use edi_energy::profile::SkeletonParties;

let profile = ReleaseRegistry::global()
    .profiles_for(MessageType::Utilmd)
    .find(|p| p.release().as_str() == "S2.2")
    .expect("shipped");
// The column, for a reader.
println!("{}", profile.pruefschablone(55001).expect("55001 is a column"));
// The minimal conformant message of the column — every Muss place filled.
let af = profile.anwendungsfall(55001).expect("55001 is a column");
let bytes = profile.skeleton_interchange(af, &SkeletonParties::default())?;
// A sender's message, completed to the column: what it states stays, what
// the column requires and it lacks is filled the same way.
let seed: Vec<edifact_rs::OwnedSegment> = edifact_rs::from_bytes(b"UNH+1+UTILMD:D:11A:UN:S2.2'BGM+E01+1'IDE+24+VG1'LOC+Z16+51238696781'UNT+5+1'")
    .map(|s| s.map(edifact_rs::Segment::into_owned))
    .collect::<Result<_, _>>()?;
let done = profile.complete(&seed, af, &SkeletonParties::default());
```

Every Anwendungsfall's skeleton validating against its own column is the
crate's proof that extraction and validator agree (`tests/skeletons.rs`, 958
columns across 32 profiles). `Profile::complete` is the sender-side
counterpart: it runs the same fixpoint from a seed and only adds, so a builder
states the business case and the profile states the rest of the Prüfschablone. `cargo run --example 07_resolve --all-features --
message.edi` prints where every segment of a message landed and why it is
rejected.

### The Bedingungen column wraps, and a wrap is not always repairable

The AHB's Bedingungen column is narrow enough to break a word without a hyphen,
and the PDF reader joins the two halves with a space — the shipped profiles
carry „Format: Zählpunktbezeichnu ng" verbatim in four AHBs (UTILMD
`fv20251001` and `fv20261001`, IFTSTA and ORDERS `fv20261001`). Nothing in the
character grid says whether the break replaced a space, and a rule that guessed
from the two halves alone would merge „STS+Z21 DE9013" as readily as it repairs
„CAV+…/SO T/WNT".

So the repair is limited to the case where the evidence is local: where the
break falls after a separator, `join_wrapped_pattern` rejoins a space that
follows `+`, `-`, `:` or `/` inside a token that already looks like `TAG+…`
(`PIA+5+1- 1?:1.9.0` → `PIA+5+1-1?:1.9.0`). `/` matters most — it separates the
alternatives of one code list (`SEQ+Z04/ ZF7`) and the AHBs set a space after it
for readability, so left in, everything after the space reads as prose and the
Voraussetzung matches only the first alternative.

What is left is a residual: a wrap swallowed the *next* Bedingung's number,
leaving its label stranded on the end of the previous text
(`"951": "Format: Zählpunktbezeichnu ng Format:"`). It is exactly five
Bedingungen — `[50]`, `[683]`, `[931]`, `[951]`, `[961]` — each in both shipped
UTILMD profiles (`fv20251001`, `fv20261001`), so ten entries. None changes a
verdict: the stranded tail follows a complete clause, so
`Voraussetzung::parse` reads the first one and stops, and the five are three
Formatbedingungen, a Hinweis (`[683]`) and one Voraussetzung (`[50]`), none of
which gate presence on the strength of the swallowed half.
`cargo xtask validate-profiles` cannot see it either: its citation check asks
whether every cited `[n]` has a text, and the swallowed number is still cited —
and still has text — elsewhere.

## Element positions

An element's position inside a segment is fixed by the UN/EDIFACT directory —
it is what a counterparty writes on the wire — and the BDEW MIGs list every
element (unused ones as `N`), so the imported layouts carry those positions.
The hand-authored layouts in `src/messages/layouts.rs`, which the
`EdifactDeserialize` derive resolves `#[edifact(element = "4440")]` against
when **reading** a typed segment, must agree with them; `tests/element_positions.rs`
holds every hand-authored layout against every profile's layout of that tag,
and `tests/segment_layout_guard.rs` holds the accessors.

## Format Versions

| Format version | Strom | Gas | Valid period |
|---|---|---|---|
| `FV2025-10-01` | S2.1 — **current production** | — | 2025-10-01 – 2026-09-30 |
| `FV2026-04-01` | — | G1.1 — **current production** | 2026-04-01 – 2026-09-30 |
| `FV2026-10-01` | S2.2 — next release | G1.2 — next release | from 2026-10-01 |

Cutovers are staggered per message type; `profiles/sources.json` states every
profile's window and `cargo xtask check-release-coverage` proves a date is
covered. The registry resolves the profile from the `UNH` DE 0057 wire code
and the date — no per-message format selection is needed. EDIFACT has no
Übergangsfrist: `ReleaseRegistry::with_receive_tolerance_days(n)` is a local
receiving policy for a late-arriving message in the superseded format.

## Features

| Feature | Default | Description |
|---|---|---|
| `utilmd`, `mscons`, `aperak`, `contrl` | ✓ on | The default message-type set |
| All other message-type flags above | off | Enable the corresponding profile and parser |
| `serde` | off | `serde::{Serialize, Deserialize}` on public types |
| `diagnostics` | off | Rich validation error messages with segment context |
| `tracing` | off | Emit `tracing` events during parse (performance overhead) |

To enable a minimal build for a single message type:

```bash
cargo add edi-energy --no-default-features --features utilmd,serde
```

## Multi-Tenant and Test Isolation

The module-level `parse()` and `parse_interchange()` functions use a global profile
registry. For test isolation or multi-tenant gateways use `Platform` directly:

```rust,no_run
use edi_energy::Platform;

let platform = Platform::with_all_profiles();
let msg = platform.parse(&bytes)?;
let report = msg.validate()?;
```

Platforms are cheap to clone (profile data is `Arc`-shared). See the [platform
guide][platform] for custom profile subsets, DoS limits, and hot-reload patterns.

## Built-In Examples

Run with `cargo run --example <name> --all-features`:

| Example | What it shows |
|---|---|
| `01_parse_utilmd` | Parse a UTILMD Strom message, inspect segments |
| `02_parse_mscons` | Parse a MSCONS Summenzeitreihe (MABIS path) |
| `03_build_messages` | Build UTILMD + APERAK using the type-state builders |
| `04_interchange_dispatch` | Stream-parse a bulk interchange with PID-based dispatch |
| `05_validate` | Run AHB validation and render error diagnostics |
| `06_parse_reader` | Low-allocation streaming parse from a `BufRead` |
| `07_resolve` | Where every segment sits in the Nachrichtenstruktur; `--pruefschablone`, `--skeleton`, `--structure` |

## Documentation

| Topic | Link |
|---|---|
| Getting started (full engine) | [Guide][getting-started] |
| Parsing guide | [Parsing][parsing] |
| Validation guide | [Validation][validation] |
| Builder guide | [Builders][builders] |
| Platform (multi-tenant / test isolation) | [Platform][platform] |
| Profile files and format versions | [Profile files][profile-files] · [Release lifecycle][release-lifecycle] |
| PID reference | [PID reference][pid-reference] |

## Regulatory Standards

A MIG version and its AHB version are different numbers for every message type
except UTILMD; `profiles/sources.json` carries both per profile. As of
`FV2026-10-01`:

- EDI@Energy **UTILMD** — Strom MIG S2.2 / AHB 2.2, Gas MIG G1.2 / AHB 1.2
- EDI@Energy **MSCONS** — MIG 2.5 / AHB 3.2
- EDI@Energy **APERAK** — MIG 2.2 / AHB **1.1**
- EDI@Energy **CONTRL** — MIG 2.0b / AHB 1.0, ausserordentliche
  Veröffentlichung Stand 11.12.2025
- BNetzA rulings BK6-24-174, BK6-22-024, BK7-24-01-009 (process scope)


## Related crates

| Crate | Role |
|---|---|
| [`edi-energy`](https://docs.rs/edi-energy) ← **this crate** | BDEW EDI@Energy EDIFACT — parse · validate · build · profiles |
| [`mako-engine`](https://docs.rs/mako-engine) | Event-sourced workflow runtime — `Workflow`, `Process`, `EventStore`, deadlines |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-pruefung`](https://docs.rs/mako-pruefung) | *What* the answer must be — the BDEW Entscheidungsbäume, executable |
| [`mako-gpke`](https://docs.rs/mako-gpke) · [`mako-wim`](https://docs.rs/mako-wim) · [`mako-geli-gas`](https://docs.rs/mako-geli-gas) · [`mako-mabis`](https://docs.rs/mako-mabis) | The domain packs that give these messages a process |
| [`dvgw-edi`](https://docs.rs/dvgw-edi) | The DVGW gas formats — ALOCAT, NOMINT, NOMRES, SSQNOT |
| [`mako-as4`](https://docs.rs/mako-as4) | The AS4 transport that carries an interchange |
| [`makod`](https://hupe1980.github.io/mako/docs/services/makod/) | Production daemon — ingest, routing and rendering |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>

[`makod`]: https://hupe1980.github.io/mako/docs/services/makod/
[getting-started]: https://hupe1980.github.io/mako/docs/guide/getting-started/
[parsing]: https://hupe1980.github.io/mako/docs/reference/parsing/
[validation]: https://hupe1980.github.io/mako/docs/reference/validation/
[builders]: https://hupe1980.github.io/mako/docs/reference/builders/
[platform]: https://hupe1980.github.io/mako/docs/reference/platform/
[profile-files]: https://hupe1980.github.io/mako/docs/compliance/schema-versioning/
[release-lifecycle]: https://hupe1980.github.io/mako/docs/compliance/release-lifecycle/
[pid-reference]: https://hupe1980.github.io/mako/docs/regulatory/pid-reference/
