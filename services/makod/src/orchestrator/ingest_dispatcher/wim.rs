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
                55042 | 55039 | 55051 | 55168 => {
                    let cmd = adapters::wim_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    // Business-answer Frist, per PID — 55039 → 3 WT, 55042 → 5 WT,
                    // 55051 → 7 WT, 55168 → 1 WT (BK6-24-174 WiM Strom Teil 1,
                    // Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2). A flat 5 WT here
                    // escalated the Abmeldung two days early and hid a missed
                    // Verpflichtungsanfrage for four.
                    //
                    // Distinct from the APERAK acknowledgement below, which is
                    // 45 min on weekdays (APERAK AHB 1.0 §2.4.1).
                    let frist_wt = mako_wim::antwort_frist_werktage(pid)
                        .expect("the match arm restricts this to the MSB-Wechsel family");
                    let process_due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        frist_wt,
                        HolidayCalendar::BdewMaKo,
                    );
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<WimDeviceChangeWorkflow>(
                        &melo_id,
                        "wim-device-change",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_wim::GERAETEWECHSEL_ANTWORT_FRIST_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                // Antwort PIDs (55040/55041, 55043/55044, 55052/55053, 55169/55170)
                // close an order **we** sent — resume, never spawn. An answer with
                // no open order is Skipped rather than creating an orphan stream.
                55040 | 55041 | 55043 | 55044 | 55052 | 55053 | 55169 | 55170 => {
                    let cmd = adapters::wim_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    self.resume_by_malo::<WimDeviceChangeWorkflow>(
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
                    self.resume_by_malo::<WimDeviceChangeWorkflow>(
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
                17001 | 17002 | 17009 => {
                    let cmd = adapters::wim_geraeteubernahme_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    // Process Frist: 5 Werktage (BK6-24-174 WiM Strom Teil 1).
                    // APERAK AHB 1.0 §2.4.1: Strom ORDERS — 45 min on weekdays.
                    let process_due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<WimGeraeteubernahmeWorkflow>(
                        &melo_id,
                        "wim-geraeteubernahme",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_wim::GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                19001 | 19002 => {
                    let cmd = adapters::wim_geraeteubernahme_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    self.resume_by_malo::<WimGeraeteubernahmeWorkflow>(
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
                31009 => {
                    let cmd = adapters::wim_invoic_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_invoic(msg);
                    // Settlement deadline: 5 Werktage (BK6-24-174 WiM Strom).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
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
                    self.resume_by_malo::<WimInvoicWorkflow>(&invoice_ref, "wim-invoic", cmd)
                        .await
                }
                // COMDIS 29001 (MSB invoicer rejects the payer's REMADV) resumes
                // the same process by the invoice reference.
                29001 => {
                    let cmd = adapters::wim_invoic_comdis_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_comdis(msg);
                    self.resume_by_malo::<WimInvoicWorkflow>(&invoice_ref, "wim-invoic", cmd)
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
                    let cmd = adapters::wim_insrpt_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    // INSRPT Frist: 5 Werktage (BK6-24-174 WiM Strom).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimInsrptWorkflow>(
                        &melo_id,
                        "wim-insrpt",
                        cmd,
                        &fv,
                        &[(mako_wim::insrpt::ANTWORT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                23003 | 23004 | 23008 | 23011 | 23012 => {
                    let cmd = adapters::wim_insrpt_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_utilmd(msg);
                    self.resume_by_malo::<WimInsrptWorkflow>(&melo_id, "wim-insrpt", cmd)
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
                    // Process Frist: 5 Werktage (BK6-24-174).
                    // APERAK AHB 1.0 §2.4.1: Strom ORDERS — 45 min on weekdays.
                    let process_due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<WimStammdatenWorkflow>(
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
                    let cmd = self.gate_esa_consent(msg, cmd).await;
                    let malo_id = extract_malo_from_msg(msg);
                    // UC 4.1 Nr. 2: "spätester ÜT ist der 5. WT nach dem ÜT von
                    // Nr. 1". makod issues its positive AS4 Receipt for this
                    // message in the same request, so the dispatch instant is
                    // the ÜT the Frist counts from.
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        mako_wim::wertebestellung::ANGEBOT_FRIST_WT,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<mako_wim::wertebestellung::WimWertebestellungWorkflow>(
                        malo_id.as_str(),
                        mako_wim::wertebestellung::WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[(mako_wim::wertebestellung::ANGEBOT_WINDOW_LABEL, due_at)],
                    )
                    .await
                } else if pid == mako_wim::wertebestellung::STORNIERUNG_PID.as_u32() {
                    // ORDCHG 39002 Stornierung carries no LOC — correlate by the
                    // original Bestellung's Belegnummer echoed in RFF+ON. Resume
                    // only; a Stornierung without a running order is an orphan.
                    let cmd = adapters::wim_wertebestellung_registry().dispatch(raw, &fv)?;
                    let cmd = self.gate_esa_consent(msg, cmd).await;
                    let order_ref = extract_order_ref_from_msg(msg);
                    self.resume_by_malo::<mako_wim::wertebestellung::WimWertebestellungWorkflow>(
                        &order_ref,
                        mako_wim::wertebestellung::WORKFLOW_NAME,
                        cmd,
                    )
                    .await
                } else if pid == mako_wim::wertebestellung::BESTELLUNG_PID.as_u32()
                    || pid == mako_wim::wertebestellung::ABBESTELLUNG_PID.as_u32()
                {
                    let cmd = adapters::wim_wertebestellung_registry().dispatch(raw, &fv)?;
                    // Consent gate: a revoked consent between Angebot and
                    // Bestellung turns the order into an ORDRSP 19012 Ablehnung.
                    let cmd = self.gate_esa_consent(msg, cmd).await;
                    let malo_id = extract_malo_from_msg(msg);
                    // UC 4.1 Nr. 4 / Nr. 6 and UC 4.3 Nr. 2: 2 WT nach dem ÜT.
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        mako_wim::wertebestellung::ANTWORT_FRIST_WT,
                        HolidayCalendar::BdewMaKo,
                    );
                    // Also index the process under this ORDERS' Belegnummer so a
                    // later ORDCHG Stornierung (which has no LOC) can resume it.
                    let order_belegnr = msg.message_ref();
                    self.spawn_or_resume_keyed::<mako_wim::wertebestellung::WimWertebestellungWorkflow>(
                        malo_id.as_str(),
                        mako_wim::wertebestellung::WORKFLOW_NAME,
                        cmd,
                        &fv,
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
                    let key = if matches!(msg, AnyMessage::Ordrsp(_)) {
                        extract_order_ref_from_msg(msg)
                    } else {
                        String::from(extract_malo_from_msg(msg))
                    };
                    self.resume_by_malo::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow>(
                        &key,
                        mako_wim::esa_wertebestellung::WORKFLOW_NAME,
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: mako_wim::esa_wertebestellung::WORKFLOW_NAME,
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── WiM Preisanfrage REQOTE (PIDs 35001–35005) ────────────────────
            "wim-preisanfrage" => {
                if mako_wim::preisanfrage::REQOTE_PIDS.contains(&pid) {
                    let cmd = adapters::wim_preisanfrage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Preisanfrage deadline: 5 Werktage (BK6-24-174).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimPreisanfrageWorkflow>(
                        malo_id.as_str(),
                        "wim-preisanfrage",
                        cmd,
                        &fv,
                        &[(mako_wim::preisanfrage::PREISANFRAGE_DEADLINE_LABEL, due_at)],
                    )
                    .await
                } else if mako_wim::preisanfrage::QUOTES_PIDS.contains(&pid) {
                    // QUOTES 15001–15005: the MSB's Angebot answering our REQOTE —
                    // resume the process by MaLo (QUOTES carries it in LOC).
                    let cmd = adapters::wim_preisanfrage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<WimPreisanfrageWorkflow>(
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
            "wim-technik-aenderung" => Ok(IngestOutcome::Skipped {
                workflow_name: "wim-technik-aenderung",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
