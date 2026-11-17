#!/usr/bin/env bash
# `just dev` — the estate on this machine, for working on it.
#
# PostgreSQL runs in Docker (deploy/dev/compose.yaml); the fourteen services are
# built with cargo and run here, their logs in this terminal under one coloured
# prefix each and in target/dev/<service>.log; the Desk runs under Vite against
# them; the seeder fills Stadtwerke Musterstadt. Each service runs its demo
# configuration (deploy/demo/<service>.toml), rewritten into target/dev/ so its
# peers and its database are found on loopback instead of by compose host name.
#
#   just dev               # everything, until Ctrl-C
#   just dev --no-desk     # without the Desk
#   just dev --no-seed     # an empty estate
#   just dev --ci          # headless: start, seed, verify, stop — the CI job
#
# Ctrl-C stops the services and keeps the database; `just dev-down` forgets it.
# Bash 3.2 on purpose: it is the one macOS ships.
set -euo pipefail
cd "$(dirname "$0")/../.."

ci=false desk=true seed=true
for arg in "$@"; do
  case "$arg" in
    --ci) ci=true desk=false ;;
    --no-desk) desk=false ;;
    --no-seed) seed=false ;;
    *) printf 'unknown option %s — --ci, --no-desk or --no-seed\n' "$arg" >&2; exit 2 ;;
  esac
done

# agentd last: it handshakes the MCP server of every other service at startup.
services=(marktd makod processd invoicd edmd netzbilanzd productd einsd billingd accountingd outputd vertragd portald agentd)
pg=${MAKO_DEV_PG_PORT:-5432}
compose=(docker compose -p mako-dev -f deploy/dev/compose.yaml)
dir=target/dev
bin=${CARGO_TARGET_DIR:-target}/debug
started=$SECONDS

bold=$'\e[1m' dim=$'\e[2m' green=$'\e[32m' red=$'\e[31m' off=$'\e[0m'
step() { printf '%s›%s %s\n' "$dim" "$off" "$*"; }
fail() { printf '%s✗ %s%s\n' "$red" "$*" "$off" >&2; exit 1; }

deploy/dev/doctor.sh --quiet || fail "fix the above, then run \`just dev\` again (\`just doctor\` re-checks)"

step "PostgreSQL on 127.0.0.1:$pg (Docker)"
"${compose[@]}" up -d --wait postgres >/dev/null 2>&1 \
  || { "${compose[@]}" logs postgres 2>&1 | tail -20; fail "PostgreSQL did not become healthy"; }

if [ ! -x "$bin/marktd" ]; then
  step "building the fourteen services and the seeder — the first build takes 15–30 minutes, later ones seconds"
else
  step "building the services and the seeder"
fi
cargo build $($ci || echo -q) $(printf -- '-p %s ' "${services[@]}") -p mako-seed

# The demo configuration, its peers and its database moved to loopback.
mkdir -p "$dir" deploy/dev/.data/edmd-warehouse
for s in "${services[@]}"; do
  perl -pe 's#http://(makod|marktd|invoicd|edmd|processd|netzbilanzd|productd|einsd|billingd|accountingd|portald|agentd|vertragd|outputd):(\d+)#http://127.0.0.1:$2#g;
            s#^addr *= *"0\.0\.0\.0:#addr = "127.0.0.1:#;
            s#\@postgres:5432/#\@localhost:'"$pg"'/#g' "deploy/demo/$s.toml" >"$dir/$s.toml"
done

pids=()
names=()
stop() {
  trap - EXIT INT TERM
  if [ ${#pids[@]} -gt 0 ]; then
    kill "${pids[@]}" 2>/dev/null || true
    wait 2>/dev/null || true
  fi
  $ci || printf '\n%sstopped — the database is kept; `just dev-down` forgets it%s\n' "$dim" "$off"
}
trap stop EXIT
trap 'stop; exit 130' INT TERM

# One colour per process, its name padded so the logs line up, and a copy in
# $dir/<name>.log for the start-up checks below. The process substitution keeps
# `$!` the program's own pid, so stopping it stops it.
colour=1
prefixed() {
  local name=$1; shift
  "$@" > >(tee "$dir/$name.log" | perl -pe "BEGIN { \$| = 1 } s/^/\\e[3${colour}m$(printf '%-11s' "$name")\\e[0m│ /") 2>&1 &
  pids+=($!)
  names+=("$name")
  colour=$((colour % 6 + 1))
}

start() {
  local s=$1 var
  var="$(echo "$s" | tr 'a-z-' 'A-Z_')"
  prefixed "$s" env "${var}_CONFIG=$dir/$s.toml" \
    MARKTD_WEBHOOK__ALLOW_LOOPBACK_SUBSCRIBERS=true \
    "EDMD_ARCHIVE__STORAGE_URI=file://$PWD/deploy/dev/.data/edmd-warehouse" \
    "$bin/$s"
}

port_of() { sed -n 's/^addr *= *"[^:]*:\([0-9]*\)".*/\1/p' "$dir/$1.toml" | head -1; }

# Wait for /health/ready; on failure, the tail of the service's log and why.
await() {
  local s=$1 port i pid
  port=$(port_of "$s")
  pid=""
  for i in "${!names[@]}"; do [ "${names[$i]}" = "$s" ] && pid=${pids[$i]}; done
  for _ in $(seq 240); do
    curl -fsS -o /dev/null "http://127.0.0.1:$port/health/ready" 2>/dev/null && return 0
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  printf '\n%s── the last lines of %s/%s.log ──%s\n' "$red" "$dir" "$s" "$off" >&2
  tail -n 25 "$dir/$s.log" >&2 || true
  # Schemas are edited in place, so a database kept from before an edit is
  # one a service may refuse to migrate.
  if grep -qiE "migration|was previously applied|checksum" "$dir/$s.log" 2>/dev/null; then
    fail "$s did not start on :$port — the kept database looks older than the schema: \`just dev-down\`, then \`just dev\`"
  fi
  fail "$s did not become ready on :$port — its log is above"
}

step "starting the services"
start marktd
await marktd
for s in "${services[@]}"; do
  case "$s" in marktd|agentd) continue ;; esac
  start "$s"
done
for s in "${services[@]}"; do
  case "$s" in marktd|agentd) continue ;; esac
  await "$s"
done
start agentd
await agentd

if $seed; then
  if $ci; then
    step "seeding Stadtwerke Musterstadt and verifying it"
    "$bin/mako-seed" seed --verify
  else
    step "seeding Stadtwerke Musterstadt (a second run changes nothing)"
    "$bin/mako-seed" seed 2>&1 | tail -n 3
  fi
fi

if $ci; then
  printf '\n  %smako dev%s %sverified%s in %ss\n\n' "$bold" "$off" "$green" "$off" "$((SECONDS - started))"
  exit 0
fi

desk_line="${dim}needs Node — \`just doctor\` says how${off}"
if $desk && command -v npm >/dev/null 2>&1; then
  [ -d desk/node_modules ] || (cd desk && npm ci --no-audit --no-fund >/dev/null)
  prefixed desk env MAKO_DESK_DEMO=1 bash -c 'cd desk && exec node_modules/.bin/vite --strictPort'
  for _ in $(seq 60); do
    curl -fsS -o /dev/null http://localhost:5173/ 2>/dev/null && break
    sleep 0.5
  done
  desk_line="http://localhost:5173   ${dim}switch persona in the account menu; edits reload in place${off}"
elif ! $desk; then
  desk_line="${dim}not started (--no-desk)${off}"
fi

urls=""
for s in "${services[@]}"; do urls="$urls $s :$(port_of "$s") ·"; done

cat <<EOF

  ${bold}mako dev${off} ${green}ready${off} in $((SECONDS - started))s

  Desk        $desk_line
  Services   ${urls% ·}
              ${dim}each at http://127.0.0.1:<port> — /api/v1/docs, /mcp, /health/ready${off}
  Token       TOKEN=\$(just token)   ${dim}or just token nb · lf · agent — curl -H "Authorization: Bearer \$TOKEN" …${off}
  Events      just listen
  PostgreSQL  postgres://mako:mako@127.0.0.1:$pg/<service>
  Logs        $dir/<service>.log

  ${dim}A changed service? Ctrl-C and \`just dev\` — the build is incremental, the data
  stays, and the seed is not applied twice. \`just dev-down\` starts from empty.${off}

EOF

# Stay until Ctrl-C, or until a process stops on its own.
while sleep 1; do
  for i in "${!pids[@]}"; do
    kill -0 "${pids[$i]}" 2>/dev/null || fail "${names[$i]} stopped — its last lines are above (and in $dir/${names[$i]}.log)"
  done
done
