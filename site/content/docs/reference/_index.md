+++
title = "Reference"
description = "Deep reference for parsing, validation, builders, the platform API, and every process."
weight = 3
sort_by = "weight"
template = "section.html"
page_template = "page.html"
+++
The reference layer beneath the guides: EDIFACT parsing, validation and message
builders via `edi-energy` (including the `Platform` struct for multi-party use),
the BDEW AS4 profile that carries those messages, the catalogue of every MaKo
process, Redispatch 2.0, DVGW gas transport, and the `makotest` Python toolkit
for testing against all of it.

## The pages

| Page | Read it for |
|---|---|
| [Parsing](@/docs/reference/parsing.md) | Turning EDIFACT bytes into typed messages — every entry point, the DoS limits, the error variants |
| [Validation](@/docs/reference/validation.md) | What "valid" means: the three layers, the report API, rule ids |
| [Builders](@/docs/reference/builders.md) | Constructing a message that validates, with the type-state builders |
| [Platform](@/docs/reference/platform.md) | Multi-tenant and test-isolated profile registries, instead of the global one |
| [Process catalogue](@/docs/reference/processes.md) | Every MaKo process: who starts it, which messages flow, which Frist applies, what mako implements |
| [Redispatch 2.0](@/docs/reference/redispatch.md) | The XML document family, the workflows, and the BilAReM regime |
| [DVGW EDI](@/docs/reference/dvgw.md) | Gas transport messages (ALOCAT, NOMINT, NOMRES, SSQNOT) |
| [BDEW AS4](@/docs/reference/as4-bdew.md) | The transport that carries every EDIFACT interchange between market partners |
| [makotest](@/docs/reference/makotest.md) | Writing tests in Python against the real Fristen and the real validator |

Two pages are large enough to be used as lookup tables rather than read
end to end: the [process catalogue](@/docs/reference/processes.md), which is
ordered by regulatory framework, and the
[PID reference](@/docs/regulatory/pid-reference.md) over in Regulatory, which
maps every Prüfidentifikator to the crate and workflow that owns it.

## Vocabulary

German market communication has a small, load-bearing vocabulary. These terms
recur on every page below. Fuller definitions, with the identifier formats and
the check-digit rules, are in the
[glossary](@/docs/architecture/domain-model.md#glossary).

| Term | What it is |
|---|---|
| **MaKo** | Marktkommunikation — the regulated message exchange between market participants, prescribed by BNetzA Festlegungen |
| **Prüfidentifikator** (PID) | A five-digit number identifying one business case, e.g. `55001` Lieferbeginn Strom. It selects the AHB column a message is validated against, and the workflow that handles it |
| **AHB** | Anwendungshandbuch — one column per Prüfidentifikator, saying what that case must, may and must not send |
| **MIG** | Nachrichtenimplementierungshandbuch — the message structure itself: segments, groups, data elements, code lists. One per message type |
| **EBD** | Entscheidungsbaumdiagramm — a BDEW decision tree (`E_0406`, `E_0623`, …) whose leaves are the Antwortcodes (`A01`, `A50`, …) an answer may carry |
| **Marktlokation** (MaLo) | Where energy is consumed or produced, as the market accounts for it — an 11-digit id. Not a meter |
| **Messlokation** (MeLo) | Where a meter sits — a 33-character Zählpunktbezeichnung. Several MeLo can serve one MaLo |
| **Bilanzkreis** | The balancing account energy is booked to. Every MaLo is assigned to one; the BKV answers for its balance |
| **Frist** / **Werktag** | A regulatory deadline, counted in Werktage (business days) on the BDEW calendar — not in calendar days, and not in the local Bundesland's holidays |
| **Sparte** | Strom or Gas. The same business process usually has a separate PID band per Sparte |

See [Domain model](@/docs/architecture/domain-model.md) for the identifier
formats and check digits, and
[Dates and days](@/docs/architecture/domain-model.md#dates-and-days) for what
"today" means in a market date.

### The four market roles

Every process is a conversation between two of these
([full role table](@/docs/architecture/domain-model.md#party-roles-marktrollen)):

| Role | Marktrolle | Answers for |
|---|---|---|
| **LF** | Lieferant | Supplying the customer — the retail contract, the Bilanzkreis, the invoice |
| **NB** | Netzbetreiber | The grid the MaLo hangs on — access, grid charges, and most of the confirmations |
| **MSB** | Messstellenbetreiber | The meter — installing it, reading it, and delivering the values |
| **BKV** | Bilanzkreisverantwortlicher | A Bilanzkreis's balance against the ÜNB |

A deployment is scoped to the roles it plays, and a role build contains no other
arm's code — informatorische Entflechtung, § 6a EnWG.

### The 17 EDIFACT message types

`edi-energy` ships a generated MIG and AHB for each:

| Message | Carries |
|---|---|
| `UTILMD` | Stammdaten and the master-data processes — Lieferbeginn, Kündigung, MSB-Wechsel. The largest family by far |
| `MSCONS` | Metered values — Zählerstände, Lastgänge, Zählerstandsgänge |
| `INVOIC` | Invoices — grid charges, MSB charges, retail, Mehr-/Mindermengen |
| `REMADV` | The invoice answer: paid, or itemised rejection |
| `COMDIS` | Handelsunstimmigkeit — a dispute over a REMADV or an IFTSTA |
| `APERAK` | Application-level acknowledgement or rejection of a received message |
| `CONTRL` | Syntax acknowledgement at interchange level. The only AHB with no Prüfidentifikatoren |
| `ORDERS` | An order — Sperrauftrag, Werteanforderung, Konfigurationsbestellung |
| `ORDRSP` | The answer to an order |
| `ORDCHG` | A change to an order already placed |
| `REQOTE` | Request for quotation — the ESA asking an MSB for a price |
| `QUOTES` | The quotation itself |
| `PRICAT` | Price lists — Ausgleichsenergiepreise, MSB- and NB-Preisblätter |
| `IFTSTA` | Status reports — Sperrung/Entsperrung progress, WiM Umsetzungsstatus, Redispatch |
| `INSRPT` | Störungsmeldung and Ablesesteuerung |
| `PARTIN` | Kommunikationsdaten — who to reach at a market partner, and how |
| `UTILTS` | Berechnungsformeln and Zählzeitdefinitionen between grid operators |

The wire code is the one in `UNH`; the Cargo feature that compiles each type in
is listed in `crates/edi-energy/src/message_type.rs`.
