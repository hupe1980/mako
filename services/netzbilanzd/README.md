# netzbilanzd — the Netzbetreiber's outbound billing daemon

`netzbilanzd` settles, checks and dispatches every invoice a German network operator owes its
counterparties: Netznutzungsentgelt and Konzessionsabgabe, the Mehr-/Mindermengensaldo, the
Messstellenbetrieb charge and the GeLi Gas Sperrprozess fees — and it carries the Redispatch 2.0
cost sheets. It closes the payment lifecycle when the REMADV comes back. No `f64` anywhere in the
billing path.

| Attribute | Value |
|---|---|
| **Port** | `:8680` |
| **Database** | PostgreSQL — `invoice_drafts`, `invoice_number_seq`, `abschlag_verrechnungen`, `kostenblatt_records`, `fremdkosten_records` |
| **Calculation** | `grid-billing` — pure, I/O-free, every position carries a `CalculationTrace` and its `LegalReference`s |
| **Money** | `i64` × 10⁻⁵ EUR end to end; net, Umsatzsteuer, gross **and what is left to collect** each stored, and checked to add up. A deduction may leave a Guthaben; it can never enlarge the invoice |
| **Umsatzsteuer** | 19 % on network services; §13b reverse charge on Mehr-/Mindermengen when the counterparty holds §3g status |
| **Pre-dispatch gate** | `invoic-checker`, run on the document that will actually be sent; a `Dispute` verdict blocks it |
| **Invoice numbering** | allocated by the database, consecutive per tenant/series/year (§14 Abs. 4 Nr. 4 UStG) |
| **Lifecycle** | `draft` → `dispatched` → `paid` \| `disputed`; `draft` → `rejected`. Only a dispatched invoice can be reversed or corrected |
| **Abschläge** | PID 31001 payments on account, deducted from what is owed by the invoice that settles the period (AHB [519]/[526] enforced) |
| **Corrections** | Stornorechnung recomputed from the stored settlement input and negated; Korrekturrechnung settled from corrected inputs |
| **CloudEvents** | `de.netzbilanz.invoic.{drafted,dispatched,paid,disputed,dispatch-overdue}` · `de.netzbilanz.kostenblatt.{computed,deadline-approaching}` — all seven through the transactional outbox |
| **MCP server** | 8 **read-only** tools · 6 prompts at `/mcp` (Streamable HTTP) |
| **Retention** | § 147 Abs. 3 AO / § 14b UStG — invoices are Buchungsbelege, 8 years |
| **Health** | `GET /health` · `GET /health/ready` |

## What it issues

| PID | Document | Direction | `billing_type` |
|---|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung (payment on account) | NB → LF | `abschlag` |
| 31002 | NN-Rechnung (Netznutzungsentgelt + Konzessionsabgabe) | NB → LF | `nne` |
| 31005 | Mehr-/Mindermengensaldo | NB → LF | `mmm` |
| 31009 | MSB-Rechnung (Messstellenbetrieb) | **MSB → NB / LF / ESA** | `msb` |
| 31011 | Rechnung sonstige Leistung (GeLi Gas AWH Sperrprozesse) | GNB → LFG | `gas_awh` |

Two things about this table are easy to get wrong:

- **The Sparte is not in the Prüfidentifikator.** NN-Rechnung Strom and Gas both use 31002, and both
  MMM variants use 31005. Every position states its `sparte`, which selects StromNEV §21 or
  GasNEV §14, decides whether the three EnFG network levies apply at all, and reaches the wire on
  `Rechnung.sparte`.
- **31009 runs the other way.** The Messstellenbetreiber issues it in all seven of its
  Anwendungsfälle (*Anwendungsübersicht der Prüfidentifikatoren* 4.0); it is never addressed to one.
  The draft stores the MSB as `sender_mp_id`.

## Settling an invoice

Each position names one MaLo, one period and one settlement. The settlement is a tagged union, so it
carries exactly the fields that settlement takes — and a field belonging to another kind is a 422,
not a silently ignored key.

```bash
curl -X POST http://localhost:8680/api/v1/billing/run \
  -H "Content-Type: application/json" \
  -d '{
    "invoice_date": "2026-02-01",
    "due_date": "2026-03-03",
    "rechnungskreis": "NNE",
    "positions": [{
      "malo_id": "51238696012",
      "period_from": "2026-01-01",
      "period_to": "2026-01-31",
      "settlement": {
        "billing_type": "nne",
        "nb_mp_id": "9900357000004",
        "lf_mp_id": "9900012345678",
        "sparte": "Strom",
        "arbeitspreis": {
          "Einheitlich": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" }
        },
        "konzessionsabgabe": {
          "satz_ct_per_kwh": "0.11",
          "klasse": "Sondervertragskunde"
        },
        "netzebene": "Niederspannung"
      }
    }]
  }'
```

```jsonc
{
  "drafted": 1,
  "drafts": [{
    "draft_id": "550e8400-…",
    "malo_id": "51238696012",
    "rechnungsnummer": "NNE-2026-000001",   // allocated here, never by the caller
    "pid": 31002,
    "sparte": "STROM",
    "check_outcome": "Ok",
    "check_findings": [],
    "settlement_warnings": [],
    "netto_eur":     "82.68000",
    "steuer_eur":    "15.70920",
    "brutto_eur":    "98.38920",
    // What is left to collect after any Abschlag this invoice settles. Equal to
    // the gross when none is deducted — and the figure the payment run uses.
    "zu_zahlen_eur": "98.38920"
  }]
}
```

A run is one transaction and carries at most 1 000 positions — either every position is billed or
none is.

`rechnungskreis` only names the series; the running number comes from the database, under a row lock,
inside the drafting transaction. A rolled-back run consumes no number and a retried run cannot reuse
one — which is what *einmalig vergeben und fortlaufend* means.

### Umsatzsteuer

Every invoice states its tax. §14 Abs. 4 Nr. 8 UStG requires the rate and the amount, and an
invoice carrying only a net figure is worth no Vorsteuerabzug to the counterparty.

What is taxed how turns on **what is being supplied**, which is not the same axis as the Sparte:

| Settlement | Nature | Treatment |
|---|---|---|
| NNE, MSB, Gas AWH | *sonstige Leistung* | 19 %. UStAE 13b.3a excludes network services from §13b by name |
| MMM | **Lieferung** of electricity or gas | 19 %, or reverse-charged under §13b Abs. 2 Nr. 5 Buchst. b |

The §13b condition is asymmetric, and §13b Abs. 5 states it twice on purpose:

- **Elektrizität** — the recipient owes the tax where the supplier **and** the recipient are
  Wiederverkäufer im Sinne des §3g.
- **Gas über das Erdgasnetz** — the **recipient** alone decides it.

So an MMM position carries both facts, evidenced by a valid *USt 1 TH*:

```jsonc
"wiederverkaeufer": { "leistender": true, "empfaenger": true }
```

Getting it backwards is not a rounding error: tax shown on a reverse-charge invoice is owed under
§14c Abs. 1 UStG **and** gives the recipient no Vorsteuerabzug, because the recipient still owes it
under §13b. The pre-dispatch gate refuses both shapes.

A delivery period that straddles a rate change is **refused**, not billed at one of the two rates —
the 7 % gas window (01.10.2022 – 31.03.2024, §28 Abs. 5 UStG) reached the *Lieferung* of gas, so a
Gas MMM inside it is 7 % while a Netznutzung Gas invoice for the same period is 19 %.

**Read the warnings.** `check_findings` comes from `invoic-checker` and `settlement_warnings` from the
engine. A `KA_ABOVE_KAV_MAXIMUM` warning means the Konzessionsabgabe exceeds the KAV §2 Höchstbetrag
for the group you named — 1.32 ct/kWh is the Tarifkunden maximum for a Gemeinde up to 25 000, and
twelve times the 0.11 ct/kWh ceiling a Sondervertragskunde may be charged.

### §14a EnWG

Pass the module instead of a flat rate. The three are mutually exclusive by construction
(BK6-22-300 / BK8-22/010-A):

```jsonc
// Modul 1 — pauschale Reduzierung: the energy is billed in full and a flat
// annual amount is credited pro rata. It does not scale with consumption.
"arbeitspreis": { "Modul1Pauschal": {
  "basis": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" },
  "pauschale_eur_pro_jahr": "120.00", "jahresanteil": "0.0849"
}}

// Modul 2 — prozentuale Reduzierung of the controllable device's own
// Arbeitspreis. The factor is range-checked at the request boundary.
"arbeitspreis": { "Modul2ProzentualeReduzierung": {
  "basis": { "menge_kwh": "800", "preis_ct_per_kwh": "3.5" }, "reduktion": "0.85"
}}

// Modul 3 — zeitvariable Netzentgelte (opt-in since 01.04.2025). All three
// Tarifstufen are required; a band with no energy carries menge_kwh 0.
"arbeitspreis": { "Modul3ZeitVariabel": {
  "ht": { "menge_kwh": "600", "preis_ct_per_kwh": "4.20" },
  "st": { "menge_kwh": "100", "preis_ct_per_kwh": "3.00" },
  "nt": { "menge_kwh": "400", "preis_ct_per_kwh": "1.50" }
}}
```

### Abschläge and the invoice that settles them

An **Abschlagsrechnung** asks for a payment on account against a period not yet settled. It
prices no energy, so it carries no quantity and no Arbeitspreis — and **exactly one**
Positionszeile, which the INVOIC AHB 1.0b requires by name (Änd-ID 26817, `LIN DE1082` = 1):

```jsonc
"settlement": {
  "billing_type": "abschlag",
  "nb_mp_id": "9900357000004", "lf_mp_id": "9900012345678",
  "sparte": "Strom",
  "betrag_netto_eur": "1000.00",
  // How the figure was arrived at. Recorded, not computed: the engine cannot
  // check a forecast, but an audit can ask which basis was used.
  "grundlage": "Vorjahresverbrauch"   // · Prognose · Vereinbarung
}
```

The invoice that closes the period deducts them **by draft ID**:

```jsonc
{
  "malo_id": "51238696012",
  "period_from": "2026-01-01", "period_to": "2026-12-31",
  "cadence": "Abschlussrechnung",          // IMD+7081
  "abschlaege": ["550e8400-…", "6ba7b810-…"],
  "settlement": { "billing_type": "nne", … }
}
```

Four properties of that deduction are enforced rather than trusted:

- **It reduces what is owed, never what was supplied.** §14 Abs. 5 UStG taxes an Anzahlung when
  it is received, so the Abschlussrechnung must not tax the same money twice: `gesamtnetto` and
  `gesamtsteuer` stand, and only `zuZahlen` moves. The AHB puts it in the Summenteil
  (`SG50 MOA+113`) for the same reason.
- **The amount comes from the stored Abschlag, not the request.** AHB rule **[526]** requires the
  deducted amount to equal the referenced invoice's own Rechnungsbetrag, and a caller-supplied
  figure is precisely the one that can disagree with it.
- **A reversed Abschlag is refused.** AHB rule **[519]** excludes a stornierte
  Abschlagsrechnung — nothing was paid on it, so deducting it would credit money that never moved.
- **An Abschlag is deducted once.** Each deduction is a row in `abschlag_verrechnungen`, written in
  the drafting transaction under a primary key on `(tenant, abschlag_draft_id)`, so a second invoice
  naming the same Abschlag is a `409` (`AbschlagAlreadyDeducted`) instead of a second well-formed
  credit. The rows are released when the consuming invoice is rejected, or when its Storno is
  **dispatched** — not when the Storno is drafted, because a drafted reversal can still be rejected
  and the original still stands on the wire until it goes out.

Each deduction names the invoice it reconciles against (`SG51 RFF+AFL` + `DTM+3`), because a
total the counterparty cannot break down is a total it will dispute.

A period carries many Abschläge and one final invoice, so the double-billing guard excludes
PID 31001. Its own guard is **one Abschlagsrechnung per MaLo, period and Rechnungsdatum**:
instalments differ by that date, a replayed billing run does not.

### Dispatching

```bash
# Optional: attach typed external costs first. They are merged into the
# document's own `fremdkosten` field — BO4E models this, so it does not
# travel as a free-text ZusatzAttribut.
curl -X PUT http://localhost:8680/api/v1/billing/fremdkosten/550e8400-… -d @fremdkosten.json

# Merge, re-check, and hand to makod. A Dispute verdict blocks the send.
curl -X PUT http://localhost:8680/api/v1/billing/drafts/550e8400-…/dispatch
```

Both the command and the asserted `marktrolle` follow the PID **and** the Sparte: 31002 Gas goes to
`invoic.nne.stellen` as `GNB`, PID 31009 as `MSB`, everything else as `NB`. `makod`
checks the assertion against the deployment's licensed roles, so a gas invoice asserting `NB` is
refused on a `--marktrollen GNB` instance. The payload carries `invoice_ref` (the invoice number —
the business key the inbound REMADV correlates on), both party MP-IDs, the PID, the Sparte and the
document.

Fremdkosten are **informational**: BO4E models them as a cost breakdown beside the invoice, not as
positions that add to it, so they change what the document explains and not what it charges.
Billable third-party costs belong in the settlement. Only a `draft` accepts them, because the merge
happens at dispatch.

Every PID this service issues has an issuer-side process in `makod`, including PID 31011: the
GeLi Gas workflow models both ends of the conversation, so an AWH invoice dispatches and its
REMADV correlates back like any other.

### Correcting

A Stornorechnung is **recomputed** — the stored settlement input is replayed through the engine and
the result negated by `grid_billing::reverse`, so every position flips sign and the document declares
itself a reversal on `ist_storno` + `original_rechnungsnummer`, which is what the counterparty's own
`invoic-checker` reads. A Korrekturrechnung is a fresh settlement from corrected inputs, not an edited
document.

```bash
# 1. Reverse the original. 2. Then issue the corrected invoice.
curl -X POST .../drafts/{id}/storno    -d '{"grund": "Messwertkorrektur"}'
curl -X POST .../drafts/{id}/korrektur -d '{"grund": "Messwertkorrektur", "settlement": { … }}'
```

**The order is enforced.** A Korrekturrechnung carries the *whole* corrected amount, not the
difference, so issuing one against a live invoice bills the period twice — and both documents are
well-formed, so nothing downstream notices. `/korrektur` is a `409` until the original is reversed.

The reason is part of the audit trail: `Rechenfehler` and `Stammdatenkorrektur` indicate a defect
worth counting; `RegulatorischeAenderung` is a lawful recalculation.

Four guards sit on that path.

- **An invoice is reversed once** — a second Storno is a `409`. Both reversals would be
  well-formed documents crediting the counterparty twice.
- **Only a dispatched invoice can be reversed or corrected.** `draft` and `rejected` both mean the
  counterparty never received it, so both are a `409`.
- **The recomputation must reproduce the original exactly** — net, Umsatzsteuer *and* gross, or the
  reversal is refused with both figures named. The tax is derived from the §13b Wiederverkäufer
  status and the rate window, so a corrected table can move it while the net matches.
- **A Korrekturrechnung must correct the same invoice.** The corrected settlement is
  caller-supplied and corrections are exempt from the double-billing guard, so a changed
  `settlement_type`, `sparte`, `sender_mp_id` or `recipient_mp_id` is a `422`.

### Mehr-/Mindermengen

```bash
curl -X POST http://localhost:8680/api/v1/billing/mmm-run/51238696012 \
  -d '{"nb_mp_id":"9900357000004","lf_mp_id":"9900012345678","sparte":"Gas",
       "period_year":2026,"period_month":1,"bilanziert_kwh":"1000.000"}'
```

`bilanziert_kwh` is required and cannot be fetched: it is what the Bilanzkreis was charged from the
load profile, which lives on the balancing side. `edmd` holds only the measured half. `sparte` also
picks the balancing day `edmd` aggregates over — gas balances on the 06:00 Gastag, not the calendar
day — and which price series is auto-fetched: Trading Hub Europe per Marktgebiet for Gas, and for Strom
the nationwide BDEW series (§ 13 Abs. 3 StromNZV makes those prices *einheitlich*, so the
application month is the whole key and there is no per-operator series to configure). The price
fetch is memoised per run: one round-trip per
`(Sparte, year, month)`, since a monthly sweep settles every MaLo against the same series.

Every JSON request body rejects unknown fields: a misspelt `konzessionsabgabe` would otherwise drop
that charge from the invoice. Query strings are not strict — an unknown query parameter is a proxy
artefact, not a missing charge.

The sign convention is defined from the network operator's side (GPKE BK6-24-174 Teil 1 Kap. 8.4;
GaBi Gas 2.1 Tenor Nr. 5), which inverts the intuitive reading: measured **below** profiled is an
ungewollte *Mehrmenge* and the NB **credits** the LF; measured **above** profiled is an ungewollte
*Mindermenge* and the NB **charges**.

### §42b EnWG Gemeinschaftliche Gebäudeversorgung

```bash
curl -X POST http://localhost:8680/api/v1/billing/ggv-nne/51238696012 \
  -d '{"nb_mp_id":"…","lf_mp_id":"…","period_from":"2026-01-01","period_to":"2026-01-31",
       "arbeitspreis_ct_per_kwh":"5.50",
       "tenant_consumption":{"51238696781":"450.000","51238696129":"550.000"}}'
```

`tenant_consumption` is required. §42b attributes the Netzentgelt to each tenant Marktlokation, and an
equal split is not an attribution — it bills one tenant for another's consumption. The whole building
settles in one transaction: either every tenant is billed or none is.

## Listing, paging and reporting

```bash
# Gas invoices only — PID 31002 and 31005 are each shared between the Sparten.
curl '.../api/v1/billing/drafts?sparte=Gas&status=dispatched&limit=100'

# The next page. Cursors are stable against inserts; OFFSET is not.
curl '.../api/v1/billing/drafts?limit=100&after=2026-02-01T08:30:00Z_550e8400-…'
```

`/drafts` and `/audit` return `next_cursor`, omitted on the last page. It is the `(created_at, id)`
of the last row — a keyset, so paging an eight-year Buchungsbeleg table neither re-reads the prefix
nor skips a row inserted mid-walk.

`GET /api/v1/billing/summary` totals net, Umsatzsteuer, gross and `zu_zahlen` per month. Reconcile
against `zu_zahlen`: the gross is what was invoiced, not what is left to collect.

## Redispatch 2.0

```bash
# Quantify an activation from the edmd Lastgang, summed over the exact window.
curl -X POST .../api/v1/redispatch/kostenblatt/{activation_id}/compute -d @activation.json

# What is still unquantified before the 15th.
curl .../api/v1/redispatch/kostenblatt/gaps/2026/1

# Submit the month (BK6-20-061 §4.2).
curl -X POST .../api/v1/redispatch/kostenblatt/submit/2026/1
```

Check `dispatch_source` on the result. `lastgang_sum` is the intended path; `billing_period` means the
monthly aggregate was used because no Lastgang existed, which for a 15-minute activation is wrong by
three orders of magnitude and should be replaced with a verified `dispatch_kwh_override`.

The window is half-open `[start, end)` and the Lastgang's own UTC offset is honoured — an
`11:00:00+01:00` interval is summed as 10:00 UTC.

Compensation to the curtailed operator is a separate calculation:
`POST /api/v1/redispatch/verguetung/{activation_id}/compute` (§13a Abs. 2 EnWG). An **Aufforderungsfall**
settles against the schedule transmitted to the EIV, not against the measured Lastgang — the endpoint
refuses to guess, because using the wrong counterfactual misstates the compensation in whichever
direction the plant deviated.

A **Duldungsfall** settles against the measured Lastgang, and the endpoint refuses a window the
series does not span: an activation half of which is missing pays the Anlagenbetreiber for half of
what it lost, and nothing in the kWh says so. Completeness is judged from the intervals — they must
be contiguous, and what they leave uncovered at either end must be shorter than one interval. That
is exactly the misalignment an activation running 10:07–11:07 against a quarter-hour series
produces, and it is never what a genuinely absent interval looks like. The `coverage_pct` `edmd`
reports is measured against the window **as requested**, so a fully metered hour of that shape
reports around 75 %; it travels back with the figure as information, not as the test.

The stateless BilAReM Kap.-3 Ausfallarbeit engine sits at `/api/v1/redispatch/ausfallarbeit/*`.

## Configuration

```toml
# netzbilanzd.toml
port   = 8680
tenant = "9900357000004"        # NB MP-ID / logical tenant name

marktd_url     = "http://marktd:8180"
marktd_api_key = "env:NETZBILANZD_MARKTD_API_KEY"
makod_url      = "http://makod:8080"
makod_api_key  = "env:NETZBILANZD_MAKOD_API_KEY"
edmd_url       = "http://edmd:8380"
edmd_api_key   = "env:NETZBILANZD_EDMD_API_KEY"

# Start without authentication (dev only). Without it the daemon refuses to
# start unless [oidc] and inbound_secret are both configured.
# allow_insecure_no_auth = true

# All CloudEvents go here. Delivery is durable: each event is written to
# `event_outbox` in the same transaction as the change it describes and drained
# by a worker with retry and dead-lettering, so a crash never drops one.
erp_webhook_url    = "http://erp:9000/webhooks/mako"
erp_webhook_secret = "env:NETZBILANZD_WEBHOOK_SECRET"

# Verifies inbound REMADV CloudEvents. Set it: without it a forged REMADV can
# mark an invoice paid, or contest one that was not.
inbound_secret = "env:NETZBILANZD_INBOUND_SECRET"

# Alert workers. 0 disables either one.
dispatch_alert_interval_secs    = 3600
dispatch_stale_hours            = 48
kostenblatt_alert_interval_secs = 86400

[database]
url = "postgres://nb:secret@db:5432/netzbilanzd"

[mcp]
api_key = "env:NETZBILANZD_MCP_API_KEY"

# Verifies the bearer token on every REST route and on /mcp.
[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-netzbilanzd"
```

## Authentication and authorization

Every REST route takes a verified OIDC token and is then checked against
[`policies/netzbilanzd.cedar`](policies/netzbilanzd.cedar). The daemon refuses to start without
`[oidc]`, without `inbound_secret`, or with an MCP surface that has neither `[oidc]` nor an `[mcp]`
key — each of those doors opens invoice dispatch, Storno, mark-paid, the § 147 AO export or the
Kostenblatt submission to anyone who can reach the port. `allow_insecure_no_auth = true` accepts
that posture for local development and says so loudly at startup.

| Action | Routes | Who |
|---|---|---|
| `read-settlement` | `GET /billing/drafts[/{id}]`, `/malo/{malo_id}`, `/summary`, `/fremdkosten/{draft_id}` | any caller of the tenant |
| `export-audit` | `GET /billing/audit` | any caller of the tenant |
| `read-kostenblatt` | `GET /redispatch/kostenblatt[/{activation_id}]`, `/kostenblatt/gaps/{year}/{month}` | any caller of the tenant |
| `compute-ausfallarbeit` | `POST /redispatch/ausfallarbeit/*` | any caller of the tenant |
| `run-settlement` | `POST /billing/run`, `/mmm-run/{malo_id}`, `/ggv-nne/{ggv_malo_id}` | NB, MSB |
| `amend-settlement` | `PUT /billing/fremdkosten/{draft_id}`, `/drafts/{id}/reject` | NB, MSB |
| `dispatch-settlement` | `PUT /billing/drafts/{id}/dispatch`, `POST /drafts/dispatch-batch` | NB, MSB |
| `correct-settlement` | `POST /billing/drafts/{id}/storno`, `/korrektur` | NB, MSB |
| `record-payment` | `PUT /billing/drafts/{id}/mark-paid`, `/mark-disputed` | NB, MSB |
| `compute-kostenblatt` | `PUT /redispatch/kostenblatt/{activation_id}`, `POST …/compute` | NB, ÜNB |
| `submit-kostenblatt` | `POST /redispatch/kostenblatt/submit/{year}/{month}` | NB, ÜNB |
| `compute-verguetung` | `POST /redispatch/verguetung/{activation_id}/compute` | NB, ÜNB |
| `use-mcp` | `/mcp` | any caller of the tenant |

Reading is a different action from every write, so a token carrying no market role — an auditor's —
reaches the § 147 AO export and can dispatch, reverse or settle nothing.

`POST /api/v1/webhooks/remadv` is the one route with no bearer token: it is authenticated by the
`inbound_secret` HMAC, which is also replay-checked.

`tests/authorization_guard.rs` pins the surface in three directions, none of which the compiler
sees: a handler with no `Claims` extractor is unauthenticated, a handler that takes `Claims` and
authorizes nothing is open to every accepted token, and a Cedar action checked in code but absent
from the policy is a permanent 403 (Cedar is default-deny) while a policy action nothing checks is a
dead grant. The REMADV webhook is the one declared exception.

## MCP server

Eight read-only tools and six prompts at `/mcp`: `list_drafts`, `get_draft`, `list_disputed`,
`list_undispatched`, `list_corrections`, `get_billing_summary`, `list_pending_kostenblatt`,
`list_kostenblatt_gaps`.

`list_drafts` filters by MaLo, party, Prüfidentifikator, **Sparte**, status, Rechnungsart and
checker verdict. The Sparte filter matters more than it looks: PID 31002 and 31005 are each shared
between Strom and Gas, so the Prüfidentifikator alone cannot answer "show me the gas invoices".

Nothing there mutates. Dispatching an invoice sends EDIFACT to a counterparty and starts a payment
obligation whose only reversal is a Stornorechnung; model output is untrusted input, so settling,
dispatching, rejecting and correcting live on the REST API where the action is attributable to an
operator. **Read over MCP, act over REST.**

## See also

- [Operator guide](https://hupe1980.github.io/mako/docs/services/netzbilanzd/) — full API reference and diagrams
- [`grid-billing`](../../crates/grid-billing/README.md) — the settlement engine
- [`invoic-checker`](../../crates/invoic-checker/README.md) — the pre-dispatch gate
