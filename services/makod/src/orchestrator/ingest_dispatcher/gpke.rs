//! GPKE (Strom) ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the GPKE (Strom) workflow family:
    /// `gpke-sperrung`
    /// `gpke-sperrung-lf`
    /// `gpke-supplier-change`
    /// `gpke-lf-abmeldung`
    /// `gpke-lf-anmeldung`
    /// `gpke-allokationsliste`
    /// `gpke-abrechnung`
    /// `gpke-konfiguration`
    /// `gpke-stornierung`
    /// `gpke-ankuendigung-zuordnung-lf`
    /// `gpke-neuanlage`
    /// `gpke-anfrage-bestellung`
    /// `gpke-partin`
    /// `gpke-messwerte`
    /// `gpke-utilts`
    /// `gpke-datenabruf`
    /// `gpke-konfiguration-aenderung`
    pub(super) async fn dispatch_gpke(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── GPKE Sperrung — NB side ───────────────────────────────────────
            // PIDs 17115/17117: Sperrauftrag / Entsperrauftrag (LF → NB) — spawn.
            // PIDs 19118/19119: MSB → NB Antwort — resume.
            "gpke-sperrung" => match pid {
                17115 | 17117 => {
                    let cmd = adapters::gpke_sperrung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Business answer Frist: „spätester ÜT ist der 1. WT nach
                    // dem ÜT" for the ORDRSP 19116/19117 (GPKE Teil 2
                    // §§ 3.5.1.2 / 3.5.2.2 Prozessschritt 2) — a Werktag
                    // deadline, not 24 wall-clock hours. The APERAK is a
                    // separate 45-minute clock on the same message (APERAK AHB
                    // 1.0 §2.4.1; Saturday → Sunday 12:00 Berlin).
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    // Also index the process under this Sperrauftrag's Belegnummer so
                    // a later ORDCHG 39000 Stornierung (which carries no LOC) can
                    // resume it by the RFF+ON order reference.
                    self.spawn_or_resume_keyed::<GpkeSperrungWorkflow>(
                        malo_id.as_str(),
                        "gpke-sperrung",
                        cmd,
                        &fv,
                        &[
                            (mako_gpke::SPERRUNG_WINDOW_LABEL, process_due_at),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                        &[msg.message_ref()],
                        Some(|s| !s.is_terminal()),
                    )
                    .await
                }
                19118 | 19119 => {
                    let cmd = adapters::gpke_sperrung_msb_response_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeSperrungWorkflow>(
                        malo_id.as_str(),
                        "gpke-sperrung",
                        cmd,
                    )
                    .await
                }
                // ORDCHG 39000 (LF → NB Stornierung) and 39001 (NB → MSB
                // Weiterleitung der Stornierung) — both LOC-less; resume by the
                // original order reference echoed in RFF+ON.
                39000 | 39001 => {
                    let cmd = adapters::gpke_sperrung_stornierung_registry().dispatch(raw, &fv)?;
                    let order_ref = extract_order_ref_from_msg(msg);
                    self.resume_by_key::<GpkeSperrungWorkflow>(&order_ref, "gpke-sperrung", cmd)
                        .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-sperrung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Sperrung — LF side ───────────────────────────────────────
            // PIDs 19116/19117: Bestätigung/Ablehnung Sperrauftrag (NB → LF) — resume.
            "gpke-sperrung-lf" => match pid {
                // 19116/19117 answer the Sperrauftrag (ORDERS 17115);
                // 19128/19129 answer the Stornierung (ORDCHG 39000);
                // 21039 is the IFTSTA Auftragsstatus after execution.
                // All resume the LF-side process by MaLo (ORDRSP/IFTSTA carry it in LOC).
                19116 | 19117 | 19128 | 19129 | 21039 => {
                    let cmd = adapters::gpke_sperrung_lf_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeSperrungLfWorkflow>(
                        malo_id.as_str(),
                        "gpke-sperrung-lf",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-sperrung-lf",
                    reason: "pid_not_in_resume_table",
                }),
            },
            // ── GPKE SupplierChange — NB side ────────────────────────────────
            // PIDs 55001, 55002, 55016: Lieferbeginn/Lieferende ANFRAGE (LF → NB) — spawn.
            "gpke-supplier-change" => match pid {
                // Drive off the workflow's own Anfrage set rather than a copy:
                // a hardcoded list drifts from the router registration, and a
                // PID missing here is dropped silently.
                //
                // `UTILMD_ANFRAGE_PIDS`, not `UTILMD_PIDS`: the latter also
                // carries 55557, which has no Antwort mapping. Spawning it
                // yields a process that cannot be answered and the 24-hour
                // deadline turns into a false `Rejected`.
                p if mako_gpke::UTILMD_ANFRAGE_PIDS.contains(&p) => {
                    let cmd = adapters::gpke_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Business answer Frist, per PID, from the GPKE Teil 2
                    // table in `mako_gpke::antwortfrist` — 11:00 Uhr des 1. WT
                    // nach dem ÜT for an Anmeldung (55001/55077), 06:00 for an
                    // Abmeldung (55004), Ablauf des 1. WT for a Kündigung
                    // (55016). The APERAK acknowledgement is a separate,
                    // 45-minute clock on the same message.
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeSupplierChangeWorkflow>(
                        malo_id.as_str(),
                        "gpke-supplier-change",
                        cmd,
                        &fv,
                        &[
                            (mako_gpke::GPKE_PROCESS_RESPONSE_LABEL, process_due_at),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                // IFTSTA Vollzugs-/Statusmeldung (21024–21028, 21033, 21035, 21045,
                // 21047) — the NB reports the supplier change's completion status.
                // Informational; resume the process by MaLo (IFTSTA carries it in
                // the single LOC per the AHB profile) and record it for audit.
                p if mako_gpke::IFTSTA_VOLLZUGS_PIDS.contains(&p) => {
                    let cmd = adapters::gpke_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeSupplierChangeWorkflow>(
                        malo_id.as_str(),
                        "gpke-supplier-change",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-supplier-change",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // ── GPKE LF-Abmeldung — LF side (NB-initiated Lieferende) ───────
            //
            // PID 55007: Ankündigung NB-seitiges Lieferende (NB → LFN) — spawn.
            //
            // The NB proactively terminates a supply relationship (§41 EnWG or
            // judicial order). The LF receives PID 55007 and responds with
            // PID 55008 (Bestätigung) or 55009 (Ablehnung) via ERP command
            // `gpke.nb-lieferende.bestaetigen` / `.ablehnen`.
            //
            // Note: PIDs 55007–55009 are present in UTILMD AHB Strom 2.1
            // (FV2025-10-01). They were NOT removed by BK6-22-024 (LFW24);
            // only the LF-initiated processes (55001/55002) were redesigned
            // for 24h processing. APERAK Frist: 24h (BK6-22-024 §4).
            "gpke-lf-abmeldung" => match pid {
                55007 => {
                    let cmd = adapters::gpke_lf_abmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let received = OffsetDateTime::now_utc();
                    // 05:00 Uhr des 1. WT nach dem ÜT (GPKE Teil 2, SD
                    // Lieferende von NB an LF Prozessschritt 2).
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeLfAbmeldungWorkflow>(
                        malo_id.as_str(),
                        "gpke-lf-abmeldung",
                        cmd,
                        &fv,
                        &[
                            (mako_gpke::LF_ABMELDUNG_ANTWORT_WINDOW_LABEL, process_due_at),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-lf-abmeldung",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // PID 55010 (Anfrage zur Beendigung der Zuordnung, NB → LFA) — GPKE
            // Teil 2. LFA-role makod receives 55010, answers 55011/55012.
            "gpke-beendigung-zuordnung" => match pid {
                // The LFA's answer, coming back to the **NB** that asked. Not a
                // spawn: the process is the one the 55010 opened, and it is
                // keyed by MaLo. „Nach Ablauf der Frist eingehende Antworten
                // sind für den Fortlauf dieses Prozesses unerheblich" — a late
                // one resolves to `process_not_found` or is refused by the
                // workflow, both of which leave the Anmeldung's outcome alone.
                55011 | 55012 => {
                    let cmd = adapters::gpke_beendigung_zuordnung_antwort_registry()
                        .dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeBeendigungZuordnungWorkflow>(
                        malo_id.as_str(),
                        "gpke-beendigung-zuordnung",
                        cmd,
                    )
                    .await
                }
                55010 => {
                    let cmd = adapters::gpke_beendigung_zuordnung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    let received = OffsetDateTime::now_utc();
                    // 09:00 Uhr des 1. WT nach dem ÜT (GPKE Teil 2, SD
                    // Lieferbeginn Prozessschritt 4).
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeBeendigungZuordnungWorkflow>(
                        malo_id.as_str(),
                        "gpke-beendigung-zuordnung",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::BEENDIGUNG_ZUORDNUNG_ANTWORT_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-beendigung-zuordnung",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // PIDs 55036/55037/55038 (Zuordnungs-Meldungen, NB → LFN/LFA/LFZ)
            // — GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 2 / 10 / 13.
            //
            // **No business deadline is registered.** There is no
            // Antwortnachricht, so nothing can be late on the receiving side;
            // the Frist these carry is the *sender's*, and it lives on the
            // NB-side process. Registering one here would fire on every
            // Meldung mako ever receives.
            "gpke-zuordnungsmeldung" => {
                if mako_gpke::ZUORDNUNGSMELDUNG_PIDS.contains(&pid) {
                    let cmd = adapters::gpke_zuordnungsmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // **No deadline at all**, not even the APERAK one. The
                    // Anerkennung is enqueued in the same transactional write
                    // as the event, so a 45-minute timer could only ever fire
                    // on an obligation already discharged — the defect
                    // `deadline_labels.rs` exists to prevent. A delivery
                    // failure surfaces through the outbox worker's own retry
                    // and dead-letter path instead.
                    //
                    // Guarded with an always-`false` occupancy verdict: three
                    // Meldungen are owed on one MaLo around a single
                    // Lieferbeginn, so the second must spawn its own process
                    // rather than resume the first — which would refuse it as
                    // an invalid state transition and drop the message.
                    self.spawn_or_resume_guarded::<mako_gpke::GpkeZuordnungsmeldungWorkflow>(
                        malo_id.as_str(),
                        "gpke-zuordnungsmeldung",
                        cmd,
                        &fv,
                        &[],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-zuordnungsmeldung",
                        reason: "pid_not_in_spawn_table",
                    })
                }
            }
            // PID 55016 (Kündigung, LFN → LFA) — GPKE Teil 2 § 1.2. Its own
            // workflow rather than `gpke-supplier-change`: both are keyed by
            // Marktlokation, and an integrated NB+LF deployment runs the NB's
            // Anmeldung on the same MaLo.
            "gpke-kuendigung" => match pid {
                55016 => {
                    let cmd = adapters::gpke_kuendigung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    let received = OffsetDateTime::now_utc();
                    // Ablauf des 1. WT nach dem ÜT (GPKE Teil 2, SD Kündigung
                    // Prozessschritt 2).
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<mako_gpke::GpkeKuendigungWorkflow>(
                        malo_id.as_str(),
                        "gpke-kuendigung",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::kuendigung::KUENDIGUNG_ANTWORT_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-kuendigung",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // ── GPKE Ersatz-/Grundversorgung (§36/§38 EnWG) ─────────────────
            //
            // PID 55013 (Anmeldung/Zuordnung EOG, NB → LF) — spawn the E/G
            // responder. Deadlines (APERAK 45-min + answer window at 15:00
            // next Werktag) are registered by the workflow itself from
            // `received_at`.
            // PIDs 55014/55015 (Bestätigung/Ablehnung, LF → NB) — resume the
            // NB initiator by MaLo.
            "gpke-eog" => match pid {
                55013 => {
                    let cmd = adapters::gpke_eog_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<mako_gpke::GpkeEogWorkflow>(
                        malo_id.as_str(),
                        "gpke-eog",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                55014 | 55015 => {
                    let cmd = adapters::gpke_eog_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<mako_gpke::GpkeEogWorkflow>(
                        malo_id.as_str(),
                        "gpke-eog",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-eog",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // ── GPKE Teil 4 Stammdatenänderung ──────────────────────────────
            // Änderung PIDs spawn a Berechtigter process (apply + Rückmeldung);
            // Rückmeldung PIDs resume a change we initiated. Deadlines (APERAK
            // 45-min + 2-WT Rückmeldung window) are registered by the workflow.
            "gpke-stammdatenaenderung" => {
                let malo_id = extract_malo_from_msg(msg);
                if mako_gpke::is_aenderung_pid(pid) {
                    let cmd = adapters::gpke_stammdaten_registry().dispatch(raw, &fv)?;
                    self.spawn_or_resume::<mako_gpke::GpkeStammdatenaenderungWorkflow>(
                        malo_id.as_str(),
                        "gpke-stammdatenaenderung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else if mako_gpke::is_rueckmeldung_pid(pid) {
                    let cmd = adapters::gpke_stammdaten_registry().dispatch(raw, &fv)?;
                    self.resume_by_key::<mako_gpke::GpkeStammdatenaenderungWorkflow>(
                        malo_id.as_str(),
                        "gpke-stammdatenaenderung",
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-stammdatenaenderung",
                        reason: "pid_not_in_spawn_table",
                    })
                }
            }
            // ── GPKE LF-Anmeldung — LF side ─────────────────────────────────
            // The NB's (or LFA's) Antwort — resume: 55002/55003 Bestätigung/
            // Ablehnung Anmeldung verb. MaLo, 55078/55080 the same for an erz.
            // MaLo, 55005/55006 Abmeldung, 55017/55018 Kündigung.
            "gpke-lf-anmeldung" => match pid {
                55002 | 55003 | 55004 | 55005 | 55006 | 55017 | 55018 | 55078 | 55080 => {
                    let cmd = adapters::gpke_lf_anmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeLfAnmeldungWorkflow>(
                        malo_id.as_str(),
                        "gpke-lf-anmeldung",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-lf-anmeldung",
                    reason: "pid_not_in_resume_table",
                }),
            },
            // ── GPKE Allokationsliste — LF side ──────────────────────────────
            // PIDs 19110/19115: NB rejects the LF's ORDERS request — resume.
            // PID 13014: NB sends MSCONS data for Strom bilanzierte Menge — resume.
            //   Note: PID 13013 (Gas Allokationsliste, Gas-only) is registered by
            //   GaBiGasModule → "gabi-gas-mmma" and handled in that arm below.
            //
            // Note: PIDs 17110/17114 (LF → NB ORDERS request) are spawned by the
            // ERP (via CommandAPI), not by inbound EDIFACT at the LF. They are
            // registered in the PID router for completeness but have no inbound
            // dispatch handler here (LF is the sender, not the receiver).
            "gpke-allokationsliste" => match pid {
                19110 | 19115 => {
                    let cmd =
                        adapters::gpke_allokationsliste_ordrsp_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeAllokationslisteWorkflow>(
                        malo_id.as_str(),
                        "gpke-allokationsliste",
                        cmd,
                    )
                    .await
                }
                13014 => {
                    let cmd =
                        adapters::gpke_allokationsliste_mscons_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeAllokationslisteWorkflow>(
                        malo_id.as_str(),
                        "gpke-allokationsliste",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-allokationsliste",
                    reason: "pid_not_in_resume_table",
                }),
            },
            // ── GPKE INVOIC billing — PIDs 31001/31002/31005/31006 ───────────
            //
            // The NB (invoicer) sends an INVOIC to the LF (payer).  `makod`
            // acting as the LF receives the INVOIC and spawns a new billing
            // process.  The settlement window is the invoice's own Zahlungsziel
            // (`SG8 DTM+265`) — BK6-24-174 GPKE Teil 2 § 3.3.2 and GeLi Gas 3.0
            // Kap. 4.5 both answer „zum Zahlungsziel", never in 24 wall-clock
            // hours.  After spawning, the `ProcessInitiated` outbox
            // message notifies `invoicd`, which runs automated plausibility
            // checks and submits a SettleInvoice or DisputeInvoice command.
            //
            // REMADV 33001–33004 (payer-side payment advice to invoicer) and
            // COMDIS 29001 (invoicer rejects payer's REMADV) resume an
            // existing process keyed on the original invoice message-ref.
            //
            // Regulatory basis: INVOIC AHB 2.8e / 1.0; REMADV AHB 1.0;
            // COMDIS AHB 1.0; BK6-22-024 §5.
            "gpke-abrechnung" => match pid {
                31001 | 31002 | 31005 | 31006 => {
                    let cmd = adapters::gpke_abrechnung_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_malo_from_invoic(msg);
                    // The settlement window of an INVOIC is its **Zahlungsziel**
                    // (`SG8 DTM+265`), which the sender states on the document —
                    // the receiver must answer „zum Zahlungsziel". Ten Werktage
                    // is the fallback the MMM/GeLi Gas process tables name when
                    // the invoice omits the date; 24 hours was neither.
                    let received = OffsetDateTime::now_utc();
                    let due_at = faelligkeitsdatum_from_invoic(msg).unwrap_or_else(|| {
                        fristen::deadline_at_werktage(
                            received,
                            fristen::vorlauf::ZAHLUNGSZIEL_MINDEST_WT,
                            HolidayCalendar::BdewMaKo,
                        )
                    });
                    self.spawn_or_resume::<GpkeAbrechnungWorkflow>(
                        &invoice_ref,
                        "gpke-abrechnung",
                        cmd,
                        &fv,
                        &[(mako_gpke::ABRECHNUNG_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                33001..=33004 => {
                    // REMADV from payer — resume the invoicer-side billing process.
                    let cmd = adapters::gpke_abrechnung_remadv_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_remadv(msg);
                    self.resume_by_key::<GpkeAbrechnungWorkflow>(
                        &invoice_ref,
                        "gpke-abrechnung",
                        cmd,
                    )
                    .await
                }
                29001 => {
                    // COMDIS from invoicer — resume the payer-side billing process.
                    let cmd = adapters::gpke_abrechnung_comdis_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_comdis(msg);
                    self.resume_by_key::<GpkeAbrechnungWorkflow>(
                        &invoice_ref,
                        "gpke-abrechnung",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-abrechnung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Konfiguration — PIDs 17134/17135 (NB role) ──────────────
            // PIDs 17134/17135: NB sends ORDERS Konfiguration to MSB — spawn.
            // PIDs 19001/19002: MSB → NB ORDRSP Bestätigung/Ablehnung — resume.
            //
            // Role guard: registered only when DeploymentRoles contains Nb.
            // nMSB instances use PIDs 19001/19002 for wim-geraeteubernahme instead.
            "gpke-konfiguration" => match pid {
                17134 | 17135 => {
                    let cmd = adapters::gpke_konfiguration_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // `mako_fristen::antwort` has no published window for the
                    // GPKE Teil 3 Konfigurationsbestellung yet, so this falls
                    // back to the operating convention — and says so, rather
                    // than citing a Festlegung that does not state it.
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeKonfigurationWorkflow>(
                        malo_id.as_str(),
                        "gpke-konfiguration",
                        cmd,
                        &fv,
                        &[
                            (mako_gpke::KONFIGURATION_WINDOW_LABEL, process_due_at),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                19001 | 19002 => {
                    let cmd = adapters::gpke_konfiguration_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeKonfigurationWorkflow>(
                        malo_id.as_str(),
                        "gpke-konfiguration",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-konfiguration",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Stornierung — PIDs 55022/55023/55024 ────────────────────
            // PIDs 55022–55024: UTILMD Stornierung Lieferbeginn/Lieferende — spawn.
            "gpke-stornierung" => match pid {
                55022..=55024 => {
                    let cmd = adapters::gpke_stornierung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No published window for the UTILMD Stornierung — the
                    // fallback is an operating convention, not a citation.
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeStornierungWorkflow>(
                        malo_id.as_str(),
                        "gpke-stornierung",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::STORNIERUNG_GPKE_ANTWORT_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-stornierung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Ankündigung/Zuordnung LF — PID 55607 ────────────────────
            // PID 55607: UTILMD Ankündigung Zuordnung LF (NB → LFN) — spawn.
            "gpke-ankuendigung-zuordnung-lf" => match pid {
                55607 => {
                    let cmd =
                        adapters::gpke_ankuendigung_zuordnung_lf_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Business answer Frist: „spätester ÜZ ist 15:00 Uhr am ÜT"
                    // (GPKE Teil 2 § 2.4.2.2 Nr. 2). Missing it is not a lapsed
                    // obligation — Prozessschritt 3 has the NB assign the LFN
                    // without an answer.
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeAnkuendigungZuordnungLfWorkflow>(
                        malo_id.as_str(),
                        "gpke-ankuendigung-zuordnung-lf",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::ANKUENDIGUNG_ZUORDNUNG_ANTWORT_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-ankuendigung-zuordnung-lf",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Neuanlage (PIDs 55600, 55601) ────────────────────────────
            "gpke-neuanlage" => match pid {
                55600 | 55601 => {
                    let cmd = adapters::gpke_neuanlage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Business answer Frist: „spätester ÜZ ist 00:00 Uhr des
                    // 61. WT nach dem ÜT" (GPKE Teil 2 § 2.2.2 Prozessschritte
                    // 2/3) — `E_0608` Prüfschritte 110/590 give the NB 60
                    // Werktage of *daily* re-identification before it may refuse
                    // a newly commissioned Marktlokation.
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeNeuanlageWorkflow>(
                        malo_id.as_str(),
                        "gpke-neuanlage",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::neuanlage::NEUANLAGE_ANTWORT_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-neuanlage",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Abrechnungsdaten (PIDs 55156/55220/55673) ────────────────
            "gpke-abrechnungsdaten" => match pid {
                55156 | 55220 | 55673 => {
                    let cmd = adapters::gpke_abrechnungsdaten_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // „2. Werktag nach dem ÜT" (GPKE Teil 2 §§ 3.1.1.2 /
                    // 3.1.2.2 / 3.1.3.2) — a business Frist, longer than the
                    // 45-minute APERAK clock that runs beside it.
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<mako_gpke::GpkeAbrechnungsdatenWorkflow>(
                        malo_id.as_str(),
                        "gpke-abrechnungsdaten",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::abrechnungsdaten::BEARBEITUNGSSTAND_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-abrechnungsdaten",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE Anfrage / Bestellung (PID 55555) ─────────────────────────
            "gpke-anfrage-bestellung" => match pid {
                55555 => {
                    let cmd = adapters::gpke_anfrage_bestellung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    let received = OffsetDateTime::now_utc();
                    let process_due_at = antwort_due_at(pid, received);
                    let aperak_due_at = fristen::aperak_strom_due_at(received);
                    self.spawn_or_resume::<GpkeAnfrageBestellungWorkflow>(
                        malo_id.as_str(),
                        "gpke-anfrage-bestellung",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::anfrage_bestellung::ANFRAGE_WINDOW_LABEL,
                                process_due_at,
                            ),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-anfrage-bestellung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GPKE PARTIN Kommunikationsdaten (PIDs 37000–37006) ────────────
            "gpke-partin" => {
                if mako_gpke::partin::PARTIN_STROM_PIDS.contains(&pid) {
                    let cmd = adapters::gpke_partin_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No APERAK Frist for pure data delivery messages.
                    self.spawn_or_resume::<GpkePartinWorkflow>(
                        malo_id.as_str(),
                        "gpke-partin",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-partin",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── GPKE Messwerte MSCONS (PIDs 13005, 13006, …) ─────────────────
            "gpke-messwerte" => {
                if mako_gpke::messwerte::MSCONS_PIDS.contains(&pid) {
                    let cmd = adapters::gpke_messwerte_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // A 13027 „Werte nach Typ 2" is also the ESA's own signal
                    // that its subscription has started delivering. UC 4.1 Nr. 5
                    // admits a Stornierung only while it has not, so without
                    // this the ESA-side window never closes and the ESA can send
                    // a 39002 the MSB has to refuse (`E_0257` `A02`).
                    //
                    // Best-effort and separate from storage: the values are
                    // stored either way, and a delivery for which this
                    // deployment holds no ESA process is simply not ours to
                    // mark. `SG1 RFF+AGI` names the ORDERS the ESA indexed its
                    // process under (MSCONS AHB 3.2 §11.2 hint [574]).
                    if pid == 13027 {
                        let key = esa_korrelation_key(msg, pid);
                        if !key.is_empty() {
                            // A *miss* is legitimate and `resume_by_key` already
                            // reports it as `Skipped` — this deployment may hold
                            // no ESA process for the delivery. An `Err` is a
                            // different thing: the process was found and the
                            // command could not be applied, so an ESA order
                            // stays open with its Lieferung unmarked. Both
                            // looked like nothing happened.
                            if let Err(e) = self
                                .resume_by_key::<mako_wim::esa_wertebestellung::EsaWertebestellungWorkflow>(
                                    &key,
                                    mako_wim::esa_wertebestellung::WORKFLOW_NAME,
                                    mako_wim::esa_wertebestellung::EsaWertebestellungCommand::MarkLieferungBegonnen,
                                )
                                .await
                            {
                                tracing::warn!(
                                    %key,
                                    error = %e,
                                    "ingest dispatcher: ESA Wertebestellung not marked as begun \
                                     — the values are stored, but the ESA process still shows \
                                     the Lieferung outstanding",
                                );
                            }
                        }
                    }
                    // No deadline for pure data delivery.
                    self.spawn_or_resume::<GpkeMesswerteLieferungWorkflow>(
                        malo_id.as_str(),
                        "gpke-messwerte",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-messwerte",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── GPKE UTILTS Konfigurationsdaten ───────────────────────────────
            "gpke-utilts" => {
                if mako_gpke::utilts::UTILTS_PIDS.contains(&pid) {
                    let cmd = adapters::gpke_utilts_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<GpkeUtiltsWorkflow>(
                        malo_id.as_str(),
                        "gpke-utilts",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-utilts",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── GPKE Datenabruf ORDRSP/Ablehnung ──────────────────────────────
            // The outbound ORDERS is sent by LF; the only inbound message is a
            // rejection ORDRSP from NB/MSB (PIDs 19101, 19102, 19114).
            "gpke-datenabruf" => {
                if mako_gpke::datenabruf::ORDRSP_ABLEHNUNG_PIDS.contains(&pid) {
                    let cmd = adapters::gpke_datenabruf_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Resume existing process; no spawn — LF initiates.
                    self.resume_by_key::<GpkeDatenabrufWorkflow>(
                        malo_id.as_str(),
                        "gpke-datenabruf",
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-datenabruf",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── GPKE Konfigurationsänderung ORDRSP ────────────────────────────
            // LF sends ORDERS (PIDs 19120–19133); NB/MSB responds with ORDRSP.
            "gpke-konfiguration-aenderung" => {
                // ORDRSP responses (17102/17113) and IFTSTA Bestellungsantwort/
                // -beendigung (21043/21044) both resume the process by MaLo.
                if mako_gpke::konfiguration_aenderung::ORDRSP_PIDS.contains(&pid)
                    || mako_gpke::konfiguration_aenderung::IFTSTA_PIDS.contains(&pid)
                {
                    let cmd =
                        adapters::gpke_konfiguration_aenderung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GpkeKonfigurationAenderungWorkflow>(
                        malo_id.as_str(),
                        "gpke-konfiguration-aenderung",
                        cmd,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gpke-konfiguration-aenderung",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
