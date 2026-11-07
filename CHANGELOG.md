# Changelog

What changed for people who run, integrate or build on mako. Docs and READMEs
describe the current state; this file is the one place that records change.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The workspace version in `Cargo.toml` is the release being worked on, and the
top section collects its changes until the release replaces „unreleased" with
its date; `just bump <version>` opens the next section. mako is pre-1.0: any
release may break, and every break is listed under **Changed** or **Removed**.
History before this file is in `git log`.

## [0.27.0] — 2026-11-07

### Added

- Desk: a Rechnungsdisput is decided in Heute — Beilegen held for undo,
  Ablehnung senden (a REMADV) after the confirm — and so is a Prüfposten: a
  held invoice released after the confirm, one under review marked „Geprüft"
  on a hold. The assistant's waiting proposal covers disputes, belegt only when
  the cited check result names the finding it rests on. A disputed invoice is
  titled by its Rechnungssteller's name and Rechnungsnummer, and every finding
  by its German name (`FindingKind::bezeichnung`), never the checker's
  identifier.
- marktd `partner_namen` names up to 200 Marktpartner by MP-ID in one typed
  read; marktd's MCP contract is pinned by `mcp.snapshot.json`, and the Desk's
  gateway routes marktd.
- Desk Überblick: Dead Letters for Netz (with a new Dead-Letters list),
  Rechnungsprüfung and Rechnungsdispute for Lieferant, überfällige
  Ableseaufträge for Messstelle — each counting what its Heute source lists.
- makod's door answers every inbound message the APERAK AHB 1.1 owes an
  answer, in the same write as what it records:
  - a processed Strom message whose workflow sends none: the
    Anerkennungsmeldung (§ 2.2, § 2.4) — about thirty Strom workflows answered
    nothing;
  - a failed AHB check: `Z29` / `Z39`, every AHB-Fehler in its own `SG4 ERC`
    group with its MIG segment name as Ortsangabe (§ 2.1.1), in both Sparten;
  - a Prüfidentifikator no Marktrolle here processes: `Z31` (Kap. 4.3);
  - a follow-up no process takes, where the PID-Übersicht marks the
    Zuordnungsprüfung `V`: `Z33` (§ 2.1.3) — or, under the new policy
    `[policy.ingest] unbekannter_vorgang = "zur_pruefung"`, a dead letter for
    a person, for a cutover from another MaKo system.

  mako-engine adds `RunOptions::acknowledgement`,
  `Workflow::APERAK_AFTER_RECEIPT` and `DeadLetterSink::reject_answered`;
  `pid-overview.json` carries the PID-Übersicht's Zuordnung columns.
- `mako_fristen::APERAK_STROM_ALLGEMEIN_LABEL`: the delivery window of a
  Strom message that is not a UTILMD or ORDERS, the next Werktag 12 Uhr
  (APERAK AHB 1.1 § 2.4.1). mako-engine's `RunOptions` carries `received_at`
  and `delivery_windows`; an outbox entry its `occurred_at`.
- agentd pins its skill catalog in `catalog.snapshot.json`, so a grant, model or
  trigger added to a manifest changes the snapshot in the same commit;
  `mako_service::mcp_snapshot::compare_json` pins any served JSON contract.

### Changed

- **Breaking:** makod derives every inbound message's APERAK window in one
  place and registers it on spawn and resume — Strom always, Gas only beside a
  Verarbeitbarkeitsfehlermeldung; the GPKE, GeLi Gas and WiM workflows no
  longer register it themselves. `SupplierChangeCommand::ReceiveUtilmd` and
  `NeuanlageCommand::ReceiveAnmeldung` lose `received_at`; the adapters and
  `dispatch_dvgw` / the Redispatch entry points take the receipt instant.
- **Breaking:** agentd's waiting proposal is keyed by kind:
  `GET /api/v1/vorschlaege?freigabe=…&disput=…` answers per kind, admission is
  `vorschlag:<kind>:<id>`; desk-assistant 1.2.0 reads invoicd `get_receipt` and
  `get_check_result`.
- **Breaking:** invoicd `list_disputes` adds `abgesendet`, `rechnungsnummer`
  and `befunde` (each with `bezeichnung`, as in `get_check_result`);
  `resolve-dispute` records the caller as `dispute_resolved_by`, required beside
  `dispute_resolved_at`.
- **Breaking:** edi-energy's `Abweichungsgrund` gains `erlaeuterung`
  (`mit_erlaeuterung`), the Kopfebene's `SG7 FTX+ABO`.
- **Breaking:** billingd's `POST /api/v1/billing/{id}/reviewed` (Cedar
  `review-billing`) records the review of a REVIEW record.
- **Breaking:** makod `list_dead_letters` answers typed `{dead_letters, gesamt}`;
  `gesamt` is the total held, not the page.
- processd's `get_stp_rate`, `get_process_stp_rate` and `get_kpi_report`
  declare output schemas; the 95 % STP target is processd's operating target,
  not a regulatory one.
- **Breaking:** `missing_docs` is set once in `[workspace.lints.rust]` (a
  deny under the gate's `-D warnings`); `check-crate-lints` refuses a
  crate-level level, and only `publish = false` crates may opt out. The public
  items of `mako-markt`, `energy-billing`, `mako-pruefung`, `invoic-checker`
  and `mako-service` are documented.
- **Breaking:** invoic-checker's unused `CheckError` is removed.
- The `#[allow]` ratchet is a list of named sites with reasons instead of a
  ceiling; 24 suppressions that suppressed nothing are removed and 21 are
  `#[expect]`.
- `testcontainers` 0.27 and `testcontainers-modules` 0.15; every PostgreSQL
  suite names its image tag. RUSTSEC-2026-0440 (`astral-tokio-tar`, dev-only)
  is accepted; RUSTSEC-2025-0111 and -0134 no longer apply.
- rmcp 3.9: a tool output field that is always serialized is listed as
  `required` (nullable where it can be empty); the MCP snapshots and the
  Desk's generated types follow.

### Fixed

- The CloudEvent `time` of a notification an inbound message caused is its
  receipt instant, not the enqueue instant, so processd counts the
  Antwortfrist from the Übertragungstag (GPKE Teil 1 Kap. 7, S. 27).
- Every makod ingest arm but the GPKE supplier-change pair decides
  resume-or-spawn on its workflow's published occupancy verdict, so a second
  message for a settled process opens a new one instead of being dead-lettered
  or silently dropped (`PRESENCE_ARMS` 27 → 2).
- PARTIN, UTILTS and PRICAT messages were acknowledged on the wire and recorded
  by no process; they are keyed — UTILTS on `LOC+172`, PARTIN and PRICAT on the
  sender's `NAD+MS` — and recorded.
- A dead letter's `rejected_at` is stored as RFC 3339.
- A refusal's REMADV states why: makod writes the Ablehnungsgrund as the
  Kopf-`AJT`'s `SG7 FTX+ABO` (REMADV AHB `[548]`) — it never reached the wire —
  and every invoic-checker finding is German, written for the Rechnungssteller,
  within one DE 4440 (`an..512`), ending on a whole finding. invoicd records
  each answer before sending it, and `dispatch-answer` sends exactly that answer
  again; it sent a placeholder sentence and no Antwortcode.
- A product's tax treatment (`StromsteuerTarif`, `EnergiesteuerTarif`)
  refuses an unknown key; a mistyped one was ignored, so `{"art": "REGEL",
  "grnd": …}` read as the operator's statement. **Breaking:** `Regel` is an
  empty struct variant (`Regel {}`), the form serde's `deny_unknown_fields`
  holds for in an internally tagged enum.
- A gas invoice states its Preisgarantie and warns before it ends
  (`PREISGARANTIE_ENDET`), as an electricity invoice does; the gas engine read
  neither.
- `check-known-wrong` catches the § 12 Abs. 3 UStG PV claim with the kWp
  figure on either side; the false comments in `energy-billing` are corrected.
- billingd's review queue lists only undecided records; a released or
  reviewed one never left it.
- The Desk's gateway routes invoicd, billingd, edmd and netzbilanzd, which
  Heute reads; behind the gateway those sources answered 404.

## [0.26.0] — 2026-11-06

### Added

- The assistant's proposal waits on every Freigabe: agentd reads processd's
  open Freigaben every `[vorschlaege] intervall_secs` and admits one
  desk-assistant run per entry, keyed on its id. The Desk shows the proposal
  under the row in Heute and in the decision; `U` takes it into the form,
  unsent. `GET /api/v1/vorschlaege` reads it and starts no run;
  `automatisch = false` leaves it to a question.
- grid-billing tables the 2027 network levies as the ÜNB published them on
  26.10.2026: § 19 StromNEV-Aufschlag A′ 2,027 / B′ 0,050 / C′ 0,025 ct/kWh,
  Offshore-Netzumlage 1,143 ct/kWh, KWKG-Umlage 0,451 ct/kWh. The ÜNB published
  the § 19 Aufschlag under the § 24d EnWG reservation, which the row carries;
  a 2027 settlement priced from it warns `UMLAGE_UNTER_VORBEHALT`.
- The Desk's Heute is every Rolle's home: one list of what waits on a person,
  in Frist bands, with the assistant's Hinweis under the item it concerns,
  `⌘J` asking about the item in focus, a Freigabe decided in place, and each
  source's count or refusal beneath.

### Changed

- **Breaking:** marktd's NbEnergiemix history renders `updated_at` as
  RFC 3339.
- **Breaking:** `mako_invoic::InvoicCommand::SendInvoic` carries `nachricht`,
  the built INVOIC, and enqueues it as the `INVOIC` outbox entry; makod's
  INVOIC renderer only wraps it. edi-energy adds `Profile::seed_segment` and
  `Profile::completion` (with `Unstated`); `SkeletonParties` is re-exported at
  the crate root.
- **Breaking:** makod's INVOIC commands take `zugehoeriges_dokument`
  (`RFF+ACE` — Lieferschein, ORDERS or QUOTES), `preise_mp_id`, `rechnungstyp`
  (`IMD` codes BO4E lacks), `ausfuehrungsdatum` (`DTM+203`) and a Storno's
  `original_rechnungsdatum`; `invoice_ref` must equal the Rechnungsnummer.
- **Breaking:** netzbilanzd refuses at drafting an NN-Rechnung without
  `cadence`, and a Strom one without an Artikel-ID for every charge
  (`artikel_ids`) or without `lieferschein_ref`. Its dispatch names both
  parties and the Marktlokation from marktd (`422` naming a record marktd
  lacks); every invoice it issues — 31001, 31002, 31005, 31009, 31011 — goes
  out as a valid INVOIC.
- **Breaking:** grid-billing adds `QuantityUnit::Stueck` (the Abschlag and each
  AWH position, `QTY+47` `H87`) and `bdew_artikelnummer_code` from the Codeliste
  der Artikelnummern 5.6 Kap. 2; every position with an Artikelnummer carries its
  13-digit code, the Abschlag `9990001000376`.
- **Breaking:** grid-billing states every position net in cents, rounded once
  kaufmännisch from the stated quantity and price, and forms the totals from
  those nets (INVOIC `MOA+203` `[930]`); a Mehr-/Mindermenge is whole kWh
  (`[937]`). A monthly, yearly or daily charge is `1` `H87` with its time at
  `QTY+136`. `SettlementPosition` gains `artikel_id`, `QuantityUnit` `Tag`,
  `MsbInput` `tagespreise`.
- **Breaking:** netzbilanzd's `gas_awh` settlement requires `auftrag_ref` and
  `ausfuehrungsdatum`; its `msb` settlement takes `bezugsdokument`,
  `artikel_ids` and `tagespreise` where the 31009 column requires them.
- **Breaking:** grid-billing credits § 14a Modul 1 for the period's days — one
  `NneModul1Pauschale` position per calendar year at the pauschale over that
  year's days (`QTY+136` `DAY`, Artikel-ID `1-02-0-015` €/Tag) — and drops the
  `Jahresanteil` input; the 31002 Strom column admits no `MON`/`ANN`.
- **Breaking:** grid-billing settles one Mehr-/Mindermenge leg per document:
  the Mindermenge as 31005, the Mehrmenge as the NB's selbst ausgestellte 31006,
  stated positive as the LF's Guthaben (INVOIC AHB 1.0b Kap. 3; BDEW-AWH
  Mehr-/Mindermengen V2.1 Kap. 6.5.2). `MmmInput::selbstausgestellt` is gone, a
  period before 01.07.2018 is refused, and `invoic.mmm.stellen` sends the
  Mehrmenge as 31006.
- invoic-checker multiplies a position by its `zeitbezogene_menge` and accepts
  a net within half a cent of the product.
- **Breaking:** the event a GPKE / GeLi Gas Lieferantenwechsel emits when its
  business Antwortfrist lapses is `de.mako.antwortfrist.abgelaufen`
  (`ErpEventType::AntwortfristAbgelaufen`, processd state
  `antwortfrist_abgelaufen`), replacing `de.mako.aperak.timeout`. It was never an
  APERAK clock.
- **Breaking:** `makod_aperak_missed_total` is
  `makod_delivery_window_missed_total` and counts a missed CONTRL window too.
- **Breaking:** processd names the operator's affiliated Lieferanten in a
  required `[identity] verbundene_lieferanten` (`[]` included), replacing
  `own_mp_ids`. The NB hold on an affiliate's Anmeldung and the § 7a Abs. 5
  evidence both read it; the NB's own MP-ID never named an affiliate, since
  each Marktrolle has its own.
- **Breaking:** the § 27 Abs. 38 UStG rule moved from `energy_billing::e_rechnung`
  to `mako_fristen::e_rechnung` and takes the Gesamtumsatz in cents. Another
  electronic format needs the recipient's recorded Zustimmung; EDI goes out as
  an E-Rechnung when its format extracts into EN 16931 (§ 14 Abs. 1 Satz 6
  Nr. 2), otherwise under Nr. 1 or Nr. 3 (`edi_zulaessig`).
- **Breaking:** makod refuses an outbound INVOIC the recipient's EDI agreement
  does not carry (`422 edi_unzulaessig`); marktd's partner record holds
  `edi_zustimmung` and `edi_extrahierbar`.
- **Breaking:** billingd's `[[e_rechnung.gesamtumsatz]]` entries name the
  issuer's `ust_id` or `steuernummer`; the Nr. 2 grace reads the figure of the
  identity a record was issued under, stored on the record.
- **Breaking:** billingd classifies the recipient from vertragd's
  `rechnungsempfaenger` block, which carries `unternehmer` (from that party's
  own `kundentyp`) and the per-customer `zustimmung_sonstige_erechnung`,
  `edi_zustimmung` and `edi_extrahierbar`.
- **Breaking:** invoicd reads `check.auto_dispute_threshold_eur` as an EUR
  decimal string (`"250.00"`) and compares it exactly.
- **Breaking:** netzbilanzd `list_korrekturbedarf`, edmd
  `list_overdue_reading_orders` and invoicd `list_disputes` answer typed results
  pinned in `mcp.snapshot.json`; billingd serves `list_review_queue`.
- productd's quotation reads the Strom- and Energiesteuer of its Stichtag year
  from the dated series and refuses an unread year (`STEUERSATZ_FEHLT`).
- `check-rounding` also refuses the `billing` crate's non-kaufmännisch
  `Rounding` modes.

### Removed

- **Breaking:** invoicd's `POST /api/v1/selbstausstellen`, its
  `dispatch-selbstausstellen` action and `[edmd]` config, makod's
  `gpke.abrechnung.selbstausstellen` and mako-events' `RECEIPT_DISPATCHED`. The
  selbst ausgestellte MMM-Rechnung runs NB → LF (PID-Übersicht 4.0), so the
  Lieferant never issues it; netzbilanzd does. invoicd's receipts lose the
  `direction` column and the ERP payload its `direction` key; `invoice_ref` is
  `NOT NULL`.
- **Breaking:** edi-energy's `builders::InvoicBuilder`, which wrote the INVOIC
  header only.
- **Breaking:** a BKV can no longer answer `gabi.rechnung.*` or
  `invoic.stornorechnung.*`: the
  Kapazitätsrechnung 31010 runs NB → KN (INVOIC AHB 1.0b Kap. 3.1.4), and
  31007/31008 reach the MGV.
- **Breaking:** `mako-markt`'s 29 repository traits and its `testing` feature;
  the storage methods are inherent methods of marktd's PostgreSQL types.

### Fixed

- edmd completes a reading order and files its Zählerstand in one commit; a
  reading write that fails, or a completion refused for its unit or a missing
  register, leaves the order open instead of completed without its reading.
- accountingd answers a ledger refusal `422`, an unknown subject `404`, and only
  a store or integrity failure `500` with an `error_id`, on every ledger-backed
  route and the CloudEvent ingest. A booking into a sealed period was a `500`,
  a CloudEvent the ledger refuses was redelivered indefinitely, and a seal's
  store failure was a `422` carrying the database's text.
- marktd serves an NbEnergiemix's `eeg_einspeisung_kwh` and
  `gesamtentnahme_kwh`; every read returned them as `null`.
- outputd's `500` carries `error.error_id`, the id its log line records.
- An invoice and its answers correlate on the Rechnungsnummer: an inbound
  INVOIC is registered under `BGM` DE1004, and a REMADV or COMDIS is matched by
  `DOC` DE1004. The REMADV's `RFF+Z13` is its own Prüfidentifikator and the
  INVOIC's UNH reference numbers only the message, so no answer found its
  invoice.
- No outbound INVOIC could be issued: the six issuing commands answered
  `process_not_found` and the message lacked positions and totals. Each now
  builds, validates against its AHB and spawns from its `rechnung` — 31001,
  31002, 31005, 31006, 31009, 31011 and the Storno — refusing an invalid one at
  the command with the MIG places it misses, and a character outside UNOC;
  makod's inbound INVOIC reader takes MIG 2.8e's qualifiers.
- invoicd's monthly billing summary counts an invoice in its Berlin month; it
  bucketed by the UTC month, so a receipt after 23:00 on a month's last day fell
  into the next. `check-business-dates` now also refuses a calendar component
  read off a stored `TIMESTAMPTZ` column outside Berlin.
- marktd's MCP `get_partner` answers from the repository REST reads through; it
  decoded `makoadresse` as a string and returned every partner without an AS4
  endpoint, and its `updated_at` was not RFC 3339.
- billingd names two disagreeing Gesamtumsatz figures of one issuer when it
  refuses the paper grace, instead of calling the figure unrecorded.
- A Tarifwechsel invoice (`Invoice::merge`) derives its VAT once per rate from the
  merged lines; it summed each leg's separately rounded tax, a cent too high where
  each leg rounded up.
- A position's amount is rounded once, kaufmännisch, to the cent; it was put on five
  decimals first, so 0,004995 EUR became 0,01 EUR instead of 0,00 EUR.

## [0.25.0] — 2026-11-06

### Added

- The Desk decides Freigaben from the keyboard: <kbd>J</kbd>/<kbd>K</kbd> step
  through the list, <kbd>A</kbd>/<kbd>R</kbd> choose Freigeben or Ablehnen.
- A question typed into the Desk's ⌘K palette goes to the assistant on the
  page.
- `cargo xtask check-quotations` (in `just ci`): a „…" quotation attributed to
  a mirrored Festlegung must occur in that document. It skips without the
  `regulatories/` mirror or `pdftotext`. Seven misquotations it found in doc
  comments are corrected.
- The Desk holds a routine decision — „Hilfreich?", „Verwerfen" on a
  proposal — for five seconds behind a „Rückgängig" toast and <kbd>⌘Z</kbd>,
  sends it when the hold ends, and sends every hold at once with `keepalive`
  when the tab closes.
- When a service refuses an action with `403`, the Desk puts the service's
  reason where the action was and offers that action of that service no more
  for the tab's session.

### Changed

- processd answers a decision on an entry decided first by somebody else with
  `409` `already_decided`, naming `status`, `decided_by` and `decided_at`
  (was `404`); the Desk shows who decided and that the second decision was not
  sent.
- **Breaking:** marktd's `PUT /api/v1/zaehler/{zaehler_id}/register` and
  `PUT /api/v1/zaehler-register/{register_id}/saisons` take request bodies
  without `id`, `zaehler_id`, `tenant`, `register_id` or `updated_at`, and
  answer `200` with the stored row. A register is identified by `bezeichnung`
  and `valid_from`, a window by `saison` and `zeit_von`. `MarktdClient`'s two
  methods take `ZaehlzeitRegisterUpsert` / `ZaehlzeitSaisonUpsert` and return
  the stored record.
- **Breaking:** every `energy_billing::Product` category refuses a key it does
  not define, and `WAERMEPUMPE`, `WALLBOX` and `SHARING` carry their
  electricity supply under `strom`. A misspelt price in a posted tariff is a
  `422` instead of a product billed without it.
- Every failure a service answers carries a body: a 500 its `error_id`, a 404
  what is missing. `check-error-tuples` refuses a bare non-2xx
  `StatusCode::….into_response()` as it refuses a tuple.
  `mako_service::webhook::verify_request_only` returns the `WebhookError`, which
  converts into a `401` `ApiError` naming the reason.
- **Breaking:** a `SOLAR` product states its `stromsteuer_tarif`; there is
  no default exemption. § 9 Abs. 1 Nr. 3 StromStG is claimed as
  `{"art": "KLEINANLAGE", …}` with the plant's Energieträger, Nennleistung,
  Entnahme, distance and Erlaubnis, and refused when they do not carry it:
  above 2 MW, beyond 4,5 km (§ 12b Abs. 3 StromStV), or above the 1 MW of the
  allgemeine Erlaubnis (§ 10 Abs. 2 StromStV) without an Einzelerlaubnis.
  `StromsteuerBefreiung::Kleinanlage` is gone and `StromsteuerTarif` is no
  longer `Copy`.
- **Breaking:** productd's product body takes `abrechnung`, the billing
  settings that are not prices, typed and checked on `PUT`; billingd reads them
  from the product's `abrechnung` and refuses a tax treatment it cannot parse
  instead of billing the Regelsatz.
- **Breaking:** makod's API-Webdienste port refuses an unsigned request with
  `401` (Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3:
  „Jede API-Interaktion … wird signiert").

### Fixed

- marktd refuses a season window for a register of another tenant (`404`); it
  used to write the window onto that register.
- A productd product can carry its tax treatment and the other settings
  billingd reads (`abrechnung`); billingd read them from keys productd never
  served, so a catalog product was billed without any of them. A priced
  catalog position whose category has no field for it is logged as dropped.
- The Desk shows a service's refusal in the service's own sentence (`detail`,
  with the `error_id` where there is one) instead of the status's reason
  phrase, and tells agentd's refusal of a question or a verdict apart from
  agentd being unreachable.

## [0.24.4] — 2026-11-05

### Fixed

- The release builds its images from the binaries the builder jobs compiled:
  each pushes its stage as `mako-<stage>:<sha>-<arch>`, and the Dockerfile's
  `BUILDER_IMAGE` / `DEMO_BUILDER_IMAGE` (default: the local stages) select
  them, so the bake compiles nothing. Its cache never matched across jobs,
  and both platforms recompiled into the 60-minute limit. `mako-desk` and the
  GitHub Release ship with this release.
- `check-action-pins` judges a `FROM ${ARG}` by the arg's default: a stage of
  the same Dockerfile passes, an external image still needs its digest.

## [0.24.3] — 2026-11-05

### Fixed

- The release generates a CycloneDX SBOM of the Desk's npm tree
  (`desk_bin.cdx.json`), which `mako-desk`'s image attestation and the
  evidence pack read: without it the Desk's manifest job failed and no GitHub
  Release was created. `mako-desk` and the GitHub Release ship with this
  release; the service images ship as of v0.24.2.

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
