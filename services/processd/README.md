# processd

`processd` (`:8580`) is the process decision engine. It subscribes to `marktd`'s
durable fan-out and, for each `de.mako.process.initiated`, applies the policy of
the Marktrolle that owes the answer — automatically where a published
Entscheidungsbaum decides, through an operator approval queue (bounded by the
business Frist) where it does not. Answers go to `makod` as commands.

Two modules ride the same delivery. The **process read model** (`src/obs/`)
projects every `de.mako.*` lifecycle event into `process_projections` — per-PID
KPIs, the overdue list against each process's Antwortfrist, the § 7a Abs. 5 EnWG
Gleichbehandlung evidence — in every role build, and is rebuildable from
`marktd`'s replay log. In an NB build the **Sperrauftrag queue** (`src/sperr/`)
turns an inbound ORDERS 17115 / 17117 into a field work order and reports its
outcome as IFTSTA 21039. `POST /webhook` verifies once and hands each event to
the read model first, then to the queue, then to the decision modules.

## Run it

```bash
just dev processd                                              # dev/config/processd.toml
cargo run -p processd --no-default-features --features nb-only # one role only
```

Role features select which decision paths are registered, so a binary cannot
answer as a role it was not built for (`tests/it/role_separation.rs`):
`role-{lf,nb}-{strom,gas}`, `role-msb`, and the composites `lf-only`,
`nb-only`, `msb-only`, `integrated`. The default build is `integrated`,
which registers every role's paths.

## What answers what

The PID is the inbound trigger, never the answer. Every window comes from
`mako_fristen::antwort` — the table `makod` registers the process deadline from —
and is evaluated in Europe/Berlin; `processd` adds only `OPERATOR_HEADROOM`.
The APERAK (45 min Strom UTILMD) is `makod`'s, not this service's.

| Role | Inbound PID | Process | Business Frist | Decision basis |
|---|---|---|---|---|
| **NB** | 55001, 55077 | GPKE Anmeldung (verb. / erz. MaLo) | **11:00 Uhr des 1. WT nach dem ÜT** | `mako_pruefung::evaluate` — `E_0622`'s two branches, confirmed out of `E_0623` |
| **NB** | 55004 | GPKE Abmeldung — Lieferende von LF an NB | **06:00 Uhr des 1. WT nach dem ÜT** | `mako_pruefung::evaluate_abmeldung` (`E_0607`) |
| **NB** | 44001 | GeLi Gas Anmeldung NN | **Ablauf des 4. Werktags** | `mako_pruefung::evaluate` (`G_0011`) |
| **NB** | 44004 | GeLi Gas Abmeldung NN | **Ablauf des 3. Werktags** | `mako_pruefung::evaluate_abmeldung` (`G_0007`) |
| **NB** | 55042 | WiM Anmeldung MSB (MSBN → NB) | 5 WT | MeLo / partner / Zählertyp checks |
| **NB** | 55051 | WiM Ende MSB (MSBA → NB) | 7 WT | operator queue |
| **NB** | 55600, 55601 | GPKE Neuanlage (verb. / erz. MaLo) | **00:00 Uhr des 61. WT nach dem ÜT** | `mako_pruefung::nb::neuanlage` (`E_0608`), daily Prüflauf |
| **NB** | *(event)* | EoG gap closure → 55013 (§ 36/§ 38 EnWG) | unverzüglich; 3-month timer | `grundversorger` from `marktd` |
| **LF** | 55007 | Abmeldung / Beendigung der Zuordnung vom NB (EBD `E_0609`) | **05:00 Uhr des 1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` + `mako_fristen::abmeldung` |
| **LFA** | 55010 | Anfrage zur Beendigung der Zuordnung (EBD `E_0624`) | **09:00 Uhr des 1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` |
| **LFA** | 55016 | GPKE Kündigung (LFN → LFA, EBD `E_0614`) | **1. WT nach dem ÜT** | `VersorgungsStatus` from `marktd` |
| **LFN** | 55607 | Ankündigung Zuordnung LF, erz. MaLo / Tranche (EBDs `E_0603`–`E_0606`) | **15:00 Uhr am ÜT** | `malo.bilanzierungsgebiet` from `marktd` + `[[lf.bilanzkreise]]` |
| **E/G** | 55013 | Anmeldung Ersatz-/Grundversorgung (EBD `E_0615`) | **15:00 Uhr am ÜT** | `[lf] grundversorgungs_netzgebiete` + `VersorgungsStatus` |
| **LFA** | 44007, 44010, 44016 | GeLi Gas Lieferende von NB / Beendigung der Zuordnung / Kündigung | 3 Werktage | `VersorgungsStatus` from `marktd` |
| **E/G** | 44013 | GeLi Gas Anmeldung Ersatz-/Grundversorgung (`E_3008`) | **Ablauf des 2. WT** | as 55013 |
| **MSB** | 55039 | WiM Kündigung MSB (MSBN → MSBA) | 3 WT | MeLo / partner checks |
| **MSB** | 55168 | WiM Verpflichtungsanfrage (NB → gMSB) | 1 WT | operator queue |
| **MSB** | 35001, 35002, 35004, 35005 | REQOTE Preisanfrage | 5 WT | `PreisblattMessung` from `marktd` |
| **MSB** | 35003 | ESA Werteanfrage (WiM Teil 2 Kap. 4) | 5 WT | `mako_pruefung::esa::wertebestellung` (`E_0252`) — refusals dispatch, a surviving Anfrage queues |
| **MSB** | 17007 | ESA Bestellung von Werten | 2 WT | `mako_pruefung::esa::wertebestellung` (`E_0256`) |
| **MSB** | 39002 | ESA Stornierung der Bestellung | 2 WT | `mako_pruefung::esa::wertebestellung` (`E_0257`) |
| **MSB** | 17008 | ESA Abbestellung von Werten | 2 WT | `mako_pruefung::esa::wertebestellung` (`E_0254`) |
| **MSB** | *(workflow)* | § 14a Steuerungsauftrag → ORDRSP | 5 WT | contracted `konfigurationsprodukte` |

A transport failure on master data answers `5xx` so the fan-out redelivers; only
a genuine absence of that data escalates to an operator.

## Routes

| Method | Path | Cedar action | Description |
|---|---|---|---|
| `POST` | `/webhook` | *(HMAC)* | Inbound `marktd` CloudEvent |
| `GET` | `/api/v1/decisions` | `read-decisions` | Recent Anmeldung STP decisions |
| `GET` | `/api/v1/queue` | `read-queue` | Approval-queue entries |
| `POST` | `/api/v1/queue/{id}/approve` · `/reject` | `decide-queue` | Decide an entry; dispatches the answer frozen on it, or the operator's (`antwort_code`, `antwort_tree`, `antwort_codeliste`, `bemerkung` in the optional body) — refused with `422` when the command's answer contract is not met |
| `POST` | `/api/v1/start-supply` · `-gas` | `initiate-supply` | LFN bootstrap (Strom / Gas). Both require a **Bilanzkreis** — Strom as the `SG8 SEQ+Z79` Produktpaket, Gas as `SG10 CCI+Z19` — falling back to a single configured `[[lf.bilanzkreise]] standard` entry, else `422` |
| `POST` | `/api/v1/end-supply` · `-gas` | `initiate-supply` | LF Lieferende bootstrap |
| `GET` | `/api/v1/eog` | `read-eog` | EoG case log (`?status=`) |
| `GET` | `/api/v1/neuanlage` | `read-neuanlage` | `E_0608` case log (`?status=`), with each case's `letzter_pruefungstag` |
| `PUT` | `/api/v1/neuanlage/{id}/identifikation` | `identify-neuanlage` | Record the MaLo-ID the NB matched from address/device data |
| `GET` | `/obs/processes` · `/obs/processes/{id}` | `read-process` | Process projections |
| `GET` | `/obs/kpis` | `read-kpi` | Per-PID KPIs for a `YYYY-MM` month |
| `GET` | `/obs/overdue` | `read-overdue` | Past their business Antwortfrist |
| `GET` | `/api/v1/audit/gleichbehandlung` | `read-kpi` | § 7a Abs. 5 EnWG evidence (`?year=&format=csv`) |
| `POST` | `/api/v1/obs/rebuild` | `rebuild-projection` | Replay `marktd`'s `GET /admin/events` into the projection (`ADMIN`) |
| `GET` | `/obs/metrics` | — | Read-model gauges; restrict at the ingress |
| `GET` · `POST` | `/api/v1/sperr-orders` | `read-sperr-order` · `create-sperr-order` | The Sperrauftrag queue (NB build) |
| `GET` | `/api/v1/sperr-orders/{id}` · `/stats` | `read-sperr-order` | One order; counters incl. outstanding IFTSTA |
| `PUT` | `/api/v1/sperr-orders/{id}/execute` · `/fail` | `execute-sperr-order` | Field outcome → IFTSTA 21039 `Z14` / `Z13` |
| `PUT` | `/api/v1/sperr-orders/{id}/cancel` | `cancel-sperr-order` | Withdraw a pending order |
| `GET` | `/health/live` · `/health/ready` · `/metrics` | — | Mounted by `mako_service::run` |
| `GET\|POST` | `/mcp` | `use-mcp` | MCP Streamable HTTP |

Every route checks a Cedar action in `policies/processd.cedar`; the deciding
principal is recorded in `approval_queue.decided_by` and on the dispatched
command.

## Configuration

`PROCESSD_CONFIG` names the file; `PROCESSD_<SECTION>__<KEY>` overrides any key.

- **Wiring:** `[http]`, `[database]`, `[identity] own_mp_id` (must match
  `makod`'s primary party), `[makod]`, `[marktd]`, `[webhook] inbound_secret`,
  `[subscription]` (self-registers with `marktd` at startup), `[oidc]`.
- **Automation per role:** `[nb]` (`auto_accept`, `einsd_url`), `[lf]`
  (`auto_respond`, `[[lf.bilanzkreise]]`), `[msb]`, `[esa]`, `[eog]`.
- **Read model:** `[identity] own_mp_ids` (§ 7a Abs. 5 affiliate detection),
  `[obs]` (`outbound_url` / `outbound_secret` for the `de.obs.*` producers, the
  sweep intervals, the operator's `parity_threshold_pp`).
- **Sperrauftrag queue:** `[sperr]` (`erp_webhook_url` / `erp_hmac_secret` for
  the `de.sperr.*` notices).

DB suites: `just test-processd-db` — the approval queue, the read model's
sweeps, its rebuild and the webhook fan-out, and the Sperrauftrag queue.

The [operator guide](https://hupe1980.github.io/mako/docs/services/processd/)
covers the NB and LF decision pipelines, the Entscheidungsbäume, metrics and the
full configuration reference.
