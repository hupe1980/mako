# Copilot Instructions — mako

## Project Overview

Rust workspace implementing an end-to-end pipeline for German energy market
communication (MaKo / BDEW EDI@Energy). Two distinct concerns:

- **`edi-energy`** — EDIFACT parsing, validation, and schema layer (stateless library)
- **`mako-engine`** — event-sourced process runtime for long-running MaKo workflows

---

## Workspace Structure

```
crates/edi-energy/        EDIFACT parse/validate/schema — stateless library
crates/mako-engine/       Event-sourced runtime (EventStore, Workflow, Process, …)
crates/mako-gpke/         GPKE — UTILMD Strom (55001–55018, 55022–55024, 55555, 55607–55609) + INVOIC (31001, 31002, 31005, 31006) + ORDERS Sperrung (17115–17117) + ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002) + PARTIN Strom (37000–37006)
crates/mako-wim/          WiM Strom — Messstellenbetrieb (55039, 55042, 55051, 55168) + ORDERS Geräteübernahme (17001–17011, 19001/19002 nMSB role) + Stammdaten (StammdatenUebermittelt carries standorteigenschaften+zaehlwerke for marktd auto-update) + Preisanfrage/REQOTE (35001–35005) + Preisliste/PRICAT (27001–27003) + INVOIC 31009 + INSRPT (23001, 23003, 23004, 23008) + Stornierung + Technik-Änderung + iMS Steuerungsauftrag
crates/mako-geli-gas/     GeLi Gas 3.0 — UTILMD G (44001–44021) + LFN-side Lieferbeginn workflow (`geli-gas-lf-anmeldung`, PIDs 44001 outbound + 44003/44004 inbound) + UTILMD G Stornierung role-conditional (44022 Nb-only, 44023/44024 Lf-only) + ORDERS Sperrung Gas (17115–17117, LF-role `geli-gas-sperrung-lf` + GNB-role `geli-gas-sperrung-nb`) + ORDERS Datenabruf (17103/17104 + ORDRSP 19103/19104, `geli-gas-datenabruf`) + PARTIN Gas (37008–37014) + INVOIC 31011 (AWH Sperrprozesse Gas)
crates/mako-mabis/        MABIS — PID 13003 (Bilanzkreisabrechnung Strom, BKV↔ÜNB)
crates/mako-wim-gas/      WiM Gas — UTILMD G (44022–44024 + 44039–44053, 44168–44170) + INVOIC (31003, 31004) + INSRPT Gas-only (23005, 23009)
crates/mako-gabi-gas/     GaBi Gas 2.0 (BK7-14-020) — INVOIC 31010 (Kapazitätsrechnung) + INVOIC 31007/31008 (Aggreg. MMM-Rechnung Gas, NB → MGV) + MSCONS 13013 (Allokationsliste Gas, MMMA) + ORDERS 17110/ORDRSP 19110 + DVGW workflows ALOCAT/NOMINT/NOMRES/SCHEDL/IMBNOT/TRANOT/DELORD/DELRES (8 workflows); typed gas domain: `GasDay` (DST-aware 06:00 CET per DVGW G 2000), `GasQuantity` (Decimal kWh_Hs), `GasBeschaffenheit` (Hs + Zustandszahl, DVGW G 685), `AllocationVersion` (Initial/Correction/Final per KoV §6.4), `GasMarketRole` (BKV/FNB/VNB/MGV/LF/Händler), `GasPortfolioBalance` (BKV portfolio across Bilanzkreise), `GasImbalanceSaldo` (Mehr/Minder/Balanced)
crates/dvgw-edi/          DVGW EDIFACT formats — ALOCAT, NOMINT, NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD, DELRES
crates/mako-nbw/          Netzbetreiberwechsel — PARTIN bulk DSO handover [placeholder]
crates/energy-api/        BDEW API-Webdienste Strom REST/WebSocket client+server
crates/mako-as4/          BDEW AS4-Profil v1.2 — `BdewAs4Profile`, `bdew_pmode()` (sign+encrypt, X509PKIPathv1, BrainpoolP256r1), `bdew_pmode_sign_only()`, `bdew_push_policy()` (require_encrypted_inbound), `WsSecOutboundKeyInfoProfile`, `BdewAction` enum (**16 standard variants** + `Custom`: UTILMD, APERAK, CONTRL, MSCONS, INVOIC, REMADV, IFTSTA, ORDRSP, ORDERS, ORDCHG, REQOTE, INSRPT, PRICAT, QUOTES, PARTIN, UTILTS), `Display`/`FromStr` impls on `BdewAction`, `bdew_action_from_str()` free fn, `ParseBdewActionError` uninhabited, `PartnerDirectory`, per-partner encryption cert registry; testing feature: `BdewTestPki` + `generate_self_signed_bdew_keypair()` + `MockAs4Endpoint` (delegating to asx-rs v0.7 `generate_self_signed_ec_keypair(EcCurve::BrainpoolP256r1)` + `mock_endpoint::MockAs4Endpoint`); constants: `SIG_ALGO_ECDSA_SHA256`, `ENC_KEY_AGREEMENT_ECDH_ES`, `ENC_KEY_DERIVATION_CONCAT_KDF`, `ENC_KEY_WRAP_AES128`, `ENC_CONTENT_AES128_GCM`; asx-rs v0.7 auto-detects ECDSA vs RSA from key type and ECDH-ES vs RSA-OAEP from cert type
crates/mako-redispatch/   Redispatch 2.0 [placeholder]
crates/redispatch-xml/    Redispatch 2.0 XML/XSD format parsing
crates/mako-markt/        Market data library — MaloId, MeloId, MarktpartnerId, repository traits, AppState, CloudEvents, VersorgungsStatus, PriCatRepository, MaloGridRecord/MaloGridRepository, PreisblattMessungRepository, SteuerbareRessourceRepository, DeviceRepository (Zaehler/Geraet), testing feature
crates/mako-edm/          Energy data library — MeterDataReceipt, TimeSeriesRepository (with obis_code), ImbalanceReport, MSCONS PID set
crates/mako-obs/          Observability library — ProcessProjection, KpiReport, DeadlineRisk, ProcessProjectionRepository
crates/invoic-checker/    INVOIC plausibility library — 6 checks: period validity, position arithmetic, document total, tariff match (ToU-aware: HT/NT position text classifies against zeitvariablePreispositionen band prices), tariff found; check 6 = MMM settlement price check (PIDs 31002/31005/31007/31008 vs. marktd MMMA store, `InvoicCheckEngine::check_mmm_settlement()`)
crates/netz-checker/      NB Anmeldung validation library — pure, deterministic 6-check pipeline; ERC codes A02/A05/A06/A97/A99; no I/O; used by processd NB module
crates/mako-service/      **Service SDK** — cross-cutting infrastructure shared by all 17 daemons. Modules: `config` (load_config, DatabaseConfig, HttpConfig, TOML+env+_FILE layering), `shutdown` (token() SIGINT+SIGTERM, serve() graceful drain), `oidc` (OidcConfig, OidcVerifier, build_verifier() factory, looks_like_jwt routing), `mcp_auth` (McpAuth, McpAuthConfig, McpApiKey SecretString, McpIdentity, from_auth_config[_oidc]() factories), `telemetry` (init_tracing_from_env, init_tracing, OtelConfig), `cedar` (CedarEnforcer), `health` (health_routes), `http` (default_client 5s+30s timeouts), `webhook` (verify_signature HMAC-SHA256), `builder` (ServiceBuilder), `event_bus` (EventBus, WebhookBus), `metrics`, `rate_limit`, `mako-plugin` (PluginRegistry)
crates/grid-billing/       Role-neutral **grid settlement engine** — NNE/KA/MMM/MSB/AWH for PIDs 31001/31002/31005/31006/31009/31011; `calculate_nne_invoice`, `calculate_mmm_invoice`, `calculate_msb_invoice`, `calculate_gas_awh_invoice`, `calculate_reversal`, `calculate_correction`; §14a Modul 1 (flat reduction, `sect14a_modul1_reduction_factor`) + Modul 2 HT/NT ToU + Modul 3 (modeled); Gas Grundpreis (`nne_grundpreis_eur_per_month`/`nne_grundpreis_months`); returns **`GridSettlement`** (canonical output — **no rubo4e dep**; `GridInvoice` = backward-compat alias); every `InvoicePosition` carries `CalculationTrace { explanation, legal_refs: Vec<LegalReference>, tariff_source: Option<TariffSource>, gross_eur, … }` + **`kind: BillingPositionKind`** (bridges to `BdewArtikelnummer` in service layer) + **`artikel_id: Option<String>`** (BDEW Artikel-ID for AWH Gas 2-01-7-xxx and NNE Strom from tariff sheet); **`BillingPositionKind`** (15 variants, rubo4e-free); **BDEW Codeliste Artikelnummern und Artikel-ID v5.6** (valid 01.09.2025): NNE Gas/MMM/KA use classic `artikelnummer` (9990001…); NNE Strom (BK6-20-160) uses `artikel_id` from PreisblattNetznutzung; AWH Gas (PID 31011) uses `artikel_id` 2-01-7-001..006; service-layer `kind_to_artikelnummer()` maps BillingPositionKind → BdewArtikelnummer; `Sparte { Strom, Gas }` drives `LegalReference` (StromNEV §21 vs GasNEV §14) + `SettlementType` + default PID automatically; **`Sect14aModule { Modul1, Modul2, Modul3 }`** typed in `LegalReference::Sect14aEnwg`; **`SettlementType::MmmGas`** separate from `MmmStrom` for correct GasNZV §14 legal refs; `KaKlasse { TarifkundeLow, TarifkundeMedium, SonderkundeHigh, Exempt }` annotates KAV §2 tier; `QuantityUnit { Kwh, Kw, Kvarh, Kvar, Monat }`; `counterparty_mp_id` auto-populated from input (lf_mp_id or msb_mp_id); `LegalReference` has 12 variants incl. `ARegV`; `validate_nne_input`/`validate_mmm_input`/`validate_msb_input`/`validate_gas_awh_input` → `ValidationResult`; **partial HT/NT validation** (`PARTIAL_TOU_FIELDS`); **Modul1+Modul2 mutual exclusion** (`MODUL1_AND_MODUL2_CONFLICT`); `debug_assert_eq!(total_eur, recomputed_total())` inside all `calculate_*`; service layer (netzbilanzd/invoicd) owns `into_rechnung()` conversion; no float money; invoic-checker validation lives in netzbilanzd, not in this crate.
crates/eeg-billing/       Pure EEG/KWKG feed-in settlement library — `calculate_settlement()` for all 9 settlement schemes (`SettlementScheme + TariffSource`); EEG-version-aware rules via `EegGesetz` enum (Eeg2000–Eeg2023+Kwkg); §51 Negativpreisregel with Bestandsschutz (§100 Abs. 1 Satz 4 EEG 2017 boundary 2016-01-01; EEG 2017: 6h/Wind<3MW/other<500kW; EEG 2021: 4h/<500kW; EEG 2023: any/<100kW); **§20 Abs. 3 Managementprämie** incorporated into AW before spread (EEG 2023 correct — NOT a separate floor payment); **§19 EEG Einspeisemanagement compensation** (`einspeisemanagement_kwh` field — §51 does not apply per §19 Abs. 2); **§36k Wind Korrekturfaktor** (`wind_standort: Option<WindStandort>` or `wind_korrekturfaktor: Option<Decimal>` — explicit wins; `WindStandort` auto-derives); §52 Pflichtzahlungen via `SanktionsTyp` (€10/kW or €2/kW per §52 Abs. 3 Nr. 2) + `SanktionAlt` (3-tier old-regime) + §52 Abs. 6 Netting via `ReductionPipeline`; §23a quarterly solar PV degression via `degression::Quarter + DegressionTier`; §§20-22 Direktvermarktung rules via `direktverm::is_direktvermarktung_mandatory()` + `requires_ausschreibung()`; multi-meter Messkonzept via `metering::MeterConfiguration + compute_einspeisemenge()` (§42b GGV, §14a HT/NT); `SettlementPeriodState` lifecycle (Active/Reduced/Suspended/PostEeg/Ended) via `settlement_state::derive_settlement_state()`; `RepoweringScope` (Full/RotorOnly/NacelleAndRotor/TurbineUnit — only Full resets Förderdauer); §53 Vergütungsabzug helper (0.4ct solar/wind, 0.2ct biomasse/gas); §54 Ausschreibungsreduzierung; `foerderendedatum_eeg()` = Dec 31 of year+20 (§25 Abs. 1 Satz 2); `ErzeugungsArt` enum (19 variants); `post_eeg_price_floor: Option<Decimal>` (configurable — negative EPEX not a statutory rule); uses `billing 0.6.0`: `LineItem::for_usage_rounded(scale=6, MidpointAwayFromZero)` for BO4E 6dp compliance, `EuroAmount::to_decimal()` for non-consuming conversion, `Amount::<5>::ZERO` in bridge; 301 tests; zero float money; no I/O; used by einsd
crates/meter-quality/     Pure Hampel-filter quality scoring for time-series meter data — hampel_filter(), score_intervals(), QualityGrade A/B/C/F; no I/O; 15 tests
crates/metering/          German energy metering domain library — `MeterInterval`, `Sparte`, `QualityFlag` (8 variants: Measured/Estimated/Substituted/Calculated/Corrected/Preliminary/Faulty/Unknown, billability per §17 MessZV); Gas m³→kWh_Hs (§24 GasGVV / DVGW G 685); billing period aggregation (Spitzenleistung §2 Nr. 17 MessZV, HT/NT split with DST-correct CET/CEST via time-tz); SLP/RLM/iMSys classification (§3/§4 MessZV, §41a EnWG); Mehr-/Mindermengensaldo (§27 MessZV); Hampel quality scoring (QualityGrade A/B/C/F); **§17 MessZV substitute value generation**: `fill_gaps()` (auto: linear for short gaps, carry-forward for long) + `fill_gaps_with_config(intervals, secs, from, to, &FillGapsConfig)` for explicit method selection; `FillGapsConfig { method: SubstituteMethod, prior_period_intervals, short_gap_threshold }` — supports `LinearInterpolation`, **`PriorPeriodAverage`** (same time-slot from prior-week data per §17 Abs. 2 MessZV), `ZeroFill`, `LastValueCarryForward`; **V01–V10 validation engine** (`validate_intervals()` — Gap/Overlap/NegativeEnergy/ImpossibleSpike/ZeroRun/InconsistentInterval/DstAmbiguity/FutureTimestamp/NonBillableQuality/RegisterRollover); **`resample(intervals, ResampleConfig)`** — hourly/daily/monthly/yearly down-sampling with `ResampledBucket` (total_kwh, peak_kw, coverage_pct, has_missing_data); **`compute_virtual_meter(rule, sources)`** — Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation per §42b EnWG GGV (Solarpaket I); **`project_annual_consumption()`** — §17 MessZV Jahresprognose with optional prior-year seasonal correction; **`prior_period_substitutes()`** — §17 Abs. 2 MessZV prior-period gap-fill with audit trail; **`SmgwSession`** (BSI TR-03109 gateway lifecycle, `has_valid_tls_cert()`, `expiring_certificates()`); **`ClsChannel`** (§14a EnWG CLS, `is_section_14a_compliant()`); **`MeasurementPoint`** (MaLo+MeLo+OBIS+`MarktRolle` binding); **`MeterRegister`** (Wandlerfaktor, HT/NT, active/inactive); **`PowerQualityInterval`** (DIN EN 50160: `voltage_out_of_range()`, `power_factor_below_threshold()`, `has_quality_issue()`); **`AggregationRule`** (Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation); zero I/O, no async, no float money; 214 tests
services/makod/           Production daemon — assembles all modules
  services/makod/src/mcp_server.rs  MCP server (8 tools, 6 prompts, malo:// + partner:// resources) at /mcp
services/marktd/         Market Data Hub — MaLo/MeLo/NeLo/TR/SR (typed `rubo4e::current` API responses for Marktlokation/Messlokation/Zaehler/Geraet; schema-validated on PUT); NB contracts with full BO4E `Vertrag` JSONB (`vertragsart`/`vertragsstatus` as indexed columns; `de.markt.nb-contract.updated` CloudEvent); Lokationszuordnung graph (lokationszuordnungen table, recursive-CTE BFS), preisblaetter, VersorgungsStatus, event_log replay, W3C traceparent forwarding; **konfigurationsprodukte** typed sub-resource on SteuerbareRessource (mandatory `produktcode` per BK6-24-174 §4.3, `de.markt.sr.konfigurationsprodukt.updated`); **MMMA import worker** (monthly auto-import of Gas/Strom MMM settlement prices, configurable URL, `de.markt.mmma.*.imported` events); **ZeitvariablePreisposition** validation on PreisblattMessung PUT (mandatory `zaehlzeitregister`, rejects `bandNummer`); PostgreSQL, OIDC/JWT, port :8180; **pure data hub — no domain policy**
  services/marktd/src/pg/      PostgreSQL implementations (PgLokationszuordnungRepository, PgTechnischeRessourceRepository, PgSteuerbareRessourceRepository, PgDeviceRepository, upsert_versorgungsstatus, PgPriCatRepository, PgMaloGridRepository, PgPreisblattKaRepository, PgZaehlzeitRepository); zaehler_register + zaehler_saisons tables defined in 0001_initial.sql; **ZaehlzeitRegister auto-populated from WiM Stammdaten (PIDs 17102–17133)**: `event_ingest` handler receives `de.mako.process.completed` from makod, looks up Zaehler by MeLo, upserts register + saison rows (deterministic UUID from register_id+saison+tagtyp+zeit_von for idempotency)
services/processd/        Process Decision Engine — NB Anmeldung STP (netz-checker) + LF E_0624 auto-response (gpke.nb-lieferende.bestaetigen/ablehnen) + LFN bootstrap Strom (POST /api/v1/start-supply, LFW24 Vorlauffrist 15:00 validated) + Gas (POST /api/v1/start-supply-gas, geli.lieferbeginn.anmelden) + Gas stornierung (geli.gas.stornierung.initiieren) + **MSB-Wechsel STP** (evaluate_msb_anmeldung/kuendigung, PIDs 55039/55042, 5 checks, ERC A02/A05/A97, escalate-on-iMSys/SR/no-zaehler) + **REQOTE auto-response** (PIDs 35001–35005, auto-fetches PreisblattMessung, dispatches QUOTES; `[msb] auto_preisanfrage` flag) + **§14a Steuerungsauftrag** (produktcode contract check against konfigurationsprodukte, auto-bestaetigen/ablehnen per BK6-24-174 §4.3); role-gated features (lf-only/nb-only/integrated); §20 EnWG parity; port :8580
  services/processd/src/nb_module.rs  NB STP evaluation (wraps netz-checker)
  services/processd/src/lf_module.rs  LF E_0624 auto-response
services/invoicd/         INVOIC plausibility-check daemon (LF role) — PIDs 31001/31002/31005/31006 (Strom GPKE, embedded Rechnung); 31003 (WiM Gas Rechnung, NB→LF); 31004 (WiM Gas Stornorechnung, auto-accept arithmetic-only); 31007/31008 (GaBi Gas MMM-Rechnung + MMM check 6 Gas); 31009 (WiM MSB-Rechnung, `InvoicCheckEngine::check_msb_rechnung()` + `PreisblattMessung`); 31011 (GeLi Gas AWH Sperrprozesse); auto-settles/disputes; persists receipts to PostgreSQL with indexed `malo_id` column (§22 MessZV 3-year); `POST /api/v1/selbstausstellen/{malo_id}` (31006); `POST /api/v1/receipts/{id}/dispatch-remadv` (manual); `POST /api/v1/receipts/{id}/resolve-dispute`; `GET /api/v1/receipts/{id}/rechnung`; payment CloudEvents (de.invoic.receipt.settled/disputed) via `[erp] webhook_url`; check 6 Strom MMM uses MMMA Strom prices; check 6 Gas MMM (31007/31008) uses MMMA Gas prices (THE); **MCP: 7 tools, 4 prompts**; port :8280
  services/invoicd/src/pg/      PostgreSQL receipt persistence (upsert_receipt, mark_dispatched)
services/netzbilanzd/     NNE/KA/MMM/MSB/AWH billing daemon (NB role) — uses grid-billing to generate INVOIC 31001/31002/31005/31009/31011; invoice_drafts + kostenblatt_records + fremdkosten_records tables; draft status: draft→dispatched→paid|Dispute, draft→rejected; POST /billing/run (positions batch), PUT /dispatch, PUT /reject, PUT /mark-paid (REMADV 33001), PUT /mark-disputed (REMADV 33002); POST /webhooks/remadv (CloudEvent ingest); GET /billing/summary + GET /billing/audit (§22 MessZV BNetzA export); POST /billing/mmm-run/{malo_id} (auto-fetches edmd imbalance + marktd MMM prices); POST /billing/ggv-nne/{ggv_malo_id} (§42a GGV NNE); redispatch kostenblatt (BK6-20-061 §4.2, 15th-of-month submit); Fremdkosten typed BO4E REST; Strom MMM auto-fetch when unb_mp_id configured; background workers: hourly dispatch-overdue alert + daily Kostenblatt deadline alert; CloudEvents: de.netzbilanz.invoic.{drafted,dispatched,paid,disputed,dispatch_overdue} + de.netzbilanz.kostenblatt.deadline_approaching; 13-tool MCP server + 6 prompts at /mcp; double-billing prevented by partial UNIQUE index; port :8680
services/sperrd/          Sperrung execution tracking daemon (NB role) — sperr_orders table (pending → executed/failed/cancelled); POST/GET sperr-orders, PUT /execute → auto-dispatches IFTSTA 21039, PUT /fail → operator escalation, PUT /cancel → operator cancellation, GET /stats → compliance snapshot (`overdue_pending`, `executed_missing_iftsta`); `?older_than_hours=N` filter for stuck-order detection; tenant column isolation (migration 0002); `iftsta_dispatched_at` SLA tracking; 5-tool MCP server (list_sperr_orders, get_sperr_order, get_sperr_stats, list_overdue_orders, cancel_sperr_order); GPKE BK6-22-024 compliance; port :8780
services/einsd/           Einspeiser Registry + EEG/KWKG Settlement daemon — delegates to crates/eeg-billing; eeg_anlagen register (EegGesetz-typed, 19 ErzeugungsArt variants, **8 DB migrations**); **9 settlement schemes** (SettlementScheme + TariffSource: FEED_IN_TARIFF, MARKET_PREMIUM, TENANT_ELECTRICITY, POST_EEG, EIGENVERBRAUCH, KWK_SURCHARGE, FLEXIBILITY_PREMIUM, FLEXIBILITY_SURCHARGE, TEMPORARY_FEED_IN_TARIFF); **§20 Abs. 3 Managementprämie** correctly incorporated into AW (not a floor); MaStR lifecycle: `mastr_registriert` column + `angemeldet`→`aktiv` status + SEPA CT bank columns; **§52 violation start tracking** (`mastr_violation_start`, `fernsteuerbarkeit_violation_start` — auto-set on registration, cleared on confirmation); `foerderendedatum` = Dec 31 year+20 for statutory, exact 20y for Ausschreibung; §51 version-aware rules via EegGesetz; §52 SanktionAlt (old EEG) + SanktionsTyp Pflichtzahlung (EEG 2023, cumulative from violation start) + §52 Abs. 6 Netting; **§53b regional reduction** (`sect53b_reductions` table, per `grid_area`); **§54 Ausschreibungsreduzierung** (`sect54_reductions` per plant); **§19 EInsMan compensation** (`einspeisemanagement_kwh` settle request field — §51 exemption per §19 Abs. 2 EEG 2023); **§21b Veräußerungsform Wechsel** (`POST /switch-veraeusserungsform` — monthly guard + mandatory threshold); **§22 MessZV correction receipts** (`POST /settlements/{y}/{m}/correction` — `correction_of` + `settlement_receipt_history` snapshot); **derive_settlement_state** called after every settlement to drive `settlement_state` column; §23a quarterly degression; §36k Wind Korrekturfaktor; multi-meter Messkonzept (`meter_config` JSONB, `metering_mode`); `SettlementPeriodState` lifecycle (`settlement_state` column, `settlement_state_transitions` table); `DirectvermarktungsPerioden` JSONB + `direktvermarktung_pflicht` flag; `RepoweringScope` (Full/Partial — only Full resets Förderdauer); `capacity_blocks` JSONB for §24 Erweiterung (deserialized and passed to eeg-billing); §55 Pönalen schema (`zuschlag_erloeschen_datum`, `award_expired`); Repowering §22 + Zusammenlegung §24; KWKG Förderdauer; monthly auto-settle + batch settle; edmd auto-fetch; MCP /mcp (14 tools, 6 prompts); **`check_direktvermarktung_compliance`** (§3 Nr. 1 + §20 EEG: lists plants >100 kW settled under non-market scheme, §52 Abs. 2 Nr. 4 violation risk); **`check_sect44b_quota`** (§44b EEG 2023: annual biogas cap = leistung_kw × 0.45 × 8760 kWh, YTD tracking, alert at 75%/90%); eeg-agent + eeg-compliance-agent in agentd (triggers: `de.eeg.*` + `de.edmd.reading.direct.stored` for iMSys rollout detection; eeg-compliance-agent covers §52/§44b/§20 EEG compliance checks); port :9180
  services/einsd/migrations/ 0001_initial.sql (eeg_anlagen + eeg_verguetungssaetze + settlement_receipts), 0002_mastr_bankverbindung.sql (mastr_registriert/nummer/datum, bank_iban/bic/zahlungsempfaenger, angemeldet status, eeg_gesetz CHECK constraint)
services/tarifbd/         Product & Tariff Catalog daemon (LF role) — user-defined energy products (STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE), all prices in Tarifpreisblatt JSONB + version history, customer_products (MaLo→product assignment), epex_prices (hourly §41a day-ahead), Energiemix §42 EnWG per product, Angebote B2B quotation pipeline (ANGELEGT→VERSANDT→ANGENOMMEN/ABGELEHNT/ABGELAUFEN); **`GET /api/v1/comparison-feed`** (comparison portal feed — ETag-cached, cursor-paginated; extracts `TarifPreise` + `jahreskosten_supply_*` from JSONB; `Cache-Control: public, max-age=300`; §42d EnWG compliant); **MCP: 10 tools, 3 prompts** at /mcp; **tarifbd-agent** in agentd (§41a EPEX monitoring, §42 Energiemix completeness, Angebot hygiene); port :9080
services/billingd/         Energy Billing Engine (LF role) — pure calculation, all prices user-defined in tarifbd; **`Product` typed enum** (12 categories, serde-tagged `#[serde(tag="category")]`); per-category typed structs (`ElectricityProduct`, `GasProduct`, `HeatProduct`, `SolarProduct`, `ControllableLoadProduct` §14a, `SharingProduct` §42c, …); `Product::build_engine(&grid, &rates)` primary dispatch (no `Option<BillingEngine>`, no `PricingModel::try_from()`); **`ControllableLoadProvider`** for §14a WAERMEPUMPE/WALLBOX (delegates to `ElectricityProvider` + appends §14a credits); **`BillingEngine::validate()`** pre-flight regulatory check (no positions); **`BillingEngine::bill_batch()`** portfolio billing; **`Invoice.warnings: Vec<BillingWarning>`** + `has_errors()` + `has_warnings()`; **§41b iMSys guard** — hard `BillingError` when `dynamic_epex + !Imsys`; **`StromsteuerBefreiung`** typed enum (§9 Nr. 1-5 + §9a); **`EnergieQuellen`** struct (CO₂ label §42 Abs. 2 Nr. 2 EnWG); BEHG L-Gas CO₂ factor `BEHG_CO2_FACTOR_L_GAS`; `GasProduct::gas_indexed_price` (TTF/NCG); `GasProduct::gas_leistungspreis_ct_per_kw_month` (RLM demand); **`PositionTrace`** on every `BillingPosition` (formula, regulatory_basis, tariff_source); **`BillingWarning`/`WarningSeverity`** for non-fatal issues; **`CustomerKategorie`** (Haushalt/Gewerbe/Industrie); **`EnergyShareProvider`** §42c; **historic levy lookups** `stromsteuer_for_year` / `energiesteuer_gas_for_year` (incl. 2022 0-rate) / `behg_ct_per_kwh_for_year`; **`Invoice::merge()`** Tarifwechsel; **`Invoice::allocate_proportionally()`** B2B; `prorate_days()` pro-rata; `billing_run_id` audit; `anlage_kwp` auto-0% MwSt; `MeteringMode` (SLP/RLM/iMSys); `eeg` optional feature; **no `rubo4e` dep** (bo4e feature removed); `to_rechnung_json()` BO4E JSON; **12 MCP tools** (`validate_tariff_config`, `explain_invoice_position`, …); **160 tests**; zero I/O; `POST /api/v1/billing/{malo_id}/tarifwechsel`; **VPP auto-billing**: `vpp_contracts` table (SR-ID → capacity_price_eur_per_kwh), `vpp_dispatch_ledger` (idempotency), `POST /api/v1/webhooks/vpp-dispatch` (receives `de.vpp.dispatch.confirmed`, HMAC-verified, computes `flexibility_kwh = max_power_kw × duration_h`, generates `Rechnung`, emits `de.vpp.settlement.berechnet`), `vpp_auto_billing` config flag; migrations 0001–0002; port :9280
services/accountingd/      Massenkontokorrent / Customer Account Ledger (LF role) — accounts + ledger_entries (immutable, idempotent CE ingest) + sepa_mandates + dunning_cases; 6 DB migrations; ingest de.billing.rechnung.erstellt (debit) + de.eeg.verguetung.berechnet (credit); `GET /accounts/{malo_id}/balance`, `/ledger`, `/kontoauszug`, **`/open-items`** (FIFO clearing); **`POST /reconcile`** (balance cache drift detection+repair); **`POST /anonymize`** (GDPR Art. 17 pseudonymization, immutable `anonymization_log`); SEPA pain.008 XML via sepa 0.3.0 (typed `SequenceType`, hard `creditor_iban` validation — no placeholder fallback); pain.001 SCT credit-transfer via `build_pain_001()`; **auto-dunning rule engine** (`dunning_auto_enabled=true` → daily worker → Mahnstufe 1→2→3 auto-escalation, `auto_dunning_runs` idempotency); Mahnwesen Mahnstufe 1-3; CAMT.054 payment import via `sepa::ct_from_eur_str`; IBAN validated via ISO 13616 mod-97; `Vorauszahlung` + `Zahlungsinformation` typed BO4E; tenant isolation: `(malo_id, lf_mp_id, tenant)` UNIQUE; 71 unit tests; emits de.accounting.mahnung.issued + de.accounting.sperrauftrag; port :9380
services/vertragd/         Contract & Customer Management (LF role) — Kunden (B2C + B2B) with kunden_identitaeten (N OIDC logins per company, rolle=VOLLZUGRIFF/ADMIN/FINANZEN/TECHNIK/READONLY, standort_filter for site-scoped B2B access); Rahmenverträge (B2B portfolio: Sammelrechnung, indexation, volume discount, angebot_id); Versorgungsverträge per site/commodity (ANGELEGT→IN_BEARBEITUNG→TEILERFUELLUNG→AKTIV→GEKÜNDIGT→ABGELAUFEN); triggers GPKE/GeLi Gas Lieferbeginn/-ende via processd; Tarifwechsel endpoint (§41 EnWG) with **Preisgarantie guard** (blocks price changes within `preisgarantie_bis` window, `override_preisgarantie=true` operator bypass — logged to `preisgarantie_override_log`); `PUT/GET /api/v1/vertraege/{id}/preisgarantie` typed `rubo4e::current::Preisgarantie` BO4E COM; `PUT/GET /api/v1/kunden/{id}/person` typed `rubo4e::current::Person` BO4E (GDPR Art. 15); **`PUT/GET /api/v1/kunden/{id}/zahlungsinformation`** typed `rubo4e::current::Zahlungsinformation` COM (IBAN mod-97 validated); **`GET /api/v1/kunden/{id}/export`** GDPR Art. 15/20 full PII export; **`POST /api/v1/kunden/{id}/anonymize`** GDPR Art. 17 irreversible pseudonymization (immutable `anonymization_log`); `POST /api/v1/vertraege/{id}/stornieren` pre-activation cancel; Kündigung with coordinated Schlussablesung; auto-renewal background worker; Preisanpassungsbenachrichtigung worker (§41 Abs. 3 EnWG, 42-day window); OIDC sub → MaLo authorization gateway (GET /kunden/authenticate) for portald; 3 DB migrations (0001 initial, 0002 zahlungsinformation, 0003 correctness+GDPR); 9-tool MCP server + 2 prompts; HMAC-SHA256 on all CloudEvent webhooks; port :9780
services/mabis-syncd/      MaBiS UTILTS synchronisation daemon (ÜNB/NB role) — aggregates per-MaLo Lastgang from edmd via `mako-mabis::SummenzeitreiheBuilder` (uses `metering::MeterInterval`, typed — no raw tuples); submits monthly Summenzeitreihen to BIKO via makod UTILTS command; vorlaeufig day 3 + endgueltig day 8 at 05:00 UTC per BK6-22-024 Anlage 3 MaBiS; `submission_runs` + `submission_malo_log` PostgreSQL tables; retry with attempt_count < 3 guard; MaLo discovery via edmd `/api/v1/billing-periods`; port :8880
services/agentd/          Multi-agent LLM orchestration daemon — Orchestrator+Specialist Mesh pattern; **27 specialists compiled into binary** (ship in container image); activated via `[bundled_agents]` in `agentd.toml` (`enable_all = true` or explicit list + per-agent `[bundled_agents.overrides.<name>]`); custom agents in `[[agents]]` can override built-ins by name; **3 dispatch modes** (`sequential`/`parallel`/`race`); OpenAI/Anthropic/AWS Bedrock SigV4 providers; ReAct loop with MCP tools across all 17 services; LanceDB RAG (persistent ANN, S3/GCS/local); WASM plugins via mako-plugin (Extism/Wasmtime sandbox); **A2A Agent Cards** at `/.well-known/agents/{name}`; **catalog endpoint** `GET /api/v1/agents/catalog`; **agent listing** `GET /api/v1/agents`; glob `trigger_event_types`; `GET /api/v1/sessions` decision ring buffer; `POST /api/v1/rag/search`; CloudEvent trigger + /api/v1/run; emits de.agent.decision.made; port :9580
services/portald/          Customer Portal read-model gateway (LF role) — aggregates Lastgang (edmd), invoices (billingd), account balance (accountingd), VersorgungsStatus (marktd), EEG settlement (einsd) into single REST + SSE API; OIDC bearer-token authentication; `GET /portal/{malo_id}/dashboard`, `/lastgang`, `/invoices`, `/balance`, `/kontoauszug`, `/vorauszahlung`, `/eeg`, `/versorgung`, `/events` (SSE); §41 EnWG self-service write API: POST `/tarifwechsel`, POST `/kuendigen`, PUT `/kontakt`, PUT `/sepa`, GET `/invoices/{id}/download` XRechnung 3.0; 8-tool MCP server; shared `auth_client` (not per-request); port :9480
services/nis-syncd/       NIS/GIS grid topology import adapter (NB role) — stateless; concurrent sync via tokio::task::JoinSet + Semaphore (default 20 parallel PUT calls); POST /api/v1/grid/sync pushes malo_grid records to marktd; dry-run mode; per-entry drift detection; `check_malo_grid` MCP tool for STP root-cause diagnosis; `get_last_sync_report` MCP tool; processd NB STP ~80%→≥95%; 4-tool MCP server; port :9680
services/edmd/            Energy Data Management daemon — stores MSCONS meter readings (with `obis_code`), direct iMSys/SMGW push (`POST /api/v1/meter-reads/rlm/{malo_id}` + gas, idempotent on session_id), Hampel-filter quality scoring (k=3 t=3.0, grades A/B/C/F, retroactive rescore) + V01–V10 validation engine, Ablesesteuerung reading orders (INSRPT PID 23001 auto-creates `INSRPT_STOERUNG` orders), `GET /api/v1/deliveries/{malo_id}` returns `Vec<Energiemenge>` (BO4E typed), BO4E `Lastgang` + `Zeitreihe` export, `MeterBillingPeriod` (RLM spitzenleistung + Gas brennwert/zustandszahl), Mehr-/Mindermengen imbalance; **virtual meters** (Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation per §42b EnWG Solarpaket I, `GET /api/v1/virtual/{id}/lastgang`); **resampled Lastgang** (`GET /api/v1/lastgang/{malo_id}/resampled?resolution=HOUR|DAY|MONTH|YEAR`); **Summenzeitreihe** monthly aggregation for MaBiS (`GET /api/v1/summenzeitreihe/{malo_id}`); **§17 MessZV annual forecast** (`GET /api/v1/forecast/{malo_id}`); **gas quality data** PID 13007 (`GET /api/v1/gas-quality/{malo_id}`); **§22 MessZV bitemporal corrections** (`POST /api/v1/corrections/{malo_id}`); **quality assessments** per batch (`GET /api/v1/quality-assessments/{malo_id}`); PostgreSQL; Apache Iceberg V2 archive; emits `de.edmd.reading.direct.stored` + `de.edmd.reading.quality.warning`; 10-tool MCP server; port :8380
services/obsd/            Business-process observability daemon — process projections with automated deadline computation (GPKE 24h/WiM 7d/GeLi Gas 14d/MABIS 2d calendar-day approximations), `completed_at` cycle-time column, `GET /api/v1/audit/bnetza-report` (§20 Abs.1 EnWG Diskriminierungsbericht), BNetzA KPI reports; 6-tool MCP server (get_process, list_overdue_processes, get_kpi_report, get_parity_report, get_stp_rate, list_processes_by_family); Alertmanager bridge; PostgreSQL; port :8480
xtask/                    Build/codegen/validation tasks
docs/                     Architecture docs
Dockerfile                Multi-stage cargo-chef + distroless image for makod
.dockerignore             Docker build context filter
```

---

## Build and Test

```bash
# Full CI gate — run before every commit:
just ci

# Individual gates:
cargo check --all-targets --all-features
cargo test --all-features
cargo test -p mako-engine --all-features
cargo test --test <name> --all-features
cargo build -p makod --release
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo deny check

# xtask tasks:
cargo xtask bump-version X.Y.Z       # bump [workspace.package].version
cargo xtask codegen                   # regenerate profile Rust code from YAML
cargo xtask validate-profiles         # validate all profiles against EDIFACT specs
cargo xtask validate-pruefids         # validate Prüfidentifikatoren (AHB check)
cargo xtask audit-ahb                 # audit Application Handbooks
cargo xtask check-release-coverage    # verify format-version coverage
cargo xtask check-bo4e-coverage       # verify rubo4e::current type count matches README (delta ≤ 2)
cargo xtask generate-fixtures         # regenerate EDIFACT test fixtures
cargo xtask extract-pdf               # extract tables from BDEW PDFs (docs/pdfs/)
cargo xtask import-codelists          # import BDEW code lists
cargo xtask import-xml-ahb            # import AHB rules from BDEW XML
cargo xtask release-diff              # diff between format versions
```

**`just ci` is the minimum gate before any commit.** It runs check + test + clippy
+ fmt-check + deny + codegen-check + validate-profiles-strict + validate-pruefids-strict.

**MSRV: 1.94** — do not use language features or stdlib APIs introduced after 1.94.

---

## Toolchain and Edition

- Rust edition: **2024** (all crates)
- Toolchain: **1.92** (pinned in `rust-toolchain.toml` — do not change to `stable`)
- Components: `rustfmt`, `clippy`

---

## Active Format Versions

| Format version | Valid period | Status |
|---|---|---|
| `FV2025-10-01` | 2025-10-01 through 2026-09-30 | **Current production** |
| `FV2026-10-01` | from 2026-10-01 | **Next release — profiles must exist** |

Both coexist in the same engine instance simultaneously. A process started under
`FV2025-10-01` continues under those rules until it completes, even after the
`FV2026-10-01` cutover.

---

## Code Conventions

### Error handling
- All public APIs return `Result<_, EngineError>` or `Result<_, WorkflowError>`.
- Use `thiserror` for error type definitions. Do not use `anyhow` inside library crates.
- `anyhow` is acceptable in `xtask` and `makod` (binary crates).
- Every `Result`-returning function must be annotated `#[must_use]`.

### Async
- All async code targets **Tokio** (version 1).
- Use async-fn-in-trait (AFIT) — stabilised at Rust 1.75, available on MSRV 1.94.
- Do not use `tokio::runtime::Handle::try_current()` as a runtime-detection backdoor.

### Types
- All IDs are UUID v4 newtypes defined via `define_id!` in `mako-engine/src/ids.rs`.
  Never accept or return plain `String` or `Uuid` where a typed ID belongs.
- Timestamps use `time::OffsetDateTime` — **not** `chrono::DateTime<Utc>`.
- EDIFACT payloads and event payloads use `serde_json::Value` — **not** `Vec<u8>` or `Bytes`.
- **`tenant: String`** is a **data-isolation key** written to every database row — it is NOT
  the BDEW-Codenummer. In demos it happens to equal the operator's BDEW-Codenummer for convenience,
  but it can be any stable unique string (e.g. a UUID, a slug). The BDEW-/DVGW-Codenummer belongs
  in `lf_mp_id`, `nb_mp_id`, `own_mp_id`, or `MarktpartnerId` fields — not in `tenant`.
  Document `tenant` as: `"Tenant identifier — data-isolation key written to every database row.
  Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid."`.
- Market participant identifiers use `MarktpartnerId` from `rubo4e::identifiers` — **not** `String` and
  **not** the removed `Gln` type alias. In BO4E the correct term is `MarktpartnerId` (= `rollencodenummer`
  in `Marktteilnehmer`). Only GS1-issued 13-digit codes are true GLNs (NAD DE3055 = `9`);
  BDEW-Codenummern (`99…`, NAD `293`) and DVGW-Codenummern (`98…`, NAD `332`) are not GLNs.
  Use `mako_markt::domain::nad_agency_code()` to derive the coding authority.
- BO4E Business Objects are imported directly from `rubo4e::current` (versioned) or
  `rubo4e::identifiers` (version-stable). **Never** write `rubo4e::v202607::Foo` — always use
  `rubo4e::current::Foo`. The `no-version-alias` CI gate enforces this.

  ```rust
  // Correct — version-stable identifiers
  use rubo4e::identifiers::{ObisCode, SrId, NeloId, MaloId};

  // Correct — versioned BOs via current alias
  use rubo4e::current::{Rechnung, PreisblattNetznutzung, Lastgang};

  // WRONG — hardcoded schema version
  // use rubo4e::v202607::Rechnung;
  ```

### Workflow determinism
- `Workflow::handle` and `Workflow::apply` must be **pure functions**: no I/O,
  no clock access, no global state mutation.
- All parsing, validation, and external calls happen before the command is
  constructed, at the transport boundary.

### Feature flags
- `slatedb` — opt in at the binary level only; never enable in library crate defaults.
- `testing` — enables `InMemoryXxx`/`NoopXxx` stores; must never appear in production builds.
- `tracing` — optional instrumentation; off by default.

### Versioning
- Use **BDEW format versions** (`FV<YYYY>-<MM>-<DD>`) as version keys, not SemVer.
- Always use `FormatVersion::parse(...)` for user-supplied or deserialized strings.
- `FormatVersion::new(...)` is unchecked — only for known-valid compile-time literals.

---

## Domain Rules — Do Not Get Wrong

### PID ownership — authoritative table

| PID range | Crate | Source |
|---|---|---|
| 55001–55018, 55555 | `mako-gpke` | BK6-24-174 |
| 55039, 55042, 55051, 55168 | `mako-wim` | BK6-24-174 |
| 13003 | `mako-mabis` | BK6-24-174 |
| 44001–44021 | `mako-geli-gas` | BK7-24-01-009 |
| 44022–44024 | `mako-wim-gas` `wim-gas-stornierung` (Msb/Nmsb/all roles) **and** `mako-geli-gas` `geli-gas-stornierung` (Nb-only: 44022 inbound) / `geli-gas-stornierung-lf` (Lf: 44023/44024 inbound) | BK7-24-01-009 |
| 37000–37006 | `mako-gpke` (PARTIN Strom Kommunikationsdaten) | PARTIN AHB 1.0f |
| 37008–37014 | `mako-geli-gas` (PARTIN Gas Kommunikationsdaten) | PARTIN AHB 1.0f |
| 17115–17117 (Sperrung Strom, ORDERS) | `mako-gpke` | BK6-22-024 |
| 17115–17117 (Sperrung Gas, ORDERS) | `mako-geli-gas` | BK7-24-01-009 |
| 44039–44041, 44042–44053, 44168–44170 | `mako-wim-gas` | BK7-24-01-009 |
| 31001–31002, 31005–31006 | `mako-gpke` (MMM-Rechnung / MMM-selbst ausgest. Rechnung Strom, NB → LF) | BK6-24-174 |
| 31007–31008 | `mako-gabi-gas` (Aggreg. MMM-Rechnung Gas / selbst ausgest., NB → MGV; Gas-only; MGV is a Gas-domain role) | BK7-14-020 |
| 13013 | `mako-gabi-gas` `gabi-gas-mmma` (Allokationsliste Gas, MMMA, Gas-only) | BK7-14-020 |
| 17110, 19110 | `mako-gabi-gas` `gabi-gas-mmma` (ORDERS/ORDRSP Allokationsliste Gas, Gas-only; ⚡=— in AHB 1.0) | BK7-14-020 |
| 31009 | `mako-wim` (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1 — routed via wim-rechnung to avoid double-registration) | BK6-24-174 |
| 31003 | `mako-wim-gas` (WiM-Rechnung) | BK7 billing |
| 31004 | `mako-wim-gas` (Stornorechnung WiM Gas) | BK7-24-01-009 |
| 31010 | `mako-gabi-gas` (Kapazitätsrechnung, Kapazitätsabrechnung Gas) | BK7 |
| 31011 | `mako-geli-gas` (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF) | BK7-24-01-009 |
| 17134–17135 | `mako-gpke` (ORDERS Konfiguration, GPKE Teil 3) | BK6-22-024 |
| 19001–19002 | `mako-wim` (ORDRSP Geräteübernahme, WiM Strom) **and** `mako-gpke` (ORDRSP Konfiguration, NB role) — multi-domain: both "WiM Gas" and "WiM Strom Teil 1" per BDEW PID 3.3/4.0 xlsx | BK6-24-174 |
| 23001, 23003, 23004, 23008 | `mako-wim` `wim-insrpt` (Strom 5WT · combined) · `mako-wim-gas` `wim-gas-insrpt` (Gas-only 10WT) | BK6-24-174 / BK7-24-01-009 |
| 23005, 23009 | `mako-wim-gas` `wim-gas-insrpt` — Gas-only INSRPT variants, always 10 WT | BK7-24-01-009 |

**PIDs that do NOT exist — never register:**
- 56001–56010: these PIDs were never assigned in any BDEW AHB document (confirmed absent from PID 3.3, 3.3 KL, PID 4.0, and all UTILMD AHB PDFs)
- 44555: does not exist in PID 3.3 or PID 4.0; Gas Sperrung process uses ORDERS PIDs 17115–17117
- 11001–11003: legacy pre-reform PIDs, superseded by 55039/55042/55051/55168
- 11004–11099: reserved but not in current WiM AHB

**PIDs that exist but belong to WiM Gas, NOT GeLi Gas:**
- 44022–44024: role-conditional routing implemented in `mako-geli-gas`:
  - `Nb`-only: PID 44022 → `geli-gas-stornierung` (GNB receives Anfrage)
  - `Lf`-only: PIDs 44023/44024 → `geli-gas-stornierung-lf` (LF receives GNB response)
  - `Msb`/`Nmsb`/`all()`: `mako-wim-gas` `wim-gas-stornierung` handles all three (default for WiM Gas / combined deployments)

### GeLi Gas 3.0
Governed by **BK7-24-01-009** (Beschluss 12.09.2025). Supersedes BK7-19-001 and BK7-06-067.
Scope: UTILMD G (PIDs 44001–44021) + UTILMD G PIDs 44022–44024 (role-conditional: `geli-gas-stornierung` for Nb, `geli-gas-stornierung-lf` for Lf) + ORDERS Sperrung Gas (17115–17117) + PARTIN Gas Kommunikationsdaten (37008–37014) + INVOIC 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF).
PID 31010 (Kapazitätsrechnung, NB → BKV) is a GaBi Gas (BK7-14-020) billing process and belongs to `mako-gabi-gas`.
PID 31011 (Rechnung sonstige Leistung, NB → LF) is billed by the GNB/VNB to the LFN/LFA for performing AWH (abrechnungswürdige Handlungen) during the Sperrprozess — it is a GeLi Gas (BK7-24-01-009) billing, NOT GaBi Gas.

### MABIS vs Messwesen
Only PID **13003** is MABIS (Bilanzkreisabrechnung Strom, BKV↔ÜNB).
PIDs 13002–13028 (excluding 13003) are Messwesen PIDs — do not register them under MABIS.
MaBiS IFTSTA PIDs are **21000–21005** (21006 does not exist; 21007 belongs to WiM Strom Teil 1 / WiM Gas, registered in `mako-wim` `wim-device-change`).

### Marktrollen (Rollenmodell V2.2) — authoritative role table

Source: BDEW-AWH Rollenmodell V2.2 (08.01.2026). Only roles with
`Marktkommunikation: zur Verwendung freigegeben` are listed.

| Abbreviation | Name | Sparte | Notes |
|---|---|---|---|
| `NB` | Netzbetreiber | Gas + Strom | In EDIFACT Gas AHBs sometimes qualified as `GNB` (Gasnetzbetreiber) |
| `LF` | Lieferant | Gas + Strom | In EDIFACT Gas AHBs sometimes qualified as `LFG` |
| `MSB` | Messstellenbetreiber | Gas + Strom | In EDIFACT Gas AHBs sometimes qualified as `GMSB` |
| `BKV` | Bilanzkreisverantwortlicher | Gas + Strom | Gas balancing handled via MGV/FNB framework |
| `ÜNB` | Übertragungsnetzbetreiber | Strom | Maps to `UNB` in config; `FNB` (Gas TSO) maps to `Uenb` in engine |
| `BIKO` | Bilanzkoordinator | Strom | BNetzA-governed; issues Abrechnungssummenzeitreihe (PID 13003) |
| `MGV` | Marktgebietsverantwortlicher | Gas | No engine deployment role |
| `KN` | Kapazitätsnutzer | Gas | GaBi Gas capacity booking; no engine deployment role yet |
| `DP` | Data Provider | Strom | UTILTS metering data distribution; no engine deployment role yet |
| `EIV` | Einsatzverantwortlicher | Strom | Redispatch 2.0 (placeholder crate) |
| `ESA` | Energieserviceanbieter des Anschlussnutzers | Strom | iMS / smart meter context |
| `RB` | Registerbetreiber | Gas + Strom | MaStR data registry; sparte-neutral |

**Roles that do NOT exist in Rollenmodell V2.2 — never use:**
- `NBG`, `MSBG`: these abbreviations do not appear in BDEW documents
- Sub-role qualifiers `GNB`, `LFG`, `GMSB`, `ANB`, `VNB`, `NMSB`, `AMSB`, `FNB` are
  EDIFACT-AHB sub-qualifiers or operational sub-types used in `[[party]]` config and
  NAD role fields — they are NOT standalone Rollenmodell roles.

### MP-ID formats and EDIFACT identification codes — never mix these up

Source: BDEW-AWH Identifikatoren V1.2 (07.02.2025) §2.2;
Allgemeine Festlegungen V6.1d (01.04.2026) §2.13, §3;
UTILMD AHB Gas 1.2 NAD+MS/MR tables.

#### BDEW-Codenummer vs. DVGW-Codenummer vs. GLN

| Type | Positions 1–2 | Digits | NAD DE3055 | UNB DE0007 | Registry |
|---|---|---|---|---|---|
| BDEW-Codenummer (Strom) | `99` | 13 | **`293`** | **`500`** | bdew-codes.de |
| DVGW-Codenummer (Gas) | `98` | 13 | **`332`** | **`502`** | codevergabe.dvgw-sc.de |
| GLN (GS1) | varies | 13 | **`9`** | **`14`** | GS1 |
| EIC | — | 16 | **`ZEW`** | — | ENTSO-E |

- NAD DE3055 and UNB DE0007 use **different code values** for the same organisation.
- `332` (DVGW in NAD DE3055) ≠ `502` (DVGW in UNB DE0007).
- `9` (GS1 in NAD DE3055) ≠ `14` (GS1 in UNB DE0007).
- In `services/makod/src/party_registry.rs` the agency code is auto-derived from the GLN
  prefix: `99…` → `"293"`, `98…` → `"332"`, other 13-digit → `"9"`, 16-char → `"ZEW"`.
- Each Marktrolle must have **exactly one MP-ID** (`"einem Marktteilnehmer kann für jede
  Marktrolle nur genau eine MP-ID zugeordnet sein"` — Identifikatoren AWH §2.1).
- UNB `NAD+MS` (sender) and `NAD+MR` (receiver) must use **identical** MP-IDs as the
  corresponding UNB DE0004/DE0010 sender/receiver fields (§2.13).

#### §2.12 Filename convention (Allgemeine Festlegungen V6.1d §2.12)

`<MsgType>_<SenderMPID>_<ReceiverMPID>_<YYMMDD>_<HHMM>_<Ref>.txt`
(`.txt.gz` when compressed)

#### §2.14 Publication requirement

- Only published MP-IDs may be used in production messages.
- Strom: https://bdew-codes.de/Codenumbers/BDEWCodes/CodeOverview
- Gas: https://codevergabe.dvgw-sc.de/MarketParticipants
- Operator must be reachable within **3 Werktage** after initial contact (§2.14).

### EDIFACT time encoding — never mix UTC and local time

Source: Allgemeine Festlegungen V6.1d §3.

- All **EDIFACT times are in UTC** (DTM qualifier 303: `CCYYMMDDHHMMZZZ`, ZZZ always `+00`).
- Process **deadlines** use **gesetzliche deutsche Zeit** (CET = UTC+1, CEST = UTC+2).
- An off-by-one-hour error at DST transitions is a **regulatory deadline violation**.

| Sparte | Event | UTC MEZ (CET) | UTC MESZ (CEST) |
|---|---|---|---|
| Strom | Lieferbeginn/-ende (Mitternacht) | `2300` | `2200` |
| Gas | Gastag-Beginn (06:00 local) | `0500` | `0400` |

- Bilanzierungsmonat uses DTM qualifier **610**: `DTM+492:202106:610'`
- `DE0035 = 1` in UNB marks a **test message** (do not process as production).

### APERAK Fristen — never mix these up

#### APERAK *sending* deadline (how quickly the receiver must send the APERAK)
Per **APERAK AHB 1.0** (FV2025-10-01):

| Sparte | Message type | Deadline | Source |
|---|---|---|---|
| **Strom** | UTILMD / ORDERS (weekday) | **45 Minuten** | APERAK AHB 1.0 §2.4.1 |
| **Strom** | UTILMD / ORDERS (Saturday) | **Sonntag 12 Uhr** | APERAK AHB 1.0 §2.4.1 |
| **Strom** | all other | **nächster Werktag 12 Uhr** | APERAK AHB 1.0 §2.4.1 |
| **Gas** | Folgeprozesse | **nächster Werktag 12 Uhr** | APERAK AHB 1.0 §2.3.1 |
| **Gas** | Initialprozesse | **3 Werktage** | APERAK AHB 1.0 §2.3.1 |

Gas APERAKs are always **Verarbeitbarkeitsfehlermeldungen** (BGM+313) only — no Anerkennungsmeldung.
Strom APERAKs include **both** Anerkennungsmeldung (BGM+312, accepted) and Verarbeitbarkeitsfehlermeldung (BGM+313, rejected).
Gas CONTRL rule: "Auf eine APERAK ist immer eine CONTRL zu senden." (APERAK AHB 1.0 §2.3, CONTRL AHB 1.0 §2.3.1)

#### Process *response* deadline (how long the business process can take overall)
These are NOT APERAK deadlines. Never use these as the APERAK-sending window.

| Process | Deadline | Function | Source |
|---|---|---|---|
| GPKE | **24 wall-clock hours** | `fristen::add_hours(t, 24)` | BK6-22-024 §5 |
| WiM | **5 Werktage** | `fristen::add_werktage(d, 5, BdewMaKo)` | BK6-24-174 |
| GeLi Gas | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` | BK7-24-01-009 |
| WiM Gas | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` | BK7-24-01-009 |
| MABIS (Prüfmitteilung) | **1 Werktag** | `fristen::add_werktage(d, 1, BdewMaKo)` | BK6-24-174 §13.8 |

**Saturday = Werktag.** Sunday and public holidays do not count.
All deadline arithmetic uses **German local time (CET/CEST)**, not UTC.
An off-by-one-hour error at DST transitions is a regulatory deadline violation.

### Format-version coexistence
`WorkflowVersionPolicy::ForwardCompatible` is the correct default for **all** MaKo
workflows. Do not default to `Pinned`.

### Dual-write atomicity
Events and outbox entries must be written in a single `WriteBatch` via
`AtomicAppend::append_with_outbox`. Never write events first and outbox second —
a crash between the two produces a lost APERAK with no recovery path.

---

## Licenses

Only these SPDX identifiers are allowed (enforced by `cargo deny`):
MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause,
ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0, MIT-0.

---

## Key Documentation

| Topic | File |
|---|---|
| Architecture overview | [docs/architecture.md](../docs/architecture.md) |
| Process engine guide | [docs/engine.md](../docs/engine.md) |
| `makod` operator guide | [docs/makod.md](../docs/makod.md) |
| `marktd` operator guide | [docs/marktd.md](../docs/marktd.md) |
| `processd` operator guide | [docs/processd.md](../docs/processd.md) |
| `invoicd` operator guide | [docs/invoicd.md](../docs/invoicd.md) |
| `netzbilanzd` operator guide | [docs/netzbilanzd.md](../docs/netzbilanzd.md) |
| `sperrd` operator guide | [docs/sperrd.md](../docs/sperrd.md) |
| `einsd` operator guide | [docs/einsd.md](../docs/einsd.md) |
| `tarifbd` operator guide | [docs/tarifbd.md](../docs/tarifbd.md) |
| `billingd` operator guide | [docs/billingd.md](../docs/billingd.md) |
| `accountingd` operator guide | [docs/accountingd.md](../docs/accountingd.md) |
| `nis-syncd` operator guide | [docs/nis-syncd.md](../docs/nis-syncd.md) |
| `edmd` operator guide | [docs/edmd.md](../docs/edmd.md) |
| `obsd` operator guide | [docs/obsd.md](../docs/obsd.md) |
| MCP server (LLM tooling) | [services/makod/src/mcp_server.rs](../services/makod/src/mcp_server.rs) · [docs/makod.md#mcp-server](../docs/makod.md) |
| ERP integration (CloudEvents 1.0 webhooks, Command API) | [docs/erp-integration.md](../docs/erp-integration.md) |
| Parsing guide | [docs/parsing.md](../docs/parsing.md) |
| Validation guide | [docs/validation.md](../docs/validation.md) |
| Builder patterns | [docs/builders.md](../docs/builders.md) |
| Annual release workflow | [docs/annual-release-workflow.md](../docs/annual-release-workflow.md) |
| Schema versioning | [docs/schema-versioning.md](../docs/schema-versioning.md) |
| API-Webdienste Strom | [docs/api-webdienste.md](../docs/api-webdienste.md) |
| Release lifecycle | [docs/release-lifecycle.md](../docs/release-lifecycle.md) |
| BNetzA regulatory reference | [docs/bnetza.md](../docs/bnetza.md) |
| PID reference | [docs/pid-reference.md](../docs/pid-reference.md) |
| Compensation / APERAK timeout flows | [docs/compensation.md](../docs/compensation.md) |
