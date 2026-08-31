# marktd — Market Data Hub

**The single source of truth for German energy market entity state. No domain policy.**

`marktd` is the companion to [`makod`](../makod): `makod` speaks EDIFACT over AS4,
`marktd` holds what the messages are *about* — Marktlokationen and Messlokationen, the
supply-status lifecycle, the device registry, the location graph, published price sheets,
counterparty channels, and the ERP webhook fan-out.

**Architecture principle:** `marktd` contains **no domain policy**. Automated NB An- and
Abmeldung STP decisions belong to [`processd`](../processd), which subscribes to the
fan-out. `marktd` emits events; `processd` reacts. That keeps `marktd` independently
testable and deployable with no decision logic in it.

The boundary is subtle in one place: `marktd` writes `lf_mp_id_next` from an inbound
Anmeldung *before* fanning the event out, so what it does with a **competing**
announcement decides whether `processd` can apply EBD `E_0622` Prüfschritt 70 at all —
see [The first announcement wins](#the-first-announcement-wins).

---

## VersorgungsStatus derivation

Inbound `makod` process events drive the supplier-transition lifecycle. Every write
appends to `versorgungsstatus_history` in the same transaction, so any past state is
retrievable with `?at=YYYY-MM-DD`, and **every** transition emits
`de.markt.versorgung.changed` carrying the state it produced.

| Event | PID | Operation | Effect |
|---|---|---|---|
| `de.mako.process.initiated` | 55001 / **55077** / 44001 | `announce_lf_next` | Sets `lf_mp_id_next` + `lf_next_lieferbeginn` (who and when). Does **not** change `lieferstatus`. |
| `de.mako.process.completed` | 55002 / **55078** / 44002 | `confirm_supply` | `lf_mp_id ← lf_mp_id_next`, `lieferbeginn ← lf_next_lieferbeginn`, `lieferstatus = Beliefert`, clears the announcement. A `lfa_lieferende` before the Zuordnungsbeginn is **Fall b** → `de.markt.versorgung.gap-detected` for the days between. |
| `de.mako.process.completed` | 55003 / **55080** / 44003 | `clear_lf_next` | Ablehnung Anmeldung — drops the announced future Lieferant so nothing acts on a switch that will not happen. |
| `de.mako.process.completed` | 55005 / 44005 | `end_supply` | `lieferstatus = Unbeliefert`, clears `lf_mp_id`, records the contractual `lieferende` from the process, preserves `lf_mp_id_next`; an uncovered interval → `de.markt.versorgung.gap-detected`. |
| `de.mako.process.completed` | 55013 / 44013 | `begin_eog_supply` | `lieferstatus = Ersatzversorgung`/`Grundversorgung`, `eog_seit` set (the **§ 38 Abs. 4 EnWG** three-month anchor); emits `de.markt.versorgung.eog-begonnen`. |

**55002 confirms and 55003 rejects** — "Bestätigung Anmeldung verb. MaLo" and "Ablehnung
Anmeldung verb. MaLo" respectively, per the EDI@Energy *Anwendungsübersicht der
Prüfidentifikatoren* 4.0, GPKE Teil 2, Prozessschritte 5 and 6.

**55077 is the erzeugende-Marktlokation twin of 55001**, answered 55078 / 55080 (55079 is
unassigned), and drives the identical projection — an EEG-/KWKG-MaLo's supplier change is
a supplier change.

### The first announcement wins

A second Anmeldung by a *different* supplier while one is pending does **not** overwrite
`lf_mp_id_next`, which is what makes EBD `E_0622` Prüfschritt 70 („Andere Anmeldung in
Bearbeitung", `A06`) decidable at all. `marktd` writes the marker while ingesting the
`process.initiated`, *before* fanning the event out, so the Anmeldung under evaluation has
already written its own MP-ID by the time `processd` checks — the check compares MP-IDs
rather than testing for presence, and overwriting would make that comparison always
succeed. The same supplier re-sending (a corrected date, an at-least-once redelivery)
still updates its own announcement.

A transition that changes nothing emits nothing: a redelivered Ablehnung against a MaLo
with no announcement is a no-op, with no version bump, no history row and no
`de.markt.versorgung.changed`.

### A supply gap is an interval, not an absence

`de.markt.versorgung.gap-detected` fires on an **uncovered interval**, and two
routes lead to one:

- a **Lieferende** whose successor starts later than the day after it — which
  § 38 Abs. 1 EnWG treats exactly like an open-ended gap;
- a **Bestätigung Anmeldung** in **Fall b**, where the Altlieferant answered the
  Abmeldeanfrage with its own earlier Lieferendedatum (`E_0624` `A34`) while the
  confirmation stands at the Zuordnungsbeginn the new supplier asked for.

Neither route's message states both ends — the answer states one and the
Anmeldung the other — so this projection is where they meet. The event carries
`gap_from` and `gap_until` (`null` only for an open-ended gap).

---

## At a glance

| Feature | Detail |
|---|---|
| **HTTP port** | `:8180` |
| **Lifecycle** | `mako_service::run` — tracing, tuned pool, migrations, real readiness, `/health/*`, `/metrics`, HTTP trace layer, graceful SIGINT **and SIGTERM** |
| **Database** | PostgreSQL 15+ (sqlx 0.8). Requires the `btree_gist` extension (created by the migration) |
| **Auth** | OIDC/JWT (RS256 / ES256 / PS256), JWKS background refresh |
| **Authorization** | Cedar ABAC (`policies/marktd.cedar`) — per-tenant, role-gated, coverage-tested |
| **API spec** | OpenAPI 3.1 — Swagger UI at `/api/v1/docs/`, spec at `/api/v1/openapi.json` (the same pair `makod` serves) |
| **Events** | Outbound CloudEvents 1.0 (`application/cloudevents+json`) + HMAC-SHA256, durable two-phase fan-out |
| **Typed BO4E API** | `GET` responses are canonical `rubo4e::current` types — `Marktlokation`, `Messlokation`, `Zaehler`, `Geraet`. Every `PUT` validates `_typ` and rejects out-of-schema enum values (422). |
| **Timestamps** | RFC 3339 throughout (`2026-01-01T00:00:00Z`); wall-clock tariff windows are `HH:MM:SS` |
| **Event source** | `urn:mako:marktd:tenant:{tenant}` |
| **CE extensions** | `marktrole`, `marktsparte`, `marktmaloid`, `marktmeloid`, `markterpref` |
| **Body limit** | 2 MiB per request |
| **MCP** | Read-only, at `/mcp` |

---

## Configuration

Loaded by `mako_service::load_config`: `marktd.toml` first (path from `MARKTD_CONFIG`,
default `./marktd.toml`), then `MARKTD_*` environment variables with `__` as the section
separator, then any `*_FILE` variable read from a file. **The file is optional** — a
container can be configured entirely from the environment.

```toml
[database]
url             = "env:DATABASE_URL"
pool_size       = 20
min_connections = 2

[http]
addr = "0.0.0.0:8180"

[markt]
# This deployment's own operator identity: the `resource_tenant` every Cedar check
# compares the caller's `mako_tenant` claim against, the `tenant` column on
# tenant-scoped rows, and the source URN of every outbound CloudEvent.
tenant = "9900357000004"

[oidc]
issuer            = "https://auth.example.com"
audience          = "marktd"
jwks_refresh_secs = 3600

[makod]
base_url = "http://makod:8080"
api_key  = "env:MAKOD_API_KEY"

[webhook]
inbound_path          = "/api/v1/mako/events"
inbound_secret        = "env:MAKOD_WEBHOOK_SECRET"
delivery_timeout_secs = 10
max_retry_attempts    = 3

[mmma_import]
enabled   = true
gas_url   = "https://www.tradinghub.eu/mmma-preise.csv"   # or file:///path/to/prices.csv
strom_url = "https://www.bdew.de/…/Mehr-Mindermengen-Preise-Strom.csv"
check_hour_utc = 6

[otel]
endpoint     = "http://otel-collector:4317"
service_name = "marktd"

[mcp]
path = "/mcp"
```

Equivalent environment overrides:

```bash
MARKTD_CONFIG=/etc/marktd/marktd.toml
MARKTD_DATABASE__URL=postgres://marktd:secret@postgres/marktd
MARKTD_MARKT__TENANT=9900357000004
MARKTD_MAKOD__API_KEY_FILE=/run/secrets/makod-api-key   # contents become the value
MARKTD_LOG_LEVEL=info
```

### Fail-closed startup

Without `[oidc]` every request is admitted with synthetic dev claims, and without
`webhook.inbound_secret` the inbound events endpoint accepts unsigned events that mutate
VersorgungsStatus and the device registry. Startup **refuses** when either is missing
unless `allow_insecure_no_auth = true` is set — both postures have to be asked for by name.

---

## Authorization

Every endpoint except `/health/*`, `/metrics` and the HMAC-authenticated inbound events
path takes a bearer token and checks a Cedar action.

| Tier | Actions | Requirement |
|---|---|---|
| Read | `read-malo`, `read-melo`, `read-partner`, `read-preisblatt`, `read-versorgungsstatus`, `read-device`, … | Any authenticated principal of the tenant |
| Write | `write-malo`, `write-melo`, `write-partner`, `write-device`, `write-bilanzierung`, … | Any authenticated principal of the tenant |
| NB authority | `write-preisblatt`, `write-nelo`, `write-tranche`, `write-malo-grid`, `write-grundversorger`, `write-energiemix`, `write-mmma-preis`, `dispatch-pricat` | `NB` role — data the Netzbetreiber publishes and other services treat as authoritative |
| ESA consent | `read-einwilligung`, `write-einwilligung` | `MSB` or `ESA` role — § 49 Abs. 2 Nr. 9 MsbG; only the two parties to the relationship |
| Operator | `manage-subscription`, `manage-fanout` | `ADMIN` role — a subscription is an outbound export channel for every event this hub emits |

`tests/authorization_guard.rs` pins the surface in four ways: every action checked in code
is permitted by the policy (a missing one is a permanent 403, not a compile error), every
permitted action is checked somewhere, every handler module takes a `Claims` extractor, and
no handler extracts `Claims` as a request `Extension` (nothing inserts it, so that is a
guaranteed 500).

---

## REST API

### Marktlokation

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/malos` | List Marktlokationen (paginated, filterable) |
| `GET`/`PUT` | `/api/v1/malos/{id}` | Fetch / upsert a MaLo + Rollenzuordnung |
| `GET` | `/api/v1/malos/{id}/lastprofil` | Derived SLP profile (NNE tariff zone, `billingd`) |
| `GET`/`PUT` | `/api/v1/malos/{id}/grid` | NB grid topology (read by the `processd` NB module) |
| `GET`/`PUT` | `/api/v1/malos/{id}/bilanzierung` | BO4E `Bilanzierung` — temporal balancing resource |
| `GET` | `/api/v1/malos/{id}/bilanzierung/history` | All Bilanzierung versions |
| `GET` | `/api/v1/malos/{id}/lokationen` | Reachable location graph from this MaLo |
| `GET` | `/api/v1/malos/{id}/buendel` | Lokationsbündel members |
| `GET` | `/api/v1/malos/{id}/technische-ressourcen` | TRs linked to this MaLo |
| `GET`/`PUT` | `/api/v1/versorgung/{malo_id}` | VersorgungsStatus — `?at=YYYY-MM-DD` for point-in-time |
| `GET` | `/api/v1/versorgung/{malo_id}/history` | Supply-state change history (newest first, paged) |

### Messlokation and devices

| Method | Path | Description |
|---|---|---|
| `GET`/`PUT` | `/api/v1/melos/{id}` | Fetch / upsert a MeLo |
| `GET` | `/api/v1/melos/{id}/standorteigenschaften` | BO4E `Standorteigenschaften` |
| `GET`/`PUT` | `/api/v1/melos/{id}/msb` | Dated MSB assignment (WiM Teil 2 UC 4.1.1) |
| `GET` | `/api/v1/melos/{id}/msb/history` | Full MSB timeline |
| `GET` | `/api/v1/melos/{id}/zaehler` | Meters at this MeLo |
| `GET` | `/api/v1/melos/{id}/sharing-eligibility` | § 42c Energy-Sharing master-data verdict |
| `GET` | `/api/v1/melos/{id}/lokationen` | Reachable location graph from this MeLo |
| `PUT` | `/api/v1/zaehler/{id}` | Upsert a Zähler |
| `GET` | `/api/v1/zaehler/{id}/geraete` · `/geraete/{geraet_id}` | Devices on a meter |
| `GET`/`PUT` | `/api/v1/zaehler/{id}/geraete/{geraet_id}/konfigurationen` | Typed device configuration (MsbG § 23, BSI TR-03109) |
| `GET` | `/api/v1/zaehler/{id}/zaehlwerke` | Register definitions |
| `GET`/`PUT` | `/api/v1/zaehler/{id}/register` | ZaehlzeitRegister (iMSys ToU) |
| `GET` | `/api/v1/zaehler/{id}/zaehlzeitdefinitionen` | BO4E `Zaehlzeitdefinition` projection |
| `GET` | `/api/v1/zaehler/{id}/tariff-zone` | Resolve HT/NT/EINZEL at an instant |
| `GET`/`PUT` | `/api/v1/zaehler-register/{register_id}/saisons` | ToU windows (`HH:MM:SS`, ISO weekdays) |
| `PUT` | `/api/v1/geraete/{geraet_id}` | Upsert a Gerät |
| `GET`/`PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}` | SteuerbareRessource (§ 14a iMS) |
| `GET`/`PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` | Contracted control products (`produktcode` mandatory — the § 14a Konfigurationsprodukt of **BK6-22-300**; BK6-24-174 is GPKE) |
| `DELETE` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}` | Remove one product |
| `GET`/`PUT` | `/api/v1/technische-ressourcen/{tr_id}` | TechnischeRessource (E-mobility, generation, storage) |
| `PUT` | `/api/v1/lokationszuordnungen` | Upsert a graph edge |
| `DELETE` | `/api/v1/lokationszuordnungen/{von_id}/{nach_id}` | Remove a graph edge |

### Counterparties, contracts and registries

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/partners` | List trading partners |
| `GET`/`PUT` | `/api/v1/partners/{mp_id}` | Fetch / upsert a partner |
| `GET` | `/api/v1/partners/{mp_id}/as4-address` | AS4 endpoint (`Marktteilnehmer.makoadresse`) |
| `GET` | `/api/v1/partners/{mp_id}/marktteilnehmer` | Typed BO4E `Marktteilnehmer` view |
| `GET` | `/api/v1/nb-contracts` · `GET`/`PUT` `/api/v1/nb-contracts/{id}` | NB network contracts (full BO4E `Vertrag`) |
| `GET`/`PUT` | `/api/v1/grundversorger/{nb_mp_id}` | § 36 Abs. 2 EnWG Feststellung (`?sparte=STROM\|GAS`) — read by the `processd` EoG closure |
| `GET`/`PUT` | `/api/v1/energiemix/{nb_mp_id}` · `GET` `/history` | § 42 EnWG grid-area Energiemix |
| `GET` | `/api/v1/nelos` · `GET`/`PUT` `/api/v1/nelos/{id}` | Netz-Element-Lokationen (Redispatch 2.0) |
| `GET` | `/api/v1/tranchen` · `GET`/`PUT` `/api/v1/tranchen/{id}` | Tranchen (GPKE Teil 4) |
| `GET` | `/api/v1/mabis-zp` | Every Bilanzierungsgebiet → MaBiS-Zählpunkt assignment |
| `GET`/`PUT` | `/api/v1/bilanzierungsgebiete/{eic}/mabis-zp` | Resolve / assign the MaBiS-Zählpunkt. `404` means **refuse the submission**, never *substitute the EIC*. Strom only — MaBiS has no Gas counterpart |
| `PUT`/`GET` | `/api/v1/netzzugang/antraege` · `/{id}` · `PATCH /{id}/status` | § 20b EnWG Netzzugangsplattform requests |
| `PUT`/`GET` | `/api/v1/msb-rahmenvertraege-gas` · `/{id}` | Gas MSB framework contracts (GeLi Gas 3.0 Tenor 13–16) |
| `GET` | `/api/v1/correlations` · `/{id}` | Running MaKo processes per MaLo |

### Price sheets

| Method | Path | Description |
|---|---|---|
| `GET`/`PUT` | `/api/v1/preisblaetter/{nb_mp_id}` | `PreisblattNetznutzung`; the `PUT` versions it and emits `de.markt.pricat.published` |
| `GET`/`PUT` | `/api/v1/preisblaetter-messung/{msb_mp_id}` | `PreisblattMessung` — validates `zaehlzeitregister`, rejects `bandNummer` (422) |
| `GET`/`PUT` | `/api/v1/preisblaetter-ka/{nb_mp_id}` | `PreisblattKonzessionsabgabe` (KAV § 2) |
| `GET`/`PUT` | `/api/v1/preisblaetter-dienstleistung/{msb_mp_id}` | `PreisblattDienstleistung` |
| `GET`/`PUT` | `/api/v1/preisblaetter-hardware/{msb_mp_id}` | `PreisblattHardware` |
| `GET` | `/api/v1/mmma-preise/gas` · `GET`/`PUT` `/gas/{year}/{month}` | Gas Mehr-/Mindermengenpreise per Marktgebiet (Trading Hub Europe) |
| `GET`/`PUT` | `/api/v1/mmm-preise/strom/{year}/{month}` | Strom Mehr-/Mindermengenpreise — **one nationwide series** (§ 13 Abs. 3 StromNZV, published by the BDEW), keyed by month alone |
| `POST` | `/api/v1/mmma-preise/import-trigger` | Run the import now (`?year=&month=`) |
| `GET` | `/api/v1/pricat/{nb_mp_id}/history` | PRICAT version history |
| `GET` | `/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}` | Dispatch audit log |
| `POST` | `/api/v1/pricat/{nb_mp_id}/dispatch` | (Re-)dispatch to all active LF partners |

### Subscriptions, events and admin

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/subscriptions` | List subscriptions |
| `GET`/`PUT`/`DELETE` | `/api/v1/subscriptions/{id}` | Fetch / register / deactivate a webhook subscription |
| `POST` | `/api/v1/subscriptions/{id}/test` | Send a test event to one endpoint |
| `POST` | *(`webhook.inbound_path`)* | Ingest an inbound `makod` CloudEvent (HMAC-verified, idempotent) |
| `GET`/`POST`/`DELETE` | `/admin/fanout/dlq…` | Dead-letter queue: inspect, requeue, discard |
| `GET` | `/admin/events` | Full-envelope CloudEvent replay log |
| `GET` | `/health/live` · `/health/ready` | Liveness · readiness (bounded DB ping) |
| `GET` | `/metrics` | Prometheus |

---

## Events

### Emitted

Every entity event carries `marktmaloid` / `marktmeloid` where one applies (the delivery
ordering key) and `marktsparte` where the Sparte is known.

`de.markt.malo.updated`, `de.markt.melo.updated`, `de.markt.partner.updated`,
`de.markt.nb-contract.updated`, `de.markt.versorgung.changed`,
`de.markt.versorgung.gap-detected`, `de.markt.versorgung.eog-begonnen`,
`de.markt.pricat.published`, `de.markt.sr.konfigurationsprodukt.updated`,
`de.markt.geraet.konfiguration.updated`, `de.markt.mmma.import.success`,
`de.markt.mmma.import.failed`, `de.markt.einwilligung.erteilt`,
`de.markt.einwilligung.widerrufen`, `de.markt.subscription.test`.

### Durable fan-out

Producers persist the whole CloudEvent envelope to `event_log` **before** any delivery, in
the same transaction as the business write. The fan-out worker is the only consumer and
runs in two phases:

1. **Fan-out** — claim pending `event_log` rows in `seq` order, snapshot the matching
   subscriber set into `event_delivery`, stamp `fanned_out_at`. A crash before commit
   leaves the row pending.
2. **Deliver** — claim due deliveries with a lease (`FOR UPDATE SKIP LOCKED`), POST them
   signed, and on failure back off with jitter. After `max_retry_attempts` the row is
   **dead-lettered, never dropped** (§ 147 AO / GoBD: a `de.mako.process.initiated` to
   `invoicd` announces a message that becomes a Buchungsbeleg).

#### Ordering: per aggregate

Deliveries are ordered **per Marktlokation**, by `event_log.seq`: a delivery is held back
while an earlier event about the same MaLo is still outstanding to the same subscriber.
Events about different MaLos, and events tied to no MaLo, never wait for each other.

One MaLo's supply lifecycle is the only sequence whose order carries meaning, so that is
the scope of the guarantee. Per-endpoint FIFO would serialise the hub behind its slowest
subscriber; unordered delivery would let a retried `versorgung.changed` arrive after the
transition that superseded it. A dead-lettered delivery stops blocking its key, so
head-of-line blocking is bounded by `max_retry_attempts`.

`seq` is a `BIGSERIAL`, not `received_at`: `received_at` defaults to `now()`, the
transaction start time, so every event from one ingest shares a timestamp.

### Registering a subscription

```bash
curl -s -X PUT http://localhost:8180/api/v1/subscriptions/erp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "webhook_url":  "https://erp.example.com/markt/events",
    "webhook_secret": "mysecret",
    "event_types":  ["de.markt.malo.updated", "de.markt.pricat.published"],
    "sparten":      ["STROM"],
    "roles":        ["NB"]
  }'
```

`roles` and `sparten` are filters: an empty array matches everything, otherwise the event's
`marktrole` / `marktsparte` extension must appear in it. An event with no `marktsparte` is
not Sparte-scoped (a Marktpartner, a subscription test) and matches every `sparten` filter.

`webhook_url` must be `http`/`https` and must not name a loopback, link-local or private
address — the worker POSTs from inside the deployment's network, and the shared HTTP client
refuses redirects so the check cannot be bypassed with a `302`.

Deliveries carry `webhook-signature: v1,<base64>` when a secret is set:

```python
import hmac, hashlib

def verify(secret: str, body: bytes, signature: str) -> bool:
    received = signature.removeprefix("sha256=")
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, received)
```

`DELETE` deactivates rather than removes: `event_delivery` rows reference the subscriber and
are the record of whether a market event was delivered.

#### Webhook secret at rest

`subscriptions.webhook_secret` is stored **in plaintext** and used directly as the
HMAC-SHA256 key. It is an integrity secret (it lets a subscriber verify a delivery came from
this hub), not a confidentiality key over customer data. Protect it with database-level
controls: least-privilege grants on the `subscriptions` table and storage encryption.

---

## Temporal integrity

Every "who was responsible on this date" answer — which Netzbetreiber, which MSB, which
price sheet — is a read filtered by a validity window. If two rows can cover one date the
query has two answers and returns whichever the planner reached first, which surfaces as a
settlement against the wrong tariff rather than as an error.

Validity is half-open `[valid_from, valid_to)`, and overlap is refused by the database:

```sql
CONSTRAINT rollenzuordnungen_no_overlap EXCLUDE USING gist (
    malo_id       WITH =,
    zuordnungstyp WITH =,
    daterange(valid_from, valid_to, '[)') WITH &&
)
```

applied to `rollenzuordnungen`, `melo_msb_zuordnungen`, `nb_contracts` and all five price
sheet tables. Open-ended rows participate (a `NULL` bound reads as infinity), and a
successor may start on the day its predecessor ends. Price-sheet natural keys additionally
use `UNIQUE NULLS NOT DISTINCT`, so "open-started" is one row rather than an unbounded
family of them. `tests/temporal_constraints_integration.rs` pins all of it against a real
PostgreSQL.

---

## MMMA / MMM price import

A background worker keeps the current month's Mehr-/Mindermengenpreise present, retrying
hourly for as long as either commodity is missing, and emitting
`de.markt.mmma.import.success` / `.failed` per run.

- **Gas** — Trading Hub Europe publishes per Marktgebiet (only `THE` since 2021).
- **Strom** — § 13 Abs. 3 StromNZV requires *einheitliche* prices from monthly market
  prices; the BDEW determines and publishes **one nationwide series**, with a Mehr and a
  Minder value per application month. There is no per-Netzbetreiber and no per-ÜNB variant,
  so the month is the entire key.

Both are read by `netzbilanzd` (INVOIC 31002/31005) and `invoicd` (MMM check 6).

---

## Database

Migrations run at startup. The schema is one file: `migrations/0001_initial.sql`. Drop and
recreate the database to reset — all application data is reproducible from the EDIFACT event
streams in `makod`.

- PostgreSQL 15+
- `btree_gist`, created by the migration (it backs the exclusion constraints above).
  `pgcrypto` is *not* needed: `gen_random_uuid()` has been built in since PostgreSQL 13.

---

## Building and testing

```bash
just ci                      # full workspace gate
cargo build -p marktd --release
cargo test -p marktd         # unit + source-level guards, no database
just test-marktd-db          # every integration suite against a real PostgreSQL
```

The database suites self-manage PostgreSQL via **testcontainers** — the only requirement is
a running Docker daemon (no manual `docker run`, no `DATABASE_URL`), and they skip
gracefully without one.

---

## Docker

```yaml
services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_DB:       marktd
      POSTGRES_USER:     marktd
      POSTGRES_PASSWORD: secret

  marktd:
    image: ghcr.io/hupe1980/mako-marktd:latest
    depends_on: [postgres]
    environment:
      MARKTD_DATABASE__URL:           postgres://marktd:secret@postgres/marktd
      MARKTD_MARKT__TENANT:           "9900357000004"
      MARKTD_OIDC__ISSUER:            https://auth.example.com
      MARKTD_OIDC__AUDIENCE:          marktd
      MARKTD_WEBHOOK__INBOUND_SECRET: env:MAKOD_WEBHOOK_SECRET
    ports: ["8180:8180"]
```

The image's `HEALTHCHECK` runs `marktd --check`, which probes this instance's own
`/health/ready` over loopback — what a distroless image can do without a shell or `curl`.
It does not open a second pool and does not re-run migrations.
