# marktd

`marktd` (`:8180`) is the market data hub: the source of truth for what market
messages are *about* — Marktlokationen and Messlokationen, the supply-status
lifecycle, devices, the location graph, price sheets, counterparties and the
ERP webhook fan-out. It holds **no domain policy**: it records `makod`'s process
events and fans them out; `processd` decides.

## Run it

```bash
just dev marktd        # dev/config/marktd.toml against the dev PostgreSQL
```

The file is optional: `MARKTD_CONFIG` names it, `MARKTD_<SECTION>__<KEY>`
overrides any key, and a `*_FILE` variable reads a value from a file.

## At a glance

| | |
|---|---|
| **Database** | PostgreSQL 15+ with `btree_gist` — validity is half-open `[valid_from, valid_to)`, enforced by `EXCLUDE` constraints |
| **Auth** | OIDC + Cedar `policies/marktd.cedar`: reads and writes for the tenant; NB-published data (Preisblatt, NeLo, Tranche, Grundversorger) needs `NB`; ESA consent needs `MSB`/`ESA`; subscriptions need `ADMIN` |
| **BO4E** | `GET` returns `rubo4e::current` types; every `PUT` validates `_typ` and refuses out-of-schema values (`422`) |
| **Supply status** | `de.mako.process.*` events drive `lf_zuordnung` (one row per assignment: supplier, share, Tranche, `Angekuendigt`/`Aktiv`); every transition writes history (`?at=YYYY-MM-DD`) and emits `de.markt.versorgung.changed` |
| **Events** | CloudEvents 1.0 from `urn:mako:marktd:tenant:{tenant}`, HMAC-signed, durable two-phase fan-out ordered per aggregate |
| **API spec** | OpenAPI 3.1 at `/api/v1/openapi.json` — open it in any Swagger UI or Redoc |
| **MCP** | 17 read-only tools and 4 prompts at `/mcp` |

Startup refuses a missing `[oidc]` or `webhook.inbound_secret` unless
`allow_insecure_no_auth = true`.

## Routes

| Area | Routes |
|---|---|
| Marktlokation | `/api/v1/malos` (+ `/{id}`, `/lastprofil`, `/grid`, `/bilanzierung`, `/lokationen`, `/buendel`), `/api/v1/versorgung/{malo_id}` (+ `/history`) |
| Messlokation & devices | `/api/v1/melos/{id}` (+ `/msb`, `/zaehler`, `/sharing-eligibility`), `/api/v1/zaehler/{id}/…`, `/geraete/{id}`, `/steuerbare-ressourcen/{sr_id}`, `/technische-ressourcen/{tr_id}`, `/lokationszuordnungen` |
| Counterparties & registries | `/api/v1/partners`, `/nb-contracts`, `/grundversorger/{nb_mp_id}`, `/energiemix/{nb_mp_id}`, `/nelos`, `/tranchen`, `/mabis-zp`, `/bilanzierungsgebiete/{eic}/mabis-zp`, `/netzzugang/antraege`, `/msb-rahmenvertraege-gas`, `/correlations` |
| ESA | `/api/v1/esa/einwilligungen`, `/framework`, `/preise`, `/messprodukte`, `/subscriptions/{bestellung_ref}`, `/consent-check` |
| Price sheets | `/api/v1/preisblaetter{,-messung,-ka,-dienstleistung,-hardware}/{mp_id}`, `/mmma-preise/gas`, `/mmm-preise/strom/{year}/{month}`, `/pricat/{nb_mp_id}/…` |
| Fan-out & admin | `/api/v1/subscriptions` (+ `/{id}/test`), inbound at `webhook.inbound_path`, `/admin/fanout/dlq…`, `/admin/events` |

A `404` from `/bilanzierungsgebiete/{eic}/mabis-zp` means *refuse the
submission*, never *substitute the EIC*.

## Configuration

Sections: `[database]`, `[http]`, `[markt] tenant`, `[oidc]`, `[makod]`,
`[webhook]` (`inbound_path`, `inbound_secret`, delivery timeouts),
`[mmma_import]` (Gas and Strom Mehr-/Mindermengenpreise), `[mcp]`.
`dev/config/marktd.toml` is a working example.

The [operator guide](https://hupe1980.github.io/mako/docs/services/marktd/)
covers the supply-status derivation, temporal integrity, the fan-out, every
route and the full configuration reference.
