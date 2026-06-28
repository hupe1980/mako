# makod

`makod` is the production daemon that assembles the engine, transport adapters,
and optional HTTP servers for EDIFACT ingest.

## Health checks

`GET /health` is mounted on every enabled server port:

- `--http-addr` for the HTTP REST API
- `--api-webdienste-addr` for the BDEW API-Webdienste Strom server
- `--as4-addr` for the AS4 inbound transport

Each port exposes its own health endpoint. If a server is disabled, that port
has no health route. Operators should target the enabled ports directly in
liveness and readiness probes.

## Example

```text
makod \
  --http-addr 127.0.0.1:8080 \
  --api-webdienste-addr 127.0.0.1:8090 \
  --as4-addr 127.0.0.1:8443
```

In that layout, a probe must check all three ports if all three servers are
running.

## Shutdown timeout

`makod` waits for an in-flight store flush before exiting. The timeout is
configurable:

| CLI flag                  | Environment variable           | Default |
|---------------------------|--------------------------------|---------|
| `--shutdown-timeout-secs` | `MAKOD_SHUTDOWN_TIMEOUT_SECS`  | `30`    |

For cloud object-store backends (S3/GCS/Azure), large in-memory write buffers
may require more than 30 seconds to flush under load. Set a higher value if
`store close timed out` errors appear in logs during rolling deployments.
