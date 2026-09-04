# accountingd — Massenkontokorrent / Customer Account Ledger

`accountingd` is the **FI-CA equivalent** for the mako retail billing stack. Without it,
`billingd` invoices are fire-and-forget — no Offene-Posten tracking, no automated dunning,
no SEPA collection.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9380` |
| **Database** | PostgreSQL (sqlx 0.8) — customer/SEPA satellites in `public`, the ledger in the `doubleentry` schema of the same database |
| **Auth** | OIDC/JWT **+ Cedar ABAC** on every endpoint, read and write, plus inbound webhook HMAC-SHA256. Reads split three ways — `read-account` (one customer), `read-banking` (IBANs, mandates, pain.001), `read-books` (trial balance, aging, seals). `tests/authorization_guard.rs` fails the build if a handler loses its `Claims` extractor or its Cedar check |
| **Ledger** | The [`doubleentry`](https://github.com/hupe1980/doubleentry) crate: an **immutable, tamper-evident double-entry engine** — balanced by construction, an append-only Merkle log with inclusion/consistency proofs, period seals (GoBD/§ 146 AO), open-item clearing, and store-level idempotency. accountingd owns the chart of accounts (`ledger::Chart`) and the `entry_type → postings` mapping; every money movement flows through `pg::post_entry` → `ledger.post`. |
| **Double-entry** | Each Buchungsart is one balanced entry — the per-MaLo Kontokorrent (SKR 1400) against a GL contra leaf. Soll = Haben is enforced in-engine *and* by a deferred DB trigger (§ 238 HGB) |
| **Vorauszahlung** | `PUT/GET /api/v1/accounts/{malo_id}/vorauszahlung` — typed `rubo4e::current::Vorauszahlung` (§40 Abs. 1 EnWG) |
| **Aging analysis** | `GET /api/v1/aging` — receivables by 0–30d / 31–60d / 61–90d / >90d |
| **Verzugszinsen** | `GET/POST /api/v1/accounts/{malo_id}/interest-charges` — §288 BGB, §247 BGB Basiszinssatz + 5 pp (B2C) / + 9 pp (B2B). Own `VERZUGSZINSEN` Buchungsart booking to a **Zinsertrag** GL account, because §275 HGB reports *Zinsen und ähnliche Erträge* on their own line. A period with no announced Basiszinssatz seeded is refused rather than estimated |
| **Payment plans** | `GET/POST /api/v1/accounts/{malo_id}/payment-plans` — Zahlungsvereinbarung |
| **SEPA mandates** | IBAN validated via **ISO 13616 mod-97 + the SWIFT registry's per-country BBAN structure** on PUT (mod-97 alone misses an `O` typed for a `0` about 99 % of the time it is the only error); `sepa_mandates` table (UNIQUE per tenant) |
| **CORE vs B2B** | The scheme lives on the mandate **and** on every collection entry (a pain.007 restates the original as submitted), and one `PmtInf` group carries exactly one scheme. Different rulebooks: a CORE debtor has an unconditional 8-week refund right, a B2B debtor has none and their bank must hold the mandate |
| **36-month dormancy** | EPC SDD Core Rulebook — a mandate unpresented for 36 months must be cancelled by the creditor. The clock resets on **presentation**, so `last_presented_at` is stamped when the collection enters a run |
| **SEPA scheduler** | Background worker generates **one pain.008 message per collection date** (one `PmtInf` group per scheme × SequenceType, mandatory Gläubiger-ID); persisted in `sepa_collection_runs` **with one `sepa_collection_entries` row per collected mandate**. Pre-notification runs **14 calendar days** ahead per the EPC rulebook (`sepa_pre_notification_days`) |
| **SEPA Gläubiger-ID** | `creditor_id` config field (EPC AT-02); validated via `sepa::validate_creditor_id`; included as `<CdtrSchmeId>` |
| **Structured `PstlAdr`** | ISO 20022 postal addresses on creditor **and** debtor, ahead of the EPC cut-over on **15 Nov 2026**; half-filled addresses are refused, `Ctry` is checked against ISO 3166, legacy DK schemas drop it rather than emit an XSD violation |
| **FRST→RCUR transition** | Auto-transitions FRST mandate to RCUR after first successful collection |
| **Bank statement import** | `POST /api/v1/payments/import/camt053` (end-of-day) · `.../camt054` (intraday notification) · `.../camt052` (intraday report) · `.../import` (flat JSON fallback) — one booking pipeline, one sign convention, deduplicated by the bank's own transaction reference. Only `Ntry/Sts = BOOK` posts: `INFO` is not a money movement and `PDNG` may still be dropped, and an append-only ledger cannot un-book either |
| **Payment resolution** | Strongest evidence first — counterparty IBAN → `EndToEndId` → an exact Mandatsreferenz/MaLo-ID token in the Verwendungszweck. Matching on the IBAN alone loses every payment made from a spouse's or employer's account; whole-token matching (never substring) stops a stranger's payment landing on a customer. Ambiguous references resolve to nothing and are counted `unresolved` |
| **Batch integrity** | A camt batch whose itemised details do not sum to the entry total is logged and counted — that difference is money booked at the bank reaching no customer account |
| **ISO 20022 purpose** | `Purp/Cd` from the account's Sparte: `ELEC` / `GASB` / `WTER` / `ENRG`. Learned from `de.billing.rechnung.erstellt` |
| **pain.002 ingestion** | `POST /api/v1/sepa/pain002` — applies the bank's status report to payouts *and* collections, including **Verification of Payee** (mandatory since 9 Oct 2025), which is stored on its own axis rather than mistaken for an acceptance |
| **pain.007 reversal** | `POST /api/v1/sepa/reversals` — the creditor gives a settled collection back; `OrgnlTxRef` is restated from stored data (the DK subset makes it mandatory), one reversal per collection, `SEPA_STORNO` re-opens the receivable |
| **IBAN encryption ready** | `iban_hash` is an app-computed **keyed BLAKE3** lookup key (no pgcrypto); `iban_encrypted` flag; CAMT.054 matching uses the hash even when the IBAN is ciphertext |
| **Abschlag model** | An advance is a **receivable**, not a receipt: `ABSCHLAG` debits the Kontokorrent when the demand is raised, so an unpaid advance reaches the Verzug and the Mahnwesen |
| **Mahnwesen** | Mahnstufe 1→2→3; auto-dunning worker (advisory-locked, opt-in). Every step re-checks the **live** receivable, and a case whose arrears are settled is closed rather than escalated. Each open case is rendered and delivered as a MAHNUNG through `outputd` (recipient from `vertragd`), and a case that cannot be addressed is not documented |
| **Sperr-Sequenz (§§41f/41g EnWG)** | Four phases, every one applying the same § 41f Abs. 3 gates: **Sperrandrohung** (Abs. 1, 4 Wochen) → **Sperrankündigung** (Abs. 5, 8 Werktage brieflich) → **Sperrauftrag** (ORDERS **17115** via `makod`) → **Entsperrauftrag** (Abs. 7, ORDERS **17117**, once the grounds are gone). Both notices carry the Grund, the voraussichtlichen Unterbrechungs-/Wiederherstellungskosten (Abs. 6) and the avoidance options (Abs. 4) |
| **Mahnsperren** | Every §§ 41f/41g halt is a row in `dunning_locks` with a ground, a citation, a validity period and the operator who set it; lifting is an act with its own reason |
| **§41f Abs. 3 arrears** | `accounts.verzug_ct` — a second ledger-derived cache beside `balance_ct`, deliberately a different number: **open debit residuals** after FIFO clearing (so an unallocated credit cannot net an unpaid invoice out of sight), **less Verzugsschaden** (Mahngebühren and Verzugszinsen arise *because* of the default and must not fee a customer over the 100 EUR floor), **less open Forderungseinwände** |
| **Forderungseinwände (§41f Abs. 3 S. 3–5)** | Amounts that stay out of the Verzug: a claim disputed form- und fristgerecht, a disputed price increase, a claim before a §111b Schlichtung, instalments not yet due. Not halts — they reduce what the threshold is measured against, and the sequence stops by itself when what remains falls below it |
| **Business partner** | `kunden_nr` links accounts to `vertragd.kunden`; `GET /api/v1/business-partners/{kunden_nr}/{accounts,balance}` aggregate cross-MaLo |
| **Refund payout** | Jahresabschluss Erstattung → **pain.001** to the customer IBAN (credit balance carried forward when no IBAN) |
| **Metrics** | `GET /metrics` — Prometheus gauges (open receivables, credit balances, dunning by Mahnstufe, pending SEPA runs) |
| **Worker safety** | Abschlag/dunning workers hold a PostgreSQL advisory lock; all money workers are idempotent (per-run guards) |
| **Jahresabschluss** | Annual settlement (§40 EnWG); idempotent per year via `jahresabschluss_runs`; the settlement is the year's **whole** Kontokorrent movement, never a hand-picked subset of Buchungsarten; recalibrates the monthly Abschlag from supply billing alone. On demand, or from the § 40b Abs. 1 worker (`jahresabschluss_auto_enabled`) |
| **Festschreibung** | `POST /api/v1/periods/{id}/seal` closes and seals a period (GoBD / § 146 AO / § 239 HGB), committing to its closing balances folded by **booking date** |
| **Audit proofs** | Three `O(log n)` questions, verifiable without this service — inclusion, sealed balance, consistency. See [Audit proofs](#audit-proofs) |
| **Offene-Posten (OP-Verwaltung)** | Authoritative open items via recorded **FIFO Zahlungszuordnung** (doubleentry clearing) — every post matches open credits against the oldest open debits, so `GET .../open-items` shows real residuals (§ 252 HGB). `POST .../clear` re-runs matching; `POST /api/v1/clearings/{id}/reset` releases a mis-assignment |
| **Summen- und Saldenliste** | `GET /api/v1/trial-balance` — GL trial balance (§ 238 HGB): gross Soll/Haben turnover + Saldo per account, Σ debits = Σ credits (Kontokorrent leaves aggregated into one Debitoren line) |
| **MCP** | 13 tools at `/mcp` (issuing a reversal is deliberately not one — `list_sepa_collections` is read-only, the reversal is an operator decision) |
| **Metrics** | `accountingd_sepa_collections{status}` and `_open_ct` expose the collection lifecycle: a submitted count that only grows means bank replies are not arriving at all, which looks identical to "everything settled" from the ledger alone |
| **Tests** | pure unit + integration tests + DB-backed scenario tests (`tests/db_scenarios.rs`, `just test-accountingd-db` — idempotency, netting, reconcile, **period seal + backdate rejection**, **Merkle inclusion proof against Postgres**) |
| **Health** | `GET /health/live`, `GET /health/ready` |


## Audit proofs

Three questions, each answered in `O(log n)` and each verifiable by a recipient
who does not have this service:

| Endpoint | Question |
|---|---|
| `GET /api/v1/entries/{id}/proof` | Is this booking in the books? |
| `GET /api/v1/periods/{id}/balance-proof?malo_id=…&lf_mp_id=…` | What did this customer's Kontokorrent close at (§ 147 AO)? |
| `GET /api/v1/entries/consistency-proof?since=<tree_size>` | Has the journal only been appended to since an archived head? |

The balance answer carries the account-binding proof that says whose handle the
Kontokorrent was, and the whole `sealed_balance` bundle verbatim so a recipient
re-verifies without calling back. Where there is nothing to prove it says which
of the two reasons applies — `not_yet_registered` or `no_row` — neither of which
is a balance of zero.

Every root is published with the tree size it belongs to: a proof checked against
a bare root can be replayed against a different tree. `since=0` is refused for
the same reason — every log extends the empty tree, so such a proof verifies
against any root of the right size and reports success from a check that examined
nothing.

**Sealing sets a watermark.** Every date at or before the latest sealed period
end is closed, whether or not a period covers it, so a gap below a seal is not an
opening to book through — and it survives a restart.
`GET /api/v1/periods/seals` lists the chained seals, verifies the chain and
reports `sealed_through`.

## Security

- **OIDC/JWT**: all financial write endpoints require a valid Bearer token; dev mode emits `[WARN]`
- **Inbound HMAC**: `POST /webhook` verifies Standard Webhooks (`webhook-signature`); constant-time comparison
- **SecretString**: `erp_hmac_secret` never appears in logs or debug output

## IBAN validation

Every SEPA mandate PUT validates the IBAN via the ISO 13616 mod-97 checksum **and**
the SWIFT IBAN Registry's per-character BBAN structure for the country. Mod-97 alone
detects an altered character with probability 96/97 and never says which one, so an
`O` typed for a `0` in a German account number passes it about 99 % of the time it is
the only error. Malformed IBANs are rejected at the API boundary with HTTP 422.
The validation logic is covered by unit tests (DE/GB/NL/AT/CH checksums) without a database.

## SEPA pain.008

`POST /api/v1/sepa/run` returns **one pain.008 message** with one `PmtInf` group per
`SequenceType` present — FRST and RCUR live in separate payment-information blocks of
the same file (EPC SDD Core Rulebook §3.8), so a collection run is one bank submission
and one audit row. Each run is stored in `sepa_collection_runs` for audit and ERP
webhook replay, with one `sepa_collection_entries` row per collected mandate: the
attribution key for pain.002 replies (`EndToEndId`), camt bookings (`Btch/PmtInfId`)
and pain.007 reversals.

Those rows are the record of what the bank received, so a **dispatched** run is
frozen: rebuilding one answers `409` rather than replacing the entries a reply
would be matched against, and the XML is only handed back once the archive row
exists. The nightly N-5 scheduler takes an advisory lock (`LOCK_SEPA_N5`) so a
single replica builds the day's batch; the freeze is what still holds if the lock
is lost or the service restarts mid-day.

The XML schema version defaults to the current EPC releases (`pain.008.001.08`,
`pain.001.001.09`) and can be pinned per bank with the optional `pain008_schema` /
`pain001_schema` config keys (e.g. `pain.008.001.02` for the pre-2023 version).
Unknown values fail at startup rather than on a rejected batch.

## Structured postal addresses — 15 November 2026

Version 1.1 of the 2025 SEPA rulebooks, in force since 5 October 2025, ends the
unstructured address on **15 November 2026** (version 1.0 said the 22nd; it moved to
land with that year's Swift Standards MX release). From then a scheme message must
carry `TwnNm` and `Ctry`. It is an *address* deadline, not a message-version one —
`pain.001.001.09` and `pain.008.001.08` have been mandatory since 19 November 2023.

```toml
[creditor_address]
street          = "Musterstraße"
building_number = "12"
post_code       = "10115"
town            = "Berlin"
country         = "DE"
```

The operator's block feeds `Cdtr/PstlAdr` in pain.008/pain.007 and `Dbtr/PstlAdr` in
pain.001 — the same legal entity on both sides. A customer's own address lives on the
mandate (`sepa_mandates.debtor_*`) for direct debits and on the account
(`accounts.addr_*`) for anything accountingd pays out. A half-filled address is a hard
error, not a silent omission: a street with no town and country is exactly the case
the cut-over will surface.

## Emitted events

Every event goes through the transactional outbox — written in the same
transaction as the state change it announces, then drained by a worker with
retry and dead-letter.

| CloudEvent | Emitted when |
|---|---|
| `de.accounting.mahnung.issued` | a Mahnstufe case is opened (auto-dunning **and** manual escalation) |
| `de.accounting.sperrandrohung` | §41f Abs. 1 notice |
| `de.accounting.sperrankuendigung` | §41f Abs. 5 notice |
| `de.accounting.sperrauftrag` | ORDERS 17115 dispatched to the Netzbetreiber |
| `de.accounting.entsperrauftrag` | §41f Abs. 7 — ORDERS 17117, the arrears were settled |
| `de.accounting.abwendung.angeboten` | §41g Abs. 1 S. 2 — an Abwendungsvereinbarung was offered |
| `de.accounting.abwendung.gebrochen` | §41g Abs. 1 S. 11 — an accepted agreement was broken |
| `de.accounting.payment.imported` | a bank credit booked (camt.053, camt.054 or the flat import) |
| `de.accounting.bankruecklast` | a SEPA return booked — a collection that **settled** and was then given back |
| `de.accounting.sepa.collection-rejected` | pain.002 `RJCT` on a collection — the money never moved, so the receivable stays open and the mandate needs attention |
| `de.accounting.sepa.reversal-issued` | a pain.007 gave a settled collection back |
| `de.accounting.payee.verification-mismatch` | Verification of Payee reported anything but a clean match on an outgoing transfer |
| `de.accounting.abschlag.posted` | Abschlagslauf raises the monthly Abschlagsforderung |
| `de.accounting.interest.charged` | Verzugszinsen booked (§288 BGB) |
| `de.accounting.payment.due` | SEPA direct debit due date approaching |
| `de.accounting.jahresabschluss.abgeschlossen` | Annual settlement committed — every outcome |
| `de.accounting.erstattung.faellig` | Jahresabschluss yields a refund (carries the pain.001) |
| `de.accounting.eeg.payout.rejected` | pain.002 RJCT on an EEG payout |

### Delivery guarantees

Most events commit in the same transaction as the state change they announce.
Two are deliberately different:

**Sperrauftrag / Entsperrauftrag** — the order is an ORDERS 17115/17117
dispatched through `makod`; the CloudEvent only announces it. The candidate query
selects on `sperrauftrag_ce_id IS NULL`, so the mark commits **before** the
announcement: a lost announcement logs at `ERROR` and can be replayed, while a
second §41f disconnection order cannot be withdrawn.

**Abschlag** — the CloudEvent id *is* the ledger idempotency key
(`ABSCHLAG-{malo}-{YYYY}-{MM}`), so `outbox::enqueue`'s
`ON CONFLICT (event_id) DO NOTHING` makes the announcement exactly-once per MaLo
and month. The scheduler runs on a 23-hour cycle and therefore passes twice in
some months.

## Configuration

```toml
# accountingd.toml
port                  = 9380
tenant                = "9900357000004"
creditor_iban         = "DE89370400440532013000"
creditor_id           = "DE74ZZZ09999999999"   # SEPA Gläubiger-ID (EPC AT-02)
creditor_name         = "Muster Energie GmbH"
# pain008_schema      = "pain.008.001.02"       # optional; default pain.008.001.08
# pain001_schema      = "pain.001.001.03"       # optional; default pain.001.001.09
# Outbound de.accounting.* CloudEvents. Delivery is durable: each event is
# written to `event_outbox` in the same transaction as the ledger change
# (persist-before-dispatch) and drained by a background worker with retry +
# dead-letter — a crash never drops an event.
erp_webhook_url       = "http://erp:8000/events"
# The §§41f/41g market channel: Phase 3 and 4 are ORDERS 17115/17117.
# Absent → the disconnection sequence does not run.
makod_url             = "http://makod:8080"
makod_api_key         = "env:ACCOUNTINGD_MAKOD_API_KEY"
# §41f Abs. 6 — the voraussichtliche Kosten both notices must state. A Pauschale
# is permitted (Abs. 7 S. 2) provided it stays nachvollziehbar. Default 0, which
# sends a notice claiming the disconnection is free.
sperrkosten_ct        = 4500
entsperrkosten_ct     = 4500
erp_hmac_secret       = "env:ACCOUNTINGD_INBOUND_HMAC_SECRET"
dunning_auto_enabled  = true
dunning_grace_days    = 30

# Cdtr/PstlAdr on pain.008 + pain.007, Dbtr/PstlAdr on pain.001 — one legal
# entity, one block. Optional until the EPC cut-over on 2026-11-15, but not
# fillable halfway: street without town + country is a hard error.
[creditor_address]
street          = "Musterstraße"
building_number = "12"
post_code       = "10115"
town            = "Berlin"
country         = "DE"

[database]
url = "postgresql://accountingd:secret@db:5432/accountingd"
# pool_size = 10   # optional pool tuning (min_connections, acquire/idle/max_lifetime)

[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "accountingd"

[eeg]
sepa_instant   = true
auto_payout    = true
debtor_iban    = "env:LF_BANK_IBAN"
bank_submit_url = "https://banking-adapter.internal/api/v1/pain001"
bank_api_key   = "env:BANK_API_KEY"
```
