# einsd

`einsd` is the Netzbetreiber's Einspeiser registry and EEG/KWKG settlement
service. It holds the Anlagenbetreiber and their plants, settles each plant
monthly through the `eeg-billing` engine and issues the result as a § 14 UStG
Gutschrift (BO4E `Rechnung`), and tracks Förderdauer, § 52 Pflichtzahlungen and
the §§ 53b–54 reductions of the anzulegender Wert. A missing price or feed-in is
a refusal, never a substitute.

| | |
|---|---|
| **Port** | 9180 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — `einspeiser`, `eeg_anlagen`, `settlement_receipts`, `marktwert_preise`, `enfg_ledger`, … |
| **MCP** | 20 tools, 6 prompts at `/mcp` |
| **Typed reads** | six shared with their REST `GET`s, pinned in `mcp.snapshot.json` and `openapi.json` |
| **Emits** | `de.mako.eeg.verguetung.berechnet`, `de.mako.eeg.marktpraemie.berechnet`, `de.mako.eeg.veraeusserungsform.gewechselt`, `de.mako.eeg.anlage.*` |
| **Consumes** | none |
| **Depends on** | `edmd` (feed-in), `accountingd` and `marktd` (EnFG ledger) |

## Run

```bash
just dev    # the whole estate, einsd on deploy/demo/einsd.toml, seeded
EINSD_CONFIG=deploy/demo/einsd.toml cargo run -p einsd -- --check-config
```

`just test-einsd-db` runs the PostgreSQL suite.

## API

| Route | Purpose |
|---|---|
| `/api/v1/einspeiser` · `/api/v1/anlagen` | the register: Anlagenbetreiber and plants |
| `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}` | settle one plant's month |
| `POST /api/v1/settle/{year}/{month}` | settle every plant's month |
| `/api/v1/anlagen/{tr_id}/settlements` (+ `/{year}/{month}/correction`) | receipts and corrections |
| `/api/v1/anlagen/{tr_id}/pflichtverstoesse` · `/aw-reduktionen` | § 52 Pflichtverstöße, §§ 53b–54 reductions |
| `/api/v1/marktwert/…` · `/api/v1/epex-monthly/…` | market-value series |
| `/api/v1/enfg/ledger/{year}/{month}` · `/enfg/mitteilung/{year}/{month}` | EnFG Belastungsausgleich |
| `GET /api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform` | the form in force, for `processd` |
| `GET /api/v1/anlagen/ohne-mastr` · `/anlagen/unabgerechnet/{year}/{month}` · `/settlements/{year}/{month}/erklaerung` | the reads the Anlage page needs, each also an MCP tool answering the same type |
| `GET /api/v1/openapi.json` | the OpenAPI 3.1 document of every `/api/v1` route, committed as `openapi.json` |

The full list with each Cedar action is the guide's
[Endpoints](https://hupe1980.github.io/mako/docs/services/einsd/#endpoints).

## Configuration

`tenant`; `[database]`, `[oidc]`; `edmd_url`, `accountingd_url`, `marktd_url`
with their `_api_key`s; `erp_webhook_url` / `erp_hmac_secret`;
`[beihilfevorbehalt]` for recorded state-aid approvals. `[policy]` holds
[`beihilfe_politik`](https://hupe1980.github.io/mako/docs/services/einsd/#policy-beihilfe-politik)
and [`sect52_aufrechnung`](https://hupe1980.github.io/mako/docs/services/einsd/#policy-sect52-aufrechnung).

The [operator guide](https://hupe1980.github.io/mako/docs/services/einsd/)
carries every statutory rule with its Fundstelle, the settlement formulas and
the full configuration reference.
