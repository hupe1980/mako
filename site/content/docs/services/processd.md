+++
title = "processd Operator Guide"
description = "processd operator guide: Process decision engine — NB Anmeldung STP, LF answers to every process the market asks a supplier about (Strom and Gas), MSB REQOTE and §14a ORDRSP, EoG gap closure. Role-gated binaries for §7 EnWG separation, Cedar ABAC on every route, MCP tools, PostgreSQL audit log."
weight = 23
[extra]
mermaid = true
+++
# `processd` Operator Guide

`processd` is the **process decision engine** — the service that automates
regulatory decisions within mandatory deadlines.

```mermaid
graph TB
    marktd["marktd :8180<br/>fan-out"]
    processd["processd :8580<br/>(this service)"]
    makod["makod :8080"]
    pg["PostgreSQL<br/>anmeldung_decisions<br/>approval_queue"]

    marktd -->|"de.mako.process.initiated<br/>de.markt.versorgung.gap-detected<br/>HMAC POST /webhook"| processd

    subgraph NB ["NB module (--features nb-only)"]
        NC["mako-pruefung<br/>NB: E_0622 · E_0623 · E_0607 · E_0608 · G_0011 · G_0007<br/>LF: E_0609 · E_0624 · E_0614 · E_0615<br/>STP target ≥ 95%"]
        EOG["EoG gap closure<br/>§36/§38 EnWG · §38 timer"]
        NC --> pg
        EOG --> pg
    end

    subgraph LF ["LF module (--features lf-only)"]
        LFA["LF answers · Strom 55007 / 55010 / 55016<br/>Gas 44007 / 44010 / 44016"]
        LFA --> pg
    end

    processd --> NB
    processd --> LF
    NB -->|"gpke.lieferbeginn.*<br/>gpke.lieferende.*<br/>gpke.eog.anmelden<br/>POST /api/v1/commands"| makod
    LF -->|"gpke.nb-lieferende.bestaetigen/ablehnen<br/>geli.lieferbeginn.anmelden<br/>POST /api/v1/commands"| makod
    NB & LF -->|"GET /api/v1/versorgung<br/>GET /api/v1/malos/{id}/grid"| marktd
```

---

## Port layout

```
┌────────────────────────────────────────────────────────────────────┐
│  processd  :8580                                                  │
│                                                                  │
│  POST /webhook              ← marktd CloudEvents (HMAC)          │
│  GET  /api/v1/decisions     ← NB STP audit log (OIDC+Cedar)     │
│  GET  /api/v1/queue         ← approval queue (every role)       │
│  POST /api/v1/queue/{id}/approve|reject  ← operator action       │
│  POST /api/v1/start-supply              ← LFN Strom bootstrap    │
│  POST /api/v1/start-supply-gas          ← LFN Gas 44001 bootstrap│
│  POST /api/v1/end-supply[-gas]          ← LF Lieferende bootstrap│
│  GET  /api/v1/eog           ← EoG gap-closure case log (§36/§38) │
│  GET  /api/v1/neuanlage     ← E_0608 case log                    │
│  PUT  /api/v1/neuanlage/{id}/identifikation                      │
│  GET  /health/live  /health/ready                                │
│  POST|GET /mcp       ← MCP Streamable HTTP (2025-11-25)          │
└────────────────────────────────────────────────────────────────────┘
```

---

## Role isolation

`processd` is compiled with **feature flags** that gate which modules are included.
This ensures §7 EnWG separation: an `nb-only` binary provably contains no LF PIDs.

```toml
[features]
role-lf-strom  = ["mako-pruefung/role-lf"]  # LF answers 55007 / 55010 / 55016
role-lf-gas    = ["mako-pruefung/role-lf"]  # LFA Gas 44007 / 44010 / 44016
role-nb-strom  = ["mako-pruefung/role-nb"]  # GPKE STP (55001, 55077, 55004, 55600/55601), EoG closure
role-nb-gas    = ["mako-pruefung/role-nb"]  # GeLi Gas An-/Abmeldung STP (44001, 44004)
role-msb-strom = []                          # REQOTE→QUOTES, §14a ORDRSP, MSB-answered MSB-Wechsel,
                                             # ESA Wertebestellung (35003/17007/17008/39002)

lf-only    = ["role-lf-strom", "role-lf-gas"]
nb-only    = ["role-nb-strom", "role-nb-gas"]
msb-only   = ["role-msb-strom"]
integrated = ["role-lf-strom", "role-lf-gas", "role-nb-strom", "role-nb-gas", "role-msb-strom"]
```

**PID 55016 „Kündigung" is not an NB process.** The *Anwendungsübersicht der
Prüfidentifikatoren 4.0* (lfd. Nr. 20030) has it going LFN → LFA, answered
55017/55018 by the Altlieferant under EBD `E_0614` — so it belongs to
`role-lf-strom`. Routing it into the NB module would make an `nb-only` binary
answer a supplier-role message, exactly what these feature flags exist to
prevent.

For §7 EnWG deployments (≥ 100k Netzkunden): BNetzA inspects the binary SHA to
confirm no cross-contamination. Use separate container images compiled with
`nb-only` and `lf-only` respectively.

---

## NB module — Anmeldung STP

### Decision pipeline

```text
de.mako.process.initiated
  ├─ Anmeldung (55001 verb. MaLo / 55077 erz. MaLo / 44001 Gas)
  │    → GET marktd /api/v1/versorgung/{malo_id}       → VersorgungsStatus
  │    → GET marktd /api/v1/malos/{malo_id}/grid       → MaloGridRecord
  │    → GET marktd /api/v1/partners/{lf_mp_id}        → partner_known
  │    → mako_pruefung::evaluate(…)
  └─ Abmeldung (55004 Strom / 44004 Gas)
       → GET marktd /api/v1/versorgung/{malo_id}       → VersorgungsStatus
       → mako_pruefung::evaluate_abmeldung(…)

  Accept   → anmeldung_decisions(Accept, Zustimmungscode)
             [if auto_accept] → makod …bestaetigen (antwort_code, antwort_ebd)
             else             → approval_queue
  Reject   → anmeldung_decisions(Reject, Antwortcode)
             → makod …ablehnen (antwort_code, antwort_ebd, bemerkung)
  Escalate → anmeldung_decisions(Escalate) → approval_queue
```

**Three trees, three alphabets.** `E_0622` Prüfschritt 10 splits Strom into two
branches that share no Antwortcode, and Gas answers from a different Codeliste:

| Anwendungsfall | Tree | „andere Anmeldung in Bearbeitung" | Fristüberschreitung | Zustimmung |
|---|---|---|---|---|
| Strom, verbrauchende / ruhende MaLo | `E_0622` 15–70 | `A06` | `A07` | `A51` (`E_0623`) |
| Strom, erzeugende MaLo / Tranche | `E_0622` 220–830 | `A45` | `A34`/`A28`/`A29`/`A30`/`A32`/`A35`/`A44` | `A58` (`E_0623`) |
| Gas | `E_3005` / `G_0011` | `ZC5` | `E17` | `E15` (`G_0012`) |
| Abmeldung Strom | `E_0607` | — | `A02` | `A11` |
| Abmeldung Gas | `E_3019` / `G_0007` | — | `E17` | `E15` |

Putting `A06` on a 44003 is not a wrong reason — it is a code the Gas Codeliste
does not define. Every code is resolved through `mako_pruefung::codes` against
the tree that publishes it, `RejectReason` can only be built from a resolved
entry, and `makod` re-checks it at the command boundary where the published
**Cluster decides the response PID**.

**A Bestätigung carries a code too.** `SG4 STS+E01` is Muss on every
Antwortnachricht: `E_0622` is a *Vorprüfung* whose codes are all Ablehnungen, and
a message that survives it is confirmed out of `E_0623`. `NbEntscheidung::Accept`
therefore carries `A51` / `A58` / `E15`.

### `mako_pruefung::nb::evaluate` — the Anmeldung

**Strom, verbrauchende / ruhende Marktlokation** (`E_0622` Prüfschritte 15–70):

| Prüfschritt | Rule | On failure |
|---|------|------------|
| — | `MaloGridRecord` exists for the MaLo | `Escalate` (a mako data gap, not a ground to refuse) |
| 15 | Vorlauffrist — one full Werktag between receipt and Zuordnungsbeginn (LFW24, all Transaktionsgründe) | `Reject A07` |
| 30 | MaLo participates in MaKo | `Reject A02` |
| 60 | Zuordnungsermächtigung: Bilanzierungsgebiet matches, LF in the partner directory | `Reject A05` |
| 70 | No other Anmeldung in Bearbeitung | `Reject A06` |

A **ruhende** Marktlokation is not refused: Prüfschritt 30's own Hinweis names
only stillgelegte Marktlokationen and the Modell-2-Zuordnung, and Prüfschritte
16–28 exist to check a ruhende one.

**Strom, erzeugende Marktlokation** (Prüfschritte 220–830) uses `A45` / `A25` for
the same two questions and then picks between the **six** Vorlauffristen GPKE
Teil 2 § 2.1.1 publishes, keyed on `(Geschäftsvorfall, bestehende, angemeldete
Veräußerungsform)`. The *angemeldete* one is `SG10 CCI+Z22` on the wire; the
*bestehende* one and the Ausfallvergütung flag come from `einsd`
(`GET /api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform`, configured as
`[nb] einsd_url`) — wire code `Z90` covers both the uneingeschränkte
Einspeisevergütung and the Ausfallvergütung, whose Fristen differ by a month
versus five Werktage. A missing fact escalates and is named; the statutory anchor
for the Monatserster rule is **§ 21b Abs. 1 EEG 2023**, not § 10c.

**Gas** (`G_0011`) runs the `A03`/`A04`/`A16`/`A17` identification checks first,
as the AHB requires, then `E17` for a Fristüberschreitung, `E13` for a
Bilanzierungsproblem and `ZC5` / `Z08` for a conflicting or duplicate Anmeldung.

### `mako_pruefung::nb::evaluate_neuanlage` — the Neuanlage, `E_0608`

Inbound **55600** / **55601**: a Lieferant registers a Marktlokation being
commissioned for the first time. The tree has a **third outcome** — Prüfschritte
110 / 590 loop, so a Marktlokation the NB cannot yet identify is re-checked
**daily for 60 Werktage** and may only then be refused `A07` / `A16`.

`processd` therefore keeps a case log (`neuanlage_faelle`) and a daily worker
rather than deciding once:

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/neuanlage?status=offen` | The Prüflauf view — each row carries the `letzter_pruefungstag` a refusal only becomes admissible after, and the `pruefungen` count that evidences the daily attempts |
| `PUT /api/v1/neuanlage/{id}/identifikation` | The MaLo-ID the NB matched from its NIS/GIS. A Neuanlage carries address and device data, not an ID, so this is where the identification arrives — the next pass then walks the tree with it |

The two branches share no code (`A01`–`A09` verbrauchend, `A10`–`A19`
erzeugend), and the Vorlauffrist is measured against the **Übertragungstag**, so
a case re-evaluated on day 40 reaches the same verdict it did on day one.

### `mako_pruefung::nb::evaluate_abmeldung` — the Abmeldung, `E_0607` / `E_3019`

Inbound **55004** (Strom) / **44004** (Gas): the supplier ends the assignment
and the NB answers 55005/55006 (44005/44006).

| # | Prüfschritt | On failure |
|---|---|---|
| 1 | The MaLo is known to this NB | `Escalate` |
| 2 | The requesting LF is the assigned Lieferant (Prüfschritt 110) | `Escalate` |
| 3 | Vorlauffrist eingehalten (Prüfschritt 50) — Strom: one full Werktag between receipt and Zuordnungsende, or Monatserster + 1 Monat for an EEG-MaLo (§ 21b Abs. 1 EEG 2023); Gas: the GeLi Gas Kap. 3.2.1 retroactivity rules | `Reject A02` (Strom) / `E17` (Gas) |
| 4 | Kein bereits bestätigtes Lieferende zum selben Datum (Prüfschritt 120) | `Reject A09` (Strom) / `Z08` (Gas) |
| — | Prüfschritt 130 — did the *already confirmed* Abmeldung name an Auszugsgrund? | `Escalate` — the projection does not keep the earlier message's Transaktionsgrund, and `A10` and „confirm" are both live outcomes there |

Prüfschritte 10–30 (Kundenanlagen-Herauslösung) and 60–90 (ESV-Ende, Aufhebung
einer zukünftigen Zuordnung) need Transaktionsgründe and prior process history
the projection does not carry; they escalate rather than guess. Escalation is
the § 20 EnWG-safe direction — an unfounded Ablehnung keeps a customer bound to
a supplier they have left.

### STP rate targets

`processd` targets ≥ 95 % straight-through processing on a **verbrauchende**
Anmeldung. The `malo_grid` record is a prerequisite — a missing one escalates —
so STP improves markedly once it is provisioned. An **erzeugende** Marktlokation additionally needs
`[nb] einsd_url`: `E_0622` chooses between six published Vorlauffristen from the
*bestehende* Veräußerungsform, which is register data and not on the wire. A
deployment without it escalates every 55077 — the § 20 EnWG-safe outcome, since
none of the six is a defensible default.

Grid records are the NB’s own grid topology — **not** from MaStR. Provision them via
marktd’s NB-role `PUT /api/v1/malos/{malo_id}/grid` endpoint (manual / ERP provisioning).

Monitor via `GET /api/v1/decisions` or the `get_stp_rate` MCP tool.

### `[nb] auto_accept`

Set `auto_accept = false` (the default; `PROCESSD_NB__AUTO_ACCEPT` in the
environment) until you have verified:

1. Grid record coverage for your MaLo portfolio (`GET /api/v1/malos/{id}/grid`)
2. Partner directory populated for all expected LF MP-IDs
3. At least one manual review cycle confirmed correct ERC codes

### §20 EnWG — affiliate guard

When `processd` is deployed in an **integrated NB+LF utility** (§6b EnWG),
auto-acceptance is **always blocked** for Anmeldungen where the requesting LF
is an **affiliate** of the NB operator. This implements the §20 EnWG
Diskriminierungsfreiheitspflicht non-discrimination obligation.

It applies to Abmeldungen as well as Anmeldungen — an affiliate must not get a
faster automatic path in either direction.

Detection logic:

```text
initiating LF MP-ID == [identity] own_mp_id  →  initiator_is_affiliate = true
                                                auto_accept overridden to false
                                                → approval_queue (operator review)
```

The operator's own MP-ID comes from `processd.toml`:

```toml
[identity]
own_mp_id = "9900357000004"   # BDEW 99… / DVGW 98…; must match makod's primary party
```

Startup logs the coding authority derived from it (293 = BDEW, 332 = DVGW) and
warns on a GS1 GLN, because a mismatched prefix makes every parity comparison
fail silently.

`obsd` records `initiator_is_affiliate = true` on the resulting `ProcessProjection`
and the KPI report exposes the parity delta for **BNetzA audit evidence**.
See [obsd §20 EnWG parity](obsd#20-enwg-parity) for query examples.

### Selbstzahler — the Lieferantenwechsel carve-out

GPKE Teil 1 (BK6-24-174 Anlage 1a), Vorbemerkung: „Ist der Letztverbraucher selbst
Netznutzer, so tritt er in die Rolle des Lieferanten i.S. dieser Prozessbeschreibung
[…]. **Eine Ausnahme bilden die Meldungen des Lieferanten im Rahmen des
Lieferantenwechsels.**"

A Selbstzahler („Netznutzer ohne All-Inklusiv-Vertrag") is an ordinary LF in every
other GPKE process, so nothing routes differently. But when a Wechsel displaces one,
the incumbent is not acting in the LF role the automation assumes. `processd` reads
the Netznutzungsvertrag in force the day before the requested Zuordnungsbeginn
(`GET /api/v1/nb-contracts/by-malo/{malo_id}?on=`) and, when its `netznutzer_typ` is
`LETZTVERBRAUCHER`, holds the decision for the operator instead of dispatching a
Bestätigung:

```text
transaktionsgrund == "E03" (Wechsel)
  ∧ incumbent netznutzer_typ == LETZTVERBRAUCHER
  → approval_queue (operator review), Antwortfrist attached
```

Only the Wechsel Transaktionsgrund triggers it. An Einzug (`E01`) or Einzug in
Neuanlage (`E02`) on the same MaLo stays on the automated path, and the lookup does
not run at all for those — widening the hold would take an industrial customer's
whole MaLo portfolio off automation for no regulatory reason.

---

## NB module — EoG gap closure (§36/§38 EnWG)

Every consuming Marktlokation must be assigned to a Bilanzkreis at all times
(GPKE Teil 2 Kap. 2.3). The EoG module closes supply gaps automatically:

```
de.markt.versorgung.gap-detected           (marktd: 55005/44005 completed,
  │                                         no announced successor)
  ├─ record case in eog_activations         (idempotent per MaLo)
  ├─ [eog.auto_activate] GET /api/v1/grundversorger/{nb_mp_id}?sparte=…
  │    └─ found → Strom: gpke.eog.anmelden → UTILMD 55013;
  │             Gas:   geli.eog.anmelden → UTILMD G 44013 → makod → E/G
  │       (Zuordnungsbeginn = day after Lieferende — retroactive allowed)
  │    └─ missing → case stays `detected`; operator provisions the
  │       §36 Abs. 2 Feststellung and re-triggers
  └─ de.markt.versorgung.eog-begonnen → case `active`
       (eog_art = Ersatz-/Grundversorgung as classified by the E/G in 55014;
        eog_seit = Zuordnungsbeginn)
```

The case lifecycle — the states exposed by `GET /api/v1/eog?status=…`:

```mermaid
stateDiagram-v2
    [*] --> detected: gap-detected (55005/44005, no successor)
    detected --> detected: Grundversorger missing (operator provisions §36 Abs. 2)
    detected --> angemeldet: eog.anmelden → UTILMD 55013 / G 44013
    angemeldet --> active: E/G Bestätigung 55014 (eog-begonnen)
    angemeldet --> detected: E/G Ablehnung 55015 (EBD E_0615)
    active --> expiring: §38 timer, warn_days_before_expiry ahead
    expiring --> expired: three-month maximum reached (§38 Abs. 4 S. 1)
    active --> [*]: Grundversorgung (no statutory maximum)
    expired --> [*]: operator follow-up (Grundversorgung regime / vertragliche Ersatzbelieferung / Anschlussunterbrechung)
```

**§38 timer.** A daily worker enforces the three-month maximum
(§38 Abs. 4 S. 1 EnWG, calendar months from `eog_seit` — not from detection):
`warn_days_before_expiry` days ahead the case turns `expiring`, at expiry
`expired`; both emit `de.markt.versorgung.ersatz-auslaufend` to
`eog.notify_webhook_url`. `Grundversorgung` cases have no statutory maximum.
After expiry the follow-up is operator-driven: Haushaltskunden transition into
Grundversorgung automatically **without a market message** (the E/G's billing
switches regime); otherwise the NB secures the Bilanzkreis (vertragliche
Ersatzbelieferung, STS `E06`) or interrupts the Anschlussnutzung.

**Operator surface.** `GET /api/v1/eog?status=detected|angemeldet|active|expiring|expired`.

```toml
[eog]
auto_activate            = true       # default false — record-only
default_transaktionsgrund = "ZT6"     # SG4 STS DE9013 for automatic Anmeldungen
warn_days_before_expiry  = 14
notify_webhook_url       = "https://erp.example/hooks/eog"
```

---

## LF module — answering what the market asks a supplier

Seven inbound processes, each with its own Entscheidungsbaum, its own Codeliste
and its own Antwortfrist:

| Sparte | Inbound | Process | EBD | Answers | Frist |
|---|---|---|---|---|---|
| Strom | 55007 | Lieferende von NB an LF | `E_0609` | 55008 / 55009 | 05:00 Uhr des 1. WT nach dem ÜT |
| Strom | 55010 | Beendigung der Zuordnung | `E_0624` | 55011 / 55012 | 09:00 Uhr des 1. WT nach dem ÜT |
| Strom | 55016 | Kündigung (LFN → LFA) | `E_0614` | 55017 / 55018 | Ablauf des 1. WT nach dem ÜT |
| Strom | 55607 | Ankündigung Zuordnung LF (erz. MaLo / Tranche) | `E_0603`–`E_0606` | 55608 / 55609 | **15:00 Uhr am ÜT** |
| Gas | 44007 | Lieferende von NB an LF | `E_3002` | 44008 / 44009 | Ablauf des 3. WT |
| Gas | 44010 | Beendigung der Zuordnung | `E_3020` | 44011 / 44012 | Ablauf des 3. WT |
| Gas | 44016 | Kündigung beim Altlieferanten | `E_3001` | 44017 / 44018 | Ablauf des 3. WT |

The same business process carries the same command name in both Sparten —
`{gpke,geli}.nb-lieferende.*`, `{gpke,geli}.beendigung-zuordnung.*`,
`{gpke,geli}.kuendigung.*` — and one walk decides both, from one
`de.mako.process.initiated` contract.

**55607 is the one where silence is not a lapsed Frist.** GPKE Teil 2 § 2.4.2.2
Nr. 3 has the NB assign the supplier to the erzeugende Marktlokation „aufgrund
fehlender Antwort" anyway, using whichever Bilanzkreis it has on file. The
substance of the answer is that **Bilanzkreis**, not the code: `A01` and `A99`
are the only two the four trees publish. Which BK is admissible is the BKV's
grant — MaBiS § 10.2.1 issues the Zuordnungsermächtigung „je ZRT, BG, BK und
LF" — so `[[lf.bilanzkreise]]` is keyed on the Bilanzierungsgebiet, and a regime
with several authorised BKs is a choice the supplier makes, not a default.

Two of the questions the trees ask decide the answer before any contract data is
consulted, and both come from the message itself:

- **Which object is the Vorgang about?** `SG4 STS+7` DE 9013 element 3 —
  `ZW3` erzeugende, `ZW4` verbrauchende, `ZW5` Tranche, `ZAP` ruhende
  Marktlokation. The two halves of `E_0609` and `E_0624` answer from *different
  code ranges*, so a missing Ergänzung is an escalation rather than a default.
- **Was the Vorlauffrist kept?** (`E_0609` Prüfschritt 40, `E_3002` `E17`.)
  Arithmetic on the Übertragungstag and the Zuordnungsende, resolved by
  `mako_fristen::abmeldung` against GPKE Teil 2 § 2.5.2 Nr. 1. The window is a
  calendar month for EEG-Marktlokationen and the day before the last Werktag for
  everything else, so an erzeugende Marktlokation escalates unless the deployment
  can say which it is.

### How a decision is made

`mako-pruefung` walks the published Prüfschritte. `processd` assembles the facts
they ask about and routes the outcome:

```
de.mako.process.initiated (an answerable PID)
  → LfAnfrage from the CloudEvent — Transaktionsgrund and its Ergänzung
    (ZW3/ZW4/ZW5/ZAP, which selects the tree's branch), Termin, ÜT
  → LfVertragslage from marktd (supply state) + vertragd (contract state)
  → pruefe_*(…)
      Antwort    → makod command carrying the Antwortcode and its EBD
      Eskalation → approval_queue, naming the Prüfschritt it stopped at
```

The Antwortcode's published **Cluster** selects the answer PID, not the command
name: `A36` „Vertragsverhältnis wurde beendet" rides 55011, `A35` „Es besteht
eine Vertragsbindung" rides 55012.

### Contract facts

Half of what the trees ask is about a contract, not about supply state:

| Prüfschritt | Question | Source |
|---|---|---|
| `E_0624` 50 | Ist der Kunde aus der Anfrage identisch mit dem Kunden beim LFA? | `vertragd` |
| `E_0624` 60 | Hat der LFA Informationen, dass sein Kunde nicht ausgezogen ist? | `vertragd` |
| `E_0624` 90 | Bleibt das Vertragsverhältnis zum Folgetag bestehen? | `vertragd` |
| `E_0614` 70 | Ist der Vertrag zum Kündigungstermin kündbar? | `vertragd` |
| `E_0609` 40 | Wurde die Vorlauffrist eingehalten? | `mako-fristen` |
| `E_0624` 20 | Besteht zum Folgetag noch eine Zuordnung? | `marktd` |

Set `[lf] vertragd_url` to answer them. Without it those facts are
`Bekannt::Unbekannt` and any decision reaching one escalates to an operator —
deliberately: a supplier with no contract database cannot claim a
Vertragsbindung, and must not agree to release the customer instead.

`auto_respond = false` means *an operator decides*, not *nobody answers*: the
walk still runs and its outcome is queued with the Antwortfrist attached.

### Approval queue

The queue is shared by **every** compiled role: the NB queues escalated and held-back
Anmeldungen, the MSB escalated MSB-Wechsel and § 14a Steuerungsaufträge, the LF its GPKE
answers. Each entry stores the `makod` command to dispatch on approve and on reject,
resolved from the trigger PID at enqueue time.

`expires_at` is the **business** answer Frist of that process, less headroom:

| Trigger | Frist | Source |
|---|---|---|
| GPKE Anmeldung 55001 / 55077 | 11:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 Teil 2, SD Lieferbeginn 5/6 |
| GPKE Abmeldung 55004 | 06:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 Teil 2, SD Lieferende von LF an NB 2/3 |
| GPKE LF answer 55007 | 05:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 Teil 2, SD Lieferende von NB an LF 2 |
| GPKE LFA answer 55010 | 09:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 Teil 2, SD Lieferbeginn 4 |
| GPKE LFA answer 55016 | Ablauf des 1. WT nach dem ÜT | BK6-24-174 Teil 2, SD Kündigung 2 |
| GeLi Gas LF answer 44007 | Ablauf des 3. WT | AWH GeLi Gas 2.0 Kap. 2.3.2 Nr. 2 |
| GeLi Gas LFA answer 44010 | Ablauf des 3. WT | AWH GeLi Gas 2.0 Kap. 2.5.2 Nr. 4 |
| GeLi Gas LFA answer 44016 | Ablauf des 3. WT | GeLi Gas 3.0 Kap. 3.1 |
| GeLi Gas Anmeldung 44001 | Ablauf des 4. WT nach Eingang | BK7-24-01-009 Kap. 3.2.3 |
| GeLi Gas Abmeldung 44004 | Ablauf des 3. WT nach Eingang | BK7-24-01-009 Kap. 3.2.2 |
| WiM MSB-Wechsel 55039 / 55042 / 55051 / 55168 | 3 / 5 / 7 / 1 WT | WiM Strom Teil 1 |
| GPKE Neuanlage 55600 / 55601 | 00:00 Uhr des 61. WT nach dem ÜT | BK6-24-174 Teil 2 § 2.2.2 (60 WT täglicher Prüflauf, `E_0608`) |
| GPKE Sperr-/Entsperrauftrag 17115 / 17117 / 39000 | spätester ÜT ist der 1. WT nach dem ÜT | BK6-24-174 Teil 2 §§ 3.5.1.2 / 3.5.2.2 / 3.5.3.2 Nr. 2 |
| GPKE Anfrage Sperrung an den MSB 17116 | 3. WT nach dem ÜT — Fristverstreichen gilt als Zustimmung | BK6-24-174 Teil 2 § 3.5.1.2 Nr. 4 |
| GPKE Teil 4 Stammdaten-Rückmeldung 55109 / 55557 / 55639–55643 / 55693 | 2. WT nach dem ÜT | BK6-24-174 Teil 4 §§ 1.4.3 / 1.4.4 Nr. 2 |
| GPKE Teil 2 Bearbeitungsstand Abrechnungsdaten 55156 / 55220 / 55673 | 2. WT nach dem ÜT | BK6-24-174 Teil 2 §§ 3.1.1.2 / 3.1.2.2 / 3.1.3.2 |

The 45-minute APERAK window on the same message is a separate clock and is `makod`'s to
answer.

A background task runs every 60 s and sets `status = Expired` for stale entries. It is
deliberately **not** role-gated, since every role build can enqueue.

`decided_by` records the `sub` of the principal who approved or rejected (§ 20 EnWG parity
evidence and the GoBD trail both have to say *who* decided).

**Operator workflow:**
```
GET /api/v1/queue                     → list Pending entries (review before expires_at)
POST /api/v1/queue/{id}/approve       → dispatch consent command via makod AND mark Approved
POST /api/v1/queue/{id}/reject        → dispatch reject command via makod AND mark Rejected
```

> **Regulatory deadline:** `expires_at` is the per-PID business Frist less an
> hour of headroom (`OPERATOR_HEADROOM`), read from `mako_fristen::antwort` —
> the table `makod` also registers the process deadline from.
>
> The approve/reject handlers **claim** the entry (`status = 'Pending'` guard)
> before dispatching to `makod`, releasing the claim if the dispatch fails so the
> operator can retry. Claiming first is what stops a terminal entry re-sending
> its market message, and two operators deciding at once from sending both an
> einwilligung and an ablehnen. Expired entries log a `WARN` and must be
> reconciled manually.

---

## LF module — LFN bootstrap

### Strom: `POST /api/v1/start-supply`

Initiates a GPKE Lieferbeginn (UTILMD 55001) with **LFW24 Vorlauffrist validation**
(BK6-22-024, effective 2025-06-06).

```bash
curl -X POST http://processd:8580/api/v1/start-supply \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"malo_id": "10001234558", "lieferbeginn_datum": "2026-10-01"}'
```

| Field | Required | Notes |
|---|---|---|
| `malo_id` | ✓ | 11-digit Strom Marktlokations-ID |
| `lieferbeginn_datum` | ✓ | ISO-8601 date (YYYY-MM-DD) |

**Vorlauffrist rules (LFW24, BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritt 1):**

"Spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn" — the
Frist is day-granular (ÜT = calendar day of the AS4 receipt); there is no
time-of-day cutoff.

| Submission (Berlin date) | Earliest allowed Lieferbeginn |
|---|---|
| Today | Calendar day after the next Werktag after today |
| Retroactive date (`< today_berlin`) | Rejected with `RETROACTIVE_DATE` |

Response includes `earliest_lieferbeginn` and `berlin_date_at_submission` for
operator transparency.

### Gas: `POST /api/v1/start-supply-gas`

Initiates a GeLi Gas Lieferbeginn (UTILMD 44001). Both `malo_id` and `zaehlpunkt`
**are mandatory** per BK7-24-01-009 AHB rules.

```bash
curl -X POST http://processd:8580/api/v1/start-supply-gas \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "malo_id":    "10001234558",
    "zaehlpunkt": "DE00123456789012345678901234567890",
    "process_date": "20261001"
  }'
```

| Field | Required | Notes |
|---|---|---|
| `malo_id` | ✓ | Gas-MaLo-ID — rendered into `SG5 LOC+Z16` |
| `zaehlpunkt` | ✓ | Zählpunktbezeichnung (RFF+Z13) |
| `process_date` | ✓ | Lieferbeginn date (YYYYMMDD, CET/CEST) |
| `transaktionsgrund` | — | `E03` Wechsel (default), `E01`/`E02` Einzug |

**Mindestvorlauffrist: 10 WT** for a Lieferantenwechsel (AWH GeLi Gas 2.0
Kap. 2.5.2 Nr. 1), enforced here. An Einzug passes with a
`vorlauffrist_hinweis`: the NB corrects the date to the second Werktag after
confirmation rather than rejecting (Kap. 4).

The GNB responds with PID 44002 (Bestätigung) or 44003 (Ablehnung) by the
**Ablauf des 4. WT** (Kap. 3.2.3) — a different clock from the supplier's own
10-WT lead time.

> **No API-Webdienste equivalent for Gas.** The ERP must supply the Gas-MaLo-ID
> (`malo_id`) upfront from the customer contract, MaStR, or DVGW Codevergabe.

### Gas Datenabruf: `geli.datenabruf.anfragen`

Request Abrechnungsbrennwert and Zustandszahl on-demand (ORDERS 17103):

```bash
curl -X POST http://makod:8080/api/v1/commands \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"command": "geli.datenabruf.anfragen", "payload": {"malo_id": "10001234558"}}'
```

The GNB responds with MSCONS 13007 (data delivery) or ORDRSP 19103 (rejection).
Successful delivery automatically updates `edmd` `meter_billing_periods` via the
existing `update_gas_quality` path.

---

## §20 EnWG parity

Every `anmeldung_decisions` row includes:

```sql
initiator_is_affiliate BOOLEAN  -- TRUE when lf_mp_id == own_mp_id (integrated deployment)
```

This field is the BNetzA audit evidence for §20 EnWG parity compliance.
A systematically faster decision time for `initiator_is_affiliate = true` is
a §20 EnWG violation in integrated §6b EnWG deployments.

Use `obsd`'s parity report or query directly:

```sql
SELECT
    initiator_is_affiliate,
    COUNT(*) AS total,
    AVG(EXTRACT(EPOCH FROM (decided_at - created_at))) AS avg_response_secs
FROM anmeldung_decisions
WHERE tenant = $1 AND decided_at >= now() - interval '90 days'
GROUP BY initiator_is_affiliate;
```

---

## Configuration reference

`processd` reads its configuration from a **TOML file** (default: `processd.toml`),
with secrets deferred to environment variables via `"env:VAR_NAME"` values.

```bash
# The config-file path defaults to ./processd.toml; override with PROCESSD_CONFIG.
PROCESSD_CONFIG=/etc/processd/processd.toml processd
```

### Full `processd.toml` reference

```toml
[http]
addr = "0.0.0.0:8580"          # default

[database]
url       = "env:DATABASE_URL"  # required; use env: for secrets
pool_size = 10                  # default

[identity]
own_mp_id = "9900357000004"     # required — must match makod.toml [[party]] primary
tenant    = ""                  # optional; defaults to own_mp_id

[makod]
url     = "http://makod:8080"   # required
api_key = "env:MAKOD_API_KEY"   # required

[marktd]
url     = "http://marktd:8180"  # required
api_key = "env:MARKTD_API_KEY"  # required

[webhook]
inbound_secret = "env:INBOUND_WEBHOOK_SECRET"   # optional; omit for dev

[subscription]
# Self-register this subscription with marktd on startup.
# No manual curl required — topology is fully config-driven.
webhook_url   = "http://processd:8580/webhook"  # optional; omit to skip registration
subscriber_id = "processd"                       # default
# default: de.mako.process.initiated + de.markt.versorgung.{gap-detected,eog-begonnen,changed}
event_types   = "de.mako.process.initiated"

[nb]
auto_accept              = false  # true → dispatch bestaetigen automatically on Accept
gas_bearbeitungsfrist_wt = 3      # AWH GeLi Gas 2.0 Kap. 2.2
einsd_url                = ""     # EEG-/KWKG-Register; without it every 55077 escalates
einsd_api_key            = ""

[lf]
auto_respond = true   # false → every inbound LF process routed to approval_queue

# The Bilanzkreise a 55607 Zustimmung may name, by Bilanzierungsgebiet and
# regime. MaBiS § 10.2.1 grants the Zuordnungsermächtigung „je ZRT, BG, BK und
# LF", so one BK per regime answers automatically and several is an operator
# choice. Omit `bilanzierungsgebiet` for the fallback row; omit the table
# entirely and every 55607 escalates with its 15:00-Uhr Frist attached.
[[lf.bilanzkreise]]
bilanzierungsgebiet = "11YN-BG-EON---X"
eeg      = ["11XBK-EEG-----1"]
kwkg     = ["11XBK-KWKG----5"]
standard = ["11XBK-STD-----9"]

[msb]
auto_accept       = false   # true → dispatch the MSB-Wechsel Bestätigung
auto_preisanfrage = true    # false → the REQOTE goes to the approval queue

[esa]                                 # WiM Teil 2 Kap. 4 — the MSB's answers to an ESA
auto_accept                = false    # true → dispatch the E_0256/E_0257/E_0254 Zustimmungscode
auto_reject                = false    # true → dispatch a deterministic Ablehnungscode
accept_after_bindungsfrist = false    # E_0256 Prüfschritt 2 — a commercial decision

[eog]                                     # §36/§38 EnWG gap closure (NB role)
auto_activate             = false         # true → dispatch gpke.eog.anmelden on gap-detected
default_transaktionsgrund = "ZT6"         # SG4 STS DE9013 for automatic Anmeldungen
warn_days_before_expiry   = 14            # §38 Abs. 4 3-month warning lead
# notify_webhook_url      = "https://erp.example/hooks/eog"   # ersatz-auslaufend CloudEvents

# [oidc]                # omit to disable auth (dev only — never omit in production)
# issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
# audience = "api://mako-processd"
# jwks_refresh_secs = 300

# Tracing/OTel is configured from the environment (see the table below), not
# from a [otel] block.
```

### CLI flags & environment

The daemon lifecycle is owned by the shared `mako_service` runner. There is a
single CLI flag; everything else is environment-driven.

| Flag / Env var | Default | Description |
|----------------|---------|-------------|
| `--check` | — | Probe the running instance's `/health/ready` on loopback and exit 0/non-zero (container HEALTHCHECK) |
| `PROCESSD_CONFIG` | `processd.toml` | Path to the config file |
| `RUST_LOG` / `LOG_LEVEL` | `info` | Log level (`info`, `debug`, `processd=trace`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | OTLP endpoint; unset disables tracing export |

Any config key may also be overridden via a `PROCESSD_<SECTION>__<KEY>` env var
(e.g. `PROCESSD_DATABASE__URL`).

---

## marktd subscription — self-registration

`processd` **self-registers** its subscription with `marktd` on startup.
Set `[subscription] webhook_url` in `processd.toml` to the URL `marktd` should POST
events to, and `processd` calls `PUT /api/v1/subscriptions/{subscriber_id}`
automatically with exponential-backoff retry (up to 30 s).

This makes subscription topology **configuration-driven** (TOML / Helm
`values.yaml`) rather than an imperative bootstrap step.

For Helm charts, map `[subscription]` to `values.yaml` under `processd.subscription.*`.

---

## MCP tools

| Tool | Role | Description |
|------|------|-------------|
| `list_decisions` | NB | Last N Anmeldung decisions with ERC codes and affiliate flag |
| `get_decision` | NB | Single Anmeldung decision by UUID |
| `get_stp_rate` | NB | STP rate over last N days vs. 95 % target |
| `get_stp_breakdown_by_erc` | NB | Rejection breakdown by ERC code |
| `list_affiliate_decisions` | NB | Decisions involving affiliated suppliers (§20 EnWG parity) |
| `list_pending_approvals` | LF | Pending approval queue entries (most urgent first) |
| `get_queue_entry` | LF | Single queue entry by UUID |
| `approve_queue_entry` | LF | Approve a queue entry (dispatches the response) |
| `reject_queue_entry` | LF | Reject a queue entry with a reason code |

---

## Monitoring

Domain metrics register on the shared `/metrics` served by `mako_service::run`:

| Metric | Type | Description |
|--------|------|-------------|
| `processd_decisions_total{decision,pid}` | counter | NB STP decisions by outcome (`Accept`/`Reject`/`Escalate`) and PID (`55001`/`55077`/`55004`/`44001`/`44004`) |
| `processd_approval_queue_pending` | gauge | Processes waiting for an operator decision |
| `processd_approval_queue_overdue` | gauge | Pending entries past `expires_at` — the answer deadline has been missed. **Alert on > 0** |
| `processd_eog_open` | gauge | Ersatz-/Grundversorgung cases not yet closed (§ 36/§ 38 EnWG) |

### Alert rules

| Metric / Query | Target |
|----------------|--------|
| `processd_decisions_total{decision="Accept"} / processd_decisions_total` | ≥ 95 % (STP rate) |
| `processd_approval_queue_overdue` | 0 |
| `processd_decisions_total{decision="Escalate"}` / total | < 5 % (grid coverage indicator) |

Alert when:
- STP rate drops below 90 % (grid record coverage degraded — provision missing `malo_grid` records via marktd’s NB-role `PUT /api/v1/malos/{malo_id}/grid` endpoint)
- `processd_approval_queue_overdue` > 0 (a business answer Frist has been missed)
- Decision latency > 10 s (marktd connectivity issue)

---

## MSB module — WiM MSB-Wechsel STP

`processd` evaluates inbound WiM MSB-Wechsel requests automatically:
`role-nb-strom` answers 55042 (Anmeldung MSB) and 55051 (Ende MSB),
`role-msb-strom` answers 55039 (Kündigung MSB) and 55168 (Verpflichtungsanfrage).
STP target: **≥ 80 %**.

The Prüfschritte are `mako_pruefung::msb`, the executable form of the published
Entscheidungsbäume; `msb_module.rs` is the plumbing around them.

### Decision pipeline (`msb_module.rs`)

```
de.mako.process.initiated (PID 55039 / 55042 / 55051 / 55168)
  → GET marktd /api/v1/melos/{melo_id}          ← MeLo known?
  → GET marktd /api/v1/partners/{msb_mp_id}     ← Rahmenvertrag § 9 Abs. 1 Nr. 3 MsbG?
  → mako_pruefung::msb::pruefe_{anmeldung,abmeldung,kuendigung}
      Accept   → wim.geraetewechsel.bestaetigen (antwortcode) [if auto_accept]
                 else approval_queue with the WiM Frist
      Reject   → wim.geraetewechsel.ablehnen (antwortcode from the process's EBD)
      Escalate → approval_queue with the WiM Frist
```

The Frist runs from the Übertragungstag the CloudEvent carries in `time`, so a
redelivery does not restart it.

### Anmeldung MSB (55042) — `E_0201`, answered by the NB

WiM Strom Teil 1 Kap. 2.3.2 Nr. 2 names three checks, and no others:

| Check | Outcome on failure |
|---|---|
| Messlokation known to this NB | `ZC9` |
| Versicherung über die Beauftragung durch den AN vorhanden | `ZB6` |
| Mindestvorlaufzeit eingehalten — 15 WT, 7 WT bei erstmaliger Einrichtung | `E17`, naming the earliest reachable Zuordnungsbeginn |
| Vertrag nach § 9 Abs. 1 Nr. 3 MsbG mit dem MSBN | Escalate — `E_0201` publishes no code |

The metering technology at the Messlokation is **not** a ground: §5 MsbG gives the
Anschlussnutzer a free choice of MSB and §14 MsbG the right to switch, and `E_0201`
publishes no code for it.

### Ende MSB (55051) — `E_0202`, answered by the NB

A Zuordnungsende inside the 20-Werktage Mindestvorlauffrist is not refused: Kap. 2.4.2
Nr. 2 has the NB set it to the nächstmögliches Zuordnungsende and confirm with `Z01`.
An Außerbetriebnahme carries no lead time at all — it is reported after the
Geräteausbau.

### Kündigung MSB (55039) — `E_0200`, answered by the **MSBA**

The Kündigung runs on the contract layer between the two MSB (Kap. 2.1.3); the
Netzbetreiber is not a party. Every Prüfschritt is a question about the MSBA's own
Messstellenbetriebsvertrag, which `vertragd` holds
(`GET /api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}`) — the same split the LF
module uses: supply and market state from `marktd`, contract state from `vertragd`:

| Contract state | Outcome |
|---|---|
| `vertragd` unreachable or unconfigured | Escalate |
| `404` — this MSB holds no contract here | `ZC9` |
| `beendet_am` set | `Z29` |
| `gekuendigt_zum` set | the Kap. 2.2.3 table — `E15`, `Z01` to an earlier date, or `Z34` |
| Live, requested date on or after `naechstmoeglich` | `E15` |
| Live, requested date inside the binding | `Z12`, naming `naechstmoeglich` |
| Live, no `naechstmoeglich` recorded | Escalate |
| „Nächstmöglicher Termin" requested (`DTM+471`) | `Z01`, naming the date |

`vertragd` derives `naechstmoeglich` from the contract's notice period, capped by
§ 309 Nr. 9 lit. c BGB. Its absence on a live contract escalates rather than
confirming: „no Kündigungsfrist recorded" and „terminable at any time" look identical,
and only one of them makes every requested date admissible.

### Verpflichtungsanfrage (55168) — `E_0240`

Kap. 2.4.2 Nr. 4 leaves the answer to the gMSB's own commercial judgement („nach
eigenem Ermessen"), so it escalates with its 1-Werktag window attached.

All escalated decisions still generate an `anmeldung_decisions` row for the §20 EnWG
audit trail.

---

## MSB module — REQOTE auto-response

When `processd` receives `de.mako.process.initiated` for PIDs 35001, 35002, 35004 or 35005 (REQOTE Preisanfrage from an nMSB), it **automatically dispatches a QUOTES response** sourced from the active `PreisblattMessung` in `marktd`. Dispatching from master data rather than from a manual ERP trigger is what keeps the response inside the REQOTE answer window.

### Decision pipeline

```
de.mako.process.initiated (PIDs 35001/35002/35004/35005, REQOTE)
  → GET marktd /api/v1/preisblaetter-messung/{own_mp_id}  ← PreisblattMessung current?
      Found   → wim.preisanfrage.angebot-senden
                 (includes preisblatt_gueltigkeit in payload for makod QUOTES build)
      Not found → approval_queue with the REQOTE Frist — an operator quotes
```

Enable in `processd.toml`:

```toml
[msb]
auto_preisanfrage = true   # default: true
```

Set `auto_preisanfrage = false` to route every REQOTE to the approval queue instead (e.g. during PreisblattMessung update windows); it lands there with its Frist either way.

---

## MSB module — ESA Wertebestellung (WiM Teil 2 Kap. 4)

Serving an Energieserviceanbieter is a **mandatory** Zusatzleistung (§34 Abs. 2
S. 2 Nr. 10 MsbG), so `role-msb-strom` always carries these four obligations:

| Inbound PID | Process | Answered with | Frist | EBD |
|---|---|---|---|---|
| **35003** | Werteanfrage (REQOTE) | QUOTES 15003 | 5 WT | — |
| **17007** | Bestellung von Werten | ORDRSP 19011/19012 | 2 WT | `E_0256` |
| **39002** | Stornierung der Bestellung | ORDRSP 19013/19014 | 2 WT | `E_0257` |
| **17008** | Abbestellung von Werten | ORDRSP 19011/19012 | 2 WT | `E_0254` |

The Prüfschritte are `mako_pruefung::msb::esa`; `esa_module.rs` is the plumbing.

### Decision pipeline (`esa_module.rs`)

```
de.mako.process.initiated (PID 35003 / 17007 / 17008 / 39002)
  → GET marktd /api/v1/esa/framework/{msb}/{esa}   ← Rahmenvertrag established?   (E_0256 Nr. 6)
  → GET marktd /api/v1/esa/consent-check           ← Einwilligung still valid?    (E_0256 Nr. 8)
  → GET marktd /api/v1/melos/{melo}/msb?at=        ← MSB assigned for the period? (E_0256 Nr. 7)
  → GET marktd /api/v1/malos/{malo}/buendel?at=    ← one MSB across the bundle?  (E_0256 Nr. 11)
  → mako_pruefung::msb::esa::pruefe_{bestellung,stornierung,beendigung}
      Accept   → wim.wertebestellung.*-beantworten (Zustimmungscode) [if auto_accept]
                 else approval_queue with the WiM Frist
      Reject   → the same command with the tree's Ablehnungscode      [if auto_reject]
      Escalate → approval_queue with the WiM Frist
```

The answer command is the same for both clusters: the code's **Cluster** picks
the PID, so there is no separate „ablehnen" command to route to.

### The Werteanfrage always reaches an operator

`E_0253` „Angebot zur Anfrage prüfen" is published **without a tree**, and the
Angebot is a priced offer — Bindungsfrist, earliest start, one `PRI+CAL` per
Artikel-ID — whose terms the Festlegung does not specify. 35003 therefore goes
to the queue with its 5-Werktage window and both candidate commands
(`anbieten` / `anfrage-ablehnen`) attached.

### What escalates on the order PIDs

Three Prüfschritte reach facts no mako service holds, so the walk escalates
rather than guessing:

- **Optional Messprodukte.** Whether *this* MSB offers a product is commercial.
  The seven Pflichtprodukte (BNetzA *Mitteilung Nr. 3*) are answered; an
  otherwise-clean order for an optional one escalates.
- **Gerätetechnik** (Prüfschritt 9) is a device fact mako does not hold, so it is
  never asserted false.
- **`E_0254` Prüfschritte 3/4** compare the requested end against values already
  delivered — that state lives in the `makod` process, not in the event.

Enable in `processd.toml`:

```toml
[esa]
auto_accept = false                 # true → dispatch the Zustimmungscode directly
auto_reject = false                 # true → dispatch a deterministic Ablehnungscode
accept_after_bindungsfrist = false  # E_0256 Prüfschritt 2 — a commercial decision
```

Both automation flags default to `false` and are deliberately separate: a wrong
Bestätigung commits the MSB to a delivery it may not be able to make, a wrong
Ablehnung denies a §34-mandated Zusatzleistung.

---

## MSB module — §14a Steuerungsauftrag auto-ORDRSP

When an MSB receives a WiM Steuerungsauftrag (iMS ORDERS, `makoworkflow = wim-steuerungsauftrag`), `processd` auto-confirms if:

1. The `SteuerbareRessource.istFernschaltbar = true` (remote-switchable), **and**
2. The dispatched `produktcode` is in the contracted `konfigurationsprodukte` list (GPKE Teil 3 Kap. 1.3).

If the `produktcode` is not contracted, `processd` dispatches `wim.steuerungsauftrag.ablehnen` immediately — preventing unauthorized control of customer assets.

### Decision pipeline

```
de.mako.process.initiated (wim-steuerungsauftrag)
  → [parallel]
      GET marktd /api/v1/steuerbare-ressourcen/{sr_id}                 ← istFernschaltbar?
      GET marktd /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte  ← contracted?
  → istFernschaltbar=true + produktcode contracted  → bestaetigen
  → istFernschaltbar=true + produktcode NOT contracted  → ablehnen (GPKE Teil 3 Kap. 1.3)
  → istFernschaltbar=false  → Escalate (manual ORDRSP required)
  → SR not found  → Escalate
```

Register contracted products via `PUT /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` in `marktd`. Each entry requires a non-empty `zaehlzeitregister`-linked `produktcode`.
