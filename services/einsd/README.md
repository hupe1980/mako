# einsd — Einspeiser Registry + EEG/KWKG Settlement

`einsd` manages the full lifecycle of decentralised renewable feed-in plants under
the EEG and CHP plants under the KWKG, from registration through monthly settlement
through Förderdauer expiry.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9180` |
| **Database** | PostgreSQL (einspeiser, eeg_anlagen, settlement_receipts incl. `rechnung_json` + `gutschrift_nummer`, eeg_verguetungssaetze) |
| **Einspeiser (Anlagenbetreiber)** | The party behind the plants, held once. The § 19 UStG election and the payout account are properties of the *person*, so one `PUT` switches the VAT on all of its plants. No Vertrag — § 7 Abs. 1 EEG 2023 forbids conditioning the claim on one |
| **§14 UStG Gutschrift** | Every billable settlement issues the Gutschrift (Gutschriftverfahren — the NB issues the document) as a BO4E `Rechnung` in `settlement_receipts.rechnung_json`, VAT from the operator's declared `einspeiser.ust_status` (Regelbesteuerung 19 % category `S` / §19 Kleinunternehmer 0 % category `E`) |
| **Auth** | OIDC/JWT + Cedar ABAC + HMAC-signed CloudEvents |
| **Validated registration** | `POST /api/v1/anlagen` refuses a plant the settlement could not act on, naming the field. A tender plant may carry its AW in `zuschlagswert_ct` (preferred) or `direktverm_aw_ct`; a Marktprämie with no AW is `PriceMissing` in the engine too |
| **§44b Biogas quota** | 45 % Bemessungsleistung measured against the §3 Nr. 6 hours — the actual hours of the calendar year (**8 784 in a leap year**) less the hours before first generation, not a flat 8 760 |
| **One settle path** | REST, batch, MCP `trigger_settle` and the monthly worker all call `settle::settle_plant`, so the entry point cannot change the amount. They differ only in what they choose to override |
| **Cumulative counters** | `kwk_strom_kwh_gesamt` (§8 KWKG), `biogas_quota_kwh_ytd` (§44b) and `negative_price_qh_gesamt` (§51a) are running totals the settlement both reads and writes, so they are re-read under a `FOR UPDATE` lock on the plant row as the transaction's first statement — the only point serialising *every* settle of one plant. `settlement_period_accruals` holds each period's absolute contribution, so a re-settle applies the difference |
| **§9 Steuerbarkeit** | Staged by capacity: ≥100 kW Fernsteuerbarkeit only, 25–100 kW Fernsteuerbarkeit **or** the 60 % Leistungsbegrenzung, <25 kW the cap alone, Steckersolar <2 kW exempt. Each plant records **which** route it took (`sect9_erfuellung`) |
| **§ 52 Pflichtzahlungen** | Five of the twelve Abs. 1 violations are derived from the plant record in one place (`sect52`) — Nr. 1, 4, 5, 9 and 11 — and priced by the engine (Abs. 2 rate, Abs. 3 reduction, Abs. 5 cap) |
| **Ausfallvergütung** | § 21 Abs. 1 Satz 1 Nr. 3 — the § 53 Abs. 3 **−20 %** on the ordinary rate, the 3-consecutive / 6-per-year Höchstdauern counted from the receipts, and the § 51 Abs. 3 **5 % per calendar day** cut |
| **Plant types** | 18 `erzeugungsart` values: five SOLAR Bauformen, WIND_ONSHORE/OFFSHORE, BIOMASSE/BIOGAS/BIOMETHAN, KLAEGAS/GRUBENGAS/DEPONIEGAS, WASSERKRAFT, GEOTHERMIE, GEZEITEN, KWKG. There is no generic `SOLAR` — the §48 rate depends on the Bauform |
| **Settlement models** | 12, one token each (no aliases): VERGUETUNG, AUSFALLVERGUETUNG (§21 Abs. 1 Nr. 2), MIETERSTROM (§21 Abs. 3), GGV (§42b EnWG), DIREKTVERMARKTUNG (§20), AUSSCHREIBUNG (§22), SONSTIGE_DIREKTVERMARKTUNG (§21a), POST_EEG_SPOT, EIGENVERBRAUCH, KWKG_ZUSCHLAG (§7 KWKG 2023), FLEXIBILITAET (§50b), FLEXIBILITAET_ZUSCHLAG (§50a) |
| **Rate table** | Built-in `eeg_verguetungssaetze`, keyed on `(erzeugungsart, verguetungsform, leistung_min_kwp, billing_start)` — Überschuss and Volleinspeisung differ by the §48 Abs. 2a bonus, so `verguetungsform` is part of the key **and** of the lookup |
| **Repowering** | `POST /api/v1/anlagen/{tr_id}/repowering` — a Vollrepowering is a fresh Inbetriebnahme (§3 Nr. 30), so §25 restarts. §22 is the Ausschreibung provision and governs none of this |
| **Zusammenlegung** | `parent_tr_id` links merged plants. The endpoint evaluates **§24 Abs. 1 in full** — the four cumulative conditions of Satz 1 plus the Sätze 2–5 carve-outs — and refuses a merge the statute does not support with `422`, naming the rule that decided. Ownership is not a criterion ("unabhängig von den Eigentumsverhältnissen") |
| **§§53b–54 AW cuts** | Only the triggering facts are stored (`eeg_regionalnachweise`, `eeg_stromsteuerbefreiungen`, `eeg_sect54_solar_defekte`); every amount but §53c's is statutory. All three cut the anzulegender Wert **before** the settlement formula, because the Marktprämie floors at zero |
| **KWKG Förderdauer** | `kwk_foerderdauer_h` (>2 MW, 30 000 Vollbenutzungsstunden, with the §8 Abs. 4 fifteen-calendar-year backstop in `foerderendedatum`) or `kwk_foerderdauer_years` (≤2 MW) |
| **Förderdauer alerts** | Background worker emits `de.eeg.anlage.foerderung-auslaufend` **once per plant** inside the 180-day window (`foerderung_alert_sent_at`); a repowering re-arms it |
| **§ 51 Negativpreisregel** | Keyed on the **Inbetriebnahmedatum**, not the law year — the Solarspitzengesetz rewrote § 51 inside the EEG 2023 range. `NegativpreisRegime::fuer_inbetriebnahme` gives the run-length threshold and the exemption; Pilotwindenergieanlagen are exempt throughout |
| **§ 51 auto-derivation** | A settle without explicit values fetches the plant's ¼h feed-in from edmd and overlays the spot store over the **Europe/Berlin** billing month. A § 60 Abs. 2 MsbG gate skips the reduction below 95 % coverage; a genuine zero is recorded as a zero, not as "unknown" |
| **§51a Förderende-Verlängerung** | Only where §51 actually bit — and, before the Solarspitzengesetz, only for ausschreibungspflichtige Anlagen. Raw lost quarter-hours accrue in `negative_price_qh_gesamt`; `effektives_foerderende` derives the extended end at settle time (solar: the Abs. 2 Volllastviertelstunden table; others: whole calendar days) — the stored statutory `foerderendedatum` is untouched |
| **§36h Abs. 2 Standortgüte re-eval** | `POST /api/v1/anlagen/{tr_id}/wind-reevaluation` records the Gütefaktor re-evaluated from operating year 6/11/16 (`wind_guetefaktor_reevaluations`); settlement selects the effective Korrekturfaktor per period and flags `reconciliation_required` on a >2 pp deviation (§147 AO correction) |
| **edmd auto-fetch** | Automatically fetches `arbeitsmenge_kwh` and the §51 ¼h feed-in from `edmd` when not supplied (authenticated with `edmd_api_key`, registered in edmd `[[oidc.service_keys]]`) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Settlement formulas

| Model | Formula |
|---|---|
| VERGUETUNG | `kwh × rate_ct / 100` |
| AUSFALLVERGUETUNG | `kwh × (rate_ct × 0.8) / 100` — §53 Abs. 3, then §51 Abs. 3 if unreported |
| MIETERSTROM / GGV | `kwh × (rate_ct + mieter_zuschlag_ct) / 100` |
| DIREKTVERMARKTUNG | `max(0, AW_ct − Marktwert_ct) × kwh / 100` — floored at zero (no clawback) |
| AUSSCHREIBUNG | Same formula with the BNetzA tender `AW_ct` |
| SONSTIGE_DIREKTVERMARKTUNG | EUR 0 — the plant sells on the open market |
| POST_EEG_SPOT | `kwh × Marktwert_ct / 100` |
| EIGENVERBRAUCH | EUR 0 |
| KWKG_ZUSCHLAG | `kwh × kwk_ct / 100` (paid on top of the electricity market price) |
| FLEXIBILITAET | `kwh × (rate_ct + flex_praemie_ct) / 100` |
| FLEXIBILITAET_ZUSCHLAG | `kw × rate_eur_per_kw / 12` — a capacity payment, not per kWh |

All arithmetic uses `rust_decimal::Decimal` — never `f64`. Settlement formulas are covered
by unit tests without a database:

```bash
cargo test -p einsd --test settlement_tests
```

## The register answers `processd`

`GET /api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform` is the one read the NB
Anmeldung engine makes here. `E_0622` Prüfschritte 400–830 choose an Anmeldung
erzeugender Marktlokation's Vorlauffrist from the **bestehende** Veräußerungsform,
which is register data and not on the wire — and UTILMD `SG10 CCI+Z22` code `Z90`
covers both the uneingeschränkte Einspeisevergütung (§ 21 Abs. 1 Nr. 1 EEG 2023)
and the Ausfallvergütung (Nr. 2), whose Fristen differ by a month versus five
Werktage. So the response carries both the code and the flag:

| `settlement_model` | `veraeusserungsform` | `ausfallverguetung` |
|---|---|---|
| `VERGUETUNG` | `Z90` | `false` |
| `AUSFALLVERGUETUNG` | `Z90` | **`true`** |
| `DIREKTVERMARKTUNG`, `AUSSCHREIBUNG` | `Z91` | `false` |
| `SONSTIGE_DIREKTVERMARKTUNG` | `Z92` | `false` |
| `KWKG_ZUSCHLAG` | `Z94` | `false` |

`AUSSCHREIBUNG` is still the Marktprämie — § 22 EEG 2023 sets the anzulegender
Wert competitively, not the Veräußerungsform. Mieterstrom, GGV, Eigenverbrauch,
Post-EEG and the Flexibilitäts-models are settlement models with no `CCI+Z22`
code and answer `404`, as does a MaLo the register does not hold: neither is
evidence of a „Nicht-EEG-/-KWKG"-Marktlokation, so `processd` escalates.

## MCP server — `/mcp` (19 tools, 6 prompts)

`einsd` exposes a Streamable HTTP MCP server at `/mcp`. All tools are read-only
unless they explicitly trigger a side effect (e.g. `trigger_settle`).

Money and energy cross this surface as **exact decimals**, accepted as a JSON string
(`"8.11"`) or a number parsed from its own decimal text — never through `f64`. A rate
that ends up on a legally binding Gutschrift must not have passed through binary
floating point, and 0,1 ct/kWh has no exact `f64`.

| Tool | Purpose |
|---|---|
| `list_plants` | List registered plants with optional filters |
| `get_plant` | Full plant details including settlement model and Förderdauer |
| `list_expiring` | Plants with Förderdauer expiry within N days |
| `list_settlements` | Recent settlement receipts for a plant |
| `list_unsettled_plants` | Plants with no receipt for the current month |
| `lookup_verguetungssatz` | Statutory rate for technology / commissioning year |
| `lookup_statutory_rate` | Equivalent lookup — technology + year → rate |
| `trigger_settle` | Trigger one-off settlement for a plant + month |
| `get_epex_monthly_price` | EPEX Day-Ahead monthly average for a period |
| `import_epex_monthly_price` | Import a new monthly average price |
| `get_compliance_status` | Every §52 Abs. 1 violation `einsd` derives, priced with the engine's Abs. 2/3/5 rules |
| `list_plants_without_mastr` | Plants not registered in MaStR (§52 Abs. 1 Nr. 11); a pre-2023 plant owes no Pflichtzahlung and is excluded from the total |
| `check_direktvermarktung_compliance` | Plants >100 kW on an Einspeisevergütung model — §52 Abs. 1 Nr. 4; the settlement charges it |
| `check_sect44b_quota` | **§44b EEG 2023**: annual biogas cap (leistung × 0.45 × the §3 Nr. 6 hours of *that* year — 8 784 in a leap year, less the hours before first generation), YTD, remaining, 75 %/90 % alert |
| `explain_settlement` | The full position trace behind one month's EUR amount — every `SettlePosition` with its `legal_basis`, kWh and rate. What an operator dispute or a BNetzA inspection actually asks for |
| `get_aw_reduktionen` | Why the anzulegender Wert is cut on a date: every active §53b / §53c / §54 reduction with its statutory amount. These cuts shrink the payment without touching the Einspeisemenge or the rate table, so they are the first thing to check when a Gutschrift is smaller than expected |
| `get_settlement_state_history` | The § 147 AO / GoBD trail of `settlement_state` transitions with the period that caused each |
| `get_jahresmarktwert` | The stored §20 Abs. 2 technology-specific monthly Marktwert; `DEFAULT` reads the generic fallback row |
| `import_jahresmarktwert` | Store the ÜNB Marktwert (netztransparenz.de). Takes precedence over the generic EPEX average for Direktvermarktung / Ausschreibung |

## Testing

| Suite | Needs a database | Covers |
|---|---|---|
| `eeg-billing` unit tests | no | settlement arithmetic, every §-rule |
| `tests/schema_code_guard.rs` | no | the service's SQL against its own schema |
| `tests/settlement_integration.rs` | yes | the real router, real SQL, real policy |

```bash
just test-einsd-db      # throwaway PostgreSQL, runs the #[ignore]d suite
```

The guards are text-level: every `ON CONFLICT` on `settlement_receipts` repeats
the partial-index predicate, no query names a column the schema does not define,
and a `settlement_state` change records the transition it came from. The
integration suite proves the same rules against a real PostgreSQL, driving the
router through its actual layers.

## Jahresabrechnung

```
POST /api/v1/anlagen/{tr_id}/jahresabrechnung/{year}
```

Reconciles the year's monthly settlements into one statement. It is **derived
from the stored receipts, not recomputed** — the monthly runs are what created
the payment obligation, so a statement that recalculated from scratch could
disagree with what was actually paid.

Each month contributes its **latest** receipt: the correction where one exists,
the original otherwise. A correction is a separate row that neither adds to its
month nor replaces the original in place, so the statement takes one row per
month and never sums both.

| Field | Meaning |
|---|---|
| `einspeisemenge_kwh` / `settlement_eur` | totals over each month's latest receipt |
| `pflichtzahlung_eur` | §52 EEG 2023 — a separate claim, never netted into the Vergütung |
| `months_settled` / `missing_months` | of the months the plant is **entitled to**, which still carry no receipt |
| `verlaengerungsanspruch_qh` | §51a quarter-hours accrued toward the Vergütungszeitraum |
| `correction_count` | § 147 AO / GoBD corrections issued in the year |
| `status` | `vorlaeufig` until every entitled month is settled, then `endgueltig` |

`missing_months` is bounded by the commissioning date and the Förderende — a plant
commissioned in June is not missing January, so its first and last years can reach
`endgueltig` rather than demanding all twelve.

Re-running replaces the statement, so it can be produced provisionally during the
year and finalised once the last entitled month is settled.

## Authorization

Every REST route requires an OIDC-verified token and a Cedar decision; the policy
is [`policies/einsd.cedar`](policies/einsd.cedar).

| Action | Routes | Who |
|---|---|---|
| `read-anlage` / `read-settlement` / `read-marktdaten` | all `GET` | any caller in the tenant |
| `write-anlage` | plant `POST`/`PUT`/`DELETE` | `NB`, `LF`, `UENB` |
| `run-settlement` | `.../settle/...`, `/api/v1/settle/...` | `NB`, `LF`, `UENB` |
| `manage-lifecycle` | repowering (§22), zusammenlegen (§24), MaStR, §21b switch | `NB`, `LF`, `UENB` |
| `correct-settlement` | `.../correction` (§ 147 AO / GoBD) | `NB`, `UENB` |
| `write-marktdaten` | EPEX / Jahresmarktwert `PUT` | `NB`, `LF`, `UENB` |

Writes are role-gated because settling a plant creates a payment obligation to
the Anlagenbetreiber. Corrections are held to a narrower set again: they
supersede a settlement already sent and re-open a closed accounting period.

The service **refuses to start without an `[oidc]` section** unless
`allow_insecure_no_auth = true` is set explicitly. Cedar is default-deny, so
cross-tenant access needs no forbid rule.

## Configuration

```toml
# einsd.toml
port           = 9180
tenant         = "9900357000004"
edmd_url       = "http://edmd:8380"
edmd_api_key   = "env:EINSD_EDMD_SERVICE_KEY"  # opaque Bearer; register in edmd [[oidc.service_keys]]

# The auto-settle worker sweeps this many months back on each run, newest first,
# so a period whose ÜNB Marktwert arrived late is still picked up. Default 3.
auto_settle_catchup_months = 3
auto_settle_from_day       = 7   # wait for the ÜNB Marktwert window

# Outbound ERP CloudEvents, signed with HMAC-SHA256 (webhook-signature).
# Delivery is durable: each event is written to `event_outbox` in the same
# transaction as the settlement (persist-before-dispatch) and drained by a
# background worker with retry + dead-letter — a crash never drops an event.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:EINSD_ERP_HMAC_SECRET"

[database]
url = "postgresql://einsd:secret@db:5432/einsd"  # or "env:DATABASE_URL"

[oidc]                      # required unless allow_insecure_no_auth = true
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-einsd"
```
