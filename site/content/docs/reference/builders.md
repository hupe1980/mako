+++
title = "Builders"
description = "Fluent type-state builder API for constructing valid EDI@Energy EDIFACT messages: UTILMD, MSCONS, APERAK, CONTRL, INVOIC, REMADV, ORDERS, ORDRSP."
weight = 12
+++
The `edi_energy::builders` module provides a fluent, type-state builder API for
constructing valid EDI@Energy EDIFACT messages programmatically. Terms such as
Prüfidentifikator, AHB and Marktlokation are defined in the
[reference vocabulary](@/docs/reference/_index.md#vocabulary).

---

## Why Use Builders?

- **Compile-time mandatory field enforcement** — the type-state pattern prevents calling `.build()` unless all required fields have been set.
- **Correct segment ordering** — the builders emit segments in the order required by the relevant MIG profile.
- **Domain types** — use `Lokationstyp`, `Pruefidentifikator` and `Release` instead of raw strings.
- **Round-trip compatible** — the output can be re-parsed and validated by the same library.

---

## Available Builders

| Builder | Message type |
|---|---|
| `UtilmdBuilder` | UTILMD (grid connection processes) |
| `MsconsBuilder` | MSCONS (metered consumption reports) |
| `AperakBuilder` | APERAK (application error acknowledgements) |
| `ContrlBuilder` | CONTRL (interchange control acknowledgements) |
| `InvoicBuilder` | INVOIC (invoices) *(requires `invoic` feature)* |
| `RemadvBuilder` | REMADV (remittance advice) *(requires `remadv` feature)* |
| `OrdersBuilder` | ORDERS — orders (Sperrung, Konfiguration, ESA Bestellung/Abbestellung 17007/17008) *(`orders`)* |
| `OrdrespBuilder` | ORDRSP — order answers, incl. the ESA-Antworten 19011–19014 *(`ordrsp`)* |
| `OrdchgBuilder` | ORDCHG — order change / cancellation, incl. ESA Stornierung 39002 *(`ordchg`)* |
| `IftstaBuilder` | IFTSTA — status reports *(`iftsta`)* |
| `InsrptBuilder` | INSRPT — Störungsmeldung / Ablesesteuerung *(`insrpt`)* |
| `PartinBuilder` | PARTIN — Kommunikationsdaten *(`partin`)* |
| `PricatBuilder` | PRICAT — price lists *(`pricat`)* |
| `QuotesBuilder` | QUOTES — quotations, incl. ESA Angebot 15003 *(`quotes`)* |
| `ReqoteBuilder` | REQOTE — requests for quotation, incl. ESA Werteanfrage 35003 *(`reqote`)* |
| `ComdisBuilder` | COMDIS — Handelsunstimmigkeit *(`comdis`)* |
| `UtiltsBuilder` | UTILTS — Berechnungsformeln *(`utilts`)* |

The ESA (Energieserviceanbieter) families are built out to full MIG conformance,
because their messages are the ones mako both sends and has to read back:

- **ORDERS** 17007/17008 — `.reference()` (SG1 `RFF+Z13`), `.item_description()`
  (`IMD`), `.location()` (`LOC+172`), plus the mandatory `UNS`.
- **ORDRSP** 19011–19014 — `.reference(qualifier, value)` is additive and writes
  the `SG1 RFF+ACW`/`ON` echo the ESA correlates by; `.adjustment()` the
  `SG2 AJT`, `.item_description()` the `IMD`, `.line_item()` the `SG27 LIN`.
  ORDRSP carries no `LOC`.
- **ORDCHG** 39002 — `.reference()` emits the mandatory SG1 `RFF`; ORDCHG
  carries no `LOC` either.
- **QUOTES** 15003 — `.reference(qualifier, value)` covers the SG1 `RFF`
  qualifiers the MIG admits (`AAV`, `ACW`, `Z13`); the AHB needs `AAV` *and*
  `Z13`, which is why there is no single-slot variant. Then `.location()`,
  `.bindungsfrist()` → `DTM+273`, `.reason()` → `FTX+ACB`, `.currency()` →
  `CUX`, `.contact()` → `CTA+COM`, `.product()` → `LIN+PIA`, `.price()` → `PRI`.
- **REQOTE** 35003 — `.reference()` (SG1 `RFF+Z13`), `.location()` (`LOC+172`),
  `.contact()` (`CTA+COM`), `.free_text()` (`FTX`), `.characteristic()` (`CCI`),
  `.product()` (SG27 `LIN+<n>+<Z67|Z68>` + `PIA+5`), `.line_item()` (`LIN`).

**INSRPT** is likewise AHB-conformant: `.doc_reference()` → SG3 `DOC`,
`.pruefidentifikator()` → SG4 `RFF+Z13`, `.position()` → SG7 `LIN`, `.status()`
→ `STS`, `.location()` → SG8 `LOC+172`. Its AHB marks
`BGM`/`DOC`/`DTM`/`LIN`/`LOC`/`NAD`/`RFF`/`STS` mandatory for every
Prüfidentifikator, so a message missing any of them parses but fails validation.

---

## UTILMD Example

The AHB column of a Prüfidentifikator says what the message must carry —
`profile.pruefschablone(55001)` prints it — and the builder emits what it is
given. A 55001 Anmeldung, complete:

```rust
use edi_energy::builders::UtilmdBuilder;
use edi_energy::utilmd_codes::{Produktpaket, Transaktionsgrund, dtm, transaktionsgrund};
use edi_energy::{Pruefidentifikator, Release};

let bytes = UtilmdBuilder::new(Release::new("S2.2"))
    .pruefidentifikator(Pruefidentifikator::new(55001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .message_ref("MSG-001")
    .document_date("20261001")
    // `IDE+24` DE 7402 is the Vorgangsnummer, never a Lokations-ID.
    .transaction("VORGANG-0001")
    // `SG4 DTM+92` — the Lieferbeginn.
    .date(dtm::BEGINN_ZUM, "20261101")
    .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(transaktionsgrund::WECHSEL))
    // `SG8 SEQ+Z79` Produktpaket with the Bilanzkreis, and its `SEQ+ZH0`.
    .produktpaket(Produktpaket::bilanzkreis("11XBK-STD-----9"))
    // `SG5 LOC+Z16`.
    .marktlokation("51238696012")
    // Stammdaten blocks the column demands: `SEQ+Z01` Daten der
    // Marktlokation, `SEQ+Z75` Daten des Kunden.
    .stammdaten("Z01").cci("", "Z15").done()
    .stammdaten("Z75").cci("Z61", "ZF9").cav("ZU5").done()
    // `SG12 NAD+Z09` Kunde des LF and `NAD+Z04` Korrespondenzanschrift.
    .kunde_des_lf(["Mustermann".to_owned()], "Z01")
    .anschrift("Z04", ["Mustermann".to_owned()], "Z01", "Musterstr. 1", "Berlin", "10115", "DE")
    .done()
    .serialize()?;

// Validate the output immediately — every place still missing is named.
let msg = edi_energy::parse(&bytes)?;
msg.validate()?.into_error_result()?;
```

The Prüfidentifikator goes out in `SG6 RFF+Z13` of the Vorgang, `BGM` DE 1004
carries the Dokumentennummer (the message reference unless `document_number`
is given). Gas has no Produktpaket: a 44001 carries its Bilanzkreis as
`SG10 CCI+Z19` inside `SEQ+Z01` (`.merkmal(produkt::CCI_BILANZKREIS_GAS, …)`),
an `IMD` (`.imd("Z36", "Z12")`), a dated `RFF+Z18`
(`.reference_dated("Z18", "", "Z20", "2026", "802")`) and a `SEQ+Z12` with a
`QTY` (`.stammdaten("Z12").qty("Z16", "100", "P1").done()`). The generic
`stammdaten` blocks go out in the MIG's order of `SEQ` places whatever order
they are given in.

### Release constants

Use the constants in `edi_energy::releases` to avoid hard-coding version strings:

```rust
use edi_energy::releases;

let r_utilmd_strom = releases::utilmd_fv20261001();   // S2.2 — Strom
let r_utilmd_gas   = releases::utilmd_fv20261001_gas(); // G1.2 — Gas
let r_mscons       = releases::mscons_fv20261001();   // 2.5
let r_aperak       = releases::aperak_fv20261001();   // 2.2
let r_contrl       = releases::contrl_fv20260101();   // 2.0b
```

---

## MSCONS Example

A metering point is a sub-builder: `.metering_point(malo)` opens it, `.done()`
closes it and returns to the message.

```rust
use edi_energy::builders::{MsconsBuilder, QTY_ENERGIE_SUMMIERT};
use edi_energy::{Pruefidentifikator, releases};

let bytes = MsconsBuilder::new(releases::mscons_fv20261001().clone())
    .sender("9900357000004")
    .receiver("9900077000006")
    .pruefidentifikator(Pruefidentifikator::new(13003)?)   // MaBiS Summenzeitreihe
    .message_ref("SZR0001")
    .metering_point("11YAPG4CTRDNZ--P")
        .balancing_period("202606")           // DTM+492, format 610
        .version("20260714050000+00")         // DTM+293, format 304 — marks a correction
        .quantity_for_period(
            QTY_ENERGIE_SUMMIERT,             // DE 6063 = "79"
            "12.5", "KWH",
            "202606010000+00",                // DTM+163 interval start
            "202606010015+00",                // DTM+164 interval end
        )
        .done()
    .serialize()?;
```

`.quantity(qualifier, value, unit)` is the same without the interval bounds, for
the non-interval cases; `.line_item(ObisCode)` and `.obis(ObisCode)` set the
`LIN`/`PIA` of the current item.

---

## APERAK Example

```rust
use edi_energy::builders::AperakBuilder;
use edi_energy::{Pruefidentifikator, releases};

let bytes = AperakBuilder::new(releases::aperak_fv20261001().clone())
    .pruefidentifikator(Pruefidentifikator::new(29002)?)  // 29002 = Ablehnung
    .sender("4012345000023")
    .receiver("9900357000004")
    .acw_ref("MSG-ORIG-001")      // SG2 RFF+ACE / SG5 RFF+ACW — the message answered
    .error_code("Z10")            // ERC — the application error
    .serialize()?;
```

An APERAK always answers something, and the sender/receiver are the *reverse* of
the message it answers. `AperakBuilder::for_receipt(&ReceiptContext)` fills the
swap, the `acw_ref` and the document date from the received interchange, so the
direction cannot be got backwards by hand.

---

## Lokationstyp Domain Enum

UTILMD names the object a Vorgang is about in `SG5 LOC` DE 3227 — not in `IDE`,
whose DE 7495 has only `24` (Vorgang) and `Z01` (Liste). `Lokationstyp` is that
qualifier as a type:

```rust
use edi_energy::Lokationstyp;

Lokationstyp::Marktlokation.qualifier_code();        // "Z16"
Lokationstyp::Messlokation.qualifier_code();         // "Z17"
Lokationstyp::Netzlokation.qualifier_code();         // "Z18"
Lokationstyp::SteuerbareRessource.qualifier_code();  // "Z19"  — § 14a EnWG
Lokationstyp::TechnischeRessource.qualifier_code();  // "Z20"
Lokationstyp::Tranche.qualifier_code();              // "Z21"
Lokationstyp::RuhendeMarktlokation.qualifier_code(); // "Z22"
Lokationstyp::Meldepunkt.qualifier_code();           // "172" — the Gas qualifier

// Parsing is fallible: an unknown or extension code is `None`, never a guess.
assert_eq!(Lokationstyp::from_qualifier_code("Z16"), Some(Lokationstyp::Marktlokation));
assert_eq!(Lokationstyp::from_qualifier_code("Z99"), None);
```

UTILMD AHB Gas uses `172` for every Lokation and tells Markt- from Messlokation by
the *format* of DE 3225 rather than by the qualifier, so the one `Meldepunkt`
variant covers both on the Gas side.

`UtilmdTransactionBuilder::location(Lokationstyp, id)` takes the type directly;
`.marktlokation(id)` and `.messlokation(id)` are the two shorthands.

---

## Pruefidentifikator

`Pruefidentifikator` wraps a u32 in the range 10000–99999:

```rust
use edi_energy::Pruefidentifikator;

let pid = Pruefidentifikator::new(55001)?;
println!("{}", pid.as_u32());  // 55001
println!("{}", pid);           // "55001"

// Common Pruefidentifikatoren
// 13001 — MSCONS: Netzbetreiber an Lieferant (SLP)
// 29001 — APERAK: Annahme
// 29002 — APERAK: Ablehnung
// 55001 — UTILMD Strom: Anmeldung Lieferbeginn (LF → NB)
// 55004 — UTILMD Strom: Abmeldung / Lieferende (LF → NB)
// 55042 — UTILMD Strom: WiM Anmeldung MSB (MSBN → NB)
```

---

## Type-State Enforcement

Builders carry two `PhantomData` type parameters that track whether the sender
and the receiver have been set. They start `Unset` and `.sender(…)` / `.receiver(…)`
flip them to `Set` (`crates/edi-energy/src/builders/mod.rs:82`): `.build()` and
`.serialize()` exist only on `UtilmdBuilder<Set, Set>`.

Missing mandatory parties are therefore a **compile error**, not a runtime panic:

```rust
// compile error: `.serialize()` is not on UtilmdBuilder<Set, Unset>
let result = UtilmdBuilder::new(release)
    .sender("4012345000023")
    .serialize();  // ← won't compile: no receiver
```

Everything else the AHB column demands is checked by validating the output —
which is why every example here re-parses what it built.

---

## Serialization

The built message implements `EdifactSerialize`:

```rust
let msg = builder.build()?;

// To bytes
let bytes: Vec<u8> = msg.serialize()?;

// Parse back and validate
let parsed = edi_energy::parse(&bytes)?;
parsed.validate()?.into_error_result()?;
```

### From message to interchange

`serialize()` produces a **message** (`UNH`…`UNT`). That is not the wire unit: a
market partner receives an **interchange**, which wraps one or more messages in
a `UNB` header and `UNZ` trailer. Use `InterchangeBuilder`:

```rust
use edi_energy::builders::InterchangeBuilder;

let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "REF001")
    .transmission("260802", "0915")     // UNB DE0017/DE0019 — YYMMDD / HHMM
    .message(msg.serialize()?)
    .build()?;
```

The `UNZ` message count is derived from the messages actually added, so it
cannot disagree with the payload. The transmission timestamp is a parameter
rather than a clock read, keeping interchange construction deterministic for
golden-file tests.

The UNB DE 0007 party qualifier is derived by `unb_qualifier` per Allgemeine
Festlegungen 6.1d: `14` = GS1 GLN, `500` = DE BDEW (13-digit IDs starting `99`,
and 16-character EIC codes), `502` = DE DVGW (starting `98`).

`makod`'s outbound renderer and the `makotest` Python toolkit both build
interchanges through this type — the envelope is a format concern and has one
implementation.

### Separator safety

Builders never pre-join composites with `:`. Runtime data — free texts,
references, OBIS codes, party IDs — is written through
`Writer::write_composites` (the internal `emit_comp!` macro), where component
boundaries are structural and a literal `:`, `+`, `?`, or `'` inside a value
is escaped on the wire instead of being promoted to a boundary. A guard test
(`builder_writer_guard`) enforces that no `format!`-interpolated value ever
reaches the raw writer path, and a round-trip test proves a separator-hostile
free text survives builder → wire → parser unchanged.

---

## See Also

- [Parsing Guide](@/docs/reference/parsing.md)
- [Validation Guide](@/docs/reference/validation.md)
- [Getting Started](@/docs/guide/getting-started.md)
