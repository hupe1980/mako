# portald — Customer Portal Gateway (LF role)

Stateless REST gateway that aggregates the LF back-end services into one
customer-facing API. No database, no cache, no session store: every response is
assembled from the authoritative services on the request path.

| | |
|---|---|
| **Port** | `:9480` |
| **Authorization** | every route resolves `(customer token, malo_id)` through `vertragd` |
| **State** | none — scale replicas freely |

## Authorization

`portald` verifies no tokens and holds no customer↔MaLo map. It forwards the
customer's `Authorization: Bearer` header to
`vertragd GET /api/v1/kunden/authenticate?malo_id=…` and relays the verdict;
`vertragd` owns the OIDC verifier and the customer record.

Every handler takes the resulting `PortalAuthCtx` by value, so a route cannot
serve customer data without having asked. `tests/authorization_guard.rs` drives
all 15 routes against a refusing `vertragd` and fails if any answers.

Starting without `vertragd_url` is refused unless `allow_insecure_no_auth = true`
is set explicitly.

## Endpoints

All paths are prefixed `/api/v1/portal/{malo_id}`.

| Method | Path | Upstream |
|---|---|---|
| `GET` | `/dashboard` | all five, concurrently; a field is `null` when its upstream has no data |
| `GET` | `/lastgang?from=&to=` | `edmd` |
| `GET` | `/invoices?limit=&outcome=` | `billingd` |
| `GET` | `/invoices/{record_id}/download` | `billingd` — XRechnung 3.0 CII XML (EN 16931) |
| `GET` | `/dokumente?kind=&limit=` | `outputd` — the document inbox: what was issued and sent |
| `GET` | `/dokumente/{document_id}` | `outputd` — the bytes as issued; opening it records the portal read receipt |
| `GET` | `/balance` · `/kontoauszug` · `/vorauszahlung` | `accountingd` |
| `GET` | `/eeg` | `einsd` |
| `GET` | `/versorgung` | `marktd` |
| `GET` | `/vertrag` · `/kuendigungsfrist` | `vertragd` |
| `POST` | `/tarifwechsel` · `/kuendigen` | `vertragd` |
| `PUT` | `/kontakt` | `vertragd` (GDPR Art. 16) |
| `PUT` | `/sepa` | `accountingd` |

`/health/live`, `/health/ready` and `/metrics` come from the service runner.

The invoice download re-reads the record and compares its `malo_id` to the
authorised one before rendering — authorising the path parameter alone would let
any customer stream any invoice by id.

### Notice periods and tariff rules live in `vertragd`

`portald` validates date *format* only. Whether a `lieferende` or `wirksamkeit`
is reachable depends on the Vertragsart, whether the customer is a Haushaltskunde
and the termination reason (§ 20 Abs. 1 StromGVV/GasGVV, § 41b Abs. 5 EnWG,
§ 41 Abs. 5 Satz 4 EnWG, § 309 Nr. 9 lit. c BGB) — all facts `vertragd` holds.
Its 422 carries the rule it applied and is relayed unchanged; call
`GET /kuendigungsfrist` to show the customer the reachable dates first.

## Configuration

```toml
# portald.toml
port   = 9480
tenant = "9900357000004"

# Required — the authorization authority.
vertragd_url     = "http://vertragd:9780"
vertragd_api_key = "env:PORTALD_VERTRAGD_SERVICE_KEY"   # sent as X-Api-Key

edmd_url        = "http://edmd:8380"
billingd_url    = "http://billingd:9280"
accountingd_url = "http://accountingd:9380"
einsd_url       = "http://einsd:9180"
marktd_url      = "http://marktd:8180"
# …_api_key = "env:…"  — opaque service Bearer tokens

[mcp]
api_key = "env:PORTALD_MCP_API_KEY"
```

## MCP server

`/mcp` (Streamable HTTP), 8 read-only tools —  `get_dashboard`, `get_lastgang`,
`get_invoices`, `get_balance`, `get_kontoauszug`, `get_vorauszahlung`,
`get_eeg_status`, `get_versorgung` — and 3 guided prompts (`customer-overview`,
`billing-dispute`, `eeg-foerderung-check`).

**Operator-facing.** The tools take a `malo_id` and carry no customer token, so
whoever can call `/mcp` can read every customer in the tenant. Gate it with
`[mcp]` and keep it off the public ingress.

## Information separation (§§ 6a, 7a EnWG)

An LF-role service. It reads `marktd` only for VersorgungsStatus — the LF's own
supply records — never NB grid topology or NB billing data. `netzbilanzd` and
`sperrd` are not reachable through it.
