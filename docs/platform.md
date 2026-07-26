---
layout: default
title: Platform
nav_order: 13
parent: Reference
description: >
  Use Platform for multi-tenant gateways, test isolation, hot-reload, and
  custom DoS limits. Alternative to the global ReleaseRegistry singleton.
---

# Platform Guide

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
| **Custom DoS limits** | Global defaults may be too generous or too strict | `platform.parse_with_config(bytes, config)` |

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
`ReleaseRegistry::new(Vec<&'static dyn Profile>)` combined with
`Platform::new(registry)`. This path is intended for callers that supply **their
own** `Profile` implementations — for example, hand-written profiles for classic
5.5.x archive releases that the crate does not bundle:

```rust
use edi_energy::{Platform, registry::{Profile, ReleaseRegistry}};

// `my_profiles::register` pushes your own `&'static dyn Profile` implementations.
let mut profiles: Vec<&'static dyn Profile> = Vec::new();
my_profiles::register(&mut profiles);

let platform = Platform::new(ReleaseRegistry::new(profiles));
let msg = platform.parse(bytes)?;
```

You can also override the transition grace period on any platform:

```rust
use edi_energy::Platform;

// BDEW default is 7 days; widen it to 14 for a specific tenant or test scenario.
let platform = Platform::with_all_profiles().with_transition_grace_days(14);
```

---

## Custom ParseConfig

`Platform` does not store a `ParseConfig`. Instead, pass a config per call via
`parse_with_config`. `ParseConfig` exposes its DoS limits as public fields, so
build one from `ParseConfig::default()` with struct-update syntax:

```rust
use edi_energy::{Platform, ParseConfig};

let config = ParseConfig {
    max_input_bytes: Some(512_000), // 512 KB
    max_segments: Some(1_000),
    ..ParseConfig::default()
};

let platform = Platform::with_all_profiles();
let msg = platform.parse_with_config(bytes, config)?;
```

The interchange API takes a config the same way via
`Platform::parse_interchange_with_config(reader, config)`.

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

`ReleaseRegistry` maps `(message_type_code, association_code)` pairs to `Arc<dyn Profile>` objects. Each profile bundles:

- **MIG rule pack** — segment structure rules
- **AHB rule packs** — per-PID validation rules
- **Codelists** — allowed values per data element
- **Metadata** — `valid_from`, `valid_until`, `source_document`

The registry resolves the correct profile using the UNH association code (`DE 0057`) extracted from each parsed message.

### Transition windows

BDEW mandates a 7-day grace period around each annual profile boundary. The registry is aware of this:

- From `valid_from - 7 days` to `valid_until + 7 days` a release is considered "transitionally valid".
- The global constant `TRANSITION_GRACE_DAYS = 7` governs this window.
- The `ParseConfig::with_reference_date()` override lets tests simulate any date.

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

- [Getting Started](./getting-started.md)
- [Process Engine Guide](./engine.md) — `mako-engine` runtime, stores, deadlines, outbox
- [Parsing Guide](./parsing.md)
- [Validation Guide](./validation.md)
- [Release Lifecycle](./release-lifecycle.md)
