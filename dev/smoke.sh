#!/usr/bin/env bash
# Boot each service with its dev/config/<service>.toml against the dev
# PostgreSQL and wait for GET /health/ready to answer 200 — what `just dev`
# promises a fresh clone. `just smoke-dev [service…]` runs it; with no argument
# it boots every service under dev/config/.
#
# Each service stays up until the run ends, and agentd is booted last: it
# handshakes the MCP server of every other service at startup.
set -euo pipefail
cd "$(dirname "$0")/.."

services=("$@")
if [ ${#services[@]} -eq 0 ]; then
    for f in dev/config/*.toml; do services+=("$(basename "$f" .toml)"); done
fi
ordered=()
for svc in "${services[@]}"; do [ "$svc" = agentd ] || ordered+=("$svc"); done
for svc in "${services[@]}"; do [ "$svc" = agentd ] && ordered+=("$svc"); done
services=("${ordered[@]}")

docker compose -f dev/docker-compose.yml up -d --wait >/dev/null
cargo build $(printf -- '-p %s ' "${services[@]}")

target="$(cd "${CARGO_TARGET_DIR:-target}/debug" && pwd)"
logs="$(mktemp -d)"
failed=()
pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true' EXIT
for svc in "${services[@]}"; do
    cfg="dev/config/$svc.toml"
    port="$(sed -nE 's/^addr *= *"[^"]*:([0-9]+)".*/\1/p; s/^port *= *([0-9]+).*/\1/p' "$cfg" | head -1)"
    # `--help` must answer without a config; `--print-config` must accept the
    # dev config (makod's clap CLI has no `--print-config`).
    if ! (cd / && "$target/$svc" --help) >/dev/null 2>&1; then
        echo "FAILED $svc: --help did not print usage"
        failed+=("$svc")
        continue
    fi
    if [ "$svc" != makod ] && ! (. dev/env.sh "$svc" && "$target/$svc" --print-config) >/dev/null 2>"$logs/$svc.print"; then
        echo "FAILED $svc: --print-config refused the dev config"
        sed 's/^/    /' "$logs/$svc.print"
        failed+=("$svc")
        continue
    fi
    (. dev/env.sh "$svc" && exec "$target/$svc") >"$logs/$svc.log" 2>&1 &
    pid=$!
    pids+=("$pid")
    ready=no
    for _ in $(seq 1 120); do
        if ! kill -0 "$pid" 2>/dev/null; then break; fi
        if curl -fsS "http://127.0.0.1:$port/health/ready" >/dev/null 2>&1; then ready=yes; break; fi
        sleep 0.5
    done
    if [ "$ready" = yes ]; then
        echo "ready  $svc :$port"
    else
        echo "FAILED $svc :$port — last log lines:"
        tail -n 20 "$logs/$svc.log" | sed 's/^/    /'
        failed+=("$svc")
    fi
done
if [ ${#failed[@]} -gt 0 ]; then
    echo "not ready: ${failed[*]}" >&2
    exit 1
fi
