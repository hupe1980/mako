# processd — Process Decision Engine

**Automated market-process decisions, inside the regulatory deadline, without ERP involvement.**

`processd` subscribes to `marktd`'s durable fan-out, and for each
`de.mako.process.initiated` applies the policy of the role that owes the answer.
What it cannot decide it puts in front of an operator with the deadline attached.

---

## What answers what

| Role | Inbound PID | Process | Business Frist | Decision basis |
|---|---|---|---|---|
| **NB** | 55001, 55077 | GPKE Anmeldung (verb. / erz. MaLo) | **11:00 Uhr des 1. WT nach dem ÜT** | `netz-checker::evaluate` (6 checks, EBD `E_0622`) |
| **NB** | 55004 | GPKE Abmeldung — Lieferende von LF an NB | **06:00 Uhr des 1. WT nach dem ÜT** | `netz-checker::evaluate_abmeldung` (EBD `E_0607`) |
| **NB** | 44001 | GeLi Gas Anmeldung NN | **Ablauf des 4. Werktags** | `netz-checker::evaluate` |
| **NB** | 44004 | GeLi Gas Abmeldung NN | **Ablauf des 3. Werktags** | `netz-checker::evaluate_abmeldung` |
| **NB** | 55042 | WiM Anmeldung MSB (MSBN → NB) | 5 WT | MeLo / partner / Zählertyp checks |
| **NB** | 55051 | WiM Ende MSB (MSBA → NB) | 7 WT | operator queue |
| **NB** | *(event)* | EoG gap closure → 55013 (§ 36/§ 38 EnWG) | unverzüglich; 3-month timer | `grundversorger` from `marktd` |
| **LF** | 55007 | Ankündigung NB-seitiges Lieferende (EBD `E_0609`) | **05:00 Uhr des 1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` |
| **LFA** | 55010 | Anfrage zur Beendigung der Zuordnung (EBD `E_0624`) | **09:00 Uhr des 1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` |
| **MSB** | 55039 | WiM Kündigung MSB (MSBN → MSBA) | 3 WT | MeLo / partner checks |
| **MSB** | 55168 | WiM Verpflichtungsanfrage (NB → gMSB) | 1 WT | operator queue |
| **MSB** | 35001, 35002, 35004, 35005 | REQOTE Preisanfrage | 5 WT | `PreisblattMessung` from `marktd` |
| **MSB** | *(workflow)* | § 14a Steuerungsauftrag → ORDRSP | 5 WT | contracted `konfigurationsprodukte` |

Four properties of that table, each of which fails silently when broken:

- **The PID is the inbound trigger, never an answer.** `makod` emits
  `process.initiated` with the `makopid` of the message that *spawned* the
  process, so a module keyed on 55008 (the LF's answer to 55007) waits forever.
  `tests/pid_contract.rs` pins every trigger against the canonical `mako-gpke` /
  `mako-wim` constants.
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

`src/fristen.rs` reads the same tables `makod` registers the process deadline
from, so the two cannot disagree about whether an obligation was met.

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
Answering it from an `nb-only` binary would breach the § 7 EnWG separation the
Cargo features exist for. `concepts/ROADMAP.md` records what the LFA answer path
needs first.

## § 7 EnWG role separation

An operator with ≥ 100 000 Netzkunden runs the roles as separate entities, and
the BNetzA audit examines the deployed binary. The Cargo features are that
separation:

```text
role-lf-strom   # LF answers to NB-initiated GPKE processes + LFN Strom bootstrap
role-lf-gas     # LF answers to NB-initiated GeLi Gas processes
role-nb-strom   # GPKE An-/Abmeldung STP, EoG gap closure, NB-answered MSB-Wechsel
role-nb-gas     # GeLi Gas An-/Abmeldung STP
role-msb-strom  # REQOTE→QUOTES, §14a ORDRSP, MSB-answered MSB-Wechsel

lf-only     = role-lf-strom  + role-lf-gas
nb-only     = role-nb-strom  + role-nb-gas
msb-only    = role-msb-strom
integrated  = every role (§6b EnWG combined deployment)
```

`tests/role_separation.rs` asserts the exclusion per build — an `nb-only` binary
that answered an LF or MSB PID fails the test rather than shipping. The MSB role
is its own feature because its work is its own: a Kündigung MSB (55039) is
MSBN → MSBA and never reaches the NB, and an ORDRSP is the MSB's answer.

---

## NB decision pipeline

```text
de.mako.process.initiated
  ├─ Anmeldung  (55001 / 55077 / 44001)          ─ EBD E_0622
  │    → GET marktd /api/v1/versorgung/{malo_id}    → VersorgungsStatus
  │    → GET marktd /api/v1/malos/{malo_id}/grid    → MaloGridRecord
  │    → GET marktd /api/v1/partners/{lf_mp_id}     → partner_known
  │    → netz_checker::evaluate(…)
  └─ Abmeldung  (55004 / 44004)                  ─ EBD E_0607
       → GET marktd /api/v1/versorgung/{malo_id}    → VersorgungsStatus
       → netz_checker::evaluate_abmeldung(…)

  Accept   → anmeldung_decisions(Accept)   → makod bestaetigen   [if auto_accept]
                                           → approval_queue      [otherwise]
  Reject   → anmeldung_decisions(Reject)   → makod ablehnen (ERC)
  Escalate → anmeldung_decisions(Escalate) → approval_queue
```

**Anything the NB does not dispatch lands in the queue, with its deadline.**
`anmeldung_decisions` is the audit log and carries no Frist. Escalations, and Accepts held
back by `auto_accept = false` or the § 20 EnWG affiliate rule, go to `approval_queue` with
the answer Frist attached and both commands already resolved from the trigger PID.

### The two decision trees have separate code spaces

`A02` is „Marktlokation nimmt nicht an der Marktkommunikation teil" in `E_0622`
and „Vorlauffrist nicht eingehalten" in `E_0607`, so `netz-checker` exposes two
functions rather than one with a flag. Reusing the Anmeldung codes on an
Abmeldung puts a valid-looking but wrong Ablehnungsgrund on the market.

### netz-checker `evaluate` — the Anmeldung, 6 deterministic checks

| # | Rule | Outcome on failure |
|---|---|---|
| 1 | Grid record present in `marktd` | `Escalate` |
| 2 | MaLo participates in MaKo (not Stillgelegt/Ruhend) | `Reject A02` |
| 3 | No conflicting Anmeldung in Bearbeitung (`lf_mp_id_next` held by another LF) | `Reject A06` |
| 4 | Date plausibility, Transaktionsgrund-aware — Strom LFW24 future rule, § 10c EEG Monatserster for 55077; Gas E03 ≥ 10 WT future-only, E01/E02 retroactive ≤ 6 weeks (+3 WT) for SLP | `Reject A07` (Strom) / `Reject E17` (Gas); Gas backdated without Transaktionsgrund → `Escalate` |
| 5 | Bilanzierungsgebiet consistent with the grid record | `Reject A05` |
| 6 | LF MP-ID in the partner directory | `Reject A05` |

Check 3 is only decidable because `marktd` keeps the **first** announcement:
`lf_mp_id_next` is written while ingesting the `process.initiated`, before the
fan-out, so the Anmeldung under evaluation has already written its own marker
and the check must compare MP-IDs rather than test for presence.

### netz-checker `evaluate_abmeldung` — the Abmeldung, EBD `E_0607`

| # | Prüfschritt | Outcome on failure |
|---|---|---|
| 1 | The MaLo is known to this NB | `Escalate` |
| 2 | The requesting LF is the assigned Lieferant (Prüfschritt 110) | `Escalate` |
| 3 | Vorlauffrist eingehalten (Prüfschritt 50) — Strom: one full Werktag, or Monatserster + 1 Monat for an EEG-MaLo; Gas: the Kap. 3.2.1 retroactivity rules | `Reject A02` |
| 4 | Kein bereits bestätigtes Lieferende zum selben Datum (Prüfschritte 100–130) | `Reject A09` / `A10` |

Prüfschritte 10–30 (Kundenanlagen-Herauslösung) and 60–90 (ESV-Ende, Aufhebung
einer zukünftigen Zuordnung) need Transaktionsgründe and prior process history
the projection does not carry; they escalate rather than guess. Escalation is
the § 20 EnWG-safe direction — an unfounded Ablehnung keeps a customer bound to
a supplier they have left.

STP is ~60 % without `malo_grid` coverage (missing records escalate) and ≥ 95 %
once the NB has provisioned it via `marktd`'s NB-role PUT.

**§ 20 EnWG parity.** Every decision row carries `initiator_is_affiliate`
(`lf_mp_id == own_mp_id`), and `auto_accept` is suppressed for affiliate-initiated
An- *and* Abmeldungen so an affiliate never gets a faster automatic path than a
third party.

## LF decision pipeline

```text
de.mako.process.initiated (55007 | 55010)
  → GET marktd /api/v1/versorgung/{malo_id}
  → evaluate against own_mp_id
      supplying + scenario "standard"          → Bestätigung   (55008 / 55011)
      supplying + scenario "einzug"            → Ablehnung A32 (55009 / 55012)
      supplying + scenario "vertragsbindung"   → Ablehnung A35
      supplying + scenario "ersatzversorgung"  → Bestätigung
      MaLo unknown / not supplying / LF mismatch → approval_queue
```

"supplying" is `Beliefert`, `Grundversorgung` or `Ersatzversorgung` — all three
are a supply this LF may be asked to end. The two processes share the evaluation
and differ in the command pair (`gpke.nb-lieferende.*` vs
`gpke.beendigung-zuordnung.*`), the EBD (`E_0609` vs `E_0624`) and the Frist.

---

## Authorization

Every REST route takes a bearer token and checks a Cedar action
(`policies/processd.cedar`). processd does not merely report decisions, it makes
them: approving a queue entry dispatches the market answer, and `start-supply` /
`end-supply` commit the operator to a market position.

| Action | Requirement | Routes |
|---|---|---|
| `read-decisions` · `read-queue` · `read-eog` | any principal of the tenant | the three `GET` logs |
| `decide-queue` | `NB`, `LF` or `MSB` role | `POST /api/v1/queue/{id}/approve\|reject` |
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
| `POST` | `/api/v1/start-supply` · `-gas` | `initiate-supply` | LFN bootstrap (Strom / Gas) |
| `POST` | `/api/v1/end-supply` · `-gas` | `initiate-supply` | LF Lieferende bootstrap |
| `GET` | `/api/v1/eog` | `read-eog` | EoG case log (`?status=`) |
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
auto_accept = false             # enable only after verifying grid + partner coverage

[lf]
auto_respond = true             # false → every inbound LF process goes to the queue

[msb]
auto_accept       = false       # false → MSB-Wechsel Accepts go to the queue
auto_preisanfrage = true        # false → the REQOTE goes to the queue for an operator

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
# NB-only deployment (§7 EnWG separated binary)
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
| WiM REQOTE Preisanfrage | 5 Werktage | BK6-24-174 (WiM Strom) |
| APERAK technical acknowledgement | 45 min (Strom UTILMD) | APERAK AHB 1.0 § 2.4.1 — answered by `makod` |
| § 38 Abs. 4 Ersatzversorgung maximum | 3 months from Zuordnungsbeginn | EnWG |
| § 20 EnWG parity audit | provable at BNetzA | `initiator_is_affiliate` |

Every Frist is read from one table per family — `mako_gpke::antwortfrist`,
`mako_geli_gas::antwortfrist`, `mako_wim` — by both `processd` (to size the
operator queue) and `makod` (to register the process deadline), so the two can
never disagree about whether an obligation was met. GeLi Gas Werktage are
counted from the first Werktag after receipt, per § 187 Abs. 1 BGB as GeLi Gas
3.0 Kap. 2.6 restates it.

All deadline arithmetic uses **German local time (CET/CEST)** — a Frist stated
as „11:00 Uhr" is 10:00 UTC in winter and 09:00 UTC in summer. `netz-checker`
receives `now_utc()` and converts internally via `time-tz`.
