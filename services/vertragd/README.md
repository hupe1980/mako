# vertragd

`vertragd` (`:9780`) is the customer registry and retail contract lifecycle
engine for B2C and B2B customers: Kunden, Versorgungs- and Rahmenverträge,
Kündigung, Tarifwechsel and Preisanpassung, and the valid-time assignment of a
MaLo to its product. It is also the authorization gateway that maps a
customer's OIDC identity to the MaLo-IDs they may see (`portald` asks it on
every request).

## Run it

```bash
just dev vertragd      # dev/config/vertragd.toml against the dev PostgreSQL
```

## At a glance

| | |
|---|---|
| **Database** | PostgreSQL (`migrations/0001_schema.sql`) |
| **Statutory rules** | pure functions in `src/domain.rs`, chosen by `vertragsart` (`GRUNDVERSORGUNG` / `ERSATZVERSORGUNG` / `SONDERVERTRAG`) and `haushaltskunde` (§ 3 Nr. 57 EnWG) |
| **Product assignment** | half-open `[gueltig_von, gueltig_bis)` slices, no overlap (`kp_no_overlap`, GiST); `GET /api/v1/malo/{malo_id}/produkte` is what `billingd` bills from |
| **Market side** | Anmeldung and Kündigung reach the market through `processd`; inbound `de.mako.*` and `de.tarif.angebot.angenommen` arrive at `POST /api/v1/events` / `/webhooks/angebot` (HMAC) |
| **MCP** | 17 read-only tools + 4 prompts at `/mcp` |

## The rules

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

A Kündigung or Tarifwechsel that misses its Frist is refused with `422` naming
`fruehestens` and the `rechtsgrundlage`; `GET /vertraege/{id}/kuendigungsfrist`
shows every reachable date first.

## Routes

| Area | Routes |
|---|---|
| Kunden | `/api/v1/kunden` (+ `/{id}`, `/person`, `/zahlungsinformation`, `/identitaeten`, `/portfolio`, `/export`, `/anonymize`), `/kunden/authenticate`, `/kunden/by-sub/{sub}` |
| Verträge | `/api/v1/kunden/{id}/vertraege`, `/api/v1/vertraege` (+ `/{id}`, `/kuendigen`, `/widerruf-kuendigung`, `/stornieren`, `/tarifwechsel`, `/preisgarantie`, `/kuendigungsfrist`, `/expiring`, `/by-malo/{malo_id}`, `/billing-candidates`) |
| Rahmenverträge | `/api/v1/kunden/{id}/rahmenvertraege`, `/api/v1/rahmenvertraege` (+ `/{id}`, `/kuendigen`, `/malos`) |
| Other contracts | `/api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}`, `/aggregatorvertraege`, `/ggv/{ggv_id}/betreiber` |
| Operations | `GET /api/v1/outbound/dead`, `POST /outbound/dead/{id}/retry` |

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

# Drains the customer-facing CloudEvents from the transactional outbox.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:VERTRAGD_ERP_HMAC_SECRET"

# The only authentication on POST /api/v1/events and /api/v1/webhooks/angebot.
inbound_secret  = "env:VERTRAGD_INBOUND_SECRET"

# Renders and delivers the § 41 Abs. 5 EnWG letter; without it only the CloudEvent goes out.
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

DB suite: `just test-vertragd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/vertragd/)
covers the data model, authorization, Kündigung, Tarifwechsel, DSGVO erasure,
background workers and the full configuration reference.
