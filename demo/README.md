# makod demo

Quick-start guide for running and testing `makod` — the Mako process engine daemon for German energy market communication (GPKE, WiM, GeLi Gas, MABIS, GaBi Gas, Redispatch 2.0).

---

## Prerequisites

| Tool | Purpose |
|---|---|
| Docker (with Compose v2) | Run the daemon |
| `curl` | HTTP smoke tests |
| `jq` | Parse JSON responses |

For the `makod:dev` image used in the examples below, build it first from the repo root:

```bash
docker build \
  --build-arg OCI_REVISION=$(git rev-parse HEAD) \
  --build-arg OCI_CREATED=$(date -u +%Y-%m-%dT%H:%M:%SZ) \
  -t makod:dev \
  .
```

Or pull the published image and tag it locally:

```bash
docker pull ghcr.io/hupe1980/makod:0.6.0
docker tag  ghcr.io/hupe1980/makod:0.6.0 makod:dev
```

---

## Demo configuration

The demo runs makod as the **Netzbetreiber Strom (NB)** with GLN `9900357000004`. This matches the `NAD+MR` (receiver) in the bundled EDIFACT fixture, so all routing steps succeed without extra setup.

| Parameter | Value |
|---|---|
| Tenant ID | `9900357000004` |
| Marktrolle | `NB` |
| HTTP port | `8080` |
| Bearer token | `demo-secret-change-me` |

---

## Quick start — docker compose

```bash
# from the repo root:
cd demo
docker compose up -d
docker compose logs -f makod
```

The container is healthy when `docker compose ps` shows `(healthy)`.

Stop and keep data:
```bash
docker compose down
```

Stop and wipe data volume:
```bash
docker compose down -v
```

---

## Quick start — plain docker run

```bash
# volatile / ephemeral mode — data lost on restart, fine for local testing
# Mount a tmpfs at the image's data path with nonroot uid ownership.
# The distroless runtime user is uid/gid 65532.
docker run --rm -p 8080:8080 \
  --tmpfs /var/lib/makod:uid=65532,gid=65532 \
  makod:dev \
  --tenant-id  9900357000004 \
  --marktrollen NB \
  --auth-key   demo=demo-secret-change-me

# with persistent storage (survives restarts)
docker run -d -p 8080:8080 \
  -v "$PWD/makod-data:/var/lib/makod" \
  makod:dev \
  --tenant-id   9900357000004 \
  --marktrollen NB \
  --auth-key    demo=demo-secret-change-me
```

---

## Smoke test — automated

The `smoke.sh` script runs all API checks end-to-end and prints a pass/fail summary:

```bash
cd demo
./smoke.sh
```

Override defaults if needed:
```bash
BASE_URL=http://localhost:8080 AUTH_TOKEN=demo-secret-change-me ./smoke.sh
```

Expected output:
```
▶ Waiting for makod at http://localhost:8080 ...
✓ makod is ready
=================================================
  makod smoke test  →  http://localhost:8080
=================================================

✓ GET /health → ok  (instance: <hostname>-<pid>)
✓ GET /api/v1/openapi.json → makod REST API
✓ PUT /admin/partners/4012345000023 → 200
✓ GET /admin/partners → 1 partner(s) registered
✓ POST /edifact → HTTP 200  accepted=1  rejected=0  status=routed  pid=55001
✓ DELETE /admin/partners/4012345000023 → 200

=================================================
All smoke tests passed.

  Swagger UI : http://localhost:8080/api/v1/docs/
  MCP server : http://localhost:8080/mcp
=================================================
```

---

## Manual curl examples

All REST endpoints require a Bearer token.  Replace `demo-secret-change-me` with the value you passed to `--auth-key`.

### Health check (no auth required)

```bash
curl http://localhost:8080/health
```

```json
{"status":"ok","instance_id":"<hostname>-<pid>"}
```

### Interactive API documentation

Open in your browser — no token needed to browse the schema:

```
http://localhost:8080/api/v1/docs/
```

### Register a trading partner

```bash
curl -X PUT http://localhost:8080/admin/partners/4012345000023 \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  -d @fixtures/partner-lf.json
```

### List partners

```bash
curl http://localhost:8080/admin/partners \
  -H "Authorization: Bearer demo-secret-change-me" | jq .
```

### Submit a UTILMD EDIFACT interchange

```bash
curl -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary @fixtures/utilmd-55001.edi | jq .
```

Response:
```json
{
  "accepted": 1,
  "rejected": 0,
  "messages": [
    {
      "message_type": "UTILMD",
      "pid": 55001,
      "workflow": "GpkeSupplierChange",
      "status": "routed"
    }
  ]
}
```

### Download the OpenAPI spec

```bash
curl http://localhost:8080/api/v1/openapi.json \
  -H "Authorization: Bearer demo-secret-change-me" \
  -o makod-openapi.json
```

---

## Fixtures

| File | Description |
|---|---|
| `fixtures/utilmd-55001.edi` | UTILMD PID 55001 — Lieferbeginn Strom.  Sender: LF `4012345000023`, Receiver: NB `9900357000004`.  Routes when tenant is configured as this NB. |
| `fixtures/partner-lf.json` | `PartnerRecord` for the LF `4012345000023` with AS4 endpoint, email channel, and `LfStrom` role. |

---

## MCP integration (Claude Desktop / VS Code Copilot)

`makod` exposes an MCP server at `/mcp` on the same port as the REST API.

Add to your `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "makod": {
      "url": "http://localhost:8080/mcp",
      "headers": { "Authorization": "Bearer demo-secret-change-me" }
    }
  }
}
```

Available MCP tools: `list_commands`, `submit_command`, `get_malo`, `list_partners`, `get_partner`, `get_health`.

---

## Startup validation only (no server)

Run the `--check` mode to validate profiles, adapters, and config without starting any workers:

```bash
docker run --rm makod:dev --check
```

Exit code 0 means all checks passed. Use this in CI pipelines before a full deployment.

---

## Common issues

**`403 Forbidden` on API calls**
The bearer token doesn't match. Check `--auth-key` value; the token is the part after `name=`.

**`422 Unprocessable Entity` on POST /edifact**
The EDIFACT body could not be parsed at all (syntax error). Check the `"error"` field in the response.

**`"status": "unknown_pid"` in edifact response**
The PID was detected but no workflow is registered for it on this instance.  Check that `--marktrollen` includes the role that handles the PID (e.g., `NB` for PID 55001).

**Container exits immediately**
`--data-dir` was omitted without `--allow-volatile`. Either mount a volume (`-v`) and pass `--data-dir`, or add `--allow-volatile` for ephemeral mode.
