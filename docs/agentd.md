---
layout: default
title: agentd Operator Guide
nav_order: 37
parent: Services
mermaid: true
description: >
  agentd operator guide: Multi-agent LLM orchestration daemon.
  27 built-in specialists ship in container image. Orchestrator + Specialist Mesh,
  LanceDB RAG, parallel dispatch, A2A agent cards, OpenAI / Anthropic / AWS Bedrock.
---

# `agentd` — Multi-Agent LLM Orchestration

`agentd` is the **AI automation layer** for the mako platform. It connects large
language models to all 17 production services via MCP, enabling automated analysis,
decision support, and workflow orchestration.

Port: **`:9580`**

| Endpoint | Description |
|---|---|
| `POST /webhook` | Inbound CloudEvent trigger |
| `POST /api/v1/run` | Manual agent invocation |
| `GET /api/v1/sessions` | Last 100 agent decisions (in-memory ring buffer) |
| `GET /api/v1/agents` | List all active agents (built-in + custom) |
| `GET /api/v1/agents/catalog` | Full catalog of all 27 built-in definitions |
| `GET /.well-known/agents/{name}` | A2A Agent Card for a specialist |
| `POST /api/v1/rag/ingest` | Index a live text document into LanceDB |
| `POST /api/v1/rag/search` | Query the RAG knowledge base directly |
| `GET /health` · `GET /health/ready` | Liveness / readiness |

---

## Key design decisions

### 27 built-in specialists ship in the container image

The most important architectural change from the naive "put prompts in demo config" approach:
all 27 specialist system prompts are **compiled into the agentd binary** and ship in the
container image. Operators activate them via `[bundled_agents]` in `agentd.toml` without
copying hundreds of lines of system prompts.

This follows the same principle as `makod`'s compiled-in AHB profiles — domain knowledge
lives in the binary, not in operator-managed config files.

### A2A Protocol compliance

Each specialist exposes an [A2A Agent Card](https://a2a-protocol.org/) at
`/.well-known/agents/{name}` — a standards-based capability declaration that enables
external systems to discover and interact with mako specialists without prior configuration.

### Parallel dispatch (new)

The orchestrator supports three dispatch modes:

| Mode | Behaviour | Best for |
|---|---|---|
| `sequential` (default) | Route to one specialist | Clear single-domain events |
| `parallel` | Fan out to ALL matching specialists concurrently | Compliance events needing multiple checks |
| `race` | Fan out; return first specialist to complete | Latency-sensitive events |

---

## Architecture

```mermaid
graph TB
    TRIGGER["Trigger\nCloudEvent webhook\nor POST /api/v1/run"]

    subgraph orchestrator ["Orchestrator Agent"]
        ORCH["1. Direct match (trigger_patterns)\n2. LLM triage (tool call)\n3. Fallback: orchestrator handles directly"]
        MODE["DispatchMode:\nsequential | parallel | race"]
    end

    subgraph builtin ["26 Built-in Specialists (compiled into binary)"]
        direction LR
        MAKO["mako-agent\nEDIFACT · UTILMD · deadlines"]
        BILLING["billing-agent\nbillingd · invoicd · O2C"]
        BILREG["billing-regulatory-guard-agent\n§40/§41/§41b/§42 compliance"]
        JAHRB["jahresabrechnung-agent\nAnnual settlement orchestrator"]
        ANOMALY["billing-anomaly-agent\n20% deviation check"]
        EEG["eeg-agent + eeg-compliance-agent\neinsd · §52 · §44b · §20 EEG"]
        MORE["... 20 more specialists"]
    end

    subgraph rag ["RAG Knowledge Base"]
        LANCE["LanceDB\nS3 / GCS / local\nANN vector search"]
    end

    subgraph tools ["MCP Tools — all 17 services (160+ tools)"]
        T1["makod · marktd · billingd"]
        T2["edmd · accountingd · obsd"]
        T3["einsd · netzbilanzd · sperrd · ..."]
    end

    TRIGGER --> orchestrator
    orchestrator --> builtin
    builtin -->|"ReAct: reason → act → observe"| tools
    builtin -->|"background knowledge"| rag
```

### Routing flow

```mermaid
sequenceDiagram
    participant CE as CloudEvent
    participant Orch as Orchestrator
    participant Reg as AgentRegistry
    participant Spec as Specialist
    participant MCP as MCP Tools

    CE->>Orch: de.billing.rechnung.erstellt
    Orch->>Reg: find_specialist(event_type)
    Note over Reg: trigger_patterns match?
    Reg-->>Orch: billing-regulatory-guard-agent
    Orch->>Spec: run(event_data)
    loop ReAct loop (max_turns)
        Spec->>MCP: get_billing_record(record_id)
        MCP-->>Spec: Rechnung JSON
        Spec->>MCP: validate_tariff_config(tariff)
        MCP-->>Spec: compliance warnings
        Note over Spec: Structure output
    end
    Spec-->>Orch: AgentDecision {outcome, summary}
    Orch-->>CE: de.agent.decision.made (audit)
```

### Parallel dispatch flow

```mermaid
sequenceDiagram
    participant CE as CloudEvent
    participant Orch as Orchestrator
    participant A1 as billing-anomaly-agent
    participant A2 as billing-regulatory-guard-agent
    participant MCP as MCP Tools

    CE->>Orch: de.billing.rechnung.erstellt
    Note over Orch: dispatch_mode = parallel
    Note over Orch: 2 specialists match trigger
    par Concurrent execution
        Orch->>A1: run(event_data)
        A1->>MCP: check_billing_anomaly
        MCP-->>A1: deviation_pct: 35%
        A1-->>Orch: ANOMALY_DETECTED: WARNING
    and
        Orch->>A2: run(event_data)
        A2->>MCP: get_billing_record
        MCP-->>A2: Rechnung JSON
        A2->>MCP: validate_tariff_config
        MCP-->>A2: §42 ENERGIEMIX_MISSING
        A2-->>Orch: COMPLIANCE_STATUS: WARNINGS
    end
    Note over Orch: Merge all AgentDecisions
    Orch-->>CE: de.agent.decision.made (merged)
```

---

## Agent Mesh

`agentd` uses the **Orchestrator + Specialist Mesh** pattern:

1. **Orchestrator** receives the trigger and either:
   - Matches a `trigger_pattern` glob → routes directly to the specialist
   - Asks the LLM to triage → specialist selection via `transfer_to_{specialist}` tool call
   - Answers directly if no specialist applies

2. **Specialist agents** run a **ReAct loop** (Reason → Act → Observe):
   - Each iteration calls one or more MCP tools
   - Observes tool results and decides next action
   - Continues until a `Text` result or a `Handoff` to another specialist

3. **Handoffs** are followed up to 3 hops. Each hop re-runs the full ReAct loop
   with the new specialist's system prompt and tool set.

### Bundled specialists

All provider/model assignments are **operator-configured** via `[bundled_agents] default_model`
and `[bundled_agents.overrides.<name>]`. The built-in definitions contain only the system
prompt, default trigger patterns, and default MCP server requirements.

| Specialist | Default triggers | Default MCP servers |
|---|---|---|
| `mako-agent` | `de.mako.process.*`, `de.mako.aperak.*` | makod, marktd, processd, obsd |
| `deadline-alert-agent` | `de.mako.process.escalated`, `de.mako.process.timedout`, `de.obs.stp.parity.alert` | obsd, makod, marktd |
| `billing-agent` | `de.invoic.receipt.disputed`, `de.accounting.*` | invoicd, billingd, accountingd, netzbilanzd |
| `netzbilanz-agent` | `de.netzbilanz.invoic.drafted`, `de.netzbilanz.invoic.dispatched` | netzbilanzd, marktd, edmd |
| `invoice-reconciliation-agent` | `de.invoic.payment.overdue`, `de.invoic.receipt.disputed` | invoicd, marktd, netzbilanzd |
| `billing-anomaly-agent` | `de.billing.rechnung.erstellt` | billingd, edmd |
| `billing-regulatory-guard-agent` | `de.billing.rechnung.erstellt` | billingd, marktd |
| `jahresabrechnung-agent` | manual trigger | billingd, edmd, marktd |
| `eeg-agent` | `de.eeg.anlage.foerderung_auslaufend`, `de.edmd.reading.direct.stored` | einsd, edmd, marktd |
| `eeg-compliance-agent` | `de.eeg.anlage.*`, `de.eeg.verguetung.*`, `de.eeg.marktpraemie.*` | einsd, obsd |
| `payment-reconciliation-agent` | `de.accounting.payment.due`, `de.accounting.bankruecklast` | accountingd |
| `compliance-agent` | `de.obs.stp.parity.alert` | obsd, processd, marktd, invoicd |
| `msb-history-agent` | `de.edmd.reading.quality.warning`, `de.edmd.reading.direct.stored` | edmd, makod, marktd |
| `meter-data-agent` | `de.edmd.reading.quality.warning`, `de.mako.process.completed` | edmd, marktd |
| `grid-anomaly-agent` | `de.markt.grid.drift.detected`, `de.markt.nb-contract.updated` | marktd, obsd |
| `tariff-optimization-agent` | `de.billing.rechnung.erstellt`, `de.mako.process.completed` | billingd, tarifbd, edmd, marktd |
| `vertragd-agent` | `de.vertrag.*`, `de.mako.process.abgelehnt` | vertragd, processd, marktd |
| `tarifbd-agent` | `de.tarifd.product.updated`, `de.tarifd.angebot.*` | tarifbd, marktd |
| `processd-agent` | `de.mako.process.initiated`, `de.mako.process.rejected` | processd, marktd, obsd |
| `sperrd-agent` | `de.sperr.*`, `de.mako.process.completed` | sperrd, makod, marktd |
| `nis-syncd-agent` | `de.markt.grid.drift.detected`, `de.markt.malo.updated` | nis-syncd, processd, marktd, obsd |
| `portald-agent` | `de.billing.rechnung.erstellt`, `de.eeg.anlage.foerderung_auslaufend`, `de.accounting.mahnung.issued` | portald, billingd, einsd, accountingd |
| `regulatory-reporting-agent` | manual / scheduled | obsd, processd, invoicd, marktd |
| `replacement-value-agent` | `de.edmd.reading.quality.warning`, `de.mako.process.completed` | edmd, marktd, obsd |
| `mabis-syncd-agent` | `de.edmd.reading.quality.warning` | edmd, obsd, marktd |
| `smgw-diagnostics-agent` | `de.edmd.reading.quality.warning`, `de.edmd.reading.direct.stored`, `de.mako.process.initiated` | edmd, marktd, obsd, processd |
| `vpp-billing-agent` | `de.vpp.dispatch.confirmed`, `de.vpp.settlement.berechnet` | billingd, marktd, obsd |

All 27 specialist definitions are compiled into the `agentd` binary. Activate them via
`[bundled_agents]` in `agentd.toml` — no system prompt copy-paste required.
See `demo/agentd.toml` for a working example.

---

## LLM Providers

| Provider | Backend string | Notes |
|---|---|---|
| OpenAI | `openai` | `text-embedding-3-small` for embeddings; compatible with Azure OpenAI, Ollama, LM Studio |
| Anthropic | `anthropic` | Claude 3.5 Sonnet; BM25 keyword fallback (no embedding API) |
| AWS Bedrock | `bedrock` | SigV4 signed requests; Claude on Bedrock or Titan embeddings |

---

## RAG (Retrieval-Augmented Generation)

`agentd` uses **LanceDB** as its vector store — a Rust-native, serverless vector database
that stores embeddings on object storage (S3/GCS/Azure Blob) or locally.

```mermaid
flowchart LR
    SRC["Knowledge sources\n(AHB PDFs, runbooks,\nBNetzA decisions)"]
    CHUNK["Paragraph-boundary\nchunking (512 tokens)"]
    EMBED["Embedding provider\n(OpenAI text-embedding-3-small)"]
    LANCE[("LanceDB\nS3 / GCS / local\nIVF_PQ ANN index")]
    QUERY["Query vector\n(question embedding)"]
    RESULT["Top-k chunks\n→ system prompt context"]

    SRC --> CHUNK --> EMBED --> LANCE
    QUERY --> LANCE --> RESULT
```

**BM25 fallback:** When using Anthropic (no embedding API), `agentd` runs keyword search
over all stored chunks. Suitable for knowledge bases up to ~50,000 chunks.

**Storage URI examples:**
```toml
storage_uri = "./data/rag"                    # local (dev)
storage_uri = "s3://my-bucket/rag"            # AWS S3
storage_uri = "gs://my-bucket/rag"            # Google Cloud Storage
storage_uri = "az://my-container/rag"         # Azure Blob
```

---

## Configuration

### Minimal — enable built-in specialists

```toml
# agentd.toml — using built-in specialist catalog
tenant = "9900357000004"

[providers.openai]
backend = "openai"
api_key = "env:OPENAI_API_KEY"

[orchestrator]
provider   = "openai"
model      = "gpt-4o"
max_turns  = 10
dispatch_mode = "sequential"  # sequential | parallel | race

# ── Enable built-in specialists ───────────────────────────────────────────────
[bundled_agents]
enable_all       = true          # activate all 27 built-in specialists
default_provider = "openai"
default_model    = "gpt-4o-mini"

# Upgrade specific agents to more capable models
[bundled_agents.overrides.mako-agent]
model = "gpt-4o"

[bundled_agents.overrides.jahresabrechnung-agent]
model     = "gpt-4o"
max_turns = 20

[mcp_servers]
makod    = "http://makod:8080/mcp"
marktd   = "http://marktd:8180/mcp"
billingd = "http://billingd:9280/mcp"
edmd     = "http://edmd:8380/mcp"
obsd     = "http://obsd:8480/mcp"
# ... all 17 services at their respective ports
mcp_api_key = "env:AGENTD_MCP_API_KEY"

trigger_event_types = [
  "de.mako.process.escalated",
  "de.billing.rechnung.erstellt",
  "de.eeg.*",
  "de.invoic.receipt.disputed",
]
```

### Custom agents — override or extend built-ins

```toml
# Custom agent that overrides billing-anomaly-agent with stricter threshold
[[agents]]
name      = "billing-anomaly-agent"
specialty = "Billing anomaly detection — strict mode (10% threshold)"
provider  = "openai"
model     = "gpt-4o"
max_turns = 12
mcp_servers = ["billingd", "edmd"]
trigger_patterns = ["de.billing.rechnung.erstellt"]
system_prompt = """
You are the billing anomaly detection specialist (strict mode: 10% threshold).
# ... your custom prompt ...
"""
```

### Parallel dispatch — compliance events

```toml
# Fan out to ALL specialists matching the event type simultaneously
[orchestrator]
dispatch_mode  = "parallel"
parallel_limit = 4  # max concurrent specialists

# billing.rechnung.erstellt now triggers BOTH:
# - billing-anomaly-agent    (deviation check)
# - billing-regulatory-guard-agent  (§40/§41/§41b compliance)
# simultaneously, returning aggregated results
```

---

## Triggering an agent run

**Via CloudEvent webhook:**

```bash
curl -X POST http://agentd:9580/webhook \
  -H "Content-Type: application/cloudevents+json" \
  -d '{
    "specversion": "1.0",
    "type": "de.billing.rechnung.disputed",
    "source": "billingd",
    "id": "123e4567-e89b-12d3-a456-426614174000",
    "data": { "malo_id": "51238696781", "record_id": "...", "reason": "check 4 failed" }
  }'
```

**Manual run:**

```bash
curl -X POST http://agentd:9580/api/v1/run \
  -H "Content-Type: application/json" \
  -d '{
    "event_type": "manual.billing.dispute-analysis",
    "data": { "malo_id": "51238696781", "context": "Invoice R2026-001 disputed" }
  }'
```

---

## WASM Plugins

`agentd` loads WASM plugins at startup from `[[plugin]]` entries in `agentd.toml`.
Plugins are sandboxed via Extism (Wasmtime) — no filesystem or network access.

```toml
[[plugin]]
kind = "wasm"
path = "./plugins/erp-formatter.wasm"
capabilities = ["cloud_event", "webhook"]

[[plugin]]
kind = "native"
path = "./plugins/libmy_billing_rules.so"
capabilities = ["billing"]
```

Plugin interfaces: `CloudEventPlugin` (enrich/filter events), `McpToolPlugin` (add
custom tools), `BillingPlugin` (post-process positions), `ValidatorPlugin` (custom
EDIFACT rules), `WebhookPlugin` (sign/enrich outbound webhooks).

---

## CloudEvents emitted

| Event type | When |
|---|---|
| `de.agent.decision.made` | Agent completes a run (includes decision text + tools used) |

---

## Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/webhook` | Inbound CloudEvent trigger |
| `POST` | `/api/v1/run` | Manual agent invocation |
| `GET`  | `/api/v1/sessions` | Last 100 agent decisions (in-memory ring buffer) |
| `GET`  | `/api/v1/agents` | List all active agents (built-in + custom) with capabilities |
| `GET`  | `/api/v1/agents/catalog` | Full catalog of all 27 built-in definitions (even if not enabled) |
| `GET`  | `/.well-known/agents/{name}` | A2A Agent Card for a named specialist |
| `POST` | `/api/v1/rag/ingest` | Index a live text document into LanceDB |
| `POST` | `/api/v1/rag/search` | Query the RAG knowledge base directly |
| `GET`  | `/health` | Liveness |
| `GET`  | `/health/ready` | Readiness |
