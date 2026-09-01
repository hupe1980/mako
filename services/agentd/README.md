# agentd — the multi-agent plane

`agentd` (`:9580`) connects large language models to mako's production services
over MCP: automated analysis, compliance checking and deadline triage, with
every model call and tool call written down **before** it happens.

Each specialist is a declarative manifest run by
[agentplane](https://github.com/hupe1980/agentplane), a journal-first durable
agent runtime. A run survives a crash, replays for audit, and stops for a named
human before it changes anything.

| | |
|---|---|
| **HTTP port** | `:9580` |
| **Specialists** | 28 manifests in [`agents/`](agents/) — 26 `tool-calling`, 2 model-free coded skills |
| **Runtime** | agentplane — journaled effects, strict replay, Cedar gate, sealed at rest |
| **Model providers** | Anthropic · OpenAI · Gemini · any OpenAI-compatible endpoint (TGI, vLLM, Ollama, llama.cpp) · AWS Bedrock behind `--features bedrock` |
| **Tool transport** | one MCP client per server granted by an activated manifest, routed by the server component of each `tool://` grant |
| **Journal** | redb *or* Postgres — durable, tamper-evident evidence; optional sealing, workload signatures and independent witnessing |
| **Witnessing** | checkpoints cosigned over C2SP `tlog-witness` — the one control that is not a check on ourselves |
| **Case layer** | every run joins a case keyed on its MaLo/MeLo/process — the unit of approval, obligation and **erasure** |
| **Oversight** | `/api/v1/oversight/*` — worklist, run views, case history, four-eyes decisions |
| **Role scoping** | `role-lf` · `role-nb` · `role-msb` — supports § 6a EnWG confidentiality and, where applicable, § 7a EnWG operational unbundling |

## How a run happens

```mermaid
graph TB
    EV["CloudEvent<br/>de.*"] --> RT["Router<br/>event type → specialists"]
    RT --> LB["plane::label<br/>re-validate identifiers"]
    LB --> RUN["Runtime<br/>journaled effects"]
    RUN --> MCP["MCP tool calls<br/>mako's own services"]
    RUN --> WL["Worklist<br/>approval · triage"]
    RUN --> JRN[("Journal<br/>hash-chained, sealed")]
    RUN --> OUT["de.agent.decision.made<br/>journal-backed outbox"]
    WL -->|"approve"| MCP
```

Routing is the one thing agentplane has no notion of, so it stays in Rust
([`src/builtin/mod.rs`](src/builtin/mod.rs)) — a subscription table and nothing
else. Everything after admission is the runtime's: the turn loop, the journal,
the policy gate, the case layer.

## The manifest is the agent

`agents/<name>.yaml` declares the procedure, the model, every tool the agent may
call, the ceilings it runs under and the schema its result must satisfy. It is
digest-covered: editing a procedure changes the digest, which is a version bump
a reviewer sees in a diff. There are consequently **no per-agent config
overrides** — moving a regulated decision onto a different model is a manifest
edit, on the reviewable path.

Every file opens with a schema modeline:

```yaml
# yaml-language-server: $schema=https://hupe1980.github.io/agentplane/agent.schema.json
```

agentplane publishes the manifest format as a draft-07 JSON Schema generated
from the types its parser deserializes into, so an editor gives autocomplete,
hover documentation and inline unknown-field errors while a manifest is being
written. The parser stays authoritative — its semantic refusals (an unstated
budget, a declared control nothing performs) run only there — and a test reads
the URL off `Manifest::json_schema()` so no file can point at a document that
has moved.

Two properties decide how a specialist can be built:

- **The payload is not trusted because mako emitted it.** A CloudEvent field is
  promoted to `trusted` only if [`plane::label`](src/plane/label.rs)
  re-validates it against the format that identifier is defined to have — MaLo
  11 digits, MP-ID 13, PID 5. Everything else stays untrusted and carries the
  event it arrived on as its source. An 11-digit MaLo has no room for an
  instruction; that is what makes the promotion honest.
- **Validation is semantic.** MaLo and market-participant check digits, EIC
  check characters, UUID version 4 and real calendar dates are checked. The
  tenant isolation key is never promoted as a market identifier.

## Two execution shapes

| | `tool-calling` (26) | coded skill (2) |
|---|---|---|
| Control flow | the model chooses read-only evidence calls | Rust |
| Input | whole payload, per-field labels | whole payload, per-field labels |
| Mutating grants | none | none |
| Model spend | per event, up to the budget | none (`models: {}`) |

The active plane is advisory: all 150 grants are read-only. Model-backed
specialists may investigate and file triage; they cannot dispatch market
commands. Deterministic procedures are coded rather than expressed as prompts.

The deterministic boundary is unchanged: **an agent may prepare and may wait,
`makod` still dispatches.** An approved decision becomes an ordinary command
through the command API, so what goes on the wire stays a pure function of a
recorded command.

### The model-free specialist

`deadline-alert-agent` and `gabi-gas-agent` declare `models: {}` — agentplane's
spelling of *no inference, on purpose* — and no `execution` block. Their
behaviour is registered in [`src/skills/`](src/skills/): deadline severity and
missing-final-ALOCAT triage are deterministic event transformations.

Governance is identical: the tool call is still a journaled effect through the
policy gate, the clock read is still an effect so a replay classifies against
the instant the original run saw, and the manifest still binds the grant, the
ceilings and the egress. Governance was never what a model was buying.

A specialist belongs in `skills/` when its procedure is a total function of what
the tools return. It does not when the task is judgement over open-ended input.

> A model-free agent declares **no** `max_tokens`. A zero ceiling reads as
> parsimony but means "exhausted before the first effect of any kind" — the tool
> call included. `max_steps`, `max_effects` and `max_wallclock_secs` are what
> bind it.

## Every answer is a shape

All 28 specialists declare `output.schema`, and every schema is **closed**
(`additionalProperties: false`) — at the root *and* at every nested object that
names its fields. The holes were one level in: `findings[]`, `violations[]`,
`failed_checks[]` — the arrays carrying what the specialist actually concluded,
and the part a triage rule's `path` reaches. An object that declares no
`properties` is a map whose keys are data (a count per MP-ID, an event echoed
back) and stays open, because closing one would forbid what it exists to carry.
A prose `OUTPUT FORMAT` block inside a prompt
is a contract in the one place nothing can enforce it; as a schema the model is
held to it, the runtime folds it into the effect key — so editing the schema
reports divergence on replay rather than reinterpreting a stored answer — and it
is covered by the manifest digest. Closed, the declared shape is the whole
shape, which is also what keeps a triage rule's `path` total over what the model
can return.

### A code with nowhere to go

A closed schema is also a ceiling: a finding code a procedure defines and the
schema does not carry cannot be returned at all — the model is told to report
`SECT41A_IMSYS_REQUIRED`, has no field for it, and the run completes and
validates with the finding gone. Coded findings are therefore enums rather than
free strings, and a test reads every code a procedure tells the model to emit and
refuses one the schema cannot hold. Codes the procedure only *reads* — billingd's
risk findings, einsd's settlement state — are inputs and belong in no answer
schema.

## Human oversight

The active manifests use triage only. Agentplane also supports approval before a
mutating call, but agentd currently grants no mutating tool.

| | **Approval** — in front of the answer | **Triage** — beside the answer |
|---|---|---|
| Declared as | `approval: tools-only` + `requires_approval` | `approval: none` + `oversight.triage` |
| What waits | the run suspends before the tool call | nothing; the run completes |
| Who is asked | `oversight.approvers` | the rule's `audience` |
| Fires on | reaching a mutating tool | an answer matching a predicate over `output.schema` |
| When nobody answers | `on_expiry: deny` — fails closed, nothing is sent | `on_expiry: escalate` — widens the audience and keeps waiting |
| Used by | `gabi-gas-agent` | 14 specialists, on a terminal finding |

The coded specialist is the third case and reaches the same worklist by a
different road. `deadline-alert-agent` cannot declare `oversight` at all —
agentplane refuses the block on a manifest with no `execution` — so a `BREACH`
opens its row from Rust, through `StepCtx::open_task`: same store, same audience
rules, same escalation on expiry. The terminal-finding lint exempts coded
specialists by a named list it asserts against, never by assumption.

Triage exists because most specialists cannot act, so gating their answer would
gate nothing while suspending a run per finding. A triage rule changes nothing
about the run — same answer, same validation, same memories — and its only
effect is a worklist row.

**That is also why the two expire differently.** An approval gates a real market
message, so a window that closes unanswered must send nothing. A triage row
gates nothing — it *is* the finding — so expiring it deletes the delivery of
something the agent correctly detected. Escalation is the only disposition under
which an unanswered §20 EnWG parity deviation or an out-of-compliance §§41f/41g
sequence stays findable: the `escalate_to` roles **join** the audience (the
original reviewers stay eligible), the stale reservation is cleared, and the row
keeps waiting. A test pins the two dispositions apart.

Eligibility is two layers: Cedar decides who may use the worklist at all, and
the task store narrows per task by `candidate_roles`. The Cedar set admits the
union of every `oversight.approvers` entry, every `triage[].audience` **and
every `escalate_to` role**, and a test parses the manifests and fails when one
names a role the policy does not admit — without it the two drift apart
silently, and a row whose audience is refused at the door is worse than no row.
The escalation audience most needs the check: it arrives hours late, from the
sweeper, and a widening to somebody Cedar refuses looks — from the worklist —
exactly like the row having been answered.

A second test walks `agentplane::api::action::ALL` and fails when any verb the
oversight surface asks about is granted to no role. That is the quietest
authorization failure there is: a permanent `403` on a route, with a policy set
that compiles clean.

Who is acting comes from the OIDC token, never from the request body. Four-eyes
is enforced in the task store. **Without OIDC the surface is not mounted at
all**: every other dev-mode relaxation in mako accepts an unauthenticated
request and warns, but an approval is a signature on a regulated dispatch.

### A finding stays inside its own arm

Role scoping keeps a build from *containing* the other arm's specialists. A
worklist row travels the other way: it carries the run's state, its justification
and its case, so filing an NB finding on a supply-side desk is grid operational
state reaching supply people — the confidentiality and operational boundaries
in §§ 6a and 7a EnWG — and in a
role-scoped deployment that desk may not exist to answer it.

Desks are classified by arm — supply, grid, metering, and the cross-cutting
three. A role build asserts no compiled specialist names another arm's, and a
default-build check refuses an audience belonging to no classified arm, so a new
desk cannot arrive and make the first one skip it.

## agentd's own doors are authorized too

The four routes agentd serves itself ask the **same Cedar set** the runtime
checks every effect against, under three verbs of mako's own:

| Verb | Route | Audience |
|---|---|---|
| `api:run.start` | `POST /api/v1/run` | the operations union, plus `mako-service` |
| `api:agent.list` | `GET /api/v1/agents`, `/api/v1/agents/catalog` | the same |
| `api:erasure.execute` | `POST /api/v1/erasure` | `mako-operations`, `regulatory` |

`mako-service` is on the list deliberately: the two manual-only specialists exist
*because* no CloudEvent marks "the reporting period ended", so a scheduler is a
first-class caller of that door.

Cedar sees the door and not the specialist. Narrowing per agent belongs to the
manifest's own controls — ceilings, grants and triage — and putting it in two places would be two
audiences to keep in step. The deployment's tenant is pinned at extraction
(`ExpectedTenant`), so a token the same realm signed for another operator is
refused; agentplane's own surface resolves the plane from the caller's tenant and
finds none.

## The legal basis is part of the answer

A specialist's output is a recommendation with a § attached, so a wrong citation
is a wrong answer that reads as a right one. Two rules:

- **Quote the service that computes the number.** `sperrd`'s
  `frist_ueberschritten` for the GPKE execution window, `energy-billing`'s
  `guthabenerstattung` for the § 40c Abs. 3 refund. A restated rule is a second
  copy, and its drift is silent because both sides look right alone.
- **Label anything else as ours.** § 60 Abs. 2 MsbG names no substitution method
  and § 20 EnWG names no straight-through rate, so a gap table or a 95 % target
  attributed to either is an operating default wearing a statute. Same for a
  repealed authority: BK6-20-061 died with BK6-23-241, so the Kostenblatt
  Stichtag is the operator's.

## Knowledge is granted, not copied

mako's MCP servers publish 57 step-by-step prompts across 15 servers for their
own procedures.
26 specialists declare `context.prompts` grants against their own service rather
than carrying a hand-typed paraphrase that drifts the first time either side
changes. A context grant is not a tool grant — reading a prompt authorises no
action — but it does cross a trust and data-egress boundary, so it is declared
where a reviewer sees it.

## Memory, scoped to the party the run is about

`memory_formation.subject` is the unit `forget_subject` erases, so its scope is
a GDPR decision. Seven specialists form memories: **five bind
`$correlation/malo`**, so the subject resolves per run to the Marktlokation the
run was correlated on and an Art. 17 erasure destroys exactly one person's pile;
**two carry a literal** because their subject genuinely is the operator itself
(§ 7a Abs. 5 EnWG parity posture, per-PID KPI history).

A lint holds the line — a literal subject is refused unless it is one of those
two operator-wide scopes, and a binding to a correlation namespace the labeller
does not produce is refused too. A binding that cannot resolve **fails the run**
rather than falling back: a memory filed under the wrong scope is worse than no
memory. Formation reads the agent's own answer, which is model output and
therefore untrusted, so every remembering specialist declares a **quarantined**
model to do it.

## Which revision is running

`GET /api/v1/agents` and `/api/v1/agents/catalog` report each specialist's
`version` and the **digest over its canonical manifest** — the same identity
agentplane records on every admitted run, read off the running process so a
reviewer can check it against the file they approved.

A manifest that cannot produce a digest is recorded upstream as an *absent*
identity rather than a false one, which would leave the run journaled with
nothing saying what governed it. A test asserts every embedded manifest names
itself, and that no two share a digest.

## What an operator can see

`GET /metrics` serves agentplane's whole instrument catalogue, bridged onto
mako's Prometheus registry by a `tracing` layer in
[`plane::metrics`](src/plane/metrics.rs). The crate emits each instrument as a
`tracing` event on its own target and picks no exporter — right for a library,
and a job for the embedder.

| Series | What it answers |
|---|---|
| `agentd_agentplane_runs_total{dim=…}` | runs by terminal status — `failed`, `suspended`, `quarantined` in one query |
| `agentd_agentplane_policy_denials_total{dim=…}` | an agent asking for something new, by action |
| `agentd_agentplane_budget_refusals_total{dim=…}` | work refused by a ceiling before it started, by which ceiling |
| `agentd_agentplane_deadlines_breached_total` | regulatory windows that closed unmet, counted when they closed |
| `agentd_agentplane_quarantines_total` | outcomes nobody could determine — never self-healing |
| `agentd_agentplane_cases_open`, `…_oldest_age` | the backlog, and whether it is quietly ageing |
| `agentd_agentplane_tasks_open` | decisions waiting on a human |

Names are derived from the catalogue, so an instrument added upstream appears
without an edit. Durations are deliberately absent: they come from the spans the
runtime emits, computed by the collector, which can also exclude replays.

One constraint: the events are emitted at `info` under a global `EnvFilter`, so a
deployment running at `warn` silences them and the series flatline rather than
disappear. Admit the target explicitly:
`LOG_LEVEL="warn,agentplane.metric=info"`.

## Build-time guards

| Guard | Refuses |
|---|---|
| `cargo xtask check-tool-grants` | A grant naming a tool no server declares; a `mutates` flag disagreeing with the server's `read_only_hint`; a mutating grant on a `tool-calling` agent |
| `cargo xtask check-prompt-tools` | A procedure instructing a call the manifest never granted — **and** a grant no procedure step mentions |
| `cargo xtask check-wire-timestamps` | A `time` value reaching a JSON wire as its component array |
| `plane::` unit tests | An unsubscribed specialist, an answer-schema object that names its fields and accepts others, **a finding code the procedure emits and the schema cannot carry**, a customer-pooling memory subject, a terminal finding no triage rule reports, a role the policy set does not admit, an oversight verb the policy set grants to nobody, a triage row that expires instead of escalating, a manifest on disk that nobody embedded, a manifest with no schema modeline, a model id nobody reviewed, a declaration that cannot name itself, an audience belonging to no known arm |
| `plane::` unit tests, role builds | A specialist handing a finding to a desk in **another Marktrolle's arm** (§§ 6a, 7a EnWG) — run by `just smoke-roles` against `role-lf`, `role-nb` and `role-msb`, where the compiled set *is* the arm |

The prompt guard is the least obvious, and it runs in both directions because
each direction fails differently.

**A call the agent cannot make.** agentplane reports an unknown tool name back
to the model as a failed call rather than ending the run, so a procedure naming
an ungranted tool does not crash — the model asks, is refused, improvises, and
the step silently does not happen.

**A grant no step mentions.** The quieter direction, and the one that keeps a
specialist from holding a server's whole read surface: unreviewed reach no test
can see, a model choosing between seventeen marktd tools where its procedure
needs two, and a § 6a EnWG data boundary drawn wider than anything asked for. A
grant the procedure never names is dropped, or the procedure says when the model
should reach for it: 150 grants across 28 manifests.

## Tests

Ten suites run on real stores with `FakeProvider`, so the agent layer is tested
the way the engine workflows are — deterministically, and for free.

| Suite | What it pins |
|---|---|
| `plane_golden_run.rs` | The golden run and its **strict replay**, asserted with `assert_replay_was_not_backstopped()`; that the model is asked with the manifest's own procedure; where a step input lands; that a key ring seals personal data |
| `oversight.rs` | A missing final ALOCAT opens exactly one urgent gas-operations task; malformed events open none; ineligible roles cannot decide it |
| `regulatory.rs` | Only the emitted GaBi event routes; deterministic triage has no model or tool authority; authority fields require semantic validation |
| `durability.rs` | A journal append that commits while the caller sees an error duplicates no effect — model not re-asked, tool not re-dispatched, no second attempt recorded |
| `specialist_smoke.rs` | **Every** specialist completes a run, answered from its own `output.schema` |
| `procedure_contract.rs` | **What the model is actually asked**, for every specialist: the declared model answers, the tool surface offered is *exactly* the grants in their wire spelling, the requested schema is the manifest's, the procedure reaches the prompt, and the turn cannot ask for more output than the budget allows |
| `deadline_triage.rs` | The coded specialist's `BREACH` opens a worklist row on the MaKo desk — urgent, bounded by an obligation the calendar resolved, widening rather than expiring — and an *approaching* Frist opens none |
| `evidence.rs` | Records verify under the workload's public key with `require_signature` **on**, a different key rejects them, an unattested plane fails honestly, the checkpoint carries mako's own origin, and a failed run's delivery carries its `reason` while a successful one omits the field |
| `ingest.rs` | A redelivery is answered with the first call's run **the instant that call returns** — the observable consequence of admitting before acknowledging — and a refusal leaves the key free for a corrected retry |
| `plane::` unit tests | Routing, labelling, policy, manifest invariants |

## Configuration

```toml
# agentd.toml
#
# Every top-level key comes first. TOML binds a bare key to the most recent
# table header, so anything written after `[policy]` would be read as
# `policy.<key>` — which the parser refuses, because every config type here is
# `deny_unknown_fields`.
tenant           = "9900357000004"
public_base_url  = "https://agentd.internal:9580"   # what an A2A card advertises
session_timeout_secs = 300   # bounds a *manual* run's wait; /webhook waits for nothing
sweep_interval_secs  = 60    # warns, breaches, escalates, expires, wakes, recovers

mcp_api_key         = "env:AGENTD_MCP_API_KEY"            # SecretString — never logged
inbound_hmac_secret = "env:AGENTD_INBOUND_HMAC_SECRET"

# Where a completed run's decision is delivered, durably. Standard Webhooks,
# the same scheme every other mako outbound carries, so a receiver verifies an
# agentd delivery with `mako_service::webhook::verify_request` like any other.
audit_webhook_url = "https://erp.example/hooks/agent-decisions"
audit_hmac_secret = "env:AGENTD_AUDIT_HMAC"
# Mid-rotation only: the retiring key, presented *beside* the current one so a
# receiver holding either verifies. Remove once every receiver has the new key.
# Setting it alone is a startup failure — "also" with no primary key signs nothing.
# audit_hmac_secret_previous = "env:AGENTD_AUDIT_HMAC_OLD"

# How long an admission key is kept. Absent means forever, which is the only
# setting that cannot admit a duplicate on a timer. 30 days is sized for an
# operator replaying a dead-lettered delivery, not for the retry schedule.
# admission_retention_days = 30

# ── Back-pressure ─────────────────────────────────────────────────────────────
# Per-tenant, reserved in the store at admission — so it holds across instances
# and survives a restart — a per-process counter fails open on scale-out.
# A slot is released when a run seals, fails or *suspends*, so a hundred runs
# parked on approvals do not stop new work. Absent means unbounded; every run is
# still held to its manifest's mandatory `budgets`.
# Exceeding it answers /webhook with 429, which an at-least-once emitter retries.
[quota]
# max_concurrent_runs  = 64
# max_tokens_per_month = 200_000_000

# ── Who wrote each record ─────────────────────────────────────────────────────
# Without this the chain says *what happened* and nothing says *which workload
# wrote it*, and no checkpoint can be submitted to a witness. The key comes from
# you and cannot be minted here: a plane that generated its own identity would
# produce records that look attested and prove nothing.
# `openssl rand -base64 32` produces a seed. Publish the public half.
# [attestation]
# required = true
# key_id   = "spiffe://mako/agentd"
# seed     = "env:AGENTD_SIGNING_SEED"

# ── Somebody else who saw the log grow ────────────────────────────────────────
# A hash chain proves nobody edited *this* history. It cannot show that a second
# history does not exist, because both can be internally perfect and you control
# every input to that check. A witness cosigns a checkpoint only when it
# provably extends the last one it saw, so two histories cannot both be cosigned.
# Requires [attestation]. A witness you host yourself proves nothing about you.
# [witness]
# quorum        = 1     # zero is refused; above the witness count is refused
# interval_secs = 3600
# [[witness.witnesses]]
# name       = "witness.example.org"
# url        = "https://witness.example.org"
# public_key = "..."    # 32 bytes, standard base64 — without it a cosignature
#                       # is a 200 with a base64 string in it

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
api_key = "env:ANTHROPIC_API_KEY"

# [providers.local]
# backend  = "chat-completions"
# api_base = "http://vllm:8000/v1"  # required: there is no default for your server

# Which specialists this deployment runs. A name matching no compiled
# specialist is a startup failure, not an inactive agent.
[bundled_agents]
enable_all = true
# enable = ["mako-agent", "billing-anomaly-agent"]

# Required for the oversight surface: no identity, no worklist. It also supplies
# the roles `api:run.start` and `api:agent.list` are decided from, so without it
# agentd's own doors accept every request and say so at startup.
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

# A key may not contain `-` (agentplane reserves hyphens so the model-facing
# wire rendering stays injective), so a hyphenated service is keyed with an
# underscore.
[mcp_servers]
makod       = "http://makod:8080/mcp"
marktd      = "http://marktd:8180/mcp"
processd    = "http://processd:8580/mcp"
obsd        = "http://obsd:8480/mcp"
billingd    = "http://billingd:9280/mcp"
invoicd     = "http://invoicd:8280/mcp"
accountingd = "http://accountingd:9380/mcp"
edmd        = "http://edmd:8380/mcp"
einsd       = "http://einsd:9180/mcp"
netzbilanzd = "http://netzbilanzd:8680/mcp"
portald     = "http://portald:9480/mcp"
sperrd      = "http://sperrd:8780/mcp"
productd    = "http://productd:9080/mcp"
vertragd    = "http://vertragd:9780/mcp"
mabis_syncd = "http://mabis-syncd:8880/mcp"
```

## Operating notes

**Durability instead of retries.** A failed run is not a message with nowhere to
go: its effects are journaled, so it resumes from the last completed effect
rather than being replayed from the top. There is no dead-letter queue.

**Readiness is the journal, not a constant.** agentd has no `[database]`, so the
runner's built-in ping covers nothing here. `/health/ready` reads a checkpoint
instead — the cheapest call that proves the store is reachable and answering,
touching no run and writing nothing — under a two-second ceiling, and reports
*not ready* before the plane is built. It matters for the topology the Postgres
backend exists for: instances share a store that can go away *after* startup, and
an instance in rotation failing every admission advertises a capacity it does not
have.

**A `202` means the message will be acted on.** Admission commits *inside the
request* — the policy gate, the quota reservation, the case binding and the claim
on the admission key — and only then does `POST /webhook` answer, with the run
ids in the body. The work continues afterwards and is durable by a different
mechanism: a run holds a lease, and one that lapses without release is taken over
and resumed by the sweeper's recovery pass. Every CloudEvent ingest door in mako
completes its work before it answers, for the same reason: an acknowledgement
that returns before anything durable is written turns a deploy into a lost event.

`POST /api/v1/run` is the door that *waits*, because an operator asked for an
answer. `session_timeout_secs` bounds that wait and nothing else.

**The status code is a retry instruction.** mako's emitter treats 429 and 5xx as
transient and every other 4xx as permanent, so the code chosen here decides
whether a message is resent or dead-lettered:

| Answer | When | What the emitter does |
|---|---|---|
| `202` | at least one specialist admitted | done — a retry meets the runs already holding its keys |
| `204` | nothing subscribes to this type | done |
| `429` | nothing admitted, something transient (a quota ceiling) | resends |
| `422` | nothing admitted and resending cannot help — no subscribing specialist can act on this payload | dead-letters **now**, where an operator sees it |

Unknown refusals count as transient. `RuntimeError` is `#[non_exhaustive]`, and
the asymmetry is deliberate: losing a market message is not recoverable, while
spending five attempts on one that was never going to succeed is.

**Admission is at-most-once, in the store.** Inbound delivery is at-least-once,
so `POST /webhook` admits each run under a key built from the CloudEvent's
`(source, id)` and the specialist's name, claimed **inside the transaction that
appends the run's first record**. A redelivery is answered with the run it
already started — across instances and across a restart — and a refused
admission spends no key, so a corrected redelivery is still admitted. The key is
per specialist because one event fans out to several independent runs. An event
missing `id`, `source` or `type` is `400`, not a default: an unset attribute
arrives as `""`, which is a perfectly good admission key.

`POST /api/v1/run` takes the same path and honours an `Idempotency-Key`;
without one, every request is its own event.

Retiring a key reopens the door it closed, so nothing retires one by default.
`admission_retention_days` turns on a daily pass; 30 days is sized for an
operator replaying a dead letter, not for the emitter's retry schedule.

**What the ERP receives.** `de.agent.decision.made` is projected from the run's
sealed record: `{run, case, outcome, chain_head}` under a `tenantid` extension.
The answer is deliberately not in it — a run's output is domain data with a
label on it, and shipping it by default would be an egress decision made by a
default. A **failed** run additionally carries `reason` — the refusal in the runtime's own
words, absent rather than `null` on a success. `GET /api/v1/oversight/runs/{run}`
is where an operator reads the reasoning behind either, and there is deliberately
no second in-memory view beside it: a per-process ring buffer is lost on a restart
and holds only what one instance handled.

**Fan-out.** Several specialists may subscribe to one event; they are
independent opinions, so each gets its own run and its own journal. There is
deliberately no first-wins mode — cancelling a losing branch can leave a started
effect with no terminal record, which for a mutating call is an unrecoverable
unknown outcome.

**The case is the erasure unit.** Every run is admitted with correlation keys
from the event's re-validated identifiers, so all runs about one Marktlokation
share one case. With a key ring configured, everything the plane writes is
sealed under that case's wrapping key: a GDPR Art. 17 request is answered by
destroying one key — live store, replicas and backups at once — while the hash
chain still verifies. Without a key ring the plane starts and says so loudly;
`[keyring] required = true` turns that into a refusal to start.

`POST /api/v1/erasure` operationalizes the two erasure scopes. Supply `case_id`
to destroy the wrapping key, `memory_subject` to forget durable memories, or
both, plus a mandatory `reason`. Case erasure refuses without `[keyring]`;
deleting an index row cannot substitute for crypto-shredding.

**Who wrote each record.** `[attestation]` signs every journal record under the
workload identity, on the store and on the runtime from one key — so an auditor
holding the public half can tell mako's plane from anything else that reached the
same database. The key comes from the deployment and cannot be minted here: a
plane that generated its own identity would produce records that look attested
and prove nothing. Unset, the plane starts and warns; `required = true` refuses.
History written unattested stays unattested.

**Somebody else who saw the log grow.** Everything above protects the record from
*edits*. None of it detects showing a different history to each auditor, because
both can be internally perfect and we control every input to that check.
`[witness]` submits each checkpoint over C2SP `tlog-witness` to parties that
cosign only a history that provably extends the one they last saw. It runs off
the run path — evidence gathered after sealing, never a dependency of
availability — and reports an integrity refusal *even when the quorum was met*,
because the other cosigners may never have seen the history the refusing one
remembers. A witness you host yourself proves nothing about you.

**Verifying the journal offline.** The chains, signatures and Merkle checkpoints
are only *checkable* — and the checker must not be the plane itself, or the party
under examination is the party running the examination. So it is an operational
procedure, not a route, and it runs against a **copy** with the `agentplane`
CLI:

| Verb | What it answers |
|---|---|
| `export` | *here it is* — JSON Lines, self-describing header, a trailer naming any run it could not read |
| `verify` | recompute an export and check it against its own checkpoint |
| `audit` | are the chains sound, who signed them, and — only with a **prior checkpoint from outside** — has anything been removed |
| `restore` | rebuild a store from an export and prove it by that checkpoint |
| `drill` | against live stores: are the blob bytes intact and can sealed case state still be opened — telling erasure-by-design from loss |
| `replay --strict` | re-execute a run against its journal |

Keep that prior checkpoint somewhere the operator cannot rewrite; without one,
`audit` can say the history is internally sound and nothing about whether it is
complete.

## Specialists

Each row is a subscription paired with a manifest. The capability is what a run
is addressed to; the reach is every tool it may call, and every one of them is a
tool its own procedure names.

| Specialist | Capability | Shape | Reach | Subscribes to |
|---|---|---|---|---|
| `mako-agent` | `mako` | `tool-calling` | 7 on makod, marktd, obsd | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.mako.aperak.*` |
| `deadline-alert-agent` | `deadline.alert` | **coded skill** (no model) | 1 on obsd | `de.mako.process.failed`, `de.mako.aperak.timeout`, `de.obs.deadline.approaching` |
| `billing-agent` | `billing` | `tool-calling` | 9 on accountingd, billingd, invoicd, marktd | `de.invoic.receipt.disputed`, `de.accounting.mahnung.issued` |
| `netzbilanz-agent` | `netzbilanz` | `tool-calling` | 7 on marktd, netzbilanzd | `de.netzbilanz.invoic.drafted`, `de.netzbilanz.invoic.dispatched`, `de.netzbilanz.invoic.dispatch-overdue`, `de.netzbilanz.invoic.disputed` |
| `invoice-reconciliation-agent` | `invoice.reconciliation` | `tool-calling` | 6 on accountingd, billingd, invoicd | `de.invoic.payment.overdue`, `de.invoic.receipt.*` |
| `billing-anomaly-agent` | `billing.anomaly` | `tool-calling` | 6 on billingd, edmd | `de.billing.rechnung.erstellt` |
| `billing-regulatory-guard-agent` | `billing.regulatory.guard` | `tool-calling` | 5 on billingd, marktd, productd, vertragd | `de.billing.rechnung.erstellt` |
| `jahresabrechnung-agent` | `jahresabrechnung` | `tool-calling` | 7 on accountingd, edmd, marktd, productd | _manual / scheduled_ |
| `eeg-agent` | `eeg` | `tool-calling` | 4 on edmd, einsd, productd | `de.eeg.anlage.foerderung-auslaufend`, `de.messwert.reading.direct.stored` |
| `eeg-compliance-agent` | `eeg.compliance` | `tool-calling` | 5 on einsd | `de.eeg.anlage.*`, `de.eeg.verguetung.*`, `de.eeg.marktpraemie.*`, `de.eeg.compliance.*`, `de.eeg.veraeusserungsform.gewechselt` |
| `payment-reconciliation-agent` | `payment.reconciliation` | `tool-calling` | 4 on accountingd | `de.accounting.payment.due`, `de.accounting.bankruecklast`, `de.accounting.sepa.collection-rejected`, `de.accounting.sperrandrohung`, `de.accounting.sperrankuendigung`, `de.accounting.abwendung.angeboten`, `de.accounting.abwendung.gebrochen` |
| `compliance-agent` | `compliance` | `tool-calling` | 5 on obsd, processd | `de.obs.stp.parity.alert` |
| `msb-history-agent` | `msb.history` | `tool-calling` | 6 on edmd, marktd | `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.completed`, `de.messwert.reading.order.failed`, `de.messwert.reading.delivery.overdue` |
| `meter-data-agent` | `meter.data` | `tool-calling` | 3 on edmd | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `grid-anomaly-agent` | `grid.anomaly` | `tool-calling` | 3 on marktd | `de.markt.nb-contract.updated`, `de.markt.malo.updated` |
| `tariff-optimization-agent` | `tariff.optimization` | `tool-calling` | 6 on billingd, edmd, productd | `de.billing.rechnung.erstellt`, `de.mako.process.completed` |
| `vertragd-agent` | `vertragd` | `tool-calling` | 9 on marktd, processd, productd, vertragd | `de.vertrag.*`, `de.mako.aperak.rejected`, `de.mako.process.failed`, `de.vertrag.ablauf.ankuendigung`, `de.vertrag.preisaenderung.ankuendigung` |
| `productd-agent` | `productd` | `tool-calling` | 6 on productd | `de.tarif.product.updated`, `de.tarif.angebot.abgelaufen`, `de.tarif.epex.missing` |
| `processd-agent` | `processd` | `tool-calling` | 7 on marktd, processd | `de.mako.process.initiated`, `de.mako.aperak.rejected`, `de.mako.process.failed` |
| `sperrd-agent` | `sperrd` | `tool-calling` | 6 on makod, sperrd | `de.accounting.sperrauftrag`, `de.sperr.*`, `de.mako.process.completed` |
| `portald-agent` | `portald` | `tool-calling` | 7 on accountingd, billingd, einsd, portald | `de.billing.rechnung.erstellt`, `de.eeg.anlage.foerderung-auslaufend`, `de.accounting.mahnung.issued`, `de.vertrag.*` |
| `regulatory-reporting-agent` | `regulatory.reporting` | `tool-calling` | 5 on obsd, processd | _manual / scheduled_ |
| `replacement-value-agent` | `replacement.value` | `tool-calling` | 3 on edmd | `de.messwert.reading.quality.warning`, `de.mako.process.completed` |
| `mabis-syncd-agent` | `mabis.syncd` | `tool-calling` | 7 on edmd, mabis_syncd | `de.mabis.submission.failed`, `de.mabis.korrekturbedarf.opened`, `de.messwert.reading.quality.warning` |
| `smgw-diagnostics-agent` | `smgw.diagnostics` | `tool-calling` | 5 on edmd, marktd | `de.messwert.cls.compliance-issue`, `de.messwert.smgw.cert.expiry-warning`, `de.messwert.reading.quality.warning`, `de.messwert.reading.direct.stored`, `de.mako.process.initiated`, `de.markt.geraet.konfiguration.updated` |
| `vpp-billing-agent` | `vpp.billing` | `tool-calling` | 3 on billingd, marktd | `de.vpp.dispatch.confirmed`, `de.vpp.settlement.berechnet` |
| `gabi-gas-agent` | `gabi.gas.balancing` | coded | 0 | `de.gabi.alocat.missing` |
| `einsd-batch-agent` | `einsd.batch` | `tool-calling` | 8 on edmd, einsd, productd | `de.eeg.settlement.batch-due`, `de.eeg.compliance.*`, `de.eeg.anlage.foerderung-auslaufend` |
Two specialists carry no subscription on purpose: `jahresabrechnung-agent` and
`regulatory-reporting-agent` are batch shapes an operator or scheduler starts,
because no CloudEvent marks "the reporting period ended". Anything else without
a trigger fails the build.

See the [operator guide](https://hupe1980.github.io/mako/docs/services/agentd/)
for the architecture diagrams, role scoping, the erasure model and the full
configuration reference.
