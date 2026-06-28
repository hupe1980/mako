---
layout: default
title: ERP Integration
nav_order: 23
parent: Architecture
description: >
  Integrate makod with your ERP using BO4E JSON webhooks. Command API,
  HMAC-SHA256 signature verification, idempotency keys, and BO4E payload
  schema reference.
---

# ERP Integration

`makod` is a protocol processor, not a business system. It handles EDIFACT
parsing, BDEW process rules, AS4 delivery, and regulatory deadlines. All
contract data, billing logic, and master data live in your ERP.

The integration contract between the two is **BO4E** — not raw EDIFACT. Your
ERP never sees EDIFACT format versions or segment codes. When BDEW releases a
new format version (`FV2026-10-01`), the BO4E payload your ERP receives is
unchanged.

```
ERP  ←─────────── BO4E JSON ───────────→  makod
                (ErpAdapter / POST /api/v1/commands)
                        ↕
              EDIFACT / AS4 / BDEW network
```

---

## Quick-start: wire the ERP webhook in 5 minutes

This is the minimum configuration to get outbound ERP notifications working.
`makod` will POST a JSON event to your ERP endpoint every time a MaKo process
reaches a significant state (APERAK received, process completed, etc.).

**Step 1 — Generate a shared secret**

```bash
openssl rand -hex 32
# → e.g. a3f8c1d2...  (64 hex chars)
```

**Step 2 — Start makod with the webhook configured**

```bash
makod \
  --data-dir /var/lib/makod \
  --tenant-id 9900357000004 \
  --erp-webhook-url https://erp.example.com/mako/events \
  --erp-webhook-secret a3f8c1d2...
```

Or via `makod.toml`:

```toml
[erp]
webhook_url    = "https://erp.example.com/mako/events"
webhook_secret = "a3f8c1d2..."
```

**Step 3 — Implement the ERP endpoint**

Your ERP must accept `POST` requests at the configured URL:

```
POST /mako/events
Content-Type: application/json
X-Idempotency-Key: 01932a4f-7b3e-4c5d-8f6a-9e0b1c2d3e4f
X-Mako-Signature: <hmac-sha256-hex>
```

Body:

```json
{
  "idempotency_key": "01932a4f-7b3e-4c5d-8f6a-9e0b1c2d3e4f",
  "event_type": "aperak_accepted",
  "process_id": "018f3a2b-...",
  "tenant_id": "9900357000004",
  "conversation_id": "...",
  "causation_id": "...",
  "pid": 55001,
  "payload_schema": "https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202501.0.0/src/bo4e_schemas/bo/Marktlokation.json",
  "payload": {
    "_typ": "MARKTLOKATION",
    "_version": "202501",
    "marktlokationsId": "51238696782",
    "sparte": "STROM",
    "bilanzierungsmethode": "SLP",
    "energierichtung": "VERBRAUCH",
    "netzbetreibercodenr": "9900357000004"
  },
  "occurred_at": "2026-10-01T10:15:00+02:00"
}
```

**Step 4 — Verify the signature**

Compute HMAC-SHA256 over the raw request body using your shared secret and
compare with the `X-Mako-Signature` header (64-char lowercase hex):

```python
import hmac, hashlib

def verify_mako_signature(body: bytes, secret: str, header: str) -> bool:
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

```typescript
import { createHmac, timingSafeEqual } from "crypto";

function verifyMakoSignature(body: Buffer, secret: string, header: string): boolean {
  const expected = createHmac("sha256", secret).update(body).digest("hex");
  return timingSafeEqual(Buffer.from(expected), Buffer.from(header));
}
```

**Step 5 — Return `HTTP 200` for duplicates**

`makod` retries on any non-2xx response. Your endpoint **must** persist
`idempotency_key` and return `200 OK` for duplicate deliveries without
re-processing. Any `4xx` except `429` is treated as a permanent error and the
message is dead-lettered.

---

## Integration surfaces

| Direction | Mechanism | Description |
|-----------|-----------|-------------|
| makod → ERP | `--erp-webhook-url` / `WebhookErpAdapter` | POST BO4E JSON on every process event |
| ERP → makod | `POST /api/v1/commands` | Initiate a MaKo process (Lieferbeginn, Gerätewechsel, …) |
| ERP → makod | `PUT /admin/malo/{malo_id}` | Push MaLo master data to the local cache |
| ERP → makod | `PUT /admin/partners/{gln}` | Register or update a trading-partner endpoint |
| ERP → makod | `ErpCommandSource` trait | Fully event-driven inbound (Kafka, SFTP, CDC, …) |

---

## Outbound: makod notifies the ERP

### Delivery pipeline

```
Workflow::handle()
    └── WorkflowOutput { events, outbox_messages }
                │
                ▼ (single atomic WriteBatch — SSI-isolated)
        EventStore  +  OutboxStore
                │
                ▼ (OutboxErpWorker polls every 5 s)
        ERP notifications → WebhookErpAdapter::notify(ErpEvent { payload: BO4E })
                │
                ▼ (OutboxWorker polls every 5 s, separate)
        EDIFACT messages  → AS4 sender → BDEW counterparty
```

Events and outbox entries are written atomically. If `makod` crashes between
the two writes, the event is replayed on restart and the outbox entry is
re-enqueued — no lost APERAK.

### Event types

| `event_type` | Trigger | Primary BO4E payload |
|---|---|---|
| `process_initiated` | New inbound UTILMD received | `Marktlokation` |
| `aperak_accepted` | Counterparty accepted our UTILMD | `Marktlokation` |
| `aperak_rejected` | Counterparty rejected our UTILMD | `Marktlokation` + rejection reason |
| `aperak_timeout` | No APERAK within regulatory SLA | `Marktlokation` |
| `contrl_received` | CONTRL syntax acknowledgement | — (no payload) |
| `process_completed` | Lieferbeginn/Lieferende confirmed | `Marktlokation` + `Vertrag` |
| `process_failed` | Fatal error / regulatory deadline exceeded | `Marktlokation` |
| `malo_identified` | MaLo-ID lookup resolved | `Marktlokation` |

### PID → event mapping

| PID family | Process | ERP event sequence |
|---|---|---|
| GPKE 55001 | Lieferbeginn LF-AN | `process_initiated` → `aperak_accepted` → `process_completed` |
| GPKE 55002 | Lieferbeginn NB-AN | same |
| GPKE 55017 | Lieferbeginn Konfiguration | same |
| GPKE 31001–31008 | Abrechnung INVOIC | `process_initiated` → `process_completed` or `process_failed` |
| GPKE 56001–56004 | Einspeisestelle (ex-MPES) | same as 55001 |
| WiM 11001–11099 | Gerätewechsel / MSB-Wechsel | `process_initiated` → `aperak_accepted` → `process_completed` |
| GeLi Gas 44001–44006 | Lieferbeginn Gas | `process_initiated` → `aperak_accepted` → `process_completed` |
| GeLi Gas 44017–44018 | Lieferende / Konfiguration Gas | same |
| GeLi Gas 44555 | Sperrung Gas | same |
| MABIS 13003 | Bilanzkreisabrechnung Strom | `process_initiated` → `process_completed` or `process_failed` |

### Request format

```
POST <erp_webhook_url>
Content-Type: application/json
X-Idempotency-Key: <event.idempotency_key>
X-Mako-Signature: <hmac-sha256-hex>   ← only when --erp-webhook-secret is set
```

Body is `ErpEvent` serialised as compact JSON (no pretty-print).

### `ErpEvent` schema

```typescript
interface ErpEvent {
  idempotency_key: string;        // stable dedup key — store in ERP
  event_type: string;             // see event types table above
  process_id: string;             // UUID of the mako process
  tenant_id: string;              // operator GLN / EIC
  conversation_id: string;        // BDEW Vorgangsnummer
  causation_id: string;           // mako domain event that triggered this
  pid: number;                    // Prüfidentifikator
  payload_schema?: string;        // BO4E JSON Schema URL
  payload: object;                // BO4E-typed JSON object
  occurred_at: string;            // ISO 8601 with timezone offset
}
```

### Retry and back-off

`makod` retries failed webhook deliveries with **exponential back-off**:

| Attempt | Delay |
|---|---|
| 1st failure | 5 min |
| 2nd failure | 10 min |
| 3rd failure | 20 min |
| 4th failure | 40 min |
| 5th+ failure | 60 min (capped) |
| After 10 failures | Dead-lettered; `WARN` logged |

HTTP response codes:

| Code | Interpretation |
|---|---|
| `2xx` | Success — message acknowledged |
| `4xx` except `429` | Permanent error — message dead-lettered immediately |
| `429` | Transient — rescheduled with back-off |
| `5xx` | Transient — rescheduled with back-off |
| Network timeout / error | Transient — rescheduled with back-off |

### Signature verification

When `--erp-webhook-secret` is set, every POST includes:

```
X-Mako-Signature: <lowercase-hex HMAC-SHA256 of raw request body>
```

The key is the UTF-8 encoding of the shared secret. **Always use a
constant-time comparison** (e.g. `hmac.compare_digest` in Python,
`crypto.timingSafeEqual` in Node.js) to prevent timing attacks.

### No-secret mode

If `--erp-webhook-secret` is omitted, no `X-Mako-Signature` header is sent.
**Do not use no-secret mode in production.** Use it only in local development
with loopback-only ERP endpoints.

### `LogErpAdapter` (development / logging only)

When `--erp-webhook-url` is not set, `makod` falls back to `LogErpAdapter`
which emits every event at `INFO` level. Useful for verifying event flow
during development without a running ERP.

```
INFO mako::erp: ErpAdapter: event logged (no delivery configured)
    idempotency_key=01932a4f-...
    event_type=aperak_accepted
    process_id=018f3a2b-...
    pid=55001
```

---

## Inbound: ERP initiates a MaKo process

### REST (`POST /api/v1/commands`)

Submit a BO4E business object to trigger a MaKo process. `makod` resolves the
correct PID from the object type and process context.

```http
POST /api/v1/commands
Content-Type: application/json
Idempotency-Key: erp-order-991234
Authorization: Bearer <token>

{
  "_typ": "VERTRAG",
  "_version": "202501",
  "vertragsbeginn": "2026-10-01T00:00:00+02:00",
  "sparte": "STROM",
  "vertragsart": "ENERGIELIEFERVERTRAG",
  "marktrolle": "LIEFERANT",
  "vertragspartner1": {
    "_typ": "MARKTTEILNEHMER",
    "rollencodenummer": "9900357000004",
    "rollencodetyp": "GLN",
    "marktrolle": "NETZBETREIBER"
  },
  "vertragsteile": [
    {
      "_typ": "VERTRAGSTEIL",
      "lokation": "51238696782",
      "vertragsteilbeginn": "2026-10-01T00:00:00+02:00"
    }
  ]
}
```

**Response:**

```json
{ "process_id": "018f3a2b-...", "stream_id": "gpke/9900357000004/..." }
```

The `Idempotency-Key` header is forwarded to `InboxStore::accept` — duplicate
submissions within the AS4 72-hour dedup window return the same `process_id`
without re-executing.

**BO4E `_typ` → PID mapping:**

| BO4E `_typ` | `marktrolle` / context | PID family |
|---|---|---|
| `VERTRAG` (Beginn, Strom) | `LIEFERANT` | GPKE 55001 |
| `VERTRAG` (Ende, Strom) | `LIEFERANT` | GPKE 55003 |
| `VERTRAG` (Beginn, Gas) | `LIEFERANT` | GeLi Gas 44001 |
| `ZAEHLER` (Gerätewechsel) | — | WiM 11001 |
| `RECHNUNG` | `BKV` | MABIS 13003 |

### Event-driven inbound (`ErpCommandSource`)

For ERP systems with a message bus, implement `ErpCommandSource` to feed BO4E
business objects into the engine without polling:

```rust
pub trait ErpCommandSource: Send + Sync + 'static {
    async fn next(&self) -> Result<Option<InboundErpCommand>, ErpAdapterError>;
    async fn ack(&self, id: &str) -> Result<(), ErpAdapterError>;
    async fn nack(&self, id: &str, reason: &str) -> Result<(), ErpAdapterError>;
}
```

Register at startup:

```rust
EngineBuilder::new()
    .with_erp_command_source(Arc::new(MyKafkaSource::new(&config)))
    .build()
```

---

## MaLo master data cache

`makod` answers `POST /maloId/request/v1` (BDEW API-Webdienste Strom) from a
local cache. The ERP is the authoritative master — keep the cache current.

### Upsert a MaLo

```http
PUT /admin/malo/51238696782
Authorization: Bearer <token>
Content-Type: application/json

{
  "malo_id": "51238696782",
  "metering_point_operator": "9904357000003",
  "grid_operator": "9900357000004",
  "network_area": "DE-NET-001",
  "address": {
    "street": "Musterstraße", "house_number": "42",
    "postal_code": "10115", "city": "Berlin", "country_code": "DE"
  }
}
```

Trigger this from the ERP on contract activation, address change, and contract
end. Call on every grid assignment change — wrong grid-operator routing is a
common source of APERAK rejections.

### Cache admin

```http
GET    /admin/malo/stats            ← record count + last-upsert timestamp per tenant
DELETE /admin/malo/51238696782      ← remove on contract end
```

---

## Trading-partner directory

```http
PUT /admin/partners/9900000000001
Authorization: Bearer <token>
Content-Type: application/json

{
  "gln": "9900000000001",
  "display_name": "Stadtwerke Beispiel GmbH",
  "channels": [
    { "qualifier": "AK", "address": "https://partner.example/as4/inbox" }
  ],
  "roles": ["NbStrom"]
}
```

Or bulk-import from a PARTIN EDIFACT interchange:

```http
POST /admin/partners/import
Authorization: Bearer <token>
Content-Type: text/plain; charset=utf-8

<raw PARTIN interchange>
```

---

## Writing a custom `ErpAdapter`

If the built-in `WebhookErpAdapter` does not fit (e.g. you need mTLS, a
message-bus sink, or a proprietary ERP SDK), implement the trait directly:

```rust
use mako_engine::erp::{ErpAdapter, ErpAdapterError, ErpEvent};

struct MySapAdapter { client: SapHttpClient }

impl ErpAdapter for MySapAdapter {
    async fn notify(&self, event: ErpEvent) -> Result<(), ErpAdapterError> {
        // Deserialise the BO4E payload into your ERP's model.
        let malo: MyMalo = serde_json::from_value(event.payload)
            .map_err(ErpAdapterError::payload)?;

        // Use idempotency_key to guard against duplicate delivery.
        self.client
            .post_event(&event.idempotency_key, malo.id, event.event_type.label())
            .await
            .map_err(|e| {
                if e.is_retryable() {
                    ErpAdapterError::transport(e)
                } else {
                    ErpAdapterError::permanent(e)
                }
            })
    }
}
```

Wire it in `makod/src/erp_adapter.rs` alongside `WebhookErpAdapter`, or inject
it into a custom `makod` binary.

### Error classification contract

| Return | Worker behaviour |
|---|---|
| `Ok(())` | Acknowledged — removed from outbox |
| `Err(ErpAdapterError::Transport(_))` | Retried with exponential back-off |
| `Err(ErpAdapterError::Permanent(_))` | Dead-lettered immediately |
| `Err(ErpAdapterError::Payload(_))` | Dead-lettered immediately |

---

## Configuration reference

All options can be set via CLI flag, environment variable, or `makod.toml`.

| CLI flag | Env var | TOML key | Default | Description |
|---|---|---|---|---|
| `--erp-webhook-url` | `MAKOD_ERP_WEBHOOK_URL` | `erp.webhook_url` | — | ERP endpoint URL (enables HTTP delivery) |
| `--erp-webhook-secret` | `MAKOD_ERP_WEBHOOK_SECRET` | `erp.webhook_secret` | — | HMAC-SHA256 signing key |

`makod.toml` example:

```toml
[erp]
webhook_url    = "https://erp.internal/mako/events"
webhook_secret = "env:ERP_WEBHOOK_SECRET"   # read from environment at startup
```

---

## Testing

`mako-engine` ships test helpers gated behind `feature = "testing"`:

```toml
[dev-dependencies]
mako-engine = { path = "...", features = ["testing"] }
```

Available types:

| Type | Purpose |
|---|---|
| `NoopErpAdapter` | Succeeds without delivering; use in unit tests |
| `LogErpAdapter` | Logs at INFO; use when you want to see events in test output |
| `NoopErpCommandSource` | Always idle; no inbound commands |

Integration test pattern:

```rust
use mako_engine::erp::NoopErpAdapter;

#[tokio::test]
async fn aperak_accepted_triggers_erp_notification() {
    let store     = InMemoryEventStore::new();
    let outbox    = InMemoryOutboxStore::new();
    let erp       = NoopErpAdapter;

    // Build engine under test.
    let ctx = EngineBuilder::new()
        .with_event_store(store.clone())
        .with_outbox_store(outbox.clone())
        .build();

    // Execute workflow step.
    ctx.execute(tenant, workflow_id, receive_aperak_cmd()).await.unwrap();

    // Assert outbox contains an ERP-targeted message.
    let pending = outbox.pending_now(10).await.unwrap();
    let erp_msg = pending.iter().find(|m| m.payload_schema.is_some()).unwrap();
    assert_eq!(erp_msg.message_type, "AperakAccepted");
}
```

---

## Why BO4E (not EDIFACT)

BO4E (*Business Objects for Energy*, [bo4e.de](https://www.bo4e.de/)) is the
open standard for energy market data models in Germany. Implementations exist
for Python, C#, Go, Kotlin, TypeScript, and PHP — all MIT-licensed.

Without BO4E an ERP adapter must understand `D_7143` segment positions,
maintain identity translation tables, re-implement status code mappings per
vendor, and update on every BDEW format release.

With BO4E:
- `makod` absorbs EDIFACT format changes internally.
- The ERP receives `Marktlokation.marktlokationsId` — already the canonical
  German MaLo ID; no translation table needed.
- `event_type` carries semantic labels (`aperak_accepted`, `process_completed`),
  not raw EDIFACT codes.
- BO4E versioning (`v202501.0.0`) is independent of BDEW format versions.

---

## Implementation status

| Item | Status |
|---|---|
| `ErpAdapter` / `ErpEvent` traits | ✅ Implemented (`mako-engine/src/erp.rs`) |
| `ErpCommandSource` trait | ✅ Implemented (`mako-engine/src/erp.rs`) |
| `WebhookErpAdapter` with HMAC-SHA256 signing | ✅ Implemented (`makod/src/erp_adapter.rs`) |
| `OutboxErpWorker` with exponential back-off | ✅ Implemented (`makod/src/erp_adapter.rs`) |
| `POST /api/v1/commands` REST endpoint | ✅ Implemented (`makod/src/commands_api.rs`) |
| `PUT /admin/malo/{malo_id}` cache | ✅ Implemented |
| `PUT /admin/partners/{gln}` | ✅ Implemented |
| `bo4e-rs` typed Rust crate | 🔲 Planned — currently `serde_json::Value` |

---

## Related documentation

| Topic | File |
|---|---|
| `makod` operator reference | [docs/makod.md](makod.md) |
| Engine architecture | [docs/engine.md](engine.md) |
| API-Webdienste Strom (MaLo-ID) | [docs/api-webdienste.md](api-webdienste.md) |
| Annual release workflow | [docs/annual-release-workflow.md](annual-release-workflow.md) |


