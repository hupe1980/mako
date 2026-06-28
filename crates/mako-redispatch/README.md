# mako-redispatch

**Status: ⏳ Placeholder — pending `redispatch-xml` format layer.**

Event-sourced process engine for **Redispatch 2.0** congestion-management
workflows under §§ 13, 13a, 14 EnWG. Part of the `mako` workspace.

---

## Regulatory scope clarification

**This platform does not currently implement a certified Redispatch 2.0
participant role.** This crate is a placeholder for future implementation.

The regulatory facts are:
- Redispatch 2.0 is **mandatory for ÜNBs (TSOs) and VNBs (DSOs)** under
  BNetzA rulings BK6-20-059/060/061, effective 2021-10-01.
- It is **not mandatory for suppliers (LF) or metering operators (MSB)**
  in isolation.
- The MaKo market roles in scope for Redispatch 2.0 are:
  **ANB (Anschlussnetzbetreiber), VNB (Verteilnetzbetreiber), ÜNB
  (Übertragungsnetzbetreiber)**, and the technical/market resource operators
  for CIM-XML data exchange.

If this platform operates **only in supplier or MSB role**, Redispatch 2.0
is out of scope. If the platform intends to operate as a VNB or ÜNB, this
crate must be fully implemented before production deployment. See the
`CONCEPT.md` or your BDEW Marktteilnehmer role definition for scope
clarification.

---

## Architecture

Redispatch 2.0 spans three crates:

| Crate | Responsibility | Status |
|---|---|---|
| `edi-energy` | IFTSTA status messages (EDIFACT) | ✅ Implemented |
| `redispatch-xml` | XML/XSD format parsing and validation | ⏳ Placeholder |
| `mako-redispatch` ← **this crate** | Workflow impls, PID routing, deadline handling | ⏳ Blocked on `redispatch-xml` |

`makod` will activate Redispatch 2.0 handling once this crate provides a
`RedispatchModule` that implements `EngineModule`.

---

## Regulatory basis

Redispatch 2.0 entered into force on **1 October 2021** under the NABEG and
applies to all German transmission and distribution system operators. It is
the mandatory protocol for coordinating curtailment of generation units to
resolve grid congestion.

Key rulings:

| BNetzA decision | Topic |
|---|---|
| BK6-20-059 | Abrechnungsbilanzkreis |
| BK6-20-060 | Netzbetreiber-Koordination |
| BK6-20-061 | Informationsbereitstellung |

The XML schemas are published by BDEW at
[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25).

---

## Process scope

Unlike GPKE/WiM/GeLi Gas (UTILMD/APERAK-based), Redispatch 2.0 uses:
- **CIM/IEC 62325 XML** for primary data exchange (handled by `redispatch-xml`)
- **IFTSTA (EDIFACT)** for status confirmations (handled by `edi-energy`)

Planned workflows:

| Process | Parties | Direction |
|---|---|---|
| Stammdatenübermittlung | ANB → VNB → ÜNB | ANB sends asset master data |
| Planungsdaten (Abruffahrplan) | ÜNB → VNB → ANB | TSO sends dispatch schedule |
| Verfügbarkeitsmeldung | ANB → VNB | ANB reports availability |
| Redispatch-Abrechnung (Kostenblatt) | VNB → ÜNB | Cost reconciliation |

---

## Related crates

| Crate | Role |
|---|---|
| `redispatch-xml` | XML format layer (required by this crate) |
| `mako-redispatch` ← **this crate** | Process engine |
| `edi-energy` | IFTSTA status messages |
| `mako-engine` | Event-sourced workflow runtime |
