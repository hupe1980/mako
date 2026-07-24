//! GaBi Gas ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the GaBi Gas workflow family:
    /// `gabi-gas-invoic`
    /// `gabi-gas-mmma`
    /// `gabi-gas-nomination`
    /// `gabi-gas-allocation`
    /// `gabi-gas-schedl`
    /// `gabi-gas-imbnot`
    /// `gabi-gas-tranot`
    /// `gabi-gas-delivery-order`
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
                33001 => {
                    // REMADV — payer confirms payment; invoicer (us) resumes process.
                    // Correlation: RFF+Z13 back-reference to original INVOIC message_ref.
                    let cmd = adapters::gabi_gas_remadv_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_remadv(msg);
                    self.resume_by_malo::<GaBiGasInvoicWorkflow>(
                        &invoice_ref,
                        "gabi-gas-invoic",
                        cmd,
                    )
                    .await
                }
                29001 => {
                    // COMDIS — invoicer rejects payer's REMADV.
                    // Correlation: RFF+Z13 back-reference to original INVOIC message_ref.
                    let cmd = adapters::gabi_gas_comdis_registry().dispatch(raw, &fv)?;
                    let invoice_ref = extract_invoice_ref_from_comdis(msg);
                    self.resume_by_malo::<GaBiGasInvoicWorkflow>(
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
                    self.resume_by_malo::<GpkeAllokationslisteWorkflow>(
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
            // ── GaBi Gas Nomination (PIDs 90011, 90012, 90021, 90022) ─────────
            // Regulatory basis: Kooperationsvereinbarung Gas (KoV), BK7-24-01-008.
            // Nomination response (NOMRES) is required by D-1 15:00 CET (≈ 2 h after
            // the D-1 13:00 nomination deadline). The 10-Werktage value here is a
            // conservative outer bound for the engine deadline store; the actual
            // KoV intraday window is enforced by the FNB/MGV at the application layer.
            "gabi-gas-nomination" => {
                if mako_gabi_gas::nomination::NOMINATION_PIDS.contains(&pid) {
                    let cmd = adapters::gabi_gas_nomination_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Gas nomination deadline: response required within 10 Werktage.
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GaBiGasNominationWorkflow>(
                        malo_id.as_str(),
                        "gabi-gas-nomination",
                        cmd,
                        &fv,
                        &[(mako_gabi_gas::nomination::NOMRES_DEADLINE_LABEL, due_at)],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-nomination",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── GaBi Gas Allocation (PIDs 90001, 90002, 90003) ────────────────
            // Regulatory basis: DVGW ALOCAT (allocation list) — no statutory
            // response deadline in BK7-24-01-008; ALOCAT is a one-way push from MMMA
            // to participants. spawn_deadline = None.
            "gabi-gas-allocation" => {
                if mako_gabi_gas::allocation::ALLOCATION_PIDS.contains(&pid) {
                    let cmd = adapters::gabi_gas_allocation_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Allocation list: no statutory response deadline defined.
                    self.spawn_or_resume::<GaBiGasAllocationWorkflow>(
                        malo_id.as_str(),
                        "gabi-gas-allocation",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "gabi-gas-allocation",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            "gabi-gas-schedl" => Ok(IngestOutcome::Skipped {
                workflow_name: "gabi-gas-schedl",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
            "gabi-gas-imbnot" => Ok(IngestOutcome::Skipped {
                workflow_name: "gabi-gas-imbnot",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
            "gabi-gas-tranot" => Ok(IngestOutcome::Skipped {
                workflow_name: "gabi-gas-tranot",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
            "gabi-gas-delivery-order" => Ok(IngestOutcome::Skipped {
                workflow_name: "gabi-gas-delivery-order",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
