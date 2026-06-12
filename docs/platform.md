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
| **Custom DoS limits** | Global defaults may be too generous or too strict | `Platform::with_config(config)` |

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

To reduce binary size or memory at runtime, register only the profiles you need:

```rust
use edi_energy::{
    Platform,
    registry::{ReleaseRegistry, Profile},
};

// Use only UTILMD Strom and MSCONS
let mut registry = ReleaseRegistry::new();
registry.register(edi_energy::profiles::utilmd_fv20261001_strom());
registry.register(edi_energy::profiles::mscons_fv20261001());

let platform = Platform::with_registry(registry);
let msg = platform.parse(bytes)?;
```

---

## Custom ParseConfig

Override DoS limits per platform:

```rust
use edi_energy::{Platform, ParseConfig};

let config = ParseConfig::new()
    .with_max_input_bytes(512_000)   // 512 KB
    .with_max_segments(1_000);

let platform = Platform::with_config(config);
```

Or combine custom registry and config:

```rust
let platform = Platform::builder()
    .registry(registry)
    .config(config)
    .build();
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

- [Parsing Guide](./parsing.md)
- [Validation Guide](./validation.md)
- [Release Lifecycle](./release-lifecycle.md)
