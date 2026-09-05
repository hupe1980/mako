//! Command-API data types: state, envelope, dispatch outcome/error, descriptor.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

/// Shared state for the ERP commands API.
#[derive(Clone)]
pub struct CommandsApiState {
    /// Operator tenant identifier (our MP-ID as a TenantId).
    pub tenant_id: TenantId,
    /// Our operator MP-ID as a string (from `--tenant-id`).
    pub sender_party_id: String,
    /// Maximum request body size in bytes.
    ///
    /// Applied via [`DefaultBodyLimit`] to prevent unbounded heap allocation
    /// on malicious or oversized ERP payloads.
    pub max_body_bytes: usize,
    /// Marktrollen this instance is licensed and configured for (upper-case).
    ///
    /// Marktrollen this instance is authorised to submit commands for.
    ///
    /// Must be non-empty; the engine rejects commands for any role not in this
    /// list with `422 role_not_configured`.  An empty list means no roles are
    /// configured and every command is rejected.
    ///
    /// Set via `--marktrollen` at startup (e.g. `LF,LFG` for a dual-fuel supplier).
    pub configured_marktrollen: Vec<String>,
    /// Cedar-based authorization engine.
    ///
    /// Resolves bearer tokens to named principals and evaluates Cedar ABAC
    /// policies for each command submission.
    pub cedar: Arc<CedarAuthorizer>,
    /// Shared SlateDB store — used for process dispatch and deadline registration.
    pub store: Arc<mako_engine::store_slatedb::SlateDbStore>,
    /// Snapshot store — shares the same underlying DB as `store`.
    ///
    /// Passed to `execute_and_enqueue_with_snapshot_and_retry` so replay cost
    /// for long-lived processes is bounded to at most 100 tail events instead
    /// of a full O(n) event scan on every command dispatch.
    pub snapshot_store: mako_engine::store_slatedb::SlateDbSnapshotStore,
    /// MaLo master-data cache — used to resolve NB/MSB MP-IDs from MaLo IDs.
    pub malo_cache: Arc<SlateDbMaloCache>,
    /// MaLo-ID result cache — maps `tx_id → (malo_id, nb_mp_id)` for the
    /// `maloid.lieferbeginn.fortsetzen` continuation command.
    ///
    /// Written by `MaloIdentSender` after a positive callback is delivered.
    /// The ERP receives `MaloIdentified` via the ERP webhook and then calls
    /// `maloid.lieferbeginn.fortsetzen` with only the `tx_id` and
    /// `lieferbeginn_datum` — `makod` resolves the `malo_id` and `nb_mp_id`
    /// from this cache.
    pub maloid_result_cache: MaloIdentResultCache,
    /// Number of events between automatic process snapshots.
    ///
    /// Defaults to 100. Configurable via `--snapshot-interval` at startup
    /// so operators can tune replay latency vs. write amplification without
    /// recompiling.
    pub snapshot_interval: u64,
    /// Optional `marktd` client for M1 Konfigurationsprodukt guard.
    ///
    /// When set, `wim.steuerungsauftrag.bestaetigen` checks that the contracted
    /// `produkt_code` (from the original ORDERS) is in the SR's
    /// `konfigurationsprodukte` list before dispatching the positive ORDRSP.
    /// When `None`, the guard is disabled (dev mode / deployments without marktd).
    pub marktd_client: Option<std::sync::Arc<mako_markt::marktd_client::MarktdClient>>,
}

// ── Command envelope ──────────────────────────────────────────────────────────

/// Inbound ERP command envelope.
///
/// The ERP explicitly names the MaKo process command it wants to trigger and
/// its own Marktrolle.  The BO4E object(s) carrying domain data are placed in
/// `payload` — they are never used for routing.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ErpCommand {
    /// Dotted command name: `<domain>.<prozess>.<aktion>`.
    ///
    /// Examples: `gpke.lieferbeginn.anmelden`, `wim.geraetewechsel.beauftragen`
    #[schema(example = "gpke.lieferbeginn.anmelden")]
    pub command: String,

    /// Marktrolle of the ERP system issuing this command.
    ///
    /// **Required only for multi-role commands** such as
    /// `wim.geraetewechsel.beauftragen` (permitted for both `NB` and `MSB`).
    /// For single-role commands (e.g. `gpke.lieferbeginn.anmelden` → always `LF`)
    /// this field is optional — the role is inferred from the command name.
    ///
    /// Examples: `"NB"`, `"LF"`, `"LFG"`, `"GNB"`, `"MSB"`, `"BKV"`, `"ÜNB"`
    #[schema(example = "LF")]
    pub marktrolle: Option<String>,

    /// Command-specific payload.
    ///
    /// Each command defines its own required fields.  For most GPKE/GeLi
    /// commands the minimal payload is a `malo_id` string (the 11-digit
    /// Marktlokations-ID) plus the process-specific date.  The engine
    /// resolves all partner MP-IDs (NB, MSB) and MeLo data from the MaLo cache.
    ///
    /// Example for `gpke.lieferbeginn.anmelden`:
    /// ```json
    /// {
    ///   "malo_id":            "10001234558",
    ///   "lieferbeginn_datum": "2026-10-01"
    /// }
    /// ```
    ///
    /// Billing commands embed a BO4E `RECHNUNG` object because the invoice is
    /// the master data itself (no separate cache lookup needed).
    ///
    /// **Never include** `sender_party_id`, `receiver_party_id`, `pruefidentifikator`, or
    /// `message_ref` — these are engine-owned and will be ignored or rejected.
    #[schema(value_type = Object, example = json!({"malo_id": "10001234558", "lieferbeginn_datum": "2026-10-01"}))]
    pub payload: serde_json::Value,
}

/// Successful command acceptance response.
#[derive(Serialize, ToSchema)]
pub struct CommandAccepted {
    /// Idempotency key echoed back to the caller.
    #[schema(example = "01HZX1234567890ABCDEFGHIJK")]
    pub idempotency_key: String,
    /// The command name as received.
    #[schema(example = "gpke.lieferbeginn.anmelden")]
    pub command: String,
    /// The Marktrolle as received.
    #[schema(example = "LF")]
    pub marktrolle: String,
    /// Always `"accepted"` for HTTP 202.
    #[schema(value_type = String, example = "accepted")]
    pub status: &'static str,
    /// UUID of the process that was spawned or updated.
    ///
    /// Matches the `subject` field of the corresponding CloudEvent sent to the
    /// ERP webhook. Use this to correlate the command response with the
    /// `de.mako.process.initiated` (or other) event the ERP received.
    #[schema(example = "3181967a-02d1-4d0e-9105-0cc46f3b25c9")]
    pub process_id: String,
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DispatchOutcome {
    Spawned {
        process_id: ProcessId,
    },
    /// Command routed to an existing process (response / confirmation).
    Dispatched {
        process_id: ProcessId,
    },
}

#[derive(Debug)]
pub enum DispatchError {
    /// Required `malo_id` not in the cache — ERP must seed it first.
    MaloNotFound(String),
    /// Payload is missing a required field or carries a malformed value.
    InvalidPayload(String),
    /// Engine error (version conflict, storage failure, …).
    Engine(mako_engine::error::EngineError),
    /// No active process found for the given business key.
    ///
    /// For LF-side commands this means the Anmeldung has not been initiated
    /// yet, or the process has already reached a terminal state.
    ProcessNotFound {
        business_key: String,
        workflow_name: &'static str,
    },
    /// More than one active process exists for the given business key.
    ///
    /// This should not occur under normal operation — it indicates a bug in
    /// the process-initiation path that registered the same MaLo twice.
    AmbiguousProcess { business_key: String, count: usize },
    /// An active process for the given MaLo already exists.
    ///
    /// ERP must not re-initiate a process that is still in progress. Use
    /// the existing `process_id` to route follow-up commands (bestaetigen,
    /// ablehnen, aktivieren). Retry the `anmelden` command only after the
    /// existing process has reached a terminal state.
    ///
    /// Note: a narrow TOCTOU window exists if two concurrent calls arrive
    /// simultaneously. The check-then-spawn sequence is not atomic. In practice
    /// ERP serialises requests per MaLo so the window is negligible.
    DuplicateProcess {
        process_id: ProcessId,
        malo_id: String,
    },
    /// Command is known but the full workflow routing is not yet implemented.
    NotImplemented(String),
}

impl From<mako_engine::error::EngineError> for DispatchError {
    fn from(e: mako_engine::error::EngineError) -> Self {
        Self::Engine(e)
    }
}

// ── CommandDescriptor — compile-time dispatch linking ─────────────────────────

/// Type alias for a statically-linked async dispatch function.
///
/// Using a concrete function pointer (not a `Box<dyn Fn>`) means every entry
/// in `COMMAND_REGISTRY` must supply a real named function at compile time.
/// This closes the registry-dispatch gap: adding a command to the registry
/// without wiring a handler is a compile error, not a runtime 501.
pub(crate) type DispatchFn = for<'a> fn(
    &'a CommandsApiState,
    &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
>;

/// A single command descriptor: name, permitted roles, primary PID, and dispatch fn.
///
/// All three data points live together so adding a new command requires
/// filling in all fields — no parallel data structures can silently drift apart.
pub(crate) struct CommandDescriptor {
    /// Stable lowercase command name, e.g. `"gpke.lieferbeginn.anmelden"`.
    pub name: &'static str,
    /// Marktrollen permitted to call this command.
    pub permitted_roles: &'static [Marktrolle],
    /// Primary Prüfidentifikator associated with this command.
    /// `None` for commands that carry no single outbound PID
    /// (e.g. REST-only replay sinks or multi-PID ORDERS flows).
    pub primary_pid: Option<Pruefidentifikator>,
    /// Async dispatch function — called by [`dispatch_command`] after role validation.
    pub dispatch: DispatchFn,
}

/// Shorthand for a typed registry `primary_pid` entry.
///
/// Const-panics at compile time when `code` is outside the valid PID range.
pub(super) const fn pid(code: u32) -> Option<Pruefidentifikator> {
    Some(Pruefidentifikator::const_new(code))
}

/// Errors from [`validate_command`].
#[derive(Debug)]
pub enum CommandError {
    /// Command name is not in the registry.
    UnknownCommand,
    /// Multi-role command but no `marktrolle` was supplied.
    MarktrolleRequired,
    /// Asserted `marktrolle` is not in the command's permitted set.
    RoleNotPermitted,
    /// The effective role is not in [`CommandsApiState::configured_marktrollen`].
    RoleNotConfigured,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand => f.write_str("unknown_command: command name not in registry"),
            Self::MarktrolleRequired => {
                f.write_str("marktrolle_required: multi-role command requires a marktrolle field")
            }
            Self::RoleNotPermitted => {
                f.write_str("role_not_permitted: asserted marktrolle is not valid for this command")
            }
            Self::RoleNotConfigured => f.write_str(
                "role_not_configured: this instance is not configured for that marktrolle",
            ),
        }
    }
}

#[cfg(test)]
mod family_tests {
    use super::COMMAND_REGISTRY;
    use crate::orchestrator::process_family::{UNKNOWN, from_command as command_family};

    /// Every registered command must map to a named family.
    ///
    /// The `family` label was hard-coded to `"gpke"` at the only call site, so
    /// the per-family process counter attributed WiM, GeLi Gas, GaBi Gas, MaBiS
    /// and ESA initiations to GPKE and reported zero for each of them. A new
    /// command prefix that falls through to `"other"` would quietly restore a
    /// weaker version of the same defect.
    #[test]
    fn every_command_maps_to_a_known_family() {
        let unmapped: Vec<&str> = COMMAND_REGISTRY
            .iter()
            .map(|d| d.name)
            .filter(|name| command_family(name) == UNKNOWN)
            .collect();
        assert!(
            unmapped.is_empty(),
            "these commands have no metric family: {unmapped:?}\n\
             Add the prefix to a row of `orchestrator::process_family::FAMILIES`, \
             or the full command name to COMMAND_OVERRIDES when it enters a \
             workflow of another family."
        );
    }

    /// The initiation counter has exactly one call site.
    ///
    /// There are two command doors — `POST /api/v1/commands` and the MCP
    /// `submit_command` tool — so a counter in either handler leaves the
    /// processes the other starts uncounted, and the initiated-versus-completed
    /// dashboard shows completions with no matching initiations. It sits inside
    /// `dispatch_command`, which both doors go through; a second call site
    /// anywhere would either double-count or split it again.
    #[test]
    fn the_initiation_counter_is_emitted_only_from_dispatch_command() {
        const DOORS: &[(&str, &str)] = &[
            ("handler.rs", include_str!("handler.rs")),
            ("mcp_server.rs", include_str!("../../api/mcp_server.rs")),
        ];
        for (file, src) in DOORS {
            let calls = src.matches(".process_initiated(").count();
            let expected = usize::from(*file == "handler.rs");
            assert_eq!(
                calls, expected,
                "{file} has {calls} process_initiated call(s), expected {expected}. \
                 The counter belongs in dispatch_command so both command doors \
                 are covered by one emission."
            );
        }
        assert!(
            include_str!("handler.rs").contains("pub async fn dispatch_command"),
            "the single call site must be dispatch_command in handler.rs"
        );
    }

    /// The Gas families use the same hyphenated label as the workflow-name
    /// prefix the deadline and outbox metrics carry, so a dashboard can join
    /// initiation and completion on one label value.
    ///
    /// That the two *sides* agree is guarded in
    /// [`crate::orchestrator::process_family`], which owns both derivations;
    /// this only pins the spelling the registry's commands produce.
    #[test]
    fn gas_families_match_the_workflow_name_prefix() {
        assert_eq!(command_family("geli.lieferbeginn.anmelden"), "geli-gas");
        assert_eq!(command_family("gabi.invoic.senden"), "gabi-gas");
    }
}
