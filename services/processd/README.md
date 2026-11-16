# processd

`processd` is the process decision engine for NB, LF and MSB: for each
`de.mako.process.initiated` from `marktd`'s fan-out it applies the
Entscheidungsbaum of the Marktrolle that owes the answer and sends the answer to
`makod` as a command — automatically where the tree decides, through an operator
approval queue bounded by the business Frist where it does not. Beside the
decisions it keeps the process read model (per-PID KPIs, the overdue list, the
§ 7a Abs. 5 EnWG Gleichbehandlung evidence) and, in an NB build, the
Sperrauftrag queue that turns ORDERS 17115 / 17117 into field work and IFTSTA
21039. The APERAK is `makod`'s, not this service's.

| | |
|---|---|
| **Port** | 8580 (`dev/ports.toml`) |
| **Database** | PostgreSQL — approval queue, decisions, read model, Sperrauftrag queue |
| **MCP** | 22 tools, 8 prompts at `/mcp` |
| **Emits** | `de.obs.deadline.approaching`, `de.obs.stp.parity.alert`, `de.sperr.*` |
| **Consumes** | `de.mako.*` and `de.markt.versorgung.*` at `POST /webhook` (HMAC) |
| **Depends on** | makod, marktd; einsd and vertragd optional |

Role features (`role-{lf,nb}-{strom,gas}`, `role-msb`; composites `lf-only`,
`nb-only`, `msb-only`, `integrated` — the default) select which decision paths
a binary answers for. Among the NB answers: `E_0622` refuses an Anmeldung whose
Bilanzkreis no Zuordnungsermächtigung covers (Prüfschritt 60 verbrauchend, 260
erzeugend), and a Gas Anmeldung on an assigned Marktlokation first asks the LFA
with UTILMD 44010.

## Run

```bash
just dev processd    # dev/config/processd.toml against the dev PostgreSQL
PROCESSD_CONFIG=dev/config/processd.toml cargo run -p processd -- --check-config
cargo run -p processd --no-default-features --features nb-only   # one role only
```

## API

| Route | Purpose |
|---|---|
| `POST /webhook` | Inbound CloudEvent from `marktd` |
| `GET /api/v1/decisions`, `/api/v1/queue` | Anmeldung STP decisions; approval queue |
| `POST /api/v1/queue/{id}/approve` · `/reject` | Decide a queued answer |
| `POST /api/v1/start-supply` · `-gas`, `/end-supply` · `-gas` | LF Lieferbeginn / Lieferende bootstrap |
| `GET /api/v1/eog`, `/api/v1/neuanlage` | EoG and `E_0608` Neuanlage case logs |
| `GET /obs/processes`, `/obs/kpis`, `/obs/overdue` | Process read model |
| `GET /api/v1/audit/gleichbehandlung` | § 7a Abs. 5 EnWG evidence (`?year=&format=csv`) |
| `/api/v1/sperr-orders` | Sperrauftrag queue (NB build) |

Every route checks a Cedar action in `policies/processd.cedar`; the full table is
in the operator guide's [Routes and authorization](https://hupe1980.github.io/mako/docs/services/processd/#routes-and-authorization).

## Configuration

- `[identity] own_mp_id` (must match `makod`'s primary party) and
  `verbundene_lieferanten` (required, `[]` included).
- `[makod]`, `[marktd]` (`url`, `api_key`), `[webhook] inbound_secret`,
  `[subscription]` (self-registers with `marktd`), `[oidc]`.
- Per role: `[nb]`, `[lf]` (`[[lf.bilanzkreise]]`), `[msb]`, `[esa]`, `[eog]`;
  `[obs]` and `[sperr]` for the outbound notices.

Policy: [`[policy] msb_vorlauf_lesart`](https://hupe1980.github.io/mako/docs/services/processd/#policy-msb-vorlauf-lesart).

The [operator guide](https://hupe1980.github.io/mako/docs/services/processd/)
covers every decision pipeline and its Entscheidungsbaum, the Fristen, the read
model, the Sperrauftrag queue and the full configuration reference.
