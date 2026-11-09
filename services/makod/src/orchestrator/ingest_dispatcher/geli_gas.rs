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
        inbound: Inbound<'_>,
    ) -> Result<IngestOutcome, EngineError> {
        let received_at = inbound.received_at;
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── GeLi Gas Sperrung — NB side ───────────────────────────────────
            // PIDs 17115/17117: Gas-Sperrauftrag (LFG → GNB) — spawn.
            "geli-gas-sperrung-nb" => match pid {
                17115 | 17117 => {
                    let cmd = adapters::geli_gas_sperrung_nb_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // The GNB's answer: „spätester ÜT ist der 1. WT nach dem ÜT
                    // von Nr. 1". GeLi Gas 3.0 holds no Sperrprozess, so the
                    // Sparte-neutral 17115 / 17117 rows of
                    // `mako_fristen::antwort` (GPKE Teil 2 Kap. 3.5.1.2 /
                    // 3.5.2.2 Nr. 2) are the window — the one the LF side
                    // registers for the same Auftrag.
                    let due_at = antwort_due_at(pid, received_at);
                    // Index under the Sperrauftrag's Belegnummer so a later ORDCHG
                    // 39000 Stornierung (LOC-less) resumes it by RFF+ON order ref.
                    self.spawn_or_resume_keyed::<GeliGasSperrungNbWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-sperrung-nb",
                        cmd,
                        &fv,
                        &[(
                            mako_geli_gas::GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL,
                            due_at,
                        )],
                        &[msg.message_ref()],
                        Some(|s| !s.is_terminal()),
                    )
                    .await
                }
                // ORDRSP 19118/19119: the gMSB's Bestätigung/Ablehnung of the
                // Anfrage Sperrung the GNB forwarded — resume by MaLo.
                19118 | 19119 => {
                    let cmd =
                        adapters::geli_gas_sperrung_nb_response_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GeliGasSperrungNbWorkflow>(
                        inbound,
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
                    self.resume_by_key::<GeliGasSperrungNbWorkflow>(
                        inbound,
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
                    self.resume_by_key::<GeliGasSperrungLfWorkflow>(
                        inbound,
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
            // ── GeLi Gas SupplierChange ──────────────────────────────────────
            // 44008/44009, 44011/44012, 44014/44015: the LF's answer to an
            // Anfrage this GNB sent (44007, 44010, 44013) — resume the GNB's
            // own process under `gnb_anfrage_key`, the key `geli.eog.anmelden`
            // spawned it under. Never the bare MaLo: on a combined NB+LF
            // deployment the LF's process answering the same Anfrage runs
            // there. Never a spawn: an answer with no Anfrage behind it has
            // nothing to answer. Routed only on an NB deployment
            // (`GeliGasModule::register_pids_with_roles`).
            "geli-gas-supplier-change" if mako_geli_gas::GNB_ANTWORT_PIDS.contains(&pid) => {
                let cmd = adapters::geli_gas_gnb_antwort_registry().dispatch(raw, &fv)?;
                let malo_id = extract_malo_from_msg(msg);
                let key = if malo_id.as_str().is_empty() {
                    String::new()
                } else {
                    mako_geli_gas::gnb_anfrage_key(malo_id.as_str())
                };
                self.resume_by_key::<GeliGasSupplierChangeWorkflow>(
                    inbound,
                    &key,
                    "geli-gas-supplier-change",
                    cmd,
                )
                .await
            }
            // Every other PID 44001–44021 — spawn by MaLo, or resume the
            // process still occupying it. A process the GNB opened itself is
            // not on this key (`gnb_anfrage_key`), so an Anfrage never resumes
            // one.
            "geli-gas-supplier-change" => match pid {
                44001..=44021 => {
                    let cmd = adapters::geli_gas_registry(received_at).dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Business answer Frist, per PID, from the GeLi Gas 3.0
                    // table in `mako_geli_gas::antwortfrist` — Ablauf des
                    // 4. WT for an Anmeldung (44001), des 3. WT for an
                    // Abmeldung (44004), des 2. WT for the EoG Zuordnung
                    // (44013). The often-quoted 10 Werktage is the *supplier's*
                    // Vorlauffrist for a Lieferantenwechsel, not the GNB's
                    // answer window; using it left a Gas Anmeldung looking
                    // healthy six Werktage past its Frist.
                    //
                    // A PID the Festlegung does not quantify — the Bestandsliste
                    // 44019 and its Änderungsmeldung 44020 have their Frist set
                    // per Netzbetreiber under Kap. 2.6 — falls through to the
                    // operating convention in `antwort_due_at`, which logs that
                    // it is not a regulatory Frist. It must not fall back to
                    // 10 Werktage: that would give an unquantified process a
                    // longer window than any quantified one in the family.
                    let received = received_at;
                    let due_at = antwort_due_at(pid, received);
                    self.spawn_or_resume_guarded::<GeliGasSupplierChangeWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-supplier-change",
                        cmd,
                        &fv,
                        &[(mako_geli_gas::LIEFERBEGINN_RESPONSE_WINDOW_LABEL, due_at)],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-supplier-change",
                    reason: "pid_not_in_spawn_table",
                }),
            },
            // ── GeLi Gas Informationsmeldungen (44036–44038) ─────────────────
            // NB → LFN/LFA/LFZ, AWH GeLi Gas V1.2 Kap. 2.5.2 Nr. 2 / 6 / 7.
            //
            // **No deadline at all.** There is no Antwortnachricht to be late
            // with, and a Gas APERAK reports a Verarbeitbarkeitsfehler only,
            // which the workflow enqueues itself — the APERAK window
            // `dispatch` derives opens beside it. The Frist these carry
            // belongs to the *sending* GNB.
            "geli-gas-zuordnungsmeldung" => {
                if mako_geli_gas::GAS_ZUORDNUNGSMELDUNG_PIDS.contains(&pid) {
                    let cmd = adapters::geli_gas_zuordnungsmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // See the Strom twin: an always-`false` occupancy verdict,
                    // because three Meldungen ride one MaLo per Lieferbeginn.
                    self.spawn_or_resume_guarded::<mako_geli_gas::GeliGasZuordnungsmeldungWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-zuordnungsmeldung",
                        cmd,
                        &fv,
                        &[],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "geli-gas-zuordnungsmeldung",
                        reason: "pid_not_in_spawn_table",
                    })
                }
            }
            // ── GeLi Gas Stammdatenänderung (44109–44182) ─────────────────────
            // Änderung PIDs spawn a Berechtigter process; Antwort PIDs resume a
            // change we initiated. The workflow registers the 10-WT Antwort
            // deadline. An Anfrage zur Stammdatenänderung (G8–G10) spawns
            // the Verantwortlicher's `ReceiveAnfrage` process, which waits for
            // `geli.stammdaten.anfrage.beantworten` inside the same window.
            "geli-gas-stammdatenaenderung" => {
                let malo_id = extract_malo_from_msg(msg);
                if mako_geli_gas::stammdatenaenderung::is_antwort_pid(pid)
                    && !mako_geli_gas::stammdatenaenderung::is_aenderung_pid(pid)
                {
                    let cmd =
                        adapters::geli_gas_stammdaten_registry(received_at).dispatch(raw, &fv)?;
                    self.resume_by_key::<mako_geli_gas::GeliGasStammdatenaenderungWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-stammdatenaenderung",
                        cmd,
                    )
                    .await
                } else {
                    let cmd =
                        adapters::geli_gas_stammdaten_registry(received_at).dispatch(raw, &fv)?;
                    self.spawn_or_resume_guarded::<mako_geli_gas::GeliGasStammdatenaenderungWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-stammdatenaenderung",
                        cmd,
                        &fv,
                        &[],
                        |s| !s.is_terminal(),
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
                    // Gas MSCONS data delivery — no business answer is owed.
                    self.spawn_or_resume_guarded::<GeliGasMsconsWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-mscons",
                        cmd,
                        &fv,
                        &[],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
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
                    // No Festlegung quantifies the answer to a Stornierung
                    // (GeLi Gas 3.0 Kap. 2.7, AWH GeLi Gas V1.2 Kap. 1.8), so
                    // `mako_fristen::antwort` has no row and the window is the
                    // operating convention `antwort_due_at` logs as such.
                    let due_at = antwort_due_at(pid, received_at);
                    self.spawn_or_resume_guarded::<GeliGasStornierungWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-stornierung",
                        cmd,
                        &fv,
                        &[(mako_geli_gas::STORNIERUNG_RESPONSE_WINDOW_LABEL, due_at)],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
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
                    let malo_id = extract_invoice_ref_from_invoic(msg);
                    // The Zahlungsziel the invoice states (`SG8 DTM+265`, Muss
                    // on 31011 — INVOIC AHB 1.0b Kap. 3.1.5).
                    let due_at = zahlungsziel_due(msg, pid)?;
                    self.spawn_or_resume_guarded::<GeliGasSperrprozesseInvoicWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-sperrprozesse-invoic",
                        cmd,
                        &fv,
                        &[(mako_geli_gas::SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL, due_at)],
                        occupied_by_own_reference,
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
                    // Kommunikationsdaten describe the sending Marktpartner.
                    let malo_id = extract_sender_key(msg);
                    self.spawn_or_resume_guarded::<GeliGasPartinWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        "geli-gas-partin",
                        cmd,
                        &fv,
                        &[],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
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
                    self.resume_by_key::<GeliGasLfStornierungWorkflow>(
                        inbound,
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
                    self.resume_by_key::<GeliGasLfAnmeldungWorkflow>(
                        inbound,
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
            // ── Registered in the PidRouter, no typed dispatch arm ─────────
            //
            // The PIDs below route to a workflow their domain module registers,
            // but no adapter turns the message into a command. They are answered
            // "not yet dispatched" so the arm is explicit rather than falling
            // through to the catch-all warn.
            //
            // Adding one: an `AdapterRegistry<WorkflowType>` in adapters.rs plus
            // a `spawn_or_resume` arm above.
            // ── GeLi Gas Datenabruf (PIDs 17103/17104 inbound, 19103/19104 ORDRSP) ─
            // 17103: the GNB receives the LF's Anfrage Brennwert / Zustandszahl;
            // 17104: the NB Strom receives the MSB Gas's Anfrage — spawn.
            // 19103/19104: LF receives ORDRSP rejection from NB — resume.
            // ERP-initiated outbound (LF sends 17103) uses geli.datenabruf.anfragen command.
            "geli-gas-datenabruf" => match pid {
                // 17104 is keyed by the Anfrage itself: its `LOC+172` is
                // conditional ([64], ORDERS AHB 1.1b Kap. 4.1.4, S. 20) and a
                // conformant one may name the Messlokation by address alone.
                // The Messlokation it does name is indexed beside the key.
                17104 => {
                    let cmd =
                        adapters::geli_gas_datenabruf_receive_registry().dispatch(raw, &fv)?;
                    let key = msb_gas_anfrage_key(msg);
                    let melo_id = extract_malo_from_msg(msg);
                    let due_at = datenabruf_antwort_due(msg, pid, received_at)?;
                    let extra: Vec<&str> = [melo_id.as_str()]
                        .into_iter()
                        .filter(|l| !l.is_empty())
                        .collect();
                    self.spawn_or_resume_keyed::<GeliGasDatenabrufWorkflow>(
                        inbound,
                        &key,
                        mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[(mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL, due_at)],
                        &extra,
                        Some(mako_engine::workflow::OccupiesBusinessKey::occupies_business_key),
                    )
                    .await
                }
                17103 => {
                    let cmd =
                        adapters::geli_gas_datenabruf_receive_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    let due_at = datenabruf_antwort_due(msg, pid, received_at)?;
                    self.spawn_or_resume_guarded::<GeliGasDatenabrufWorkflow>(
                        inbound,
                        malo_id.as_str(),
                        mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &[(mako_geli_gas::datenabruf::ANTWORT_WINDOW_LABEL, due_at)],
                        mako_engine::workflow::OccupiesBusinessKey::occupies_business_key,
                    )
                    .await
                }
                19103 | 19104 => {
                    let cmd =
                        adapters::geli_gas_datenabruf_ablehnung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<GeliGasDatenabrufWorkflow>(
                        inbound,
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

/// The NB's answer window on an inbound Datenabruf ORDERS, from the instant it
/// arrived.
///
/// - **17103** (Anfrage Brennwert / Zustandszahl, LF → NB): „spätestens 10
///   Werktage nach Eingang der Anfrage des Lieferanten, aber nicht vor dem 10.
///   Werktag des Monats der auf den Monat folgt, in den das Ende der … Zeitspanne
///   fällt" (BK7-24-01-009 GeLi Gas 3.0 Kap. 4.2, S. 13) —
///   [`fristen::antwort::brennwert_zustandszahl_due`]. The Zeitspanne's end is
///   the `SG29 DTM+164` the ORDERS AHB 1.1b Kap. 4.1.3 makes Muss; an Anfrage
///   without one cannot be given its window and is refused, never given the
///   ten Werktage alone, which would report every Anfrage about the running or
///   the previous month late.
/// - **17104** (Anfrage des MSB Gas an den NB Strom): a Geschäftsdatenanfrage
///   of GPKE Teil 4 (PID overview 4.0, lfd. Nr. 25560), „spätester ÜZ ist 1 WT
///   nach dem ÜZ von Nr. 1" (Kap. 3.2 Nr. 2, S. 30).
fn datenabruf_antwort_due(
    msg: &AnyMessage,
    pid: u32,
    received_at: OffsetDateTime,
) -> Result<OffsetDateTime, EngineError> {
    if pid == 17104 {
        return Ok(fristen::antwort::FristShape::EndOfWerktag(
            fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE,
        )
        .due_at(received_at, HolidayCalendar::BdewMaKo));
    }
    let zeitspanne_bis = brennwert_zeitspanne_bis(msg).ok_or_else(|| {
        EngineError::Deserialization(
            "ORDERS 17103 carries no readable SG29 DTM+164 (Ende Zeitraum für Wertanfrage, \
             ORDERS AHB 1.1b Kap. 4.1.3) — the GNB's answer window (GeLi Gas 3.0 Kap. 4.2) \
             cannot be computed without it"
                .into(),
        )
    })?;
    Ok(fristen::antwort::brennwert_zustandszahl_due(
        received_at,
        zeitspanne_bis,
        HolidayCalendar::BdewMaKo,
    ))
}

/// The key an Anfrage des MSB Gas an den NB Strom (ORDERS 17104) runs under:
/// the sending MSB's MP-ID (`NAD+MS`) and the ORDERS' Dokumentennummer (`BGM`
/// DE 1004), both Muss (ORDERS AHB 1.1b Kap. 4.1.4, S. 19). The number is
/// unique only per sender, hence the MP-ID; the ORDRSP answering it echoes the
/// number.
///
/// Empty when either is missing, which the spawn reports as
/// `no_correlation_key`.
fn msb_gas_anfrage_key(msg: &AnyMessage) -> String {
    let AnyMessage::Orders(o) = msg else {
        return String::new();
    };
    let sender = o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("");
    let nummer = o
        .segments()
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("");
    if sender.is_empty() || nummer.is_empty() {
        return String::new();
    }
    format!("geli-gas-datenabruf:{sender}:{nummer}")
}

/// The last Gastag of a 17103's Zeitspanne, from its `DTM+164`.
///
/// The Ende is the instant the Zeitspanne ends — the beginning of the Gastag
/// after its last one, 06:00 gesetzliche deutsche Zeit (Allgemeine Festlegungen
/// V6.1d Kap. 3.1). The last Gastag is the one holding the instant just before
/// it. Only its month enters the window, so an Ende stamped on the Strom day
/// boundary (00:00) names the same Gastag and is read, not refused: the
/// message is inbound and must stay representable.
fn brennwert_zeitspanne_bis(msg: &AnyMessage) -> Option<time::Date> {
    let AnyMessage::Orders(o) = msg else {
        return None;
    };
    let dtm = o.dtm().iter().find(|d| d.qualifier == "164")?;
    let ende = crate::adapters::parse_dtm_datetime(dtm.value.as_deref()?, dtm.format.as_deref())?;
    let davor = fristen::in_berlin(ende - time::Duration::NANOSECOND);
    if davor.time() < fristen::Sparte::Gas.tagesbeginn() {
        davor.date().previous_day()
    } else {
        Some(davor.date())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    /// The MSB Gas asks the NB Strom (17104): a Geschäftsdatenanfrage, due one
    /// Werktag after it arrived (GPKE Teil 4 Kap. 3.2 Nr. 2) — here across the
    /// end of summer time, Friday 23.10.2026 to Monday 26.10.2026 MEZ.
    #[test]
    fn an_anfrage_vom_msb_gas_is_due_the_next_werktag() {
        let msg = edi_energy::parse(
            b"UNA:+.? 'UNB+UNOC:3+9800555000006:502+9900357000004:500+261023:1000+1++TL'\
              UNH+1+ORDERS:D:09B:UN:1.4c'BGM+Z14+1'DTM+137:202610231000?+00:303'\
              RFF+Z13:17104'NAD+MS+9800555000006::332'NAD+MR+9900357000004::293'\
              UNS+S'UNT+8+1'UNZ+1+1'",
        )
        .expect("parses");
        let due = datenabruf_antwort_due(&msg, 17104, datetime!(2026-10-23 10:00 UTC))
            .expect("17104 needs no Zeitspanne");
        assert_eq!(due, datetime!(2026-10-26 23:59:59.999_999_999 +01:00));
    }
}
