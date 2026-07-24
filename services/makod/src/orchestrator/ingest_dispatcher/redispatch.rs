//! Redispatch 2.0 (EDIFACT leg) ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the Redispatch 2.0 (EDIFACT leg) workflow family:
    /// `redispatch-aktivierung`
    pub(super) async fn dispatch_redispatch(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);

        match workflow_name {
            // ── Redispatch 2.0 Aktivierung (EDIFACT leg) ─────────────────────
            // IFTSTA 21037/21038 Vollzugsmeldungen and the MSCONS/ORDERS/ORDRSP
            // Ausfallarbeit family are recorded on the activation process —
            // spawned when none exists yet, so no Redispatch market message is
            // silently dropped. The XML ActivationDocument itself travels the
            // AS4 XML channel (outside this EDIFACT dispatcher); its state
            // machine is driven via the engine API and the deadline table.
            "redispatch-aktivierung" => match pid {
                21037 | 21038 => {
                    let (sender, receiver, message_ref) = redispatch_envelope(msg);
                    let key = redispatch_process_key(msg, &message_ref, &sender, pid);
                    let cmd = mako_redispatch::aktivierung::AktivierungCommand::ReceiveIftsta {
                        pid,
                        sender,
                        receiver,
                        message_ref,
                    };
                    self.spawn_or_resume::<mako_redispatch::aktivierung::AktivierungWorkflow>(
                        &key,
                        "redispatch-aktivierung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                13020 | 13021 | 13022 | 13023 | 13026 | 17209 | 17210 | 17211 | 19204 | 19301
                | 19302 => {
                    let (sender, receiver, message_ref) = redispatch_envelope(msg);
                    let key = redispatch_process_key(msg, &message_ref, &sender, pid);
                    let cmd =
                        mako_redispatch::aktivierung::AktivierungCommand::ReceiveMarketMessage {
                            pid,
                            sender,
                            receiver,
                            message_ref,
                        };
                    self.spawn_or_resume::<mako_redispatch::aktivierung::AktivierungWorkflow>(
                        &key,
                        "redispatch-aktivierung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "redispatch-aktivierung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
