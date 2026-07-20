# einsd — Einspeiser Registry + EEG/KWKG Settlement

`einsd` manages the full lifecycle of decentralised renewable feed-in plants under
the EEG and CHP plants under the KWKG, from registration through monthly settlement
through Förderdauer expiry.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9180` |
| **Database** | PostgreSQL (eeg_anlagen, settlement_receipts, eeg_verguetungssaetze) |
| **Auth** | OIDC/JWT + Cedar ABAC + HMAC-signed CloudEvents |
| **Plant types** | 19 `erzeugungsart` values: SOLAR variants, WIND_ONSHORE/OFFSHORE, BIOMASSE/BIOGAS/BIOMETHANE, KLAEGAS/GRUBENGAS/DEPONIEGAS, WASSERKRAFT, GEOTHERMIE, GEZEITEN, KWKG |
| **Settlement models** | 9: VERGUETUNG, MIETERSTROM (§38a), DIREKTVERMARKTUNG (§20 Gleitende Marktprämie), AUSSCHREIBUNG, POST_EEG_SPOT, EIGENVERBRAUCH, KWKG_ZUSCHLAG (§7 KWKG 2023), FLEXIBILITAET (§50), GGV (§42b Solarpaket I) |
| **Rate table** | Built-in `eeg_verguetungssaetze` — Solar 2000–2024, Wind onshore/offshore, Biomasse/Biogas, Klärgas/Grubengas/Deponiegas, Wasserkraft, KWKG 2023, Geothermie/Gezeiten |
| **Repowering** | `POST /api/v1/anlagen/{tr_id}/repowering` — resets 20-year Förderdauer (§22 EEG 2023) |
| **Zusammenlegung** | `parent_tr_id` links merged plants (§24 EEG 2023) |
| **KWKG Förderdauer** | `kwk_foerderdauer_h` (>2 MW, 30,000 h) or `kwk_foerderdauer_years` (≤2 MW) |
| **Förderdauer alerts** | Background worker emits `de.eeg.anlage.foerderung_auslaufend` 180 days before expiry |
| **edmd auto-fetch** | Automatically fetches `arbeitsmenge_kwh` from `edmd` when not supplied |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Settlement formulas

| Model | Formula |
|---|---|
| VERGUETUNG | `kwh × rate_ct / 100` |
| MIETERSTROM | `kwh × (rate_ct + mieter_zuschlag_ct) / 100` |
| DIREKTVERMARKTUNG | `max(0, AW_ct − EPEX_avg_ct) × kwh / 100` — clamped at zero (no clawback) |
| AUSSCHREIBUNG | Same formula with BNetzA tender `AW_ct` |
| POST_EEG_SPOT | `kwh × EPEX_monthly_avg_ct / 100` |
| KWKG_ZUSCHLAG | `kwh × kwk_ct / 100` (paid on top of electricity market price) |
| FLEXIBILITAET | `kwh × (rate_ct + flex_praemie_ct) / 100` |

All arithmetic uses `rust_decimal::Decimal` — never `f64`. Settlement formulas are covered
by **102 unit tests** without a database:

```bash
cargo test -p einsd --test settlement_tests
```

## MCP server — `/mcp` (18 tools, 6 prompts)

`einsd` exposes a Streamable HTTP MCP server at `/mcp`. All tools are read-only
unless they explicitly trigger a side effect (e.g. `trigger_settle`).

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
| `get_compliance_status` | §52 violations, MaStR status, Direktvermarktung flag |
| `list_plants_without_mastr` | Plants not yet registered in MaStR (§52 §11 EEG 2023) |
| `check_direktvermarktung_compliance` | **§3 Nr. 1 + §20 EEG 2023**: plants >100 kW on non-market scheme — §52 Abs. 2 Nr. 4 violation risk |
| `check_sect44b_quota` | **§44b EEG 2023**: annual biogas cap (leistung × 0.45 × 8760 kWh), YTD, remaining, 75 %/90 % alert |

## Testing

| Suite | Needs a database | Covers |
|---|---|---|
| `eeg-billing` unit tests (102) | no | settlement arithmetic, every §-rule |
| `tests/schema_code_guard.rs` (5) | no | the service's SQL against its own schema |
| `tests/settlement_integration.rs` (12) | yes | the real router, real SQL, real policy |

```bash
just test-einsd-db      # throwaway PostgreSQL, runs the #[ignore]d suite
```

The two suites answer different questions. The guards are text-level: they assert
that every `ON CONFLICT` on `settlement_receipts` repeats the partial-index
predicate, that no query names a column the schema does not define, and that a
`settlement_state` change records the transition it came from. The integration
suite proves the same rules against a real PostgreSQL and drives the router
through its actual layers.

Both exist because the arithmetic was never where the defects were. A query
naming a derived value as if it were a column, an upsert that cannot match a
partial index, an audit field accepted and then dropped — none of those are
reachable from a pure test, and each of them shipped.

## Jahresabrechnung

```
POST /api/v1/anlagen/{tr_id}/jahresabrechnung/{year}
```

Reconciles the year's monthly settlements into one statement. It is **derived
from the stored receipts, not recomputed** — the monthly runs are what created
the payment obligation, so a statement that recalculated from scratch could
disagree with what was actually paid.

| Field | Meaning |
|---|---|
| `einspeisemenge_kwh` / `settlement_eur` | totals over the year's receipts |
| `pflichtzahlung_eur` | §52 EEG 2023 — a separate claim, never netted into the Vergütung |
| `months_settled` / `missing_months` | an incomplete year names its gaps |
| `verlaengerungsanspruch_qh` | §51a quarter-hours accrued toward the Vergütungszeitraum |
| `correction_count` | §22 MessZV corrections issued in the year |
| `status` | `vorlaeufig` until all twelve months are settled, then `endgueltig` |

Two things it deliberately does not do. It never presents a partial sum as the
year: eleven settled months yield `vorlaeufig` and a list of what is missing,
because the total otherwise looks entirely plausible. And a correction does not
add to its month — the partial unique index keeps one non-correction receipt per
period, so counting the correction too would double that month.

Re-running replaces the statement, so it can be produced provisionally during
the year and finalised once December is settled.

## Authorization

Every REST route requires an OIDC-verified token and a Cedar decision; the policy
is [`policies/einsd.cedar`](policies/einsd.cedar).

| Action | Routes | Who |
|---|---|---|
| `read-anlage` / `read-settlement` / `read-marktdaten` | all `GET` | any caller in the tenant |
| `write-anlage` | plant `POST`/`PUT`/`DELETE` | `NB`, `LF`, `UENB` |
| `run-settlement` | `.../settle/...`, `/api/v1/settle/...` | `NB`, `LF`, `UENB` |
| `manage-lifecycle` | repowering (§22), zusammenlegen (§24), MaStR, §21b switch | `NB`, `LF`, `UENB` |
| `correct-settlement` | `.../correction` (§22 MessZV) | `NB`, `UENB` |
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
database_url   = "postgresql://einsd:secret@db:5432/einsd"
port           = 9180
tenant         = "9900357000004"

[oidc]                      # required unless allow_insecure_no_auth = true
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-einsd"

edmd_url       = "http://edmd:8380"

# Outbound ERP CloudEvents, signed with HMAC-SHA256 (X-Mako-Signature).
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:EINSD_ERP_HMAC_SECRET"
```
