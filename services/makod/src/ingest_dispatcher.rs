//! Phase 2 ingest execution: EDIFACT message → typed domain command → Process.
//!
//! [`EdifactIngestDispatcher`] bridges parsed [`AnyMessage`] values from the
//! EDIFACT ingest layer to running domain workflow processes.  Used by:
//!
//! - `edifact_api` — REST `POST /edifact` path
//! - `as4_ingest`  — AS4 inbound delivery path
//! - `as4_sender`  — in-process loopback for self-addressed messages
//!   (combined-role deployments: NB+LF, NB+MSB, GNB+gMSB sharing one GLN)
//!
//! ## Routing strategy
//!
//! The caller supplies the pre-computed `workflow_name` (from
//! [`mako_engine::pid_router::PidRouter`]) and raw PID so the dispatcher can
//! choose the correct adapter and spawn-vs-resume strategy without re-detecting
//! them.
//!
//! - **Spawn** (new process): the tenant receives an initiating message, e.g.
//!   NB receives ORDERS 17115 Sperrauftrag from LF.  Uses `lookup_correlated`
//!   by MaLo; if nothing found, spawns a fresh process and registers it.
//! - **Resume** (continue existing): e.g. NB receives ORDRSP 19118 from MSB.
//!   Uses `lookup_correlated` by MaLo; returns [`IngestOutcome::Skipped`] if no
//!   process is found (not an error — peer may have sent an orphan message).
//!
//! ## Combined-role loopback
//!
//! When `BdewAs4Sender` detects `recipient == own_gln`, it
//! renders the domain payload to EDIFACT wire bytes, re-parses them, and calls
//! `dispatch` here instead of transmitting over AS4.  This enables zero-latency
//! in-process delivery for Stadtwerke deployments (NB+LF, GNB+gMSB).

use std::any::Any;
use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _, ReleaseRegistry};
use mako_engine::{
    error::EngineError,
    ids::{ProcessId, ProcessIdentity, TenantId},
    process::Process,
    registry::ProcessRegistry as _,
    store_slatedb::{SlateDbSnapshotStore, SlateDbStore},
    version::{FormatVersion, WorkflowId},
    workflow::{CommandPayload, Workflow},
};
use mako_geli_gas::{
    GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow, GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    GpkeLfAnmeldungWorkflow, GpkeSperrungLfWorkflow, GpkeSperrungWorkflow,
    GpkeSupplierChangeWorkflow,
};
use time::OffsetDateTime;

use crate::adapters;

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Outcome of a successful ingest dispatch attempt.
#[derive(Debug)]
#[allow(dead_code)] // fields are read via Debug formatting in tracing events
pub enum IngestOutcome {
    /// A new process was spawned and the initiating command executed.
    Spawned {
        /// Workflow family name (e.g. `"gpke-sperrung"`).
        workflow_name: &'static str,
        /// Newly created process identifier.
        process_id: ProcessId,
    },
    /// An existing process received the continuation command.
    Dispatched {
        /// Workflow family name.
        workflow_name: &'static str,
        /// Identifier of the resumed process.
        process_id: ProcessId,
    },
    /// Dispatch was deliberately skipped — this PID/workflow is not handled
    /// at this role or the process simply does not exist yet (orphan response).
    Skipped {
        /// Workflow family name (best-effort; may be `"unregistered"`).
        workflow_name: &'static str,
        /// Machine-readable skip reason for observability.
        reason: &'static str,
    },
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

/// Phase 2 ingest execution dispatcher.
///
/// Translates parsed [`AnyMessage`] objects to typed domain commands and
/// executes them on the correct workflow process.  Share across threads via
/// [`Arc`] — all fields are `Clone + Send + Sync`.
#[derive(Clone)]
pub struct EdifactIngestDispatcher {
    store: Arc<SlateDbStore>,
    snap_store: SlateDbSnapshotStore,
    snapshot_interval: u64,
    tenant_id: TenantId,
}

impl EdifactIngestDispatcher {
    /// Construct a new dispatcher backed by the given stores.
    #[must_use]
    pub fn new(
        store: Arc<SlateDbStore>,
        snap_store: SlateDbSnapshotStore,
        snapshot_interval: u64,
        tenant_id: TenantId,
    ) -> Self {
        Self {
            store,
            snap_store,
            snapshot_interval,
            tenant_id,
        }
    }

    /// Dispatch `msg` to the appropriate workflow process.
    ///
    /// `workflow_name` must be pre-computed by the caller via `PidRouter::route`.
    /// `pid` is the raw Prüfidentifikator value already extracted from the UNH.
    ///
    /// Returns [`IngestOutcome::Skipped`] (not `Err`) when this PID/workflow
    /// combination is not in the current dispatch table, or when no process is
    /// found for a response message.  Returns `Err` only on storage or adapter
    /// failures.
    pub async fn dispatch(
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
                    self.spawn_or_resume::<GpkeSperrungWorkflow>(
                        &malo_id,
                        "gpke-sperrung",
                        cmd,
                        &fv,
                    )
                    .await
                }
                19118 | 19119 => {
                    let cmd = adapters::gpke_sperrung_msb_response_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GpkeSperrungWorkflow>(&malo_id, "gpke-sperrung", cmd)
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
                19116 | 19117 => {
                    let cmd = adapters::gpke_sperrung_lf_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GpkeSperrungLfWorkflow>(&malo_id, "gpke-sperrung-lf", cmd)
                        .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-sperrung-lf",
                    reason: "pid_not_in_resume_table",
                }),
            },

            // ── GeLi Gas Sperrung — NB side ───────────────────────────────────
            // PIDs 17115/17117: Gas-Sperrauftrag (LFG → GNB) — spawn.
            "geli-gas-sperrung-nb" => match pid {
                17115 | 17117 => {
                    let cmd = adapters::geli_gas_sperrung_nb_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<GeliGasSperrungNbWorkflow>(
                        &malo_id,
                        "geli-gas-sperrung-nb",
                        cmd,
                        &fv,
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
                19116 | 19117 => {
                    let cmd = adapters::geli_gas_sperrung_lf_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GeliGasSperrungLfWorkflow>(
                        &malo_id,
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

            // ── GPKE SupplierChange — NB side ────────────────────────────────
            // PIDs 55001, 55002, 55016: Lieferbeginn/Lieferende ANFRAGE (LF → NB) — spawn.
            "gpke-supplier-change" => match pid {
                55001 | 55002 | 55016 => {
                    let cmd = adapters::gpke_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<GpkeSupplierChangeWorkflow>(
                        &malo_id,
                        "gpke-supplier-change",
                        cmd,
                        &fv,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "gpke-supplier-change",
                    reason: "pid_not_in_spawn_table",
                }),
            },

            // ── GPKE LF-Anmeldung — LF side ─────────────────────────────────
            // PIDs 55003–55006, 55017, 55018: ANTWORT from NB — resume.
            "gpke-lf-anmeldung" => match pid {
                55003 | 55004 | 55005 | 55006 | 55017 | 55018 => {
                    let cmd = adapters::gpke_lf_anmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GpkeLfAnmeldungWorkflow>(
                        &malo_id,
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

            // ── GeLi Gas SupplierChange — NB side ────────────────────────────
            // PIDs 44001–44021: UTILMD G ANFRAGE (LFG → GNB) — spawn.
            "geli-gas-supplier-change" => match pid {
                44001..=44021 => {
                    let cmd = adapters::geli_gas_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<GeliGasSupplierChangeWorkflow>(
                        &malo_id,
                        "geli-gas-supplier-change",
                        cmd,
                        &fv,
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "geli-gas-supplier-change",
                    reason: "pid_not_in_spawn_table",
                }),
            },

            // ── All other workflows: not yet in Phase 2 dispatch table ────────
            wf_name => {
                tracing::debug!(
                    workflow_name = %wf_name,
                    pid,
                    "ingest dispatcher: no Phase 2 handler — skipped",
                );
                Ok(IngestOutcome::Skipped {
                    workflow_name: "unregistered",
                    reason: "workflow_not_in_dispatch_table",
                })
            }
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Look up an existing process by MaLo business key and workflow name.
    ///
    /// If a matching process exists, execute `cmd` on it and return
    /// [`IngestOutcome::Dispatched`].  Otherwise spawn a new process, execute
    /// `cmd`, register the process under the MaLo tag, and return
    /// [`IngestOutcome::Spawned`].
    async fn spawn_or_resume<W>(
        &self,
        malo_id: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        fv: &FormatVersion,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        if malo_id.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no MaLo ID in message — cannot register; skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_malo_id",
            });
        }

        let registry = self.store.as_process_registry();
        let identities = registry.lookup_correlated(self.tenant_id, malo_id).await?;

        // Filter for this workflow family specifically — there can be multiple
        // concurrent processes per MaLo (e.g. active Lieferbeginn + Sperrung).
        let matching: Vec<&ProcessIdentity> = identities
            .iter()
            .filter(|id| id.workflow_id.name.as_ref() == workflow_name_static)
            .collect();

        if let Some(first) = matching.first() {
            // Existing process — idempotent continuation.
            let process = Process::<W, Arc<SlateDbStore>>::from_identity(
                Arc::clone(&self.store),
                (*first).clone(),
            );
            let process_id = process.process_id();
            process
                .execute_and_enqueue_with_snapshot_and_retry(
                    cmd,
                    3,
                    &self.snap_store,
                    self.snapshot_interval,
                )
                .await?;
            return Ok(IngestOutcome::Dispatched {
                workflow_name: workflow_name_static,
                process_id,
            });
        }

        // No matching process — spawn a fresh one.
        let workflow_id = WorkflowId::new(workflow_name_static, fv.as_str());
        let process = Process::<W, Arc<SlateDbStore>>::new(
            Arc::clone(&self.store),
            self.tenant_id,
            workflow_id,
        );
        let process_id = process.process_id();
        process
            .execute_and_enqueue_with_snapshot_and_retry(
                cmd,
                3,
                &self.snap_store,
                self.snapshot_interval,
            )
            .await?;

        // Register under MaLo business key for future correlation lookups.
        let identity = process.identity();
        if let Err(e) = registry
            .register_correlated(self.tenant_id, malo_id, process_id, identity)
            .await
        {
            tracing::warn!(
                process_id = %process_id,
                malo_id    = %malo_id,
                error      = %e,
                "ingest dispatcher: MaLo registry failed (non-fatal — process was spawned)",
            );
        }

        Ok(IngestOutcome::Spawned {
            workflow_name: workflow_name_static,
            process_id,
        })
    }

    /// Look up an existing process by MaLo and execute the continuation command.
    ///
    /// Returns [`IngestOutcome::Skipped`] (not `Err`) when no process is found —
    /// this is expected when the initiating command was handled by the peer role
    /// and no local LF-side process was ever spawned.
    async fn resume_by_malo<W>(
        &self,
        malo_id: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
    {
        if malo_id.is_empty() {
            tracing::warn!(
                workflow_name = %workflow_name_static,
                "ingest dispatcher: no MaLo ID in response message — skipping",
            );
            return Ok(IngestOutcome::Skipped {
                workflow_name: workflow_name_static,
                reason: "no_malo_id",
            });
        }

        let identities = self
            .store
            .as_process_registry()
            .lookup_correlated(self.tenant_id, malo_id)
            .await?;

        let matching: Vec<&ProcessIdentity> = identities
            .iter()
            .filter(|id| id.workflow_id.name.as_ref() == workflow_name_static)
            .collect();

        let identity = match matching.first() {
            Some(id) => (*id).clone(),
            None => {
                tracing::warn!(
                    workflow_name = %workflow_name_static,
                    malo_id       = %malo_id,
                    "ingest dispatcher: no active process for MaLo — response dropped; \
                     ensure the initiating command was executed first",
                );
                return Ok(IngestOutcome::Skipped {
                    workflow_name: workflow_name_static,
                    reason: "process_not_found",
                });
            }
        };

        let process =
            Process::<W, Arc<SlateDbStore>>::from_identity(Arc::clone(&self.store), identity);
        let process_id = process.process_id();
        process
            .execute_and_enqueue_with_snapshot_and_retry(
                cmd,
                3,
                &self.snap_store,
                self.snapshot_interval,
            )
            .await?;

        Ok(IngestOutcome::Dispatched {
            workflow_name: workflow_name_static,
            process_id,
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the Marktlokations-ID from the first LOC segment (component 1, index 0).
///
/// BDEW convention: `LOC+<qualifier>+<malo_id>::<code_list>:Z13`.
/// Applies to ORDERS, ORDRSP, and UTILMD messages.
/// Returns an empty string when the LOC segment is absent (INVOIC, IFTSTA, …).
fn extract_malo_from_msg(msg: &AnyMessage) -> String {
    let segs = match msg {
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        AnyMessage::Utilmd(u) => u.segments(),
        _ => return String::new(),
    };
    segs.iter()
        .find(|s| s.tag == "LOC")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned()
}

/// Detect the BDEW format version to use when spawning a new `WorkflowId`.
///
/// Reads the association-assigned release code from UNH element S009 component
/// DE 0057 (e.g. `"S2.1"`, `"G1.1"`, `"2.8e"`), then resolves the profile active
/// for that release code today via [`ReleaseRegistry::global`].  The profile's
/// `valid_from` date is used to build the `FormatVersion` key
/// (e.g. `"FV2025-10-01"`).
///
/// Falls back to `FV2025-10-01` when:
/// - The message is an `AnyMessage::Unknown` variant (no message type).
/// - The UNH association code is absent or empty.
/// - No profile is registered for the `(message_type, release)` pair.
/// - The profile exists but has no `valid_from` date (legacy undated profile).
///
/// The fallback is safe because:
/// 1. The FV only affects the `WorkflowId` name on the spawned process.
/// 2. Adapters use `is_known_fv`, which accepts all registered FVs.
/// 3. A running process keeps its original `WorkflowId` and is never re-versioned.
fn detect_format_version(msg: &AnyMessage) -> FormatVersion {
    const FALLBACK: &str = "FV2025-10-01";
    let fallback = || FormatVersion::new(FALLBACK);

    let Some(message_type) = msg.try_message_type() else {
        return fallback();
    };
    let Ok(release) = msg.detect_release() else {
        return fallback();
    };

    let today = OffsetDateTime::now_utc().date();
    let Ok(profile) = ReleaseRegistry::global().profile_on(message_type, release, today) else {
        return fallback();
    };
    let Some(valid_from) = profile.valid_from() else {
        return fallback();
    };

    let fv_str = format!(
        "FV{:04}-{:02}-{:02}",
        valid_from.year(),
        valid_from.month() as u8,
        valid_from.day(),
    );
    FormatVersion::parse(&fv_str).unwrap_or_else(|_| fallback())
}
