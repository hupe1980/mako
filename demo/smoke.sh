#!/usr/bin/env bash
# demo/smoke.sh — automated smoke test for makod
#
# Runs a series of HTTP checks against a running makod instance.
# Assumes makod is reachable at BASE_URL with auth token AUTH_TOKEN.
#
# Usage:
#   ./smoke.sh                         # defaults: http://localhost:8080, token demo-secret-change-me
#   BASE_URL=http://localhost:8080 AUTH_TOKEN=mytoken ./smoke.sh
#
# Prerequisites: curl, jq

set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:8080}"
AUTH_TOKEN="${AUTH_TOKEN:-demo-secret-change-me}"
# Set WEBHOOK_URL to the demo webhook receiver to check ERP CloudEvents.
# When using docker compose this is http://localhost:8000 by default.
WEBHOOK_URL="${WEBHOOK_URL:-}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}✓${NC} $*"; }
fail() { echo -e "${RED}✗${NC} $*"; exit 1; }
info() { echo -e "${YELLOW}▶${NC} $*"; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || { echo "Required command not found: $1"; exit 1; }
}
require_cmd curl
require_cmd jq

# ── helpers ────────────────────────────────────────────────────────────────────

get() {
    curl -sS -w '\n%{http_code}' "$BASE_URL$1" \
        -H "Authorization: Bearer $AUTH_TOKEN"
}

put_json() {
    curl -sS -w '\n%{http_code}' -X PUT "$BASE_URL$1" \
        -H "Authorization: Bearer $AUTH_TOKEN" \
        -H "Content-Type: application/json" \
        --data-binary "$2"
}

post_edifact() {
    curl -sS -w '\n%{http_code}' -X POST "$BASE_URL/edifact" \
        -H "Authorization: Bearer $AUTH_TOKEN" \
        -H "Content-Type: text/plain; charset=utf-8" \
        --data-binary "@$1"
}

post_command() {
    curl -sS -w '\n%{http_code}' -X POST "$BASE_URL/api/v1/commands" \
        -H "Authorization: Bearer $AUTH_TOKEN" \
        -H "Content-Type: application/json" \
        --data-binary "$1"
}

delete_resource() {
    curl -sS -w '\n%{http_code}' -X DELETE "$BASE_URL$1" \
        -H "Authorization: Bearer $AUTH_TOKEN"
}

# Splits body and HTTP status code from the curl output (last line is status).
# Uses sed '$d' (delete last line) — compatible with both BSD (macOS) and GNU sed.
body()   { printf '%s' "$1" | sed '$d'; }
status() { printf '%s\n' "$1" | tail -n 1; }

# ── wait for container ────────────────────────────────────────────────────────

wait_healthy() {
    info "Waiting for makod at $BASE_URL ..."
    local retries=30
    while [[ $retries -gt 0 ]]; do
        local code
        code=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE_URL/health" 2>/dev/null || true)
        if [[ "$code" == "200" ]]; then
            pass "makod is ready"
            return 0
        fi
        retries=$((retries - 1))
        sleep 2
    done
    fail "makod did not become healthy within 60s (last HTTP status: ${code:-none})"
}

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "================================================="
echo "  makod smoke test  →  $BASE_URL"
echo "================================================="
echo

wait_healthy

# ── Reset webhook event log ───────────────────────────────────────────────────
# Clear any stale events from previous runs so steps 6 and 8 only see events
# produced by THIS smoke run.  The webhook receiver supports DELETE /events.

if [[ -n "${WEBHOOK_URL:-}" ]]; then
    curl -sS -o /dev/null -X DELETE "$WEBHOOK_URL/events" 2>/dev/null || true
fi

# ── 1. Health check ───────────────────────────────────────────────────────────

info "[1/9] Health check"
resp=$(curl -sS -w '\n%{http_code}' "$BASE_URL/health")
[[ "$(status "$resp")" == "200" ]] || fail "GET /health returned $(status "$resp")"
HEALTH_STATUS=$(body "$resp" | jq -r '.status')
[[ "$HEALTH_STATUS" == "ok" ]] || fail "health status is '$HEALTH_STATUS', expected 'ok'"
INSTANCE=$(body "$resp" | jq -r '.instance_id')
pass "GET /health → ok  (instance: $INSTANCE)"

# ── 2. OpenAPI spec available ─────────────────────────────────────────────────

info "[2/9] OpenAPI spec"
resp=$(get "/api/v1/openapi.json")
[[ "$(status "$resp")" == "200" ]] || fail "GET /api/v1/openapi.json returned $(status "$resp")"
TITLE=$(body "$resp" | jq -r '.info.title')
pass "GET /api/v1/openapi.json → $TITLE"

# ── 3. Register a trading partner ─────────────────────────────────────────────

info "[3/9] Register Lieferant trading partner (GLN 4012345000023)"
PARTNER_JSON=$(cat "$SCRIPT_DIR/fixtures/partner-lf.json")
resp=$(put_json "/admin/partners/4012345000023" "$PARTNER_JSON")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" ]] || \
    fail "PUT /admin/partners/4012345000023 returned $code: $(body "$resp")"
pass "PUT /admin/partners/4012345000023 → $code"

# ── 4. List partners ──────────────────────────────────────────────────────────

info "[4/9] List partners"
resp=$(get "/admin/partners")
[[ "$(status "$resp")" == "200" ]] || fail "GET /admin/partners returned $(status "$resp")"
COUNT=$(body "$resp" | jq '.count')
pass "GET /admin/partners → $COUNT partner(s) registered"

# ── 5. Submit UTILMD EDIFACT interchange (PID 55001 — Lieferbeginn Strom) ─────
#
# LFN → NB: Anmeldung Lieferbeginn.
# makod spawns GpkeSupplierChangeWorkflow and enqueues a ProcessInitiated
# CloudEvent for the NB ERP (step 6).  The ERP then calls the command API (step 7).

info "[5/9] POST UTILMD 55001 (Lieferbeginn Strom — LFN→NB Anmeldung)"
resp=$(post_edifact "$SCRIPT_DIR/fixtures/utilmd-55001.edi")
code=$(status "$resp")
BODY=$(body "$resp")

if [[ "$code" == "200" ]]; then
    ACCEPTED=$(echo "$BODY" | jq '.accepted')
    REJECTED=$(echo "$BODY" | jq '.rejected')
    MSG_STATUS=$(echo "$BODY" | jq -r '.messages[0].status')
    PID=$(echo "$BODY" | jq -r '.messages[0].pid')
    pass "POST /edifact → HTTP 200  accepted=$ACCEPTED  rejected=$REJECTED  status=$MSG_STATUS  pid=$PID"
    echo
    echo "$BODY" | jq '.'
    echo
elif [[ "$code" == "422" ]]; then
    fail "POST /edifact returned 422 (parse error): $BODY"
else
    fail "POST /edifact returned $code: $BODY"
fi

# ── 6. ERP webhook — automatic APERAK + ProcessInitiated from UTILMD ingest ───
#
# Immediately upon receiving the UTILMD 55001 makod executes two actions
# atomically inside ReceiveUtilmd (no ERP call needed):
#
#   a) APERAK BGM+312 (Anerkennungsmeldung) enqueued → delivered to LFN within
#      the 45-minute APERAK Frist (APERAK AHB 1.0 §2.4.1 — Strom UTILMD weekday).
#      This is a TECHNICAL acknowledgement — it does NOT imply business acceptance.
#
#   b) ProcessInitiated CloudEvent enqueued → delivered to the NB ERP webhook so
#      the ERP knows a new Anmeldung is awaiting its BUSINESS decision.
#
# The ERP then reviews the Anfrage independently and calls bestaetigen/ablehnen
# (step 7).  UTILMD 55003/55004 is the BUSINESS response — it is only sent after
# the explicit ERP command, never automatically.
#
# Regulatory basis:
#   APERAK → APERAK AHB 1.0 §2.4 (Strom: both BGM+312 and BGM+313 mandatory)
#   UTILMD Antwort → BK6-22-024 §5 (24h window, requires NB/ERP decision)

info "[6/9] Automatic outbox from UTILMD ingest: APERAK BGM+312 + ProcessInitiated  (WEBHOOK_URL=${WEBHOOK_URL:-<not set>})"
if [[ -z "${WEBHOOK_URL:-}" ]]; then
    echo "      Skipped — WEBHOOK_URL not set."
    echo "      With docker compose: WEBHOOK_URL=http://localhost:8000 ./smoke.sh"
else
    # Both APERAK and ProcessInitiated are enqueued atomically on UTILMD ingest.
    # The outbox worker poll interval is 5 s — retry every second for up to 8 s.
    echo "      Polling webhook for APERAK + ProcessInitiated (up to 8 s, outbox poll = 5 s) …"
    WEVENTS='[]'
    ACOUNT=0
    WCOUNT=0
    for _i in 1 2 3 4 5 6 7 8; do
        sleep 1
        WEVENTS=$(curl -sS "$WEBHOOK_URL/events" 2>/dev/null || echo '[]')
        ACOUNT=$(printf '%s' "$WEVENTS" | jq '[.[] | select(.body.type == "de.mako.edifact.outbound" and .body.makomessagetype == "APERAK")] | length' 2>/dev/null || echo 0)
        WCOUNT=$(printf '%s' "$WEVENTS" | jq '[.[] | select(.body.type == "de.mako.process.initiated")] | length' 2>/dev/null || echo 0)
        [[ "$ACOUNT" -gt 0 && "$WCOUNT" -gt 0 ]] && break
    done

    # 6a. APERAK BGM+312 — must arrive BEFORE any ERP decision
    # makomessagetype is set by makod to the message type string; filter on that
    # directly to avoid jq pipe-precedence pitfalls with the | test(...) form.
    APERAK_EVENTS=$(printf '%s' "$WEVENTS" | \
        jq '[.[] | select(.body.type == "de.mako.edifact.outbound" and .body.makomessagetype == "APERAK")]' 2>/dev/null || echo '[]')
    if [[ "$ACOUNT" -gt 0 ]]; then
        pass "APERAK BGM+312 (Anerkennungsmeldung) delivered to LFN — automatic, no ERP action required:"
        echo
        printf '%s' "$APERAK_EVENTS" | jq '.[] | .body | {type, makomessagetype, makorecipient, edifact: .data.edifact}'
        echo
    else
        echo -e "${YELLOW}⚠${NC}  APERAK not yet visible after 8 s — may still be in flight (non-fatal)"
    fi

    # 6b. ProcessInitiated CloudEvent — ERP notification
    INITIATED=$(printf '%s' "$WEVENTS" | \
        jq '[.[] | select(.body.type == "de.mako.process.initiated")]' 2>/dev/null || echo '[]')
    if [[ "$WCOUNT" -gt 0 ]]; then
        pass "ProcessInitiated CloudEvent delivered to ERP webhook — ERP is now notified:"
        echo
        printf '%s' "$INITIATED" | jq '.[] | .body | {type, subject, source, "makopid": .makopid, "data": .data}'
        echo
    else
        fail "No de.mako.process.initiated CloudEvent after 4 s — expected ProcessInitiated outbox entry from UTILMD 55001 ingest"
    fi

    # Clear the event log so step 8 only captures events produced by bestaetigen.
    curl -sS -o /dev/null -X DELETE "$WEBHOOK_URL/events" 2>/dev/null || true
fi

# ── Simulate ERP review time ──────────────────────────────────────────────────
#
# In production the NB ERP needs time to review the Anmeldung before calling
# bestaetigen.  We insert a short delay here to make the timing boundary
# visible: events from step 6 (APERAK, ProcessInitiated) belong to the ingest
# phase; events from step 8 (UTILMD 55003) belong to the ERP-decision phase.
#
# The UTILMD 55003/55004 business response CANNOT be automated — the ERP must
# inspect the Anfrage (e.g. check tariff eligibility, duplicate detection, …)
# before calling gpke.lieferbeginn.bestaetigen or gpke.lieferbeginn.ablehnen.

echo "      Simulating ERP review (3 s) …"
sleep 3

# ── 7. NB ERP accepts the Anmeldung via command API ──────────────────────────
#
# The ERP received the ProcessInitiated CloudEvent in step 6 and has now
# reviewed the Anfrage.  It calls the command API to accept (bestaetigen) or
# reject (ablehnen).  makod then:
#   1. Persists AntwortGesendet event on the gpke-supplier-change process
#   2. Enqueues UTILMD 55003 (Bestätigung Lieferbeginn) in the outbox for
#      delivery to LFN via AS4/EDIFACT
#
# Note: the APERAK was already sent automatically in step 6 — it is NOT
# triggered here.  bestaetigen only produces the UTILMD business response.

info "[7/9] NB ERP: accept Lieferbeginn (gpke.lieferbeginn.bestaetigen)"
CMD_PAYLOAD='{"command":"gpke.lieferbeginn.bestaetigen","payload":{"malo_id":"51238696781"}}'
resp=$(post_command "$CMD_PAYLOAD")
code=$(status "$resp")
BODY=$(body "$resp")
if [[ "$code" == "202" ]]; then
    PROCESS_ID=$(echo "$BODY" | jq -r '.process_id')
    pass "POST /api/v1/commands → HTTP 202  process_id=$PROCESS_ID"
    echo
    echo "$BODY" | jq '.'
    echo
    echo "      makod persisted AntwortGesendet and enqueued UTILMD 55003 (Bestätigung Lieferbeginn)."
    echo "      Note: APERAK BGM+312 was already delivered in step 6 — it is NOT re-sent here."
    echo "      Step 8 verifies the UTILMD 55003 arrives at the webhook."
    echo
else
    fail "POST /api/v1/commands returned $code: $BODY"
fi

# ── 8. Outbound EDIFACT — verify UTILMD 55003 Bestätigung was delivered ───────
#
# The event log was cleared after step 6, so only events produced by the
# bestaetigen command in step 7 will appear here.
#
# makod enqueued UTILMD 55003 (Bestätigung Lieferbeginn) atomically with the
# AntwortGesendet event.  The WebhookEdifactSender renders it to EDIFACT wire
# format and POSTs it as a CloudEvent to the webhook.
#
# In production this is the AS4/EDIFACT delivery to the LFN.
#
# Note: the APERAK BGM+312 was already verified in step 6.  It is NOT
# re-sent here — bestaetigen only produces the business response UTILMD.

info "[8/9] Outbound EDIFACT — UTILMD 55003 Bestätigung (ERP decision → LFN)"
if [[ -z "${WEBHOOK_URL:-}" ]]; then
    echo "      Skipped — WEBHOOK_URL not set."
else
    # The outbox worker poll interval is 5 s, so worst-case delivery is ~5 s
    # after the command is accepted.  Retry every second for up to 12 s.
    echo "      Polling webhook for UTILMD 55003 (up to 12 s, outbox worker poll = 5 s) …"
    UTILMD_EVENTS='[]'
    UCOUNT=0
    for _i in 1 2 3 4 5 6 7 8 9 10 11 12; do
        sleep 1
        ALL_EVENTS=$(curl -sS "$WEBHOOK_URL/events" 2>/dev/null || echo '[]')
        UTILMD_EVENTS=$(printf '%s' "$ALL_EVENTS" | \
            jq '[.[] | select(.body.type == "de.mako.edifact.outbound" and .body.makomessagetype == "UTILMD")]' 2>/dev/null || echo '[]')
        UCOUNT=$(printf '%s' "$UTILMD_EVENTS" | jq 'length' 2>/dev/null || echo 0)
        [[ "$UCOUNT" -gt 0 ]] && break
    done
    if [[ "$UCOUNT" -gt 0 ]]; then
        pass "UTILMD 55003 Bestätigung delivered to LFN — triggered by ERP bestaetigen command:"
        echo
        printf '%s' "$UTILMD_EVENTS" | jq '.[] | .body | {
            type,
            subject,
            makomessagetype,
            makorecipient,
            edifact: .data.edifact
        }'
        echo
    else
        fail "No de.mako.edifact.outbound CloudEvent after 12 s — expected UTILMD 55003 Bestätigung"
    fi
fi

# ── 9. Clean up — delete the test partner ─────────────────────────────────────

info "[9/9] Clean up — delete demo partner"
resp=$(delete_resource "/admin/partners/4012345000023")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "204" ]] || \
    fail "DELETE /admin/partners/4012345000023 returned $code: $(body "$resp")"
pass "DELETE /admin/partners/4012345000023 → $code"

# ── Summary ───────────────────────────────────────────────────────────────────

echo
echo "================================================="
echo -e "${GREEN}All smoke tests passed.${NC}"
echo
echo "  Swagger UI : $BASE_URL/api/v1/docs/"
echo "  MCP server : $BASE_URL/mcp  (use with Claude Desktop or VS Code Copilot)"
echo "================================================="
echo
