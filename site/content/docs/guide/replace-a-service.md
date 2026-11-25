+++
title = "Replace a service"
description = "Keep your own contract system or billing in place of a mako service, and check with the conformance kit that it keeps the contract mako's service publishes."
weight = 5
+++
Each mako service publishes its contract: a committed OpenAPI document, an MCP
tool snapshot, and a JSON schema per typed event. A Stadtwerk that keeps its
own system — an ERP as the contract system in place of `vertragd` — replaces
the service by keeping that contract. `mako-conformance` checks it.

## Run a replacement trial

Use an isolated test deployment. The kit seeds records and registers a temporary webhook endpoint. Supply a test principal that the service admits, a second authenticated principal that the service refuses on the suite’s refusal pair, the inbound signing secret and a callback reachable by the test service. Choose a target build label that you can tie to the deployment.

```sh
just conformance vertragd https://contracts.test.example \
  --token "$OPERATOR" --refused-token "$REFUSED" \
  --inbound-secret "$INBOUND" --listen 0.0.0.0:9198 \
  --callback https://kit.test.example/ \
  --acceptance --target-build contracts-rc1 --report evidence.json
```

Prepare representative test events within the hour before starting the run. Use its normal order acceptance, refusal, withdrawal, contract-change and market-response paths and lifecycle workers. Check the retained event types with `GET /api/v1/events`. The suite also seeds an Ersatzversorgung case. An absent type makes the assessment incomplete.

Use HTTPS for a remote target. HTTP is available for loopback tests. The client refuses redirects and bounds each probe response to 16 MiB and each request to 30 seconds. Help is available offline with `just conformance --help`.

| Acceptance result | Exit | What to do |
|---|---:|---|
| Passed | 0 | Review the scope and reference fingerprint before deciding whether to trial this build. |
| Failed | 1 | Inspect the first broken check, correct the target and rerun the full assessment. A broken check takes precedence over missing observations. |
| Error | 2 | Correct the invocation, target configuration or report destination and run again. |
| Incomplete | 3 | Supply the missing credential, reachable listener or representative test history. Run the full suite again. A filtered run always needs more evidence for acceptance. |

The human summary names the verdict and first actionable blocker. The JSON file records the same decision, reference fingerprint, caller-supplied build label, scope, counts and stable check IDs. Keep the exit status with the report so a failed invocation cannot be mistaken for a fresh assessment. The report can be shared with the replacement team. Diagnostic output may include local request details.

## What the suite checks

| Runner | Checks |
|---|---|
| `rest` | every operation that documents a `401` refuses a call without a token with an `ApiError`; every parameterless `GET` answers a documented status with a body valid against the document; the refused principal gets `403` |
| `events` | the kit registers its endpoint through `POST /api/v1/webhooks/endpoints`; the test delivery and every replayed event verify under the issued secret (Standard Webhooks), are CloudEvents 1.0, are of a declared type, and carry `data` valid against the published schema; an inbound route accepts a signed event twice and refuses it unsigned |
| `mcp` | a call without a token is refused; `tools/list` names exactly the snapshot's tools; the tool twin of the refused route refuses the same principal |

For local diagnosis, omit the acceptance and evidence options. `--only rest|events|mcp` selects one runner. The diagnostic output shows every held, broken or skipped probe. It returns 0 when there are no broken observations, including a run with skipped checks.

The verdict covers the declared interfaces and observed events. Approval for live market traffic also needs independent evidence of business behavior and operational recovery.

The available suite is `vertragd`. Run `just test-conformance-db` to exercise the reference service on PostgreSQL.
