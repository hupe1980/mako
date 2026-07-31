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
the retrying publisher — lives in [`mako-service`](../mako-service/)
(`CloudEvent`, `source`, `post_ce_with_retry`, `webhook::{sign, verify_hmac}`),
which already carries the serde/uuid/time/reqwest/hmac stack those need. Keeping
that out of `mako-events` is what preserves the leaf property. Emitters pass a
constant from this catalog as the `type`; the two layers meet there.

## Conventions

- Lowercase reverse-DNS: `de.<context>.<noun>.<participle>`
  (`de.markt.versorgung.eog-begonnen`). German domain nouns keep their
  spelling; hyphenated nouns stay hyphenated.
- Every constant appears in [`all()`] — enforced by tests — so catalogs,
  subscription UIs, and docs can enumerate the full event surface.
- `matches()` implements the shared glob matcher (`de.markt.*`) used by every
  subscription mechanism (marktd EventBus, agentd triggers, ERP webhooks).
- Events that are **subscribed but not yet emitted** (or vice versa) carry a
  `⚠ phantom:` / `orphan emit:` doc note at the constant — honest drift
  tracking instead of silent divergence.

## Adding an event

1. Add the `pub const` to the right context module with a doc comment naming
   the emitter and consumers.
2. Append it to `all()`.
3. Emit it via the owning service; consume it via `mako_events::matches()`.
