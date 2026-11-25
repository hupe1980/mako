//! Write or replay the synthetic LF protocol checkpoint used by the cold restore drill.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, Parser, Subcommand};
use mako_engine::{
    event_store::EventStore,
    ids::TenantId,
    outbox::OutboxStore,
    process::{Process, RunOptions},
    store_slatedb::SlateDbStore,
    types::{MaLo, MarktpartnerCode, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{GpkeLfAnmeldungWorkflow, LfAnmeldungCommand, LfAnmeldungState};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TENANT: &str = "stadtwerke-musterstadt";
const MARKER: &str = "mako-synthetic-restore-v1";
const MALO: &str = "51238696012";

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new directory with a committed LF event and one undelivered UTILMD.
    Seed { directory: PathBuf },
    /// Re-fold an isolated copy of the synthetic checkpoint.
    Inspect { directory: PathBuf },
}

fn registration() -> LfAnmeldungCommand {
    LfAnmeldungCommand::InitiateAnmeldung {
        pid: Pruefidentifikator::new(mako_gpke::lf_anmeldung::ANMELDUNG_PID)
            .expect("published PID"),
        sender: MarktpartnerCode::new("9900357000028"),
        receiver: MarktpartnerCode::new("9900357000004"),
        location_id: MaLo::new(MALO),
        process_date: "20261001".into(),
        transaktionsgrund: Some("E03".into()),
        transaktionsgrund_ergaenzung: Some("ZW4".into()),
        bilanzkreis: Some("11XBK-LF-------9".into()),
        tranchengroesse: None,
        kundendaten: Some(Box::new(mako_gpke::lf_anmeldung::Kundendaten {
            haushaltskunde: true,
            enfg_grundlage: "ZG1".into(),
            enfg_grund: None,
            stammdaten: Default::default(),
        })),
    }
}

#[must_use = "replay must agree with the independent checkpoint"]
async fn evidence(store: &SlateDbStore) -> Result<Value> {
    let messages = store.pending_now(2).await?;
    let [pending] = messages.as_slice() else {
        anyhow::bail!("expected exactly one pending protocol message");
    };
    let history = store.load(&pending.stream_id).await?;
    let [event] = history.as_slice() else {
        anyhow::bail!("expected one committed Anmeldung event");
    };
    let tenant = TenantId::from_tenant(TENANT);
    ensure!(
        event.tenant_id == tenant && pending.tenant_id == tenant,
        "wrong tenant"
    );
    ensure!(
        event.process_id == pending.process_id
            && event.event_id == pending.causation_event_id
            && event.workflow_id == WorkflowId::new("gpke-lf-anmeldung", "FV2026-10-01"),
        "outbox entry and committed event disagree"
    );
    let process = Process::<GpkeLfAnmeldungWorkflow, _>::from_stream(
        store.clone(),
        event.stream_id.clone(),
        event.process_id,
        tenant,
        event.workflow_id.clone(),
    );
    let state = process.state().await?;
    ensure!(
        matches!(state, LfAnmeldungState::Pending(_)),
        "Anmeldung is not pending"
    );
    ensure!(
        pending.message_type.as_ref() == "UTILMD" && pending.payload["malo"] == MALO,
        "pending message disagrees with the synthetic supply"
    );
    let fingerprint = Sha256::digest(serde_json::to_vec(&(&history, &state, pending))?);
    Ok(json!({
        "process_id": event.process_id, "event_id": event.event_id,
        "sequence": event.sequence_number, "outbox_id": pending.message_id,
        "state": "pending", "sha256": format!("{fingerprint:x}")
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(command) = Args::parse().command else {
        Args::command().print_help()?;
        println!();
        return Ok(());
    };
    let directory = match &command {
        Command::Seed { directory } => {
            fs::create_dir(directory).context("create fresh synthetic store")?;
            fs::write(directory.join(MARKER), MARKER)?;
            directory
        }
        Command::Inspect { directory } => {
            ensure!(
                fs::read(directory.join(MARKER))? == MARKER.as_bytes(),
                "expected synthetic store"
            );
            directory
        }
    };
    let guard = makod::startup::lock_data_dir(directory)?;
    let store = guard.open_store().await?;
    if matches!(command, Command::Seed { .. }) {
        let process = Process::<GpkeLfAnmeldungWorkflow, _>::new(
            store.clone(),
            TenantId::from_tenant(TENANT),
            WorkflowId::new("gpke-lf-anmeldung", "FV2026-10-01"),
        );
        process.run(registration(), RunOptions::system()).await?;
    }
    let report = evidence(&store).await?;
    store.close_owned().await?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
