# marktd

`marktd` is the market data hub for every Marktrolle: the source of truth for
what market messages are *about* — Marktlokationen and Messlokationen, the
supply status, devices, the location graph, price sheets, Referenzreihen,
Zuordnungsermächtigungen, counterparties — and the durable fan-out of every
`makod` and `marktd` event to its subscribers. It holds no domain policy: it
records `makod`'s process events and fans them out; `processd` decides.

| | |
|---|---|
| **Port** | 8180 (`deploy/ports.toml`) |
| **Database** | PostgreSQL 15+ with `btree_gist`; validity is half-open `[valid_from, valid_to)` under `EXCLUDE` constraints |
| **MCP** | 24 tools, 4 prompts at `/mcp` |
| **Emits** | `de.markt.*` (e.g. `versorgung.changed`, `malo.updated`) and relayed `de.mako.*`, HMAC-signed, from `urn:mako:marktd:tenant:{tenant}` |
| **Consumes** | `de.mako.*` from `makod` at `[webhook] inbound_path` (HMAC) |
| **Depends on** | makod |

## Run

```bash
just dev    # the whole estate, marktd on deploy/demo/marktd.toml, seeded
MARKTD_CONFIG=deploy/demo/marktd.toml cargo run -p marktd -- --check-config
```

## API

| Route | Purpose |
|---|---|
| `/api/v1/malos/{id}`, `/api/v1/versorgung/{malo_id}` | Marktlokation and its supply status (`?at=YYYY-MM-DD`) |
| `/api/v1/melos/{id}`, `/zaehler/{zaehler_id}`, `/geraete/{geraet_id}` | Messlokation and devices |
| `/api/v1/partners`, `/nb-contracts`, `/msb-rahmenvertraege` | Counterparties and contracts |
| `GET /api/v1/zuordnungsermaechtigungen/{lf_mp_id}` | Zuordnungsermächtigungen naming the LF (from UTILMD 55071/55072) |
| `GET /api/v1/bilanzierungsgebiete/{eic}/mabis-zp` | MaBiS-Zählpunkt; a `404` means refuse the submission, never substitute the EIC |
| `/api/v1/preisblaetter/{nb_mp_id}`, `-messung`, `-ka`, `-dienstleistung`, `-hardware` | Price sheets |
| `/api/v1/referenzreihen/{reihe}/werte` · `/wert` · `/import` | Published figures billing reads; `422 referenzwert_fehlt` when missing |
| `/api/v1/esa/…` | ESA consent and framework |
| `/api/v1/webhooks/**`, `GET /api/v1/events` | Webhook endpoints (in-cluster receivers as `[[subscriptions.endpoints]]`), deliveries, replay, the event feed |

Every `GET` returns `rubo4e::current` types and every `PUT` passes the BO4E
gate. The full table is the operator guide's [Endpoints](https://hupe1980.github.io/mako/docs/services/marktd/#endpoints);
OpenAPI 3.1 at `/api/v1/openapi.json`, committed as `openapi.json`.

## Configuration

- `[markt] tenant`, `[database] url`.
- `[makod]` (`base_url`, `api_key`), `[webhook] inbound_secret`.
- `[oidc]` — without it and `inbound_secret` marktd refuses to start, unless
  `allow_insecure_no_auth = true`.
- `[mmma_import]` for the automated Mehr-/Mindermengenpreise import.

The [operator guide](https://hupe1980.github.io/mako/docs/services/marktd/)
covers the supply-status derivation, temporal integrity, Referenzreihen, the
fan-out, every route and the full configuration reference.
