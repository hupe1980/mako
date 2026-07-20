# processd — Process Decision Engine

**Automated process decisions for German energy market communication (MaKo).**

`processd` consumes `de.mako.process.initiated` CloudEvents from `marktd` and
applies role-specific policy to make decisions within regulatory deadlines —
without ERP involvement for the common cases.

---

## Role overview

| Role | Module | PIDs | Deadline | Decision basis |
|------|--------|------|----------|----------------|
| **NB** | `nb_module` | 55001, 55016, 44001 | GPKE: 24h · GeLi Gas: 10 WT | `netz-checker` (6 checks) |
| **LF** | `lf_module` | 55008 (E_0624) | 45 min | `VersorgungsStatus` from `marktd` |
| **MSB** | `msb_module` | 55039, 55042 (MSB-Wechsel STP) | 5 WT | device/partner checks |
| **MSB** | `msb_module` | 35001–35005 (REQOTE auto-response) | APERAK window | `PreisblattMessung` lookup |
| **MSB/NB** | `handler` | `wim-steuerungsauftrag` | immediate | `konfigurationsprodukte` contract check |

## Features at a glance

| Feature | Detail |
|---------|--------|
| **HTTP port** | `:8580` (default; configure via `--listen`) |
| **Database** | PostgreSQL (SQLx, dynamic queries — no compile-time DATABASE_URL) |
| **Auth** | OIDC/JWT (RS256/ES256/PS256), JWKS background refresh |
| **Authorization** | Cedar ABAC (`policies/processd.cedar`) |
| **NB STP rate target** | ≥ 95 % (requires NIS grid records via `nis-syncd` or manual provisioning) |
| **LF E_0624 window** | 45 min (2700 s) regulatory deadline; entries expire 5 min before |
| **REQOTE auto-response** | Auto-dispatches QUOTES from `PreisblattMessung`; eliminates ERC A97 deadline risk. Disable: `[msb] auto_preisanfrage = false` |
| **§14a Steuerungsauftrag** | Auto-confirms iMS ORDERS when `istFernschaltbar=true` AND `produktcode` is in `konfigurationsprodukte` (BK6-24-174 §4.3) |
| **§20 EnWG parity** | `initiator_is_affiliate` on every `anmeldung_decisions` row |

---

## Cargo features

```text
# §7 EnWG (≥ 100k Netzkunden): must use separate binaries.
# BNetzA audit examines binary SHA — nb-only must not contain LF PIDs.

role-lf-strom  # LFA E_0624 auto-response + LFN Strom bootstrap
role-lf-gas    # LFA GeLi Gas auto-response
role-nb-strom  # GPKE Anmeldung STP (55001, 55016) via netz-checker
role-nb-gas    # GeLi Gas Anmeldung STP (44001) via netz-checker

lf-only        # = role-lf-strom + role-lf-gas
nb-only        # = role-nb-strom + role-nb-gas
integrated     # = all four roles (§6b EnWG combined deployment)
```

---

## NB decision pipeline

```text
de.mako.process.initiated (PID 55001/55016/44001)
  → parse AnmeldungAnfrage from event payload
  → GET marktd /api/v1/versorgung/{malo_id}      → VersorgungsStatus
  → GET marktd /api/v1/malo/{malo_id}/grid        → MaloGridRecord
  → GET marktd /api/v1/partners/{lf_gln}          → partner_known
  → netz_checker::evaluate(anfrage, vs, grid, partner_known, now_utc())
      Accept   → write anmeldung_decisions(Accept) → [if auto_accept] MakodClient bestaetigen
      Reject   → write anmeldung_decisions(Reject, erc_code) → MakodClient ablehnen
      Escalate → write anmeldung_decisions(Escalate) → operator alert
```

### netz-checker — 6 deterministic checks

| # | Rule | Outcome on failure |
|---|------|-------------------|
| 1 | Grid record present in `marktd` | `Escalate` |
| 2 | No conflicting active supply (`lf_mp_id_next` is `None`) | `Reject A06` |
| 3 | `process_date ≥ today_berlin(now)` (no retroactive starts) | `Reject A97` |
| 4 | Bilanzierungsgebiet consistent (UTILMD matches grid record) | `Reject A02` |
| 5 | LF GLN in partner directory | `Reject A05` |
| 6 | Mindestvorlauffrist met (SLP: > today; RLM: ≥ 2 Werktage) | `Reject A99` |

### STP targets

| Condition | Expected STP |
|-----------|-------------|
| Without NIS import (`nis-syncd` N7) | ~60 % (missing grid records → Escalate) |
| With NIS data imported | ≥ 95 % |

---

## LF decision pipeline (E_0624)

```text
de.mako.process.initiated (PID 55008)
  → parse E_0624 payload (malo_id, scenario, deadline_at = event_time + 45 min)
  → GET marktd /api/v1/versorgung/{malo_id}      → VersorgungsStatus
  → evaluate_e0624(payload, vs, own_gln)
      Beliefert + standard     → MakodClient gpke.nb-lieferende.bestaetigen  (PID 55008)
      Beliefert + Einzug       → MakodClient gpke.nb-lieferende.ablehnen (A32) (PID 55009)
      Ersatzversorgung         → MakodClient gpke.nb-lieferende.bestaetigen  (PID 55008)
      MaLo unknown / mismatch  → approval_queue (expires_at = deadline_at - 5 min)
```

---

## Database schema

### `approval_queue`

| Column | Type | Description |
|--------|------|-------------|
| `id` | UUID | Primary key |
| `process_id` | UUID | Process that triggered the entry |
| `pid` | SMALLINT | BDEW PID |
| `malo_id` | TEXT | MaLo-ID (if resolved) |
| `reason` | TEXT | Why auto-decision was not possible |
| `status` | TEXT | `Pending` · `Approved` · `Rejected` · `Expired` |
| `expires_at` | TIMESTAMPTZ | Regulatory deadline minus 5 min |
| `tenant` | TEXT | Operator GLN |

Background task runs every 60 s to expire stale `Pending` entries.

### `anmeldung_decisions`

| Column | Type | Description |
|--------|------|-------------|
| `id` | UUID | Primary key |
| `process_id` | UUID | Anmeldung process |
| `pid` | SMALLINT | 55001 / 55016 / 44001 |
| `malo_id` | TEXT | MaLo-ID |
| `lf_gln` | TEXT | Requesting LF GLN |
| `decision` | TEXT | `Accept` · `Reject` · `Escalate` |
| `erc_code` | TEXT | BDEW ERC (e.g. `A06`) on Reject |
| `initiator_is_affiliate` | BOOL | `lf_gln == own_gln` — §20 EnWG parity |
| `tenant` | TEXT | Operator GLN |

---

## REST API

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/webhook` | Receive `de.mako.process.initiated` from `marktd` (HMAC-verified) |
| `GET` | `/api/v1/decisions` | List recent Anmeldung decisions (NB) |
| `GET` | `/api/v1/queue` | List LF approval queue entries |
| `POST` | `/api/v1/queue/{id}/approve` | Approve a queue entry (LF) |
| `POST` | `/api/v1/queue/{id}/reject` | Reject a queue entry (LF) |
| `GET` | `/health/live` | Liveness probe |
| `GET` | `/health/ready` | Readiness probe |
| `GET\|POST` | `/mcp` | MCP Streamable HTTP (spec 2025-11-25) |

---

## MCP tools

| Tool | Role | Description |
|------|------|-------------|
| `list_decisions` | NB | Recent Anmeldung STP decisions |
| `get_stp_rate` | NB | STP rate over last N days (target ≥ 95%) |
| `list_queue` | LF | Approval queue entries needing operator action |
| `get_queue_entry` | LF | Single queue entry by UUID |

---

## MSB module — REQOTE auto-response

When `processd` receives `de.mako.process.initiated` for PIDs 35001–35005 (REQOTE Preisanfrage
from an nMSB), it **automatically dispatches a QUOTES response** using the active
`PreisblattMessung` from `marktd`. Dispatching from master data rather than from a manual
ERP trigger is what keeps the response inside the APERAK ERC A97 deadline.

Enabled by default. Disable for manual QUOTES dispatch (e.g. during PreisblattMessung update
windows):

```toml
# processd.toml
[msb]
auto_preisanfrage = false
```

---

## MSB module — §14a Steuerungsauftrag auto-ORDRSP

When an MSB receives a WiM Steuerungsauftrag (iMS ORDERS, `makoworkflow = wim-steuerungsauftrag`),
`processd` evaluates the request automatically:

- `istFernschaltbar = true` AND `produktcode` is in the SR's `konfigurationsprodukte` list
  → auto-confirm (`wim.steuerungsauftrag.bestaetigen`)
- `istFernschaltbar = true` AND `produktcode` **not** contracted
  → auto-reject with ERC A05 (`wim.steuerungsauftrag.ablehnen`) per BK6-24-174 §4.3
- `istFernschaltbar = false` OR SR not found
  → escalate to operator approval queue

`konfigurationsprodukte` are managed via `marktd`'s typed sub-resource API.

---

## Quick start

```bash
# NB-only deployment (§7 EnWG separated binary)
MAKOD_URL=http://makod:8080 \
MAKOD_API_KEY=<key> \
MARKTD_URL=http://marktd:8180 \
MARKTD_API_KEY=<key> \
DATABASE_URL=postgres://processd:secret@postgres/processd \
OWN_MP_ID=9900000000002 \
cargo run -p processd --features nb-only

# LF-only deployment
cargo run -p processd --features lf-only -- \
  --own-gln 9900357000004 \
  --lf-auto-respond true

# Integrated §6b EnWG (both LF + NB)
cargo run -p processd --features integrated
```

### `processd.toml` equivalent (all env vars)

```
PROCESSD_LISTEN         = "0.0.0.0:8580"
DATABASE_URL            = "postgres://processd:secret@postgres/processd"
MAKOD_URL               = "http://makod:8080"
MAKOD_API_KEY           = "<key>"
MARKTD_URL              = "http://marktd:8180"
MARKTD_API_KEY          = "<key>"
OWN_MP_ID               = "9900000000002"
TENANT                  = "9900000000002"        # defaults to OWN_MP_ID
NB_AUTO_ACCEPT          = "false"                # set true only after verifying grid coverage
LF_AUTO_RESPOND         = "true"
LF_QUEUE_TTL_SECS       = "2700"                 # 45 min = regulatory deadline
OIDC_ISSUER             = "https://login.example.com"
OIDC_AUDIENCE           = "api://mako-processd"
INBOUND_WEBHOOK_SECRET  = "<hmac-secret>"        # must match marktd subscription secret
```

---

## Regulatory basis

| Obligation | Deadline | Module | Source |
|-----------|----------|--------|--------|
| GPKE Anmeldung decision (55001/55016) | **24 wall-clock hours** | NB | BK6-22-024 §5 |
| GeLi Gas Anmeldung decision (44001) | **10 Werktage** | NB | BK7-24-01-009 |
| LFA E_0624 auto-response (55008) | **45 minutes** | LF | BK6-22-024 §5, LFW24 |
| §20 EnWG parity audit | provable at BNetzA | Both | `initiator_is_affiliate` |

All deadline arithmetic uses **German local time (CET/CEST)**, not UTC.
`netz-checker` receives `now_utc()` and converts to Berlin time internally via `time-tz`.
