# agentd — Multi-Agent LLM Orchestration

`agentd` connects large language models to the mako production services via MCP,
enabling automated analysis, compliance checking and workflow orchestration.

Every specialist is a **declarative manifest** run by
[agentplane](https://github.com/hupe1980/agentplane), a journal-first durable
agent runtime. Every model call and tool call is a journaled effect, so a run
survives a crash and replays for audit.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9580` |
| **Specialists** | 28 manifests in `agents/`, embedded at compile time |
| **Runtime** | agentplane — journaled effects, strict replay, human approval on mutating tools |
| **Model providers** | Anthropic · OpenAI (and OpenAI-compatible) · AWS Bedrock |
| **Role scoping** | `role-lf` · `role-nb` · `role-msb` — a role build contains no other arm's specialists (§ 9 EnWG) |
| **Journal** | redb at `journal_path` — the § 147 AO / GoBD record, sealed by the key ring |
| **A2A cards** | `GET /.well-known/agents/{name}` for each specialist |
| **Catalogue** | `GET /api/v1/agents/catalog` |

## The manifest is the agent

`agents/<name>.yaml` declares the procedure, the model pair, every tool the agent
may call, the ceilings it runs under and the schema its result must satisfy. It
is digest-covered: editing a procedure is a version bump a reviewer sees.

`src/builtin/mod.rs` holds the one thing agentplane has no notion of — which
CloudEvent types reach which specialist — and nothing else. A second copy of the
prompt in Rust could disagree with the manifest, and the manifest is the copy the
model reads.

Consequently there are **no per-agent config overrides**. Changing a specialist's
model is a manifest edit, which is the reviewable path by design.

Two properties matter for a regulated deployment:

- **Mutating tools require a human.** Every state-changing grant carries
  `requires_approval: true`, bounded by an obligation in `spec.oversight` with
  `on_expiry: deny`.
- **Authority-bearing arguments are bound to trusted sources.**
  `protected_fields` marks `/malo_id` and `/pid` as `require_trusted`, so a value
  derived from counterparty free text cannot reach `submit_command`.

The deterministic boundary is unchanged: an agent may prepare and may wait,
`makod` still dispatches.

## Specialists

Each row is a subscription paired with a manifest. The capability is what
`Runtime::run` is given.

| Specialist | Capability | Subscribes to |
|---|---|---|
| `mako-agent` | `mako` | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.mako.aperak.*` |
| `deadline-alert-agent` | `deadline.alert` | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.obs.deadline.approaching` |
| `billing-agent` | `billing` | `de.invoic.receipt.disputed`, `de.accounting.mahnung.issued` |
| `netzbilanz-agent` | `netzbilanz` | `de.netzbilanz.invoic.drafted`, `de.netzbilanz.invoic.dispatched`, `de.netzbilanz.invoic.dispatch-overdue` |
| `invoice-reconciliation-agent` | `invoice.reconciliation` | `de.invoic.payment.overdue`, `de.invoic.receipt.*` |
| `billing-anomaly-agent` | `billing.anomaly` | `de.billing.rechnung.erstellt` |
| `billing-regulatory-guard-agent` | `billing.regulatory.guard` | `de.billing.rechnung.erstellt` |
| `jahresabrechnung-agent` | `jahresabrechnung` | _manual / scheduled_ |
| `eeg-agent` | `eeg` | `de.eeg.anlage.foerderung-auslaufend`, `de.messwert.reading.direct.stored` |
| `eeg-compliance-agent` | `eeg.compliance` | `de.eeg.anlage.*`, `de.eeg.verguetung.*`, `de.eeg.marktpraemie.*`, `de.eeg.compliance.*` |
| `payment-reconciliation-agent` | `payment.reconciliation` | `de.accounting.payment.due`, `de.accounting.bankruecklast` |
| `compliance-agent` | `compliance` | `de.obs.stp.parity.alert` |
| `msb-history-agent` | `msb.history` | `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.completed` |
| `meter-data-agent` | `meter.data` | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `grid-anomaly-agent` | `grid.anomaly` | `de.markt.nb-contract.updated`, `de.markt.malo.updated` |
| `tariff-optimization-agent` | `tariff.optimization` | `de.billing.rechnung.erstellt`, `de.mako.process.completed` |
| `vertragd-agent` | `vertragd` | `de.vertrag.*`, `de.mako.aperak.rejected`, `de.mako.process.failed`, `de.vertrag.ablauf.ankuendigung`, `de.vertrag.preisaenderung.ankuendigung` |
| `tarifbd-agent` | `tarifbd` | `de.tarif.product.updated`, `de.tarif.angebot.abgelaufen`, `de.tarif.epex.missing` |
| `processd-agent` | `processd` | `de.mako.process.initiated`, `de.mako.aperak.rejected`, `de.mako.process.failed` |
| `sperrd-agent` | `sperrd` | `de.accounting.sperrauftrag`, `de.sperr.*`, `de.mako.process.completed` |
| `portald-agent` | `portald` | `de.billing.rechnung.erstellt`, `de.eeg.anlage.foerderung-auslaufend`, `de.accounting.mahnung.issued`, `de.vertrag.*` |
| `regulatory-reporting-agent` | `regulatory.reporting` | _manual / scheduled_ |
| `replacement-value-agent` | `replacement.value` | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `mabis-syncd-agent` | `mabis.syncd` | `de.messwert.reading.quality.warning` |
| `smgw-diagnostics-agent` | `smgw.diagnostics` | `de.messwert.cls.compliance-issue`, `de.messwert.smgw.cert.expiry-warning`, `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.initiated`, `de.markt.geraet.konfiguration.updated` |
| `vpp-billing-agent` | `vpp.billing` | `de.vpp.dispatch.confirmed`, `de.vpp.settlement.berechnet` |
| `gabi-gas-agent` | `gabi.gas.balancing` | `de.gabi.imbalance.*`, `de.gabi.alocat.missing`, `de.gabi.nomination.*`, `de.netzbilanz.invoic.drafted` |
| `einsd-batch-agent` | `einsd.batch` | `de.eeg.settlement.batch-due`, `de.eeg.compliance.*`, `de.eeg.anlage.foerderung-auslaufend` |

## Configuration

```toml
# agentd.toml
tenant       = "9900357000004"
journal_path = "/var/lib/agentd/journal.redb"   # § 147 AO record — durable storage

# The key is the name a manifest's `spec.models` refers to.
[providers.anthropic]
backend = "anthropic"
api_key = "env:ANTHROPIC_API_KEY"   # SecretString — never logged

# Which specialists this deployment runs. A name matching no compiled
# specialist is a startup failure, not an inactive agent.
[bundled_agents]
enable_all = true
# enable = ["mako-agent", "billing-anomaly-agent"]

# OIDC (optional — dev mode when absent, all POST /api/v1/run requests accepted)
[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "agentd"

# Inbound HMAC verification (strongly recommended in production)
inbound_hmac_secret  = "env:AGENTD_INBOUND_HMAC_SECRET"
max_sessions         = 20
session_timeout_secs = 300   # bounds one event's whole fan-out

mcp_api_key = "env:AGENTD_MCP_API_KEY"   # SecretString — never logged

# Keys are free-form names; values are MCP endpoints. Must come last: any
# key after a table header belongs to that table.
[mcp_servers]
makod    = "http://makod:8080/mcp"
marktd   = "http://marktd:8180/mcp"
billingd = "http://billingd:9280/mcp"
# ... more services
```

## Durability instead of retries

A failed run is not a message with nowhere to go: its effects are journaled, so
it resumes from the last completed effect rather than being replayed from the top
by a retry loop. There is no dead-letter queue and no `de.agent.session.dlq.*`
event.

`de.agent.decision.made` carries the run's real outcome — `completed`, `failed`,
`suspended`, `exhausted`, `quarantined`, `replanning` or `cancelled` — and its
`session_id` is the journal run id an operator can look up. A **suspended** run
is waiting for a human decision, not failing.

## Fan-out

Several specialists may subscribe to one event; they are independent opinions, so
each gets its own run and its own journal. There is deliberately no first-wins
mode — abandoning an in-flight branch leaves a started effect with no terminal
record, which for a mutating tool call is an unrecoverable unknown outcome.

See [the operator guide](https://hupe1980.github.io/mako/docs/services/agentd/)
for the architecture diagrams, role scoping and erasure model.
