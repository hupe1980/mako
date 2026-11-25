//! Publish one prepared occurrence atomically so an interrupted import can be repeated.
use super::{META, TENANT, open};
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use edi_energy::EdiEnergyMessage as _;
use mako_engine::envelope::Akteur;
use mako_engine::ids::TenantId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;

use crate::commands_api::{CommandsApiState, dispatch_command};
use crate::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
use crate::malo_cache::{MaloIdentResultCache, SlateDbMaloCache};
use crate::party_registry::MpIdRegistry;

const MAX_PACKET: usize = 2 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Packet {
    version: u8,
    nb: String,
    malo: String,
    wire: String,
    request_sha256: String,
    received_at: String,
    decision: Decision,
    grid_member: bool,
    supply_status: String,
    pruefschritt: u16,
    ebd_revision: String,
    coverage: Fingerprints,
    scope: String,
    direction: String,
    role: String,
    batch: String,
    revision: String,
    occurrence: String,
    facts: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    pid: u32,
    tree: String,
    code: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Fingerprints {
    journal: String,
    inventory: String,
    observed: String,
}

impl Packet {
    #[must_use = "a refused occurrence cannot be imported"]
    fn validate(&self, parties: &MpIdRegistry) -> Result<(edi_energy::AnyMessage, OffsetDateTime)> {
        ensure!(
            self.version == 1 && self.nb == parties.primary_mp_id(),
            "unsupported version or NB"
        );
        ensure!(
            [
                &self.request_sha256,
                &self.facts,
                &self.scope,
                &self.coverage.journal,
                &self.coverage.inventory,
                &self.coverage.observed
            ]
            .into_iter()
            .all(|v| hex_digest(v, 64))
                && [&self.batch, &self.revision, &self.occurrence]
                    .into_iter()
                    .all(|v| {
                        (1..=64).contains(&v.len())
                            && v.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                    }),
            "invalid input reference"
        );
        ensure!(
            self.decision.pid == 55003
                && self.decision.tree == "E_0622"
                && self.decision.code == "A02"
                && self.pruefschritt == 30
                && self.ebd_revision == "4.3",
            "unsupported NB decision"
        );
        ensure!(
            self.direction == "inbound" && self.role == "NB",
            "unsupported role or direction"
        );
        let bytes = STANDARD.decode(&self.wire)?;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == self.request_sha256,
            "request digest mismatch"
        );
        let received = OffsetDateTime::parse(
            &self.received_at,
            &time::format_description::well_known::Rfc3339,
        )?;
        let platform = edi_energy::Platform::with_all_profiles();
        let mut interchange = platform.parse_interchange_full(&bytes)?;
        ensure!(
            interchange.messages.len() == 1
                && interchange.count_matches_declared()
                && interchange.control_refs_match()
                && interchange.header.receiver_id.as_ref() == self.nb,
            "occurrence envelope mismatch"
        );
        crate::transport::sendepruefung::pruefe(
            &platform,
            &bytes,
            parties,
            mako_fristen::berlin_date(received),
        )
        .map_err(|_| anyhow::anyhow!("input profile refused"))?;
        let message = interchange.messages.remove(0).message;
        let edi_energy::AnyMessage::Utilmd(ref utilmd) = message else {
            anyhow::bail!("expected UTILMD")
        };
        ensure!(
            message.detect_pruefidentifikator()?.as_u32() == 55001
                && utilmd.transactions().len() == 1
                && utilmd.transactions()[0].marktlokation() == Some(&self.malo)
                && utilmd.transactions()[0]
                    .transaktionsgrund()
                    .and_then(|g| g.ergaenzung)
                    .as_deref()
                    == Some("ZW4"),
            "occurrence is outside the consuming NB Anmeldung scope"
        );
        let tx = &utilmd.transactions()[0];
        ensure!(
            utilmd.assoc_code() == "S2.2"
                && tx.locations.len() == 1
                && utilmd.receiver().and_then(|n| n.party_id.as_deref()) == Some(self.nb.as_str())
                && utilmd.sender().and_then(|n| n.party_id.as_deref())
                    == Some(interchange.header.sender_id.as_ref()),
            "message identities or release differ from the occurrence"
        );
        let stillgelegt = self.supply_status == "Stillgelegt";
        ensure!(self.grid_member && stillgelegt, "missing NB facts");
        let delivery = crate::adapters::prozesstag(
            tx.dtm.iter().find(|d| d.qualifier == "92"),
            mako_fristen::Sparte::Strom,
        )?
        .context("missing Lieferbeginn")?;
        let delivery = time::Date::parse(
            &delivery,
            time::macros::format_description!("[year][month][day]"),
        )?;
        let outcome = mako_pruefung::nb::evaluate_anmeldung_teilnahme(
            &self.malo,
            stillgelegt,
            None,
            mako_fristen::berlin_date(received),
            delivery,
            mako_pruefung::NetzCheckConfig::default().holiday_calendar,
        );
        ensure!(
            matches!(outcome,Some(mako_pruefung::NbEntscheidung::Reject(ref r)) if
            r.antwort.tree == self.decision.tree && r.antwort.antwortcode == self.decision.code && r.pruefschritt == self.pruefschritt),
            "decision differs from supplied facts"
        );
        Ok((message, received))
    }
}

fn hex_digest(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[must_use = "a refused occurrence cannot be imported"]
pub(super) async fn prepare(
    workspace: &Path,
    path: &Path,
    parties: Arc<MpIdRegistry>,
    scope: &str,
) -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_PACKET + 1) as u64)
        .read_to_end(&mut input)?;
    ensure!(input.len() <= MAX_PACKET, "packet exceeds 2 MiB");
    let packet: Packet = serde_json::from_slice(&input)?;
    ensure!(packet.scope == scope, "packet belongs to another scope");
    let (message, received) = packet.validate(&parties)?;
    let canonical = serde_json::to_vec(&packet)?;
    let digest = Sha256::digest(&canonical);
    if path.try_exists()? {
        let db = open(path).await?;
        let prior = db.kv_get(META, "packet_sha256").await;
        db.close().await?;
        ensure!(
            prior?.as_deref() == Some(digest.as_ref()),
            "workspace already contains a different occurrence or revision"
        );
        return Ok(());
    }
    let stage = tempfile::Builder::new()
        .prefix(".prepare-")
        .tempdir_in(workspace)?;
    let store = Arc::new(open(stage.path()).await?);
    let result = prepare_store(
        store.clone(),
        parties,
        &packet,
        &message,
        received,
        &canonical,
    )
    .await;
    store.close().await?;
    result?;
    std::fs::rename(stage.path(), path).context("publish prepared occurrence")?;
    Ok(())
}

#[must_use = "the staged database may only be published after complete preparation"]
async fn prepare_store(
    store: Arc<mako_engine::store_slatedb::SlateDbStore>,
    parties: Arc<MpIdRegistry>,
    packet: &Packet,
    message: &edi_energy::AnyMessage,
    received: OffsetDateTime,
    canonical: &[u8],
) -> Result<()> {
    let tenant_id = TenantId::from_tenant(TENANT);
    let snapshots = store.as_snapshot_store();
    let dispatcher = EdifactIngestDispatcher::new(store.clone(), snapshots.clone(), 100, tenant_id)
        .with_mp_id_registry(parties.clone());
    let ctx = super::engine(&store);
    let pid = message.detect_pruefidentifikator()?.as_u32();
    let workflow =
        crate::edifact_api::resolve_workflow(ctx.pid_router(), &parties, pid, &packet.nb)
            .context("NB deployment has no route for the request")?;
    let outcome = dispatcher
        .dispatch(message, workflow, pid, received)
        .await?;
    ensure!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "NB workflow refused the occurrence"
    );

    let state = CommandsApiState {
        tenant_id,
        tenant: TENANT.to_owned(),
        parties,
        configured_marktrollen: vec!["NB".into()],
        max_body_bytes: MAX_PACKET,
        cedar: Arc::new(crate::cedar_authz::CedarAuthorizer::unauthenticated()?),
        store: store.clone(),
        snapshot_store: snapshots,
        malo_cache: Arc::new(SlateDbMaloCache::new((*store).clone(), tenant_id)),
        maloid_result_cache: MaloIdentResultCache::new((*store).clone()),
        snapshot_interval: 100,
        marktd_client: None,
    };
    let answer = json!({
        "malo_id": packet.malo,
        "antwort_code": packet.decision.code,
        "antwort_codeliste": packet.decision.tree,
    });
    dispatch_command(
        &state,
        Akteur::System,
        "gpke.lieferbeginn.ablehnen",
        &answer,
    )
    .await
    .map_err(|_| anyhow::anyhow!("NB command refused"))?;
    store
        .kv_put(META, "received_at", packet.received_at.as_bytes())
        .await?;
    store.kv_put(META, "nb", packet.nb.as_bytes()).await?;
    store.kv_put(META, "input", canonical).await?;
    store
        .kv_put(META, "packet_sha256", &Sha256::digest(canonical))
        .await?;
    Ok(())
}
