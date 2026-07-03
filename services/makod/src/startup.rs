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

use crate::{adapters, deadline_dispatch, erp_adapter, ingest_dispatcher, malo_cache};

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
            "gpke-supplier-change",
            adapters::gpke_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-lf-anmeldung",
            adapters::gpke_lf_anmeldung_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-neuanlage",
            adapters::gpke_neuanlage_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-lf-abmeldung",
            adapters::gpke_lf_abmeldung_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-ankuendigung-zuordnung-lf",
            adapters::gpke_ankuendigung_zuordnung_lf_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-sperrung",
            adapters::gpke_sperrung_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-stornierung",
            adapters::gpke_stornierung_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-anfrage-bestellung",
            adapters::gpke_anfrage_bestellung_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-abrechnung",
            adapters::gpke_abrechnung_registry().validate_policy(fc, &known),
        ),
        (
            "gpke-konfiguration",
            adapters::gpke_konfiguration_registry().validate_policy(fc, &known),
        ),
        (
            "wim-device-change",
            adapters::wim_registry().validate_policy(fc, &known),
        ),
        (
            "wim-geraeteubernahme",
            adapters::wim_geraeteubernahme_registry().validate_policy(fc, &known),
        ),
        (
            "wim-stammdaten",
            adapters::wim_stammdaten_registry().validate_policy(fc, &known),
        ),
        (
            "wim-stornierung",
            adapters::wim_stornierung_registry().validate_policy(fc, &known),
        ),
        (
            "wim-rechnung",
            adapters::wim_rechnung_registry().validate_policy(fc, &known),
        ),
        (
            "wim-insrpt",
            adapters::wim_insrpt_registry().validate_policy(fc, &known),
        ),
        (
            "geli-gas-supplier-change",
            adapters::geli_gas_registry().validate_policy(fc, &known),
        ),
        (
            "geli-gas-stornierung",
            adapters::geli_gas_stornierung_registry().validate_policy(fc, &known),
        ),
        (
            "wim-gas-anmeldung",
            adapters::wim_gas_anmeldung_registry().validate_policy(fc, &known),
        ),
        (
            "wim-gas-kuendigung",
            adapters::wim_gas_kuendigung_registry().validate_policy(fc, &known),
        ),
        (
            "wim-gas-verpflichtungsanfrage",
            adapters::wim_gas_verpflichtungsanfrage_registry().validate_policy(fc, &known),
        ),
        (
            "wim-gas-invoic",
            adapters::wim_gas_invoic_registry().validate_policy(fc, &known),
        ),
        (
            "wim-gas-insrpt",
            adapters::wim_gas_insrpt_registry().validate_policy(fc, &known),
        ),
        (
            "gabi-gas-invoic",
            adapters::gabi_gas_invoic_registry().validate_policy(fc, &known),
        ),
        // gabi-gas-nomination: DVGW NOMINT/NOMRES adapter (synthetic PIDs 90011/90012/90021/90022).
        (
            "gabi-gas-nomination",
            adapters::gabi_gas_nomination_registry().validate_policy(fc, &known),
        ),
        // gabi-gas-allocation: DVGW ALOCAT adapter (synthetic PIDs 90001/90002/90003).
        (
            "gabi-gas-allocation",
            adapters::gabi_gas_allocation_registry().validate_policy(fc, &known),
        ),
        (
            "geli-gas-sperrprozesse-invoic",
            adapters::geli_gas_sperrprozesse_invoic_registry().validate_policy(fc, &known),
        ),
        (
            "geli-gas-sperrung-nb",
            adapters::geli_gas_sperrung_nb_registry().validate_policy(fc, &known),
        ),
        // mabis-billing: IFTSTA adapter covers PIDs 21000–21007.
        // MSCONS PID 13003 billing commands are constructed by the aggregation
        // layer; this check validates IFTSTA coverage only.
        (
            "mabis-billing (IFTSTA)",
            adapters::mabis_registry().validate_policy(fc, &known),
        ),
        // mabis-clearingliste: UTILMD adapter covers PIDs 55065, 55069, 55070.
        (
            "mabis-clearingliste",
            adapters::mabis_clearingliste_registry().validate_policy(fc, &known),
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
    pub tenant_id: String,
    pub as4_partner: Vec<String>,
    pub as4_signing_key_pem: Option<SecretString>,
    pub as4_signing_cert_pem: Option<String>,
    pub as4_trust_anchor_pem: Option<String>,
    pub as4_party_id: Option<String>,
    // ── MaLo-ID sender config ────────────────────────────────────────────
    pub maloid_partner: Vec<String>,
    pub verzeichnisdienst_url: Option<String>,
    // ── ERP webhook config ───────────────────────────────────────────────
    pub erp_webhook_url: Option<String>,
    pub erp_webhook_secret: Option<SecretString>,
    // ── EDIFACT outbox webhook (dev/no-AS4 mode) ─────────────────────────
    pub edifact_outbox_webhook_url: Option<String>,
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
    use mako_as4::PartnerDirectory;
    use secrecy::ExposeSecret as _;

    // ── Parse --maloid-partner GLN=URL pairs ─────────────────────────────
    let maloid_partners = {
        let mut map = std::collections::HashMap::new();
        for pair in &cfg.maloid_partner {
            let (gln, url_str) = pair.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--maloid-partner: expected GLN=URL, got {pair:?}")
            })?;
            let url = reqwest::Url::parse(url_str)
                .map_err(|e| anyhow::anyhow!("--maloid-partner: invalid URL {url_str:?}: {e}"))?;
            map.insert(gln.to_owned(), url);
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
            let vz_tenant_id = mako_engine::ids::TenantId::from_party_id(&cfg.tenant_id);
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

    // ── Parse --as4-partner GLN=URL pairs ────────────────────────────────
    let partners =
        PartnerDirectory::from_cli_pairs(&cfg.as4_partner).map_err(|e| anyhow::anyhow!("{e}"))?;

    if !partners.is_empty() {
        let glns: Vec<&str> = partners.iter().map(|(g, _)| g).collect();
        info!(partners = ?glns, "AS4 partner directory loaded");
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
            .unwrap_or_else(|| cfg.tenant_id.clone());

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
            Arc::new(partners),
            malo_sender,
            cfg.tenant_id.as_str(),
            Some(Arc::new(crate::edifact_api::EdifactApiState {
                platform: Arc::clone(&cfg.platform),
                pid_router: cfg.ctx.pid_router().clone(),
                cedar: Arc::new(
                    crate::cedar_authz::CedarAuthorizer::unauthenticated()
                        .expect("CedarAuthorizer::unauthenticated is infallible"),
                ),
                max_body_bytes: usize::MAX,
                partner_store: None,
                tenant_id: mako_engine::ids::TenantId::from_party_id(&cfg.tenant_id),
                dispatcher: Some(Arc::clone(&cfg.ingest_dispatcher)),
                // AS4 loopback (self-delivery) does not need a CONTRL ack:
                // we are both sender and receiver in this code path.
                contrl_ack: None,
            })),
        )?;

        info!(
            party_id        = %party_id,
            tenant_party_id = %cfg.tenant_id,
            "AS4 outbound sender active (BdewAs4Sender)",
        );
        let worker = cfg
            .ctx
            .run_outbox_worker(sender, 50, Duration::from_secs(5), 48);
        tokio::spawn(async move { worker.run().await });
    } else if let Some(ref edifact_webhook_url) = cfg.edifact_outbox_webhook_url {
        use crate::as4_sender::WebhookEdifactSender;
        let sender = WebhookEdifactSender::new(
            edifact_webhook_url.as_str(),
            cfg.tenant_id.as_str(),
            cfg.http_client.clone(),
            malo_sender,
        );
        info!(
            url = %edifact_webhook_url,
            "EDIFACT outbox webhook sender active (WebhookEdifactSender) — \
             outbound EDIFACT will be POSTed as CloudEvents",
        );
        let worker = cfg
            .ctx
            .run_outbox_worker(sender, 50, Duration::from_secs(5), 48);
        tokio::spawn(async move { worker.run().await });
    } else {
        tracing::warn!(
            "AS4 signing credentials not configured \
             (--as4-signing-key-pem / --as4-signing-cert-pem not set). \
             Outbox delivery is running in MaloIdentCallback-only mode — \
             all EDIFACT messages will be logged and rescheduled without transmission.",
        );
        let worker = cfg
            .ctx
            .run_outbox_worker(malo_sender, 50, Duration::from_secs(5), 48);
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
    let scheduler = deadline_dispatch::build_scheduler(
        &cfg.ctx,
        event_store_for_scheduler,
        cfg.snapshot_interval,
        Duration::from_secs(cfg.deadline_poll_interval_secs.max(1)),
    );
    tokio::spawn(async move { scheduler.run().await });
    info!(
        poll_interval_secs = cfg.deadline_poll_interval_secs.max(1),
        "deadline scheduler started",
    );

    // ── Projection checkpoint workers ─────────────────────────────────────
    if cfg.projection_checkpoint_interval > 0 {
        let interval = Duration::from_secs(cfg.projection_checkpoint_interval);

        let worker = crate::projection_worker::ProjectionWorker::new(
            cfg.store.clone(),
            mako_gpke::KonfigurationProjection::default(),
            Some("gpke/"),
            interval,
        );
        tokio::spawn(async move { worker.run().await });

        let worker = crate::projection_worker::ProjectionWorker::new(
            cfg.store.clone(),
            mako_gpke::SupplierChangeProjection::default(),
            Some("gpke/"),
            interval,
        );
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
}
