# AGENTS.md — mako

German energy-market communication and settlement (BDEW MaKo / EDI@Energy) in
Rust. A Cargo workspace: **25 library crates** under `crates/`, **17 service
daemons** under `services/`, plus `xtask`, `makotest` and the Zola site.

`README.md` is the service index and `site/content/docs/**` the operator guides;
neither is restated here. This file is what an agent needs before touching code.

## Ground rules

- **Breaking changes are fine.** The project is unreleased: hard cuts, no
  backward compatibility, SQL schema edited in place with no migration.
- **Docs and comments state current truth only** — no changelogs, no "used to",
  no backlog pointers. Open work lives in one backlog, nowhere else.

  The rule bites hardest on a regression test, where narrating the incident is
  the natural way to justify the test — and it is the wrong way, because a reader
  skimming past tense cannot tell which half is live. **State the invariant, then
  the failure it prevents, in the present.** The rationale is wanted; the history
  is not:

  ```text
  ✗  It used to scan `meter_billing_periods` — the cache — so a MaLo whose
     aggregate had never been requested was invisible to discovery.
  ✓  Discovery must not run off `meter_billing_periods`: that table is the
     cache, so a MaLo whose aggregate has never been requested has no row
     there and would be invisible to discovery.
  ```
- **Every regulatory claim cites a primary source** — document, chapter, page.
  A `§` reaches the codebase from a published document or not at all. Guessing a
  plausible one is the most expensive mistake available here; see the
  known-wrong citations in *Domain Rules* below.
- **A defect class becomes a guard.** When something is found, the deliverable is
  the check that makes it unrepresentable — which is why `just ci` carries 29 of
  them and the list grows with the defect list, not the feature list.

## Build and test

`just ci` is the gate. It is the whole suite plus every guard, and nothing is
"done" until it passes.

```bash
just check      # cargo check --all-targets --all-features — the minimum
just ci         # the gate: test, doctests, clippy, deny, 29 xtask guards, site-free
just check-site # mermaid + link + zola checks; NOT part of `just ci`
just test-db    # schema-per-test suites against real PostgreSQL (needs Docker)
```

Four traps, each of which has produced a false "green" or a stalled run:

- **Never pipe `just ci` through `tail`/`grep` for the verdict** — the pipe masks
  the exit code. Run `just ci > log 2>&1; echo EXIT:$?`, then grep the log for
  `test result: FAILED` and `recipe .* failed`.
- **`cargo check` is not clippy.** `just ci` runs `clippy --all-targets
  --all-features -- -D warnings`, which denies lints a per-crate `cargo check`
  says nothing about. Run clippy over what you touched before reporting.
- **Set `CARGO_TARGET_DIR` when a `Cargo.toml` has just changed.** rust-analyzer
  rebuilds the workspace into the default `target/` and will race a CI run into
  a fingerprint write failure that looks like a build error.
- **`cargo clean` between long sessions.** `target/` reaches ~85 GB and a full
  disk fails as `No space left on device` inside a compile — which reads like a
  build error and is not one. Never clean while a run is in flight; it deletes
  the artifacts underneath it.

`cargo test --all-targets` **excludes doctests**; `just test-doc` is the recipe
that runs them.

## Non-negotiables

| Rule | Why |
|---|---|
| Business dates are **Europe/Berlin** — `mako_fristen::heute`, SQL `heute()` | a Frist, a Lieferbeginn and a Rechnungsdatum are German calendar dates; `now_utc().date()` is yesterday between 23:00 and midnight |
| **No `f64` touches money** — `billing::Amount<5>`, `Decimal` only at the JSON/storage boundary | a settlement must be reproducible to the cent |
| Rounding is **kaufmännisch** (DIN 1333) | pinned by `check-rounding` |
| Every `Json<T>` request body **denies unknown fields** | serde drops an unknown key silently, so a typo becomes a setting that does nothing |
| A multi-statement write is **one transaction** | a half-applied write leaves a state no reader can interpret |
| A missing input is a **refusal, not a zero** | a substituted price misprices every unit of the period |
| Identifier literals carry a **valid check digit** | `check-malo-ids` computes the right one for you when you get it wrong |

## Where to look

| Question | Source |
|---|---|
| What a service does, its port and its MCP surface | `README.md`, then `services/<name>/README.md` |
| How to operate it | `site/content/docs/services/<name>.md` |
| Architecture, domain model, engine | `site/content/docs/architecture/**` |
| Which PID belongs to which crate and workflow | `site/content/docs/regulatory/pid-reference.md` |
| Licence governance | `site/content/docs/compliance/licenses.md` |

Per-area guidance lives in a nested `AGENTS.md`: `crates/edi-energy/`,
`crates/mako-engine/`, `services/makod/`, `xtask/`, and the domain-workflow
crates. The closest one to the file being edited wins.

## Identifier check digits

BDEW identifiers carry check digits and mako validates them: **MaLo-ID** (11
digits, *Lok- und Waggon-Kennzeichnungsverfahren*: odd positions + even
positions×2, difference to the next multiple of ten — **not Luhn**) and **EIC**
(16 characters; object type `X` = Party/Bilanzkreis, `Y` = Area/
Bilanzierungsgebiet). `cargo xtask check-malo-ids` (in `just ci`) refuses a
literal with a wrong check digit anywhere in `crates/`, `services/`, `demos/`
or `site/content/`, using `metering` and `rubo4e` themselves so the guard cannot
disagree with the validators. Fixtures: `51238696012` is the canonical valid
MaLo; `51238696782` is the refusal fixture (wrong check digit) and is
allowlisted in the guard. Never invent an identifier — derive the check digit,
or take a published code.

## Toolchain and Edition

- Rust edition: **2024** (all crates)
- Toolchain: **1.94** (pinned in `rust-toolchain.toml` — do not change to `stable`)
- Components: `rustfmt`, `clippy`

---

## Active Format Versions

Format releases ship on a **semi-annual cadence (April + October)**. Profiles are
**per-message and fv-dated** (`crates/edi-energy/profiles/<message>/fv<yyyymmdd>/`);
a message type only gets a new fv directory when its format actually changes in a
release.

**Five** releases are active at once — a message type's newest profile stays
binding until a release changes that message type, so the oldest rows below are
as live as the newest.

| Release | Binding | Message types whose profile this release supplies |
|---|---|---|
| `fv20250401` | since 2025-04-01 | ORDCHG, UTILTS |
| `fv20251001` | since 2025-10-01 | APERAK, IFTSTA, PRICAT, QUOTES, REQOTE, UTILMD |
| `fv20260101` | since 2026-01-01 | CONTRL, INSRPT |
| `fv20260401` | since 2026-04-01 | COMDIS, INVOIC, MSCONS, ORDERS, ORDRSP, PARTIN, REMADV, UTILMD Gas |
| `fv20261001` | from 2026-10-01 | APERAK, IFTSTA, INVOIC, MSCONS, ORDCHG, ORDERS, ORDRSP, PARTIN, PRICAT, QUOTES, REQOTE, UTILMD, UTILMD Gas, UTILTS |

REMADV is **not** in the 2026-10-01 release: BDEW published no REMADV AHB or MIG
with that Anwendungszeitpunkt, so `remadv/fv20260401` (AHB 1.0a / MIG 2.9e) stays
binding. `crates/edi-energy/profiles/sources.json` is the authority for every row.

The `fv` date is the **Anwendungszeitpunkt**, six months after the document's Publikationsdatum (Allgemeine Festlegungen 6.1d §2.5). `mig.json` carries both: `publikationsdatum` (source metadata) and `valid_from` (normative).

Message types untouched by a release keep their previous profile. Multiple format
versions coexist in the same engine instance simultaneously. A process started under
an older format version continues under those rules until it completes, even after
a cutover.

---

## Code Conventions

### Error handling
- All public APIs return `Result<_, EngineError>` or `Result<_, WorkflowError>`.
- Use `thiserror` for error type definitions. Do not use `anyhow` inside library crates.
- `anyhow` is acceptable in `xtask` and `makod` (binary crates).
- Every `Result`-returning function must be annotated `#[must_use]`.

### Async
- All async code targets **Tokio** (version 1).
- Use async-fn-in-trait (AFIT) — stabilised at Rust 1.75, available on MSRV 1.94.
- Do not use `tokio::runtime::Handle::try_current()` as a runtime-detection backdoor.

### Types
- All IDs are UUID v4 newtypes defined via `define_id!` in `mako-engine/src/ids.rs`.
  Never accept or return plain `String` or `Uuid` where a typed ID belongs.
- Timestamps use `time::OffsetDateTime` — **not** `chrono::DateTime<Utc>`.
- EDIFACT payloads and event payloads use `serde_json::Value` — **not** `Vec<u8>` or `Bytes`.
- **`tenant: String`** is a **data-isolation key** written to every database row — it is NOT
  the BDEW-Codenummer. In demos it happens to equal the operator's BDEW-Codenummer for convenience,
  but it can be any stable unique string (e.g. a UUID, a slug). The BDEW-/DVGW-Codenummer belongs
  in `lf_mp_id`, `nb_mp_id`, `own_mp_id`, or `MarktpartnerId` fields — not in `tenant`.
  Document `tenant` as: `"Tenant identifier — data-isolation key written to every database row.
  Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid."`.
- Market participant identifiers use `MarktpartnerId` from `rubo4e::identifiers` — **not** `String` and
  **not** the removed `Gln` type alias. In BO4E the correct term is `MarktpartnerId` (= `rollencodenummer`
  in `Marktteilnehmer`). Only GS1-issued 13-digit codes are true GLNs (NAD DE3055 = `9`);
  BDEW-Codenummern (`99…`, NAD `293`) and DVGW-Codenummern (`98…`, NAD `332`) are not GLNs.
  Use `mako_markt::domain::nad_agency_code()` to derive the coding authority.
- BO4E Business Objects are imported directly from `rubo4e::current` (versioned) or
  `rubo4e::identifiers` (version-stable). **Never** write `rubo4e::v202607::Foo` — always use
  `rubo4e::current::Foo`. The `no-version-alias` CI gate enforces this.

  ```rust
  // Correct — version-stable identifiers
  use rubo4e::identifiers::{ObisCode, SrId, NeloId, MaloId};

  // Correct — versioned BOs via current alias
  use rubo4e::current::{Rechnung, PreisblattNetznutzung, Lastgang};

  // WRONG — hardcoded schema version
  // use rubo4e::v202607::Rechnung;
  ```

### Workflow determinism
- `Workflow::handle` and `Workflow::apply` must be **pure functions**: no I/O,
  no clock access, no global state mutation.
- All parsing, validation, and external calls happen before the command is
  constructed, at the transport boundary.

### Feature flags
- `slatedb` — opt in at the binary level only; never enable in library crate defaults.
- `testing` — enables `InMemoryXxx`/`NoopXxx` stores; must never appear in production builds.
- `tracing` — optional instrumentation; off by default.

### Service architecture (daemons)
Every daemon builds on the `mako-service` SDK. Do **not** hand-roll the lifecycle.

- **Bootstrap.** `fn main()` is one line: `mako_service::run::<MyDaemon>().await`. Implement the
  `Daemon` trait — supply `type Config`, `const NAME`, `migrate(&PgPool)` (usually
  `sqlx::migrate!(...)` + `outbox::ensure_schema`), and `build(cfg, ctx) -> anyhow::Result<Router>`
  (assemble the domain router and spawn workers on `ctx.shutdown`). `run()` owns tracing, the pool,
  migrations, `/health/*`, infra routes, graceful shutdown, and `--check`. **Never** add health routes,
  bind a listener, or call `serve` inside `build`.
- **Config shape.** Embed a `[database]` block (`pub database: DatabaseConfig`) and implement
  `ServiceConfig` (`database() -> Option<&DatabaseConfig>`, `bind_addr()`). A flat
  `database_url: String` is obsolete. A stateless daemon returns `database() -> None`.
- **Pool.** Obtain the pool only from the runner (`ctx.pool()`), which comes from
  `DatabaseConfig::connect(url, NAME)` (tuned sizing + `application_name`). Never call bare
  `PgPool::connect` / `PgPoolOptions` in a service.
- **HTTP errors.** Handlers return `ApiResult<T>` and use `?`; construct failures with `ApiError`
  (`NotFound`, `unprocessable(..)`, `conflict(..)`, …). Never build ad-hoc `(StatusCode, Json)` error tuples.
- **CloudEvent emission.** Build with `CloudEvent::new(source(svc, tenant), TYPE, subject, data)`
  (type constants from `mako-events`) and send via `post_ce_with_retry`. Never hand-roll a
  `json!({"specversion": ...})` envelope or compute a signature inline.
- **Transactional outbox.** Durable emitters persist-before-dispatch: `outbox::enqueue(&mut tx, &ce)`
  inside the same transaction as the business write, drained by a background `OutboxWorker`. This is
  the Postgres `event_outbox` mechanism for **service→ERP/webhook** events — distinct from the
  mako-engine `AtomicAppend::append_with_outbox` slatedb outbox for **protocol APERAK/CONTRL** (below).
- **HMAC.** Sign and verify webhooks only through `webhook::sign` / `webhook::verify_request`
  ([Standard Webhooks](https://www.standardwebhooks.com/): `webhook-id`,
  `webhook-timestamp`, `webhook-signature: v1,<base64>` over
  `{id}.{timestamp}.{body}`). Never hand-roll the check: `verify_request` also
  refuses a stale timestamp and returns the id to deduplicate on, and both are
  the halves a local copy forgets.

### Versioning
- Use **BDEW format versions** (`FV<YYYY>-<MM>-<DD>`) as version keys, not SemVer.
- Always use `FormatVersion::parse(...)` for user-supplied or deserialized strings.
- `FormatVersion::new(...)` is unchecked — only for known-valid compile-time literals.

---

## Domain Rules — Do Not Get Wrong

### PID ownership — authoritative table

| PID range | Crate | Source |
|---|---|---|
| 55001–55018, 55555 | `mako-gpke` | BK6-24-174 |
| 55039, 55042, 55051, 55168 | `mako-wim` | BK6-24-174 |
| 13003 | `mako-mabis` | BK6-24-174 |
| 44001–44021 | `mako-geli-gas` | BK7-24-01-009 |
| 44022–44024 | `mako-geli-gas` `geli-gas-stornierung` (any Nb role: 44022 inbound) / `geli-gas-stornierung-lf` (any Lf role: 44023/44024 inbound) — one owner for the GeLi Gas *and* the WiM Gas Use-Case | BK7-24-01-009 |
| 37000–37006 | `mako-gpke` (PARTIN Strom Kommunikationsdaten) | PARTIN AHB 1.0f |
| 37008–37014 | `mako-geli-gas` (PARTIN Gas Kommunikationsdaten) | PARTIN AHB 1.0f |
| 17115–17117 (Sperrung Strom, ORDERS) | `mako-gpke` | BK6-24-174 Anlage 1b Kap. 3.5 |
| 17115–17117 (Sperrung Gas, ORDERS) | `mako-geli-gas` | BK7-24-01-009 |
| 44039–44044, 44051–44053, 44168/44169, 44183 | `mako-wim` `wim-device-change` (same workflow as Strom 55039/55042/55051/55168) | AWH WiM Gas 2.0 |
| 31001–31002, 31005–31006 | `mako-gpke` (MMM-Rechnung / MMM-selbst ausgest. Rechnung Strom, NB → LF) | BK6-24-174 |
| 31007–31008 | `mako-gabi-gas` (Aggreg. MMM-Rechnung Gas / selbst ausgest., NB → MGV; Gas-only; MGV is a Gas-domain role) | BK7-24-01-008 |
| 13013 | `mako-gabi-gas` `gabi-gas-mmma` (Allokationsliste Gas, MMMA, Gas-only) | BK7-24-01-008 |
| 17110, 19110 | `mako-gpke` `gpke-allokationsliste` (ORDERS/ORDRSP Anforderung bilanzierte Menge, Gas twins of 17114/19115). `mako-gabi-gas` names them informational and registers MSCONS 13013 only — the ORDERS/ORDRSP AHB puts their formal home in the Gas MMMA process | BK7-24-01-008 |
| 31009 | `mako-wim` (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1 — routed via wim-invoic to avoid double-registration) | BK6-24-174 |
| 31003 | `mako-wim` `wim-invoic` (WiM-Rechnung Gas) | AWH WiM Gas 2.0 Kap. 4.7 |
| 31004 | `mako-wim` `wim-invoic` (Stornorechnung, Sparte-neutral) | INVOIC AHB §3.1.2 |
| 31010 | `mako-gabi-gas` (Kapazitätsrechnung, Kapazitätsabrechnung Gas) | BK7 |
| 31011 | `mako-geli-gas` (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF) | BK7-24-01-009 |
| 17134–17135 | `mako-gpke` (ORDERS Konfiguration, GPKE Teil 3) | BK6-22-024 |
| 19001–19002 | `mako-wim` (ORDRSP Geräteübernahme, WiM Strom) **and** `mako-gpke` (ORDRSP Konfiguration, NB role) — multi-domain: both "WiM Gas" and "WiM Strom Teil 1" per BDEW PID 3.3/4.0 xlsx | BK6-24-174 |
| 23001, 23003, 23004, 23005, 23008, 23009, 23011, 23012 | `mako-wim` `wim-insrpt` — one workflow, beide Sparten; die Frist folgt Messtechnik (Strom) bzw. ist flach (Gas). Not a span: 23002, 23006, 23007 and 23010 are unassigned | BK6-24-174 Anlage 2b / AWH WiM Gas 2.0 Kap. 4.3 |
| 23005, 23009 | `mako-wim` `wim-insrpt` — Gas-only Informationsmeldungen an den NB | AWH WiM Gas 2.0 Kap. 4.3 |

**PIDs that do NOT exist — never register:**
- 56001–56010: these PIDs were never assigned in any BDEW AHB document (confirmed absent from PID 3.3, 3.3 KL, PID 4.0, and all UTILMD AHB PDFs)
- 44555: does not exist in PID 3.3 or PID 4.0; Gas Sperrung process uses ORDERS PIDs 17115–17117
- 11001–11003: legacy pre-reform PIDs, superseded by 55039/55042/55051/55168
- 11004–11020 and 11024–11099: not in any WiM AHB. **11021–11023 do exist** —
  they are BDEW API-Webdienste PIDs, not EDIFACT ones, and `energy-api` serves
  them (`server/wim_order.rs`, `mako-wim::geraetewechsel`)
- 56101–56123 and 56201–56202: provisional Energy-Sharing PIDs. No BDEW AHB
  publishes them; § 42c runs inside the existing Lieferanten-/Bilanzkreis-
  zuordnung and introduces no new message family (BNetzA Mitteilung Nr. 73)
- 13024: not a Redispatch PID, and absent from the PID overview 4.0 entirely.
  **13025 is not in this list** — it is published (Lastgang Marktlokation,
  Tranche, MSB → LF, GPKE Teil 4) and is registered by `mako-gpke`
  `gpke-messwerte`. Write the Redispatch MSCONS range as „13020–13023, 13026",
  never as a span: 13020 and 13023 are MaBiS Summenzeitreihen routed to
  `mabis-billing`, and only 13021/13022 are `mako-redispatch`

**PIDs that exist but belong to WiM Gas, NOT GeLi Gas:**
- 44022–44024: role-conditional routing implemented in `mako-geli-gas`:
  - `Nb`-only: PID 44022 → `geli-gas-stornierung` (GNB receives Anfrage)
  - `Lf`-only: PIDs 44023/44024 → `geli-gas-stornierung-lf` (LF receives GNB response)
  - `Msb`/`Nmsb` add nothing: the recipient of a 44022 is the party that received the Ursprungsnachricht

### GeLi Gas 3.0
GeLi Gas is governed by **BK7-24-01-009** (Beschluss 12.09.2025), superseding BK7-19-001 and BK7-06-067. WiM Gas has no Festlegung — it is the industry **AWH WiM Gas 2.0**.
Scope: UTILMD G (PIDs 44001–44021) + UTILMD G PIDs 44022–44024 (role-conditional: `geli-gas-stornierung` for Nb, `geli-gas-stornierung-lf` for Lf) + ORDERS Sperrung Gas (17115–17117) + PARTIN Gas Kommunikationsdaten (37008–37014) + INVOIC 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF).
PID 31010 (Kapazitätsrechnung, NB → BKV) is a GaBi Gas (BK7-24-01-008) billing process and belongs to `mako-gabi-gas`.
PID 31011 (Rechnung sonstige Leistung, NB → LF) is billed by the GNB/VNB to the LFN/LFA for performing AWH (abrechnungswürdige Handlungen) during the Sperrprozess — it is a GeLi Gas (BK7-24-01-009) billing, NOT GaBi Gas.

### MABIS vs Messwesen
MaBiS (`mako-mabis`) covers MSCONS **13003** + **13010–13012** (Bilanzkreisabrechnung
Strom, BKV↔ÜNB/BIKO, `mabis-billing`), the UTILMD Clearinglisten
**55067/55069/55070/55073** (`mabis-clearingliste`), the ZP lifecycle
**55062–55064 / 55071–55072 / 55197–55200 / 55203–55214** (`mabis-zp-lifecycle`),
the ORDERS Anforderungen **17201–17208** (`mabis-anforderung`), and the
list/correction pairs **55065+55066, 55195+55196, 55201+55202, 55223+55224**
(`mabis-listenabgleich`).

**55065 is not a Clearingliste.** The Lieferantenclearingliste carries a
Prozessschritt-3 answer — 55066 „Korrekturliste zu Lieferantenclearingliste",
LF → NB — so it is a list/correction pair and belongs to `mabis-listenabgleich`;
`mako-mabis` holds a pin asserting it is not routed to `mabis-clearingliste`. The remaining 130xx Messwesen PIDs are **not**
MaBiS — do not register them under `mako-mabis`.

Three traps in that band, all verified against the PID overview 4.0:
**55218/55220 are GPKE Teil 2** (Abr.-Daten NNA), not MaBiS; **55215–55217, 55219,
55221, 55222 do not exist**; and **55064 is the shared Antwort to both 55062 and
55063**, so an answer PID is never derived from the request by arithmetic.
MaBiS IFTSTA PIDs are **21000–21005** (21006 does not exist; 21007 belongs to WiM Strom Teil 1 / WiM Gas, registered in `mako-wim` `wim-device-change`).

### Marktrollen (Rollenmodell V2.2) — authoritative role table

Source: BDEW-AWH Rollenmodell V2.2 (08.01.2026). Only roles with
`Marktkommunikation: zur Verwendung freigegeben` are listed.

| Abbreviation | Name | Sparte | Notes |
|---|---|---|---|
| `NB` | Netzbetreiber | Gas + Strom | In EDIFACT Gas AHBs sometimes qualified as `GNB` (Gasnetzbetreiber) |
| `LF` | Lieferant | Gas + Strom | In EDIFACT Gas AHBs sometimes qualified as `LFG` |
| `MSB` | Messstellenbetreiber | Gas + Strom | In EDIFACT Gas AHBs sometimes qualified as `GMSB` |
| `BKV` | Bilanzkreisverantwortlicher | Gas + Strom | Gas balancing handled via MGV/FNB framework |
| `ÜNB` | Übertragungsnetzbetreiber | Strom | Maps to `UNB` in config; `FNB` (Gas TSO) maps to `Uenb` in engine |
| `BIKO` | Bilanzkoordinator | Strom | BNetzA-governed; issues Abrechnungssummenzeitreihe (PID 13003) |
| `MGV` | Marktgebietsverantwortlicher | Gas | No engine deployment role |
| `KN` | Kapazitätsnutzer | Gas | GaBi Gas capacity booking; no engine deployment role yet |
| `DP` | Data Provider | Strom | UTILTS metering data distribution; no engine deployment role yet |
| `EIV` | Einsatzverantwortlicher | Strom | Redispatch 2.0 (`mako-redispatch` engine; EIV party integration pending) |
| `ESA` | Energieserviceanbieter des Anschlussnutzers | Strom | iMS / smart meter context |
| `RB` | Registerbetreiber | Gas + Strom | MaStR data registry; sparte-neutral |

**Roles that do NOT exist in Rollenmodell V2.2 — never use:**
- `NBG`, `MSBG`: these abbreviations do not appear in BDEW documents
- Sub-role qualifiers `GNB`, `LFG`, `GMSB`, `ANB`, `VNB`, `NMSB`, `AMSB`, `FNB` are
  EDIFACT-AHB sub-qualifiers or operational sub-types used in `[[party]]` config and
  NAD role fields — they are NOT standalone Rollenmodell roles.

### MP-ID formats and EDIFACT identification codes — never mix these up

Source: BDEW-AWH Identifikatoren V1.2 (07.02.2025) §2.2;
Allgemeine Festlegungen V6.1d (01.04.2026) §2.13, §3;
UTILMD AHB Gas 1.2 NAD+MS/MR tables.

#### BDEW-Codenummer vs. DVGW-Codenummer vs. GLN

| Type | Positions 1–2 | Digits | NAD DE3055 | UNB DE0007 | Registry |
|---|---|---|---|---|---|
| BDEW-Codenummer (Strom) | `99` | 13 | **`293`** | **`500`** | bdew-codes.de |
| DVGW-Codenummer (Gas) | `98` | 13 | **`332`** | **`502`** | codevergabe.dvgw-sc.de |
| GLN (GS1) | varies | 13 | **`9`** | **`14`** | GS1 |
| EIC | — | 16 | **`ZEW`** | — | ENTSO-E |

- NAD DE3055 and UNB DE0007 use **different code values** for the same organisation.
- `332` (DVGW in NAD DE3055) ≠ `502` (DVGW in UNB DE0007).
- `9` (GS1 in NAD DE3055) ≠ `14` (GS1 in UNB DE0007).
- In `services/makod/src/core/party_registry.rs` the agency code is auto-derived from the GLN
  prefix: `99…` → `"293"`, `98…` → `"332"`, other 13-digit → `"9"`, 16-char → `"ZEW"`.
- Each Marktrolle must have **exactly one MP-ID** (`"einem Marktteilnehmer kann für jede
  Marktrolle nur genau eine MP-ID zugeordnet sein"` — Identifikatoren AWH §2.1).
- UNB `NAD+MS` (sender) and `NAD+MR` (receiver) must use **identical** MP-IDs as the
  corresponding UNB DE0004/DE0010 sender/receiver fields (§2.13).

#### §2.12 Filename convention (Allgemeine Festlegungen V6.1d §2.12)

`<MsgType>_<SenderMPID>_<ReceiverMPID>_<YYMMDD>_<HHMM>_<Ref>.txt`
(`.txt.gz` when compressed)

#### §2.14 Publication requirement

- Only published MP-IDs may be used in production messages.
- Strom: https://bdew-codes.de/Codenumbers/BDEWCodes/CodeOverview
- Gas: https://codevergabe.dvgw-sc.de/MarketParticipants
- Operator must be reachable within **3 Werktage** after initial contact (§2.14).

### EDIFACT time encoding — never mix UTC and local time

Source: Allgemeine Festlegungen V6.1d §3.

- All **EDIFACT times are in UTC** (DTM qualifier 303: `CCYYMMDDHHMMZZZ`, ZZZ always `+00`).
- Process **deadlines** use **gesetzliche deutsche Zeit** (CET = UTC+1, CEST = UTC+2).
- An off-by-one-hour error at DST transitions is a **regulatory deadline violation**.

| Sparte | Event | UTC MEZ (CET) | UTC MESZ (CEST) |
|---|---|---|---|
| Strom | Lieferbeginn/-ende (Mitternacht) | `2300` | `2200` |
| Gas | Gastag-Beginn (06:00 local) | `0500` | `0400` |

- Bilanzierungsmonat uses DTM qualifier **610**: `DTM+492:202106:610'`
- `DE0035 = 1` in UNB marks a **test message** (do not process as production).

### APERAK Fristen — never mix these up

#### APERAK *sending* deadline (how quickly the receiver must send the APERAK)
Per **APERAK AHB 1.1** (FV2026-04-01):

| Sparte | Message type | Deadline | Source |
|---|---|---|---|
| **Strom** | UTILMD / ORDERS | **45 Minuten** | APERAK AHB 1.1 §2.4.1 |
| **Strom** | UTILMD / ORDERS **an Samstagen** | **Sonntag 12 Uhr** | APERAK AHB 1.1 §2.4.1 |
| **Strom** | all other | **nächster Werktag 12 Uhr** | APERAK AHB 1.1 §2.4.1 |
| **Gas** | Folgeprozesse | **nächster Werktag 12 Uhr** | APERAK AHB 1.1 §2.3.1 |
| **Gas** | Initialprozesse | **3 Werktage**, to the **end** of the third | APERAK AHB 1.1 §2.3.1 |

Two readings the document does not support, both of which have been in this
table: the 45 minutes carries **no Montag–Freitag qualifier** — §2.4.1 states it
unconditionally and carves out **Samstag** alone, so Sunday is not a special
case; and the Gas Initialprozess window names **no clock time**, so it runs to
the end of the third Werktag and not to 12 Uhr on it. The „12 Uhr" belongs to
the Folgeprozess sentence in the paragraph above it.

Gas APERAKs are always **Verarbeitbarkeitsfehlermeldungen** (BGM+313) only — no Anerkennungsmeldung.
Strom APERAKs include **both** Anerkennungsmeldung (BGM+312, accepted) and Verarbeitbarkeitsfehlermeldung (BGM+313, rejected).
Gas CONTRL rule: "Auf eine APERAK ist immer eine CONTRL zu senden." (APERAK AHB 1.0 §2.3, CONTRL AHB 1.0 §2.3.1)

#### Process *response* deadline (how long the business process can take overall)
These are NOT APERAK deadlines. Never use these as the APERAK-sending window.

Every business window lives in `mako_fristen::antwort` (arrival-anchored) or
`mako_fristen::vorlauf` (anchored on a date the message carries). Read them from
there — a literal beside a call site is how the two come to disagree.

| Process | Deadline | Source |
|---|---|---|
| GPKE Strom | **a clock time on the 1. WT nach dem ÜT** — 11:00 Anmeldung (55001/55077), 06:00 Abmeldung (55004), 05:00 Lieferende NB→LF (55007), 09:00 Beendigung der Zuordnung (55010) | BK6-24-174 GPKE Teil 2 |
| GeLi Gas | Ablauf des **4. WT** Anmeldung (44001), **3. WT** Abmeldung (44004), **2. WT** Ersatz-/Grundversorgung (44013), **3. WT** Kündigung (44016) | BK7-24-01-009 Kap. 3.1–3.3 |
| WiM, beide Sparten | **3 / 5 / 7 / 1 Werktage je PID** (55039/55042/55051/55168 resp. 44039/44042/44051/44168) | BK6-24-174 Anlage 2a Kap. 2.2.2–2.5.2 · AWH WiM Gas 2.0 |
| MaBiS (Prüfmitteilung) | **1 Werktag** | BK6-24-174 Anlage 3 §13.8 |

**Saturday is not a Werktag.** GPKE Teil 1 Kap. 1.7: „alle Tage …, die kein
Samstag, Sonntag oder gesetzlicher Feiertag sind". A holiday observed in any
single Bundesland counts nationwide; 24.12. and 31.12. count as holidays. All
deadline arithmetic runs in **gesetzlicher deutscher Zeit** (CET/CEST), not UTC.
An off-by-one-hour error at DST transitions is a regulatory deadline violation.

### Format-version coexistence
A process keeps the `WorkflowId` — and therefore the format version — it was
created with, while the **inbound message's own** FV selects the
`MessageAdapter` that parses it. So a process started under `FV2026-04-01`
accepts a counterparty's `FV2026-10-01` APERAK without anything being declared:
there is no per-workflow acceptance policy, because every known FV must be
covered by an adapter anyway. `makod::startup::validate_adapter_coverage`
refuses to boot on a registry that leaves one uncovered.

### Dual-write atomicity
Events and outbox entries must be written in a single `WriteBatch` via
`AtomicAppend::append_with_outbox`. Never write events first and outbox second —
a crash between the two produces a lost APERAK with no recovery path.

---

### Known-wrong legal claims — treat any recurrence as High

Every row is a statutory near-miss this codebase actually shipped or nearly
shipped. They are here rather than in a review checklist because a plausible `§`
is the most expensive output available in this domain: it is indistinguishable
from a real one to any reader who does not hold the statute, and in a settlement
engine it silently becomes money.

| Wrong claim | Verified reality |
|---|---|
| "§41 Abs. 3 EnWG = 6-week price-change notice" | §41 Abs. 3 is an advertising-information duty. Six weeks is **§5 Abs. 2 StromGVV/GasGVV** (Grundversorgung); Sonderverträge: **§41 Abs. 5 EnWG** — ≥1 month for Haushaltskunden, ≥2 weeks otherwise, plus Sonderkündigungsrecht |
| "§38a EEG 2023 = Mieterstromzuschlag" | §38a is Zahlungsberechtigungen for first-segment solar tenders. Mieterstromzuschlag is **§21 Abs. 3 EEG 2023** (rate via §48a) |
| "MaBiS vorläufig day 3 / endgültig day 8" | BK6-24-174 Anlage 3 §3.10 counts **Werktage**: Erstaufschlag ≤ 10 WT, BKA-Clearing ≤ 30 WT, then KBKA |
| "§29 MsbG mandatory iMSys 7–100 kW band" | Current §29 Abs. 1 Nr. 2b has only a **> 7 kW** lower bound; no 100 kW cap exists |
| "§42c Energy Sharing reduces Netzentgelte" | No reduction exists in current law — full Netzentgelte apply (BK6 Mitteilung Nr. 73, BK6-06-009) |
| "2022 heating-gas Energiesteuer = 0 (Energiesteuersenkungsgesetz)" | The 2022 cut (BGBl. I 2022 S. 810, Jun–Aug) hit **motor fuels** only; §2 Abs. 3 Nr. 4 heating gas stayed 0.55 ct/kWh. The real gas reliefs: EWSG Dezemberhilfe + **7 % USt 01.10.2022–31.03.2024** (§28 Abs. 5/6 UStG) |
| Any "§… MessZV" citation | The **MessZV was repealed** by Art. 12 G. v. 29.08.2016 (folded into the MsbG). Living anchors: Ersatzwertbildung/Plausibilisierung **§ 60 Abs. 2 MsbG**; Messwert-Audit/Löschfrist **§ 60 Abs. 6 MsbG**; RLM/Spitzenleistung **§ 12 StromNZV**; MMM **§ 13 StromNZV**; business-record retention **§ 147 AO / GoBD**. ~500 dead citations were swept in 07/2026 — treat any reappearance as High |
| "§40a EnWG = Abschlagszahlungen" | §40a is **Verbrauchsermittlung**. Abschlag rules: §13 StromGVV/GasGVV (via §41 EnWG for Sonderverträge). Deadlines + 2-week due-date rule: **§40c** (3 weeks for monthly billing) |
| "invoice content (Kilowattstundenpreis, Verbrauchshistorie, Zählerstände) = §40a / §41 EnWG" | Invoice **content** is **§40 EnWG**: all-inclusive kWh-price = §40; Zählerstände = §40 Abs. 2 Nr. 6; Vorjahresvergleich = Nr. 7; Vergleichsgruppe = Nr. 8. §40a = Verbrauchsermittlung (estimation); §41 = supply-**contract** content, not the invoice |
| "dynamic-tariff iMSys requirement = §41b EnWG" | §41b is Haushaltskunden-Lieferverträge außerhalb der Grundversorgung. The iMSys precondition for §41a dynamic tariffs is **§41a Abs. 1 EnWG** (+ MsbG rollout) |
| "§53b EEG = regional Grünstromkennzeichnung / a BNetzA-certified grid-area reduction at a configurable rate" | §53b is **Regionalnachweise** (§79a EEG): a fixed **0,1 ct/kWh** cut to the anzulegender Wert, only "bei Anlagen, deren anzulegender Wert **gesetzlich bestimmt** ist" — never a tender-awarded AW, never a grid area, never a caller-supplied rate |
| "§53c EEG = structural-oversupply reduction, not yet operational" | §53c is **Verringerung des Zahlungsanspruchs bei einer Stromsteuerbefreiung**: the AW drops by the per-kWh exemption granted for grid-transited electricity exempt under the **StromStG** (§3 full rate 20,50 EUR/MWh = 2,05 ct/kWh), *not* the EnergieStG. Operative law; disabling it under-deducts and overpays |
| "§54 EEG = generic BNetzA Ausschreibungsreduzierung (§36d deadlines, §37a iMSys)" | §54 is **Ausschreibungen für Solaranlagen des ersten Segments** only, with four Absätze: −0,3 ct late Zahlungsberechtigung (>18 Kalendermonate), −0,3 ct Flurstück mismatch, −2,5 ct missing Agri-PV Nutzungsnachweis, AW → 0 for a §37c Abs. 2 Landesverordnung breach |
| "§24 EEG Anlagenzusammenfassung requires operator identity" | Satz 1 opens "**unabhängig von den Eigentumsverhältnissen**". The four cumulative tests are site, gleichartige Energien, size-dependent claim, and a twelve-calendar-month window; Sätze 2–5 then carve out biogas from one Biogaserzeugungsanlage, Freifläche vs. building solar, differing Netzverknüpfungspunkte, and small Steckersolargeräte. Keying on the operator under-fuses, which overpays |
| "A §-based reduction can be subtracted from the settled euro amount" | §§53, 53b, 53c and 54 all reduce the **anzulegender Wert**. The gleitende Marktprämie is `max(0, AW + Managementprämie − Marktwert)`, so a deduction taken after the floor drives the settlement negative — charging the operator for feeding in. Only §52 Pflichtzahlungen are a euro-level offset (Abs. 6) |
| "Zuschlag-Erlöschen = §35a EEG (or §33, or §55 Pönalen)" | Expiry for want of timely commissioning is **technology-specific**: §36e Wind an Land, §37e Solaranlagen des ersten Segments, §39e Biomasseanlagen. §35a is **Entwertung von Zuschlägen** (a BNetzA act); §33 is **Ausschluss von Geboten** (before any award exists); §55 Pönalen are a bidder↔ÜNB obligation outside settlement entirely |
| "A `None` from a period-rate helper can fall back to a default rate" | Those helpers return `None` to say **no single rate is correct for the period**. Answering it with `.unwrap_or(default)` bills part of the period wrong and reads exactly like a correct invoice downstream — a silent customer overcharge. Refuse the period and name the Stichtage (`steuer_stichtage_im_zeitraum`) |
| "§40c EnWG's three-week deadline follows from a short billing period" | The three weeks attach to **§40b Abs. 1 monthly billing** — the agreed cadence — not to the period's length. A Schlussrechnung always has six weeks, measured from the end of the **Lieferverhältnis**, however short the final period is |
| "§14a EnWG Modul 2 = zeitvariable/HT-NT Netzentgelte; Modul 3 = Spotpreis or Steuerungsentschädigung" | Numbered by **BK8-22/010-A** (NSAVER, 23.11.2023), not BK6-22-300 (the companion Festlegung for the netzorientierte Steuerung). **Modul 1** pauschale Netzentgelt-Reduzierung (default, no extra metering); **Modul 2** prozentuale Arbeitspreis-Reduzierung on the device's *separately metered* energy; **Modul 3** zeitvariable Netzentgelte, **three** Tarifstufen HT/ST/NT, from 01.04.2025, iMSys required. **2 and 3 are mutually exclusive**; 1 combines with either. All three are rate reductions — a Steuerungsentschädigung is not a module |
| "the Modul-2 percentage is the Netzbetreiber's to publish" | **BK8-22/010-A Tenor 2. b) fixes it**: „Der reduzierte Arbeitspreis entspricht **40%** des Arbeitspreises für die Entnahme ohne Leistungsmessung des Netzbetreibers in der Niederspannung.“ Tenor 2. c) makes Modul 2 verpflichtend ab 01.01.2024 and Tenor 2. d) forbids a Grundpreis on such a Marktlokation. Only the *reference* Arbeitspreis is the operator's; the percentage is not |
| "The ESA Werteanfrage shares REQOTE 35002 with the Preisanfrage, because no ESA-specific REQOTE PID exists" | It is **35003**. REQOTE AHB 1.1 §4.3 gives the Kommunikation as *ESA an MSB* and labels `SG1 RFF+Z13` "35003 Anfrage von Werten für ESA"; §4.2 **35002** is "Anfrage zur Rechnungsabwicklung des Messstellenbetriebs über den LF", **LF → MSB**, WiM Teil 1. Corroboration: `PIA` is *mandatory* on 35003 — exactly the segment the old heuristic sniffed for. REQOTE↔QUOTES pair 3500n → 1500n |
| "WiM MSB-Wechsel responses are 5 Werktage" | **Per PID, four separate Use-Cases** (WiM Teil 1): Kündigung 55039 **3 WT** (Kap. 2.2.2 Nr. 2), Beginn 55042 **5 WT** (2.3.2 Nr. 2), Ende 55051 **7 WT** (2.4.2 Nr. 2), Verpflichtungsanfrage 55168 **1 WT** (**2.5**.2 Nr. 4 — not 2.4). A flat window escalates the Abmeldung two days early and hides a missed Verpflichtungsanfrage for four. Not the **APERAK** window either: that is 45 minutes for Strom UTILMD (§2.4.1), never Werktage |
| "INVOIC 31009 (MSB-Rechnung) is NB → MSB" | It is **MSB → NB / LF / ESA** — the MSB is the invoicer in all **seven** Anwendungsfälle of the PID overview 4.0 (GPKE Teil 3 ×2, WiM Teil 1 ×2, WiM Teil 2 ×1, AWH Änderung Technik ×2), Strom only. Modelling it inverted names the party owed money as the one billing for it. The recipient's role varies, so it cannot be a bare `nb_mp_id` |
| "A MIG defines where a data element sits in a segment" | A MIG lists which elements a profile **uses**; the **position** is fixed by the UN/EDIFACT directory and is what the counterparty writes. Generating positions from the MIG's list order shifts everything after an omitted element — REQOTE's `FTX.C108` landed at 2 instead of 4 and mako **rejected valid inbound** `FTX+ACB+++text`. A missing element is a different defect: fix the profile against the MIG PDF, not the builder |
| "Blindmehrarbeit rests on StromNEV §18" | §18 StromNEV is the **Entgelt für dezentrale Erzeugung** (the crate's own `sect18.rs` says so). Reactive-energy excess is charged from the Netzbetreiber's **Preisblatt**, formed under StromNEV §17. §19 is Sonderformen der Netznutzung. The free share (cos φ 0,9 → tan φ ≈ 0,4843, often rounded to 50 %) is a price-sheet term and must be an input, not a constant |
| "Parse-don't-validate applies uniformly to inbound and outbound" | It does not. A value the system **produces** should be a validating newtype (`MabisZaehlpunktId`) so a malformed one is unconstructible. A value it **receives** must stay representable — requiring the type on an inbound command leaves the workflow unable to record what arrived and therefore unable to reject it properly. Type the outbound side; keep the inbound side raw and refuse explicitly |
| "A DB `CHECK` is enough to protect an identifier that reaches the wire" | A `CHECK` only guards rows written to *that* table. A payload assembled from a fixture, a replay, or a caller passing a value straight through never meets it. MSCONS SG6's `LOC+172`/`107`/`237` are free text at the MIG level, so a swapped pair parses, validates and is **accepted by the BIKO** — the guard has to live in the pure crate as well (`Summenzeitreihe::validate_identifiers`) |
| "A dependency's `validate()` enforces our profile's security mandate" | Library validation encodes the *generic* floor, not your profile's mandate. `asx-rs` rejects an AS4 policy layer only when it disables signing **and** encryption; BDEW AS4-Profil v1.2 §2.2.6.2.2 requires **both**, so a sign-only override validates cleanly and would send in the clear. Assert the mandate over base *and* every override layer (`BdewAs4Profile::validate` → `SecurityFloorViolation`), and pin the gap with a test that the upstream check still accepts what you reject |
| "§13a Abs. 2 EnWG compensation uses one Ausfallarbeit basis" | The counterfactual differs by redispatch case: **Duldungsfall** derives it from the measured Lastgang (the NB steered, so nothing was transmitted), **Aufforderungsfall** from the schedule transmitted to the EIV (that schedule *is* the counterfactual). Resolving both from the Lastgang settles an Aufforderungsfall against what happened rather than what was instructed — a money error nothing downstream detects. `AusfallarbeitBasis` is a required input, carried into the result and trace |
| "Gas NNE Grundpreis / Arbeitspreis rests on §14 GasNEV" | **§14 GasNEV is *Teilnetze*** — cost allocation when a Betreiber has formed Teilnetze under §6 Abs. 5 GasNZV. Netzentgelte are **§15 GasNEV** (*Ermittlung der Netzentgelte*), and the Verrechnungspreis specifically **§15 Abs. 7**: „Für leistungsgemessene Ausspeisepunkte sind … ein Entgelt für den Messstellenbetrieb, ein Entgelt … für die Messung und ein Entgelt für die Abrechnung festzulegen." The wrong § rode into the audit trace of every gas NNE invoice |
| "Gemeinschaftliche Gebäudeversorgung is §42b **EEG 2023**" | There is no §42b EEG 2023. GGV is **§42b EnWG**. §42a EnWG is Mieterstrom, §42c Energy Sharing; the Mieterstromzuschlag is §21 Abs. 3 EEG 2023 |
| "The Strom UTILMD/ORDERS APERAK window is 45 Minuten *on weekdays (Mo–Fr)*" | APERAK AHB 1.1 §2.4.1 states the 45 minutes **unqualified** and carves out **Samstag** alone („Wird an Samstagen eine UTILMD oder ORDERS übertragen … bis zum Sonntag, 12 Uhr"). There is no Montag–Freitag restriction in the document, so Sunday is not a special case. The invented qualifier was carried by an equally invented *verbatim quotation* — never attribute words to an AHB without grepping the PDF |
| "The Gas APERAK Initialprozess window is 3 Werktage **at 12 Uhr**" | §2.3.1 says „spätestens **3 Werktage nach Eingang**" and names no clock time; the „12 Uhr" belongs to the Folgeprozess sentence above it. Truncating to noon removes twelve hours in the tightening direction — a breach the counterparty is not in |
| "A Werktage-Antwortfrist expires at a 17:00 Europe/Berlin ‚MaKo cut-off'" | No BDEW or BNetzA document in this domain states one. „**Ablauf des** n. WT" (WiM, GeLi Gas) and „spätester **ÜT** ist der n. WT" (GPKE) are **one** Frist shape, not two, and both name a **day** — so it runs to the end of that day. 17:00 expired 42 obligations seven hours early, escalating counterparties still inside their Frist |
| "13025 is not a real PID" | **13025 is published** (Lastgang Marktlokation/Tranche, MSB → LF, GPKE Teil 4) and is registered by `mako-gpke` `gpke-messwerte`. **13024** is the one absent from the PID overview. Likewise 13020 and 13023 are MaBiS Summenzeitreihen routed to `mabis-billing` — only 13021/13022 are `mako-redispatch` |
| "§12 Abs. 3 UStG 0 % applies to PV electricity / feed-in ≤ 30 kWp" | §12 Abs. 3 zero-rates the **supply of the PV system** (modules/storage/installation), NOT electricity or feed-in remuneration. A retail **consumption** supply is always standard-rated even for a prosumer. A small operator's **feed-in Gutschrift** is 0 % only via the **Kleinunternehmerregelung §19 UStG** — an election (`kleinunternehmer_19_ustg`), not a function of plant size |

## Licenses

`deny.toml` holds the allow-list — thirteen SPDX identifiers, each permissive,
three of them (`0BSD`, `bzip2-1.0.6`, `CC0-1.0`) carrying a note on the
transitive dependency that introduced it. `cargo deny` enforces it and
`check-licenses` pins the operator-facing copy in
`site/content/docs/compliance/licenses.md`. Read `deny.toml`; do not keep a
fourth copy of the list here.

---
