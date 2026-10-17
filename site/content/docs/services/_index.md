+++
title = "Services"
description = "Operator guides for the 14 production daemons — ports, config, APIs, deployment."
weight = 4
sort_by = "weight"
template = "section.html"
page_template = "page.html"
+++
mako implements the BDEW **Marktkommunikation** between the four German market
roles — **LF** (Lieferant), **NB** (Netzbetreiber), **MSB**
(Messstellenbetreiber) and **BKV** (Bilanzkreisverantwortlicher); see
[Party Roles](@/docs/architecture/domain-model.md#party-roles-marktrollen).

mako consists of **14 independently deployable services**, each a self-contained
Docker image built on the [`mako-service`](https://github.com/hupe1980/mako/tree/main/crates/mako-service)
SDK, with TOML configuration (`env:` and `_FILE` secrets), OIDC/JWT and API-key
authentication, Cedar ABAC, OpenTelemetry, `/health/live` · `/health/ready` ·
`/metrics`, and an MCP server at `/mcp` (Streamable HTTP) on 11 of the 14 —
all except outputd, portald and agentd (the MCP host). `just dev <service>` runs one
against `dev/config/<service>.toml`.

---

## Service Map

```mermaid
graph TB
    ext["BDEW Counterparty<br/>(NB · LF · MSB · BKV)"]

    subgraph protocol ["Protocol & Market Data"]
        makod[":8080 makod<br/>EDIFACT runtime · 71 workflows<br/>AS4 · SlateDB · MCP"]
        marktd[":8180 marktd<br/>MaLo/MeLo/NeLo · contracts<br/>VersorgungsStatus · fan-out"]
        processd[":8580 processd<br/>Anmeldung STP ≥95%<br/>LF answers · §14a<br/>process projections · KPI · §7a parity<br/>Sperrauftrag queue · IFTSTA 21039"]
    end

    subgraph nb_billing ["Invoice & Grid Billing (NB)"]
        invoicd[":8280 invoicd<br/>INVOIC 8-stage plausibility<br/>auto-settle/dispute"]
        netzbilanzd[":8680 netzbilanzd<br/>NNE/KA/MMM/MSB billing<br/>SettlementResult · CalculationTrace<br/>MaBiS MSCONS 13003 · 10. Werktag"]
    end

    subgraph data ["Energy Data & Observability"]
        edmd[":8380 edmd<br/>MSCONS · iMSys direct push<br/>validation · Ersatzwerte · virtual"]
        einsd[":9180 einsd<br/>EEG/KWKG settlement<br/>10 schemes · §14 UStG Gutschrift"]
    end

    subgraph lf_billing ["Retail Billing (LF)"]
        productd[":9080 productd<br/>14 categories · §42 Stromkennzeichnung<br/>EPEX §41a · B2B Angebote"]
        billingd[":9280 billingd<br/>13 categories · XRechnung 3.0<br/>RLM demand · §54 exemption"]
        outputd[":9880 outputd<br/>Typst templates · ZUGFeRD carrier<br/>issued documents · delivery evidence"]
        accountingd[":9380 accountingd<br/>Massenkontokorrent<br/>SEPA FRST/RCUR · GLN ID · Aging · §288 BGB"]
    end

    subgraph b2c ["Contract & Customer (LF)"]
        vertragd[":9780 vertragd<br/>Kunden B2C+B2B · Rahmenverträge<br/>OIDC→MaLo · 18 MCP tools"]
        portald[":9480 portald<br/>customer portal read-model<br/>§41 self-service"]
    end

    agentd[":9580 agentd<br/>28 declarative manifests on agentplane<br/>journaled effects · strict replay<br/>human triage worklist · A2A cards<br/>OIDC · HMAC · 5 model backends"]

    ext -->|AS4 / REST| makod
    makod <-->|CloudEvents| marktd
    marktd -->|webhook fan-out| processd & invoicd & edmd & agentd
    makod -->|commands| netzbilanzd & invoicd
    netzbilanzd -->|MSCONS 13003 cmd| makod
    billingd -->|de.billing.rechnung.erstellt| accountingd
    billingd -->|render · pin hash| outputd
    vertragd -->|start-supply| processd
    portald -->|aggregates| billingd & accountingd & edmd & einsd & marktd
```

---

## Protocol & Market Data

| Service | Port | Role | Purpose |
|---|---|---|---|
| [makod](@/docs/services/makod.md) | `:8080` · `:4080` · `:8090` | All | Protocol daemon — 71 workflows over 469 Prüfidentifikatoren, AS4/REST/iMS |
| [marktd](@/docs/services/marktd.md) | `:8180` | All | Market Data Hub — MaLo/MeLo/contracts, VersorgungsStatus, typed BO4E API, durable fan-out, MMMA monthly import worker |
| [processd](@/docs/services/processd.md) | `:8580` | NB + LF + MSB | Process Decision Engine — Anmeldung STP ≥95%, LF answers to the NB-initiated GPKE processes, MSB REQOTE auto-response, §14a Steuerungsauftrag produktcode check; the process read model — per-PID KPIs with the APERAK and Antwortfrist clocks reported separately, deadlines read from `mako-fristen`, `GET /api/v1/audit/gleichbehandlung` for the § 7a Abs. 5 EnWG filing, rebuildable from marktd's replay log; the NB Sperr-/Entsperrauftrag queue with IFTSTA 21039 on field confirmation; role-gated binaries, which cannot answer as a role they were not built for |

## Invoice & Grid Billing

| Service | Port | Role | Purpose |
|---|---|---|---|
| [invoicd](@/docs/services/invoicd.md) | `:8280` | LF | INVOIC plausibility-check — eight stages plus the WiM 31003 send window (incl. ToU band routing via `zaehlzeitregister`), auto-settle/dispute, § 147 AO / GoBD receipts |
| [netzbilanzd](@/docs/services/netzbilanzd.md) | `:8680` | NB/ÜNB | NNE/KA/MMM/MSB/AWH billing — generates INVOIC 31001/31002/31005/31009/31011, full REMADV lifecycle, §14a Modul 3 ToU, §42b EnWG GGV, Redispatch 2.0 Kostenblatt; MaBiS Summenzeitreihen per Bilanzierungsgebiet filed with the BIKO as MSCONS 13003 on the 10. Werktag, with the BIKO-assigned Datenstatus and open Korrekturbedarf; 12-tool MCP server |

## Energy Data & Observability

| Service | Port | Role | Purpose |
|---|---|---|---|
| [edmd](@/docs/services/edmd.md) | `:8380` | All | Energy Data Management — MSCONS, iMSys direct push, Kafka batch ingest, quality scoring and validation, virtual meters, tiered storage; 16-tool MCP server |
| [einsd](@/docs/services/einsd.md) | `:9180` | NB/LF | Einspeiser Registry + EEG/KWKG settlement — 10 settlement schemes; issues the **§14 UStG Gutschrift** (Gutschriftverfahren) per billable settlement as a BO4E `Rechnung` with per-rate USt breakdown; 20-tool MCP server |

`edmd`'s quality layer is `metering`'s validation rules, run per register with an A/B/C/F
grade, and its Ersatzwertbildung; its derived products are virtual meters (UTILTS
Berechnungsformel, § 42b EnWG GGV), the § 40a Abs. 2 EnWG
Verbrauchsschätzung, Resampling and Ablesesteuerung (INSRPT auto-order). `meterstore` tiers
hot PostgreSQL against cold Apache Iceberg with cross-tier OLAP and a read-only Iceberg REST
catalog, and every Cedar write action is role-gated to MSB/NB/admin.

## Retail Billing (LF)

| Service | Port | Role | Purpose |
|---|---|---|---|
| [productd](@/docs/services/productd.md) | `:9080` | LF | Product & Tariff Catalog — user-defined energy products, EPEX Spot for §41a, B2B Angebote/quotations |
| [billingd](@/docs/services/billingd.md) | `:9280` | LF | Energy Billing Engine — 13 categories, §41a dynamic, §42b EnWG GGV community solar, EN 16931 e-invoicing (XRechnung 3.0 CII / PEPPOL UBL); the ZUGFeRD PDF renders via outputd |
| [outputd](@/docs/services/outputd.md) | `:9880` | — | Customer Communications — operator Typst templates, the ZUGFeRD PDF/A-3 carrier, the append-only store of issued documents and their delivery |
| [accountingd](@/docs/services/accountingd.md) | `:9380` | LF | Customer Account Ledger — tamper-evident double-entry ledger (`doubleentry`: Merkle proofs, period seals for GoBD/§146 AO); per-MaLo Kontokorrent, FIFO open-item clearing, Summen- und Saldenliste §238 HGB, Verzugszinsen §288 BGB, Zahlungsvereinbarung; SEPA pain.008 and CAMT.054; dunning delivered through outputd; §40b Abs. 1 Jahresabschluss; GDPR Art. 17 |

`outputd`'s templates are content-addressed and append-only, and a publish is gated by
proof; besides the invoice it renders the Textform kinds — Mahnung (§ 126b BGB) and
Preisanpassung (§ 41 Abs. 5 EnWG) — and delivers over portal, e-mail, print spool and ERP
with per-channel evidence.

## B2C & AI

| Service | Port | Role | Purpose |
|---|---|---|---|
| [vertragd](@/docs/services/vertragd.md) | `:9780` | LF + MSB | Contract & Customer Management — Kunden (B2C+B2B), Rahmenverträge, Versorgungsverträge, kunden_identitaeten (N portal users per company), Tarifwechsel with its § 41 Abs. 5 EnWG Preisänderungsanzeige (rendered and delivered through outputd), Kündigung, OIDC→MaLo auth gateway for portald |
| [portald](@/docs/services/portald.md) | `:9480` | LF | Customer Portal gateway — stateless aggregation over all LF services plus the §41 EnWG self-service writes (Tarifwechsel, Kündigung, SEPA, GDPR Art. 16) and the document inbox served out of outputd; every route resolves customer ownership through `vertragd`; no MCP server — agents read each datum from the service that owns it |
| [agentd](@/docs/services/agentd.md) | `:9580` | All | Multi-agent LLM orchestration — **28 declarative manifests** on the agentplane durable runtime, every effect journaled; read-only by construction, so oversight is a triage worklist |

`agentd`'s manifests are activated through `[bundled_agents]`; one event yields
one journaled run per subscribing specialist, and since no manifest grants a
mutating tool, a finding opens a triage row rather than suspending the run.

---

## Shared foundation — the `mako-service` SDK

Every daemon is built on the [`mako-service`](https://github.com/hupe1980/mako/tree/main/crates/mako-service)
crate. A service's `main` is a single line:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Billingd>().await   // Billingd: Daemon impl
}
```

`run::<D>()` owns the whole lifecycle. A `Daemon` implementation supplies only what is
service-specific — the config type, the migrations, and a `build()` that assembles the domain
router and spawns background workers:

```mermaid
flowchart TD
    start(["main → run::&lt;D&gt;"]) --> check{"--check?"}
    check -->|yes| probe["GET /health/ready<br/>exit 0/1"] --> done([exit])
    check -->|no| trace["init tracing"]
    trace --> cfg["load config<br/>TOML + env + _FILE"]
    cfg --> pool["connect tuned pool<br/>DatabaseConfig::connect(url, NAME)<br/>sets application_name"]
    pool --> migrate["D::migrate<br/>sqlx::migrate! + outbox::ensure_schema"]
    migrate --> build["D::build(cfg, ctx)<br/>domain Router + spawn workers on ctx.shutdown"]
    build --> infra["mount infra routes<br/>/health · /health/live · /health/ready · /metrics"]
    infra --> serve["serve with graceful drain<br/>SIGINT / SIGTERM"]
    serve -.->|readiness| ready["/health/ready = bounded SELECT 1 + D::ready"]
```

What every service gets for free from the runner:

| Concern | Provided by `run::<D>()` |
|---|---|
| **Tracing** | Structured logs + optional OTLP export (`RUST_LOG`, `OTEL_EXPORTER_OTLP_ENDPOINT`) |
| **Config** | `[database]` + service blocks, `env:`/`_FILE` substitution, `<SVC>_CONFIG` path |
| **Pool** | Tuned sizing with a per-service `application_name` for `pg_stat_activity` |
| **Migrations** | Applied at startup before the first request |
| **Readiness** | Real `/health/ready` — a bounded `SELECT 1` DB ping, not a static `true` |
| **Shutdown** | SIGINT/SIGTERM graceful drain; workers observe `ctx.shutdown` |
| **Health probe** | `--check` in-container HEALTHCHECK (no shell, no curl) |

Event-emitting services — accountingd, billingd, edmd, einsd,
netzbilanzd, processd and vertragd — use the SDK's **transactional outbox**: each
CloudEvent is written to `event_outbox` *in the same transaction* as the
business change and drained by an `OutboxWorker` with retry and a status-column
dead-letter queue. `marktd` keeps its own `event_log` outbox ahead of the durable
fan-out, and `invoicd` retries ERP notifications from columns on
`invoic_receipts`. Emission always goes through `CloudEvent::new` +
`post_ce_with_retry`, signed as Standard Webhooks (`webhook-signature`).

Both outboxes wake their worker from an `AFTER INSERT` trigger's `NOTIFY`, which
Postgres holds until commit; the 30 s poll is the fallback.
`cargo xtask check-outbox-notify` holds trigger, channel and listener together.

`makod` alone keeps a bespoke `main` for its SlateDB event store and AS4
runtime; every other service, the stateless `portald` and `agentd` included,
runs on `mako_service::run`.

---

## Deployment

All services are available as multi-stage Docker images built with `cargo-chef`:

```bash
# Single all-in-one daemon (makod only)
docker pull ghcr.io/hupe1980/mako-makod:latest

# NB STP demo — UTILMD 55001 Lieferbeginn end-to-end
git clone https://github.com/hupe1980/mako
cd mako/demos/nb-stp
docker compose up

# EEG billing demo — solar plant registration + §21 EEG 2023 settlement
cd mako/demos/eeg-billing
docker compose up

# Order-to-cash demo — Tarif → Vertrag → Rechnung → Dokument → Offener Posten
cd mako/demos/o2c
docker compose up
```

See the [Getting Started](@/docs/guide/getting-started.md) guide for the full deployment walkthrough.
