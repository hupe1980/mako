+++
title = "marktd Operator Guide"
description = "Operator guide for marktd, the market data hub: Marktpartner, Marktlokationen, price sheets and the durable CloudEvents fan-out every other service reads."
weight = 22
+++
`marktd` is the **market data hub** — the single source of truth for market
entity state in a MaKo deployment, on port **8180**. It stores:

- Marktlokationen, Messlokationen, Zähler and Geräte as typed `rubo4e::current`
  BOs, gated on every `PUT`;
- Marktpartner (`Geschaeftspartner`), NB network contracts (BO4E `Vertrag`),
  price sheets (Netznutzung, Messung, KA, Dienstleistung, Hardware) and the
  Mehr-/Mindermengen prices;
- the **VersorgungsStatus** per MaLo with full history and point-in-time reads;
- MaLo grid topology (`malo_grid`, from the NB's NIS/GIS), NeLos, Tranchen,
  steuerbare and technische Ressourcen, the location graph, ESA consents and
  §20b EnWG Netzzugang requests.

It also runs the **durable fan-out**: inbound `de.mako.*` events from `makod`
and its own `de.markt.*` events reach every registered subscriber (ERP,
`processd`, `invoicd`, `edmd`, `obsd`) as signed webhooks.

`marktd` holds **no domain policy**. Automated Anmeldung decisions belong to
[`processd`](@/docs/services/processd.md), which reads `marktd` and decides with
`mako-pruefung`.

```mermaid
graph TB
    makod["makod :8080<br/>EDIFACT ↔ BO4E"]
    marktd["marktd :8180<br/>Market Data Hub"]
    processd["processd :8580<br/>NB STP + LF answers"]
    erp["ERP"]
    invoicd["invoicd :8280"]
    edmd["edmd :8380"]
    obsd["obsd :8480"]
    pg["PostgreSQL"]

    makod -->|"de.mako.process.*<br/>signed POST"| marktd
    marktd --- pg
    marktd -->|"de.mako.* + de.markt.*"| erp
    marktd -->|"de.mako.process.initiated"| processd
    marktd -->|"de.mako.process.initiated"| invoicd
    marktd -->|"de.mako.*"| edmd
    marktd -->|"de.mako.*"| obsd
    erp -->|"PUT /api/v1/malos/{id}<br/>PUT /api/v1/partners/{mp_id}"| marktd
    invoicd -->|"GET /api/v1/preisblaetter/{nb_mp_id}"| marktd
    processd -->|"GET /api/v1/versorgung/{malo_id}<br/>GET /api/v1/malos/{id}/grid"| marktd
    processd -->|"POST /api/v1/commands"| makod
```

---

## Quick start

`just dev marktd` starts PostgreSQL and runs `marktd` with
`dev/config/marktd.toml`. The NB STP demo (`demos/nb-stp/docker-compose.yml`)
runs the five services the Lieferbeginn path needs.

A minimal container:

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
    image: ghcr.io/hupe1980/mako-marktd:<version>
    depends_on:
      postgres:
        condition: service_healthy
    volumes:
      - ./marktd.toml:/etc/marktd/marktd.toml:ro
    environment:
      MARKTD_CONFIG:        /etc/marktd/marktd.toml
      DATABASE_URL:         postgres://marktd:secret@postgres/marktd
      MAKOD_API_KEY:        my-makod-api-key
      MAKOD_WEBHOOK_SECRET: my-webhook-secret
    ports: ["8180:8180"]
```

The config path is an **environment variable**. The binary accepts only
`--check`, `--print-config` and `--help`, and refuses any other argument, so a
`--config=…` flag stops the container from starting.

Migrations (`migrations/0001_initial.sql`) run at startup.

---

## Configuration

`mako_service::load_config` reads `marktd.toml` (path from `MARKTD_CONFIG`,
default `./marktd.toml`), then `MARKTD_*` variables with `__` as the section
separator, then any `*_FILE` variable from a file. The file is optional
(`MARKTD_DATABASE__URL`, `MARKTD_MARKT__TENANT`, `MARKTD_MAKOD__API_KEY_FILE`, …).
`"env:VAR"` defers a value to the environment. Unknown keys are refused.

```toml
[http]
addr = "0.0.0.0:8180"     # default

[database]
url             = "env:DATABASE_URL"  # required
pool_size       = 20
min_connections = 2

[markt]
# This deployment's identity: the `resource_tenant` every Cedar check compares
# the caller's `mako_tenant` against, the `tenant` column on tenant-scoped rows,
# and the source URN of every outbound CloudEvent.
tenant = "9900357000004"           # required

[makod]
base_url  = "http://makod:8080"   # required
api_key   = "env:MAKOD_API_KEY"   # required

[webhook]
inbound_path          = "/api/v1/mako/events"        # default; must match makod's [erp] webhook_url
inbound_secret        = "env:MAKOD_WEBHOOK_SECRET"   # required unless allow_insecure_no_auth
delivery_timeout_secs = 10                           # default; per outbound delivery
max_retry_attempts    = 3                            # default; then dead-lettered

[oidc]              # required unless allow_insecure_no_auth
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-marktd"
jwks_refresh_secs = 300

[mcp]               # the /mcp surface's own credential layer
api_key = "env:AGENTD_KEY"   # optional; also named_keys, api_key_roles

[mmma_import]       # monthly Mehr-/Mindermengenpreis import; off by default
enabled        = false
gas_url        = ""   # THE Gas MMMA CSV/JSON; empty skips Gas
strom_url      = ""   # BDEW Strom MMM CSV/JSON; empty skips Strom
check_hour_utc = 6    # 1st of each month, at or after this UTC hour

# Fail-closed: without [oidc] AND webhook.inbound_secret marktd refuses to
# start. The insecure dev posture must be requested by name:
# allow_insecure_no_auth = true
```

| Setting | How | Default |
|---|---|---|
| Health probe | `--check` — `GET /health/ready` on loopback against the **running** instance; exit 0 when ready. Not a config validator | — |
| Effective config | `--print-config` — merged layers, secrets redacted | — |
| Config path | `MARKTD_CONFIG` | `./marktd.toml` |
| Log level | `MARKTD_LOG_LEVEL`, else `LOG_LEVEL`, else `RUST_LOG` | `info` |
| Log format | `MARKTD_LOG_FORMAT`, else `LOG_FORMAT` (`json` \| `pretty` \| `compact`) | `json` |

---

## Authentication and authorization

Every REST request carries a JWT Bearer token (RS256, ES256 or PS256; HS256/HS512
are refused) with these claims:

| Claim | Type | Required | Description |
|---|---|---|---|
| `sub` | `string` | yes | Principal |
| `mako_tenant` | `string` | yes | Tenant the principal belongs to |
| `mako_roles` | `string[]` | yes | e.g. `["NB"]`, `["MSB","NB"]` |
| `mako_sparte` | `string[]` | no | Commodity scope, e.g. `["Strom","Gas"]` |

### Cedar policy

`policies/marktd.cedar` is loaded at startup and has **five `permit`
statements over 49 actions**, each starting from tenant equality:

| Group | Actions | Extra condition |
|---|---|---|
| Tenant-wide reads | every `read-*` action except the ESA pair, plus `use-mcp` | none |
| Ordinary writes | `write-malo`, `write-melo`, `write-nb-contract`, `write-partner`, `write-versorgungsstatus`, `write-device`, `write-sr`, `write-netzzugang`, `write-msb-rv-gas`, `write-melo-msb`, `write-bilanzierung`, `write-mabis-zp`, `write-lokationszuordnung` | none |
| Grid-operator writes | `write-preisblatt`, `write-nelo`, `write-tranche`, `write-malo-grid`, `write-grundversorger`, `write-energiemix`, `write-mmma-preis`, `dispatch-pricat` | `mako_roles` contains `NB` |
| ESA consent | `read-einwilligung`, `write-einwilligung` | `MSB` **or** `ESA` |
| Operations | `manage-subscription`, `manage-fanout` | `ADMIN` |

```cedar
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

Tenant and roles arrive in the **context** — `context.principal_tenant`
(`mako_tenant`), `context.principal_roles` (`mako_roles`),
`context.resource_tenant` (`[markt] tenant`). A policy written against
`principal.tenant` compiles and denies every caller.

A denial is `403` built by `ApiError::forbidden`:
`{"error": "Forbidden", "detail": "<reason>"}`. The reason names the action
(`manage-subscription denied`) or reads `access denied`; it never names the
resource tenant.

To customise, replace `policies/marktd.cedar` and restart.

### The authorization guard

`marktd` has no global auth middleware: `Claims` is an axum extractor, so a
handler that does not name it is unauthenticated, and an action missing from
the policy is a permanent `403`. `services/marktd/tests/it/authorization_guard.rs`
refuses a build where:

| Check | Defect caught |
|---|---|
| Every action checked in code appears in the policy | A permanent `403` on a live route |
| Every policy action is checked somewhere | A dead grant |
| Every handler module extracts `Claims` and calls the enforcer | An unauthenticated route |
| No handler reads `Claims` as an `Extension` | Claims that bypass verification |
| No request body carries a `tenant` field | A caller naming its own tenant |

A custom policy that drops an action therefore fails the build.

---

## REST API

OpenAPI: `GET /api/v1/openapi.json`; interactive browser at `/api/v1/docs/`.
Every request body denies unknown fields.

### The BO4E gate

Every `PUT` carrying a BO4E payload runs `mako_markt::bo4e::decode` — see
[The BO4E gate](@/docs/architecture/domain-model.md#the-bo4e-gate). A refusal
is a `422` naming the stage in `code`:

```json
{
  "error": "MARKTLOKATION carries 1 out-of-schema enum value(s) at: sparte",
  "code":  "bo4e.unknown_enum",
  "paths": ["sparte"]
}
```

- **The stored `data` is the canonical round-trip**, and the typed columns are
  derived from the same object, so a column cannot disagree with its document.
- **The envelope never asks for a field the BO declares.** `sparte` on a NeLo,
  `nutzung`/`verbrauchsart`/`ist_fernschaltbar`/`malo_id`/`melo_id` on a
  TechnischeRessource, `konfigurationsprodukte` on a SteuerbareRessource and
  `zaehler_typ`/`eichung_bis` on a Zaehler come from the payload. Fields the BO
  lacks — `nelos.nb_mp_id`, a MaLo's `fallgruppe`, `abwicklungsmodell`,
  `fernsteuerbar` — stay in the envelope, and an upsert leaves them alone.
- Writes that touch a shadowed column merge into the JSONB in the same
  statement.

### Endpoints

| Method | Path | Cedar action | Description |
|---|---|---|---|
| `GET` | `/health/live` · `/health/ready` | — | Liveness; readiness with a bounded DB ping |
| `GET` | `/metrics` | — | Prometheus |
| `PUT` | `/api/v1/malos/{malo_id}` | `write-malo` | Upsert a Marktlokation through the gate; pushes NB/MSB to makod's MaLo cache |
| `GET` | `/api/v1/malos/{malo_id}` | `read-malo` | Typed `Marktlokation` plus the typed columns |
| `GET` | `/api/v1/malos/{malo_id}/lastprofil` | `read-malo` | Lastprofil (SLP/TLP) of the MaLo |
| `GET` | `/api/v1/malos` | `read-malo` | List MaLos (records that do not decode are skipped) |
| `PUT` | `/api/v1/melos/{melo_id}` | `write-melo` | Upsert a Messlokation through the gate |
| `GET` | `/api/v1/melos/{melo_id}` | `read-melo` | Typed `Messlokation` |
| `GET` | `/api/v1/melos/{melo_id}/standorteigenschaften` | `read-melo` | BO4E `Standorteigenschaften` |
| `PUT` | `/api/v1/partners/{mp_id}` | `write-partner` | Upsert a Marktpartner — see [Trading partners](#trading-partners) |
| `GET` | `/api/v1/partners/{mp_id}` · `/api/v1/partners` | `read-partner` | One partner; list |
| `GET` | `/api/v1/partners/{mp_id}/as4-address` | `read-partner` | AS4 endpoint and certificate |
| `GET` | `/api/v1/partners/{mp_id}/marktteilnehmer` | `read-partner` | BO4E `Marktteilnehmer` view |
| `GET/PUT` | `/api/v1/mmma-preise/gas/{year}/{month}` | `read-mmma-preis` / `write-mmma-preis` | Gas MMM prices (THE) |
| `GET` | `/api/v1/mmma-preise/gas` | `read-mmma-preis` | List Gas MMM prices (`?limit=`) |
| `POST` | `/api/v1/mmma-preise/import-trigger` | `write-mmma-preis` | Run the monthly import now |
| `GET/PUT` | `/api/v1/mmm-preise/strom/{year}/{month}` | `read-mmma-preis` / `write-mmma-preis` | Strom MMM prices (one nationwide series) |
| `PUT` | `/api/v1/preisblaetter/{nb_mp_id}` | `write-preisblatt` | Upsert a Netznutzung sheet, snapshot it, emit `de.markt.pricat.published` |
| `GET` | `/api/v1/preisblaetter/{nb_mp_id}` | `read-preisblatt` | Sheet valid on `?date=` |
| `GET` | `/api/v1/pricat/{nb_mp_id}/history` | `read-pricat` | PRICAT versions, newest first |
| `GET` | `/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}` | `read-pricat` | Dispatch audit log of a version |
| `POST` | `/api/v1/pricat/{nb_mp_id}/dispatch` | `dispatch-pricat` | Re-queue the latest PRICAT to all active LF |
| `GET` | `/api/v1/versorgung/{malo_id}` | `read-versorgungsstatus` | Current state; `?at=YYYY-MM-DD` for point-in-time |
| `GET` | `/api/v1/versorgung/{malo_id}/history` | `read-versorgungsstatus` | Change history, newest first, paged |
| `PUT` | `/api/v1/versorgung/{malo_id}` | `write-versorgungsstatus` | ERP override (`If-Match` for OCC) |
| `GET/PUT` | `/api/v1/grundversorger/{nb_mp_id}` | `read-grundversorger` / `write-grundversorger` | §36 Abs. 2 EnWG Feststellung (`?sparte=STROM\|GAS`); optional `default_bilanzkreis` for an EoG ohne Antwort |
| `POST` | `/api/v1/esa/einwilligungen` | `write-einwilligung` | Grant an ESA consent (§49 Abs. 2 Nr. 9 MsbG); emits `de.markt.einwilligung.erteilt` |
| `GET` | `/api/v1/esa/einwilligungen` · `/{id}` | `read-einwilligung` | List active consents (`?esa_mp_id=`); one consent |
| `DELETE` | `/api/v1/esa/einwilligungen/{id}` | `write-einwilligung` | Revoke (GDPR Art. 7(3)); emits `de.markt.einwilligung.widerrufen` and one ORDERS 17008 Abbestellung per covered location |
| `PUT` | `/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}` | `write-einwilligung` | Prices of an accepted QUOTES 15003, filed by makod on the ORDRSP 19011 |
| `GET` | `/api/v1/esa/preise/{msb_mp_id}/{esa_mp_id}?at=` | `read-einwilligung` | Prices in force; invoicd's basis for an ESA INVOIC 31009 (an ESA has no Preisblatt) |
| `PUT/GET` | `/api/v1/esa/framework/{msb_mp_id}/{esa_mp_id}` | `write-einwilligung` / `read-einwilligung` | Framework agreement and AS4 certificate state |
| `PUT` | `/api/v1/esa/messprodukte/{msb_mp_id}` | `write-einwilligung` | Which Kapitel-4.6 Messprodukte the MSB serves, in which Abo mode; another code is `422` |
| `GET` | `/api/v1/esa/messprodukte/{msb_mp_id}/{messprodukt}?at=` | `read-einwilligung` | Served on `at`, in which mode — see [notes](#notes-on-the-endpoints) |
| `GET` | `/api/v1/esa/subscriptions/{bestellung_ref}` | `read-einwilligung` | Messprodukt behind an ORDERS 17007 Belegnummer, for edmd's Typ-2 surveillance; `404` → configured threshold |
| `GET` | `/api/v1/esa/consent-check` | `read-einwilligung` | `?esa_mp_id=&msb_mp_id=&location_id=&perspective=` → `{allowed, code, reason}`; `msb_inbound` (default, lenient) or `esa_outbound` (strict) |
| `GET` | `/api/v1/mabis-zp` | `read-mabis-zp` | Every Bilanzierungsgebiet → MaBiS-Zählpunkt |
| `GET` | `/api/v1/bilanzierungsgebiete/{eic}/mabis-zp` | `read-mabis-zp` | MaBiS-Zählpunkt (`LOC+172`); `404` makes `mabis-syncd` refuse, never fall back to the EIC |
| `PUT` | `/api/v1/bilanzierungsgebiete/{eic}/mabis-zp` | `write-mabis-zp` | Assign it; `400` for a value equal to the EIC or not 33 characters |
| `PUT` | `/api/v1/netzzugang/antraege` | `write-netzzugang` | Upsert a §20b EnWG request; emits `de.markt.netzzugang.antrag.updated` |
| `GET` | `/api/v1/netzzugang/antraege` · `/{id}` | `read-netzzugang` | List (`?status=&netzanschluss_id=`); one |
| `PATCH` | `/api/v1/netzzugang/antraege/{id}/status` | `write-netzzugang` | `erfasst → uebermittelt → bestaetigt/abgelehnt`; optional `platform_ref`, `expected_version` (→ `412`) |
| `PUT` | `/api/v1/msb-rahmenvertraege-gas` | `write-msb-rv-gas` | Gas MSB-Rahmenvertrag (GeLi Gas 3.0 Tenor 13–16); idempotent on `(tenant, gnb_mp_id, msb_mp_id, valid_from)`; `version` → `412` |
| `GET` | `/api/v1/msb-rahmenvertraege-gas` · `/{id}` | `read-msb-rv-gas` | List (`?msb_mp_id=&status=`); one |
| `GET` | `/api/v1/nelos` · `/{id}` | `read-nelo` | List (`?nb_mp_id=`); one by EIC or BDEW-Codenummer |
| `PUT` | `/api/v1/nelos/{id}` | `write-nelo` | Upsert through the gate; `sparte` from the payload, `STROM` or `GAS` only |
| `GET` | `/api/v1/tranchen` · `/{id}` | `read-tranche` | List (`?malo_id=`); one |
| `PUT` | `/api/v1/tranchen/{id}` | `write-tranche` | GPKE Teil 4 „Daten der Tranche" |
| `GET/PUT` | `/api/v1/malos/{malo_id}/grid` | `read-malo-grid` / `write-malo-grid` | Grid topology from NIS/GIS |
| `GET/PUT` | `/api/v1/preisblaetter-messung/{msb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattMessung`; GET includes `auf_abschlaege` |
| `GET/PUT` | `/api/v1/preisblaetter-ka/{nb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattKonzessionsabgabe` |
| `GET/PUT` | `/api/v1/preisblaetter-dienstleistung/{msb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattDienstleistung` |
| `GET/PUT` | `/api/v1/preisblaetter-hardware/{msb_mp_id}` | `read-preisblatt` / `write-preisblatt` | `PreisblattHardware` |
| `GET/PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}` | `read-sr` / `write-sr` | `SteuerbareRessource` |
| `GET/PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` | `read-sr` / `write-sr` | List or replace the §14a Konfigurationsprodukte |
| `DELETE` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}` | `write-sr` | Remove one |
| `GET/PUT` | `/api/v1/technische-ressourcen/{tr_id}` | `read-device` / `write-device` | `TechnischeRessource` |
| `GET` | `/api/v1/malos/{malo_id}/technische-ressourcen` | `read-device` | Those linked to a MaLo |
| `GET` | `/api/v1/malos/{id}/lokationen` · `/api/v1/melos/{id}/lokationen` | `read-lokationszuordnung` | Location graph (`?at=`) |
| `GET` | `/api/v1/malos/{id}/buendel` | `read-lokationszuordnung` | The [Lokationsbündel](#lokationsbuendel) and its audit |
| `PUT` | `/api/v1/lokationszuordnungen` | `write-lokationszuordnung` | Upsert an edge; `data` is a gated `Bo4e<Lokationszuordnung>` |
| `DELETE` | `/api/v1/lokationszuordnungen/{von_id}/{nach_id}` | `write-lokationszuordnung` | Delete an edge pair, every temporal variant |
| `GET` | `/api/v1/melos/{melo_id}/zaehler` | `read-device` | Zähler of a MeLo |
| `GET/PUT` | `/api/v1/melos/{melo_id}/msb` | `read-melo-msb` / `write-melo-msb` | MSB on `?at=` (WiM Teil 2 UC 4.1.1); PUT `{ msb_mp_id, valid_from }` closes the open assignment |
| `GET` | `/api/v1/melos/{melo_id}/msb/history` | `read-melo-msb` | Dated MSB timeline |
| `GET/PUT` | `/api/v1/malos/{malo_id}/bilanzierung` | `read-bilanzierung` / `write-bilanzierung` | BO4E `Bilanzierung` keyed on `(malo, bilanzierungsbeginn)`; GET at `?at=`. `422` without `bilanzierungsbeginn` or with an end at/before it |
| `GET` | `/api/v1/malos/{malo_id}/bilanzierung/history` | `read-bilanzierung` | Full history |
| `GET` | `/api/v1/melos/{melo_id}/sharing-eligibility` | `read-sharing-eligibility` | §42c EnWG metering capability: Zählerstandsgangmessung (§2 Satz 1 Nr. 27 MsbG) or viertelstündliche RLM, with `basis`, `required_action`, `reasons`, `evidence` |
| `PUT` | `/api/v1/zaehler/{zaehler_id}` | `write-device` | Upsert a Zähler through the gate |
| `GET` | `/api/v1/zaehler/{zaehler_id}/zaehlwerke` | `read-device` | `Zaehlwerk` registers (`[]` when none) |
| `GET` | `/api/v1/zaehler/{zaehler_id}/geraete` · `/{geraet_id}` | `read-device` | Geräte; one with `konfigurationen` |
| `GET/PUT` | `/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}/konfigurationen` | `read-device` / `write-device` | Replace `GeraetKonfiguration` entries; emits `de.markt.geraet.konfiguration.updated` |
| `GET/PUT` | `/api/v1/zaehler/{zaehler_id}/register` | `read-device` / `write-device` | TOU registers |
| `GET/PUT` | `/api/v1/zaehler-register/{register_id}/saisons` | `read-device` / `write-device` | Seasonal windows |
| `GET` | `/api/v1/zaehler/{zaehler_id}/tariff-zone` | `read-device` | HT/NT/EINZEL at `?datetime=` |
| `GET` | `/api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen` | `read-device` | BO4E `Zaehlzeitdefinition`; `?valid_only=true` |
| `PUT` | `/api/v1/geraete/{geraet_id}` | `write-device` | Upsert a Gerät through the gate |
| `GET/PUT` | `/api/v1/nb-contracts/{id}` | `read-nb-contract` / `write-nb-contract` | NB contract as `Vertrag`; PUT emits `de.markt.nb-contract.updated` |
| `GET` | `/api/v1/nb-contracts` | `read-nb-contract` | List (`?nb_mp_id=` required) |
| `GET` | `/api/v1/nb-contracts/by-malo/{malo_id}` | `read-nb-contract` | Contract in force on `?on=` |
| `GET/PUT` | `/api/v1/energiemix/{nb_mp_id}` | `read-energiemix` / `write-energiemix` | §42 EnWG Energiemix (`?year=`) |
| `GET` | `/api/v1/energiemix/{nb_mp_id}/history` | `read-energiemix` | Every published year |
| `GET` | `/api/v1/subscriptions` | `manage-subscription` | List subscribers |
| `GET/PUT/DELETE` | `/api/v1/subscriptions/{id}` | `manage-subscription` | One subscriber |
| `POST` | `/api/v1/subscriptions/{id}/test` | `manage-subscription` | Send a `de.markt.subscription.test` probe |
| `GET` | `/api/v1/correlations` · `/{id}` | `read-correlation` | Process correlations (`?malo_id=`, `?workflow=`); one by `process_id` or `erp_order_id` |
| `POST` | `[webhook] inbound_path` | — | Signed CloudEvent from makod, logged before fan-out |
| `GET` | `/admin/fanout/dlq` | `manage-fanout` | Dead-lettered deliveries |
| `POST` | `/admin/fanout/dlq/{event_id}/{subscriber_id}/retry` | `manage-fanout` | Re-deliver |
| `DELETE` | `/admin/fanout/dlq/{event_id}/{subscriber_id}` | `manage-fanout` | Discard |
| `GET` | `/admin/events` | `manage-fanout` | Replay log (`?from=&to=&type=&limit=`) |

### Notes on the endpoints

- **The Messprodukt answer has three states.** `als_abo` and `als_einmalig` are
  `true`, `false` or `null`; `null` („nothing recorded") makes the `E_0252` /
  `E_0256` walks escalate rather than refuse. A Pflichtprodukt is served whatever
  the catalogue holds (BNetzA Mitteilung Nr. 3, § 34 Abs. 2 S. 2 Nr. 10 MsbG).
- **A NeLo is Strom or Gas.** A `WASSER` or `FERNWAERME` Netzlokation is a
  payload error.
- **The Gas MSB-Rahmenvertrag** follows KoV XV Anlage 8 from 01.10.2026;
  `status=anpassung_erforderlich` lists the contracts still to migrate. Every
  upsert emits `de.markt.msb-rahmenvertrag-gas.updated` with `version`,
  `valid_from`, `valid_to`, `signed_at`.
- **Partner role `LFG` is refused** (`422`): a gas supplier is `LF` +
  Rollencodetyp `DVGW`.

### Consent expiry

`E_0256` Prüfschritt 8 code `A08` covers a consent „widerrufen **oder ihre
Gültigkeit ist abgelaufen**", and both owe the ORDERS 17008 Abbestellung.
`DELETE` covers the Widerruf; an **hourly sweep** closes every consent past its
`valid_to` through the same path.

It takes one consent at a time (`FOR UPDATE SKIP LOCKED`) and commits the
revocation together with its `de.markt.einwilligung.widerrufen` event; the 17008
is dispatched after the commit, so one consent yields one Abbestellung. A
failure leaves the consent open for the next sweep. The event's `grund` is
`einwilligung_widerrufen` or `einwilligung_abgelaufen`.

---

## Price sheets — PreisblattNetznutzung

Validity comes from the payload's `gueltigkeit.startdatum` / `enddatum`.
`?date=YYYY-MM-DD` selects the sheet (default today, Europe/Berlin).

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
  }
}
```

`bo4e_version` is provenance, returned on `GET` and not accepted on `PUT`. The
`GET` adds `source`, `updated_at` and `zeitvariable_preispositionen`:

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

A BO4E `Preis` keeps the currency in `einheit` (`CT` or `EUR` only), the
per-unit in `bezugswert` and the kind in `preisreferenz` (`ENERGIEMENGE` /
`LEISTUNG` / `ANZAHL` / `ZEITRAUM` / `PAUSCHAL`); a composite such as
`"CT_PRO_KWH"` in `einheit` decodes to `Unknown`.

Time bands live in the `zaehlzeitdefinition` the position's `zaehlzeitregister`
points into. The array is omitted when empty; `netzbilanzd` reads it for §14a
Modul 3 (BK8-22/010-A).

**`source` and the operator override.** `api` rows come from the REST `PUT`,
`mako` rows from the PRICAT ingest. An `api` row is never overwritten by a
`mako` row for the same NB and `valid_from`:

```sql
ON CONFLICT (nb_mp_id, valid_from)
DO UPDATE SET data = EXCLUDED.data, ...
WHERE preisblaetter.source <> 'api' OR EXCLUDED.source = 'api';
```

`preisblaetter_messung` carries the same protection.

### PRICAT 27003 dispatch

| | `preisblaetter` | `pricat_versions` |
|---|---|---|
| Answers | Which sheet is valid on date X? | What did we transmit, to whom, when? |
| Overlap | Forbidden (`EXCLUDE USING gist`) | Allowed |
| Correction | Replaces the row | Adds a version |
| Read by | `invoicd`, `netzbilanzd`, `invoic-checker` | Operators, via `/pricat/{nb}/history` |

`pricat_versions.data` is a **snapshot**, so a later correction never rewrites
what was transmitted. Only the Netznutzung sheet has a PRICAT.

A `PUT /api/v1/preisblaetter/{nb_mp_id}` writes, in **one transaction**, the
sheet, a snapshot keyed on `(nb_mp_id, tenant, valid_from)`, and the
`de.markt.pricat.published` outbox row. A background task then dispatches
PRICAT 27003 to every active LF via makod and logs each NB × LF attempt in
`pricat_dispatch_log`.

```mermaid
sequenceDiagram
    participant ERP
    participant marktd
    participant makod
    ERP->>marktd: PUT /api/v1/preisblaetter/{nb_mp_id}
    marktd->>marktd: preisblaetter + pricat_versions + outbox (one tx)
    marktd-->>ERP: 204 No Content
    Note over marktd: background task
    loop per active LF partner
        marktd->>makod: POST /api/v1/commands dispatch-pricat-27003
        marktd->>marktd: pricat_dispatch_log
    end
```

| State | Meaning |
|---|---|
| `pending` | Stored; no dispatch started |
| `queued` | Picked up by the dispatch task |
| `done` | Every active LF reached |
| `error` | Last attempt failed; retried on the next poll |

Registering a partner with `marktrolle = "LF"` re-queues the latest PRICAT for
it. `POST /api/v1/pricat/{nb_mp_id}/dispatch` re-queues manually — after an AS4
outage, for instance.

### PreisblattMessung

MSB metering tariffs, keyed on `msb_mp_id`; the basis for REQOTE/QUOTES and for
invoicd's checks of INVOIC 31009.

```bash
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
    }
  }'
```

Every `ZeitvariablePreisposition` must carry a non-empty `zaehlzeitregister`
(`422` otherwise, §14a Modul 3, BK8-22/010-A); a `bandNummer` field is `422`
because BO4E v202607 has none. `invoic-checker` check 4 routes INVOIC 31009
positions by that code.

---

## MMM settlement prices

Mehr-/Mindermengen prices are B2B settlement prices read by `netzbilanzd`
(issues MMM invoices) and `invoicd` (check 6 on inbound ones).

| | Gas | Strom |
|---|---|---|
| Publisher | Trading Hub Europe, per Marktgebiet | BDEW, one nationwide series |
| Key | month + `marktgebiet` | month alone |
| Route | `/api/v1/mmma-preise/gas/{year}/{month}` | `/api/v1/mmm-preise/strom/{year}/{month}` |
| `source` (CHECK) | `manual` \| `the-api` \| `csv-import` | `manual` \| `bdew-csv` \| `csv-import` |
| INVOIC | 31007/31008 | 31005/31006 |

The Strom series is *einheitlich* (§ 13 Abs. 3 StromNZV bis 31.12.2025, § 20
Abs. 3 EnWG über BK6-24-174 seit 01.01.2026), so `mmm_preise_strom` has no
operator or area column and the import refuses a file with more than one price
pair for a month.

```bash
curl -s -X PUT "http://marktd:8180/api/v1/mmma-preise/gas/2026/7" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "marktgebiet": "THE",
    "mehr_ct_kwh": "1.25",
    "minder_ct_kwh": "0.87",
    "source": "manual"
  }'

curl -s -X PUT "http://marktd:8180/api/v1/mmm-preise/strom/2026/7" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "mehr_ct_kwh": "2.10",
    "minder_ct_kwh": "1.45",
    "source": "manual"
  }'
```

**Monthly import.** With `[mmma_import] enabled = true` the worker wakes hourly
and, on the 1st at or after `check_hour_utc`, fetches `gas_url` / `strom_url`,
parses CSV (`year,month,marktgebiet,mehr_ct_kwh,minder_ct_kwh`) or JSON (one
object or an array) and upserts idempotently. It emits
`de.markt.mmma.import.success` or `de.markt.mmma.import.failed` (operator
action). Catch up by hand:

```bash
curl -s -X POST "http://marktd:8180/api/v1/mmma-preise/import-trigger?year=2026&month=7" \
  -H "Authorization: Bearer <token>"
```

Without the month's prices `netzbilanzd` needs them in the request and
`invoicd` skips check 6.

---

## Trading partners

`partners` is keyed on `mp_id` (BDEW- or DVGW-Codenummer).

| Part | Validation |
|---|---|
| `marktrolle`, `rollencodetyp`, `sparte` (top level) | Typed enums; `422` on an unknown value, including the EDIFACT `LFG` |
| `geschaeftspartner` | A `Bo4e<Geschaeftspartner>` through the gate; the canonical round-trip is stored |

`marktrolle`, `rollencodetyp` and `marktteilnehmerstatus` are `Marktteilnehmer`
fields. Inside `geschaeftspartner` they are absorbed as unvalidated extension
fields and read back as nothing — put them at the top level. AS4 endpoints go
in `makoadresse`.

```bash
# `mp_id` may be omitted — the path names the partner — and is refused if it differs.
curl -s -X PUT "http://marktd:8180/api/v1/partners/9904234560001" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Muster Energieversorgung GmbH",
    "marktrolle":    "LF",
    "rollencodetyp": "BDEW",
    "sparte":        "STROM",
    "makoadresse":   ["https://as4.muster-ev.de/as4/in"],
    "geschaeftspartner": {
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

The `GET` returns the same fields plus `mp_id`, `version` and `updated_at`. A
stored document that does not decode is returned raw in `geschaeftspartner`
rather than failing the read.

---

## Marktlokation and Messlokation

A `PUT /api/v1/malos/{malo_id}` decodes `data` as `Marktlokation` (`_typ`
injected when absent, `422` for another `_typ` or an unknown enum value),
stores the canonical camelCase form, and derives the typed columns from the
typed object:

| Column | Vocabulary | Purpose |
|---|---|---|
| `netzebene` | `NSP` \| `MSP` \| `HSP` \| `HSS` \| `MSP_NSP_UMSP` \| `HSP_MSP_UMSP` \| `HSS_HSP_UMSP` \| `HD` \| `MD` \| `ND` | NNE tier |
| `bilanzierungsgebiet` | EIC (`Y`) | `processd` NB check 4 |
| `gasqualitaet` | `H_GAS` \| `L_GAS` | Gas tariff routing |
| `energierichtung` | `EINSP` (feeds the grid) \| `AUSSP` (draws from it) | Named from the grid's side |
| `bilanzierungsmethode` | `RLM` \| `SLP` \| `IMS` \| `TLP_GEMEINSAM` \| `TLP_GETRENNT` \| `PAUSCHAL` | `netzbilanzd` Leistungspreis routing |
| `regelzone` | EIC | ÜNB for MaBiS IFTSTA 21000 and Redispatch 2.0 |

Every enum column holds a BO4E wire value (`as_wire()`), enforced by a SQL
`CHECK` listing the enum's `VARIANTS`; a `mako-markt` test pins the lists to
`rubo4e`. A field the payload lacks leaves its column `NULL`.

Three MaLo columns are **not** written by this `PUT`: `fallgruppe` (GaBi RLM)
and `abwicklungsmodell` (NZR-EMob, `MODELL_1`/`MODELL_2`; `NULL` means none
stated, not Modell 1) belong to `Bilanzierung`, and `fernsteuerbar` (§14a EnWG)
has no BO4E field. The `Bilanzierung` resource and the UTILMD
Stammdatenänderung (`TM+Z10`, `CCI+Z24++Z96/Z97`, `CCI+ZA2++ZE9/ZF0`) write
them. `lokationsbuendel_objektcode` on `malo` and `melo` is validated on write.

The `PUT` also pushes to makod's MaLo cache (`PUT /admin/malo/{malo_id}`,
fire-and-forget): `nb_mp_id` and `msb_mp_id` from `rollenzuordnung[]`
(`NB`/`GNB`, `MSB`/`GMSB`), `bilanzierungsgebiet`, `netzgebiet`
(`netzgebietsnr`) and `sparte`.

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

**From makod.** A `de.mako.process.initiated` for 55001/55077 or 44001 patches
`malo.bilanzierungsmethode` (UTILMD `TM+EM`: Z01→SLP, Z02→RLM, Z04→IMS) and
`malo.fallgruppe` (`TM+Z10`). A MaLo not yet stored is a no-op.

A `PUT /api/v1/melos/{melo_id}` runs the same gate and derives
`netzebene_messung` (BO4E `Netzebene` — `NSP`/`MSP`/…, never `NS`) and
`regelzone` from `standorteigenschaften.eigenschaftenStrom[0].regelzoneEic`.
Fill `regelzoneEic`, not `regelzone`: the latter is the zone's free-text name,
and a payload carrying only it leaves the column `NULL`.

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

---

## NB network contracts

`nb_contracts` carries typed columns (`netzebene`, `bilanzierungsmethode`,
`billing_schedule`, `netznutzer_mp_id`, `netznutzer_typ`, `valid_from`,
`valid_to`, plus `vertragsart` and `vertragsstatus` from the payload) and the
full BO4E `Vertrag` in `data`.

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

Without `data` a minimal `Vertrag` (`NETZNUTZUNGSVERTRAG`, `AKTIV`) is built.
`_typ` other than `VERTRAG` or an unknown enum is `422`. The `GET` adds
`contract_id`, `vertragsart`, `vertragsstatus`, `version` and `tenant`.
`netzebene` accepts the Strom levels `NS`/`MS`/`MSP`/`HSP`/`HS`/`HöS`/`HöS/HS`
and the Gas levels `GND`/`GMT`/`GHD`.

`netznutzer_typ` is `LIEFERANT` (default; the LF owes the Netznutzungsentgelt)
or `LETZTVERBRAUCHER` — the **Selbstzahler**, „Netznutzer ohne
All-Inklusiv-Vertrag", who takes the LF role in GPKE (Teil 1, Vorbemerkung) and
is an ordinary LF on the wire.

The flag matters only for the Lieferantenwechsel carve-out, where
[`processd`](@/docs/services/processd.md#selbstzahler-the-lieferantenwechsel-carve-out)
holds an `E03` for the operator.

`GET /api/v1/nb-contracts/by-malo/{malo_id}?on=` is the read `processd` makes
before deciding an Anmeldung.

---

## Database

`migrations/0001_initial.sql` is the whole schema.

| Table | Purpose |
|---|---|
| `malo` · `melo` | Location BOs — JSONB, `bo4e_version`, typed columns |
| `rollenzuordnungen` | Dated NB/LF/MSB roles per MaLo |
| `melo_msb_zuordnungen` | Dated MSB per MeLo, from IFTSTA 21012 — never from the vorläufige Anmeldebestätigung 55043 |
| `bilanzierungen` | BO4E `Bilanzierung` per MaLo; a currently effective one derives `malo.fallgruppe` |
| `lokationszuordnungen` | Location graph edges, typed `Lokationstyp` (`MALO`/`MELO`/`NELO`/`SR`/`TR`) |
| `partners` · `subscriptions` | Marktpartner; webhook subscribers |
| `process_correlation` · `processed_events` | Process tracking per MaLo; inbound idempotency |
| `preisblaetter` · `preisblaetter_messung` · `preisblaetter_konzessionsabgabe` · `preisblaetter_dienstleistung` · `preisblaetter_hardware` | Price sheets |
| `pricat_versions` · `pricat_dispatch_log` | PRICAT snapshots and dispatch attempts |
| `nb_energiemix` | §42 EnWG energy mix per NB and year |
| `mmma_preise_gas` · `mmm_preise_strom` | MMM prices |
| `versorgungsstatus` · `lf_zuordnung` · `versorgungsstatus_history` | Supply state, the supplier list, the append-only history |
| `grundversorger` | §36 Abs. 2 EnWG Feststellung per (NB, Sparte) and `default_bilanzkreis` |
| `mabis_zaehlpunkte` | Bilanzierungsgebiet → MaBiS-Zählpunkt |
| `nb_contracts` | NB contracts |
| `nelo` · `tranche` · `malo_grid` | NeLos; Tranchen (open `data`, no BO4E type exists); grid topology |
| `steuerbare_ressourcen` · `technische_ressourcen` | SR (`C[A-Z0-9]{9}[0-9]`); TR |
| `zaehler` · `geraete` · `zaehler_register` · `zaehler_saisons` | Device registry and TOU definitions |
| `event_log` · `event_delivery` | Fan-out outbox and per-subscriber delivery queue |
| `esa_einwilligungen` · `esa_framework_agreements` · `esa_messprodukt_preise` · `esa_messprodukt_katalog` | ESA consents, agreements, accepted prices, product catalogue |
| `netzzugang_antraege` · `msb_rahmenvertraege_gas` | §20b requests; Gas MSB-Rahmenverträge |

### Temporal integrity

Dated rows use **half-open `[valid_from, valid_to)`**: a successor starts on its
predecessor's `valid_to`, `NULL` is open-ended. Overlap is unrepresentable —
eight `EXCLUDE USING gist` constraints (with `btree_gist`) refuse two rows in
force on the same day:

| Constraint | Scoped by |
|---|---|
| `rollenzuordnungen_no_overlap` | MaLo × Zuordnungstyp |
| `melo_msb_no_overlap` | tenant × MeLo |
| `nb_contracts_no_overlap` | tenant × MaLo × NB |
| `preisblaetter_no_overlap` | NB |
| `preisblaetter_messung_no_overlap` | MSB |
| `preisblaetter_ka_no_overlap` | NB × Sparte × Kundengruppe |
| `preisblaetter_dienstleistung_no_overlap` · `preisblaetter_hardware_no_overlap` | MSB |

The price-sheet tables add `UNIQUE NULLS NOT DISTINCT (…, valid_from)`, so a
re-`PUT` of an open-started sheet updates instead of duplicating.

An overlap (`23P01`) or a `CHECK` breach (`23514`) is a **`422`** naming the
constraint:

```json
{
  "error": "conflicting key value violates exclusion constraint \"melo_msb_no_overlap\": the row's [valid_from, valid_to) window overlaps one already stored. Close the existing row at the new start date, or move valid_from."
}
```

Close the incumbent's `valid_to` at the new start. An abutting successor is
never an overlap. `tests/it/temporal_constraints_integration.rs` and
`tests/it/overlap_is_a_client_error.rs` pin each constraint against PostgreSQL.

---

## VersorgungsStatus

One record per MaLo, derived from inbound CloudEvents. Every write appends to
`versorgungsstatus_history` in the same transaction and emits
`de.markt.versorgung.changed`. All operations are idempotent under
at-least-once delivery.

| Phase | CloudEvent | PID | Operation | Effect |
|---|---|---|---|---|
| Announce | `process.initiated` | 55001 / 55077 / 44001 | `announce_lf_next` | Adds an `Angekuendigt` assignment (100 % or the Tranche share) with its Zuordnungsbeginn; `lieferstatus` unchanged |
| Confirm | `process.completed` | 55002 / 55078 / 44002 | `confirm_supply` | Promotes the named supplier to `Aktiv`, displacing only the running assignment on the same Tranche; `Beliefert` |
| End | `process.completed` | 55005 / 44005 | `end_supply` | Removes the named running assignment (all when none named); `Unbeliefert` only when none is left. An uncovered interval emits `de.markt.versorgung.gap-detected` |
| EoG | `process.completed` | 55013 / 44013 | `begin_eog_supply` | `Ersatzversorgung`/`Grundversorgung` (per `eog_art`), `eog_seit` = Zuordnungsbeginn; Bilanzkreis from the payload, else the NB's `default_bilanzkreis`. Emits `de.markt.versorgung.eog-begonnen` |
| Clear | `process.completed` | 55003 / 55080 / 44003 | `clear_lf_next` | Drops the refused supplier's announcement; also the write behind 55038 / 44038 |
| Stammdatenänderung | `process.completed` | GPKE Teil 4 / GeLi Gas | `patch_stammdaten` | See below |

```
VersorgungsStatusRecord
├── malo_id, nb_mp_id, msb_mp_id
├── lieferstatus         — Beliefert | Unbeliefert | Grundversorgung | Ersatzversorgung | Ruhend | Stillgelegt
├── zuordnungen[]        — the authoritative supplier list
│   ├── lf_mp_id, prozent, tranche_id (SG5 LOC+Z21; null untranchiert)
│   ├── status           — Angekuendigt | Aktiv
│   └── zuordnungsbeginn, zuordnungsende
├── lf_mp_id, lf_mp_id_next, lieferbeginn, lf_next_lieferbeginn
│                        — derived; null when there are 0 OR several
├── lieferende, eog_seit, last_process_id, updated_at
└── version              — OCC counter
```

**A `null` derived supplier does not mean unsupplied** — it also means
„several", the normal state of a tranchierte Marktlokation. Read `zuordnungen`.

```mermaid
stateDiagram-v2
    [*] --> Unbeliefert : MaLo registered
    Unbeliefert --> Beliefert   : 55002/55078/44002 confirm_supply
    Beliefert --> Beliefert     : 55005/44005 on one Tranche
    Beliefert --> Unbeliefert   : 55005/44005, last assignment gone
    Unbeliefert --> Ersatzversorgung : 55013/44013 (§38 EnWG)
    Unbeliefert --> Grundversorgung  : 55013/44013 (§36 EnWG)
    Ersatzversorgung --> Grundversorgung : 3 Monate, Haushaltskunde (by law)
    Ersatzversorgung --> Beliefert   : confirm_supply (clears eog_seit)
    Grundversorgung --> Beliefert    : confirm_supply
    Beliefert --> Ruhend    : suspended
    Ruhend --> Beliefert    : reactivated
    Beliefert --> Stillgelegt : decommissioned
    Stillgelegt --> [*]
```

**Gap closure (§38 EnWG).** A gap is an interval no supplier covers. `end_supply`
finds one when the successor starts later or not at all; a Bestätigung finds one
in Fall b, when the Altlieferant answered with an earlier Lieferende (`E_0624`
`A34`, carried as `lfa_lieferende`). Both emit `gap-detected` with
`gap_from`/`gap_until`; `processd` resolves the Grundversorger and dispatches
`gpke.eog.anmelden` (UTILMD 55013).

**GPKE rule A06.** `processd` checks for an announcement by a supplier **other
than** the requesting one. The announcement is recorded before the event is
fanned out, so the Anmeldung under evaluation is already in the projection.

**Tranchen.** `E_0623` Prüfschritte 500–540 decide on the arithmetic over the
shares. A constraint trigger holds the `Aktiv` shares of one MaLo to ≤ 100 % (a
60/60 split is `422`; 60/30 is stored, and Prüfschritt 530 asks about the
remainder).

`Angekuendigt` rows are unbounded — `E_0622` Prüfschritt 70 rules on competing
announcements. A Tranche is always > 0 % and < 100 % (GPKE Teil 1 § 3.2.1.5), so
a `tranche_id` at 100 % is refused. Each refusal is a `422` mapped from the
SQLSTATE.

**Concurrency.** Writes are `WHERE … AND version = $n`; a conflict is
`412 Precondition Failed`.

### `patch_stammdaten`

`data.objekt` selects the typed-column patch; each present field overwrites its
column, the JSONB and `version` are untouched, and an object not held locally is
a no-op.

| `objekt` | Table | Beyond the plain columns |
|---|---|---|
| `MARKTLOKATION` | `malo` | §14a `fernsteuerbar` |
| `MESSLOKATION` | `melo` | MSB-Zuordnung (`CAV+7111=Z91`) on `melo_msb_zuordnungen`, effective the Änderungsdatum |
| `NETZLOKATION` | `nelo` | §14a `steuerkanal` |
| `TECHNISCHE_RESSOURCE` | `technische_ressourcen` | `nutzung` (`CCI+7059`), `verbrauchsart` (`CAV+7111`), `ist_fernschaltbar` |
| `STEUERBARE_RESSOURCE` | `steuerbare_ressourcen` | Konfigurationsprodukte, **replaced**: one per `SG8 SEQ+Z79` (`PIA+5`, `CAV+Z91`/`ZF0`, `CCI+Z66`) |
| `TRANCHE` | `tranche` | — |

It emits `de.markt.malo.stammdaten-geaendert` for a MaLo and
`de.markt.stammdaten.geaendert` otherwise.

### Reading it

```http
GET  /api/v1/versorgung/{malo_id}
GET  /api/v1/versorgung/{malo_id}?at=2025-10-01
GET  /api/v1/versorgung/{malo_id}/history?page=0&size=50
PUT  /api/v1/versorgung/{malo_id}
```

`?at=` returns the state at end of that German calendar day, or `404` when no
history exists by then.

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

---

## Device registry

```
MeLo ──► Zaehler (1..n) ──► Geraete (0..n)
              └──► Zaehlwerk (0..n, OBIS registers in data.zaehlwerke)
```

- **`Zaehler`** — `data` through the gate. `zaehler_typ` (from `zaehlertyp`,
  `CHECK`ed to the 13 BO4E `Zaehlertyp` variants; `GASZAEHLER` and `UNKNOWN`
  are not among them) and `eichung_bis` (from `eichungBis`) are derived, never
  sent. `Zaehlertyp` spells `INTELLIGENTES_MESSSYSTEM` (three `s`),
  `Geraetetyp` `INTELLIGENTES_MESSYSTEM` (two).
- **`Geraet`** — `data` through the gate; `geraet_typ` from `geraetetyp`, not
  `CHECK`ed; `konfigurationen` stored separately.
- The four device `PUT`s (`zaehler`, `geraete`, `steuerbare-ressourcen`,
  `technische-ressourcen`) inject `_typ` when absent.

```bash
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "melo_id": "DE00056789000000000000000012345678",
    "data": {
      "zaehlertyp": "DREHSTROMZAEHLER",
      "eichungBis": "2030-12-31T23:00:00Z",
      "zaehlwerke": [
        { "_typ": "ZAEHLWERK", "obisKennzahl": "1-0:1.8.0", "richtung": "EINSP" }
      ]
    }
  }'
```

### Gerät configuration (MsbG §23)

`geraete.geraet_konfigurationen` is a GIN-indexed JSONB array of
`{parameter, wert, updated_at, notiz}`. `updated_at` is set by the server;
duplicate `parameter`s in one body keep the last.

| `parameter` | Value | Source |
|---|---|---|
| `FIRMWARE_VERSION` | string | BSI TR-03109-1 §4.3 |
| `HARDWARE_REVISION` · `HERSTELLER` · `MSB_VERTRAGSNUMMER` | string | MsbG §23 |
| `KOMMUNIKATION` | `GPRS` / `PLC` / `ETHERNET` / `FUNK` / `FESTNETZ` / `GSM` | — |
| `FERN_UPDATE_FAEHIG` | `"true"` / `"false"` | BSI TR-03109-4 |
| `CLS_FAEHIG` | `"true"` / `"false"` | §14a EnWG, BK6-22-300 |
| `SMGW_TLS_CERT_FINGERPRINT` | 64 hex chars | BSI TR-03109-3 |
| `SMGW_CERT_ABLAUFDATUM` | ISO date | BSI TR-03109-4 §6.3 |
| `CLS_KANAL_ID` | string | GPKE Teil 3 Kap. 2 |
| `GWA_CODENUMMER` | 13 digits | BDEW |
| `INBETRIEBNAHMEDATUM` · `LETZTE_WARTUNG` · `NAECHSTE_WARTUNG` | ISO date | — |
| `AUSLESE_PROTOKOLL` | `SML` / `DLMS` / `IEC62056` | — |
| `SONSTIGES` | string; key in `notiz` | — |

```bash
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001/konfigurationen" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "konfigurationen": [
      { "parameter": "FIRMWARE_VERSION",       "wert": "3.1.2"            },
      { "parameter": "CLS_FAEHIG",             "wert": "true"             },
      { "parameter": "CLS_KANAL_ID",           "wert": "CLS-00042"        },
      { "parameter": "SMGW_CERT_ABLAUFDATUM",  "wert": "2027-06-30"       },
      { "parameter": "GWA_CODENUMMER",         "wert": "9900000000099"    }
    ]
  }'
# → 204 No Content + de.markt.geraet.konfiguration.updated
```

The configuration is master data for the ERP and `agentd`'s
`smgw-diagnostics-agent`. `processd`'s §14a Steuerungsauftrag decides on the
SteuerbareRessource's `istFernschaltbar` and Konfigurationsprodukte, not on
`CLS_FAEHIG`; `edmd`'s certificate-expiry worker sweeps the `SmgwSession`
registered with edmd, not `SMGW_CERT_ABLAUFDATUM`.

### TOU registers and Zaehlzeitdefinition

`zaehler_register` (HT / NT / EINZEL, OBIS, validity) and `zaehler_saisons`
(`SOMMER` / `WINTER` / `GESAMT`, `wochentage`, `zeit_von` inclusive and
`zeit_bis` exclusive, German local time) describe iMSys TOU bands.
`GET …/tariff-zone?datetime=` resolves one local datetime in a single JOIN;
`GET …/zaehlzeitdefinitionen` assembles them into a BO4E `Zaehlzeitdefinition`.

Both `PUT` bodies are the stored record, so every field is required:
`zaehler_id`, `tenant` and `register_id` are overwritten from the path and the
caller, `updated_at` by the server — send them anyway, or the `422` names the
missing field.

```bash
REGISTER_ID=$(uuidgen)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567/register" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d "{
    \"id\": \"${REGISTER_ID}\",
    \"zaehler_id\": \"Z001234567\",
    \"tenant\": \"9900357000004\",
    \"bezeichnung\": \"HT\",
    \"zaehlerauspraegung\": \"HT\",
    \"obis_kennzahl\": \"1-1:1.29.0\",
    \"einheit\": \"KWH\",
    \"valid_from\": \"2025-01-01\",
    \"updated_at\": \"2025-01-01T00:00:00Z\"
  }"

curl -s -X PUT "http://marktd:8180/api/v1/zaehler-register/${REGISTER_ID}/saisons" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d "{
    \"id\": \"$(uuidgen)\",
    \"register_id\": \"${REGISTER_ID}\",
    \"saison\": \"WINTER\",
    \"wochentage\": [1,2,3,4,5],
    \"zeit_von\": \"07:00\",
    \"zeit_bis\": \"22:00\",
    \"updated_at\": \"2025-01-01T00:00:00Z\"
  }"

curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/tariff-zone?datetime=2026-01-15T14:30:00" \
  -H "Authorization: Bearer <token>"
# → { "zaehler_id": "Z001234567", "local_datetime": "2026-01-15T14:30:00", "tariff_zone": "HT" }
```

**From WiM Stammdaten.** A `de.mako.process.completed` for ORDERS 17102–17133
carrying `melo_id` and `zaehlwerke` (makod's `extract_zak_ze_zaehlwerke` over
`ZAK`/`ZE`/`ZD`) upserts the registers and seasons of the MeLo's first Zähler,
with deterministic ids so a redelivery is harmless. A failure is logged and does
not fail the ingest.

### SteuerbareRessource and TechnischeRessource

An SR-ID is `C[A-Z0-9]{9}[0-9]` (check digit by the ASCII procedure). The
`konfigurationsprodukte` sub-resource is replaced as a whole; each element must
decode as `Konfigurationsprodukt` with a non-empty `produktcode`, and
`bandNummer` is `422`. A `PUT` emits `de.markt.sr.konfigurationsprodukt.updated`;
`processd` refuses a Steuerungsauftrag for an uncontracted `produktcode`.

```bash
curl -s -X PUT "http://marktd:8180/api/v1/steuerbare-ressourcen/C0000000011/konfigurationsprodukte" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '[
    { "produktcode": "FLEX-PRODUCT-001", "zaehlzeitregister": "HT" }
  ]'
```

A TechnischeRessource links a charging point, generator or storage unit to a
MaLo/MeLo. `nutzung` (`STROMVERBRAUCHSART` | `STROMERZEUGUNGSART` | `SPEICHER`),
`verbrauchsart` (`KRAFT_LICHT` | `WAERME` | `E_MOBILITAET` |
`STRASSENBELEUCHTUNG`, consumption only) and `ist_fernschaltbar` come from the
payload; an invalid value is `400`.

### NeLo

A Netzlokation is identified by a 16-character EIC (NAD `ZEW`) or a 13-digit
BDEW-Codenummer and owned by an NB. `steuerkanal`, `eigenschaft_msb_lokation`
and `grundzustaendiger_msb_codenr` are typed columns. Writes need the `NB` role.

---

## Location graph

`lokationszuordnungen` holds directed, dated edges between typed nodes.
`GET /api/v1/malos/{id}/lokationen` (or `/melos/{id}/lokationen`) walks them
with a recursive CTE (max depth 8), ordered by `depth`; `?at=` filters by
validity. Several edges per `(von_id, nach_id)` are allowed with distinct
`valid_from`, at most one open-started.

### Lokationsbündel {#lokationsbuendel}

`GET /api/v1/malos/{id}/buendel` projects the bundle of MeLos, NeLos, SRs and
TRs under a MaLo (`mako_markt::repository::Lokationsbuendel`). A consuming MaLo
needs at least one MeLo and every MeLo the same MSB; a bundle incomplete
mid-Einzug reports `valid: false` with `validation_error` rather than failing.

`struktur` and `befunde` audit the declared `lokationsbuendelcode` — a code from
EDI@Energy's *Codeliste der Lokationsbündelstrukturen* (v1.0, 31.03.2023,
applicable from 01.10.2024): check digit, structure, then per-object-type
cardinalities.

Steuerbare Ressourcen have no code in the list, and the projection keeps only
the root MaLo, so the three *Summenmessung* structures report „audit the BO4E
Lokationszuordnung instead". The per-code audit is `rubo4e`'s
`Lokationszuordnung::audit_buendel()` over an edge's `data`.

```json
{
  "malo_id": "51238696012",
  "lokationsbuendelcode": "9992000000026",
  "messlokationen": ["DE0001234567890123456789012345678"],
  "netzlokationen": [],
  "steuerbare_ressourcen": [],
  "technische_ressourcen": [],
  "valid": true,
  "struktur": {
    "code": "9992000000026",
    "bezeichnung": "Verbrauch mit einer Messlokation (Standard)",
    "max_ebene": 1
  },
  "befunde": []
}
```

A departure from the declared structure is **reported, not refused**
(`"the structure permits 1 Messlokation, the bundle holds 2"`).
`PUT /api/v1/lokationszuordnungen` answers `{ "id", "befunde" }` on the same
terms — an edge is often asserted before its objects exist. A `malo`/`melo`
`PUT` does refuse a mis-typed object code, because it lands in a typed column.

```http
PUT /api/v1/lokationszuordnungen
Content-Type: application/json

{
  "von_id":    "51238696012",
  "von_typ":   "MALO",
  "nach_id":   "DE-MEL-001",
  "nach_typ":  "MELO",
  "valid_from": null,
  "valid_to":   null,
  "data": {
    "lokationsbuendelcode": "9992000000026"
  }
}
```

---

## Fan-out

Every produced or relayed event is written to `event_log` **before** delivery.
A two-phase worker drains it:

1. **Fan-out** claims pending `event_log` rows, resolves matching subscribers and
   writes one `event_delivery` row each, in one transaction.
2. **Deliver** claims due deliveries (`FOR UPDATE SKIP LOCKED`), signs and POSTs
   them, and backs off 30 s → 5 min → 30 min → 2 h; after
   `max_retry_attempts` the row is marked `dead_lettered_at`.

The worker polls every 30 s and is woken by a Postgres `NOTIFY` from an
`AFTER INSERT` trigger, which fires only on commit. The **claim** counts the
attempt, so a lost outcome write still spends budget; a sweep dead-letters rows
whose budget is spent and whose lease has lapsed. Delivery is at-least-once —
deduplicate on the CloudEvent `id`. Retention of every event follows § 147 AO /
GoBD.

**Ordering is per aggregate.** A delivery waits while an earlier event about the
same MaLo (`ordering_key`) is outstanding to the same subscriber; a
dead-lettered delivery stops blocking its key.

**Filters.** `roles` and `sparten` match the `marktrole` / `marktsparte`
extensions; an empty array matches everything, and an event without the
extension matches every filter on that axis. `event_types` uses
`mako_events::matches`: exact types, trailing `*` (`de.markt.*`), mid-pattern
globs (`de.*.rechnung.erstellt`) and `?`.

```bash
curl -X PUT http://localhost:8180/api/v1/subscriptions/erp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "webhook_url":    "https://erp.example.com/mdm/events",
    "webhook_secret": "mysecret64hexchars",
    "event_types":    ["de.markt.malo.updated", "de.markt.pricat.published",
                       "de.mako.process.completed"]
  }'
```

### Signature verification

Deliveries are signed [Standard Webhooks](https://www.standardwebhooks.com/)
style by `mako_service::webhook::sign`:

```text
webhook-id:        de.markt.malo.updated/3f2b…    ← also the dedup key
webhook-timestamp: 1786012800                     ← seconds since epoch
webhook-signature: v1,K5oT9r…                     ← base64, space-separated list
```

The signature is the base64 HMAC-SHA256 of `{webhook-id}.{webhook-timestamp}.{body}`
under the subscription secret. The list form allows secret rotation.

```python
import base64, hmac, hashlib

def verify(secret: str, headers, body: bytes) -> bool:
    signed = b"%s.%s.%s" % (headers["webhook-id"].encode(),
                            headers["webhook-timestamp"].encode(), body)
    expected = base64.b64encode(
        hmac.new(secret.encode(), signed, hashlib.sha256).digest()).decode()
    return any(hmac.compare_digest(expected, sig.split(",", 1)[1])
               for sig in headers["webhook-signature"].split()
               if sig.startswith("v1,"))
```

Also refuse a `webhook-timestamp` more than **5 minutes** from your clock,
deduplicate on `webhook-id`, and answer `200` to a duplicate (non-2xx is
retried). The secret is stored in plaintext in `subscriptions.webhook_secret`;
protect it with database grants and storage encryption.

### Dead-letter queue

Dead-lettered deliveries stay on `event_delivery`, keyed by
`(event_id, subscriber_id)`:

```bash
curl http://localhost:8180/admin/fanout/dlq \
  -H "Authorization: Bearer $TOKEN" | jq '.[] | {event_id, subscriber_id, attempts, last_error}'

curl -X POST http://localhost:8180/admin/fanout/dlq/$EVENT_ID/$SUBSCRIBER_ID/retry \
  -H "Authorization: Bearer $TOKEN"

curl -X DELETE http://localhost:8180/admin/fanout/dlq/$EVENT_ID/$SUBSCRIBER_ID \
  -H "Authorization: Bearer $TOKEN"
```

A retry signs with the subscriber's current secret. `GET /admin/events`
replays `event_log` oldest first (`?from=&to=&type=&limit=`) — for onboarding a
subscriber or re-delivering after a consumer fix.

### Metrics

Four gauges, sampled together once a minute:

| Metric | Meaning |
|---|---|
| `marktd_fanout_dlq_depth` | Dead-lettered deliveries; non-zero means a subscriber is missing events |
| `marktd_fanout_pending` | Logged but not yet fanned out; rising means the worker is behind or stopped |
| `marktd_active_subscriptions` | Active subscribers |
| `marktd_processed_events` | Rows in the inbound idempotency table, bounded by the retention sweep; unbounded growth means the sweep is not running |

`mako_service` adds request count, latency and the BO4E decimal-spelling
counter — see
[what `/metrics` carries on every service](@/docs/architecture/_index.md#what-metrics-carries-on-every-service).

---

## CloudEvents

Source: `urn:mako:marktd:tenant:{tenant}`.

| `type` | `subject` | Trigger | Consumers |
|---|---|---|---|
| `de.markt.malo.updated` | `malo_id` | MaLo `PUT` | `edmd`, `processd`, ERP |
| `de.markt.melo.updated` | `melo_id` | MeLo `PUT` | `edmd`, `processd`, ERP |
| `de.markt.malo.stammdaten-geaendert` | `malo_id` | Stammdatenänderung on a MaLo, with the `patch` | ERP |
| `de.markt.stammdaten.geaendert` | object id | Stammdatenänderung on another object | ERP |
| `de.markt.partner.updated` | `mp_id` | Partner `PUT` | `makod`, ERP |
| `de.markt.nb-contract.updated` | `contract_id` | NB contract `PUT` | ERP |
| `de.markt.pricat.published` | `nb_mp_id` | Preisblatt `PUT` | `netzbilanzd`, `invoicd`, ERP |
| `de.markt.sr.konfigurationsprodukt.updated` | `sr_id` | Konfigurationsprodukte replaced | `processd`, ERP |
| `de.markt.geraet.konfiguration.updated` | `geraet_id` | Konfigurationen `PUT` (`geraet_id`, `zaehler_id`, `count`) | `agentd`, ERP |
| `de.markt.msb-rahmenvertrag-gas.updated` | contract id | Gas MSB-Rahmenvertrag upsert | ERP |
| `de.markt.netzzugang.antrag.updated` | request id | §20b request upserted or advanced | `makod`, ERP |
| `de.markt.einwilligung.erteilt` | consent id | Consent granted | `makod`, ERP |
| `de.markt.einwilligung.widerrufen` | consent id | Revoked or expired (`grund`) | `makod` (17008), ERP |
| `de.markt.subscription.test` | subscriber id | Test probe | the subscriber |
| `de.markt.versorgung.changed` | `malo_id` | Every supply transition and the REST upsert | `vertragd`, `billingd`, ERP |
| `de.markt.versorgung.gap-detected` | `malo_id` | Uncovered interval (§38 EnWG) | `processd` |
| `de.markt.versorgung.eog-begonnen` | `malo_id` | 55013/44013 completed | `processd`, ERP |
| `de.markt.mmma.import.success` · `.failed` | year-month | Monthly MMM import | `netzbilanzd`, `invoicd`; operator |

Relayed from makod unchanged: `de.mako.process.initiated`,
`de.mako.process.completed`, `de.mako.process.failed`,
`de.mako.aperak.accepted`, `de.mako.aperak.rejected`, `de.mako.aperak.timeout`.

| Extension | Meaning |
|---|---|
| `marktmaloid` · `marktmeloid` | Resolved MaLo / MeLo; the MaLo is the ordering key |
| `marktrole` | `NB`, `LF`, `MSB`, `BIKO`, `UNB` |
| `marktsparte` | `STROM` / `GAS`; absent = not Sparte-scoped |
| `markterpref` | ERP `Idempotency-Key` |
| `makoconvid` · `makopid` · `makoworkflow` | Forwarded from makod |
| `makoerc` | ERC code on `aperak.rejected` |
| `traceparent` · `tracestate` | W3C Trace Context |

`marktrole` on a relayed event comes from `makoworkflow`: ending in `-lf` or
containing `-lf-` → `LF`; `wim-` → `MSB`; `mabis-` → `BIKO`; otherwise `NB`.

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

To receive makod's events, point makod at the inbound path:

```toml
# makod.toml
[erp]
webhook_url    = "http://marktd:8180/api/v1/mako/events"
webhook_secret = "shared-hmac-secret"
```

Inbound delivery is idempotent on `event_id`.

---

## MCP server

`POST /mcp` / `GET /mcp` (Streamable HTTP, spec 2025-11-25) has its **own**
credential layer, `[mcp]` (API key or OIDC), independent of the REST `[oidc]`;
the principal is then checked against the single Cedar action `use-mcp`.

**Tools (17)**, every one a read:

| Tool | Description |
|---|---|
| `get_malo` / `list_malo` | One MaLo / list |
| `get_melo` / `get_melo_standorteigenschaften` | MeLo record / site properties |
| `get_partner` / `list_partners` | Marktpartner registry |
| `get_preisblatt` | Netznutzung sheet for an NB and date |
| `list_pricat_versions` | PRICAT history; dispatch stays REST-only, because `use-mcp` cannot tell a read from a write |
| `get_versorgungsstatus` / `get_versorgungsstatus_history` / `get_versorgung_at` | Supply state — current, history, point-in-time |
| `get_rollenzuordnung` | Dated NB/MSB/LF roles |
| `get_nb_contract` | NB contract for a MaLo |
| `get_nb_energiemix` | §42 EnWG energy mix |
| `get_correlation` | Process correlation |
| `get_technische_ressource` / `get_steuerbare_ressource` | §14a device registry |

**Prompts (4):** `lookup-malo`, `investigate-supply-gap`,
`versorgungswechsel-tracking`, `grid-topology`.

---

## Troubleshooting

| Symptom | Cause |
|---|---|
| Container exits at start | A CLI flag other than `--check`/`--print-config`/`--help`, or a missing `[markt] tenant` — the config path is `MARKTD_CONFIG` |
| `401` | `[oidc] issuer`, an expired token, or no `mako_tenant` claim |
| `403` | `mako_tenant` ≠ `[markt] tenant`, or the role is missing (`NB` for grid-operator writes, `ADMIN` for subscriptions and the DLQ) |
| `404` on a Preisblatt | No sheet valid on `?date=` |
| A PRICAT ingest does not replace a sheet | Intended: an `api` row is never overwritten by a `mako` row |
| `422` naming a `*_no_overlap` constraint | The new window overlaps the row in force; close its `valid_to` |
| Anmeldungen refused or deferred | A `processd` decision. `marktd` owes it data: the MaLo `PUT`, an NB `rollenzuordnung` naming this tenant, a price sheet covering the date |
| `relation "malo" does not exist` | Migrations did not run — check the database URL |

---

## See also

- [`makod`](@/docs/services/makod.md) · [`processd`](@/docs/services/processd.md) · [`netzbilanzd`](@/docs/services/netzbilanzd.md) · [`invoicd`](@/docs/services/invoicd.md) · [`edmd`](@/docs/services/edmd.md) · [`obsd`](@/docs/services/obsd.md) · [`sperrd`](@/docs/services/sperrd.md)
- [ERP Integration](@/docs/architecture/erp-integration.md) · [Domain Model](@/docs/architecture/domain-model.md) · [Getting Started](@/docs/guide/getting-started.md)
