# mako-events

**The compile-time CloudEvents catalog for the mako platform.**

Every CloudEvents `type` string exchanged between mako services is a `pub const`
in this crate, grouped by bounded context (`mako`, `markt`, `billing`,
`vertrag`, `tarif`, `messwert`, `invoic`, `netzbilanz`, `obs`, `agent`, …).
Producer/consumer drift is a **compile error**, not a runtime surprise: services
import the constant instead of typing the string.

```rust
use mako_events::markt;

assert_eq!(markt::VERSORGUNG_GAP_DETECTED, "de.markt.versorgung.gap-detected");
```

## Scope: type names only (a zero-dependency leaf)

This crate is deliberately a **single-purpose leaf with no dependencies**: it owns
the event *identity* layer — the `type` constants and the glob `matches()` — and
nothing else. It is safe for any crate to depend on because it pulls in nothing
and rebuilds nothing.

Everything about the CloudEvent *envelope* and its *transport* — building the
structured-mode body, the `source` URI convention, HMAC signing/verification, and
the retrying publisher — lives in [`mako-service`](https://github.com/hupe1980/mako/tree/main/crates/mako-service)
(`CloudEvent`, `source`, `post_ce_with_retry`, `webhook::{sign, verify_request}`),
which already carries the serde/uuid/time/reqwest/hmac stack those need. Keeping
that out of `mako-events` is what preserves the leaf property. Emitters pass a
constant from this catalog as the `type`; the two layers meet there.

## Conventions

- Lowercase reverse-DNS: `de.<context>.<noun>.<participle>`
  (`de.markt.versorgung.eog-begonnen`). German domain nouns keep their
  spelling; hyphenated nouns stay hyphenated.
- Every constant appears in `all()` — enforced by tests — so catalogs,
  subscription UIs, and docs can enumerate the full event surface.
- `matches()` implements the shared glob matcher (`de.markt.*`) used by every
  subscription mechanism (marktd fan-out, agentd triggers, ERP webhooks).
- A constant **no service in this workspace emits** carries a `⚠ phantom:` doc
  note saying so and why, and a test demands the note — honest drift tracking
  instead of silent divergence. The note is about emission only: some phantoms
  are subscriptions placed ahead of their emitter, others are neither emitted
  nor consumed. A phantom is a recorded gap, not a promise.

## Adding an event

1. Add the `pub const` to the right context module with a doc comment naming
   the emitter and consumers.
2. Append it to `all()`.
3. Emit it via the owning service; consume it via `mako_events::matches()`.

A constant that stops at step 2 needs the `⚠ phantom:` note. Minting one with no
emitter and no note declares an event that cannot fire — a subscriber matching it
waits forever, and the doc comment naming its emitter is simply untrue.

## Related crates

| Crate | Role |
|---|---|
| [`mako-events`](https://docs.rs/mako-events) ← **this crate** | The `type` constants and the glob `matches()` — nothing else |
| [`mako-service`](https://github.com/hupe1980/mako/tree/main/crates/mako-service) | The CloudEvents *envelope* and transport — building, signing, delivering |
| [`mako-obs`](https://docs.rs/mako-obs) | Consumes `de.mako.*` into process projections and KPI reports |
| [`mako-markt`](https://docs.rs/mako-markt) | Emits the `de.markt.*` half of the catalog |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
