# Feedback to agentplane, from mako

What `agentd` has found using [agentplane](https://github.com/hupe1980/agentplane)
in a regulated domain. Open items first, then the ones that became mechanisms
upstream — kept because the pattern of *how* they were found is the useful part.

Tested against **0.19**.

---

## Open: there is no admission-level idempotency key

**What we need.** `agentd`'s `POST /webhook` receives CloudEvents from `marktd`'s
fan-out, which is at-least-once: the emitter retries until it sees a 2xx. A
redelivery must not start a second fan-out of runs. Today `agentd` deduplicates
in an in-process TTL map, which means it does **not** deduplicate across
instances or across a restart — exactly the topology `store::PostgresStore`
exists to serve.

**What we tried, and why it is not the answer.** `EventStore::buffer` already
returns `false` for a `(source, id)` already seen, over the shared store, with
`InboundEvent::dedup_key` as the single definition of that identity. It looks
like the primitive.

It is not, because buffering has a second effect: an event nobody subscribes to
is dead-lettered by `Runtime::sweep` after the grace period. Using `buffer` as an
idempotency ledger would dead-letter almost every event `agentd` receives —
`SweepReport::dead_lettered` would be permanently non-zero, `needs_attention()`
permanently true, and the signal that "a correlation key is wrong" would be
destroyed to get deduplication. The two concerns share an identity and must not
share a table.

**What would close it.** An idempotency key on admission, arbitrated by the store
that already arbitrates lease fencing — something in the shape of:

```rust
// Returns the existing run when this key was already admitted.
pub async fn run_correlated_once(
    &self,
    capability: &str,
    input: Tainted<Value>,
    kind: &str,
    keys: &[CorrelationKey],
    idempotency_key: &str,          // e.g. InboundEvent::dedup_key()
) -> Result<RunOutcome, RuntimeError>;
```

Returning the *original* run rather than an error is what makes it usable: a
caller that retried wants the same answer, not a conflict it has to interpret.

**Why it matters more here than the numbers suggest.** The blast radius of a
duplicate fan-out looks small — no effect is duplicated inside a run, and no
market message is dispatched twice, because the one dispatching grant requires a
human and the deterministic engine sends the message. What *is* duplicated is a
second `DecisionRequest` on the same case. A reviewer looking at two identical
Freigabe tasks for one Sperrauftrag is a four-eyes control degrading into a
guess, and it is the one duplicate that is not merely wasted tokens.

**Workaround in place.** `handlers::SeenEvents`, documented on the type as
per-process, with a startup warning when the Postgres backend is selected. It is
a mitigation, not a fix, and it is written down as one.

---

## 0.19's `signed_with`: the scheme change was right — two seams around it are missing

0.18: `Destination::signed_with(header, secret)` — an operator picks the header
and gets `HMAC-SHA256(secret, body)`.

0.19: `Destination::signed_with(secret)` — [Standard Webhooks],
`webhook-signature: v1,<base64>` over `{webhook-id}.{webhook-timestamp}.{body}`.

**Dropping the custom-header form was the right call, and mako followed it.** Our
own convention (`X-Mako-Signature: sha256=<hex>` over the raw body) was the GitHub
shape and had the weakness we had already written down on our side: *a signature
authenticates bytes, not freshness*, so a captured POST replayed forever. mako's
whole outbound surface is now Standard Webhooks, `mako_service::webhook` verifies
agentplane's deliveries like any other, and both implementations are pinned
against the spec's reference vector. Briefly the 0.19 upgrade was blocked *by*
the mismatch; migrating removed the blocker entirely.

Two seams would have made that cheaper, and both stand independently of which
scheme a deployment picks:

1. **Ship a verifier beside the signer.** `push` signs; a receiver has to
   re-implement the check, including the two things the doc correctly identifies
   as the receiver's job — refusing a stale `webhook-timestamp` and
   deduplicating on `webhook-id`. Those are exactly the parts a second
   implementation gets wrong, and every receiver is a second implementation. We
   found twelve hand-rolled copies of the old check in our own tree, and *none*
   of them did the equivalent of the timestamp half; centralising it is what
   fixed them all at once. `verify` next to `sign`, taking the header map and the
   raw body and returning a typed refusal plus the id, would mean nobody writes
   the interesting half twice.
2. **Do not panic on a deployment's own configuration.** `signed_with` panics on
   a key under 24 bytes or a malformed `whsec_` secret. For us that call is
   inside `Daemon::build`, so a mistyped secret aborts the process rather than
   failing the way every other configuration error does. A `try_signed_with`
   returning the diagnostic would match how the rest of the crate refuses
   (`RuntimeBuilder::try_build`, `BuildError::PolicyUnevaluable`) — and that
   precedent is explicit that a daemon should refuse to start with a diagnostic
   rather than abort.

[Standard Webhooks]: https://www.standardwebhooks.com/

---

## Closed upstream — four for four

Every gap reported so far became a mechanism within a release. Recorded because
each was found the same way: by an end-to-end suite, not by reading the API.

| Gap | Closed in | How it was found |
|---|---|---|
| An unevaluable Cedar rule presented as a plane that denied every effect, at first run rather than at startup | 0.17 — build-time evaluation against canonical requests, `BuildError::PolicyUnevaluable` names the rule | `tests/oversight.rs` running the whole approval path. Every unit test passed: a hand-written policy context is a context assembled to suit the rule |
| `push::Destination` could only authenticate with a header scheme, so `de.agent.decision.made` was the one unsigned mako outbound | 0.17 — `Destination::signed_with`, and `PushSender::for_operator_destinations` *takes* the destination list so a sender cannot be built without the signing configuration | Reviewing which outbound paths carried a signature, after the transactional-outbox sweep |
| A failed run's `RunOutcome` had no reason, so a refusal read as "the agent said nothing" | 0.17 — `RunOutcome::reason()` | An operator-facing decision log with empty summaries |
| `budgets: { max_tokens: 0 }` parsed as parsimony and acted as a refusal of the agent's first tool call | 0.18 — parse-time refusal of a zero ceiling | `tests/specialist_smoke.rs`, on its first run. A model-free specialist had it set deliberately, meaning "must not spend" |

The pattern worth naming: **all four presented as silence.** A plane that denies
everything, an unsigned delivery, an empty summary and a refused first tool call
all look like an agent that ran and found nothing. None was caught by a unit
test; each was caught by a suite that built the real plane and ran a real path
end to end.

---

## Two things we would not change

**No `race` primitive.** Declining it is right, and the reasoning transfers:
abandoning an in-flight branch manufactures the unknown outcome the effect
protocol exists to prevent. `PlanIR::fan_out` keyed by capability is the better
shape, and "adding a specialist cannot silently renumber what the aggregator
reads" is a property we have relied on.

**No `AllowAll`.** A permissive engine and no engine being the same behaviour is
a better default than mako's own `default.cedar`, which is a catch-all permit
switched off with a flag. We are following agentplane here, not the other way
round.
