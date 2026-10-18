# einsd

`einsd` (`:9180`) is the Einspeiser registry and EEG/KWKG settlement service of
the Netzbetreiber. It holds the Anlagenbetreiber and their plants, settles each
plant monthly through the `eeg-billing` engine and issues the result as a § 14
UStG Gutschrift (BO4E `Rechnung`), and tracks Förderdauer, § 52 Pflichtzahlungen
and the §§ 53b–54 reductions of the anzulegender Wert.

## Run it

```bash
just dev einsd         # dev/config/einsd.toml against the dev PostgreSQL
```

## At a glance

| | |
|---|---|
| **Database** | PostgreSQL — `einspeiser`, `eeg_anlagen`, `settlement_receipts`, `eeg_pflichtverstoesse`, `marktwert_preise`, `eeg_verguetungssaetze` |
| **Auth** | OIDC + Cedar [`policies/einsd.cedar`](policies/einsd.cedar): reads for the tenant, writes and settlement for `NB`/`LF`/`UENB`, corrections for `NB`/`UENB`. Refuses to start without `[oidc]` unless `allow_insecure_no_auth` |
| **Settlement** | 12 models (VERGUETUNG … FLEXIBILITAET_ZUSCHLAG), 18 `erzeugungsart` values; REST, batch, MCP and the monthly worker all call `settle::settle_plant`. `Decimal` only |
| **Inputs** | `arbeitsmenge_kwh` and the § 51 ¼h feed-in fetched from `edmd` when not supplied; Marktwert series per Anlage 1 Nr. 2 — a miss is `PriceMissing`, never a substitute |
| **Events** | `de.eeg.*` through the transactional outbox to `erp_webhook_url`, HMAC-signed |

## Routes

| Area | Routes |
|---|---|
| Register | `/api/v1/einspeiser`, `/api/v1/anlagen` (+ `/{tr_id}`, `/foerderung-auslaufend`), `/repowering`, `/zusammenlegen` (§ 24), `/mastr-registrierung`, `/switch-veraeusserungsform`, `/wind-reevaluation` |
| Settlement | `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}`, `POST /api/v1/settle/{year}/{month}` (batch), `/settlements` (+ `/{year}/{month}/correction`), `POST /anlagen/{tr_id}/jahresabrechnung/{year}` |
| Pflichten & AW cuts | `/anlagen/{tr_id}/pflichtverstoesse` (+ `/{typ}/behoben`), `/aw-reduktionen` (Regionalnachweis, Stromsteuerbefreiung, § 54 Defekt) |
| Market data | `/api/v1/marktwert/{year}/{art}/{erzeugungsart}`, `/marktwert/{year}/nachbewertung`, `/epex-monthly/{year}/{month}`, `PUT /epex-spot`, `POST /verguetungssatz-lookup` |
| For `processd` | `GET /api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform` — the `CCI+Z22` code plus an `ausfallverguetung` flag |

## MCP server — `/mcp` (20 tools, 6 prompts)

Seventeen tools read; three write — `trigger_settle`, `import_marktwert` and
`import_epex_monthly_price`. Money crosses the surface as exact decimals.

## Configuration

Top-level: `tenant`, `edmd_url` / `edmd_api_key`,
`auto_settle_catchup_months`, `auto_settle_from_day`, `jahresmarktwert_url`,
`erp_webhook_url` / `erp_hmac_secret`. Tables: `[http]` (`addr`), `[database]`, `[oidc]`,
`[beihilfevorbehalt]` (recorded state-aid approvals), `[policy]`
(`beihilfe_politik = "verweigern"`, `"unter_vorbehalt"` or `"halten"` for an
uncovered payment from 01.01.2027; `halten` stores a `gehalten` receipt with no
Gutschrift and no payout until an approval releases it). The policy report is
logged at startup, served at `GET /api/v1/policy` and by the MCP tool
`get_policy`. DB suite: `just test-einsd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/einsd/)
carries every statutory rule with its Fundstelle, the settlement formulas and
the full configuration reference.
