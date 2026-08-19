# grid-billing

> Deterministic, regulation-aware German grid settlement engine —
> NNE, KA, MMM, MSB, and GeLi Gas AWH Sperrprozesse (PIDs 31001, 31002, 31005, 31006, 31009, 31011).

[![Crates.io](https://img.shields.io/crates/v/grid-billing?label=grid-billing&color=f59e0b&logo=rust)](https://crates.io/crates/grid-billing)

## Regulatory ceilings and structure

### KAV §2 — Konzessionsabgabe

The Höchstbeträge are checked on every settlement, and each position cites the
paragraph its group is actually capped under: **§2 Abs. 2** for Tarifkunden and
Schwachlast, **Abs. 3** for Sondervertragskunden, **Abs. 7** where the customer is
freigestellt.

The rates themselves are undated because the statute has not changed them since
the Euro conversion — the annual reductions people remember were the §3
transitional phase-down, which completed long ago.

### MsbG §30 — Preisobergrenzen für den Messstellenbetrieb

| §30 Abs. 1 band | Netzbetreiber | Letztverbraucher | Total |
|---|---|---|---|
| > 6 000 – ≤ 10 000 kWh | 80 € | 40 € | 120 € |
| > 10 000 – ≤ 20 000 kWh · steuerbare VE · > 7 – ≤ 15 kW | 80 € | 50 € | 130 € |
| > 20 000 – ≤ 50 000 kWh · > 15 – ≤ 25 kW | 80 € | 110 € | 190 € |
| > 50 000 – ≤ 100 000 kWh · > 25 – ≤ 100 kW | 80 € | 140 € | 220 € |
| > 100 000 kWh · > 100 kW | 80 € | angemessenes Entgelt | — |

§30 Abs. 3 (optionaler Einbau) is 30 € each, 60 € total. §30 Abs. 2 adds up to
50 € a year per party for a Steuereinrichtung.

The charge is **annualised before comparison** — billing a year in monthly
instalments does not raise the cap. A charge above the ceiling raises
`MSB_ABOVE_MSBG_POG`.

### §17 StromNEV — Netzebene and Benutzungsstundenzahl

`Netzebene` covers the seven levels, distinguishing network levels from
transformation levels. It is **recorded, not applied**: Netzentgelte are
published per level, so the level is what makes a rate checkable against a price
sheet, but this crate is given rates rather than resolving them.

The same holds for the Benutzungsstundenzahl (annual energy ÷ annual peak). It
does not appear in §17 as a threshold — it is the convention by which a price
sheet publishes two rate pairs — so it goes into the trace rather than selecting
anything. Zero peak yields `None`, not zero.

What *is* enforced is §17 Abs. 6: an Arbeitspreis-only tariff is permitted only
in Niederspannung up to 100 000 kWh a year. Billing without a Leistungspreis
outside that raises `ARBEITSPREIS_ONLY_OUTSIDE_SECT17_ABS6`.

### §19 Abs. 2 StromNEV — individuelle Netzentgelte

Both forms are settled, with the statutory floors — which are in the ordinance
text itself, not only in the BK4-22-089 methodology:

| Form | Qualification | Mindestentgelt |
|---|---|---|
| Atypische Netznutzung (Satz 1) | peak in the low-load windows (BNetzA-approved) | 20 % |
| Intensive Netznutzung (Satz 2) | ≥ 7 000 h **and** ≥ 10 GWh | 20 % |
| | ≥ 7 500 h | 15 % |
| | ≥ 8 000 h | 10 % |

`Sect19Vereinbarung` carries the agreed fraction; the engine applies it as a
reduction over the Arbeits- and Leistungspreis positions **only** — the
Konzessionsabgabe and the levies are untouched, because the Netzbetreiber's lost
revenue is recovered through the §19-Umlage billed separately. An agreement
below the floor raises `SECT19_BELOW_MINDESTENTGELT`; a Satz 2 agreement whose
utilisation data does not qualify raises `SECT19_BANDLAST_CRITERIA_NOT_MET`.

### §18 StromNEV — Entgelte für dezentrale Erzeugung, under Abschmelzung

`settle_dezentrale_einspeisung` pays the plant operator the avoided upstream
costs, at the factor Festlegung **GBK-25-02-1#1** (17.02.2026) leaves standing:

| Period | Factor |
|---|---|
| to 30.06.2026 | 1.00 |
| 01.07.2026 – 31.12.2027 | 0.50 |
| 2028 | 0.25 |
| from 2029 | 0.00 |

The Tenor cuts in three steps (50 % from 01.07.2026, 50 % from 01.01.2027, 75 %
from 01.01.2028) — the annual averages fall by 25 points a year, which is the
decision's own cross-check. A period crossing a step is **refused**, not
averaged; an EEG-funded plant is refused outright (§18 Abs. 1 Satz 4 Nr. 1 —
the payment would be unlawful).

### Gas — Druckstufen and Kapazitätsprodukte (§15 GasNEV)

`Druckstufe` (Hoch-/Mittel-/Niederdruck) is the gas analogue of the Strom
Netzebene; `GasKapazitaet` bills a booked capacity at the price sheet's annual
rate, pro-rated by calendar days, distinguishing feste from unterbrechbarer
Kapazität — the latter cites §15 Abs. 5, and its discount stays where the
ordinance leaves it: on the price sheet, not in this crate.

## Invalid inputs are unrepresentable

`NneInput`'s cross-field rules live in the types, not in a validator a caller
could forget:

| Rule | Enforced by |
|---|---|
| Exactly one Arbeitspreis form (einheitlich, Modul 1 pauschal, Modul 2 prozentual, Modul 3 zeitvariabel, or spot-linked) | `ArbeitspreisModell` — one variant at a time; each replaces the flat position, so the same energy is never billed twice |
| Modul 2 and Modul 3 are mutually exclusive (BK6-22-300) | `ArbeitspreisModell` holds one variant at a time. Note this also blocks the Modul 1 + Modul 3 combination the Festlegung *permits* — see `Sect14aModule::combinable_with` |
| Reduction factors in `(0, 1]` | `Reduktionsfaktor` enforces the range at construction |
| Leistungspreis needs both peak and rate | `Leistungspreis` — a pair |
| Grundpreis needs both rate and months | `Grundpreis` — a pair |
| KAV Höchstbetrag is always checked | `Konzessionsabgabe` pairs the rate with its `KaKundengruppe` |
| Period ordering | `SettlementPeriod` — constructing it is the check |

What the types cannot express — negative energy, empty or inverted Modul 3
intervals — `settle_nne` enforces itself and returns `Err`. There is no
separate NNE validator: `settle_nne` is pure and cheap, run it and read
`warnings`. `validate_mmm_input` / `validate_msb_input` /
`validate_gas_awh_input` exist for the settlement types whose engines accept
looser shapes.

## Settlement, not invoice

The engine calculates **what is owed and why**. It does not know what the invoice
looks like:

```
Input → Validation → Settlement Engine → SettlementResult → InvoiceDocument → BO4E → EDIFACT
```

`SettlementResult` carries the positions, totals, warnings, the applied
`RegulatoryRegime` and a `CalculationTrace` per position. `InvoiceDocument`
carries everything that is a property of the *document* — invoice number, issue
and due dates, the Prüfidentifikator that routes it, the reference to what it
supersedes — and is built by an adapter around a settlement.

The separation is what makes a settlement recomputable: the same period can be
settled twice, for a correction or a dispute or an audit, and the two results
compared, without inventing an invoice number each time.

Position numbering follows the same rule. `InvoiceDocument::numbered_positions()`
assigns 1-based numbers at rendering time; the engine carries no counter.

## No BO4E inside the engine

`SpotPriceFormula` states the pricing formula behind a §14a Modul 3 rate as a
typed value — reference, unit, method, steps — never a `serde_json::Value` carrying
a hand-built BO4E COM. That keeps BO4E *schema knowledge* out of the engine: an
adapter that needs the COM builds it from the value object, and the crate has no
`serde_json` dependency at all.

## SettlementPeriod

A validated pair, not two loose `period_from` / `period_to` dates each calculation
would have to re-check for ordering. Constructing `SettlementPeriod` *is* the check,
so an inverted period is unrepresentable rather than rejected at every call site.

## Regulatory regime

German network-charge law is several timelines, each turning over on its own date:

| Axis | Turns over | Successor |
|---|---|---|
| Netzzugang | 31.12.2025 | §20 Abs. 3 EnWG via BNetzA Festlegungen (GPKE BK6-24-174, GaBi Gas 2.1) |
| Entgeltbildung | 31.12.2028 | BNetzA framework Festlegung *AgNeS*, replacing StromNEV and ARegV |
| Umlagen | annually | ÜNB publication each October |

[`RegulatoryRegime`](src/regulatory.rs) resolves those dates **once**, at the edge;
every calculation then matches on an enum. Scattering `if period_to <= date`
through the engine is how a rule change becomes a bug — each site has to be found
and each has to agree. Adding the AgNeS turnover is a new variant the compiler
forces every deciding site to handle.

The regime can also be supplied explicitly, so a historical settlement is
reproduced under the rules that applied then rather than under today's calendar.
A period crossing a turnover raises `REGIME_TURNOVER_IN_PERIOD`: different rules
govern its start and its end, so it should be split rather than half-billed.

## Explainability

Every position carries a `CalculationTrace` — the inputs used, the paragraphs
applied, the tariff source, the reduction factor, the rounding. `SettlementResult`
additionally exposes `all_legal_refs()`, deduplicated across positions.

These types are `Serialize`, and the service adapters emit them as BO4E
`ZusatzAttribut`e (`mako:calculation_trace` per position,
`mako:legal_references` and `mako:settlement_warnings` per settlement). BO4E has
no field for a calculation trace and inventing one would break the schema; a
`ZusatzAttribut` is the sanctioned place for what a standard does not model.

This matters because the settlement value itself is dropped once the Rechnung is
stored — the attribute is the only surviving record of *why* an amount is what it
is, and it is what a §20 EnWG audit or an LF dispute is answered from.

## Netzseitige Umlagen

Three levies ride on the network charge rather than the commodity, and a Strom
NNE invoice carries all three:

| Levy | Basis | 2026 (nicht privilegiert) |
|---|---|---|
| Aufschlag für besondere Netznutzung (§19 StromNEV-Umlage) | §19 Abs. 2 StromNEV | A′ 1.559 · B′ 0.050 · C′ 0.025 ct/kWh |
| Offshore-Netzumlage | §17f EnWG | 0.941 ct/kWh |
| KWKG-Umlage | §26 KWKG | 0.446 ct/kWh |

Rates are set annually by the ÜNB and published by 25 October for the following
year. They are held as a year-indexed series in [`umlagen`](src/umlagen.rs) so a
correction reopening an earlier period bills it at the rate that applied then —
a single configured scalar cannot express two years at once. `NneInput` carries
a per-levy override for the cases an EnFG decision does not fit the published
schedule.

### Letztverbrauchergruppen (EnFG §§21 ff.)

The Energiefinanzierungsgesetz replaced the older per-levy privilege rules with
one scheme. `Letztverbrauchergruppe` selects the band: **A′** is the full levy
and covers the first 1 GWh at an Entnahmestelle; **B′** and **C′** apply above
that, C′ for energy-intensive undertakings; **Befreit** (§21 EnFG) is zero
rather than reduced, and emits no line at all.

Only the §19 StromNEV-Umlage is published as an explicit A′/B′/C′ schedule. The
other two publish the non-privileged rate, with privileges granted per
Entnahmestelle — supply those through the override.

A year the series does not cover yields **no** rate rather than a neighbouring
year's, and the levy is omitted with an `UMLAGE_RATE_MISSING` warning. Billing
2027 at the 2026 rate would be wrong by an amount nobody notices until the ÜNB
reconciliation.

## Regulatory baseline (2026)

**StromNZV and GasNZV ceased to apply with the end of 31.12.2025** — Art. 15
Abs. 4 (Strom) and Abs. 6 (Gas) of the Gesetz v. 22.12.2023, BGBl. 2023 I Nr. 405.
The successor competence is **§20 Abs. 3 EnWG**, exercised through BNetzA
Festlegungen:

| Domain | Until 31.12.2025 | From 01.01.2026 |
|---|---|---|
| Mehr-/Mindermengen Strom | StromNZV §13 Abs. 3 | GPKE (BK6-24-174) Teil 1 Kap. 8.4 |
| Mehr-/Mindermengen Gas | GasNZV §25 | GaBi Gas 2.1 (BK7-24-01-008) |
| Standardlastprofile Strom | StromNZV §12 | GPKE (BK6-24-174), "Profilverfahren" |
| Standardlastprofile Gas | GasNZV §24 | GaBi Gas 2.1 (BK7-24-01-008) |
| Bilanzkreisabrechnung Strom | StromNZV §4 | MaBiS (Anlage 3 zu BK6-24-174) |
| Konzessionsabgabe | **KAV §2** (unchanged) | KAV §2 |

`settle_mmm` picks its legal references from `period_to`, so a
settlement for a 2025 period still cites the ordinance that governed it and one
for 2026 does not. `LegalReference::citation` appends "(außer Kraft seit
01.01.2026)" to a repealed ordinance, keeping archived invoices self-explanatory.

Konzessionsabgabe is governed by the KAV plus §48 EnWG — not by StromNZV §17
or GasNZV §7, which concern balancing-group and network-access matters.

## Mehr-/Mindermengen sign convention

Both quantities are named from the **network operator's** side, which inverts the
intuitive reading. GPKE Kap. 8.4 Nr. 3:

> Unterschreitet die Summe der in einem Zeitraum ermittelten elektrischen Arbeit
> die Summe der Arbeit, die den bilanzierten Profilen zu Grunde gelegt wurde
> (ungewollte Mehrmenge), so vergütet der Netzbetreiber dem Lieferanten oder dem
> Kunden diese Differenzmenge.

| Measurement vs profile | Quantity | Money |
|---|---|---|
| measured **<** profiled | ungewollte **Mehrmenge** | NB vergütet → **credit** |
| measured **>** profiled | ungewollte **Mindermenge** | NB stellt in Rechnung → **charge** |

GaBi Gas 2.1 states the same for gas: the Ausspeisenetzbetreiber *nimmt
Mehrmengen entgegen* and *liefert Mindermengen*. Consuming below the profile
leaves surplus energy the network absorbed — that surplus is the Mehrmenge, and
it is reimbursed.

## Konzessionsabgabe (KAV §2)

`KaKundengruppe` models the two orthogonal tests KAV actually applies:
Tarifkunde vs Sondervertragskunde is a **contract-type** test, and Tarifkunden
rates band on **municipality inhabitants**, not on annual consumption.

| Group | Strom | Gas |
|---|---|---|
| Tarifkunde, Gemeinde ≤ 25 000 Einw. | 1.32 | 0.51 (Kochen/Warmwasser) · 0.22 (übrige) |
| ≤ 100 000 | 1.59 | 0.61 · 0.27 |
| ≤ 500 000 | 1.99 | 0.77 · 0.33 |
| > 500 000 | 2.39 | 0.93 · 0.40 |
| Schwachlast (Strom only) | 0.61 | — |
| Sondervertragskunde | 0.11 | 0.03 |

These are **Höchstbeträge**, so `settle_nne` emits
`KA_ABOVE_KAV_MAXIMUM` when the agreed rate exceeds the ceiling for the group,
and `KA_CHARGED_WHILE_EXEMPT` when a rate is applied to a §2 Abs. 7 exemption.

## What this crate does

`grid-billing` computes BDEW INVOIC billing positions with full explainability:

- **NNE Strom** (PID 31002, NN-Rechnung) — flat-rate Arbeit, Leistung (RLM), Konzessionsabgabe
- **NNE Gas** (PID 31002, NN-Rechnung) — GasNEV §14 legal basis, auto-set when `Sparte::Gas`
- **§14a modules** — Modul 1 (pauschale Reduzierung), Modul 2 (prozentuale Reduzierung des Arbeitspreises), Modul 3 (zeitvariable Netzentgelte HT/ST/NT, opt-in since 01.04.2025) — BNetzA BK6-22-300 / BK8-22/010-A
- **MMM Strom** (PID 31005) — Mehr-/Mindermengensaldo, GPKE (BK6-24-174) Teil 1 Kap. 8.4
- **MMM Gas** (PID 31005) — Gas imbalance, GaBi Gas 2.1 (BK7-24-01-008)
- **NNE Gas** (PID 31002) — GasNEV §14 Arbeits-/Grundpreis and §15 Kapazitätsentgelt
- **Abschlagsrechnung** (PID 31001) — a payment on account: one Positionszeile, no quantity, no
  Arbeitspreis (INVOIC AHB 1.0b Änd-ID 26817). The invoice that settles the period deducts it
  from what is **owed** via `InvoiceDocument::abschlaege`, never from the net or the tax, because
  §14 Abs. 5 UStG taxed the Anzahlung when it was received
- **MMM Mehrmenge selbst ausgestellt** (PID 31006) — Mehr-/Mindermenge als Lieferung, self-issued (INVOIC AHB Selbstausstellung)
- **MSB-Rechnung** (PID 31009) — Grundgebühr Messstellenbetrieb + optional Messdienstleistung
- **GeLi Gas AWH Sperrprozesse** (PID 31011) — abrechnungswürdige Handlungen (BK7-24-01-009 §5.4)
- **§13a EnWG Redispatch-Vergütung** — `redispatch_verguetung()` computes the angemessene Vergütung per activation (entgangene Einnahmen + zusätzliche − ersparte Aufwendungen; `eeg_entgangene_einnahmen()` for the Nr. 5 EEG basis)
- **Reversal (Stornorechnung)** — `reverse()` negates any prior settlement immutably

- **Umsatzsteuer** — every settlement states its tax (§14 Abs. 4 Nr. 8 UStG). Network services
  are 19 % and never reverse-charged (UStAE 13b.3a excludes them by name); a Mehr-/Mindermenge is
  a *Lieferung* and takes the §13b Abs. 2 Nr. 5 Buchst. b reverse charge on the asymmetric
  condition the statute sets — electricity needs both parties to hold §3g status, gas needs the
  recipient alone. A delivery period straddling a rate change is refused rather than billed at
  one of the two.

All calculations are **pure functions** — zero I/O, zero async, no side effects.
All monetary arithmetic uses `rust_decimal::Decimal` via `EuroAmount` — no `f64` anywhere.

## Architecture

### Settlement flow

```
NneInput / MmmInput / MsbInput / GasAwhInput
        │
        ▼
validate_*_input()          ← optional pre-check: ValidationResult
        │
        ▼
settle_*()                  ← pure, deterministic, no I/O
        │
        ▼
SettlementResult {
  settlement_type, status, period, regime, sparte,
  malo_id, sender_mp_id, recipient_mp_id,
  positions: Vec<SettlementPosition {
    text, kind,                   ← what was charged
    quantity, unit, unit_price_eur, net_eur,
    spot_price_formula,           ← the formula behind the rate, as a value
    trace: CalculationTrace {           ← "why is this amount here?"
      explanation,
      legal_refs: Vec<LegalReference>,  ← StromNEV §17, KAV §2, §14a Modul 2…
      tariff_source: Option<TariffSource>,
      gross_eur, regulatory_reduction_factor, …
    }
  }>,
  total_eur,
  warnings: Vec<SettlementWarning>,
}
        │
        ▼   (adapter — this is where document identity enters)
InvoiceDocument { settlement, pid, rechnungsnummer, invoice_date, due_date }
        │
        ▼   (rubo4e lives in the service; grid-billing has no BO4E dep)
into_rechnung(&document)  → rubo4e::current::Rechnung {
                              rechnungspositionen[].positionsnummer ← assigned here
                              rechnungspositionen[].artikelnummer   ← via kind.artikelnummer()
                              rechnungstyp                          ← Netznutzungsrechnung (NNE + MMM only)
                              netznutzungrechnungsart               ← Handels-/Selbstausgestellt
                              netznutzungrechnungstyp               ← Mehrmindermengenrechnung (MMM only)
                            }
        │
        ▼
InvoicCheckEngine::check(pid, &sender_mp_id, &rechnung, …)
        │
        ▼
invoice_drafts (PostgreSQL) → AS4 dispatch
```

### Netznutzungsrechnung typing

`into_rechnung` marks the document so a consumer can recognise it without
inspecting positions:

| Field | Set for | Value |
|---|---|---|
| `rechnungstyp` | NNE Strom/Gas, MMM Strom/Gas, MMM selbst ausgestellt | `Netznutzungsrechnung` |
| `netznutzungrechnungsart` | the same five | `Selbstausgestellt` for PID 31006, else `Handelsrechnung` |
| `netznutzungrechnungstyp` | MMM only | `Mehrmindermengenrechnung` |

The other four settlement types this engine produces — MSB-Rechnung (31009),
Gas-AWH Sperrung (31011), Redispatch Kostenblatt, dezentrale Einspeisung (§18
StromNEV) — are **left untyped on purpose**. They are not network-use invoices,
and typing them as one would assert something the AHB does not.

`netznutzungrechnungstyp` stays unset for NNE for the same reason: its remaining
codes (Turnus-, Monats-, Abschlags-, Abschluss-, Zwischenrechnung) describe the
billing *cadence*, and the same NNE computation is billed monthly or annually
depending on contract. That is a document fact like `rechnungsnummer`, not a
property of the settlement, so it needs a model change rather than a mapping.

### BDEW Artikelnummern architecture

The service layer owns the BDEW Artikelnummer mapping. `grid-billing` stays free
of `rubo4e`:

```mermaid
flowchart LR
    calc["grid_billing<br/>settle_*()"]
    pos["SettlementPosition<br/>.kind: BillingPositionKind<br/>.trace: CalculationTrace"]
    svc["Service layer<br/>kind_to_artikelnummer()"]
    bo4e["Rechnungsposition<br/>.artikelnummer  ← Gas/MMM/KA<br/>.artikel_id     ← NNE Strom/AWH Gas"]

    calc --> pos --> svc --> bo4e

    note1["BK6-20-160:<br/>NNE Strom replaced<br/>artikelnummer → artikel_id<br/>from PreisblattNetznutzung"]
    note2["BDEW Codeliste v5.6:<br/>Gas NNE/MMM/KA use<br/>classic 9990001… codes<br/>AWH: 2-01-7-001/002"]

    note1 -.->|Strom| bo4e
    note2 -.->|Gas| bo4e
```

### Responsibility split

`grid-billing` has **zero dependency on `rubo4e`**. BO4E conversion lives exclusively in the
service layer, keeping this crate publishable to crates.io without pulling in internal workspace crates.

| Responsibility | Where |
|---|---|
| Settlement math + legal refs | `grid-billing` |
| BO4E `Rechnung` conversion | `netzbilanzd::into_rechnung()` / `invoicd::into_rechnung()` |
| INVOIC plausibility checks 1–6 | `invoic-checker` |
| EDIFACT serialization + AS4 dispatch | `makod` |

## Domain types

### `SettlementResult` — canonical output

```rust
pub struct SettlementResult {
    pub settlement_type: SettlementType, // NneStrom | NneGas | MmmStrom | MsbRechnung | …
    pub status: SettlementStatus,        // Initial | Correction | Reversal | Final
    pub korrektur_grund: Option<KorrekturGrund>, // why — None only for Initial
    pub period: SettlementPeriod,        // validated pair, both bounds inclusive
    pub regime: RegulatoryRegime,        // the rules this calculation applied
    pub sparte: Sparte,
    pub malo_id: String,
    pub sender_mp_id: String,            // NB, or MSB for a MSB-Rechnung (31009)
    pub recipient_mp_id: String,         // LF, NB, MSB, MGV or ESA
    pub positions: Vec<SettlementPosition>,
    pub total_eur: Decimal,              // rounded to 2 dp
    pub warnings: Vec<SettlementWarning>,
}
```

### `InvoiceDocument` — the settlement presented as an invoice

```rust
pub struct InvoiceDocument {
    pub settlement: SettlementResult,
    pub pid: u32,                        // BDEW Prüfidentifikator — routes the document
    pub rechnungsnummer: String,
    pub correction_of: Option<String>,   // what this supersedes
    pub invoice_date: time::Date,
    pub due_date: time::Date,
}
```

Nothing on `InvoiceDocument` affects what is owed. `numbered_positions()` assigns
the 1-based document numbering at render time.

Helper methods on `SettlementResult`:

| Method | Returns | Description |
|---|---|---|
| `is_clean()` | `bool` | `true` when no `Warning`/`Error` severity items in `warnings` |
| `recomputed_total()` | `Decimal` | Re-sums positions — should equal `total_eur` (regression guard) |
| `all_legal_refs()` | `Vec<String>` | Deduplicated citation strings across all positions |
| `positions_count()` | `usize` | Number of settlement positions |

### `SettlementPosition` with `CalculationTrace`

Every position carries a full audit record so any amount can be explained without
re-running the calculation. The `kind` field drives the BDEW Artikelnummer mapping
in the service layer. A position carries **no** position number and **no**
Artikel-ID: both are properties of the *document* that presents the settlement,
not of the calculation — an adapter numbers the positions it renders and resolves
Artikel-IDs (AWH Gas `2-01-7-xxx`, NNE Strom from the `PreisblattNetznutzung`)
from the price sheet:

```rust
pub struct SettlementPosition {
    pub text: String,                        // e.g. "Netznutzung Arbeit HT (§14a Modul 2)"
    pub kind: BillingPositionKind,           // what was charged
    pub quantity: Decimal,                   // rounded to 3 dp
    pub unit: QuantityUnit,                  // Kwh | Kw | Kvarh | Kvar | Monat
    pub unit_price_eur: Decimal,             // rounded to 6 dp
    pub net_eur: Decimal,                    // quantity × unit_price_eur, rounded to 5 dp
    pub spot_price_formula: Option<SpotPriceFormula>,  // the formula behind the rate
    pub trace: CalculationTrace,
}

// No position number and no Artikel-ID: both are properties of the document that
// presents the settlement, not of the calculation.

pub struct CalculationTrace {
    /// Human-readable explanation, e.g.:
    ///   "1500.000 kWh × 0.035000 EUR/kWh = 52.50000 EUR"
    pub explanation: String,
    pub input_quantity: Decimal,
    pub input_unit_price_eur: Decimal,
    pub gross_eur: Decimal,                       // qty × price before rounding
    pub legal_refs: Vec<LegalReference>,          // at least one, always
    pub tariff_source: Option<TariffSource>,      // where the rate came from
    pub regulatory_reduction_factor: Option<Decimal>, // §14a Modul 2 factor (0–1)
    pub rounding_note: Option<&'static str>,
}
```

### `LegalReference`

```rust
pub enum LegalReference {
    StromNev { paragraph: &'static str },       // "§21" Arbeit, "§17" Leistung
    GasNev   { paragraph: &'static str },       // "§14"
    Kav      { paragraph: &'static str },       // "§2 Abs. 2"
    Kwkg     { paragraph: &'static str },       // "§26" KWKG-Umlage
    EnFG     { paragraph: &'static str },       // "§§21 ff." Letztverbrauchergruppe
    Sect14aEnwg { module: Sect14aModule },      // Modul1 | Modul2 | Modul3
    MsbG     { paragraph: &'static str },       // "§§6–7"
    BnetzaDecision { reference: &'static str }, // "BK6-22-300"
    BdewAhb  { reference: &'static str },       // "GPKE BK6-22-024"
    StromNzv { paragraph: &'static str },       // "§13 Abs. 3" — außer Kraft seit 01.01.2026
    GasNzv   { paragraph: &'static str },       // "§25" — außer Kraft seit 01.01.2026
    Enwg     { paragraph: &'static str },       // "§14a"
    ARegV    { paragraph: &'static str },       // "§17" incentive regulation
}
```

`.citation()` returns a short German-language string (e.g. `"StromNEV §17"`,
`"KAV §2 Abs. 2"`, `"ARegV §17"`). Repealed ordinances carry their expiry:
`StromNzv`/`GasNzv` append `"(außer Kraft seit 01.01.2026)"`.

### `Sect14aModule`

```rust
pub enum Sect14aModule {
    Modul1, // §14a pauschale Reduzierung — flat % reduction (BK6-22-300 Anlage 2, default 85%)
    Modul2, // §14a HT/NT time-variable — Zaehlzeitdefinition from UTILTS
    Modul3, // §14a Spotpreis-Netzentgelt — spot-price linked (iMSys required)
}
```

`Sect14aModule::Modul1.label()` = `"§14a EnWG Modul 1 (pauschale Reduzierung)"`;
`.bnentza_reference()` = `"BK6-22-300"` for all three modules.

### `TariffSource`

```rust
pub enum TariffSource {
    PublishedTariffSheet { sheet_id: String },
    HistoricalTariff     { valid_from: time::Date },
    RegulatoryTariff     { decision_ref: &'static str },
    ContractTariff       { contract_ref: String },
    ManualOverride       { reason: String },
}
```

### `Sparte` — commodity dispatch

```rust
#[derive(Default)]
pub enum Sparte {
    #[default]
    Strom,  // → StromNEV §21, SettlementType::NneStrom, PID 31002 (NN-Rechnung)
    Gas,    // → GasNEV §14,   SettlementType::NneGas,   PID 31002 (NN-Rechnung)
}
```

`Sparte` is required on `NneInput` and `MmmInput`. The calculation automatically
selects the correct legal references and `SettlementType` (from which
`default_pid()` yields the PID) — the caller sets no PID for standard Gas paths.

### `SettlementType`

```rust
pub enum SettlementType {
    NneStrom,          // PID 31002 — NN-Rechnung Strom (NB → LF)
    NneGas,            // PID 31002 — NN-Rechnung Gas  (GNB → LFG)
    MmmStrom,          // PID 31005 — MMM Strom, GPKE (BK6-24-174) Teil 1 Kap. 8.4
    MmmGas,            // PID 31005 — MMM Gas,   GaBi Gas 2.1 (BK7-24-01-008) (separate to ensure correct legal refs)
    MmmSelbstausstellt,// PID 31006 — MMM Mehrmenge, selbst ausgestellte Rechnung (Lieferung)
    MsbRechnung,       // PID 31009 — MSB-Rechnung (MSB → NB / LF / ESA)
    GasAwhSperrung,    // PID 31011 — AWH Sperrprozesse Gas (GNB → LFG)
    RedispatchKostenblatt, // no standard PID — Redispatch 2.0 Einsatzkosten (NB → ÜNB)
    DezentraleEinspeisung, // no standard PID — §18 StromNEV, NB → Anlagenbetreiber (bilateral)
}
```

`SettlementType::default_pid()` returns the standard PID for the type; it is `0`
for `RedispatchKostenblatt` and `DezentraleEinspeisung`, which are not EDIFACT
market processes. `MmmGas` and `MmmStrom` share PID 31002 but carry different
legal references.

### `BillingPositionKind` — BDEW Artikelnummern bridge

`BillingPositionKind` is the rubo4e-free type carried by every `SettlementPosition.kind`.
The service layer maps it to `rubo4e::current::BdewArtikelnummer` in `into_rechnung()`.

```rust
pub enum BillingPositionKind {
    NneArbeit,           // Wirkarbeit       (9990001 00026 9)
    NneArbeitHt,         // Wirkarbeit       (9990001 00026 9) — §14a Modul 2 HT
    NneArbeitNt,         // Wirkarbeit       (9990001 00026 9) — §14a Modul 2 NT
    NneArbeitModul1,     // Wirkarbeit       (9990001 00026 9) — Modul 1 Arbeit + pauschale credit
    NneArbeitModul3,     // Wirkarbeit — §14a Modul 3 spot, one position per dispatch interval
    NneLeistung,         // Leistung         (9990001 00005 3)
    NneGasGrundpreis,    // Grundpreis       (9990001 00008 7)
    Konzessionsabgabe,   // Konzessionsabgabe(9990001 00041 7)
    Mehrmenge,           // Mehrmenge        (9990001 00074 8)
    Mindermenge,         // Mindermenge      (9990001 00075 6)
    MsbGrundgebuehr,     // EntgeltEinbauBetriebWartungMesstechnik (9990001 00061 5)
    Messdienstleistung,  // EntgeltMessungAblesung (9990001 00062 3)
    GasAwhSperrung,      // Sperrkosten — Artikel-ID "2-01-7-001" (BK7-24-01-009 §5.4)
    GasAwhEntsprrung,    // Entsperrkosten — Artikel-ID "2-01-7-002"
    GasAwhSonstige,      // Artikel-ID from AwhPositionInput.artikel_id
    Blindmehrarbeit,     // Blindmehrarbeit  (9990001 00047 5)
    Sect19StromNevUmlage,// §19 StromNEV-Umlage — artikelnummer PARAGRAF_19_STROM_NEV_UMLAGE
    OffshoreNetzumlage,  // §17f EnWG — artikelnummer OFFSHORE_HAFTUNGSUMLAGE (legacy code name)
    KwkgUmlage,          // §26 KWKG — artikelnummer ABGABE_KWKG
    DezentraleEinspeisung,   // §18 StromNEV payment out (net_eur negative); no article number
    Sect19IndividuellesEntgelt, // §19 Abs. 2 StromNEV reduction over the Netzentgelt; no article number
    GasKapazitaetsentgelt,   // §15 GasNEV booked capacity — Leistung on Gas
}
```

> **NNE Strom (PIDs 31001/31006):** BK6-20-160 replaced classic `artikelnummer` codes
> with `artikel_id` from the BNetzA Netznutzungspreisblatt. The service layer
> (`netzbilanzd`, `invoicd`) populates `Rechnungsposition.artikel_id` from the tariff
> sheet for those positions; `BillingPositionKind::artikelnummer(settlement_type)`
> returns `None` for Strom NNE. Gas NNE, MMM, Konzessionsabgabe still use classic
> Artikelnummer codes.

Source: BDEW Codeliste Artikelnummern und Artikel-ID v5.6 (valid 01.09.2025).

### `KaKundengruppe` / `GemeindeGroesse` — KAV §2 classifier

KAV applies two orthogonal tests: contract type (Tarifkunde vs
Sondervertragskunde), and — for Tarifkunden — municipality size, not annual
consumption.

```rust
pub enum KaKundengruppe {
    Tarifkunde {                     // KAV §2 Abs. 2 — rate bands on municipality size
        gemeinde: GemeindeGroesse,
        nur_kochen_warmwasser: bool, // Gas only: cooking/hot-water column vs übrige; ignored for Strom
    },
    Schwachlast,          // KAV §2 Abs. 2 — Strom only; gas has no such tier
    Sondervertragskunde,  // KAV §2 Abs. 3 — flat, independent of municipality size
    Exempt,               // KAV §2 Abs. 7 — freigestellt
}

pub enum GemeindeGroesse {
    Bis25k,    // bis 25 000 Einwohner
    Bis100k,   // bis 100 000
    Bis500k,   // bis 500 000
    Ueber500k, // über 500 000
}
```

`KaKundengruppe::hoechstsatz_ct_per_kwh(sparte)` returns the statutory KAV §2
Höchstbetrag (or `None` for `Exempt`, and for `Schwachlast` on Gas).
`.kav_paragraph()` returns the paragraph the group is actually capped under
(`"§2 Abs. 2"`, `"§2 Abs. 3"`, or `"§2 Abs. 7"`) and `.label()` the position
text. The group is carried on `Konzessionsabgabe.klasse`, so the ceiling check
always has what it needs: `settle_nne` emits `KA_ABOVE_KAV_MAXIMUM` when the
agreed rate exceeds the ceiling, and `KA_CHARGED_WHILE_EXEMPT` when a rate is
applied to a §2 Abs. 7 exemption.

## Who uses this library

| Consumer | Role | Use case |
|---|---|---|
| `netzbilanzd` | **NB** (and **MSB** for 31009) | Generate INVOIC 31001/31002/31005/31011 to LF/LFG, and 31009 from the MSB to NB/LF/ESA |
| `invoicd` | **LF** | INVOIC AHB Selbstausstellung selbstausstellen PID 31006 — same formula, LF-initiated |

## Quick start

```bash
cargo add grid-billing
cargo add rust_decimal time
```

### NNE flat-rate (SLP, Strom)

```rust,no_run
use grid_billing::{NneInput, Sparte, SettlementType, settle_nne};
use grid_billing::types::{
    ArbeitspreisModell, MengePreis, Konzessionsabgabe, KaKundengruppe, SettlementPeriod,
};
use grid_billing::umlagen::Letztverbrauchergruppe;
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let settlement = settle_nne(&NneInput {
    malo_id: "51238696012".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    // The delivery period is a validated pair — inverted bounds are unrepresentable.
    period: SettlementPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
    // Letztverbrauchergruppe drives the network-levy rates (EnFG §§21 ff.).
    letztverbrauchergruppe: Letztverbrauchergruppe::A,
    // Exactly one Arbeitspreis form — here a single flat rate.
    arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
        menge_kwh: d("1500"),
        preis_ct_per_kwh: d("3.5"),
    }),
    leistungspreis: None,   // SLP — no RLM demand charge
    grundpreis: None,       // Strom has no separate Grundpreis
    konzessionsabgabe: Some(Konzessionsabgabe {
        satz_ct_per_kwh: d("0.11"),
        klasse: KaKundengruppe::Sondervertragskunde,
    }),
    sparte: Sparte::Strom,
    tariff_sheet_id: Some("Preisblatt-NNE-2026-Q1".into()),
    netzebene: None,
    jahreshoechstleistung_kw: None,
    jahresarbeit_kwh: None,
    sect19: None,            // no §19 Abs. 2 individual charge
    gas_kapazitaet: None,
    sect19_umlage_ct_per_kwh: None,   // use the tabled rate for the delivery year/group
    offshore_umlage_ct_per_kwh: None,
    kwkg_umlage_ct_per_kwh: None,
}).expect("valid NNE input");

// The settlement carries what was settled, not a PID — invoice number, dates and
// the Prüfidentifikator are properties of InvoiceDocument. SettlementType maps to
// the standard PID:
assert_eq!(settlement.settlement_type, SettlementType::NneStrom);
assert_eq!(settlement.settlement_type.default_pid(), 31001);
// recipient_mp_id is auto-populated from lf_mp_id:
assert_eq!(settlement.recipient_mp_id, "9900012345678");

// A Strom NNE settlement also carries the three netzseitige Umlagen (§19 StromNEV,
// Offshore, KWKG) alongside the Arbeit and Konzessionsabgabe positions.
for pos in &settlement.positions {
    println!("{}: {}", pos.text, pos.trace.explanation);
    for lr in &pos.trace.legal_refs {
        println!("  → {}", lr.citation());
    }
}
```

### NNE Gas (GasNEV §14)

```rust,no_run
use grid_billing::{NneInput, Sparte, SettlementType, settle_nne};
use grid_billing::types::{ArbeitspreisModell, MengePreis};

// Only Sparte changes — GasNEV §14 legal refs and SettlementType::NneGas are automatic:
let settlement = settle_nne(&NneInput {
    sparte: Sparte::Gas,  // ← drives GasNEV §14 + NneGas (PID 31002)
    arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
        menge_kwh: d("3000"),        // already kWh_Hs from edmd gas conversion
        preis_ct_per_kwh: d("1.80"),
    }),
    konzessionsabgabe: None,  // KA typically not applicable for Gas
    grundpreis: None,
    leistungspreis: None,
    // … other identity + levy-override fields, all None …
}).unwrap();

assert_eq!(settlement.settlement_type, SettlementType::NneGas);
assert_eq!(settlement.settlement_type.default_pid(), 31002);
```

### §14a Modul 3 — zeitvariable Netzentgelte (HT/ST/NT, opt-in since 2025-04-01)

```rust,no_run
use grid_billing::{NneInput, Sparte, settle_nne};
use grid_billing::types::{
    ArbeitspreisModell, MengePreis, Konzessionsabgabe, KaKundengruppe, GemeindeGroesse,
};

let settlement = settle_nne(&NneInput {
    // Modul 3 requires all three bands; the enum makes the flat/ToU states exclusive.
    arbeitspreis: ArbeitspreisModell::Modul3ZeitVariabel {
        ht: MengePreis { menge_kwh: d("600"), preis_ct_per_kwh: d("4.20") },
        nt: MengePreis { menge_kwh: d("400"), preis_ct_per_kwh: d("1.50") },
    },
    konzessionsabgabe: Some(Konzessionsabgabe {
        satz_ct_per_kwh: d("1.32"),
        // The group fixes the KAV §2 ceiling and annotates the position for audit.
        klasse: KaKundengruppe::Tarifkunde {
            gemeinde: GemeindeGroesse::Bis25k,
            nur_kochen_warmwasser: false,
        },
    }),
    sparte: Sparte::Strom,
    tariff_sheet_id: Some("Preisblatt-14a-2026".into()),
    leistungspreis: None,
    grundpreis: None,
    // … identity + levy-override fields …
}).unwrap();

// Positions: HT + NT Arbeit, the three netzseitige Umlagen, and Konzessionsabgabe.
assert!(settlement.all_legal_refs().iter().any(|r| r.contains("§14a EnWG Modul 2")));
```

### §14a Modul 1 — pauschale Reduzierung (offered since 2024-01-01)

Modul 1 is a **flat annual amount**, credited pro rata for the settlement period.
It does not scale with consumption — that is what makes it *pauschal*, and what
separates it from Modul 2, which reduces the Arbeitspreis by a percentage. The
energy is billed at the full Arbeitspreis and the credit sits beside it as its
own position, so the invoice shows both.

```rust,no_run
use grid_billing::{NneInput, Sparte, settle_nne};
use grid_billing::types::{ArbeitspreisModell, MengePreis};
use rust_decimal::{Decimal, dec};

let settlement = settle_nne(&NneInput {
    // Modul 1 is a variant of ArbeitspreisModell, so it cannot coexist with a
    // flat rate or the Modul 3 bands — the conflict is unrepresentable.
    arbeitspreis: ArbeitspreisModell::Modul1Pauschal {
        basis: MengePreis { menge_kwh: d("1500"), preis_ct_per_kwh: d("3.5") },
        // The NB's published annual amount, and the share of a year this
        // period covers.
        pauschale_eur_pro_jahr: dec!(120.00),
        jahresanteil: Decimal::ONE / Decimal::from(12u32),
    },
    sparte: Sparte::Strom,
    leistungspreis: None,
    grundpreis: None,
    konzessionsabgabe: None,
    // … other fields …
}).unwrap();

// 1500 × 0.035 × 0.85 = 44.625 → 44.62 EUR (MidpointNearestEven)
assert!(settlement.all_legal_refs().iter().any(|r| r.contains("Modul 1")));
assert!(settlement.positions[0].trace.regulatory_reduction_factor == Some(dec!(0.85)));
```

### Gas NNE with Grundpreis (GasNEV monthly standing charge)

```rust,no_run
use grid_billing::{NneInput, Sparte, settle_nne};
use grid_billing::types::{ArbeitspreisModell, MengePreis, Grundpreis};

let settlement = settle_nne(&NneInput {
    sparte: Sparte::Gas,
    arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
        menge_kwh: d("3000"),
        preis_ct_per_kwh: d("1.80"),
    }),
    // Grundpreis pairs the monthly rate with the months billed — one without the
    // other is meaningless, so they travel together.
    grundpreis: Some(Grundpreis {
        eur_per_month: d("15.00"),  // monthly base fee from PreisblattNetznutzung
        months: d("1"),
    }),
    leistungspreis: None,
    konzessionsabgabe: None,
    // … other fields …
}).unwrap();

// Gas carries no netzseitige Umlagen: Grundpreis (15.00) + Arbeit (54.00) = 69.00 EUR
assert_eq!(settlement.positions.len(), 2);
assert!(settlement.positions[0].text.contains("Grundpreis"));
```

### GeLi Gas AWH Sperrprozesse (PID 31011)

```rust,no_run
use grid_billing::{GasAwhInput, AwhPositionInput, SettlementType, settle_gas_awh};
use grid_billing::types::SettlementPeriod;
use time::macros::date;

let settlement = settle_gas_awh(&GasAwhInput {
    malo_id: "51238696012".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    period: SettlementPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
    tariff_sheet_id: Some("Preisblatt-AWH-2026".into()),
    awh_positionen: vec![
        AwhPositionInput {
            beschreibung: "Sperrung Gaszähler".into(),
            anzahl: 1,
            preis_eur: d("45.00"),
            artikel_id: Some("2-01-7-001".to_owned()),  // BDEW Codeliste v5.6 §3.2
        },
        AwhPositionInput {
            beschreibung: "Entsperrung Gaszähler".into(),
            anzahl: 1,
            preis_eur: d("45.00"),
            artikel_id: Some("2-01-7-002".to_owned()),
        },
    ],
}).unwrap();

// Invoice number and dates live on InvoiceDocument, not on the settlement:
assert_eq!(settlement.settlement_type, SettlementType::GasAwhSperrung);
assert_eq!(settlement.settlement_type.default_pid(), 31011);
assert_eq!(settlement.total_eur, d("90.00"));
// Both positions cite BK7-24-01-009 §5.4
assert!(settlement.all_legal_refs().iter().any(|r| r.contains("BK7-24-01-009")));
```

### Correction lifecycle (reversal + replacement pair)

Two different facts are recorded in two different places, and the split is
deliberate:

- **What was replaced** — invoice numbers — lives on the `InvoiceDocument`.
  The same pair of settlements can be presented under different invoice numbers,
  so the chain is a property of the documents exchanged.
- **Why the recalculation happened** — `KorrekturGrund` — lives on the
  `SettlementResult`. That is a fact about the settlement, and the invoice
  numbers never answer it: they cannot say whether the meter was wrong, the
  tariff was wrong, or the law changed underneath. Those have different
  consequences, so `reverse()` and `correct()` require the reason.

```rust,no_run
use grid_billing::{settle_nne, correct, KorrekturGrund, SettlementStatus};

let original = settle_nne(&nne_input).unwrap();
let corrected = settle_nne(&corrected_input).unwrap();

let (reversal, replacement) =
    correct(&original, corrected, KorrekturGrund::Tarifkorrektur);

assert_eq!(reversal.status, SettlementStatus::Reversal);
assert_eq!(reversal.total_eur, -original.total_eur);
assert_eq!(replacement.status, SettlementStatus::Correction);
assert_eq!(replacement.korrektur_grund, Some(KorrekturGrund::Tarifkorrektur));
assert!(replacement.lineage_is_consistent());
```

| `KorrekturGrund` | Meaning | Defect? |
|---|---|---|
| `Messwertkorrektur` | replaced or re-read metering (§ 60 Abs. 2 MsbG) | no |
| `Tarifkorrektur` | wrong tariff or price-sheet version applied | no |
| `Stammdatenkorrektur` | wrong Netzebene, KA-Klasse or Konzessionsgemeinde | **yes** |
| `RegulatorischeAenderung` | a regulatory change applies retroactively | no |
| `Rechenfehler` | arithmetic or logic error in the original | **yes** |
| `Clearing` | a clearing result between the parties (MMM, MaBiS) | no |
| `Sonstiges` | anything else — detail rides in the warnings | no |

`indicates_defect()` separates the two: a rising `Rechenfehler` count is an
engineering signal, a rising `RegulatorischeAenderung` count is not.
`lineage_is_consistent()` catches the state this exists to prevent — a
`Correction` with no reason, which looks like a complete settlement and answers
none of the questions an audit asks of one.

```rust,no_run
use grid_billing::{settle_nne, reverse, SettlementStatus};

let original = settle_nne(&/* … NneInput … */).unwrap();

// reverse() mirrors every position with the sign flipped; it takes only the
// original — the storno invoice number and dates belong to the InvoiceDocument.
let storno = reverse(&original);

assert_eq!(storno.status, SettlementStatus::Reversal);
assert_eq!(storno.total_eur, -original.total_eur);
```

### Pre-calculation validation

```rust,no_run
use grid_billing::{MmmInput, validate_mmm_input};

let input = MmmInput { /* … */ };
let v = validate_mmm_input(&input);

if !v.is_valid {
    for w in &v.warnings {
        eprintln!("[{}] {}", w.code, w.message);
    }
    return;
}
let settlement = grid_billing::settle_mmm(&input).unwrap();
```

(`settle_nne` validates inline — malformed NNE input returns `Err` directly;
`validate_mmm_input` / `validate_msb_input` / `validate_gas_awh_input` exist
for the settlement types where a pre-flight warning list is useful.)

### Service-layer conversion to BO4E `Rechnung`

```rust,no_run
// In netzbilanzd/src/billing.rs — grid-billing itself has no rubo4e dep:
use grid_billing::{InvoiceDocument, QuantityUnit};
use rubo4e::current::{Betrag, Menge, Mengeneinheit, Preis, Rechnungsposition, Rechnung, Zeitraum};

fn into_rechnung(doc: &InvoiceDocument) -> Rechnung {
    let s = &doc.settlement;
    let lz = Zeitraum {
        // SettlementPeriod is a validated pair, read via its accessors:
        startdatum: Some(s.period.from()),
        enddatum:   Some(s.period.to()),
        ..Default::default()
    };
    // numbered_positions() assigns the 1-based document numbering at render time;
    // the engine carries no position counter.
    let positions = doc.numbered_positions().map(|(nr, p)| {
        let einheit = match p.unit {
            QuantityUnit::Kwh   => Some(Mengeneinheit::Kwh),
            QuantityUnit::Kw    => Some(Mengeneinheit::Kw),
            QuantityUnit::Kvarh => Some(Mengeneinheit::Kwh),   // map kVARh → kWh bucket
            QuantityUnit::Kvar  => Some(Mengeneinheit::Kw),    // map kVAR  → kW  bucket
            QuantityUnit::Monat => Some(Mengeneinheit::Monat),
        };
        Rechnungsposition {
            positionsnummer:    Some(i64::from(nr)),
            positionstext:      Some(p.text.clone()),
            // BillingPositionKind::artikelnummer() returns the BDEW codelist name
            // (Gas/MMM/KA…), or None for Strom NNE, which carries an Artikel-ID
            // the renderer resolves from the tariff sheet instead.
            artikelnummer:      p.kind.artikelnummer(s.settlement_type)
                                    .and_then(|name| name.parse().ok()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(Menge { wert: Some(p.quantity), einheit, ..Default::default() }),
            einzelpreis:  Some(Preis  { wert: Some(p.unit_price_eur.round_dp(6)), ..Default::default() }),
            gesamtpreis:  Some(Betrag { wert: Some(p.net_eur.round_dp(5)), ..Default::default() }),
            ..Default::default()
        }
    }).collect();
    Rechnung {
        // Document identity comes from InvoiceDocument, not the settlement:
        rechnungsnummer:   Some(doc.rechnungsnummer.clone()),
        rechnungsdatum:    Some(doc.invoice_date),
        faelligkeitsdatum: Some(doc.due_date),
        rechnungsperiode:  Some(lz),
        gesamtnetto: Some(Betrag { wert: Some(s.total_eur), ..Default::default() }),
        rechnungspositionen: Some(positions),
        ..Default::default()
    }
}
```

## Generated invoice types

| PID | Description | Direction | Sparte |
|---|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung | NB → LF | both |
| 31002 | NN-Rechnung Strom (Netznutzung) | NB → LF | Strom |
| 31002 | NN-Rechnung Gas (Netznutzung) | GNB → LFG | Gas (auto via `Sparte::Gas`) |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo) | NB → LF | both |
| 31006 | MMM Mehrmenge, selbst ausgestellt | LF | both |
| 31009 | MSB-Rechnung | **MSB → NB / LF / ESA** | Strom |
| 31011 | AWH Sperrprozesse Gas | GNB → LFG | Gas |

## Billing position reference

### NNE

| # | Position text | Unit | `kind` | Condition | Legal basis | Artikelnummer |
|---|---|---|---|---|---|---|
| 1 | `Netznutzung Arbeit` | kWh | `NneArbeit` | `arbeitspreis: ArbeitspreisModell::Einheitlich` | StromNEV §21 (Strom) · GasNEV §14 (Gas) | `Wirkarbeit` (Gas); `artikel_id` (Strom) |
| 1 | `Netznutzung Arbeit §14a Modul 1 (85% Reduzierung)` | kWh | `NneArbeitModul1` | `arbeitspreis: ArbeitspreisModell::Modul1Pauschal` | §14a EnWG Modul 1 · BK6-22-300 | same as NneArbeit |
| 1–3 | `Netznutzung Arbeit HT/ST/NT (§14a Modul 3)` | kWh | `NneArbeitHt` / `NneArbeitSt` / `NneArbeitNt` | `arbeitspreis: ArbeitspreisModell::Modul3ZeitVariabel` | §14a EnWG Modul 3 · BK6-22-300 | same as NneArbeit |
| opt | `Netzentgelt Grundpreis Gas` | Monat | `NneGasGrundpreis` | `grundpreis` set | GasNEV §14 | `Grundpreis` |
| next | `Netznutzung Leistung` | kW | `NneLeistung` | `leistungspreis` set (RLM) | StromNEV §17 | `Leistung` (Gas); `artikel_id` (Strom) |
| next | `Blindmehrarbeit` | kvarh | `Blindmehrarbeit` | `blindarbeit` set **and** the draw exceeds the free share | StromNEV §17 (Preisblatt) | `Blindmehrarbeit` |
| last | `Konzessionsabgabe[tier]` | kWh | `Konzessionsabgabe` | `konzessionsabgabe` set | KAV §2 Abs. 2 | `Konzessionsabgabe` |

#### Blindmehrarbeit

A Netzbetreiber supplies a *free share* of reactive energy alongside the active
energy and charges only what exceeds it. The customary boundary is a power factor
of cos φ 0,9 — reactive energy up to **tan φ ≈ 0,4843** of the active energy —
but many Preisblätter round that to a flat 50 %, and some set separate shares for
inductive and capacitive draw.

The share is therefore an **input**, not a constant: it is a term of the price
sheet, and hard-coding one would bill some networks wrongly.
`Blindarbeit::COS_PHI_0_9` is the documented default.

```rust,no_run
use grid_billing::{Blindarbeit, NneInput};
use rust_decimal::dec;

let blindarbeit = Some(Blindarbeit {
    blindarbeit_kvarh: dec!(600),
    freigrenze_anteil: Blindarbeit::COS_PHI_0_9,
    preis_ct_per_kvarh: dec!(2.0),
});
// 1 000 kWh active → 484,3 kvarh free → 115,7 kvarh charged.
```

An unused allowance is never a credit — the excess floors at zero. The charge
rests on the Netzbetreiber's published Preisblatt, formed under **StromNEV §17**;
it is not §18 (Entgelt für dezentrale Erzeugung) and not §19 (Sonderformen der
Netznutzung).

### MMM

| # | Position text | `kind` | Artikelnummer | Condition |
|---|---|---|---|---|
| 1 | `Mehrmengen` | `Mehrmenge` | `Mehrmenge` | `actual > profil` |
| 2 | `Mindermengen (Gutschrift)` | `Mindermenge` | `Mindermenge` | `profil > actual` |

### MSB

| # | Position text | `kind` | Artikelnummer | Condition |
|---|---|---|---|---|
| 1 | `Grundgebühr Messstellenbetrieb` | `MsbGrundgebuehr` | `EntgeltEinbauBetriebWartungMesstechnik` | Always |
| 2 | `Messdienstleistung` | `Messdienstleistung` | `EntgeltMessungAblesung` | `messdienstleistung_eur` set |

### AWH Gas Sperrprozesse (PID 31011)

| # | Position text | `artikel_id` | Condition |
|---|---|---|---|
| any | `Sperrung Gaszähler` | `2-01-7-001` | Unterbrechung reguläre AZ |
| any | `Entsperrung Gaszähler` | `2-01-7-002` | Wiederherstellung reguläre AZ |
| any | `Erfolglose Unterbrechung` | `2-01-7-003` | Sperrung failed |
| any | `Stornierung Sperrauftrag (Vortag)` | `2-01-7-004` | Cancelled day before |
| any | `Stornierung Sperrauftrag (Sperrtag)` | `2-01-7-005` | Cancelled same day |
| any | `Entsperrung außerhalb AZ` | `2-01-7-006` | Out of hours |

Source: BDEW Codeliste Artikelnummern und Artikel-ID v5.6, Section 3.2 (valid 01.09.2025).

## Design invariants

| Invariant | Detail |
|---|---|
| **No floating-point money** | `rust_decimal::Decimal` throughout; `EuroAmount` for overflow guard. No `f64`. |
| **No rubo4e dependency** | Returns `SettlementResult`; service layer owns `into_rechnung()`. |
| **`recipient_mp_id` auto-populated** | `lf_mp_id` (NNE/MMM) or `empfaenger.mp_id` (PID 31009) copied automatically; `sender_mp_id` is the NB, or the **MSB** for 31009. |
| **`Sparte` drives settlement type** | `Sparte::Gas` → `SettlementType::NneGas`, `GasNEV §14`. NN-Rechnung is PID 31002 for both Sparten — the Sparte rides on `Rechnung.sparte`, not on the Prüfidentifikator. |
| **Every position cites regulation** | `trace.legal_refs` is non-empty for every position. Enables BNetzA audit without re-calculation. |
| **Artikelnummer on every position** | `BillingPositionKind::artikelnummer()` in this crate. Never empty. |
| **`MmmGas` ≠ `MmmStrom`** | Separate `SettlementType` variants ensure correct legal refs (`GaBi Gas 2.1 (BK7-24-01-008)` vs `GPKE (BK6-24-174) Teil 1 Kap. 8.4`) per position. |
| **Immutable correction chain** | `reverse()` mirrors positions, sets `status = Reversal`, links via `correction_of`. Original never mutated. |
| **`correct()` pair** | Returns `(reversal, replacement)` — both get status set atomically; caller dispatches both. |
| **Abschläge reduce `zuZahlen` only** | `gesamtnetto` and `gesamtsteuer` stand; §14 Abs. 5 UStG taxes an Anzahlung on receipt, so the settling invoice does not tax it again. |
| **Cadence is a document fact** | `IMD+7081` (Turnus-, Monats-, Abschlags-, Abschluss-, Zwischenrechnung) rides on `InvoiceDocument`, not on `SettlementType`: the same settlement is the same arithmetic at any billing rhythm. |
| **Pure functions** | All settlement functions are sync with no side effects. |
| **`recomputed_total` guard** | `debug_assert_eq!(result.total_eur, result.recomputed_total())` inside every settlement function — catches rounding bugs in debug builds. |

## See also

- [`invoic-checker`](../invoic-checker/README.md) — validates the generated `Rechnung` in the service layer
- [`netzbilanzd`](../../services/netzbilanzd/README.md) — NB billing service that calls `grid-billing`
- [`invoicd`](../../services/invoicd/README.md) — LF service using `grid-billing` for selbstausstellen
- [Operator guide → netzbilanzd](https://hupe1980.github.io/mako/docs/services/netzbilanzd/)

