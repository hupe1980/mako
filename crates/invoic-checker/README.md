# invoic-checker

**Pure INVOIC plausibility and tariff validation library for German energy market suppliers.**

`invoic-checker` implements the eight-stage pipeline that `invoicd` runs automatically
against every incoming INVOIC — and that `netzbilanzd` runs before dispatching to
prevent an immediate dispute.

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No async** | Synchronous throughout. |
| **No float money** | All monetary comparisons use `rust_decimal`. |
| **Pure functions** | `InvoicCheckEngine::check()` cannot fail — it always returns a `CheckReport`. |

---

## Why this is a separate crate from `mako-pruefung`

The two answer different questions for different audiences:

| | `mako-pruefung` | `invoic-checker` |
|---|---|---|
| Produces | published BDEW Antwortcodes for the wire | mako's own `Finding`s for the operator queue and the § 147 AO receipt |
| Runs for | Prüfidentifikatoren (PIDs) with an Entscheidungsbaum | **every** INVOIC PID, including those with none |
| Knows about | nothing but Prüfschritte | BO4E, money, Preisblätter |
| Dependencies | `mako-fristen`, `serde`, `time`, `uuid` | + `rubo4e`, `billing`, `rust_decimal` |
| Consumers | every crate that answers a BDEW process | `invoicd`, `netzbilanzd` |

Merging them would put `rubo4e` and `billing` behind `mako-wim`, `mako-gpke`,
`mako-mabis`, `processd` and `einsd`, none of which touches an invoice — and
hand `netzbilanzd`, which uses only `CheckOutcome`, every BDEW tree in the
catalogue to get one enum.

The dependency runs **one way**: this crate maps BO4E onto
`mako_pruefung::rechnung`'s Prüfschritte and calls the walk. That module holds
no BO4E type and no money type, which is what keeps it linkable from a
role-gated build.

**Where the two overlap, they must agree.** Several checks below ask the same
question as a Prüfschritt — position arithmetic against `A20`, the document
total against `A24`, the tax breakdown against `A22`/`A23`. Both paths therefore
read the same tolerance: Summen-level checks `total_tolerance_ppm`,
position-level ones `arithmetic_tolerance_ppm`. Two knobs for one question would
let the engine record a `TotalMismatch` Dispute while the walk dispatched a
Zahlungsavis.

## The checks

The pipeline is eight stages, in this order:

| # | Rule | Outcome on failure |
|---|---|---|
| 1 | **Storno reference** — `ist_storno = true` must name an `original_rechnungsnummer` | `Dispute` |
| 2 | **Period validity** — `rechnungsperiode_start < end`, both within plausible range | `Dispute` |
| 3 | **Zahlungsziel** — `faelligkeitsdatum < rechnungsdatum` (invalid) or beyond `max_zahlungsziel_days` (exceeded; default 30 per §7 Allg. Festlegungen) | `Dispute` · `Warn` |
| 4 | **Currency agreement** — the document totals, every position's `gesamtpreis` and every `steuerbetraege` entry agree on one currency. Runs *before* the arithmetic, because every amount is read as EUR and a `CHF` field would otherwise compare silently right | `Dispute` |
| 5 | **Position arithmetic** — every `Rechnungsposition` satisfies `menge × einzelpreis ≈ gesamtpreis`. An unrepresentable product is itself a finding, not a panic | `Dispute` |
| 6 | **Document total** — the positions sum to `gesamtnetto` | `Warn` |
| 7 | **Umsatzsteuer** — the invoice states a rate and an amount (§14 Abs. 4 Nr. 8 UStG); `gesamtbrutto = gesamtnetto + gesamtsteuer`; each breakdown entry's `steuerwert` is the stated rate applied to its stated `basiswert` (to within one cent, the unit the amounts are stated in); and a reverse-charged invoice states **no** tax | `Dispute` |
| 8 | **Tariff / Angebot** — `einzelpreis` against the published Preisblatt, or against the accepted QUOTES for an ESA invoice. **Skipped for a Stornorechnung**, which carries the original's negated amounts rather than new tariff positions. For PIDs 31005–31008 this is the Mehr-/Mindermengen price from the MMMA store | `Warn` or `Dispute` |

Stage 7's per-entry check is what stops an invoice that sums to its own totals
perfectly while charging the wrong tax: 19 % stated on a base of 10 000 with a
Steuerwert of 100 satisfies `netto + steuer = brutto` and agrees with
`gesamtsteuer`, and 1 800 EUR is then neither charged nor deductible.

### Why a missing tax block is a dispute

§14 Abs. 4 Nr. 8 UStG makes the rate and the tax amount mandatory content, or a note saying why
neither is stated. Paying an invoice without them means paying tax that cannot be recovered —
the receiving LF's money — so this is a refusal rather than a note. A reverse-charged invoice
states no tax *by design* and is accepted on that footing; one that states tax anyway is a
dispute, because that tax is owed under §14c Abs. 1 UStG and is still not deductible.

### The market branch — published Antwortcodes, three families

The pipeline above answers „is this invoice plausible" in mako's own vocabulary
([`Finding`]), which is what an operator queue and a § 147 AO receipt need. It
is *not* what the market resolves: the answer owed is a REMADV carrying
**published Antwortcodes**, one per defect, each naming the Ebene and — on the
Positionsebene — the Positionsnummer.

The `rechnung` module is that bridge. `antwort_auf_rechnung` maps a BO4E
`Rechnung` plus the recipient's own facts onto the tree's Prüfschritte and
returns a `RechnungsAntwort`, which also knows which REMADV Prüfidentifikator it
must ride (33001 Zahlungsavis, 33003 Kopf/Summe, 33004 Position).
`antwort_auf_erneute_rechnung` runs the second round, after the MSB answered a
Nicht-Zahlungsavis with a COMDIS 29001.

**One walk, three families.** The BDEW publishes the same Rechnungsprüfung under
twelve EBD numbers, because a code is resolved against the tree the answer
names:

| Familie | Rechnung | erneut | Nicht-Zahlungsavis | Storno |
|---|---|---|---|---|
| `ESA` — WiM Teil 2 Kap. 4.5 | `E_0264` | `E_0266` | `E_0265` | `E_0267` |
| `PREISBLATT_B_LF` — AWH Kap. 9.3 | `E_0270` | `E_0276` | `E_0271` | `E_0272` |
| `PREISBLATT_B_NB` — AWH Kap. 9.4 | `E_0273` | `E_0277` | `E_0274` | `E_0275` |

Every entry point takes the `RechnungsFamilie`, and the walk itself lives in
`mako_pruefung::rechnung`. Three things differ and nothing else does: the second
round's Prüfschritt-1 code (**`A25`** for the ESA, **`AC1`** for Preisblatt B),
two Kopf-Prüfschritte that only the Preisblatt-B trees publish (80 „kein
gültiges Preisblatt", 90 „Abrechnungszeitraum doppelt abgerechnet"), and the
Prüfschritt number `A90` therefore sits at (90 against 100).

That first difference is the reason this is one parameterised walk rather than
three copies: **`A25` is the ESA's second-round refusal and the Preisblatt-B
doppelter Abrechnungszeitraum** — one spelling, two meanings, in trees that
answer on the same REMADV Prüfidentifikatoren.

**The ESA's price basis is the offer, not a Preisblatt.** § 35 MsbG leaves the
Entgelt for a Zusatzleistung to be agreed per request, so there is no published
sheet for a Kapitel-4.6 Messprodukt; the QUOTES 15003 the ESA ordered against is
the agreement, and the join is exact — the offer prices Artikel-IDs and the
invoice names the same ones back. An **empty** offer list means mako holds no
record of one, and Prüfschritte 300 / 320 / 500 then answer „not comparable"
rather than „wrong": disputing on a gap in mako's own books refuses a correct
invoice. A Preisblatt-B invoice does have a sheet — PRICAT 27002, called
„Preisblatt Technik" — which is what Prüfschritt 80 asks about.

Prüfschritte that need facts no INVOIC carries live on `EmpfaengerFakten`, and
an unknown answer never refuses. Two are **not** among them: Prüfschritt 40
(`SG1 RFF+ACE` names the order, so the invoice either matches one on record or
bills against none) and Prüfschritt 50 (the Rechnungsnummer is einmalig per
Rechnungssteller under § 14 Abs. 4 Nr. 4 UStG, and `invoicd` answers it from its
own receipt store).

### Stornierung handling (`ist_storno = true`)

When `ist_storno = Some(true)`, stage 8 (tariff check) is automatically skipped.
A Stornierung carries negated amounts from the original invoice, not new tariff positions —
checking them against PRICAT would always produce false `TariffDeviation` disputes.
`check_storno` runs stages 1–6; stage 7 (Umsatzsteuer) is skipped with it.

Stage 1 enforces that `original_rechnungsnummer` is present on every Storno.
Use `is_stornierung(&rechnung)` to test the flag before routing to `check_storno()`.

```rust
use invoic_checker::{InvoicCheckEngine, is_stornierung, CheckConfig};
use rubo4e::current::Rechnung;

let rechnung: Rechnung = /* ... */;
if is_stornierung(&rechnung) {
    // Arithmetic-only path — no tariff check.
    let report = InvoicCheckEngine::check_storno(pid, &rechnung, &CheckConfig::default());
} else {
    let report = InvoicCheckEngine::check(pid, sender_mp_id, &rechnung, &store, &config);
}
```

### Stage 3 — Zahlungsziel

`faelligkeitsdatum` (DTM+265) is validated against `rechnungsdatum` and the
configured `max_zahlungsziel_days` (default: 30, per §7 Allgemeine Festlegungen V6.1d).
Set `max_zahlungsziel_days = 0` in `CheckConfig` to disable this check.

### Stage 8 — ToU-aware tariff matching

For a time-of-use Preisblatt (§ 14a Modul 3, BK6-22-300), the bands come from
the `zeitvariablePreispositionen` extension, each entry pairing a
`zaehlzeitregister` code with its price. There is **no** hard-coded HT/NT
keyword list: the position's own `positionstext` is lower-cased and matched
against the register codes the Preisblatt actually publishes, so an operator who
names a band `ST` or `NT2` needs no code change.

The band search then degrades in a fixed order — a matching register code wins;
failing that the flat `preisstaffeln` prices apply; failing those, every ToU band
price is accepted. Each fallback only widens what passes, so a missing band
produces no invented deviation — but it does mean a `TariffDeviation` finding is
evidence and a clean result is not proof.

### Stage 8 — MMM settlement price

`InvoicCheckEngine::check_mmm_settlement()` fetches the monthly Mehr-/Mindermengenpreis
(Gas or Strom) from `marktd`'s MMMA store and compares it against the invoice's
`mehr_preis` / `minder_preis` fields.

### The MSB path — PIDs 31003 and 31009

For a WiM/MSB-Rechnung, use `check_msb_rechnung()`. It runs the document stages
2–7 exactly as `check()` does — period, currency, position arithmetic, document
total, **Zahlungsziel and Umsatzsteuer** — and reads the tariff for stage 8 from
`PreisblattMessung.preispositionen` rather than `PreisblattNetznutzung`. With no
Preisblatt it warns instead of disputing, as the standard engine does.
`check_msb_rechnung_with_aufabschlaege()` adds one further check: every discount
or surcharge position is backed by a contracted `AufAbschlag` (WiM PRICAT
27001–27003).

Stages 3 and 7 used to be skipped here, so an MSB invoice stating no
Umsatzsteuer at all was accepted. Nothing about metering service warrants that:
the INVOIC AHB makes the Fälligkeitsdatum (`SG8 DTM+265`) and the tax block
(`TAX`/`MOA`) **Muss** on 31003 and 31009 just as on 31001/31002, § 14 Abs. 4
Nr. 8 UStG reaches every invoice, and `check_esa_rechnung()` already ran both
for the same PID 31009.

---

## Supported PIDs

| PID | Process | Billing direction |
|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung | NB → LF |
| 31002 | NN-Rechnung (Netznutzung Strom + Gas) | NB → LF |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo) | NB → LF |
| 31006 | MMM Mehrmenge, selbst ausgestellt | LF → LF |
| 31007 | Aggreg. MMM-Rechnung Gas | NB → MGV |
| 31008 | Selbst ausgest. Aggreg. MMM-Rechnung Gas | MGV → MGV |
| 31009 | MSB-Rechnung | MSB → LF |
| 31003 | WiM-Rechnung (Dienstleistungen im Messwesen) | NB ↔ MSBN, beide Sparten |
| 31004 | Stornorechnung — Sparte-neutral, routed to `check_storno` | as the original |
| 31010 | Kapazitätsrechnung Gas | FNB/VNB → BKV |
| 31011 | AWH Sperrprozesse Gas | GNB → LFG |

---

## `FindingKind` variants

| Variant | Stage | Dispute? | Meaning |
|---|---|---|---|
| `StorniertWithoutReference` | 1 | ✓ | `ist_storno=true` but `original_rechnungsnummer` missing |
| `PeriodInvalid` | 2 | ✓ | Billing period start ≥ end |
| `ZahlungszielInvalid` | 3 | ✓ | `faelligkeitsdatum` before `rechnungsdatum` |
| `ZahlungszielExceeded` | 3 | ✗ | Payment term exceeds `max_zahlungsziel_days` |
| `WaehrungMismatch` | 4 | ✓ | The document, a position or a tax entry names a currency the others do not |
| `ArithmeticError` | 5 | ✓ | Line `menge × einzelpreis ≠ gesamtpreis` |
| `TotalMismatch` | 6 | ✗ | Σ line nets ≠ `gesamtnetto` |
| `SteuerMissing` | 7 | ✓ | No Umsatzsteuer stated at all — no Vorsteuerabzug for the recipient |
| `SteuerMismatch` | 7 | ✓ | `gesamtbrutto ≠ gesamtnetto + gesamtsteuer`, or a breakdown entry's `steuerwert` is not its rate on its `basiswert` |
| `ReverseChargeStatesTax` | 7 | ✓ | A §13b invoice states tax anyway — owed under §14c Abs. 1 and still not deductible |
| `TariffDeviation` | 8 | ✓ | Unit price deviates from the Preisblatt |
| `TariffNotFound` | 8 | config | No Preisblatt for the sender |
| `AngebotDeviation` | 8 | ✓ | An ESA invoice prices an Artikel-ID differently from the accepted QUOTES |
| `AngebotPositionUnknown` | 8 | ✓ | An ESA invoice names an Artikel-ID the accepted QUOTES does not offer |

## Regulatory basis

- **BK6-24-174** — INVOIC AHB Strom (NNE/MSB-Rechnung, PIDs 31001/31002/31005/31006/31009)
- **BK7-24-01-009** — INVOIC AHB Gas (GeLi Gas PID 31011)
- **BK7-24-01-008** — INVOIC AHB Gas (GaBi Gas PIDs 31007/31008)
- **BK7 billing** — WiM Gas PIDs 31003/31004
- **§7 Allgemeine Festlegungen V6.1d** — Zahlungsziel 30 days (Strom + Gas)
- **§ 147 AO / GoBD** — Pflicht zur Rechnungslegung (MSB-Rechnung receipt persistence)
- **REMADV AHB 1.0a** — § 3.1.1 (33001/33002) and § 3.1.2 (33003/33004); `SG7 AJT` DE 1082 admits a different list of Entscheidungsbäume on each
- **Entscheidungsbaum-Diagramme und Codelisten 4.3** Kap. 8.27 — `E_0264`–`E_0267`, the ESA billing round trip; Kap. 9.3/9.4 — `E_0270`–`E_0277`, the Preisblatt-B one
- **BDEW AWH Prozesse zur Änderung der Technik an Lokationen** V1.1 (31.03.2025)

## Related crates

| Crate | Role |
|---|---|
| [`invoic-checker`](https://docs.rs/invoic-checker) ← **this crate** | The eight-stage plausibility and tariff pipeline — mako's own `Finding`s |
| [`mako-pruefung`](https://docs.rs/mako-pruefung) | The published BDEW Antwortcodes a REMADV must carry |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-invoic`](https://docs.rs/mako-invoic) | The settle/dispute workflow whose decision these findings feed |
| [`grid-billing`](https://docs.rs/grid-billing) | Produces the grid-side invoices this crate checks on the receiving end |
| [`invoicd`](https://hupe1980.github.io/mako/docs/services/invoicd/) · [`netzbilanzd`](https://hupe1980.github.io/mako/docs/services/netzbilanzd/) | Production daemons — run the pipeline on receipt and before dispatch |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
