# agentd — Multi-Agent LLM Orchestration

`agentd` connects large language models to all 17 mako production services via MCP,
enabling automated analysis, compliance checking, and workflow orchestration.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9580` |
| **Built-in agents** | 29 specialists compiled into binary — ship in container image |
| **Custom agents** | `[[agents]]` in `agentd.toml` — fully customizable |
| **LLM providers** | OpenAI · Anthropic Claude · AWS Bedrock SigV4 |
| **Dispatch modes** | `sequential` · `parallel` (fan-out) · `race` (first wins) |
| **RAG** | LanceDB vector store (S3/GCS/Azure Blob/local) |
| **WASM plugins** | Custom extensions via `mako-plugin` (Extism sandbox) |
| **A2A cards** | `GET /.well-known/agents/{name}` for each specialist |
| **Catalog** | `GET /api/v1/agents/catalog` — all 27 built-in definitions |

## Built-in specialists (shipped in container)

All 29 specialists are compiled into the binary. Activate them via `[bundled_agents]`:

```toml
[bundled_agents]
enable_all       = true
default_provider = "openai"
default_model    = "gpt-4o-mini"

# Upgrade specific agents
[bundled_agents.overrides.mako-agent]
model = "gpt-4o"
```

| Specialist | Trigger events | MCP tools used |
|---|---|---|
| `mako-agent` | `de.mako.process.escalated`, `de.mako.aperak.*` | makod, marktd, obsd |
| `deadline-alert-agent` | `de.mako.process.timedout` | obsd, makod |
| `billing-agent` | `de.invoic.receipt.disputed` | invoicd, billingd, accountingd |
| `billing-anomaly-agent` | `de.billing.rechnung.erstellt` | billingd, edmd |
| `billing-regulatory-guard-agent` | `de.billing.rechnung.erstellt` | billingd, marktd |
| `jahresabrechnung-agent` | manual | billingd, edmd, marktd |
| `eeg-compliance-agent` | `de.eeg.anlage.*`, `de.eeg.verguetung.*` | einsd, obsd |
| `eeg-agent` | `de.eeg.anlage.foerderung_auslaufend` | einsd, edmd |
| `payment-reconciliation-agent` | `de.accounting.payment.due` | accountingd |
| `compliance-agent` | `de.obs.stp.parity.alert` | obsd, processd |
| `msb-history-agent` | `de.edmd.reading.quality.warning` | edmd, makod, marktd |
| `meter-data-agent` | `de.edmd.reading.quality.warning` | edmd, marktd |
| `grid-anomaly-agent` | `de.markt.grid.drift.detected` | marktd, obsd |
| `tariff-optimization-agent` | `de.billing.rechnung.erstellt` | billingd, tarifbd, edmd |
| `replacement-value-agent` | `de.edmd.reading.quality.warning` | edmd, marktd, obsd |
| `mabis-syncd-agent` | `de.edmd.reading.quality.warning` | edmd, obsd, marktd |
| `smgw-diagnostics-agent` | `de.edmd.reading.direct.stored` | edmd, marktd, processd |
| `invoice-reconciliation-agent` | `de.invoic.receipt.*` | invoicd, billingd |
| `netzbilanz-agent` | `de.netzbilanz.invoic.*` | netzbilanzd, marktd |
| `nis-syncd-agent` | `de.markt.grid.drift.detected` | marktd, processd |
| `portald-agent` | `de.vertrag.*` | portald, vertragd, billingd |
| `processd-agent` | `de.mako.process.escalated` | processd, obsd, marktd |
| `regulatory-reporting-agent` | manual | obsd, marktd, processd |
| `sperrd-agent` | `de.accounting.sperrauftrag` | sperrd, accountingd |
| `tarifbd-agent` | `de.tarifbd.*` | tarifbd, billingd |
| `vertragd-agent` | `de.vertrag.*` | vertragd, processd, marktd |
| `vpp-billing-agent` | `de.vpp.dispatch.confirmed`, `de.vpp.settlement.berechnet` | billingd, marktd, obsd |
| `gabi-gas-agent` | `de.gabi.imbalance.*`, `de.gabi.alocat.missing`, `de.gabi.nomination.*` | makod, netzbilanzd, marktd, obsd |
| `einsd-batch-agent` | `de.eeg.settlement.batch_due`, `de.eeg.compliance.*` | einsd, edmd, tarifbd, obsd |

## Configuration

```toml
# agentd.toml
tenant = "9900357000004"

[providers.openai]
backend = "openai"
api_key = "env:OPENAI_API_KEY"   # SecretString — never logged

[orchestrator]
provider = "openai"
model    = "gpt-4o"

[bundled_agents]
enable_all       = true
default_provider = "openai"
default_model    = "gpt-4o-mini"

# OIDC (optional — dev mode when absent, all POST /api/v1/run requests accepted)
[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "agentd"

# Inbound HMAC verification (strongly recommended in production)
inbound_hmac_secret = "env:AGENTD_INBOUND_HMAC_SECRET"

# Dead-letter queue (retries failed sessions with exponential backoff)
[dlq]
capacity         = 100
max_retries      = 4
base_backoff_secs = 30   # retry delays: 30s, 90s, 270s, 810s

mcp_api_key = "env:AGENTD_MCP_API_KEY"   # SecretString — never logged

# Keys are free-form names; values are MCP endpoints. Must come last: any
# key after a table header belongs to that table.
[mcp_servers]
makod    = "http://makod:8080/mcp"
marktd   = "http://marktd:8180/mcp"
billingd = "http://billingd:9280/mcp"
# ... more services

[rag]
enabled           = true
storage_uri       = "/var/lib/agentd/rag"
embedding_provider = "openai"
embedding_model   = "text-embedding-3-small"
score_threshold   = 0.3   # min cosine similarity (filters low-quality results)
top_k             = 5
```

## Research basis

The Orchestrator → Specialist Mesh pattern is proven at scale (Guo et al. 2024 multi-agent
survey) and aligns with:

- **LangGraph** supervisor pattern — orchestrator routes to subagent graphs
- **AutoGen** GroupChat — specialists communicate via shared context
- **CrewAI** hierarchical process — orchestrator assigns tasks to specialists
- **A2A protocol** — each specialist exposes a discoverable Agent Card

Key design choices vs alternatives:
- **ReAct over CoT** — interleaved tool calls reduce hallucination for factual energy-domain tasks
- **Structured output format** — every specialist ends with a machine-parseable block
  (STATUS/OUTCOME/FINDINGS), not prose, enabling downstream automation
- **Domain specialization** — narrow prompts outperform general-purpose agents on EDIFACT/§-law tasks
- **Parallel dispatch** — compliance events (§40/§41/§41b) benefit from simultaneous multi-specialist checks
