# agentd

`agentd` is mako's agent plane: declarative specialists that read mako's
services over MCP to analyse, check compliance and triage deadlines, for the
LF, NB and MSB arms. Each specialist is a manifest run by
[agentplane](https://github.com/hupe1980/agentplane) 0.61, which journals every
model and tool call before it happens, so a run survives a crash and replays
for audit. Specialists are advisory: they file worklist rows and proposals;
`makod` alone dispatches market commands.

| | |
|---|---|
| **Port** | 9580 (`deploy/ports.toml`) |
| **Database** | the journal — `redb` (single instance) or PostgreSQL (`[journal]`) |
| **MCP** | client only: mako's MCP servers publish 54 step-by-step prompts across 11 servers |
| **Specialists** | 30 manifests in [`agents/`](agents/) — 28 `tool-calling`, 2 model-free coded skills |
| **Grants** | read-only: 161 grants across 30 manifests, each named by its procedure |
| **Answers** | All 30 specialists declare a closed `output.schema` |
| **Emits** | `de.mako.agent.decision.made` to `audit_webhook_url` |
| **Consumes** | CloudEvents at `POST /webhook`, routed by each specialist's subscription |
| **Depends on** | the MCP servers in `[mcp_servers]`; a model provider |

## Run

```bash
just dev    # the whole estate, agentd on deploy/demo/agentd.toml, seeded
AGENTD_CONFIG=deploy/demo/agentd.toml cargo run -p agentd -- --check-config
```

## API

| Route | Purpose |
|---|---|
| `POST /webhook` | CloudEvent ingest; admission commits before the answer |
| `POST /api/v1/runs` | start a run (`Idempotency-Key`) |
| `GET /api/v1/runs/{run}/antwort` · `/schritte` | the checked answer; its reads as Server-Sent Events |
| `POST /api/v1/runs/{run}/stopp` · `/bewertung` | stop a run; record a person's verdict |
| `GET /api/v1/hinweise`, `/api/v1/vorschlaege` | the Desk's worklist rows and waiting proposals, each with its Prüfvermerk (processd `rehearse_decision`, makod `rehearse_command`) |
| `GET /api/v1/agents`, `/api/v1/agents/catalog` | what runs here, for the Desk's Beipackzettel |
| `POST /api/v1/erasure` | crypto-shred a case or forget a memory subject (GDPR Art. 17) |
| `/api/v1/oversight/*` | agentplane's worklist, runs, cases and four-eyes decisions (with `[oidc]`) |

`GET /api/v1/openapi.json` serves the OpenAPI 3.1 document of agentd's own
`/api/v1` routes, committed as [`openapi.json`](openapi.json). The
`/api/v1/oversight/*` surface is agentplane's and is described by
[agentplane's own document](https://hupe1980.github.io/agentplane/openapi.json),
its paths relative to that mount.

## Configuration

`[policy]`: [`agent_posture`](https://hupe1980.github.io/mako/docs/services/agentd/#policy-agent-posture).
A production config needs `[oidc]`, `[keyring]` (Vault transit), the HMAC
secrets and every granted server in `[mcp_servers]`; the example below is
parsed and validated by `config::tests::the_readme_example_config_parses`.

```toml
# agentd.toml
# Every top-level key before the first [table]: TOML binds a bare key to the latest header, and every config type denies unknown fields.
tenant           = "stadtwerke-musterstadt"
public_base_url  = "https://agentd.internal:9580"   # what an A2A card advertises
sweep_interval_secs  = 60

mcp_api_key         = "env:AGENTD_MCP_API_KEY"
inbound_hmac_secret = "env:AGENTD_INBOUND_HMAC_SECRET"

audit_webhook_url = "https://erp.example/hooks/agent-decisions"
audit_hmac_secret = "env:AGENTD_AUDIT_HMAC"

[quota]

[journal]
backend = "redb"
path    = "/var/lib/agentd/journal.redb"

[providers.anthropic]
backend = "anthropic"
api_key = "env:ANTHROPIC_API_KEY"

[bundled_agents]
enable_all = true

[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "agentd"

[keyring]
required = true
[keyring.vault]
address = "https://vault.internal:8200"
mount   = "transit"
token   = "env:VAULT_TOKEN"

[policy]

[quellen]
# The passage index `cargo xtask index-sources` builds; quote_source and find_source read it.
index = "/var/lib/agentd/quellen-index.json"

[mcp_servers]
makod       = "http://makod:8080/mcp"
marktd      = "http://marktd:8180/mcp"
processd    = "http://processd:8580/mcp"
billingd    = "http://billingd:9280/mcp"
invoicd     = "http://invoicd:8280/mcp"
accountingd = "http://accountingd:9380/mcp"
edmd        = "http://edmd:8380/mcp"
einsd       = "http://einsd:9180/mcp"
netzbilanzd = "http://netzbilanzd:8680/mcp"
productd    = "http://productd:9080/mcp"
vertragd    = "http://vertragd:9780/mcp"
```

The [operator guide](https://hupe1980.github.io/mako/docs/services/agentd/)
covers the specialists and their subscriptions, role builds, oversight, the
trust boundary, erasure, the journal's evidence chain and the full
configuration reference.
