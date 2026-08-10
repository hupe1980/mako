# agentd — the multi-agent plane

`agentd` connects large language models to the mako production services over MCP:
automated analysis, compliance checking and deadline work, with every model call
and tool call written down before it happens.

Every specialist is a **declarative manifest** run by
[agentplane](https://github.com/hupe1980/agentplane), a journal-first durable
agent runtime. A run survives a crash, replays for audit, and stops for a named
human before it changes anything.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9580` |
| **Specialists** | 28 manifests in `agents/`, embedded at compile time |
| **Runtime** | agentplane 0.14 — journaled effects, strict replay, Cedar gate, sealed at rest; RFC 8785-complete canonicalization (`canon::VERSION` 3), oversight fails closed without a case store |
| **Model providers** | Anthropic · OpenAI · Gemini · the OpenAI-compatible wire (TGI, vLLM, Ollama, llama.cpp) · AWS Bedrock behind `--features bedrock` |
| **Tool transport** | one MCP client per server in `[mcp_servers]`, routed by the server component of each `tool://` grant |
| **Journal** | redb *or* Postgres — the § 147 AO / GoBD record, sealed by a Vault-held key |
| **Case layer** | every run joins a case keyed on its MaLo/MeLo/process — the unit of approval, obligation and **erasure** |
| **Oversight** | `/api/v1/oversight/*` — worklist, run views, case history, four-eyes decisions |
| **Role scoping** | `role-lf` · `role-nb` · `role-msb` — a role build contains no other arm's specialists (§ 9 EnWG) |
| **A2A cards** | `GET /.well-known/agents/{name}`, derived from each manifest |

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

- **Authority-bearing arguments are bound to trusted sources.**
  `protected_fields` marks `/malo_id`, `/pid` and `/mp_id` as `require_trusted`,
  so a value derived from counterparty free text cannot reach `submit_command`.
- **The payload is not trusted just because mako emitted it.** A CloudEvent field
  is promoted only if `plane::label` re-validates it against the format that
  identifier is defined to have; everything else is untrusted and carries the
  event it arrived on as its source. Admitting the payload wholesale would have
  satisfied `require_trusted` with a counterparty-chosen value.

## Two execution shapes, and what each one can do

**27 specialists run `tool-calling`.** The model chooses each next call and is
handed the whole payload with per-field labels. They **read and report**: their
conclusion leaves as `de.agent.decision.made`, and acting on it is the ERP's job.

**`gabi-gas-agent` runs `planned`.** One privileged call reads only the
re-validated identifiers and emits a plan — which granted tools, in what order,
with which arguments — and the runtime executes that plan itself. Control flow is
fixed before anything untrusted is read, and step outputs move between steps by
reference rather than back through a model's context, so a hostile tool result
cannot steer the steps that follow it. Counterparty material is read in a `parse`
step on the **quarantined** model, under a declared schema; the only thing that
step can say out of band is *not enough information*, which fails it.

The split is not a preference, and it decides what a specialist may be granted:

> **A `tool-calling` agent cannot dispatch a mutating call at all.** Its
> arguments come out of a model completion, agentplane labels every completion
> `untrusted` by construction, and the taint gate refuses a mutating sink with
> untrusted arguments. A `planned` agent can, because its step arguments are
> `$input/…` references the runtime resolves itself — they arrive carrying the
> run input's own labels, having never passed through a model's context.

So the 27 advisory manifests declare no mutating grant and no `oversight` block:
both absences are the same fact. `cargo xtask check-tool-grants` enforces it, and
`tests/oversight.rs` runs the working shape end to end — plan, suspend, approve,
dispatch. Regaining dispatch for a specialist means converting it to `planned`,
not adding a grant.

The other 27 declare no quarantined model on purpose. Under `tool-calling` with
no memory formation nothing would select it, so the declaration would read as
dual-model isolation while every call went to the privileged model — which
agentplane refuses at parse.

The deterministic boundary is unchanged: an agent may prepare and may wait,
`makod` still dispatches.

## Human oversight

A mutating grant carries `requires_approval: true`, and reaching it suspends the
run and opens a **task** — the exact tool and the exact arguments, not a
description of them — on the worklist of the roles the manifest names in
`oversight.approvers`. The obligation beside it is a real deadline: resolved
through mako's own BDEW Werktage calendar (`plane::calendar`), journaled with the
calendar's digest, and expired by the sweeper with `on_expiry: deny`.

The worklist is agentplane's own operator surface, mounted at
`/api/v1/oversight`:

| Route | The question it answers |
|---|---|
| `GET /tasks` | What is waiting for me? |
| `POST /tasks/{id}/claim` · `/release` | This one is mine — don't duplicate the work |
| `POST /tasks/{id}/decide` | Approve or reject, **as myself** |
| `GET /runs/{run}` | What is this run doing, and why is it not finishing? |
| `GET /cases/{case}` | What has happened on this matter, and by when must it end? |
| `POST /runs/{run}/cancel` | Stop it, with a reason on the record |
| `POST /events` | This message arrived; wake whoever wanted it |

Who is acting comes from the OIDC token, never from the request body — the wire
types carry no actor field, so an approval cannot be forged by the thing being
approved. Four-eyes is enforced in the task store: the actor who proposed an
action cannot approve it.

**Without OIDC the surface is not mounted at all.** Every other dev-mode
relaxation in mako accepts an unauthenticated request and warns; an approval is
the one place where that is not a relaxation but a forged signature on a
regulated dispatch.

## The case is the erasure unit

Every run is admitted with correlation keys derived from the event's re-validated
identifiers — `malo`, `melo`, `process` — so all runs about one Marktlokation
share one case. With a key ring configured, everything the plane writes down is
sealed under that case's wrapping key, so a GDPR Art. 17 request is answered by
destroying one key: the plaintext goes in the live store, every replica and every
backup at once, while the hash chain still verifies.

Without a key ring the plane starts and says so, loudly and specifically —
journaled personal data in an append-only chain cannot be erased by any later
configuration change. `[keyring] required = true` turns the warning into a
refusal to start.

## Configuration

```toml
# agentd.toml
tenant           = "9900357000004"
public_base_url  = "https://agentd.internal:9580"   # what an A2A card advertises

# The journal, the cases, the tasks, the timers and the events — one backend.
[journal]
backend = "redb"                                  # single instance
path    = "/var/lib/agentd/journal.redb"
# backend = "postgres"                            # several instances, one store
# url     = "env:AGENTD_JOURNAL_URL"

# The key is the name a manifest's `spec.models` refers to; `backend` is the
# wire it speaks. `[providers.anthropic]` backed by `chat-completions` against
# your own vLLM changes no manifest.
[providers.anthropic]
backend = "anthropic"
api_key = "env:ANTHROPIC_API_KEY"   # SecretString — never logged

# [providers.local]
# backend  = "chat-completions"
# api_base = "http://vllm:8000/v1"  # required: there is no default for your server

# Which specialists this deployment runs. A name matching no compiled
# specialist is a startup failure, not an inactive agent.
[bundled_agents]
enable_all = true
# enable = ["mako-agent", "billing-anomaly-agent"]

# Required for the oversight surface: no identity, no worklist.
[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "agentd"

# Envelope encryption. The wrapping key is created inside Vault and never
# leaves it, so erasure is something mako asks for and cannot undo.
[keyring]
required = true
[keyring.vault]
address = "https://vault.internal:8200"
mount   = "transit"
token   = "env:VAULT_TOKEN"

# Cedar rules. Omit to use mako's own, embedded from policy/agentd.cedar.
# A file here *replaces* them — Cedar allows on any matching permit, so a
# least-privilege file cannot narrow one it inherited.
[policy]
# path = "/etc/agentd/policy.cedar"

inbound_hmac_secret  = "env:AGENTD_INBOUND_HMAC_SECRET"
max_sessions         = 20
session_timeout_secs = 300   # bounds one event's whole fan-out
sweep_interval_secs  = 60    # warns, breaches, expires, wakes

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
`suspended`, `exhausted`, `quarantined`, `replanning`, `cancelled` or
`not-admitted` — with `run_id` (the journal key an operator looks up),
`waiting_for` (what a suspended run is waiting *for*, so an operator knows
whether to approve, chase or wait) and `tokens` (what the run cost).

## Fan-out

Several specialists may subscribe to one event; they are independent opinions, so
each gets its own run and its own journal. There is deliberately no first-wins
mode — abandoning an in-flight branch leaves a started effect with no terminal
record, which for a mutating tool call is an unrecoverable unknown outcome.

## Specialists

Each row is a subscription paired with a manifest. The capability is what a run
is addressed to.

| Specialist | Capability | Shape | Subscribes to |
|---|---|---|---|
| `mako-agent` | `mako` | `tool-calling` | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.mako.aperak.*` |
| `deadline-alert-agent` | `deadline.alert` | `tool-calling` | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.obs.deadline.approaching` |
| `billing-agent` | `billing` | `tool-calling` | `de.invoic.receipt.disputed`, `de.accounting.mahnung.issued` |
| `netzbilanz-agent` | `netzbilanz` | `tool-calling` | `de.netzbilanz.invoic.drafted`, `de.netzbilanz.invoic.dispatched`, `de.netzbilanz.invoic.dispatch-overdue` |
| `invoice-reconciliation-agent` | `invoice.reconciliation` | `tool-calling` | `de.invoic.payment.overdue`, `de.invoic.receipt.*` |
| `billing-anomaly-agent` | `billing.anomaly` | `tool-calling` | `de.billing.rechnung.erstellt` |
| `billing-regulatory-guard-agent` | `billing.regulatory.guard` | `tool-calling` | `de.billing.rechnung.erstellt` |
| `jahresabrechnung-agent` | `jahresabrechnung` | `tool-calling` | _manual / scheduled_ |
| `eeg-agent` | `eeg` | `tool-calling` | `de.eeg.anlage.foerderung-auslaufend`, `de.messwert.reading.direct.stored` |
| `eeg-compliance-agent` | `eeg.compliance` | `tool-calling` | `de.eeg.anlage.*`, `de.eeg.verguetung.*`, `de.eeg.marktpraemie.*`, `de.eeg.compliance.*` |
| `payment-reconciliation-agent` | `payment.reconciliation` | `tool-calling` | `de.accounting.payment.due`, `de.accounting.bankruecklast` |
| `compliance-agent` | `compliance` | `tool-calling` | `de.obs.stp.parity.alert` |
| `msb-history-agent` | `msb.history` | `tool-calling` | `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.completed` |
| `meter-data-agent` | `meter.data` | `tool-calling` | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `grid-anomaly-agent` | `grid.anomaly` | `tool-calling` | `de.markt.nb-contract.updated`, `de.markt.malo.updated` |
| `tariff-optimization-agent` | `tariff.optimization` | `tool-calling` | `de.billing.rechnung.erstellt`, `de.mako.process.completed` |
| `vertragd-agent` | `vertragd` | `tool-calling` | `de.vertrag.*`, `de.mako.aperak.rejected`, `de.mako.process.failed`, `de.vertrag.ablauf.ankuendigung`, `de.vertrag.preisaenderung.ankuendigung` |
| `tarifbd-agent` | `tarifbd` | `tool-calling` | `de.tarif.product.updated`, `de.tarif.angebot.abgelaufen`, `de.tarif.epex.missing` |
| `processd-agent` | `processd` | `tool-calling` | `de.mako.process.initiated`, `de.mako.aperak.rejected`, `de.mako.process.failed` |
| `sperrd-agent` | `sperrd` | `tool-calling` | `de.accounting.sperrauftrag`, `de.sperr.*`, `de.mako.process.completed` |
| `portald-agent` | `portald` | `tool-calling` | `de.billing.rechnung.erstellt`, `de.eeg.anlage.foerderung-auslaufend`, `de.accounting.mahnung.issued`, `de.vertrag.*` |
| `regulatory-reporting-agent` | `regulatory.reporting` | `tool-calling` | _manual / scheduled_ |
| `replacement-value-agent` | `replacement.value` | `tool-calling` | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `mabis-syncd-agent` | `mabis.syncd` | `tool-calling` | `de.messwert.reading.quality.warning` |
| `smgw-diagnostics-agent` | `smgw.diagnostics` | `tool-calling` | `de.messwert.cls.compliance-issue`, `de.messwert.smgw.cert.expiry-warning`, `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.initiated`, `de.markt.geraet.konfiguration.updated` |
| `vpp-billing-agent` | `vpp.billing` | `tool-calling` | `de.vpp.dispatch.confirmed`, `de.vpp.settlement.berechnet` |
| `gabi-gas-agent` | `gabi.gas.balancing` | **`planned`** | `de.gabi.imbalance.*`, `de.gabi.alocat.missing`, `de.gabi.nomination.*`, `de.netzbilanz.invoic.drafted` |
| `einsd-batch-agent` | `einsd.batch` | `tool-calling` | `de.eeg.settlement.batch-due`, `de.eeg.compliance.*`, `de.eeg.anlage.foerderung-auslaufend` |

See [the operator guide](https://hupe1980.github.io/mako/docs/services/agentd/)
for the architecture diagrams, role scoping and erasure model.
