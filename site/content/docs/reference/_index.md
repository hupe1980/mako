+++
title = "Reference"
description = "Deep reference for parsing, validation, builders, the platform API, and every process."
weight = 3
sort_by = "weight"
template = "section.html"
page_template = "page.html"
+++
The reference layer beneath the guides: `edi-energy` parsing, validation and
builders, the AS4 transport, the process catalogue, Redispatch 2.0, DVGW gas
transport, and the `makotest` Python toolkit.

## The pages

| Page | Read it for |
|---|---|
| [Cold restore](@/docs/reference/recovery.md) | Recover a supplier close, reconcile an accepted delivery and decide when to resume |
| [Parsing](@/docs/reference/parsing.md) | Turning EDIFACT bytes into typed messages — every entry point, the DoS limits, the error variants |
| [Validation](@/docs/reference/validation.md) | What "valid" means: the three layers, the report API, rule ids |
| [Builders](@/docs/reference/builders.md) | Constructing a message that validates, with the type-state builders |
| [Platform](@/docs/reference/platform.md) | Multi-tenant and test-isolated profile registries, instead of the global one |
| [Process catalogue](@/docs/reference/processes.md) | Every MaKo process: who starts it, which messages flow, which Frist applies, what mako implements |
| [Redispatch 2.0](@/docs/reference/redispatch.md) | The XML document family, the workflows, and the BilAReM regime |
| [DVGW EDI](@/docs/reference/dvgw.md) | Gas transport messages (ALOCAT, NOMINT, NOMRES, SSQNOT) |
| [BDEW AS4](@/docs/reference/as4-bdew.md) | The transport that carries every EDIFACT interchange between market partners |
| [makotest](@/docs/reference/makotest.md) | Writing tests in Python against the real Fristen and the real validator |
| [Operator policy](@/docs/reference/operator-policy.md) | Every operator setting: its key, its options with their primary sources, its default or that it is required |
| [HTTP APIs](@/docs/reference/api.md) | The OpenAPI 3.1 document of every service, as it commits and serves it |
| [Event catalog](@/docs/reference/events.md) | Every CloudEvent type: who emits it, who names it, and the JSON Schema of its `data` |

The process catalogue and the [PID reference](@/docs/regulatory/pid-reference.md)
are lookup tables, ordered by regulatory framework and by message type.

## Vocabulary

The terms every page below uses; fuller definitions are in the
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

### The six market roles a deployment plays

A deployment plays one or more of these; its counterparties include the
others of the Rollenmodell — the ÜNB and the BIKO among them
([full role table](@/docs/architecture/domain-model.md#party-roles-marktrollen)):

| Role | Marktrolle | Answers for |
|---|---|---|
| **LF** | Lieferant | Supplying the customer — the retail contract, the Bilanzkreis, the invoice |
| **NB** | Netzbetreiber | The grid the MaLo hangs on — access, grid charges, and most of the confirmations |
| **MSB** | Messstellenbetreiber | The meter — installing it, reading it, and delivering the values |
| **ESA** | Energieserviceanbieter | Services on metered values — ordering them from the MSB under the § 34 MsbG Wertebestellung |
| **BKV** | Bilanzkreisverantwortlicher | A Bilanzkreis's balance against the ÜNB |
| **EIV** | Einsatzverantwortlicher | A plant's dispatch under Redispatch 2.0 — its plans, unavailabilities and the answer to an activation |

A deployment is scoped to the roles it plays: makod registers the modules of
the roles its `[deployment] rollen` declares. Which principal may read which
role's processes — the confidentiality § 6a Abs. 1 EnWG asks for — is the
deployment's Cedar policy, not the role scope.

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
