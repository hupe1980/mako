# processd — Process Decision Engine

**Automated market-process decisions, inside the regulatory deadline, without ERP involvement.**

`processd` subscribes to `marktd`'s durable fan-out, and for each
`de.mako.process.initiated` applies the policy of the role that owes the answer.
What it cannot decide it puts in front of an operator with the deadline attached.

---

## What answers what

| Role | Inbound PID | Process | Business Frist | Decision basis |
|---|---|---|---|---|
| **NB** | 55001, 55077 | GPKE Anmeldung (verb. / erz. MaLo) | **11:00 Uhr des 1. WT nach dem ÜT** | `mako_pruefung::evaluate` — `E_0622`'s two branches, confirmed out of `E_0623` |
| **NB** | 55004 | GPKE Abmeldung — Lieferende von LF an NB | **06:00 Uhr des 1. WT nach dem ÜT** | `mako_pruefung::evaluate_abmeldung` (`E_0607`) |
| **NB** | 44001 | GeLi Gas Anmeldung NN | **Ablauf des 4. Werktags** | `mako_pruefung::evaluate` (`G_0011`) |
| **NB** | 44004 | GeLi Gas Abmeldung NN | **Ablauf des 3. Werktags** | `mako_pruefung::evaluate_abmeldung` (`G_0007`) |
| **NB** | 55042 | WiM Anmeldung MSB (MSBN → NB) | 5 WT | MeLo / partner / Zählertyp checks |
| **NB** | 55051 | WiM Ende MSB (MSBA → NB) | 7 WT | operator queue |
| **NB** | 55600, 55601 | GPKE Neuanlage (verb. / erz. MaLo) | **00:00 Uhr des 61. WT nach dem ÜT** | `mako_pruefung::nb::neuanlage` (`E_0608`), daily Prüflauf |
| **NB** | *(event)* | EoG gap closure → 55013 (§ 36/§ 38 EnWG) | unverzüglich; 3-month timer | `grundversorger` from `marktd` |
| **LF** | 55007 | Abmeldung / Beendigung der Zuordnung vom NB (EBD `E_0609`) | **05:00 Uhr des 1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` + `mako_fristen::abmeldung` |
| **LFA** | 55010 | Anfrage zur Beendigung der Zuordnung (EBD `E_0624`) | **09:00 Uhr des 1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` |
| **LFA** | 55016 | GPKE Kündigung (LFN → LFA, EBD `E_0614`) | **1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` |
| **LFN** | 55607 | Ankündigung Zuordnung LF, erz. MaLo / Tranche (EBDs `E_0603`–`E_0606`) | **15:00 Uhr am ÜT** | `malo.bilanzierungsgebiet` from `marktd` + `[[lf.bilanzkreise]]` |
| **E/G** | 55013 | Anmeldung Ersatz-/Grundversorgung (EBD `E_0615`) | **15:00 Uhr am ÜT** | `[lf] grundversorgungs_netzgebiete` + `VersorgungsStatus` |
| **LFA** | 44007, 44010, 44016 | GeLi Gas Lieferende von NB / Beendigung der Zuordnung / Kündigung | 3 Werktage | `VersorgungsStatus` from `marktd` |
| **E/G** | 44013 | GeLi Gas Anmeldung Ersatz-/Grundversorgung (`E_3008`) | **Ablauf des 2. WT** | as 55013 |
| **MSB** | 55039 | WiM Kündigung MSB (MSBN → MSBA) | 3 WT | MeLo / partner checks |
| **MSB** | 55168 | WiM Verpflichtungsanfrage (NB → gMSB) | 1 WT | operator queue |
| **MSB** | 35001, 35002, 35004, 35005 | REQOTE Preisanfrage | 5 WT | `PreisblattMessung` from `marktd` |
| **MSB** | 35003 | ESA Werteanfrage (WiM Teil 2 Kap. 4) | 5 WT | operator queue — `E_0253` publishes no tree |
| **MSB** | 17007 | ESA Bestellung von Werten | 2 WT | `mako_pruefung::esa::wertebestellung` (`E_0256`) |
| **MSB** | 39002 | ESA Stornierung der Bestellung | 2 WT | `mako_pruefung::esa::wertebestellung` (`E_0257`) |
| **MSB** | 17008 | ESA Abbestellung von Werten | 2 WT | `mako_pruefung::esa::wertebestellung` (`E_0254`) |
| **MSB** | *(workflow)* | § 14a Steuerungsauftrag → ORDRSP | 5 WT | contracted `konfigurationsprodukte` |

Four properties of that table, each of which fails silently when broken:

- **The PID is the inbound trigger, never an answer.** `makod` emits
  `process.initiated` with the `makopid` of the message that *spawned* the
  process, so a module keyed on 55008 (the LF's answer to 55007) waits forever.
  `tests/pid_contract.rs` pins every trigger against the canonical `mako-gpke` /
  `mako-wim` constants.
- **The trigger has to be published at all.** `makod` delivers a CloudEvent only
  for an outbox entry, and the APERAK an inbound UTILMD produces is a *technical*
  acknowledgement addressed to the counterparty. Each LF-answered workflow
  therefore emits its own `de.mako.process.initiated` carrying the `SG4` facts
  the trees branch on. A workflow that emits only the APERAK is invisible here,
  and every one of its Fristen expires unanswered.
- **The GPKE Frist is a wall-clock instant, not a duration.** A Friday-afternoon
  Anmeldung is answerable until Monday 11:00; a Tuesday-evening one until
  Wednesday 11:00 — fifteen hours. A flat window is therefore both too tight and
  too loose, and the loose direction reports a lapsed Frist as still running.
- **The GeLi Gas 10 Werktage is not an answer window.** It is the *supplier's*
  Vorlauffrist for a Lieferantenwechsel Anmeldung; the GNB answers within
  4 Werktage (Kap. 3.2.3) and 3 for the Abmeldung (Kap. 3.2.2), counted from the
  first Werktag after receipt (§ 187 Abs. 1 BGB, Kap. 2.6).
- **A dependency outage is not a business finding.** Where a decision needs
  master data, a transport failure answers `5xx` so the fan-out redelivers; only
  a genuine *absence* of that data escalates to an operator.

Every window comes straight from `mako_fristen::antwort` — the same table
`makod` registers the process deadline from — so the two cannot disagree about
whether an obligation was met. `processd` adds only `OPERATOR_HEADROOM`, the hour
between when an operator must decide and when the answer is due.

### Two clocks on every inbound message

| Clock | Window | Owner |
|---|---|---|
| Technical acknowledgement (APERAK) | **45 min** on weekdays; Sunday 12:00 Berlin for a Saturday arrival | **`makod`**, automatically |
| Business answer | the per-PID Frist above | `processd` / the operator |

The approval queue is bounded by the **business** window, less an hour of
headroom so an operator's answer still reaches the counterparty in time.

### What processd deliberately does not answer

**PID 55016 „Kündigung" is not an NB process.** The EDI@Energy
*Anwendungsübersicht der Prüfidentifikatoren 4.0* (lfd. Nr. 20030) has it going
**LFN → LFA**, answered 55017/55018 by the *Altlieferant* under EBD `E_0614`.
Answering it from an `nb-only` binary would put a Willenserklärung on the market
in a role that build does not hold; it belongs to `role-lf-strom`, which answers
it as the Altlieferant.

## Role-gated builds

Compile with exactly the Marktrolle(n) you operate. The features select which
decision paths are registered, so a deployment cannot answer as a role it was
not built for — `tests/role_separation.rs` asserts that per build.

This is an operational guard, not a regulatory one. §7 EnWG requires a separate
*legal entity* for a VNB at or above 100 000 connected customers (§7 Abs. 2 is
the de-minimis threshold), §7a operational and §6a informational separation.
None is discharged by a build flag and no BNetzA procedure inspects a binary;
unbundling is decided above this repository.

```text
role-lf-strom   # LF answers to NB-initiated GPKE processes + LFN Strom bootstrap
role-lf-gas     # LF answers to NB-initiated GeLi Gas processes
role-nb-strom   # GPKE An-/Abmeldung STP, EoG gap closure, NB-answered MSB-Wechsel
role-nb-gas     # GeLi Gas An-/Abmeldung STP
role-msb        # REQOTE→QUOTES, §14a ORDRSP, MSB-answered MSB-Wechsel (beide Sparten)

lf-only     = role-lf-strom  + role-lf-gas
nb-only     = role-nb-strom  + role-nb-gas
msb-only    = role-msb
integrated  = every role (one operator holding several Marktrollen)
```

`tests/role_separation.rs` asserts the exclusion per build — an `nb-only` binary
that answered an LF or MSB PID fails the test rather than shipping. The MSB role
is its own feature because its work is its own: a Kündigung MSB (55039) is
MSBN → MSBA and never reaches the NB, and an ORDRSP is the MSB's answer.

---

## NB decision pipeline

```text
de.mako.process.initiated
  ├─ Anmeldung  (55001 / 55077 / 44001)          ─ E_0622 / G_0011
  │    → GET marktd /api/v1/versorgung/{malo_id}    → VersorgungsStatus
  │    → GET marktd /api/v1/malos/{malo_id}/grid    → MaloGridRecord
  │    → GET marktd /api/v1/partners/{lf_mp_id}     → partner_known
  │    → mako_pruefung::evaluate(…)
  └─ Abmeldung  (55004 / 44004)                  ─ EBD E_0607
       → GET marktd /api/v1/versorgung/{malo_id}    → VersorgungsStatus
       → mako_pruefung::evaluate_abmeldung(…)

  Accept   → anmeldung_decisions(Accept)   → makod bestaetigen   [if auto_accept]
                                           → approval_queue      [otherwise]
  Reject   → anmeldung_decisions(Reject)   → makod ablehnen (ERC)
  Escalate → anmeldung_decisions(Escalate) → approval_queue
```

**Anything the NB does not dispatch lands in the queue, with its deadline.**
`anmeldung_decisions` is the audit log and carries no Frist. Escalations, and Accepts held
back by `auto_accept = false` or the § 20 EnWG affiliate rule, go to `approval_queue` with
the answer Frist attached and both commands already resolved from the trigger PID.

### Three trees, three alphabets

`E_0622` Prüfschritt 10 splits Strom into two branches that share **no**
Antwortcode, and Gas answers from a different Codeliste again:

| Anwendungsfall | Tree | „andere Anmeldung in Bearbeitung" | Fristüberschreitung | Zustimmung |
|---|---|---|---|---|
| Strom, verbrauchende / ruhende MaLo | `E_0622` 15–70 | `A06` | `A07` | `A51` (`E_0623`) |
| Strom, erzeugende MaLo / Tranche | `E_0622` 220–830 | `A45` | `A34`/`A28`/`A29`/`A30`/`A32`/`A35`/`A44` | `A58` (`E_0623`) |
| Gas Anmeldung | `E_3005` / `G_0011` | `ZC5` | `E17` | `E15` (`G_0012`) |
| Strom Abmeldung | `E_0607` | — | `A02` | `A11` |
| Gas Abmeldung | `E_3019` / `G_0007` | — | `E17` | `E15` |

`A02` is „nimmt nicht an der Marktkommunikation teil" in `E_0622` and
„Vorlauffrist nicht eingehalten" in `E_0607`, and the Gas trees define neither.
`mako-pruefung` resolves every code inside its own tree, `makod` re-checks it at
the command boundary, and the published **Cluster decides the response PID**.

### `mako_pruefung::evaluate` — the Anmeldung

**Strom, verbrauchende / ruhende Marktlokation** (`E_0622` Prüfschritte 15–70):

| Prüfschritt | Rule | Outcome on failure |
|---|---|---|
| — | Grid record present in `marktd` | `Escalate` — a mako data gap, not a ground to refuse |
| 15 | Vorlauffrist: one full Werktag between receipt and Zuordnungsbeginn | `Reject A07` |
| 30 | MaLo participates in MaKo (stillgelegt, or Modell-2-zugeordnet) | `Reject A02` |
| 60 | Zuordnungsermächtigung: Bilanzierungsgebiet matches, LF known | `Reject A05` |
| 70 | No other Anmeldung in Bearbeitung | `Reject A06` |

A **ruhende** Marktlokation is not refused — Prüfschritt 30's own Hinweis names
only stillgelegte Marktlokationen and the Modell-2-Zuordnung, and Prüfschritte
16–28 exist to check a ruhende one.

Prüfschritt 70 is only decidable because `marktd` projects the assignments as a
**list** and keeps every pending announcement: the Anmeldung under evaluation is
written while ingesting the `process.initiated`, before the fan-out, so the
check must look for an announcement by a supplier *other than* the requesting
one rather than test for presence.

**Strom, erzeugende Marktlokation** (220–830) picks between the **six**
Vorlauffristen GPKE Teil 2 § 2.1.1 publishes, keyed on `(Geschäftsvorfall,
bestehende, angemeldete Veräußerungsform)`. The *Geschäftsvorfall* is `SG4 STS+7`
DE 9013 element 3 — `ZW0` (die ganze Marktlokation), `ZW1` (eine bestehende
Tranche), `ZW2` (eine neu zu bildende Tranche) — and the *angemeldete*
Veräußerungsform is `SG10 CCI+Z22`; the *bestehende* one and the
Ausfallvergütung flag come from `einsd` (`[nb] einsd_url`), because wire code
`Z90` covers two regimes whose Fristen differ by a month versus five Werktage.
A missing fact escalates and is named. The Monatserster rule is
**§ 21b Abs. 1 EEG 2023**, not § 10c.

A `ZW2` also makes the **Tranchengröße** Muss: `SG8` Produkt-Code
`9991000002090` with the Produkteigenschaft „prozentuale Aufteilung", which is
the share `E_0623` Prüfschritt 520 measures the released Tranchen against. The
product's other two Eigenschaften — an Aufteilungsfaktor and an Aufteilung auf
Technische Ressourcen — are different quantities, so an Anmeldung stating one
escalates rather than being measured against a number that is not a share.

**Gas** (`G_0011`) runs the `A03`/`A04`/`A16`/`A17` identification checks first as
the AHB requires, then `E17`, `E13`, `ZC5` / `Z08`.

### `mako_pruefung::evaluate_neuanlage` — `E_0608`, and its third outcome

Inbound **55600** / **55601**. Prüfschritte 110 / 590 loop: a Marktlokation the
NB cannot yet identify is re-checked **daily for 60 Werktage** and may only then
be refused `A07` / `A16`, which is why the answer window is 00:00 Uhr des 61. WT.
`processd` keeps a case log (`neuanlage_faelle`) and a daily worker rather than
deciding once, and counts the Prüfungen as evidence the obligation was met.

| Endpoint | Purpose |
|---|---|
| `GET /api/v1/neuanlage?status=offen` | The Prüflauf view, with `letzter_pruefungstag` and `pruefungen` per case |
| `PUT /api/v1/neuanlage/{id}/identifikation` | The MaLo-ID the NB matched from its NIS/GIS — a Neuanlage carries address and device data, not an ID |

### `mako_pruefung::evaluate_abmeldung` — `E_0607` / `E_3019`

| Prüfschritt | Rule | Outcome on failure |
|---|---|---|
| — | The MaLo is known to this NB | `Escalate` |
| 110 | The requesting LF is the assigned Lieferant | `Escalate` |
| 50 | Vorlauffrist — Strom: one full Werktag, or Monatserster + 1 Monat for an EEG-MaLo; Gas: the Kap. 3.2.1 retroactivity rules | `Reject A02` (Strom) / `E17` (Gas) |
| 120 | Kein bereits bestätigtes Lieferende zum selben Datum | `Reject A09` (Strom) / `Z08` (Gas) |
| 130 | Did the *already confirmed* Abmeldung name an Auszugsgrund? | `Escalate` — the projection does not keep it, and `A10` and „confirm" are both live outcomes |

Prüfschritte 10–30 (Kundenanlagen-Herauslösung) and 60–90 (ESV-Ende, Aufhebung
einer zukünftigen Zuordnung) need Transaktionsgründe and prior process history
the projection does not carry; they escalate rather than guess. Escalation is
the § 20 EnWG-safe direction — an unfounded Ablehnung keeps a customer bound to
a supplier they have left.

STP is ~60 % without `malo_grid` coverage (missing records escalate) and ≥ 95 %
once the NB has provisioned it via `marktd`'s NB-role PUT. An erzeugende
Marktlokation additionally needs `[nb] einsd_url`; without it every 55077
escalates.

**§ 20 EnWG parity.** Every decision row carries `initiator_is_affiliate`
(`lf_mp_id == own_mp_id`), and `auto_accept` is suppressed for affiliate-initiated
An- *and* Abmeldungen so an affiliate never gets a faster automatic path than a
third party.

## LF decision pipeline

```text
de.mako.process.initiated (55007 | 55010 | 55016 | 55607 | 44007 | 44010 | 44016)
  → GET marktd /api/v1/versorgung/{malo_id}
  → evaluate against own_mp_id
      supplying + scenario "standard"          → Bestätigung   (55008 / 55011)
      supplying + scenario "einzug"            → Ablehnung A32 (55009 / 55012)
      supplying + scenario "vertragsbindung"   → Ablehnung A35
      supplying + scenario "ersatzversorgung"  → Bestätigung
      MaLo unknown / not supplying / LF mismatch → approval_queue
```

"supplying" is `Beliefert`, `Grundversorgung` or `Ersatzversorgung` — all three
are a supply this LF may be asked to end. Six processes share this evaluation and
differ only in the command pair, the EBD and the Frist. The same business process
carries the same command name in both Sparten: 55007 / 44007
(`{gpke,geli}.nb-lieferende.*`, `E_0609` / `E_3002`), 55010 / 44010
(`{gpke,geli}.beendigung-zuordnung.*`, `E_0624` / `E_3020`) and 55016 / 44016
(`{gpke,geli}.kuendigung.*`, `E_0614` / `E_3001`).

**55607's shape is different.** It is not about ending a supply but about the NB
assigning this LF to an *erzeugende* Marktlokation or Tranche, and the substance
of the answer is the **Bilanzkreis**, not the code — `A01` / `A99` are the only
two the trees publish. Which of the four Anwendungsfälle applies is on the wire:
`SG4 STS+7` DE 9013 element 3, `ZW8`…`ZX1`, mapped onto `E_0603`–`E_0606` by
AHB Bedingungen [161]–[164]. The Bilanzkreis rides the Produktpaket
(`SG8 SEQ+Z79`, Produkt-Code `9991000002082`), never `FTX+ACB`.

Missing the 15:00 window does not lapse the obligation: GPKE Teil 2 § 2.4.2.2
Nr. 3 has the NB assign the LF anyway, using whichever BK it has on file. The
admissible set is `[[lf.bilanzkreise]]`, keyed on the Bilanzierungsgebiet.

**55013 / 44013 is the ninth, and the only Anmeldung.** Its Zustimmung accepts a
statutory § 36 / § 38 EnWG supply duty for a customer this deployment has no
contract with, so it takes a second opt-in: the walk always runs and queues its
outcome with the Frist, and only `[lf] eog_auto_respond = true` dispatches it.
`E_0615` Prüfschritt 20 („Grundversorgungsgebiet des Empfängers?") is answered
from `[lf] grundversorgungs_netzgebiete`; an empty list escalates.

---

## Authorization

Every REST route takes a bearer token and checks a Cedar action
(`policies/processd.cedar`). processd does not merely report decisions, it makes
them: approving a queue entry dispatches the market answer, and `start-supply` /
`end-supply` commit the operator to a market position.

| Action | Requirement | Routes |
|---|---|---|
| `read-decisions` · `read-queue` · `read-eog` · `read-neuanlage` | any principal of the tenant | the four `GET` logs |
| `decide-queue` | `NB`, `LF` or `MSB` role | `POST /api/v1/queue/{id}/approve\|reject` |
| `identify-neuanlage` | `NB` role | `PUT /api/v1/neuanlage/{id}/identifikation` |
| `initiate-supply` | `LF` role | `POST /api/v1/{start,end}-supply[-gas]` |
| `use-mcp` | any principal of the tenant | `/mcp` |

The deciding principal's `sub` is recorded twice: in `approval_queue.decided_by` and on
the dispatched command (`approved_by` / `rejected_by`). § 20 EnWG parity evidence and the
GoBD trail both have to say *who* decided, and the command alone answers neither for an
entry that expired unanswered.

`tests/authorization_guard.rs` pins the surface: every action checked in code is
permitted by policy (an unpermitted one is a permanent 403), every permitted
action is checked somewhere, and every routed handler takes a `Claims` extractor.

---

## REST API

| Method | Path | Cedar action | Description |
|---|---|---|---|
| `POST` | `/webhook` | *(HMAC)* | Inbound `marktd` CloudEvent |
| `GET` | `/api/v1/decisions` | `read-decisions` | Recent Anmeldung STP decisions |
| `GET` | `/api/v1/queue` | `read-queue` | Approval-queue entries |
| `POST` | `/api/v1/queue/{id}/approve` · `/reject` | `decide-queue` | Decide an entry; dispatches the market answer stored on it |
| `POST` | `/api/v1/start-supply` · `-gas` | `initiate-supply` | LFN bootstrap (Strom / Gas). Both require a **Bilanzkreis** — Strom as the `SG8 SEQ+Z79` Produktpaket, Gas as `SG10 CCI+Z19` — falling back to a single configured `[[lf.bilanzkreise]] standard` entry, else `422` |
| `POST` | `/api/v1/end-supply` · `-gas` | `initiate-supply` | LF Lieferende bootstrap |
| `GET` | `/api/v1/eog` | `read-eog` | EoG case log (`?status=`) |
| `GET` | `/api/v1/neuanlage` | `read-neuanlage` | `E_0608` case log (`?status=`), with each case's `letzter_pruefungstag` |
| `PUT` | `/api/v1/neuanlage/{id}/identifikation` | `identify-neuanlage` | Record the MaLo-ID the NB matched from address/device data |
| `GET` | `/health/live` · `/health/ready` · `/metrics` | — | Mounted by `mako_service::run` |
| `GET\|POST` | `/mcp` | `use-mcp` | MCP Streamable HTTP |

---

## Configuration

Loaded by `mako_service::load_config`: `processd.toml` first (path from
`PROCESSD_CONFIG`, default `./processd.toml`), then `PROCESSD_*` environment
variables with `__` as the section separator, then any `*_FILE` variable read
from a file. The file is optional — a container can be configured entirely from
the environment.

```toml
[http]
addr = "0.0.0.0:8580"

[database]
url       = "env:DATABASE_URL"
pool_size = 10

[identity]
own_mp_id = "9900000000002"   # BDEW 99… / DVGW 98…; must match makod's primary party
# tenant defaults to own_mp_id

[makod]
url     = "http://makod:8080"
api_key = "env:MAKOD_API_KEY"

[marktd]
url     = "http://marktd:8180"
api_key = "env:MARKTD_API_KEY"   # also authenticates the subscription self-registration

[webhook]
inbound_secret = "env:INBOUND_WEBHOOK_SECRET"   # must match the marktd subscription secret

[subscription]
webhook_url   = "http://processd:8580/webhook"
subscriber_id = "processd"

[nb]
auto_accept              = false  # enable only after verifying grid + partner coverage
gas_bearbeitungsfrist_wt = 3      # AWH GeLi Gas 2.0 Kap. 2.2
einsd_url                = ""     # EEG-/KWKG-Register; without it every 55077 escalates
einsd_api_key            = ""

[lf]
auto_respond = true             # false → every inbound LF process goes to the queue

# The Bilanzkreise a 55607 Zustimmung may name (MaBiS § 10.2.1 grants the
# Zuordnungsermächtigung je ZRT, BG, BK und LF). One BK per regime answers
# automatically; several is an operator choice. A row without a
# `bilanzierungsgebiet` is the fallback.
[[lf.bilanzkreise]]
bilanzierungsgebiet = "11YN-BG-EON---X"
eeg      = ["11XBK-EEG-----1"]
kwkg     = ["11XBK-KWKG----5"]
standard = ["11XBK-STD-----9"]

[msb]
auto_accept       = false       # false → MSB-Wechsel Accepts go to the queue
auto_preisanfrage = true        # false → the REQOTE goes to the queue for an operator

[esa]                              # WiM Teil 2 Kap. 4 — the MSB's answers to an ESA
auto_accept = false                # true → dispatch the E_0254/E_0256/E_0257 Zustimmungscode
auto_reject = false                # true → dispatch a deterministic Ablehnungscode
accept_after_bindungsfrist = false # E_0256 Prüfschritt 2 — a commercial decision

[eog]
auto_activate           = false
warn_days_before_expiry = 14
# notify_webhook_secret = "env:EOG_WEBHOOK_SECRET"

# [oidc]   omit only in dev — without it every request gets synthetic dev claims
# [otel]   omit to disable tracing export
```

Equivalent environment overrides:

```bash
PROCESSD_CONFIG=/etc/processd/processd.toml
PROCESSD_DATABASE__URL=postgres://processd:secret@postgres/processd
PROCESSD_IDENTITY__OWN_MP_ID=9900000000002
PROCESSD_MAKOD__API_KEY_FILE=/run/secrets/makod-api-key
PROCESSD_NB__AUTO_ACCEPT=true
PROCESSD_LOG_LEVEL=info
```

### Self-registration with `marktd`

When `[subscription] webhook_url` is set, `processd` upserts
`PUT {marktd}/api/v1/subscriptions/{subscriber_id}` on startup, authenticated
with `[marktd] api_key`, retrying for 30 s to tolerate startup ordering. A
`401`/`403` fails immediately with the reason rather than retrying — that
principal needs `manage-subscription` on `marktd` (ADMIN role).

---

## Database

| Table | Purpose |
|---|---|
| `anmeldung_decisions` | NB STP audit log for both An- and Abmeldung decisions — `UNIQUE (process_id, tenant)`, so an at-least-once redelivery does not double-record |
| `approval_queue` | Entries awaiting an operator — from **every** compiled role — with the market command to dispatch on approve/reject, the business Frist they expire at, and `decided_by` once decided. A background worker expires `Pending` rows past `expires_at`; it is deliberately **not** role-gated, because the NB, LF and MSB all enqueue |
| `eog_activations` | EoG case log; the daily § 38 Abs. 4 timer scans it |
| `neuanlage_faelle` | `E_0608` case log; the daily Prüflauf re-runs the tree per open case and counts the Prüfungen |

Migrations run at startup from `migrations/0001_initial.sql`.

---

## Metrics

On the shared `/metrics` (registered on the same Prometheus registry
`mako_service::run` serves):

| Metric | Type | Meaning |
|---|---|---|
| `processd_decisions_total{decision,pid}` | counter | STP decisions; the ≥ 95 % target is `rate(…{decision="Accept"}) / rate(…)` |
| `processd_approval_queue_pending` | gauge | Processes waiting for an operator |
| `processd_approval_queue_overdue` | gauge | Pending entries past `expires_at` — **alert on > 0** |
| `processd_eog_open` | gauge | Unclosed Ersatz-/Grundversorgung cases |

---

## Quick start

```bash
# NB-only deployment — no LF or MSB decision path registered
PROCESSD_DATABASE__URL=postgres://processd:secret@postgres/processd \
PROCESSD_IDENTITY__OWN_MP_ID=9900000000002 \
PROCESSD_MAKOD__URL=http://makod:8080 \
PROCESSD_MAKOD__API_KEY=<key> \
PROCESSD_MARKTD__URL=http://marktd:8180 \
PROCESSD_MARKTD__API_KEY=<key> \
cargo run -p processd --no-default-features --features nb-only

# Pure LF / pure MSB
cargo run -p processd --no-default-features --features lf-only
cargo run -p processd --no-default-features --features msb-only

# Integrated §6b EnWG
cargo run -p processd --no-default-features --features integrated
```

---

## Testing

```bash
just ci                                  # full workspace gate, incl. the role matrix
cargo test -p processd --features integrated
just test-processd-db                    # SQL suite against a real PostgreSQL
```

---

## Regulatory basis

| Obligation | Frist | Source |
|---|---|---|
| GPKE Anmeldung decision (55001 / 55077) | 11:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6 |
| GPKE Abmeldung decision (55004) | 06:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 GPKE Teil 2, SD Lieferende von LF an NB Prozessschritte 2/3 |
| GPKE LF answer (55007 → 55008/55009) | 05:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 GPKE Teil 2, SD Lieferende von NB an LF Prozessschritt 2 |
| GPKE LFA answer (55010 → 55011/55012) | 09:00 Uhr des 1. WT nach dem ÜT | BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritt 4 |
| GeLi Gas Anmeldung decision (44001) | Ablauf des 4. Werktags nach Eingang | BK7-24-01-009 Kap. 3.2.3 |
| GeLi Gas Abmeldung decision (44004) | Ablauf des 3. Werktags nach Eingang | BK7-24-01-009 Kap. 3.2.2 |
| WiM MSB-Wechsel answers | 3 / 5 / 7 / 1 WT per PID | BK6-22-024 WiM Strom Teil 1, Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2 |
| WiM REQOTE Preisanfrage | 4 / 5 / 10 WT per PID (35001 / 35002 / 35005) | BK6-22-024 WiM Strom Teil 1, Kap. 3.2.2 / 3.6.3.6.2 / 3.3.1.2 Nr. 2 |
| WiM Rechnungsabwicklung über den LF (15002 / 17006) | 8 Werktage | BK6-22-024 WiM Strom Teil 1, Kap. 3.6.3.4.2 / 3.6.3.5.2 Nr. 2 |
| WiM-Rechnung Antwort (REMADV) | zum Zahlungsziel; NB bei 31009: 4. WT davor | BK6-22-024 WiM Strom Teil 1, Kap. 3.6.3.8.2 / 3.7.2 / 6.2 |
| APERAK technical acknowledgement | 45 min (Strom UTILMD) | APERAK AHB 1.0 § 2.4.1 — answered by `makod` |
| § 38 Abs. 4 Ersatzversorgung maximum | 3 months from Zuordnungsbeginn | EnWG |
| § 20 EnWG parity audit | provable at BNetzA | `initiator_is_affiliate` |

Every Frist is read from one table, `mako_fristen::antwort`, by both
`processd` (to size the operator queue) and `makod` (to register the process
deadline), so the two can never disagree about whether an obligation was met. GeLi Gas Werktage are
counted from the first Werktag after receipt, per § 187 Abs. 1 BGB as GeLi Gas
3.0 Kap. 2.6 restates it.

All deadline arithmetic uses **German local time (CET/CEST)** — a Frist stated
as „11:00 Uhr" is 10:00 UTC in winter and 09:00 UTC in summer. `mako-pruefung`
receives `now_utc()` and converts internally via `time-tz`.
