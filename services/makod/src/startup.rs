//! Startup helpers extracted from `async_main`.
//!
//! Each function in this module covers a distinct startup phase, enabling
//! per-phase unit tests with `InMemoryEventStore` without starting the
//! full daemon.
//!
//! ## Type alias
//!
//! `MakodCtx` names the concrete `EngineContext` type used throughout the
//! production daemon.  Tests that need an engine context can build one with
//! `EngineBuilder::with_stores(...)` and store it as `MakodCtx`.
//!
//! ## Phases
//!
//! | Function | What it does |
//! |---|---|
//! | `validate_adapter_coverage` | Panics if any workflow lacks a `MessageAdapter` for an active FV |
//! | `spawn_workers` | Spawns outbox, ERP-webhook, deadline-scheduler, projection, and inbox-purge workers |
//!
//! [`EngineContext`]: mako_engine::builder::EngineContext

use std::sync::Arc;
use std::time::Duration;

use edi_energy::Platform;
use mako_engine::{
    builder::EngineContext,
    store_slatedb::{
        SlateDbDeadlineStore, SlateDbInboxStore, SlateDbProcessRegistry, SlateDbSnapshotStore,
        SlateDbStore,
    },
    version::WorkflowVersionPolicy,
};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    adapters, deadline_dispatch, erp_adapter, ingest_dispatcher, malo_cache,
    party_registry::MpIdRegistry,
};

// ── Domain workflow name imports ──────────────────────────────────────────────
// Import WORKFLOW_NAME constants rather than using inline string literals.
// This makes typos a compile error instead of a silent dispatch gap.

use mako_gabi_gas::{
    ALLOCATION_WORKFLOW_NAME, INVOIC_COMDIS_RESUME_PATH, INVOIC_REMADV_RESUME_PATH,
    INVOIC_WORKFLOW_NAME, NOMINATION_WORKFLOW_NAME,
};
use mako_geli_gas::{
    GAS_MSCONS_WORKFLOW_NAME, GELI_GAS_PARTIN_WORKFLOW_NAME,
    GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME, GELI_GAS_SPERRUNG_LF_WORKFLOW_NAME,
    GELI_GAS_SPERRUNG_NB_WORKFLOW_NAME,
    STORNIERUNG_LF_WORKFLOW_NAME as GELI_GAS_STORNIERUNG_LF_WORKFLOW_NAME,
    STORNIERUNG_WORKFLOW_NAME as GELI_GAS_STORNIERUNG_WORKFLOW_NAME,
    WORKFLOW_NAME as GELI_GAS_SUPPLIER_CHANGE_WORKFLOW_NAME,
};
use mako_gpke::{
    ABRECHNUNG_WORKFLOW_NAME, ALLOKATIONSLISTE_WORKFLOW_NAME, ANFRAGE_BESTELLUNG_WORKFLOW_NAME,
    ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW_NAME, DATENABRUF_WORKFLOW_NAME,
    KONFIGURATION_AENDERUNG_WORKFLOW_NAME, KONFIGURATION_WORKFLOW_NAME, LF_ABMELDUNG_WORKFLOW_NAME,
    LF_ANMELDUNG_WORKFLOW_NAME, MESSWERTE_WORKFLOW_NAME, NEUANLAGE_WORKFLOW_NAME,
    PARTIN_WORKFLOW_NAME, SPERRUNG_LF_WORKFLOW_NAME, SPERRUNG_WORKFLOW_NAME,
    STORNIERUNG_GPKE_WORKFLOW_NAME, SUPPLIER_CHANGE_WORKFLOW_NAME, UTILTS_WORKFLOW_NAME,
};
use mako_mabis::{BILLING_WORKFLOW_NAME, CLEARINGLISTE_WORKFLOW_NAME};
use mako_wim::{
    GERAETEUBERNAHME_WORKFLOW_NAME, INSRPT_WORKFLOW_NAME, PREISANFRAGE_WORKFLOW_NAME,
    PREISLISTE_WORKFLOW_NAME, RECHNUNG_WORKFLOW_NAME, STAMMDATEN_WORKFLOW_NAME,
    STORNIERUNG_WORKFLOW_NAME as WIM_STORNIERUNG_WORKFLOW_NAME,
    WORKFLOW_NAME as WIM_DEVICE_CHANGE_WORKFLOW_NAME,
};
use mako_wim_gas::{
    ANMELDUNG_WORKFLOW_NAME as WIM_GAS_ANMELDUNG_WORKFLOW_NAME,
    INSRPT_GAS_WORKFLOW_NAME as WIM_GAS_INSRPT_WORKFLOW_NAME,
    INVOIC_WORKFLOW_NAME as WIM_GAS_INVOIC_WORKFLOW_NAME,
    KUENDIGUNG_WORKFLOW_NAME as WIM_GAS_KUENDIGUNG_WORKFLOW_NAME,
    STORNIERUNG_WORKFLOW_NAME as WIM_GAS_STORNIERUNG_WORKFLOW_NAME,
    VERPFLICHTUNGSANFRAGE_WORKFLOW_NAME as WIM_GAS_VERPFLICHTUNGSANFRAGE_WORKFLOW_NAME,
};

// ── Type aliases ──────────────────────────────────────────────────────────────

/// Concrete [`EngineContext`] type used by the `makod` production daemon.
///
/// Type alias for the five-parameter generic with the SlateDB-backed store
/// types.  Useful in tests and startup helpers to avoid repeating all five
/// type parameters.
pub(crate) type MakodCtx = EngineContext<
    SlateDbStore,
    SlateDbSnapshotStore,
    SlateDbStore,
    SlateDbDeadlineStore,
    SlateDbProcessRegistry,
>;

// ── validate_adapter_coverage ─────────────────────────────────────────────────

/// Validate that every domain workflow has a registered [`MessageAdapter`] for
/// every active BDEW format version.
///
/// Called once during startup before any worker is spawned.
///
/// # Panics
///
/// Panics when a workflow has no adapter registered for one or more active
/// format versions.  This is a hard fail-fast: a missing adapter means
/// inbound messages would be silently dead-lettered rather than dispatched.
///
/// # Tests
///
/// Because this function uses only static adapter registries (no I/O, no
/// store access), it can be called in unit tests:
///
/// ```rust,ignore
/// #[test]
/// fn all_workflows_have_adapter_coverage() {
///     // Panics if coverage is missing — the test itself serves as the assertion.
///     startup::validate_adapter_coverage();
/// }
/// ```
pub(crate) fn validate_adapter_coverage() {
    let known = adapters::known_fvs();
    let fv_names: Vec<&str> = known.iter().map(|fv| fv.as_str()).collect();
    let fc = &WorkflowVersionPolicy::ForwardCompatible;

    let checks: &[(&str, _)] = &[
        (
            SUPPLIER_CHANGE_WORKFLOW_NAME,
            adapters::gpke_registry().validate_policy(fc, &known),
        ),
        (
            LF_ANMELDUNG_WORKFLOW_NAME,
            adapters::gpke_lf_anmeldung_registry().validate_policy(fc, &known),
        ),
        (
            NEUANLAGE_WORKFLOW_NAME,
            adapters::gpke_neuanlage_registry().validate_policy(fc, &known),
        ),
        (
            LF_ABMELDUNG_WORKFLOW_NAME,
            adapters::gpke_lf_abmeldung_registry().validate_policy(fc, &known),
        ),
        (
            ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW_NAME,
            adapters::gpke_ankuendigung_zuordnung_lf_registry().validate_policy(fc, &known),
        ),
        (
            SPERRUNG_WORKFLOW_NAME,
            adapters::gpke_sperrung_registry().validate_policy(fc, &known),
        ),
        (
            STORNIERUNG_GPKE_WORKFLOW_NAME,
            adapters::gpke_stornierung_registry().validate_policy(fc, &known),
        ),
        (
            ANFRAGE_BESTELLUNG_WORKFLOW_NAME,
            adapters::gpke_anfrage_bestellung_registry().validate_policy(fc, &known),
        ),
        (
            ABRECHNUNG_WORKFLOW_NAME,
            adapters::gpke_abrechnung_registry().validate_policy(fc, &known),
        ),
        (
            KONFIGURATION_WORKFLOW_NAME,
            adapters::gpke_konfiguration_registry().validate_policy(fc, &known),
        ),
        (
            WIM_DEVICE_CHANGE_WORKFLOW_NAME,
            adapters::wim_registry().validate_policy(fc, &known),
        ),
        (
            GERAETEUBERNAHME_WORKFLOW_NAME,
            adapters::wim_geraeteubernahme_registry().validate_policy(fc, &known),
        ),
        (
            STAMMDATEN_WORKFLOW_NAME,
            adapters::wim_stammdaten_registry().validate_policy(fc, &known),
        ),
        (
            WIM_STORNIERUNG_WORKFLOW_NAME,
            adapters::wim_stornierung_registry().validate_policy(fc, &known),
        ),
        (
            RECHNUNG_WORKFLOW_NAME,
            adapters::wim_rechnung_registry().validate_policy(fc, &known),
        ),
        (
            INSRPT_WORKFLOW_NAME,
            adapters::wim_insrpt_registry().validate_policy(fc, &known),
        ),
        (
            GELI_GAS_SUPPLIER_CHANGE_WORKFLOW_NAME,
            adapters::geli_gas_registry().validate_policy(fc, &known),
        ),
        (
            GELI_GAS_STORNIERUNG_WORKFLOW_NAME,
            adapters::geli_gas_stornierung_registry().validate_policy(fc, &known),
        ),
        (
            WIM_GAS_ANMELDUNG_WORKFLOW_NAME,
            adapters::wim_gas_anmeldung_registry().validate_policy(fc, &known),
        ),
        (
            WIM_GAS_KUENDIGUNG_WORKFLOW_NAME,
            adapters::wim_gas_kuendigung_registry().validate_policy(fc, &known),
        ),
        (
            WIM_GAS_VERPFLICHTUNGSANFRAGE_WORKFLOW_NAME,
            adapters::wim_gas_verpflichtungsanfrage_registry().validate_policy(fc, &known),
        ),
        (
            WIM_GAS_INVOIC_WORKFLOW_NAME,
            adapters::wim_gas_invoic_registry().validate_policy(fc, &known),
        ),
        (
            WIM_GAS_INSRPT_WORKFLOW_NAME,
            adapters::wim_gas_insrpt_registry().validate_policy(fc, &known),
        ),
        (
            INVOIC_WORKFLOW_NAME,
            adapters::gabi_gas_invoic_registry().validate_policy(fc, &known),
        ),
        // gabi-gas-invoic resume adapters: REMADV 33001 (payment confirmation)
        // and COMDIS 29001 (payment rejection) — both format-version-sensitive.
        (
            INVOIC_REMADV_RESUME_PATH,
            adapters::gabi_gas_remadv_registry().validate_policy(fc, &known),
        ),
        (
            INVOIC_COMDIS_RESUME_PATH,
            adapters::gabi_gas_comdis_registry().validate_policy(fc, &known),
        ),
        // gabi-gas-nomination: DVGW NOMINT/NOMRES adapter (synthetic PIDs 90011/90012/90021/90022).
        (
            NOMINATION_WORKFLOW_NAME,
            adapters::gabi_gas_nomination_registry().validate_policy(fc, &known),
        ),
        // gabi-gas-allocation: DVGW ALOCAT adapter (synthetic PIDs 90001/90002/90003).
        (
            ALLOCATION_WORKFLOW_NAME,
            adapters::gabi_gas_allocation_registry().validate_policy(fc, &known),
        ),
        (
            GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
            adapters::geli_gas_sperrprozesse_invoic_registry().validate_policy(fc, &known),
        ),
        (
            GELI_GAS_SPERRUNG_NB_WORKFLOW_NAME,
            adapters::geli_gas_sperrung_nb_registry().validate_policy(fc, &known),
        ),
        // mabis-billing: IFTSTA adapter covers PIDs 21000–21007.
        // MSCONS PID 13003 billing commands are constructed by the aggregation
        // layer; this check validates IFTSTA coverage only.
        (
            BILLING_WORKFLOW_NAME,
            adapters::mabis_registry().validate_policy(fc, &known),
        ),
        // mabis-clearingliste: UTILMD adapter covers PIDs 55065, 55069, 55070.
        (
            CLEARINGLISTE_WORKFLOW_NAME,
            adapters::mabis_clearingliste_registry().validate_policy(fc, &known),
        ),
        // gpke-partin: PARTIN Kommunikationsdaten (PIDs 37000–37006).
        (
            PARTIN_WORKFLOW_NAME,
            adapters::gpke_partin_registry().validate_policy(fc, &known),
        ),
        // gpke-messwerte: MSCONS Messwertelieferung (PIDs 13xxx, Strom).
        (
            MESSWERTE_WORKFLOW_NAME,
            adapters::gpke_messwerte_registry().validate_policy(fc, &known),
        ),
        // gpke-utilts: UTILTS Konfigurationsdaten (GPKE Teil 3).
        (
            UTILTS_WORKFLOW_NAME,
            adapters::gpke_utilts_registry().validate_policy(fc, &known),
        ),
        // gpke-datenabruf: ORDRSP rejection inbound (GPKE Datenabruf ORDRSP).
        (
            DATENABRUF_WORKFLOW_NAME,
            adapters::gpke_datenabruf_registry().validate_policy(fc, &known),
        ),
        // gpke-konfiguration-aenderung: ORDRSP inbound acceptance/rejection.
        (
            KONFIGURATION_AENDERUNG_WORKFLOW_NAME,
            adapters::gpke_konfiguration_aenderung_registry().validate_policy(fc, &known),
        ),
        // gpke-allokationsliste: ORDRSP + MSCONS adapters (PIDs 55022–55024).
        (
            ALLOKATIONSLISTE_WORKFLOW_NAME,
            adapters::gpke_allokationsliste_ordrsp_registry().validate_policy(fc, &known),
        ),
        (
            ALLOKATIONSLISTE_WORKFLOW_NAME,
            adapters::gpke_allokationsliste_mscons_registry().validate_policy(fc, &known),
        ),
        // geli-gas-partin: PARTIN Gas Kommunikationsdaten (PIDs 37008–37014).
        (
            GELI_GAS_PARTIN_WORKFLOW_NAME,
            adapters::geli_gas_partin_registry().validate_policy(fc, &known),
        ),
        // wim-preisanfrage: REQOTE Preisanfrage (PIDs 35001–35005).
        (
            PREISANFRAGE_WORKFLOW_NAME,
            adapters::wim_preisanfrage_registry().validate_policy(fc, &known),
        ),
        // wim-preisliste: PRICAT Preisliste (PIDs 27001–27003).
        (
            PREISLISTE_WORKFLOW_NAME,
            adapters::wim_preisliste_registry().validate_policy(fc, &known),
        ),
        // wim-gas-stornierung: UTILMD G PID 44022 (Anfrage Stornierung, LF → GNB).
        (
            WIM_GAS_STORNIERUNG_WORKFLOW_NAME,
            adapters::wim_gas_stornierung_registry().validate_policy(fc, &known),
        ),
        // geli-gas-stornierung-lf: UTILMD G PIDs 44023/44024 (GNB response to LF).
        (
            GELI_GAS_STORNIERUNG_LF_WORKFLOW_NAME,
            adapters::geli_gas_stornierung_lf_registry().validate_policy(fc, &known),
        ),
        // geli-gas-sperrung-lf: ORDRSP Gas-Sperrung (GNB → LFG).
        (
            GELI_GAS_SPERRUNG_LF_WORKFLOW_NAME,
            adapters::geli_gas_sperrung_lf_registry().validate_policy(fc, &known),
        ),
        // gpke-sperrung-lf: ORDRSP/IFTSTA Sperrung-Antwort (NB → LF).
        (
            SPERRUNG_LF_WORKFLOW_NAME,
            adapters::gpke_sperrung_lf_registry().validate_policy(fc, &known),
        ),
        // geli-gas-mscons: MSCONS Gas Messdaten (GNB/gMSB → LFG).
        (
            GAS_MSCONS_WORKFLOW_NAME,
            adapters::geli_gas_mscons_registry().validate_policy(fc, &known),
        ),
    ];

    for (name, result) in checks {
        match result {
            Ok(()) => {
                info!(
                    workflow = name,
                    format_versions = ?fv_names,
                    "adapter coverage validated"
                );
            }
            Err(uncovered) => {
                let missing: Vec<&str> = uncovered.iter().map(|fv| fv.as_str()).collect();
                panic!(
                    "startup failure: workflow {name:?} has no registered MessageAdapter \
                     for format versions {missing:?}. Register adapters in adapters.rs."
                );
            }
        }
    }
}

// ── validate_dispatch_completeness ───────────────────────────────────────────

/// Validate that every workflow name reachable via `PidRouter` has a
/// corresponding dispatch arm in [`EdifactIngestDispatcher`].
///
/// This is a startup guard against the gap where a domain crate registers a
/// new PID → workflow mapping, but the developer forgets to add a matching
/// `match` arm in `ingest_dispatcher.rs`.  Without this check, inbound
/// messages for the new PID would be silently dead-lettered at runtime.
///
/// # How it works
///
/// 1. Enumerate all unique workflow names from `router` (both unambiguous and
///    commodity-qualified entries).
/// 2. Compare against [`EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES`] — the
///    compile-time list of workflow names that have a dispatch arm.
/// 3. Panic with an actionable message listing every undispatched workflow.
///
/// # When to update `KNOWN_WORKFLOW_NAMES`
///
/// When adding a new PID in a domain crate's `register_pids`:
/// 1. Add a dispatch arm in `ingest_dispatcher.rs::dispatch`.
/// 2. Add the workflow name string to `EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES`.
///
/// # Panics
///
/// Panics when any PidRouter-registered workflow name is absent from
/// `EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES`.
pub(crate) fn validate_dispatch_completeness(router: &mako_engine::pid_router::PidRouter) {
    use std::collections::HashSet;
    let known: HashSet<&str> = ingest_dispatcher::EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES
        .iter()
        .copied()
        .collect();

    let mut missing: Vec<&str> = router
        .workflow_names()
        .into_iter()
        .filter(|name| !known.contains(name))
        .collect();
    missing.sort_unstable();

    if !missing.is_empty() {
        panic!(
            "startup failure: the following workflows are registered in the PidRouter \
             but have no dispatch arm in EdifactIngestDispatcher:\n  {}\n\
             Add a dispatch arm in ingest_dispatcher.rs AND add the workflow name to \
             EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES.",
            missing.join("\n  ")
        );
    }
    info!(
        dispatched_workflows =
            ingest_dispatcher::EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES.len(),
        "dispatch completeness validated"
    );
}

// ── spawn_workers ─────────────────────────────────────────────────────────────

/// Configuration for all background workers spawned after server bind.
///
/// Build from the parsed [`Cli`] and assembled engine context, then pass to
/// [`spawn_workers`].
///
/// [`Cli`]: crate::main::Cli
pub(crate) struct WorkersConfig {
    /// Assembled engine context (consumed by outbox and deadline workers).
    pub ctx: MakodCtx,
    /// Store shared across projection and ERP workers.
    pub store: SlateDbStore,
    /// Inbox store used by the daily deduplication purge task.
    pub inbox_store_for_purge: SlateDbInboxStore,
    /// Shared Platform instance (used by the AS4 loopback path).
    pub platform: Arc<Platform>,
    /// Ingest dispatcher (used by the AS4 loopback path).
    pub ingest_dispatcher: Arc<ingest_dispatcher::EdifactIngestDispatcher>,
    /// Shared HTTP client (OIDC JWKS, MaLo-ID callbacks).
    pub http_client: reqwest::Client,
    /// MaLo cache (for MaloIdentSender and MCP server).
    pub malo_cache: Arc<malo_cache::SlateDbMaloCache>,
    /// Graceful-shutdown token — currently unused by background workers
    /// (workers run until process exit; HTTP servers hold their own clones).
    /// Reserved for future graceful-drain support.
    #[allow(dead_code)]
    pub shutdown_token: CancellationToken,
    // ── Outbound AS4 config ──────────────────────────────────────────────
    /// GLN registry — maps roles to GLNs and provides own-GLN detection.
    ///
    /// Built from `[[party]]` entries in `makod.toml`. The primary GLN is used
    /// as the storage partition key (`TenantId`) and AS4 `partyId` fallback.
    pub gln_registry: Arc<MpIdRegistry>,
    pub as4_partner: Vec<String>,
    pub as4_signing_key_pem: Option<SecretString>,
    pub as4_signing_cert_pem: Option<String>,
    pub as4_trust_anchor_pem: Option<String>,
    /// Per-partner encryption certificates: `GLN=<PEM>` pairs.
    ///
    /// The PEM is the trading partner's X.509 encryption certificate (EC BrainpoolP256r1).
    /// Used by the outbound AS4 sender to populate `As4SendCredentials::recipient_cert_pem`
    /// when `security.encrypt = true` (the BDEW-compliant default).
    pub as4_partner_certs: Vec<String>,
    pub as4_party_id: Option<String>,
    // ── MaLo-ID sender config ────────────────────────────────────────────
    pub maloid_partner: Vec<String>,
    pub verzeichnisdienst_url: Option<String>,
    // ── ERP webhook config ───────────────────────────────────────────────
    pub erp_webhook_url: Option<String>,
    pub erp_webhook_secret: Option<SecretString>,
    // ── EDIFACT outbox webhook (dev/no-AS4 mode) ─────────────────────────
    pub edifact_outbox_webhook_url: Option<String>,
    /// When `true`, allow the daemon to start without AS4 signing credentials
    /// and without an EDIFACT outbox webhook configured.  Defaults to `false`
    /// for production safety — set to `true` only in integration-test or
    /// CI environments where outbound EDIFACT delivery is intentionally
    /// disabled.
    ///
    /// Pass `--allow-no-as4-signing` on the CLI to set this flag.
    pub allow_no_as4_signing: bool,
    // ── Scheduler / timing config ────────────────────────────────────────
    pub snapshot_interval: u64,
    pub deadline_poll_interval_secs: u64,
    pub projection_checkpoint_interval: u64,
    /// `true` when neither `--as4-addr` nor `--http-addr` is set.
    ///
    /// `spawn_workers` will call `std::process::exit(1)` when this is `true`
    /// because a daemon that can receive no inbound messages is almost always
    /// a misconfiguration.
    pub no_transport_configured: bool,
    /// Health state — worker heartbeats are registered here after spawning.
    pub health_state: crate::health::HealthState,
}

/// Spawn all background workers and return immediately.
///
/// Workers run as Tokio tasks and stop when `cfg.shutdown_token` is cancelled.
///
/// # Errors
///
/// Returns an error if the outbound AS4 session cannot be built (invalid PEM),
/// the partner directory is malformed, the MaLo-ID partner URL is invalid, or
/// the Verzeichnisdienst URL is invalid.
///
/// # Panics
///
/// Never panics directly; the [`validate_adapter_coverage`] caller must run
/// first.  The no-transport guard calls `std::process::exit(1)` instead of
/// panicking (regulatory: the engine must not start silently discarding messages).
pub(crate) async fn spawn_workers(cfg: WorkersConfig) -> anyhow::Result<()> {
    use crate::as4_sender::BdewAs4Sender;
    use crate::malo_ident_sender::MaloIdentSender;
    use crate::verzeichnisdienst_worker;
    use crate::worker_health::new_heartbeat;
    use mako_as4::profile::BdewAs4Profile;
    use secrecy::ExposeSecret as _;

    // ── Parse --maloid-partner GLN=URL pairs ─────────────────────────────
    let maloid_partners = {
        let mut map = std::collections::HashMap::new();
        for pair in &cfg.maloid_partner {
            let (mp_id, url_str) = pair.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--maloid-partner: expected GLN=URL, got {pair:?}")
            })?;
            let url = reqwest::Url::parse(url_str)
                .map_err(|e| anyhow::anyhow!("--maloid-partner: invalid URL {url_str:?}: {e}"))?;
            map.insert(mp_id.to_owned(), url);
        }
        if !map.is_empty() {
            let glns: Vec<&str> = map.keys().map(String::as_str).collect();
            info!(partners = ?glns, "MaLo-ID partner directory loaded");
        }
        map
    };

    // ── Optional Verzeichnisdienst lookup ────────────────────────────────
    let verzeichnisdienst_lookup: Option<verzeichnisdienst_worker::VerzeichnisdienstLookup> =
        if let Some(ref vz_url_str) = cfg.verzeichnisdienst_url {
            let base_url = reqwest::Url::parse(vz_url_str).map_err(|e| {
                anyhow::anyhow!("--verzeichnisdienst-url: invalid URL {vz_url_str:?}: {e}")
            })?;
            let vz_client = energy_api::directory::DirectoryServiceClient::new(
                base_url.clone(),
                cfg.http_client.clone(),
            );
            let vz_partner_store = cfg.store.as_partner_store();
            let vz_tenant_id =
                mako_engine::ids::TenantId::from_party_id(cfg.gln_registry.primary_gln());
            info!(url = %base_url, "Verzeichnisdienst integration enabled");
            let lookup = verzeichnisdienst_worker::VerzeichnisdienstLookup::new(
                vz_client,
                vz_partner_store,
                vz_tenant_id,
            );
            let refresh_lookup = lookup.clone();
            tokio::spawn(verzeichnisdienst_worker::verzeichnisdienst_refresh_task(
                refresh_lookup,
                Duration::from_secs(300),
            ));
            Some(lookup)
        } else {
            None
        };

    let malo_sender = MaloIdentSender::new(
        (*cfg.malo_cache).clone(),
        cfg.http_client.clone(),
        maloid_partners,
        verzeichnisdienst_lookup,
        cfg.store.clone(),
    );

    // ── Build AS4 partner P-Mode registry from --as4-partner GLN=URL pairs ──
    let as4_profile = {
        let mut profile = BdewAs4Profile::new();
        for pair in &cfg.as4_partner {
            let (mp_id, url) = pair.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--as4-partner: expected GLN=HTTPS-URL, got {pair:?}")
            })?;
            let mp_id = mp_id.trim();
            let url = url.trim();
            if mp_id.is_empty() {
                return Err(anyhow::anyhow!(
                    "--as4-partner: GLN must not be empty in {pair:?}"
                ));
            }
            if !url.starts_with("https://") {
                return Err(anyhow::anyhow!(
                    "--as4-partner: endpoint URL must use HTTPS (got {url:?} for GLN {mp_id:?})"
                ));
            }
            profile.register_partner_all_actions(mp_id, url);
        }
        // ── Register per-partner encryption certificates ──────────────────
        // Format: GLN=<PEM> (the partner's X.509 encryption certificate).
        // Used for ECDH-ES key agreement (BDEW AS4-Profil v1.2 §2.2.6.2.2).
        for pair in &cfg.as4_partner_certs {
            let (mp_id, cert_pem) = pair.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--as4-partner-cert: expected GLN=<PEM>, got {pair:?}")
            })?;
            let mp_id = mp_id.trim();
            let cert_pem = cert_pem.trim();
            if mp_id.is_empty() {
                return Err(anyhow::anyhow!(
                    "--as4-partner-cert: GLN must not be empty in {pair:?}"
                ));
            }
            profile.register_partner_encryption_cert(mp_id, cert_pem.as_bytes().to_vec());
        }
        if !cfg.as4_partner_certs.is_empty() {
            let glns: Vec<&str> = cfg
                .as4_partner_certs
                .iter()
                .filter_map(|p| p.split_once('=').map(|(g, _)| g.trim()))
                .collect();
            info!(partners = ?glns, "AS4 per-partner encryption certificates registered");
        } else if !cfg.as4_partner.is_empty() {
            tracing::warn!(
                "AS4 partners registered without encryption certificates. \
                 BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every outbound message to be encrypted \
                 with the recipient's EC (BrainpoolP256r1) certificate. \
                 Add --as4-partner-cert GLN=<PEM> for each partner."
            );
        }
        profile
    };

    if !as4_profile.registry().is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for pm in as4_profile.all_pmodes() {
            seen.insert(pm.partner_id.as_str());
        }
        let glns: Vec<&str> = seen.into_iter().collect();
        info!(partners = ?glns, "AS4 partner P-Mode registry loaded");
    }

    // ── Outbox delivery worker ────────────────────────────────────────────
    //
    // BdewAs4Sender when signing credentials are present; MaloIdentSender
    // (MaLo-ID callbacks only) otherwise.
    if let (Some(key_pem), Some(cert_pem)) = (
        cfg.as4_signing_key_pem.as_ref().map(|s| s.expose_secret()),
        cfg.as4_signing_cert_pem.as_deref(),
    ) {
        let party_id = cfg
            .as4_party_id
            .clone()
            .unwrap_or_else(|| cfg.gln_registry.primary_gln().to_owned());

        let outbound_session = {
            let session_id = format!("makod-outbound-{}", uuid::Uuid::new_v4());
            let trust_anchor = cfg
                .as4_trust_anchor_pem
                .clone()
                .unwrap_or_else(|| cert_pem.to_owned());
            asx_rs::core::SessionContextBuilder::new(&session_id, &party_id)
                .with_signing_cert_pem(cert_pem.to_owned())
                .with_signing_key_pem(key_pem.to_owned())
                .with_trust_anchor_pem(trust_anchor)
                .build()
                .map_err(|e| anyhow::anyhow!("AS4 outbound session build failed: {e}"))?
        };
        let outbound_bus = Arc::new(
            asx_rs::observability::EventBus::new(256)
                .map_err(|e| anyhow::anyhow!("AS4 EventBus (outbound) init failed: {e}"))?,
        );

        let sender = BdewAs4Sender::new(
            Arc::new(outbound_session),
            outbound_bus,
            Arc::new(as4_profile),
            malo_sender,
            Arc::clone(&cfg.gln_registry),
            Some(Arc::new(crate::edifact_api::EdifactApiState {
                platform: Arc::clone(&cfg.platform),
                pid_router: cfg.ctx.pid_router().clone(),
                cedar: Arc::new(
                    crate::cedar_authz::CedarAuthorizer::unauthenticated()
                        .expect("CedarAuthorizer::unauthenticated is infallible"),
                ),
                max_body_bytes: 256 * 1024 * 1024, // 256 MiB — generous but finite (F-009)
                partner_store: None,
                tenant_id: mako_engine::ids::TenantId::from_party_id(
                    cfg.gln_registry.primary_gln(),
                ),
                dl_sink: std::sync::Arc::new(mako_engine::dead_letter::LogDeadLetterSink),
                dispatcher: Some(Arc::clone(&cfg.ingest_dispatcher)),
                // AS4 loopback (self-delivery) does not need a CONTRL ack:
                // we are both sender and receiver in this code path.
                contrl_ack: None,
            })),
        )?;

        info!(
            party_id        = %party_id,
            primary_gln     = %cfg.gln_registry.primary_gln(),
            own_glns        = ?cfg.gln_registry.own_glns().collect::<Vec<_>>(),
            "AS4 outbound sender active (BdewAs4Sender)",
        );
        let (outbox_hb, outbox_watch) = new_heartbeat("outbox-worker", 120);
        let worker = cfg
            .ctx
            .run_outbox_worker(sender, 50, Duration::from_secs(5), 48)
            .with_heartbeat(outbox_hb.last_tick_raw());
        cfg.health_state.register_worker(outbox_watch);
        tokio::spawn(async move { worker.run().await });
    } else if let Some(ref edifact_webhook_url) = cfg.edifact_outbox_webhook_url {
        use crate::as4_sender::WebhookEdifactSender;
        let sender = WebhookEdifactSender::new(
            edifact_webhook_url.as_str(),
            Arc::clone(&cfg.gln_registry),
            cfg.http_client.clone(),
            malo_sender,
        );
        info!(
            url = %edifact_webhook_url,
            "EDIFACT outbox webhook sender active (WebhookEdifactSender) — \
             outbound EDIFACT will be POSTed as CloudEvents",
        );
        let (outbox_hb, outbox_watch) = new_heartbeat("outbox-worker", 120);
        let worker = cfg
            .ctx
            .run_outbox_worker(sender, 50, Duration::from_secs(5), 48)
            .with_heartbeat(outbox_hb.last_tick_raw());
        cfg.health_state.register_worker(outbox_watch);
        tokio::spawn(async move { worker.run().await });
    } else {
        if !cfg.allow_no_as4_signing {
            anyhow::bail!(
                "AS4 signing credentials not configured \
                 (--as4-signing-key-pem / --as4-signing-cert-pem not set) and no \
                 --edifact-outbox-webhook-url fallback is configured. \
                 Outbound EDIFACT delivery would silently fail for all messages. \
                 To suppress this error in non-production environments, pass \
                 --allow-no-as4-signing.",
            );
        }
        tracing::warn!(
            "AS4 signing credentials not configured \
             (--as4-signing-key-pem / --as4-signing-cert-pem not set). \
             Outbox delivery is running in MaloIdentCallback-only mode — \
             all EDIFACT messages will be logged and rescheduled without transmission. \
             Pass --allow-no-as4-signing to silence this warning.",
        );
        let (outbox_hb, outbox_watch) = new_heartbeat("outbox-worker", 120);
        let worker = cfg
            .ctx
            .run_outbox_worker(malo_sender, 50, Duration::from_secs(5), 48)
            .with_heartbeat(outbox_hb.last_tick_raw());
        cfg.health_state.register_worker(outbox_watch);
        tokio::spawn(async move { worker.run().await });
    }

    info!("outbox delivery worker started");

    // ── ERP webhook outbound worker ───────────────────────────────────────
    if let Some(erp_url) = cfg.erp_webhook_url.clone() {
        let adapter = erp_adapter::WebhookErpAdapter::new(erp_url.clone(), cfg.erp_webhook_secret);
        let worker = erp_adapter::OutboxErpWorker::new(
            cfg.store.clone(),
            adapter,
            50,
            Duration::from_secs(5),
        );
        info!(erp_webhook_url = %erp_url, "ERP webhook outbound worker started");
        tokio::spawn(async move { worker.run().await });
    } else {
        let adapter = mako_engine::erp::LogErpAdapter;
        let worker = erp_adapter::OutboxErpWorker::new(
            cfg.store.clone(),
            adapter,
            50,
            Duration::from_secs(30),
        );
        tracing::debug!(
            "ERP outbound notifications are logged only \
             (--erp-webhook-url not set; set to enable HTTP delivery)",
        );
        tokio::spawn(async move { worker.run().await });
    }

    // ── No-transport guard ────────────────────────────────────────────────
    if cfg.no_transport_configured {
        tracing::error!(
            "No ingest transport configured: neither --as4-addr nor --http-addr \
             is set.  The engine cannot receive any inbound messages.  \
             Set at least one of these options and restart."
        );
        std::process::exit(1);
    }

    // ── Deadline scheduler ────────────────────────────────────────────────
    let event_store_for_scheduler = Arc::clone(cfg.ctx.event_store());
    let (deadline_hb, deadline_watch) = new_heartbeat(
        "deadline-scheduler",
        (cfg.deadline_poll_interval_secs.max(1) * 3) as i64,
    );
    let scheduler = deadline_dispatch::build_scheduler(
        &cfg.ctx,
        event_store_for_scheduler,
        cfg.snapshot_interval,
        Duration::from_secs(cfg.deadline_poll_interval_secs.max(1)),
    )
    .with_heartbeat(deadline_hb.last_tick_raw());
    cfg.health_state.register_worker(deadline_watch);
    tokio::spawn(async move { scheduler.run().await });
    info!(
        poll_interval_secs = cfg.deadline_poll_interval_secs.max(1),
        "deadline scheduler started",
    );

    // ── Projection checkpoint workers ─────────────────────────────────────
    if cfg.projection_checkpoint_interval > 0 {
        let interval = Duration::from_secs(cfg.projection_checkpoint_interval);

        let (proj1_hb, proj1_watch) = new_heartbeat(
            "projection-worker:gpke-konfiguration",
            (cfg.projection_checkpoint_interval * 5).max(300) as i64,
        );
        let worker = crate::projection_worker::ProjectionWorker::new(
            cfg.store.clone(),
            mako_gpke::KonfigurationProjection::default(),
            Some("gpke/"),
            interval,
        )
        .with_heartbeat(proj1_hb.last_tick_raw());
        cfg.health_state.register_worker(proj1_watch);
        tokio::spawn(async move { worker.run().await });

        let (proj2_hb, proj2_watch) = new_heartbeat(
            "projection-worker:gpke-supplier-change",
            (cfg.projection_checkpoint_interval * 5).max(300) as i64,
        );
        let worker = crate::projection_worker::ProjectionWorker::new(
            cfg.store.clone(),
            mako_gpke::SupplierChangeProjection::default(),
            Some("gpke/"),
            interval,
        )
        .with_heartbeat(proj2_hb.last_tick_raw());
        cfg.health_state.register_worker(proj2_watch);
        tokio::spawn(async move { worker.run().await });

        info!(
            interval_secs = cfg.projection_checkpoint_interval,
            "projection checkpoint workers started",
        );
    } else {
        tracing::warn!(
            "--projection-checkpoint-interval=0: projection checkpoints disabled; \
             every restart will trigger a full event-store replay",
        );
    }

    // ── Inbox purge worker ────────────────────────────────────────────────
    //
    // Deletes deduplication keys older than 72 hours once per day.
    let inbox_store_for_purge = cfg.inbox_store_for_purge;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            interval.tick().await;
            let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(72);
            match inbox_store_for_purge.purge_expired(cutoff).await {
                Ok(n) => tracing::info!(removed = n, "inbox purge complete"),
                Err(e) => tracing::error!(error = %e, "inbox purge failed"),
            }
        }
    });
    info!("inbox purge worker started (daily, 72h TTL)");

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that every domain workflow has an adapter registered for all
    /// active BDEW format versions.
    ///
    /// This test is the primary guard against accidentally shipping a build
    /// where some workflows silently dead-letter cross-FV messages.  Any
    /// breakage here means `adapters.rs` needs a new adapter entry.
    #[test]
    fn all_workflows_have_adapter_coverage() {
        // validate_adapter_coverage panics on missing coverage — the panic
        // itself is the assertion.
        validate_adapter_coverage();
    }

    /// Verify that `KNOWN_WORKFLOW_NAMES` is sorted and deduplicated.
    ///
    /// The list is maintained by hand; this test catches accidental duplicates
    /// or missorting that would make the coverage diff harder to read.
    #[test]
    fn known_workflow_names_sorted_and_unique() {
        use ingest_dispatcher::EdifactIngestDispatcher;
        let names = EdifactIngestDispatcher::KNOWN_WORKFLOW_NAMES;

        // Check sorted order.
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            names,
            sorted.as_slice(),
            "KNOWN_WORKFLOW_NAMES must be sorted alphabetically; \
             expected {sorted:?}",
        );

        // Check no duplicates.
        sorted.dedup();
        assert_eq!(
            names.len(),
            sorted.len(),
            "KNOWN_WORKFLOW_NAMES contains duplicates: {:?}",
            {
                let mut seen = std::collections::HashSet::new();
                names
                    .iter()
                    .filter(|&&n| !seen.insert(n))
                    .copied()
                    .collect::<Vec<_>>()
            }
        );
    }
}
