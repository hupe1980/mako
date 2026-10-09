# mabis-syncd

`mabis-syncd` (`:8880`) is the ÜNB/NB filer of MaBiS Summenzeitreihen. It
aggregates per-MaLo Lastgänge from `edmd` into one Bilanzierungsgebiets-
Summenzeitreihe per territory on the Berlin-local 15-minute grid and files each
with the BIKO as MSCONS PID 13003 through `makod` (BK6-24-174 Anlage 3). Strom
only — Gas balances through GaBi Gas.

## Run it

```bash
just dev mabis-syncd   # dev/config/mabis-syncd.toml against the dev PostgreSQL
```

## How a run behaves

- **One filing per territory**, recorded in `submission_series` with its own
  `message_ref` (from `makod`'s `202`) or its failure reason; `submission_runs`
  keeps the aggregate.
- **Refuse rather than under-report.** A run fails when a discovered MaLo cannot
  be aggregated, a territory's grid still has empty slots, a territory has no
  MaBiS-Zählpunkt, or `edmd` reports its MaLo list `truncated`.
- **Acked is final.** A retry skips territories the BIKO already accepted for
  the Bilanzierungsmonat; a correction (`corrects_run_id`) re-files them under a
  higher version.
- **The BIKO assigns the Datenstatus** (inbound via IFTSTA); this service records
  it and never derives one. The phase follows the Werktag calendar of § 3.10
  Tabelle 2 (BG-SZR rows):

| Window | Werktage after period end | Datenstatus of a new version |
|---|---|---|
| Erstaufschlag (BKA) | 1.–10. WT | Abrechnungsdaten directly |
| Clearing (BKA) | 11.–30. WT | Prüfdaten, promoted by a positive Prüfmitteilung |
| KBKA | 31. WT – end of month 7 | Prüfdaten |

The scheduler fires at `run_hour_utc` on `erstaufschlag_werktag`, the last
Werktag of the Erstaufschlag window.

## Routes

| Method | Path | Cedar action |
|---|---|---|
| `POST` | `/api/v1/sync` | `trigger-mabis-run` (`NB`, `UENB`) — refuses a period with a live run |
| `GET` | `/api/v1/runs` · `/runs/{id}` | `read-mabis-run` |
| `PUT` | `/api/v1/runs/{id}/retry` | `trigger-mabis-run` |
| `POST` | `/api/v1/datenstatus` · `/pruefmitteilung` | `record-biko-response` |
| `GET` | `/api/v1/korrekturbedarf` | `read-mabis-run` — open § 9.8.1 corrections after a negative Prüfmitteilung |

Events `de.mabis.submission.failed` and `de.mabis.korrekturbedarf.opened` go
through the transactional outbox to `erp_webhook_url`. MCP: 4 read-only tools at
`/mcp`.

## Configuration

```toml
# mabis-syncd.toml
# Top-level keys first: every block below is a table, and each nested block
# rejects an unknown field, so a bare key placed under one fails startup.

# de.mabis.* CloudEvents, drained from the transactional outbox.
erp_webhook_url = "https://erp.example.com/webhooks/mabis"
erp_hmac_secret = "env:MABIS_ERP_HMAC_SECRET"

[http]
addr = "0.0.0.0:8880"

[database]
url = "env:DATABASE_URL"

[identity]
tenant                 = "9900357000004"
sender_mp_id           = "9900357000004"   # MSCONS NAD+MS
receiver_mp_id         = "9900077000006"   # BIKO, MSCONS NAD+MR
bilanzierungsgebiet_id = "11YMAKO-TEST-01U"  # Y-type (Area) EIC

[edmd]
url     = "http://edmd:8380"
api_key = "env:MABIS_EDMD_API_KEY"

[marktd]
url     = "http://marktd:8180"
api_key = "env:MABIS_MARKTD_API_KEY"

[makod]
url     = "http://makod:8080"
api_key = "env:MABIS_MAKOD_API_KEY"

[schedule]
erstaufschlag_werktag = 10
run_hour_utc          = 5    # 06:00 CET / 07:00 CEST

[oidc]
issuer   = "https://auth.example.com/realms/mako"
audience = "api://mako-mabis-syncd"
```

Startup refuses a `bilanzierungsgebiet_id` that is not a Y-type EIC,
`submission_target = "mabis-hub"` (no published wire format), and a missing
`[oidc]` unless `allow_insecure_no_auth = true`. DB suite:
`just test-mabis-syncd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/mabis-syncd/)
covers the aggregation, versioning, timekeeping and the full configuration
reference.
