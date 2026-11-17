#!/usr/bin/env bash
# `just demo [--build]` — Stadtwerke Musterstadt on the released images, in
# Docker: PostgreSQL, the fourteen services, the Desk and the seeder (root
# compose.yaml). No Rust needed; `--build` builds the images from this tree
# instead of pulling them (20–45 minutes cold).
#
# Ends when the seed has run and verified, printing the Desk's URL and the
# personas. `just demo-down` stops it and forgets its data.
# Bash 3.2 on purpose: it is the one macOS ships.
set -euo pipefail
cd "$(dirname "$0")/../.."

port=${MAKO_PORT:-8000}
version=$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version *= *"\(.*\)"/\1/p' | head -1)
export MAKO_VERSION=${MAKO_VERSION:-$version} MAKO_PORT=$port
compose=(docker compose -p mako-demo)
started=$SECONDS
build=false
for a in "$@"; do
  case "$a" in
    --build) build=true ;;
    *) printf 'unknown argument %s — `just demo [--build]`\n' "$a" >&2; exit 2 ;;
  esac
done

bold=$'\e[1m' dim=$'\e[2m' green=$'\e[32m' red=$'\e[31m' off=$'\e[0m'
step() { printf '%s›%s %s\n' "$dim" "$off" "$*"; }
fail() { printf '%s✗ %s%s\n' "$red" "$*" "$off" >&2; exit 1; }

command -v docker >/dev/null || fail "Docker is missing — https://docs.docker.com/get-docker/"
docker info >/dev/null 2>&1 || fail "Docker is not running — open Docker Desktop, or: sudo systemctl start docker"
docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is missing — https://docs.docker.com/compose/install/"
holder=$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fc 2>/dev/null | sed -n 's/^c//p' | head -1 || true)
if [ -n "$holder" ] && ! docker ps --filter "publish=$port" --filter "label=com.docker.compose.project=mako-demo" -q | grep -q .; then
  fail "port $port is taken by $holder — stop it, or: MAKO_PORT=8001 just demo"
fi

if $build; then
  step "building the images of $MAKO_VERSION from this tree (20–45 minutes cold)"
  "${compose[@]}" build || fail "the build failed — its output is above"
else
  step "pulling the images of $MAKO_VERSION"
  if ! "${compose[@]}" pull --quiet 2>"${TMPDIR:-/tmp}/mako-demo-pull.log"; then
    tail -5 "${TMPDIR:-/tmp}/mako-demo-pull.log" >&2
    fail "the images for $MAKO_VERSION are not published yet — \`just demo --build\` builds them from this tree"
  fi
fi

step "starting PostgreSQL, the services and the Desk"
"${compose[@]}" up -d --no-build --remove-orphans >/dev/null \
  || { "${compose[@]}" ps; fail "the estate did not start — \`docker compose -p mako-demo logs <service>\` says why"; }

step "seeding Stadtwerke Musterstadt (waits for every service to be healthy)"
# `compose wait` prints the exit code bare (Compose v2) or in a sentence (v5),
# so the code is read from the container itself once it has exited.
"${compose[@]}" wait seed >/dev/null 2>&1 || true
status=$("${compose[@]}" ps -a seed --format '{{.ExitCode}}' 2>/dev/null | head -1)
if [ "$status" != 0 ]; then
  "${compose[@]}" logs --no-log-prefix seed | tail -30 >&2
  "${compose[@]}" ps >&2
  fail "the seed did not verify — its log is above"
fi
"${compose[@]}" logs --no-log-prefix seed | tail -3

cat <<MSG

  ${bold}mako demo${off} ${green}ready${off} in $((SECONDS - started))s   ${dim}images $MAKO_VERSION${off}

  Desk       http://localhost:$port   ${dim}work as any persona: the menu at the foot of the sidebar${off}
  API token  TOKEN=\$(just token)      ${dim}or just token nb, lf, agent — Authorization: Bearer \$TOKEN${off}
  Services   http://localhost:$port/svc/<service>/…   ${dim}makod processd vertragd marktd invoicd edmd netzbilanzd billingd accountingd agentd${off}
  Events     just listen --host host.docker.internal

  ${dim}A demonstration, not a deployment. \`just demo-down\` stops it and forgets its data.${off}

MSG
