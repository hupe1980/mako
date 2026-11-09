# Changelog

What changed for people who run, integrate or build on mako. Docs and READMEs
describe the current state; this file is the one place that records change.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The workspace version in `Cargo.toml` is the release being worked on, and the
top section collects its changes until the release replaces „unreleased" with
its date; `just bump <version>` opens the next section. mako is pre-1.0: any
release may break, and every break is listed under **Changed** or **Removed**.
History before this file is in `git log`.

## [0.31.0] — 2026-11-09

### Added

- Configuration findings (`mako_service::befund::Befund`): every daemon
  reports every finding about its configuration — severity
  (`blockiert`/`warnung`), checklist area, TOML path, policy anchor and source.
  `<daemon> --check-config [--json]` prints them and exits non-zero on a
  blocking one; every daemon but portald serves them at
  `GET /admin/einrichtung` (Cedar `read-config`; makod `AdminEinrichtung`).
- Deployment checklist: makod folds the reports of the daemons listed in
  `[[einrichtung.dienst]]` into one Ampel per area — identities, certificates,
  Bilanzkreise, access, reachability, format coverage — at
  `GET /admin/einrichtung/deployment` and the MCP tool
  `einrichtung_checkliste`, and flags two daemons naming different MP-IDs for
  one Rolle and Sparte. `just setup` prints it for the dev configs (or `--url`
  a running makod) and exits non-zero until it is ready. Guide: Einrichtung.
- makod findings: a Gas role under a `99…` BDEW-Codenummer or a Strom role
  under a `98…` DVGW-Codenummer blocks (BDEW-AWH Identifikatoren V1.2 Kap.
  2.2); an expired AS4 certificate blocks, and one inside `[as4]
  zertifikat_warnfenster_tage` (default 30) warns.
- makod: `POST /api/v1/probe/{command}` and the MCP tool `rehearse_command`
  rehearse a command before go-live — the door runs to its pre-send check and
  returns the verdict and each UTILMD it built, segment by segment with its MIG
  names and findings. Nothing is queued, sent or stored. Rehearsable: GPKE
  55001/55004/55016 and GeLi Gas 44001/44004/44016; anything else is a `422`.
  New Cedar action `RehearseCommand`, granted to `erp-operator` in
  `conservative.cedar`.
- makod: GeLi Gas Stammdatenänderung — `geli.stammdaten-aenderung.beantworten`
  answers an inbound Änderung (the 44115 to a GNB's 44112, UTILMD AHB Gas 1.2
  Kap. 5.10.1) with a code of its Codeliste; `geli.stammdaten-aenderung.senden`
  sends the GNB's 44112 and the LF's 44109 (Kap. 5.11.3), each held against its
  profile.
- Desk: English beside German (account menu, kept in the browser) across every
  view; the market's vocabulary stays German. `cargo xtask check-desk` refuses
  a German literal in a view or a `.ts` module.
- Desk: the Einrichtung view — makod's deployment checklist by area with each
  finding's setting path, manual link and source, refreshed every 15 s — and
  the Probelauf, which rehearses a message and shows its segments with their
  AHB names and the failing data element marked.
- Desk: list–detail layout (first on Fristen überschritten), phone width with
  the sidebar as a sheet, and `/config.json` beside the bundle naming the
  operator manual a finding links to (`handbuch`).
- agentd: `tool://agentd/quote_source` and `find_source` quote a rule verbatim
  by document, edition and chapter or § from the index `cargo xtask
  index-sources [--download]` builds out of the operator's `regulatories/`
  mirror and the EnWG/MsbG (`[quellen] index`). An address the index lacks is
  refused. Belegprüfung marks a rule `belegt` only when it quotes that address.
- agentd `[policy] agent_posture` (`advisory` default / `supervised`, with
  `klassifizierung_ref`): which specialist grants refuse startup. processd and
  agentd report their settings and `allow_insecure_no_auth` in the policy
  report.
- processd `[policy] msb_vorlauf_lesart` (`additiv` default / `wortlaut`):
  whether the Fristen of GPKE Teil 2 Kap. 3.5.1.2 Nr. 3 and Nr. 4 add up; each
  Sperrauftrag served by `GET /api/v1/sperr-orders` carries its `msb_fenster`.
- processd keeps the Antwortcode a counterparty answered with
  (`antwort_code`, `antwort_codeliste`, schema edited in place); `get_process`
  and `GET /obs/processes/{id}` return it with its BDEW wording `bedeutung`,
  and the Desk's Prozess card shows it.
- `mako_service::mcp` (feature `mcp`): the MCP revisions every server speaks
  (`REVISIONS` = 2026-07-28, 2025-11-25), the Streamable HTTP `service`, and
  under `testing` the conformance round trip every MCP server's tests drive.
- Guards in `just ci`: `check-config-findings` (no `bail!`/`ensure!` in a
  `Daemon::validate` or a findings collector) and `check-mcp-surface` (MCP
  revisions, session and capability rules). `check-frist-literals` reads EnWG
  and MsbG as sources too.

### Changed

- **Breaking:** `mako_service::Daemon::validate` returns `Vec<Befund>` instead
  of `anyhow::Result<()>`; the runner refuses the boot on a blocking finding
  and logs each warning. `mako_service::einrichtung::router` replaces
  `policy::router` and serves both routes. makod reports an adapter gap as a
  finding instead of panicking.
- **Breaking:** the eleven MCP servers speak revisions 2026-07-28 and
  2025-11-25 only (an older `initialize` settles on 2025-11-25) and hold no
  session: no `Mcp-Session-Id`, `GET /mcp` is `405`, any replica answers any
  request.
- **Breaking:** makod renders every outbound process message — UTILMD,
  ORDERS, ORDRSP, REQOTE, QUOTES, ORDCHG, INVOIC, REMADV, MSCONS, IFTSTA,
  INSRPT — in the release of its process's format version; an APERAK or
  CONTRL goes out in the release in force on the day it is sent. A process
  whose release is not mirrored is refused, not rendered in today's: a
  process started before 01.04.2026 cannot render ORDERS, ORDRSP, MSCONS or
  INVOIC, whose oldest mirrored profile is fv20260401.
  `mako-engine` `OutboxMessage::format_version` carries the process's format
  version; `FormatVersion::date` reads its Anwendungszeitpunkt.
- **Breaking:** the Strom Anmeldung 55001 body is typed
  (`mako_markt::commands::GpkeLieferbeginnAnmelden`): `transaktionsgrund`
  (`E01`/`E03`), `bilanzkreis`, `haushaltskunde`, `enfg_grundlage`/
  `enfg_grund` and `kunde` are required (UTILMD AHB Strom 2.2 Kap. 8.2), and
  the rendered 55001 is held against its profile before anything is written.
  processd `POST /api/v1/start-supply` requires them too
  (`TRANSAKTIONSGRUND_UNZULAESSIG`, `ANMELDEDATEN_UNVOLLSTAENDIG`,
  `ANMELDEDATEN_UNGUELTIG`); vertragd stores `enfg_grundlage`/`enfg_grund` and
  refuses a Strom component without them or a `lieferbeginn_anlass`
  (schema edited in place). `mako-gpke` `InitiateAnmeldung` carries
  `kundendaten`.
- **Breaking:** `mako-wim` `wim-geraeteubernahme`: the ORDRSP answer follows
  ORDRSP AHB 1.1b Kap. 4.9.2/4.10 — `ordrsp_antwort_pid` takes the
  `AntwortCode` (`ZB4`/`ZB5` → 19015, `E17`/`Z07` → 19016, so a `ZB5` leaves
  the Gerätewechsel open), `DispatchAntwort` takes `AntwortAngaben`
  (Ansprechpartner, Lieferadresse für Altgeräte, Eigentümer, each refused when
  missing or not in the column), the commands carry the ORDERS'
  `orders_nummer`, and `AntwortGesendet` states `abgelehnt`/`begruendung`.
  `edi-energy` `OrdrespBuilder` gains `execution_date`, `contact`,
  `involved_party` and `delivery_address`.
- **Breaking:** `eeg-billing` Beihilfevorbehalt: a fourth state
  `Eeg2023Verlaengert` (a recorded extension of the EEG 2023 approval
  C(2022) 9900 final, SA.102084, which ends 31.12.2026) keeps EEG 2023 to the
  extension's `gilt_bis`; `Genehmigt` is `Eeg2027Genehmigt`;
  `Beihilfegenehmigung.regelung` replaces `umfang`; `Fassung::fuer_periode`
  takes the approvals from `SettleInput::beihilfegenehmigungen`. einsd's
  `[[beihilfevorbehalt.genehmigungen]]` names `regelung = "eeg2023_verlaengerung"
  | "eeg2027"` and refuses `umfang`, an extension without its end, and two
  overlapping approvals.
- **Breaking:** `mako-pruefung` `AnmeldungMsb.msb_rahmenvertrag` is an
  `Option<RahmenvertragBefund>` and `mako-markt`
  `MsbRahmenvertragPruefung.befund` a `MsbRahmenvertragBefund`; processd's
  escalation names the Befund (`kein_vertrag`, `angeboten`, `beendet`, or a Gas
  contract to adapt per GeLi Gas 3.0 Tenor 15 Satz 2) instead of always
  „missing contract".
- **Breaking:** `mako-geli-gas` `GasStammdatenCommand::SendAenderung` takes the
  changed `stammdaten` and refuses an Änderung naming none (Bedingung `[92]`).
- **Breaking:** processd `get_process` is a typed MCP tool with an output
  schema (`ProcessProjectionView`, also for `list_processes_by_family`);
  `fruehester_sperrtermin_ohne_msb_zustimmung` takes a `MsbVorlaufLesart`.
- `mako-gpke` `gpke-supplier-change`: each Anmeldung answer (55002/55003/
  55078/55080) queues a `ProcessCompleted`, so the NB's marktd moves the
  supplier on its own answer.

### Removed

- **Breaking:** makod `[engine] deployment_roles`, `--deployment-roles` and
  `MAKOD_DEPLOYMENT_ROLES`: the engine roles come from `[deployment] rollen`
  and `[[party]]`. The old key is refused.

### Fixed

- A contract-started Strom Anmeldung 55001 lacked `SG4 STS+7`, `SEQ+Z01`,
  `SEQ+Z75` and the `SG12` Kunde, all Muss (UTILMD AHB Strom 2.2 Kap. 8.2), so
  the pre-send gate dead-lettered every one.
- An answer to a process started before the 01.10.2026 cutover goes out in
  fv20251001, not in the release in force on the day it is sent.
- WiM Geräteübernahme: a Gerätewechseltermin inside the Mindestvorlaufzeit
  (WiM Teil 1 Kap. 3.1.2 Nr. 1; AWH WiM Gas 2.0 Kap. 4.1.2 Nr. 1) is answered
  with ORDRSP 19016 `E17` instead of an APERAK; the ORDRSP 19001/19002/19015/
  19016 carry `BGM` `Z10`/`Z13`, `SG1 RFF+ON`, `DTM+203` and the parties their
  AHB column requires.
- makod: an inbound ORDRSP 19001/19002/19015/19016 reaches the ORDRSP adapter
  and is dead-lettered as a registered PID with no receiver (`NotDispatchable`),
  not as „expected ORDERS message".
- The Fall-b Lieferende (`E_0624` `A34`) reaches marktd: the NB's 55002 carries
  `lfa_lieferende` on its `ProcessCompleted`, and marktd opens the § 38 Abs. 1
  EnWG gap up to the Zuordnungsbeginn.
- agentd connects to every mako MCP server on 2026-07-28 (`McpClient::connect`,
  `server/discover` first) instead of negotiating down to 2025-11-25.
- productd prompts: an Angebot defaults to today + 14 calendar days, not „10
  Werktage"; the B2B prompt cites § 312g Abs. 1 BGB, not „§41 EnWG", for the
  missing cooling-off period; the dynamic-tariff prompt cites § 41a Abs. 2
  EnWG and names no clock time for the EPEX import.

## [0.30.0] — 2026-11-09

### Added

- marktd: `GET /api/v1/msb-rahmenvertraege/check` answers whether an
  MSB-Rahmenvertrag of a Sparte between an NB and an MSBN is concluded and valid
  on a day, `befund` naming why not; `mako-markt` gains
  `MarktdClient::msb_rahmenvertrag_check`.
- marktd: `GET /api/v1/malos/{id}/lokationsbuendel?stichtag=` serves the
  Lokationsbündel on a Stichtag, edges in either direction, and
  `PUT /api/v1/malos/{id}/lokationsbuendel-angaben` stores the NB's dated
  Angaben (ÜNB, NNA positions, OBIS Verwendungszwecke, Wertegranularität,
  technische Einrichtungen, normiertes Profil).
- makod: a MSB Strom deployment receives the ORDERS addressed to it —
  `gpke-sperrung-msb` takes the Anfrage Sperrung 17116 (3 WT; a lapse counts as
  Zustimmung, GPKE Teil 2 Kap. 3.5.1.2 Nr. 4), `gpke-datenabruf-msb` takes
  17004 / 17102 / 17113 (Ablehnung 2 / 1 / 3 WT), `wim-technik-aenderung` takes
  17011 with the MSB's 10 WT.
- GeLi Gas: `geli.stammdaten.anfrage.beantworten` answers the Kap. 5.12
  Antworten — the GNB's 44142 (Stammdatenänderung), 44157 (Prognosegrundlage)
  and 44181/44182 (Marktlokationsstruktur) from marktd's record on the
  Änderungsdatum, the LF's 44138 and 44151/44152 from the facts its command
  states; the code selects the column, its PID and Codeliste, and unreadable
  master data is refused (`503` / `422`) with nothing sent.
- marktd: `malo_netznutzungsdaten` holds the Turnusablesung and the
  Netznutzungsvertrag / Zahler (`KUNDE` only under `KUNDE_NB`).
- mako-pruefung catalogues `E_3013`, `E_3026` and the Codelisten `G_0034`–`G_0036`,
  `G_0038`–`G_0040` under their own numbers.
- `mako_fristen::antwort` rows for 17004, 17102 and 17113; the Meldepflicht
  catalogue adds 55037 (LF-Zuordnung bei EEG-/KWKG-Anlagen) and 55038.
- `edi-energy`: `OrdersBuilder::leistung` and `OrdersBuilder::positionszeitraum`.
- makod counts every inbound AHB finding by class
  (`makod_inbound_findings_total`) and every Formatbedingung finding by number
  (`makod_formatbedingung_findings_total`); a Formatbedingung finding maps to
  APERAK `Z35` with the MIG segment as Ortsangabe (APERAK AHB 1.1 Kap. 2.1.2,
  4.3), and the door can refuse an AHB-Fehler per class before any workflow
  runs — no class is refused until its counts are explained.
- accountingd pays the Marktprämie by the same chain rule as the Vergütung,
  books a payout on the Kontokorrent when its pain.002 confirms it (`ACSC`,
  final), and records a plant operator's repayment
  (`POST /api/v1/eeg/rueckzahlungen`), which takes the chain off
  `GET /api/v1/eeg/rueckforderungen`.

### Changed

- edmd runs on krafka 0.28. The Kafka ingest consumer leaves its group on
  every exit, so a reconnect or the next instance takes the partitions at once
  rather than after the session timeout, and a commit refused for a rebalance
  in flight is retried after the next poll instead of reconnecting. The
  `[kafka_ingest]` keys are unchanged.
- **Breaking:** `mako_engine::payload` types (`UtilmdPayload`, `AperakPayload`,
  `MaloIdentifiedPayload` and their nested types) refuse unknown keys, and
  `PendingOutbox::new` refuses an undeclared key in debug builds
  (`payload::undeclared_key`): a key no renderer reads is no longer dropped in
  silence. `check-event-payloads` requires the attribute and keeps these types
  out of event and state enums.
- **Breaking:** `mako_geli_gas::GasAntwort` is removed; `SendAntwort { antwort_code }`
  is checked against the column's Codeliste (`ANTWORT_CODELISTEN`),
  `ReceiveAenderung` gains `vertragsbeginn`, `vorgangsnummer`, `dokumentenart`,
  `SendAenderung` gains `vertragsbeginn`.
- **Breaking:** the GeLi Gas Anmeldung 44001 is sendable: `geli.lieferbeginn.anmelden`
  reads `mako_markt::commands::GeliLieferbeginnAnmelden`, refuses unknown keys,
  requires the contract's facts (`transaktionsgrund`, `bilanzkreis`,
  `abrechnungsintervall_monate`, `haushaltskunde`, `netznutzungsvertrag`,
  `netznutzung_zahler`, `gemeinderabatt_prozent`, `kunde`, `ablesekarte`) and
  renders `IMD++Z36+Z12`, `RFF+Z18 DTM+Z20`, `CCI+++Z15/Z18`, `CCI+++Z88`,
  `SEQ+Z12`, `NAD+Z09/Z04/Z05` (UTILMD AHB Gas 1.2 Kap. 5.6), passing the
  pre-send gate before the process is spawned. processd `start-supply-gas`
  takes the facts and refuses `ANMELDEDATEN_UNVOLLSTAENDIG` /
  `ANMELDEDATEN_UNGUELTIG` / `TRANSAKTIONSGRUND_UNZULAESSIG`; an absent
  `transaktionsgrund` no longer reads as `E03`. vertragd `vertragskomponenten`
  gains `lieferbeginn_anlass`, `netznutzungsvertrag`, `netznutzung_zahler`,
  `gemeinderabatt_prozent`, and a GAS component registered at the NB is
  refused without them or without the Kunde's name and address.
- **Breaking:** `zaehlpunkt` is removed from the GeLi Gas LF Anmeldung,
  Abmeldung and Kündigung (workflow, makod, processd, vertragd): the Gas UTILMD
  names the Marktlokation alone in `SG5 LOC+172`.
- **Breaking:** marktd's MSB-Rahmenvertrag registry covers both Sparten:
  `msb_rahmenvertraege_gas` is `msb_rahmenvertraege` with `sparte` in the key
  and `nb_mp_id` for `gnb_mp_id`, the routes move to
  `/api/v1/msb-rahmenvertraege`, the Cedar actions to `read-msb-rv` /
  `write-msb-rv`, and two contracts of the same parties and Sparte cannot cover
  one day; `mako-events` `markt::MSB_RAHMENVERTRAG_UPDATED`
  (`de.markt.msb-rahmenvertrag.updated`) carries `sparte`.
- **Breaking:** processd confirms an Anmeldung MSB (55042 / 44042) only on an
  `abgeschlossen` MSB-Rahmenvertrag of the order's Sparte with the MSBN, valid
  on the Zuordnungsbeginn, and escalates otherwise — a known Marktpartner is no
  contract; a Gas contract still in Fassung BK7-17-026 escalates from
  01.10.2026 (GeLi Gas 3.0 Tenor 13–15). A deployment escalates every
  Anmeldung MSB until its contracts are loaded.
- **Breaking:** a GeLi Gas Anfrage zur Stammdatenänderung (44139/44140) opens the
  NB's process with its 10-WT window (AWH GeLi Gas V1.2 Kap. 4.4.2 Nr. 2)
  instead of being answered at once; `GasStammdatenCommand` gains
  `SendAnfrageAntwort`, and `ANFRAGE_ANTWORT_PAIRS` answers 44165 and 44166
  with 44167.
- **Breaking:** `gpke.geschaeftsdatenanfrage.beantworten` builds the 55035 from
  marktd's Stammdaten and Lokationsbündel on the Stichtag and no longer takes
  `stammdaten`; 55095 and 55060 are refused naming the groups not yet
  assembled.
- **Breaking:** `geli.datenabruf.anfragen` requires `melo_id` and
  `zeitspanne_von`, and refuses a Zeitspanne not yet ended or ended more than
  12 months ago (GeLi Gas 3.0 Kap. 4.2); the ORDERS 17103 carries its full
  Prüfschablone, `SG29 DTM+163/164` included.
- **Breaking:** `mabis-billing` closes the KBKA clearing window on its own
  deadline; the BKA close leads to `KbkaOffen`, which still takes versions
  (BK6-24-174 Anlage 3 Kap. 9.7.1).
- **Breaking:** `mako_fristen::meldung::meldepflicht` is removed for
  `meldepflichten_for`; `Meldepflicht::due_at` takes the Zuordnungsbeginn.
- **Breaking:** `GpkeModule` registers no LF or NB PID for a MSB-only role set;
  `gpke-sperrung` no longer lists 17116.
- **Breaking:** `edi-energy` `Bgm::pruefidentifikator` is removed — `BGM` DE
  1004 is a Dokumentennummer on most messages; the Prüfidentifikator is read
  through `detect_pruefidentifikator` alone.
- The GNB side of ORDERS 17103 registers `brennwert_zustandszahl_due` from the
  Anfrage's `DTM+164`; 17104 registers the 1-WT Geschäftsdatenanfrage window
  and spawns without `LOC+172`.
- The MaBiS Erstaufschlag is judged on the Berlin receipt day.
- `check-frist-literals` refuses an integer Werktage count in code;
  `check-vorlauf-consulted` ignores comments; `validate-profiles` refuses a
  `RETIRED_PIDS` entry naming the wrong release.
- **Breaking:** every processd `[[lf.bilanzkreise]]` row names its `sparte`
  (`strom` / `gas`), and every Bilanzkreis lookup — the 55607, the E/G
  Zustimmung, the start-supply default — reads only its own Sparte's rows, so
  a supplier of both never names a Strom Bilanzkreis on a Gas message or the
  reverse; startup refuses two rows for one Sparte and Bilanzierungsgebiet, an
  `eeg`/`kwkg` list on a Gas row, and an `eog_versorgungsart` outside
  `ZC9`/`ZD0`/`ZE3`.
- processd: a queued E/G Bestätigung whose Bilanzkreis, Versorgungsart or
  Abrechnungsintervall did not resolve is released with the operator's —
  `bilanzkreis`, `versorgungsart`, `abrechnungsintervall_monate` on the
  decision (REST and MCP), checked against what the segment admits; the
  resolved ones stay frozen, and the view lists the rest with their admissible
  values (`fakten_freigeben` / `fakten_ablehnen`). The answer contracts of
  `gpke.eog.bestaetigen` and `geli.eog.bestaetigen` name these facts, so a
  release without them is refused before anything reaches makod.
- Desk: a Freigabe asks for the facts processd lists, in its view and in ⌘K,
  and sends nothing until they are given.
- **Breaking:** processd's automatic Bestätigung EoG Gas 44014 carries its
  `bilanzkreis` (the `[[lf.bilanzkreise]]` row of the Marktlokation's
  Bilanzierungsgebiet) and `abrechnungsintervall_monate` from the new `[lf]
  eog_abrechnungsintervall_monate` — 1, 3, 6 or 12 (UTILMD MIG Gas G1.2) —
  refused at startup when `eog_auto_respond` is on without it; makod reads the
  44014 as `mako_markt::commands::GeliEogBestaetigen`, refuses undeclared keys
  and takes its Stammdaten from marktd; a queued E/G answer freezes the facts
  that resolved.
- **Breaking:** the outbound UTILMD answers pass their AHB profile, held by
  a sweep test: the Bestätigung EoG 44014, 55052,
  55604/55605 and 55608/55609; every Gas answer restates the Anfrage's `STS+7`
  and `RFF+TN` and names its `STS+E01` Codeliste, which every one left
  empty. 55602/55603 are refused at
  the door naming the Muss data no command holds.
- **Breaking:** the ORDERS 17001/17009 Termin is read from `DTM+203` as the
  German day it begins; processd's `wire_date::parse` takes the Sparte and
  reads a `303` stamp the same way.
- **Breaking:** accountingd's `run-payout` and the Rückforderung list are the
  NB's (EEG payouts are the Netzbetreiber's, § 19 Abs. 1 EEG 2023), no longer
  LF's/MSB's; the MCP tool `list_overdue` takes `limit`, which is what its
  `days_overdue` parameter always did.
- **Breaking:** einsd announces every decided settlement — `kein_anspruch`,
  `foerderung_beendet`, `sanctioned`, `jahreskontingent_erschoepft`, a zero
  included — and numbers each period's receipts (`ketten_folge`, on the
  event); accountingd orders a chain by that number, not the event time, and
  refuses an event without it.
- **Breaking:** `gpke.kuendigung.anmelden` and `geli.kuendigung.anmelden`
  take `lfa_mp_id` and send to that Altlieferant — the 55016 with
  `STS+7++E03` and its Lokationsart; the Gas body is
  `mako_markt::commands::GeliKuendigungAnmelden` and refuses an undeclared key.
- **Breaking:** processd's `POST /api/v1/end-supply` and `/end-supply-gas`,
  makod's `gpke.lieferende.anmelden` and `geli.lieferende.anmelden`
  (`GeliLieferendeAnmelden`) take a required `transaktionsgrund` (Strom: an
  optional `transaktionsgrund_ergaenzung`); vertragd states it from the
  Kündigungsgrund (`UMZUG` → `E01`, every other → `ZT5`).
- **Breaking:** `gpke.eog.bestaetigen` requires `bilanzkreis` and
  `enfg_grundlage` (`enfg_grund` beside `ZF9`, `[463]`) and refuses a missing
  fact by name; `EogCommand::Anmelden` takes a `vorgangsnummer`,
  `ReceiveAnmeldung` one read from `IDE+24`, `SendAntwort` the
  Gruppenzuordnung, the EnFG facts and the `SG12` Stammdaten.

### Fixed

- processd cited the Zuordnungsermächtigung by a paragraph sign; MaBiS
  (BK6-24-174 Anlage 3) has Kapitel, not §§ — it is Kap. 10.2.1, and
  `check-known-wrong` refuses the paragraph form.
- The Kündigung went to the grid operator: the GPKE 55016 and the GeLi Gas
  44016 are „LFN an LFA" (GeLi Gas 3.0 Kap. 3.1), and the Gas door resolved
  the GNB from the MaLo cache for every Anfrage. A test over the command
  registry holds every door whose AHB column reads „LFN an LFA" to the
  Altlieferant.
- The Abmeldung carried no `SG4 STS+7`, which its column makes Muss — the
  55004 with a Lokationsart Ergänzung (UTILMD AHB Strom 2.2 Kap. 5.4), the
  44004 per UTILMD AHB Gas 1.2 Kap. 5.5, `[2061]` — so the pre-send gate
  dead-lettered every one. makod takes the Strom Ergänzung from the MaLo's
  `energyDirection` unless the caller names `ZW5`/`ZAP`; under
  `ZG9`/`ZH1`/`ZH2` both name the confirmed Beginn as `DTM+92` (Gas
  Bedingungen `[7]`/`[11]`); the GPKE Abmeldung and Kündigung and every GeLi
  Gas LF Anfrage are held against their profile before they are written.
- The profile importer read a citation inside a Bedingung as that condition's
  definition: „Bedingungen [266] ∧ [479] an der Segmentgruppe …" gave `[479]`
  the rest of `[2010]`'s sentence and cut `[2006]`–`[2010]` and `[2018]` short
  in the UTILMD Strom profiles, so a valid 55004 for an erzeugende
  Marktlokation (`ZW3`) failed its own profile. A `[n]` after „Bedingung(en)",
  an operator or another citation is text; both profiles are re-imported.
- The Strom Bestätigung EoG 55014 carries every group its column makes Muss
  (UTILMD AHB Strom 2.2 Kap. 8.6, S. 124–140) and passes the pre-send gate: the
  55013's Transaktionsgrund with `ZW4`, `SG6 RFF+TN`, the Bilanzkreis as `SG8
  SEQ+Z79` Produktpaket, `SEQ+Z01` with the Gruppenzuordnung and the
  Versorgungsart `CCI+Z36` (the new `UtilmdPayload::versorgungsart`), `SEQ+Z75`
  with the Grundlage nach EnFG, and the `SG12` Kunde des LF with his
  Korrespondenzanschrift — the Gruppenzuordnung and the Kunde from marktd. The
  55014 and 55015 name no `SG5` Lokation, which their column does not list, so
  the NB finds its process by the answer's `RFF+TN`
  (`mako_gpke::eog_nb_vorgang_key`) — the 55013's Vorgangsnummer, which
  `gpke.eog.anmelden` draws and registers. The 55013 adapter reads the
  Transaktionsgrund from `STS+7` `C556`, where it rides, not DE 4405.
- processd's E/G Strom Zustimmung states its Grundlage nach EnFG: the
  operator gives it on the Freigabe as `enfg` (`ZG0`, `ZG1` or
  `ZF9+<ZU5…ZV3>`, listed with the AHB's wording in `fakten_freigeben`, the
  Desk and the MCP queue actions), and processd sends it to makod as
  `enfg_grundlage`/`enfg_grund`. No setting states it — it is a fact about the
  Kunde — so every Strom E/G Zustimmung is queued. **Breaking:** the
  `gpke.eog.bestaetigen` answer contract requires `enfg`.
- Adapters read an Antwortcode where the MIG puts it: the GPKE Rückmeldung
  reads `STS+E01` from `C556` (DE 4405 is empty, so an `A02` read as
  `Uebernommen`), and the MaBiS Ablehnung 19204 from `SG2 AJT` (an ORDRSP has
  no `STS`, so every 19204 was refused). `cargo xtask check-sts-codes` refuses
  a `status_code` read and a raw `STS` read outside the IFTSTA readers.
- The Vorgangsnummer makod draws for a Beendigung der Zuordnung fits `IDE`
  DE 7402 (`an..35`); it was 36 characters. Every door draws it through one
  helper, held against the UTILMD MIGs' bound by a test.
- The NB reads the E/G's Bilanzkreis from a 55014's Produktpaket `SEQ+Z79
  CAV+ZV4` and records it on the Zuordnung.
- edmd's Kafka ingest skips a record whose content is refused (an unknown
  `sparte`, `source` or `quality`, or an unparseable interval) and commits past
  it, as the operator guide states. Treating the refusal as a storage failure
  redelivered the record without end and held back every record behind it on
  the partition; `kafka_ingest::` in edmd's integration suite pins it.
- `check-command-constructors` read a test module kept in its own file
  (`#[cfg(test)] mod name;`) as shipped code, so a variant only a test
  constructs passed as wired; such a file is now test code, as an inline
  `#[cfg(test)]` module is.
- The GeLi Gas Antworten auf Änderung (44111, 44115, 44119, 44121, 44124,
  44161, 44176) carried no Antwortcode — it rode a payload key no renderer
  read; they send `SG4 STS+E01` with their column's Codeliste (`G_0016`,
  `G_0019`, `G_0022`, `G_0023`, `G_0025`, `G_0026`, `G_0027`, `G_0030`), the
  column's `STS+7` and `RFF+TN`, and 44176 and the outbound Änderungen send
  `DTM+92` and `DTM+157`. Fifteen more producers in GeLi Gas, GPKE, WiM and
  MaBiS wrote keys no renderer read; among them the GPKE Rückmeldung now
  sends its `A01`/`A02` and the EoG 55014/55015 its `A09` or `E_0615`
  Ablehnungscode in `STS+E01`.
- makod: the REMADV (GPKE and WiM) and INSRPT adapters read the
  Prüfidentifikator from `BGM` DE 1004, so every conformant message carrying it
  in `RFF+Z13` was refused; the 31011 adapter likewise.
- makod: the GNB answers a Gas Sperr-/Entsperrauftrag (17115/17117) within
  1 WT, not 10 — no BK7-24-01-009 Sperrprozess exists; INVOIC 31011 / 31007 /
  31008 / 31010 are due at their `DTM+265` Zahlungsziel and refused without
  one; an outbound Beendigung 17006 waits its 8 WT, not 5.
- makod: ORDRSP 19007 / 19102 / 19114 find their process by `SG1 RFF+ON`, not
  by a Lokation the ORDRSP does not carry; 19007 resumes `gpke-datenabruf`.
- marktd: the MSB-Rahmenvertrag `PUT` stores the contract and its CloudEvent
  in one transaction.
- A WiM MSB-Wechsel answer reused the order's Vorgangsnummer as its own; an
  ORDERS 17001 Geräteübernahme was always read without its Termin.
- An EEG plant account owing money back could be dunned and handed to the
  Sperr sequence; an account an EEG receipt was booked to is never an Offener
  Posten, never dunned and never in a Sperr phase.
- The door's `Z29`/`Z39` Fehlermeldung never rendered: the validator's
  typographic quotes and dashes are outside UNOC. `FTX` text is mapped into
  UNOC and capped at 512 characters.
- An EEG period re-decided lower or to nothing was never claimed back, and a
  § 52 EEG 2021 reduced Vergütung or a final KWKG month was never paid at all.

## [0.29.0] — 2026-11-08

### Added

- **The assistant at work** (Desk, agentd): a question's reads stream in as a
  trail, one line per read with its source chip, and fold into „n Quellen
  gelesen ▸"; Send becomes Stop and Esc stops the run; each answer sentence
  carries its sources' numbers and focusing one outlines the row it cites; a
  question left mid-answer rides along as a pill. agentd serves the stream
  (`GET /api/v1/runs/{run}/schritte`, Server-Sent Events read from the run's
  journal, resumable by `Last-Event-ID`) and the stop (`POST …/stopp`); no
  service is read after a stop.
- **⌘K acts on what it finds** (Desk): an identifier or the focused entry opens
  its actions — ↵ primary, ⌘↵ secondary — with their parameters in place, the
  answer code limited to its tree's codes, prefilled from the assistant's
  waiting proposal and marked as the assistant's until changed.
- `just desk-budgets` (first streamed step ≤ 1 s, ⌘K actions and a keyboard
  decision's INP ≤ 100 ms at p95) and `just desk-axe` (WCAG 2.2 A/AA, light and
  dark, reduced motion) run in the Desk's CI job.
- **The EnFG Belastungsausgleich**: einsd books the EEG/KWKG payments
  accountingd made (`ACSC`-settled payouts and Aufrechnungen) per ÜNB, month
  and EnFG head, reconciles them to the cent and produces the § 50 Nr. 1 EnFG
  Mitteilung with the § 72 EEG 2023 items, naming every item it does not hold
  (`/api/v1/enfg/ledger/{year}/{month}`, `/api/v1/enfg/mitteilung/…`).
- marktd serves a Marktlokation's UTILMD Stammdaten for a Stichtag
  (`GET /api/v1/malos/{id}/utilmd-stammdaten`) and stores the NB's
  Netznutzungsdaten; the Anmeldung EoG 55013/44013 is filled from them and
  passes its AHB profile, a missing fact refused by name.
- A GNB records the LF's answer to its own GeLi Gas request (44008/09,
  44011/12, 44014/15) with its code and tree.
- The Verlauf words every remaining answer code: the Geschäftsdatenanfrage
  refusals (`E_0441`/`E_0443` catalogued), the Listenabgleich
  Gesamtablehnung, the Weiterverpflichtung and the GeLi Gas GNB answers.
- productd's quotation and the Gas/Fernwärme invoice state the § 4 Abs. 1
  CO2KostAufG price, the invoice also the § 3 Abs. 3 amount; a quotation for a
  year whose figure is unpublished omits it and says so.
- The project's Cyber Resilience Act position is published (compliance page
  „CRA and AI Act"): outside the scope of Regulation (EU) 2024/2847, as not
  supplied in a commercial activity (Art. 2(1), 3(22), recitals 15 and 18);
  `SECURITY.md` states it and how a vulnerability is reported.
- `mako_service::json_from_value` reads a value as `Json` reads a body.

### Changed

- **Breaking:** edi-energy refuses a bare `CCYYMMDD` in a `303` DTM
  (`Error::DtmNotAnInstant`) instead of padding it to UTC midnight;
  `ReceiptContext::transmission_date` is `transmission_datetime`.
  `mako_fristen::tagesbeginn_utc` / `tag_des_beginns` convert between a German
  process date and its UTC instant per Sparte.
- **Breaking:** every Desk decision — a market message included — is held for
  undo (countdown ring, ⌘Z) and asks no confirmation, in its view as in ⌘K.
- **Breaking:** agentd records who starts a run; a run a person started is read
  and stopped only by them (`403 not_the_starter`); a run nobody started stays
  readable under `api:run.read`.
- **Breaking:** `energy-billing` refuses a billing period across a USt Stichtag
  or a BEHG year boundary, naming the Stichtage; billingd's pinned `[rates]
  mwst_rate` waives none. VPP settlements take the statutory VAT rate.
- **Breaking:** a Gas or Fernwärme invoice whose year has no recorded § 4
  CO2KostAufG price is refused (`RATE_REFUSED`) — 2027 until the UBA's figure
  is recorded. productd reads the BEHG regime from billingd's `[policy]
  behg_regime` and refuses its own key.
- **Breaking:** accountingd pays an EEG correction as the difference to what
  its chain (plant, billing month) was paid; a lower one is listed by
  `GET /api/v1/eeg/rueckforderungen`. The receipt events carry `receipt_id` and
  `korrektur_von`; `POST /api/v1/eeg/payouts/run` requires the billing period.
  accountingd refuses `auto_payout` without a valid `debtor_iban`.
- **Breaking:** a marktd Zählzeit window may cross midnight and belongs to the
  day it opens on; a SOMMER/WINTER window carries its season's days
  (`saison_von`/`saison_bis`, `MM-DD`, from the Preisblatt); the tariff zone
  resolves against the instant's day, register and season, and windows of
  different zones over one instant are a `422`.
- **Breaking:** a vertragd contract component carries its `bilanzkreis`
  (an X-type EIC for Strom) and its supply bodies are processd's request types;
  a 4xx refusal dead-letters at once.
- **Breaking:** the EoG commands take only `malo_id`, `gv_mp_id`,
  `process_date` and `transaktionsgrund` — makod reads the Stammdaten from
  marktd — and each Sparte's NB side is keyed apart (`nb-anfrage:` /
  `gnb-anfrage:{MaLo}`), so a combined NB+LF deployment runs both sides.
- **Breaking:** the GeLi EoG answer is validated against `E_3008`, and the GeLi
  answer PIDs a GNB receives route only with the `Nb` role.
- **Breaking:** `check-tool-grants` refuses any grant to a tool whose server
  does not declare `read_only_hint = true`; `check-malo-ids` refuses an
  EIC-shaped token of any length but 16.

### Removed

- **Breaking:** `mwst_rate_override` on the VPP request and the
  Aggregatorvertrag; the portal identity's `rolle`.

### Fixed

- **Every UTILMD process date left as UTC midnight**: Beginn, Ende and
  Bilanzierungsbeginn are now the UTC instant their German day begins —
  `2300`/`2200` Strom, `0500`/`0400` Gas (Allgemeine Festlegungen V6.1d
  Kap. 3.1) — and inbound ones are read back to the German date; the ESA
  REQOTE/ORDERS/QUOTES/IFTSTA dates likewise. An Abmeldung 55004/44004 was
  read with an empty date. PRICAT/UTILTS wrote a malformed `+0000` zone.
- No vertragd contract activation reached the NB: every Lieferbeginn body
  carried keys processd refuses. The Gas Lieferende sent the wrong fields.
- accountingd paid an EEG correction in full on top of the original and paid a
  negative settlement out as a positive amount.
- billingd and outputd taxed a cross-border supply as domestic; they refuse it
  (`CROSS_BORDER_SUPPLY_UNSUPPORTED`, `EXPORT_SUPPLY_UNSUPPORTED`).
- billingd credited the 2026 nEHS auction price to § 4 CO2KostAufG.
- The Desk: ↵ on a focused answer code in ⌘K sent nothing; ⌘K's search lost
  letters over views with letter shortcuts; the hold's ring never emptied; an
  Überblick tile showed „NaN" for a figure its service did not return; content
  scrolled into view could stop under the sticky top bar; the sidebar kept the
  previous view marked after navigating.

## [0.28.0] — 2026-11-07

### Added

- **Every answer explains itself**: an event that is a market answer
  states its Antwortcode and Codeliste (`EventPayload::antwort`), stamped on
  the envelope beside the `Verlauf`; makod's `get_process_history` returns
  `antwortcode`, `codeliste` and the BDEW's `bedeutung`, resolved by
  `mako_pruefung::codes::lies` through the tree the list belongs to (an `S_`/
  `G_` Codeliste included) and never from another tree; the Desk's Verlauf
  shows code and wording on the message line; desk-assistant 1.4.0 explains a
  refusal from that wording. processd's `anmeldung_decisions` keeps the
  `pruefschritt` a refusal stopped at. `check-event-verlauf` refuses a message
  event in a file that keeps an answer code and states none.

- accountingd books through **booking drafts**: every ledger entry is staged in
  `booking_drafts` in the transaction of the business change that causes it —
  a dunning case and its Mahngebühr, a bank row with its `bank_import_log` entry
  and event, a Jahresabschluss and its refund, a SEPA reversal, an interest
  charge, an Abschlagsforderung and its Verrechnung — and recorded after the
  commit; a recovery worker books what a crash left open, exactly once. A
  booking the ledger refuses for good is **parked** with its code, listed by
  the MCP tool `list_parked_bookings` and the Lieferant's Heute in the Desk, and
  resolved by `POST /api/v1/bookings/{key}/rebook` or `/discard`. In the Desk
  a geparkte Buchung reads in plain German with its Beleg and refusal, is
  rebooked on the date the service suggests or discarded with a Grund, each
  confirmed, and counts on the Überblick.
  `cargo xtask check-ledger-drafts` refuses a ledger write outside a draft and a
  cent amount formatted through `f64`.
- `[database] migration_url`: the runner migrates as the owning role and grants
  `url`'s role data access without DDL. accountingd grants its runtime role
  exactly doubleentry's list, so the journal cannot be rewritten by the process
  that writes it. The dev and o2c configs run accountingd as `accountingd_app`;
  **recreate the dev database volume** so `dev/init-db.sh` creates the role.

- The assistant is in every decision: a parked booking gets a waiting proposal
  like a Freigabe and a Rechnungsdisput — `neu_buchen` on the first open day,
  or `verwerfen` when accountingd's ledger shows the same Beleg already booked
  (`list_ledger` lines carry their `beleg`; MCP `get_parked_booking`). Every
  Heute item with an Anker answers „Warum?" in place. agentd's catalog reports
  each skill's last 30 days from agentplane's track record (`bilanz`), shown on
  the Beipackzettel, beside every proposal and as an Überblick tile — a share
  only from five verdicts.

### Changed

- **Breaking:** `EventEnvelope`/`NewEvent` carry `antwort`; the LF Anmeldung
  workflows' `HandleAntwort` and `AntwortReceived` (`gpke-lf-anmeldung`,
  `geli-gas-lf-anmeldung`) carry the inbound `STS+E01` as `antwort`, and their
  `ProcessCompleted` payload adds `antwortcode` and `codeliste` (Gas also
  `reason`).
- **Breaking:** accountingd's ledger is doubleentry 0.9 in schema `ledger`
  (0.7 used `doubleentry`; no migration — 0.9 refuses an older ledger by
  name).
  Periods are calendar months from 2015-01; `POST /api/v1/periods/{id}/seal`
  takes no body and seals every open month through `id`; `GET /periods/seals`
  reports `verified` (a full audit) instead of `chain_valid`; the proof routes
  return doubleentry 0.9's proof shapes; a clearing is one matched pair, a
  Zahlungszuordnung's pairs are cleared in one transaction, and a clearing is
  reset by its position (`POST /api/v1/clearings/{position}/reset`). A ledger
  refusal answers `422` with code `ledger.<n>` (`409` for a reused key), an
  unavailable ledger `503`.
- **Breaking:** `Daemon::migrate` takes the daemon's config.
- **Breaking:** Rust **1.95** (meterstore 0.16's MSRV); `rust-toolchain.toml`,
  the workspace `rust-version` and the builder image move with it.
- sqlx **0.9**, one workspace dependency (rustls on aws-lc-rs, the provider
  meterstore and doubleentry use). SQL built at runtime goes through
  `mako_service::sql::dynamic`, the one `AssertSqlSafe`;
  `cargo xtask check-dynamic-sql` holds every file to its listed count and
  reason.
- edmd on meterstore **0.16** (DataFusion 54, iceberg 0.11): meterstore opens
  its own pool (`edmd-meterstore`, 10 connections — count it against
  `max_connections`). A GDPR erasure commits edmd's own rows first and then
  destroys the subject mapping in meterstore's transaction; a failure between
  the two leaves the request recorded and re-running completes it.
- agentd on agentplane 0.60: a skill's track record (runs by outcome, each
  person's latest verdict per metric) under
  `/api/v1/oversight/capabilities/{capability}/track-record`, granted to the
  roles that give verdicts. **Recreate an existing agentd PostgreSQL store**:
  agentplane adds two tables.
- accountingd: an Abschlag Verrechnung reads which advances were received
  before its own credit is booked, so an unpaid advance is no longer stamped
  as absorbed; every money amount in an event or MCP answer is formatted from
  integer cents.

- `check-known-wrong` also refuses a spec requirement or success-criterion id
  and a milestone named in parentheses or after „spec" in a tracked file.
- **Breaking:** `mako_bo4e_decimal_from_json_number_total` is
  `mako_bo4e_fractional_json_number_total`, counted by the BO4E gate
  (`mako_markt::bo4e::decode`) instead of `rubo4e`: it counts the fractional
  JSON numbers an inbound BO4E payload carries, on every daemon that decodes
  through the gate. makod no longer exports it; its BO4E reads do not cross
  the gate.
- `rubo4e` 0.15 (no `sqlx` feature: marktd binds identifiers as TEXT and
  parses them back) and `en16931` / `en16931-formats` 0.10: a missing required
  term in a received e-invoice is fatal, and sub-invoice lines, CII payment
  means and BT-89 are read and written in full.

### Fixed

- makod read a GeLi Gas answer to the LFN by the wrong PIDs: 44003 (Ablehnung
  Anmeldung) counted as accepted and 44002 (Bestätigung) as refused, so a
  refused gas Anmeldung activated supply; 44017 was refused as well. Acceptance
  is `LF_ANMELDUNG_BESTAETIGUNG_PIDS` (44002, 44005, 44017), held by a test
  that renders, parses and adapts both answers.

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
