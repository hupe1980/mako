//! Local capture for a single reconciled Strom NB occurrence.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mako_engine::builder::As4Sender;
use mako_engine::deadline::DeadlineStore as _;
use mako_engine::error::EngineError;
use mako_engine::ids::TenantId;
use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
use mako_engine::outbox::{OutboxMessage, OutboxStore};
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{config::PartyConfig, deployment::Rolle, party_registry::MpIdRegistry, startup};
mod prepare;

const STORE_PREFIX: &str = "shadow-v1";
const META: KvNamespace = KvNamespace::new("shadow/meta/");
const CAPTURES: KvNamespace = KvNamespace::new("shadow/captures/");
const TENANT: &str = "shadow-rehearsal";

#[derive(Debug, Clone, clap::Parser)]
#[command(bin_name = "makod shadow")]
pub(super) struct Args {
    /// Existing private working directory for this occurrence.
    #[arg(long)]
    workspace: PathBuf,
    /// Own Strom NB MP-ID.
    #[arg(long)]
    nb: String,
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Action {
    /// Print the effective composition without opening the store.
    Check,
    /// Import one verified makotest daemon-input packet from stdin.
    Prepare {
        /// Scope fingerprint from the independently selected coverage report.
        #[arg(long)]
        scope: String,
    },
    /// Drain due deadlines and ready outbox entries into local capture.
    Run,
    /// Read the durable capture counts.
    Report,
    /// Write the captured private UTILMD reply to stdout.
    Answer,
}

#[must_use = "a failed shadow operation must reach the operator"]
async fn open(path: &Path) -> Result<SlateDbStore> {
    let local = object_store::local::LocalFileSystem::new_with_prefix(path)?;
    // The live composition opens the fixed prefix `db`, even for a nested data dir.
    Ok(SlateDbStore::open(STORE_PREFIX, Arc::new(local)).await?)
}

#[must_use = "a failed shadow operation must reach the operator"]
fn registry(nb: &str) -> Result<Arc<MpIdRegistry>> {
    Ok(Arc::new(MpIdRegistry::from_config(&[PartyConfig {
        mp_id: nb.to_owned(),
        roles: vec!["NB".into()],
        primary: true,
        agency: None,
        as4: None,
    }])?))
}

#[must_use = "a failed shadow operation must reach the operator"]
pub(super) fn start(args: &Args) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

#[must_use = "a failed shadow operation must reach the operator"]
async fn run(args: &Args) -> Result<()> {
    let parties = registry(&args.nb)?;
    if matches!(args.action, Action::Check) {
        println!(
            "{}",
            json!({"mode":"shadow","role":"NB","storage":"local","outbound":"capture","listeners":0})
        );
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(&args.workspace)?;
    ensure!(metadata.is_dir(), "workspace must be a directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "workspace must be private (chmod 700)"
        );
    }
    let workspace = startup::lock_data_dir(&args.workspace)?;
    let path = workspace.path().join("occurrence");
    if path.try_exists()? {
        ensure!(
            std::fs::symlink_metadata(&path)?.is_dir()
                && std::fs::symlink_metadata(path.join(STORE_PREFIX)).is_ok_and(|m| m.is_dir()),
            "invalid occurrence directory"
        );
    }
    if let Action::Prepare { scope } = &args.action {
        return prepare::prepare(workspace.path(), &path, parties, scope).await;
    }
    ensure!(path.is_dir(), "prepare an occurrence first");
    let store = Arc::new(open(&path).await?);
    let result = async {
        ensure!(
            store.kv_get(META, "nb").await?.as_deref() == Some(args.nb.as_bytes()),
            "workspace belongs to a different NB"
        );
        match args.action {
            Action::Run => workers(store.clone(), parties).await,
            Action::Report | Action::Answer => {
                report(&store, matches!(args.action, Action::Answer)).await
            }
            Action::Check | Action::Prepare { .. } => unreachable!(),
        }
    }
    .await;
    store.close().await?;
    result
}

struct Capture {
    store: Arc<SlateDbStore>,
    parties: Arc<MpIdRegistry>,
    day: time::Date,
    platform: edi_energy::Platform,
}

impl As4Sender for Capture {
    async fn send(&self, msg: &OutboxMessage) -> Result<(), EngineError> {
        let key = msg.message_id.to_string();
        if self.store.kv_get(CAPTURES, &key).await?.is_some() {
            return Ok(());
        }
        let effect = if let Some(event) = crate::erp_adapter::erp_event(msg) {
            json!({"kind": "erp", "event": crate::erp_adapter::cloud_event_json(&event)})
        } else if matches!(
            msg.message_type.as_ref(),
            "MaloIdentCallback" | crate::netzzugang::NETZZUGANG_MESSAGE_TYPE
        ) {
            json!({"kind": "callback", "intent": msg.payload})
        } else {
            match crate::edifact_renderer::render_to_wire_bytes(msg, &self.parties) {
                Ok(wire) => {
                    let valid = crate::transport::sendepruefung::pruefe(
                        &self.platform,
                        &wire.bytes,
                        &self.parties,
                        self.day,
                    )
                    .is_ok();
                    let kind = if valid { "edifact" } else { "refused" };
                    json!({"kind": kind, "wire": STANDARD.encode(wire.bytes)})
                }
                Err(err) if crate::edifact_renderer::is_suppressed(&err) => {
                    json!({"kind": "suppressed"})
                }
                Err(_) => json!({"kind": "unrenderable", "intent": msg.payload}),
            }
        };
        let record = json!({
            "message_id": key,
            "type": msg.message_type,
            "cause": msg.causation_event_id,
            "workflow": msg.workflow_name,
            "format_version": msg.format_version,
            "effect": effect,
        });
        self.store
            .kv_put(CAPTURES, &key, record.to_string().as_bytes())
            .await
    }
}

fn engine(store: &SlateDbStore) -> startup::MakodCtx {
    startup::build_engine(
        store,
        store.as_dead_letter_sink(TenantId::from_tenant(TENANT)),
        DeploymentRoles::from_roles([Marktrolle::Nb]),
        &BTreeSet::from([Rolle::NbStrom]),
    )
}

#[must_use = "a failed shadow operation must reach the operator"]
async fn workers(store: Arc<SlateDbStore>, parties: Arc<MpIdRegistry>) -> Result<()> {
    let received: time::OffsetDateTime = time::OffsetDateTime::parse(
        std::str::from_utf8(
            &store
                .kv_get(META, "received_at")
                .await?
                .context("missing receipt")?,
        )?,
        &time::format_description::well_known::Rfc3339,
    )?;
    let ctx = engine(&store);
    let cancel = CancellationToken::new();
    let interval = Duration::from_millis(100);
    let sender = Capture {
        store: store.clone(),
        parties,
        day: mako_fristen::berlin_date(received),
        platform: edi_energy::Platform::with_all_profiles(),
    };
    let outbox = ctx
        .run_outbox_worker(
            sender,
            50,
            interval,
            startup::OUTBOX_MAX_ATTEMPTS,
            startup::OUTBOX_RETRY_WINDOW,
        )
        .with_shutdown(cancel.clone());
    let deadlines = crate::deadline_dispatch::build_scheduler(&ctx, store.clone(), 100, interval)
        .with_shutdown(cancel.clone());
    let drain = async {
        let idle = async {
            loop {
                // A due deadline remains visible until its workflow commits any resulting outbox entries.
                if store
                    .as_deadline_store()
                    .due_now(1)
                    .await?
                    .deadlines
                    .is_empty()
                    && store.pending_now(1).await?.is_empty()
                {
                    return Ok::<_, EngineError>(());
                }
                tokio::time::sleep(interval).await;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(30), idle).await;
        cancel.cancel();
        result
    };
    let ((), (), result) = tokio::join!(outbox.run(), deadlines.run(), drain);
    result.context("capture workers did not drain in 30 seconds")??;
    report(&store, false).await
}

#[must_use = "a failed shadow operation must reach the operator"]
async fn report(store: &SlateDbStore, answer: bool) -> Result<()> {
    use std::io::Write as _;
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut reply = None;
    for (_, value) in store.kv_scan_prefix(CAPTURES).await? {
        let record: Value = serde_json::from_slice(&value)?;
        let effect = &record["effect"];
        let kind = effect["kind"].as_str().context("invalid capture")?;
        *counts.entry(kind.to_owned()).or_default() += 1;
        if answer && kind == "edifact" && record["type"] == "UTILMD" {
            ensure!(reply.is_none(), "ambiguous answer");
            reply = Some(STANDARD.decode(effect["wire"].as_str().context("missing wire")?)?);
        }
    }
    if answer {
        std::io::stdout()
            .lock()
            .write_all(&reply.context("no validated answer captured")?)?;
    } else {
        println!(
            "{}",
            json!({
                "mode": "shadow",
                "captured": counts,
                "pending": OutboxStore::len(store).await?,
                "dead_letters": !store.list_dead_letters(1).await?.is_empty(),
            })
        );
    }
    Ok(())
}
