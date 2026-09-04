//! GaBi Gas ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Dispatch arms for the GaBi Gas workflows that carry **BDEW** messages:
    /// `gabi-gas-invoic` (INVOIC/REMADV/COMDIS) and `gabi-gas-mmma` (MSCONS).
    ///
    /// The DVGW transport workflows take a different message family and live in
    /// [`dispatch_gabi_gas_dvgw`](Self::dispatch_gabi_gas_dvgw).
    pub(super) async fn dispatch_gabi_gas(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── GaBi Gas INVOIC billing (PIDs 31010, 31007, 31008) ───────────
            // PIDs 31010/31007/31008: inbound INVOIC (payer receives) — spawn.
            // PID 33001 (REMADV): payment confirmation from payer — resume.
            // PID 29001 (COMDIS): payment rejection by invoicer — resume.
            //
            // Regulatory basis: BK7-24-01-008 (GaBi Gas 2.1).
            // Settlement window: no statutory deadline in BK7-24-01-008;
            // 10 Werktage applied by analogy with Gas process norms.
            "gabi-gas-invoic" => match pid {
                31010 | 31007 | 31008 => {
                    let cmd = adapters::gabi_gas_invoic_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_malo_from_invoic(msg);
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GaBiGasInvoicWorkflow>(
                        &invoice_ref,
                        "gabi-gas-invoic",
                        cmd,
                        &fv,
                        &[(mako_gabi_gas::INVOIC_SETTLEMENT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                29001 => {
                    // COMDIS — invoicer rejects payer's REMADV.
                    // Correlation: RFF+Z13 back-reference to original INVOIC message_ref.
                    let cmd = adapters::gabi_gas_comdis_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_comdis(msg);
                    self.resume_by_key::<GaBiGasInvoicWorkflow>(
                        &invoice_ref,
                        "gabi-gas-invoic",
                        cmd,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gabi-gas-invoic",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── GaBi Gas MMMA — Gas Allokationsliste MSCONS (PID 13013) ──────
            // PID 13013: NB delivers Gas MMM Allokationsliste to LF — resume the
            // GpkeAllokationslisteWorkflow process spawned when LF sent ORDERS 17110.
            //
            // The PidRouter routes 13013 to "gabi-gas-mmma" (registered by
            // GaBiGasModule).  Since gabi-gas-mmma has no independent workflow
            // implementation yet, we delegate the MSCONS delivery to the existing
            // GpkeAllokationslisteWorkflow using the same resume path as PID 13014.
            "gabi-gas-mmma" => match pid {
                13013 => {
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
                    workflow_name: "gabi-gas-mmma",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }

    /// Dispatch arms for the GaBi Gas workflows that carry **DVGW** messages.
    ///
    /// A separate entry point because the message family is different: an
    /// ALOCAT is not an [`AnyMessage`] and never can be. The two families share
    /// one `PidRouter` — DVGW allocates 70000–79999 and BDEW does not — so the
    /// caller resolves the workflow the same way for both and only the parse and
    /// this dispatch differ.
    pub(super) async fn dispatch_gabi_gas_dvgw(
        &self,
        msg: &dvgw_edi::DvgwMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        // DVGW publishes no BDEW Formatversion, so there is nothing on the wire
        // to derive one from. `UNH` DE 0057 is a DVGW package code (`DVGW17`) or
        // the message version (`5.11a`) depending on the format, and neither
        // maps onto the BDEW `FVYYYY-MM-DD` scheme the workflow registry is
        // keyed by. The newest registered version is used, which is the same
        // fallback `detect_format_version` applies when a BDEW message does not
        // state one.
        let fv = adapters::known_fvs()
            .into_iter()
            .max()
            .unwrap_or_else(|| FormatVersion::parse("FV2025-10-01").expect("valid FV literal"));
        let raw: &dyn Any = msg;

        // A message with no published Zuordnung — or a `ZO-T*` tuple with no gas
        // day to scope it to — has no defined way to reach a process (ALOCAT
        // 5.11a §3.3). Attaching it to a guessed key would merge it into a
        // stream it does not belong to, so it is skipped loudly.
        let Some(key) = msg.process_key() else {
            tracing::warn!(
                pid,
                workflow = workflow_name,
                document = msg.document.code(),
                "ingest: DVGW message has no published Zuordnung — cannot correlate",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: "gabi-gas-dvgw",
                reason: "no_correlation_key",
            });
        };

        match workflow_name {
            // ── Nomination — NOMINT 70030–70034 / NOMRES 70035–70039 ─────────
            //
            // The NOMRES response window closes at 15:00 CET on gas day D-1,
            // roughly two hours after the nomination deadline. It is a
            // wall-clock instant tied to the nominated gas day, not an elapsed
            // duration from arrival, so it comes from the command's own
            // `GasDay` (DST-correct through `time-tz`). A relative Werktage
            // window cannot express it: any multi-day bound outruns the
            // two-hour obligation by so far that a missed NOMRES would never be
            // detected.
            //
            // The key is the business key both messages carry, because a NOMRES
            // carries no reference to the nomination it answers.
            "gabi-gas-nomination" => {
                if !mako_gabi_gas::nomination::NOMINATION_PIDS.contains(&pid) {
                    return Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-nomination",
                        reason: "pid_not_in_dispatch_table",
                    });
                }

                // A Matching-Benachrichtigung (`07G`/`19G`) reports the state of
                // the match and accepts nothing; only a Bestätigung decides the
                // nomination. Recorded here and left to the workflow untouched,
                // so the nomination stays open for it. The matching obligations
                // themselves are a process question.
                if matches!(
                    msg.document,
                    dvgw_edi::DvgwDocument::MatchingBenachrichtigung
                        | dvgw_edi::DvgwDocument::VhpMatchingBenachrichtigung
                ) {
                    tracing::info!(
                        pid,
                        document = msg.document.code(),
                        key = %key,
                        "ingest: NOMRES Matching-Benachrichtigung recorded — it states \
                         no acceptance, so the nomination stays open for its Bestätigung",
                    );
                    return Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-nomination",
                        reason: "matching_notification_states_no_acceptance",
                    });
                }

                let cmd = adapters::gabi_gas_nomination_registry().dispatch(raw, &fv)?;
                let due_at = match &cmd {
                    NominationCommand::SendNomination { gas_day, .. }
                    | NominationCommand::ReceiveNomint { gas_day, .. }
                    | NominationCommand::ReceiveNomres { gas_day, .. } => {
                        Some(gas_day.nomres_deadline_utc())
                    }
                    // Neither is built from an inbound message: the answer this
                    // tenant sends and the fired deadline carry no gas day of
                    // their own to register a window from.
                    NominationCommand::SendNomres { .. }
                    | NominationCommand::NomresDeadlineExpired { .. } => None,
                };
                let deadlines: Vec<(&'static str, OffsetDateTime)> = due_at
                    .map(|d| (mako_gabi_gas::nomination::NOMRES_DEADLINE_LABEL, d))
                    .into_iter()
                    .collect();

                // Only the NOMINT initiates. Spawning on an answer hands a fresh
                // process a `ReceiveNomres` it rejects from `New`, so an orphan
                // answer is skipped — as every other family treats one.
                if mako_gabi_gas::nomination::NOMRES_PIDS.contains(&pid) {
                    return self
                        .resume_by_key::<GaBiGasNominationWorkflow>(
                            &key,
                            "gabi-gas-nomination",
                            cmd,
                        )
                        .await;
                }
                self.spawn_or_resume_guarded::<GaBiGasNominationWorkflow>(
                    &key,
                    "gabi-gas-nomination",
                    cmd,
                    &fv,
                    &deadlines,
                    |s| !s.is_terminal(),
                )
                .await
            }
            // ── Allocation — ALOCAT 70001–70023 ──────────────────────────────
            //
            // One-way from the NB/MGV, so there is nothing to answer — but KoV
            // §6.4 binds the *sender*: corrections may follow, and a binding
            // final allocation is due by the end of month M+2 at 12:00 CET.
            // That window is registered on spawn from the message's own gas
            // day, so a gas day that never receives its final allocation
            // becomes a recorded fact rather than a silently unsettled
            // imbalance.
            "gabi-gas-allocation" => {
                if !mako_gabi_gas::allocation::ALLOCATION_PIDS.contains(&pid) {
                    return Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-allocation",
                        reason: "pid_not_in_dispatch_table",
                    });
                }
                let cmd = adapters::gabi_gas_allocation_registry().dispatch(raw, &fv)?;
                let AllocationCommand::ReceiveAlocat { gas_day, .. } = &cmd else {
                    return Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-allocation",
                        reason: "pid_not_in_dispatch_table",
                    });
                };
                // § 47 Ziffer 1 KoV XV gives the final allocation two different
                // deadlines, and only one of them is watchable from an inbound
                // ALOCAT: an **SLP** allocation is final on D-1 12:00, which is
                // already past by the time the first ALOCAT for gas day D
                // arrives, so nothing could ever be registered for it. The
                // window that is watchable is the **RLM** one — M+14 Werktage
                // after the delivery month — and that is what is registered.
                let final_due_at = gas_day.finale_allokation_deadline_utc(
                    mako_gabi_gas::AllokationsSerie::Rlm,
                    |from, n| {
                        mako_fristen::add_werktage(
                            from,
                            u32::from(n),
                            mako_fristen::HolidayCalendar::BdewMaKo,
                        )
                    },
                );
                self.spawn_or_resume::<GaBiGasAllocationWorkflow>(
                    &key,
                    "gabi-gas-allocation",
                    cmd.clone(),
                    &fv,
                    &[(mako_gabi_gas::FINAL_ALOCAT_DEADLINE_LABEL, final_due_at)],
                )
                .await
            }
            // ── Mehr-/Mindermengen — SSQNOT 70095/70096 ──────────────────────
            //
            // One-way from the NB to the MGV; the key is the published 2-Tupel
            // (Netzkonto, Netzbetreiber) plus the Abrechnungszeitraum, so a
            // later report for the same period resumes the process that holds
            // the earlier one. No Frist binds the receiver.
            "gabi-gas-mehr-mindermengen" => {
                if !mako_gabi_gas::MEHR_MINDERMENGEN_PIDS.contains(&pid) {
                    return Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-mehr-mindermengen",
                        reason: "pid_not_in_dispatch_table",
                    });
                }
                let cmd = adapters::gabi_gas_mehr_mindermengen_registry().dispatch(raw, &fv)?;
                self.spawn_or_resume::<GaBiGasMehrMindermengenWorkflow>(
                    &key,
                    "gabi-gas-mehr-mindermengen",
                    cmd,
                    &fv,
                    &[],
                )
                .await
            }
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
