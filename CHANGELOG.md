# Changelog

What changed for people who run, integrate or build on mako. Docs and READMEs
describe the current state; this file is the one place that records change.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The workspace version in `Cargo.toml` is the release being worked on, and the
top section collects its changes until the release replaces „unreleased" with
its date; `just bump <version>` opens the next section. mako is pre-1.0: any
release may break, and every break is listed under **Changed** or **Removed**.
History before this file is in `git log`.

## [0.24.2] — 2026-11-05

### Fixed

- The release's image builds run from the checkout (`source: .`), as the
  builder jobs do: the default git-URL context gave `COPY . .` other cache
  keys, so the registry cache never matched and every stage recompiled.
- The bake metadata leaves provenance out (`BUILDX_METADATA_PROVENANCE`), so
  the digest export starts: with fifteen images it outgrew the 128 KiB an
  environment string may hold. The provenance attestations are still pushed.
  v0.24.0's and v0.24.1's images, `mako-desk` included, ship with this
  release.

## [0.24.1] — 2026-11-04

### Fixed

- The release's Docker images build again: the `builder` and `demo-builder`
  stages reach the bake jobs as registry cache manifests
  (`mako-<stage>:cache-<arch>`) instead of the GHA cache, which CI's
  rust-cache evicted, so the bake recompiled every service until it timed
  out. The bake job's limit is 60 minutes. v0.24.0's images, `mako-desk`
  included, ship with this release.
- `cargo xtask bump-version` sets the Desk's version, and `check-desk`
  refuses a Desk version other than the workspace's.

### Changed

- Dependencies refreshed; `thrift` 0.23 fixes RUSTSEC-2026-0435, so its
  exception and VEX statement are gone.

## [0.24.0] — 2026-11-04

### Added

- The `mako-desk` image: the Desk's static bundle on :8080 (nginx), released
  with the service images, for oauth2-proxy as the OIDC gateway
  (`desk/deploy/gateway.yaml` forwards `/svc/<service>/` with the person's
  access token). CSP `script-src 'self'`, styles by a per-request nonce; the
  account menu names the person from the gateway and signs out through it.
- Desk **Freigaben**: the decisions processd holds for a person, per Rolle
  and Frist, with the codes each action needs; every decision is confirmed
  and sent under the person's name. The assistant may propose one
  (`vorschlag` in the answer); the Desk prefills the form, never sends it,
  and records its fate via `POST /api/v1/runs/{run}/bewertung {vorschlag}`.
- Desk Fragen shows its work: the model that answers, the services being read
  — each ticked off as it answers, from agentd's new `schritte` on the answer —
  every read the finished answer made, and „Nochmal fragen" after a failed
  run. `⌘J` asks from anywhere on the page; without agentd the panel is not
  offered.
- agentd `POST /api/v1/runs/{run}/bewertung` and the Desk's „Hilfreich?"
  under every answer and Hinweis: a person's verdict, with an optional reason,
  kept by the runtime beside the run under their name (`api:run.assess`); the
  answer carries the reader's standing verdict as `hilfreich`.
- makod commands `gpke.geschaeftsdatenanfrage.anfragen` / `.beantworten` /
  `.ablehnen`: the ORDERS 17132, the UTILMD 55035/55095/55060 answer
  (`RFF+AAV`, Stammdaten passed segment by segment and checked against the AHB)
  and the ORDRSP 19101 refusal. `mako_engine::payload::UtilmdPayload` gains
  `anfrage_referenz` and `stammdaten`; edi-energy's UTILMD builder gains
  `Sg8Builder::segment` and `UtilmdTransactionBuilder::sg12_segment`.
- **mako Desk** (`desk/`): the operator web UI — open a process by its
  business key, every Frist in Berlin time with a countdown, ⌘K command
  palette, one market role at a time (Netz, Lieferant, Messstelle,
  Energieservice). `just desk` runs it against makod; see the
  [guide](https://hupe1980.github.io/mako/docs/guide/desk/).
- vertragd MCP tool `search_kunden`: find a customer by the start of a name or
  Kundennummer, or by a MaLo-ID of one of their Lieferstellen (at most 20).
- Desk: **Heute** for the Lieferant — every Anmeldung still waiting, the one
  whose makod Frist runs out first on top.
- Desk: **Kunden** and the customer view for the Lieferant — every Lieferstelle
  with the state of its supply and the next Frist of any process on it, each
  opening its MaLo's processes; ⌘K finds customers as you type. Each role opens
  on its own home page.
- MCP tools can declare an `outputSchema`; makod `get_process` and vertragd
  `search_kunden` do, and each service's tool contract is pinned in
  `services/<name>/mcp.snapshot.json` (`BLESS_MCP_SNAPSHOT=1` rewrites it).
- `cargo xtask check-desk`: the Desk's npm licences are held to `deny.toml`,
  and no Desk source computes a date or amount outside `src/format/`.
- Desk: the account menu at the foot of the sidebar — your market roles and
  the appearance (System, Hell, Dunkel) — and `⌘/`, which lists
  every key the Desk answers to.
- Desk: the **Überblick** — a tile per figure, each its service's own number
  opening the list it counts — and **Fristen überschritten**, every open
  process past its Antwortfrist with its Fundstelle (processd).
- processd `list_open_processes`: open processes by PID and/or MaLo, the
  earliest Frist first, a keyset page at a time (`nach` / `weiter`).
- vertragd `kunden_zu_malos`: the customers of up to 200 MaLos in one call.
- processd `list_overdue_processes` declares an `outputSchema`; processd's
  tool contract is pinned in `services/processd/mcp.snapshot.json`.
- `just bump <version>` opens the version's section in this file.
- Desk: the **Assistent** page — each skill's title, purpose, trigger, model and
  the services it may read, from its manifest; and process cards titled for an
  operator („Lieferantenwechsel · Strom"), process states in German.
- Desk: **Hinweise** — what a skill noticed and left for a person, on Heute
  and the Überblick and under the customer or MaLo it concerns; a skill can be
  muted in one click. agentd `GET /api/v1/hinweise` serves them, each with its
  skill and the Anker its run's case is about.
- Every agentd manifest carries `mako.de/titel` and `mako.de/zweck`, the
  German name and purpose the Desk shows.
- `cargo xtask check-known-wrong` refuses attributing the annual equal-treatment
  report to the Netzzugang paragraph: it is the Gleichbehandlungsbericht of
  § 7a Abs. 5 Satz 3 EnWG.
- Desk: each process card carries its **Verlauf** — messages in on the left,
  out on the right, with PID, Berlin time and document number, the pending
  Fristen at the end.
- Desk: **ask the assistant** under a customer or a MaLo's processes. The
  answer comes sentence by sentence, each with the read it rests on; a
  sentence whose figures that read does not carry is marked „nicht belegt".
- Desk: services are named as an operator knows them — Marktkommunikation,
  Prozessüberwachung, Kundenverwaltung — never by their crate name.
- `mako_engine::EventPayload::verlauf`: an event that is a market message
  states its direction, PID and document number, stamped on the envelope at
  append. Every message event type in the workflow crates states its own;
  `cargo xtask check-event-verlauf` refuses one whose `event_type` names a
  message and that states none. mako-redispatch's ack-forward event types
  pass it through to the event they wrap.
- makod `get_process_history`: a process's events, oldest first, its market
  messages marked by direction and PID. Each pending deadline in
  `get_process` carries its `art` — `aperak`, `contrl` or `prozess` — and the
  Desk names it („Eingangsbestätigung (APERAK)", „Frist des Prozesses").
- processd `count_open_processes`: the open processes a PID/MaLo filter keeps,
  and how many of their Fristen end before the Berlin day does, with the
  earliest — one aggregate however many are open.
- Desk: the **Lage** on Heute — Fristen ending today with the earliest,
  overdue Fristen, Anmeldungen waiting and not yet sent — every figure a
  service's count, no model in it; each line opens the list it counts.
- agentd `desk-assistant` specialist and `GET /api/v1/runs/{run}/antwort`: a
  run's answer, each sentence checked against the tool call it cites
  (Belegprüfung).
- `mako_fristen::in_berlin`: an instant on the German wall clock.
- `mako_markt::bo4e::Bo4e::ensure_no_undefined_fields`: refuse a BO4E
  document carrying a key BO4E does not define, which the gate keeps under
  `_additional` for documents a counterparty sends.
- `cargo xtask check-request-bodies` refuses a request body extracted as
  `serde_json::Value`: with no fields, a misspelt key is read as absent.
- makod commands for outbound legs that had none: `gpke.stornierung.bestaetigen`
  / `.ablehnen` (NB, UTILMD 55023/55024) and `geli.stornierung.bestaetigen` /
  `.ablehnen` (GNB, 44023/44024) answer a Stornierungsanfrage;
  `wim.ersteinbau.vorabinformation-senden` (gMSB, IFTSTA 21029, refused inside
  the 3-Monate-und-3-WT Vorlauf) and `wim.ersteinbau.beantworten` (wMSB,
  21030/21031 by the `E_0233` code); `wim.preisanfrage.anfragen` sends the
  REQOTE. `wim.preisanfrage.angebot-senden` now queues the QUOTES it recorded
  and finds the process by `melo_id` or `malo_id` — **breaking** for
  `mako_wim::PreisanfrageCommand::SendAngebot`, which takes the `location`.

### Changed

- **Breaking — processd Freigaben**: `list_pending_approvals` and
  `get_queue_entry` return a typed `Freigabe` (`{ freigaben, count, saturated }`
  for the list, the earliest `expires_at` first, with an optional `marktrolle`
  filter) instead of the raw queue row; each entry records the
  Entscheidungsbaum its module decided with (`approval_queue.ablehnung_ebd`),
  and approve / reject refuse with `422` an `antwort_code` outside that tree's
  cluster for the answer sent. `GET /api/v1/queue` lists by `expires_at` too.
- **Breaking — Geschäftsdatenanfrage**: new `gpke-geschaeftsdatenanfrage`
  for ORDERS 17132, answered by UTILMD 55035/55095/55060 or ORDRSP 19101
  (ORDERS AHB 1.1b Kap. 4.1.1); `wim-stammdaten` is removed, and the other
  ORDERS 17102–17133 go to their own owner or are dead-lettered. marktd's
  `ZaehlzeitRegister` feed from those ORDERS is gone.
- **Breaking — GPKE / GeLi Gas Stornierung**: the NB answers 55022 / 44022
  with UTILMD 55023 / 55024 (Gas 44023 / 44024), not an APERAK.
  `DispatchAperak` / `AperakDispatched` are now `SendAntwort { ablehnung }` /
  `AntwortGesendet`; `ReceiveUtilmd` carries `document_code`.
- **Breaking — workflow events** carry the PID they were sent or received
  with (new fields on the GPKE, GeLi Gas, mako-invoic and MaBiS variants that
  lacked one), and events beside an APERAK 312/313 state it as outbound. GeLi
  Gas Datenabruf's own ORDERS is the new `GeliGasDatenabrufAnfrageGesendet`.
  Rebuild any literal of these variants.
- **Breaking — WiM Ersteinbau**: the gMSB records an inbound 21030/21031 with
  the new `ReceiveAntwort` and sends nothing; `DispatchAntwort` and
  `MeldeScheitern` are the wMSB's. makod reads the `E_0233` code from `STS`
  DE 9013 and refuses an answer without one. The Scheitermeldung 21027 follows
  `A02` (new state `Eigenausbau`), per WiM Teil 1 Kap. 3.5.2 Nr. 6.
- **Breaking — WiM Preisanfrage** (`wim-preisanfrage`): the requester sends its
  REQOTE with the new `SendReqote` command (`PreisanfrageAnfrageGesendet`,
  outbound, REQOTE in the outbox) and records the QUOTES answering it from the
  new `Angefragt` state; `ReceiveAngebot` is refused on the responding side
  and for a QUOTES that does not answer the REQOTE's PID.
- **Breaking — accountingd** `PUT …/abschlag`, `PUT …/business-partner`, `POST
  /dunning/{account_id}/escalate`, `PUT …/vorauszahlung` and `PUT
  …/zahlungsinformation` read typed bodies through `mako_service::Json` (the
  last two as `Bo4e<T>`, refusing a key BO4E does not define): an unknown key
  or a missing mandatory field is a `422`; `escalate` requires `stufe` (1–3)
  and `due_days` (≥ 1) instead of defaulting them to 1 and 14, and a
  Vorauszahlung `betrag.wert` out of ct range is refused instead of stored as
  0.
- **Breaking — processd** `start-supply[-gas]`, `end-supply[-gas]` and `PUT
  /neuanlage/{id}/identifikation` read typed bodies through
  `mako_service::Json`: an unknown key is `422 unknown_field`, a missing
  mandatory one a `422`; `start-supply-gas` refuses a non-YYYYMMDD
  `process_date` (`INVALID_DATE`) and forwards only its declared fields;
  `end-supply-gas` takes `zaehlpunkt` + `process_date` instead of
  `lieferende_datum` and dispatches as `LFG`, which `geli.lieferende.anmelden`
  requires.
- **Breaking — edmd** `PUT /api/v1/smgw/{malo_id}` reads a typed `SmgwSession`
  and `POST /api/v1/virtual` a typed request through `mako_service::Json`: an
  unknown key is a `422`; a malformed `rule_json` or a `sparte` outside
  `STROM`/`GAS`/`WAERME`/`WASSER` is a `422` instead of a `400` or a failed
  insert.
- **Breaking — marktd** `PUT
  /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` takes
  `{"konfigurationsprodukte": [...]}` instead of a bare array: the list is
  required, each element crosses the BO4E gate, and an unknown key at any
  depth is a `422`.
- **Breaking — vertragd** `PUT /kunden/{id}/person`,
  `/kunden/{id}/zahlungsinformation`
  and `/vertraege/{id}/preisgarantie` read their body as `Bo4e<T>` and refuse a key
  BO4E does not define (`422`, `bo4e.unknown_field`) instead of storing it under
  `_additional`.
- **Breaking — makod `get_process`** returns every process filed under the
  business key — `{business_key, processes: [{stream_id, workflow_name,
  process_id, deadlines}]}` — instead of one process. It now finds processes
  started by an inbound message; before, it found none of them. The `active`
  field is gone.
- **Breaking — vertragd `get_kunde`** returns the operator's view of a
  customer — Kundennummer, Kundentyp, Haushaltskunde, name and every
  Lieferstelle with its status — and no longer the raw row with payment data
  and portal identities. A failed read is an error, never an empty list.
- **Breaking — vertragd `find_stuck_workflows` is replaced by
  `list_anmeldungen`**: every Lieferstelle still angemeldet, with its customer;
  no threshold and no age. Whether one is late is makod's deadline for the MaLo
  (`get_process`). The old tool flagged an Anmeldung after a flat 5 (Strom) or
  10 (Gas) days since its row last changed, which is no Frist.
- `cargo xtask check-known-wrong` refuses a bare Antwortcode glossed with a
  meaning (`A02 = …`): a code means something only within its tree.
- `cargo xtask check-frist-literals` also reads the agent manifests: a
  procedure states no Frist beyond the explanations it admits by name.
- **Breaking — makod `list_active_processes`** reports `process_count`: the
  processes started for the tenant, finished ones included.
- **Breaking — agentd `POST /api/v1/run` is replaced by `POST /api/v1/runs`**,
  which admits and answers `202` with the run ids instead of holding the
  request until every run ends; the run is read at
  `/api/v1/oversight/runs/{run}`. An unknown body field is a `422` — a
  misspelt `agent` started every subscribing specialist. `session_timeout_secs`
  is gone.
- makod `list_overdue_deadlines` describes what it lists: deadlines past due
  that the deadline worker has not fired yet, answered processes included —
  a worker backlog, not missed Fristen. Missed Fristen are processd
  `list_overdue_processes`.
- makod `get_process` answers a deadline-store failure with an error instead
  of an empty deadline list.
- energy-api signs API-Webdienste JWS with `p256` 0.14 (`crypto-bigint`
  0.7.10, past RUSTSEC-2026-0387).
- The agentd `sperrd-agent` skill no longer holds `list_active_processes`.
- The deadline and GaBi Gas triage rows read in German.
- agentd runs on agentplane 0.58 (from 0.47); `check-tool-grants` reads the
  manifests with the parser agentplane uses, `serde-saphyr`. A run's answer is
  read with `Runtime::output` and its tool outputs with `journal::calls`, and
  the oversight route `GET /runs/{run}/output` (`api:run.output`) is granted
  to whoever may read the run.
- **Breaking — agentd `GET /api/v1/agents/catalog`** returns `{faehigkeiten:
  [...]}`: title, purpose, active, triggers, model, every read with its reason,
  `nur_lesend`.
- The `compliance-agent` and `regulatory-reporting-agent` skills name the
  annual report the Gleichbehandlungsbericht (§ 7a Abs. 5 Satz 3 EnWG, filed by
  the Gleichbehandlungsbeauftragte); they had attributed it to the Netzzugang
  paragraph under another name. processd no longer says § 20 EnWG forbids
  auto-accepting an affiliate: holding it for a person is mako's control for
  the § 20 Abs. 1 parity comparison.
- **Breaking — vertragd `list_anmeldungen`** returns `gesamt`, every
  Lieferstelle in ANGEMELDET, beside the page it lists, and takes
  `ohne_prozess` for those no makod process carries yet.
- **Breaking — vertragd `get_kunde`** lists at most 200 Lieferstellen, newest
  first, and reports `lieferstellen_gesamt`.
- vertragd `search_kunden` matches the start of any word of a name or of the
  Kundennummer, answered from a trigram index on `kunden.suchtext` instead of
  reading every customer per keystroke.
- Desk: **Heute** takes its order from processd, a page of 50 at a time, and
  names each page with one vertragd call; the customer view reads its
  Lieferstellen's Fristen in one processd call. No view makes a request per
  row.
- **Breaking — processd `get_stp_breakdown_by_erc`** counts refusals per
  Entscheidungsbaum and Antwortcode (`{ebd, antwortcode, bedeutung, count}`);
  the same code in two trees is two rows. The `remediation` field is gone: it
  gave each code one meaning regardless of its tree, and three of the four
  were wrong for the Anmeldung tree `E_0622`.
- processd `get_decision` adds `bedeutung`, the BDEW's wording for the
  decision's Antwortcode within its tree. The tool descriptions and prompts
  state no code meaning of their own.
- The agentd `vertragd-agent` skill reads a refused Anmeldung's state from
  processd `get_process` and names no refusal code; it no longer holds
  processd `get_decision` or marktd `get_malo` (150 grants).

### Fixed

- In a Strom+Gas makod, every Sperrung PID — ORDERS 17115–17117, ORDRSP
  19116–19119 and 19128/19129, ORDCHG 39000/39001 — reached whichever of GPKE
  and GeLi Gas registered last. Both now register them per Sparte, and ingest
  routes by the recipient's Sparte.

### Security

- Dependency updates for RUSTSEC-2026-0381, -0382, -0385, -0386, -0391,
  -0398 and -0403; RUSTSEC-2026-0387 and -0435 are accepted with
  `release/vex.json` statements for the paths that remain.
