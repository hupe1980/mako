# processd

`processd` is the process decision engine for NB, LF and MSB: for each
`de.mako.engine.process.initiated` from `marktd`'s fan-out it applies the
Entscheidungsbaum of the Marktrolle that owes the answer and sends the answer to
`makod` as a command — automatically where the tree decides, through an operator
approval queue bounded by the business Frist where it does not. Beside the
decisions it keeps the process read model (per-PID KPIs, the overdue list, the
§ 7a Abs. 5 EnWG Gleichbehandlung evidence) and, in an NB build, the
Sperrauftrag queue that turns ORDERS 17115 / 17117 into field work and IFTSTA
21039. The APERAK is `makod`'s, not this service's.

| | |
|---|---|
| **Port** | 8580 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — approval queue, decisions, read model, Sperrauftrag queue |
| **MCP** | 29 tools, 8 prompts at `/mcp` |
| **Emits** | `de.mako.obs.deadline.approaching`, `de.mako.obs.stp.parity.alert`, `de.mako.sperr.*` |
| **Consumes** | `de.mako.engine.*` and `de.mako.markt.versorgung.*` at `POST /webhook` (HMAC) |
| **Depends on** | makod, marktd; einsd and vertragd optional |

Role features (`role-{lf,nb}-{strom,gas}`, `role-msb`; composites `lf-only`,
`nb-only`, `msb-only`, `integrated` — the default) select which decision paths
a binary answers for. Among the NB answers: `E_0622` refuses an Anmeldung whose
Bilanzkreis no Zuordnungsermächtigung covers (Prüfschritt 60 verbrauchend, 260
erzeugend), and a Gas Anmeldung on an assigned Marktlokation first asks the LFA
with UTILMD 44010.

## Run

```bash
just dev    # the whole estate, processd on deploy/demo/processd.toml, seeded
PROCESSD_CONFIG=deploy/demo/processd.toml cargo run -p processd -- --check-config
cargo run -p processd --no-default-features --features nb-only   # one role only
```

## API

| Route | Purpose |
|---|---|
| `POST /webhook` | Inbound CloudEvent from `marktd` |
| `GET /api/v1/decisions`, `/api/v1/queue` | Anmeldung STP decisions; approval queue |
| `GET /api/v1/klaerfaelle` · `/{id}` | Open Klärfälle: cause, action, correction and due date from code tables; one with its history |
| `POST /api/v1/klaerfaelle/{id}/assign` · `/resolve` · `/dispatch` | Take, close or „korrigieren & senden" a Klärfall, journaled against the principal |
| `POST /api/v1/queue/{id}/approve` · `/reject` | Decide a queued answer (`application/json`; `body_sha256` → `412` when stale); makod's pre-send check runs first — `422 pruefung_abgelehnt` / `nicht_geprueft`, nothing marked |
| `POST /api/v1/start-supply` · `-gas`, `/end-supply` · `-gas` | LF Lieferbeginn / Lieferende bootstrap |
| `GET /api/v1/eog`, `/api/v1/neuanlage` | EoG and `E_0608` Neuanlage case logs |
| `GET /obs/processes`, `/obs/kpis`, `/obs/overdue` | Process read model |
| `GET /api/v1/audit/gleichbehandlung` | § 7a Abs. 5 EnWG evidence (`?year=&format=csv`) |
| `GET /api/v1/counterparties`, `/{mp_id}/{rolle}` | Gegenpartei-Akte: how a counterparty answered what it owed (`?von=&bis=&format=csv`) |
| `/api/v1/sperr-orders` | Sperrauftrag queue (NB build) |
| `/api/v1/webhooks/endpoints` · `/api/v1/events` | Webhook endpoints, deliveries and replay for the emitted CloudEvents; the outbox as a feed |
| `GET /api/v1/openapi.json` | The OpenAPI 3.1 document of every `/api/v1` route, committed as `openapi.json` |

Every route checks a Cedar action in `policies/processd.cedar`; the full table is
in the operator guide's [Routes and authorization](https://hupe1980.github.io/mako/docs/services/processd/#routes-and-authorization).

## Configuration

- `[identity]`: `tenant` (required, the data-isolation key), one MP-ID list
  per Marktrolle — `nb_mp_ids`, `lf_mp_ids`, `msb_mp_ids`, one code per Sparte
  each, matching `makod`'s `[[party]]` entries — and `verbundene_lieferanten`
  (required, `[]` included; the own `lf_mp_ids` are affiliated as well). Each module answers only under its own role's
  codes.
- `[makod]`, `[marktd]` (`url`, `api_key`), `[webhook] inbound_secret` — the
  secret of processd's entry in marktd's `[[subscriptions.endpoints]]`, which
  is how marktd delivers to it — `[oidc]`.
- Per role: `[nb]`, `[lf]` (`[[lf.bilanzkreise]]`), `[msb]`, `[esa]`, `[eog]`;
  `[obs]` and `[sperr]` for the outbound notices — `[sperr] erp_webhook_url` is
  the webhook endpoint `default`; `[subscriptions]` (`retention_days`,
  `allow_loopback`) governs the ones registered over the API.

Policy: [`[policy] msb_vorlauf_lesart`](https://hupe1980.github.io/mako/docs/services/processd/#policy-msb-vorlauf-lesart).

The [operator guide](https://hupe1980.github.io/mako/docs/services/processd/)
covers every decision pipeline and its Entscheidungsbaum, the Fristen, the read
model, the Sperrauftrag queue and the full configuration reference.
