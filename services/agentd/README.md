# agentd

`agentd` (`:9580`) is mako's agent plane: declarative specialists that read
mako's services over MCP to analyse, check compliance and triage deadlines. Each
specialist is a manifest run by [agentplane](https://github.com/hupe1980/agentplane),
a journal-first runtime — every model and tool call is journaled before it
happens, a run survives a crash and replays for audit. Specialists are advisory:
they file worklist rows; `makod` alone dispatches market commands.

## Run it

```bash
just dev agentd        # dev/config/agentd.toml, one coded specialist enabled
```

The dev config backs the provider name `anthropic` with an OpenAI-compatible
server on `127.0.0.1:11434` (Ollama's default), called only when an event wakes
a model-backed specialist.

## At a glance

| | |
|---|---|
| **Specialists** | 28 manifests in [`agents/`](agents/) — 26 `tool-calling`, 2 model-free coded skills ([`src/skills/`](src/skills/)) |
| **Grants** | read-only only: 151 grants across 28 manifests, each named by its procedure (`cargo xtask check-tool-grants`, `check-prompt-tools`) |
| **Prompts** | mako's MCP servers publish 54 step-by-step prompts across 11 servers; specialists reference them through `context.prompts` |
| **Answers** | All 28 specialists declare a closed `output.schema`; coded finding codes are enums |
| **Oversight** | triage rules open worklist rows under `/api/v1/oversight/*`; rows escalate, never expire. Mounted only with `[oidc]` |
| **Journal** | `redb` (single instance) or Postgres; optional sealing (`[keyring]`), workload signatures (`[attestation]`) and C2SP `tlog-witness` cosigning (`[witness]`) |
| **Erasure** | every run joins a case keyed on its MaLo; `POST /api/v1/erasure` crypto-shreds the case key or forgets a memory subject (GDPR Art. 17) |
| **Providers** | Anthropic · OpenAI · Gemini · any OpenAI-compatible endpoint · AWS Bedrock with `--features bedrock` |
| **Role builds** | `role-lf` · `role-nb` · `role-msb` carry only one arm's specialists and desks, so a misconfigured plane cannot answer as a role it was not built for. The confidentiality duty of §§ 6a, 7a EnWG is the deployment's Cedar access policy, enforced at runtime |

## Routes

| Route | Purpose |
|---|---|
| `POST /webhook` | CloudEvent ingest. Admission commits before the answer: `202` admitted, `204` no subscriber, `429` transient (quota), `422` unprocessable, dead-lettered |
| `POST /api/v1/run` | start a run and wait for it (`session_timeout_secs`); honours `Idempotency-Key` |
| `GET /api/v1/agents`, `/api/v1/agents/catalog` | each specialist's version and manifest digest |
| `POST /api/v1/erasure` | `case_id` and/or `memory_subject`, plus a mandatory `reason` |
| `/api/v1/oversight/*` | agentplane's worklist, runs, cases and four-eyes decisions |
| `GET /metrics` | agentplane's instrument catalogue as `agentd_agentplane_*` series |

A run that completes emits `de.agent.decision.made` — `{run, case, outcome,
chain_head}`, plus `reason` on failure — to `audit_webhook_url`, signed with
Standard Webhooks.

## Configuration

```toml
# agentd.toml
# Every top-level key before the first [table]: TOML binds a bare key to the latest header, and every config type denies unknown fields.
tenant           = "9900357000004"
public_base_url  = "https://agentd.internal:9580"   # what an A2A card advertises
session_timeout_secs = 300   # bounds a *manual* run's wait; /webhook waits for nothing
sweep_interval_secs  = 60    # warns, breaches, escalates, expires, wakes, recovers

mcp_api_key         = "env:AGENTD_MCP_API_KEY"            # SecretString — never logged
inbound_hmac_secret = "env:AGENTD_INBOUND_HMAC_SECRET"

audit_webhook_url = "https://erp.example/hooks/agent-decisions"
audit_hmac_secret = "env:AGENTD_AUDIT_HMAC"
# audit_hmac_secret_previous = "env:AGENTD_AUDIT_HMAC_OLD"   # mid-rotation only
# admission_retention_days   = 30                          # absent = keep forever

[quota]
# max_concurrent_runs  = 64
# max_tokens_per_month = 200_000_000

# [attestation]
# required = true
# key_id   = "spiffe://mako/agentd"
# seed     = "env:AGENTD_SIGNING_SEED"

# [witness]            # requires [attestation]
# quorum        = 1     # zero is refused; above the witness count is refused
# interval_secs = 3600
# [[witness.witnesses]]
# name       = "witness.example.org"
# url        = "https://witness.example.org"
# public_key = "..."    # 32 bytes, standard base64

[journal]
backend = "redb"                                  # single instance
path    = "/var/lib/agentd/journal.redb"
# backend = "postgres"                            # several instances, one store
# url     = "env:AGENTD_JOURNAL_URL"

[providers.anthropic]
backend = "anthropic"
api_key = "env:ANTHROPIC_API_KEY"

# [providers.local]
# backend  = "chat-completions"
# api_base = "http://vllm:8000/v1"  # required: there is no default for your server

[bundled_agents]
enable_all = true
# enable = ["mako-agent", "billing-anomaly-agent"]

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
# path = "/etc/agentd/policy.cedar"

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

## Specialists

| Specialist | Capability | Shape | Reach | Subscribes to |
|---|---|---|---|---|
| `mako-agent` | `mako` | `tool-calling` | 7 on makod, marktd, processd | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.mako.aperak.*` |
| `deadline-alert-agent` | `deadline.alert` | **coded skill** (no model) | 1 on processd | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.obs.deadline.approaching` |
| `billing-agent` | `billing` | `tool-calling` | 9 on accountingd, billingd, invoicd, marktd | `de.invoic.receipt.disputed`, `de.accounting.mahnung.issued` |
| `netzbilanz-agent` | `netzbilanz` | `tool-calling` | 7 on marktd, netzbilanzd | `de.netzbilanz.invoic.drafted`, `de.netzbilanz.invoic.dispatched`, `de.netzbilanz.invoic.dispatch-overdue`, `de.netzbilanz.invoic.disputed` |
| `invoice-reconciliation-agent` | `invoice.reconciliation` | `tool-calling` | 6 on accountingd, billingd, invoicd | `de.invoic.payment.overdue`, `de.invoic.receipt.*` |
| `billing-anomaly-agent` | `billing.anomaly` | `tool-calling` | 6 on billingd, edmd | `de.billing.rechnung.erstellt` |
| `billing-regulatory-guard-agent` | `billing.regulatory.guard` | `tool-calling` | 5 on billingd, marktd, productd, vertragd | `de.billing.rechnung.erstellt` |
| `jahresabrechnung-agent` | `jahresabrechnung` | `tool-calling` | 7 on accountingd, edmd, marktd, productd | _manual / scheduled_ |
| `eeg-agent` | `eeg` | `tool-calling` | 4 on edmd, einsd, productd | `de.eeg.anlage.foerderung-auslaufend`, `de.messwert.reading.direct.stored` |
| `eeg-compliance-agent` | `eeg.compliance` | `tool-calling` | 5 on einsd | `de.eeg.anlage.*`, `de.eeg.verguetung.*`, `de.eeg.marktpraemie.*`, `de.eeg.compliance.*`, `de.eeg.veraeusserungsform.gewechselt` |
| `payment-reconciliation-agent` | `payment.reconciliation` | `tool-calling` | 4 on accountingd | `de.accounting.payment.due`, `de.accounting.bankruecklast`, `de.accounting.sepa.collection-rejected`, `de.accounting.sperrandrohung`, `de.accounting.sperrankuendigung`, `de.accounting.abwendung.angeboten`, `de.accounting.abwendung.gebrochen` |
| `compliance-agent` | `compliance` | `tool-calling` | 5 on processd | `de.obs.stp.parity.alert` |
| `msb-history-agent` | `msb.history` | `tool-calling` | 6 on edmd, marktd | `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.completed`, `de.messwert.reading.order.failed`, `de.messwert.reading.delivery.overdue` |
| `meter-data-agent` | `meter.data` | `tool-calling` | 3 on edmd | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `grid-anomaly-agent` | `grid.anomaly` | `tool-calling` | 3 on marktd | `de.markt.nb-contract.updated`, `de.markt.malo.updated` |
| `tariff-optimization-agent` | `tariff.optimization` | `tool-calling` | 6 on billingd, edmd, productd | `de.billing.rechnung.erstellt`, `de.mako.process.completed` |
| `vertragd-agent` | `vertragd` | `tool-calling` | 9 on marktd, processd, productd, vertragd | `de.vertrag.*`, `de.mako.aperak.rejected`, `de.mako.process.failed`, `de.vertrag.ablauf.ankuendigung`, `de.vertrag.preisaenderung.ankuendigung` |
| `productd-agent` | `productd` | `tool-calling` | 6 on productd | `de.tarif.product.updated`, `de.tarif.angebot.abgelaufen`, `de.tarif.epex.missing` |
| `processd-agent` | `processd` | `tool-calling` | 7 on marktd, processd | `de.mako.process.initiated`, `de.mako.aperak.rejected`, `de.mako.process.failed` |
| `sperrd-agent` | `sperrd` | `tool-calling` | 6 on makod, processd | `de.accounting.sperrauftrag`, `de.sperr.*`, `de.mako.process.completed` |
| `portald-agent` | `portald` | `tool-calling` | 8 on accountingd, billingd, einsd, marktd | `de.billing.rechnung.erstellt`, `de.eeg.anlage.foerderung-auslaufend`, `de.accounting.mahnung.issued`, `de.vertrag.*` |
| `regulatory-reporting-agent` | `regulatory.reporting` | `tool-calling` | 5 on processd | _manual / scheduled_ |
| `replacement-value-agent` | `replacement.value` | `tool-calling` | 3 on edmd | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `mabis-agent` | `mabis.submission` | `tool-calling` | 7 on edmd, netzbilanzd | `de.mabis.submission.failed`, `de.mabis.korrekturbedarf.opened`, `de.messwert.reading.quality.warning` |
| `smgw-diagnostics-agent` | `smgw.diagnostics` | `tool-calling` | 5 on edmd, marktd | `de.messwert.cls.compliance-issue`, `de.messwert.smgw.cert.expiry-warning`, `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.initiated`, `de.markt.geraet.konfiguration.updated` |
| `vpp-billing-agent` | `vpp.billing` | `tool-calling` | 3 on billingd, marktd | `de.vpp.dispatch.confirmed`, `de.vpp.settlement.berechnet` |
| `gabi-gas-agent` | `gabi.gas.balancing` | coded | 0 | `de.gabi.alocat.missing` |
| `einsd-batch-agent` | `einsd.batch` | `tool-calling` | 8 on edmd, einsd, productd | `de.eeg.settlement.batch-due`, `de.eeg.compliance.*`, `de.eeg.anlage.foerderung-auslaufend` |

`jahresabrechnung-agent` and `regulatory-reporting-agent` have no subscription:
an operator or scheduler starts them through `POST /api/v1/run`.

## Tests

`cargo test -p agentd --test it` runs the suites under `tests/it/` against real
stores with `FakeProvider`: the golden run and its strict replay, every
specialist answering its own schema, the exact procedure and tool surface each
model is offered, durability, ingest idempotency, oversight and evidence
signatures.

The [operator guide](https://hupe1980.github.io/mako/docs/services/agentd/)
covers the trust boundary, oversight, erasure, the journal's evidence chain and
the full configuration reference.
