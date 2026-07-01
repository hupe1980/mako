---
layout: home
title: mako — EDI@Energy for Rust
nav_order: 1
description: >-
  Rust library for German energy market communication
  (BDEW MaKo / EDI@Energy). EDIFACT parsing, AHB/MIG validation,
  event-sourced process runtime, AS4 transport, regulatory deadlines,
  OpenTelemetry observability, and a REST Command API with CloudEvents 1.0 webhooks —
  all in one workspace.
permalink: /
---

<!-- ── Hero ──────────────────────────────────────────────────────────────── -->
<div class="mako-hero">
  <div class="mako-hero__badge-row">
    <a href="https://github.com/hupe1980/mako/actions/workflows/ci.yml">
      <img src="https://github.com/hupe1980/mako/actions/workflows/ci.yml/badge.svg" alt="CI">
    </a>
    <a href="https://crates.io/crates/edi-energy">
      <img src="https://img.shields.io/crates/v/edi-energy?label=edi-energy&color=f59e0b&logo=rust" alt="edi-energy on crates.io">
    </a>
    <a href="https://crates.io/crates/mako-engine">
      <img src="https://img.shields.io/crates/v/mako-engine?label=mako-engine&color=f59e0b&logo=rust" alt="mako-engine on crates.io">
    </a>
    <img src="https://img.shields.io/badge/MSRV-1.88-orange?logo=rust" alt="MSRV 1.88">
    <a href="https://github.com/hupe1980/mako/blob/main/LICENSE-MIT">
      <img src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue" alt="MIT / Apache-2.0">
    </a>
    <img src="https://img.shields.io/badge/BDEW-FV2026--10--01-green" alt="BDEW FV2026-10-01">
  </div>

  <h1 class="mako-hero__title">mako ⚡</h1>
  <p class="mako-hero__subtitle">
    The only Rust library that covers the full German energy market stack —<br>
    from raw EDIFACT bytes to durable, auditable MaKo process state.
  </p>

  <div class="mako-hero__cta">
    <a href="{{ '/getting-started' | relative_url }}" class="btn btn-primary mako-cta-primary">
      Get started →
    </a>
    <a href="{{ '/reference' | relative_url }}" class="btn mako-cta-secondary">
      API Reference
    </a>
    <a href="https://github.com/hupe1980/mako" class="btn mako-cta-secondary">
      GitHub
    </a>
  </div>

  <div class="mako-hero__warning">
    <strong>⚠ Pre-1.0 — Experimental.</strong>
    APIs may change between patch releases. Not yet recommended for production
    without thorough in-house validation.
  </div>
</div>

<!-- ── KPI strip ─────────────────────────────────────────────────────────── -->
<div class="mako-kpis">
  <div class="mako-kpi">
    <span class="mako-kpi__value">17</span>
    <span class="mako-kpi__label">EDIFACT message types</span>
  </div>
  <div class="mako-kpi">
    <span class="mako-kpi__value">40+</span>
    <span class="mako-kpi__label">AHB/MIG format versions</span>
  </div>
  <div class="mako-kpi">
    <span class="mako-kpi__value">5-layer</span>
    <span class="mako-kpi__label">validation pipeline</span>
  </div>
  <div class="mako-kpi">
    <span class="mako-kpi__value">238+</span>
    <span class="mako-kpi__label">Prüfidentifikatoren</span>
  </div>
  <div class="mako-kpi">
    <span class="mako-kpi__value">1.88</span>
    <span class="mako-kpi__label">MSRV stable Rust</span>
  </div>
  <div class="mako-kpi">
    <span class="mako-kpi__value">0</span>
    <span class="mako-kpi__label">unsafe blocks</span>
  </div>
</div>

<!-- ── Six-column feature grid ──────────────────────────────────────────── -->
<div class="mako-features">
  <div class="mako-feature">
    <div class="mako-feature__icon">🔍</div>
    <h3>Parse &amp; Validate</h3>
    <p>
      All 17 EDI@Energy EDIFACT types. Five-layer pipeline: schema → code
      lists → MIG → AHB → semantic rules. Structured <code>EdiEnergyReport</code>
      with per-rule violation details, not raw strings.
    </p>
    <a href="{{ '/parsing' | relative_url }}">Parsing guide →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">⚙️</div>
    <h3>Process Runtime</h3>
    <p>
      Event-sourced FSM engine with optimistic concurrency, atomic dual-write
      (events + outbox in one <code>WriteBatch</code>), and APERAK deadline
      enforcement — GPKE 24 h, WiM 5 Werktage, GeLi Gas 10 Werktage.
    </p>
    <a href="{{ '/engine' | relative_url }}">Engine guide →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">🔌</div>
    <h3>Command API &amp; Webhooks</h3>
    <p>
      ERP integration via <code>POST /api/v1/commands</code> (BO4E JSON).
      Outbound events pushed to your ERP over HMAC-SHA256-signed <a href="erp-integration">CloudEvents 1.0 webhooks</a>.
      Idempotency keys prevent double-processing on retries.
    </p>
    <a href="{{ '/erp-integration' | relative_url }}">ERP integration →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">📡</div>
    <h3>AS4 + REST Dual Channel</h3>
    <p>
      AS4/ebMS3 inbound on <code>:4080</code>, HTTP REST on <code>:8080</code>,
      and BDEW API-Webdienste Strom (iMS REST/JSON) on <code>:8090</code>.
      Startup coverage validation panics on missing message adapters.
    </p>
    <a href="{{ '/api-webdienste' | relative_url }}">API-Webdienste →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">📊</div>
    <h3>OpenTelemetry Observability</h3>
    <p>
      Structured traces and metrics exported via OTLP. Every workflow command,
      event append, outbox delivery, and deadline dispatch carries a trace
      context. Plug into Grafana, Jaeger, or any OTLP-compatible backend.
    </p>
    <a href="{{ '/makod' | relative_url }}#observability">Observability →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">🏭</div>
    <h3>Production Daemon</h3>
    <p>
      <code>makod</code> — a single binary deploying all domain modules behind
      a durable SlateDB event store. Docker-ready, Kubernetes-native, with
      health endpoints, graceful shutdown, and S3/GCS/Azure object-store
      backends for cloud deployments.
    </p>
    <a href="{{ '/makod' | relative_url }}">Operator guide →</a>
  </div>
</div>

<div markdown="1">

---

## Quick Start
{: .mt-8 }

**EDIFACT parsing** — parse and validate a UTILMD message in three lines:

```toml
[dependencies]
edi-energy = { version = "0.5", features = ["utilmd", "mscons", "aperak"] }
```

```rust
use edi_energy::{parse, EdiEnergyMessage};

let msg = parse(std::fs::read("lieferbeginn.edi")?.as_ref())?;
msg.validate()?.into_error_result()?;  // returns Err if any AHB rule fires
println!("PID {}", msg.detect_pruefidentifikator()?.as_u32()); // → 55001
```

**Full process runtime** — run a GPKE supplier-change workflow:

```toml
[dependencies]
mako-engine = { version = "0.5", features = ["testing"] }
mako-gpke   = "0.5"
```

```rust
use mako_engine::{builder::EngineBuilder, event_store::InMemoryEventStore, ids::TenantId, version::WorkflowId};
use mako_gpke::wechselprozesse::{GpkeSupplierChangeWorkflow, SupplierChangeCommand};

let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .build();
let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(TenantId::new(), WorkflowId::new("gpke-supplier-change", "FV2025-10-01"));
let envelopes = process.execute_and_enqueue(SupplierChangeCommand::ReceiveUtilmd { .. }).await?;
// Events and APERAK outbox entry written atomically — no lost messages on crash.
```

→ Full walkthrough in the [Getting Started guide]({{ '/getting-started' | relative_url }}).

---

## Workspace at a Glance
{: .mt-8 }

| Crate / service | Purpose |
|---|---|
| [`edi-energy`](https://crates.io/crates/edi-energy) | Parse · validate · build all 17 EDI@Energy EDIFACT types |
| [`mako-engine`](https://crates.io/crates/mako-engine) | Event-sourced runtime: `Workflow`, `Process`, `EventStore`, outbox, deadlines, OpenTelemetry |
| `mako-gpke` | GPKE — UTILMD Strom (55001–55018, 55555) + INVOIC (31001–31002, 31005–31009) + ORDERS Sperrung (17115–17117) + ORDERS/ORDRSP Konfiguration |
| `mako-wim` | WiM Strom — Messstellenbetrieb (PIDs 55039, 55042, 55051, 55168) + ORDERS Geräteübernahme + Stammdaten |
| `mako-wim-gas` | WiM Gas — UTILMD G (44022–44053) MSB-Wechsel Gas |
| `mako-geli-gas` | GeLi Gas 3.0 (BK7-24-01-009) — UTILMD G (44001–44021) |
| `mako-mabis` | MABIS — PID 13003 Bilanzkreisabrechnung Strom (BKV↔ÜNB) |
| `mako-redispatch` | Redispatch 2.0 — 8 XML-document-driven workflows (Activation, Stammdaten, NetworkConstraint, …); IFTSTA PIDs 21037/21038 |
| `redispatch-xml` | Redispatch 2.0 XML/XSD parsing |
| `mako-gabi-gas` | GaBi Gas — INVOIC 31010 (Kapazitätsrechnung), 31011 (Rechnung sonstige Leistung) *(placeholder)* |
| `mako-nbw` | Netzbetreiberwechsel — PARTIN bulk DSO handover *(placeholder)* |
| `energy-api` | BDEW API-Webdienste Strom — REST/WebSocket client + Axum server |
| `makod` | Production daemon — all modules, three ports, SlateDB, OTLP |

---

## Regulatory Compliance
{: .mt-8 }

mako tracks every BNetzA ruling that governs German energy market communication
and ships AHB/MIG profiles for every active format version:

| Ruling | Scope | Effective |
|---|---|---|
| BK6-24-174 | GPKE Teil 1–3 + WiM + MABIS | 06.06.2025 |
| BK6-22-024 | GPKE Teil 4 — Stammdatenprozesse | 06.06.2025 |
| BK7-24-01-009 | GeLi Gas 3.0 — UTILMD G supplier-switch | 01.10.2025 |
| BDEW FV2026-10-01 | All message types — annual release | 01.10.2026 |

Both `FV2025-10-01` and `FV2026-10-01` coexist in the same engine instance
simultaneously. A process started under the old format version continues to
completion under the same rules even after the annual cutover.

→ [BNetzA regulatory reference]({{ '/bnetza' | relative_url }}) · [PID reference]({{ '/pid-reference' | relative_url }}) · [Release lifecycle]({{ '/release-lifecycle' | relative_url }})

---

## Why mako?
{: .mt-8 }

| | mako | Hand-rolled EDIFACT | Generic workflow engine |
|---|:---:|:---:|:---:|
| AHB/MIG validation built in | ✅ | ❌ | ❌ |
| APERAK deadline enforcement | ✅ | ❌ | ⚠ manual |
| Annual format-version migration | ✅ codegen | ❌ | ❌ |
| Atomic dual-write (events + outbox) | ✅ | ❌ | ⚠ 2-phase |
| AS4/ebMS3 transport | ✅ | ❌ | ❌ |
| API-Webdienste Strom (iMS) | ✅ | ❌ | ❌ |
| CloudEvents 1.0 ERP webhooks | ✅ | ❌ | ❌ |
| OpenTelemetry traces + metrics | ✅ | ❌ | ⚠ varies |
| 100% safe Rust, no OpenSSL for TLS | ✅ | ❌ | ❌ |

</div>
