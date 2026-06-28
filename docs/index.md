---
layout: home
title: mako — EDI@Energy for Rust
nav_order: 1
description: >-
  Production-grade Rust library for German energy market communication
  (BDEW MaKo / EDI@Energy). EDIFACT parsing, AHB/MIG validation,
  event-sourced process runtime, AS4 transport, and regulatory deadlines —
  all in one workspace.
permalink: /
---

<!-- ── Hero ──────────────────────────────────────────────────────────────── -->
<div class="mako-hero">
  <div class="mako-hero__badge-row">
    <a href="https://github.com/hupe1980/edi-energy-rs/actions/workflows/ci.yml">
      <img src="https://github.com/hupe1980/edi-energy-rs/actions/workflows/ci.yml/badge.svg" alt="CI">
    </a>
    <a href="https://crates.io/crates/edi-energy">
      <img src="https://img.shields.io/crates/v/edi-energy?label=edi-energy&color=f74c00&logo=rust" alt="crates.io">
    </a>
    <a href="https://crates.io/crates/mako-engine">
      <img src="https://img.shields.io/crates/v/mako-engine?label=mako-engine&color=f74c00&logo=rust" alt="crates.io">
    </a>
    <img src="https://img.shields.io/badge/MSRV-1.88-orange?logo=rust" alt="MSRV 1.88">
    <a href="https://github.com/hupe1980/edi-energy-rs/blob/main/LICENSE-MIT">
      <img src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue" alt="License">
    </a>
    <img src="https://img.shields.io/badge/BDEW-EDI%40Energy-green" alt="BDEW EDI@Energy">
  </div>

  <h1 class="mako-hero__title">mako ⚡</h1>
  <p class="mako-hero__subtitle">
    End-to-end <strong>German energy market communication</strong> in Rust.<br>
    Parse &amp; validate every EDI@Energy EDIFACT message. Run long-lived MaKo
    workflows with event sourcing, regulatory deadlines, and AS4 transport.
  </p>

  <div class="mako-hero__cta">
    <a href="{{ '/getting-started' | relative_url }}" class="btn btn-primary mako-cta-primary">
      Get started →
    </a>
    <a href="https://github.com/hupe1980/edi-energy-rs" class="btn mako-cta-secondary">
      View on GitHub
    </a>
  </div>

  <div class="mako-hero__warning">
    <strong>⚠ Pre-1.0 — Experimental.</strong>
    APIs may change between releases. Not yet recommended for production without
    thorough in-house testing.
  </div>
</div>

<!-- ── Three-column pitch ────────────────────────────────────────────────── -->
<div class="mako-features">
  <div class="mako-feature">
    <div class="mako-feature__icon">📦</div>
    <h3>Parse &amp; Validate</h3>
    <p>
      All <strong>17 EDI@Energy EDIFACT message types</strong> — UTILMD, MSCONS,
      APERAK, CONTRL, INVOIC, REMADV, ORDERS, ORDRSP, IFTSTA, INSRPT, and more.
      Five-layer validation: schema → code lists → MIG → AHB → semantic rules.
      Multi-version profile registry with 7-day transition grace windows, fully
      compliant with BDEW annual release cycles.
    </p>
    <a href="{{ '/parsing' | relative_url }}">Parsing guide →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">♻️</div>
    <h3>Process Runtime</h3>
    <p>
      Event-sourced <strong>MaKo workflow engine</strong> with optimistic
      concurrency, atomic dual-write (events + outbox in one
      <code>WriteBatch</code>), APERAK deadline enforcement (GPKE 24 h / WiM 5
      Werktage / GeLi Gas 10 Werktage), and a SlateDB-backed durable store.
      Typestate workflow FSMs make invalid transitions a compile error.
    </p>
    <a href="{{ '/engine' | relative_url }}">Engine guide →</a>
  </div>

  <div class="mako-feature">
    <div class="mako-feature__icon">🚀</div>
    <h3>Production Daemon</h3>
    <p>
      <strong><code>makod</code></strong> — a single binary that assembles all
      domain modules (GPKE, WiM, GeLi Gas, MABIS) behind three independent
      ports: AS4/ebMS3 inbound&nbsp;(<code>:4080</code>), HTTP REST
      (<code>:8080</code>), and BDEW API-Webdienste Strom
      (<code>:8090</code>). Startup coverage validation panics on missing
      adapters rather than silently dead-lettering messages.
    </p>
    <a href="{{ '/makod' | relative_url }}">Operator guide →</a>
  </div>
</div>

<!-- ── Workspace at a Glance ─────────────────────────────────────────────── -->
## Workspace at a Glance
{: .mt-8 }

| Crate / service | Purpose |
|---|---|
| [`edi-energy`](https://crates.io/crates/edi-energy) | Parse · validate · build all 17 EDI@Energy EDIFACT message types |
| [`mako-engine`](https://crates.io/crates/mako-engine) | Event-sourced runtime: `Workflow`, `Process`, `EventStore`, outbox, deadlines |
| `mako-gpke` | GPKE — UTILMD Strom (55001–55018, 55555, 56001–56004) + INVOIC (31001–31008) + ORDERS/ORDRSP (17134–17135, 19001–19002) |
| `mako-wim` | WiM (Wechselprozesse im Messwesen **Strom**) — UTILMD (11001–11003 Gerätewechsel) + ORDERS (17001–17011 Geräteübernahme, 17101 Stammdaten) + ORDCHG (39000 Stornierung) |
| `mako-geli-gas` | GeLi Gas 3.0 (BK7-24-01-009) — UTILMD G (44001–44018, 44555) |
| `mako-mabis` | MABIS — PID 13003 (Bilanzkreisabrechnung Strom, BKV↔ÜNB) |
| `mako-wim-gas` | WiM Gas (BK7-24-01-009) — UTILMD G (44022–44053, 44168–44170 MSB change) *(placeholder)* |
| `mako-gabi-gas` | GaBi Gas — Allokation, Nominierung, MMM Gas INVOIC (31010–31011) *(placeholder)* |
| `mako-nbw` | Netzbetreiberwechsel — PARTIN bulk DSO handover *(placeholder)* |
| `energy-api` | BDEW API-Webdienste Strom — REST/WebSocket client + Axum server |
| `redispatch-xml` | Redispatch 2.0 XML/XSD format parsing |
| `makod` | Production daemon — assembles all modules |

## Quick Start
{: .mt-8 }

Add `edi-energy` to `Cargo.toml`:

```toml
[dependencies]
edi-energy = { version = "0.1", features = ["utilmd", "mscons", "aperak"] }
```

Parse and validate a UTILMD message in three lines:

```rust
use edi_energy::{parse, EdiEnergyMessage};

let msg = parse(std::fs::read("lieferbeginn.edi")?.as_ref())?;
msg.validate()?.into_error_result()?;
println!("PID {}", msg.detect_pruefidentifikator()?.as_u32()); // → 55001
```

For the full process runtime, see the [Getting Started guide]({{ '/getting-started' | relative_url }}).

## Regulatory Compliance
{: .mt-8 }

mako tracks all current BNetzA rulings that govern German energy market
communication:

| Ruling | Scope | Effective |
|---|---|---|
| [BK6-24-174](https://www.bundesnetzagentur.de/) | GPKE Teil 1–3 + WiM + MaBiS | 06.06.2025 |
| [BK6-22-024](https://www.bundesnetzagentur.de/) | GPKE Teil 4 (Stammdaten, ex-MPES PIDs 56001–56004) | 06.06.2025 |
| [BK7-24-01-009](https://www.bundesnetzagentur.de/) | GeLi Gas 3.0 — UTILMD G supplier-switch | 01.10.2025 |
| BDEW EDI@Energy FV2026-10-01 | All message types — annual release | 01.10.2026 |

See the [BNetzA regulatory reference]({{ '/bnetza' | relative_url }}) for the full ruling index
and the [PID reference]({{ '/pid-reference' | relative_url }}) for a complete Prüfidentifikator table.
