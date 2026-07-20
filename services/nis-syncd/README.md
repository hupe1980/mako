# nis-syncd — NIS/GIS Grid Topology Import (NB role, stateless)

`nis-syncd` imports MaLo grid records from the NB's NIS/GIS system into `marktd`.
It is **stateless** — no database, no persistent state. Every sync run is idempotent.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9680` |
| **Stateless** | No database; one sync endpoint |
| **Sync** | `POST /api/v1/grid/sync` — pushes `malo_grid` records to `marktd` |
| **Dry-run** | `?dry_run=true` — shows what would be updated without writing |
| **Drift detection** | Emits `de.markt.grid.drift.detected` CloudEvent on Bilanzierungsgebiet divergence |
| **STP impact** | Without `nis-syncd`: Anmeldung STP ≈ 60 %; with: ≈ 95 % |
| **Source** | NB's own NIS (SAP IS-U, Smallworld, Schneider EcoStruxure) — NOT BNetzA MaStR |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Why NIS/GIS, not MaStR?

The BNetzA Marktstammdatenregister (MaStR) contains generation assets (EEG plants,
controllable loads). The `Bilanzierungsgebiet` and `Netzgebiet` for each MaLo — which
determines Anmeldung STP check #4 — comes from the NB's own NIS/GIS, not from MaStR.

## Configuration

```toml
# nis-syncd.toml
port           = 9680
nb_mp_id       = "9900357000004"      # required — BDEW Codenummer of the grid operator

marktd_url     = "http://marktd:8180"
marktd_api_key = "env:MARKTD_API_KEY"

# Optional — CloudEvents receiver for topology drift notifications.
drift_webhook_url = "http://erp:8000/events"

sync_concurrency = 8
max_batch_size   = 500
```
