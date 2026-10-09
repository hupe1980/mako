# obsd

`obsd` (`:8480`) projects every `de.mako.*` CloudEvent into a read-model of
running and completed MaKo processes and serves per-PID KPIs, Antwortfrist
alerts and the § 7a Abs. 5 EnWG Gleichbehandlung parity evidence. The
projection is a CQRS read-model: it holds no authoritative data and rebuilds by
replaying `marktd`'s event stream.

## Run it

```bash
just dev obsd          # dev/config/obsd.toml against the dev PostgreSQL
```

`obsd --check` is the container `HEALTHCHECK`; the config file comes from
`OBSD_CONFIG`.

## At a glance

| Feature | Detail |
|---|---|
| Database | PostgreSQL 15+, one table: `process_projections` (`migrations/0001_schema.sql`) |
| Inbound | `de.mako.*` from `marktd` at `POST /webhook` (Standard Webhooks), six types by default, widened with `[subscription] event_types` |
| Deadlines | from `mako-fristen` only. The APERAK Frist (`de.mako.aperak.timeout`) and the business Antwortfrist (`deadline_at`, with `deadline_source`) are separate fields; a PID with no published window has `deadline_risk = unknown`, never `green` |
| Reporting periods | keyed on `started_at` read as a Europe/Berlin date |
| Auth | OIDC + Cedar `policies/obsd.cedar` on REST and MCP; startup refuses without `[oidc]` unless `allow_insecure_no_auth` |
| MCP | 6 tools + 2 prompts at `/mcp` |

## Routes

| Route | Purpose |
|---|---|
| `GET /obs/processes` | filter by `state`, `family`, `pid`, `partner_mp_id`, `mdm_role`, `since`, `limit`; an unknown `state` is a `400` |
| `GET /obs/processes/{process_id}` | one projection |
| `GET /obs/kpis?pid=&period=YYYY-MM` | per-PID KPI report (`pid` required) |
| `GET /obs/overdue` | open processes past `deadline_at`, most urgent first |
| `GET /api/v1/audit/gleichbehandlung` | affiliate vs third-party evidence (`initiator_is_affiliate`) |
| `GET /obs/metrics` | business gauges, unauthenticated — restrict at the ingress |

With `[webhook] outbound_url` set, sweep workers emit `de.obs.deadline.approaching`
and `de.obs.stp.parity.alert` (tuned in `[worker]`).

## Configuration

Sections: `[http]`, `[database]`, `[identity]` (`tenant`, `own_mp_ids` — every
operator MP-ID, Strom and Gas, for affiliate detection), `[marktd]`, `[webhook]`
(`inbound_secret`, `outbound_url`, `outbound_secret`), `[subscription]`,
`[worker]`, `[oidc]`. `dev/config/obsd.toml` is a working example.

The [operator guide](https://hupe1980.github.io/mako/docs/services/obsd/)
covers the projection, KPIs, the parity report and the full configuration
reference. The read-model types live in [`mako-obs`](../../crates/mako-obs/).
