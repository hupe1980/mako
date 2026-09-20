#!/usr/bin/env bash
# demos/o2c/smoke.sh — order-to-cash end-to-end smoke test.
#
# The retail money path, one customer, one period:
#
#   1. productd    PUT a Tarifpreisblatt (Grundpreis + Arbeitspreis)
#   2. vertragd    POST a Kunde, then a Versorgungsvertrag on a Marktlokation
#   3. billingd    POST calculate → an EN 16931 invoice priced from that tariff
#   4. billingd    POST versenden → the document is recorded in outputd
#   5. accountingd the invoice is an Offener Posten on the customer account
#   6. accountingd import the payment → the Offener Posten closes
#
# Every step asserts the outcome, not the status code: the invoice must carry
# the amount the tariff and the reading imply, and the receivable must be that
# amount to the cent.
#
# Prerequisites:
#   just build-demo-o2c && docker compose up -d
#
# Usage:
#   bash smoke.sh

set -euo pipefail

PRODUCTD_URL="${PRODUCTD_URL:-http://localhost:9080}"
VERTRAGD_URL="${VERTRAGD_URL:-http://localhost:9780}"
BILLINGD_URL="${BILLINGD_URL:-http://localhost:9280}"
OUTPUTD_URL="${OUTPUTD_URL:-http://localhost:9880}"
ACCOUNTINGD_URL="${ACCOUNTINGD_URL:-http://localhost:9380}"
WEBHOOK_URL="${WEBHOOK_URL:-http://localhost:8001}"

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
pass() { echo -e "${GREEN}✓${NC} $*"; }
fail() { echo -e "${RED}✗${NC} $*"; exit 1; }
info() { echo -e "${YELLOW}▶${NC} $*"; }

# The operator. Synthetic on purpose: an MP-ID that satisfies a published
# check-digit procedure is an assigned code naming a real company, and a public
# demo must not carry one.
LF_MP_ID="9912345000005"
NB_MP_ID="9900357000004"

# A fresh Marktlokation per run, so the demo can be re-run without wiping the
# volume. The eleventh digit is the BDEW check digit (odd positions weight 1,
# even weight 2) — `marktd` and `metering::MaloId` both refuse a bad one.
_EPOCH=$(date +%s)
_BASE="1$(printf '%09d' "$(( _EPOCH % 1000000000 ))")"
_SUM=0
for (( _i=0; _i<10; _i++ )); do
    if (( _i % 2 == 0 )); then (( _SUM += ${_BASE:_i:1} ));
    else (( _SUM += ${_BASE:_i:1} * 2 )); fi
done
MALO_ID="${_BASE}$(( (10 - _SUM % 10) % 10 ))"
unset _BASE _SUM _i

PRODUCT_CODE="STROM-H0-DEMO"
PERIOD_FROM="2026-01-01"
PERIOD_TO="2026-01-31"
# The day the bank statement is for. **Today**, not a date inside the billing
# period: the customer's Kontokorrent is opened in the ledger when the invoice
# first posts to it, and `doubleentry` refuses a posting to an account that was
# not open on the booking date. A payment backdated before the account exists is
# rejected — correctly, and the demo would be asserting the opposite.
BOOKING_DATE="$(date +%F)"
CUSTOMER_IBAN="DE02120300000000202051"

# What the invoice must come to, computed here so the assertion is independent
# of the service that produces it:
#
#   Grundpreis     20 ct/Tag  × 31 Tage          =  6.20 EUR
#   Arbeitspreis   32 ct/kWh  × 250 kWh          = 80.00 EUR
#   Stromsteuer    2.05 ct/kWh × 250 kWh         =  5.125 EUR  (§ 3 StromStG)
#   Netto                                         = 91.325 EUR
#   USt 19 %                                      = 17.35175 → 17.35 EUR
#   Brutto                                        = 108.675 EUR
#
# The Stromsteuer is **not** a caller override: § 3 StromStG fixes it at
# 2.05 ct/kWh and `billingd` applies it to every Strom supply whatever the
# `grid` block says. A demo that expected 86.20 would be asserting that mako
# forgets a tax.
VERBRAUCH_KWH="250"
EXPECTED_NETTO="91.325"
EXPECTED_BRUTTO="108.675"
# What the customer actually owes. A receivable is a payable amount, so the
# ledger holds integer cents — 108.675 rounds kaufmännisch to 108.68, and the
# payment is for that.
EXPECTED_FORDERUNG="108.68"

# Decimal strings come back at whatever scale the service chose ("102.58" vs
# "102.580"), so amounts are compared by value, never by spelling.
numeric_eq() {
    python3 -c "
import sys
from decimal import Decimal, InvalidOperation
try:
    sys.exit(0 if Decimal(sys.argv[1]) == Decimal(sys.argv[2]) else 1)
except (InvalidOperation, ArithmeticError):
    sys.exit(1)
" "$1" "$2"
}

req() {  # req METHOD URL [JSON] → "<body>\n<status>"
    local method="$1" url="$2" data="${3:-}"
    if [ -n "$data" ]; then
        curl -sS -w '\n%{http_code}' -X "$method" "$url" \
            -H 'Content-Type: application/json' -d "$data"
    else
        curl -sS -w '\n%{http_code}' -X "$method" "$url"
    fi
}
status() { tail -n1 <<<"$1"; }
body()   { sed '$d' <<<"$1"; }

wait_for() {  # wait_for NAME URL
    for i in $(seq 1 40); do
        curl -sf "$2" >/dev/null 2>&1 && { pass "$1 is ready"; return 0; }
        [ "$i" -eq 40 ] && fail "$1 did not become healthy within 80s"
        sleep 2
    done
}

echo
echo "================================================="
echo "  mako order-to-cash smoke test"
echo "  productd    → ${PRODUCTD_URL}"
echo "  vertragd    → ${VERTRAGD_URL}"
echo "  billingd    → ${BILLINGD_URL}"
echo "  outputd     → ${OUTPUTD_URL}"
echo "  accountingd → ${ACCOUNTINGD_URL}"
echo "  MaLo        → ${MALO_ID}"
echo "================================================="
echo

# ── 0. Health ────────────────────────────────────────────────────────────────
wait_for productd    "${PRODUCTD_URL}/health"
wait_for vertragd    "${VERTRAGD_URL}/health"
wait_for billingd    "${BILLINGD_URL}/health"
wait_for outputd     "${OUTPUTD_URL}/health"
wait_for accountingd "${ACCOUNTINGD_URL}/health"
echo

# ── 1. productd — the Tarifpreisblatt ────────────────────────────────────────
#
# `billingd` extracts `grundpreis_ct_per_day` and `arbeitspreis_ct_per_kwh` by
# traversing `data.tarifpreise` keyed on `preistyp`. Those fields name their
# unit: a `preis` is read verbatim as **cents**, so 20 ct/Tag is `"20"` and
# 32 ct/kWh is `"32"`. Writing `"0.20"` prices the tariff at a fifth of a cent
# a day, which bills without complaint.
#
# What a PUT stores is the canonical round-trip through `rubo4e`, not the
# request body.
info "[1] productd — PUT the Tarifpreisblatt $PRODUCT_CODE"
PRODUCT_JSON=$(cat <<JSON
{
  "category": "STROM",
  "name": "Strom Zuhause Demo",
  "sparte": "STROM",
  "register_count": "Eintarif",
  "kundentyp": "Haushalt",
  "valid_from": "2026-01-01",
  "product_status": "PUBLISHED",
  "data": {
    "_typ": "TARIFPREISBLATT",
    "bezeichnung": "Strom Zuhause Demo 2026",
    "zeitlicheGueltigkeit": { "startdatum": "2026-01-01" },
    "tarifpreise": [
      { "preistyp": "GRUNDPREIS",            "preisstaffeln": [{ "preis": "20" }] },
      { "preistyp": "ARBEITSPREIS_EINTARIF", "preisstaffeln": [{ "preis": "32" }] }
    ]
  }
}
JSON
)
resp=$(req PUT "${PRODUCTD_URL}/api/v1/products/${LF_MP_ID}/${PRODUCT_CODE}" "$PRODUCT_JSON")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" || "$code" == "204" ]] || \
    fail "PUT /api/v1/products → $code: $(body "$resp")"
pass "PUT /api/v1/products/${LF_MP_ID}/${PRODUCT_CODE} → $code"

resp=$(req GET "${PRODUCTD_URL}/api/v1/products/${LF_MP_ID}/${PRODUCT_CODE}")
[[ "$(status "$resp")" == "200" ]] || fail "GET product → $(status "$resp")"
pass "GET the product back → 200  (the catalogue is the only price source)"
echo

# ── 2. vertragd — the Kunde and the Vertrag ──────────────────────────────────
# `kundentyp` is the **segment** — `B2C` / `B2B_SLP` / `B2B_RLM` / `B2B_HV` —
# and `haushaltskunde` is the § 3 Nr. 57 EnWG fact, which is not the same
# question: a small business under 10 000 kWh a year is a Haushaltskunde too,
# and it decides three deadlines (§ 41 Abs. 5, § 41b Abs. 5, § 309 Nr. 9 BGB).
# The customer is a **BO4E `Geschaeftspartner`** — not a flat bag of `vorname` /
# `strasse` / `plz`. That is not a stylistic preference: § 14 Abs. 4 Nr. 1 UStG
# makes the Leistungsempfänger's full name and address part of what an invoice
# has to state, EN 16931 makes BT-44 mandatory, and the party this object names
# is the party `billingd` puts on the document. Every request body denies
# unknown fields, so a flat payload naming fields the API does not have is a
# `422` naming the field rather than a `201` with a nameless customer and an
# invoice addressed to "Marktlokation 5123…"
# (`cargo xtask check-request-bodies`).
info "[2] vertragd — POST the Kunde (BO4E Geschaeftspartner)"
KUNDE_JSON=$(cat <<JSON
{
  "kundentyp": "B2C",
  "haushaltskunde": true,
  "email": "erika.mustermann@example.org",
  "erp_kunde_id": "DEMO-KUNDE-${MALO_ID}",
  "geschaeftspartner": {
    "anrede": "FRAU",
    "vorname": "Erika",
    "nachname": "Mustermann",
    "adresse": {
      "strasse": "Musterstr.",
      "hausnummer": "1",
      "postleitzahl": "10115",
      "ort": "Berlin",
      "landescode": "DE"
    },
    "kontaktwege": [
      {
        "kontaktart": "E_MAIL",
        "kontaktwert": "erika.mustermann@example.org",
        "istBevorzugterKontaktweg": true
      }
    ]
  }
}
JSON
)
resp=$(req POST "${VERTRAGD_URL}/api/v1/kunden" "$KUNDE_JSON")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" ]] || fail "POST /api/v1/kunden → $code: $(body "$resp")"
KUNDE_ID=$(body "$resp" | jq -r '.id // .kunden_id')
[ -n "$KUNDE_ID" ] && [ "$KUNDE_ID" != "null" ] || fail "no customer id in $(body "$resp")"
pass "POST /api/v1/kunden → $code  (id=$KUNDE_ID)"

# What was stored is the gate's canonical round-trip, not the request body: the
# `_typ` the request omitted is present, and every enum is in BO4E's own wire
# spelling. Reading it back is also the read half of DSGVO Art. 16 — a partial
# `PUT` replaces the whole document, so a client corrects it by reading, editing
# and sending it back.
resp=$(req GET "${VERTRAGD_URL}/api/v1/kunden/${KUNDE_ID}")
[[ "$(status "$resp")" == "200" ]] || fail "GET the Kunde back → $(status "$resp")"
GP=$(body "$resp" | jq -c '.kunde.geschaeftspartner')
[ "$(jq -r '._typ' <<<"$GP")" = "GESCHAEFTSPARTNER" ] || \
    fail "the stored document is not a canonical BO4E Geschaeftspartner: $GP"
[ "$(jq -r '.nachname' <<<"$GP")" = "Mustermann" ] || \
    fail "the customer's name did not survive the round-trip: $GP"
[ "$(jq -r '.adresse.postleitzahl' <<<"$GP")" = "10115" ] || \
    fail "the customer's address did not survive the round-trip: $GP"
pass "GET /api/v1/kunden/$KUNDE_ID → the stored BO4E Geschaeftspartner, _typ stamped by the gate"

# The gate refuses what it cannot store, and says which stage refused. Two
# shapes, because they fail for different reasons and answer differently:
#   * an out-of-schema enum  → 422 bo4e.unknown_enum, with the JSON-path
#   * a field the API has not  → 422, naming the field
resp=$(req POST "${VERTRAGD_URL}/api/v1/kunden" \
    '{"kundentyp":"B2C","geschaeftspartner":{"anrede":"FRAUU"}}')
[[ "$(status "$resp")" == "422" ]] || fail "an out-of-schema Anrede must be a 422, got $(status "$resp")"
[ "$(body "$resp" | jq -r '.code')" = "bo4e.unknown_enum" ] || \
    fail "the refusal must name the gate stage: $(body "$resp")"
[ "$(body "$resp" | jq -r '.paths[0]')" = "anrede" ] || \
    fail "the refusal must name the field: $(body "$resp")"
pass "POST a bad Anrede → 422 bo4e.unknown_enum at \`anrede\`  (the gate, not the handler)"

resp=$(req POST "${VERTRAGD_URL}/api/v1/kunden" '{"kundentyp":"B2C","vorname":"Erika"}')
[[ "$(status "$resp")" == "422" ]] || \
    fail "a field the API does not have must be a 422, got $(status "$resp"): $(body "$resp")"
grep -q 'vorname' <<<"$(body "$resp")" || fail "the refusal must name the field: $(body "$resp")"
pass "POST an unknown field → 422 naming \`vorname\`  (it is not silently dropped)"

# The SEPA mandate is a BO4E `Zahlungsinformation`, on its own sub-resource:
# the IBAN is validated mod-97 and the BIC is checked, neither of which a flat
# `"iban"` field beside the customer would do.
info "[2a] vertragd — PUT the Zahlungsinformation (BO4E)"
resp=$(req PUT "${VERTRAGD_URL}/api/v1/kunden/${KUNDE_ID}/zahlungsinformation" "$(cat <<JSON
{
  "zahlungsart": "SEPA_LASTSCHRIFT",
  "iban": "${CUSTOMER_IBAN}",
  "kontoinhaber": "Erika Mustermann"
}
JSON
)")
[[ "$(status "$resp")" == "200" ]] || \
    fail "PUT zahlungsinformation → $(status "$resp"): $(body "$resp")"
pass "PUT /api/v1/kunden/$KUNDE_ID/zahlungsinformation → 200  (IBAN checked mod-97)"

resp=$(req PUT "${VERTRAGD_URL}/api/v1/kunden/${KUNDE_ID}/zahlungsinformation" \
    '{"zahlungsart":"SEPA_LASTSCHRIFT","iban":"DE00000000000000000000"}')
[[ "$(status "$resp")" == "422" ]] || \
    fail "an IBAN failing mod-97 must be a 422, got $(status "$resp")"
pass "PUT a bad IBAN → 422  (mod-97, before a collection is ever built)"

# `vertragsbeginn` before the billed period, and a household term § 309 Nr. 9
# BGB permits: 24 months is the ceiling, and the tacit extension is into an
# unbefristeten Vertrag (`renewal_monate: 0`).
info "[2b] vertragd — POST the Versorgungsvertrag on MaLo $MALO_ID"
VERTRAG_JSON=$(cat <<JSON
{
  "kundentyp": "B2C",
  "vertragsart": "SONDERVERTRAG",
  "vertragsbeginn": "2026-01-01",
  "kuendigungsfrist_monate": 1,
  "abrechnungszyklus": "MONATLICH",
  "auto_renewal": true,
  "renewal_monate": 0,
  "zahlungsziel_tage": 14,
  "erp_contract_id": "DEMO-${MALO_ID}",
  "standort_bezeichnung": "Musterstr. 1, 10115 Berlin",
  "komponenten": [
    {
      "sparte": "STROM",
      "malo_id": "${MALO_ID}",
      "nb_mp_id": "${NB_MP_ID}",
      "product_code": "${PRODUCT_CODE}",
      "lieferbeginn": "2026-01-01",
      "jahresverbrauch_kwh": "3000"
    }
  ]
}
JSON
)
resp=$(req POST "${VERTRAGD_URL}/api/v1/kunden/${KUNDE_ID}/vertraege" "$VERTRAG_JSON")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" ]] || fail "POST vertrag → $code: $(body "$resp")"
VERTRAG_ID=$(body "$resp" | jq -r '.id // .vertrag_id')
pass "POST /api/v1/kunden/$KUNDE_ID/vertraege → $code  (id=$VERTRAG_ID)"

# Listed under the customer, not under the MaLo. `GET /vertraege/by-malo/{id}`
# answers only for a contract **in supply** — status `AKTIV`, component
# `BESTAETIGT` — and supply starts when the *NB* confirms the Lieferbeginn, not
# when the supplier files the contract. No processd runs here, so this one stays
# `ANGELEGT`, which is the correct state for a contract nobody has answered yet.
#
# That is *not* the same question as "may this period be billed". A filed
# contract is billable and has a customer; only a rejected or withdrawn one has
# neither. One predicate answers both
# (`vertragd::pg::vertraege::KOMPONENTE_BILLABLE`) — a price feed and a
# recipient lookup on different status lists would price this exact contract and
# address the invoice to nobody. Step 3 asserts the name.
resp=$(req GET "${VERTRAGD_URL}/api/v1/kunden/${KUNDE_ID}/vertraege")
[[ "$(status "$resp")" == "200" ]] || fail "GET the customer's contracts → $(status "$resp")"
COUNT=$(body "$resp" | jq -r 'if type=="array" then length else (.vertraege // .items // [] | length) end')
[ "$COUNT" -ge 1 ] || fail "the customer has no contract: $(body "$resp")"
pass "GET /api/v1/kunden/$KUNDE_ID/vertraege → $COUNT contract(s), status ANGELEGT"
echo

# ── 3. billingd — the invoice ────────────────────────────────────────────────
#
# `meter` and `grid` are the documented overrides: this demo runs no edmd, so
# the reading is supplied here and the invoice is reproducible to the cent.
# `demos/eeg-billing` is where a reading actually comes out of edmd.
info "[3] billingd — POST calculate for ${PERIOD_FROM}..${PERIOD_TO}"
CALC_JSON=$(cat <<JSON
{
  "lf_mp_id": "${LF_MP_ID}",
  "nb_mp_id": "${NB_MP_ID}",
  "period_from": "${PERIOD_FROM}",
  "period_to": "${PERIOD_TO}",
  "meter": { "arbeitsmenge_kwh": "${VERBRAUCH_KWH}" },
  "grid": { "netzentgelt_eur": "0", "messentgelt_eur": "0",
            "umlagen_eur": "0", "konzessionsabgabe_eur": "0",
            "stromsteuer_eur": "0" }
}
JSON
)
resp=$(req POST "${BILLINGD_URL}/api/v1/billing/${MALO_ID}/calculate" "$CALC_JSON")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" ]] || fail "POST calculate → $code: $(body "$resp")"
RECHNUNG=$(body "$resp")
BILLING_ID=$(jq -r '.id' <<<"$RECHNUNG")
NETTO=$(jq -r '.total_netto_eur // .netto_eur' <<<"$RECHNUNG")
BRUTTO=$(jq -r '.total_brutto_eur // .brutto_eur' <<<"$RECHNUNG")
RECHNUNGSNUMMER=$(jq -r '.rechnungsnummer' <<<"$RECHNUNG")
[ -n "$BILLING_ID" ] && [ "$BILLING_ID" != "null" ] || fail "no invoice id in $RECHNUNG"
pass "POST /api/v1/billing/${MALO_ID}/calculate → $code  (id=$BILLING_ID)"

# **Who the invoice is addressed to.** § 14 Abs. 4 Nr. 1 UStG makes the
# Leistungsempfänger part of what an invoice has to state and EN 16931 makes
# BT-44 mandatory, so this is not a nicety: without this assertion a nameless
# customer priced correctly and the run is green. The party lives on the billing
# engine's context, which is the one field the BO4E `Rechnung` and the EN 16931
# model both read — naming it twice is how the two maps come to disagree.
resp=$(req GET "${BILLINGD_URL}/api/v1/billing/${BILLING_ID}")
[[ "$(status "$resp")" == "200" ]] || fail "GET the invoice back → $(status "$resp")"
RECORD=$(body "$resp")
BO4E_EMPF=$(jq -c '.rechnung_json.rechnungsempfaenger' <<<"$RECORD")
[ "$(jq -r '.organisationsname' <<<"$BO4E_EMPF")" = "Erika Mustermann" ] || \
    fail "the BO4E Rechnung names no recipient: $BO4E_EMPF"
[ "$(jq -r '.adresse.ort' <<<"$BO4E_EMPF")" = "Berlin" ] || \
    fail "the BO4E Rechnung carries no recipient address: $BO4E_EMPF"
pass "rechnung_json.rechnungsempfaenger → Erika Mustermann, Berlin  (§ 14 Abs. 4 Nr. 1 UStG)"

EN_BUYER=$(jq -c '.en16931_json.buyer' <<<"$RECORD")
[ "$(jq -r '.name' <<<"$EN_BUYER")" = "Erika Mustermann" ] || \
    fail "the EN 16931 model names a different party than the BO4E document: $EN_BUYER"
pass "en16931_json.buyer → the same party  (one field feeds both maps)"

numeric_eq "$NETTO" "$EXPECTED_NETTO" || \
    fail "netto is $NETTO, expected $EXPECTED_NETTO (20 ct × 31 Tage + 32 ct × ${VERBRAUCH_KWH} kWh \
+ 2.05 ct/kWh Stromsteuer)"
pass "netto = $NETTO EUR  (Grundpreis 6.20 + Arbeitspreis 80.00 + Stromsteuer 5.125)"
numeric_eq "$BRUTTO" "$EXPECTED_BRUTTO" || \
    fail "brutto is $BRUTTO, expected $EXPECTED_BRUTTO (netto + 19 % USt)"
pass "brutto = $BRUTTO EUR  (19 % USt, kaufmännisch gerundet)"
echo

# ── 3b. outputd — roll out the invoice layout ────────────────────────────────
#
# outputd ships **no** layout of its own: a customer document carries the
# operator's Briefkopf, and the template is theirs. It refuses to render an
# INVOICE until one is rolled out — a legible `422 NO_CURRENT_TEMPLATE`, not a
# blank page under someone else's letterhead.
#
# `GET /templates/reference/{kind}` is the starting point outputd offers;
# `POST /templates` *proves* it by rendering a specimen invoice to PDF/A-3b
# before storing it, and `PUT /templates/{kind}/current` rolls that proven hash
# out. A template that does not compile is refused with Typst's own diagnostics.
info "[3b] outputd — publish and roll out the INVOICE template"
# `text/plain`, not JSON: the response *is* the Typst source, with the compiler
# version it was written for in the content type.
TEMPLATE_SRC=$(curl -sf "${OUTPUTD_URL}/api/v1/templates/reference/INVOICE") \
    || fail "GET the reference template failed"
[ -n "$TEMPLATE_SRC" ] || fail "the reference template is empty"
pass "GET /api/v1/templates/reference/INVOICE → 200  ($(printf '%s' "$TEMPLATE_SRC" | wc -l | tr -d ' ') lines of Typst)"

resp=$(req POST "${OUTPUTD_URL}/api/v1/templates" \
    "$(jq -n --arg src "$TEMPLATE_SRC" '{"kind": "INVOICE", "source": $src}')")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" ]] || fail "POST /api/v1/templates → $code: $(body "$resp")"
TEMPLATE_HASH=$(body "$resp" | jq -r '.hash')
PROOF=$(body "$resp" | jq -r '.proof')
[ -n "$TEMPLATE_HASH" ] && [ "$TEMPLATE_HASH" != "null" ] || fail "no hash in $(body "$resp")"
pass "POST /api/v1/templates → $code  (hash=${TEMPLATE_HASH:0:12}…, proof=$PROOF)"

resp=$(req PUT "${OUTPUTD_URL}/api/v1/templates/INVOICE/current" \
    "$(jq -n --arg h "$TEMPLATE_HASH" '{"hash": $h}')")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "204" ]] || \
    fail "PUT /api/v1/templates/INVOICE/current → $code: $(body "$resp")"
pass "PUT /api/v1/templates/INVOICE/current → $code  (the layout every invoice renders with)"
echo

# ── 4. outputd — the issued document ─────────────────────────────────────────
#
# § 147 AO keeps an issued invoice for eight years, and reproducing one means
# reproducing the *document*, not re-running the calculation. `versenden`
# records it in outputd and queues it on the customer's channels.
info "[4] billingd → outputd — POST versenden"
resp=$(req POST "${BILLINGD_URL}/api/v1/billing/${BILLING_ID}/versenden" '{}')
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "202" || "$code" == "204" ]] || \
    fail "POST versenden → $code: $(body "$resp")"
pass "POST /api/v1/billing/${BILLING_ID}/versenden → $code"

# Scoped by MaLo. `GET /api/v1/documents` refuses an unscoped list — a document
# store that answers "everything" is a customer-data export, not a query.
# The kind is `INVOICE`: it is what billingd posts to
# `POST /api/v1/documents/issue/INVOICE`.
for i in $(seq 1 15); do
    DOCS=$(curl -sf "${OUTPUTD_URL}/api/v1/documents?malo_id=${MALO_ID}" || echo '[]')
    COUNT=$(jq -r 'if type=="array" then length else (.documents // .items // [] | length) end' <<<"$DOCS")
    [ "$COUNT" -gt 0 ] && break
    [ "$i" -eq 15 ] && fail "no document reached outputd within 30s: $DOCS"
    sleep 2
done
DOC=$(jq -r 'if type=="array" then .[0] else (.documents // .items)[0] end' <<<"$DOCS")
DOC_KIND=$(jq -r '.kind' <<<"$DOC")
DOC_BYTES=$(jq -r '.byte_size' <<<"$DOC")
DOC_MEDIA=$(jq -r '.media_type' <<<"$DOC")
[ "$DOC_KIND" = "INVOICE" ] || fail "the stored document is a $DOC_KIND, expected an INVOICE"
[ "$DOC_MEDIA" = "application/pdf" ] || fail "the stored document is $DOC_MEDIA, expected application/pdf"
pass "GET /api/v1/documents?malo_id=$MALO_ID → $COUNT × $DOC_KIND, $DOC_BYTES bytes of $DOC_MEDIA"

# § 147 AO wants the document *as issued*, so the content route serves the
# stored bytes — a reproduction, never a re-render.
DOC_ID=$(jq -r '.document_id' <<<"$DOC")
BYTES=$(curl -sf -o /dev/null -w '%{size_download}' \
    "${OUTPUTD_URL}/api/v1/documents/${DOC_ID}/content" || echo 0)
[ "$BYTES" = "$DOC_BYTES" ] || \
    fail "the content route served $BYTES bytes, the record says $DOC_BYTES"
pass "GET /api/v1/documents/$DOC_ID/content → $BYTES bytes, byte-identical to the record"
echo

# ── 5. accountingd — the Offener Posten ──────────────────────────────────────
#
# `de.billing.rechnung.erstellt` is a **debit** on the customer account. An
# invoice without it is fire-and-forget: no Offene-Posten tracking, no
# Mahnwesen, no collection.
info "[5] accountingd — the invoice is a receivable"
# Every read is filtered to **this run's** MaLo. The demo is re-runnable without
# a database reset, so earlier runs' accounts are still open in the ledger and
# `.[0]` would assert against whichever one sorted first.
mine() { jq --arg m "$MALO_ID" '[ (if type=="array" then .[] else (.posten // .items // [])[] end)
                                  | select(.malo_id == $m) ]'; }
for i in $(seq 1 15); do
    OP=$(curl -sf "${ACCOUNTINGD_URL}/api/v1/offene-posten" | mine || echo '[]')
    OPEN=$(jq -r 'length' <<<"$OP")
    [ "$OPEN" -gt 0 ] && break
    [ "$i" -eq 15 ] && fail "no Offener Posten for ${MALO_ID} within 30s: $OP"
    sleep 2
done
# `balance_ct` — integer cents, because that is what can be owed and paid.
SALDO_CT=$(jq -r '.[0].balance_ct' <<<"$OP")
SALDO=$(python3 -c "from decimal import Decimal;print(Decimal('$SALDO_CT')/100)")
numeric_eq "$SALDO" "$EXPECTED_FORDERUNG" || \
    fail "the receivable is $SALDO, expected the invoice's $EXPECTED_BRUTTO rounded to $EXPECTED_FORDERUNG"
pass "GET /api/v1/offene-posten → $SALDO EUR open  ($SALDO_CT ct, the invoice to the cent)"
echo

# ── 6. accountingd — the payment closes it ───────────────────────────────────
#
# The flat import contract, not an ISO 20022 message: a real deployment prefers
# `POST /api/v1/payments/import/camt054`, because `EndToEndId`, the `Btch` block
# and return reason codes do not survive a flattening.
#
# The customer is found by a ladder: the counterparty IBAN first (a keyed hash),
# then the `EndToEndId` of a collection this answers, then an **exact
# identifier in the Verwendungszweck** — the Marktlokations-ID or a
# Mandatsreferenz. The invoice number alone is not one of them, so the
# Verwendungszweck here names the MaLo, as a customer's own transfer would.
# Two accounts matching means the text named two customers, and booking either
# would be a guess: the row is skipped, visibly, rather than posted.
info "[6] accountingd — import the payment"
PAY_JSON=$(cat <<JSON
[
  {
    "iban": "${CUSTOMER_IBAN}",
    "amount_eur": "${EXPECTED_FORDERUNG}",
    "date": "${BOOKING_DATE}",
    "reference": "Rechnung ${RECHNUNGSNUMMER} Marktlokation ${MALO_ID}",
    "bank_transaction_id": "DEMO-PAY-${MALO_ID}"
  }
]
JSON
)
resp=$(req POST "${ACCOUNTINGD_URL}/api/v1/payments/import" "$PAY_JSON")
code=$(status "$resp")
[[ "$code" == "200" || "$code" == "201" ]] || fail "POST payments/import → $code: $(body "$resp")"
# The summary says *why* a row was skipped — `unmatched`, `malformed` or
# `failed` — so a red run names the cause instead of reporting a bare count.
ACCEPTED=$(body "$resp" | jq -r '.accepted // 0')
[ "$ACCEPTED" -ge 1 ] || fail "the payment was not accepted: $(body "$resp")"
pass "POST /api/v1/payments/import → $code  (accepted=$ACCEPTED)"

for i in $(seq 1 15); do
    OP=$(curl -sf "${ACCOUNTINGD_URL}/api/v1/offene-posten?min_balance_eur=0.01" | mine || echo '[]')
    OPEN=$(jq -r 'length' <<<"$OP")
    [ "$OPEN" -eq 0 ] && break
    [ "$i" -eq 15 ] && fail "the Offener Posten for ${MALO_ID} did not close within 30s: $OP"
    sleep 2
done
pass "GET /api/v1/offene-posten → 0 open for ${MALO_ID}  (the receivable is settled)"
echo

# ── 7. The ERP saw it happen ─────────────────────────────────────────────────
info "[7] what the ERP receiver saw"
# `de.billing.rechnung.erstellt` goes to **accountingd**, not here: that event
# *is* the debit entry, and step 5 already proved it landed. What reaches the
# ERP receiver is what accountingd itself emits.
#
# `de.accounting.payment.imported` is asserted, not just listed. It leaves
# accountingd through the transactional outbox and its drain worker, so it
# arrives a poll interval after the ledger entry: a single immediate read of the
# receiver is green before the last event has been sent, which is why this polls.
#
# In practice it arrives in milliseconds: the `event_outbox` trigger raises a
# Postgres `NOTIFY` that the drain worker is listening for, and Postgres holds it
# until the import's transaction commits. The window below is nonetheless sized
# past the 30 s poll interval that remains the fallback, because an assertion
# window that depends on the hint arriving is an assertion about latency rather
# than about the event.
# Matched on **this run's** MaLo, not merely on the event type: the receiver
# keeps every run's events, so an earlier run's payment would satisfy a
# type-only wait and the assertions below would then read its amount.
paid_event() {
    jq -r --arg m "$MALO_ID" '[ .[] | select(.body.type == "de.accounting.payment.imported")
                                    | .body.data | select(.malo_id == $m) ] | last'
}
# 45 polls × 2 s = 90 s — three outbox poll intervals, so the run is green even
# with the wake-up hint lost entirely and every delivery on the poll.
OUTBOX_POLLS=45
for i in $(seq 1 "$OUTBOX_POLLS"); do
    EVENTS=$(curl -sf "${WEBHOOK_URL}/events" || echo '[]')
    TYPES=$(jq -r '[.[].body.type] | unique | sort | join(", ")' <<<"$EVENTS")
    PAID=$(paid_event <<<"$EVENTS")
    [ "$PAID" != "null" ] && break
    [ "$i" -eq "$OUTBOX_POLLS" ] && \
        fail "de.accounting.payment.imported for ${MALO_ID} never reached the ERP \
in $(( OUTBOX_POLLS * 2 )) s: ${TYPES:-<none>}"
    sleep 2
done
[ "$(jq -r '.amount_eur' <<<"$PAID")" = "$EXPECTED_FORDERUNG" ] || \
    fail "the ERP was told a different amount: $(jq -c . <<<"$PAID")"
# Which rung of the resolution ladder matched. The demo pays from an IBAN
# accountingd has never seen (the account was opened from a billing event,
# which carries none), so the Verwendungszweck is what identifies the customer.
MATCHED_BY=$(jq -r '.matched_by' <<<"$PAID")
[ "$MATCHED_BY" = "remittance_token" ] || \
    fail "expected the payment to be matched by its Verwendungszweck, got '$MATCHED_BY'"
pass "ERP events: ${TYPES:-<none>}"
pass "de.accounting.payment.imported → ${EXPECTED_FORDERUNG} EUR, matched_by=${MATCHED_BY}"
echo

echo -e "${GREEN}All order-to-cash smoke tests passed.${NC}"
echo "  Flow: Tarifpreisblatt → Vertrag → Rechnung → Dokument → Offener Posten → Zahlung"
echo "  MaLo ${MALO_ID}, Rechnung ${RECHNUNGSNUMMER}, ${EXPECTED_FORDERUNG} EUR settled"
