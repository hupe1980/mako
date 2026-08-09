//! Sealing at rest, and the erasure it makes possible.
//!
//! A run's journal records carry what the step was given. For our specialists
//! that is a MaLo, an address, a customer name — personal data under GDPR, in a
//! chain that is append-only by construction. Deleting a record is not available
//! and would not help: the hash chain commits to it.
//!
//! agentplane's answer is envelope encryption. With a key ring wired,
//! `RuntimeBuilder::build` seals the journal, the case state, the buffered
//! events and the task proposals: the chain commits to *ciphertext*, an auditor
//! with no keys still verifies it, and destroying a case's wrapping key destroys
//! the plaintext in every copy at once — live store, replica, and every backup
//! ever taken. `tests/plane_golden_run.rs` asserts both halves against a real
//! store.
//!
//! ## Why Vault and not a key in the config
//!
//! The wrapping key is created inside Vault's transit engine and never leaves
//! it, so erasure is something mako *asks for* and cannot undo by holding a copy
//! — which is the difference between crypto-shredding and hoping nobody kept the
//! bytes. A transit key without `deletion_allowed` refuses the destroy loudly
//! rather than reporting a success that did not happen.
//!
//! ## Why an unsealed plane is allowed to start, loudly
//!
//! A development plane with no Vault must be able to run. What it must not do is
//! run *quietly*: an operator who never sees the warning discovers at the first
//! erasure request that the chain holds plaintext no key destroys. So the
//! absence is a startup warning that names the consequence, and
//! `[keyring] required = true` turns it into a refusal for deployments that
//! carry the duty.

use std::sync::Arc;

use agentplane::keyring::{KeyRing, VaultTransit};
use secrecy::{ExposeSecret as _, SecretString};

use crate::config::KeyringConfig;

/// Build the key ring a deployment configured, if any.
///
/// `token` is the resolved Vault token — the config holds the `env:VAR`
/// placeholder, and sending that literally would authenticate as nobody.
///
/// Returns `None` when no key ring is configured *and* the deployment has not
/// declared one required — the journal is then written in the clear and the
/// caller is expected to say so.
///
/// # Errors
///
/// When `required = true` and no Vault is configured, or when the Vault client
/// cannot be constructed. Both are deployment faults: a plane that believes it
/// seals and does not is worse than one that refuses to start.
pub fn build(
    cfg: Option<&KeyringConfig>,
    token: Option<&SecretString>,
) -> Result<Option<Arc<dyn KeyRing>>, String> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };
    let Some(vault) = cfg.vault.as_ref() else {
        if cfg.required {
            return Err(
                "[keyring] required = true but no [keyring.vault] is configured — \
                 the journal would hold personal data no key can destroy"
                    .to_owned(),
            );
        }
        return Ok(None);
    };

    let token = token
        .map(|t| t.expose_secret().to_owned())
        .unwrap_or_else(|| vault.token.expose_secret().to_owned());
    let ring = VaultTransit::new(&vault.address, &vault.mount, token)
        .map_err(|e| format!("build the Vault transit key ring at {}: {e}", vault.address))?;

    Ok(Some(Arc::new(ring) as Arc<dyn KeyRing>))
}

/// What an operator is told when the plane runs unsealed.
///
/// A constant rather than an inline string so the same sentence appears in the
/// log, in the README and in the test that pins it — a warning that drifts from
/// its documentation is one people stop reading.
pub const UNSEALED_WARNING: &str = "no [keyring.vault] configured — journal records, case state, buffered events and \
     task proposals are written in the clear. Personal data written into the chain \
     cannot then be erased on request (GDPR Art. 17), because the chain is append-only \
     and there is no wrapping key to destroy. Configure Vault transit, or keep personal \
     data out of every event payload this plane admits.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VaultConfig;
    use secrecy::SecretString;

    fn vault_cfg() -> KeyringConfig {
        KeyringConfig {
            required: true,
            vault: Some(VaultConfig {
                address: "https://vault.internal:8200".to_owned(),
                mount: "transit".to_owned(),
                token: SecretString::from("s.token".to_owned()),
            }),
        }
    }

    /// No configuration at all is an unsealed plane, not an error.
    #[test]
    fn an_absent_key_ring_is_permitted() {
        assert!(build(None, None).expect("no key ring is allowed").is_none());
    }

    /// Declaring a key ring required and configuring none refuses to start.
    ///
    /// The failure this prevents is the one that cannot be repaired afterwards:
    /// a plane that ran for a week unsealed has journal records no later
    /// configuration change can reach.
    #[test]
    fn required_without_a_vault_is_a_startup_failure() {
        let cfg = KeyringConfig {
            required: true,
            vault: None,
        };
        let err = build(Some(&cfg), None).expect_err("required with no vault must refuse");
        assert!(err.contains("required"), "the error explains itself: {err}");
    }

    /// A configured Vault produces a key ring without reaching the network.
    ///
    /// Construction is offline on purpose: a plane must fail on an unreachable
    /// Vault at its first sealed write, where the failure names the run, rather
    /// than at boot where it names nothing.
    #[test]
    fn a_configured_vault_builds_a_key_ring() {
        assert!(
            build(Some(&vault_cfg()), None).expect("builds").is_some(),
            "a configured Vault must produce a key ring"
        );
    }
}
