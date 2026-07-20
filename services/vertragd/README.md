# vertragd — Contract & Customer Management

`vertragd` is the **customer registry and retail contract lifecycle engine** for both
B2C (private households) and B2B (commercial, RLM) customers. It is the single source
of truth for contract state and the authorization gateway between OIDC identities and
MaLo IDs.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9780` |
| **Database** | PostgreSQL (3 migrations, sqlx 0.8 dynamic queries) |
| **Auth** | OIDC/JWT + Cedar ABAC (MCP: API-key bearer) |
| **Kunden** | B2C persons + B2B companies; `Geschaeftspartner` schema-validated on PUT |
| **B2C Person** | `PUT/GET /api/v1/kunden/{id}/person` — `rubo4e::current::Person` BO (GDPR Art. 15) |
| **Zahlungsinformation** | `PUT/GET /api/v1/kunden/{id}/zahlungsinformation` — typed `Zahlungsinformation` COM; IBAN mod-97 validated |
| **GDPR Art. 17** | `POST /api/v1/kunden/{id}/anonymize` — irreversible pseudonymization; immutable `anonymization_log` audit trail |
| **GDPR Art. 15/20** | `GET /api/v1/kunden/{id}/export` — complete PII export (Kunde + Person + IBAN + identities + contracts) |
| **B2B portal access** | `kunden_identitaeten` — N OIDC logins per company; role-based + site-scoped (`standort_filter`) |
| **Rahmenverträge** | B2B framework contracts with Sammelrechnung, indexation, `angebot_id` (CPQ) |
| **Versorgungsverträge** | Per-site/commodity; status machine ANGELEGT→AKTIV→ABGELAUFEN; idempotent on `erp_contract_id` |
| **MaKo triggering** | `POST processd /start-supply` per commodity on contract creation; **3× exponential-backoff retry** (10s, 20s, 40s) — failures detected by `find_stuck_komponents` after 5 WT |
| **Tarifwechsel** | `POST /api/v1/vertraege/{id}/tarifwechsel` — changes product without new UTILMD; **blocked within `preisgarantie_bis` window**; override logged to `preisgarantie_override_log` with operator JWT sub |
| **Preisgarantie** | `PUT/GET /api/v1/vertraege/{id}/preisgarantie` — typed `rubo4e::current::Preisgarantie` COM |
| **Kündigung** | Coordinated Lieferende + Schlussablesung across all commodities; §14 StromGVV / §13 GasGVV notice period enforced |
| **Kündigung Widerruf** | `POST /api/v1/vertraege/{id}/widerruf-kuendigung` — reverts GEKÜNDIGT → AKTIV before lieferende; §20 EnWG LF right to withdraw |
| **Rahmenvertrag Cascade** | `POST /api/v1/rahmenvertraege/{id}/kuendigen` — terminates all active child Versorgungsverträge; individual notice periods respected; returns dispatched/skipped summary |
| **Stornierung** | `POST /api/v1/vertraege/{id}/stornieren` — pre-activation cancel (ANGELEGT/IN_BEARBEITUNG only) |
| **OIDC→MaLo auth** | `GET /kunden/authenticate?malo_id=` — used by `portald` to scope all portal requests; updates `letzter_login` on every successful check |
| **Contract-by-MaLo** | `GET /api/v1/vertraege/by-malo/{malo_id}` — the active Versorgungsvertrag behind a MaLo, plus the next possible Kündigungstermin (incl. §309 Nr. 9 BGB one-month cap after auto-renewal); `billingd` sources the §40 Abs. 1 EnWG invoice facts here |
| **Preisanpassungsbenachrichtigung** | Daily worker emits `de.vertrag.preisaenderung.ankuendigung` 42 days before Tarifwechsel (§41 Abs. 3 EnWG ≥ 6 weeks notice) |
| **Auto-renewal** | Daily worker extends `vertragsende` + emits 30-day advance notice (§13 GasGVV / §14 StromGVV) |
| **Expiry notifications** | Daily worker emits `de.vertrag.ablauf.ankuendigung` 30 days before `vertragsende` or `preisgarantie_bis` expiry (§41 EnWG) |
| **CPQ pipeline** | `POST /api/v1/webhooks/angebot` — `de.angebot.angenommen` → auto-creates Rahmenvertrag with `angebot_id` traceability + N Versorgungsverträge |
| **Max identities** | `max_identitaeten_per_kunde = 50` (configurable) — prevents resource exhaustion from unbounded portal user creation |
| **Health** | `GET /health/live`, `GET /health/ready` |
| **MCP** | **16 read-only tools + 4 prompts** at `/mcp` (incl. GDPR erasure workflow, Preisgarantie dispute resolution) |

## Tarifwechsel + Preisgarantie

```bash
# Store a price guarantee (will block Tarifwechsel until 2027-06-30)
curl -X PUT http://vertragd:9780/api/v1/vertraege/{id}/preisgarantie \
  -H "Content-Type: application/json" \
  -d '{"_typ":"PREISGARANTIE","preisgarantietyp":"ALLE_PREISBESTANDTEILE",
       "zeitlicheGueltigkeit":{"_typ":"ZEITRAUM","startdatum":"2025-01-01","enddatum":"2027-06-30"}}'

# Tarifwechsel before 2027-07-01 → HTTP 422 (blocked by Preisgarantie)
curl -X POST http://vertragd:9780/api/v1/vertraege/{id}/tarifwechsel \
  -d '{"komp_id":"...","new_product_code":"STROM-PREMIUM-2027","wirksamkeit":"2026-08-01"}'
# → 422 {"error":"Tarifwechsel blocked by Preisgarantie","preisgarantie_bis":"2027-06-30",...}

# Operator bypass (requires documented customer consent; logged to preisgarantie_override_log)
curl -X POST http://vertragd:9780/api/v1/vertraege/{id}/tarifwechsel \
  -d '{"komp_id":"...","new_product_code":"STROM-PREMIUM-2027","wirksamkeit":"2026-08-01","override_preisgarantie":true}'
```

## GDPR erasure

```bash
# Pseudonymize all PII (irreversible; retains contract records for §147 AO)
curl -X POST http://vertragd:9780/api/v1/kunden/{id}/anonymize \
  -H "Content-Type: application/json" \
  -d '{"requested_by": "operator-dpo"}'
# → 200 {"anonymized": true, "audit_log": "anonymization_log Eintrag erstellt"}
```

## Kündigung Widerruf

```bash
# Revoke a pending Kündigung before lieferende (GPKE §20 EnWG)
curl -X POST http://vertragd:9780/api/v1/vertraege/{id}/widerruf-kuendigung
# → 200 {"vertrag_id": "...", "status": "AKTIV", "message": "Kündigung widerrufen"}
# Note: cancel in-flight Lieferende UTILMD via processd separately

# Cascade Kündigung for a B2B framework contract (all sites)
curl -X POST http://vertragd:9780/api/v1/rahmenvertraege/{id}/kuendigen \
  -H "Content-Type: application/json" \
  -d '{"lieferende": "2026-12-31"}'
# → 202 {"dispatched": 8, "skipped": 1, "skipped_details": [...]}
```

## Database migrations

| Migration | Contents |
|---|---|
| `0001_initial.sql` | `kunden`, `kunden_identitaeten`, `rahmenvertraege`, `versorgungsvertraege`, `vertragskomponenten`, `received_events`; `person` column; pending Tarifwechsel columns |
| `0002_zahlungsinformation.sql` | `kunden.zahlungsinformation JSONB` for IBAN/SEPA |
| `0003_correctness_gdpr.sql` | Unique partial index for `upsert_kunde` idempotency; `anonymization_log`; `preisgarantie_override_log` |

## Configuration

```toml
# vertragd.toml
database_url   = "postgresql://vertragd:secret@db:5432/vertragd"
port           = 9780
tenant         = "9900357000004"   # data-isolation key (operator tenant; value = BDEW-Codenummer in this example)
lf_mp_id       = "9900357000004"
processd_url   = "http://processd:8580"
tarifbd_url    = "http://tarifbd:9080"
accountingd_url = "http://accountingd:9380"
edmd_url       = "http://edmd:8380"

# Outbound ERP CloudEvents. `erp_hmac_secret` puts an HMAC-SHA256
# X-Mako-Signature on every event.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:VERTRAGD_ERP_HMAC_SECRET"
```
