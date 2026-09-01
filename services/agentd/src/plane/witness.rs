//! Somebody else who saw the log grow.
//!
//! Everything else in the journal protects the record from **edits**: the hash
//! chain detects a rewritten record, the Merkle root detects a removed run, and
//! [`attest`](super::attest) says which workload wrote each one. None of it
//! detects mako showing a *different history to each auditor*, because both
//! histories can be internally perfect and whoever controls the store controls
//! every input to that check.
//!
//! That is the stated limit of the audit-evidence argument: a checkpoint that never
//! leaves the operator's store is exactly as trustworthy as the operator.
//!
//! A witness breaks the symmetry by being somebody else. It remembers the last
//! checkpoint it saw for this log and cosigns a new one only when the new one
//! **provably extends** it, so two divergent histories cannot both be cosigned.
//! A split view stops being invisible and becomes either a witness that refuses
//! or two cosignatures that contradict each other and can be shown to anyone.
//!
//! ## Off the run path, on purpose
//!
//! Witnessing is retrospective evidence gathered after sealing. A run whose
//! witnesses are unreachable finished long ago, and making the plane's
//! availability depend on a third party would be the wrong trade for evidence
//! that is read after the fact — a Betriebsprüfung is not a request/response
//! deadline. What a shortfall gets instead is a report that cannot be mistaken
//! for success.
//!
//! ## Where a cosignature is kept
//!
//! At the witness, not here. Storing a copy beside the log would put the
//! evidence back under the control of the party it is evidence about, which is
//! the exact symmetry this module exists to break. An auditor asks the witness
//! what it last cosigned for `mako/agentd/<tenant>` and compares that with what
//! mako hands them; the whole value is that those are two different sources.
//! What this module keeps locally is a log line naming the size and root that
//! were cosigned, so an operator can see the mechanism working.
//!
//! ## A witness you host yourself proves nothing about you
//!
//! Worth stating in mako's own tree, because the configuration makes it easy to
//! point this at a URL inside the same cluster and read the resulting green log
//! line as evidence. It is not. The counterparty has to be one that would not
//! cooperate in a rewrite: a `transparency-dev` omniwitness instance, an
//! auditor's own, a second market participant's.

use std::sync::Arc;
use std::time::Duration;

use agentplane::core::CheckpointSigner;
use agentplane::journal::{
    HttpWitness, JournalStore, NoteSignature, TrustedWitness, Witness, WitnessQuorum,
    cosign_quorum, key_id,
};
use agentplane::policy::Ed25519Signer;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::WitnessConfig;

/// C2SP `signed-note`'s algorithm byte for a plain Ed25519 signature.
///
/// **Not `0x04`**, which names a `tlog-cosignature` — the timestamped
/// construction a *witness* produces. The log's own signature on its checkpoint
/// is the plain kind, and deriving the key id with the wrong byte yields four
/// bytes no conforming witness matches, so every submission comes back as an
/// unknown key. Named rather than inlined, because the two are one digit apart.
const ED25519_NOTE_SIGNATURE: u8 = 0x01;

/// Submits this plane's checkpoints to the witnesses an operator configured.
///
/// Holds the peers rather than built clients because a [`HttpWitness`] carries
/// **the log's signature over one specific checkpoint** — the witness needs it
/// to recognise which log is speaking — so the client is a per-submission
/// object, not a long-lived one.
pub struct Submitter {
    journal: Arc<dyn JournalStore>,
    signer: Arc<Ed25519Signer>,
    peers: Vec<Peer>,
    quorum: WitnessQuorum,
    /// The last size this plane got a quorum for, so an unchanged checkpoint is
    /// not resubmitted every tick. Nothing depends on it being durable: a
    /// restart resubmits once, which a witness answers by cosigning the same
    /// checkpoint again.
    last_witnessed: u64,
}

impl std::fmt::Debug for Submitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Submitter")
            .field("peers", &self.peers.len())
            .field("quorum", &self.quorum.required())
            .finish_non_exhaustive()
    }
}

/// One configured witness: where to submit, and whose cosignature to believe.
struct Peer {
    prefix: String,
    trusted: TrustedWitness,
}

/// Build the submitter a deployment configured, if any.
///
/// # Errors
///
/// When `[witness]` is configured without `[attestation]` — a witness
/// recognises a log by its own signature, so there is nothing to submit — when
/// the quorum exceeds the number of witnesses configured (a bar that can never
/// be met reads as evidence and produces a permanent shortfall), when a public
/// key is not 32 bytes of standard base64, or when a witness name is not usable
/// on a signature line.
pub fn build(
    cfg: Option<&WitnessConfig>,
    journal: &Arc<dyn JournalStore>,
    signer: Option<&Arc<Ed25519Signer>>,
) -> Result<Option<Submitter>, String> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };

    let Some(signer) = signer else {
        return Err(
            "[witness] is configured but [attestation] is not — a witness recognises a log \
             by the signature on its checkpoint, so an unattested plane has nothing a \
             witness would accept. Configure [attestation] first, or remove [witness] \
             rather than running a submitter whose every request is refused"
                .to_owned(),
        );
    };

    if cfg.witnesses.is_empty() {
        return Err(
            "[witness] names no witnesses — an empty list is witnessing that is off, \
             spelled as if it were on. Omit the section instead"
                .to_owned(),
        );
    }

    let quorum = WitnessQuorum::of(cfg.quorum).map_err(|e| format!("[witness] quorum: {e}"))?;
    if cfg.quorum > cfg.witnesses.len() {
        return Err(format!(
            "[witness] quorum = {} but only {} witnesses are configured, so the bar can \
             never be met and every tick reports a shortfall — which is a permanent alarm \
             about the configuration rather than about the log, and an alarm that is always \
             on is one nobody reads",
            cfg.quorum,
            cfg.witnesses.len()
        ));
    }

    let mut peers = Vec::with_capacity(cfg.witnesses.len());
    for peer in &cfg.witnesses {
        // The name is structure on a space-delimited signature line, not a
        // label. Refused here, where whoever wrote the configuration is present
        // to read the message, rather than as an unparseable note.
        agentplane::journal::SignedNote::validate_name(&peer.name)
            .map_err(|e| format!("[witness] '{}': {e}", peer.name))?;
        peers.push(Peer {
            prefix: peer.url.clone(),
            trusted: TrustedWitness::ed25519(peer.name.clone(), decode_key(peer)?),
        });
    }

    Ok(Some(Submitter {
        journal: Arc::clone(journal),
        signer: Arc::clone(signer),
        peers,
        quorum,
        last_witnessed: 0,
    }))
}

/// A witness's 32-byte Ed25519 public key, as the operator registered it.
fn decode_key(peer: &crate::config::WitnessPeer) -> Result<[u8; 32], String> {
    use base64::Engine as _;

    let complaint = |what: &str| {
        format!(
            "[witness] '{}': public_key {what}. A witness publishes a 32-byte Ed25519 key \
             in standard base64; without the right one a cosignature is a 200 with a \
             base64 string in it, and counting those toward a quorum is the failure \
             witnessing exists to rule out",
            peer.name
        )
    };

    base64::engine::general_purpose::STANDARD
        .decode(peer.public_key.trim())
        .map_err(|e| complaint(&format!("is not standard base64 ({e})")))?
        .try_into()
        .map_err(|v: Vec<u8>| complaint(&format!("decoded to {} bytes rather than 32", v.len())))
}

impl Submitter {
    /// Submit the current checkpoint once, and report what came back.
    ///
    /// # Errors
    ///
    /// Only on a **store** failure — reading the checkpoint, or building a
    /// consistency proof. A witness failing is never an error here; that is
    /// what the quorum outcome reports.
    pub async fn submit_once(&mut self) -> Result<(), String> {
        let checkpoint = self
            .journal
            .checkpoint()
            .await
            .map_err(|e| format!("read the journal checkpoint: {e}"))?;

        if checkpoint.size == 0 {
            debug!("no sealed run to witness yet");
            return Ok(());
        }
        if checkpoint.size == self.last_witnessed {
            debug!(
                size = checkpoint.size,
                "checkpoint unchanged since the last quorum"
            );
            return Ok(());
        }

        // The signature the witness uses to recognise *which log* is speaking.
        // Over the note bytes, not over a digest of them: C2SP `signed-note`
        // specifies pure Ed25519 over the note text, and signing a hash of it
        // produces 64 bytes that verify against nothing any witness computes.
        let note = checkpoint.to_note();
        let signature = CheckpointSigner::sign(self.signer.as_ref(), note.as_bytes())
            .await
            .map_err(|e| format!("sign the checkpoint note: {e}"))?;
        let log_signature = NoteSignature {
            // The C2SP convention: a log's key name is its origin, so a witness
            // that has one entry per origin can find the key without a second
            // identifier that could disagree with the note it is attached to.
            name: checkpoint.origin.clone(),
            key_id: key_id(
                &checkpoint.origin,
                ED25519_NOTE_SIGNATURE,
                &self.signer.verifying_key(),
            ),
            signature,
        };

        let mut witnesses: Vec<Arc<dyn Witness>> = Vec::with_capacity(self.peers.len());
        for peer in &self.peers {
            let client = HttpWitness::new(
                &peer.prefix,
                log_signature.clone(),
                vec![peer.trusted.clone()],
            )
            .map_err(|e| format!("witness '{}': {e}", peer.trusted.name()))?;
            witnesses.push(Arc::new(client) as Arc<dyn Witness>);
        }

        let outcome = cosign_quorum(self.journal.as_ref(), &checkpoint, &witnesses, self.quorum)
            .await
            .map_err(|e| format!("build a consistency proof: {e}"))?;

        // An integrity refusal is reported **even when the quorum was met**. A
        // fork report from one witness among five cosigners is the alarm, not
        // noise: the four may simply never have seen the history the fifth
        // remembers, and only one of those histories is ours.
        for (index, refusal) in &outcome.integrity {
            error!(
                witness = %self.peers[*index].trusted.name(),
                size = checkpoint.size,
                error = %refusal,
                "a witness remembers a different history for this log — either the journal \
                 was rewritten or two histories exist under one origin. This is the event \
                 witnessing exists to detect; do not clear it by removing the witness"
            );
        }
        // Routine failures are the witness being unreachable or still stale, and
        // they self-heal on a later tick. Reported at debug so a flapping peer
        // does not train an operator to skim the line above.
        for (index, routine) in &outcome.routine {
            debug!(
                witness = %self.peers[*index].trusted.name(),
                error = %routine,
                "witness submission did not complete; retrying on the next tick"
            );
        }

        if outcome.met() {
            self.last_witnessed = checkpoint.size;
            info!(
                size = checkpoint.size,
                root = %checkpoint.root.to_hex(),
                cosignatures = outcome.cosignatures.len(),
                required = self.quorum.required(),
                origin = %checkpoint.origin,
                "checkpoint cosigned — ask a witness what it last saw for this origin \
                 rather than taking this line as the evidence"
            );
        } else {
            warn!(
                size = checkpoint.size,
                cosignatures = outcome.cosignatures.len(),
                required = self.quorum.required(),
                shortfall = outcome.shortfall(),
                "checkpoint did not reach its witness quorum — the log grew and no \
                 independent party has vouched for it yet"
            );
        }
        Ok(())
    }
}

/// Submit checkpoints on a slow tick until shutdown.
///
/// `every` is the submission interval. It bounds how *stale* the witnessed
/// checkpoint is, not whether the log is sound — a checkpoint witnessed an hour
/// late still proves the same extension. Slow on purpose: a witness is somebody
/// else's server, and a per-minute submission of a log that grows a few times an
/// hour is load on a volunteer for no extra evidence.
pub fn spawn(mut submitter: Submitter, every: Duration, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    info!("agent witness submitter stopping");
                    return;
                }
                _ = ticker.tick() => {}
            }
            if let Err(e) = submitter.submit_once().await {
                error!(error = %e, "witness submission could not be attempted");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WitnessPeer;
    use base64::Engine as _;

    fn journal() -> Arc<dyn JournalStore> {
        let tenant = agentplane::core::TenantId::new("9900357000004").expect("a key scope");
        Arc::new(
            agentplane::store::RedbStore::open_in_memory()
                .expect("store")
                .origin(super::super::attest::ORIGIN)
                .for_tenant(tenant),
        )
    }

    fn signer() -> Arc<Ed25519Signer> {
        Arc::new(Ed25519Signer::new("spiffe://mako/agentd", &[3_u8; 32]))
    }

    fn peer(name: &str) -> WitnessPeer {
        WitnessPeer {
            name: name.to_owned(),
            url: "https://witness.example.org".to_owned(),
            public_key: base64::engine::general_purpose::STANDARD.encode([1_u8; 32].as_slice()),
        }
    }

    fn cfg(quorum: usize, peers: Vec<WitnessPeer>) -> WitnessConfig {
        WitnessConfig {
            quorum,
            interval_secs: 3600,
            witnesses: peers,
        }
    }

    /// No configuration at all is a plane nobody witnesses, not an error.
    #[test]
    fn an_absent_witness_section_is_permitted() {
        assert!(
            build(None, &journal(), Some(&signer()))
                .expect("no witness is allowed")
                .is_none()
        );
    }

    /// Witnessing without attestation is refused, not attempted.
    ///
    /// The failure this prevents is the quietest one available: the submitter
    /// starts, every request is refused because the checkpoint carries no
    /// signature the witness can attribute, and the refusals are classified as
    /// *routine* — so the log fills with retries and the operator's dashboard
    /// says the witness is merely flaky.
    #[test]
    fn a_witness_without_an_attested_log_is_a_startup_failure() {
        let err = build(Some(&cfg(1, vec![peer("w")])), &journal(), None)
            .expect_err("a witness needs a signed checkpoint");
        assert!(err.contains("[attestation]"), "{err}");
    }

    /// A quorum nothing can satisfy is refused at startup.
    #[test]
    fn a_quorum_above_the_witness_count_is_refused() {
        let err = build(Some(&cfg(2, vec![peer("w")])), &journal(), Some(&signer()))
            .expect_err("2 of 1 can never be met");
        assert!(err.contains("quorum"), "{err}");
    }

    /// Zero is witnessing that is off, spelled as if it were on.
    #[test]
    fn a_quorum_of_zero_is_refused() {
        let err = build(Some(&cfg(0, vec![peer("w")])), &journal(), Some(&signer()))
            .expect_err("a quorum of nothing");
        assert!(err.contains("quorum"), "{err}");
    }

    /// A key that is not 32 bytes is refused rather than trusted.
    #[test]
    fn a_malformed_public_key_is_refused() {
        let mut p = peer("w");
        p.public_key = "c2hvcnQ=".to_owned();
        let err =
            build(Some(&cfg(1, vec![p])), &journal(), Some(&signer())).expect_err("a short key");
        assert!(err.contains("public_key"), "{err}");
    }

    /// A witness name with a space would serialise into a different name.
    #[test]
    fn a_witness_name_that_breaks_the_signature_line_is_refused() {
        let err = build(
            Some(&cfg(1, vec![peer("witness one")])),
            &journal(),
            Some(&signer()),
        )
        .expect_err("a space is structure on the signature line");
        assert!(err.contains("witness one"), "{err}");
    }

    /// An empty log is not submitted.
    ///
    /// A first submission establishes the origin at a witness and every later
    /// checkpoint is held to it, so there is nothing to gain from registering a
    /// log with no runs in it — and one incoherent size-0 submission poisons an
    /// origin permanently.
    #[tokio::test]
    async fn an_empty_log_is_not_submitted() {
        let mut submitter = build(Some(&cfg(1, vec![peer("w")])), &journal(), Some(&signer()))
            .expect("builds")
            .expect("a submitter");
        // No witness is reachable at the configured URL; reaching one at all
        // would already be the failure this asserts against.
        submitter
            .submit_once()
            .await
            .expect("an empty log is a quiet no-op, not a store error");
        assert_eq!(submitter.last_witnessed, 0);
    }
}
