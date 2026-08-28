//! WiM Strom ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the WiM Strom workflow family:
    /// `wim-device-change`
    /// `wim-geraeteubernahme`
    /// `wim-invoic`
    /// `wim-insrpt`
    /// `wim-stammdaten`
    /// `wim-preisanfrage`
    /// `wim-preisliste`
    /// `wim-technik-aenderung`
    pub(super) async fn dispatch_wim(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── WiM Messstellenbetrieb — EDIFACT/AS4 channel ──────────────
            // PIDs 55042/55039: nMSB initiates (Anmeldung/Kündigung → NB) — spawn.
            // PIDs 55051/55168: NB initiates (Ende/Verpflichtungsanfrage → MSB) — spawn.
            //
            // NOTE: the REST API-Webdienste channel (WimOrderHandler in webdienste.rs)
            // is the primary transport for API-capable counterparties.  This arm
            // covers the AS4/EDIFACT path for counterparties that only support AS4.
            // MeLo ID is extracted from the first UTILMD transaction IDE segment
            // (object_id component) — the same field the wim_registry adapter uses.
            "wim-device-change" => match pid {
                // The MSB-Wechsel Anfragen, in **both Sparten**: 55039/55042/
                // 55051/55168 (WiM Strom Teil 1) and 44039/44042/44051/44168
                // (AWH WiM Gas 2.0). Same Use-Cases, same Fristen, one workflow.
                55_042 | 55_039 | 55_051 | 55_168 | 44_042 | 44_039 | 44_051 | 44_168 => {
                    let cmd = adapters::wim_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    // **No deadlines passed here.** Both windows depend on the
                    // PID and the Sparte, and the workflow returns them with the
                    // `ValidationPassed` event so they land on the same
                    // transaction: the business answer is 3 / 5 / 7 / 1 Werktage
                    // per Anwendungsfall, and the APERAK is 45 min in Strom
                    // against the Initial-/Folgeprozess split in Gas
                    // (APERAK AHB 1.1 §2.3.1/§2.4.1). Registering a Strom
                    // APERAK window on a Gas message would report a breach
                    // 45 minutes into a window that runs to noon the next
                    // Werktag.
                    self.spawn_or_resume_guarded::<WimDeviceChangeWorkflow>(
                        &melo_id,
                        "wim-device-change",
                        cmd,
                        &fv,
                        &[],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                }
                // UTILMD 44183 „Ende MSB von NB" — the Gas NB informing the MSB
                // of a Stilllegung. It asks nothing and is recorded against the
                // Messlokation's open process, or skipped when there is none.
                mako_wim::geraetewechsel::ENDE_MSB_VOM_NB_PID => {
                    let cmd = adapters::wim_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    self.resume_by_key::<WimDeviceChangeWorkflow>(
                        &melo_id,
                        "wim-device-change",
                        cmd,
                    )
                    .await
                }
                // Antwort PIDs close an order **we** sent — resume, never spawn.
                // An answer with no open order is Skipped rather than creating
                // an orphan stream. 44170 is absent: it does not exist under
                // FV2026-10-01 (PID-Übersicht 4.0 publishes 44168 → 44169 only).
                55_040 | 55_041 | 55_043 | 55_044 | 55_052 | 55_053 | 55_169 | 55_170 | 44_040
                | 44_041 | 44_043 | 44_044 | 44_052 | 44_053 | 44_169 => {
                    let cmd = adapters::wim_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    self.resume_by_key::<WimDeviceChangeWorkflow>(
                        &melo_id,
                        "wim-device-change",
                        cmd,
                    )
                    .await
                }
                // IFTSTA status/Vollzugsmeldung (21007, 21009–21018, 21029–21032) —
                // informational device-change status; resume by the MeLo carried in
                // the IFTSTA's single LOC (per the AHB profile). Never spawns.
                p if mako_wim::geraetewechsel::IFTSTA_PIDS.contains(&p) => {
                    let cmd = adapters::wim_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<WimDeviceChangeWorkflow>(
                        melo_id.as_str(),
                        "wim-device-change",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-device-change",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Geräteübernahme (nMSB role) ──────────────────────────────
            // PID 17001: Bestellung Geräteübernahmeangebot, MSBN → MSBA — spawn.
            // PID 17002: Weiterverpflichtung, NB → MSBA — spawn.
            // PID 17009: Anzeige Gerätewechselabsicht, MSBN → MSBA — spawn.
            // PIDs 19001/19002: ORDRSP Bestätigung/Ablehnung (MSBA → MSBN) — resume.
            //
            // 17005 and 17011 are deliberately absent — they are different
            // processes and `GERAETEUBERNAHME_PIDS` never registers them here:
            //   17005 Bestellung Rechnungsabwicklung MSB über LF (LF → MSB)
            //   17011 Bestellung Angebot Änderung Technik (NB/LF → MSB),
            //         owned by `wim-technik-aenderung`.
            //
            // Note: PIDs 19001/19002 are multi-domain — GPKE Konfiguration (NB role)
            // and WiM Geräteübernahme (nMSB role) share them.  Role-conditional
            // routing in the PidRouter ensures only one workflow is registered per
            // role (both cannot be active simultaneously — build() panics if both are).
            "wim-geraeteubernahme" => match pid {
                17001 | 17009 => {
                    let cmd = adapters::wim_geraeteubernahme_registry(self.sparte_of(msg))
                        .dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    // **No deadlines passed here.** Both windows depend on the
                    // Use-Case and the Sparte, and only the workflow knows both:
                    // 17001 answers 2 WT after the arrival ÜT, 17009 by the
                    // 2. WT *before* the Gerätewechseltermin the message
                    // carries, and the APERAK window is 45 min in Strom against
                    // the Initial-/Folgeprozess split in Gas. The workflow
                    // returns them with the `ValidationPassed` event, so they
                    // are registered on the same transaction.
                    self.spawn_or_resume_guarded::<WimGeraeteubernahmeWorkflow>(
                        &melo_id,
                        "wim-geraeteubernahme",
                        cmd,
                        &fv,
                        &[],
                        |s| !s.is_terminal(),
                    )
                    .await
                }
                19001 | 19002 | 19015 | 19016 => {
                    let cmd = adapters::wim_geraeteubernahme_registry(self.sparte_of(msg))
                        .dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    self.resume_by_key::<WimGeraeteubernahmeWorkflow>(
                        &melo_id,
                        "wim-geraeteubernahme",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-geraeteubernahme",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Rechnung (Strom) — PID 31009 ─────────────────────────────
            // PID 31009: MSB-Rechnung (MSB → NB, multi-domain GPKE/WiM) — spawn.
            "wim-invoic" => match pid {
                31009 | 31003 | 31004 => {
                    let cmd = adapters::wim_invoic_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_invoic(msg);
                    // The window is anchored on the Zahlungsziel the invoice
                    // carries in `DTM+265`, not on a fixed Werktage count, and
                    // where it sits relative to that date depends on who is
                    // paying: the NB answers the iMS-Rechnung by the 4. WT
                    // *before* it (WiM Strom Teil 1 Kap. 6.2 Nr. 2), everyone
                    // else *zum* Zahlungsziel (Kap. 3.6.3.8.2 Nr. 2/4,
                    // Kap. 3.7.2 Nr. 2/4, AWH WiM Gas 2.0 Kap. 4.7.2 Nr. 2/4/6).
                    // The 10 Werktage stated beside it is a sender-side floor on
                    // the Zahlungsziel itself, already baked into that date.
                    let zahlungsziel = faelligkeitsdatum_from_invoic(msg).unwrap_or_else(|| {
                        fristen::deadline_at_werktage(
                            OffsetDateTime::now_utc(),
                            fristen::vorlauf::ZAHLUNGSZIEL_MINDEST_WT,
                            HolidayCalendar::BdewMaKo,
                        )
                    });
                    let due_at = fristen::vorlauf::rechnung_antwort_spaetester_uet(
                        pid,
                        self.rechnung_empfaenger(msg),
                        zahlungsziel.date(),
                        HolidayCalendar::BdewMaKo,
                    )
                    .with_time(zahlungsziel.time())
                    .assume_offset(zahlungsziel.offset());
                    self.spawn_or_resume::<WimInvoicWorkflow>(
                        malo_id.as_str(),
                        "wim-invoic",
                        cmd,
                        &fv,
                        &[(mako_wim::INVOIC_SETTLEMENT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                // REMADV 33001–33004 (payer → MSB invoicer: payment confirmation
                // or itemized rejection) resume the billing process by the original
                // 31009 invoice reference (RFF+Z13). 33003/33004 are the Strom
                // itemized Abweisungen owned by mako-wim.
                33001..=33004 => {
                    let cmd = adapters::wim_invoic_remadv_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_remadv(msg);
                    self.resume_by_key::<WimInvoicWorkflow>(&invoice_ref, "wim-invoic", cmd)
                        .await
                }
                // COMDIS 29001 (MSB invoicer rejects the payer's REMADV) resumes
                // the same process by the invoice reference.
                29001 => {
                    let cmd = adapters::wim_invoic_comdis_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_comdis(msg);
                    self.resume_by_key::<WimInvoicWorkflow>(&invoice_ref, "wim-invoic", cmd)
                        .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-invoic",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM INSRPT (Strom) — PIDs 23001, 23003, 23004, 23008, 23011/23012 ──
            // PIDs 23001: INSRPT Anfrage Störungsmeldung (gMSB → NB) — spawn.
            // PIDs 23003/23004/23008/23011/23012: INSRPT Antwort — resume.
            "wim-insrpt" => match pid {
                23001 => {
                    // Neither Frist is a function of the PID. Strom states
                    // 3 Werktage for a kME ohne RLM or an mME and 1 for a kME
                    // mit RLM or an iMS (WiM Strom Teil 2 Kap. 1.2 Nr. 2); Gas
                    // states a flat 3, because it has no iMS rollout to branch
                    // on (AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 2). Neither number is
                    // in the message — the MSB's own device registry decides
                    // it. Until that lookup happens at ingest, the **shortest**
                    // window the Sparte can state is registered: an alert that
                    // fires early is visible, one that fires late is not.
                    // `Messtechnik::RlmOderImsMsHs` is that shortest branch in
                    // Strom, and in Gas every branch is the same 3 WT.
                    //
                    // The workflow returns the deadline with the event, so it
                    // is registered on the same transaction as the receipt.
                    let cmd = adapters::wim_insrpt_registry(
                        self.sparte_of(msg),
                        mako_fristen::antwort::Messtechnik::RlmOderImsMsHs,
                    )
                    .dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_insrpt(msg);
                    self.spawn_or_resume_guarded::<WimInsrptWorkflow>(
                        &melo_id,
                        "wim-insrpt",
                        cmd,
                        &fv,
                        &[],
                        |s| !s.is_terminal(),
                    )
                    .await
                }
                // 23005/23009 are the Gas Informationsmeldungen an den NB and
                // 23011/23012 the Strom Weiterleitung an betroffene
                // Marktlokationen. All four accompany an answer rather than
                // being one, so they resume without closing the process.
                23003 | 23004 | 23005 | 23008 | 23009 | 23011 | 23012 => {
                    let cmd = adapters::wim_insrpt_registry(
                        self.sparte_of(msg),
                        mako_fristen::antwort::Messtechnik::RlmOderImsMsHs,
                    )
                    .dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_insrpt(msg);
                    self.resume_by_key::<WimInsrptWorkflow>(&melo_id, "wim-insrpt", cmd)
                        .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-insrpt",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Stammdaten ORDERS (PID 17132 inbound, 17102–17133 inbound) ──────
            "wim-stammdaten" => match pid {
                17132 => {
                    let cmd = adapters::wim_stammdaten_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    // The Geschäftsdatenanfrage is answered „spätester ÜZ ist
                    // 1 WT nach dem ÜZ" (GPKE Teil 4 § 3.2 Nr. 2). The APERAK
                    // runs beside it on 45 minutes (APERAK AHB 1.0 §2.4.1).
                    let process_due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE,
                        HolidayCalendar::BdewMaKo,
                    );
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume_guarded::<WimStammdatenWorkflow>(
                        &melo_id,
                        "wim-stammdaten",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_wim::stammdaten::STAMMDATEN_DEADLINE_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                        |s| !s.is_terminal(),
                    )
                    .await
                }
                // PIDs 17102–17133: Stammdatenübermittlung response (MSB → NB).
                // Extract ZAK+ZE+ZD register definitions and resume the existing
                // wim-stammdaten process started by PID 17132.
                // No new deadline — this message resolves the 5-Werktage window.
                17102..=17133 => {
                    let cmd =
                        adapters::wim_stammdaten_uebermittlung_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    self.spawn_or_resume::<WimStammdatenWorkflow>(
                        &melo_id,
                        "wim-stammdaten",
                        cmd,
                        &fv,
                        &[], // no new deadlines — resolves the existing 5WT window
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-stammdaten",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── WiM ESA Wertebestellung (WiM Teil 2 Kap. 4) ───────────────────
            //
            // ORDERS 17007 (Bestellung), 17008 (Abbestellung) and ORDCHG 39002
            // (Stornierung), plus the REQOTE 35003 Anfrage that opens the
            // handshake — 35003 is ESA-specific, so it is registered straight to
            // this workflow rather than being sorted out of the Preisanfrage
            // stream on content.
            name if name == mako_wim::wertebestellung::WORKFLOW_NAME => {
                if pid == mako_wim::wertebestellung::ANFRAGE_PID.as_u32() {
                    let cmd = adapters::wim_wertebestellung_registry().dispatch(raw, &fv)?;
                    // Consent gate: a revoked consent or an unestablished
                    // framework agreement answers the Werteanfrage with a
                    // QUOTES 15003 Ablehnung instead of an Angebot.
                    let cmd = self.gate_esa_consent(msg, cmd, None).await;
                    // The subscription is the (Meldepunkt, Messprodukt) pair:
                    // several Kapitel-4.6 products exist for one Marktlokation
                    // and an ESA may hold more than one at a time.
                    let malo_id = extract_malo_from_msg(msg);
                    let messprodukt = adapters::extract_messprodukt(msg).unwrap_or_default();
                    let subscription_key =
                        mako_wim::esa::business_key(malo_id.as_str(), &messprodukt);
                    // UC 4.1 Nr. 2: "spätester ÜT ist der 5. WT nach dem ÜT von
                    // Nr. 1". makod issues its positive AS4 Receipt for this
                    // message in the same request, so the dispatch instant is
                    // the ÜT the Frist counts from.
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        mako_wim::wertebestellung::ANGEBOT_FRIST_WT,
                        HolidayCalendar::BdewMaKo,
                    );
                    // Also index under the REQOTE's own Belegnummer: our QUOTES
                    // answer echoes it in `RFF+AAV`, and the ESA's ORDERS then
                    // references our Angebot.
                    let anfrage_belegnr = msg.message_ref();
                    self.spawn_or_resume_keyed::<mako_wim::wertebestellung::WimWertebestellungWorkflow>(
                        subscription_key.as_str(),
                        mako_wim::wertebestellung::WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[(mako_wim::wertebestellung::ANGEBOT_WINDOW_LABEL, due_at)],
                        &[anfrage_belegnr, malo_id.as_str()],
                        None,
                    )
                    .await
                } else if pid == mako_wim::wertebestellung::STORNIERUNG_PID.as_u32() {
                    // ORDCHG 39002 Stornierung carries no LOC — correlate by
                    // the original Bestellung's Belegnummer echoed in `RFF+ON`
                    // (`ZG-T51`). Resume only; a Stornierung without a running
                    // order is an orphan. Index it under its own Belegnummer
                    // too: our ORDRSP 19013/19014 answer echoes *that* in
                    // `RFF+ACW`, and the ESA keys its process on it.
                    let cmd = adapters::wim_wertebestellung_registry().dispatch(raw, &fv)?;
                    let key = esa_korrelation_key(msg, pid);
                    let cmd = self.gate_esa_consent(msg, cmd, None).await;
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        mako_wim::wertebestellung::ANTWORT_FRIST_WT,
                        HolidayCalendar::BdewMaKo,
                    );
                    let ordchg_belegnr = msg.message_ref();
                    self.resume_by_key_indexing::<mako_wim::wertebestellung::WimWertebestellungWorkflow>(
                        &key,
                        mako_wim::wertebestellung::WORKFLOW_NAME,
                        cmd,
                        &[(mako_wim::wertebestellung::ANTWORT_WINDOW_LABEL, due_at)],
                        &[ordchg_belegnr],
                    )
                    .await
                } else if pid == mako_wim::wertebestellung::BESTELLUNG_PID.as_u32()
                    || pid == mako_wim::wertebestellung::ABBESTELLUNG_PID.as_u32()
                {
                    let cmd = adapters::wim_wertebestellung_registry().dispatch(raw, &fv)?;
                    // A conformant ORDERS 17007/17008 carries **no LOC** —
                    // ORDERS AHB 1.1b §4.15 lists no Meldepunkt segment for
                    // either PID. It correlates by the reference the PID's own
                    // Zuordnungsschlüssel names: `RFF+AAG` (our QUOTES
                    // Angebotsnummer, `ZG-T24`) on the Bestellung, `RFF+ACW`
                    // (the ORDERS Bestellnummer, `ZG-T41`) on the
                    // Abbestellung.
                    let key = esa_korrelation_key(msg, pid);
                    // Consent can be revoked or expire between the Angebot and
                    // the Bestellung, and the registry is keyed on locations —
                    // so the location is read back off the running process.
                    let location = self.esa_location_of(&key).await;
                    let cmd = self.gate_esa_consent(msg, cmd, location.as_deref()).await;
                    // UC 4.1 Nr. 4 / Nr. 6 and UC 4.3 Nr. 2: 2 WT nach dem ÜT.
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        mako_wim::wertebestellung::ANTWORT_FRIST_WT,
                        HolidayCalendar::BdewMaKo,
                    );
                    // Index the process under this ORDERS' own Belegnummer too:
                    // our ORDRSP answer echoes it in `RFF+ON`, and a later
                    // ORDCHG Stornierung references it in `RFF+ON` as well.
                    let order_belegnr = msg.message_ref();
                    self.resume_by_key_indexing::<mako_wim::wertebestellung::WimWertebestellungWorkflow>(
                        &key,
                        mako_wim::wertebestellung::WORKFLOW_NAME,
                        cmd,
                        &[(mako_wim::wertebestellung::ANTWORT_WINDOW_LABEL, due_at)],
                        &[order_belegnr],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: mako_wim::wertebestellung::WORKFLOW_NAME,
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }

            // ── ESA Wertebestellung — ESA origination side ────────────────────
            // The MSB's answers come back inbound and resume the process this
            // ESA started. Never spawns — a stray response with no matching
            // process is Skipped.
            //
            // QUOTES 15003 still carries a LOC, so it correlates by MaLo. The
            // ORDRSP 19011-19014 answers carry no LOC and correlate by the order
            // reference echoed in RFF+ACW (the Belegnummer of the ORDERS/ORDCHG
            // this ESA sent, under which the process was indexed).
            name if name == mako_wim::esa_wertebestellung::WORKFLOW_NAME => {
                if mako_wim::esa_wertebestellung::ESA_INBOUND_PIDS
                    .iter()
                    .any(|p| p.as_u32() == pid)
                {
                    let cmd = adapters::esa_wertebestellung_registry().dispatch(raw, &fv)?;
                    // Every MSB→ESA answer correlates by the reference its own
                    // Zuordnungsschlüssel names, not by location: the QUOTES by
                    // `RFF+AAV` (our REQOTE), the ORDRSP 19011/19012 by
                    // `RFF+ON` (our ORDERS), the ORDRSP 19013/19014 by
                    // `RFF+ACW` (our ORDCHG) and the IFTSTA 21042 by
                    // `SG15 RFF+AGI` (our ORDERS). Only the QUOTES also carries
                    // a `LOC`, which is why it is the sole location fallback.
                    let key = esa_korrelation_key(msg, pid);
                    let outcome = self
                        .resume_by_key::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow>(
                            &key,
                            mako_wim::esa_wertebestellung::WORKFLOW_NAME,
                            cmd,
                        )
                        .await?;
                    // An ORDRSP 19011 answering a Bestellung is the moment the
                    // MSB's Angebot stops being an offer and becomes the price
                    // agreement — and it is the **only** price basis an ESA has
                    // for the INVOIC 31009 that follows (§35 MsbG leaves the
                    // Entgelt for a Zusatzleistung to be agreed per request;
                    // there is no published Preisblatt for a Kapitel-4.6
                    // Messprodukt). File it where `invoicd` looks.
                    if pid == mako_wim::esa_wertebestellung::BESTAETIGUNG_PID.as_u32() {
                        self.record_accepted_angebot(&key).await;
                    }
                    Ok(outcome)
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: mako_wim::esa_wertebestellung::WORKFLOW_NAME,
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── WiM Preisanfrage REQOTE (PIDs 35001/35002/35004/35005) ────────────────────
            "wim-preisanfrage" => {
                if mako_wim::preisanfrage::REQOTE_PIDS.contains(&pid) {
                    let cmd = adapters::wim_preisanfrage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Per-PID, from the same table processd sizes its operator
                    // queue by: 35001 → 4 WT, 35002 → 5 WT, 35005 → 10 WT.
                    // 35004 is GPKE Teil 3 and states no WiM window, so it
                    // carries no process deadline rather than a guessed one.
                    let deadlines = mako_wim::preisanfrage::antwort_frist_werktage(pid).map(|wt| {
                        (
                            mako_wim::preisanfrage::PREISANFRAGE_DEADLINE_LABEL,
                            fristen::deadline_at_werktage(
                                OffsetDateTime::now_utc(),
                                wt,
                                HolidayCalendar::BdewMaKo,
                            ),
                        )
                    });
                    self.spawn_or_resume::<WimPreisanfrageWorkflow>(
                        malo_id.as_str(),
                        "wim-preisanfrage",
                        cmd,
                        &fv,
                        deadlines.as_slice(),
                    )
                    .await
                } else if mako_wim::preisanfrage::QUOTES_PIDS.contains(&pid) {
                    // QUOTES 15001/15002/15004/15005: the MSB's Angebot answering our REQOTE —
                    // resume the process by MaLo (QUOTES carries it in LOC).
                    let cmd = adapters::wim_preisanfrage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<WimPreisanfrageWorkflow>(
                        malo_id.as_str(),
                        "wim-preisanfrage",
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "wim-preisanfrage",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── WiM Rechnungsabwicklung MSB über LF ──────────────────────────
            // ORDERS 17005 (Bestellung — terminal on receipt, nothing answers
            // it) and 17006 (Beendigung, either direction) spawn; ORDRSP
            // 19009/19010 answer a Beendigung *mako* sent and resume that
            // process by MaLo. Directions per BDEW PID overview 4.0 / AWH
            // Aktivitätsdiagramme WiM V1.3 §§2.8–2.11 (EBDs E_0206/E_0209).
            "wim-rechnungsabwicklung" => {
                if mako_wim::RECHNUNGSABWICKLUNG_ORDERS_PIDS.contains(&pid) {
                    let cmd = adapters::wim_rechnungsabwicklung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Only the Beendigung 17006 opens a window. 17005 is
                    // itself the answer to the Angebot — receiving it is the
                    // arrangement taking effect and nothing replies to it, so a
                    // deadline there would expire on a settled process.
                    let deadlines = if pid == 17006 {
                        vec![(
                            mako_wim::RECHNUNGSABWICKLUNG_DEADLINE_LABEL,
                            fristen::deadline_at_werktage(
                                OffsetDateTime::now_utc(),
                                fristen::antwort::RECHNUNGSABWICKLUNG_WERKTAGE,
                                HolidayCalendar::BdewMaKo,
                            ),
                        )]
                    } else {
                        Vec::new()
                    };
                    self.spawn_or_resume::<mako_wim::WimRechnungsabwicklungWorkflow>(
                        malo_id.as_str(),
                        "wim-rechnungsabwicklung",
                        cmd,
                        &fv,
                        &deadlines,
                    )
                    .await
                } else if mako_wim::RECHNUNGSABWICKLUNG_ORDRSP_PIDS.contains(&pid)
                    || pid == mako_wim::rechnungsabwicklung::RECHNUNGSABWICKLUNG_ABLEHNUNG_PID
                {
                    // 19009/19010 answer a Beendigung mako sent; IFTSTA 21032 is
                    // the LF refusing an Angebot mako sent. Both resume — with
                    // no open Vorgang there is nothing being answered.
                    let cmd = adapters::wim_rechnungsabwicklung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<mako_wim::WimRechnungsabwicklungWorkflow>(
                        malo_id.as_str(),
                        "wim-rechnungsabwicklung",
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "wim-rechnungsabwicklung",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── WiM Preisliste PRICAT (PIDs 27001–27003) ──────────────────────
            "wim-preisliste" => {
                if mako_wim::preisliste::PRICAT_PIDS.contains(&pid) {
                    let cmd = adapters::wim_preisliste_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Price list is a publish-only workflow; no statutory deadline.
                    self.spawn_or_resume::<WimPreislisteWorkflow>(
                        malo_id.as_str(),
                        "wim-preisliste",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "wim-preisliste",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── WiM Technikänderung (requester role) ─────────────────────────
            // mako initiates: ORDERS 17011 (Änderung der Technik, LF/NB → MSB)
            // and 17118 (Konfigurationsänderung, MSB → MSB) are rendered
            // outbound by the workflow's `SendAuftrag` command. The MSB-side
            // receiver for those two is not implemented — they are listed in
            // `SEND_ONLY_PIDS`.
            //
            // ORDRSP 19003–19007 close an order **we** sent — resume, never
            // spawn. An answer with no open order is Skipped rather than
            // creating an orphan stream.
            "wim-technik-aenderung" => {
                if mako_wim::TECHNIK_AENDERUNG_ORDRSP_PIDS.contains(&pid) {
                    let cmd = adapters::wim_technik_aenderung_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    self.resume_by_key::<WimTechnikAenderungWorkflow>(
                        &melo_id,
                        "wim-technik-aenderung",
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "wim-technik-aenderung",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── WiM Weiterverpflichtung (ORDERS 17002 → ORDRSP 19003/19004) ──
            //
            // NB → MSBA: keep operating a Messlokation whose Abmeldung has no
            // successor yet (WiM Teil 1 Kap. 2.4.2 Nr. 5). The MSBA answers
            // within **one** Werktag out of `E_0203`.
            mako_wim::weiterverpflichtung::WORKFLOW_NAME => match pid {
                mako_wim::weiterverpflichtung::AUFTRAG_PID => {
                    let cmd = adapters::wim_weiterverpflichtung_registry(self.sparte_of(msg))
                        .dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    // One Werktag, from the same table `processd` sizes its
                    // operator queue by and `obsd` raises the breach against.
                    let process_due_at =
                        mako_fristen::antwort::antwort_deadline(pid, OffsetDateTime::now_utc())
                            .unwrap_or_else(|| {
                                fristen::deadline_at_werktage(
                                    OffsetDateTime::now_utc(),
                                    1,
                                    HolidayCalendar::BdewMaKo,
                                )
                            });
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume_guarded::<WimWeiterverpflichtungWorkflow>(
                        &melo_id,
                        mako_wim::weiterverpflichtung::WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[
                            (
                                mako_wim::WEITERVERPFLICHTUNG_ANTWORT_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                }
                // 19003/19004 are *our* answer, rendered from the outbox. An
                // inbound one would be a counterparty answering an order we
                // never sent in this role, so it is skipped rather than
                // resuming a process it does not belong to.
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: mako_wim::weiterverpflichtung::WORKFLOW_NAME,
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── WiM Ersteinbau eines iMS (WiM Strom Teil 1 Kap. 3.5) ─────────
            //
            // IFTSTA 21029 opens the Vorgang and arms the wMSB's three-Werktage
            // answer window; 21030/21031 are that answer and resume it. The
            // Vorgang is keyed on the **Messlokation**, which is what the
            // rollout is about — the gMSB may have many open at once.
            mako_wim::ersteinbau::WORKFLOW_NAME => match pid {
                mako_wim::ersteinbau::VORABINFORMATION_PID => {
                    let cmd = adapters::wim_ersteinbau_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_malo_from_msg(msg);
                    // Three Werktage, from the same table `processd` sizes its
                    // operator queue by and `obsd` raises the breach against.
                    let process_due_at =
                        mako_fristen::antwort::antwort_deadline(pid, OffsetDateTime::now_utc())
                            .unwrap_or_else(|| {
                                fristen::deadline_at_werktage(
                                    OffsetDateTime::now_utc(),
                                    mako_fristen::antwort::ERSTEINBAU_ANTWORT_WERKTAGE,
                                    HolidayCalendar::BdewMaKo,
                                )
                            });
                    self.spawn_or_resume_guarded::<mako_wim::ersteinbau::WimErsteinbauWorkflow>(
                        melo_id.as_str(),
                        mako_wim::ersteinbau::WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[(mako_wim::ersteinbau::ANTWORT_WINDOW_LABEL, process_due_at)],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                }
                // 21030/21031 answer a Vorabinformation **we** sent. With no
                // open Vorgang they are skipped rather than spawning an orphan
                // — an answer to a rollout nobody announced decides nothing.
                mako_wim::ersteinbau::ZUSTIMMUNG_PID | mako_wim::ersteinbau::ABLEHNUNG_PID => {
                    let cmd = adapters::wim_ersteinbau_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<mako_wim::ersteinbau::WimErsteinbauWorkflow>(
                        melo_id.as_str(),
                        mako_wim::ersteinbau::WORKFLOW_NAME,
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: mako_wim::ersteinbau::WORKFLOW_NAME,
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
