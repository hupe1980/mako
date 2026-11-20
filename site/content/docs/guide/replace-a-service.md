+++
title = "Replace a service"
description = "Keep your own contract system or billing in place of a mako service, and check with the conformance kit that it keeps the contract mako's service publishes."
weight = 5
+++
Each mako service publishes its contract: a committed OpenAPI document, an MCP
tool snapshot, and a JSON schema per typed event. A Stadtwerk that keeps its
own system — an ERP as the contract system in place of `vertragd` — replaces
the service by keeping that contract. `mako-conformance` checks it.

```sh
just conformance vertragd https://contracts.test.example \
  --token "$OPERATOR" --refused-token "$NB_ONLY" \
  --inbound-secret "$INBOUND" --listen 0.0.0.0:9198 \
  --callback https://kit.test.example/
```

Run it against a **test deployment**: the kit creates records to make the
service emit, and registers — then removes — a webhook endpoint.

| Runner | Checks |
|---|---|
| `rest` | every operation that documents a `401` refuses a call without a token with an `ApiError`; every parameterless `GET` answers a documented status with a body valid against the document; the refused principal gets `403` |
| `events` | the kit registers its endpoint through `POST /api/v1/webhooks/endpoints`; the test delivery and every replayed event verify under the issued secret (Standard Webhooks), are CloudEvents 1.0, are of a declared type, and carry `data` valid against the published schema; an inbound route accepts a signed event twice and refuses it unsigned |
| `mcp` | a call without a token is refused; `tools/list` names exactly the snapshot's tools; the tool twin of the refused route refuses the same principal |

Each line of the report is `✓` held, `✗` broken with what was seen and why it
matters, or `–` not checked (a type the replay window did not contain, a type
with no published schema). Exit `0` when nothing broke, `1` on a gap, `2` on a
usage error. `--only rest|events|mcp` runs one runner.

Passing means the contract's **shape** holds. It does not mean mako's
regulatory behaviour does: a replacement that passes may still send an
Anmeldung a held Kündigung forbids, or bill past a Zuordnungsende. Those rules
are proven by mako's own tests, not by the kit.

Suites exist for `vertragd`. `just test-conformance-db` runs each against its
reference service on PostgreSQL.
