# vertragd — Contract & Customer Management

`vertragd` is the **customer registry and retail contract lifecycle engine** for
both B2C (private households) and B2B (commercial, RLM) customers. It is the
single source of truth for contract state and the authorization gateway between
OIDC identities and MaLo IDs.

| | |
|---|---|
| **HTTP port** | `:9780` |
| **Database** | PostgreSQL (single consolidated `0001_schema.sql`, sqlx 0.8) |
| **MCP** | 17 read-only tools + 4 prompts at `/mcp` |
| **Health** | `GET /health/live`, `GET /health/ready` |

## The rules are in one place

Every deadline `vertragd` enforces comes from a statute, and the statute differs
by *which* contract and *which* customer. `src/domain.rs` holds them as pure
functions — no HTTP, no SQL, no clock — so they are unit-testable and a handler
cannot invent a fourth notice period.

| Rule | Source | Value |
|---|---|---|
| Kündigung Grundversorgung | § 20 Abs. 1 StromGVV / GasGVV | 2 Wochen, jederzeit |
| Kündigungsbestätigung | § 41 Abs. 8 Nr. 2 EnWG, § 20 Abs. 2 GVV | unverzüglich, Textform |
| Kündigung Sondervertrag | Vertrag, gedeckelt durch § 309 Nr. 9 lit. c BGB | ≤ 1 Monat für Verbraucher |
| Sonderkündigung Preisanpassung | § 41 Abs. 5 Satz 4 EnWG, § 5 Abs. 3 GVV | fristlos zum Wirksamwerden |
| Sonderkündigung Umzug | § 41b Abs. 5 EnWG | 6 Wochen (Haushaltskunden) |
| Preisänderungsanzeige Sondervertrag | § 41 Abs. 5 Satz 2 EnWG | 1 Monat (Haushaltskunde), sonst 2 Wochen |
| Preisänderungsanzeige Grundversorgung | § 5 Abs. 2 StromGVV / GasGVV | 6 Wochen, nur zum Monatsersten |
| Erstlaufzeit Verbrauchervertrag | § 309 Nr. 9 lit. a BGB | ≤ 24 Monate |
| Stillschweigende Verlängerung | § 309 Nr. 9 lit. b BGB | nur unbefristet, ≤ 1 Monat kündbar |
| Ersatzversorgung | § 38 Abs. 4 EnWG | endet spätestens nach 3 Monaten |

Two contract facts decide which column applies, so both are stored explicitly
rather than guessed:

- `versorgungsvertraege.vertragsart` — `GRUNDVERSORGUNG` / `ERSATZVERSORGUNG` /
  `SONDERVERTRAG`. An unknown value reads as `SONDERVERTRAG`, the regime with
  the least statutory privilege, so a typo cannot claim GVV-Fristen.
- `kunden.haushaltskunde` — § 3 Nr. 57 EnWG. **Not** the same fact as
  `kundentyp`: a commercial customer consuming ≤ 10 000 kWh a year is a
  Haushaltskunde too. Defaults to `kundentyp == "B2C"` on create.

## Which product a MaLo is on is a contract fact — and lives here

Agreeing it is a **Tarifwechsel**: governed by § 41 Abs. 5 EnWG, guarded by the
contract's Preisgarantie, decided here. So it is stored here, once, as
valid-time slices on the component:

```text
[gueltig_von, gueltig_bis)   gueltig_bis is the first day NOT covered
```

`kp_no_overlap` (GiST) makes two products for one component on one day
unrepresentable, and half-open ranges make consecutive slices tile a billing
period exactly — a switch on the 15th ends one slice and starts the next on the
same date, and no day belongs to both.

```bash
# Period form — what billingd bills from
GET /api/v1/malo/{malo_id}/produkte?from=2026-11-01&to=2026-11-30
# → { "slice_count": 2, "fully_covered": true, "slices": [
#      { "product_code": "STROM-ALT", "gueltig_von": "2026-11-01", "gueltig_bis": "2026-11-15" },
#      { "product_code": "STROM-NEU", "gueltig_von": "2026-11-15", "gueltig_bis": "2026-12-01" } ] }

# Point form — the product in force on a day (default today)
GET /api/v1/malo/{malo_id}/produkte?as_of=2026-11-20
```

`productd` answers the other half — what that code **costs** on that day. It does
not know who is on it.

A **future-dated** Tarifwechsel is a slice that starts in the future: there is no
pending state and nothing applies it on the day. Re-applying the same change is
idempotent; a change dated *behind* a later one is refused, because it would
reprice a period already decided and, for an announced change, one the customer
has been told about.

## Nothing is dispatched from a detached task

A contract change and every obligation that follows from it commit together.
There are exactly two durability rails, and neither is a `tokio::spawn`:

```text
handler ─┬─ contract write ───────┐
         ├─ enqueue outbound_task ┤ COMMIT ─→ outbound worker  ─→ processd / edmd / accountingd
         └─ enqueue event_outbox  ┘        └─ outbox worker    ─→ ERP webhook (HMAC-signed)
```

- **`outbound_tasks`** — every service-to-service call: `LIEFERBEGINN`,
  `LIEFERENDE`, `ABLESUNG_BEGINN`, `ABLESUNG_ENDE`, `ABRECHNUNGSKONTO`.
  Exponential backoff (30 s → 1 h), dead-lettered after 8 attempts, claimed
  with `FOR UPDATE SKIP LOCKED` so replicas share the queue.
  A unique `dedupe_key` makes the enqueue exactly-once — an idempotent re-POST
  of the same `erp_contract_id` cannot produce a second UTILMD.
- **`event_outbox`** (`mako_service::outbox`) — every customer-facing
  `de.vertrag.*` CloudEvent, including the statutory notices.

A crash therefore costs a retry, never an obligation. What the retries could not
discharge is visible at `GET /api/v1/outbound/dead` and requeueable at
`POST /api/v1/outbound/dead/{id}/retry`.

## Authentication and authorization

Every REST route extracts `Claims`, and the extractor rejects a token whose
`mako_tenant` is not this deployment's — a validly signed token from another
operator in the same OIDC realm is otherwise indistinguishable from a local one.

Authentication is not enough here, because **not every token that reaches this
service is an operator's**. `portald` forwards an end customer's own token to
`GET /api/v1/kunden/authenticate`, deliberately: that route answers whether *the
token's subject* owns a Marktlokation, and only a proven subject can be asked.
So every route also runs a Cedar check against `policies/vertragd.cedar`:

- **Customer-scoped actions** — `authenticate-portal-identity` and
  `read-own-portal-identity` (`/kunden/by-sub/{sub}` when the subject asked
  about is the caller's own) — are open to any authenticated principal of this
  tenant.
- **Every other action** requires a market role (`LF`, `MSB`, `ESA`, `ADMIN`):
  reading a customer's profile, bank details or DSGVO export, granting portal
  access, creating or terminating supply, changing a tariff, erasing a record.
  The MCP surface sits behind the same split as the blanket `use-mcp` action.

This rests on one deployment invariant: **the portal IdP realm must not issue
`mako_roles`.** A market role on a customer token makes that customer an
operator of this service.

`tests/authorization_guard.rs` pins that every routed handler both extracts
`Claims` and authorizes, that no Cedar action is checked without a grant or
granted without a check, and that a role-less principal is refused on every
operator action.

The two webhook routes carry no token and are authenticated by the shared
Standard Webhooks signature over the raw body. `main` refuses to start without
**both** `[oidc]` and `inbound_secret` unless `allow_insecure_no_auth = true`: a
forged event on those routes confirms supply or creates a contract.

Service-to-service readers (`billingd`, `portald`, `processd`) authenticate with
an `[[oidc.service_keys]]` entry, which carries the market roles the operator
actions require.

## Data model

```text
Kunde (B2C: Haushalt/SLP, B2B: Unternehmen/RLM/HV)
├── N × KundenIdentitaet   OIDC portal users; 1:1 for B2C, 1:N for B2B
├── [B2B] Rahmenvertrag    shared pricing, Sammelrechnung, indexation, angebot_id
│    └── N × Versorgungsvertrag        one per site
│          └── N × Vertragskomponente  one per commodity
└── [B2C] Versorgungsvertrag
       └── N × Vertragskomponente
```

`vertrags_nr` and `rahmenvertrag_nr` are generated from a sequence, so no
contract exists without the number § 41 Abs. 1 Nr. 1 EnWG expects it to identify
itself by and every invoice, Mahnung and support call quotes.

Contract status: `ANGELEGT → IN_BEARBEITUNG → TEILERFUELLUNG → AKTIV →
GEKÜNDIGT → ABGELAUFEN`, plus the terminal `ABGELEHNT` (every commodity refused
by the NB) and `STORNIERT` (cancelled before supply began). It is derived from
the component statuses and never returns from a terminal state.

## REST API

### Kunden

| Method | Path | |
|---|---|---|
| `POST` | `/api/v1/kunden` | Create/upsert, idempotent on `erp_kunde_id` |
| `GET`/`PUT` | `/api/v1/kunden/{id}` | Profile; PUT is a partial update |
| `GET` | `/api/v1/kunden` | Operator list, `?kundentyp=`, `?limit=` |
| `GET` | `/api/v1/kunden/by-sub/{sub}` | OIDC subject → customer + the MaLos that identity may see |
| `GET` | `/api/v1/kunden/authenticate?malo_id=` | portald's authorization check |
| `POST`/`GET` | `/api/v1/kunden/{id}/identitaeten` | Portal users (max 50, configurable) |
| `DELETE` | `/api/v1/kunden/{id}/identitaeten/{sub}` | Revoke portal access |
| `GET`/`PUT` | `/api/v1/kunden/{id}/person` | BO4E `Person` (B2C), through the BO4E gate |
| `GET`/`PUT` | `/api/v1/kunden/{id}/zahlungsinformation` | BO4E `Zahlungsinformation`, through the gate, then IBAN mod-97 and BIC validated |
| `GET` | `/api/v1/kunden/{id}/export` | DSGVO Art. 15 / Art. 20 |
| `POST` | `/api/v1/kunden/{id}/anonymize` | DSGVO Art. 17 |
| `GET` | `/api/v1/kunden/{id}/portfolio` | One row per active MaLo/Sparte |

### Verträge

| Method | Path | |
|---|---|---|
| `POST`/`GET` | `/api/v1/kunden/{id}/vertraege` | Create (idempotent on `erp_contract_id`) / list |
| `GET` | `/api/v1/vertraege` | Open contracts |
| `GET` | `/api/v1/vertraege/{id}` | Contract + components |
| `GET` | `/api/v1/vertraege/by-malo/{malo_id}` | § 40 Abs. 1 EnWG invoice facts + BG-7 buyer + the next admissible Kündigungstermin; `?stichtag=YYYY-MM-DD` measures the notice period from that date instead of today (`processd` passes the Eingangsdatum der Kündigung, which `E_0614` Prüfschritt 70 names); `?kunde=<Name>` answers `E_0624` Prüfschritt 50 as a tri-state `kundenidentitaet` — token-set match, with Jaro-Winkler and Kölner Phonetik widening `UNKLAR` only |
| `GET` | `/api/v1/vertraege/{id}/kuendigungsfrist` | Earliest lawful end date **per reason**, with the rule |
| `GET` | `/api/v1/malo/{malo_id}/produkte` | The valid-time product assignment — `?as_of=` or `?from=&to=` |
| `POST` | `/api/v1/vertraege/{id}/kuendigen` | Terminate |
| `POST` | `/api/v1/vertraege/{id}/widerruf-kuendigung` | Withdraw before the Lieferende |
| `POST` | `/api/v1/vertraege/{id}/stornieren` | Cancel before supply began |
| `POST` | `/api/v1/vertraege/{id}/tarifwechsel` | Change product |
| `GET`/`PUT` | `/api/v1/vertraege/{id}/preisgarantie` | BO4E `Preisgarantie`, through the gate — which checks the `Zeitraum` in `zeitlicheGueltigkeit`, the field `preisgarantie_bis` and the Tarifwechsel guard are derived from |
| `GET` | `/api/v1/vertraege/billing-candidates` | § 40b EnWG cadence feed for billingd |
| `GET` | `/api/v1/vertraege/expiring` | `?days=` (default 30) |

The three BO4E routes (`person`, `zahlungsinformation`, `preisgarantie`) store
the **canonical round-trip**, not the request body, which is why the gate's
strict-enum stage matters here: a BO4E enum that decodes to the `Unknown`
catch-all serialises back as the literal string `"UNKNOWN"`, so skipping it did
not merely accept an unrecognised value — it replaced what the caller sent.

### Rahmenverträge, Stammdaten, Betrieb

| Method | Path | |
|---|---|---|
| `POST`/`GET` | `/api/v1/kunden/{id}/rahmenvertraege` | Create / list per customer |
| `GET` | `/api/v1/rahmenvertraege`, `/{id}` | Operator list / single |
| `GET` | `/api/v1/rahmenvertraege/{id}/malos` | Sammelrechnung sites + the bundle's BG-7 buyer |
| `POST` | `/api/v1/rahmenvertraege/{id}/kuendigen` | Cascade termination |
| `GET`/`PUT` | `/api/v1/ggv/{ggv_id}/betreiber` | § 42b EnWG GGV operator as a Kunde |
| `GET` | `/api/v1/aggregatorvertraege` | § 41e EnWG VPP contracts (Art. 17 RL (EU) 2019/944); `?sr_id=`, `?on=` |
| `PUT` | `/api/v1/aggregatorvertraege/{sr_id}` | Upsert one, keyed on the SR-ID |
| `GET`/`PUT` | `/api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}` | MSB contract per MeLo; `?on=` and `?haushaltskunde=` date the § 309 Nr. 9 lit. c BGB cap on the next possible end |
| `GET` | `/api/v1/outbound/dead` | Obligations the retries gave up on |
| `POST` | `/api/v1/outbound/dead/{id}/retry` | Requeue one |
| `POST` | `/api/v1/events` | MaKo outcomes from makod/processd (HMAC) |
| `POST` | `/api/v1/webhooks/angebot` | CPQ `de.tarif.angebot.angenommen` (HMAC) |

## Kündigung

The notice period comes from the **reason**, not from the contract record. Ask
before you act:

```bash
curl -s http://vertragd:9780/api/v1/vertraege/{id}/kuendigungsfrist
# → { "vertragsart": "GRUNDVERSORGUNG", "haushaltskunde": true,
#     "fristen": {
#       "ORDENTLICH":         { "fruehestens": "2026-09-15", "frist": "2 Wochen",
#                               "rechtsgrundlage": "§ 20 Abs. 1 StromGVV / GasGVV" },
#       "UMZUG":              { "fruehestens": "2026-10-13", "frist": "6 Wochen",
#                               "rechtsgrundlage": "§ 41b Abs. 5 EnWG" },
#       "PREISANPASSUNG":     { "fruehestens": "2026-09-01", "frist": "fristlos …" },
#       "LIEFERANTENWECHSEL": { … } } }

curl -X POST http://vertragd:9780/api/v1/vertraege/{id}/kuendigen \
  -d '{"lieferende":"2026-10-13","grund":"UMZUG","eingang":"2026-09-01"}'
# → 202 { "status": "GEKÜNDIGT", "frist": "6 Wochen",
#         "rechtsgrundlage": "§ 41b Abs. 5 EnWG", "mako_dispatched": 1 }
```

A `lieferende` earlier than the rule allows is a **422 quoting the rule**. One
transaction then records the end date on each component, enqueues the Lieferende
UTILMDs *and* the Schlussablesung, sets `vertragsende`, clears `auto_renewal`,
and emits `de.vertrag.kuendigung` carrying the § 41 Abs. 8 Nr. 2 EnWG Textform
confirmation the supplier owes the customer.

**Supply does not end when the Kündigung is filed.** A termination three months
out leaves the customer supplied — and billable — for those three months, so the
components keep their status until the date arrives; the daily worker then ends
them and closes the contract with `de.vertrag.abgeschlossen`. Ending them at
once took the remaining months and the Schlussrechnung out of the § 40b billing
feed.

`eingang` exists because the notice period runs from **receipt**, not from data
entry: an operator keying in last week's letter says so.

## Tarifwechsel

A Tarifwechsel changes price, not supply — no UTILMD, no MaKo status change.

`initiator` is required, because the two cases are different legal acts.
§ 41 Abs. 5 Satz 1 EnWG binds a supplier who *exercises* a reserved right to
change the contract, and Satz 4 gives the customer a fee-free termination right
because the supplier exercised it. A tariff the customer asked for
(`"initiator":"KUNDE"` — what the portal sends) is an agreed change: it is
confirmed, carries no Sonderkündigungsrecht, and is not held to the supplier's
notice periods.

For a supplier-initiated future change, the following are enforced at the API
boundary, so compliance is structural rather than a worker's best effort:

```bash
# Blocked by the price guarantee
curl -X POST …/tarifwechsel -d '{"komp_id":"…","new_product_code":"X",
                                 "initiator":"LIEFERANT","wirksamkeit":"2026-08-01"}'
# → 422 {"error":"Tarifwechsel durch Preisgarantie gesperrt","preisgarantie_bis":"2027-06-30"}

# No preise[], with outputd_url configured — the rendered notice would state
# no Umfang
# → 422 {"error":"der Umfang der Preisänderung fehlt",
#        "rechtsgrundlage":"§ 41 Abs. 5 Satz 3 EnWG"}

# Too close for the § 41 Abs. 5 Satz 2 EnWG notice
# → 422 {"fruehestens":"2026-09-18","frist":"1 Monat","rechtsgrundlage":"§ 41 Abs. 5 Satz 2 EnWG"}

# A Grundversorgungspreis mid-month
# → 422 {"rechtsgrundlage":"§ 5 Abs. 2 StromGVV / GasGVV",
#        "naechster_zulaessiger_termin":"2026-10-01"}

# Operator bypass — logged to preisgarantie_override_log with the JWT subject
curl -X POST …/tarifwechsel -d '{…,"override_preisgarantie":true,"grund":"Kundenverzicht 2026-08-01"}'
```

A retroactive correction (`wirksamkeit ≤ today`) applies immediately and is
exempt: it is not an announced price change but the repair of one already
agreed.

**Who states the Umfang.** § 41 Abs. 5 Satz 3 EnWG governs what the *customer*
is told, so it binds whoever composes the notice. Where this deployment renders
it (`outputd_url` set), `preise[]` is the document's content and a change
without it is refused. Where the CloudEvent is the notice, the ERP composes the
letter from its own price sheets: `preise[]` is optional, travels on the event
when given, and `umfang_vollstaendig: false` on
`de.vertrag.preisaenderung.ankuendigung` says the lines are the composer's to
supply — a letter that states no Umfang is not a valid notice on any channel.

**A pending notice cannot be relabelled away.** The write is idempotent on
`(komp_id, wirksamkeit)`, and a replay that changes `initiator` while the notice
is still owed is refused with `409`: re-sending a `LIEFERANT` price rise as
`KUNDE` would take it out of the announcement queue, the customer would never be
told, and the breach report would come back clean because the record — not the
fact — had changed.

## DSGVO Art. 17

```bash
curl -X POST http://vertragd:9780/api/v1/kunden/{id}/anonymize \
  -d '{"requested_by":"dpo"}'
```

**409 while supply runs.** The data is needed to perform the contract
(Art. 6 Abs. 1 lit. b DSGVO), so Art. 17 Abs. 1 lit. a does not apply and
Art. 17 Abs. 3 lit. b keeps what the § 41 EnWG obligations require; the response
names the contracts, so the answer to the data subject writes itself. `force`
exists for the cases where erasure applies regardless (Art. 17 Abs. 1 lit. d)
and demands a `request_reason` on record.

Once it applies, one transaction pseudonymises the Geschäftspartner, the Person,
the Zahlungsinformation, the VAT-ID, the notes, **the supply address**, and every
portal login — each with its own pseudonym, because a B2B customer has several
and one token for all of them violates `UNIQUE (tenant, oidc_sub)`. The contract
rows survive without personal data for § 147 Abs. 3 AO (Handelsbriefe 6 Jahre,
Buchungsbelege 8 Jahre), and `anonymization_log` records what was overwritten.

## Background workers

| Worker | What it does |
|---|---|
| **Outbound** | Drains `outbound_tasks` every 5 s, up to 64 per wake-up |
| **Outbox** | Delivers `de.vertrag.*` to the ERP webhook (`mako_service::outbox`) |
| **Preisanpassung** | Sends the § 41 Abs. 5 notice, with the Sonderkündigungsrecht, for every scheduled **supplier-initiated** Tarifwechsel whose notice is still owed |
| **Auto-renewal** | Announces the extension once per term, then applies it: **unbefristet with ≤ 1 month notice for consumers** (§ 309 Nr. 9 lit. b BGB), a further fixed term only for business customers |
| **Ablauf** | Ends supply whose Lieferende has passed and closes the contract behind it; announces a term or price guarantee running out — once per date, tracked in `ablauf_notif_fuer` |

With `outputd_url` and `[absender]` set, the Preisanpassung worker renders and
delivers the letter *before* it records the notice as sent; a notice that could
not be issued is written back to the slice (`notif_versuche`,
`notif_letzter_fehler`) and retried, and a change that reached its Wirksamkeit
unannounced is reported as a breach on every run.

Every notice goes through the outbox rather than a direct webhook call: a notice
the supplier owes must not depend on the ERP being reachable at the moment the
worker happens to run.

**One replica runs each of the three daily workers per cycle.** Idempotency
serialises repeats of the same run; it says nothing about two replicas reading
the same unmarked slice in the same second, which would send the § 41 Abs. 5
notice twice. Each takes its own session-level PostgreSQL advisory lock
(`mako_service::worker_lock`, shared with `accountingd`; the keys are per
service, `0x_7e64_xxxx` here) and skips the cycle when another instance holds
it.

## Configuration

```toml
# vertragd.toml
port     = 9780
tenant   = "9900357000004"   # data-isolation key (here: the operator's BDEW-Codenummer)
lf_mp_id = "9900357000004"   # market identity registered on the UTILMD

processd_url    = "http://processd:8580"
accountingd_url = "http://accountingd:9380"
edmd_url        = "http://edmd:8380"
edmd_api_key    = "env:VERTRAGD_EDMD_SERVICE_KEY"

# Customer-facing CloudEvents. Without this the statutory notices still land in
# event_outbox, but nothing delivers them.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:VERTRAGD_ERP_HMAC_SECRET"

# The only authentication on POST /api/v1/events and /api/v1/webhooks/angebot.
inbound_secret  = "env:VERTRAGD_INBOUND_SECRET"

# Renders and delivers the § 41 Abs. 5 EnWG letter. Without it the notice still
# travels as a CloudEvent and the ERP composes the letter from its own prices.
outputd_url     = "http://outputd:9880"
outputd_api_key = "env:VERTRAGD_OUTPUTD_SERVICE_KEY"

max_identitaeten_per_kunde = 50

[database]
url = "postgresql://vertragd:secret@db:5432/vertragd"

[oidc]
# required in production; see mako-service oidc docs

# The declarant on that letter — § 126b BGB Textform needs a named sender.
[absender]
name      = "Beispiel Energie GmbH"
post_code = "10115"
city      = "Berlin"
```

## Tests

```bash
cargo test -p vertragd                       # domain rules, event parsing, status derivation
just test-vertragd-db                        # real PostgreSQL (testcontainers)
```

The real-PostgreSQL suite proves the invariants that live in SQL rather than in
Rust: idempotent creation enqueuing exactly one registration, the Kündigung
transaction, the Stornierung guards, the DSGVO erasure across several logins,
tenant scoping, the Sammelrechnung site enumeration, and the consumer
auto-renewal shape.
