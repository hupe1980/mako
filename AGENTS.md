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
- **Every regulatory claim cites a primary source** — document, chapter, page.
  A `§` reaches the codebase from a published document or not at all. Guessing a
  plausible one is the most expensive mistake available here; see the
  known-wrong citations in *Domain Rules* below.
- **A defect class becomes a guard.** When something is found, the deliverable is
  the check that makes it unrepresentable — which is why `just ci` carries 26 of
  them and the list grows with the defect list, not the feature list.

## Build and test

`just ci` is the gate. It is the whole suite plus every guard, and nothing is
"done" until it passes.

```bash
just check      # cargo check --all-targets --all-features — the minimum
just ci         # the gate: test, doctests, clippy, deny, 26 guards, site-free
just check-site # mermaid + link + zola checks; NOT part of `just ci`
just test-db    # schema-per-test suites against real PostgreSQL (needs Docker)
```

Three traps, each of which has produced a false "green":

- **Never pipe `just ci` through `tail`/`grep` for the verdict** — the pipe masks
  the exit code. Run `just ci > log 2>&1; echo EXIT:$?`, then grep the log for
  `test result: FAILED` and `recipe .* failed`.
- **`cargo check` is not clippy.** `just ci` runs `clippy --all-targets
  --all-features -- -D warnings`, which denies lints a per-crate `cargo check`
  says nothing about. Run clippy over what you touched before reporting.
- **Set `CARGO_TARGET_DIR` when a `Cargo.toml` has just changed.** rust-analyzer
  rebuilds the workspace into the default `target/` and will race a CI run into
  a fingerprint write failure that looks like a build error.

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
| The regulatory source corpus (index + download URLs) | `regulatories/README.md` |

Deeper design notes may exist under `concepts/` — role docs, billing, EDMD, the
agent plane, the backlog. That directory is **not tracked**, so it is absent from
a fresh clone and nothing here depends on it.

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

| Release | Binding | Message types with changed formats |
|---|---|---|
| `fv20260401` | since 2026-04-01 | COMDIS, INVOIC, MSCONS, ORDERS, ORDRSP, PARTIN, REMADV, UTILMD Gas |
| `fv20261001` | from 2026-10-01 | APERAK, IFTSTA, INVOIC, MSCONS, ORDCHG, ORDERS, ORDRSP, PARTIN, PRICAT, QUOTES, REMADV, REQOTE, UTILMD, UTILTS |

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
- **HMAC.** Sign and verify webhooks only through `webhook::sign` / `webhook::verify_hmac`
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
| 17115–17117 (Sperrung Strom, ORDERS) | `mako-gpke` | BK6-22-024 |
| 17115–17117 (Sperrung Gas, ORDERS) | `mako-geli-gas` | BK7-24-01-009 |
| 44039–44044, 44051–44053, 44168/44169, 44183 | `mako-wim` `wim-device-change` (same workflow as Strom 55039/55042/55051/55168) | AWH WiM Gas 2.0 |
| 31001–31002, 31005–31006 | `mako-gpke` (MMM-Rechnung / MMM-selbst ausgest. Rechnung Strom, NB → LF) | BK6-24-174 |
| 31007–31008 | `mako-gabi-gas` (Aggreg. MMM-Rechnung Gas / selbst ausgest., NB → MGV; Gas-only; MGV is a Gas-domain role) | BK7-24-01-008 |
| 13013 | `mako-gabi-gas` `gabi-gas-mmma` (Allokationsliste Gas, MMMA, Gas-only) | BK7-24-01-008 |
| 17110, 19110 | `mako-gabi-gas` `gabi-gas-mmma` (ORDERS/ORDRSP Allokationsliste Gas, Gas-only; ⚡=— in AHB 1.0) | BK7-24-01-008 |
| 31009 | `mako-wim` (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1 — routed via wim-invoic to avoid double-registration) | BK6-24-174 |
| 31003 | `mako-wim` `wim-invoic` (WiM-Rechnung Gas) | AWH WiM Gas 2.0 Kap. 4.7 |
| 31004 | `mako-wim` `wim-invoic` (Stornorechnung, Sparte-neutral) | INVOIC AHB §3.1.2 |
| 31010 | `mako-gabi-gas` (Kapazitätsrechnung, Kapazitätsabrechnung Gas) | BK7 |
| 31011 | `mako-geli-gas` (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF) | BK7-24-01-009 |
| 17134–17135 | `mako-gpke` (ORDERS Konfiguration, GPKE Teil 3) | BK6-22-024 |
| 19001–19002 | `mako-wim` (ORDRSP Geräteübernahme, WiM Strom) **and** `mako-gpke` (ORDRSP Konfiguration, NB role) — multi-domain: both "WiM Gas" and "WiM Strom Teil 1" per BDEW PID 3.3/4.0 xlsx | BK6-24-174 |
| 23001–23012 | `mako-wim` `wim-insrpt` — one workflow, beide Sparten; die Frist folgt Messtechnik (Strom) bzw. ist flach (Gas) | BK6-22-024 Anlage 2b / AWH WiM Gas 2.0 Kap. 4.3 |
| 23005, 23009 | `mako-wim` `wim-insrpt` — Gas-only Informationsmeldungen an den NB | AWH WiM Gas 2.0 Kap. 4.3 |

**PIDs that do NOT exist — never register:**
- 56001–56010: these PIDs were never assigned in any BDEW AHB document (confirmed absent from PID 3.3, 3.3 KL, PID 4.0, and all UTILMD AHB PDFs)
- 44555: does not exist in PID 3.3 or PID 4.0; Gas Sperrung process uses ORDERS PIDs 17115–17117
- 11001–11003: legacy pre-reform PIDs, superseded by 55039/55042/55051/55168
- 11004–11099: reserved but not in current WiM AHB

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
Strom, BKV↔ÜNB/BIKO, `mabis-billing`), the UTILMD Clearinglisten **55065/55069/55070**
(`mabis-clearingliste`), the ZP lifecycle **55062–55064 / 55071–55072 / 55197–55200 /
55203–55214** (`mabis-zp-lifecycle`), the ORDERS Anforderungen **17201–17208**
(`mabis-anforderung`), and the list/correction pairs **55195+55196, 55201+55202,
55223+55224** (`mabis-listenabgleich`). The remaining 130xx Messwesen PIDs are **not**
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
Per **APERAK AHB 1.0** (FV2025-10-01):

| Sparte | Message type | Deadline | Source |
|---|---|---|---|
| **Strom** | UTILMD / ORDERS (weekday) | **45 Minuten** | APERAK AHB 1.0 §2.4.1 |
| **Strom** | UTILMD / ORDERS (Saturday) | **Sonntag 12 Uhr** | APERAK AHB 1.0 §2.4.1 |
| **Strom** | all other | **nächster Werktag 12 Uhr** | APERAK AHB 1.0 §2.4.1 |
| **Gas** | Folgeprozesse | **nächster Werktag 12 Uhr** | APERAK AHB 1.0 §2.3.1 |
| **Gas** | Initialprozesse | **3 Werktage** | APERAK AHB 1.0 §2.3.1 |

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
| WiM, beide Sparten | **3 / 5 / 7 / 1 Werktage je PID** (55039/55042/55051/55168 resp. 44039/44042/44051/44168) | BK6-22-024 Anlage 2a Kap. 2.2.2–2.5.2 · AWH WiM Gas 2.0 |
| MaBiS (Prüfmitteilung) | **1 Werktag** | BK6-24-174 Anlage 3 §13.8 |

**Saturday is not a Werktag.** GPKE Teil 1 Kap. 1.7: „alle Tage …, die kein
Samstag, Sonntag oder gesetzlicher Feiertag sind". A holiday observed in any
single Bundesland counts nationwide; 24.12. and 31.12. count as holidays. All
deadline arithmetic runs in **gesetzlicher deutscher Zeit** (CET/CEST), not UTC.
An off-by-one-hour error at DST transitions is a regulatory deadline violation.

### Format-version coexistence
`WorkflowVersionPolicy::ForwardCompatible` is the correct default for **all** MaKo
workflows. Do not default to `Pinned`.

### Dual-write atomicity
Events and outbox entries must be written in a single `WriteBatch` via
`AtomicAppend::append_with_outbox`. Never write events first and outbox second —
a crash between the two produces a lost APERAK with no recovery path.

---

## Licenses

Only these SPDX identifiers are allowed (enforced by `cargo deny`):
MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause,
ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0, MIT-0.

---
