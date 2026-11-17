#!/usr/bin/env bash
# `just doctor` — can this machine run `just dev`? One line per check, and for
# each miss the command that fixes it. `--quiet` prints only the misses and the
# warnings; the exit status is 1 when anything blocks `just dev`.
#
# Bash 3.2 on purpose: it is the one macOS ships.
set -uo pipefail
cd "$(dirname "$0")/../.."

quiet=false; [ "${1:-}" = --quiet ] && quiet=true
green=$'\e[32m' red=$'\e[31m' yellow=$'\e[33m' dim=$'\e[2m' off=$'\e[0m'
missing=0

ok()   { $quiet || printf '  %s✓%s %s\n' "$green" "$off" "$1"; }
bad()  { printf '  %s✗%s %s\n      %s%s%s\n' "$red" "$off" "$1" "$dim" "$2" "$off"; missing=1; }
warn() { printf '  %s!%s %s\n      %s%s%s\n' "$yellow" "$off" "$1" "$dim" "$2" "$off"; }

# ── Docker and Compose v2 ─────────────────────────────────────────────────────
if ! command -v docker >/dev/null 2>&1; then
  bad "Docker is missing" "install Docker Desktop (macOS) or Docker Engine: https://docs.docker.com/get-docker/"
elif ! docker info >/dev/null 2>&1; then
  bad "Docker is installed but not running" "open -a Docker   # macOS;   sudo systemctl start docker   # Linux"
else
  ok "Docker is running"
  if docker compose version >/dev/null 2>&1; then
    ok "Docker Compose v2 ($(docker compose version --short 2>/dev/null))"
  else
    bad "Docker Compose v2 is missing (\`docker compose\`, not \`docker-compose\`)" \
      "update Docker Desktop, or: sudo apt-get install docker-compose-plugin"
  fi
fi

# ── Rust: rustup and the pinned toolchain ─────────────────────────────────────
channel=$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' rust-toolchain.toml)
if ! command -v rustup >/dev/null 2>&1; then
  bad "rustup is missing" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
elif ! rustup toolchain list 2>/dev/null | grep -q "^$channel"; then
  bad "the pinned toolchain $channel is not installed (rust-toolchain.toml)" "rustup toolchain install $channel --component rustfmt,clippy"
else
  ok "Rust $channel (rust-toolchain.toml)"
fi

# ── The tools the scripts call ────────────────────────────────────────────────
for tool in curl perl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    bad "$tool is missing" "brew install $tool   # macOS;   sudo apt-get install $tool   # Linux"
  fi
done

# ── Node, for the Desk (optional: `just dev` runs without it) ─────────────────
want_node=$(cat desk/.node-version 2>/dev/null || echo 26)
if ! command -v node >/dev/null 2>&1; then
  warn "Node is missing — \`just dev\` runs without the Desk" "install Node $want_node (https://nodejs.org, or: fnm install $want_node)"
else
  have_node=$(node --version | sed 's/^v//; s/\..*//')
  if [ "$have_node" -lt "$want_node" ] 2>/dev/null; then
    warn "Node $(node --version) is older than the Desk's $want_node — \`just dev\` runs without the Desk" "fnm install $want_node, or nvm install $want_node"
  elif [ ! -d desk/node_modules ]; then
    ok "Node $(node --version) and npm $(npm --version 2>/dev/null) — \`just dev\` installs the Desk's packages"
  else
    ok "Node $(node --version) and npm $(npm --version 2>/dev/null), the Desk's packages installed"
  fi
fi

# ── Ports: every service's (deploy/ports.toml), PostgreSQL's and the Desk's ───
ports=$(sed -n '/^\[services\]/,/^\[/s/^[a-z][a-z-]* *= *\([0-9][0-9]*\).*/\1/p' deploy/ports.toml)
taken=""
for port in ${MAKO_DEV_PG_PORT:-5432} $ports 5173; do
  # A port `just dev`'s own PostgreSQL holds is ours.
  if docker ps --filter "publish=$port" --filter "label=com.docker.compose.project=mako-dev" -q 2>/dev/null | grep -q .; then
    continue
  fi
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    holder=$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -Fc 2>/dev/null | sed -n 's/^c//p' | head -1)
    bad "port $port is taken${holder:+ by $holder}" "stop it: lsof -nP -iTCP:$port -sTCP:LISTEN   (a running \`just dev\`? Ctrl-C it; \`just demo\`? just demo-down)"
    taken=yes
  fi
done
[ -z "$taken" ] && ok "ports ${MAKO_DEV_PG_PORT:-5432}, $(echo $ports | tr ' ' ',') and 5173 are free"

# ── Disk: target/ grows to ~100 GB over a few rounds ──────────────────────────
free_kb=$(df -Pk . | awk 'NR==2 {print $4}')
free_gb=$((free_kb / 1024 / 1024))
if [ "$free_gb" -lt 20 ]; then
  bad "only ${free_gb} GB free — a build fails as \`No space left on device\`" "cargo clean   # target/ is rebuilt on the next \`just dev\`"
elif [ "$free_gb" -lt 100 ]; then
  warn "${free_gb} GB free — target/ grows to ~100 GB; a full disk fails a build as \`No space left on device\`" "cargo clean   # between long sessions, never while a build runs"
else
  ok "${free_gb} GB free"
fi

if [ "$missing" = 0 ]; then
  $quiet || printf '\n  %sready for `just dev`%s\n' "$green" "$off"
fi
exit "$missing"
