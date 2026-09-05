# mabis-syncd — MaBiS Summenzeitreihen (ÜNB / NB role)

Aggregates per-MaLo Lastgang from `edmd` into Bilanzierungsgebiets-Summenzeitreihen
and files them with the BIKO as MSCONS PID 13003, through `makod`.

| | |
|---|---|
| **Port** | `:8880` |
| **Regulatory basis** | BK6-24-174 Anlage 3 (MaBiS); MSCONS AHB 3.2 § 8.3.1 |
| **Commodity** | Strom only — gas balances through GaBi Gas, on the Gastag and against a Marktgebiet |

```text
edmd /billing-periods ──► discover MaLos (Sparte = STROM)
marktd /malos/{id}    ──► group by Bilanzierungsgebiet
marktd /…/mabis-zp    ──► the LOC+172 Meldepunkt per territory
edmd /energy/{id}     ──► the canonical Bezug projection (domain::register)
                          │
              SummenzeitreiheBuilder (15-min grid, Berlin-local)
                          │
              makod ──► one MSCONS 13003 per Bilanzierungsgebiet
```

## One filing per territory

MaBiS settles per Bilanzierungsgebiet, so a run files one MSCONS per territory
and records each in `submission_series` with its own `message_ref` or its reason
for failing. `submission_runs` keeps the aggregate.

An acked Summenzeitreihe **cannot be withdrawn**, so a retry of a partly-filed
run skips the territories the BIKO already accepted; re-sending one is a
correction under a higher version, not a retry. The run still fails as a whole
when any territory did not go out — a month settled short is not a success.

A retry is a *new* run, so what it may skip is read for the whole
Bilanzierungsmonat rather than for the run: acknowledgement is a property of the
(Bilanzierungsgebiet, Bilanzierungsmonat) the BIKO holds, not of whichever of our
runs filed it. A **correction** is told apart by its `corrects_run_id`: every run
answering one negative Prüfmitteilung shares it, so a correction re-files the
acked territory — which is the point of it — while a retry of that correction
does not send it twice. A retry that finds every territory already acked files
nothing and succeeds.

## The run refuses rather than under-reporting

The BIKO cannot tell a short Summenzeitreihe from a complete one, and a filing
is irreversible once acked. A run therefore fails when:

- a discovered MaLo could not be aggregated (no Bilanzierungsgebiet in `marktd`,
  a fetch failure, a Lastgang that does not match the settlement grid);
- a territory's grid still has empty slots after every MaLo is folded in;
- a territory has no MaBiS-Zählpunkt assigned, or `marktd` returns the
  Bilanzierungsgebiet EIC in its place.

Nothing falls back to the configured territory: misfiling energy into the wrong
zone is a settlement error the BIKO cannot detect.

## Windows and Datenstatus

The phase follows from the Werktag calendar (§ 3.10, Tabelle 2), not from the
caller:

| Window | Werktage after period end | Datenstatus of a new version |
|---|---|---|
| Erstaufschlag (BKA) | 1.–10. WT | Abrechnungsdaten directly |
| Clearing (BKA) | 11.–30. WT | Prüfdaten, promoted by a positive Prüfmitteilung |
| KBKA | 31. WT – end of month 7 | Prüfdaten |

Those are the **BG-SZR (Kategorie B)** rows of § 3.10 Tabelle 2, which is what
this service files. A BK-SZR runs two Werktage longer at each end (1.–12. and
13.–30. WT) and the DZÜ has no Erstaufschlag at all — `mako_mabis::fristen` has
the whole table, keyed on the Summenzeitreihe.

The BIKO's own Abrechnungsstichtage sit after the clearing window: the
vorläufige Bilanzierung on the 18. WT (Datenstand 15. WT) and the
abrechnungsrelevante on the 42. WT (Datenstand 30. WT).

The scheduler fires at `run_hour_utc` on `erstaufschlag_werktag` — the last
Werktag of the Erstaufschlag window, which gives the aggregate the most complete
input while the BIKO still assigns Abrechnungsdaten automatically.

A Summenzeitreihe is identified by (MaBiS-Zählpunkt, Bilanzierungsmonat,
Version), and the version ascends across the whole BKA (§ 3.8.2). It is stored
truncated to whole seconds, because MSCONS SG6 `DTM+293` carries no more and the
BIKO echoes it back for matching.

The Datenstatus is assigned **exclusively by the BIKO** (§ 3.8.3) and arrives
inbound via IFTSTA; this service records it and never derives one.

## Timekeeping

The MaBiS grid is Berlin-local. `period_to` is the inclusive last day, so the
window runs to the *following* Berlin midnight — March holds `31 × 96 − 4` slots
and October `31 × 96 + 4`. A UTC window keeps every month at 24 h a day, which
is wrong for exactly the two DST months.

## API

| Method | Path | Cedar action |
|---|---|---|
| `POST` | `/api/v1/sync` | `trigger-mabis-run` |
| `GET` | `/api/v1/runs` · `/runs/{id}` | `read-mabis-run` |
| `PUT` | `/api/v1/runs/{id}/retry` | `trigger-mabis-run` |
| `POST` | `/api/v1/datenstatus` | `record-biko-response` |
| `POST` | `/api/v1/pruefmitteilung` | `record-biko-response` |
| `GET` | `/api/v1/korrekturbedarf` | `read-mabis-run` |

Filing is restricted to the `NB` and `UENB` roles. Recording an inbound BIKO
response is a separate, unrestricted-by-role action: relaying an IFTSTA needs
none of the power to file. `tests/authorization_guard.rs` pins both directions.

`POST /api/v1/sync` refuses a period that already has a live run — filing it
again is a correction, which is what `corrects_run_id` is for.
`GET /api/v1/runs/{id}` returns the per-territory `series`.

## Korrekturbedarf (§ 9.8.1)

A negative Prüfmitteilung opens an obligation: the ÜNB answers with a corrected
BG-SZR under a higher version. `GET /api/v1/korrekturbedarf` lists the open
ones; filing the correction with `corrects_run_id` closes them.

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

Startup refuses a `bilanzierungsgebiet_id` that is not a Y-type EIC — a
Bilanzkreis is type `X` and the same length, and `LOC+107` carries the value as
free text, so the BIKO would accept either. It also refuses
`submission_target = "mabis-hub"`: BK6-24-210 has no Beschluss, so no wire
format is published, and an invented one that reaches a real Hub is
indistinguishable at the point of failure from a correct one that was rejected.

Without `[oidc]`, startup refuses unless `allow_insecure_no_auth = true` — a
MaBiS submission is a binding filing that cannot be withdrawn.

## Events

`de.mabis.submission.failed` and `de.mabis.korrekturbedarf.opened`, written to
the transactional outbox in the same transaction as the row they describe. Set
`erp_webhook_url` or nothing delivers them.

## MCP server

`/mcp` (Streamable HTTP), read-only over submission state.

## Tests

`cargo test -p mabis-syncd` runs the unit and policy tests. The real-PostgreSQL
suite (`tests/schema_pg.rs`) needs a Docker daemon and skips without one:

```sh
DOCKER_HOST=unix://$HOME/.docker/run/docker.sock cargo test -p mabis-syncd
```
