# mako-events

[![Crates.io](https://img.shields.io/crates/v/mako-events?label=mako-events&color=f59e0b&logo=rust)](https://crates.io/crates/mako-events)

The compile-time catalogue of every CloudEvents `type` the mako services emit
and consume: one `pub const` per type, grouped by bounded context, so a
producer and a consumer that disagree on a name fail to compile. It is a leaf
with no dependencies and owns the type names and the glob `matches()` only —
the envelope, signing and delivery live in `mako-service` (`CloudEvent`,
`source`, `post_ce_with_retry`, `webhook::{sign, verify_request}`).

## Example

```rust
use mako_events::{markt, vertrag};

assert_eq!(markt::VERSORGUNG_GAP_DETECTED, "de.markt.versorgung.gap-detected");
assert_eq!(vertrag::WIDERRUF_EINGEGANGEN, "de.vertrag.widerruf.eingegangen");

// The one glob matcher every subscription uses.
assert!(mako_events::matches("de.vertrag.bestellung.*", vertrag::BESTELLUNG_ANGENOMMEN));
assert!(mako_events::all().contains(&markt::VERSORGUNG_GAP_DETECTED));
```

## What it covers

| Item | Contents |
|---|---|
| context modules | `mako`, `markt`, `billing`, `invoic`, `eeg`, `accounting`, `netzbilanz`, `messwert`, `tarif`, `vertrag`, `vpp`, `agent`, `gabi`, `obs`, `sperr`, `mabis` |
| `vertrag::BESTELLUNG_*` | an online order `EINGEGANGEN`, `ANGENOMMEN` or `ABGELEHNT`; `subject` is the `bestellreferenz` |
| `vertrag::WIDERRUF_EINGEGANGEN` | a consumer's Widerruf (§ 355 BGB) recorded, timely or not |
| `all()` | every constant, held complete by a test |
| `matches(pattern, type)` | `*` any sequence, `?` one character, everything else literal |

## Conventions

- Lowercase reverse-DNS, `de.<context>.<noun>.<participle>`; a multi-word
  segment joins with `-`, never `_`. German domain nouns keep their spelling.
- A constant no service in this workspace emits carries a `⚠ phantom:` doc
  note saying why, and a test demands the note. It is about emission only.
- Adding an event: the `pub const` with a doc comment naming emitter and
  consumers, an entry in `all()`, then the emission in the owning service.

Event delivery: [ERP Integration](https://hupe1980.github.io/mako/docs/architecture/erp-integration/) ·
API: <https://docs.rs/mako-events>

License: MIT OR Apache-2.0
