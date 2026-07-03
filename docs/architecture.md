---
layout: default
title: Architecture
nav_order: 4
has_children: true
description: >-
  mako-engine and makod architecture: process runtime, AS4 transport,
  ERP integration via CloudEvents 1.0, and API-Webdienste Strom.
---

# Architecture

This document covers the internal design of `mako-engine` and `makod`: the
event-sourced process runtime, inbound/outbound transport channels, ERP
integration via BO4E + CloudEvents 1.0, and the SlateDB persistence layer.

---

## Design principles

| Principle | Consequence |
|---|---|
| **Protocol processor, not a business system** | `makod` handles EDIFACT, BDEW rules, AS4 delivery, and regulatory deadlines. Contract data and billing logic live in your ERP. |
| **`Workflow::handle` and `Workflow::apply` are pure functions** | All I/O, parsing, and clock access happens at the transport boundary before a command is constructed. This makes processes deterministic, replayable, and trivially testable. |
| **Atomic dual-write** | Events and outbox entries are written in a single `WriteBatch` via `AtomicAppend::append_with_outbox`. There is no two-phase commit, no compensation path for a lost APERAK. |
| **Event sourcing** | State is rebuilt by replaying the append-only event log. Audit trails, bug reproductions, and format-version migrations are a consequence of the model, not bolt-ons. |
| **Format-version coexistence** | `FV2025-10-01` and `FV2026-10-01` coexist in the same running instance. A process started under the old format version continues under those rules until it completes. |

---

## System layers

```
┌─────────────────────────────────────────────────────────────────────┐
│  Transport                                                           │
│  ┌──────────┐  ┌─────────────┐  ┌──────────────────────────────┐   │
│  │ AS4/SOAP │  │ HTTP REST   │  │ BDEW API-Webdienste Strom     │   │
│  │ :4080    │  │ :8080       │  │ :8090                         │   │
│  └────┬─────┘  └──────┬──────┘  └──────────────┬───────────────┘   │
└───────┼───────────────┼──────────────────────────┼──────────────────┘
        │               │                          │
┌───────▼───────────────▼──────────────────────────▼──────────────────┐
│  edi-energy — Parse · Validate · Build                              │
│  Profile registry (MIG + AHB rules) · 17 message types             │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ typed Command
┌───────────────────────────▼─────────────────────────────────────────┐
│  mako-engine — Process Runtime                                      │
│  PidRouter · EngineContext · Process · Workflow (handle / apply)    │
│  DeadlineStore · OutboxStore · EventStore · SnapshotStore           │
└───────┬──────────────────────────────────────────────────────────┬──┘
        │                                                          │
        ▼  events + outbox (single WriteBatch)                     ▼  HTTP POST
┌───────────────────────────────┐         ┌────────────────────────────┐
│  SlateDB (object store)       │         │  ERP system                │
│  e/ events                    │         │  CloudEvents 1.0 + BO4E    │
│  om/ outbox messages          │         │  HMAC-SHA256 signed        │
│  dl/ deadlines                │         │  (application/cloudevents  │
│  pr/ process registry         │         │   +json)                   │
│  pt/ partner directory        │         └────────────────────────────┘
│  ib/ inbox dedup              │
│  sv/ stream versions          │
└───────────────────────────────┘
```

---

## Inbound data flow (AS4 push from BDEW counterparty)

```
BDEW counterparty
    │  AS4/ebMS3 push (SOAP+MTOM over HTTPS)
    ▼
makod/as4_ingest
    │  WSS-verify signature · extract MIME attachment
    ▼
InboxStore::accept     ← 72-hour dedup (prevents double-processing)
    │  raw EDIFACT bytes
    ▼
Platform::parse_interchange (edi-energy)
    │  structured messages, detected PID per message
    ▼
PidRouter::route       ← selects domain module by Prüfidentifikator
    │  workflow_name + PID
    ▼
EdifactIngestDispatcher::dispatch   ← spawns or resumes process by MaLo business key
    │  typed Command (via AdapterRegistry → MessageAdapter)
    ▼
Process::execute_and_enqueue_with_snapshot_and_retry
    ├── replay EventStore → rebuild State   (Workflow::apply — pure)
    ├── Workflow::handle(state, command)     (pure, returns events + outbox)
    └── AtomicAppend::append_with_outbox    (single WriteBatch)
         ├── EventStore  (e/<tenant>/<stream_id>/seq)
         └── OutboxStore (om/<tenant>/<id>)
```

---

## Outbound flows

### AS4 EDIFACT delivery

`OutboxWorker` polls `OutboxStore` every 5 seconds. For each pending message:

1. Render EDIFACT interchange via `edi-energy` builders.
2. Look up trading partner AS4 endpoint in `PartnerStore`.
3. Sign with operator PKCS#12 credential.
4. POST via `asx-rs` AS4 sender.
5. On HTTP 200: delete outbox entry. On 4xx/5xx: back-off and retry.

**Self-addressed messages** (`recipient == tenant_party_id`) bypass the AS4
transport entirely.  `BdewAs4Sender` renders the EDIFACT bytes, re-parses
them via `Platform::parse_interchange`, and passes each message to
`EdifactIngestDispatcher::dispatch` for in-process delivery to the correct
workflow.  See [Integrated operators](./makod.md#integrated-operators-nb--msb-same-gln)
for the full dispatch table and configuration notes.

### ERP CloudEvents delivery

`OutboxErpWorker` polls `OutboxStore` every 5 seconds. For each ERP-targeted message:

1. Build a [CloudEvents 1.0](https://cloudevents.io) envelope from the `ErpEvent`.
2. Set `Content-Type: application/cloudevents+json`.
3. Sign with `HMAC-SHA256` over the raw body (when `--erp-webhook-secret` is set).
4. POST to the configured `--erp-webhook-url`.
5. On `2xx`: acknowledged. On `429`/`5xx`: exponential back-off. On `4xx`: dead-letter immediately.

See [ERP Integration](./erp-integration.md) for the full CloudEvents schema and receiver implementation guide.

### Deadline scheduler

`DeadlineScheduler` ticks every **30 seconds** by default (configurable via
`--deadline-poll-interval-secs`; minimum 1 second). For each due entry in `DeadlineStore`:

1. Reconstruct the `ProcessIdentity` from the deadline record.
2. Dispatch a `TimeoutExpired` command to the workflow.
3. The workflow produces a `DeadlineExpired` event and an `AperakTimeout` outbox entry.
4. The outbox entry routes to `OutboxErpWorker`, which delivers the `de.mako.aperak.timeout` CloudEvent to the ERP.

---

## Domain crate layering

Each domain crate is a thin wrapper that:
- Defines `Command`, `Event`, and `State` enums specific to its regulatory process family.
- Implements `Workflow` with pure `handle` and `apply` functions.
- Registers itself in the `PidRouter` via a `register_*` function called from `makod`.

`makod` wires the domain modules, transport adapters, and the ingest dispatcher
at startup:

```
makod (binary)
├── registers mako-gpke    → PIDs 55001–55024, 55555, 55607–55609, 17115–17117 (Strom NB),
│                            17134/17135, 19001/19002, 31001–31008, 37000–37006
├── registers mako-wim     → PIDs 55039, 55042, 55051, 55168, 31009, 23001/23003/23004/23008,
│                            17001–17011, 19001/19002 (nMSB role), 39000, 35001–35005, 15001–15005
├── registers mako-geli-gas → PIDs 44001–44021, 44022* (Nb role), 44023–44024* (Lf role),
│                             37008–37014, 31011, 17115–17117 (Gas NB)
├── registers mako-mabis   → PID 13003
├── registers mako-wim-gas → PIDs 44022–44024* (Msb/Nmsb role), 44039–44053, 44168–44170,
│                            31003, 31004, 23005, 23009
├── registers mako-redispatch → Redispatch 2.0 XML workflows
│
└── wires EdifactIngestDispatcher
         ├── called by: AS4 inbound (as4_ingest), REST ingest (edifact_api)
         └── called by: AS4 sender loopback (BdewAs4Sender, recipient == own GLN)
```

`*` PIDs 44022–44024 use role-conditional routing:
- `mako-wim-gas` `wim-gas-stornierung`: Msb/Nmsb/all-role deployments
- `mako-geli-gas` `geli-gas-stornierung`: Nb-only (44022 inbound as GNB)
- `mako-geli-gas` `geli-gas-stornierung-lf`: Lf-only (44023/44024 inbound as LFN/LFA)

See [PID Reference](./pid-reference.md) for the complete table.

---

## SlateDB key schema

All state is stored in a single SlateDB column family. Keys are byte-sortable
to enable efficient range scans per tenant and stream.

| Prefix | Content | Key pattern |
|--------|---------|-------------|
| `e/` | Event log | `e/<tenant_id>/<stream_id>/<seq_u64_big_endian>` |
| `sv/` | Stream version (optimistic lock) | `sv/<tenant_id>/<stream_id>` |
| `om/` | Outbox messages | `om/<tenant_id>/<ulid>` |
| `dl/` | Deadlines | `dl/<tenant_id>/<due_timestamp_secs>/<id>` |
| `pr/` | Process registry | `pr/<tenant_id>/<conversation_id>` |
| `pt/` | Partner directory | `pt/<tenant_id>/<gln>` |
| `ib/` | Inbox dedup | `ib/<tenant_id>/<message_ref>` |
| `sn/` | Snapshots | `sn/<tenant_id>/<stream_id>` |

The `dl/` prefix sorts by due timestamp, so `range_scan(prefix, now_key)` is
the entire scheduler implementation.

---

## Testing strategy

| Layer | Test type | Tooling |
|---|---|---|
| EDIFACT parse/validate | Unit + property | `edi-energy` tests, `cargo-fuzz` (1 100+ corpus entries) |
| Workflow logic | Unit (sync) | `InMemoryEventStore`, `InMemoryOutboxStore`, `NoopErpAdapter` |
| End-to-end process flows | Async integration | `mako-engine` integration tests; `makod` e2e AHB conformance test |
| Deadline arithmetic | Unit | `fristen` crate with Germany public holiday fixtures |
| CloudEvents delivery | Integration | `OutboxErpWorker` test with mock HTTP server |
| AS4 inbound routing | Integration | `e2e_ahb_conformance.rs` — real fixture EDIFACT → full pipeline |

---

## Related documentation

| Topic | File |
|---|---|
| Getting started | [getting-started.md](getting-started.md) |
| Engine internals | [engine.md](engine.md) |
| `makod` operator guide | [makod.md](makod.md) |
| ERP integration | [erp-integration.md](erp-integration.md) |
| PID reference | [pid-reference.md](pid-reference.md) |
| Compensation flows | [compensation.md](compensation.md) |
