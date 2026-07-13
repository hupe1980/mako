# vertragd — Contract & Customer Management

`vertragd` is the **customer registry and retail contract lifecycle engine** for both
B2C (private households) and B2B (commercial, RLM) customers. It is the single source
of truth for contract state and the authorization gateway between OIDC identities and
MaLo IDs.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9780` |
| **Database** | PostgreSQL (sqlx 0.8, dynamic queries) |
| **Auth** | OIDC/JWT + Cedar ABAC |
| **Kunden** | B2C persons + B2B companies; `Geschaeftspartner` schema-validated on PUT |
| **B2C Person** | `PUT/GET /api/v1/kunden/{id}/person` — `rubo4e::current::Person` BO (GDPR Art. 15) |
| **B2B portal access** | `kunden_identitaeten` — N OIDC logins per company; role-based + site-scoped (`standort_filter`) |
| **Rahmenverträge** | B2B framework contracts with Sammelrechnung, indexation, `angebot_id` (CPQ) |
| **Versorgungsverträge** | Per-site/commodity; status machine ANGELEGT→AKTIV→ABGELAUFEN |
| **MaKo triggering** | `POST processd /start-supply` per commodity on contract creation |
| **Tarifwechsel** | `POST /api/v1/vertraege/{id}/tarifwechsel` — changes product without new UTILMD; **blocked within `preisgarantie_bis` window** |
| **Preisgarantie** | `PUT/GET /api/v1/vertraege/{id}/preisgarantie` — typed `rubo4e::current::Preisgarantie` COM; guard prevents price-lock violations |
| **Kündigung** | Coordinated Lieferende + Schlussablesung across all commodities |
| **OIDC→MaLo auth** | `GET /kunden/authenticate?malo_id=` — used by `portald` to scope all portal requests |
| **Preisanpassungsbenachrichtigung** | Background worker emits `de.vertrag.preisaenderung.ankuendigung` 42 days before Tarifwechsel (§41 Abs. 3 EnWG ≥ 6 weeks notice) |
| **Health** | `GET /health/live`, `GET /health/ready` |

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

# Operator bypass (requires documented customer consent)
curl -X POST http://vertragd:9780/api/v1/vertraege/{id}/tarifwechsel \
  -d '{"komp_id":"...","new_product_code":"STROM-PREMIUM-2027","wirksamkeit":"2026-08-01","override_preisgarantie":true}'
```

## Configuration

```toml
# vertragd.toml
database_url   = "postgresql://vertragd:secret@db:5432/vertragd"
port           = 9780
tenant         = "9900357000004"   # LF BDEW-Codenummer
lf_mp_id       = "9900357000004"
processd_url   = "http://processd:8580"
tarifbd_url    = "http://tarifbd:9080"
accountingd_url = "http://accountingd:9380"
edmd_url       = "http://edmd:8380"

[erp]
webhook_url  = "http://erp:8000/events"
hmac_secret  = "${ERP_HMAC_SECRET}"
```
