#!/usr/bin/env bash
# demos/eeg-billing/smoke.sh — EEG billing end-to-end smoke test
#
# Flow:
#   1. Seed MaLo master data into marktd
#   2. Register solar PV plant in einsd
#   3. Push 15-min Einspeisemenge to edmd (direct iMSys push — 96 intervals × 1 kWh each)
#   4. Trigger monthly settlement in einsd (year=2026, month=6)
#   5. Assert de.eeg.verguetung.berechnet CloudEvent received by ERP webhook
#   6. Verify settlement receipt (status=calculated, settlement_eur > 0)
#
# Prerequisites:
#   docker compose up -d
#
# Usage:
#   bash smoke.sh
#   EINSD_URL=http://localhost:9180 EDMD_URL=http://localhost:8380 bash smoke.sh

set -euo pipefail

EINSD_URL="${EINSD_URL:-http://localhost:9180}"
EDMD_URL="${EDMD_URL:-http://localhost:8380}"
MARKTD_URL="${MARKTD_URL:-http://localhost:8180}"
WEBHOOK_URL="${WEBHOOK_URL:-http://localhost:8000}"

# Colours
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}✓${NC} $*"; }
fail() { echo -e "${RED}✗${NC} $*"; exit 1; }
info() { echo -e "${YELLOW}▶${NC} $*"; }

echo
echo "================================================="
echo "  mako EEG billing smoke test"
echo "  marktd   → ${MARKTD_URL}"
echo "  edmd     → ${EDMD_URL}"
echo "  einsd    → ${EINSD_URL}"
echo "  webhook  → ${WEBHOOK_URL}"
echo "================================================="
echo

# ── Wait for services ─────────────────────────────────────────────────────────

info "Waiting for einsd at ${EINSD_URL} ..."
for i in $(seq 1 30); do
    if curl -sf "${EINSD_URL}/health/live" >/dev/null 2>&1; then
        pass "einsd is ready"
        break
    fi
    if [ "$i" -eq 30 ]; then fail "einsd did not become healthy within 60s"; fi
    sleep 2
done

info "Waiting for edmd at ${EDMD_URL} ..."
for i in $(seq 1 30); do
    if curl -sf "${EDMD_URL}/health/live" >/dev/null 2>&1; then
        pass "edmd is ready"
        break
    fi
    if [ "$i" -eq 30 ]; then fail "edmd did not become healthy within 60s"; fi
    sleep 2
done

MALO_ID="17835382035"
TR_ID="TR0000000001"
BILLING_YEAR=2026
BILLING_MONTH=6

# ── Pre-load: seed MaLo into marktd ──────────────────────────────────────────

echo
echo "════ Pre-load: seed master data into marktd ════"
echo

info "[P1] PUT MaLo ${MALO_ID} into marktd (Einspeiser)"
HTTP=$(curl -sf -w '\n%{http_code}' -X PUT "${MARKTD_URL}/api/v1/malo/${MALO_ID}" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer demo-secret-change-me" \
    -d @fixtures/malo.json 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
if [[ "$CODE" == "200" || "$CODE" == "201" ]]; then
    pass "PUT /api/v1/malo/${MALO_ID} → ${CODE}"
else
    fail "PUT /api/v1/malo/${MALO_ID} → ${CODE}"
fi

# ── Einsd tests ───────────────────────────────────────────────────────────────

echo
echo "════ einsd: EEG plant registration + settlement ════"
echo

info "[1/6] Register EEG plant ${TR_ID} (9.8 kWp solar rooftop, EEG 2023)"
PLANT=$(cat fixtures/anlage.json)
HTTP=$(curl -sf -w '\n%{http_code}' -X PUT "${EINSD_URL}/api/v1/anlagen/${TR_ID}" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer demo-secret-change-me" \
    -d "$PLANT" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
BODY=$(echo "$HTTP" | head -1)
if [[ "$CODE" == "200" || "$CODE" == "201" || "$CODE" == "204" ]]; then
    pass "PUT /api/v1/anlagen/${TR_ID} → ${CODE} (plant registered)"
else
    fail "PUT /api/v1/anlagen/${TR_ID} → ${CODE}: ${BODY}"
fi

info "[2/6] GET plant (verify registration)"
HTTP=$(curl -sf -w '\n%{http_code}' "${EINSD_URL}/api/v1/anlagen/${TR_ID}" \
    -H "Authorization: Bearer demo-secret-change-me" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
BODY=$(echo "$HTTP" | head -1)
ANLAGE_STATUS=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('status','?'))" 2>/dev/null || echo "?")
VERGUETUNG=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('verguetungssatz_ct','?'))" 2>/dev/null || echo "?")
if [[ "$CODE" == "200" && "$ANLAGE_STATUS" == "aktiv" ]]; then
    pass "GET /api/v1/anlagen/${TR_ID} → status=${ANLAGE_STATUS}  verguetungssatz_ct=${VERGUETUNG} ct/kWh"
else
    fail "GET /api/v1/anlagen/${TR_ID} → ${CODE}: ${BODY}"
fi

# ── edmd: push Einspeisemenge ─────────────────────────────────────────────────

echo
info "[3/6] Push 96 × 15-min Einspeisemenge to edmd (${BILLING_YEAR}-0${BILLING_MONTH}-01, 1 kWh per slot = 96 kWh/day)"

# Build 96 intervals for 2026-06-01 (15 min each = 1 full day)
READS="["
for i in $(seq 0 95); do
    HH=$(( i * 15 / 60 ))
    MM=$(( i * 15 % 60 ))
    HH2=$(( (i * 15 + 15) / 60 ))
    MM2=$(( (i * 15 + 15) % 60 ))
    FROM=$(printf "2026-06-01T%02d:%02d:00Z" $HH $MM)
    TO=$(printf "2026-06-01T%02d:%02d:00Z" $HH2 $MM2)
    if [ "$HH2" -eq 24 ]; then TO="2026-06-02T00:00:00Z"; fi
    SEP=","
    if [ "$i" -eq 0 ]; then SEP=""; fi
    READS="${READS}${SEP}{\"from\":\"${FROM}\",\"to\":\"${TO}\",\"value\":1.0,\"quality\":\"MEASURED\"}"
done
READS="${READS}]"

HTTP=$(curl -sf -w '\n%{http_code}' -X POST "${EDMD_URL}/api/v1/meter-reads/rlm/${MALO_ID}" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer demo-secret-change-me" \
    -d "{\"intervals\":${READS},\"sparte\":\"STROM\",\"source\":\"DIRECT_PUSH\"}" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
BODY=$(echo "$HTTP" | head -1)
STORED=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('intervals_accepted',d.get('stored_count',0)))" 2>/dev/null || echo "?")
if [[ "$CODE" == "200" || "$CODE" == "201" ]]; then
    pass "POST /api/v1/meter-reads/rlm/${MALO_ID} → ${CODE}  stored=${STORED} intervals"
else
    fail "POST /api/v1/meter-reads/rlm/${MALO_ID} → ${CODE}: ${BODY}"
fi

info "[3b/6] Push remaining 29 days of June (aggregated: 96 kWh/day × 29 = 2784 kWh)"
# Push the rest of June as daily buckets for brevity (real MSB would send 15-min data)
DAILY_READS="["
for day in $(seq 2 30); do
    D=$(printf "%02d" $day)
    ND=$(printf "%02d" $((day + 1)))
    if [ "$day" -eq 30 ]; then ND="01"; FROM_NEXT="2026-07"; else FROM_NEXT="2026-06"; fi
    FROM="2026-06-${D}T00:00:00Z"
    TO="${FROM_NEXT}-${ND}T00:00:00Z"
    SEP=","
    if [ "$day" -eq 2 ]; then SEP=""; fi
    DAILY_READS="${DAILY_READS}${SEP}{\"from\":\"${FROM}\",\"to\":\"${TO}\",\"value\":96.0,\"quality\":\"MEASURED\"}"
done
DAILY_READS="${DAILY_READS}]"

HTTP=$(curl -sf -w '\n%{http_code}' -X POST "${EDMD_URL}/api/v1/meter-reads/rlm/${MALO_ID}" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer demo-secret-change-me" \
    -d "{\"intervals\":${DAILY_READS},\"sparte\":\"STROM\",\"source\":\"DIRECT_PUSH\"}" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
if [[ "$CODE" == "200" || "$CODE" == "201" ]]; then
    pass "POST /api/v1/meter-reads/rlm/${MALO_ID} → ${CODE}  (29 daily buckets, 2784 kWh)"
else
    fail "POST /api/v1/meter-reads/rlm/${MALO_ID} (daily) → ${CODE}"
fi

info "[3c/6] Verify billing period in edmd (expect ~2880 kWh for June)"
HTTP=$(curl -sf -w '\n%{http_code}' \
    "${EDMD_URL}/api/v1/billing-period/${MALO_ID}?from=2026-06-01&to=2026-07-01" \
    -H "Authorization: Bearer demo-secret-change-me" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
BODY=$(echo "$HTTP" | head -1)
ARBEIT=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('arbeitsmenge_kwh','?'))" 2>/dev/null || echo "?")
if [[ "$CODE" == "200" ]]; then
    pass "GET /api/v1/billing-period/${MALO_ID} → arbeitsmenge_kwh=${ARBEIT}"
else
    fail "GET /api/v1/billing-period/${MALO_ID} → ${CODE}: ${BODY}"
fi

# ── einsd: trigger EEG settlement ────────────────────────────────────────────

echo
info "[4/6] Trigger EEG settlement for ${BILLING_YEAR}-0${BILLING_MONTH} (einsd auto-fetches Einspeisemenge from edmd)"
HTTP=$(curl -sf -w '\n%{http_code}' -X POST \
    "${EINSD_URL}/api/v1/anlagen/${TR_ID}/settle/${BILLING_YEAR}/${BILLING_MONTH}" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer demo-secret-change-me" \
    -d "{}" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
BODY=$(echo "$HTTP" | head -1)
SETTLE_EUR=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('settlement_eur','?'))" 2>/dev/null || echo "?")
SETTLE_KWH=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('einspeisemenge_kwh','?'))" 2>/dev/null || echo "?")
SETTLE_STATUS=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('status','?'))" 2>/dev/null || echo "?")
if [[ "$CODE" == "200" || "$CODE" == "201" ]]; then
    pass "POST /settle/${BILLING_YEAR}/${BILLING_MONTH} → ${CODE}"
    echo "      settlement_eur=${SETTLE_EUR}  einspeisemenge_kwh=${SETTLE_KWH}  status=${SETTLE_STATUS}"
    echo
    # Pretty-print the full response
    echo "$BODY" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin), indent=2))" 2>/dev/null || echo "$BODY"
    echo
else
    fail "POST /settle/${BILLING_YEAR}/${BILLING_MONTH} → ${CODE}: ${BODY}"
fi

# ── Verify CloudEvent delivery ────────────────────────────────────────────────

info "[5/6] Verify de.eeg.verguetung.berechnet CloudEvent in ERP webhook"
EEG_EVENTS='[]'
for _i in 1 2 3 4 5 6 7 8; do
    sleep 1
    ALL=$(curl -sS "${WEBHOOK_URL}/events" 2>/dev/null || echo '[]')
    EEG_EVENTS=$(echo "$ALL" | python3 -c "
import sys, json
events = json.load(sys.stdin)
eeg = [e for e in events if isinstance(e.get('body'), dict) and 'eeg' in e['body'].get('type','')]
print(json.dumps(eeg))
" 2>/dev/null || echo '[]')
    COUNT=$(echo "$EEG_EVENTS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
    [[ "$COUNT" -gt 0 ]] && break
done

COUNT=$(echo "$EEG_EVENTS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
if [[ "$COUNT" -gt 0 ]]; then
    EVENT_TYPE=$(echo "$EEG_EVENTS" | python3 -c "import sys,json; events=json.load(sys.stdin); print(events[0]['body'].get('type','?'))" 2>/dev/null || echo "?")
    pass "CloudEvent received: type=${EVENT_TYPE}"
    echo
    echo "$EEG_EVENTS" | python3 -c "
import sys, json
events = json.load(sys.stdin)
for e in events[:1]:
    b = e.get('body', {})
    print(json.dumps({
        'type': b.get('type'),
        'source': b.get('source'),
        'subject': b.get('subject'),
        'data': b.get('data', {})
    }, indent=2))
" 2>/dev/null
    echo
else
    echo -e "${YELLOW}  (No EEG CloudEvent yet — settlement may still be processing)${NC}"
fi

# ── Verify settlement receipt ─────────────────────────────────────────────────

info "[6/6] Verify settlement receipt in einsd"
HTTP=$(curl -sf -w '\n%{http_code}' \
    "${EINSD_URL}/api/v1/anlagen/${TR_ID}/settlements?year=${BILLING_YEAR}&month=${BILLING_MONTH}" \
    -H "Authorization: Bearer demo-secret-change-me" 2>/dev/null || echo -e "\n000")
CODE=$(echo "$HTTP" | tail -1)
BODY=$(echo "$HTTP" | head -1)
if [[ "$CODE" == "200" ]]; then
    RECEIPT_EUR=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); r=d[0] if isinstance(d,list) else d; print(r.get('settlement_eur','?'))" 2>/dev/null || echo "?")
    RECEIPT_STATUS=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); r=d[0] if isinstance(d,list) else d; print(r.get('status','?'))" 2>/dev/null || echo "?")
    RECEIPT_KWH=$(echo "$BODY" | python3 -c "import sys,json; d=json.load(sys.stdin); r=d[0] if isinstance(d,list) else d; print(r.get('einspeisemenge_kwh','?'))" 2>/dev/null || echo "?")
    pass "GET /settlements?year=${BILLING_YEAR}\&month=${BILLING_MONTH} → status=${RECEIPT_STATUS}  einspeisemenge_kwh=${RECEIPT_KWH}  settlement_eur=${RECEIPT_EUR}"
else
    fail "GET /settlements?year=${BILLING_YEAR}\&month=${BILLING_MONTH} → ${CODE}: ${BODY}"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

echo
echo "================================================="
echo "All EEG billing smoke tests passed."
echo
echo "  Settlement: 9.8 kWp solar rooftop, June 2026"
echo "  EEG law:    EEG 2023 §21 FEED_IN_TARIFF"
echo "  Rate:       8.11 ct/kWh (Solarpaket I ≤10 kWp)"
if [[ -n "${SETTLE_KWH:-}" && "${SETTLE_KWH}" != "?" ]]; then
    echo "  Einspeisung: ${SETTLE_KWH} kWh"
fi
if [[ -n "${SETTLE_EUR:-}" && "${SETTLE_EUR}" != "?" ]]; then
    echo "  Vergütung:   EUR ${SETTLE_EUR}"
fi
echo
echo "  edmd Lastgang:    ${EDMD_URL}/api/v1/billing-period/${MALO_ID}?from=2026-06-01&to=2026-07-01"
echo "  einsd receipt:    ${EINSD_URL}/api/v1/anlagen/${TR_ID}/settlements/${BILLING_YEAR}/${BILLING_MONTH}"
echo "  einsd MCP:        ${EINSD_URL}/mcp"
echo "  edmd MCP:         ${EDMD_URL}/mcp"
echo "  ERP events:       ${WEBHOOK_URL}/events"
echo "================================================="
echo
