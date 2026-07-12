---
layout: default
title: Services
nav_order: 5
has_children: true
description: >-
  Operator guides for all 16 mako production services — makod, marktd, processd,
  invoicd, netzbilanzd, sperrd, edmd, einsd, obsd, nis-syncd, tarifbd, billingd,
  accountingd, vertragd, portald, and agentd.
---

# Services

mako consists of **16 independently deployable services**, each built as a self-contained Docker image with:
- TOML configuration with `_FILE` suffix for Kubernetes secrets
- Cedar ABAC authorization
- OIDC/JWT + API-key authentication  
- OpenTelemetry traces and metrics
- Built-in MCP server at `/mcp` (Streamable HTTP 2025-11-05)
- Structured health endpoints (`/health`, `/health/ready`)

---

## Protocol & Market Data

| Service | Port | Role | Purpose |
|---|---|---|---|
| [makod](./makod) | `:8080` · `:4080` · `:8090` | All | Protocol daemon — 45+ GPKE/WiM/GeLi Gas/MABIS/GaBi Gas workflows, AS4/REST/iMS |
| [marktd](./marktd) | `:8180` | All | Market Data Hub — MaLo/MeLo/contracts, VersorgungsStatus, typed BO4E API, EventBus fan-out, MMMA monthly import worker |
| [processd](./processd) | `:8580` | NB + LF + MSB | Process Decision Engine — Anmeldung STP ≥95%, LF E_0624 45-min auto-response, MSB REQOTE auto-response, §14a Steuerungsauftrag produktcode check |

## Invoice & Billing (NB)

| Service | Port | Role | Purpose |
|---|---|---|---|
| [invoicd](./invoicd) | `:8280` | LF | INVOIC plausibility-check — 6 checks (incl. ToU band routing via `zaehlzeitregister`), auto-settle/dispute, §22 MessZV receipts |
| [netzbilanzd](./netzbilanzd) | `:8680` | NB | NNE/KA/MMM billing — generates INVOIC 31001/31002/31005, draft lifecycle |
| [sperrd](./sperrd) | `:8780` | NB | Sperrung execution tracking — IFTSTA 21039 auto-dispatch on field confirmation |

## Energy Data & Observability

| Service | Port | Role | Purpose |
|---|---|---|---|
| [edmd](./edmd) | `:8380` | All | Energy Data Management — MSCONS, iMSys direct push, Hampel quality scoring, Ablesesteuerung (INSRPT auto-order), Iceberg/S3 OLAP |
| [einsd](./einsd) | `:9180` | NB/LF | Einspeiser Registry + EEG/KWKG settlement — 8 settlement models |
| [obsd](./obsd) | `:8480` | All | Business-process observability — KPI reports, §20 EnWG parity, BNetzA audit export, Alertmanager |
| [nis-syncd](./nis-syncd) | `:9680` | NB | NIS/GIS grid topology import — lifts Anmeldung STP ~80% → ≥95% (stateless) |

## Retail Billing (LF)

| Service | Port | Role | Purpose |
|---|---|---|---|
| [tarifbd](./tarifbd) | `:9080` | LF | Product & Tariff Catalog — user-defined energy products, EPEX Spot for §41a, B2B Angebote/quotations |
| [billingd](./billingd) | `:9280` | LF | Energy Billing Engine — 12 categories, §41a dynamic, §42a GGV community solar, XRechnung 3.0 / ZUGFeRD 2.3 |
| [accountingd](./accountingd) | `:9380` | LF | Customer Account Ledger — Massenkontokorrent, SEPA pain.008 (N-5 scheduler), Mahnwesen |

## B2C & AI

| Service | Port | Role | Purpose |
|---|---|---|---|
| [vertragd](./vertragd) | `:9780` | LF | Contract & Customer Management — Kunden (B2C+B2B), Rahmenverträge, Versorgungsverträge, kunden_identitaeten (N portal users per company), Tarifwechsel, Kündigung, OIDC→MaLo auth gateway for portald |
| [portald](./portald) | `:9480` | LF | Customer Portal gateway — aggregates all LF services, REST + SSE, OIDC-gated |
| [agentd](./agentd) | `:9580` | All | Multi-agent LLM orchestration — Orchestrator + Specialist Mesh, LanceDB RAG, MCP tools |

---

## Deployment

All services are available as multi-stage Docker images built with `cargo-chef`:

```bash
# Single all-in-one daemon (makod only)
docker pull ghcr.io/hupe1980/makod:latest

# Full stack via Docker Compose
git clone https://github.com/hupe1980/mako
cd mako/demo
docker compose up
```

See the [Getting Started](../getting-started) guide for the full deployment walkthrough.
