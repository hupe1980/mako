+++
title = "marktd Operator Guide"
description = "Operator guide for marktd, the market data hub: Marktpartner, Marktlokationen, price sheets and the durable CloudEvents fan-out every other service reads."
weight = 22
+++
`marktd` is the **Market Data Hub** — the single source of truth for all market entity
state in a MaKo deployment. It stores Marktlokationen (MaLo) with typed `rubo4e::current::Marktlokation`
API responses (schema validated on PUT), Messlokationen (MeLo) with typed `rubo4e::current::Messlokation`
responses, Zaehler + Geraete (device registry) with typed `rubo4e::current::Zaehler`/`Geraet` responses,
energy contracts, trading partners, network contracts (`nb_contracts`) with full BO4E **`Vertrag`** payload
(`vertragsart`, `vertragsstatus` as indexed columns for ERP digital LRV exchange), price sheets
(PreisblattNetznutzung), **VersorgungsStatus per MaLo** (with full history and
point-in-time queries), **MaLo grid topology** (`malo_grid` — sourced from the NB's
NIS/GIS system and provisioned via `PUT /api/v1/malos/{id}/grid`; read by `processd`
for Anmeldung STP decisions), and
**Netz-Element-Lokationen (NeLo)** for Redispatch 2.0.

Beyond data storage, `marktd` includes:

- **durable fan-out** — enriches inbound `de.mako.*` events with `marktrole` and fans out
  to all registered subscribers (ERP, `processd`, `invoicd`, `obsd`) via HMAC-signed webhooks.
- **VersorgungsStatus derivation** — five transitions driven by inbound `de.mako.*`
  process events: `announce_lf_next` (55001/55077/44001), `confirm_supply`
  (55002/55078/44002), `clear_lf_next` (55003/55080/44003), `end_supply` (55005/44005)
  and `begin_eog_supply` (55013/44013). Who supplies the Marktlokation is a **list**
  (`lf_zuordnung`): a tranchierte erzeugende MaLo is held by several LFA at once and
  more than one Anmeldung can be pending. An uncovered interval emits
  `de.markt.versorgung.gap-detected` — the §38 EnWG gap-closure trigger — and every
  change appends a whole-list snapshot to `versorgungsstatus_history`, which
  `?at=YYYY-MM-DD` resolves against. The lifecycle table below has the detail.

`marktd` is a **pure data hub**. Automated Anmeldung STP decisions are the
responsibility of `processd`'s NB module, which subscribes to `marktd`'s fan-out
and uses the pure `mako-pruefung` library for all decisions.
This separation keeps `marktd` free of domain policy and makes `processd` independently
scalable and testable.

```mermaid
graph TB
    makod["makod :8080<br/>EDIFACT ↔ BO4E"]
    marktd["marktd :8180<br/>Market Data Hub<br/>(this service)"]
    processd["processd :8580<br/>Process decisions<br/>(NB STP + LF answers)"]
    erp["ERP<br/>(Powercloud / SAP IS-U)"]
    invoicd["invoicd :8280<br/>Billing"]
    edmd["edmd :8380<br/>Energy Data"]
    obsd["obsd :8480<br/>Observability"]
    pg["PostgreSQL"]

    makod -->|"de.mako.process.*<br/>HMAC-signed POST"| marktd
    marktd -->|"VersorgungsStatus<br/>derivation"| pg
    marktd -->|"de.mako.* + de.markt.*<br/>HMAC webhooks"| erp
    marktd -->|"de.mako.process.initiated"| processd
    marktd -->|"de.mako.process.initiated"| invoicd
    marktd -->|"de.mako.*"| edmd
    marktd -->|"de.mako.*"| obsd
    marktd --- pg
    erp -->|"PUT /api/v1/malos<br/>PUT /api/v1/partners"| marktd
    invoicd -->|"GET /api/v1/preisblaetter<br/>GET /api/v1/nb-contracts"| marktd
    processd -->|"GET /api/v1/versorgung<br/>GET /api/v1/malos/{id}/grid<br/>GET /api/v1/partners"| marktd
    processd -->|"POST /api/v1/commands"| makod
```

The clean separation of concerns:

| Service | Responsibility |
|---------|----------------|
| `makod` | EDIFACT parsing, BDEW process rules, AS4 delivery, regulatory deadlines |
| `marktd` | Market data, VersorgungsStatus, ERP subscriptions, durable fan-out |
| `processd` | Automated STP decisions (NB: `mako-pruefung`; LF: answers to 55007 / 55010) |

---

## Port Layout

```
┌─────────────────────────────────────────────────────────────────┐
│  marktd  :8180                                                    │
│                                                                 │
│  Axum REST API                                                  │
│   ├─ OIDC/JWT middleware  → JwtClaims { sub, mako_tenant,      │
│   │                                     mako_roles, mako_sparte}│
│   ├─ Cedar ABAC enforcer  → permit / deny                      │
│   └─ Handlers             → PostgreSQL (SQLx)                  │
│                                                                 │
│  POST /api/v1/mako/events  ← makod CloudEvents ingest          │
│   ├─ Verify HMAC signature                                     │
│   ├─ Deduplicate via processed_events table                    │
│   ├─ Fan-out to all fan-out subscribers                       │
│   └─ Derive VersorgungsStatus (55002/55078/55005/55013 + Gas)  │
│                                                                 │
│  GET  /admin/fanout/dlq                       ← DLQ inspection │
│  POST /admin/fanout/dlq/{event}/{sub}/retry   ← re-deliver     │
│  DEL  /admin/fanout/dlq/{event}/{sub}         ← discard entry  │
│  GET  /metrics                                ← Prometheus     │
│                                                                 │
│  Note: Automated STP decisions live in processd :8580          │
│  marktd is a pure data hub — no domain policy.                  │
│                                                                 │
│  GET /health/live   — liveness (no DB check)                   │
│  GET /health/ready  — readiness (bounded PostgreSQL ping)      │
└─────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### With Docker Compose (full stack)

See `demos/nb-stp/docker-compose.yml` for the complete 8-service stack (postgres + webhook +
marktd + processd + makod + invoicd + edmd + obsd).

Minimal compose snippet for marktd alone:

```yaml
services:
  postgres:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB:       marktd
      POSTGRES_USER:     marktd
      POSTGRES_PASSWORD: secret
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U marktd -d marktd"]
      interval: 5s
      retries: 10

  marktd:
    image: ghcr.io/hupe1980/mako-marktd:0.12.0
    depends_on:
      postgres:
        condition: service_healthy
    volumes:
      - ./marktd.toml:/etc/marktd/marktd.toml:ro
    environment:
      DATABASE_URL:         postgres://marktd:secret@postgres/marktd
      MAKOD_API_KEY:        my-makod-api-key
      MAKOD_WEBHOOK_SECRET: my-webhook-secret
    command: ["--config=/etc/marktd/marktd.toml"]
    ports: ["8180:8180"]
```

### Binary

```bash
marktd --config /etc/marktd/marktd.toml
# or: MARKTD_CONFIG=/etc/marktd/marktd.toml marktd
```

Migrations run automatically at startup via `sqlx migrate run`.

---

## Configuration

`marktd` reads its configuration from a **TOML file** (default: `marktd.toml`),
with secrets deferred to environment variables via `"env:VAR_NAME"` values.

### Full `marktd.toml` reference

Config is loaded by `mako_service::load_config`: `marktd.toml` first (path from
`MARKTD_CONFIG`, default `./marktd.toml`), then `MARKTD_*` environment variables with `__`
as the section separator, then any `*_FILE` variable read from a file. The file is
optional — a container can be configured entirely from the environment
(`MARKTD_DATABASE__URL`, `MARKTD_MARKT__TENANT`, `MARKTD_MAKOD__API_KEY_FILE`, …).

```toml
[http]
addr = "0.0.0.0:8180"     # default

[database]
url             = "env:DATABASE_URL"  # required; use env: for secrets
pool_size       = 20
min_connections = 2

[markt]
# This deployment's own operator identity: the `resource_tenant` every Cedar check
# compares the caller's `mako_tenant` claim against, the `tenant` column on
# tenant-scoped rows, and the source URN of every outbound CloudEvent.
tenant = "9900357000004"           # required

[makod]
base_url  = "http://makod:8080"   # required
api_key   = "env:MAKOD_API_KEY"   # required

[webhook]
inbound_path          = "/api/v1/mako/events"        # default; must match makod's [erp] webhook_url
inbound_secret        = "env:MAKOD_WEBHOOK_SECRET"   # required unless allow_insecure_no_auth
delivery_timeout_secs = 10                           # default; per outbound webhook delivery
max_retry_attempts    = 3                            # default; then the delivery is dead-lettered

[oidc]              # required unless allow_insecure_no_auth
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-marktd"
jwks_refresh_secs = 300

[mcp]               # the /mcp surface's own API-key or OIDC layer
path = "/mcp"

[mmma_import]       # monthly Mehr-/Mindermengenpreis import; off by default
enabled        = false
gas_url        = ""   # THE Gas MMMA CSV/JSON; empty skips Gas
strom_url      = ""   # ÜNB Strom MMM CSV/JSON; empty skips Strom
check_hour_utc = 6    # runs on the 1st of each month at this UTC hour

# marktd is fail-closed: without [oidc] AND webhook.inbound_secret it refuses
# to start. The insecure dev posture must be requested by name:
# allow_insecure_no_auth = true

```

### CLI flags and environment

`marktd` parses exactly one argument; everything else is configured through the
environment, which is what a container image can set.

| Setting | How | Default | Description |
|---|---|---|---|
| `--check` | argv | — | Probe the **already-running** instance: `GET /health/ready` on loopback, exit 0 when ready. The distroless-friendly `HEALTHCHECK` — the image carries no shell or `curl`. It does not start the service, and it is not a config validator. |
| Config path | `MARKTD_CONFIG` | `./marktd.toml` | Absolute or relative path to the TOML file |
| Log level | `MARKTD_LOG_LEVEL`, else `LOG_LEVEL`, else `RUST_LOG` | `info` | Env-filter directive (`info`, `debug`, `marktd=trace`) |
| Log format | `LOG_FORMAT` | JSON | Structured-log encoding |

---

## Authentication & JWT Claims

`marktd` validates every request using a JWT Bearer token. The JWT must contain these
custom claims (in addition to standard OIDC claims):

| Claim | Type | Required | Description |
|---|---|---|---|
| `sub` | `string` | yes | Principal identifier |
| `mako_tenant` | `string` | yes | GLN of the tenant this principal belongs to |
| `mako_roles` | `string[]` | yes | Roles, e.g. `["NB"]`, `["LF"]`, `["MSB","NB"]` |
| `mako_sparte` | `string[]` | no | Optional commodity scope, e.g. `["Strom","Gas"]` |

Configure your OIDC provider (Keycloak, Zitadel, Auth0, Entra ID) to populate
`mako_tenant` and `mako_roles` from your user store or service account attributes.

**Supported signing algorithms: RS256, ES256, PS256.**
HS256/HS512 are rejected — symmetric algorithms are not acceptable for OIDC.

---

## Authorization: Cedar ABAC

`marktd` uses [Cedar](https://www.cedarpolicy.com/) (AWS PARC model) for fine-grained
Attribute-Based Access Control. The policy file is loaded once at startup.

### Default policy (`policies/marktd.cedar`)

The shipped policy has **five `permit` statements over 49 actions**, and every one
of them starts from the same tenant equality. Three of the five add a role.

| Group | Actions | Extra condition |
|---|---|---|
| Tenant-wide reads | 23 `read-*` actions plus `use-mcp` — every read except the ESA consent pair below | none beyond the tenant |
| Ordinary writes | `write-malo`, `write-melo`, `write-nb-contract`, `write-partner`, `write-versorgungsstatus`, `write-device`, `write-sr`, `write-netzzugang`, `write-msb-rv-gas`, `write-melo-msb`, `write-bilanzierung`, `write-mabis-zp`, `write-lokationszuordnung` | none beyond the tenant |
| Grid-operator writes | `write-preisblatt`, `write-nelo`, `write-tranche`, `write-malo-grid`, `write-grundversorger`, `write-energiemix`, `write-mmma-preis`, `dispatch-pricat` | `mako_roles` contains `NB` |
| ESA consent | `read-einwilligung`, `write-einwilligung` | `mako_roles` contains `MSB` **or** `ESA` |
| Operations | `manage-subscription`, `manage-fanout` | `mako_roles` contains `ADMIN` |

The **Endpoints** table below names the action each route checks; this table says
which role that action needs.

```cedar
// Only NB-role principals may publish a price sheet or dispatch a PRICAT.
permit(
    principal,
    action in [
        Action::"write-preisblatt", Action::"write-nelo", Action::"write-tranche",
        Action::"write-malo-grid",  Action::"write-grundversorger",
        Action::"write-energiemix", Action::"write-mmma-preis",
        Action::"dispatch-pricat"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};
```

### Context fields

The tenant and the roles arrive in the **`context`**, not as attributes on the
principal entity — a policy written against `principal.tenant` compiles and then
denies every caller, because no such attribute is ever populated.

| Field | Value |
|---|---|
| `context.principal_tenant` | `mako_tenant` JWT claim |
| `context.principal_roles` | `mako_roles` JWT claim |
| `context.resource_tenant` | This deployment's own `[markt] tenant` |

### Denied response

```json
{
  "error": "Forbidden",
  "detail": "access denied"
}
```

The detail is deliberately uninformative: naming the action and the resource
tenant would tell an unauthorised caller which tenant it reached.

### Four ways authorization fails silently, and the guard that pins them

`marktd` has **no global auth middleware**. `Claims` is an axum
`FromRequestParts` extractor, so a handler that simply does not name it is
served to anyone — the §42 EnWG Energiemix routes shipped that way. Cedar is
default-deny, so the mirror-image mistake is just as quiet: an action checked in
code that appears in no policy is a permanent `403`, which is how the §36 Abs. 2
Grundversorger routes and the whole `/admin/fanout/dlq` surface were dead.

`services/marktd/tests/authorization_guard.rs` reads the sources and refuses a
build where any of these hold:

| Check | The defect it catches |
|---|---|
| Every action checked in code appears in the policy | A permanent `403` on a live route |
| Every action in the policy is checked somewhere | A dead grant nothing enforces |
| Every handler module extracts `Claims` **and** calls the enforcer | An unauthenticated or unauthorized route |
| No handler reads `Claims` as an `Extension` | Claims injected by a layer are not verified by the extractor |
| No request body type carries a `tenant` field | A caller naming its own tenant defeats the isolation key |

### Custom policies

Replace `policies/marktd.cedar` and restart `marktd`; policies are loaded at
startup. The guard above runs against whatever is in that file, so a custom
policy that drops an action fails the build rather than the request.

---

## REST API

Interactive docs: `http://localhost:8180/api/v1/docs/`

OpenAPI spec: `GET /api/v1/openapi.json`

### The BO4E gate

Every `PUT` carrying a BO4E payload runs the same four stages —
`mako_markt::bo4e::decode`, described in
[The BO4E gate](@/docs/architecture/domain-model.md#the-bo4e-gate) — and every
refusal is a `422` naming the stage in `code`:

```json
{
  "error": "MARKTLOKATION carries 1 out-of-schema enum value(s) at: sparte",
  "code":  "bo4e.unknown_enum",
  "paths": ["sparte"]
}
```

Two consequences are specific to `marktd`, which stores what it accepts:

- **The stored `data` is the canonical round-trip**, and the typed columns
  beside it are derived from the same object, so a column cannot disagree with
  the document it shadows. That is also why the strict-enum stage matters here:
  `Unknown` serialises back as the literal `"UNKNOWN"`, so skipping it would
  replace a caller's value rather than merely accept it.
- **The envelope never asks for a field the BO declares.** `sparte` on a NeLo,
  `nutzung`/`verbrauchsart`/`ist_fernschaltbar`/`malo_id`/`melo_id` on a
  TechnischeRessource, `konfigurationsprodukte` on a SteuerbareRessource and
  `zaehler_typ`/`eichung_bis` on a Zaehler are derived from the payload. Where
  the BO declares no such field — `nelos.nb_mp_id`, a MaLo's `fallgruppe`,
  `abwicklungsmodell` and `fernsteuerbar` — the envelope keeps it and an upsert
  leaves the column alone.

Writes that touch a shadowed column merge into the JSONB in the same statement,
so the EDIFACT Stammdatenänderung patch and the `konfigurationsprodukte`
sub-resource cannot leave the column and the document disagreeing.

### Endpoints

| Method | Path | Cedar action | Description |
|---|---|---|---|
| `GET` | `/health/live` | — | Liveness (no DB, no auth) |
| `GET` | `/health/ready` | — | Readiness (bounded DB ping, no auth). Mounted by `mako_service::run` |
| `GET` | `/metrics` | — | Prometheus (no auth) |
| `PUT` | `/api/v1/malos/{malo_id}` | `write-malo` | Upsert Marktlokation through [the BO4E gate](#the-bo4e-gate); pushes to makod MaLo cache |
| `GET` | `/api/v1/malos/{malo_id}` | `read-malo` | Get Marktlokation as typed `rubo4e::current::Marktlokation` (canonical BO4E camelCase) |
| `GET` | `/api/v1/malos/{malo_id}/lastprofil` | `read-malo` | Lastprofil (SLP/TLP) assigned to the Marktlokation |
| `GET` | `/api/v1/malos` | `read-malo` | List Marktlokationen (schema-drift records silently filtered) |
| `PUT` | `/api/v1/melos/{melo_id}` | `write-melo` | Upsert Messlokation through [the BO4E gate](#the-bo4e-gate) |
| `GET` | `/api/v1/melos/{melo_id}` | `read-melo` | Get Messlokation as typed `rubo4e::current::Messlokation` |
| `GET` | `/api/v1/melos/{melo_id}/standorteigenschaften` | `read-melo` | BO4E `Standorteigenschaften` for the MeLo |
| `PUT` | `/api/v1/partners/{mp_id}` | `write-partner` | Upsert trading partner as a `Geschaeftspartner` through [the BO4E gate](#the-bo4e-gate); the stored form is the canonical camelCase round-trip |
| `GET` | `/api/v1/partners/{mp_id}` | `read-partner` | Get trading partner — returns a `geschaeftspartner` field with the typed `rubo4e::current::Geschaeftspartner` payload (graceful fallback for legacy records) |
| `GET` | `/api/v1/partners` | `read-partner` | List partners |
| `GET` | `/api/v1/partners/{mp_id}/as4-address` | `read-partner` | AS4 endpoint URL and certificate for a partner |
| `GET` | `/api/v1/partners/{mp_id}/marktteilnehmer` | `read-partner` | BO4E `Marktteilnehmer` view of a partner (typed `marktrolle`/`rollencodetyp`, mp_id → `rollencodenummer`). Note: partner PUTs with the legacy literal role `"LFG"` are rejected 422 — model gas suppliers as `LF` + Rollencodetyp `DVGW` |
| `GET/PUT` | `/api/v1/mmma-preise/gas/{year}/{month}` | `read-mmma-preis` / `write-mmma-preis` | Gas MMM Abrechnungspreise (Trading Hub Europe / MGV, monthly) — `{mehr_ct_kwh, minder_ct_kwh}`; queried by `netzbilanzd` for INVOIC 31007/31008 billing and `invoicd` check 6 validation |
| `GET` | `/api/v1/mmma-preise/gas` | `read-mmma-preis` | List all Gas MMM price records (newest first; `?limit=`) |
| `POST` | `/api/v1/mmma-preise/import-trigger` | `write-mmma-preis` | Run the monthly THE/ÜNB price import now, instead of waiting for the scheduled sweep |
| `GET/PUT` | `/api/v1/mmm-preise/strom/{year}/{month}` | `read-mmma-preis` / `write-mmma-preis` | Strom Mehr-/Mindermengenpreise — `{mehr_ct_kwh, minder_ct_kwh}`, keyed on the application month alone. Read by `netzbilanzd` for MMM INVOIC 31005/31006 and `invoicd` check 6 |
| `PUT` | `/api/v1/preisblaetter/{nb_mp_id}` | `write-preisblatt` | Upsert price sheet + store versioned snapshot + emit `de.markt.pricat.published` |
| `GET` | `/api/v1/preisblaetter/{nb_mp_id}` | `read-preisblatt` | Get price sheet valid on date |
| `GET` | `/api/v1/pricat/{nb_mp_id}/history` | `read-pricat` | List PRICAT version history (newest first) |
| `GET` | `/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}` | `read-pricat` | PRICAT dispatch audit log for a version |
| `POST` | `/api/v1/pricat/{nb_mp_id}/dispatch` | `dispatch-pricat` | Enqueue (re-)dispatch of latest PRICAT to all active LF partners |
| `GET` | `/api/v1/versorgung/{malo_id}` | `read-versorgungsstatus` | Current VersorgungsStatus; add `?at=YYYY-MM-DD` for point-in-time |
| `GET` | `/api/v1/versorgung/{malo_id}/history` | `read-versorgungsstatus` | Full supply-state change history (newest first, paged) |
| `PUT` | `/api/v1/versorgung/{malo_id}` | `write-versorgungsstatus` | Upsert VersorgungsStatus (ERP-driven override) |
| `GET` | `/api/v1/grundversorger/{nb_mp_id}` | `read-grundversorger` | Grundversorger Feststellung (§36 Abs. 2 EnWG); `?sparte=STROM\|GAS` |
| `PUT` | `/api/v1/grundversorger/{nb_mp_id}` | `write-grundversorger` | Upsert the Feststellung (NB role) — read by the processd EoG gap closure. Optional `default_bilanzkreis` deposits the GPKE-Teil-4 default BK applied when an EoG completes without the E/G supplying its own (EoG ohne Antwort). |
| `POST` | `/api/v1/esa/einwilligungen` | `write-einwilligung` | Grant an ESA consent (§49 Abs. 2 Nr. 9 MsbG). Emits `de.markt.einwilligung.erteilt`. Evidence-agnostic |
| `GET` | `/api/v1/esa/einwilligungen` | `read-einwilligung` | List active consents (`?esa_mp_id=`) |
| `GET` | `/api/v1/esa/einwilligungen/{id}` | `read-einwilligung` | Get a consent |
| `DELETE` | `/api/v1/esa/einwilligungen/{id}` | `write-einwilligung` | Revoke (GDPR Art. 7(3)) — emits `de.markt.einwilligung.widerrufen` and fires the 17008 Abbestellung at makod, once **per covered location and no Messprodukt**: a location may carry several subscriptions and makod stops every one of them |
| `PUT` | `/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}` | `write-einwilligung` | Record the prices of an **accepted QUOTES 15003 Angebot** (`esa_messprodukt_preise`). Filed by makod on the ORDRSP 19011 — the moment the offer becomes the agreement |
| `GET` | `/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}?at=` | `read-einwilligung` | The prices in force on `at`. Read by invoicd to check an INVOIC 31009: an ESA has **no Preisblatt** (§35 MsbG leaves the Entgelt for a Zusatzleistung to be agreed per request), so the offer it ordered against is the price basis |
| `PUT`/`GET` | `/api/v1/esa/framework/{msb_mp_id}/{esa_mp_id}` | `write-einwilligung` / `read-einwilligung` | Bilateral EDI@Energy framework agreement + AS4 cert state |
| `PUT` | `/api/v1/esa/messprodukte/{msb_mp_id}` | `write-einwilligung` | Record which optional **Kapitel-4.6 Messprodukte** this MSB serves an ESA, and in which Abo mode (`E_0252` Prüfschritt 2, `E_0256` Prüfschritte 4/5). A code outside Kapitel 4.6 is refused with `422` — the catalogue of orderable products is code, not data |
| `GET` | `/api/v1/esa/messprodukte/{msb_mp_id}/{messprodukt}?at=` | `read-einwilligung` | Does this MSB serve the product on `at`, in which Abo mode? See [below](#notes-on-four-rows-above) |
| `GET` | `/api/v1/esa/subscriptions/{bestellung_ref}` | `read-einwilligung` | Which **Messprodukt** an ORDERS 17007 Belegnummer subscribed to. `edmd`'s Typ-2 surveillance is the caller: an inbound MSCONS 13027 names only the Belegnummer (`SG1 RFF+AGI`), and the cadence hangs off the product. `404` → fall back to the configured threshold |
| `GET` | `/api/v1/esa/consent-check` | `read-einwilligung` | Gate an ESA message (`?esa_mp_id=&msb_mp_id=&location_id=&perspective=`) → `{allowed, code, reason}`. `perspective=msb_inbound` (default, lenient: missing record = self-assertion) or `esa_outbound` (strict: missing record = no lawful basis). makod calls this before running the Wertebestellung workflow |
| `GET` | `/api/v1/mabis-zp` | `read-mabis-zp` | Every Bilanzierungsgebiet → MaBiS-Zählpunkt assignment for the tenant |
| `GET` | `/api/v1/bilanzierungsgebiete/{eic}/mabis-zp` | `read-mabis-zp` | Resolve the MaBiS-Zählpunkt (MSCONS SG6 `LOC+172`) for a territory. `404` is the signal `mabis-syncd` turns into a refused submission — it must never be read as "use the Bilanzierungsgebiet EIC instead" |
| `PUT` | `/api/v1/bilanzierungsgebiete/{eic}/mabis-zp` | `write-mabis-zp` | Assign the MaBiS-Zählpunkt (NB role). `400` for a Meldepunkt equal to the EIC, and for one that is not a 33-character Zählpunktbezeichnung — the length check catches *another* territory's 16-character EIC. Enforced at the API and by table `CHECK`s |
| `PUT` | `/api/v1/netzzugang/antraege` | `write-netzzugang` | Upsert a §20b EnWG Netzzugangsplattform request (makod `netzzugang` adapter projection). Emits `de.markt.netzzugang.antrag.updated` |
| `GET` | `/api/v1/netzzugang/antraege` | `read-netzzugang` | List §20b requests (`?status=&netzanschluss_id=`) |
| `GET` | `/api/v1/netzzugang/antraege/{id}` | `read-netzzugang` | Get a §20b request |
| `PATCH` | `/api/v1/netzzugang/antraege/{id}/status` | `write-netzzugang` | Advance lifecycle (`erfasst → uebermittelt → bestaetigt/abgelehnt`, optional `platform_ref`; optional `expected_version` → 412 on mismatch). Used by the makod sender and by the operator recording the platform's answer |
| `PUT` | `/api/v1/msb-rahmenvertraege-gas` | `write-msb-rv-gas` | Upsert a Gas MSB-Rahmenvertrag conclusion (GeLi Gas 3.0 Tenor 13–16; `status=anpassung_erforderlich` tracks the BK7-17-026 migration duty). Idempotent on `(tenant, gnb_mp_id, msb_mp_id, valid_from)`; optimistic `version` → 412; rejects `valid_to < valid_from` |
| `GET` | `/api/v1/msb-rahmenvertraege-gas` | `read-msb-rv-gas` | List Gas MSB framework contracts (`?msb_mp_id=&status=`) |
| `GET` | `/api/v1/msb-rahmenvertraege-gas/{id}` | `read-msb-rv-gas` | Get one Gas MSB framework contract |
| `GET` | `/api/v1/nelos` | `read-nelo` | List NeLos (`?nb_mp_id=` filters by Netzbetreiber) |
| `GET` | `/api/v1/nelos/{id}` | `read-nelo` | Get a NeLo by EIC / BDEW Codenummer |
| `PUT` | `/api/v1/nelos/{id}` | `write-nelo` (NB role) | Insert or update a NeLo through [the BO4E gate](#the-bo4e-gate). `sparte` is **derived from the payload** and must be `STROM` or `GAS`; only `nb_mp_id`, which `Netzlokation` declares no field for, rides in the envelope |
| `GET` | `/api/v1/tranchen` | `read-tranche` | List Tranchen (`?malo_id=` filters by parent MaLo) |
| `GET` | `/api/v1/tranchen/{id}` | `read-tranche` | Get a Tranche |
| `PUT` | `/api/v1/tranchen/{id}` | `write-tranche` (NB role) | Insert or update a Tranche (GPKE Teil 4 „Daten der Tranche") |
| `GET` | `/api/v1/malos/{malo_id}/grid` | `read-malo-grid` | MaLo grid topology (Netzgebiet, Bilanzierungsgebiet) |
| `PUT` | `/api/v1/malos/{malo_id}/grid` | `write-malo-grid` (NB role) | Upsert grid record from NIS/GIS |
| `GET` | `/api/v1/preisblaetter-messung/{msb_mp_id}` | `read-preisblatt` | `PreisblattMessung` valid on date (MSB metering tariffs); includes `auf_abschlaege` |
| `PUT` | `/api/v1/preisblaetter-messung/{msb_mp_id}` | `write-preisblatt` | Upsert MSB metering price sheet |
| `GET/PUT` | `/api/v1/preisblaetter-ka/{nb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattKonzessionsabgabe` valid on date |
| `GET/PUT` | `/api/v1/preisblaetter-dienstleistung/{msb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattDienstleistung` valid on date (MSB services) |
| `GET/PUT` | `/api/v1/preisblaetter-hardware/{msb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattHardware` valid on date (MSB devices) |
| `GET` | `/api/v1/steuerbare-ressourcen/{sr_id}` | `read-sr` | Get a `SteuerbareRessource` by SR-ID |
| `PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}` | `write-sr` | Upsert a `SteuerbareRessource` |
| `GET/PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` | `read-sr` / `write-sr` | List or atomically replace the §14a `Konfigurationsprodukte` of a `SteuerbareRessource` |
| `GET` | `/api/v1/technische-ressourcen/{tr_id}` | `read-device` | Get a `TechnischeRessource` by `TrId` |
| `DELETE` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}` | `write-sr` | Remove one Konfigurationsprodukt |
| `PUT` | `/api/v1/technische-ressourcen/{tr_id}` | `write-device` | Upsert a `TechnischeRessource` (E-mobility, generation, storage) |
| `GET` | `/api/v1/malos/{malo_id}/technische-ressourcen` | `read-device` | List `TechnischeRessource` for a `MaLo` |
| `GET` | `/api/v1/malos/{id}/lokationen` | `read-lokationszuordnung` | Recursive `Lokationszuordnung` graph from a MaLo (`?at=YYYY-MM-DD`) |
| `GET` | `/api/v1/malos/{id}/buendel` | `read-lokationszuordnung` | First-class **Lokationsbündel** rooted at a MaLo — the bundle projected from the typed graph plus its structural-integrity status (`valid` + `validation_error`; ≥1 MeLo required) |
| `GET` | `/api/v1/melos/{id}/lokationen` | `read-lokationszuordnung` | Recursive `Lokationszuordnung` graph from a MeLo |
| `PUT` | `/api/v1/lokationszuordnungen` | `write-lokationszuordnung` | Upsert a directed location graph edge (`lokationsbuendelcode` extracted into a typed column). Note the single-write-path invariant: a MeLo `PUT` reconciles the `melo→malo` graph edge in the same transaction (previous edges closed with `valid_to`, never deleted), so the `melo.malo_id` FK and the graph cannot drift |
| `DELETE` | `/api/v1/lokationszuordnungen/{von_id}/{nach_id}` | `write-lokationszuordnung` | Hard-delete an edge pair (all temporal variants) |
| `GET` | `/api/v1/melos/{melo_id}/zaehler` | `read-device` | List `Zaehler` for a MeLo (typed `Vec<ZaehlerResponse>` with `data: rubo4e::current::Zaehler`) |
| `GET` | `/api/v1/melos/{melo_id}/msb` | `read-melo-msb` | The MSB responsible for the MeLo on `?at=YYYY-MM-DD` (default today) — WiM Teil 2 UC 4.1.1 historical Werteanfrage routing |
| `PUT` | `/api/v1/melos/{melo_id}/msb` | `write-melo-msb` | Record a dated MSB assignment (`{ msb_mp_id, valid_from }`); closes the previously-open assignment atomically |
| `GET` | `/api/v1/melos/{melo_id}/msb/history` | `read-melo-msb` | Full dated MSB timeline for the MeLo (newest first) |
| `PUT` | `/api/v1/malos/{malo_id}/bilanzierung` | `write-bilanzierung` | Upsert a **BO4E `Bilanzierung`** (BO #3) through [the BO4E gate](#the-bo4e-gate), keyed on `(malo, bilanzierungsbeginn)`; typed columns (Bilanzkreis/Aggregationsverantwortung/Prognosegrundlage/Fallgruppe) extracted, full BO stored as JSONB |
| `GET` | `/api/v1/malos/{malo_id}/bilanzierung` | `read-bilanzierung` | The Bilanzierung effective at `?at=<RFC3339\|YYYY-MM-DD>` (default now) — point-in-time by validity window |
| `GET` | `/api/v1/malos/{malo_id}/bilanzierung/history` | `read-bilanzierung` | Full Bilanzierung history for the MaLo (newest validity-start first) |
| `GET` | `/api/v1/melos/{melo_id}/sharing-eligibility` | `read-sharing-eligibility` | §42c EnWG metering **capability** — qualifies via Zählerstandsgangmessung (§2 Satz 1 Nr. 27 MsbG) **or** viertelstündliche RLM. Returns `capability`, `basis`, `required_action`, `reasons`, `bilanzierungsgebiet`, and the master-data `evidence` it decided from. |
| `GET` | `/api/v1/zaehler/{zaehler_id}/zaehlwerke` | `read-device` | List `Zaehlwerk` registers for a Zaehler (typed `Vec<Zaehlwerk>` from JSONB) |
| `PUT` | `/api/v1/zaehler/{zaehler_id}` | `write-device` | Upsert a `Zaehler` through [the BO4E gate](#the-bo4e-gate). `zaehler_typ` and `eichung_bis` are **derived from the BO**, never taken beside it |
| `GET` | `/api/v1/zaehler/{zaehler_id}/geraete` | `read-device` | List `Geraete` for a `Zaehler` (typed `Vec<GeraetResponse>` with `data: rubo4e::current::Geraet` + `konfigurationen: Vec<GeraetKonfiguration>`) |
| `GET` | `/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}` | `read-device` | Get a single `Geraet` — full BO4E payload + `konfigurationen`; 404 when not found |
| `GET/PUT` | `/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}/konfigurationen` | `read-device` / `write-device` | Get or atomically replace typed `GeraetKonfiguration` entries (MsbG §23); PUT emits `de.markt.geraet.konfiguration.updated` |
| `GET/PUT` | `/api/v1/zaehler/{zaehler_id}/register` | `read-device` / `write-device` | List/upsert iMSys TOU registers (`ZaehlzeitRegister`) |
| `GET/PUT` | `/api/v1/zaehler-register/{register_id}/saisons` | `read-device` / `write-device` | List/upsert seasonal TOU windows (`ZaehlzeitSaison`) |
| `GET` | `/api/v1/zaehler/{zaehler_id}/tariff-zone` | `read-device` | Resolve HT/NT/EINZEL tariff zone for a given local datetime |
| `GET` | `/api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen` | `read-device` | Return typed `rubo4e::current::Zaehlzeitdefinition` assembled from `zaehler_register` + `zaehler_saisons`; `?valid_only=true` filters to current registers |
| `PUT` | `/api/v1/geraete/{geraet_id}` | `write-device` | Upsert a `Geraet` through [the BO4E gate](#the-bo4e-gate) |
| `GET` | `/api/v1/nb-contracts/{id}` | `read-nb-contract` | Get NB network contract with typed BO4E `Vertrag` payload |
| `PUT` | `/api/v1/nb-contracts/{id}` | `write-nb-contract` | Upsert NB network contract as a `Vertrag` through [the BO4E gate](#the-bo4e-gate); emits `de.markt.nb-contract.updated` |
| `GET` | `/api/v1/nb-contracts` | `read-nb-contract` | List NB contracts (`?nb_mp_id=...` required) |
| `GET` | `/api/v1/nb-contracts/by-malo/{malo_id}` | `read-nb-contract` | Contract in force for a MaLo on `?on=` (default today) — the Netznutzer and its type |
| `GET/PUT` | `/api/v1/energiemix/{nb_mp_id}` | `read-energiemix` / `write-energiemix` | §42 EnWG Energiemix for a Netzbetreiber (`?year=`) |
| `GET` | `/api/v1/energiemix/{nb_mp_id}/history` | `read-energiemix` | Every published Energiemix year for the NB |
| `GET` | `/api/v1/subscriptions` | `manage-subscription` | List durable fan-out subscriptions |
| `GET/PUT/DELETE` | `/api/v1/subscriptions/{id}` | `manage-subscription` | Read, register or remove one subscriber |
| `POST` | `/api/v1/subscriptions/{id}/test` | `manage-subscription` | Send a probe CloudEvent to the subscriber's webhook |
| `GET` | `/api/v1/correlations` | `read-correlation` | List process correlations (`?malo_id=`, `?workflow=`) |
| `GET` | `/api/v1/correlations/{id}` | `read-correlation` | One correlation by `process_id` or `erp_order_id` |
| `POST` | `[webhook] inbound_path` | — | Inbound CloudEvent from `makod` (HMAC-verified); appended to `event_log` before fan-out. Default `/api/v1/mako/events`; it must match `makod`'s `[erp] webhook_url` |
| `GET` | `/admin/fanout/dlq` | `manage-fanout` | List unresolved DLQ entries |
| `POST` | `/admin/fanout/dlq/{event_id}/{subscriber_id}/retry` | `manage-fanout` | Re-deliver a dead-lettered delivery |
| `DELETE` | `/admin/fanout/dlq/{event_id}/{subscriber_id}` | `manage-fanout` | Discard a dead-lettered delivery |
| `GET` | `/admin/events` | `manage-fanout` | CloudEvent replay log — `?from=RFC3339&to=RFC3339&type=&limit=` |
| `GET` | `/api/v1/openapi.json` · `/api/v1/docs` | — | OpenAPI 3.1 document and the interactive browser |

### Notes on four rows above

**The Messprodukt answer carries three states.** `als_abo` and `als_einmalig`
are `true`, `false`, or `null`. „Not carried" is a decision the MSB made;
„nothing recorded" is not, and the `E_0252` / `E_0256` walks escalate on the
difference rather than treating an empty catalogue as a refusal. The answer also
folds in the dated **Pflicht** rule: a Pflichtprodukt is served whatever the
catalogue holds (BNetzA Mitteilung Nr. 3, § 34 Abs. 2 S. 2 Nr. 10 MsbG).

**Strom MMM prices are one nationwide series.** § 13 Abs. 3 StromNZV requires
*einheitliche* Mehr-/Mindermengenpreise computed from monthly market prices, and
the BDEW determines and publishes them centrally — so the application month is
the entire key. There is no per-VNB and no per-ÜNB variant, and a schema that
allowed one would invite a settlement priced against the wrong sheet.

**A NeLo is Strom or Gas.** BO4E's `Sparte` has seven values; MaKo is a
two-commodity market, so this endpoint's profile narrows it. A `WASSER` or
`FERNWAERME` Netzlokation is a payload error, not a Sparte marktd has no code
for yet.

**The Gas MSB-Rahmenvertrag has a deadline.** KoV XV Anlage 8 applies from
01.10.2026; `status=anpassung_erforderlich` is how an operator finds the
contracts still to be migrated. Every upsert emits
`de.markt.msb-rahmenvertrag-gas.updated` carrying `version`, `valid_from`,
`valid_to` and `signed_at`.

### Consent expiry

A consent stops being a lawful basis two ways — Widerruf (GDPR Art. 7(3)) and
expiry — and `E_0256` Prüfschritt 8 names both in one code: `A08` is „widerrufen
**oder ihre Gültigkeit ist abgelaufen**". Both owe the same message, because the
only protocol-level stop is the ORDERS 17008 the ESA sends.

`DELETE` covers the Widerruf; an **hourly sweep** closes every consent whose
`valid_to` has passed and stops the deliveries it authorised, through the same
code path. Idempotent by construction: `revoked_at` is stamped in the statement
that selects, so a second sweep — or a `DELETE` racing it — returns nothing.

Both paths emit `de.markt.einwilligung.widerrufen`. The payload's `grund`
(`einwilligung_widerrufen` / `einwilligung_abgelaufen`) is what lets an audit
tell them apart.

---

## Price Sheets — PreisblattNetznutzung

Price sheets record the Netznutzungspreise for a Netzbetreiber. Validity is
derived from the BO4E `gueltigkeit.startdatum` / `gueltigkeit.enddatum` fields
inside the JSON payload.

### PUT request body

```json
{
  "data": {
    "_typ": "PREISBLATTNETZNUTZUNG",
    "bezeichnung": "Netznutzungspreise 2025 — 9900357000004",
    "gueltigkeit": { "startdatum": "2025-10-01", "enddatum": "2026-09-30" },
    "marktteilnehmer": {
      "_typ": "MARKTTEILNEHMER",
      "marktrolle": "NB",
      "rollencodenummer": "9900357000004",
      "rollencodetyp": "BDEW"
    },
    "preispositionen": [ ... ]
  },
  "bo4e_version": "202607.1.0"
}
```

### GET response

```json
{
  "data":         { "_typ": "PREISBLATTNETZNUTZUNG", ... },
  "source":       "api",
  "bo4e_version": "202607.1.0",
  "updated_at":   "2025-10-01T08:15:00Z",
  "zeitvariable_preispositionen": [
    {
      "_typ": "ZEITVARIABLEPREISPOSITION",
      "bezeichnung": "HT-Arbeitspreis",
      "zaehlzeitregister": "HT",
      "preisreferenz": "ENERGIEMENGE",
      "preis": { "wert": "8.35", "einheit": "CT", "bezugswert": "KWH" }
    }
  ]
}
```

`zeitvariable_preispositionen` contains the `ZeitvariablePreisposition` array
from the BO4E payload. Note the shape of `preis`: it is a BO4E `Preis` COM, and
its `einheit` is a **`Waehrungseinheit`** — only `CT` or `EUR`. The per-what
lives in `bezugswert` (a `Mengeneinheit`) and the what-it-is-a-price-of in
`preisreferenz` (`ENERGIEMENGE` / `LEISTUNG` / `ANZAHL` / `ZEITRAUM` /
`PAUSCHAL`). A composite such as `"CT_PRO_KWH"` in `einheit` decodes to the
`Unknown` catch-all. The COM has no `bezugsgroesse` and no `zeitfenster` field:
the time bands live in the `zaehlzeitdefinition` this position's
`zaehlzeitregister` code points into. If the price sheet has no ToU tariffs the field is omitted
(serialized with `#[serde(skip_serializing_if = "Vec::is_empty")]`). This array
is consumed by `netzbilanzd` for §14a Modul 2 ToU billing (BNetzA BK6-22-300).

Query parameter: `?date=YYYY-MM-DD` (defaults to today in CET/CEST).

### Source field

Every price sheet row carries a `source` field:

| Source | Set by | Semantics |
|---|---|---|
| `api` | REST `PUT /api/v1/preisblaetter/{nb_mp_id}` | Operator-supplied via REST API or ERP export |
| `mako` | PRICAT ingest path (invoicd/makod) | Received as EDIFACT from the NB |

**Operator-override rule:** an `api` entry always supersedes a `mako` entry for
the same NB GLN and validity period. A price sheet uploaded via the REST API
cannot be silently overwritten by an incoming EDIFACT PRICAT.

Enforced in SQL:

```sql
ON CONFLICT (nb_mp_id, valid_from)
DO UPDATE SET data = EXCLUDED.data, ...
WHERE preisblaetter.source <> 'api' OR EXCLUDED.source = 'api';
```

### PRICAT 27003 dispatch pipeline

#### `preisblaetter` vs `pricat_versions` — why both

The two tables hold the same JSON and are not redundant. They answer different
questions, so they carry opposite constraints:

| | `preisblaetter` | `pricat_versions` |
|---|---|---|
| Question | *Which Preisblatt is valid on date X?* | *Which document did we transmit, to whom, and when?* |
| Kind | Current state | Audit trail |
| Overlap | Forbidden — `EXCLUDE USING gist` guarantees one answer per party per day | Allowed by design |
| Correction | Replaces the row | Adds a version; earlier ones stay queryable |
| Read by | `invoic-checker`, `netzbilanzd`, `invoicd` for tariff resolution | Operators and auditors, via `/pricat/{nb}/history` |

`pricat_versions.data` is a **snapshot**, not a reference. A PRICAT sent last
quarter remains a fact after the sheet behind it is corrected, and pointing at
the live row would let that correction retroactively rewrite what was
transmitted.

Only the NNE sheet feeds this ledger: PRICAT 27003 is the NB→LF *Preisblatt
Netznutzung* transmission, so `preisblaetter_messung` (MSB) and
`preisblaetter_konzessionsabgabe` have no PRICAT of their own.

#### The write path

Every `PUT /api/v1/preisblaetter/{nb_mp_id}` call, in **one transaction**:

1. Writes or updates the current price sheet in `preisblaetter`
2. Inserts a versioned snapshot in `pricat_versions` keyed on `(nb_mp_id, tenant, valid_from)`
3. Enqueues `de.markt.pricat.published` on the outbox → fan-out to ERP webhook subscribers

A background task then dispatches PRICAT 27003 per active LF partner via
`MakodClient`. The three writes share one transaction so the sheet, the
snapshot and the dispatch trigger cannot diverge.

The dispatch audit log (`pricat_dispatch_log`) records every outbound dispatch attempt
(NB × LF pair) with outcome and `makod` process ID.

```mermaid
sequenceDiagram
    participant ERP
    participant marktd
    participant pricat_versions
    participant MakodClient
    participant AS4

    ERP->>marktd: PUT /api/v1/preisblaetter/{nb_mp_id}
    marktd->>preisblaetter: UPSERT (existing)
    marktd->>pricat_versions: UPSERT versioned snapshot
    marktd-->>ERP: 204 No Content
    marktd-)fanout: de.markt.pricat.published
    Note over marktd: background task
    marktd->>pricat_versions: mark_queued(version_id)
    loop per active LF partner
        marktd->>MakodClient: POST /api/v1/commands dispatch-pricat-27003
        MakodClient->>AS4: PRICAT 27003
        marktd->>pricat_dispatch_log: log(lf_mp_id, outcome)
    end
    marktd->>pricat_versions: mark_done(version_id)
```

**Auto-dispatch on LF partner registration:** when `PUT /api/v1/partners/{mp_id}` registers
a partner with `marktrolle = "LF"`, the latest PRICAT version for the operator's NB GLN is
automatically re-queued for dispatch to the new partner.

**Manual re-dispatch:** `POST /api/v1/pricat/{nb_mp_id}/dispatch` resets dispatch state to
`queued` so the background task picks it up again. Use this after AS4 outages or to
force distribution to newly on-boarded partners.

**Dispatch states:**

| State | Meaning |
|---|---|
| `pending` | Version stored; no dispatch started yet |
| `queued` | Dispatch task picked this version up |
| `done` | All active LF partners successfully reached |
| `error` | Last dispatch attempt failed; will be retried on next poll |

---

## Trading Partners — `Geschaeftspartner` BO4E

`marktd` stores trading partners in the `partners` table, keyed by `mp_id`
(BDEW-Codenummer or DVGW-Codenummer). Every `PUT` validates and normalises the
partner payload as `rubo4e::current::Geschaeftspartner`.

### Schema validation on PUT

The record has two halves, validated differently:

| Part | Validation |
|---|---|
| `marktrolle`, `rollencodetyp`, `sparte` — top-level record fields | Typed enums; `422` when serde's lenient decode falls through to `Unknown` (a typo, or the legacy EDIFACT `LFG` — BO4E models a gas supplier as `LF` + `rollencodetyp: DVGW`) |
| `channels` — the BO4E payload | Decoded as `rubo4e::current::Geschaeftspartner`: `_typ` injected when absent, `422` when it names another type, every declared enum checked, re-serialised to canonical camelCase before storage |

> `marktrolle`, `rollencodetyp` and `marktteilnehmerstatus` are **`Marktteilnehmer`**
> fields — BO4E does not define them on `Geschaeftspartner`. Putting them inside
> `channels` does not fail: the decode absorbs them as extension fields, stores
> them unvalidated and reads them back as nothing. The role fields belong at the
> top level, where they are typed.

```bash
# Register a trading partner (LF). The role fields are top-level; `channels`
# carries the Geschaeftspartner.
curl -s -X PUT "http://marktd:8180/api/v1/partners/9904234560001" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Muster Energieversorgung GmbH",
    "marktrolle":    "LF",
    "rollencodetyp": "BDEW",
    "sparte":        "STROM",
    "makoadresse":   ["https://as4.muster-ev.de/as4/in"],
    "channels": {
      "_typ": "GESCHAEFTSPARTNER",
      "anrede": "FRAU",
      "adresse": {
        "_typ": "ADRESSE",
        "strasse": "Musterstraße",
        "hausnummer": "1",
        "postleitzahl": "10115",
        "ort": "Berlin",
        "landescode": "DE"
      }
    }
  }'
```

### Typed GET response

`GET /api/v1/partners/{mp_id}` returns a structured response with a `geschaeftspartner`
field containing the typed `rubo4e::current::Geschaeftspartner` payload:

```json
{
  "mp_id":   "9904234560001",
  "display_name": "Muster Energieversorgung GmbH",
  "marktrolle": "LF",
  "rollencodetyp": "BDEW",
  "makoadresse": ["https://as4.muster-ev.de/as4/in"],
  "geschaeftspartner": {
    "_typ": "GESCHAEFTSPARTNER",
    "anrede": "FRAU",
    "adresse": { "_typ": "ADRESSE", "strasse": "Musterstraße", "hausnummer": "1", ... }
  },
  "version": 3,
  "updated_at": "2026-07-11T09:15:00Z"
}
```

A partner record with no schema-valid payload is returned with the raw `channels`
JSONB in the `geschaeftspartner` field.

---

## Database Schema

`marktd` uses a single SQL schema file (`migrations/0001_initial.sql`).
Migrations run automatically at startup via `sqlx migrate run`.

### Tables

| Table | Purpose |
|---|---|
| `malo` | Marktlokationen — JSONB payload, `bo4e_version`, GIN index |
| `rollenzuordnungen` | Temporal NB/LF/MSB role assignments per MaLo |
| `melo_msb_zuordnungen` | Per-MeLo **dated MSB timeline** — `(tenant, melo_id, msb_mp_id, valid_from, valid_to)`; point-in-time MSB resolution for WiM Teil 2 UC 4.1.1 (a MaLo can bundle MeLos with divergent MSB history). Derived from **IFTSTA 21012** (`derive_msb_zuordnung`), never from the *vorläufige* Anmeldebestätigung 55043 |
| `bilanzierungen` | **BO4E `Bilanzierung`** (BO #3) per MaLo — `bilanzierungsbeginn/ende` validity, typed `bilanzkreis`/`aggregationsverantwortung`/`prognosegrundlage`/`fallgruppenzuordnung`, full BO in `data JSONB`. Writing a currently-effective one **derives** `malo.fallgruppe`. `bilanzierungsmethode` and `bilanzierungsgebiet` stay on `malo`: they are `Marktlokation` fields (BO #12), not `Bilanzierung` ones |
| `lokationszuordnungen` | Location graph edges — `(tenant, von_id, von_typ, nach_id, nach_typ, valid_from, valid_to)`; `von_typ`/`nach_typ` are the canonical BO4E `Lokationstyp` codes (`MALO`/`MELO`/`NELO`/`SR`/`TR`); recursive-CTE BFS traversal |
| `melo` | Messlokationen — JSONB payload, `bo4e_version` |
| `partners` | Trading partners (GLN → channels) — JSONB |
| `subscriptions` | ERP webhook registrations |
| `process_correlation` | Running/completed MaKo process tracking per MaLo |
| `processed_events` | Inbound event idempotency log |
| `preisblaetter` | NB price sheets (Netznutzung) — `source CHECK ('api','mako')`, GIN index |
| `preisblaetter_messung` | MSB metering price sheets — same source-override protection |
| `preisblaetter_konzessionsabgabe` | KA price sheets per (NB, Sparte, Kundengruppe) |
| `preisblaetter_dienstleistung` · `preisblaetter_hardware` | MSB service and device price sheets |
| `nb_energiemix` | §42 EnWG grid-area energy mix per NB and year |
| `mmma_preise_gas` · `mmm_preise_strom` | Mehr-/Mindermengen settlement prices — THE monthly (Gas), one nationwide BDEW series (Strom) |
| `versorgungsstatus` | VersorgungsStatus per MaLo — `LieferStatus CHECK`, optimistic concurrency `version BIGINT`. Carries **no** supplier column |
| `lf_zuordnung` | Who supplies a MaLo, as a **list** — one row per (MaLo, LF, Tranche, status) with `prozent` and `status ∈ Angekuendigt \| Aktiv`. A constraint trigger holds the `Aktiv` shares of one MaLo to ≤ 100 %. Rows are deleted when the assignment ends; the trace is the history snapshot |
| `versorgungsstatus_history` | Append-only audit log of every supply-state transition — carries the whole assignment list as JSONB, which is what `?at=` and `/history` resolve against |
| `grundversorger` | §36 Abs. 2 EnWG Feststellung per (NB, Sparte), plus the `default_bilanzkreis` an EoG falls back to |
| `mabis_zaehlpunkte` | Bilanzierungsgebiet EIC → MaBiS-Zählpunkt (33-char Zählpunktbezeichnung), `CHECK`ed against both substitutions |
| `nb_contracts` | NB network contracts — typed SQL columns (`netzebene`, `bilanzierungsmethode`, `billing_schedule`, `netznutzer_mp_id`, `netznutzer_typ`) + full BO4E `Vertrag` JSONB (`data`) for ERP digital LRV exchange |
| `pricat_versions` | Versioned PRICAT snapshots — `(nb_mp_id, tenant, valid_from)` unique, dispatch state |
| `pricat_dispatch_log` | Dispatch audit log — one row per NB × LF dispatch attempt |
| `nelo` | Netz-Element-Lokationen (Redispatch 2.0) — EIC or BDEW Codenummer, owner NB GLN, JSONB data |
| `tranche` | Tranchen der Marktlokation (GPKE Teil 4 „Daten der Tranche") — keyed by `(tranche_id, tenant)`, parent `malo_id`; `bilanzierungsgebiet`/`netzebene`/`energierichtung` typed columns + BO4E `Tranche` JSONB |
| `malo_grid` | MaLo grid topology — Netzgebiet, Bilanzierungsgebiet, sourced from NIS/GIS |
| `steuerbare_ressourcen` | WiM iMS controllable resources — keyed by SR-ID (`C[A-Z0-9]{9}[0-9]`), linked to MaLo; `konfigurationsprodukte JSONB` for contracted iMS control products  |
| `technische_ressourcen` | E-mobility, generation, storage resources — keyed by TrId; BO4E-aligned `nutzung` (`TechnischeRessourceNutzung`) + `verbrauchsart` (`TechnischeRessourceVerbrauchsart`) + `ist_fernschaltbar` typed columns; linked to MaLo/MeLo |
| `zaehler` | Meter registry — linked to MeLo; `zaehler_typ` (CHECK-constrained to BO4E `Zaehlertyp`) and `eichung_bis` are **derived from** the BO4E payload, which also carries the `zaehlwerke` array |
| `geraete` | Device registry — linked to Zaehler, stores `geraet_typ`, BO4E payload, and `geraet_konfigurationen JSONB` (typed `GeraetKonfiguration[]` per MsbG §23; GIN-indexed for cert-expiry queries) |
| `event_log` | Durable CloudEvent replay log — keyed by `event_id` (unique); indexed by `ce_type` + `received_at` |
| `event_delivery` | One row per (event × subscriber) delivery attempt — the fan-out queue and its dead-letter |
| `zaehler_register` · `zaehler_saisons` | iMSys TOU registers and their seasonal windows, assembled into a `Zaehlzeitdefinition` on read |
| `esa_einwilligungen` | ESA consents (§49 Abs. 2 Nr. 9 MsbG) with their validity window |
| `esa_framework_agreements` | Bilateral EDI@Energy framework agreement + AS4 certificate state per (MSB, ESA) |
| `esa_messprodukt_preise` | The prices of an accepted QUOTES 15003 — an ESA has no Preisblatt |
| `esa_messprodukt_katalog` | Which Kapitel-4.6 Messprodukte an MSB serves an ESA, and in which Abo mode |
| `netzzugang_antraege` | §20b EnWG Netzzugangsplattform requests |
| `msb_rahmenvertraege_gas` | Gas MSB-Rahmenverträge (GeLi Gas 3.0 Tenor 13–16) |

All columns listed below are part of the single **`migrations/0001_initial.sql`** file.
There are no incremental migration files — the initial schema is the authoritative source.

### Temporal integrity — half-open ranges and `EXCLUDE`

Almost everything in `marktd` is dated: which MSB serves a Messlokation, which
NB contract governs a MaLo, which price sheet applies. All of it uses one
convention.

**`[valid_from, valid_to)` is half-open.** `valid_to` is the first day *not*
covered, so the day a successor starts is the day its predecessor ends — the two
carry the same date and consecutive rows tile a period exactly, with no
off-by-one and no gap-of-one-day. A `NULL` `valid_from` reads as −infinity and a
`NULL` `valid_to` as +infinity, so „open-ended" needs no sentinel date.

**Overlap is unrepresentable, not merely discouraged.** Eight tables carry an
`EXCLUDE USING gist` constraint over `daterange(valid_from, valid_to, '[)')`,
mixed with plain equality on the key columns — which is what the `btree_gist`
extension is for. A uniqueness index only stops two rows with the same *start*;
an exclusion constraint stops two rows in force on the same *day*, which is the
condition that actually matters, because every read is
`ORDER BY valid_from DESC LIMIT 1` and would otherwise pick one of two answers
with nothing to say which.

| Constraint | Table | Scoped by |
|---|---|---|
| `rollenzuordnungen_no_overlap` | `rollenzuordnungen` | MaLo × Zuordnungstyp |
| `melo_msb_no_overlap` | `melo_msb_zuordnungen` | tenant × MeLo |
| `nb_contracts_no_overlap` | `nb_contracts` | tenant × MaLo × NB |
| `preisblaetter_no_overlap` | `preisblaetter` | NB |
| `preisblaetter_messung_no_overlap` | `preisblaetter_messung` | MSB |
| `preisblaetter_ka_no_overlap` | `preisblaetter_konzessionsabgabe` | NB × Sparte × Kundengruppe |
| `preisblaetter_dienstleistung_no_overlap` · `preisblaetter_hardware_no_overlap` | the two MSB sheets | MSB |

The price-sheet tables pair the constraint with
`UNIQUE NULLS NOT DISTINCT (…, valid_from)`: under a plain `UNIQUE`, `NULL`s are
distinct, so every re-`PUT` of an open-started sheet inserted another duplicate
instead of updating one.

`services/marktd/tests/temporal_constraints_integration.rs` exercises each of
them against a real PostgreSQL (testcontainers; the tests skip when Docker is
absent), because a missing exclusion constraint is invisible in a unit test — the
second row simply inserts, and the wrong tariff shows up in a settlement months
later.

**An overlap is a `422`, not a `500`.** The constraint fires because the
*request* asked for two rows in force on one day, which is the caller's to fix —
so the refusal names the constraint and says what to move:

```json
{
  "error": "conflicting key value violates exclusion constraint \"melo_msb_no_overlap\": the row's [valid_from, valid_to) window overlaps one already stored. Close the existing row at the new start date, or move valid_from."
}
```

PostgreSQL raises `23P01` (`exclusion_violation`) for these, which is a different
code from the `23514` (`check_violation`) a column bound raises. Only the latter
used to be translated, and only on `lf_zuordnung`, so every one of the eight
constraints above answered `500 internal` — the server saying it broke, when what
broke was the request. Both codes now map to `422` on every write path, and
`services/marktd/tests/overlap_is_a_client_error.rs` drives each of the eight
through the repository the API writes through and asserts the status and the
message.

> **Operator note.** `422` after a `PUT` whose `valid_from`/`valid_to` overlaps
> is the constraint working, not an outage — check the `valid_to` of the row
> already in force. An *abutting* successor (`valid_from` equal to the
> incumbent's `valid_to`) is never an overlap; that is what half-open means.

### Key typed columns on `malo`

| Column | Type | Description |
|---|---|---|
| `netzebene` | `TEXT` | Netzebene (voltage/pressure level) — drives NNE tariff selection |
| `bilanzierungsgebiet` | `TEXT` | Bilanzierungsgebiet code for EEG/KWK allocation |
| `energierichtung` | `TEXT` | `AUSSP` (consumption) / `EINSP` (generation) |
| `gasqualitaet` | `TEXT` | Gas quality (H/L) |
| `bilanzierungsmethode` | `TEXT` | `RLM` \| `SLP` \| `IMS` \| `TLP_*`; drives `netzbilanzd` Leistungspreis routing |
| `regelzone` | `TEXT` | Regelzone EIC code — maps to ÜNB for Redispatch 2.0 + MABIS |
| `fallgruppe` | `TEXT` | GaBi Gas RLM category (e.g. `LNF`, `LF`, `TK`) |
| `fernsteuerbar` | `BOOLEAN` | §14a EnWG „Status der Fernsteuerbarkeit" — `true` = technisch fernsteuerbar, `false` = nicht (UTILMD `CCI+7037` `Z97`/`Z96`) |
| `abwicklungsmodell` | `TEXT` | NZR-EMob (BK6-20-160 Anlage 6) — `MODELL_1` (balanced at the Marktlokation) \| `MODELL_2` (balanced in a Ladepunktbetreiber's Bilanzierungsgebiet), from UTILMD `CCI+ZA2++ZE9`/`ZF0`. `NULL` means no counterparty has stated one — **not** Modell 1 |

### Key typed columns on `melo`

| Column | Type | Description |
|---|---|---|
| `netzebene_messung` | `TEXT` | Netzebene where the meter is installed |
| `regelzone` | `TEXT` | Regelzone EIC code — extracted from `standorteigenschaften.eigenschaftenStrom[0].regelzoneEic` |
| `standorteigenschaften` | `JSONB` | Full `Standorteigenschaften` object (GIN indexed) for WiM Stammdaten enrichment |

## NB Network Contracts — `Vertrag` BO4E

NB network contracts are stored in `nb_contracts` as **both** fast-query typed SQL
columns (`netzebene`, `bilanzierungsmethode`, `billing_schedule`, `valid_from`, `valid_to`)
**and** a full BO4E `Vertrag` JSON payload for ERP digital LRV exchange.

`vertragsart` and `vertragsstatus` are extracted from the payload as indexed columns,
enabling SQL-level filtering. A `de.markt.nb-contract.updated` CloudEvent is emitted on
every successful upsert so ERP subscribers can rebuild `Vertrag` caches without polling.

### `PUT /api/v1/nb-contracts/{contract_id}`

```json
{
  "malo_id":             "51238696012",
  "nb_mp_id":            "9900357000004",
  "sparte":              "STROM",
  "netzebene":           "NS",
  "bilanzierungsmethode": "SLP",
  "billing_schedule":    "MONTHLY",
  "netznutzer_mp_id":    "9905555550003",
  "netznutzer_typ":      "LIEFERANT",
  "valid_from":          "2026-10-01",
  "valid_to":            null,
  "data": {
    "_typ":            "VERTRAG",
    "vertragsart":     "NETZNUTZUNGSVERTRAG",
    "vertragsstatus":  "AKTIV",
    "sparte":          "STROM",
    "vertragsbeginn":  "2026-10-01T00:00:00+00:00"
  }
}
```

`data` is optional — if omitted, a minimal `Vertrag` is auto-constructed from the other
fields (`vertragsart = NETZNUTZUNGSVERTRAG`, `vertragsstatus = AKTIV`).

**Validation:** `_typ` must be `"VERTRAG"` (422 if wrong). All enum fields
(`vertragsart`, `vertragsstatus`) are validated against `rubo4e::current::Vertrag`.

### `GET /api/v1/nb-contracts/{contract_id}` response

```json
{
  "contract_id":         "nv-9900357000004-51238696012",
  "malo_id":             "51238696012",
  "nb_mp_id":            "9900357000004",
  "sparte":              "STROM",
  "netzebene":           "NS",
  "bilanzierungsmethode": "SLP",
  "billing_schedule":    "MONTHLY",
  "netznutzer_mp_id":    "9905555550003",
  "netznutzer_typ":      "LIEFERANT",
  "valid_from":          "2026-10-01",
  "valid_to":            null,
  "data": {
    "_typ":           "VERTRAG",
    "vertragsart":    "NETZNUTZUNGSVERTRAG",
    "vertragsstatus": "AKTIV",
    "sparte":         "STROM",
    "vertragsbeginn": "2026-10-01T00:00:00+00:00"
  },
  "vertragsart":    "NETZNUTZUNGSVERTRAG",
  "vertragsstatus": "AKTIV",
  "version":        1,
  "tenant":         "9900357000004"
}
```

`netzebene` accepts all Strom voltage levels (`NS`/`MS`/`MSP`/`HSP`/`HS`/`HöS`/`HöS/HS`)
and Gas pressure levels (`GND`/`GMT`/`GHD`). `bilanzierungsmethode` accepts `RLM`, `SLP`,
`IMS`, and TLP variants.

### The Netznutzer, and the Selbstzahler

`netznutzer_mp_id` is the counterparty — the party that owes the Netznutzungsentgelt.
`netznutzer_typ` says what kind of party it is:

| Value | Meaning |
|---|---|
| `LIEFERANT` (default) | The ordinary case: an all-inclusive supply contract, the LF is Netznutzer |
| `LETZTVERBRAUCHER` | **Selbstzahler** — „Netznutzer ohne All-Inklusiv-Vertrag". The Letztverbraucher pays the Netznutzung itself |

A Selbstzahler takes the LF role in GPKE (Teil 1, Vorbemerkung) and is an ordinary
LF on the wire — nothing routes differently. Registered as a Marktpartner with the
LF role, he already receives the PRICAT Preisblatt and the „sonstige Leistung"
invoice Teil 2 Kap. 3.4.4 / 3.4.5 owe him. The flag exists for the one carve-out,
the LF's Lieferantenwechsel-Meldungen, where
[`processd`](@/docs/services/processd.md#selbstzahler-the-lieferantenwechsel-carve-out)
holds a Wechsel (`E03`) for the operator rather than answering it automatically.

### `GET /api/v1/nb-contracts/by-malo/{malo_id}?on=YYYY-MM-DD`

The contract in force for a MaLo on a date (default today), or `404`. This is the read
`processd` uses before it decides an Anmeldung.

---

## Inbound Events from `makod`

`marktd` receives process lifecycle events from `makod` via `POST /api/v1/mako/events`.

### Enable push in makod config

```toml
# makod.toml
[erp]
webhook_url    = "http://marktd:8180/api/v1/mako/events"
webhook_secret = "shared-hmac-secret"
```

Inbound delivery is idempotent — duplicates are detected by `event_id` and silently
acknowledged without re-processing.

### Automatic `malo.bilanzierungsmethode` + `malo.fallgruppe` update

When `marktd` receives `de.mako.process.initiated` for PIDs 55001/55077 (GPKE) or 44001 (GeLi
Gas), it calls `MaloRepository::patch_typenmerkmal()` to update the `malo` table:

| Payload field | Column | Source |
|---|---|---|
| `bilanzierungsmethode` | `malo.bilanzierungsmethode` | UTILMD `TM+EM` segment (Z01→SLP, Z02→RLM, Z04→IMS) extracted by `makod` adapter |
| `fallgruppe` | `malo.fallgruppe` | UTILMD `TM+Z10` segment (Gas GaBi RLM category) extracted by `makod` adapter |

This keeps the MaLo's billing mode and GaBi Gas Fallgruppe in sync with the UTILMD
without requiring a separate ERP `PUT /api/v1/malos` call. The update is best-effort:
if the MaLo row does not yet exist, the patch silently no-ops (the values will be set
on the first `PUT /api/v1/malos`).

```bash
# After a 55001 Anmeldung, verify the update:
curl -s "http://marktd:8180/api/v1/malos/10001234558" \
  -H "Authorization: Bearer <token>" | jq '.bilanzierungsmethode, .fallgruppe'
# → "SLP", null     (for a Strom SLP point)
# → "RLM", "Z01"   (for a Gas RLM point with GaBi category Z01)
```

---

## `PUT /api/v1/malos` — MaLo Typed Columns & Schema Validation

Every `PUT /api/v1/malos/{malo_id}` call:
1. **Validates** the incoming `data` payload as `rubo4e::current::Marktlokation`:
   - Auto-injects `_typ: "MARKTLOKATION"` if absent
   - Returns **422** if `_typ` is present but not `MARKTLOKATION`
   - Returns **422** if any typed field contains an unknown enum value
     (e.g. `"bilanzierungsmethode": "UNKNOWN"`)
2. **Normalises** to canonical camelCase BO4E form before storage. The
   repository serialises the *typed* `Marktlokation`, so the canonical form is
   the only shape that reaches the `data JSONB` column; keys the schema does
   not define round-trip losslessly through the `_additional` extension map.
3. **Derives typed columns** from that same typed object — never from string
   lookups on its JSON:

| Column | `Marktlokation` field | Vocabulary | Purpose |
|---|---|---|---|
| `netzebene` | `netzebene` | `NSP` \| `MSP` \| `HSP` \| `HSS` \| `MSP_NSP_UMSP` \| `HSP_MSP_UMSP` \| `HSS_HSP_UMSP` \| `HD` \| `MD` \| `ND` | Voltage/pressure level for the NNE billing tier |
| `bilanzierungsgebiet` | `bilanzierungsgebiet` | EIC (object type `Y`, Area) | Drives `processd` NB check 4 |
| `gasqualitaet` | `gasqualitaet` | `H_GAS` \| `L_GAS` | Gas tariff routing |
| `energierichtung` | `energierichtung` | `EINSP` \| `AUSSP` | `EINSP` (Einspeisung) **feeds** the grid — a generating MaLo; `AUSSP` (Ausspeisung) **draws** from it — a consuming one. The direction is named from the grid's point of view |
| `bilanzierungsmethode` | `bilanzierungsmethode` | `RLM` \| `SLP` \| `IMS` \| `TLP_GEMEINSAM` \| `TLP_GETRENNT` \| `PAUSCHAL` | Drives `netzbilanzd` Leistungspreis routing — RLM requires `spitzenleistung_kw` |
| `regelzone` | `regelzone` | EIC | Maps the MaLo to its ÜNB for MABIS IFTSTA 21000 routing and Redispatch 2.0 Stammdaten forwarding |

Every enum column holds a **BO4E wire value and nothing else**: the value comes
from the enum's own `as_wire()`, and a SQL `CHECK` constraint listing that
enum's `VARIANTS` refuses anything else. A test in `mako-markt` compares the
`CHECK` lists against the schema, so a `rubo4e` bump that adds a variant fails
the build instead of rejecting valid data at run time.

All columns are `NULL` when the BO4E payload does not carry the field.

**Three columns are deliberately not in that table.** `fallgruppe` (the GaBi RLM
Fallgruppe) and `abwicklungsmodell` (NZR-EMob) are `Bilanzierung` fields, and
`fernsteuerbar` (§14a EnWG) has no BO4E field at all — none is on
`Marktlokation`, so a `PUT /malos/{id}` leaves all three alone. They are written
by the `Bilanzierung` resource and by the UTILMD Stammdatenänderung path
(`TM+Z10`, `CCI+Z24++Z96/Z97`, `CCI+ZA2++ZE9/ZF0`) respectively.

`abwicklungsmodell` is nevertheless `CHECK`ed against the BO4E
`Abwicklungsmodell` enum like the shadowed columns, because its vocabulary *is*
BO4E's even though its writer is not the payload.

The call also automatically pushes the NB and MSB GLNs to `makod`'s MaLo cache
via `PUT /admin/malo/{malo_id}` — fire-and-forget; `makod` failure does not fail
the API call.

Fields forwarded to `makod`:

| Field | Source |
|---|---|
| `nb_mp_id` | `rollenzuordnung[]` entry with `zuordnungstyp == "NB"` or `"GNB"` |
| `msb_mp_id` | `rollenzuordnung[]` entry with `zuordnungstyp == "MSB"` or `"GMSB"` |
| `bilanzierungsgebiet` | `Marktlokation.bilanzierungsgebiet` |
| `netzgebiet` | `Marktlokation.netzgebietsnr` |
| `sparte` | `sparte` field |

**`MaloResponse`** (GET) exposes the typed columns as top-level fields
alongside the validated `data` payload, so a caller can filter without parsing
the BO:

```json
{
  "malo_id": "10001234558",
  "sparte": "STROM",
  "version": 3,
  "netzebene": "NSP",
  "bilanzierungsgebiet": "11YDE-RWE-NETZ-1",
  "gasqualitaet": null,
  "energierichtung": "AUSSP",
  "bilanzierungsmethode": "SLP",
  "regelzone": "10YDE-EON------1",
  "rollenzuordnung": [...],
  "data": { "_typ": "MARKTLOKATION", ... }
}
```

---

## ERP Subscriptions & Fan-Out

`marktd` delivers CloudEvents 1.0 to every matching ERP subscriber when master data changes or when `makod` lifecycle events arrive. The fan-out worker runs in a dedicated Tokio task and delivers independently per subscriber — a slow or unavailable ERP does not block other subscribers.

**Ordering is per aggregate.** A delivery is held back while an earlier event about the
same Marktlokation (`event_log.seq`, `event_delivery.ordering_key`) is still outstanding to
the same subscriber; events about different MaLos never wait for each other. A dead-lettered
delivery stops blocking its key, so head-of-line blocking is bounded by `max_retry_attempts`.

**`roles` and `sparten` filter on CloudEvents extensions.** An empty array matches
everything; otherwise the event's `marktrole` / `marktsparte` must appear in it. An event
with no `marktsparte` is not Sparte-scoped (a Marktpartner, a subscription test) and matches
every `sparten` filter.

### Event types

| Source | Event type | Trigger |
|---|---|---|
| marktd master data | `de.markt.malo.updated` | `PUT /api/v1/malos/{malo_id}` |
| marktd master data | `de.markt.malo.stammdaten-geaendert` | UTILMD Stammdatenänderung applied to a MaLo (GPKE Teil 4 / GeLi Gas) — carries the applied `patch` |
| marktd master data | `de.markt.stammdaten.geaendert` | UTILMD Stammdatenänderung applied to a non-MaLo object (MeLo/NeLo/Tranche) — carries `objekt` + the applied `patch` |
| marktd master data | `de.markt.partner.updated` | `PUT /api/v1/partners/{mp_id}` |
| marktd NB contract | `de.markt.nb-contract.updated` | `PUT /api/v1/nb-contracts/{id}` — carries `vertragsart`, `version`, `tenant` in `data` |
| marktd PRICAT | `de.markt.pricat.published` | `PUT /api/v1/preisblaetter/{nb_mp_id}` |
| marktd supply | `de.markt.versorgung.changed` | **every** VersorgungsStatus transition — announce (55001/55077/44001), confirm (55002/55078/44002), reject (55003/55080/44003), end (55005/44005), EoG (55013/44013) — and the REST upsert. Carries the resulting `lieferstatus`, `zuordnungen`, `lieferende`, `eog_seit`, `sparte`, `version` |
| marktd supply | `de.markt.versorgung.gap-detected` | An interval no supplier covers — a Lieferende (55005/44005) the announced successor does not follow on, or a Fall-b Bestätigung (55002/55078/44002) whose Altlieferant released earlier. §38 EnWG gap-closure trigger (consumer: `processd`) |
| marktd supply | `de.markt.versorgung.eog-begonnen` | 55013/44013 completed → `begin_eog_supply` (Ersatz-/Grundversorgung active; consumer: `processd`) |
| makod process relay | `de.mako.process.initiated` | forwarded from `makod` ingest |
| makod process relay | `de.mako.aperak.accepted` | forwarded from `makod` ingest |
| makod process relay | `de.mako.aperak.rejected` | forwarded from `makod` ingest |
| makod process relay | `de.mako.aperak.timeout` | forwarded from `makod` ingest |
| makod process relay | `de.mako.process.completed` | forwarded from `makod` ingest |
| makod process relay | `de.mako.process.failed` | forwarded from `makod` ingest |

> `de.mako.*` events carry the CloudEvents extensions `makoconvid`, `makopid`,
> `makoworkflow`, and `marktrole` (role of the counterparty: `NB`, `LF`, `MSB`,
> `BIKO`). Downstream services (`invoicd`, `edmd`, `obsd`) filter on `makopid`
> to select only the event types they care about.

### Register a subscription

```bash
curl -X PUT http://localhost:8180/api/v1/subscriptions/erp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "endpoint_url": "https://erp.example.com/mdm/events",
    "secret":       "mysecret64hexchars",
    "event_types":  ["de.markt.malo.updated", "de.markt.pricat.published",
                     "de.mako.process.completed"]
  }'
```

`event_types` entries are matched with the canonical shared matcher
(`mako_events::matches` — the same one agentd trigger patterns use): exact
types, trailing-`*` prefixes (`de.markt.*`), full mid-pattern globs
(`de.*.rechnung.erstellt`) and `?` single-character wildcards all work; an
empty list subscribes to everything.

### Webhook payload

```
POST https://erp.example.com/mdm/events
Content-Type: application/cloudevents+json
webhook-signature: v1,<base64>
```

```json
{
  "specversion":     "1.0",
  "id":              "01932a4f-7b3e-4c5d-8f6a-9e0b1c2d3e4f",
  "source":          "urn:mako:marktd:tenant:9900357000004",
  "type":            "de.mako.process.completed",
  "time":            "2025-10-01T08:15:00+02:00",
  "subject":         "018f3a2b-7c4e-7d5f-8a9b-0c1d2e3f4a5b",
  "datacontenttype": "application/json",
  "makoconvid":      "018f3a2b-7c4e-7d5f-8a9b-0c1d2e3f4a5b",
  "makopid":         55001,
  "makoworkflow":    "gpke-lieferbeginn",
  "marktrole":       "LF",
  "data": { "_typ": "MARKTLOKATION", "marktlokationsId": "51238696012", ... }
}
```

### Signature verification

`webhook-signature` carries an HMAC-SHA256 hex digest over the raw request body,
prefixed with `sha256=`, computed with the `secret` registered in the subscription
(the workspace-wide format emitted by `mako_service::webhook::sign`).

The subscription secret is an **integrity** key stored in plaintext in
`subscriptions.webhook_secret` — protect it with least-privilege database
grants and storage-level encryption; it never protects confidentiality of
customer data.

```python
import hmac, hashlib

def verify(body: bytes, secret: str, header: str) -> bool:
    received = header.removeprefix("sha256=")          # strip the algorithm prefix
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, received)
```

Return `200 OK` for duplicates — fan-out retries on non-2xx.

### Durability & retry behaviour

Fan-out is **persist-before-fan-out**. Every produced event is written to the
durable `event_log` outbox (the full CloudEvent envelope) *before* any delivery
is attempted, so a marktd crash never loses an in-flight event. A two-phase
worker drains it:

1. **Fan-out** — claims pending `event_log` rows, resolves the matching
   subscribers, and snapshots one `event_delivery` row per subscriber (in one
   transaction with stamping `fanned_out_at`).
2. **Deliver** — claims due `event_delivery` rows with a lease
   (`FOR UPDATE SKIP LOCKED`), signs + POSTs each, and on failure backs off
   (30 s → 5 m → 30 m → 2 h) or, after the attempt cap, marks `dead_lettered_at`.

**The claim counts the attempt, not the outcome.** `attempts` is incremented by
the claim itself, so the retry budget is spent by *trying*, whatever happens
next. It used to be incremented only when a failure was successfully recorded,
which meant a lost outcome write — or a worker that died between the POST and
the write — advanced nothing: the delivery retried for ever and never
dead-lettered. Because the claim is per-aggregate FIFO, one such delivery also
held back every later event with the same `ordering_key` for that subscriber, so
a single stuck row silently froze one Marktlokation's whole stream to it. A
sweep at the top of each cycle dead-letters rows whose budget is spent and whose
lease has lapsed, so the dead-letter decision does not rest on one write
succeeding either.

A crash at any boundary is recoverable from the two tables; subscribers receive
at-least-once and dedup on the CloudEvent `id`. This durability is required by
**§ 147 AO / GoBD / §41 EnWG**: a silent drop of a `de.mako.process.initiated`
event to `invoicd` would mean the INVOIC plausibility check never runs.

### Dead-letter queue (DLQ)

A delivery that exhausts its attempts is flagged `dead_lettered_at` on its
`event_delivery` row (a status-column DLQ — no separate table). Operators inspect
and remediate via the admin endpoints, keyed by `(event_id, subscriber_id)`:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/admin/fanout/dlq` | List dead-lettered deliveries (newest first, paged) |
| `POST` | `/admin/fanout/dlq/{event_id}/{subscriber_id}/retry` | Requeue for immediate redelivery |
| `DELETE` | `/admin/fanout/dlq/{event_id}/{subscriber_id}` | Discard without retry |

```bash
# Inspect dead-lettered deliveries
curl http://localhost:8180/admin/fanout/dlq \
  -H "Authorization: Bearer $TOKEN" | jq '.[] | {event_id, subscriber_id, attempts, last_error}'

# Requeue a specific (event, subscriber) delivery
curl -X POST http://localhost:8180/admin/fanout/dlq/$EVENT_ID/$SUBSCRIBER_ID/retry \
  -H "Authorization: Bearer $TOKEN" | jq .

# Discard after manual ERP re-import
curl -X DELETE http://localhost:8180/admin/fanout/dlq/$EVENT_ID/$SUBSCRIBER_ID \
  -H "Authorization: Bearer $TOKEN"
```

Requeuing redelivers with the subscriber's current webhook secret.

### Prometheus metrics (`/metrics`)

`GET /metrics` exposes four **gauges** in Prometheus text format — not counters,
so alert on level and trend rather than on `rate()`. All four are sampled
together once a minute in one query, so a scrape reads values that describe the
same instant.

| Metric | Description |
|--------|-------------|
| `marktd_fanout_dlq_depth` | Deliveries that exhausted their retries and were dead-lettered. Non-zero means a subscriber is not receiving market events — they are retained, never dropped (§ 147 AO / GoBD). Inspect via `GET /admin/fanout/dlq` |
| `marktd_fanout_pending` | Events written to the outbox but not yet fanned out. A rising value means the fan-out worker is behind or stopped — the first signal an operator wants |
| `marktd_active_subscriptions` | Rows in `subscriptions` with `active` set |
| `marktd_processed_events` | Rows in the inbound idempotency table. Bounded by the retention sweep, so this is **not** an all-time total: unbounded growth means the sweep is not running |

`mako_service` adds `mako_http_requests_total{method,path,status}` and the
`mako_http_request_duration_seconds` histogram on the same endpoint.

Scrape via Prometheus `static_configs` or a `ServiceMonitor` in Kubernetes.

---

## Process Correlations

Track which MaKo processes are currently running against a given MaLo:

```bash
curl "http://localhost:8180/api/v1/correlations/51238696012" \
  -H "Authorization: Bearer $TOKEN"
```

```json
[
  {
    "malo_id":      "51238696012",
    "pid":          55001,
    "conv_id":      "018f3a2b-...",
    "initiated_at": "2026-07-01T08:00:00Z",
    "status":       "RUNNING"
  }
]
```

---

## Docker Deployment

```bash
docker pull ghcr.io/hupe1980/mako-marktd:0.19.0

docker run -d \
  --name marktd \
  -p 8180:8180 \
  -v /etc/marktd/marktd.toml:/etc/marktd/marktd.toml:ro \
  -e MARKTD_CONFIG=/etc/marktd/marktd.toml \
  -e DATABASE_URL=postgres://marktd:secret@postgres/marktd \
  -e MAKOD_API_KEY=my-api-key \
  -e MAKOD_WEBHOOK_SECRET=my-webhook-secret \
  ghcr.io/hupe1980/mako-marktd:0.19.0
```

The config path is an **environment variable**, not a flag: `marktd` parses only
`--check`, so a trailing `--config=…` is read by nothing and the container falls
back to `./marktd.toml`, which is not there — and then refuses to start for want
of `[markt] tenant`.

---

## Health Checks

| Endpoint | DB check | Use for |
|---|---|---|
| `GET /health/live` | no | Kubernetes `livenessProbe` |
| `GET /health/ready` | yes (ping) | Kubernetes `readinessProbe` |

---

## Common Issues

**`401 Unauthorized`**
JWT validation failed. Check: correct `[oidc] issuer`, token not expired,
`mako_tenant` claim present.

**`403 Forbidden`**
Cedar denied the request. Check: `mako_tenant` matches tenant GLN in URL,
`mako_roles` contains required role (`NB` for `write-preisblatt`).

**`404 Not Found` on GET preisblatt**
No price sheet valid on the requested date. Upload one first with
`PUT /api/v1/preisblaetter/{nb_mp_id}`.

**Price sheet not updating (`mako` source rejected)**
Intentional. An existing `source=api` row cannot be overwritten by
`source=mako` — operator-override protection is working. Use the REST API
to update operator-controlled price sheets.

**Anmeldungen refused, deferred or auto-rejected**
Not a `marktd` question. Automated STP decisions live in
[`processd`](@/docs/services/processd.md)'s NB module — `marktd` holds no rule
and no `auto_accept` switch. What `marktd` owes those decisions is data, and the
three usual gaps are: the MaLo has never been `PUT` at all; its
`rollenzuordnung` carries no `zuordnungstyp = "NB"` entry naming this
deployment's `tenant`; or no price sheet's `gueltigkeit.startdatum` /
`enddatum` brackets the UTILMD's process date.

**`relation "malo" does not exist`**
Migrations have not run. Check `DATABASE_URL` and PostgreSQL connectivity.
`marktd` runs `sqlx migrate run` automatically on startup.

---

## MCP Server

`marktd` exposes an MCP (Model Context Protocol) Streamable HTTP server at
`POST /mcp` / `GET /mcp` (spec 2025-11-25). It has its **own** credential layer —
`[mcp]`, an API key or OIDC, configured independently of the REST `[oidc]` block
— and the resulting principal is then checked against the single Cedar action
`use-mcp`.

**Tools (17)**, every one a read:

| Tool | Description |
|---|---|
| `get_malo` / `list_malo` | Fetch one MaLo / list MaLos (Sparte, Netzgebiet, NB) |
| `get_melo` / `get_melo_standorteigenschaften` | MeLo record / site properties |
| `get_partner` / `list_partners` | Market-partner registry (MP-ID, name, roles) |
| `get_preisblatt` | Price sheet for an NB MP-ID and date (Netznutzung) |
| `list_pricat_versions` | PRICAT version history. Dispatch is REST-only (`POST /api/v1/pricat/{nb_mp_id}/dispatch`, Cedar `dispatch-pricat`, NB role) — `use-mcp` is one blanket gate over the whole MCP surface and cannot tell a read tool from a destructive one |
| `get_versorgungsstatus` / `get_versorgungsstatus_history` / `get_versorgung_at` | Supply state — current, history, point-in-time |
| `get_rollenzuordnung` | Temporal NB/MSB/LF role assignments for a MaLo |
| `get_nb_contract` | NB contract record for a MaLo |
| `get_nb_energiemix` | §42 EnWG grid-area energy mix |
| `get_correlation` | Process correlation for a MaLo |
| `get_technische_ressource` / `get_steuerbare_ressource` | §14a device registry |

**Prompts (4)** — guided multi-tool investigations for LLM clients. The names an
MCP client calls are hyphenated: `lookup-malo`, `investigate-supply-gap`,
`versorgungswechsel-tracking`, `grid-topology`.

---

## VersorgungsStatus

`marktd` maintains one `VersorgungsStatus` record per MaLo, derived automatically from
inbound CloudEvents.  Every write appends a row to `versorgungsstatus_history` in the same
transaction, enabling full audit trails and `?at=YYYY-MM-DD` point-in-time queries.

### Supplier-transition lifecycle

A Lieferantenwechsel spans three distinct phases, each triggering a targeted partial update:

| Phase | CloudEvent | PID | Operation | Effect |
|---|---|---|---|---|
| **Announce** | `process.initiated` | 55001 / 55077 / 44001 | `announce_lf_next` | Adds one `Angekuendigt` assignment with its share (100 %, or the Tranchengröße `9991000002090`) and Zuordnungsbeginn. Does **not** change `lieferstatus`. Several may be pending at once — see [GPKE rule A06](#gpke-rule-a06). |
| **Confirm** | `process.completed` | 55002 / 55078 / 44002 | `confirm_supply` | Promotes **the named supplier's** announcement to `Aktiv` and displaces the running assignment on the same Tranche — only that one, so an Anmeldung for a 25 % Tranche leaves the LFA holding the other 75 % in place. `lieferstatus = Beliefert`. |
| **End** | `process.completed` | 55005 / 44005 | `end_supply` | Removes the named running assignment, or all of them when none is named. `lieferstatus` becomes `Unbeliefert` only once **no** assignment is left: one LFA leaving a tranchierte Marktlokation does not make it unsupplied. Announced assignments are preserved. An uncovered interval → emits `de.markt.versorgung.gap-detected`. |
| **EoG** | `process.completed` | 55013 / 44013 | `begin_eog_supply` | `lieferstatus = Ersatzversorgung`/`Grundversorgung` (per `data.eog_art`), `lf_mp_id = E/G`, `eog_seit = Zuordnungsbeginn` (may be retroactive — anchors §38 Abs. 4). Resolves the Bilanzkreis from the completion payload, else the NB's deposited `default_bilanzkreis` (EoG ohne Antwort). Emits `de.markt.versorgung.eog-begonnen` (incl. `bilanzkreis`). |
| **Stammdatenänderung** | `process.completed` | GPKE Teil 4 / GeLi Gas Änderung PIDs | `patch_stammdaten` | Object-generic apply — see below |
| **Clear** | `process.completed` | 55003 / 55080 / 44003 | `clear_lf_next` | Ablehnung Anmeldung: drops **the refused supplier's** announcement, leaving a rival the NB has not ruled on in place. Also the write behind 55038 / 44038 „Aufhebung einer zukünftigen Zuordnung". Idempotent. |

All operations are idempotent under at-least-once fan-out delivery, and each emits
`de.markt.versorgung.changed` carrying the state it produced.

#### `patch_stammdaten` — one apply for every object

The `data.objekt` marker selects the typed-column patch; each `Some` field
overwrites its column via `COALESCE`, the JSONB payload and `version` are left
alone, and an object mako does not hold locally is a no-op.

| `objekt` | Table | Fields the apply carries beyond the plain columns |
|---|---|---|
| `MARKTLOKATION` | `malo` | §14a `fernsteuerbar` |
| `MESSLOKATION` | `melo` | the MSB-Zuordnung (`CAV+7111=Z91`), recorded on the dated `melo_msb_zuordnungen` timeline by `assign_msb`, effective the Änderungsdatum |
| `NETZLOKATION` | `nelo` | §14a `steuerkanal` |
| `TECHNISCHE_RESSOURCE` | `technische_ressourcen` | `nutzung` (`CCI+7059` Z17/Z50/Z56), `verbrauchsart` (`CAV+7111` Z64/Z65/ZE5/ZA8), `ist_fernschaltbar` |
| `STEUERBARE_RESSOURCE` | `steuerbare_ressourcen` | the Konfigurationsprodukte (below) |
| `TRANCHE` | `tranche` | — |

Each `SG8 SEQ+Z79` product group becomes one BO4E `Konfigurationsprodukt`, with
`produktcode` from `PIA+5` DE 7140, the zugeordneter Marktpartner from
`CAV+Z91`/`ZF0` and `leistungskurvendefinition` from `CCI+Z66`. The contracted
array is **replaced**, not merged.

The apply emits `de.markt.malo.stammdaten-geaendert` for a MaLo and
`de.markt.stammdaten.geaendert` for every other object. Deep MeLo
`standorteigenschaften` are acknowledged without a typed apply — they are
structural-MIG level.

### Schema

```
VersorgungsStatusRecord
├── malo_id              — 11-digit Marktlokations-ID
├── lieferstatus         — Beliefert | Unbeliefert | Grundversorgung | Ersatzversorgung | Ruhend | Stillgelegt
├── zuordnungen[]        — who supplies it, as a list; the authoritative answer
│   ├── lf_mp_id         — Lieferant MP-ID
│   ├── prozent          — share of the Marktlokation (100 untranchiert, 0 < p < 100 per Tranche)
│   ├── tranche_id       — Tranchen-ID (`SG5 LOC+Z21`), null when untranchiert
│   ├── status           — Angekuendigt | Aktiv
│   ├── zuordnungsbeginn — Lieferbeginn of this assignment
│   └── zuordnungsende   — agreed end, once there is one
├── lf_mp_id             — derived: the one Aktiv supplier, null when 0 or >1
├── lf_mp_id_next        — derived: the one Angekuendigt supplier, same rule
├── lieferbeginn         — derived: Zuordnungsbeginn of that single Aktiv assignment
├── lf_next_lieferbeginn — derived: Zuordnungsbeginn of that single announcement
├── lieferende           — announced supply end date of the Marktlokation
├── msb_mp_id            — active Messstellenbetreiber MP-ID
├── nb_mp_id             — Netzbetreiber MP-ID (partition key)
├── eog_seit             — start of a running Ersatz-/Grundversorgung (§38/§36 EnWG)
├── last_process_id      — UUID of the last process that triggered a state change
├── updated_at           — UTC timestamp of last write
└── version              — optimistic-concurrency counter (OCC)
```

The four derived fields are a convenience for the ordinary one-supplier case.
**A `null` there does not mean „unsupplied"** — it also means „several", which is
the normal state of a tranchierte Marktlokation. Read `zuordnungen` to tell the
two apart.

### State machine

```mermaid
stateDiagram-v2
    [*] --> Unbeliefert : MaLo registered

    Unbeliefert --> Unbeliefert : 55001/55077/44001 process.initiated<br/>→ Angekuendigt assignment added
    Unbeliefert --> Beliefert   : 55002/55078/44002 process.completed<br/>→ confirm_supply (Angekuendigt → Aktiv)

    Beliefert --> Beliefert     : 55001/55077/44001 process.initiated<br/>→ next LF announced; running assignments untouched
    Beliefert --> Beliefert     : 55005/44005 on one Tranche<br/>→ end_supply; other Tranchen still run
    Beliefert --> Unbeliefert   : 55005/44005 process.completed<br/>→ end_supply, last assignment gone<br/>(announcements preserved)

    Unbeliefert --> Ersatzversorgung : 55013/44013 process.completed<br/>→ begin_eog_supply (§38 EnWG, eog_seit set)
    Unbeliefert --> Grundversorgung  : 55013/44013 process.completed<br/>→ begin_eog_supply (§36 EnWG, Haushaltskunde)
    Ersatzversorgung --> Grundversorgung : 3 Monate um, Haushaltskunde<br/>(automatic by law, no market message)
    Ersatzversorgung --> Beliefert   : new supply confirmed<br/>→ confirm_supply (clears eog_seit)
    Grundversorgung --> Beliefert    : new supply confirmed<br/>→ confirm_supply

    Beliefert --> Ruhend    : MaKo suspended
    Ruhend --> Beliefert    : Reactivated
    Beliefert --> Stillgelegt : Decommissioned
    Stillgelegt --> [*]
```

**NB gap-closure (§38 EnWG).** A gap is an *interval* no supplier covers, and two
routes lead to one. `end_supply` reaches it when the Lieferende leaves days
before the announced successor starts — or has no successor at all. A
**Bestätigung Anmeldung** reaches it through Fall b: the Altlieferant answered
the Abmeldeanfrage with its own, earlier Lieferendedatum (`E_0624` `A34`), which
rides the completion payload as `lfa_lieferende` while the confirmation stands at
the Zuordnungsbeginn the new supplier asked for. Neither route's message states
both ends; this projection holds them together.

Both emit the same `de.markt.versorgung.gap-detected` with `gap_from`/`gap_until`,
so the `processd` EoG module handles them identically: it resolves the
Grundversorger (`GET /api/v1/grundversorger/{nb_mp_id}`) and dispatches
`gpke.eog.anmelden` (UTILMD 55013).

#### GPKE rule A06

`processd` looks for an announcement by a supplier **other than** the
requesting one before accepting a new Anmeldung. `marktd` records the announcement while
ingesting the `process.initiated`, before fanning the event out, so the Anmeldung under
evaluation is already in the projection by the time the check runs — a bare „is anything
pending?" test would reject every first-time Anmeldung against itself.

Both announcements are kept: the tree has to rule on the rival, and 55038 / 44038's
addressee *is* it.

**Tranchierte Marktlokationen.** An erzeugende Marktlokation can be split across Tranchen
held by several LFA at once, each with its own Aufteilungsfaktor. `E_0623` Prüfschritte
500–540 decide such an Anmeldung on the arithmetic over those shares — „ist ein ausreichend
großer Prozentsatz frei geworden?" — rather than on one supplier's answer, and four of that
tree's six outcomes exist only there. `lf_mp_id` and `lf_mp_id_next` are derived from the
list and are **absent** when several suppliers hold it: there is no single supplier to name,
and naming one arbitrarily would be worse than naming none.

**Conservation.** A constraint trigger holds the `Aktiv` shares of one Marktlokation
to at most 100 %, so a 60/60 split is refused with `422` rather than stored — `E_0623`
Prüfschritt 530 („verbleibt ein Anteil im Bilanzkreis des Netzbetreibers?") reads the
remainder as a *fact*, and cannot tell an over-allocation from a real one. A 60/30
split is untouched: that remainder is what Prüfschritt 530 asks about. The trigger sums
**`Aktiv` rows only**, so the competing `Angekuendigt` announcements above are not bounded
at all — two 100 % announcements on one Marktlokation are a normal, representable state,
and it is `E_0622` Prüfschritt 70 that rules on them, not the schema.

**A Tranche is never the whole.** GPKE Teil 1 § 3.2.1.5: „Eine Tranche umfasst folglich
stets weniger als 100 % der aus einer Marktlokation eingespeisten Energiemenge. Der
Prozentsatz einer Tranche ist immer größer 0% und kleiner als 100%." A row naming a
`tranche_id` at 100 % is Geschäftsvorfall 1 wearing a Tranchen-ID, and is refused. The
untranchierte case — no `tranche_id`, 100 % — is the ordinary one and unaffected.

Every one of these arrives as a `422` naming the constraint it broke: the mapping is on
the SQLSTATE, so a rule added to the schema is classified as a bad request without being
listed in the service.

**Optimistic concurrency.** Every write uses `WHERE malo_id = $1 AND tenant = $2 AND version = $3`.
Conflict → `412 Precondition Failed` → retry after re-read.

### REST API

```http
# Current state
GET  /api/v1/versorgung/{malo_id}

# Point-in-time state (as of end-of-day on that German calendar date, CET/CEST)
GET  /api/v1/versorgung/{malo_id}?at=2025-10-01

# Full state-change history (newest first, paged)
GET  /api/v1/versorgung/{malo_id}/history?page=0&size=50

# Admin override or ERP-driven upsert; supply If-Match: "<version>" for OCC
PUT  /api/v1/versorgung/{malo_id}
```

**Response shape** (`GET /api/v1/versorgung/{malo_id}`):

```json
{
  "malo_id": "51238696012",
  "lieferstatus": "Beliefert",
  "zuordnungen": [
    {
      "lf_mp_id": "4012345000023",
      "prozent": "100.000",
      "tranche_id": null,
      "status": "Aktiv",
      "zuordnungsbeginn": "2026-10-01",
      "zuordnungsende": null
    }
  ],
  "lf_mp_id": "4012345000023",
  "lf_mp_id_next": null,
  "lf_next_lieferbeginn": null,
  "lieferbeginn": "2026-10-01",
  "lieferende": null,
  "msb_mp_id": "9900000000002",
  "nb_mp_id": "9900357000004",
  "eog_seit": null,
  "last_process_id": "...",
  "updated_at": "2026-07-10T08:23:41Z",
  "version": 5
}
```

**Point-in-time query (`?at=YYYY-MM-DD`):** Returns the supply state as it was at
end-of-day on the given date in German local time (CET/CEST).  Returns `404` when no
history exists on or before that date.

```mermaid
sequenceDiagram
    participant ERP
    participant marktd
    participant PostgreSQL

    ERP->>marktd: GET /api/v1/versorgung/51238696012?at=2025-07-01
    marktd->>PostgreSQL: SELECT * FROM versorgungsstatus_history<br/>WHERE malo_id=$1 AND tenant=$2<br/>AND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= '2025-07-01'<br/>ORDER BY valid_from DESC LIMIT 1
    PostgreSQL-->>marktd: snapshot (LieferStatus, LF, NB, …)
    marktd-->>ERP: 200 OK + VersorgungsStatusResponse
```

`processd` reads `GET /api/v1/versorgung/{malo_id}` to drive the LF's automated
responses without ERP involvement (GPKE Teil 1 §5).

---

## MSB Price Sheets — PreisblattMessung

`marktd` stores **MSB metering price sheets** (`PreisblattMessung`) in the
`preisblaetter_messung` table. These cover Messentgelte per Messpreistyp and
form the tariff basis for REQOTE/QUOTES (PIDs 35001/35002/35004/35005) and for
`invoicd` plausibility checks on INVOIC 31009 (MSB-Rechnung).

The API mirrors `PreisblattNetznutzung` exactly but is keyed by `msb_mp_id`
(the MSB's BDEW-Codenummer) instead of `nb_mp_id`.

```bash
# Upload an MSB price sheet (operator or ERP)
curl -s -X PUT "http://marktd:8180/api/v1/preisblaetter-messung/9900012345678" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "data": {
      "_typ": "PREISBLATTMESSUNG",
      "bezeichnung": "Messentgelte 2026",
      "gueltigkeit": { "startdatum": "2026-10-01", "enddatum": "2027-09-30" },
      "preispositionen": [],
      "zeitvariablePreispositionen": [
        { "zaehlzeitregister": "HT", "preis": { "wert": "12.50", "einheit": "CT", "bezugswert": "KWH" } },
        { "zaehlzeitregister": "NT", "preis": { "wert": "8.75",  "einheit": "CT", "bezugswert": "KWH" } }
      ]
    },
    "bo4e_version": "202607.1.0"
  }'

# Retrieve for a billing date — response includes typed zeitvariable_preispositionen
curl -s "http://marktd:8180/api/v1/preisblaetter-messung/9900012345678?date=2026-01-15" \
  -H "Authorization: Bearer <token>"
# → {
#     "data": { ... },
#     "zeitvariable_preispositionen": [
#       { "zaehlzeitregister": "HT", "preis": { "wert": "12.50", "einheit": "CT", ... } },
#       { "zaehlzeitregister": "NT", "preis": { "wert": "8.75",  "einheit": "CT", ... } }
#     ],
#     "auf_abschlaege": [],
#     "schema_drift_count": 0
#   }
```

### §14a Modul 2 — `zeitvariablePreispositionen`

For MSBs that operate under §14a Modul 2 (time-of-use pricing for controllable loads), each `ZeitvariablePreisposition` element in the price sheet **must** carry a non-empty `zaehlzeitregister` band code (e.g. `"HT"`, `"NT"`, `"ST"`). The PUT endpoint validates this:

| Validation | Error |
|-----------|-------|
| Missing `zaehlzeitregister` | 422 — mandatory per §14a Modul 2 (BK6-22-300) |
| `bandNummer` field present | 422 — does not exist in BO4E v202607 |
| Invalid BO4E schema | 422 — `serde_json` schema error |

`invoic-checker` check 4 uses the `zaehlzeitregister` codes to route INVOIC 31009 positions against the correct ToU band price, rather than guessing from `positionstext` keywords.

**Source-override protection.** Same as `preisblaetter`: an operator REST
upload (`source = 'api'`) is never silently overwritten by an engine ingest
(`source = 'mako'`).

---

## MMM Settlement Prices — Gas MMMA + Strom Ausgleichsenergie

`marktd` stores monthly settlement reference prices for Mehr-/Mindermengenabrechnungen
(MMM). These are **B2B settlement prices** — not B2C retail tariffs — and must therefore
live in `marktd`, not in a retail tariff service. Both `netzbilanzd` (NB, generates MMM
invoices) and `invoicd` (LF, validates inbound MMM invoices) need them, and they cannot
share a database directly.

### Gas MMM Abrechnungspreise — Trading Hub Europe (THE)

Published monthly by Trading Hub Europe (THE, the German gas market area operator
since 2021). `netzbilanzd` auto-fetches these when `mehr_preis_ct_per_kwh` /
`minder_preis_ct_per_kwh` are not supplied in the `POST /api/v1/billing/run` request.
`invoicd` uses them for **check 6** on inbound INVOIC 31007/31008.

```bash
# Import THE Gas MMMA prices for a billing month (operator monthly task)
curl -s -X PUT "http://marktd:8180/api/v1/mmma-preise/gas/2026/7" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "marktgebiet": "THE",
    "mehr_ct_kwh": "1.25",
    "minder_ct_kwh": "0.87",
    "source": "manual"
  }'
# source ∈ manual | the-api | csv-import (CHECK-constrained)

# Query (used by netzbilanzd and invoicd)
curl -s "http://marktd:8180/api/v1/mmma-preise/gas/2026/7" \
  -H "Authorization: Bearer <token>"
# → { "price_month": "2026-07-01", "marktgebiet": "THE", "mehr_ct_kwh": "1.25", ... }

# List all imported months
curl -s "http://marktd:8180/api/v1/mmma-preise/gas?limit=12" \
  -H "Authorization: Bearer <token>"
```

### Strom MMM — one nationwide BDEW series (§ 13 Abs. 3 StromNZV)

§ 13 Abs. 3 StromNZV requires *einheitliche* Mehr-/Mindermengenpreise computed
from monthly market prices; since 2016 the BDEW determines and publishes them
centrally, one Mehr and one Minder value per application month, and every
Netzbetreiber settles against that same series. **The month is the whole key** —
`mmm_preise_strom` has no operator or area column, and the worker refuses a
source file that carries more than one price pair for a month.

An earlier `vnb_mp_id` column modelled a per-ÜNB series that does not exist: it
let several rows claim one month with different prices and no rule for choosing,
and it made `netzbilanzd` refuse every Strom MMM settlement until an operator
configured an ÜNB whose price series was never published. Gas is genuinely
different and keeps its `marktgebiet` key, because there the
Marktgebietsverantwortliche (THE) publishes per market area.

Read by `netzbilanzd` for MMM INVOIC 31005/31006 and by `invoicd` check 6.

```bash
# Import the BDEW Strom MMM prices for an application month
curl -s -X PUT "http://marktd:8180/api/v1/mmm-preise/strom/2026/7" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "mehr_ct_kwh": "2.10",
    "minder_ct_kwh": "1.45",
    "source": "manual"
  }'
# source ∈ manual | bdew-csv | csv-import (CHECK-constrained); no ÜNB field exists

# Query
curl -s "http://marktd:8180/api/v1/mmm-preise/strom/2026/7" \
  -H "Authorization: Bearer <token>"
```

**Operator task:** import these prices monthly before running MMM billing.
A missed monthly import causes `netzbilanzd` to require manual ERP input and
`invoicd` to skip check 6 (logged at debug level, not a hard error).

### Automated monthly import

`marktd` includes a **background worker** that automatically fetches and imports
Gas MMMA and Strom MMM prices on the 1st of each month. Configure it in `marktd.toml`:

```toml
[mmma_import]
enabled       = true
gas_url       = "https://www.the-group.de/gas/mmma/export.csv"  # THE CSV endpoint
strom_url     = "https://example.invalid/bdew/mmm-strom.json"   # BDEW publication
check_hour_utc = 6   # import at 06:00 UTC (after THE typically publishes ~05:00 UTC)
```

The worker wakes every hour; on the 1st of the month at or after `check_hour_utc`
it fetches, parses (CSV or JSON), and upserts — idempotent if prices already exist
for the current month.

**Supported formats:**

```csv
# CSV (5-column)
year,month,marktgebiet,mehr_ct_kwh,minder_ct_kwh
2026,7,THE,1.25,0.87
```

```json
// JSON (single object or array)
{ "mehr_ct_kwh": "1.25", "minder_ct_kwh": "0.87", "marktgebiet": "THE" }
```

**Manual trigger** (catch-up after downtime or testing):

```bash
curl -s -X POST "http://marktd:8180/api/v1/mmma-preise/import-trigger?year=2026&month=7" \
  -H "Authorization: Bearer <token>"
# → { "year": 2026, "month": 7, "import_enabled": true, "results": [...] }
```

**CloudEvents emitted:**

| Event type | Trigger |
|------------|---------|
| `de.markt.mmma.import.success` | Successful monthly import |
| `de.markt.mmma.import.failed` | Fetch or parse failure (requires operator action) |

---

## Device Registry — Zaehler + Geraete

`marktd` maintains a **device registry** for meters (Zähler) and their
associated devices (Geräte). This is populated by WiM MSB/NB device-handover
processes (ORDERS PIDs 17001, 17002, 17009) and by operator REST uploads.

### Hierarchy

```
MeLo ──► Zaehler ──► Geraete
         (1..n)       (0..n)
              │
              └──► Zaehlwerk (0..n)
                   (OBIS registers)
```

A `Zaehler` carries:
- `data` — the full BO4E `Zaehler` payload. `_typ` is auto-injected if absent,
  and every enum in the tree is strict-decoded on write
  (`Bo4eStrict::ensure_known_enums`), so an unrecognised value is a **422**
  naming the field rather than a row that reads `UNKNOWN`.
- `zaehler_typ` — **derived** from `data.zaehlertyp`, not supplied beside it.
  A BO4E `Zaehlertyp` wire value, `CHECK`-constrained to the 13 v202607
  variants (`DREHSTROMZAEHLER`, `INTELLIGENTES_MESSSYSTEM`,
  `MODERNE_MESSEINRICHTUNG`, …). `GASZAEHLER` is *not* one of them, and neither
  is `UNKNOWN`. §42c Energy-Sharing eligibility reads this column;
  `mako-markt`'s `bo4e_check_constraints_match_the_schema` test pins the list
  to `rubo4e::current::Zaehlertyp::VARIANTS`.
  Watch the spelling: `Zaehlertyp` uses `INTELLIGENTES_MESSSYSTEM` (three `s`),
  while `Geraetetyp` uses `INTELLIGENTES_MESSYSTEM` (two). That is a BO4E quirk.
- `eichung_bis` — calibration valid-until date (Eichgültigkeitsdatum), derived
  from `data.eichungBis`. Neither it nor `zaehler_typ` is a request field: the
  column drives the replacement workflow, so it must not be able to disagree
  with the meter record it shadows.
- `data.zaehlwerke` — list of `Zaehlwerk` OBIS registers; exposed via
  `GET /api/v1/zaehler/{id}/zaehlwerke` as typed `Vec<Zaehlwerk>`

A `Geraet` carries:
- `data` — full BO4E `Geraet` payload (`_typ` auto-injected if absent,
  strict-decoded on write)
- `geraet_typ` — derived from `data.geraetetyp`; a BO4E `Geraetetyp` wire value
  such as `STROMWANDLER`, `MODEM_GSM` or `MULTIPLEXANLAGE`. Deliberately **not**
  `CHECK`-constrained: the enum has 46 variants and turns over between BO4E
  versions, so an inline list would be the next thing to drift.
- `konfigurationen` — typed `Vec<GeraetKonfiguration>` for MSB device management (see below)

> **BO4E `_typ` discriminator.** All four PUT device endpoints
> (`zaehler`, `geraete`, `steuerbare-ressourcen`, `technische-ressourcen`) automatically
> inject the correct `_typ` discriminator into the `data` JSONB blob if the caller omits it.
> Callers that include `_typ` in the request body have their value preserved.

### `Zaehlwerk` registers

Each `Zaehler` stores 0..n `Zaehlwerk` objects in `data["zaehlwerke"]` (BO4E `v202607`).
A `Zaehlwerk` is an individual measurement register on the meter, identified by its OBIS
code. iMSyS (intelligent metering systems) expose multiple registers simultaneously —
demand, reactive energy, export, time-of-use tariff splits.

`GET /api/v1/zaehler/{zaehler_id}/zaehlwerke` extracts the `zaehlwerke` array from
`data` and returns it as typed `Vec<Zaehlwerk>`:

```bash
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/zaehlwerke" \
  -H "Authorization: Bearer <token>" | jq '.[] | {
    obisKennzahl,
    richtung,
    verbrauchsart,
    anzahlAblesungen
  }'
```

Response shape:

```json
[
  {
    "_typ": "ZAEHLWERK",
    "obisKennzahl": "1-0:1.8.0",
    "richtung":     "EINSP",
    "verbrauchsart": "WIRKARBEIT",
    "anzahlAblesungen": 1
  },
  {
    "_typ": "ZAEHLWERK",
    "obisKennzahl": "1-0:2.8.0",
    "richtung":     "AUSSP",
    "verbrauchsart": "WIRKARBEIT",
    "anzahlAblesungen": 1
  }
]
```

Returns `[]` (not 404) when no registers are stored. Cedar action: `read-device`.

**Use cases:**
- TOU (time-of-use) billing: identify HT/NT registers before computing Arbeitspreis split
- iMSyS demand management: enumerate active demand registers for `wim.steuerungsauftrag.bestaetigen`
- MSB tariff selection: `PreisblattMessung` Preisstaffel matching uses `richtung` + OBIS

### Device endpoints

```bash
# List meters for a MeLo
curl -s "http://marktd:8180/api/v1/melos/DE00056789000000000000000012345678/zaehler" \
  -H "Authorization: Bearer <token>" | jq '.[] | {zaehler_id, zaehler_typ, eichung_bis}'

# Register or update a meter (include zaehlwerke in data for structured register access)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "melo_id": "DE00056789000000000000000012345678",
    "zaehler_typ": "DREHSTROMZAEHLER",
    "eichung_bis": "2030-12-31",
    "data": {
      "zaehlwerke": [
        { "_typ": "ZAEHLWERK", "obisKennzahl": "1-0:1.8.0", "richtung": "EINSP" }
      ]
    },
    "bo4e_version": "202607.1.0"
  }'

# List Zaehlwerk registers for a meter (typed Vec<Zaehlwerk>)
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/zaehlwerke" \
  -H "Authorization: Bearer <token>" | jq .

# List devices for a meter
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/geraete" \
  -H "Authorization: Bearer <token>" | jq '.[] | {geraet_id, geraet_typ}'

# Get a single Geraet — full BO4E payload + typed konfigurationen
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001" \
  -H "Authorization: Bearer <token>" | jq '{geraet_id, geraet_typ, konfigurationen}'
```

---

## Geraet Konfigurationen — device configuration records (MsbG §23)

`marktd` maintains typed **device-configuration entries** for each `Geraet`, stored in the
`geraet_konfigurationen` JSONB column (separate from the BO4E `data` payload so they can
be updated atomically without rewriting the full `Geraet`).

The `geraet_konfigurationen` column has a GIN index, enabling fast SQL queries such as
"find all devices with SMGW cert expiring within 30 days":

```sql
SELECT malo_id FROM geraete
WHERE geraet_konfigurationen @> '[{"parameter":"SMGW_CERT_ABLAUFDATUM"}]'
  AND tenant = '...'
```

### `Konfigurationsparameter` enum

| Value | Type | Legal basis | Purpose |
|---|---|---|---|
| `FIRMWARE_VERSION` | string | BSI TR-03109-1 §4.3 | Current firmware version (e.g. `"3.1.2"`) |
| `HARDWARE_REVISION` | string | MsbG §23 | Hardware revision string |
| `KOMMUNIKATION` | enum string | — | Communication technology: `"GPRS"` / `"PLC"` / `"ETHERNET"` / `"FUNK"` / `"FESTNETZ"` / `"GSM"` |
| `FERN_UPDATE_FAEHIG` | bool string | BSI TR-03109-4 | Supports OTA firmware update (`"true"` / `"false"`) |
| `CLS_FAEHIG` | bool string | §14a EnWG BK6-22-300 | CLS channel capable (`"true"` / `"false"`) — checked by `processd` §14a auto-acknowledge |
| `SMGW_TLS_CERT_FINGERPRINT` | hex string | BSI TR-03109-3 | SHA-256 fingerprint (64 hex chars) of the SMGW TLS cert |
| `SMGW_CERT_ABLAUFDATUM` | ISO date | BSI TR-03109-4 §6.3 | TLS cert expiry date — monitored by `edmd` cert-expiry worker |
| `CLS_KANAL_ID` | string | GPKE Teil 3 Kap. 2 | CLS channel ID for §14a Steuerungsauftrag routing |
| `GWA_CODENUMMER` | 13-digit | BDEW | GWA (Gateway-Administrator) BDEW-Codenummer |
| `HERSTELLER` | string | MsbG §23 | Manufacturer name |
| `INBETRIEBNAHMEDATUM` | ISO date | § 13 StromNZV | Commissioning date |
| `LETZTE_WARTUNG` | ISO date | § 13 StromNZV | Last maintenance date |
| `NAECHSTE_WARTUNG` | ISO date | § 13 StromNZV | Next scheduled maintenance date |
| `AUSLESE_PROTOKOLL` | enum string | — | Readout protocol: `"SML"` / `"DLMS"` / `"IEC62056"` |
| `MSB_VERTRAGSNUMMER` | string | MsbG §23 | MSB contract number for this device |
| `SONSTIGES` | string | — | Custom parameter — use `notiz` for the actual key name |

### `GeraetKonfiguration` entry shape

```json
{
  "parameter":  "SMGW_CERT_ABLAUFDATUM",
  "wert":       "2027-06-30",
  "updated_at": "2026-07-18T08:00:00Z",
  "notiz":      null
}
```

`updated_at` is **always set server-side** on write — callers must not include it in PUT requests.
Duplicate `parameter` values in a single PUT body are deduplicated (last entry wins) before storage.

### Konfiguration endpoints

```bash
# Get current configuration entries for a device
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001/konfigurationen" \
  -H "Authorization: Bearer <token>" | jq '.[] | {parameter, wert, updated_at}'

# Set SMGW configuration after BSI TR-03109-4 Admin session (§14a fleet rollout)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001/konfigurationen" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "konfigurationen": [
      { "parameter": "FIRMWARE_VERSION",       "wert": "3.1.2"            },
      { "parameter": "HARDWARE_REVISION",      "wert": "Rev. C"           },
      { "parameter": "KOMMUNIKATION",          "wert": "GPRS"             },
      { "parameter": "CLS_FAEHIG",             "wert": "true"             },
      { "parameter": "CLS_KANAL_ID",           "wert": "CLS-00042"        },
      { "parameter": "SMGW_TLS_CERT_FINGERPRINT", "wert": "a1b2c3d4..."   },
      { "parameter": "SMGW_CERT_ABLAUFDATUM",  "wert": "2027-06-30"       },
      { "parameter": "GWA_CODENUMMER",         "wert": "9900000000099"    },
      { "parameter": "HERSTELLER",             "wert": "Sagemcom"         },
      { "parameter": "INBETRIEBNAHMEDATUM",    "wert": "2024-03-15"       }
    ]
  }'
# → 204 No Content + emits de.markt.geraet.konfiguration.updated CloudEvent
# If CLS_FAEHIG is set, processd auto-acknowledges §14a Steuerungsauftrag for this device.
# If SMGW_CERT_ABLAUFDATUM is set, edmd cert-expiry worker starts monitoring.
```

### Integration with `processd` (§14a Steuerungsauftrag)

When `CLS_FAEHIG = "true"` is stored, `processd` **auto-acknowledges** §14a
`WimSteuerungsauftrag` requests for this device (GPKE Teil 3 Kap. 2).
When `CLS_FAEHIG = "false"` or absent, `processd` rejects the Steuerungsauftrag (GPKE Teil 3 Kap. 2 — device not remotely controllable).

### Integration with `edmd` (SMGW cert-expiry monitoring)

Setting `SMGW_CERT_ABLAUFDATUM` triggers the `edmd` **daily compliance worker** to monitor
the device. When the expiry date is ≤ 30 days away, `edmd` emits
`de.messwert.cls.compliance-issue` (severity `WARNING`); after expiry, severity `CRITICAL`.

---

## ZaehlzeitDefinition — typed TOU definition for ERP and portals

`GET /api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen` assembles a complete
`rubo4e::current::Zaehlzeitdefinition` BO4E object from `zaehler_register` + `zaehler_saisons`
rows and returns it in canonical JSON. This is the endpoint ERP systems and customer portals
use to **display ToU register schedules** to end customers without custom ETL.

```bash
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/zaehlzeitdefinitionen" \
  -H "Authorization: Bearer <token>" | jq .
```

Response shape (BO4E `Zaehlzeitdefinition`):

```json
{
  "_typ": "ZAEHLZEITDEFINITION",
  "_id": "Z001234567",
  "saisons": [
    {
      "_typ": "ZAEHLZEITSAISON",
      "bezeichnung": "WINTER",
      "tagtypen": [
        {
          "_typ": "ZAEHLZEITTAGTYP",
          "tagtyp": "WERKTAGS",
          "umschaltzeiten": [
            { "_typ": "UMSCHALTZEIT", "registercode": "HT", "umschaltzeit": "07:00" },
            { "_typ": "UMSCHALTZEIT", "registercode": "NT", "umschaltzeit": "22:00" }
          ]
        },
        {
          "_typ": "ZAEHLZEITTAGTYP",
          "tagtyp": "WOCHENENDE",
          "umschaltzeiten": [
            { "_typ": "UMSCHALTZEIT", "registercode": "NT", "umschaltzeit": "00:00" }
          ]
        }
      ]
    }
  ]
}
```

The `?valid_only=true` query parameter restricts the response to currently valid registers
(`valid_to IS NULL OR valid_to >= today`).

**Why this endpoint?** ERP systems (Schleupen, SAP IS-U, powercloud) need the nested
`Zaehlzeitdefinition` shape for customer portal display. Without it, clients must query
two endpoints and assemble the hierarchy themselves. The endpoint returns canonical BO4E
that can be schema-validated client-side.

**§14a Modul 2 context.** Under BK6-22-300, the NB assigns HT/NT registers to controllable
loads at specific switching times communicated via WiM Stammdaten (ORDERS 17102–17133 ZAK+ZE segments).
`marktd` auto-populates the underlying data from those events; this endpoint exposes it in
BO4E form. See also [`billingd`](@/docs/services/billingd.md) §14a Modul 2 billing.

---

## ZaehlzeitRegister — iMSys TOU register definitions

`ZaehlzeitRegister` and `ZaehlzeitSaison` provide structured Time-of-Use (TOU) register
definitions for iMSys (intelligent metering systems). They underpin §14a Modul 2 billing
by enabling automatic classification of 15-min Lastgang intervals into HT/NT tariff bands
without per-meter manual configuration.

### Data model

```
Zaehler (1) ──► ZaehlzeitRegister (N)
                   zaehlerauspraegung: HT | NT | EINZEL
                   obis_kennzahl: "1-1:1.29.0" (HT), "1-1:1.49.0" (NT)
                   valid_from / valid_to  (seasonal changeover supported)

ZaehlzeitRegister (1) ──► ZaehlzeitSaison (N)
                               saison: SOMMER | WINTER | GESAMT
                               wochentage: [1,2,3,4,5] (Mon–Fri)
                               zeit_von: "07:00"   (inclusive, local German time CET/CEST)
                               zeit_bis: "22:00"   (exclusive)
```

A typical residential iMSys meter has two registers (HT + NT) each with two seasons
(SOMMER, WINTER). Weekdays differ from weekends. `marktd` stores all combinations and
resolves them efficiently via a single PostgreSQL JOIN with JSONB `@>` containment.

### TOU register endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/zaehler/{id}/register` | List all TOU registers for a Zaehler |
| `PUT` | `/api/v1/zaehler/{id}/register` | Upsert a ZaehlzeitRegister |
| `GET` | `/api/v1/zaehler-register/{id}/saisons` | List seasonal windows for a register |
| `PUT` | `/api/v1/zaehler-register/{id}/saisons` | Upsert a ZaehlzeitSaison |
| `GET` | `/api/v1/zaehler/{id}/tariff-zone` | Resolve HT/NT/EINZEL at a given datetime |

### Setting up TOU registers

```bash
# 1. Create an HT register
REGISTER_ID=$(uuidgen)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567/register" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d "{
    \"id\": \"${REGISTER_ID}\",
    \"bezeichnung\": \"HT\",
    \"zaehlerauspraegung\": \"HT\",
    \"obis_kennzahl\": \"1-1:1.29.0\",
    \"einheit\": \"KWH\",
    \"valid_from\": \"2025-01-01\"
  }"

# 2. Add winter season: Mon–Fri 07:00–22:00
curl -s -X PUT "http://marktd:8180/api/v1/zaehler-register/${REGISTER_ID}/saisons" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d "{
    \"id\": \"$(uuidgen)\",
    \"saison\": \"WINTER\",
    \"wochentage\": [1,2,3,4,5],
    \"zeit_von\": \"07:00\",
    \"zeit_bis\": \"22:00\"
  }"

# 3. Resolve tariff zone at a specific time
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/tariff-zone?datetime=2026-01-15T14:30:00" \
  -H "Authorization: Bearer <token>" | jq .
# → { "zaehler_id": "Z001234567", "local_datetime": "2026-01-15T14:30:00", "tariff_zone": "HT" }
```

The `tariff-zone` endpoint performs a single JOIN between `zaehler_register` and
`zaehler_saisons`, filtering by JSONB array containment (`wochentage @> $3::jsonb`)
and time range. No application-level iteration is needed on the consumer side.

**Integration with `billingd` (§14a Modul 2):**
`billingd` calls `GET /api/v1/zaehler/{id}/tariff-zone?datetime=<slot-start>` for each
15-min slot in the billing month and aggregates kWh by zone. This eliminates the need for
operators to maintain manual HT/NT time-window configuration per meter in the billing engine.

### Automatic population from WiM Stammdaten (ORDERS 17102–17133)

`marktd` populates `ZaehlzeitRegister` and `ZaehlzeitSaison` **automatically** when `makod`
receives an inbound WiM Stammdaten response (ORDERS PIDs 17102–17133) from the MSB.

The adapter `extract_zak_ze_zaehlwerke()` in `makod` parses the EDIFACT **ZAK+ZE+ZD** segments:

| Segment | Field | Codes |
|---------|-------|-------|
| `ZAK` element 0 | `obis_kennzahl` | OBIS code (e.g. `"1-1:1.8.0"`) |
| `ZAK` element 1 | `zaehlerauspraegung` | `Z01`→`HT`, `Z02`→`NT`, `Z03`→`EINZEL` |
| `ZAK` element 2 | `bezeichnung` | Human-readable register label |
| `ZE` element 0 | `saison` | `Z01`→`SOMMER`, `Z02`→`WINTER`, `Z03`→`GESAMT` |
| `ZD` element 0 | `tagtyp` | `Z01`→`WERKTAG`, `Z02`→`SAMSTAG`, `Z03`→`SONNTAG_FEIERTAG` |
| `ZD` elements 1..N | time windows | `"HHMM:RegisterCode"` switch-point pairs |

After parsing, `makod` emits a `ProcessCompleted` outbox entry (CloudEvent type
`de.mako.process.completed`, PID 17102–17133) carrying `melo_id` and `zaehlwerke`.
When `marktd`'s `event_ingest` handler receives this event, it:

1. Looks up the Zaehler associated with the MeLo via `list_zaehler_by_melo()`
2. For each `zaehlwerk`, calls `upsert_register()` (idempotent on `zaehler_id + bezeichnung + valid_from`)
3. For each season window (ZE→ZD), calls `upsert_saison()` with a deterministic UUID
   derived from `(register_id, saison, tagtyp, zeit_von)` — safe for at-least-once delivery

This means operators **do not need to manually provision** ZaehlzeitRegister entries for
meters where the MSB sends WiM ORDERS Stammdaten responses — `marktd` and `makod` handle
it automatically.

**Data flow:**

```mermaid
sequenceDiagram
    participant MSB
    participant makod
    participant marktd
    participant billingd

    MSB->>makod: ORDERS PID 17102–17133<br/>ZAK+ZE+ZD segments
    makod->>makod: extract_zak_ze_zaehlwerke()<br/>parse OBIS / saisons / fenster
    makod--)marktd: de.mako.process.completed<br/>{melo_id, zaehlwerke:[...]}
    marktd->>marktd: list_zaehler_by_melo(melo_id)
    marktd->>marktd: upsert_register() + upsert_saison()<br/>(idempotent, deterministic UUID)
    billingd->>marktd: GET /zaehler/{id}/tariff-zone?datetime=...
    marktd-->>billingd: { "tariff_zone": "HT" }
```

---

## SteuerbareRessource Registry

`marktd` stores **steuerbare Ressourcen** (SR) — iMS controllable resources per
GPKE Teil 3 Kap. 2. An SR-ID has the format `C[A-Z0-9]{9}[0-9]` (Codetyp `C` +
9 alphanumeric chars + ASCII-Verfahren check digit).

Populated by WiM iMS Steuerungsauftrag processes (PID 55168) and by operator
uploads. Linked optionally to a MaLo and/or MeLo.

The `konfigurationsprodukte` field stores the contracted iMS control products
— used for pre-dispatch eligibility checks in `wim.steuerungsauftrag.bestaetigen`.
The value is preserved across PUT calls unless explicitly replaced via the sub-resource endpoint.

### Konfigurationsprodukte — typed API

The `konfigurationsprodukte` sub-resource has its own endpoints with **full BO4E validation** per GPKE Teil 3 Kap. 1.3:

```bash
# Retrieve typed Konfigurationsprodukte (returns Vec<ZeitvariablePreisposition> deserialized)
curl -s "http://marktd:8180/api/v1/steuerbare-ressourcen/C0000000011/konfigurationsprodukte" \
  -H "Authorization: Bearer <token>"
# → {
#     "sr_id": "C0000000011",
#     "konfigurationsprodukte": [{ "produktcode": "FLEX-001", ... }],
#     "count": 1,
#     "schema_drift": 0
#   }

# Replace all contracted products (validates each element + enforces non-empty produktcode)
curl -s -X PUT "http://marktd:8180/api/v1/steuerbare-ressourcen/C0000000011/konfigurationsprodukte" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '[
    { "produktcode": "FLEX-PRODUCT-001", "zaehlzeitregister": "HT" }
  ]'
# → 204 No Content + emits de.markt.sr.konfigurationsprodukt.updated CloudEvent

# Remove a single product by produktcode
curl -s -X DELETE "http://marktd:8180/api/v1/steuerbare-ressourcen/C0000000011/konfigurationsprodukte/FLEX-PRODUCT-001" \
  -H "Authorization: Bearer <token>"
# → 204 No Content
```

**Validation rules (GPKE Teil 3 Kap. 1.3):**

- Each element must deserialize as `rubo4e::current::Konfigurationsprodukt`
- `produktcode` **must not be empty** — every contracted product requires a unique code
- `bandNummer` is **rejected with 422** — it does not exist in BO4E v202607; use `zaehlzeitregister`

`processd` checks this list before auto-confirming a Steuerungsauftrag. An uncontracted `produktcode` triggers `wim.steuerungsauftrag.ablehnen` automatically.

---

## `PUT /api/v1/melos/{melo_id}` — MeLo Typed Columns & Schema Validation

Every `PUT /api/v1/melos/{melo_id}` call:
1. **Validates** the incoming `data` as `rubo4e::current::Messlokation`: auto-injects `_typ: "MESSLOKATION"`, rejects wrong `_typ` or invalid enum values with 422.
2. **Normalises** to canonical camelCase BO4E form before storage.
3. **Extracts typed columns** for efficient SQL queries:

| Column | Source field | Purpose |
|---|---|---|
| `netzebene_messung` | `data.netzebeneMessung` | Netzebene at the metering point. Same BO4E `Netzebene` vocabulary and same `CHECK`-drift guard as `malo.netzebene` — the wire values are `NSP`/`MSP`/`HSP`/`HSS`/…, never `"NS"` or `"MS"` |
| `regelzone` | `data.standorteigenschaften.eigenschaftenStrom[0].regelzoneEic` | Regelzone EIC code → maps MeLo to the ÜNB for Redispatch 2.0 Stammdaten forwarding and MABIS IFTSTA 21000 routing |

**`MeloResponse`** (GET) returns `data: rubo4e::current::Messlokation` (typed):

```json
{
  "melo_id": "DE00056789000000000000000012345678",
  "malo_id": "10001234558",
  "version": 2,
  "netzebene_messung": "NS",
  "regelzone": "10YDE-EON------1",
  "data": { "_typ": "MESSLOKATION", "netzebeneMessung": "NS", ... }
}
```

To populate `regelzone` from a NIS export, include the BO4E path in the PUT body:

```json
{
  "malo_id": "10001234558",
  "data": {
    "_typ": "MESSLOKATION",
    "standorteigenschaften": {
      "eigenschaftenStrom": [
        { "regelzoneEic": "10YDE-EON------1" }
      ]
    }
  }
}
```

**`regelzoneEic`, not `regelzone`.** BO4E ships both and they are different
things: `regelzone` is „Der Name der Regelzone", a free `String`; `regelzoneEic`
is „De EIC-Nummer der Regelzone" (BO4E's own typo). The column derives from the
EIC field, so a payload that fills `regelzone` instead is accepted, stored — and
leaves the column `NULL`, with nothing anywhere reporting it.

---

## Netz-Element-Lokationen (NeLo) — Redispatch 2.0

`marktd` maintains a registry of Netz-Element-Lokationen (NeLo) for BDEW Redispatch 2.0.
A NeLo is a network element location identified by a 16-char EIC code (ENTSO-E,
`NAD DE3055 = ZEW`) or a 13-digit BDEW Codenummer.

NeLos are owned by the Netzbetreiber (NB role) responsible for the network element.
They carry structural metadata (Sparte, Netzebene) and an open-ended JSONB `data` payload
for additional Redispatch 2.0 attributes.

**REST API:**

```http
# List all NeLos for this tenant (optionally filter by Netzbetreiber GLN)
GET  /api/v1/nelos
GET  /api/v1/nelos?nb_mp_id=9900357000004&page=0&size=50

# Get a single NeLo by EIC or BDEW Codenummer
GET  /api/v1/nelos/{nelo_id}

# Insert or update a NeLo (NB role required; supply If-Match for OCC)
PUT  /api/v1/nelos/{nelo_id}
```

**Request body for PUT** (includes typed NeLo columns):

```json
{
  "sparte": "STROM",
  "name": "Umspannwerk Musterstadt 110/20 kV",
  "netzebene": "HS",
  "nb_mp_id": "9900357000004",
  "steuerkanal": true,
  "eigenschaft_msb_lokation": "NB",
  "grundzustaendiger_msb_codenr": "9900357000004",
  "data": {
    "eic": "10XDE-EON-NETZ--G",
    "regelzone": "10YDE-EON------1"
  }
}
```

The `steuerkanal`, `eigenschaft_msb_lokation`, and `grundzustaendiger_msb_codenr` fields
are extracted into typed SQL columns at write time for efficient Redispatch 2.0 queries
(e.g. "find all controllable NeLos for a given NB").

**Authorization:** `read-nelo` is open to all authenticated users in the tenant.
`write-nelo` requires the `NB` role (Cedar policy `write-nelo`).

---

## Location Graph — `Lokationszuordnung`

The `lokationszuordnungen` table stores the full MaKo location graph as directed edges with
temporal validity (`valid_from`, `valid_to`). Each edge connects two location nodes typed by the
BO4E `Lokationstyp` — `MALO`, `MELO`, `NELO`, `SR` (SteuerbareRessource), or `TR`
(TechnischeRessource). Node types are the typed `rubo4e::current::Lokationstyp` end to end
(request body, storage, and traversal results), not free strings.

```mermaid
graph LR
    MaLo["MaLo (51238696012)"] --> MeLo["MeLo (DE-MEL-001)"]
    MeLo --> NeLo["NeLo (EIC 10XDE-…)"]
    MeLo --> SR["SR (C0000000011)"]
    MeLo --> TR["TR (D0000000010)"]
```

### Graph traversal

`GET /api/v1/malos/{id}/lokationen` runs a recursive-CTE BFS query (max depth 8) and returns
all reachable edges from the given MaLo, ordered by depth.

```http
# Full graph from a MaLo (all edges regardless of validity)
GET /api/v1/malos/51238696012/lokationen

# Graph valid on a specific date (temporal filter)
GET /api/v1/malos/51238696012/lokationen?at=2025-01-15

# Graph from a MeLo
GET /api/v1/melos/DE-MEL-001/lokationen?at=2025-01-15
```

Response: `Vec<LokationszuordnungEdge>` ordered by `depth` (0 = direct edges from root).

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "tenant": "9900357000004",
    "von_id": "51238696012",
    "von_typ": "MALO",
    "nach_id": "DE-MEL-001",
    "nach_typ": "MELO",
    "valid_from": null,
    "valid_to": null,
    "data": {},
    "depth": 0
  }
]
```

### Lokationsbündel

`GET /api/v1/malos/{id}/buendel` returns the **Lokationsbündel** (UTILMD
Lokationsbündelstruktur) as a first-class aggregate — the set of MeLos, NeLos,
SRs, and TRs bundled under a MaLo, projected from the typed graph — together
with its structural-integrity status. The aggregate (`mako_markt::repository::Lokationsbuendel`)
enforces the bundle invariants at the domain boundary: a consuming MaLo must
carry at least one MeLo, and all MeLos of the bundle must share one MSB
(`validate` / `validate_msb_consistency`). A bundle can be transiently
incomplete mid-Einzug, so the endpoint reports `valid: false` with a
`validation_error` rather than failing the request.

```json
{
  "malo_id": "51238696012",
  "lokationsbuendelcode": "1S",
  "messlokationen": ["DE-MEL-001"],
  "netzlokationen": [],
  "steuerbare_ressourcen": [],
  "technische_ressourcen": [],
  "valid": true
}
```

### Upsert and delete

```http
# Upsert an edge (idempotent)
PUT /api/v1/lokationszuordnungen
Content-Type: application/json

{
  "von_id":    "51238696012",
  "von_typ":   "MALO",
  "nach_id":   "DE-MEL-001",
  "nach_typ":  "MELO",
  "valid_from": null,
  "valid_to":   null,
  "data":       {}
}

# Hard-delete an edge pair (all temporal variants)
DELETE /api/v1/lokationszuordnungen/51238696012/DE-MEL-001
```

**Temporal succession:** Multiple edges between the same `(von_id, nach_id)` pair are
allowed when `valid_from` differs. One open-ended edge (`valid_from IS NULL`) is permitted
per pair. Dated edges allow modelling supplier-switch driven reassignments.

---

## TechnischeRessource (E-mobility, Generation, Storage)

`TechnischeRessource` records link E-mobility charging points, generation units, and storage
to MaLos/MeLos. Required for WiM iMS Steuerungsauftrag routing and Redispatch 2.0 flexibility
registration.

```http
# Get a TechnischeRessource by TrId
GET  /api/v1/technische-ressourcen/{tr_id}

# Upsert
PUT  /api/v1/technische-ressourcen/{tr_id}

# List all TechnischeRessourcen linked to a MaLo
GET  /api/v1/malos/{malo_id}/technische-ressourcen
```

**Request body for PUT:**

```json
{
  "data":              { "_typ": "TECHNISCHERESSOURCE", ... },
  "malo_id":           "51238696012",
  "melo_id":           "DE-MEL-001",
  "nutzung":           "STROMVERBRAUCHSART",
  "verbrauchsart":     "E_MOBILITAET",
  "ist_fernschaltbar": true,
  "bo4e_version":      "202607.1.0"
}
```

BO4E-aligned classification (both fall back to the typed `data` payload when
omitted; invalid values are **rejected with `400 Bad Request`**):
`nutzung` = `TechnischeRessourceNutzung`
(`"STROMVERBRAUCHSART"` | `"STROMERZEUGUNGSART"` | `"SPEICHER"`);
`verbrauchsart` = `TechnischeRessourceVerbrauchsart`, only for `STROMVERBRAUCHSART`
(`"KRAFT_LICHT"` | `"WAERME"` | `"E_MOBILITAET"` | `"STRASSENBELEUCHTUNG"`).
`ist_fernschaltbar: true` marks the resource as remotely controllable for Redispatch 2.0.

---

## CloudEvent Replay Log

Every inbound CloudEvent is appended to the durable `event_log` table **before** fan-out,
enabling full replay without data loss.

```http
# Query the event log (all parameters optional)
GET /admin/events?from=2025-01-01T00:00:00Z&to=2025-02-01T00:00:00Z&type=de.mako.process.initiated&limit=500
```

Response: `Vec<EventLogRow>` ordered by `received_at ASC` (oldest first = deterministic replay).

**Use cases:**
- New subscriber onboarding: replay all `de.mako.process.initiated` events since go-live
- Bug fix replay: re-deliver specific event types after a `invoicd` fix
- Post-incident forensics: trace which events were delivered to which subscriber

---

## CloudEvents — outbound event catalog

`marktd` emits CloudEvents to all registered ERP webhook subscribers for the following
domain events. Each event carries the `markt*` extension attributes listed below the table.

| `type` | `subject` | Trigger | Consumers |
|---|---|---|---|
| `de.markt.malo.updated` | `malo_id` | MaLo PUT | `edmd`, `processd`, ERP |
| `de.markt.malo.stammdaten-geaendert` | `malo_id` | UTILMD Stammdatenänderung applied to a MaLo | ERP audit |
| `de.markt.stammdaten.geaendert` | object id | The same apply for a MeLo, NeLo, Tranche, technische or steuerbare Ressource | ERP audit |
| `de.markt.melo.updated` | `melo_id` | MeLo PUT | `edmd`, `processd`, ERP |
| `de.markt.pricat.published` | `nb_mp_id` | PRICAT 27003 dispatch | `netzbilanzd`, `invoicd`, ERP |
| `de.markt.nb-contract.updated` | `contract_id` | NB contract PUT | ERP |
| `de.markt.sr.konfigurationsprodukt.updated` | `sr_id` | SR Konfigurationsprodukt replace | `processd` (§14a eligibility check), ERP |
| `de.markt.geraet.konfiguration.updated` | `geraet_id` | Geraet konfigurationen PUT | `edmd` cert-expiry worker, `processd` §14a auto-ack check, ERP |
| `de.markt.partner.updated` | `mp_id` | Marktpartner PUT | `makod` partner sync, ERP |
| `de.markt.msb-rahmenvertrag-gas.updated` | contract id | Gas MSB-Rahmenvertrag upsert — carries `version`, `valid_from`, `valid_to`, `signed_at` | ERP |
| `de.markt.netzzugang.antrag.updated` | request id | §20b EnWG Netzzugangsplattform request upserted or advanced | `makod` sender, ERP |
| `de.markt.einwilligung.erteilt` | consent id | ESA consent granted (§49 Abs. 2 Nr. 9 MsbG) | `makod`, ERP |
| `de.markt.einwilligung.widerrufen` | consent id | Consent revoked or expired — `grund` distinguishes `einwilligung_widerrufen` from `einwilligung_abgelaufen` | `makod` (fires the 17008 Abbestellung), ERP |
| `de.markt.subscription.test` | subscriber id | `POST /api/v1/subscriptions/{id}/test` — a probe with no business meaning, so an operator can verify a webhook endpoint end-to-end before it carries real events | the subscriber under test |
| `de.markt.versorgung.changed` | `malo_id` | VersorgungsStatus transition | `vertragd`, `billingd`, ERP |
| `de.markt.versorgung.gap-detected` | `malo_id` | An interval no supplier covers: a Lieferende (55005/44005) with a gap before the successor, or a Fall-b Bestätigung (55002/55078/44002) — §38 EnWG | `processd` EoG module |
| `de.markt.versorgung.eog-begonnen` | `malo_id` | 55013/44013 completed → `begin_eog_supply` | `processd` EoG module, ERP |
| `de.markt.mmma.import.success` | `year-month`, `commodity` | Monthly MMMA/MMM price import (Gas or Strom) | `netzbilanzd`, `invoicd` |
| `de.markt.mmma.import.failed` | `year-month`, `commodity` | Monthly import fetch/parse/store failure | operator |

### `de.markt.geraet.konfiguration.updated` data payload

```json
{
  "specversion": "1.0",
  "type":        "de.markt.geraet.konfiguration.updated",
  "source":      "urn:mako:marktd:tenant:9900000000003",
  "subject":     "SMGW-2026-001",
  "id":          "a1b2c3d4-...",
  "time":        "2026-07-18T08:01:00Z",
  "data": {
    "geraet_id":  "SMGW-2026-001",
    "zaehler_id": "Z001234567",
    "count":      10
  }
}
```

`edmd` subscribes to this event type and, upon receipt, updates the `smgw_sessions` table
for the corresponding MaLo if an existing session exists — ensuring the cert-expiry
compliance worker always has fresh `SMGW_CERT_ABLAUFDATUM` data without polling `marktd`.

## CloudEvents Extensions

All outbound events carry the `markt*` extension namespace (CloudEvents §3.3 — lowercase
alphanumeric only):

| Extension | Type | Description |
|---|---|---|
| `marktmaloid` | string | Resolved Marktlokations-ID |
| `marktmeloid` | string | Resolved Messlokations-ID |
| `marktrole` | string | Marktrolle: `NB`, `LF`, `MSB`, `BIKO`, `UNB` |
| `markterpref` | string | ERP-supplied idempotency key |
| `makopid` | u32 | Forwarded BDEW Prüfidentifikator |
| `makoworkflow` | string | Workflow family name |
| `makoerc` | string | BDEW ERC error code (on `aperak.rejected`) |
| `traceparent` | string | W3C Trace Context — `00-<trace-id>-<parent-id>-<flags>` |
| `tracestate` | string | W3C Trace Context vendor bag — forwarded unchanged |

**`marktrole` derivation** from `makoworkflow`:

| Pattern | Role |
|---|---|
| ends with `-lf` or contains `-lf-` | `LF` |
| starts with `wim-` | `MSB` |
| starts with `mabis-` | `BIKO` |
| everything else | `NB` |

**Event source:** `"urn:mako:marktd:tenant:{tenant}"`

---

## See Also

- [`makod` Operator Guide](@/docs/services/makod.md)
- [`netzbilanzd` Operator Guide](@/docs/services/netzbilanzd.md) — NNE/KA/MMM billing (NB role)
- [`sperrd` Operator Guide](@/docs/services/sperrd.md) — Sperrung execution tracking (NB role)
- [`invoicd` Operator Guide](@/docs/services/invoicd.md) — INVOIC plausibility, receipt persistence
- [`edmd` Operator Guide](@/docs/services/edmd.md) — meter-data store, time-series API
- [`obsd` Operator Guide](@/docs/services/obsd.md) — process observability, BNetzA KPI reports
- [ERP Integration](@/docs/architecture/erp-integration.md)
- [Domain Model](@/docs/architecture/domain-model.md)
- [Getting Started](@/docs/guide/getting-started.md)
