# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`ReleaseTrack` enum** (`Strom`, `Gas`, `Classic`, `Short`, `Other`) in
  `edi_energy::ReleaseTrack` with `ReleaseKind::track()` and `Release::track()`
  accessors.  Replaces string-prefix matching with a typed discriminant.
- **`Release::is_registered(message_type, registry)`** — returns `true` when the
  registry contains at least one profile for the given `(MessageType, Release)` pair.
- **`EdiEnergyReport::issues_by_origin(origin)`** — zero-allocation iterator over
  validation issues from a specific layer (`"parse"`, `"directory"`, `"mig"`,
  `"ahb"`, `"custom"`). Enables monitoring dashboards to separate structural errors
  from conformance violations without post-hoc `rule_id` string matching.
- **`ValidationIssueSummary::rule_origin`** field — populated automatically from
  the `rule_id` prefix, classifying each issue's validation layer.
- **`cargo xtask generate-fixtures`** — generates minimal synthetic `.gen.edi`
  test fixtures for every Prüfidentifikator that lacks a hand-crafted fixture.
  Raises `validate-pruefids` coverage from 9 % (20/220) to 100 % (220/220).
  Generated fixtures are placed in `tests/fixtures/<type>/gen/` to keep them
  visually distinct from authoritative hand-crafted artefacts.
- **`cargo xtask extract-pdf --min-segments N --min-pids N`** — new quality-gate
  flags that abort the extraction before writing any draft file if the parser
  produced fewer than N segment entries or M Prüfidentifikatoren. Prevents silent
  partial-extraction overwrites when a BDEW PDF layout change degrades output.
- **Schema version history** section in `docs/schema-versioning.md` documenting
  the v1 field inventory and contributor guidance for future version bumps.
- **Gas API scope section** in `docs/api-webdienste.md` clarifying that no BDEW
  Gas API-Webdienste specification has been published; Gas iMS processes continue
  via EDIFACT over AS4.
- **`contrl_same_wire_code_disambiguation` test** in `tests/registry.rs`: asserts
  that `profile_on(Contrl, "2.0b", date)` returns `fv20260101` for 2026+ dates
  and `fv20251001` for 2025 dates (with `contrl-archive` feature).
- **Large-fixture benchmarks**: 1 000-message MSCONS interchange throughput,
  `bench_light_vs_full_parse` (`parse_envelope_only` vs full parse), and
  `bench_validate_multi_pid` (per-PID rule-cost variance) added to
  `benches/benchmarks.rs`.

### Changed

- **BREAKING** `EdiEnergyMessage::message_type()` removed.  Use
  `try_message_type() -> Option<MessageType>` instead.  The `Option` is `None`
  only for `AnyMessage::Unknown`; all concrete message structs always return
  `Some(…)`.
- **BREAKING** `ReleaseRegistry::profile_for_date_and_track_prefix(date, msg_type, prefix)` removed.
  Use `profile_for_date_and_track(date, msg_type, &ReleaseTrack)` with the typed
  `ReleaseTrack` enum.  `ProcessContext::active_release_for_track` and
  `active_profile_for_track` updated accordingly.
- `ReleaseKind::Opaque` now emits a `tracing::warn!` (under the `tracing` feature)
  when produced, surfacing unrecognised release codes in production observability.
- `Platform::parse()` and `Platform::parse_with_config()` now record
  `message_type`, `release`, and `pruefidentifikator` as structured tracing span
  fields (requires `tracing` feature).
- `MessageCore::new()` now caches `has_interchange_wrapper: bool` at construction
  time to avoid rescanning the segment list on every `validate()` call.
- Generated profile modules now emit `pub(crate) const CODEGEN_SCHEMA_VERSION: u32 = 1`
  with a compile-time assertion in `mod.rs` — schema drift causes a compile error
  rather than a silent mismatch.
- `EXPECTED_ORDER` in generated modules now includes group-trigger segments (e.g.
  UTILMD: `RFF`, `NAD`, `IDE`) that were previously missing, enabling correct MIG
  structural ordering checks.
- `DirectoryValidator` cached in a `static LazyLock` per generated profile module;
  first-call allocation only.

- **APERAK 2.0a** profile with MIG, AHB (PIDs 29001–29002), and codelists

- **APERAK 2.0a** profile with MIG, AHB (PIDs 29001–29002), and codelists
- **CONTRL 1.0a** profile with MIG and codelists (CONTRL has no Pruefidentifikatoren)
- **Layer 5 semantic rules** for APERAK (`SEM-APERAK-REF-MISSING`) and CONTRL
  (`SEM-CONTRL-SYNTAX-CODE-UNKNOWN`)
- **Conformance test suite** (`tests/conformance.rs`) driven by file-based
  fixtures in `tests/fixtures/` with `*.edi` + `*.expected.json` companions
- `xtask validate-pruefids` — checks that every Pruefidentifikator in the AHB
  profiles has at least one test-fixture coverage entry
- `xtask validate-profiles` — validates all profile JSON against the JSON Schema
- `xtask codegen --dry-run` and `--message-type` flags
- `cargo-semver-checks` gate in CI (PR-only, via `semver-checks` job)
- **Typed fields** (`bgm`, `dtm`, `sender`, `receiver`) on all 7 optional
  message types: `InvoicMessage`, `RemadvMessage`, `OrdersMessage`,
  `IftstaMessage`, `InsrptMessage`, `ReqoteMessage`, `PartinMessage`
- Profiles (MIG + AHB + codelists) for all 7 new types — invoic/2.8e,
  remadv/2.9e, orders/1.4b, iftsta/2.0g, insrpt/1.1a, reqote/1.3c, partin/1.0f
- Valid fixture files for all 7 new message types + typed-field integration tests
- APERAK PID 29002 fixture (`tests/fixtures/aperak/valid/pid_29002.edi`)
- `xtask validate-pruefids --message-type <TYPE>` flag to filter coverage check
  by message type; scanner now also reads `*.edi` fixture files
- `xtask import-codelists` — imports `DE_ID, Code, Description` CSV files and
  merges them into `codelists.json`; supports `--dry-run` mode
- Fuzz target `fuzz/fuzz_targets/fuzz_parse_validate.rs` using `libfuzzer-sys`;
  seeded corpus from real fixtures; 395K+ executions/10s with zero crashes
- `changelog-check` CI job — verifies `[Unreleased]` section is non-empty on PRs
- **`cargo xtask codegen --prune-expired [--grace-days N]`** — marks profiles
  whose `valid_until` + grace period (default 90 days) is in the past as
  `"archived": true` in `mig.json`, then regenerates `mod.rs` with
  archive-gated `#[cfg]` attributes.  The explicit JSON flag keeps
  `--check` deterministic in CI regardless of when codegen is run.
- **Archive Cargo features** — per-type `{type}-archive` features and an
  `archive` meta-feature so expired profiles can be compiled for historical
  validation without inflating the default build.  Initial archived profiles:
  `MSCONS fv20240401`, `INSRPT fv20211001`, `CONTRL fv20251001`.
- **`docs/schema-versioning.md`** — policy document covering additive vs.
  structural schema changes, version range semantics (`MIN`/`MAX`), the
  `archived` field lifecycle, and the annual prune workflow.

### Changed

- Profile schema `"const": 1` replaced by `"minimum": 1, "maximum": 1` in all
  three JSON schemas (`mig`, `ahb`, `codelists`) — permits forward-compatible
  range checks as the schema evolves.
- `codegen.rs` schema version check changed from exact-equality to
  `MIN_SCHEMA_VERSION`/`MAX_SCHEMA_VERSION` range with directional error
  messages: too-old tells the developer to update the profile JSON; too-new
  tells them to update `xtask`.

### Changed

- Codegen now correctly scopes mandatory-segment rules to globally mandatory
  paths only — segments inside optional groups no longer generate spurious
  global presence checks
- UTILMD and MSCONS validation now run Layer 5 semantic rules automatically
  through `validate()`, `validate_against()`, and `validate_pruefidentifikator()`

### Fixed

- CONTRL `detect_pruefidentifikator()` now correctly returns
  `Error::MissingPruefidentifikator` (CONTRL is a syntax acknowledgement
  message and does not use Pruefidentifikatoren)

---

## [0.1.0] — (initial release, pending)

### Added

- `edi-energy` crate with five-layer EDIFACT validation for the German energy
  market (EDI@Energy)
- Supported message types: UTILMD 5.5.3a, MSCONS 2.5a, APERAK 2.0a,
  CONTRL 1.0a (default features); INVOIC, REMADV, ORDERS, IFTSTA, INSRPT,
  REQOTE, PARTIN behind optional features
- `parse()` / `parse_reader()` / `parse_with_config()` entry points
- `EdiEnergyMessage` trait with `validate()`, `validate_against()`,
  `validate_pruefidentifikator()`, `detect_release()`,
  `detect_pruefidentifikator()`, and `serialize()`
- `EdiEnergyReport` with `is_valid()`, `has_errors()`, `errors()`,
  `warnings()`, `infos()`, `filter_by_rule_prefix()`, `filter_by_rule_id()`
- `AnyMessage` enum for type-erased dispatch
- Profile-driven code generation via `cargo xtask codegen`
- `miette` integration behind the `diagnostics` feature

[Unreleased]: https://github.com/hupe1980/edi-energy-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hupe1980/edi-energy-rs/releases/tag/v0.1.0
