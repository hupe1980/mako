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
//! When `BdewAs4Sender` detects `recipient == own_mp_id`, it
//! renders the domain payload to EDIFACT wire bytes, re-parses them, and calls
//! `dispatch` here instead of transmitting over AS4.  This enables zero-latency
//! in-process delivery for Stadtwerke deployments (NB+LF, GNB+gMSB).

use std::any::Any;
use std::sync::Arc;

use edi_energy::{AnyMessage, EdiEnergyMessage as _, ReleaseRegistry};
use mako_engine::{
    deadline::Deadline,
    error::EngineError,
    fristen::{self, HolidayCalendar},
    ids::{ProcessId, ProcessIdentity, TenantId},
    process::Process,
    registry::ProcessRegistry as _,
    store_slatedb::{SlateDbSnapshotStore, SlateDbStore},
    version::{FormatVersion, WorkflowId},
    workflow::{CommandPayload, Workflow},
};
use mako_gabi_gas::{GaBiGasAllocationWorkflow, GaBiGasInvoicWorkflow, GaBiGasNominationWorkflow};
use mako_geli_gas::{
    GeliGasLfStornierungWorkflow, GeliGasMsconsWorkflow, GeliGasPartinWorkflow,
    GeliGasSperrprozesseInvoicWorkflow, GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow,
    GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    GpkeAbrechnungWorkflow, GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeDatanabrufWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeMesswerteLieferungWorkflow, GpkeNeuanlageWorkflow,
    GpkePartinWorkflow, GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, GpkeStornierungWorkflow,
    GpkeSupplierChangeWorkflow, GpkeUtiltsWorkflow,
};
use mako_mabis::{MabisBillingWorkflow, MabisClearinglisteWorkflow};
use mako_wim::{
    WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimInsrptWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimRechnungWorkflow, WimStammdatenWorkflow,
    WimStornierungWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungWorkflow, WimGasInsrptWorkflow, WimGasInvoicWorkflow, WimGasKuendigungWorkflow,
    WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageWorkflow,
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
    /// All workflow names that have a dispatch arm in [`Self::dispatch`].
    ///
    /// Used by `startup::validate_dispatch_completeness` to verify at startup
    /// that every workflow name registered in the `PidRouter` has a matching
    /// arm here. When a new workflow is added to a domain crate's
    /// `register_pids`, add its name here AND add the corresponding `match`
    /// arm in `dispatch` below.
    pub const KNOWN_WORKFLOW_NAMES: &'static [&'static str] = &[
        "gabi-gas-allocation",
        "gabi-gas-delivery-order",
        "gabi-gas-imbnot",
        "gabi-gas-invoic",
        "gabi-gas-mmma",
        "gabi-gas-nomination",
        "gabi-gas-schedl",
        "gabi-gas-tranot",
        "geli-gas-datenabruf",
        "geli-gas-mscons",
        "geli-gas-partin",
        "geli-gas-sperrprozesse-invoic",
        "geli-gas-sperrung-lf",
        "geli-gas-sperrung-nb",
        "geli-gas-stornierung",
        "geli-gas-stornierung-lf",
        "geli-gas-supplier-change",
        "gpke-abrechnung",
        "gpke-allokationsliste",
        "gpke-anfrage-bestellung",
        "gpke-ankuendigung-zuordnung-lf",
        "gpke-datenabruf",
        "gpke-konfiguration",
        "gpke-konfiguration-aenderung",
        "gpke-lf-abmeldung",
        "gpke-lf-anmeldung",
        "gpke-messwerte",
        "gpke-neuanlage",
        "gpke-partin",
        "gpke-sperrung",
        "gpke-sperrung-lf",
        "gpke-stornierung",
        "gpke-supplier-change",
        "gpke-utilts",
        "mabis-billing",
        "mabis-clearingliste",
        "redispatch-aktivierung",
        "wim-device-change",
        "wim-gas-anmeldung",
        "wim-gas-insrpt",
        "wim-gas-invoic",
        "wim-gas-kuendigung",
        "wim-gas-stornierung",
        "wim-gas-verpflichtungsanfrage",
        "wim-geraeteubernahme",
        "wim-insrpt",
        "wim-preisanfrage",
        "wim-preisliste",
        "wim-rechnung",
        "wim-stammdaten",
        "wim-stornierung",
        "wim-technik-aenderung",
    ];

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
        let outcome = self.dispatch_inner(msg, workflow_name, pid).await;
        let result = match &outcome {
            Ok(IngestOutcome::Spawned { .. } | IngestOutcome::Dispatched { .. }) => "dispatched",
            Ok(IngestOutcome::Skipped { .. }) => "skipped",
            Err(_) => "error",
        };
        mako_engine::metrics::EngineMetrics::global().inbound_received(pid, result);
        outcome
    }

    async fn dispatch_inner(
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
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom ORDERS — 45 min on weekdays,
                    // Sunday 12:00 Berlin if received on Saturday.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeSperrungWorkflow>(
                        &malo_id,
                        "gpke-sperrung",
                        cmd,
                        &fv,
                        &[
                            (mako_gpke::SPERRUNG_WINDOW_LABEL, process_due_at),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
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
                    // APERAK Frist: 10 Werktage (BK7-24-01-009 §5).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GeliGasSperrungNbWorkflow>(
                        &malo_id,
                        "geli-gas-sperrung-nb",
                        cmd,
                        &fv,
                        &[(
                            mako_geli_gas::GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL,
                            due_at,
                        )],
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
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays,
                    // Sunday 12:00 Berlin if received on Saturday.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeSupplierChangeWorkflow>(
                        &malo_id,
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
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §4).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeLfAbmeldungWorkflow>(
                        &malo_id,
                        "gpke-lf-abmeldung",
                        cmd,
                        &fv,
                        &[
                            (mako_gpke::LF_ABMELDUNG_APERAK_WINDOW_LABEL, process_due_at),
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
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<GeliGasSupplierChangeWorkflow>(
                        &malo_id,
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
                    self.resume_by_malo::<GpkeAllokationslisteWorkflow>(
                        &malo_id,
                        "gpke-allokationsliste",
                        cmd,
                    )
                    .await
                }
                13014 => {
                    let cmd =
                        adapters::gpke_allokationsliste_mscons_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GpkeAllokationslisteWorkflow>(
                        &malo_id,
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
                    // Process Frist: 5 Werktage (BK6-24-174 WiM Strom Teil 1).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let process_due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<WimDeviceChangeWorkflow>(
                        &melo_id,
                        "wim-device-change",
                        cmd,
                        &fv,
                        &[
                            (mako_wim::GERAETEWECHSEL_APERAK_WINDOW_LABEL, process_due_at),
                            (fristen::APERAK_STROM_WINDOW_LABEL, aperak_due_at),
                        ],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-device-change",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── WiM Geräteübernahme (nMSB role) ──────────────────────────────
            // PIDs 17001/17002: nMSB → NB Anfrage/Weiterverpflichtung — spawn.
            // PID  17005:       NB   → MSB Bestellung Rechnungsabwicklung — spawn.
            // PIDs 17009/17011: Stornierung (ORDCHG) — spawn.
            // PIDs 19001/19002: ORDRSP Bestätigung/Ablehnung (NB → nMSB) — resume.
            //
            // Note: PIDs 19001/19002 are multi-domain — GPKE Konfiguration (NB role)
            // and WiM Geräteübernahme (nMSB role) share them.  Role-conditional
            // routing in the PidRouter ensures only one workflow is registered per
            // role (both cannot be active simultaneously — build() panics if both are).
            "wim-geraeteubernahme" => match pid {
                17001 | 17002 | 17005 | 17009 | 17011 => {
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

            // ── WiM Stornierung (Strom) — PID 39002 ──────────────────────────
            // PID 39002: Stornierung der Bestellung (ORDCHG, nMSB → NB) — spawn.
            "wim-stornierung" => match pid {
                39002 => {
                    let cmd = adapters::wim_stornierung_registry().dispatch(raw, &fv)?;
                    let melo_id = extract_melo_from_orders(msg);
                    // No APERAK Frist for pure stornierung — no outbox deadline needed.
                    self.spawn_or_resume::<WimStornierungWorkflow>(
                        &melo_id,
                        "wim-stornierung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-stornierung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── GPKE INVOIC billing — PIDs 31001/31002/31005/31006 ───────────
            //
            // The NB (invoicer) sends an INVOIC to the LF (payer).  `makod`
            // acting as the LF receives the INVOIC and spawns a new billing
            // process.  The settlement window is 24 wall-clock hours per
            // BK6-22-024.  After spawning, the `ProcessInitiated` outbox
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
                    // Settlement window: 24 wall-clock hours (BK6-22-024 §5).
                    let due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
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
                    self.resume_by_malo::<GpkeAbrechnungWorkflow>(
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
                    self.resume_by_malo::<GpkeAbrechnungWorkflow>(
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

            // ── WiM Rechnung (Strom) — PID 31009 ─────────────────────────────
            // PID 31009: MSB-Rechnung (MSB → NB, multi-domain GPKE/WiM) — spawn.
            "wim-rechnung" => match pid {
                31009 => {
                    let cmd = adapters::wim_rechnung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_invoic(msg);
                    // Settlement deadline: 5 Werktage (BK6-24-174 WiM Strom).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        5,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimRechnungWorkflow>(
                        &malo_id,
                        "wim-rechnung",
                        cmd,
                        &fv,
                        &[(mako_wim::WIM_RECHNUNG_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-rechnung",
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
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom ORDERS — 45 min on weekdays.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeKonfigurationWorkflow>(
                        &malo_id,
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
                    self.resume_by_malo::<GpkeKonfigurationWorkflow>(
                        &malo_id,
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
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeStornierungWorkflow>(
                        &malo_id,
                        "gpke-stornierung",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::STORNIERUNG_GPKE_APERAK_WINDOW_LABEL,
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
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeAnkuendigungZuordnungLfWorkflow>(
                        &malo_id,
                        "gpke-ankuendigung-zuordnung-lf",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL,
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

            // ── GeLi Gas MSCONS data delivery ─────────────────────────────────
            // PIDs 13002, 13007–13009: MSCONS Gas Messdaten (NB/MSB → LFG) — spawn.
            "geli-gas-mscons" => match pid {
                13002 | 13007 | 13008 | 13009 => {
                    let cmd = adapters::geli_gas_mscons_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Gas MSCONS data delivery — no APERAK Frist for pure data messages.
                    self.spawn_or_resume::<GeliGasMsconsWorkflow>(
                        &malo_id,
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
                        &malo_id,
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
                        &malo_id,
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

            // ── WiM Gas Anmeldung — PIDs 44042–44044, 44051–44053 ────────────
            // PIDs 44042–44044: Anmeldung neuer MSB Gas (MSBN ↔ NB) — spawn.
            // PIDs 44051–44053: Ende MSB Gas / Vorläufige Abmeldung (NB ↔ MSBA) — spawn.
            "wim-gas-anmeldung" => match pid {
                44042..=44044 | 44051..=44053 => {
                    let cmd = adapters::wim_gas_anmeldung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasAnmeldungWorkflow>(
                        &malo_id,
                        "wim-gas-anmeldung",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::anmeldung::RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-anmeldung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── WiM Gas Kündigung — PIDs 44039/44040/44041 ───────────────────
            // PID 44039: Kündigung MSB Gas Anfrage (MSBA → NB) — spawn.
            // PIDs 44040/44041: Bestätigung/Ablehnung (NB → MSBA) — spawn (NB-initiating path).
            "wim-gas-kuendigung" => match pid {
                44039..=44041 => {
                    let cmd = adapters::wim_gas_kuendigung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasKuendigungWorkflow>(
                        &malo_id,
                        "wim-gas-kuendigung",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::kuendigung::RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-kuendigung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── WiM Gas Verpflichtungsanfrage — PIDs 44168/44169/44170 ───────
            // PID 44168: Verpflichtungsanfrage (NB → gMSB) — spawn.
            // PIDs 44169/44170: Bestätigung/Ablehnung (gMSB → NB) — spawn.
            //
            // PID 44170 present in FV2025-10-01 (PID 3.3), absent from FV2026-10-01
            // (PID 4.0).  In-flight FV2025 processes may still receive it after the
            // cutover — the adapter handles it for forward compatibility.
            "wim-gas-verpflichtungsanfrage" => match pid {
                44168..=44170 => {
                    let cmd =
                        adapters::wim_gas_verpflichtungsanfrage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // APERAK Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasVerpflichtungsanfrageWorkflow>(
                        &malo_id,
                        "wim-gas-verpflichtungsanfrage",
                        cmd,
                        &fv,
                        &[(
                            mako_wim_gas::verpflichtungsanfrage::RESPONSE_WINDOW_LABEL,
                            due_at,
                        )],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-verpflichtungsanfrage",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── WiM Gas INVOIC billing — PIDs 31003/31004 ────────────────────
            // PID 31003: WiM-Rechnung Gas (gMSB → NB) — spawn.
            // PID 31004: Stornorechnung WiM Gas (gMSB → NB) — spawn.
            "wim-gas-invoic" => match pid {
                31003 | 31004 => {
                    let cmd = adapters::wim_gas_invoic_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_invoic(msg);
                    // Settlement deadline: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasInvoicWorkflow>(
                        &malo_id,
                        "wim-gas-invoic",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::invoic::SETTLEMENT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-invoic",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── WiM Gas INSRPT — PIDs 23001/23003–23005/23008/23009 ──────────
            // PID 23001: Anfrage Störungsmeldung (shared, gMSB → NB) — spawn.
            // PIDs 23003/23004/23008: Antwort (shared, NB → gMSB) — spawn.
            // PIDs 23005/23009: Gas-only variants — spawn.
            "wim-gas-insrpt" => match pid {
                23001 | 23003 | 23004 | 23005 | 23008 | 23009 => {
                    let cmd = adapters::wim_gas_insrpt_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // INSRPT Frist: 10 Werktage (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasInsrptWorkflow>(
                        &malo_id,
                        "wim-gas-insrpt",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::insrpt::ANTWORT_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-insrpt",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── MABIS Clearingliste — PIDs 55065/55069/55070 ──────────────────
            // PIDs 55065/55069/55070: MABIS IFTSTA Clearingliste (BKV ↔ ÜNB) — spawn.
            "mabis-clearingliste" => match pid {
                55065 | 55069 | 55070 => {
                    let cmd = adapters::mabis_clearingliste_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No APERAK Frist for pure MABIS data messages.
                    self.spawn_or_resume::<MabisClearinglisteWorkflow>(
                        &malo_id,
                        "mabis-clearingliste",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "mabis-clearingliste",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

            // ── GaBi Gas INVOIC billing (PIDs 31010, 31007, 31008) ───────────
            // PIDs 31010/31007/31008: inbound INVOIC (payer receives) — spawn.
            // PID 33001 (REMADV): payment confirmation from payer — resume.
            // PID 29001 (COMDIS): payment rejection by invoicer — resume.
            //
            // Regulatory basis: BK7-14-020 (GaBi Gas 2.0).
            // Settlement window: no statutory deadline in BK7-14-020;
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
                        &malo_id,
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

            // ── GPKE Neuanlage (PIDs 55600, 55601) ────────────────────────────
            "gpke-neuanlage" => match pid {
                55600 | 55601 => {
                    let cmd = adapters::gpke_neuanlage_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeNeuanlageWorkflow>(
                        &malo_id,
                        "gpke-neuanlage",
                        cmd,
                        &fv,
                        &[
                            (
                                mako_gpke::neuanlage::NEUANLAGE_APERAK_WINDOW_LABEL,
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

            // ── GPKE Anfrage / Bestellung (PID 55555) ─────────────────────────
            "gpke-anfrage-bestellung" => match pid {
                55555 => {
                    let cmd = adapters::gpke_anfrage_bestellung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Process Frist: 24 wall-clock hours (BK6-22-024 §5).
                    // APERAK AHB 1.0 §2.4.1: Strom UTILMD — 45 min on weekdays.
                    let process_due_at = fristen::add_hours(OffsetDateTime::now_utc(), 24);
                    let aperak_due_at = fristen::aperak_strom_due_at(OffsetDateTime::now_utc());
                    self.spawn_or_resume::<GpkeAnfrageBestellungWorkflow>(
                        &malo_id,
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
                        &malo_id,
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
                    // No deadline for pure data delivery.
                    self.spawn_or_resume::<GpkeMesswerteLieferungWorkflow>(
                        &malo_id,
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
                        &malo_id,
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
                    self.resume_by_malo::<GpkeDatanabrufWorkflow>(&malo_id, "gpke-datenabruf", cmd)
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
                if mako_gpke::konfiguration_aenderung::ORDRSP_PIDS.contains(&pid) {
                    let cmd =
                        adapters::gpke_konfiguration_aenderung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_malo::<GpkeKonfigurationAenderungWorkflow>(
                        &malo_id,
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

            // ── GeLi Gas PARTIN Kommunikationsdaten (PIDs 37008–37014) ────────
            "geli-gas-partin" => {
                if mako_geli_gas::partin::PARTIN_GAS_PIDS.contains(&pid) {
                    let cmd = adapters::geli_gas_partin_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<GeliGasPartinWorkflow>(
                        &malo_id,
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

            // ── MABIS Bilanzkreisabrechnung IFTSTA (PIDs 21000–21005) ──────────
            "mabis-billing" => {
                if mako_mabis::bilanzkreisabrechnung::IFTSTA_PIDS.contains(&pid) {
                    let cmd = adapters::mabis_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Prüfmitteilung deadline: 1 Werktag (BK6-24-174 §13.8).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        1,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<MabisBillingWorkflow>(
                        &malo_id,
                        "mabis-billing",
                        cmd,
                        &fv,
                        &[(mako_mabis::PRUEFMITTEILUNG_DEADLINE_LABEL, due_at)],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-billing",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }

            // ── WiM Stammdaten ORDERS (PID 17132) ─────────────────────────────
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
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-stammdaten",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

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
                        &malo_id,
                        "wim-preisanfrage",
                        cmd,
                        &fv,
                        &[(mako_wim::preisanfrage::PREISANFRAGE_DEADLINE_LABEL, due_at)],
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
                        &malo_id,
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

            // ── GaBi Gas Nomination (PIDs 90011, 90012, 90021, 90022) ─────────
            // Regulatory basis: Kooperationsvereinbarung Gas (KoV), BK7-14-020.
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
                        &malo_id,
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
            // response deadline in BK7-14-020; ALOCAT is a one-way push from MMMA
            // to participants. spawn_deadline = None.
            "gabi-gas-allocation" => {
                if mako_gabi_gas::allocation::ALLOCATION_PIDS.contains(&pid) {
                    let cmd = adapters::gabi_gas_allocation_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Allocation list: no statutory response deadline defined.
                    self.spawn_or_resume::<GaBiGasAllocationWorkflow>(
                        &malo_id,
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

            // ── WiM Gas Stornierung — GNB side (PID 44022) ───────────────────
            // PID 44022: Anfrage nach Stornierung (LF → GNB) — spawn.
            // Response PIDs 44023/44024 are outbound (dispatched by the ERP layer); no
            // inbound arm needed on the GNB side.
            "wim-gas-stornierung" => match pid {
                44022 => {
                    let cmd = adapters::wim_gas_stornierung_registry().dispatch(raw, &fv)?;
                    let vorgang_id = extract_melo_from_utilmd(msg);
                    // WiM Gas: 10 Werktage response deadline (BK7-24-01-009).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        10,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<WimGasStornierungWorkflow>(
                        &vorgang_id,
                        "wim-gas-stornierung",
                        cmd,
                        &fv,
                        &[(mako_wim_gas::STORNIERUNG_RESPONSE_WINDOW_LABEL, due_at)],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "wim-gas-stornierung",
                    reason: "pid_not_in_dispatch_table",
                }),
            },

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
            "geli-gas-datenabruf" => Ok(IngestOutcome::Skipped {
                workflow_name: "geli-gas-datenabruf",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
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
            "redispatch-aktivierung" => Ok(IngestOutcome::Skipped {
                workflow_name: "redispatch-aktivierung",
                reason: "phase2_dispatch_not_yet_implemented",
            }),
            "wim-technik-aenderung" => Ok(IngestOutcome::Skipped {
                workflow_name: "wim-technik-aenderung",
                reason: "phase2_dispatch_not_yet_implemented",
            }),

            // ── All other workflows: not yet in Phase 2 dispatch table ────────
            //
            // WARNING: messages routed here are dead-lettered. Any workflow name
            // appearing here should be investigated and given an explicit dispatch arm.
            wf_name => {
                tracing::warn!(
                    workflow_name = %wf_name,
                    pid,
                    "ingest dispatcher: no Phase 2 handler — message dead-lettered; \
                     add a dispatch arm in ingest_dispatcher.rs to handle this workflow",
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
    ///
    /// `spawn_deadlines`: zero or more `(label, due_at)` pairs to register
    /// atomically with the events in a single `WriteBatch` via
    /// [`Process::execute_and_enqueue_with_deadlines`].  Pass `&[]` for
    /// workflows that have no deadlines (e.g. pure continuation handlers).
    /// Deadlines are only registered for freshly-spawned processes — resuming
    /// an existing process must not re-register (deadlines were set at spawn).
    ///
    /// To satisfy APERAK AHB 1.0 §2.4.1 for Strom UTILMD/ORDERS, callers
    /// should pass **two** deadlines: the process-response window and the
    /// 45-minute APERAK sending window (`fristen::APERAK_STROM_WINDOW_LABEL`).
    async fn spawn_or_resume<W>(
        &self,
        malo_id: &str,
        workflow_name_static: &'static str,
        cmd: W::Command,
        fv: &FormatVersion,
        spawn_deadlines: &[(&'static str, time::OffsetDateTime)],
    ) -> Result<IngestOutcome, EngineError>
    where
        W: Workflow + 'static,
        W::Command: CommandPayload + Clone,
        W::State: serde::Serialize,
        // `execute_and_enqueue_with_deadlines` requires `AtomicAppend`:
        Arc<SlateDbStore>: mako_engine::event_store::AtomicAppend,
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
            workflow_id.clone(),
        );
        let process_id = process.process_id();

        // Atomically persist events and (when applicable) the APERAK/process Frist
        // deadlines.  Using `execute_and_enqueue_with_deadlines` ensures a crash
        // between event write and deadline registration cannot produce a process with
        // no monitoring window (dual-write atomicity requirement).
        if spawn_deadlines.is_empty() {
            process
                .execute_and_enqueue_with_snapshot_and_retry(
                    cmd,
                    3,
                    &self.snap_store,
                    self.snapshot_interval,
                )
                .await?;
        } else {
            let deadlines: Vec<Deadline> = spawn_deadlines
                .iter()
                .map(|&(label, due_at)| {
                    Deadline::new(
                        process.stream_id().clone(),
                        process_id,
                        self.tenant_id,
                        workflow_id.clone(),
                        label,
                        due_at,
                    )
                })
                .collect();
            process
                .execute_and_enqueue_with_deadlines(cmd, &deadlines)
                .await?;
        }

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

/// Extract the Messlokations-ID from an ORDERS/ORDRSP/ORDCHG message's IDE segment.
///
/// WiM ORDERS messages (Geräteübernahme, Stammdaten, Stornierung) identify the
/// Messlokation in the `IDE` segment (element 1, component 0 = object ID).
/// Returns an empty string when the message is not an ORDERS/ORDRSP/ORDCHG or
/// when the IDE segment is absent.
pub fn extract_melo_from_orders(msg: &AnyMessage) -> String {
    let segs = match msg {
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        AnyMessage::Ordchg(o) => o.segments(),
        _ => return String::new(),
    };
    segs.iter()
        .find(|s| s.tag == "IDE")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned()
}

/// Extract the INVOIC sender/document reference for MaLo correlation.
///
/// INVOIC messages do not carry a LOC or IDE segment. Use the message reference
/// (UNH DE 0062) as the correlation key — workflows that receive INVOIC messages
/// as initial commands are keyed on the invoice reference, not a MaLo.
///
/// Returns an empty string when the message is not an INVOIC.
pub fn extract_malo_from_invoic(msg: &AnyMessage) -> String {
    match msg {
        AnyMessage::Invoic(_) => msg.message_ref().to_owned(),
        _ => String::new(),
    }
}

/// Extract the Marktlokations-ID from the first LOC segment (component 1, index 0).
///
/// BDEW convention: `LOC+<qualifier>+<malo_id>::<code_list>:Z13`.
/// Applies to ORDERS, ORDRSP, and UTILMD messages.
/// Returns an empty string when the LOC segment is absent (INVOIC, IFTSTA, …).
pub fn extract_malo_from_msg(msg: &AnyMessage) -> String {
    let segs = match msg {
        AnyMessage::Orders(o) => o.segments(),
        AnyMessage::Ordrsp(o) => o.segments(),
        AnyMessage::Utilmd(u) => u.segments(),
        // MSCONS carries the MaLo in LOC (same convention as ORDERS/ORDRSP).
        // Used by gpke-allokationsliste to correlate MSCONS 13013/13014 with the
        // process that was spawned when the LF sent ORDERS 17110/17114.
        AnyMessage::Mscons(m) => m.segments(),
        _ => return String::new(),
    };
    segs.iter()
        .find(|s| s.tag == "LOC")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned()
}

/// Extract the Messlokations-ID from the first UTILMD transaction's IDE segment.
///
/// WiM UTILMD messages (55039, 55042, 55051, 55168) identify the Messlokation in
/// the transaction header via `IDE+24+<melo_id>:::Z19` rather than the LOC segment
/// convention used by GPKE messages.  The `wim_registry()` adapter extracts the
/// MeLo from the same `transactions()[0].ide.object_id` path.
///
/// Returns an empty string when the message is not a UTILMD or the IDE is absent.
pub fn extract_melo_from_utilmd(msg: &AnyMessage) -> String {
    let AnyMessage::Utilmd(u) = msg else {
        return String::new();
    };
    u.transactions()
        .first()
        .and_then(|t| t.ide.object_id.as_deref())
        .unwrap_or("")
        .to_owned()
}

/// Extract the original invoice message-reference from a REMADV for process correlation.
///
/// BDEW convention: the REMADV carries `RFF+Z13:<original_message_ref>` where the
/// reference value is the UNH message-reference (DE 0062) of the originating INVOIC.
/// This matches the key used when spawning the billing process (`extract_malo_from_invoic`).
///
/// Falls back to the REMADV's own `msg.message_ref()` when the RFF+Z13 is absent.
pub fn extract_invoice_ref_from_remadv(msg: &AnyMessage) -> String {
    let AnyMessage::Remadv(r) = msg else {
        return msg.message_ref().to_owned();
    };
    r.segments()
        .iter()
        .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some("Z13"))
        .and_then(|s| s.component_str(0, 1))
        .map(|s| s.to_owned())
        .unwrap_or_else(|| msg.message_ref().to_owned())
}

/// Extract the original invoice message-reference from a COMDIS for process correlation.
///
/// Same `RFF+Z13` convention as [`extract_invoice_ref_from_remadv`].
pub fn extract_invoice_ref_from_comdis(msg: &AnyMessage) -> String {
    let AnyMessage::Comdis(c) = msg else {
        return msg.message_ref().to_owned();
    };
    c.segments()
        .iter()
        .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some("Z13"))
        .and_then(|s| s.component_str(0, 1))
        .map(|s| s.to_owned())
        .unwrap_or_else(|| msg.message_ref().to_owned())
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
    // Derive the fallback dynamically from the registry so it stays current
    // across annual format-version cutovers without a code change.
    let fallback = || {
        adapters::known_fvs().into_iter().max().unwrap_or_else(|| {
            // Last-resort: if the registry is empty (pathological), use
            // the current production FV. This branch should never fire.
            FormatVersion::parse("FV2025-10-01")
                .expect("FV2025-10-01 is always a valid FormatVersion literal")
        })
    };

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
