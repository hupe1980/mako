# dvgw-edi

**DVGW EDIFACT parser, validator and writer for the German gas transport and
balancing market**

Covers the DVGW-governed formats used in GaBi Gas 2.1 (BNetzA BK7-24-01-008):
ALOCAT, NOMINT, NOMRES and SSQNOT. The DVGW counterpart to `edi-energy`, which
covers BDEW EDI@Energy (UTILMD, MSCONS, INVOIC, APERAK, …).

## The one thing to know first

**A DVGW message does not name itself in `UNH`.** Every format is a subset of a
UN/EDIFACT D.07A message, so `UNH` carries the *carrier* — `ORDERS` or `ORDRSP`
— and `BGM` C002 DE 1001 carries the message:

```text
UNH+1+ORDERS:D:07A:UN:DVGW18'          ← the carrier
BGM+01G::332+NOMINT00052'              ← *this* says NOMINT
DTM+Z05:0:805'                         ← the timestamps below are UTC
DTM+137:201801042056:203'              ← message date/time
DTM+Z01:201801050400201801060400:719'  ← Gültigkeitszeitraum = the gas day
RFF+Z13:70030'                         ← Prüfidentifikator
```

Matching `UNH` against `"NOMINT"` therefore rejects every conformant message.
Identity here comes from `DvgwDocument`, with the carrier as a cross-check.

Two more consequences worth stating plainly:

- **`DTM+137` is not the gas day.** It is when the message was written. The gas
  day is `DTM+Z01`, a *period* in format `719`.
- **DVGW publishes real Prüfidentifikatoren** in `SG1 RFF+Z13` — ALOCAT
  70001–70023, NOMINT 70030–70034, NOMRES 70035–70039, SSQNOT 70095–70096. No
  synthetic encoding is needed, and the range does not collide with BDEW's.

## Supported formats

| Message | Carrier | Document codes (`BGM` DE 1001) | Prüfidentifikatoren |
|---|---|---|---|
| **ALOCAT** — Allokationsnachricht | `ORDRSP` | `X1G X2G X3G X4G X5G X6G X7G XBG` | 70001–70023 |
| **NOMINT** — Nominierung | `ORDERS` | `01G 55G Y1G Y6G Y7G` | 70030–70034 |
| **NOMRES** — Nominierungsantwort | `ORDRSP` | `07G 08G 19G 20G Y2G` | 70035–70039 |
| **SSQNOT** — Mehr-/Mindermengenmeldung | `ORDRSP` | `BAG` | 70095 (SLP), 70096 (RLM) |

`CONTRL` and `APERAK` acknowledge DVGW interchanges but are BDEW formats; they
live in `edi-energy` and are not reimplemented here.

`UNH` S009 DE 0057 holds either a package code (`DVGW17`) or the message version
(`5.11a`) depending on the format, so it is captured verbatim and nothing selects
behaviour from it; the builder writes the family's value unless told otherwise.

## Reading

```rust
use dvgw_edi::{DvgwMessageType, DvgwPlatform};

let platform = DvgwPlatform::default();
for result in platform.parse_interchange(raw) {
    let msg = result?;
    println!("{} ({})", msg.message_type, msg.document.description());

    // The gas day, decoded through the DTM's own format code.
    if let Some(period) = msg.validity_period {
        println!("  Gastag {period}");
    }

    // A LOC group carries a time series — Edig@s SG37 repeats up to 199 times —
    // so a profile transmits many quantities, each with its own period.
    for qty in msg.quantities() {
        println!("  {:?} {:?} {:?}", qty.value, qty.unit, qty.period);
    }

    if msg.message_type == DvgwMessageType::Nomint {
        // RFF+AGO — the nomination this one corrects, for a re-nomination.
        // A NOMRES has no such reference: it is paired on the business key.
        println!("  korrigiert {:?}", msg.original_nomination_ref());
    }

    let report = DvgwPlatform::validate_message(&msg);
    for issue in report.errors() {
        eprintln!("  {issue}");
    }
}
# Ok::<(), dvgw_edi::Error>(())
```

### A quantity is a rate — unless its unit says otherwise

`KW1` is **kWh/h** and `KW2` **kWh/d**, so such a `QTY` states a rate over the
period its own `DTM+2` names, and the energy is `Σ(rate × duration)`; `KWH` is
the energy itself. Which units a family admits is the Segmentlayout's
(`DvgwMessageType::admitted_units`):

```rust
// 100 kWh/h for one hour + 200 kWh/h for two hours = 500 kWh (not 300).
let totals = msg.energy_by_qualifier();
assert_eq!(totals["Z02"].to_string(), "500");
```

Totals stay **per qualifier** because the qualifier is the direction — `Z02` in,
`Z03` out — and a VHP nomination states a purchase and a sale in one interchange.
`single_energy_kwh(keep)` returns one total only when there *is* one, refusing
when the selected positions mix directions or when any quantity could not be
integrated.

The `keep` filter is for NOMRES, which reports both sides of a match: `IMD` `17G`
is what you nominated, `18G` the counterparty's mirror, `16G` the matched result.
Exactly one label may be counted — `16G` when present.

Values are `Decimal`, not float: gas settles to at least three decimal places
(DVGW G 685 §7) and binary floating point cannot hold those fractions exactly.
`Quantity::raw_value` keeps the wire text so a non-numeric value is reportable
rather than silently zero.

The DVGW column of every Nachrichtenstruktur caps `DTM+2` and `SG37 QTY` at one
per `LOC` group, so a profile is a run of `LOC` groups. The reader keeps every
`QTY` it meets under a `LOC` all the same, `DVGW-LOC-MAX` reports the excess,
and the builder writes the conformant shape.

### SSQNOT as one record

```rust
use dvgw_edi::ssqnot::MehrMindermengenmeldung;

let record = MehrMindermengenmeldung::from_message(&msg)?;
// Netzkonto, Netzbetreiber, Abrechnungszeitraum, Verfahren (SLP/RLM),
// Mehrmenge and Mindermenge in kWh.
println!("{} {} kWh", record.netzkonto, record.saldo_kwh());
```

A message the Segmentlayout refuses — no `NAD+ZSH`, no `STS` Verfahren, a
non-numeric Menge — is refused here too rather than read as a partial figure.

### How a message finds its process

ALOCAT 5.11a §3.3 publishes, per Prüfidentifikator, which *Zuordnungstupel* the
receiver applies, and names the segments each element comes from — `ZO-T1`
(Bilanzkreis, Netzbetreiber, Zeitreihentyp) through `ZG-T1` (Clearingnummer).
The Zeitreihentyp is the `STS` code under the quantity (`09G`, `14G`, …), not
`LIN` C212, which reads `Z01` „allokiert" on every position. SSQNOT 5.7 §3.3
adds its own 2-Tupel (Netzkonto, Netzbetreiber), labelled `ZO-T1:SSQNOT`.

```rust
let key = msg.correlation_key().expect("published Zuordnung");
assert_eq!(key.zuordnung, dvgw_edi::Zuordnung::ZoT3);

// `process_key` adds the gas day for the ZO-T* tuples: they identify an
// *object*, and a process is one gas day of it — or one Abrechnungszeitraum
// for a SSQNOT.
assert_eq!(msg.process_key().as_deref(), Some("ZO-T3|BK1|NK1|09G|2026-03-01"));
```

`ZG-T1` is returned unchanged — a Clearingnummer already identifies one
Geschäftsvorfall, and a clearing case legitimately spans several days. A
Prüfidentifikator with no published assignment yields `None` rather than a
guessed key.

Nominations have no published tuple: a NOMRES carries no reference back to the
NOMINT it answers, so the two are paired on the business key both carry.

## Writing

A BKV that can only parse cannot nominate, and a Netzbetreiber that can only
parse cannot report its Mehr-/Mindermengen. `MessageBuilder` renders the header
and `LIN` loops the Nachrichtenbeschreibungen prescribe — `SG1` in the order
each structure lists it, every coded value under its agency (`332`, or `9` for
a GLN through `sender_coded`/`receiver_coded`/`party_coded`), one `LOC` per
period, the family's unit and `UNH` Anwendungscode unless overridden:

```rust
use dvgw_edi::{DvgwDocument, DvgwPeriod, MessageBuilder, Position};
use time::macros::datetime;

let gas_day = DvgwPeriod {
    start: datetime!(2026-03-01 05:00 UTC),
    end:   datetime!(2026-03-02 05:00 UTC),
};

let wire = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
    .document_number("NOMINT00052")
    .version("DVGW17")
    .pruefidentifikator(70030)
    .message_datetime(datetime!(2026-02-28 20:56 UTC))
    .validity_period(gas_day)
    .sender("9870009700005")
    .receiver("9870009700006")
    .position(
        Position::new()
            .location("Z19", Some("ABCD1234"))
            .quantity("Z03", "6782", gas_day)
            .party("ZEU", "BK-CODE-1")
            .party("ZES", "BK-CODE-2"),
    )
    .build()?;
# Ok::<(), dvgw_edi::Error>(())
```

A re-nomination names the original with `original_nomination(reference,
processed_at)`, which writes `RFF+AGO` with the `DTM+9` NOMINT marks
Erforderlich beside it. A SSQNOT is a `MehrMindermengenmeldung` position:
`location("Z99", None)`, `quantity("ZY2", "6782", zeitraum)` (in `KWH`),
`status("A1G")` and `party("ZSH", netzkonto)`.

`build()` refuses rather than emitting a message missing a `Muss` field. The
`UNB`/`UNZ` envelope is deliberately not written — the AS4 layer owns it and its
control reference.

## Validation

`DvgwPlatform::validate` checks the message against the Segmentlayout of its
Nachrichtenbeschreibung: the mandatory `BGM` fields, the three header `DTM` rows
and whether each value matches the format code it declares, `RFF+Z13` and its
range, `NAD+MS`/`NAD+MR`, at least one `LIN`, a `LOC` group per position with
one `DTM+2` and one `QTY`, and the qualifiers, units and position `NAD` rows the
family's Segmentlayout lists. Rows that hang on the Anwendungsfall are keyed on
the Prüfidentifikator: `BGM` DE 1001 must be the code the column publishes
(`DvgwDocument::for_pid`), `RFF+ANX` is Muss on the six Allokationsclearing
columns only, `DTM+9` beside a NOMINT's `RFF+AGO`, the `STS` Verfahren on every
SSQNOT Menge, and the RLM Anwendungsfall (70096, `STS+A2G`) is retired for
Zeiträume from 1.10.2015.

Findings come back as `DvgwIssue` with a typed `Severity` and a stable rule id;
only failures that stop the message being *identified* are `Err`.

## Telling the families apart

A DVGW message rides `ORDERS`/`ORDRSP`, so a BDEW parser accepts one — and reads
`70001` straight out of `RFF+Z13`, exactly where it looks for a
Prüfidentifikator. Neither `UNH` nor the Prüfidentifikator separates the
families; only `BGM` DE 1001 does.

`sniff` reads `BGM` DE 1001 out of the head of the interchange and stops, so an
ingest boundary can decide which parser owns the bytes for the price of one
segment:

```rust
match dvgw_edi::sniff(bytes) {
    Some(document) => { /* DVGW — parse with DvgwPlatform */ }
    None => { /* BDEW — hand to edi-energy */ }
}
```

## Market roles

| Role | Abbreviation |
|---|---|
| Fernleitungsnetzbetreiber | FNB |
| Verteilnetzbetreiber | VNB |
| Bilanzkreisverantwortlicher | BKV |
| Marktgebietsverantwortlicher | MGV |

## Regulatory references

- **§ 20 Abs. 3 EnWG** — Festlegungskompetenz for gas network access and balancing
- **BNetzA BK7-24-01-008** — GaBi Gas 2.1
- **Kooperationsvereinbarung Gas (KoV)** — nomination and allocation deadlines
- **DVGW-Nachrichtenbeschreibungen** ALOCAT 5.11a, NOMINT 4.6, NOMRES 4.7,
  SSQNOT 5.7 —
  <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas>

## Relationship to other crates

| Crate | Layer |
|---|---|
| `dvgw-edi` | EDIFACT parsing / validation / writing — **this crate** |
| `mako-gabi-gas` | GaBi Gas process engine (workflows, deadlines) |
| `edi-energy` | BDEW EDI@Energy formats |
