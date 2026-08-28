# invoic-checker

**Pure INVOIC plausibility and tariff validation library for German energy market suppliers.**

`invoic-checker` implements the six-check pipeline that `invoicd` runs automatically
against every incoming INVOIC — and that `netzbilanzd` runs before dispatching to
prevent an immediate dispute.

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No async** | Synchronous throughout. |
| **No float money** | All monetary comparisons use `rust_decimal`. |
| **Pure functions** | `InvoicCheckEngine::check()` cannot fail — it always returns `CheckResult`. |

---

## Why this is a separate crate from `mako-pruefung`

The two answer different questions for different audiences:

| | `mako-pruefung` | `invoic-checker` |
|---|---|---|
| Produces | published BDEW Antwortcodes for the wire | mako's own `Finding`s for the operator queue and the § 147 AO receipt |
| Runs for | PIDs with an Entscheidungsbaum | **every** INVOIC PID, including those with none |
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

| # | Rule | Outcome on failure |
|---|---|---|
| 0 | **Storno reference** — `ist_storno=true` must have `original_rechnungsnummer` | `Dispute` |
| 1 | **Period validity** — `rechnungsperiode_start < end`, both within plausible range | `Dispute` |
| 1.5 | **Zahlungsziel** — `faelligkeitsdatum < rechnungsdatum` (invalid) or `> max_zahlungsziel_days` (exceeded; default 30 per §7 Allg. Festlegungen) | `Dispute` or `Warn` |
| 2 | **Position arithmetic** — every `Rechnungsposition` `menge × preis ≈ betrag` (±1%) | `Dispute` |
| 3 | **Document total** — sum of all positions ≈ `gesamtnetto` (±1%) | `Warn` |
| 3.5 | **Umsatzsteuer** — the invoice states a rate and an amount (§14 Abs. 4 Nr. 8 UStG), `gesamtbrutto = gesamtnetto + gesamtsteuer`, and a reverse-charged invoice states **no** tax | `Dispute` |
| 4 | **Tariff match** — `einzelpreis` within tolerance of PRICAT tariff. **Skipped for Stornorechnungen** (`ist_storno=true`). | `Dispute` |
| 5 | **Tariff found** — a PRICAT tariff record exists for the sender GLN | `Warn` (auto-accept) |
| 6 | **MMM settlement price** — for PIDs 31005/31006/31007/31008: Mehr-/Mindermengen prices match MMMA store | `Warn` or `Dispute` |

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

When `ist_storno = Some(true)`, stage 4 (tariff check) is automatically skipped.
A Stornierung carries negated amounts from the original invoice, not new tariff positions —
checking them against PRICAT would always produce false `TariffDeviation` disputes.

Stage 0 enforces that `original_rechnungsnummer` is present on every Storno.
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

### Check 1.5 — Zahlungsziel

`faelligkeitsdatum` (DTM+92) is validated against `rechnungsdatum` and the
configured `max_zahlungsziel_days` (default: 30, per §7 Allgemeine Festlegungen V6.1d).
Set `max_zahlungsziel_days = 0` in `CheckConfig` to disable this check.

### Check 4 — ToU-aware tariff matching

For time-of-use tariffs, the position text (`positionsbezeichnung`) is used to
classify HT/NT positions against the corresponding `zeitvariablePreisposition`
band price. Positions containing `"HT"`, `"Hochtarif"`, or `"Haupttarif"` are
matched against the HT band; `"NT"`, `"Niedertarif"`, `"Nebentarif"` against NT.

### Check 6 — MMM settlement price

`InvoicCheckEngine::check_mmm_settlement()` fetches the monthly Mehr-/Mindermengenpreis
(Gas or Strom) from `marktd`'s MMMA store and compares it against the invoice's
`mehr_preis` / `minder_preis` fields.

For PID 31009 (MSB-Rechnung), use `check_msb_rechnung()` which applies
`PreisblattMessung` pricing (not NNE) for checks 4 and 5.

---

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

---

## `FindingKind` variants

| Variant | Stage | Dispute? | Meaning |
|---|---|---|---|
| `StorniertWithoutReference` | 0 | ✓ | `ist_storno=true` but `original_rechnungsnummer` missing |
| `PeriodInvalid` | 1 | ✓ | Billing period start ≥ end |
| `ZahlungszielInvalid` | 1.5 | ✓ | `faelligkeitsdatum` before `rechnungsdatum` |
| `ZahlungszielExceeded` | 1.5 | ✗ | Payment term exceeds `max_zahlungsziel_days` |
| `ArithmeticError` | 2 | ✓ | Line `qty × price ≠ net` |
| `TotalMismatch` | 3 | ✗ | Σ line nets ≠ `gesamtnetto` |
| `SteuerMissing` | 3.5 | ✓ | No Umsatzsteuer stated at all — no Vorsteuerabzug for the recipient |
| `SteuerMismatch` | 3.5 | ✓ | `gesamtbrutto ≠ gesamtnetto + gesamtsteuer` |
| `ReverseChargeStatesTax` | 3.5 | ✓ | A §13b invoice states tax anyway — owed under §14c Abs. 1 and still not deductible |
| `TariffDeviation` | 4 | ✓ | Unit price deviates from PRICAT |
| `TariffNotFound` | 5 | config | No PRICAT tariff for sender GLN |

## ERC codes

| Code | Meaning |
|---|---|
| `Z30` | Rechnungsposition arithmetic error |
| `Z31` | Document total mismatch |
| `Z32` | Tariff price deviation above tolerance |
| `Z33` | Tariff not found |
| `Z34` | Invalid billing period |
| `Z35` | MMM settlement price mismatch |
| `Z36` | Stornorechnung missing original reference |
| `Z37` | Zahlungsziel exceeds maximum payment term |

---

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
