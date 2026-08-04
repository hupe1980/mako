//! GeLi Gas ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the GeLi Gas workflow family:
    /// `geli-gas-sperrung-nb`
    /// `geli-gas-sperrung-lf`
    /// `geli-gas-supplier-change`
    /// `geli-gas-mscons`
    /// `geli-gas-stornierung`
    /// `geli-gas-sperrprozesse-invoic`
    /// `geli-gas-partin`
    /// `geli-gas-stornierung-lf`
    /// `geli-gas-lf-anmeldung`
    /// `geli-gas-datenabruf`
    pub(super) async fn dispatch_geli_gas(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── GeLi Gas Sperrung — NB side ───────────────────────────────────
            // PIDs 17115/17117: Gas-Sperrauftrag (LFG → GNB) — spawn.
            "geli-gas-sperrung-nb" => match pid {
                17115 | 17117 => {
                    let cmd = adapters::geli_gas_sperrung_nb_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009 §5).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    // Index under the Sperrauftrag's Belegnummer so a later ORDCHG
                    // 39000 Stornierung (LOC-less) resumes it by RFF+ON order ref.
                    self.spawn_or_resume_keyed::<GeliGasSperrungNbWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-sperrung-nb",
                        cmd,
                        &fv,
                        &[(
                            mako_geli_gas::GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL,
                            due_at,
                        )],
                        &[msg.message_ref()],
                    )
                    .await
                }
                // ORDRSP 19118/19119: the gMSB's Bestätigung/Ablehnung of the
                // Anfrage Sperrung the GNB forwarded — resume by MaLo.
                19118 | 19119 => {
                    let cmd =
                        adapters::geli_gas_sperrung_nb_response_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GeliGasSperrungNbWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-sperrung-nb",
                        cmd,
                    )
                    .await
                }
                // ORDCHG 39000 (LFG → GNB Stornierung) and 39001 (GNB → gMSB
                // Weiterleitung) — both LOC-less; resume by the RFF+ON order ref.
                39000 | 39001 => {
                    let cmd =
                        adapters::geli_gas_sperrung_nb_stornierung_registry().dispatch(raw, &fv)?;
                    let order_ref = extract_order_ref_from_msg(msg);
                    self.resume_by_malo::<GeliGasSperrungNbWorkflow>(
                        &order_ref,
                        "geli-gas-sperrung-nb",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-sperrung-nb",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GeLi Gas Sperrung — LF side ───────────────────────────────────
            // PIDs 19116/19117: Gas-Bestätigung/Ablehnung (GNB → LFG) — resume.
            "geli-gas-sperrung-lf" => match pid {
                // 19116/19117 answer the Gas-Sperrauftrag (ORDERS 17115);
                // 19128/19129 answer the Stornierung (ORDCHG 39000).
                19116 | 19117 | 19128 | 19129 => {
                    let cmd = adapters::geli_gas_sperrung_lf_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GeliGasSperrungLfWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-sperrung-lf",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-sperrung-lf",
                    reason: "pid_not_in_resume_table",
                }),
            },
            // ── GeLi Gas SupplierChange — NB side ────────────────────────────
            // PIDs 44001–44021: UTILMD G ANFRAGE (LFG → GNB) — spawn.
            "geli-gas-supplier-change" => match pid {
                44001..=44021 => {
                    let cmd = adapters::geli_gas_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GeliGasSupplierChangeWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-supplier-change",
                        cmd,
                        &fv,
                        &[(mako_geli_gas::LIEFERBEGINN_RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-supplier-change",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // ── GeLi Gas Stammdatenänderung (44109–44182) ─────────────────────
            // Änderung PIDs spawn a Berechtigter process; Antwort PIDs resume a
            // change we initiated. The workflow registers the APERAK + 10-WT
            // Antwort deadlines. Stammdatenanfrage PIDs (G8–G10) spawn the data
            // owner's `ReceiveAnfrage` process, which auto-answers with the
            // requested master data (data-return).
            "geli-gas-stammdatenaenderung" => {
                let malo_id = extract_malo_from_msg(msg);
                if mako_geli_gas::stammdatenaenderung::is_antwort_pid(pid)
                    && !mako_geli_gas::stammdatenaenderung::is_aenderung_pid(pid)
                {
                    let cmd = adapters::geli_gas_stammdaten_registry().dispatch(raw, &fv)?;
                    self.resume_by_malo::<mako_geli_gas::GeliGasStammdatenaenderungWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-stammdatenaenderung",
                        cmd,
                    )
                    .await
                } else {
                    let cmd = adapters::geli_gas_stammdaten_registry().dispatch(raw, &fv)?;
                    self.spawn_or_resume::<mako_geli_gas::GeliGasStammdatenaenderungWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-stammdatenaenderung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
            }
            // ── GeLi Gas MSCONS data delivery ─────────────────────────────────
            // PIDs 13002, 13007–13009: MSCONS Gas Messdaten (NB/MSB → LFG) — spawn.
            "geli-gas-mscons" => match pid {
                13002 | 13007 | 13008 | 13009 => {
                    let cmd = adapters::geli_gas_mscons_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Gas MSCONS data delivery — no APERAK Frist for pure data messages.
                    self.spawn_or_resume::<GeliGasMsconsWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-mscons",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-mscons",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GeLi Gas Stornierung — PIDs 44022/44023/44024 ────────────────
            // PID 44022: GNB receives Stornierungsanfrage (LFG → GNB) — spawn (Nb role).
            // PIDs 44023/44024: LFG receives GNB response — spawn (Lf role).
            //
            // Multi-domain: PIDs 44022–44024 are also used by wim-gas-stornierung
            // on nMSB/gMSB instances.  Role-conditional routing ensures only one
            // workflow is registered per role (PidRouter enforces at build time).
            "geli-gas-stornierung" => match pid {
                44022..=44024 => {
                    let cmd = adapters::geli_gas_stornierung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GeliGasStornierungWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-stornierung",
                        cmd,
                        &fv,
                        &[(mako_geli_gas::STORNIERUNG_RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-stornierung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GeLi Gas Sperrprozesse INVOIC — PID 31011 ────────────────────
            // PID 31011: Rechnung sonstige Leistung AWH (GNB → LFG) — spawn.
            "geli-gas-sperrprozesse-invoic" => match pid {
                31011 => {
                    let cmd =
                        adapters::geli_gas_sperrprozesse_invoic_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_invoic(msg);
                    // Settlement deadline: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GeliGasSperrprozesseInvoicWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-sperrprozesse-invoic",
                        cmd,
                        &fv,
                        &[(mako_geli_gas::SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-sperrprozesse-invoic",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GeLi Gas PARTIN Kommunikationsdaten (PIDs 37008–37014) ────────
            "geli-gas-partin" => {
                if mako_geli_gas::partin::PARTIN_GAS_PIDS.contains(&pid) {
                    let cmd = adapters::geli_gas_partin_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<GeliGasPartinWorkflow>(
                        malo_id.as_str(),
                        "geli-gas-partin",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "geli-gas-partin",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── GeLi Gas Stornierung — LF side (PIDs 44023–44024) ────────────
            // PIDs 44023/44024: GNB response (Bestätigung / Ablehnung) to LF — resume.
            // The LF's process was spawned by the ERP-side InitiateStornierung command.
            "geli-gas-stornierung-lf" => match pid {
                44023 | 44024 => {
                    let cmd = adapters::geli_gas_stornierung_lf_registry().dispatch(raw, &fv)?;
                    let vorgang_id = extract_melo_from_utilmd(msg);
                    self.resume_by_malo::<GeliGasLfStornierungWorkflow>(
                        &vorgang_id,
                        "geli-gas-stornierung-lf",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-stornierung-lf",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GeLi Gas LFN-side Anmeldung (PIDs 44003–44006 inbound) ───────
            // PIDs 44003/44005: GNB Bestätigung Lieferbeginn/Lieferende → resume.
            // PIDs 44004/44006: GNB Ablehnung Lieferbeginn/Lieferende → resume.
            // Spawned via ERP command geli.lieferbeginn.anmelden (PIDs 44001/44002 outbound).
            "geli-gas-lf-anmeldung" => match pid {
                // The GNB's answers, from the workflow's own constant rather
                // than a range: a hand-written range drifts from the registered
                // set, and a PID missing here is dropped silently.
                p if mako_geli_gas::lf_anmeldung::ANTWORT_PIDS_LF.contains(&p) => {
                    let cmd = adapters::geli_gas_lf_anmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GeliGasLfAnmeldungWorkflow>(
                        malo_id.as_str(),
                        mako_geli_gas::LF_ANMELDUNG_WORKFLOW_NAME,
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-lf-anmeldung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── Workflows registered in PidRouter but Phase 2 dispatch not yet implemented ─
            //
            // These workflows handle inbound PIDs that are registered in their
            // respective domain modules. Full dispatch arms with typed adapters and
            // workflow commands will be added in a follow-up. Until then, inbound
            // messages are explicitly acknowledged as "not yet dispatched" rather
            // than silently falling through to the catch-all warn arm.
            //
            // To implement one of these: add an AdapterRegistry<WorkflowType> function
            // to adapters.rs and add a proper spawn_or_resume arm above.
            // ── GeLi Gas Datenabruf (PIDs 17103/17104 inbound, 19103/19104 ORDRSP) ─
            // 17103/17104: NB/MSB receives ORDERS Anfrage from LF — spawn.
            // 19103/19104: LF receives ORDRSP rejection from NB — resume.
            // ERP-initiated outbound (LF sends 17103) uses geli.gas.datenabruf.anfragen command.
            "geli-gas-datenabruf" => match pid {
                17103 | 17104 => {
                    let cmd =
                        adapters::geli_gas_datenabruf_receive_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GeliGasDatanabrufWorkflow>(
                        malo_id.as_str(),
                        mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[(mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                19103 | 19104 => {
                    let cmd =
                        adapters::geli_gas_datenabruf_ablehnung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GeliGasDatanabrufWorkflow>(
                        malo_id.as_str(),
                        mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-datenabruf",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
