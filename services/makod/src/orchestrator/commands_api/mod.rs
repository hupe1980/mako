//! ERP command ingestion API — `POST /api/v1/commands`.
//!
//! The ERP names the MaKo command it wants triggered; the engine resolves the
//! Marktrolle, dispatches to the workflow and renders the EDIFACT. BO4E objects
//! are master data and are never used for routing, so each command takes a
//! typed [`ErpCommand`] envelope that embeds BO4E objects beside the process
//! fields BO4E does not carry.
//!
//! **Marktrolle.** A single-role command (`gpke.lieferbeginn.anmelden` → `LF`)
//! infers it from the name; a multi-role command
//! (`wim.geraetewechsel.beauftragen` → `NB` or `MSB`) requires `marktrolle`. A
//! role outside `--marktrollen` is refused with `422 role_not_configured`.
//!
//! **`Idempotency-Key`** is required. The accepted response is stored under it
//! for 24 h and replayed verbatim on a retry; reusing one key for a different
//! command or payload is refused with `422 idempotency_key_reuse`.
//!
//! ```json
//! {
//!   "command": "gpke.lieferbeginn.anmelden",
//!   "payload": { "malo_id": "10001234558", "lieferbeginn_datum": "2026-10-01" }
//! }
//! ```
//!
//! ## What the ERP supplies vs. what the engine derives
//!
//! The ERP supplies only what the engine cannot know: which location the
//! process concerns, the process dates and process-specific hints. The engine
//! owns the rest:
//!
//! | Field | Source |
//! |---|---|
//! | sender MP-ID | the operator's own party (`--tenant-id`) |
//! | receiver MP-ID, MSB MP-ID, MeLo IDs | the MaLo cache, keyed by `marktlokationsId` |
//! | Prüfidentifikator | the command name |
//! | message reference, document date | the EDIFACT renderer at dispatch |
//!
//! The ERP seeds the MaLo cache via `PUT /admin/malo/{malo_id}` from the NB's
//! `MaloIdentResultPositive`; until it does, the command is refused with
//! `422 malo_not_found`. WiM Gerätewechsel is keyed by `melo_id` instead.
//!
//! The command set, its roles and primary PIDs live in
//! `registry::COMMAND_REGISTRY` — the single source this module dispatches
//! from.
//!
//! ## Invoicing commands (`gpke.abrechnung.*`)
//!
//! These carry flat scalar fields (`document_id`, `document_code`,
//! `document_date`, and on an Abweisung `antwort_code` / `antwort_codeliste` /
//! `ablehnungsgrund`), not a BO4E `Rechnung`. `makod` renders only the INVOIC
//! envelope with an empty `UNS+D` detail section; a document with positions is
//! built and gated by `billingd`, `invoicd` or `netzbilanzd`.

use mako_engine::process::RunOptions;
use std::sync::Arc;

use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use mako_engine::{
    deadline::Deadline,
    ids::{ProcessId, TenantId},
    marktrolle::{DeploymentRoles, LicensingError, Marktrolle, resolve_role},
    metrics::EngineMetrics,
    registry::ProcessRegistry as _,
    types::{BikoId, BillingPeriod, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::OccupiesBusinessKey as _,
};
use mako_gabi_gas::{
    GaBiGasInvoicWorkflow, GaBiGasMehrMindermengenWorkflow, GaBiGasNominationWorkflow, GasDay,
    INVOIC_WORKFLOW_NAME as GABI_GAS_INVOIC_WORKFLOW_NAME, MehrMindermengenCommand,
    NOMINATION_WORKFLOW_NAME as GABI_GAS_NOMINATION_WORKFLOW_NAME, NominationCommand,
    NomresAcceptance,
};
use mako_geli_gas::{
    GELI_GAS_DATENABRUF_WORKFLOW_NAME, GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
    GasSupplierChangeCommand, GeliGasDatenabrufCommand, GeliGasDatenabrufWorkflow,
    GeliGasLfAnmeldungCommand, GeliGasLfAnmeldungWorkflow, GeliGasSperrprozesseInvoicWorkflow,
    GeliGasSupplierChangeWorkflow, LF_ANMELDUNG_ANFRAGE_PIDS,
};
use mako_gpke::{
    AnkuendigungZuordnungLfCommand, GpkeAbrechnungWorkflow, GpkeAnkuendigungZuordnungLfWorkflow,
    GpkeLfAbmeldungWorkflow, GpkeLfAnmeldungWorkflow, GpkeSupplierChangeWorkflow,
    LfAnmeldungCommand, SupplierChangeCommand,
};
use mako_gpke::{
    GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, SPERRUNG_LF_ANFRAGE_PIDS,
    SPERRUNG_LF_ANTWORT_WINDOW_LABEL, SPERRUNG_LF_WORKFLOW_NAME, SPERRUNG_WORKFLOW_NAME,
    SperrungCommand, SperrungLfCommand,
};
use mako_invoic::InvoicCommand;
use mako_mabis::{
    BillingCommand, Datenstatus, IFTSTA_ABWEISUNG_PID as MABIS_IFTSTA_ABWEISUNG_PID,
    IFTSTA_DATENSTATUS_PIDS as MABIS_IFTSTA_DATENSTATUS_PIDS,
    IFTSTA_PRUEFMITTEILUNG_PIDS as MABIS_IFTSTA_PRUEFMITTEILUNG_PIDS, MabisBillingWorkflow,
    SzrVersion,
};
use mako_markt::repository::{
    NetzzugangAktion, NetzzugangAntrag, NetzzugangAntragTyp, NetzzugangStatus,
};
use mako_wim::invoic::WimInvoicWorkflow;
use mako_wim::steuerungsauftrag::{SteuerungsauftragCommand, WimSteuerungsauftragWorkflow};
use mako_wim::{DeviceChangeCommand, WimDeviceChangeWorkflow};
use mako_wim::{PreisanfrageCommand, WimPreisanfrageWorkflow};
use rubo4e::identifiers::{MaloId, MeloId};
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use crate::cedar_authz::{CedarAuthorizer, CommandResource};
use crate::malo_cache::{MaloIdentResultCache, SlateDbMaloCache};

// ── Identifier extraction helpers ─────────────────────────────────────────────

/// Extract and validate a `malo_id` field from an ERP JSON payload.
///
/// Returns a 422-ready [`DispatchError::InvalidPayload`] when:
/// - the `malo_id` key is absent or not a JSON string, or
/// - the string fails BDEW MaLo-ID validation (11 digits + alternating-weight
///   checksum, per BDEW MaLo-ID spec).
///
/// Returns the validated identifier as a domain-typed
/// [`mako_engine::types::MaLo`] (constructed from the rubo4e-validated value)
/// so the type survives past the boundary; string sinks use `.as_str()`.
fn extract_malo_id(payload: &serde_json::Value) -> Result<MaLo, DispatchError> {
    let s = payload
        .get("malo_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"malo_id\" (11-digit Marktlokations-ID)".into(),
            )
        })?;
    s.parse::<MaloId>()
        .map(|id| MaLo::new(id.as_ref()))
        .map_err(|e| DispatchError::InvalidPayload(format!("invalid malo_id {s:?}: {e}")))
}

/// Require a non-empty string field from an ERP JSON payload.
///
/// The 422 names the field, because a command refused for a missing key is
/// otherwise indistinguishable from one refused for a wrong value.
fn require_payload_str(
    payload: &serde_json::Value,
    field: &'static str,
) -> Result<String, DispatchError> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!("payload must contain a non-empty {field:?}"))
        })
}

/// `SG12 NAD+VY` — the beteiligte Marktpartner a Zuordnungs-Meldung names.
///
/// A list because UTILMD AHB Strom Bedingung `[518]` says „Es sind **alle**
/// Altlieferanten anzugeben, an die eine Abmeldeanfrage gesendet wird": a
/// Geschäftsvorfall-3 Marktlokation is split across Tranchen and so across
/// several LFA. A bare string is accepted as a one-element list, which is the
/// shape every other case takes.
fn beteiligte_marktpartner(payload: &serde_json::Value) -> Vec<MarktpartnerCode> {
    match payload.get("beteiligte_marktpartner") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(MarktpartnerCode::new)
            .collect(),
        Some(serde_json::Value::String(one)) if !one.is_empty() => {
            vec![MarktpartnerCode::new(one.clone())]
        }
        _ => Vec::new(),
    }
}

/// Spawn a one-message Meldepflicht process and register it under its MaLo.
///
/// No duplicate guard and no deadline. A Meldepflicht has no Antwortnachricht,
/// so nothing resumes the process and nothing can be late on it; and three of
/// them are owed on the same Marktlokation around one Lieferbeginn, so refusing
/// the second as a duplicate would drop an obligation the Festlegung states.
async fn spawn_meldung_process<W>(
    state: &CommandsApiState,
    workflow_name: &'static str,
    malo_id: &MaLo,
    command: W::Command,
) -> Result<DispatchOutcome, DispatchError>
where
    W: mako_engine::workflow::Workflow,
    Arc<mako_engine::store_slatedb::SlateDbStore>: mako_engine::event_store::AtomicAppend,
{
    let process_id = spawn_process::<W>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(workflow_name, latest_format_version()),
        command,
        &[],
        &[malo_id.as_str()],
    )
    .await?;
    Ok(DispatchOutcome::Spawned { process_id })
}

/// Extract and validate a `melo_id` field from an ERP JSON payload.
///
/// Returns a 422-ready [`DispatchError::InvalidPayload`] when:
/// - the `melo_id` key is absent or not a JSON string, or
/// - the string fails BDEW MeLo-ID validation (ISO 3166-1 alpha-2 prefix
///   + 31 digits, per BO4E / WiM spec).
fn extract_melo_id(payload: &serde_json::Value) -> Result<MeLo, DispatchError> {
    let s = payload
        .get("melo_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"melo_id\" (Messlokations-ID, e.g. DE000…)".into(),
            )
        })?;
    s.parse::<MeloId>()
        .map(|id| MeLo::new(id.as_ref()))
        .map_err(|e| DispatchError::InvalidPayload(format!("invalid melo_id {s:?}: {e}")))
}

/// Extract a `invoice_ref` field from an ERP JSON payload.
///
/// Used for billing commands (`gpke.abrechnung.annehmen`/`ablehnen`) where the
/// business key is the INVOIC message-reference rather than a MaLo-ID.
/// Returns a 422-ready [`DispatchError::InvalidPayload`] when the field is absent.
fn extract_invoice_ref(payload: &serde_json::Value) -> Result<String, DispatchError> {
    payload
        .get("invoice_ref")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"invoice_ref\" (EDIFACT INVOIC message reference)".into(),
            )
        })
}

/// The Belegnummer of the outbound REMADV a settle/dispute command emits.
///
/// It must equal the wire UNH reference the renderer puts out, because the
/// invoice issuer correlates its process by it. Callers may supply one
/// (`remadv_ref`) when they are re-driving an answer; otherwise a fresh one is
/// minted, which is the ordinary case — the answer is sent once.
fn remadv_message_ref(payload: &serde_json::Value) -> mako_engine::types::MessageRef {
    let raw = payload
        .get("remadv_ref")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                crate::edifact_renderer::msg_ref_from_uuid(&format!(
                    "REMADV{}",
                    uuid::Uuid::new_v4()
                ))
            },
            str::to_owned,
        );
    mako_engine::types::MessageRef::new(raw)
}

/// Read the published Antwortcode(s) a Nicht-Zahlungsavis must carry.
///
/// `SG7 AJT` is **Muss** on every REMADV Abweisung — DE 4465 the code, DE 1082
/// the Entscheidungsbaum that publishes it — so a refusal without one gives the
/// issuer nothing to correct. `invoicd` resolves both from
/// [`mako_pruefung::codes::rechnungspruefung`] and puts them here.
///
/// `None` when the caller supplied neither. The workflow then falls back to the
/// plain Abweisung 33002 and the renderer refuses to emit an `AJT` it does not
/// have, which surfaces as an incomplete answer rather than a wrong one.
fn remadv_antwort(payload: &serde_json::Value) -> Option<mako_invoic::RemadvAntwort> {
    let ebd = payload
        .get("antwort_codeliste")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    // The itemised form, when `invoicd` walked a tree that answers with a set.
    let befunde: Vec<mako_invoic::RemadvBefund> = payload
        .get("antwort_befunde")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| {
            payload
                .get("antwort_code")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|code| {
                    vec![mako_invoic::RemadvBefund {
                        code: code.to_owned(),
                        ebene: "summe".to_owned(),
                        positionsnummer: None,
                        detail: None,
                    }]
                })
                .unwrap_or_default()
        });
    if befunde.is_empty() {
        return None;
    }
    let remadv_pid = payload
        .get("remadv_pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or(mako_invoic::ABWEISUNG_PID);
    Some(mako_invoic::RemadvAntwort {
        ebd: ebd.to_owned(),
        befunde,
        remadv_pid,
    })
}

// ── Submodules ────────────────────────────────────────────────────────────────
//
// `types`, `handler`, and `registry` hold the command-API machinery; the
// remaining submodules hold one market-communication domain each. Everything
// is re-exported here so the flat `crate::commands_api::*` paths keep working.

mod esa;
mod gabi_gas;
mod geli_gas;
mod gpke;
mod handler;
pub mod idempotency;
mod iftsta;
mod mabis;
mod netzbilanz;
mod netzzugang;
mod registry;
mod types;
mod wim;

#[cfg(test)]
mod geli_frist_tests;
#[cfg(test)]
mod spawn_tests;

pub use handler::*;
pub use registry::*;
pub use types::*;

use esa::*;
use gabi_gas::*;
use geli_gas::*;
use gpke::*;
use iftsta::*;
use mabis::*;
use netzbilanz::*;
use netzzugang::*;
use wim::*;

/// Return the current BDEW format-version `WorkflowId` for `gpke-lf-anmeldung`.
///
/// Uses the most recent format version from the compiled `edi-energy` profile
/// registry — the same strategy as the AS4 ingest layer.
fn current_workflow_id() -> WorkflowId {
    // Prefer the latest compiled FV.  Falls back to a known good FV if
    // the registry is somehow empty (should never happen in production).
    WorkflowId::new(
        mako_gpke::lf_anmeldung::WORKFLOW_NAME,
        latest_format_version(),
    )
}

/// The latest BDEW format version **in force today**, for stamping an outbound
/// process.
///
/// Not simply the registry's newest: a profile is compiled in as soon as BDEW
/// publishes it, six months before its Anwendungszeitpunkt (Allgemeine
/// Festlegungen 6.1d § 2.5). EDIFACT has no Übergangsfrist, so a message
/// stamped with the next release before its Stichtag is refused by the
/// counterparty — the registry is filtered to `valid_from <= heute()` first.
fn latest_format_version() -> mako_engine::version::FormatVersion {
    edi_energy::registry::ReleaseRegistry::global()
        .format_versions_in_force_on(mako_fristen::heute())
        .into_iter()
        .filter_map(|s| mako_engine::version::FormatVersion::parse(&s).ok())
        .max()
        .unwrap_or_else(|| {
            mako_engine::version::FormatVersion::parse("FV2025-10-01")
                .expect("fallback FV is valid")
        })
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

/// Spawn a fresh `W` process and write everything the spawn consists of in one
/// atomic batch: its events, its outbox entries, its `deadlines` and its
/// correlation `keys`.
///
/// The keys are not an index to be written afterwards. The duplicate guard
/// ([`find_occupying_process`]) and every follow-up command find a process
/// only through them, so a spawn whose events and outbound message are durable
/// while its key is not is invisible: the follow-up answers `process_not_found`
/// and the ERP's re-issue of the same command spawns a second process and sends
/// a second message the counterparty cannot be made to forget. One batch makes
/// the spawn either findable or absent. The ingest spawn writes the same batch
/// (`Process::run` with `RunOptions::correlations`), and
/// `cargo xtask check-atomic-spawn` refuses a door that registers a key in a
/// separate write after it spawns.
///
/// `deadlines` are `(label, due_at)` pairs; each label must be one the
/// workflow's deadline arm matches.
///
/// # Errors
///
/// Propagates the workflow's rejection of `command` and any store failure,
/// including a key the correlation index refuses. Nothing is written then.
pub(crate) async fn spawn_process<W>(
    store: &Arc<mako_engine::store_slatedb::SlateDbStore>,
    tenant_id: TenantId,
    workflow_id: WorkflowId,
    command: W::Command,
    deadlines: &[(&'static str, time::OffsetDateTime)],
    keys: &[&str],
) -> Result<ProcessId, mako_engine::error::EngineError>
where
    W: mako_engine::workflow::Workflow,
    Arc<mako_engine::store_slatedb::SlateDbStore>: mako_engine::event_store::AtomicAppend,
{
    let process =
        mako_engine::process::Process::<W, Arc<mako_engine::store_slatedb::SlateDbStore>>::new(
            Arc::clone(store),
            tenant_id,
            workflow_id.clone(),
        );
    let process_id = process.process_id();
    let identity = process.identity();
    let deadlines: Vec<Deadline> = deadlines
        .iter()
        .map(|&(label, due_at)| {
            Deadline::new(
                process.stream_id().clone(),
                process_id,
                tenant_id,
                workflow_id.clone(),
                label,
                due_at,
            )
        })
        .collect();
    #[cfg_attr(
        not(test),
        expect(unused_mut, reason = "only the test fault point appends")
    )]
    let mut correlations: Vec<mako_engine::event_store::CorrelationEntry> = keys
        .iter()
        .map(|&key| mako_engine::event_store::CorrelationEntry {
            tenant_id,
            tag: key.to_owned(),
            process_id,
            identity: identity.clone(),
        })
        .collect();
    #[cfg(test)]
    if key_registration_fault() {
        // A tag the correlation index refuses, so the store itself rejects the
        // key write.
        correlations.push(mako_engine::event_store::CorrelationEntry {
            tenant_id,
            tag: "fault\0key".to_owned(),
            process_id,
            identity,
        });
    }
    process
        .run(
            command,
            RunOptions::new()
                .deadlines(&deadlines)
                .correlations(&correlations),
        )
        .await?;
    Ok(process_id)
}

#[cfg(test)]
tokio::task_local! {
    /// Test-only fault point: while set, the spawn's business-key write is
    /// refused by the store.
    pub(crate) static KEY_REGISTRATION_FAULT: bool;
}

/// Whether [`KEY_REGISTRATION_FAULT`] is set for the current task.
#[cfg(test)]
pub(crate) fn key_registration_fault() -> bool {
    KEY_REGISTRATION_FAULT.try_with(|f| *f).unwrap_or(false)
}

// ── Duplicate guard ───────────────────────────────────────────────────────────

/// What [`find_occupying_process`] found, and the business-key lock it took.
///
/// The caller holds this until its spawn has committed: the lock is what makes
/// the occupancy check and the spawn one step, and dropping it early reopens the
/// window two concurrent spawns pass through together.
struct KeyOccupancy {
    /// The process still holding the key, if any.
    process: Option<ProcessId>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Return the process that still occupies `business_key`, if any.
///
/// This is the check every `anmelden`-style dispatcher runs before spawning a
/// process, and it must not be written as "does the correlation index contain
/// an entry?". The spawn writes the entry ([`spawn_process`]) and the process
/// ending does not remove it, so presence proves a process once existed, never
/// that one is still running. Refusing on presence would block a MaLo
/// permanently after its first process, including the routine flow where the
/// counterpart rejects and the sender submits a correction.
///
/// So each candidate is rehydrated and asked
/// [`OccupiesBusinessKey::occupies_business_key`]. That also makes the guard
/// self-healing: a stale index entry stops blocking as soon as its process
/// reaches a terminal state, with no cleanup job to run or to forget.
///
/// # Errors
///
/// Propagates store failures from the correlation lookup or the state replay.
async fn find_occupying_process<W>(
    state: &CommandsApiState,
    business_key: &str,
    workflow_name: &str,
) -> Result<KeyOccupancy, DispatchError>
where
    W: mako_engine::workflow::Workflow,
    W::State: mako_engine::workflow::OccupiesBusinessKey,
{
    // Held by the caller across this check and its spawn: the ingest
    // dispatcher spawns on the same index under the same lock, so a concurrent
    // command, or an inbound message for the same key, waits rather than
    // passing the check beside this one and spawning a second process.
    let guard = crate::ingest_dispatcher::business_key_locks()
        .acquire(business_key)
        .await;
    let existing = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, business_key)
        .await
        .map_err(DispatchError::Engine)?;

    for identity in existing
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == workflow_name)
    {
        let process_id = identity.process_id;
        let candidate = mako_engine::process::Process::<
            W,
            Arc<mako_engine::store_slatedb::SlateDbStore>,
        >::from_identity(Arc::clone(&state.store), identity);
        if candidate
            .state()
            .await
            .map_err(DispatchError::Engine)?
            .occupies_business_key()
        {
            return Ok(KeyOccupancy {
                process: Some(process_id),
                _guard: guard,
            });
        }
        // Retire the stale index entry now that replay has *proved* the process
        // finished. This is what keeps the index's meaning true — "processes that
        // may still be resumed by this business key" — and it is not merely
        // housekeeping:
        //
        // `dispatch_to_process_keyed` requires exactly one match for a business
        // key and fails with `AmbiguousProcess` on two. Allowing a second process
        // (which is the whole point of this guard) would therefore break every
        // follow-up command — `bestaetigen`, `ablehnen`, `aktivieren` — on that
        // MaLo. Pruning here means that by the time a replacement process is
        // registered, the finished one is already gone, so the resume path still
        // sees exactly one.
        //
        // A failure to prune is logged, not propagated: the guard's answer is
        // already known and correct, and refusing a legitimate Anmeldung because
        // a cleanup write failed would be the worse outcome. The next dispatch
        // retries the prune.
        if let Err(e) = state
            .store
            .as_process_registry()
            .remove_correlated(state.tenant_id, business_key, process_id)
            .await
        {
            tracing::warn!(
                business_key,
                workflow_name,
                %process_id,
                error = %e,
                "duplicate guard: could not retire the finished process's correlation entry",
            );
        } else {
            tracing::debug!(
                business_key,
                workflow_name,
                %process_id,
                "duplicate guard: prior process has finished — entry retired, not blocking",
            );
        }
    }
    Ok(KeyOccupancy {
        process: None,
        _guard: guard,
    })
}

// ── Phase 3 — Response command dispatchers ────────────────────────────────────
//
// These dispatchers look up an active process by MaLo / MeLo business key
// using the `ProcessRegistry` correlated index and route the ERP response
// command into the appropriate workflow.

/// Look up the single active process for `business_key` under `workflow_name`
/// and execute `make_command` against it.
///
/// Returns `ProcessNotFound` when no active process exists and `AmbiguousProcess`
/// when more than one active process is found (data integrity violation).
async fn dispatch_to_process<W, F>(
    state: &CommandsApiState,
    business_key: &str,
    workflow_name: &'static str,
    make_command: F,
) -> Result<DispatchOutcome, DispatchError>
where
    W: mako_engine::workflow::Workflow,
    W::Command: mako_engine::workflow::CommandPayload + Clone,
    W::State: Default + serde::Serialize,
    F: FnOnce() -> W::Command,
{
    dispatch_to_process_keyed::<W, F>(state, business_key, workflow_name, &[], make_command).await
}

/// [`dispatch_to_process`] that additionally indexes the resumed process under
/// `extra_keys`.
///
/// The ESA registers its process under the Belegnummer of each outbound
/// REQOTE/ORDERS/ORDCHG it sends, so the MSB's answer — which carries no LOC —
/// can be correlated back by the reference that PID's own Zuordnungsschlüssel
/// names (`AAV`, `ON` or `ACW`; see [`mako_wim::esa::korrelation`]).
async fn dispatch_to_process_keyed<W, F>(
    state: &CommandsApiState,
    business_key: &str,
    workflow_name: &'static str,
    extra_keys: &[&str],
    make_command: F,
) -> Result<DispatchOutcome, DispatchError>
where
    W: mako_engine::workflow::Workflow,
    W::Command: mako_engine::workflow::CommandPayload + Clone,
    W::State: Default + serde::Serialize,
    F: FnOnce() -> W::Command,
{
    // ── Lookup active process(es) for business key ────────────────────────────
    let registry = state.store.as_process_registry();
    let identities = registry
        .lookup_correlated(state.tenant_id, business_key)
        .await
        .map_err(|e| {
            DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string()))
        })?;

    // Filter to identities belonging to the target workflow.
    let matching: Vec<_> = identities
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == workflow_name)
        .collect();

    let identity = match matching.len() {
        0 => {
            return Err(DispatchError::ProcessNotFound {
                business_key: business_key.to_owned(),
                workflow_name,
            });
        }
        1 => matching.into_iter().next().expect("len == 1"),
        n => {
            return Err(DispatchError::AmbiguousProcess {
                business_key: business_key.to_owned(),
                count: n,
            });
        }
    };

    let process_id = identity.process_id;
    // The Belegnummern land in the same write as the command's events: an
    // answer that references only this Belegnummer has no other way back to
    // the process, so the key must be durable exactly when the message is.
    let keys: Vec<mako_engine::event_store::CorrelationEntry> = extra_keys
        .iter()
        .filter(|k| !k.is_empty())
        .map(|k| mako_engine::event_store::CorrelationEntry {
            tenant_id: state.tenant_id,
            tag: (*k).to_owned(),
            process_id,
            identity: identity.clone(),
        })
        .collect();
    let process = mako_engine::process::Process::<
        W,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::from_identity(Arc::clone(&state.store), identity);

    let domain_cmd = make_command();
    process
        .run(
            domain_cmd,
            RunOptions::new()
                .correlations(&keys)
                .snapshot(&state.snapshot_store, state.snapshot_interval),
        )
        .await?;

    Ok(DispatchOutcome::Dispatched { process_id })
}

/// [`dispatch_to_process`] where the command depends on the process's own
/// state.
///
/// Needed when a default can only be resolved from what the process is
/// answering: the WiM MSB-Wechsel Antwortcode comes from the inbound PID's
/// Entscheidungsbaum, and the caller of the REST command knows the MeLo but not
/// which of the four processes is running on it.
async fn dispatch_to_process_with_state<W, F>(
    state: &CommandsApiState,
    business_key: &str,
    workflow_name: &'static str,
    make_command: F,
) -> Result<DispatchOutcome, DispatchError>
where
    W: mako_engine::workflow::Workflow,
    W::Command: mako_engine::workflow::CommandPayload + Clone,
    W::State: Default + serde::Serialize,
    F: FnOnce(&W::State) -> Result<W::Command, DispatchError>,
{
    let registry = state.store.as_process_registry();
    let identities = registry
        .lookup_correlated(state.tenant_id, business_key)
        .await
        .map_err(|e| {
            DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string()))
        })?;
    let matching: Vec<_> = identities
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == workflow_name)
        .collect();
    let identity = match matching.len() {
        0 => {
            return Err(DispatchError::ProcessNotFound {
                business_key: business_key.to_owned(),
                workflow_name,
            });
        }
        1 => matching.into_iter().next().expect("len == 1"),
        n => {
            return Err(DispatchError::AmbiguousProcess {
                business_key: business_key.to_owned(),
                count: n,
            });
        }
    };
    let process_id = identity.process_id;
    let process = mako_engine::process::Process::<
        W,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::from_identity(Arc::clone(&state.store), identity);

    let current = process.state().await?;
    let domain_cmd = make_command(&current)?;
    process
        .run(
            domain_cmd,
            RunOptions::new().snapshot(&state.snapshot_store, state.snapshot_interval),
        )
        .await?;

    Ok(DispatchOutcome::Dispatched { process_id })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(r: &[&str]) -> Vec<String> {
        r.iter().map(|s| s.to_string()).collect()
    }

    // ── Single-role commands ──────────────────────────────────────────────────

    #[test]
    fn single_role_inferred_without_marktrolle() {
        let role = validate_command("gpke.lieferbeginn.anmelden", None, &roles(&["LF"])).unwrap();
        assert_eq!(role, "LF");
    }

    #[test]
    fn single_role_asserted_marktrolle_is_ignored() {
        // For single-role commands the asserted marktrolle is always ignored.
        // An ERP connector that always sends marktrolle should not be rejected.
        let role =
            validate_command("gpke.lieferbeginn.anmelden", Some("NB"), &roles(&["LF"])).unwrap();
        assert_eq!(role, "LF"); // canonical role, not what ERP asserted
    }

    #[test]
    fn nb_role_resolved_for_bestaetigen() {
        let role =
            validate_command("gpke.lieferbeginn.bestaetigen", None, &roles(&["NB"])).unwrap();
        assert_eq!(role, "NB");
    }

    #[test]
    fn gnb_role_resolved_for_gas_bestaetigen() {
        let role =
            validate_command("geli.lieferbeginn.bestaetigen", None, &roles(&["GNB"])).unwrap();
        assert_eq!(role, "GNB");
    }

    #[test]
    fn lfg_role_resolved_for_gas_lieferbeginn() {
        let role = validate_command("geli.lieferbeginn.anmelden", None, &roles(&["LFG"])).unwrap();
        assert_eq!(role, "LFG");
    }

    // ── Instance configuration enforcement ───────────────────────────────────

    #[test]
    fn lf_instance_may_issue_lf_commands() {
        let lf = roles(&["LF"]);
        assert!(validate_command("gpke.lieferbeginn.anmelden", None, &lf).is_ok());
        assert!(validate_command("gpke.lieferende.anmelden", None, &lf).is_ok());
        assert!(validate_command("gpke.kuendigung.anmelden", None, &lf).is_ok());
    }

    #[test]
    fn nb_instance_may_not_issue_lf_command() {
        // Resolves to LF; NB is not configured → RoleNotConfigured.
        let nb = roles(&["NB"]);
        assert!(validate_command("gpke.lieferbeginn.anmelden", None, &nb).is_err());
    }

    #[test]
    fn lf_instance_may_not_issue_nb_command() {
        // Resolves to NB; LF is not configured for NB role.
        let lf = roles(&["LF"]);
        assert!(validate_command("gpke.lieferbeginn.bestaetigen", None, &lf).is_err());
    }

    #[test]
    fn dual_fuel_instance_permits_strom_and_gas() {
        let lf_lfg = roles(&["LF", "LFG"]);
        assert!(validate_command("gpke.lieferbeginn.anmelden", None, &lf_lfg).is_ok());
        assert!(validate_command("geli.lieferbeginn.anmelden", None, &lf_lfg).is_ok());
    }

    #[test]
    fn strom_only_instance_blocks_gas_commands() {
        let lf = roles(&["LF"]);
        assert!(validate_command("geli.lieferbeginn.anmelden", None, &lf).is_err());
    }

    #[test]
    fn gas_only_instance_blocks_strom_commands() {
        let lfg = roles(&["LFG"]);
        assert!(validate_command("gpke.lieferbeginn.anmelden", None, &lfg).is_err());
    }

    // ── Multi-role commands ───────────────────────────────────────────────────

    #[test]
    fn multi_role_requires_marktrolle() {
        // wim.geraetewechsel.beauftragen → NB or MSB; no marktrolle → err.
        assert!(validate_command("wim.geraetewechsel.beauftragen", None, &roles(&["NB"])).is_err());
    }

    #[test]
    fn nb_may_initiate_geraetewechsel() {
        assert!(
            validate_command(
                "wim.geraetewechsel.beauftragen",
                Some("NB"),
                &roles(&["NB"])
            )
            .is_ok()
        );
    }

    #[test]
    fn msb_may_initiate_geraetewechsel() {
        assert!(
            validate_command(
                "wim.geraetewechsel.beauftragen",
                Some("MSB"),
                &roles(&["MSB"])
            )
            .is_ok()
        );
    }

    #[test]
    fn lf_may_not_initiate_geraetewechsel() {
        // LF is not in the permitted set → RoleNotPermitted.
        assert!(
            validate_command(
                "wim.geraetewechsel.beauftragen",
                Some("LF"),
                &roles(&["LF"])
            )
            .is_err()
        );
    }

    /// The Sparte-neutral Storno must be answerable by everyone it can reach.
    ///
    /// PID 31004 cancels an invoice from *any* billing family (INVOIC AHB
    /// § 3.1.2 — GPKE, MMM, WiM Strom + Gas, Kapazität, AWH, GeLi), so whoever
    /// could receive the original can receive its cancellation. Its permitted
    /// set is the union of the families' own sets, derived here rather than
    /// trusted: a role missing from the hand-written list cannot answer a
    /// cancellation of an invoice it received itself.
    #[test]
    fn the_storno_answers_every_family_it_cancels() {
        let family_commands = [
            "gpke.abrechnung.annehmen",
            "wim.rechnung.annehmen",
            "gabi.rechnung.annehmen",
            "invoic.sonstige-leistung.annehmen",
        ];
        let mut required: Vec<Marktrolle> = Vec::new();
        for command in family_commands {
            let d = COMMAND_REGISTRY
                .iter()
                .find(|d| d.name == command)
                .unwrap_or_else(|| panic!("{command} is registered"));
            for role in d.permitted_roles {
                if !required.contains(role) {
                    required.push(*role);
                }
            }
        }

        let storno = COMMAND_REGISTRY
            .iter()
            .find(|d| d.name == "invoic.stornorechnung.annehmen")
            .expect("the Storno command is registered");
        for role in &required {
            assert!(
                storno.permitted_roles.contains(role),
                "{role:?} can receive a billing invoice but not its Stornorechnung"
            );
        }
    }

    /// The GaBi Gas billing family reaches **two** roles, and the command that
    /// answers it must permit both.
    ///
    /// One workflow settles all three PIDs: the MGV receives the aggregated
    /// MMM-Rechnung 31007/31008, the BKV the Kapazitätsrechnung 31010. A
    /// descriptor naming one PID must still permit both, or the recipient of
    /// the other cannot answer its own invoices.
    #[test]
    fn the_gabi_billing_family_is_answerable_by_both_of_its_recipients() {
        for role in ["MGV", "BKV"] {
            for command in ["gabi.rechnung.annehmen", "gabi.rechnung.ablehnen"] {
                assert!(
                    validate_command(command, Some(role), &roles(&[role])).is_ok(),
                    "{role} receives a GaBi Gas invoice and must be able to answer it \
                     with {command}"
                );
            }
        }
    }

    /// …and not by a role that receives none of them.
    #[test]
    fn a_supplier_may_not_answer_a_gabi_gas_invoice() {
        assert!(validate_command("gabi.rechnung.annehmen", Some("LF"), &roles(&["LF"])).is_err());
    }

    #[test]
    fn nb_msb_instance_permits_both_wim_roles() {
        let nb_msb = roles(&["NB", "MSB"]);
        assert!(validate_command("wim.geraetewechsel.beauftragen", Some("NB"), &nb_msb).is_ok());
        assert!(validate_command("wim.geraetewechsel.beauftragen", Some("MSB"), &nb_msb).is_ok());
    }

    #[test]
    fn nb_only_instance_may_not_act_as_msb() {
        let nb = roles(&["NB"]);
        // MSB is permitted by the registry but not configured on this instance.
        assert!(validate_command("wim.geraetewechsel.beauftragen", Some("MSB"), &nb).is_err());
    }

    // ── BKV / ÜNB ────────────────────────────────────────────────────────────

    #[test]
    fn bkv_single_role_commands_infer_role() {
        // einleiten and daten-einreichen are single-role (BKV only).
        assert_eq!(
            validate_command("mabis.abrechnung.einleiten", None, &roles(&["BKV"])).unwrap(),
            "BKV"
        );
        assert_eq!(
            validate_command("mabis.abrechnung.daten-einreichen", None, &roles(&["BKV"])).unwrap(),
            "BKV"
        );
    }

    #[test]
    fn begleichen_is_multi_role_bkv_and_uenb() {
        // begleichen permits BKV and ÜNB → marktrolle required.
        assert!(
            validate_command("mabis.abrechnung.begleichen", Some("BKV"), &roles(&["BKV"])).is_ok()
        );
        assert!(
            validate_command("mabis.abrechnung.begleichen", Some("ÜNB"), &roles(&["ÜNB"])).is_ok()
        );
        assert!(validate_command("mabis.abrechnung.begleichen", None, &roles(&["BKV"])).is_err());
    }

    #[test]
    fn uenb_instance_may_not_initiate_mabis() {
        // einleiten resolves to BKV; ÜNB instance not configured for BKV.
        let uenb = roles(&["ÜNB"]);
        assert!(validate_command("mabis.abrechnung.einleiten", None, &uenb).is_err());
    }

    // ── Unknown command ───────────────────────────────────────────────────────

    #[test]
    fn unknown_command_is_rejected() {
        assert!(validate_command("gpke.nonexistent.action", None, &roles(&["LF"])).is_err());
    }

    // ── MABIS billing dispatch ──────────────────────────────────────
    //
    // These tests exercise the full `dispatch_command` path for the three MABIS
    // billing commands, which must not fall through to the `NotImplemented`
    // catch-all.

    /// Build a minimal `CommandsApiState` backed by an in-memory store.
    ///
    /// Uses the same wiring as `main.rs` but with `SlateDbStore::open_in_memory()`
    /// so no filesystem setup is required.  Cedar keys are omitted — dispatch
    /// tests call `dispatch_command` directly, bypassing the HTTP auth layer.
    async fn mabis_dispatch_state(marktrollen: &[&str]) -> CommandsApiState {
        let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("open in-memory SlateDB");
        let cedar = Arc::new(
            CedarAuthorizer::new(
                vec![],
                None,
                None,
                None,
                crate::cedar_authz::DefaultPolicy::PermitAll,
            )
            .expect("cedar build"),
        );
        CommandsApiState {
            tenant_id: TenantId::from_party_id("4033872000022"),
            sender_party_id: "4033872000022".to_owned(),
            configured_marktrollen: marktrollen.iter().map(|s| s.to_uppercase()).collect(),
            max_body_bytes: 1_048_576,
            snapshot_interval: 100,
            cedar,
            snapshot_store: store.as_snapshot_store(),
            malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
            maloid_result_cache: MaloIdentResultCache::new(store.clone()),
            store: Arc::new(store),
            marktd_client: None,
        }
    }

    /// The Bilanzierungsmonat that is open for filing today, so the tests do
    /// not go stale as the calendar moves.
    ///
    /// A settlement accepts versions only inside its Erstaufschlag or clearing
    /// window (Kap. 3.10 Tabelle 2), so a hard-coded month would start failing
    /// the moment its KBKA closed. Two months back is still open for every
    /// Summenzeitreihe in the table — the KBKA runs to the end of the 7. Monat —
    /// whichever phase of it the day happens to fall in.
    fn offener_bilanzierungsmonat() -> String {
        let heute = mako_fristen::heute();
        let mut d = heute.replace_day(1).expect("day 1 exists");
        for _ in 0..2 {
            d = d
                .checked_sub(time::Duration::days(1))
                .expect("date underflow")
                .replace_day(1)
                .expect("day 1 exists");
        }
        format!("{}-{:02}", d.year(), u8::from(d.month()))
    }

    fn mabis_einleiten_payload(version: &str) -> serde_json::Value {
        serde_json::json!({
            // SG10 CAV Z99 — BK-SZR (Kategorie B) auf Ebene Bilanzierungsgebiet.
            "zeitreihe":          "Z99",
            "mabis_zp_id":        "DE0001111222233334444555566667777",
            "bilanzierungsmonat": offener_bilanzierungsmonat(),
            "version":            version,
            "biko_id":            "10YDE-VE-TRANSMIX",
            "absender_mp_id":     "9900357000004",
        })
    }

    /// `mabis.abrechnung.einleiten` must spawn a new settlement.
    #[tokio::test]
    async fn dispatch_mabis_einleiten_spawns_process() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        let outcome = dispatch_command(
            &state,
            "mabis.abrechnung.einleiten",
            &mabis_einleiten_payload("20260101120001+00"),
        )
        .await
        .expect("einleiten must succeed");
        assert!(
            matches!(outcome, DispatchOutcome::Spawned { .. }),
            "first einleiten must return Spawned; got: {outcome:?}"
        );
    }

    /// A settlement **accumulates** versions, so a second `einleiten` with a
    /// higher version resumes the existing process rather than being refused.
    ///
    /// That is the whole point of the Clearingphase: more versions arrive.
    #[tokio::test]
    async fn dispatch_mabis_einleiten_accumulates_versions() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        dispatch_command(
            &state,
            "mabis.abrechnung.einleiten",
            &mabis_einleiten_payload("20260101120001+00"),
        )
        .await
        .expect("first einleiten");
        let second = dispatch_command(
            &state,
            "mabis.abrechnung.einleiten",
            &mabis_einleiten_payload("20260101120002+00"),
        )
        .await
        .expect("a higher version must be accepted");
        assert!(
            matches!(second, DispatchOutcome::Dispatched { .. }),
            "a second version must resume the settlement; got: {second:?}"
        );
    }

    /// Kap. 3.8.2 — versions ascend, so a repeated one is refused.
    #[tokio::test]
    async fn dispatch_mabis_einleiten_refuses_a_repeated_version() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        let payload = mabis_einleiten_payload("20260101120001+00");
        dispatch_command(&state, "mabis.abrechnung.einleiten", &payload)
            .await
            .expect("first einleiten");
        assert!(
            dispatch_command(&state, "mabis.abrechnung.einleiten", &payload)
                .await
                .is_err(),
            "the same version twice is a filing error, not a redelivery"
        );
    }

    /// An unknown `zeitreihe` code is refused rather than defaulted: eleven
    /// Summenzeitreihen share PID 13003's activation codes and five of them owe
    /// no answer, so a wrong guess invents or drops an obligation.
    #[tokio::test]
    async fn dispatch_mabis_einleiten_refuses_an_unknown_zeitreihe() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        let mut payload = mabis_einleiten_payload("20260101120001+00");
        payload["zeitreihe"] = serde_json::json!("ZZZ");
        assert!(
            dispatch_command(&state, "mabis.abrechnung.einleiten", &payload)
                .await
                .is_err()
        );
    }

    /// A version that is not an Erstellungszeitpunkt is refused: it is the key
    /// the BIKO matches on, and a repaired one no longer matches.
    #[tokio::test]
    async fn dispatch_mabis_einleiten_refuses_a_non_timestamp_version() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        for bad in ["1", "vorlaeufig", "2026010112000100"] {
            let payload = mabis_einleiten_payload(bad);
            assert!(
                dispatch_command(&state, "mabis.abrechnung.einleiten", &payload)
                    .await
                    .is_err(),
                "version {bad:?} must be refused"
            );
        }
    }

    /// `mabis.abrechnung.daten-einreichen` must route to the settlement.
    #[tokio::test]
    async fn dispatch_mabis_daten_einreichen_routes_to_process() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        dispatch_command(
            &state,
            "mabis.abrechnung.einleiten",
            &mabis_einleiten_payload("20260101120001+00"),
        )
        .await
        .expect("einleiten");
        let outcome = dispatch_command(
            &state,
            "mabis.abrechnung.daten-einreichen",
            &serde_json::json!({
                "mabis_zp_id":        "DE0001111222233334444555566667777",
                "bilanzierungsmonat": offener_bilanzierungsmonat(),
                "version":            "20260101120001+00",
                "pid":                21005,
                // BK-SZR (Kategorie B) is decided by `E_0064`, whose
                // Zustimmung is `A03`. The code is named, not derived from a
                // boolean — see `Zeitreihe::pruef_ebd`.
                "antwortcode":        "A03",
            }),
        )
        .await
        .expect("daten-einreichen must succeed");
        assert!(
            matches!(outcome, DispatchOutcome::Dispatched { .. }),
            "daten-einreichen must return Dispatched; got: {outcome:?}"
        );
    }

    /// A Prüfmitteilung may only ride an outbound PID (21000/21001/21005).
    #[tokio::test]
    async fn dispatch_mabis_daten_einreichen_refuses_an_inbound_pid() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        dispatch_command(
            &state,
            "mabis.abrechnung.einleiten",
            &mabis_einleiten_payload("20260101120001+00"),
        )
        .await
        .expect("einleiten");
        for pid in [21002, 21003, 21004] {
            assert!(
                dispatch_command(
                    &state,
                    "mabis.abrechnung.daten-einreichen",
                    &serde_json::json!({
                        "mabis_zp_id":        "DE0001111222233334444555566667777",
                        "bilanzierungsmonat": offener_bilanzierungsmonat(),
                        "version":            "20260101120001+00",
                        "pid":                pid,
                    }),
                )
                .await
                .is_err(),
                "PID {pid} is inbound"
            );
        }
    }

    /// `mabis.abrechnung.begleichen` closes the clearing window.
    ///
    /// It does **not** set a Datenstatus: that is assigned exclusively by the
    /// BIKO (Kap. 3.8.3) and arrives as IFTSTA 21003/21004.
    #[tokio::test]
    async fn dispatch_mabis_begleichen_routes_to_process() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        dispatch_command(
            &state,
            "mabis.abrechnung.einleiten",
            &mabis_einleiten_payload("20260101120001+00"),
        )
        .await
        .expect("einleiten");
        let outcome = dispatch_command(
            &state,
            "mabis.abrechnung.begleichen",
            &serde_json::json!({
                "mabis_zp_id":        "DE0001111222233334444555566667777",
                "bilanzierungsmonat": offener_bilanzierungsmonat(),
                "lauf":               "bka",
            }),
        )
        .await
        .expect("begleichen must succeed");
        assert!(
            matches!(outcome, DispatchOutcome::Dispatched { .. }),
            "begleichen must return Dispatched; got: {outcome:?}"
        );
    }

    /// `netzzugang.energysharing.registrieren` (§20b Abs. 2 Nr. 3) must
    /// validate the payload, enqueue a `NetzzugangAntrag` outbox message and
    /// return `Spawned` — the sender then delivers it with outbox semantics.
    #[tokio::test]
    async fn dispatch_netzzugang_registrierung_enqueues_outbox_antrag() {
        use mako_engine::outbox::OutboxStore as _;
        let state = mabis_dispatch_state(&["LF"]).await;

        // Missing required fields → InvalidPayload.
        let err = dispatch_command(
            &state,
            "netzzugang.energysharing.registrieren",
            &serde_json::json!({ "netzanschluss_id": "NA-1" }),
        )
        .await
        .expect_err("incomplete payload must be rejected");
        assert!(matches!(err, DispatchError::InvalidPayload(_)), "{err:?}");

        // Complete payload → Spawned + one outbox message with the antrag.
        let outcome = dispatch_command(
            &state,
            "netzzugang.energysharing.registrieren",
            &serde_json::json!({
                "netzanschluss_id": "NA-1",
                "nb_mp_id": "9900001000001",
                "antragsteller_ref": "kunde-42",
                "details": { "vereinbarung_ref": "ES-2026-001" },
            }),
        )
        .await
        .expect("registrieren must succeed");
        assert!(
            matches!(outcome, DispatchOutcome::Spawned { .. }),
            "{outcome:?}"
        );

        let pending = state
            .store
            .pending(
                10,
                time::OffsetDateTime::now_utc() + time::Duration::minutes(1),
            )
            .await
            .expect("outbox pending");
        let msg = pending
            .iter()
            .find(|m| m.message_type.as_ref() == crate::netzzugang::NETZZUGANG_MESSAGE_TYPE)
            .expect("NetzzugangAntrag outbox message enqueued");
        let antrag: NetzzugangAntrag =
            serde_json::from_value(msg.payload.clone()).expect("payload is a NetzzugangAntrag");
        assert_eq!(
            antrag.antrag_typ,
            NetzzugangAntragTyp::EnergySharingVereinbarung
        );
        assert_eq!(antrag.aktion, NetzzugangAktion::Registrierung);
        assert_eq!(antrag.status, NetzzugangStatus::Erfasst);
        assert_eq!(msg.recipient.as_ref(), "9900001000001");
    }

    /// The two Bestellung use cases (§20b Abs. 2 Nr. 1/2) require a valid
    /// `aktion` from the statutory triple.
    #[tokio::test]
    async fn dispatch_netzzugang_bestellung_validates_the_aktion() {
        let state = mabis_dispatch_state(&["MSB"]).await;
        let base = serde_json::json!({
            "netzanschluss_id": "NA-2",
            "nb_mp_id": "9900001000001",
            "antragsteller_ref": "kunde-7",
        });

        let mut bad = base.clone();
        bad["aktion"] = "stornierung".into();
        let err = dispatch_command(&state, "netzzugang.zaehlpunktanordnung.beauftragen", &bad)
            .await
            .expect_err("invalid aktion must be rejected");
        assert!(matches!(err, DispatchError::InvalidPayload(_)), "{err:?}");

        let mut ok = base;
        ok["aktion"] = "aenderung".into();
        let outcome = dispatch_command(&state, "netzzugang.verrechnungskonzept.beauftragen", &ok)
            .await
            .expect("aenderung must succeed");
        assert!(
            matches!(outcome, DispatchOutcome::Spawned { .. }),
            "{outcome:?}"
        );
    }

    /// `mabis.abrechnung.einleiten` with a missing required field must return
    /// `DispatchError::InvalidPayload`, not `NotImplemented`.
    #[tokio::test]
    async fn dispatch_mabis_einleiten_missing_field_returns_invalid_payload() {
        let state = mabis_dispatch_state(&["BKV"]).await;
        let payload = serde_json::json!({
            // "billing_period" intentionally omitted
            "bkv_id":  "4033872000022",
            "biko_id": "10YDE-VE-TRANSMIX",
            "version": "vorlaeufig",
        });
        let err = dispatch_command(&state, "mabis.abrechnung.einleiten", &payload)
            .await
            .expect_err("missing billing_period must return Err");
        assert!(
            matches!(err, DispatchError::InvalidPayload(_)),
            "must be InvalidPayload; got: {err:?}"
        );
    }
}
