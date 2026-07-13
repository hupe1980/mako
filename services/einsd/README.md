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
| **Settlement models** | 8: VERGUETUNG, MIETERSTROM (§38a), DIREKTVERMARKTUNG (§20 Gleitende Marktprämie), AUSSCHREIBUNG, POST_EEG_SPOT, EIGENVERBRAUCH, KWKG_ZUSCHLAG (§7 KWKG 2023), FLEXIBILITAET (§50) |
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
by **18 unit tests** without a database:

```bash
cargo test -p einsd --test settlement_tests
```

## Configuration

```toml
# einsd.toml
database_url   = "postgresql://einsd:secret@db:5432/einsd"
port           = 9180
tenant         = "9900357000004"

edmd_url       = "http://edmd:8380"

[erp]
webhook_url    = "http://erp:8000/events"
hmac_secret    = "${ERP_HMAC_SECRET}"
```
