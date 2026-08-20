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
crates/mako-gpke/         GPKE — UTILMD Strom (55001–55018, 55022–55024, 55555, 55607–55609) + INVOIC (31001, 31002, 31005, 31006) + ORDERS Sperrung (17115–17117) + ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002) + Stammdatenänderung GPKE Teil 4 (55615–55694, 55109/55110 — `gpke-stammdatenaenderung`, apply MaLo change + Rückmeldung A01/A02) + PARTIN Strom (37000–37006)
crates/mako-wim/          WiM Strom — Messstellenbetrieb (55039, 55042, 55051, 55168) + ORDERS Geräteübernahme (17001 Bestellung Geräteübernahmeangebot · 17002 Weiterverpflichtung · 17009 Anzeige Gerätewechselabsicht → 19001/19002, 19003/19004, 19015/19016; nMSB role — 17005 Rechnungsabwicklung and 17011 Änderung Technik are *different* processes and are not routed here) + Stammdaten (StammdatenUebermittelt carries standorteigenschaften+zaehlwerke for marktd auto-update) + Preisanfrage/REQOTE (35001–35005) + Preisliste/PRICAT (27001–27003) + INVOIC 31009 + INSRPT (23001, 23003, 23004, 23008) + Technik-Änderung (17011/17118/17121 → 19003–19007) + iMS Steuerungsauftrag + ESA Wertebestellung (WiM Teil 2 UC 4.1/4.3): the whole 35003→15003→17007/17008→19011/19012→39002→19013/19014 handshake is ONE `wim-wertebestellung` process (MSB) / `esa-wertebestellung` process (ESA); inbound REQOTE **35003** (ESA-specific — 35002 is Rechnungsabwicklung MSB über LF, LF→MSB)/ORDERS 17007/17008/ORDCHG 39002 → wim-wertebestellung (there is NO standalone `wim-stornierung` workflow — 39002 Storno is `ReceiveStornierung` in the lifecycle, correlated by RFF+ON), **outbound answers rendered** — QUOTES 15003 (Angebot/Ablehnung) + ORDRSP 19011/19012/19013/19014, PID in BGM DE1004
crates/mako-geli-gas/     GeLi Gas 3.0 — UTILMD G (44001–44021) + LFN-side Lieferbeginn workflow (`geli-gas-lf-anmeldung`, PIDs 44001/44004 outbound + 44002/44003 + 44005/44006 inbound) + UTILMD G Stornierung role-conditional (44022 Nb-only, 44023/44024 Lf-only) + ORDERS Sperrung Gas (17115–17117, LF-role `geli-gas-sperrung-lf` + GNB-role `geli-gas-sperrung-nb`) + ORDERS Datenabruf (17103/17104 + ORDRSP 19103/19104, `geli-gas-datenabruf`) + Stammdatenänderung GeLi Gas (44109–44182 — `geli-gas-stammdatenaenderung`, Zustimmung/Ablehnung E15/E13/E17, Monatserster rule) + PARTIN Gas (37008–37014) + INVOIC 31011 (AWH Sperrprozesse Gas)
crates/mako-mabis/        MaBiS — 5 workflows: Bilanzkreisabrechnung Strom (MSCONS 13003, 13010–13012 `mabis-billing`, BKV↔ÜNB/BIKO); UTILMD Clearinglisten (55065/55069/55070 `mabis-clearingliste`, record-only); ZP-Lifecycle (`mabis-zp-lifecycle`, 55062–55064/55071–55072/55197–55200/55203–55214 — half the families have no Antwort PID and are terminal on arrival; 55064 answers BOTH 55062 and 55063); Anforderungen (`mabis-anforderung`, ORDERS 17201–17208 — the Abonnement verb is in the payload, NOT the PID); Listenabgleich (`mabis-listenabgleich`, 55195/55196, 55201/55202, 55223/55224 — reply is a correction count, not accept/reject)
crates/mako-wim-gas/      WiM Gas — UTILMD G (44022–44024 + 44039–44053, 44168–44170) + INVOIC (31003, 31004) + INSRPT Gas-only (23005, 23009)
crates/mako-gabi-gas/     GaBi Gas 2.1 (BK7-24-01-008) — INVOIC 31010 (Kapazitätsrechnung) + INVOIC 31007/31008 (Aggreg. MMM-Rechnung Gas, NB → MGV) + MSCONS 13013 (Allokationsliste Gas, MMMA) + ORDERS 17110/ORDRSP 19110 + DVGW workflows ALOCAT/NOMINT/NOMRES/SCHEDL/IMBNOT/TRANOT/DELORD/DELRES (8 workflows); typed gas domain: `GasDay` (DST-aware 06:00 CET per DVGW G 2000), `GasQuantity` (Decimal kWh_Hs), `GasBeschaffenheit` (Hs + Zustandszahl, DVGW G 685, `.validate()` per DVGW G 260), `GasQualityFlag` (8 variants: Measured/Estimated/Substituted/Calculated/Corrected/Rejected/Unknown; billability per GaBi Gas 2.1 (BK7-24-01-008)), `AllocationVersion` (Initial/Correction/Final per KoV §6.4), `GasMarketRole` (BKV/FNB/VNB/MGV/LF/Händler), `GasPortfolioBalance` (BKV portfolio across Bilanzkreise, `conservation_check()`), `GasImbalanceSaldo` (Mehr/Minder/Balanced, `ausgleichsenergie_price_ct_per_kwh` per KoV §9); `cloud_events` module (`de.gabi.*` typed CloudEvent constants); `dvgw_versions` module (DVGW biannual release management: ALOCAT 5.11a / NOMINT 4.6 FK / NOMRES 4.7 FK / SCHEDL 4.4 FK / IMBNOT 5.7a / TRANOT 5.8b / DELORD 4.5 FK / DELRES 4.6 FK); `GasDay::nomres_deadline_utc()` (D-1 15:00 CET per KoV) + `initial_alocat_deadline_utc()` (D+3 12:00 CET per KoV §6.4) + `final_alocat_deadline_utc()` (M+2 per KoV §6.4); nomination correction chain (`corrects_nomination_ref` + `correction_sequence` in `NominationData`); `ConservationViolation` error type; `quantity_decimal()` on every DVGW quantity type (Decimal only — there is no `f64` accessor)
crates/dvgw-edi/          DVGW EDIFACT formats — ALOCAT, NOMINT, NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD, DELRES
crates/mako-nbw/          Netzbetreiberwechsel — PARTIN bulk DSO handover [placeholder]
crates/energy-api/        BDEW API-Webdienste Strom REST/WebSocket client+server
crates/mako-as4/          BDEW AS4-Profil v1.2 — `BdewAs4Profile`, `bdew_pmode()` (sign+encrypt, X509PKIPathv1, BrainpoolP256r1), `bdew_pmode_sign_only()`, `bdew_push_policy()` (require_encrypted_inbound), `WsSecOutboundKeyInfoProfile`, `BdewAction` enum (**16 standard variants** + `Custom`: UTILMD, APERAK, CONTRL, MSCONS, INVOIC, REMADV, IFTSTA, ORDRSP, ORDERS, ORDCHG, REQOTE, INSRPT, PRICAT, QUOTES, PARTIN, UTILTS), `Display`/`FromStr` impls on `BdewAction`, `bdew_action_from_str()` free fn, `ParseBdewActionError` uninhabited, `PartnerDirectory`, per-partner encryption cert registry; testing feature: `BdewTestPki` + `generate_self_signed_bdew_keypair()` + `MockAs4Endpoint::builder().with_decryption_key_pem(key)` (full sign+encrypt→decrypt round-trip); constants: `SIG_ALGO_ECDSA_SHA256`, `ENC_KEY_AGREEMENT_ECDH_ES`, `ENC_KEY_DERIVATION_CONCAT_KDF`, `ENC_KEY_WRAP_AES128`, `ENC_CONTENT_AES128_GCM`; asx-rs **v0.13** (SwA packaging with an empty SOAP Body per BDEW §2.2.3.2; BDEW envelope vocabulary set explicitly on send — `AGREEMENT_REF`, `ROLE_INITIATOR`/`ROLE_RESPONDER`, `party_id_type_for_agency`, `PAYLOAD_MIME_TYPE`; synchronous receipt verification via `verify_sync_response`/`send_and_verify` — signature-bound + NRI-digest-verified NRR, `As4ReceiptPolicy::regulated()`; `regulated_with_decryption_key()`; AS2 MIC moved to `As2ValidationPolicy`) auto-detects ECDSA vs RSA from key type and ECDH-ES vs RSA-OAEP from cert type; v0.8 new APIs used: `SessionContextBuilder::with_signing_material(cert, key)` (atomic, auto-derives key_id), `EventBus::new_for_testing()` (BestEffort, no audit sink), `As4HttpTransport::new_for_localhost_testing()` + `send_to_localhost()` (SSRF bypass for tests), partial `As4SendCredentials` (None fields fall back to session cert_handle)
crates/mako-redispatch/   Redispatch 2.0 process engine — 8 workflows (§§ 13/13a/14 EnWG)
crates/redispatch-xml/    Redispatch 2.0 XML/XSD format parsing
crates/mako-markt/        Market data library — MaloId, MeloId, MarktpartnerId, repository traits, AppState, CloudEvents, VersorgungsStatus, PriCatRepository, MaloGridRecord/MaloGridRepository, PreisblattMessungRepository, SteuerbareRessourceRepository, DeviceRepository (Zaehler/Geraet), testing feature
crates/mako-obs/          Observability library — ProcessProjection, KpiReport, DeadlineRisk, ProcessProjectionRepository
crates/mako-events/       Compile-time catalog of every CloudEvents `type` used across the workspace — one `pub const` per event type in bounded-context modules; emitters/subscribers reference these constants instead of inline strings
crates/mako-plugin/       Operator extension point for the event bus — the `CloudEventPlugin` trait + `PluginRegistry`, run by `mako-service::event_bus` before delivery
crates/invoic-checker/    INVOIC plausibility library — 6 checks: period validity, position arithmetic, document total, tariff match (ToU-aware: HT/NT position text classifies against zeitvariablePreispositionen band prices), tariff found; check 6 = MMM settlement price check (PIDs 31002/31005/31007/31008 vs. marktd MMMA store, `InvoicCheckEngine::check_mmm_settlement()`)
crates/netz-checker/      NB Anmeldung validation library — pure, deterministic 6-check pipeline (EBD E_0622/G_0011); ERC codes A02/A05/A06/A07/E17; Transaktionsgrund-aware date rules incl. the Gas 6-week retro window; no I/O; used by processd NB module
crates/mako-service/      **Service SDK** — cross-cutting infrastructure shared by all 16 daemons. Modules: `service` (**`run::<D>()` daemon lifecycle runner** + the `Daemon`/`ServiceConfig`/`ServiceContext` traits — a service `main` is one line; run() owns tracing, tuned pool, migrate, real `/health/ready`, infra routes, graceful serve, `--check`), `error` (`ApiError`/`ApiResult` — `?`-friendly HTTP errors → JSON problem body, `From<sqlx::Error>`), `cloudevent` (`CloudEvent::new`, `source(svc, tenant)` → `urn:mako:{service}:tenant:{tenant}`, `post_ce_with_retry`), `outbox` (`enqueue`, `OutboxWorker`, `ensure_schema`, `list_dead_letters`, `requeue` — persist-before-dispatch), `config` (load_config, DatabaseConfig, HttpConfig, TOML+env+_FILE layering), `shutdown` (token() SIGINT+SIGTERM, serve() graceful drain), `oidc` (OidcConfig, OidcVerifier, build_verifier() factory, looks_like_jwt routing, opt-in service-to-service keys via `[[oidc.service_keys]]` — opaque Bearer → service principal, for internal edmd/marktd calls), `mcp_auth` (McpAuth, McpAuthConfig, McpApiKey SecretString, McpIdentity, from_auth_config[_oidc]() factories), `telemetry` (init_tracing_from_env, init_tracing, OtelConfig), `cedar` (CedarEnforcer), `health` (health_routes), `http` (default_client 5s+30s timeouts), `webhook` (`sign`, `verify_hmac`, `hmac_hex` — one canonical HMAC-SHA256 signer, `sha256=<hex>`), `builder` (ServiceBuilder), `event_bus` (EventBus, WebhookBus), `metrics`, `rate_limit`
crates/grid-billing/       Role-neutral **grid settlement engine** — NNE/KA/MMM/MSB/AWH for PIDs 31001/31002/31005/31006/31009/31011; `settle_nne`, `settle_mmm`, `settle_msb`, `settle_gas_awh`, `reverse`, `correct`; **§14a EnWG per BNetzA BK6-22-300**: Modul 1 = *pauschale Reduzierung* (`Modul1Pauschal`), Modul 2 = *prozentuale Arbeitspreisreduzierung* on the device's separately metered energy (`Modul2ProzentualeReduzierung`), Modul 3 = *zeitvariable Netzentgelte* with **three** Tarifstufen HT/ST/NT (`Modul3ZeitVariabel`, from 01.04.2025). Modul 2 ⊕ Modul 3 (`Sect14aModule::combinable_with`); a spot-linked NNE is `SpotpreisNetzentgelt` and is **not** a §14a module; Gas Grundpreis (`nne_grundpreis_eur_per_month`/`nne_grundpreis_months`); returns **`SettlementResult`** (canonical output — **no rubo4e dep**) carrying **`korrektur_grund: Option<KorrekturGrund>`** (Messwertkorrektur/Tarifkorrektur/Stammdatenkorrektur/RegulatorischeAenderung/Rechenfehler/Clearing/Sonstiges — *why* a recalculation happened, a settlement fact; the invoice numbers saying *what* was replaced stay on `InvoiceDocument`). `reverse()`/`correct()` require the reason; `lineage_is_consistent()` catches a Correction with none; `indicates_defect()` separates engineering signal from lawful recalculation; every `SettlementPosition` carries `CalculationTrace { explanation, legal_refs: Vec<LegalReference>, tariff_source: Option<TariffSource>, gross_eur, … }` + **`kind: BillingPositionKind`** (bridges to `BdewArtikelnummer` in service layer) + **`artikel_id: Option<String>`** (BDEW Artikel-ID for AWH Gas 2-01-7-xxx and NNE Strom from tariff sheet); **`BillingPositionKind`** (24 variants, rubo4e-free); **BDEW Codeliste Artikelnummern und Artikel-ID v5.6** (valid 01.09.2025): NNE Gas/MMM/KA use classic `artikelnummer` (9990001…); NNE Strom (BK6-20-160) uses `artikel_id` from PreisblattNetznutzung; AWH Gas (PID 31011) uses `artikel_id` 2-01-7-001..006; service-layer `kind_to_artikelnummer()` maps BillingPositionKind → BdewArtikelnummer; `Sparte { Strom, Gas }` drives `LegalReference` (StromNEV §21 vs GasNEV §14) + `SettlementType` + default PID automatically; **`Sect14aModule { Modul1, Modul2, Modul3 }`** typed in `LegalReference::Sect14aEnwg`; **`SettlementType::MmmGas`** separate from `MmmStrom` for correct GaBi Gas 2.1 (BK7-24-01-008) legal refs; `KaKlasse { TarifkundeLow, TarifkundeMedium, SonderkundeHigh, Exempt }` annotates KAV §2 tier; `QuantityUnit { Kwh, Kw, Kvarh, Kvar, Monat }`; **Blindmehrarbeit** (`NneInput.blindarbeit: Option<Blindarbeit>` — charged only on the excess over the Preisblatt's free share; `Blindarbeit::COS_PHI_0_9` = tan φ 0.4843 is the documented default but the share is an *input* since many Preisblätter round to 50 %; legal basis is the NB's Preisblatt under **StromNEV §17**, NOT §18 dezentrale Erzeugung); `sender_mp_id`/`recipient_mp_id` auto-populated from input — the NB sends NNE/MMM/KA, but PID 31009 is issued **by the MSB** to the NB/LF/ESA (`MsbInput.empfaenger`), so the two are named for their role rather than for one of the roles that fills them; `LegalReference` has 13 variants incl. `ARegV`; `validate_mmm_input`/`validate_msb_input`/`validate_gas_awh_input` → `ValidationResult` (NNE validates inline); **partial HT/NT validation** (`PARTIAL_TOU_FIELDS`); the `ArbeitspreisModell` enum holds one model at a time, so module combinations are structurally exclusive — note BK6-22-300 *permits* Modul 1 + Modul 3, which is therefore not yet representable; `debug_assert_eq!(total_eur, recomputed_total())` inside all settle functions; **§13a EnWG Redispatch-Vergütung** (`redispatch::redispatch_verguetung` — entgangene Einnahmen + zusätzliche − ersparte Aufwendungen, net may be negative per Satz 4; `eeg_entgangene_einnahmen` for the Nr. 5 EEG basis). **`AusfallarbeitBasis` is a required input**: Duldungsfall derives the curtailed energy from the measured Lastgang, Aufforderungsfall from the schedule transmitted to the EIV — resolving both from the Lastgang settles against what happened rather than what was instructed. The basis is carried into the result and its trace; service layer (netzbilanzd/invoicd) owns `into_rechnung()` conversion; no float money; invoic-checker validation lives in netzbilanzd, not in this crate.
crates/eeg-billing/       Pure EEG/KWKG feed-in settlement library — `calculate_settlement()` for all 10 settlement schemes (`SettlementScheme + TariffSource`); EEG-version-aware rules via `EegGesetz` enum (Eeg2000–Eeg2023+Kwkg); §51 Negativpreisregel with Bestandsschutz (§100 Abs. 1 Satz 4 EEG 2017 boundary 2016-01-01; EEG 2017: 6h/Wind<3MW/other<500kW; EEG 2021: 4h/<500kW; EEG 2023: any/<100kW); **Anlage 1 gleitende Marktprämie** = `max(0, AW − Monatsmarktwert)` (no additive Managementprämie — marketing costs are folded into the AW, mirrored by the §53 Vergütungsabzug); **§13a EnWG Einspeisemanagement/Redispatch compensation** (`einspeisemanagement_kwh` field — §51 does not touch curtailed kWh, which were never fed in); **§36h Abs.1 Wind Korrekturfaktor + Abs.2 5-year Standortgüte re-eval** (`GuetefaktorReeval`, `korrekturfaktor_fuer_periode`, `wind_standort: Option<WindStandort>` or `wind_korrekturfaktor: Option<Decimal>` — explicit wins; `WindStandort` auto-derives); §52 Pflichtzahlungen via `SanktionsTyp` (€10/kW or €2/kW per §52 Abs. 3 Nr. 2) + `SanktionAlt` (3-tier old-regime) + §52 Abs. 6 Netting via `ReductionPipeline`; §49 semi-annual solar degression (fixed 1 % on 1 Feb / 1 Aug from 2024-02-01, compounded unrounded, kaufmännisch to the cent) via `degression::degressionsfenster + abgesenkter_wert`; §§20-22 Direktvermarktung rules via `direktverm::is_direktvermarktung_mandatory()` + `requires_ausschreibung()`; metering topology / Eigenverbrauch / §42b GGV allocation live in the external `metering` crate + edmd (NOT eeg-billing); `SettlementPeriodState` lifecycle (Active/Reduced/Suspended/PostEeg/Ended) via `settlement_state::derive_settlement_state()`; `RepoweringScope` (Full/RotorOnly/NacelleAndRotor/TurbineUnit — only Full resets Förderdauer); §53 Vergütungsabzug helper (0.4ct solar/wind, 0.2ct biomasse/gas); §54 Ausschreibungsreduzierung; `foerderendedatum_eeg()` = Dec 31 of year+20 (§25 Abs. 1 Satz 2); `ErzeugungsArt` enum (19 variants); `post_eeg_price_floor: Option<Decimal>` (configurable — negative EPEX not a statutory rule); uses `billing 0.13`: `LineItem::for_usage(Quantity::new(kwh, "kWh").with_code("KWH"), UnitPrice::rounded(6, MidpointAwayFromZero))` (EN 16931 BT-130 unit code + BO4E 6dp compliance), `billing::PricingModel` (`type Usage = ()`) for the settlement adapter (`EegSettleTariff`), `Amount::from_decimal_rounded` where a settlement product is rounded to 5dp, `FixedRateTax::exempt`/`zero_rated`/`.boxed()` for the VAT layers, `EuroAmount::into_decimal()`, `Amount::<5>::ZERO` in bridge; zero float money; no I/O; used by einsd
crates/energy-billing/    Pure multi-product **LF retail billing engine** (used by billingd) — STROM/GAS/WAERME/WASSER (Trinkwasser + gesplittete Abwassergebühr)/SOLAR/EEG/EINSPEISUNG/§14a WAERMEPUMPE+WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/SHARING §42c; §3 StromStG + §2 EnergieStG + BEHG CO₂ rates; §41a EPEX dynamic; §17 UStG Boni; BO4E Rechnung JSON (accounting) + opt-in `en16931` feature → `Invoice::to_en16931` EN 16931 semantic model (per-line VAT, rendered to XRechnung/CII + PEPPOL UBL via en16931-formats); zero I/O, zero async, no float money
crates/metering/          German energy metering domain library — `MeterInterval`, `Sparte`, `QualityFlag` (8 variants: Measured/Estimated/Substituted/Calculated/Corrected/Preliminary/Faulty/Unknown, billability per § 60 Abs. 2 MsbG); Gas m³→kWh_Hs (§25 Nr. 4 MessEV / DVGW G 685); billing period aggregation (Spitzenleistung § 12 StromNZV, HT/NT split with DST-correct CET/CEST via time-tz); SLP/RLM/iMSys classification (§3/§ 12 StromNZV, §41a EnWG); Mehr-/Mindermengensaldo (§ 13 StromNZV); Hampel quality scoring (QualityGrade A/B/C/F); **§ 60 Abs. 2 MsbG substitute value generation**: `fill_gaps()` (auto: linear for short gaps, carry-forward for long) + `fill_gaps_with_config(intervals, secs, from, to, &FillGapsConfig)` for explicit method selection; `FillGapsConfig { method: SubstituteMethod, prior_period_intervals, short_gap_threshold }` — supports `LinearInterpolation`, **`PriorPeriodAverage`** (same time-slot from prior-week data per § 60 Abs. 2 MsbG), `ZeroFill`, `LastValueCarryForward`; **V01–V10 validation engine** (`validate_intervals()` — Gap/Overlap/NegativeEnergy/ImpossibleSpike/ZeroRun/InconsistentInterval/DstAmbiguity/FutureTimestamp/NonBillableQuality/RegisterRollover); **`resample(intervals, ResampleConfig)`** — hourly/daily/monthly/yearly down-sampling with `ResampledBucket` (total_kwh, peak_kw, coverage_pct, has_missing_data); **`compute_virtual_meter(rule, sources)`** — Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation per §42b EnWG GGV (Solarpaket I); **`project_annual_consumption()`** — § 60 Abs. 2 MsbG Jahresprognose with optional prior-year seasonal correction; **`prior_period_substitutes()`** — § 60 Abs. 2 MsbG prior-period gap-fill with audit trail; **`SmgwSession`** (BSI TR-03109 gateway lifecycle, `has_valid_tls_cert()`, `expiring_certificates()`); **`ClsChannel`** (§14a EnWG CLS, `is_section_14a_compliant()`); **`MeasurementPoint`** (MaLo+MeLo+OBIS+`MarktRolle` binding); **`MeterRegister`** (Wandlerfaktor, HT/NT, active/inactive); **`PowerQualityInterval`** (DIN EN 50160: `voltage_out_of_range()`, `power_factor_below_threshold()`, `has_quality_issue()`); **`AggregationRule`** (Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation); zero I/O, no async, no float money + `zaehlzeit` (Zählzeitdefinition §14a, DST-correct register resolution), `rollout` (§29/§45 MsbG Pflichteinbaufälle + Fahrplan), BDEW-2025 SLP (H25/G25/L25/P25/S25 + Dynamisierung 4dp/3dp), `G685Rounding`, forecast 95% confidence bounds, V04 plant-capacity ceiling
services/makod/           Production daemon — assembles all modules; AS4 sender: fail-closed encryption (startup refuses missing decryption key / partner certs unless `--allow-unencrypted-as4`), signed `eb:Receipt` (+NRI) inbound, acknowledgement only after the counterparty's synchronous receipt verifies (`--as4-lenient-receipts` opt-out), outbound = full `UNB…UNZ` Übertragungsdatei (UNB MP-IDs ≡ NAD+MS/MR, DAR stable per outbox message), pre-send AHB validation gate, per-IP + per-sender-MP-ID rate limits, W3C traceparent persisted through the outbox
  services/makod/src/api/mcp_server.rs  MCP server (12 tools, 6 prompts, malo:// + partner:// resources) at /mcp — the inventory is pinned by `tool_inventory_tests`
services/marktd/         Market Data Hub — MaLo/MeLo/NeLo/TR/SR (typed `rubo4e::current` API responses for Marktlokation/Messlokation/Zaehler/Geraet; schema-validated on PUT); NB contracts with full BO4E `Vertrag` JSONB (`vertragsart`/`vertragsstatus` as indexed columns; `de.markt.nb-contract.updated` CloudEvent); Lokationszuordnung graph (lokationszuordnungen table, recursive-CTE BFS), preisblaetter, VersorgungsStatus, event_log replay, W3C traceparent forwarding; **konfigurationsprodukte** typed sub-resource on SteuerbareRessource (mandatory `produktcode` per BK6-24-174 §4.3, `de.markt.sr.konfigurationsprodukt.updated`); **ESA consent registry** (§49 Abs. 2 Nr. 9 MsbG): `esa_einwilligungen` (evidence-agnostic — evidence_uri/hash stored verbatim, never form-validated per BNetzA) + `esa_framework_agreements`; `POST/GET/DELETE /api/v1/esa/einwilligungen`, `PUT/GET /api/v1/esa/framework/{msb}/{esa}`; emits `de.markt.einwilligung.{erteilt,widerrufen}`; **Widerruf (Art. 7(3) GDPR) fires the 17008 Abbestellung** at makod (best-effort `wim.wertebestellung.abbestellen` + durable CE); **MMMA import worker** (monthly auto-import of Gas/Strom MMM settlement prices, configurable URL, `de.markt.mmma.import.{success,failed}` events); **ZeitvariablePreisposition** validation on PreisblattMessung PUT (mandatory `zaehlzeitregister`, rejects `bandNummer`); **GeraetKonfiguration typed sub-resource** (`geraet_konfigurationen JSONB NOT NULL DEFAULT '[]'` column + GIN index on `geraete` table; 16-variant `Konfigurationsparameter` enum covering MsbG §23 / BSI TR-03109 / §14a: `GET/PUT /api/v1/zaehler/{id}/geraete/{geraet_id}/konfigurationen`, `GET /api/v1/zaehler/{id}/geraete/{geraet_id}`; atomic replace with server-side dedup + `updated_at`; emits `de.markt.geraet.konfiguration.updated` CloudEvent; `CLS_FAEHIG` drives processd §14a auto-ack; `SMGW_CERT_ABLAUFDATUM` drives edmd cert-expiry worker); **Zaehlzeitdefinition typed endpoint** (`GET /api/v1/zaehler/{id}/zaehlzeitdefinitionen` returns `rubo4e::current::Zaehlzeitdefinition` assembled from zaehler_register + zaehler_saisons; `?valid_only=true` filter; wochentage → `Wiederholungstyp` mapping); **temporal integrity in the schema** (half-open `[valid_from, valid_to)` with `EXCLUDE USING gist` non-overlap constraints on rollenzuordnungen / melo_msb_zuordnungen / nb_contracts / all five Preisblatt tables, `UNIQUE NULLS NOT DISTINCT` on price-sheet natural keys, `btree_gist` required); **Strom Mehr-/Mindermengenpreise are one nationwide BDEW series** keyed by month alone (§ 13 Abs. 3 StromNZV — no per-VNB/ÜNB dimension); MaBiS-Zählpunkt is Strom-only (no `sparte`); lifecycle via `mako_service::run` (SIGTERM, figment env config, loopback `--check`, shared `/metrics`); Cedar coverage pinned by `tests/authorization_guard.rs` (unpermitted action = permanent 403; `Extension<Claims>` = permanent 500); **read-only MCP** (PRICAT dispatch is REST-only); RFC 3339 timestamps throughout; PostgreSQL 15+, OIDC/JWT, port :8180; **pure data hub — no domain policy**
  services/marktd/src/pg/      PostgreSQL implementations (PgLokationszuordnungRepository, PgTechnischeRessourceRepository, PgSteuerbareRessourceRepository, PgDeviceRepository, upsert_versorgungsstatus, PgPriCatRepository, PgMaloGridRepository, PgPreisblattKaRepository, PgZaehlzeitRepository); zaehler_register + zaehler_saisons tables defined in 0001_initial.sql; **ZaehlzeitRegister auto-populated from WiM Stammdaten (PIDs 17102–17133)**: `event_ingest` handler receives `de.mako.process.completed` from makod, looks up Zaehler by MeLo, upserts register + saison rows (deterministic UUID from register_id+saison+tagtyp+zeit_von for idempotency)
services/processd/        Process Decision Engine — NB An-/Abmeldung STP (netz-checker: `evaluate` for 55001/55077/44001 per EBD E_0622, `evaluate_abmeldung` for 55004/44004 per EBD E_0607 — separate ERC code spaces, A02 differs between the two trees; 55016 Kündigung is LFN→LFA and answered by no role here) + LF answers to NB-initiated GPKE processes — inbound **55007** = EBD E_0609 (gpke.nb-lieferende.*) and **55010** = EBD E_0624 (gpke.beendigung-zuordnung.*); triggers are inbound PIDs, never answer PIDs (pinned by tests/pid_contract.rs); business Fristen from `mako_gpke::antwortfrist` / `mako_geli_gas::antwortfrist` — wall-clock „HH:00 Uhr des 1. WT nach dem ÜT" (11:00/06:00/05:00/09:00) and „Ablauf des n. Werktags" (4 WT/3 WT), never flat 24 h or 10 WT (that is the *supplier's* Vorlauffrist); the same tables makod registers the process deadline from + LFN bootstrap Strom (POST /api/v1/start-supply, LFW24 ÜT date rule validated — day-granular, no time-of-day cutoff) + Gas (POST /api/v1/start-supply-gas, geli.lieferbeginn.anmelden) + Gas stornierung (geli.gas.stornierung.initiieren) + **MSB-Wechsel STP** split by answering role — NB answers 55042/55051, MSB answers 55039/55168 (55039 is MSBN→MSBA and never reaches the NB); per-PID Antwortfrist 3/5/7/1 WT, never flat + **REQOTE auto-response** (REQOTE_PIDS from mako-wim — 35003 is the ESA Werteanfrage and excluded; auto-fetches PreisblattMessung, dispatches QUOTES; `[msb] auto_preisanfrage` flag) + **§14a Steuerungsauftrag** (produktcode contract check against konfigurationsprodukte, auto-bestaetigen/ablehnen per BK6-24-174 §4.3) + **EoG gap closure** (§36/§38 EnWG: de.markt.versorgung.gap-detected → Grundversorger lookup → gpke.eog.anmelden; daily §38 3-month timer; GET /api/v1/eog; `[eog]` config); role-gated features (lf-only/nb-only/**msb-only**/integrated) pinned by tests/role_separation.rs; **Cedar on every REST route** pinned by tests/authorization_guard.rs; deciding principal recorded as approved_by/rejected_by; §20 EnWG parity; port :8580
  services/processd/src/nb_module.rs  NB STP evaluation (wraps netz-checker)
  services/processd/src/lf_module.rs  LF answers to NB-initiated GPKE processes (inbound 55007 NB-seitiges Lieferende, 55010 Beendigung der Zuordnung = EBD E_0624); 24 h business Frist, NOT the 45-min APERAK window
services/invoicd/         INVOIC plausibility-check daemon (LF role) — PIDs 31001/31002/31005/31006 (Strom GPKE, embedded Rechnung); 31003 (WiM Gas Rechnung, NB→LF); 31004 (WiM Gas Stornorechnung, auto-accept arithmetic-only); 31007/31008 (GaBi Gas MMM-Rechnung + MMM check 6 Gas); 31009 (WiM MSB-Rechnung, `InvoicCheckEngine::check_msb_rechnung()` + `PreisblattMessung`); 31011 (GeLi Gas AWH Sperrprozesse); auto-settles/disputes; persists receipts to PostgreSQL with indexed `malo_id` column (§ 147 AO / GoBD 3-year); `POST /api/v1/selbstausstellen/{malo_id}` (31006); `POST /api/v1/receipts/{id}/dispatch-remadv` (manual); `POST /api/v1/receipts/{id}/resolve-dispute`; `GET /api/v1/receipts/{id}/rechnung`; payment CloudEvents (de.invoic.receipt.settled/disputed) via `[erp] webhook_url`; check 6 Strom MMM uses MMMA Strom prices; check 6 Gas MMM (31007/31008) uses MMMA Gas prices (THE); **MCP: 7 tools, 4 prompts**; port :8280
  services/invoicd/src/pg/      PostgreSQL receipt persistence (upsert_receipt, mark_dispatched)
services/netzbilanzd/     NNE/KA/MMM/MSB/AWH billing daemon (NB role) — uses grid-billing to generate INVOIC 31001/31002/31005/31009/31011; invoice_drafts + kostenblatt_records + fremdkosten_records tables; draft status: draft→dispatched→paid|Dispute, draft→rejected; POST /billing/run (positions batch), PUT /dispatch, PUT /reject, PUT /mark-paid (REMADV 33001), PUT /mark-disputed (REMADV 33002); POST /webhooks/remadv (CloudEvent ingest); GET /billing/summary + GET /billing/audit (§ 147 AO / GoBD BNetzA export); POST /billing/mmm-run/{malo_id} (auto-fetches edmd imbalance + marktd MMM prices); POST /billing/ggv-nne/{ggv_malo_id} (§42a GGV NNE); redispatch kostenblatt (BK6-20-061 §4.2, 15th-of-month submit); Fremdkosten typed BO4E REST; Strom MMM auto-fetch from the nationwide BDEW series (§ 13 Abs. 3 StromNZV — no per-operator key); background workers: hourly dispatch-overdue alert + daily Kostenblatt deadline alert; CloudEvents: de.netzbilanz.invoic.{drafted,dispatched,paid,disputed,dispatch_overdue} + de.netzbilanz.kostenblatt.deadline-approaching; 13-tool MCP server + 6 prompts at /mcp; double-billing prevented by partial UNIQUE index; port :8680
services/sperrd/          Sperr-/Entsperrauftrag execution queue (NB role) — `sperr_orders` shaped by the ORDERS AHB (BGM Z51/Z52 order_type; **DTM+203 `ausfuehrung_am` XOR DTM+469 `fruehestens_am`**, mutually exclusive by CHECK; IMD+7081 `arbeitszeit` Z53/Z54; SG2 NAD+Z24 `treffpunkt_*`; SG29 FTX+ACB `hinweis`); **market inbox `POST /webhook`** consumes `de.mako.process.initiated` for PIDs 17115/17117 (17116 = NB→MSB Anfrage, deliberately not queued), deduplicated by `UNIQUE(tenant, process_id)`; typed `OrderType`/`OrderStatus`/`Arbeitszeit` enums (no loose strings, no leaked SQL errors); PUT `/execute` → IFTSTA 21039 `STS Z14` + `DTM+293 Fertigstellungsdatum` (refused if in the future — AHB 2.1 cond. [495]), PUT `/fail` → `STS Z13` with `pruefschritt_code` (SG15 STS DE9013 is a **Muss**), PUT `/cancel` (no IFTSTA); the claim is a single guarded UPDATE and a **failed dispatch keeps the report** and queues a retry (`iftsta_attempts` < IFTSTA_MAX_ATTEMPTS, FOR UPDATE SKIP LOCKED worker, same idempotency key), exhaustion → `de.sperr.iftsta.ausstehend` once; `GET /stats` splits `iftsta_outstanding` (in flight) from `iftsta_stuck` (needs a human); emits `de.sperr.{auftrag.eingegangen,ausgefuehrt,fehlgeschlagen,storniert,iftsta.ausstehend}` via the outbox; **OIDC + Cedar** (`policies/sperrd.cedar`, NB role, 4 actions) with `tests/authorization_guard.rs`; 4-tool **read-only** MCP (list_sperr_orders, get_sperr_order, get_sperr_stats, list_due_orders); **GPKE fixes no Werktage execution deadline** — BK6-22-024 §5's 24 h is the ORDRSP window (makod's), and the old "2 Werktage / §3.4 / §9" citations were fabricated; port :8780
services/einsd/           Einspeiser Registry + EEG/KWKG Settlement daemon — delegates to crates/eeg-billing; eeg_anlagen register (EegGesetz-typed, 19 ErzeugungsArt variants, consolidated schema `0001_schema.sql`); **10 settlement schemes** (SettlementScheme + TariffSource: FEED_IN_TARIFF, MARKET_PREMIUM, TENANT_ELECTRICITY, POST_EEG, EIGENVERBRAUCH, KWK_SURCHARGE, FLEXIBILITY_PREMIUM, FLEXIBILITY_SURCHARGE, TEMPORARY_FEED_IN_TARIFF, SONSTIGE_DIREKTVERMARKTUNG); **Anlage 1 gleitende Marktprämie** (no additive Managementprämie); MaStR lifecycle: `mastr_registriert` column + SEPA CT bank columns; **§52 violation start tracking** (`mastr_violation_start`, `fernsteuerbarkeit_violation_start` — auto-set on registration, cleared on confirmation); `foerderendedatum` = Dec 31 year+20 for statutory, exact 20y for Ausschreibung; §51 version-aware rules via EegGesetz; §52 SanktionAlt (old EEG) + SanktionsTyp Pflichtzahlung (EEG 2023, cumulative from violation start) + §52 Abs. 6 Netting; **§§53b–54 AW-Reduktionen** — cuts to the *anzulegender Wert*, applied before the settlement formula because the gleitende Marktprämie floors at zero: §53b Regionalnachweise (§79a, fixed **0,1 ct/kWh**, only where the AW is *gesetzlich bestimmt* — never a tender award) via `eeg_regionalnachweise`; §53c Stromsteuerbefreiung (per-kWh exemption for grid-transited electricity, CHECK-capped at the §3 StromStG rate 2,05 ct/kWh) via `eeg_stromsteuerbefreiungen`; §54 solar first-segment auction defects (Abs. 1/2 −0,3 ct, Abs. 3 −2,5 ct, Abs. 4 AW→0) via `eeg_sect54_solar_defekte`. Only the triggering facts are stored — every amount but §53c's is statutory. `GET/POST /api/v1/anlagen/{tr_id}/aw-reduktionen[...]` + MCP `get_aw_reduktionen`; **§13a EnWG EInsMan/Redispatch compensation** (`einspeisemanagement_kwh` settle request field — §51 does not touch curtailed kWh); **§21b Veräußerungsform Wechsel** (`POST /switch-veraeusserungsform` — monthly guard + mandatory threshold); **§ 147 AO / GoBD correction receipts** (`POST /settlements/{y}/{m}/correction` — `correction_of` + `settlement_receipt_history` snapshot); **derive_settlement_state** called after every settlement to drive `settlement_state` column; §49 semi-annual solar degression; §36h Abs.1 Wind Korrekturfaktor + §36h Abs.2 Standortgüte re-eval (year 6/11/16, `wind_guetefaktor_reevaluations` JSONB, `POST /wind-reevaluation`); §51 auto-derivation (edmd `GET /feed-in` × `epex_spot_prices` store overlay, §60-gated) + §51a Förderende-Verschiebung (`negative_price_qh_gesamt` → `effektives_foerderende`); metering classification (`metering_mode`); `SettlementPeriodState` lifecycle (`settlement_state` column, `settlement_state_transitions` table); `DirectvermarktungsPerioden` JSONB + `direktvermarktung_pflicht` flag; `RepoweringScope` (Full/Partial — only Full resets Förderdauer); `capacity_blocks` JSONB for §24 Erweiterung (deserialized and passed to eeg-billing); **Erlöschen von Zuschlägen** (`zuschlag_erloeschen_datum`, `award_expired`) — technology-specific §36e/§37e/§39e EEG 2023, *not* §55 Pönalen (a bidder↔ÜNB obligation outside settlement) and not §35a Entwertung; Repowering §22; **Zusammenlegung §24** — `eeg_billing::sind_eine_anlage` decides all of Abs. 1 (Satz 1's four cumulative conditions + Sätze 2–5 carve-outs; ownership is NOT a criterion) and `POST /zusammenlegen` answers 422 naming the deciding rule, backed by `standort_id`/`solar_montage`/`netzverknuepfungspunkt`/`biogaserzeugungsanlage_id` columns; KWKG Förderdauer; monthly auto-settle + batch settle; edmd auto-fetch; MCP /mcp (19 tools, 6 prompts); **`check_direktvermarktung_compliance`** (§3 Nr. 1 + §20 EEG: lists plants >100 kW settled under non-market scheme, §52 Abs. 2 Nr. 4 violation risk); **`check_sect44b_quota`** (§44b EEG 2023: annual biogas cap = leistung_kw × 0.45 × 8760 kWh, YTD tracking, alert at 75%/90%); eeg-agent + eeg-compliance-agent in agentd (triggers: `de.eeg.*` + `de.messwert.reading.direct.stored` for iMSys rollout detection; eeg-compliance-agent covers §52/§44b/§20 EEG compliance checks); port :9180
  services/einsd/migrations/ consolidated `0001_schema.sql` (eeg_anlagen + eeg_verguetungssaetze + settlement_receipts + mastr/bank columns + eeg_gesetz CHECK constraint); `emit_settlement_ce()` includes `bank_iban`/`bank_bic`/`zahlungsempfaenger` in `de.eeg.verguetung.berechnet` CE data so `accountingd` can generate pain.001 without secondary DB lookup
services/tarifbd/         Product & Tariff Catalog daemon (LF role) — **14 user-defined product categories** (STROM/GAS/WAERME/WASSER/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE/SHARING §42c); all prices in Tarifpreisblatt JSONB + version history; `product_status` DRAFT/PUBLISHED workflow (DRAFT invisible to billingd and comparison feed); OIDC/JWT auth on write endpoints; `dyn_source` CHECK (`epex-spot-day-ahead` only); `_version` validation (v202607.0.0); **30-value preistyp whitelist**; soft-delete `DELETE /api/v1/products/{lf}/{code}` (sets `valid_to = today`); customer_products (MaLo→product assignment, validates `assigned_from >= product.valid_from`, rejects DRAFT products); epex_prices (hourly §41a day-ahead, idempotent); Energiemix §42 EnWG per product (archived in `product_history` for §42 audit trail); Angebote B2B quotation pipeline (ANGELEGT→VERSANDT→ANGENOMMEN/ABGELEHNT/ABGELAUFEN); **`GET /api/v1/comparison-feed`** (comparison portal feed — ETag-cached, cursor-paginated, PUBLISHED non-expired only; extracts `TarifPreise` + `jahreskosten_supply_*` from JSONB; `Cache-Control: public, max-age=300`; §42d EnWG compliant); **MCP: 14 tools, 3 prompts** at /mcp; **tarifbd-agent** in agentd (triggers: `de.tarif.product.updated`, `de.tarif.angebot.abgelaufen`, `de.tarif.epex.missing`; §41a EPEX monitoring, §42 Energiemix completeness, Angebot hygiene); port :9080
services/billingd/         Energy Billing Engine (LF role) — pure calculation, all prices user-defined in tarifbd; **`Product` typed enum** (13 categories, serde-tagged `#[serde(tag="category")]`); per-category typed structs (`ElectricityProduct`, `GasProduct`, `HeatProduct`, `SolarProduct`, `ControllableLoadProduct` §14a, `WaterProduct` (Trinkwasser 7 % USt + gesplittete Abwassergebühr, Absetzungen incl. Schleppwasser), `SharingProduct` §42c, …); `Product::build_engine(&grid, &rates)` primary dispatch (no `Option<BillingEngine>`, no `PricingModel::try_from()`); **`ControllableLoadProvider`** for §14a WAERMEPUMPE/WALLBOX (delegates to `ElectricityProvider` + appends §14a credits); **`BillingEngine::validate()`** pre-flight regulatory check (no positions); **`BillingEngine::bill_batch()`** portfolio billing; **`Invoice.warnings: Vec<BillingWarning>`** + `has_errors()` + `has_warnings()`; **§41a iMSys guard** — hard `EngineError::ValidationBlocked` when `dynamic_epex + !Imsys`; **`StromsteuerBefreiung`** typed enum (§9 Nr. 1-5 + §9a); **`EnergieQuellen`** struct (CO₂ label §42 Abs. 2 Nr. 2 EnWG); BEHG L-Gas CO₂ factor `BEHG_CO2_FACTOR_L_GAS`; `GasProduct::gas_indexed_price` (TTF/NCG); `GasProduct::gas_leistungspreis_ct_per_kw_month` (RLM demand); `HeatProduct::waerme_indexed_price` (AVBFernwärmeV §24 Preisgleitklausel); dynamic `dynamic_epex_floor/cap_ct_kwh` clamp; `sofortbonus_eur`/`treuebonus_eur_per_year` → Bonus (§17 UStG Entgeltminderung); **`PositionTrace`** on every `BillingPosition` (formula, regulatory_basis, tariff_source); **`BillingWarning`/`WarningSeverity`** for non-fatal issues; **`CustomerKategorie`** (Haushalt/Gewerbe/Industrie); **`EnergyShareProvider`** §42c; **historic levy lookups** `stromsteuer_for_year` / `energiesteuer_gas_for_year` (heating gas constant 0.55 — the 2022 relief was the 7 % USt window, §28 Abs. 5/6 UStG) / `behg_ct_per_kwh_for_year`; **`Invoice::merge()`** Tarifwechsel; **`Invoice::allocate_proportionally()`** B2B; `prorate_days()` pro-rata; `billing_run_id` audit; `kleinunternehmer_19_ustg` → 0% USt on feed-in Gutschrift (§19 UStG); consumption always standard-rated; `MeteringMode` (SLP/RLM/iMSys); `eeg` optional feature; **no `rubo4e` dep** (bo4e feature removed); `to_rechnung_json()` BO4E JSON; **deterministic risk gate** (`risk.rs` — every invoice scored inline, banded HOLD requires `POST …/release`); **§40b EnWG billing runs** (`billing_runs.rs` + `[billing_runs]` config — monthly/quarterly cycles + multi-period catch-up (bills every missed period, not just the latest) + iMSys monthly Abrechnungsinformation CloudEvent); kaufmännische Rundung single-sourced via `billing::RoundingStrategy::MidpointAwayFromZero`; **EN 16931 e-invoicing**: `energy_billing::Invoice::to_en16931` semantic model stored in `billing_records.en16931_json`, `einvoice` module renders XRechnung/CII + PEPPOL UBL via `en16931`/`en16931-formats` (`reconcile` derives BG-23; hand-rolled `xrechnung.rs` retired), per-line VAT (BT-151/152), `/submit-b2g` validates against the XRechnung 3.0 profile before dispatch (`seller_iban`/`seller_bic` → BG-16 SEPA); **12 MCP tools** (`validate_tariff_config`, `explain_invoice_position`, …); zero I/O; `POST /api/v1/billing/{malo_id}/tarifwechsel`; **VPP auto-billing**: `vpp_contracts` table (SR-ID → capacity_price_eur_per_kwh), `vpp_dispatch_ledger` (idempotency), `POST /api/v1/webhooks/vpp-dispatch` (receives `de.vpp.dispatch.confirmed`, HMAC-verified, computes `flexibility_kwh = max_power_kw × duration_h`, generates `Rechnung`, emits `de.vpp.settlement.berechnet`), `vpp_auto_billing` config flag; migration 0001; port :9280
services/accountingd/      Massenkontokorrent / Customer Account Ledger (LF role) — **ledger = the `doubleentry` crate** (immutable, tamper-evident double-entry: BLAKE3 Merkle log + inclusion/consistency proofs, period seals GoBD/§146 AO, open-item clearing, store-level idempotency) in the `doubleentry` PG schema of the same DB; accountingd owns the chart of accounts (`ledger::Chart`: per-MaLo Kontokorrent Asset leaf + GL contras Bank/Erlöse/Mahnerlöse/**Zinserträge**/EEG-Aufwand/Erstattungen; Verzugszinsen book to Zinserträge under their own `VERZUGSZINSEN` Buchungsart, §275 HGB keeps them off the Mahnerlöse line) and the `entry_type→postings` map; **every money movement flows through `pg::post_entry` → `ledger.post`**; **Festschreibung** `POST /api/v1/periods/{id}/seal` (GoBD §146 AO — seals a period, then backdated bookings into it are refused) + `GET /api/v1/periods/seals` (chain verify) + `GET /api/v1/entries/{id}/proof` (Merkle inclusion proof); **authoritative Offene-Posten** via recorded FIFO clearing (`ledger.apply_fifo_clearing` on every post; `POST .../clear`, `POST /clearings/{id}/reset`) + **Summen- und Saldenliste** `GET /api/v1/trial-balance` (§238 HGB, Σ Soll = Σ Haben)** (idempotency key required: ce_id / `bank:{txn}` / `mahngebuehr:{malo}:{stufe}:{date}` / `ABSCHLAG-{malo}-{YYYY}-{MM}`); satellites in `public`: accounts (master data + `balance_ct` **ledger-derived read cache**, set absolutely from the ledger net so it can't drift) + sepa_mandates (**`UNIQUE(tenant,mandatsref)`**) + dunning_cases + **eeg_payout_orders** + payment_plans/installments + dunning_locks + forderungs_einwaende + interest_charges + ecb_base_rates + bank_import_log + anonymization_log + auto_dunning_runs + sepa_collection_runs + jahresabschluss_runs + account_audit_log; **OIDC/JWT + Cedar ABAC on every endpoint** (`policies/accountingd.cedar`, 11 actions, reads split `read-account`/`read-banking`/`read-books`; `tests/authorization_guard.rs` — 24 handlers previously named no `Claims` extractor at all, serving balances, Kontokorrent, mandates, IBANs, the trial balance and pain.001 XML anonymously); **inbound HMAC-SHA256 verification** on `/webhook` (`sha256=` prefix, constant-time compare; a CloudEvent id is required); `iban_hash` = app-layer **keyed BLAKE3** (pgcrypto dropped); **ABSCHLAG = advance-payment credit** (full-cost Jahresrechnung debit; balance nets to Nachzahlung/Erstattung); Jahresabschluss idempotent + **refund pain.001 on Erstattung**; business-partner aggregation (`kunden_nr` → `/business-partners/{nr}/{accounts,balance}`); **§§41f/41g EnWG disconnection sequence** — Androhung (Abs. 1, 4 Wochen) → Ankündigung (Abs. 5, 8 Werktage) → Sperrauftrag (**ORDERS 17115 via makod**) → Entsperrauftrag (Abs. 7, **17117**, once the grounds fall away); every phase re-evaluates the Abs. 3 gates against the **live ledger** and the Verzug excludes MAHNGEBUEHR/Verzugszinsen (Verzugsschaden); the dunning worker's first step closes cases whose receivable is settled (nothing did that before, so a paid-up customer was escalated to Mahnstufe 3 and disconnected); notices carry Abs. 6 Grund + voraussichtliche Kosten and the Abs. 4 avoidance options; §41g Abs. 1 S. 2 offer recorded (`/abwendung/angebot`); **all halts are `dunning_locks`** (grund abwendungsvereinbarung|schutzbeduerftigkeit|zahlungsaussicht|operator + rechtsgrundlage + valid_from/valid_to + created_by; lifted with a reason; `vereinbarung_gebrochen` = §41g Abs. 1 S. 11 → clears the Ankündigung + emits `de.accounting.abwendung.gebrochen`; open-ended ones listed at `/dunning/locks/review`) — replacing three set-only timestamps that nothing could lift; **`forderungs_einwaende`** (§41f Abs. 3 S. 3–5: forderung_bestritten|preiserhoehung_bestritten|schlichtung|ratenzahlung_nicht_faellig) reduce the Verzug rather than halting; **`accounts.verzug_ct`** is a second ledger-derived cache (open debit residuals after FIFO clearing − Verzugsschaden − open Einwände, floored at 0, set absolutely, refreshed post-clearing); `GET /metrics` financial gauges; advisory-locked workers; SEPA pain.008 as **one message with one `PmtInf` group per (scheme × SequenceType)** — `sepa_mandates.scheme` CORE|B2B (different rulebooks: CORE has the 8-week refund right, B2B none), recorded on `sepa_collection_entries` too so a pain.007 restates the original as submitted; **EPC 36-month dormancy** via `last_presented_at` (stamped on presentation, not settlement — the clock resets on rejected/refunded collections too), dormant mandates excluded from runs + `GET /api/v1/sepa/mandates/dormant`; pre-notification **14 calendar days** (`sepa_pre_notification_days`, previously read by nothing while the worker hard-coded 5) (sepa 0.5 `DirectDebitGroup`; typed `IsoDate` dates; config-selectable schema version via `pain008_schema`/`pain001_schema` — `DirectDebitSchema`/`CreditTransferSchema`, validated at startup; mandatory `creditor_id` Gläubiger-ID EPC AT-02; `mandatsref` CHECK ≤35 chars = Max35Text EndToEndId), **persisted** in `sepa_collection_runs` (one row per collection date); `FRST→RCUR mandate auto-transition` after first collection; `jahresabschluss_runs` idempotency; `account_audit_log` (§238 HGB traceability); `erp_hmac_secret` as `SecretString`; pain.001 SCT credit-transfer via `build_pain_001(instant, schema)`; **`EegConfig`** + **`eeg_payout_orders`** SCT_INST/SCT_CORE pipeline; auto-dunning `auto_dunning_runs` idempotency; camt.054 **XML** import (`POST /api/v1/payments/import/camt054`, batch-booked entries expanded per TxDtls, returns → BANKRUECKLAST) + simplified-JSON import; IBAN ISO 13616 mod-97; `Vorauszahlung` + `Zahlungsinformation` typed BO4E; emits de.accounting.{mahnung.issued, sperrauftrag, entsperrauftrag, abwendung.angeboten, abwendung.gebrochen, bankruecklast, payment.imported, abschlag.posted, interest.charged, payment.due, erstattung.faellig, sperrandrohung, sperrankuendigung, eeg.payout.rejected} — all via the transactional outbox; the Sperr-/Entsperrauftrag is an **ORDERS 17115/17117 dispatched through makod**, the CloudEvent is its announcement; port :9380
services/vertragd/         Contract & Customer Management (LF role) — Kunden (B2C + B2B) with kunden_identitaeten (N OIDC logins per company, max 50 per Kunde; rolle=VOLLZUGRIFF/ADMIN/FINANZEN/TECHNIK/READONLY; `letzter_login` updated on every authentication; standort_filter for site-scoped B2B access); Rahmenverträge (B2B portfolio: Sammelrechnung, indexation, volume discount, `angebot_id` CPQ traceability linkage; cascade Kündigung via `POST /rahmenvertraege/{id}/kuendigen`); Versorgungsverträge per site/commodity (ANGELEGT→IN_BEARBEITUNG→TEILERFUELLUNG→AKTIV→GEKÜNDIGT→ABGELAUFEN→STORNIERT); OIDC/JWT auth on all write endpoints; Lieferbeginn dispatch with 3× exponential-backoff retry; Kündigung Widerruf via `POST /vertraege/{id}/widerruf-kuendigung` (reverts GEKÜNDIGT→AKTIV); Tarifwechsel endpoint (§41 EnWG) with **Preisgarantie guard** (blocks price changes within `preisgarantie_bis` window; `override_preisgarantie=true` operator bypass — logs to immutable `preisgarantie_override_log` with JWT sub); `PUT/GET /api/v1/vertraege/{id}/preisgarantie` typed `rubo4e::current::Preisgarantie` BO4E COM; `PUT/GET /api/v1/kunden/{id}/person` typed `rubo4e::current::Person` BO4E (GDPR Art. 15); **`PUT/GET /api/v1/kunden/{id}/zahlungsinformation`** typed `rubo4e::current::Zahlungsinformation` COM (IBAN mod-97 validated); **`GET /api/v1/kunden/{id}/export`** GDPR Art. 15/20 full PII export; **`POST /api/v1/kunden/{id}/anonymize`** GDPR Art. 17 irreversible pseudonymization (immutable `anonymization_log`); `POST /api/v1/vertraege/{id}/stornieren` pre-activation cancel; Kündigung with coordinated Schlussablesung; auto-renewal background worker; §41 Abs. 3 EnWG 42-day Preisanpassungsbenachrichtigung worker; **proactive expiry worker** (`de.vertrag.ablauf.ankuendigung` 30 days before `vertragsende` or `preisgarantie_bis`); OIDC sub → MaLo authorization gateway (`GET /kunden/authenticate`) for portald; consolidated schema `0001_schema.sql`; **16-tool MCP server + 4 prompts** (incl. GDPR erasure workflow, Preisgarantie dispute resolution); HMAC-SHA256 on all CloudEvent webhooks (`sha256=` prefix per workspace standard); port :9780
services/mabis-syncd/      MaBiS Summenzeitreihe synchronisation daemon (ÜNB/NB role) — aggregates per-MaLo quarter-hourly Lastgang from edmd via `mako-mabis::SummenzeitreiheBuilder` (typed `metering::MeterInterval`, ¼-h slot length enforced); files one Summenzeitreihe per Bilanzierungsgebiet with the BIKO as MSCONS PID 13003 via the makod command `mabis.summenzeitreihe.uebermitteln`; ascending version per (Bilanzierungsgebiet, Bilanzierungsmonat) and BIKO-assigned Datenstatus per BK6-24-174 Anlage 3 §3.8.2/§3.8.3; submits on the `erstaufschlag_werktag` (default 10. Werktag) at 05:00 UTC per §3.10; `submission_runs` + `submission_malo_log` + `pruefmitteilung` PostgreSQL tables; inbound `POST /api/v1/datenstatus` (IFTSTA 21003/21004) + `POST /api/v1/pruefmitteilung` (21000/21001) + `GET /api/v1/korrekturbedarf`; OIDC + Cedar (`read-mabis-run`, `trigger-mabis-run` NB/ÜNB only); retry with attempt_count < 3 guard; MaLo discovery via edmd `/api/v1/billing-periods`, Bilanzierungsgebiet and MaBiS-Zählpunkt (`GET /api/v1/bilanzierungsgebiete/{eic}/mabis-zp`) via marktd — a territory with no assignment **refuses** its submission rather than substituting the EIC; `submission_target` selects `biko-bilateral` (implemented) or `mabis-hub` (BK6-24-210, refuses until the Beschluss lands); **read-only MCP server** at /mcp (4 tools: `get_submission_status`/`list_failed_submissions`/`get_submission_run`/`list_korrekturbedarf`, 1 prompt `submission-triage` — filing stays behind the Cedar-authorised REST surface; agentd grants it as `mabis_syncd`, underscore, since agentplane refuses `-` in a tool server name); emits **`de.mabis.submission.failed`** and **`de.mabis.korrekturbedarf.opened`** through the transactional outbox (row + event commit together; `erp_webhook_url` drains it); REST reads serve RFC 3339 instants, never `time` component arrays; port :8880
services/agentd/          Multi-agent plane — **28 declarative manifests** in `agents/*.yaml` run by **agentplane** (journal-first durable runtime); manifests embedded via the `manifests!` macro, keyed by `metadata.name`; **`src/builtin/mod.rs` is the subscription table ONLY** (name + specialty + trigger_patterns — the procedure lives in the manifest, never in Rust; an empty trigger array is refused unless the specialist is declared manual/batch); `plane::Router` maps CloudEvent type → capability, `plane::Plane` runs one journaled run per subscribing specialist (**no race/first-wins mode**); **`plane::label` is the trust boundary** — a payload field is promoted to `Tainted::trusted` ONLY if re-validated against its identifier format (MaLo 11 digits, MP-ID 13, PID 5, MeLo 33, EIC 16, UUID, ISO date); everything else is `from_source(cloudevent:<type>)`; admission is always `run_correlated` with a `Tainted` value (a bare trusted input would make `protected_fields` vacuous, and a run outside a case cannot open an approval task); **three execution shapes**: 26 `tool-calling` (whole payload, per-field labels, no quarantined model unless memory formation selects one) + `gabi-gas-agent` `planned` (routing envelope of re-validated ids only; plan compiled before untrusted data is read; counterparty text read by the **quarantined** model in a `parse` step) + `deadline-alert-agent` **coded skill** (`models: {}`, no `execution` block, behaviour in `src/skills/`, and NO `max_tokens` — a zero ceiling refuses the first effect of any kind); **a `tool-calling` agent can never dispatch a mutating tool** (model output is untrusted by construction and the taint gate refuses it, even post-approval) — regaining dispatch means converting to `planned`; **`plane::tools`** connects one `McpClient` per `[mcp_servers]` entry over rmcp streamable-HTTP (a granted server missing from config = startup failure; keys use `_` not `-`, and required wiring follows the *compiled* specialists so a role build never demands the other arm's endpoints); `try_build` not `build`; one backend supplies all seven store seams (journal, cases, tasks, timers, events, push, memory — a manifest-declared seam left unwired is a build refusal); `Activation` honours `[bundled_agents] enable_all`/`enable` (unknown name = startup failure); **no per-agent config overrides** (manifest-owned, digest-covered); journal (redb or Postgres) = §147 AO record, sealed by the key ring, case = erasure unit; **oversight has two modes** — `approval` + `requires_approval` in front of a mutating call (`gabi-gas-agent` only) and `approval: none` + `oversight.triage` beside a completed answer (14 specialists); every role a manifest names must be admitted by `policy/agentd.cedar` (a test parses the manifests and fails otherwise); Cedar is fail-closed and every rule over a conditional context attribute needs `context has <attr> && …`; `protected_fields` bind `/malo_id`+`/pid`+`/mp_id` to trusted sources; all 28 `output.schema` are **closed** (`additionalProperties: false`); memory subjects bind `$correlation/malo` (only operator-wide scopes may be literals); **no DLQ, no RAG** (durability replaces retries); decisions deliver on the journal-backed push outbox, signed with Standard Webhooks (`webhook-signature`, by agentplane — the same scheme `mako_service::webhook` uses); **A2A Agent Cards** at `/.well-known/agents/{name}`; `GET /api/v1/agents`, `/agents/catalog`, `/decisions`; `/api/v1/oversight/*` mounted ONLY when OIDC is enabled; CloudEvent trigger + `POST /api/v1/run`; **inbound HMAC**; `max_sessions` semaphore (429); `session_timeout_secs` bounds the whole fan-out; **`SecretString`** for all secrets; guards: `xtask check-tool-grants`/`check-prompt-tools`/`check-wire-timestamps`; emits de.agent.decision.made (outcome: completed/failed/suspended/exhausted/quarantined/replanning/cancelled/not-admitted; carries run_id, waiting_for, tokens); port :9580
services/portald/          Customer Portal read-model gateway (LF role) — aggregates Lastgang (edmd), invoices (billingd), account balance (accountingd), VersorgungsStatus (marktd), EEG settlement (einsd) into single REST + SSE API; OIDC bearer-token authentication; `GET /portal/{malo_id}/dashboard`, `/lastgang`, `/invoices`, `/balance`, `/kontoauszug`, `/vorauszahlung`, `/eeg`, `/versorgung`, `/events` (SSE); §41 EnWG self-service write API: POST `/tarifwechsel`, POST `/kuendigen`, PUT `/kontakt`, PUT `/sepa`, GET `/invoices/{id}/download` XRechnung 3.0; 8-tool MCP server; shared `auth_client` (not per-request); port :9480
services/edmd/            Energy Data Management daemon — stores MSCONS meter readings (with `obis_code`), direct iMSys/SMGW push (`POST /api/v1/meter-reads/rlm/{malo_id}` + gas, idempotent on session_id), Hampel-filter quality scoring (k=3 t=3.0, grades A/B/C/F, retroactive rescore) + V01–V10 validation engine, Ablesesteuerung reading orders (INSRPT PID 23001 auto-creates `INSRPT_STOERUNG` orders), `GET /api/v1/deliveries/{malo_id}` returns `Vec<Energiemenge>` (BO4E typed), BO4E `Lastgang` + `Zeitreihe` export, `MeterBillingPeriod` (RLM spitzenleistung + Gas brennwert/zustandszahl), Mehr-/Mindermengen imbalance; **virtual meters** (Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation per §42b EnWG Solarpaket I, `GET /api/v1/virtual/{id}/lastgang`); **resampled Lastgang** (`GET /api/v1/lastgang/{malo_id}/resampled?resolution=HOUR|DAY|MONTH|YEAR`); **Summenzeitreihe** monthly aggregation for MaBiS (`GET /api/v1/summenzeitreihe/{malo_id}`); **§ 60 Abs. 2 MsbG annual forecast** (`GET /api/v1/forecast/{malo_id}`); **gas quality data** PID 13007 (`GET /api/v1/gas-quality/{malo_id}`); **§ 147 AO / GoBD bitemporal corrections** (`POST /api/v1/corrections/{malo_id}`); **quality assessments** per batch (`GET /api/v1/quality-assessments/{malo_id}`); **§14a Fernsteuerbarkeit compliance** (MsbG §21c / BSI TR-03109): `smgw_sessions` JSONB table (GIN-indexed) + `cls_compliance_log` audit table; `smgw.rs` module — `check_session_compliance()` pure (6 issue types: `CERT_EXPIRED`/`CERT_EXPIRING`/`TLS_CERT_MISSING`/`CLS_NOT_COMPLIANT`/`COMMUNICATION_FAULT`/`GATEWAY_REVOKED`), `spawn_cls_compliance_worker()` daily background task, `run_cls_compliance_sweep()`; endpoints: `PUT/GET /api/v1/smgw/{malo_id}` (upsert + immediate check), `GET /api/v1/smgw` (fleet list), `GET /api/v1/smgw/compliance` (read-only scan), `POST /api/v1/smgw/compliance/scan` (side-effecting); emits `de.messwert.cls.compliance-issue` CloudEvent (cert_serial/days_to_expiry/channel_id/severity); meterstore hot/cold tier (PostgreSQL recent window + Apache Iceberg V2 history, owned by the meterstore crate; edmd business tables stay in edmd's own pool); emits `de.messwert.reading.direct.stored` + `de.messwert.reading.quality.warning`; optional Kafka batch ingest (`[kafka_ingest]`, krafka consumer, at-least-once, same V01–V10 path); Cedar write actions role-gated (MSB/NB/admin, LF read-only); meterstore owns meter_reads/esa_typ2 storage & version-resolved reads; 15-tool MCP server (incl. `trigger_substitution`); port :8380
services/obsd/            Business-process observability daemon — process projections with automated deadline computation (GPKE 24h/WiM 7d/GeLi Gas 14d/MABIS 2d calendar-day approximations), `completed_at` cycle-time column, `GET /api/v1/audit/bnetza-report` (§20 Abs.1 EnWG Diskriminierungsbericht), BNetzA KPI reports; 6-tool MCP server (get_process, list_overdue_processes, get_kpi_report, get_parity_report, get_stp_rate, list_processes_by_family); Alertmanager bridge; PostgreSQL; port :8480
xtask/                    Build/codegen/validation tasks
makotest/                 Python test & simulation toolkit (PyO3 + maturin, abi3-py311).
                          Binds the same Rust crates the platform runs: BDEW identifier
                          check digits, Werktag/Fristen arithmetic, EDIFACT build +
                          interchange envelope + AHB validation. Pure-Python layer adds
                          EPEX curve generation, domain assertions and a `pytest11`
                          plugin. `just test-makotest`.
site/                     Zola documentation site (published to hupe1980.github.io/mako)
demos/                    Runnable end-to-end scenarios (nb-stp, eeg-billing)
Dockerfile                Multi-stage cargo-chef + distroless image for makod
.dockerignore             Docker build context filter
```

---

## Identifier check digits

BDEW identifiers carry check digits and mako validates them: **MaLo-ID** (11
digits, *Lok- und Waggon-Kennzeichnungsverfahren*: odd positions + even
positions×2, difference to the next multiple of ten — **not Luhn**) and **EIC**
(16 characters; object type `X` = Party/Bilanzkreis, `Y` = Area/
Bilanzierungsgebiet). `cargo xtask check-malo-ids` (in `just ci`) refuses a
literal with a wrong check digit anywhere in `crates/`, `services/`, `demos/`
or `site/content/`, using `metering` and `rubo4e` themselves so the guard cannot
disagree with the validators. Fixtures: `51238696012` is the canonical valid
MaLo; `51238696782` is the refusal fixture (wrong check digit) and is
allowlisted in the guard. Never invent an identifier — derive the check digit,
or take a published code.

## Build and Test

```bash
# Full CI gate — run before every commit:
just ci

# Individual gates:
cargo check --all-targets --all-features
cargo test --all-features
cargo test -p mako-engine --all-features
cargo test --test <name> --all-features
just test-edmd-db                     # edmd meterstore storage integration (testcontainers Docker)
cargo build -p makod --release
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo deny check

# xtask tasks:
cargo xtask bump-version X.Y.Z       # bump [workspace.package].version
cargo xtask codegen                   # regenerate profile Rust code from profiles/*/*/mig.json + ahb.json
cargo xtask validate-profiles         # validate all profiles against EDIFACT specs
cargo xtask validate-pruefids         # validate Prüfidentifikatoren (AHB check)
cargo xtask audit-ahb                 # audit Application Handbooks
cargo xtask check-release-coverage    # verify format-version coverage
cargo xtask check-bo4e-coverage       # verify rubo4e::current type count matches README (delta ≤ 2)
cargo xtask generate-fixtures         # regenerate EDIFACT test fixtures
cargo xtask extract-pdf               # extract tables from BDEW specification PDFs
cargo xtask import-codelists          # import BDEW code lists
cargo xtask import-xml-ahb            # import AHB rules from BDEW XML
cargo xtask release-diff              # diff between format versions
```

**`just ci` is the minimum gate before any commit.** It runs check + test + clippy
(including `clippy-roles`, which lints each role-scoped makod build — `--all-features`
enables every role at once and so can never catch role-gating mistakes)
+ fmt-check + deny + no-version-alias + doc-check + codegen-check + validate-profiles-strict
+ validate-pruefids-strict-ci.

**MSRV: 1.94** — do not use language features or stdlib APIs introduced after 1.94.

---

## Toolchain and Edition

- Rust edition: **2024** (all crates)
- Toolchain: **1.94** (pinned in `rust-toolchain.toml` — do not change to `stable`)
- Components: `rustfmt`, `clippy`

---

## Active Format Versions

Format releases ship on a **semi-annual cadence (April + October)**. Profiles are
**per-message and fv-dated** (`crates/edi-energy/profiles/<message>/fv<yyyymmdd>/`);
a message type only gets a new fv directory when its format actually changes in a
release.

| Release | Binding | Message types with changed formats |
|---|---|---|
| `fv20260401` | since 2026-04-01 (Mitteilung Nr. 54) | INVOIC, ORDERS, ORDRSP, ORDCHG, PARTIN, PRICAT, QUOTES, REMADV, REQOTE, UTILTS |
| `fv20261001` | from 2026-10-01 (Mitteilung Nr. 56) | UTILMD, MSCONS, APERAK, COMDIS, IFTSTA |

Message types untouched by a release keep their previous profile. Multiple format
versions coexist in the same engine instance simultaneously. A process started under
an older format version continues under those rules until it completes, even after
a cutover.

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

### Service architecture (daemons)
Every daemon builds on the `mako-service` SDK. Do **not** hand-roll the lifecycle.

- **Bootstrap.** `fn main()` is one line: `mako_service::run::<MyDaemon>().await`. Implement the
  `Daemon` trait — supply `type Config`, `const NAME`, `migrate(&PgPool)` (usually
  `sqlx::migrate!(...)` + `outbox::ensure_schema`), and `build(cfg, ctx) -> anyhow::Result<Router>`
  (assemble the domain router and spawn workers on `ctx.shutdown`). `run()` owns tracing, the pool,
  migrations, `/health/*`, infra routes, graceful shutdown, and `--check`. **Never** add health routes,
  bind a listener, or call `serve` inside `build`.
- **Config shape.** Embed a `[database]` block (`pub database: DatabaseConfig`) and implement
  `ServiceConfig` (`database() -> Option<&DatabaseConfig>`, `bind_addr()`). A flat
  `database_url: String` is obsolete. A stateless daemon returns `database() -> None`.
- **Pool.** Obtain the pool only from the runner (`ctx.pool()`), which comes from
  `DatabaseConfig::connect(url, NAME)` (tuned sizing + `application_name`). Never call bare
  `PgPool::connect` / `PgPoolOptions` in a service.
- **HTTP errors.** Handlers return `ApiResult<T>` and use `?`; construct failures with `ApiError`
  (`NotFound`, `unprocessable(..)`, `conflict(..)`, …). Never build ad-hoc `(StatusCode, Json)` error tuples.
- **CloudEvent emission.** Build with `CloudEvent::new(source(svc, tenant), TYPE, subject, data)`
  (type constants from `mako-events`) and send via `post_ce_with_retry`. Never hand-roll a
  `json!({"specversion": ...})` envelope or compute a signature inline.
- **Transactional outbox.** Durable emitters persist-before-dispatch: `outbox::enqueue(&mut tx, &ce)`
  inside the same transaction as the business write, drained by a background `OutboxWorker`. This is
  the Postgres `event_outbox` mechanism for **service→ERP/webhook** events — distinct from the
  mako-engine `AtomicAppend::append_with_outbox` slatedb outbox for **protocol APERAK/CONTRL** (below).
- **HMAC.** Sign and verify webhooks only through `webhook::sign` / `webhook::verify_hmac`
  ([Standard Webhooks](https://www.standardwebhooks.com/): `webhook-id`,
  `webhook-timestamp`, `webhook-signature: v1,<base64>` over
  `{id}.{timestamp}.{body}`). Never hand-roll the check: `verify_request` also
  refuses a stale timestamp and returns the id to deduplicate on, and both are
  the halves a local copy forgets.

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
| 31007–31008 | `mako-gabi-gas` (Aggreg. MMM-Rechnung Gas / selbst ausgest., NB → MGV; Gas-only; MGV is a Gas-domain role) | BK7-24-01-008 |
| 13013 | `mako-gabi-gas` `gabi-gas-mmma` (Allokationsliste Gas, MMMA, Gas-only) | BK7-24-01-008 |
| 17110, 19110 | `mako-gabi-gas` `gabi-gas-mmma` (ORDERS/ORDRSP Allokationsliste Gas, Gas-only; ⚡=— in AHB 1.0) | BK7-24-01-008 |
| 31009 | `mako-wim` (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1 — routed via wim-invoic to avoid double-registration) | BK6-24-174 |
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
PID 31010 (Kapazitätsrechnung, NB → BKV) is a GaBi Gas (BK7-24-01-008) billing process and belongs to `mako-gabi-gas`.
PID 31011 (Rechnung sonstige Leistung, NB → LF) is billed by the GNB/VNB to the LFN/LFA for performing AWH (abrechnungswürdige Handlungen) during the Sperrprozess — it is a GeLi Gas (BK7-24-01-009) billing, NOT GaBi Gas.

### MABIS vs Messwesen
MaBiS (`mako-mabis`) covers MSCONS **13003** + **13010–13012** (Bilanzkreisabrechnung
Strom, BKV↔ÜNB/BIKO, `mabis-billing`), the UTILMD Clearinglisten **55065/55069/55070**
(`mabis-clearingliste`), the ZP lifecycle **55062–55064 / 55071–55072 / 55197–55200 /
55203–55214** (`mabis-zp-lifecycle`), the ORDERS Anforderungen **17201–17208**
(`mabis-anforderung`), and the list/correction pairs **55195+55196, 55201+55202,
55223+55224** (`mabis-listenabgleich`). The remaining 130xx Messwesen PIDs are **not**
MaBiS — do not register them under `mako-mabis`.

Three traps in that band, all verified against the PID overview 4.0:
**55218/55220 are GPKE Teil 2** (Abr.-Daten NNA), not MaBiS; **55215–55217, 55219,
55221, 55222 do not exist**; and **55064 is the shared Antwort to both 55062 and
55063**, so an answer PID is never derived from the request by arithmetic.
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
| `EIV` | Einsatzverantwortlicher | Strom | Redispatch 2.0 (`mako-redispatch` engine; EIV party integration pending) |
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
- In `services/makod/src/core/party_registry.rs` the agency code is auto-derived from the GLN
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
| WiM Strom | **3 / 5 / 7 / 1 Werktage, per PID** (55039 / 55042 / 55051 / 55168) | `mako_wim::antwort_frist_werktage(pid)` → `fristen::deadline_at_werktage` | BK6-24-174 Teil 1 Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2 |
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

Docs are authored under `site/content/docs/**` (Zola) and published at
<https://hupe1980.github.io/mako/>. Links below point to the source `.md` files.

| Topic | Source |
|---|---|
| Architecture overview | [architecture](../site/content/docs/architecture/_index.md) |
| Domain model | [domain-model](../site/content/docs/architecture/domain-model.md) |
| Process engine guide | [engine](../site/content/docs/architecture/engine.md) |
| `makod` operator guide | [makod](../site/content/docs/services/makod.md) |
| `marktd` operator guide | [marktd](../site/content/docs/services/marktd.md) |
| `processd` operator guide | [processd](../site/content/docs/services/processd.md) |
| `invoicd` operator guide | [invoicd](../site/content/docs/services/invoicd.md) |
| `netzbilanzd` operator guide | [netzbilanzd](../site/content/docs/services/netzbilanzd.md) |
| `sperrd` operator guide | [sperrd](../site/content/docs/services/sperrd.md) |
| `einsd` operator guide | [einsd](../site/content/docs/services/einsd.md) |
| `tarifbd` operator guide | [tarifbd](../site/content/docs/services/tarifbd.md) |
| `billingd` operator guide | [billingd](../site/content/docs/services/billingd.md) |
| `accountingd` operator guide | [accountingd](../site/content/docs/services/accountingd.md) |
| `edmd` operator guide | [edmd](../site/content/docs/services/edmd.md) |
| `obsd` operator guide | [obsd](../site/content/docs/services/obsd.md) |
| `vertragd` operator guide | [vertragd](../site/content/docs/services/vertragd.md) |
| `portald` operator guide | [portald](../site/content/docs/services/portald.md) |
| `outputd` operator guide | [outputd](../site/content/docs/services/outputd.md) |
| `mabis-syncd` operator guide | [mabis-syncd](../site/content/docs/services/mabis-syncd.md) |
| `agentd` operator guide | [agentd](../site/content/docs/services/agentd.md) |
| `makotest` (Python bindings) | [makotest](../site/content/docs/reference/makotest.md) |
| MCP server (LLM tooling) | [mcp_server.rs](../services/makod/src/api/mcp_server.rs) · [makod guide](../site/content/docs/services/makod.md) |
| ERP integration (CloudEvents 1.0 webhooks, Command API) | [erp-integration](../site/content/docs/architecture/erp-integration.md) |
| Parsing guide | [parsing](../site/content/docs/reference/parsing.md) |
| Validation guide | [validation](../site/content/docs/reference/validation.md) |
| Builder patterns | [builders](../site/content/docs/reference/builders.md) |
| Annual release workflow | [annual-release-workflow](../site/content/docs/compliance/annual-release-workflow.md) |
| Schema versioning | [schema-versioning](../site/content/docs/compliance/schema-versioning.md) |
| API-Webdienste Strom | [api-webdienste](../site/content/docs/architecture/api-webdienste.md) |
| Release lifecycle | [release-lifecycle](../site/content/docs/compliance/release-lifecycle.md) |
| BNetzA regulatory reference | [bnetza](../site/content/docs/regulatory/bnetza.md) |
| PID reference | [pid-reference](../site/content/docs/regulatory/pid-reference.md) |
| Compensation / APERAK timeout flows | [compensation](../site/content/docs/architecture/compensation.md) |
