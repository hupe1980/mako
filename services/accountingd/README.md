# accountingd — Massenkontokorrent / Customer Account Ledger

`accountingd` is the **FI-CA equivalent** for the mako retail billing stack. Without it,
`billingd` invoices are fire-and-forget — no Offene-Posten tracking, no automated dunning,
no SEPA collection.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9380` |
| **Database** | PostgreSQL (sqlx 0.8) — customer/SEPA satellites in `public`, the ledger in the `doubleentry` schema of the same database |
| **Auth** | OIDC/JWT on write endpoints + inbound webhook HMAC-SHA256 |
| **Ledger** | The [`doubleentry`](https://github.com/hupe1980/doubleentry) crate: an **immutable, tamper-evident double-entry engine** — balanced by construction, an append-only Merkle log with inclusion/consistency proofs, period seals (GoBD/§ 146 AO), open-item clearing, and store-level idempotency. accountingd owns the chart of accounts (`ledger::Chart`) and the `entry_type → postings` mapping; every money movement flows through `pg::post_entry` → `ledger.post`. |
| **Double-entry** | Each Buchungsart is one balanced entry: the per-MaLo **Kontokorrent** (SKR 1400 subledger, an Asset leaf whose signed net *is* the balance) against a GL contra leaf (Bank 1200 / Erlöse 4000 / Mahnerlöse 4003 / EEG-Aufwand / Erstattungen). Soll = Haben is enforced in-engine **and** by a deferred DB trigger; §238 HGB. `accounts.balance_ct` is a ledger-derived read cache (set absolutely from the ledger net — never incremented, so it cannot drift) backing the portfolio SUM queries. |
| **Vorauszahlung** | `PUT/GET /api/v1/accounts/{malo_id}/vorauszahlung` — typed `rubo4e::current::Vorauszahlung` (§40 Abs. 1 EnWG) |
| **Aging analysis** | `GET /api/v1/aging` — receivables by 0–30d / 31–60d / 61–90d / >90d |
| **Verzugszinsen** | `GET/POST /api/v1/accounts/{malo_id}/interest-charges` — §288 BGB B2C/B2B |
| **Payment plans** | `GET/POST /api/v1/accounts/{malo_id}/payment-plans` — Zahlungsvereinbarung |
| **SEPA mandates** | IBAN validated via **ISO 13616 mod-97 + the SWIFT registry's per-country BBAN structure** on PUT (mod-97 alone misses an `O` typed for a `0` about 99 % of the time it is the only error); `sepa_mandates` table (UNIQUE per tenant) |
| **SEPA scheduler** | N-5 background worker generates **one pain.008 message per collection date** (one `PmtInf` group per SequenceType, mandatory Gläubiger-ID); persisted in `sepa_collection_runs` **with one `sepa_collection_entries` row per collected mandate** |
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
| **Abschlag model** | ABSCHLAG booked as advance-payment **credit** (negative); full-cost Jahresrechnung as debit — the balance nets to the Nachzahlung/Erstattung |
| **Mahnwesen** | Mahnstufe 1→2→3; auto-dunning worker (advisory-locked, opt-in) |
| **Sperr-Sequenz (§§41f/41g EnWG)** | Mahnstufe-3 arrears ≥ `sperrung_threshold_ct` (default 100 EUR) run the three-step disconnection sequence: **Sperrandrohung** (4 Wochen) → **Sperrankündigung** (8 Werktage im Voraus) → **Sperrauftrag** → `POST sperrd /api/v1/sperr-orders`. Each step is idempotent; the first two emit signed CloudEvents via the outbox (persist-before-dispatch). Halted by an accepted **Abwendungsvereinbarung** (§41g) or an **Unverhältnismäßigkeit/Schutzbedürftigkeit** flag (§41f Abs. 1/2). |
| **Business partner** | `kunden_nr` links accounts to `vertragd.kunden`; `GET /api/v1/business-partners/{kunden_nr}/{accounts,balance}` aggregate cross-MaLo |
| **Refund payout** | Jahresabschluss Erstattung → **pain.001** to the customer IBAN (credit balance carried forward when no IBAN) |
| **Metrics** | `GET /metrics` — Prometheus gauges (open receivables, credit balances, dunning by Mahnstufe, pending SEPA runs) |
| **Worker safety** | Abschlag/dunning workers hold a PostgreSQL advisory lock; all money workers are idempotent (per-run guards) |
| **Jahresabschluss** | Annual settlement (§40 EnWG); idempotent per year via `jahresabschluss_runs`; recalibrates the monthly Abschlag |
| **Festschreibung** | `POST /api/v1/periods/{id}/seal` closes + seals a period (GoBD / § 146 AO / § 239 HGB) — after which a backdated booking into it is refused; `GET /api/v1/periods/seals` lists the chained seals and verifies the chain |
| **Audit proofs** | `GET /api/v1/entries/{id}/proof` returns an `O(log n)` Merkle inclusion proof (content hash + tree head) — an auditor can verify an entry is committed without this service |
| **Offene-Posten (OP-Verwaltung)** | Authoritative open items via recorded **FIFO Zahlungszuordnung** (doubleentry clearing) — every post matches open credits against the oldest open debits, so `GET .../open-items` shows real residuals (§ 252 HGB). `POST .../clear` re-runs matching; `POST /api/v1/clearings/{id}/reset` releases a mis-assignment |
| **Summen- und Saldenliste** | `GET /api/v1/trial-balance` — GL trial balance (§ 238 HGB): gross Soll/Haben turnover + Saldo per account, Σ debits = Σ credits (Kontokorrent leaves aggregated into one Debitoren line) |
| **MCP** | 13 tools at `/mcp` (issuing a reversal is deliberately not one — `list_sepa_collections` is read-only, the reversal is an operator decision) |
| **Metrics** | `accountingd_sepa_collections{status}` and `_open_ct` expose the collection lifecycle: a submitted count that only grows means bank replies are not arriving at all, which looks identical to "everything settled" from the ledger alone |
| **Tests** | pure unit + integration tests + DB-backed scenario tests (`tests/db_scenarios.rs`, `just test-accountingd-db` — idempotency, netting, reconcile, **period seal + backdate rejection**, **Merkle inclusion proof against Postgres**) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Security

- **OIDC/JWT**: all financial write endpoints require a valid Bearer token; dev mode emits `[WARN]`
- **Inbound HMAC**: `POST /webhook` verifies `X-Mako-Signature: sha256=...`; constant-time comparison
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
| `de.accounting.sperrauftrag` | Sperrauftrag handed to `sperrd` |
| `de.accounting.payment.imported` | a bank credit booked (camt.053, camt.054 or the flat import) |
| `de.accounting.bankruecklast` | a SEPA return booked — a collection that **settled** and was then given back |
| `de.accounting.sepa.collection-rejected` | pain.002 `RJCT` on a collection — the money never moved, so the receivable stays open and the mandate needs attention |
| `de.accounting.sepa.reversal-issued` | a pain.007 gave a settled collection back |
| `de.accounting.payee.verification-mismatch` | Verification of Payee reported anything but a clean match on an outgoing transfer |
| `de.accounting.abschlag.posted` | Abschlagslauf posts the monthly advance payment |
| `de.accounting.interest.charged` | Verzugszinsen booked (§288 BGB) |
| `de.accounting.payment.due` | SEPA direct debit due date approaching |
| `de.accounting.erstattung.faellig` | Jahresabschluss yields a refund |
| `de.accounting.eeg.payout.rejected` | pain.002 RJCT on an EEG payout |

### Delivery guarantees

Most events commit in the same transaction as the state change they announce.
Two are deliberately different:

**Sperrauftrag** — the order is a `POST sperrd /api/v1/sperr-orders`; the
CloudEvent only announces it. `sperrd` does not deduplicate orders and the
candidate query selects on `sperrauftrag_ce_id IS NULL`, so the mark commits
**before** the announcement. The asymmetry is deliberate: a lost announcement
logs at `ERROR` with the case id and can be replayed, while a second §41f
disconnection order cannot be withdrawn.

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
