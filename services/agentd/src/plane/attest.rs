//! Who wrote each record, and which log it belongs to.
//!
//! The journal is hash-chained, so a rewritten record is detectable and a
//! removed run breaks the Merkle root. Neither of those says **who wrote it**.
//! An [`Attestation`](agentplane::core::Attestation) does: every record carries
//! a signature over its chain hash and the key id of the workload that produced
//! it, so an auditor holding the public key can tell mako's plane from anything
//! else that reached the same database.
//!
//! ## Both seams, from one key
//!
//! Two settings take a signer and they are one method name apart:
//! [`signing_as`](agentplane::store::RedbStore::signing_as) on the **store**
//! covers records, and `RuntimeBuilder::signing_as` covers the plane's outward
//! claims to a tool server. A signer given only to the runtime leaves every
//! record unsigned. mako gives both the same key, so an auditor reading a record
//! and a server reading a provenance block see one workload.
//!
//! ## The key comes from outside, and cannot be minted here
//!
//! [`Ed25519Signer`] is constructed from a seed the deployment supplies and has
//! no constructor that generates one — deliberately, upstream: *"a plane that
//! mints its own identity produces records that look attested and prove
//! nothing, because the party being audited chose the key."* So an unattested
//! plane is allowed to start, loudly, exactly as an unsealed one is; what it
//! must not do is start quietly.
//!
//! ## The origin is a constant, and that is the point
//!
//! [`ORIGIN`] names this plane's Merkle log, and the store appends the tenant to
//! it — so one operator's checkpoints read `mako/agentd/9900357000004`. It is a
//! constant rather than a config key because a witness holds **every later
//! checkpoint to the first one it saw under a name**: changing an origin is not
//! a rename, it is a new log with no history, and the old name is poisoned for
//! good. A value that must never drift belongs in a diff a reviewer reads, not
//! in a file an operator edits.

use std::sync::Arc;

use agentplane::policy::Ed25519Signer;
use secrecy::{ExposeSecret as _, SecretString};

use crate::config::AttestationConfig;

/// Names this plane's Merkle log, before the store appends the tenant.
///
/// Stable forever. See the module docs for why it is not configurable.
pub const ORIGIN: &str = "mako/agentd";

/// How many bytes an Ed25519 seed is.
const SEED_LEN: usize = 32;

/// What an operator is told when the plane writes unattested records.
///
/// A constant so the same sentence appears in the log, in the README and in the
/// test that pins it — a warning that drifts from its documentation is one
/// people stop reading.
pub const UNATTESTED_WARNING: &str = "no [attestation] configured — journal records are written without a signature, so \
     the chain says what happened and nothing says which workload wrote it. An auditor \
     can then check that the history is internally consistent but not that mako's plane \
     produced it, and no checkpoint can be submitted to a witness (a witness recognises \
    a log by its signature). Configure [attestation], or accept that audit evidence \
    stops at an unattributed tamper-evident history.";

/// Build the record signer a deployment configured, if any.
///
/// `seed` is the resolved 32-byte Ed25519 seed — the config holds the `env:VAR`
/// placeholder, and signing with that literal would produce an identity nobody
/// can verify against the key the operator published.
///
/// Returns `None` when no attestation is configured *and* the deployment has not
/// declared one required. The caller is expected to say so out loud.
///
/// # Errors
///
/// When `required = true` and nothing is configured, or when the seed is not
/// 32 bytes of standard base64. Both are deployment faults: a plane that
/// believes it attests and does not is worse than one that refuses to start.
pub fn build(
    cfg: Option<&AttestationConfig>,
    seed: Option<&SecretString>,
) -> Result<Option<Arc<Ed25519Signer>>, String> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };
    let Some(seed) = seed.or(cfg.seed.as_ref()) else {
        if cfg.required {
            return Err(
                "[attestation] required = true but no seed is configured — every record \
                 would be written unsigned"
                    .to_owned(),
            );
        }
        return Ok(None);
    };

    // The name that lands on every record. An empty one would attest to
    // nobody, which is the same evidence as not attesting while reading in a
    // review as if it were more.
    if cfg.key_id.trim().is_empty() {
        return Err(
            "[attestation] key_id is empty — \"some key signed this\" is a much weaker \
             statement than \"this workload signed this\", and the second is what an audit \
             is asking. Give it the workload's real name (a SPIFFE ID if there is one)"
                .to_owned(),
        );
    }

    let bytes = decode_seed(seed.expose_secret())?;
    Ok(Some(Arc::new(Ed25519Signer::new(
        cfg.key_id.trim(),
        &bytes,
    ))))
}

/// Decode a base64 Ed25519 seed, refusing anything that is not exactly 32 bytes.
///
/// One encoding, not two. Accepting hex as well would mean a 64-character
/// string is ambiguous — valid hex *and* valid base64 — and the two decode to
/// different keys, so a deployment could silently attest under an identity
/// nobody published. The diagnostic names the command that produces the right
/// thing, which is the part an operator actually needs.
fn decode_seed(raw: &str) -> Result<[u8; SEED_LEN], String> {
    use base64::Engine as _;

    let complaint = |what: &str| {
        format!(
            "[attestation] seed {what}. It must be a 32-byte Ed25519 seed in standard \
             base64 — `openssl rand -base64 32` produces one. Publish the matching public \
             key to whoever verifies these records"
        )
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| complaint(&format!("is not standard base64 ({e})")))?;

    decoded.try_into().map_err(|v: Vec<u8>| {
        complaint(&format!(
            "decoded to {} bytes rather than {SEED_LEN}",
            v.len()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentplane::core::Signer as _;
    use base64::Engine as _;

    fn seed(byte: u8) -> SecretString {
        SecretString::from(
            base64::engine::general_purpose::STANDARD.encode([byte; SEED_LEN].as_slice()),
        )
    }

    fn cfg() -> AttestationConfig {
        AttestationConfig {
            required: false,
            key_id: "spiffe://mako/agentd".to_owned(),
            seed: Some(seed(7)),
        }
    }

    /// No configuration at all is an unattested plane, not an error.
    #[test]
    fn an_absent_signer_is_permitted() {
        assert!(build(None, None).expect("no signer is allowed").is_none());
    }

    /// Declaring attestation required and configuring no seed refuses to start.
    ///
    /// The failure this prevents cannot be repaired afterwards: a plane that ran
    /// for a week unattested has records no later configuration change can sign.
    #[test]
    fn required_without_a_seed_is_a_startup_failure() {
        let cfg = AttestationConfig {
            required: true,
            key_id: "spiffe://mako/agentd".to_owned(),
            seed: None,
        };
        let err = build(Some(&cfg), None).expect_err("required with no seed must refuse");
        assert!(err.contains("required"), "the error explains itself: {err}");
    }

    /// A configured seed produces a signer that actually signs.
    #[test]
    fn a_configured_seed_builds_a_signer() {
        let signer = build(Some(&cfg()), None)
            .expect("builds")
            .expect("a signer");
        assert_eq!(signer.key_id().as_str(), "spiffe://mako/agentd");
        assert_eq!(
            signer
                .sign(&agentplane::core::Digest::of(b"a record"))
                .len(),
            64,
            "an Ed25519 signature is 64 bytes"
        );
    }

    /// The seed is the deployment's, so a wrong one is a diagnostic and never a
    /// key this process invented to get past the error.
    #[test]
    fn a_malformed_seed_is_refused_rather_than_replaced() {
        for (raw, why) in [
            ("not base64 at all!!", "not base64"),
            ("c2hvcnQ=", "too short"),
            ("", "empty"),
        ] {
            let mut cfg = cfg();
            cfg.seed = Some(SecretString::from(raw.to_owned()));
            let err = build(Some(&cfg), None).expect_err(why);
            assert!(
                err.contains("openssl rand -base64 32"),
                "the diagnostic names how to produce a usable seed: {err}"
            );
        }
    }

    /// An anonymous attestation is refused: it reads as more evidence than it is.
    #[test]
    fn an_empty_key_id_is_refused() {
        let mut cfg = cfg();
        cfg.key_id = "   ".to_owned();
        let err = build(Some(&cfg), None).expect_err("an unnamed identity");
        assert!(err.contains("key_id"), "{err}");
    }

    /// The resolved secret wins over the placeholder the config still holds.
    ///
    /// `Secrets` is where `env:VAR` indirection lands; signing with the literal
    /// `env:AGENTD_SIGNING_SEED` would fail to decode, and if it ever decoded it
    /// would attest under a key nobody published.
    #[test]
    fn the_resolved_seed_is_the_one_used() {
        let mut cfg = cfg();
        cfg.seed = Some(SecretString::from("env:AGENTD_SIGNING_SEED".to_owned()));
        let resolved = seed(9);
        assert!(
            build(Some(&cfg), Some(&resolved)).is_ok(),
            "the resolved seed must be preferred to the placeholder"
        );
    }

    /// The origin is a valid signed-note key name.
    ///
    /// It becomes the name on the checkpoint's signature line, which is
    /// space-delimited: a space or a control character there serialises fine and
    /// reads back as a different name or an extra line. Checked here rather than
    /// discovered by a witness answering 403.
    #[test]
    fn the_origin_is_a_usable_note_name() {
        agentplane::journal::SignedNote::validate_name(ORIGIN)
            .expect("the log origin must be a usable signed-note key name");
        assert!(
            !ORIGIN.is_empty() && !ORIGIN.ends_with('/'),
            "the store appends `/{{tenant}}`"
        );
    }
}
