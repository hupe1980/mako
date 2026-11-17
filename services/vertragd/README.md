# vertragd

`vertragd` is the LF's customer registry and retail contract engine for B2C and
B2B customers: Kunden, Versorgungs- and Rahmenverträge, online Vertragsschluss
and Widerruf, Kündigung, Tarifwechsel and Preisanpassung, and the valid-time
assignment of a MaLo to its product and § 14a EnWG modules. It is also the
gateway that maps a customer's OIDC identity to the MaLo-IDs they may see. It
speaks no EDIFACT: Anmeldung and Kündigung reach the market through `processd`.

| | |
|---|---|
| **Port** | 9780 (`deploy/ports.toml`) |
| **Database** | PostgreSQL, `public` schema (`migrations/0001_schema.sql`) |
| **MCP** | 22 tools, 4 prompts at `/mcp` |
| **Emits** | `de.vertrag.*` (contract lifecycle, Bestellung, Widerruf) through the transactional outbox, to every matching webhook endpoint |
| **Consumes** | `de.mako.*` process outcomes at `POST /api/v1/events`; `de.tarif.angebot.angenommen` at `POST /api/v1/webhooks/angebot` |
| **Depends on** | processd, accountingd, edmd, outputd, productd |

## Run

```bash
just dev    # the whole estate, vertragd on deploy/demo/vertragd.toml, seeded
VERTRAGD_CONFIG=deploy/demo/vertragd.toml cargo run -p vertragd -- --check-config
just test-vertragd-db
```

## API

| Route | Purpose |
|---|---|
| `/api/v1/kunden` (+ `/{id}`, `/person`, `/zahlungsinformation`, `/identitaeten`, `/portfolio`, `/export`, `/anonymize`) | Kunden and their DSGVO export / erasure |
| `POST /api/v1/kunden/authenticate`, `GET /kunden/by-sub/{sub}` | OIDC identity → Kunde → MaLo-IDs (asked by `portald`) |
| `/api/v1/kunden/{id}/vertraege`, `/api/v1/vertraege/{id}` (+ `/kuendigen`, `/widerruf-kuendigung`, `/stornieren`, `/tarifwechsel`, `/preisgarantie`, `/kuendigungsfrist`) | Versorgungsverträge |
| `/api/v1/kunden/{id}/rahmenvertraege`, `/api/v1/rahmenvertraege/{id}` (+ `/kuendigen`, `/malos`) | Rahmenverträge |
| `PUT\|GET /api/v1/rechtsdokumente/{key}` | Rechtsdokumente — `PUT` by `art`, `GET` by SHA-256 |
| `POST /api/v1/bestellungen` (+ `/{id}/annehmen`, `/ablehnen`, `/nachweis`, `/widerruf`) | Online order with its Bestellnachweis (§ 312j Abs. 4 BGB), `angebot_ref` and `Idempotency-Key` |
| `POST /api/v1/vertraege/{id}/widerruf` | Widerruf of an online contract |
| `GET /api/v1/malo/{malo_id}/produkte`, `GET\|POST /malo/{malo_id}/sect14a` | Product and § 14a module slices `billingd` bills from |
| `/api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}`, `/aggregatorvertraege`, `/ggv/{ggv_id}/betreiber` | Other customer contracts |
| `/api/v1/webhooks/endpoints` (+ `/{id}`, `/rotate-secret`, `/test`, `/deliveries`, `/replay`), `GET /api/v1/events` | Webhook subscriptions and the event feed |
| `GET /api/v1/openapi.json` | The OpenAPI document, committed as `openapi.json` |

A Kündigung or Tarifwechsel that misses its Frist is refused `422` with
`fruehestens` and the `rechtsgrundlage`.

## Configuration

`tenant`, `lf_mp_id`, `processd_url`, `accountingd_url`, `edmd_url` (each with
an `_api_key`), `inbound_secret` (the only authentication on the two webhook
routes), `erp_webhook_url` / `erp_hmac_secret` (the webhook endpoint `default`),
`[subscriptions] retention_days`, `outputd_url`, `[absender]
name` (the § 126b BGB declarant), `[database]`, `[oidc]`. `productd_url`,
`outputd_url` and `[absender] name` are required once `[policy]
vertragsschluss` is set.

`[policy]`:
[`vertragsschluss`](https://hupe1980.github.io/mako/docs/services/vertragd/#policy-vertragsschluss)
(`mit_bestellung` / `mit_annahme`, no default — unset, `POST
/api/v1/bestellungen` refuses `409 policy_nicht_gesetzt`),
`lieferbeginn_in_widerrufsfrist` and `bestellbutton_beschriftungen` beside it,
and [`dsr`](https://hupe1980.github.io/mako/docs/services/vertragd/#policy-dsr).
All are listed in the
[operator policy reference](https://hupe1980.github.io/mako/docs/reference/operator-policy/).

The [operator guide](https://hupe1980.github.io/mako/docs/services/vertragd/)
covers the statutory rules, data model, authorization, online Vertragsschluss
and Widerruf, Kündigung, Tarifwechsel, DSGVO, events and the full configuration.
