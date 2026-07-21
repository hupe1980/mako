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

## The checks

| # | Rule | Outcome on failure |
|---|---|---|
| 0 | **Storno reference** — `ist_storno=true` must have `original_rechnungsnummer` | `Dispute` |
| 1 | **Period validity** — `rechnungsperiode_start < end`, both within plausible range | `Dispute` |
| 1.5 | **Zahlungsziel** — `faelligkeitsdatum < rechnungsdatum` (invalid) or `> max_zahlungsziel_days` (exceeded; default 30 per §7 Allg. Festlegungen) | `Dispute` or `Warn` |
| 2 | **Position arithmetic** — every `Rechnungsposition` `menge × preis ≈ betrag` (±1%) | `Dispute` |
| 3 | **Document total** — sum of all positions ≈ `gesamtbrutto` (±1%) | `Warn` |
| 4 | **Tariff match** — `einzelpreis` within tolerance of PRICAT tariff. **Skipped for Stornorechnungen** (`ist_storno=true`). | `Dispute` |
| 5 | **Tariff found** — a PRICAT tariff record exists for the sender GLN | `Warn` (auto-accept) |
| 6 | **MMM settlement price** — for PIDs 31002/31005/31007/31008: Mehr-/Mindermengen prices match MMMA store | `Warn` or `Dispute` |

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

## Usage

```rust
use invoic_checker::{InvoicCheckEngine, InvoicCheckInput, CheckResult};

let engine = InvoicCheckEngine::new(tariff_repo, mmma_store);

let input = InvoicCheckInput {
    pid: 31001,
    rechnung_json: serde_json::from_str(raw_invoic)?,
    malo_id: "51238696780".to_owned(),
    ..Default::default()
};

match engine.check(&input).await? {
    CheckResult::Accept(summary) => {
        // dispatch REMADV 33001 (Zahlungsavis)
    }
    CheckResult::Dispute { check, reason } => {
        // dispatch REMADV 33002 — include check number and reason
    }
    CheckResult::Warn(summary) => {
        // log and auto-accept
    }
}
```

---

## Supported PIDs

| PID | Process | Billing direction |
|---|---|---|
| 31001 | NNE-Rechnung Strom | NB → LF |
| 31002 | MMM-Rechnung Strom | NB → LF |
| 31005 | Selbst ausgest. NNE-Rechnung | LF → LF |
| 31006 | Selbst ausgest. Rechnung (§ 147 AO / GoBD) | LF → LF |
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
- **REMADV AHB 1.0** — ERC-code mapping for dispute messages
