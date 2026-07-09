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
| 2 | No conflicting active supply (`lf_gln_next` is `None`) | A06 | |
| 3 | `process_date ≥ today_berlin(now)` | A97 | |
| 4 | Bilanzierungsgebiet matches grid record (when both present) | A02 | |
| 5 | LF GLN in partner directory (`partner_known = true`) | A05 | |
| 6 | Mindestvorlauffrist met (SLP: > today; RLM: ≥ 2 Werktage) | A99 | |

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
    malo_id: "51238696780".to_owned(),
    new_supplier_gln: "9900357000004".to_owned(),
    grid_operator_gln: "9900000000002".to_owned(),
    bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
    process_date: time::Date::from_calendar_date(2026, time::Month::August, 1).unwrap(),
    sparte: Sparte::Strom,
    messtyp: netz_checker::Messtyp::Slp,
};

let grid = MaloGridRecord {
    malo_id: "51238696780".to_owned(),
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

let result = evaluate(&anfrage, None, Some(&grid), true, time::OffsetDateTime::now_utc());

match result {
    NetzCheckResult::Accept => { /* dispatch bestaetigen */ }
    NetzCheckResult::Reject(r) => { /* dispatch ablehnen with r.erc_code */ }
    NetzCheckResult::Escalate { reason } => { /* alert operator */ }
}
```

---

## ERC codes

| Code | Meaning | Check |
|------|---------|-------|
| `A02` | Bilanzierungsgebiet mismatch | 4 |
| `A05` | Unknown Marktpartner | 5 |
| `A06` | Conflicting active supply or duplicate Anmeldung | 2 |
| `A97` | Invalid date (retroactive start) | 3 |
| `A99` | Mindestvorlauffrist not met | 6 |

Source: APERAK AHB 1.0 + GPKE AHB (BK6-22-024) + GeLi Gas AHB (BK7-24-01-009).

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
