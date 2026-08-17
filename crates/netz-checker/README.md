# netz-checker

**Pure, deterministic Anmeldung validation library for German energy market NB STP decisions.**

`netz-checker` implements the six objective checks that a Netzbetreiber (NB) must
perform when receiving a Lieferbeginn (Anmeldung) request from a Lieferant (LF).
The result drives automatic `bestaetigen` or `ablehnen` dispatch in `processd`.

---

## Design constraints

| Constraint | Detail |
|-----------|--------|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No clock** | `now: OffsetDateTime` is injected by the caller for testability. |
| **Deterministic** | Same inputs always produce the same output. |
| **No async** | Synchronous throughout — wraps cheaply in `tokio::task::spawn_blocking` if needed. |
| **Pure functions** | `evaluate()` cannot fail — it always returns `NetzCheckResult`. |

---

## The six checks

| # | Rule | Reject code | Escalate? |
|---|------|-------------|-----------|
| 1 | Grid record present (`MaloGridRecord` is `Some`) | — | ✓ missing data |
| 2 | MaLo participates in market communication (not Stillgelegt/Ruhend) | A02 | |
| 3 | No conflicting Anmeldung in Bearbeitung (`lf_mp_id_next` is `None`) | A06 | |
| 4 | Date plausibility, Transaktionsgrund-aware — Strom: LFW24 future rule (one full Werktag between receipt and Zuordnungsbeginn; retroactivity abolished for **all** Transaktionsgründe); Gas: E03 Wechsel ≥ 10 WT future-only, E01/E02 retroactive up to 6 weeks (+3 WT) for SLP metering | A07 (Strom) / E17 (Gas) | ✓ Gas backdated without Transaktionsgrund |
| 5 | Bilanzierungsgebiet matches grid record (when both present) | A05 | ✓ grid record incomplete |
| 6 | LF GLN in partner directory (`partner_known = true`) | A05 | |

Checks run in order; the first failure short-circuits and returns the result immediately.

---

## Usage

```rust
use netz_checker::{AnmeldungAnfrage, MaloGridRecord, evaluate};
use netz_checker::types::NetzCheckResult;
use mako_markt::domain::Sparte;
use mako_markt::repository::VersorgungsStatusRecord;

let anfrage = AnmeldungAnfrage {
    pid: 55001,
    process_id: uuid::Uuid::new_v4(),
    malo_id: "51238696012".to_owned(),
    new_supplier_gln: "9900357000004".to_owned(),
    grid_operator_gln: "9900000000002".to_owned(),
    bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
    process_date: time::Date::from_calendar_date(2026, time::Month::August, 1).unwrap(),
    sparte: Sparte::Strom,
    messtyp: netz_checker::Messtyp::Slp,
};

let grid = MaloGridRecord {
    malo_id: "51238696012".to_owned(),
    nb_mp_id: "9900000000002".to_owned(),
    bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
    netzgebiet: None,
    sparte: Sparte::Strom,
    source: "mastr".to_owned(),
    updated_at: time::OffsetDateTime::now_utc(),
    tenant: "9900000000002".to_owned(),
};

// vs: Option<&VersorgungsStatusRecord> — None if MaLo not yet in marktd
// partner_known: true if GET /api/v1/partners/{lf_gln} returned 200

// config: NetzCheckConfig — holiday calendar, Gas Bearbeitungsfrist, EEG lead.
// Use NetzCheckConfig::default() for the regulatory defaults.
let result = evaluate(
    &anfrage,
    None,
    Some(&grid),
    true,
    time::OffsetDateTime::now_utc(),
    &netz_checker::NetzCheckConfig::default(),
);

match result {
    NetzCheckResult::Accept => { /* dispatch bestaetigen */ }
    NetzCheckResult::Reject(r) => { /* dispatch ablehnen with r.erc_code */ }
    NetzCheckResult::Escalate { reason } => { /* alert operator */ }
}
```

---

## ERC codes

| Code | Meaning (EBD E_0622 / G_0011) | Check |
|------|-------------------------------|-------|
| `A02` | Marktlokation nimmt nicht an der Marktkommunikation teil (Stillgelegt/Ruhend) | 2 |
| `A06` | Andere Anmeldung in Bearbeitung / duplicate Anmeldung | 3 |
| `A07` | Vorlauffrist wurde nicht eingehalten (Strom LFW24 date rule) | 4 |
| `E17` | Ablehnung wg. Fristüberschreitung (Gas date rules) | 4 |
| `A05` | Anforderungen können nicht erfüllt werden (Bilanzierungsgebiet / unknown Marktpartner) | 5, 6 |

Source: EBD 4.2 E_0622 (GPKE, BK6-24-174) + AWH GeLi Gas 2.0 V1.2 Kap. 2.2 +
EBD 4.2 Kap. 13.6 codeliste G_0011. Note: `A97` is **not** a date code (it was
the pre-LFW24 AHB-Prüfung result code, deleted in EBD 4.x); `A99` „Sonstiges"
ends 01.10.2026 — neither is used.

### Gas retroactive window (AWH GeLi Gas 2.0 Kap. 2.2)

Retroactive An-/Abmeldungen are permitted for non-Wechsel Transaktionsgründe
(E01 Ein-/Auszug, E02 Einzug in Neuanlage) on SLP-metered MaLos, up to
**6 weeks + 3 WT Bearbeitungsfrist** before receipt. RLM / SMGW-attached
metering is future-only; Wechsel (E03) requires ≥ 10 WT lead. The
Bearbeitungsfrist default (3 WT) follows the E/G rule — the AWH does not
quantify it for An-/Abmeldungen (documented ambiguity) — and is **configurable**
via `NetzCheckConfig::gas_bearbeitungsfrist_wt`.

### EEG-/KWKG-MaLo Zuordnung (§10c EEG)

When the Transaktionsgrund is one of `A27`–`A29`/`A31`/`A32`, Check 4 switches to
the EEG date rule: the Zuordnungsbeginn must be a **Monatserster** and lie at
least one whole month ahead (configurable via
`NetzCheckConfig::eeg_zuordnung_vorlauf_monate`). Violations reject with **A07**.

### Werktag arithmetic

All Werktag math uses the BDEW-MaKo holiday calendar
(`mako-engine::fristen`, selected via `NetzCheckConfig::holiday_calendar`) — not
a bare Mon–Fri approximation. Public holidays observed in any German Bundesland
count as non-Werktage.

---

## Supported PIDs

| PID | Process | Sparte |
|-----|---------|--------|
| 55001 | GPKE Lieferbeginn Standard | Strom |
| 55016 | GPKE Lieferbeginn Netzentnahme | Strom |
| 44001 | GeLi Gas Lieferbeginn | Gas |

---

## Regulatory basis

- **GPKE:** BK6-22-024 §5 + UTILMD Strom AHB
- **GeLi Gas:** BK7-24-01-009 §3 + UTILMD Gas AHB
- **ERC codes:** APERAK AHB 1.0 §2 decision trees
- **Deadline arithmetic:** German local time (CET/CEST) via `time-tz`
