+++
title = "Platform"
description = "Use Platform for multi-tenant gateways, test isolation, hot-reload, and custom DoS limits. Alternative to the global ReleaseRegistry singleton."
weight = 13
+++
The `Platform` struct provides explicit, isolated instances of the EDI@Energy processing pipeline. This is the recommended approach for multi-tenant servers, integration tests, and any application that needs more than one profile configuration at the same time.

---

## Why `Platform`?

The top-level `parse()` and `parse_interchange()` functions use `ReleaseRegistry::global()` — a process-wide singleton initialized on first use. This is fine for simple command-line tools and single-tenant services.

`Platform` is better when you need:

| Need | Problem with globals | Solution |
|---|---|---|
| **Test isolation** | Concurrent tests that register custom profiles interfere with each other | Each test gets its own `Platform` |
| **Multi-tenant gateways** | Strom and Gas tenants need different profile subsets | One `Platform` per tenant |
| **Hot-reload** | New BDEW release requires a process restart | Swap `Arc<Platform>` at runtime |
| **Custom DoS limits** | Global defaults may be too generous or too strict | `platform.parse_with_config(bytes, config)` for one message, `platform.parse_interchange_with_config(reader, config)` for an interchange |

---

## Basic Usage

```rust
use edi_energy::Platform;

// Create a platform with all built-in profiles enabled
let platform = Platform::with_all_profiles();

let input = std::fs::read("message.edi")?;
let msg = platform.parse(&input)?;
let report = msg.validate()?;
```

---

## Custom Profile Subset

`Platform::with_all_profiles()` is the supported way to obtain a platform backed
by the crate's built-in profiles. Each call builds a fresh, independent
`ReleaseRegistry`:

```rust
use edi_energy::Platform;

let platform = Platform::with_all_profiles();
let msg = platform.parse(bytes)?;
```

There is **no public API to build a registry containing an arbitrary subset of
the built-in profiles at runtime**. The generated per-profile statics and the
profile-registration entry point are crate-private and are not re-exported. The
`edi_energy::releases` module exposes `&'static Release` *identifiers* (e.g.
`edi_energy::releases::mscons_fv20261001()` returns a `&'static Release`), not
`Profile` objects, so they cannot be handed to a registry constructor.

To trim which built-in profiles are compiled in, use the crate's per-message-type
**Cargo features** (`utilmd`, `mscons`, `aperak`, `invoic`, …). A profile that is
not enabled at build time is simply absent from `with_all_profiles()`.

The one runtime constructor for a custom registry is
`ReleaseRegistry::new(Vec<&'static Profile>)` combined with
`Platform::new(registry)`:

```rust
use edi_energy::{Platform, registry::ReleaseRegistry};

let platform = Platform::new(ReleaseRegistry::new(profiles));
let msg = platform.parse(bytes)?;
```

`Profile` is a **struct**, not a trait — a profile is the MIG and AHB read as
data (`crates/edi-energy/src/profile/mod.rs:31`), so there is nothing to
implement. The `&'static Profile` values that go into that `Vec` are the
crate-private generated statics, which is why this constructor is in practice
for tests and benchmarks that want an isolated registry rather than a way to
supply profiles of your own.

You can also widen the receive tolerance on any platform:

```rust
use edi_energy::Platform;

// The BDEW default is 0 — EDIFACT changes format at a single instant.
// Raise it for a tenant whose contract tolerates a late-arriving old-format message.
let platform = Platform::with_all_profiles().with_receive_tolerance_days(3);
```

---

## Custom ParseConfig

`Platform` does not store a `ParseConfig`; a config is passed per call.
`ParseConfig` exposes its DoS limits as public fields, so build one from
`ParseConfig::default()` with struct-update syntax:

```rust
use edi_energy::{ParseConfig, Platform};

let config = ParseConfig {
    max_input_bytes: Some(512_000), // 512 KB
    max_segments: Some(1_000),
    ..ParseConfig::default()
};

let platform = Platform::with_all_profiles();

// One message, the platform's registry, the caller's limits:
let msg = platform.parse_with_config(bytes, config)?;

// Interchange iteration takes the config and the platform's registry:
for msg in platform.parse_interchange_with_config(reader, config) { /* ... */ }

// Outside a platform entirely, `Parser` carries the config against the global
// registry:
let msg = edi_energy::Parser::with_config(config).parse(bytes)?;
```

`parse_with_config` is the single-message counterpart of `parse`: same registry,
same profile dispatch, with the DoS limits stated per call rather than taken from
the global defaults.

The only builder-style method on `ParseConfig` is `with_reference_date`, which
pins the date used for profile validity lookups during validation (useful for
deterministic tests):

```rust
use edi_energy::ParseConfig;

let config = ParseConfig::default().with_reference_date(
    time::Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
);
```

---

## Test Isolation

The most important use case. Instead of depending on the global registry, give each test its own platform:

```rust
#[test]
fn my_utilmd_test() {
    let platform = Platform::with_all_profiles();
    let msg = platform.parse(UTILMD_BYTES).unwrap();
    let report = msg.validate().unwrap();
    assert!(report.is_valid());
}

#[test]
fn my_mscons_test() {
    // Independent — does not share state with my_utilmd_test
    let platform = Platform::with_all_profiles();
    let msg = platform.parse(MSCONS_BYTES).unwrap();
    assert_eq!(msg.try_message_type().map(|t| t.as_str()), Some("MSCONS"));
}
```

Platforms are cheap to create (profiles use `LazyLock` internally so rule-pack construction is amortized).

---

## Sharing Platforms (`Arc`)

Platforms implement `Clone` via `Arc<ReleaseRegistry>` sharing — the underlying profile data is not duplicated:

```rust
use std::sync::Arc;
use edi_energy::Platform;

let shared = Arc::new(Platform::with_all_profiles());

// Hand clones to worker threads
let worker_platform = shared.clone();
std::thread::spawn(move || {
    let msg = worker_platform.parse(bytes).unwrap();
    // ...
});
```

---

## `ReleaseRegistry` Deep Dive

`ReleaseRegistry` maps `(MessageType, Release)` to `&'static Profile`
(`crates/edi-energy/src/registry/mod.rs:115`). Each profile bundles:

- the **MIG** — the Nachrichtenstruktur and every place's Segmentlayout
- the **AHB** — one Prüfschablone per Prüfidentifikator
- **code lists** — the values each data element admits
- **metadata** — `valid_from()`, `valid_until()`, `source_document()`

Resolution is by the UNH association code (`DE 0057`) **and** the reference date:
several profiles may share one wire release code where BDEW revised a format
without changing the EDIFACT version — COMDIS `1.0g` appears in both
`fv20251001` and `fv20261001` — so the index keeps the candidates sorted by
`valid_from` and picks the latest one at or before the date.

### Format boundaries

**EDIFACT has no transition window.** Allgemeine Festlegungen 6.1 §2.5 gives the
EDIFACT formats a single *Anwendungszeitpunkt* — 1 April or 1 October — with no
overlap around it: before that instant the old format applies, from it the new
one does.

The 15-*Werktage* Übergangszeitraum that does exist is §8.5, and it is the **XML**
rule. It starts *at* the Anwendungszeitpunkt rather than before it, runs in
Werktage rather than calendar days, and picks the version by the *Erfüllungsdatum*
in the message rather than by when it was sent. None of that carries over.

- `is_acceptable_on` accepts a release from its `valid_from` through its
  `valid_until` — the leading edge is hard.
- `ReleaseRegistry::with_receive_tolerance_days(n)` extends the *trailing* edge
  by `n` calendar days, for operators who choose to accept a late-arriving
  message in the superseded format. It is a local receiving policy, not a BDEW
  rule, and it defaults to `DEFAULT_RECEIVE_TOLERANCE_DAYS = 0`.
- `TransitionState::Transition` therefore occurs only with a non-zero tolerance,
  and only after the boundary.
- `ParseConfig::with_reference_date()` pins the date profile resolution uses, so
  a test can sit on any boundary deterministically.

---

## Interchange Parsing

`Platform` exposes the same interchange API as the free functions:

```rust
use std::io::BufReader;
use std::fs::File;

let file = File::open("bulk.edi")?;
let reader = BufReader::new(file);

for result in platform.parse_interchange(reader) {
    let msg = result?;
    if let Some(mt) = msg.try_message_type() { println!("{}", mt.as_str()); }
}
```

---

## See Also

- [Getting Started](@/docs/guide/getting-started.md)
- [Process Engine Guide](@/docs/architecture/engine.md) — `mako-engine` runtime, stores, deadlines, outbox
- [Parsing Guide](@/docs/reference/parsing.md)
- [Validation Guide](@/docs/reference/validation.md)
- [Release Lifecycle](@/docs/compliance/release-lifecycle.md)
