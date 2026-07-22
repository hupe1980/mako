# grid-billing

> Deterministic, regulation-aware German grid settlement engine —
> NNE, KA, MMM, MSB, and GeLi Gas AWH Sperrprozesse (PIDs 31001, 31002, 31005, 31006, 31009, 31011).

[![Crates.io](https://img.shields.io/crates/v/grid-billing?label=grid-billing&color=f59e0b&logo=rust)](https://crates.io/crates/grid-billing)

## Regulatory ceilings and structure

### KAV §2 — Konzessionsabgabe

The Höchstbeträge are checked on every settlement, and the position now cites the
paragraph its group is actually capped under: **§2 Abs. 2** for Tarifkunden and
Schwachlast, **Abs. 3** for Sondervertragskunden, **Abs. 7** where the customer is
freigestellt. Every position previously cited Abs. 2 regardless.

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
`MSB_ABOVE_MSBG_POG`. This was previously unchecked: the MSB settlement validated
only that the fee was non-negative, while the analogous KAV ceiling *was*
checked.

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
| Exactly one Arbeitspreis form (einheitlich, Modul 1, HT/NT Modul 2, or Modul 3 spot) | `ArbeitspreisModell` — one variant at a time; `Modul3Spotpreis` replaces the flat position, so the same energy is never billed twice |
| §14a modules are mutually exclusive | `ArbeitspreisModell` — `Modul1Pauschal`/`Modul2ZeitVariabel`/`Modul3Spotpreis` are variants of the same enum |
| Reduction factors in `(0, 1]` | `Reduktionsfaktor` enforces the range at construction |
| Leistungspreis needs both peak and rate | `Leistungspreis` — a pair |
| Grundpreis needs both rate and months | `Grundpreis` — a pair |
| KAV Höchstbetrag is always checked | `Konzessionsabgabe` pairs the rate with its `KaKlasse` |
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
value — reference, unit, method, steps. It used to be a `serde_json::Value`
holding a hand-built BO4E `LastvariablePreisposition`, which kept the *type*
dependency out while moving BO4E *schema knowledge* in, untyped and unvalidated.
An adapter that needs the COM builds it from the value object. The crate has no
`serde_json` dependency at all.

## SettlementPeriod

A validated pair, not two loose dates. Every input struct used to carry
`period_from` and `period_to` independently and every calculation re-checked
their ordering — five copies of one guard, each able to be forgotten.
Constructing `SettlementPeriod` *is* the check, so an inverted period is
unrepresentable rather than rejected five times over.

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

- **NNE Strom** (PID 31001) — flat-rate Arbeit, Leistung (RLM), Konzessionsabgabe
- **NNE Gas** (PID 31005) — GasNEV §14 legal basis, auto-set when `Sparte::Gas`
- **§14a Modul 2 ToU** — mandatory HT/NT Arbeit split for controllable loads (BNetzA BK6-22-300)
- **Selbst ausgestellte NNE** (PID 31006) — LF runs the identical formula (INVOIC AHB Selbstausstellung)
- **MMM Strom** (PID 31002) — Mehr-/Mindermengensaldo, GPKE (BK6-24-174) Teil 1 Kap. 8.4
- **MMM Gas** (PID 31002) — Gas imbalance, GaBi Gas 2.1 (BK7-24-01-008)
- **MSB-Rechnung** (PID 31009) — Grundgebühr Messstellenbetrieb + optional Messdienstleistung
- **GeLi Gas AWH Sperrprozesse** (PID 31011) — abrechnungswürdige Handlungen (BK7-24-01-009 §5.4)
- **§13a EnWG Redispatch-Vergütung** — `redispatch_verguetung()` computes the angemessene Vergütung per activation (entgangene Einnahmen + zusätzliche − ersparte Aufwendungen; `eeg_entgangene_einnahmen()` for the Nr. 5 EEG basis)
- **Reversal (Stornorechnung)** — `calculate_reversal()` negates any prior settlement immutably

All calculations are **pure functions** — zero I/O, zero async, no side effects.
All monetary arithmetic uses `rust_decimal::Decimal` via `billing::EuroAmount` — no `f64` anywhere.

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
  malo_id, nb_mp_id, counterparty_mp_id,
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
                            }
        │
        ▼
InvoicCheckEngine::check(pid, &nb_mp_id, &rechnung, …)
        │
        ▼
invoice_drafts (PostgreSQL) → AS4 dispatch
```

### BDEW Artikelnummern architecture

The service layer owns the BDEW Artikelnummer mapping. `grid-billing` stays free
of `rubo4e`:

```mermaid
flowchart LR
    calc["grid_billing\nsettle_*()"]
    pos["SettlementPosition\n.kind: BillingPositionKind\n.trace: CalculationTrace"]
    svc["Service layer\nkind_to_artikelnummer()"]
    bo4e["Rechnungsposition\n.artikelnummer  ← Gas/MMM/KA\n.artikel_id     ← NNE Strom/AWH Gas"]

    calc --> pos --> svc --> bo4e

    note1["BK6-20-160:\nNNE Strom replaced\nartikelnummer → artikel_id\nfrom PreisblattNetznutzung"]
    note2["BDEW Codeliste v5.6:\nGas NNE/MMM/KA use\nclassic 9990001… codes\nAWH: 2-01-7-001/002"]

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
    pub period: SettlementPeriod,        // validated pair, both bounds inclusive
    pub regime: RegulatoryRegime,        // the rules this calculation applied
    pub sparte: Sparte,
    pub malo_id: String,
    pub nb_mp_id: String,                // sender (NB, or MSB for a metering settlement)
    pub counterparty_mp_id: String,      // recipient
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
in the service layer, and `artikel_id` carries the new-format article code where applicable
(e.g. AWH Gas: `"2-01-7-001"`, NNE Strom: populated from `PreisblattNetznutzung`):

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
    pub regulatory_reduction_factor: Option<Decimal>, // §14a Modul 1 factor (0–1)
    pub rounding_note: Option<&'static str>,
}
```

### `LegalReference`

```rust
pub enum LegalReference {
    StromNev { paragraph: &'static str },       // "§21" Arbeit, "§17" Leistung
    GasNev   { paragraph: &'static str },       // "§14"
    Kav      { paragraph: &'static str },       // "§2 Abs. 2"
    Sect14aEnwg { module: Sect14aModule },      // Modul1 | Modul2 | Modul3
    BnetzaDecision { reference: &'static str }, // "BK6-22-300"
    BdewAhb  { reference: &'static str },       // "GPKE BK6-22-024"
    MsbG     { paragraph: &'static str },       // "§§6–7"
    StromNzv { paragraph: &'static str },       // "§15" MMM
    GasNzv   { paragraph: &'static str },       // "§14" Gas MMM
    Enwg     { paragraph: &'static str },       // "§14a"
    ARegV    { paragraph: &'static str },       // "§17" incentive regulation
}
```

`.citation()` returns a short German-language string (e.g. `"StromNEV §17"`,
`"§14a EnWG Modul 2 (HT/NT variable)"`, `"ARegV §17"`).

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
    Strom,  // → StromNEV §21, SettlementType::NneStrom, PID 31001
    Gas,    // → GasNEV §14,   SettlementType::NneGas,   PID 31005
}
```

`Sparte` is required on `NneInput` and `MmmInput`. The calculation automatically
selects the correct legal references, `SettlementType`, and default PID — no
manual `r.pid = 31005` override needed for standard Gas paths.

### `SettlementType`

```rust
pub enum SettlementType {
    NneStrom,          // PID 31001 — NNE Strom (NB → LF)
    NneGas,            // PID 31005 — NNE Gas  (GNB → LFG)
    NneSelbstausstellt,// PID 31006 — NNE selbst ausgestellt (LF)
    MmmStrom,          // PID 31002 — MMM Strom, GPKE (BK6-24-174) Teil 1 Kap. 8.4
    MmmGas,            // PID 31002 — MMM Gas,   GaBi Gas 2.1 (BK7-24-01-008) (separate to ensure correct legal refs)
    MsbRechnung,       // PID 31009 — MSB-Rechnung (NB → MSB)
    GasAwhSperrung,    // PID 31011 — AWH Sperrprozesse Gas (GNB → LFG)
    RedispatchKostenblatt, // no standard PID — Redispatch 2.0
}
```

`SettlementType::default_pid()` returns the standard PID for the type.
`MmmGas` and `MmmStrom` share PID 31002 but carry different legal references.

### `BillingPositionKind` — BDEW Artikelnummern bridge

`BillingPositionKind` is the rubo4e-free type carried by every `SettlementPosition.kind`.
The service layer maps it to `rubo4e::current::BdewArtikelnummer` in `into_rechnung()`.

```rust
pub enum BillingPositionKind {
    NneArbeit,           // Wirkarbeit       (9990001 00026 9)
    NneArbeitHt,         // Wirkarbeit       (9990001 00026 9)
    NneArbeitNt,         // Wirkarbeit       (9990001 00026 9)
    NneArbeitModul1,     // Wirkarbeit       (9990001 00026 9) — reduced rate
    NneLeistung,         // Leistung         (9990001 00005 3)
    NneGasGrundpreis,    // Grundpreis       (9990001 00008 7)
    Konzessionsabgabe,   // Konzessionsabgabe(9990001 00041 7)
    Mehrmenge,           // Mehrmenge        (9990001 00074 8)
    Mindermenge,         // Mindermenge      (9990001 00075 6)
    MsbGrundgebuehr,     // EntgeltEinbauBetriebWartungMesstechnik (9990001 00061 5)
    Messdienstleistung,  // EntgeltMessungAblesung (9990001 00062 3)
    GasAwhSperrung,      // artikel_id: "2-01-7-001" (BK7-24-01-009 §5.4)
    GasAwhEntsprrung,    // artikel_id: "2-01-7-002"
    GasAwhSonstige,      // artikel_id from AwhPositionInput.artikel_id
    Blindmehrarbeit,     // Blindmehrarbeit  (9990001 00047 5)
}
```

> **NNE Strom (PIDs 31001/31006):** BK6-20-160 replaced classic `artikelnummer` codes
> with `artikel_id` from the BNetzA Netznutzungspreisblatt. The service layer
> (`netzbilanzd`, `invoicd`) populates `Rechnungsposition.artikel_id` from the tariff
> sheet for those positions; `kind_to_artikelnummer()` returns `None` for Strom NNE.
> Gas NNE, MMM, Konzessionsabgabe still use classic `articlenummer` codes.

Source: BDEW Codeliste Artikelnummern und Artikel-ID v5.6 (valid 01.09.2025).

### `KaKlasse` — KAV rate tier

```rust
pub enum KaKlasse {
    TarifkundeLow,    // ≤25 MWh/a residential — highest rate (KAV §2 Abs. 2)
    TarifkundeMedium, // ≤150 MWh/a commercial
    SonderkundeHigh,  // >150 MWh/a industrial
    Exempt,           // §2 Abs. 7 KAV exemptions
}
```

When `ka_klasse` is set, the KA position text and trace include the tier so
auditors can verify the rate matches the correct KAV §2 band without looking up
the underlying master data.

## Who uses this library

| Consumer | Role | Use case |
|---|---|---|
| `netzbilanzd` | **NB** | Generate INVOIC 31001/31002/31005/31009/31011 to LF/MSB/LFG |
| `invoicd` | **LF** | INVOIC AHB Selbstausstellung selbstausstellen PID 31006 — same formula, LF-initiated |

## Quick start

```toml
[dependencies]
grid-billing = { version = "0.13" }
rust_decimal = "1"
time         = "0.3"
```

### NNE flat-rate (SLP, Strom)

```rust,no_run
use grid_billing::{NneInput, Sparte, settle_nne};
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let settlement = settle_nne(&NneInput {
    malo_id: "51238696780".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    rechnungsnummer: "NNE-2026-01-0001".into(),
    period_from: date!(2026-01-01),
    period_to:   date!(2026-01-31),
    invoice_date: date!(2026-02-15),
    due_date:    date!(2026-03-17),
    arbeitsmenge_kwh: d("1500"),
    arbeitspreis_ct_per_kwh: d("3.5"),
    arbeitsmenge_ht_kwh: None,
    arbeitspreis_ht_ct_per_kwh: None,
    arbeitsmenge_nt_kwh: None,
    arbeitspreis_nt_ct_per_kwh: None,
    spitzenleistung_kw: None,
    leistungspreis_eur_per_kw: None,
    ka_satz_ct_per_kwh: Some(d("0.11")),
    tariff_sheet_id: Some("Preisblatt-NNE-2026-Q1".into()),
    sparte: Sparte::Strom,
    ka_klasse: None,
    sect14a_modul1_reduction_factor: None,  // §14a Modul 1 not active for this MaLo
    nne_grundpreis_eur_per_month: None,     // no Gas Grundpreis (Strom)
    nne_grundpreis_months: None,
}).expect("valid NNE input");

// settlement.total_eur = 52.50 + 1.65 = 54.15 EUR
assert_eq!(settlement.pid, 31001);
// counterparty_mp_id is auto-populated from lf_mp_id:
assert_eq!(settlement.counterparty_mp_id, "9900012345678");

// Every position is self-explanatory:
for pos in &settlement.positions {
    println!("{}: {}", pos.text, pos.trace.explanation);
    for lr in &pos.trace.legal_refs {
        println!("  → {}", lr.citation());
    }
}
```

### NNE Gas (GasNEV §14)

```rust,no_run
use grid_billing::{NneInput, Sparte, settle_nne};

// Only Sparte changes — GasNEV §14 legal refs and PID 31005 are automatic:
let settlement = settle_nne(&NneInput {
    sparte: Sparte::Gas,  // ← drives GasNEV §14 + PID 31005
    arbeitsmenge_kwh: d("3000"),  // already kWh_Hs from edmd gas conversion
    arbeitspreis_ct_per_kwh: d("1.80"),
    ka_satz_ct_per_kwh: None,  // KA typically not applicable for Gas
    sect14a_modul1_reduction_factor: None,
    nne_grundpreis_eur_per_month: None,
    nne_grundpreis_months: None,
    // … other identity fields …
}).unwrap();

assert_eq!(settlement.pid, 31005);
```

### §14a Modul 2 ToU (HT/NT split, mandatory since 2024-01-01)

```rust,no_run
use grid_billing::{NneInput, KaKlasse, Sparte, settle_nne};

let settlement = settle_nne(&NneInput {
    arbeitsmenge_kwh: d("1000"),          // total — ignored when HT/NT supplied
    arbeitspreis_ct_per_kwh: d("3.5"),    // fallback — ignored when HT/NT supplied
    arbeitsmenge_ht_kwh: Some(d("600")),
    arbeitspreis_ht_ct_per_kwh: Some(d("4.20")),
    arbeitsmenge_nt_kwh: Some(d("400")),
    arbeitspreis_nt_ct_per_kwh: Some(d("1.50")),
    ka_satz_ct_per_kwh: Some(d("1.32")),
    ka_klasse: Some(KaKlasse::TarifkundeLow),  // ← auditable tier annotation
    sparte: Sparte::Strom,
    tariff_sheet_id: Some("Preisblatt-14a-2026".into()),
    sect14a_modul1_reduction_factor: None,  // Modul 1 and Modul 2 are mutually exclusive
    nne_grundpreis_eur_per_month: None,
    nne_grundpreis_months: None,
    // … identity fields …
}).unwrap();

// HT: 600×4.20ct=25.20; NT: 400×1.50ct=6.00; KA: 1000×1.32ct=13.20 → total 44.40 EUR
assert_eq!(settlement.positions.len(), 3);  // HT + NT + KA
assert!(settlement.all_legal_refs().iter().any(|r| r.contains("§14a EnWG Modul 2")));
```

### §14a Modul 1 (flat percentage reduction, mandatory offer since 2024-01-01)

```rust,no_run
use grid_billing::{NneInput, Sparte, settle_nne};
use rust_decimal::dec;

// BK6-22-300 Anlage 2: default reduction factor = 0.85 (customer pays 85% of full rate).
// The NB may publish a different approved value in their PreisblattNetznutzung.
let settlement = settle_nne(&NneInput {
    arbeitsmenge_kwh: d("1500"),
    arbeitspreis_ct_per_kwh: d("3.5"),
    sect14a_modul1_reduction_factor: Some(dec!(0.85)),  // ← 15% reduction
    sparte: Sparte::Strom,
    // All HT/NT fields must be None — Modul 1 and Modul 2 are mutually exclusive
    arbeitsmenge_ht_kwh: None, arbeitspreis_ht_ct_per_kwh: None,
    arbeitsmenge_nt_kwh: None, arbeitspreis_nt_ct_per_kwh: None,
    // … other fields …
}).unwrap();

// 1500 × 0.035 × 0.85 = 44.625 → 44.62 EUR (MidpointNearestEven)
assert!(settlement.all_legal_refs().iter().any(|r| r.contains("Modul 1")));
assert!(settlement.positions[0].trace.regulatory_reduction_factor == Some(dec!(0.85)));
```

### Gas NNE with Grundpreis (GasNEV monthly standing charge)

```rust,no_run
let settlement = settle_nne(&NneInput {
    sparte: Sparte::Gas,
    arbeitsmenge_kwh: d("3000"),
    arbeitspreis_ct_per_kwh: d("1.80"),
    nne_grundpreis_eur_per_month: Some(d("15.00")),  // monthly base fee from PreisblattNetznutzung
    nne_grundpreis_months: Some(1),
    sect14a_modul1_reduction_factor: None,
    // … other fields …
}).unwrap();

// Positions: Grundpreis (15.00) + Arbeit (54.00) = 69.00 EUR
assert_eq!(settlement.positions.len(), 2);
assert!(settlement.positions[0].text.contains("Grundpreis"));
```

### GeLi Gas AWH Sperrprozesse (PID 31011)

```rust,no_run
use grid_billing::{GasAwhInput, AwhPositionInput, settle_gas_awh};

let settlement = settle_gas_awh(&GasAwhInput {
    malo_id: "51238696780".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    rechnungsnummer: "AWH-2026-01-0001".into(),
    period_from: date!(2026-01-01),
    period_to:   date!(2026-01-31),
    invoice_date: date!(2026-02-15),
    due_date:    date!(2026-03-17),
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

assert_eq!(settlement.pid, 31011);
assert_eq!(settlement.total_eur, d("90.00"));
// Both positions cite BK7-24-01-009 §5.4
assert!(settlement.all_legal_refs().iter().any(|r| r.contains("BK7-24-01-009")));
```

### Correction lifecycle (reversal + replacement pair)

```rust,no_run
use grid_billing::{settle_nne, calculate_correction, SettlementStatus};

let original = settle_nne(&nne_input).unwrap();
let corrected = settle_nne(&corrected_input).unwrap();

let (reversal, replacement) = calculate_correction(
    &original,
    corrected,
    "STORNO-NNE-2026-01-0001".to_owned(),
    date!(2026-03-01),
    date!(2026-03-31),
);

assert_eq!(reversal.status, SettlementStatus::Reversal);
assert_eq!(reversal.total_eur, -original.total_eur);
assert_eq!(replacement.status, SettlementStatus::Correction);
assert_eq!(replacement.correction_of.as_deref(), Some("NNE-2026-01-0001"));
```

```rust,no_run
use grid_billing::{settle_nne, calculate_reversal};
use time::macros::date;

let original = settle_nne(&/* … NneInput … */).unwrap();

let storno = calculate_reversal(
    &original,
    "STORNO-NNE-2026-01-0001".to_owned(),
    date!(2026-03-01),
    date!(2026-03-31),
);

assert_eq!(storno.total_eur, -original.total_eur);
assert_eq!(storno.correction_of.as_deref(), Some("NNE-2026-01-0001"));
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

fn into_rechnung(d: &InvoiceDocument) -> Rechnung {
    let lz = Zeitraum {
        startdatum: Some(s.period_from),
        enddatum: Some(s.period_to),
        ..Default::default()
    };
    let positions = s.positions.iter().map(|p| {
        let einheit = match p.unit {
            QuantityUnit::Kwh   => Some(Mengeneinheit::Kwh),
            QuantityUnit::Kw    => Some(Mengeneinheit::Kw),
            QuantityUnit::Kvarh => Some(Mengeneinheit::Kwh),   // map kVARh → kWh bucket
            QuantityUnit::Kvar  => Some(Mengeneinheit::Kw),    // map kVAR  → kW  bucket
            QuantityUnit::Monat => Some(Mengeneinheit::Monat),
        };
        Rechnungsposition {
            positionsnummer:    Some(p.number as i64),
            positionstext:      Some(p.text.clone()),
            // BDEW Artikelnummer from BillingPositionKind (Gas/MMM/KA positions)
            artikelnummer:      kind_to_artikelnummer(p.kind, s.settlement_type),
            // BDEW Artikel-ID (NNE Strom from tariff sheet; AWH Gas 2-01-7-xxx)
            artikel_id:         p.artikel_id.clone(),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(Menge { wert: Some(p.quantity), einheit, ..Default::default() }),
            einzelpreis:  Some(Preis  { wert: Some(p.unit_price_eur.round_dp(6)), ..Default::default() }),
            gesamtpreis:  Some(Betrag { wert: Some(p.net_eur.round_dp(5)), ..Default::default() }),
            ..Default::default()
        }
    }).collect();
    Rechnung {
        rechnungsnummer:   Some(s.rechnungsnummer.clone()),
        rechnungsdatum:    Some(s.invoice_date),
        faelligkeitsdatum: Some(s.due_date),
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
| 31001 | NNE Strom | NB → LF | Strom |
| 31002 | MMM Strom | NB → LF | Strom |
| 31002 | MMM Gas | GNB → LFG | Gas |
| 31005 | NNE Gas | GNB → LFG | Gas (auto via `Sparte::Gas`) |
| 31006 | Selbst ausgestellte NNE | LF | Strom |
| 31009 | MSB-Rechnung | NB → MSB | both |
| 31011 | AWH Sperrprozesse Gas | GNB → LFG | Gas |

## Billing position reference

### NNE

| # | Position text | Unit | `kind` | Condition | Legal basis | Artikelnummer |
|---|---|---|---|---|---|---|
| 1 | `Netznutzung Arbeit` | kWh | `NneArbeit` | flat / SLP | StromNEV §21 (Strom) · GasNEV §14 (Gas) | `Wirkarbeit` (Gas); `artikel_id` (Strom) |
| 1 | `Netznutzung Arbeit §14a Modul 1 (85% Reduzierung)` | kWh | `NneArbeitModul1` | `sect14a_modul1_reduction_factor` set | §14a EnWG Modul 1 · BK6-22-300 | same as NneArbeit |
| 1+2 | `Netznutzung Arbeit HT (§14a Modul 2)` + NT | kWh | `NneArbeitHt` / `NneArbeitNt` | HT + NT both set | §14a EnWG Modul 2 · BK6-22-300 | same as NneArbeit |
| opt | `Netzentgelt Grundpreis Gas` | Monat | `NneGasGrundpreis` | `nne_grundpreis_eur_per_month` set | GasNEV §14 | `Grundpreis` |
| next | `Netznutzung Leistung` | kW | `NneLeistung` | `spitzenleistung_kw` set (RLM) | StromNEV §17 | `Leistung` (Gas); `artikel_id` (Strom) |
| last | `Konzessionsabgabe[tier]` | kWh | `Konzessionsabgabe` | `ka_satz_ct_per_kwh` set | KAV §2 Abs. 2 | `Konzessionsabgabe` |

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
| **No floating-point money** | `rust_decimal::Decimal` throughout; `billing::EuroAmount` for overflow guard. No `f64`. |
| **No rubo4e dependency** | Returns `SettlementResult`; service layer owns `into_rechnung()`. |
| **`counterparty_mp_id` auto-populated** | `lf_mp_id` (NNE/MMM) or `msb_mp_id` (PID 31009) copied automatically. |
| **`Sparte` drives settlement type** | `Sparte::Gas` → `SettlementType::NneGas`, `GasNEV §14`, PID 31005. No manual override needed. |
| **Every position cites regulation** | `trace.legal_refs` is non-empty for every position. Enables BNetzA audit without re-calculation. |
| **Artikelnummer on every position** | `BillingPositionKind::artikelnummer()` in this crate. Never empty. |
| **`MmmGas` ≠ `MmmStrom`** | Separate `SettlementType` variants ensure correct legal refs (`GaBi Gas 2.1 (BK7-24-01-008)` vs `GPKE (BK6-24-174) Teil 1 Kap. 8.4`) per position. |
| **Immutable correction chain** | `calculate_reversal()` mirrors positions, sets `status = Reversal`, links via `correction_of`. Original never mutated. |
| **`calculate_correction()` pair** | Returns `(reversal, replacement)` — both get status set atomically; caller dispatches both. |
| **Pure functions** | All `calculate_*` functions are sync with no side effects. |
| **`recomputed_total` guard** | `debug_assert_eq!(result.total_eur, result.recomputed_total())` inside every `calculate_*` — catches rounding bugs in debug builds. |

## See also

- [`invoic-checker`](../invoic-checker/README.md) — validates the generated `Rechnung` in the service layer
- [`netzbilanzd`](../../services/netzbilanzd/README.md) — NB billing service that calls `grid-billing`
- [`invoicd`](../../services/invoicd/README.md) — LF service using `grid-billing` for selbstausstellen
- [Operator guide → netzbilanzd](../../docs/netzbilanzd.md)

