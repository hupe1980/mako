//! `agentd.toml` — multi-agent configuration.
//!
//! What is *not* here is the point: no prompts, no models, no tool grants, no
//! ceilings. Those live in each specialist's manifest, where they are covered by
//! the digest a reviewer approves. This file is deployment wiring — where the
//! journal lives, which providers exist, which MCP servers to reach, who may
//! approve, and what the plane is allowed to do.
//!
//! **Which events wake an agent is not here either.** That is the manifests'
//! subscription table, and `plane::Router::accepts` is the only admission
//! filter: a second event-type list in config that nothing reconciles with the
//! manifests is a mute switch.
//!
//! ## The three blocks that decide what the record is worth
//!
//! | Block | Absent | Present |
//! |---|---|---|
//! | `[keyring]` | personal data written into an append-only chain no key can erase | crypto-shredding reaches every copy at once |
//! | `[attestation]` | the chain says *what happened* and nothing says *which workload wrote it* | every record carries a signature an auditor can check |
//! | `[witness]` | tamper-*evident* to whoever holds a prior checkpoint, nothing to a regulator holding none | an independent party refuses to cosign a rewritten history |
//!
//! Each is optional, each starts the plane with a warning naming what is lost,
//! and each can declare itself `required`. They compose in that order:
//! `[witness]` without `[attestation]` is refused at startup, because a witness
//! recognises a log by the signature on its checkpoint.

use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentdConfig {
    /// Where the agentplane journal and case layer live.
    #[serde(default)]
    pub journal: JournalConfig,
    /// HTTP listen port (default: 9580).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Operator tenant identifier.
    pub tenant: String,
    /// Permit unauthenticated API and unsigned inbound webhooks for local development.
    ///
    /// Production startup fails closed without OIDC and inbound Standard
    /// Webhooks verification unless this flag is set explicitly.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
    /// How this plane is reached from outside, for the A2A Agent Cards.
    ///
    /// A card states where an agent is; that is deployment wiring and not a
    /// property of the agent, which is why it is here and not in a manifest.
    #[serde(default = "default_public_base_url")]
    pub public_base_url: String,
    /// Per-tenant ceilings on the plane, accounted in the store rather than in
    /// this process — a counter held here would fail *open* the moment a second
    /// instance started, which is the topology the Postgres backend exists for.
    #[serde(default)]
    pub quota: QuotaConfig,

    /// Named LLM provider configurations.
    ///
    /// The key is the name a manifest's `spec.models` refers to, so a manifest
    /// declaring `provider: anthropic` needs a `[providers.anthropic]` block.
    pub providers: HashMap<String, ProviderConfig>,

    /// Which built-in specialists this deployment activates.
    ///
    /// ```toml
    /// [bundled_agents]
    /// enable_all = true
    /// ```
    ///
    /// The prompt, model pair, tool grants and ceilings are **not** configurable
    /// here — they live in the specialist's manifest, where they are covered by
    /// the digest a reviewer approves. Changing a model is a manifest edit and a
    /// version bump, which is the point: an operator cannot silently move a
    /// regulated decision onto a different model.
    #[serde(default)]
    pub bundled_agents: BundledAgentsConfig,

    /// MCP server endpoints (name → base URL).
    pub mcp_servers: HashMap<String, String>,
    /// Bearer token for MCP authentication.
    /// Use `"env:AGENTD_MCP_API_KEY"` to defer to environment; never log this value.
    pub mcp_api_key: SecretString,

    /// Audit CloudEvent webhook (marktd event_log).
    pub audit_webhook_url: Option<String>,
    /// Secret signing every `de.agent.decision.made` delivery.
    ///
    /// [Standard Webhooks](https://www.standardwebhooks.com/), signed by
    /// agentplane's `push::Destination` — the same scheme
    /// `mako_service::webhook` signs every other mako outbound with, so
    /// `mako_service::webhook::verify_request` accepts an agentd delivery like
    /// any other. `mako-service`'s own test pins the header names and the
    /// signed-payload shape against agentplane's, because two implementations
    /// of one spec is where a wire contract drifts.
    ///
    /// The signature covers the message id and the timestamp, not just the
    /// body, so a captured delivery cannot be replayed once the tolerance window
    /// has passed.
    pub audit_hmac_secret: Option<SecretString>,

    /// The retiring signing secret, presented **beside** `audit_hmac_secret`
    /// during a rotation.
    ///
    /// Standard Webhooks makes `webhook-signature` a space-separated list, so a
    /// delivery can carry both and a receiver holding either verifies. That is
    /// what turns a key rollover into something each receiver does at its own
    /// pace instead of a flag day where every delivery fails until both sides
    /// restart together. Remove it once every receiver has the new key.
    ///
    /// Setting this without `audit_hmac_secret` is a startup failure: "also"
    /// with no primary key signs nothing.
    pub audit_hmac_secret_previous: Option<SecretString>,

    /// How long a claimed admission key is kept, in days. Absent means forever.
    ///
    /// `POST /webhook` admits at most one run per `(CloudEvent source, id,
    /// specialist)`, and the claim is what makes that true across instances and
    /// restarts. **Retiring a key reopens the door it closed**, so there is
    /// deliberately no default: absent this, keys are kept, which is the only
    /// setting that cannot admit a duplicate on a timer.
    ///
    /// When a deployment does set it, the window must exceed every emitter's
    /// retry horizon. mako's own fan-out gives up after five attempts over
    /// roughly two and a half hours — but a dead-lettered delivery can be
    /// replayed by an operator days later, and that replay is the *same*
    /// message and must still be answered with the original run. **30 days** is
    /// the recommended figure for that reason, not for the retry schedule.
    ///
    /// A window under a day is refused at startup: it is short enough to expire
    /// a key while its own emitter is still retrying, which is the failure the
    /// key exists to prevent, delivered on a schedule.
    pub admission_retention_days: Option<u32>,

    /// HMAC-SHA256 secret for verifying **inbound** CloudEvent webhook signatures.
    /// When set, `POST /webhook` verifies the Standard Webhooks headers through
    /// `mako_service::webhook::verify_request`, which also refuses a stale
    /// `webhook-timestamp`.
    /// When absent, all inbound webhooks are accepted (dev mode only — logs a WARNING).
    pub inbound_hmac_secret: Option<SecretString>,

    /// Wall-clock ceiling in seconds for a **manual** run's fan-out (default: 300).
    ///
    /// `POST /api/v1/run` is the one door that waits for an answer, so it is the
    /// one that needs a ceiling on the wait. Exceeding it abandons the wait, not
    /// the work: each run's effects stay journaled, and a run this process stops
    /// executing is taken over and resumed by the sweeper's recovery pass once
    /// its lease lapses.
    ///
    /// `POST /webhook` does not use it. That door admits durably and returns
    /// `202` without waiting for anything, so there is no wait to bound.
    #[serde(default = "default_session_timeout_secs")]
    pub session_timeout_secs: u64,

    /// How often the sweeper ticks, in seconds (default: 60).
    ///
    /// The tick that warns on approaching deadlines, breaches the ones that
    /// passed, expires overdue approvals and wakes sleeping runs. It bounds how
    /// *late* a warning is, not when a breach is recorded — the breach instant
    /// was resolved and journaled when the obligation was registered.
    #[serde(default = "default_sweep_interval_secs")]
    pub sweep_interval_secs: u64,

    /// OIDC configuration.
    ///
    /// Authenticates `POST /api/v1/run` and — with no dev-mode relaxation — the
    /// whole oversight surface. When absent, manual runs are accepted with a
    /// warning and **the worklist is not mounted at all**: an approval by an
    /// unauthenticated caller is a forged signature on a regulated dispatch, not
    /// a convenience.
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// The Cedar policy set. Omit to use mako's own, embedded in the binary.
    #[serde(default)]
    pub policy: PolicyConfig,

    /// Envelope encryption for everything the plane writes down.
    pub keyring: Option<KeyringConfig>,

    /// The workload identity every journal record is signed as.
    ///
    /// Omit and records are written unsigned, with a startup warning that names
    /// the consequence: the chain still says *what happened* and nothing says
    /// *which workload wrote it*, and no checkpoint can be submitted to a
    /// witness — a witness recognises a log by its signature.
    pub attestation: Option<AttestationConfig>,

    /// Independent parties that cosign this plane's checkpoints.
    ///
    /// Requires `[attestation]`. Omit and the journal is tamper-*evident* to
    /// anyone holding a prior checkpoint, and nothing at all to a regulator
    /// holding none — because a checkpoint that never leaves the operator's
    /// store is exactly as trustworthy as the operator.
    pub witness: Option<WitnessConfig>,
}

impl mako_service::service::ServiceConfig for AgentdConfig {
    /// No `[database]`: agentd owns no SQL schema. Its durable state is the
    /// agentplane journal, which is either an embedded redb file or a Postgres
    /// database agentplane connects to and migrates itself.
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        None
    }

    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }
}

// ── Journal ────────────────────────────────────────────────────────────────

/// Where the journal, the cases, the tasks, the timers and the events live.
///
/// One backend holds every state seam. The journal is durable evidence for the
/// agent layer — every model call, tool call and human decision is written here
/// before it happens — so it belongs on durable storage, not a container's
/// ephemeral filesystem. Tax-record compliance additionally requires the
/// deployment's retention, readability and audit-access policy.
#[derive(Debug, Deserialize)]
#[serde(tag = "backend", rename_all = "lowercase", deny_unknown_fields)]
pub enum JournalConfig {
    /// The embedded backend: one file, pure Rust, ACID. Single-instance
    /// deployments.
    Redb {
        #[serde(default = "default_journal_path")]
        path: String,
    },
    /// The shared backend: several agentd instances on one database, where
    /// fencing and exactly-once are arbitrated by Postgres rather than by hoping
    /// the writers agree. Use `"env:AGENTD_JOURNAL_URL"` to keep the DSN out of
    /// the file.
    Postgres { url: String },
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self::Redb {
            path: default_journal_path(),
        }
    }
}

// ── Policy ─────────────────────────────────────────────────────────────────

/// Which rules govern this plane.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    /// A Cedar policy file that **replaces** the embedded rules.
    ///
    /// Replaces rather than extends: Cedar allows on any matching permit, so a
    /// least-privilege file layered over a broader one cannot narrow anything.
    /// An operator who mounts a file gets exactly their rules.
    pub path: Option<String>,
}

// ── Keyring ────────────────────────────────────────────────────────────────

/// Envelope encryption for journal records, case state, events and tasks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyringConfig {
    /// Refuse to start unless a key ring is configured.
    ///
    /// For any deployment whose events can carry personal data: an unsealed
    /// plane writes it into an append-only chain that no later configuration
    /// change can reach.
    #[serde(default)]
    pub required: bool,
    /// HashiCorp Vault's transit engine. The wrapping key is created inside
    /// Vault and never leaves it, so erasure is something mako asks for and
    /// cannot undo.
    pub vault: Option<VaultConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultConfig {
    /// e.g. `https://vault.internal:8200`.
    pub address: String,
    /// Mount path of the transit engine (usually `transit`).
    #[serde(default = "default_vault_mount")]
    pub mount: String,
    /// A token that may use the transit mount. Prefer `"env:VAULT_TOKEN"`.
    pub token: SecretString,
}

// ── BundledAgentsConfig ────────────────────────────────────────────────────

/// Which compiled-in specialists this deployment activates.
///
/// Activation is the whole of the operator's control. What an activated
/// specialist *does* — its procedure, its model pair, the tools it may call and
/// the ceilings it runs under — is declared in its manifest and covered by that
/// manifest's digest.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledAgentsConfig {
    /// Activate every specialist compiled into this binary.
    ///
    /// When `true`, `enable` is ignored. Note that a role-scoped build contains
    /// only its own role's specialists, so this never activates another
    /// Marktrolle's agents (§§ 6a, 7a EnWG).
    #[serde(default)]
    pub enable_all: bool,

    /// Activate specific specialists by name.
    ///
    /// Example: `enable = ["eeg-compliance-agent", "billing-anomaly-agent"]`
    ///
    /// A name that matches no compiled specialist is a startup failure, not a
    /// silently inactive agent — the usual cause is a name that exists only in
    /// another role's build.
    #[serde(default)]
    pub enable: Vec<String>,
}

// ── Quota ──────────────────────────────────────────────────────────────────

/// Per-tenant ceilings, accounted in the store rather than in this process.
///
/// Every field is optional and every absent field is *unbounded*. There is
/// deliberately no default: the right number is the model concurrency a
/// deployment has actually bought, and a ceiling believed to bound something it
/// does not is worse than none.
///
/// What is *not* unbounded absent this block: every run is still held to its
/// manifest's mandatory `budgets`. This bounds how many of them run at once.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaConfig {
    /// Runs this tenant may have **executing** at once.
    ///
    /// A slot is taken at admission and given back when the run seals, fails or
    /// **suspends** — a run parked on a four-eyes approval costs a database row,
    /// not a slot, so a tenant waiting on a hundred reviews can still start
    /// work. A resume is not gated either: refusing to resume would strand a run
    /// waiting on something that has now happened.
    ///
    /// Exceeding it refuses the admission as back-pressure, which `POST
    /// /webhook` answers `429` — so an at-least-once emitter retries rather than
    /// treating the message as impossible.
    pub max_concurrent_runs: Option<u32>,

    /// Model tokens this tenant may spend in one calendar month (UTC).
    ///
    /// Checked at admission, not mid-run, so a run already executing when the
    /// ceiling is crossed finishes. The overshoot is therefore bounded and
    /// computable — at most `max_concurrent_runs` times the largest per-run
    /// budget — rather than unknown.
    pub max_tokens_per_month: Option<u64>,
}

// ── Attestation ────────────────────────────────────────────────────────────

/// Who wrote each journal record.
///
/// The key comes from the deployment and cannot be minted here: a plane that
/// generated its own identity would produce records that look attested and
/// prove nothing, because the party being audited chose the key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationConfig {
    /// Refuse to start unless a signer is configured.
    ///
    /// For any deployment whose agent decisions feed a booking or a dispatched
    /// market message: unsigned records cannot be attributed after the fact, and
    /// a chain written unattested for a week cannot be signed retroactively.
    #[serde(default)]
    pub required: bool,

    /// The identity that lands on every record.
    ///
    /// Give it the workload's real name — a SPIFFE ID if there is one. *"Some
    /// key signed this"* is a much weaker statement than *"this workload signed
    /// this"*, and the second is what an audit is asking.
    pub key_id: String,

    /// A 32-byte Ed25519 seed in standard base64. Prefer `"env:VAR"`.
    ///
    /// `openssl rand -base64 32` produces one. Publish the matching public key
    /// to whoever verifies these records — without it an auditor can check that
    /// the history is internally consistent but not who produced it.
    pub seed: Option<SecretString>,
}

/// Hand-written: this type holds a signing seed, and a derived `Debug` would put
/// it in whatever line printed the config.
impl std::fmt::Debug for AttestationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttestationConfig")
            .field("required", &self.required)
            .field("key_id", &self.key_id)
            .field("seed", &self.seed.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

// ── Witness ────────────────────────────────────────────────────────────────

/// Independent parties that cosign this plane's checkpoints.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessConfig {
    /// How many cosignatures a checkpoint must gather.
    ///
    /// A trust decision only a deployment can make: one independent witness
    /// rules out a silent rewrite by mako alone, three rule out collusion with
    /// any single witness. Zero is refused — a quorum of nothing is witnessing
    /// that is off, spelled as if it were on — and a quorum above the number of
    /// witnesses configured is refused too, because a bar that can never be met
    /// is a permanent alarm about the configuration rather than about the log.
    pub quorum: usize,

    /// How often a checkpoint is submitted, in seconds (default: 3600).
    ///
    /// Slow on purpose. This bounds how *stale* the witnessed checkpoint is, not
    /// whether the log is sound — one witnessed an hour late proves the same
    /// extension — and a witness is somebody else's server.
    #[serde(default = "default_witness_interval_secs")]
    pub interval_secs: u64,

    /// Where to submit, and whose cosignature to believe.
    pub witnesses: Vec<WitnessPeer>,
}

/// One witness.
///
/// The public key is not optional and not derived from the URL. Without it a
/// client could only record that *something* answered `200`, and the whole
/// argument for witnessing — that an independent party observed this log —
/// would rest on a status code.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessPeer {
    /// The witness's key name, as it appears on the signature line.
    ///
    /// No spaces or control characters: the line is space-delimited, so a name
    /// containing one serialises fine and reads back as a different name or an
    /// extra signature nobody wrote.
    pub name: String,
    /// The submission prefix, without `/add-checkpoint`.
    pub url: String,
    /// The witness's 32-byte Ed25519 public key, in standard base64.
    pub public_key: String,
}

// ── Provider config ────────────────────────────────────────────────────────

/// LLM provider configuration.
///
/// Intentionally does not derive `Debug` to prevent secrets appearing in logs.
/// A custom `Debug` impl redacts all secret fields.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Which driver: `openai` · `anthropic` · `gemini` · `chat-completions` ·
    /// `bedrock`.
    ///
    /// The **key** of the `[providers.…]` table is the name a manifest refers
    /// to; this is the wire it speaks. The two are separate on purpose: a
    /// deployment may register `[providers.anthropic]` backed by
    /// `chat-completions` against its own vLLM, and no manifest changes.
    pub backend: String,
    /// API base URL. Required for `chat-completions` (there is no default
    /// endpoint for your own server); an override elsewhere — an Azure
    /// deployment, a gateway, a recording proxy.
    pub api_base: Option<String>,
    /// API key / secret (never logged).
    /// Use `"env:OPENAI_API_KEY"` form in TOML to read from environment.
    #[serde(default)]
    pub api_key: SecretString,
    /// AWS region, for `bedrock`. Credentials come from the standard AWS chain
    /// (IAM role, environment, profile) and deliberately not from this file.
    pub aws_region: Option<String>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("backend", &self.backend)
            .field("api_base", &self.api_base)
            .field("api_key", &"[REDACTED]")
            .field("aws_region", &self.aws_region)
            .finish()
    }
}

// ── Defaults ──────────────────────────────────────────────────────────────

fn default_journal_path() -> String {
    "/var/lib/agentd/journal.redb".to_owned()
}
fn default_port() -> u16 {
    9580
}
fn default_public_base_url() -> String {
    "http://localhost:9580".to_owned()
}
fn default_session_timeout_secs() -> u64 {
    300
}
fn default_sweep_interval_secs() -> u64 {
    60
}
fn default_vault_mount() -> String {
    "transit".to_owned()
}
fn default_witness_interval_secs() -> u64 {
    3600
}
/// Every credential the config points at, with its `env:VAR` indirection
/// resolved.
///
/// Separate from [`AgentdConfig`] rather than resolved in place, and the reason
/// is the runner's ownership: `mako_service::run` hands `build` an
/// `Arc<Self::Config>` and keeps its own reference for the bind address, so
/// there is no `&mut` to resolve into and no way to take the config back. Every
/// consumer therefore reads its credential from here.
///
/// No `Debug`: this type is nothing but secrets.
pub struct Secrets {
    /// Providers with `api_key` resolved, keyed as in `[providers.*]`.
    pub providers: HashMap<String, ProviderConfig>,
    pub mcp_api_key: SecretString,
    /// Secret signing outbound audit deliveries (Standard Webhooks).
    pub audit_hmac_secret: Option<SecretString>,
    /// The retiring secret, signed with beside the current one mid-rotation.
    pub audit_hmac_secret_previous: Option<SecretString>,
    pub inbound_hmac_secret: Option<SecretString>,
    /// The Vault token, when a key ring is configured.
    pub vault_token: Option<SecretString>,
    /// The Ed25519 seed every journal record is signed with, when attestation
    /// is configured.
    pub signing_seed: Option<SecretString>,
    /// The journal DSN, when the backend is Postgres.
    pub journal_url: Option<String>,
}

impl AgentdConfig {
    /// Validate security and worker invariants before opening any transport.
    ///
    /// # Errors
    ///
    /// Returns an error for an implicitly insecure deployment, unsigned audit
    /// delivery, or an interval that would panic its detached worker.
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.allow_insecure_no_auth && self.oidc.is_none() {
            anyhow::bail!(
                "refusing to start without [oidc]: manual runs and agent inventory would be unauthenticated. Configure [oidc] or set allow_insecure_no_auth = true (dev only)."
            );
        }
        if !self.allow_insecure_no_auth && self.inbound_hmac_secret.is_none() {
            anyhow::bail!(
                "refusing to start without inbound_hmac_secret: unsigned CloudEvents could spend model budget and open operator tasks. Configure inbound_hmac_secret or set allow_insecure_no_auth = true (dev only)."
            );
        }
        if self.audit_webhook_url.is_some() && self.audit_hmac_secret.is_none() {
            anyhow::bail!(
                "audit_webhook_url requires audit_hmac_secret so decision deliveries are authenticated"
            );
        }
        anyhow::ensure!(
            self.sweep_interval_secs > 0,
            "sweep_interval_secs must be greater than zero"
        );
        if let Some(witness) = &self.witness {
            anyhow::ensure!(
                witness.interval_secs > 0,
                "witness.interval_secs must be greater than zero"
            );
        }
        Ok(())
    }

    /// Resolve every `env:VAR` indirection in secret-bearing fields.
    ///
    /// Config values like `api_key = "env:OPENAI_API_KEY"` are placeholders,
    /// not credentials — a config that ships them unresolved sends the literal
    /// string as the bearer token and fails as a 401 against the provider.
    /// Resolve once at startup and pass [`Secrets`] to whatever needs them.
    ///
    /// # Errors
    ///
    /// Returns an error naming the missing environment variable.
    pub fn resolve_secrets(&self) -> anyhow::Result<Secrets> {
        use mako_service::config::{resolve_env, resolve_env_secret};
        use secrecy::ExposeSecret as _;

        let mut providers = HashMap::with_capacity(self.providers.len());
        for (name, p) in &self.providers {
            let mut resolved = p.clone();
            resolved.api_key = resolve_env_secret(p.api_key.expose_secret())
                .map_err(|e| anyhow::anyhow!("providers.{name}.api_key: {e}"))?;
            providers.insert(name.clone(), resolved);
        }

        let secret = |field: &str, s: &SecretString| -> anyhow::Result<SecretString> {
            resolve_env_secret(s.expose_secret()).map_err(|e| anyhow::anyhow!("{field}: {e}"))
        };

        Ok(Secrets {
            providers,
            mcp_api_key: secret("mcp_api_key", &self.mcp_api_key)?,
            audit_hmac_secret: self
                .audit_hmac_secret
                .as_ref()
                .map(|s| secret("audit_hmac_secret", s))
                .transpose()?,
            audit_hmac_secret_previous: self
                .audit_hmac_secret_previous
                .as_ref()
                .map(|s| secret("audit_hmac_secret_previous", s))
                .transpose()?,
            inbound_hmac_secret: self
                .inbound_hmac_secret
                .as_ref()
                .map(|s| secret("inbound_hmac_secret", s))
                .transpose()?,
            vault_token: self
                .keyring
                .as_ref()
                .and_then(|k| k.vault.as_ref())
                .map(|v| secret("keyring.vault.token", &v.token))
                .transpose()?,
            signing_seed: self
                .attestation
                .as_ref()
                .and_then(|a| a.seed.as_ref())
                .map(|seed| secret("attestation.seed", seed))
                .transpose()?,
            journal_url: match &self.journal {
                JournalConfig::Postgres { url } => {
                    Some(resolve_env(url).map_err(|e| anyhow::anyhow!("journal.url: {e}"))?)
                }
                JournalConfig::Redb { .. } => None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;

    fn cfg_with_key(key: &str) -> AgentdConfig {
        serde_json::from_value(serde_json::json!({
            "tenant": "9900000000001",
            "mcp_servers": {},
            "mcp_api_key": "test-key",
            "providers": {
                "openai": { "backend": "openai", "api_key": key }
            }
        }))
        .expect("valid config")
    }

    /// Non-placeholder values pass through untouched; a missing environment
    /// variable is a descriptive startup error, not a literal token sent to
    /// the provider.
    #[test]
    fn passthrough_and_missing_var_error() {
        let secrets = cfg_with_key("sk-plain").resolve_secrets().expect("resolve");
        assert_eq!(
            secrets.providers["openai"].api_key.expose_secret(),
            "sk-plain"
        );

        // `map_or_else` rather than `unwrap_err`: reporting the failure would
        // need `Debug` on `Secrets`, and `Secrets` is nothing but secrets.
        let err = cfg_with_key("env:AGENTD_TEST_KEY_DOES_NOT_EXIST")
            .resolve_secrets()
            .map_or_else(|e| e.to_string(), |_| String::new());
        assert!(err.contains("AGENTD_TEST_KEY_DOES_NOT_EXIST"), "{err}");
    }

    /// Resolution reads the config without needing to own it.
    ///
    /// The runner hands `Daemon::build` an `Arc<Config>` and keeps its own
    /// reference for the bind address, so the `Arc` is always shared at this
    /// point and an in-place `&mut` resolution cannot work.
    #[test]
    fn secrets_resolve_from_a_shared_config() {
        let shared = std::sync::Arc::new(cfg_with_key("sk-plain"));
        let _also_held = std::sync::Arc::clone(&shared);
        let secrets = shared
            .resolve_secrets()
            .expect("resolve from behind an Arc");
        assert_eq!(secrets.mcp_api_key.expose_secret(), "test-key");
    }

    /// A config that says nothing about its journal gets the embedded one.
    #[test]
    fn the_journal_defaults_to_the_embedded_backend() {
        let cfg = cfg_with_key("sk");
        assert!(
            matches!(cfg.journal, JournalConfig::Redb { .. }),
            "a deployment that names no backend runs on redb"
        );
    }

    #[test]
    fn production_security_and_worker_intervals_fail_closed() {
        let mut cfg = cfg_with_key("sk");
        let error = cfg.validate().expect_err("OIDC is required").to_string();
        assert!(error.contains("[oidc]"), "{error}");

        cfg.allow_insecure_no_auth = true;
        cfg.sweep_interval_secs = 0;
        let error = cfg
            .validate()
            .expect_err("a zero sweep interval would panic the worker")
            .to_string();
        assert!(error.contains("sweep_interval_secs"), "{error}");
    }

    /// The Postgres backend is chosen by name and carries its DSN.
    #[test]
    fn a_postgres_journal_parses() {
        let cfg: AgentdConfig = toml::from_str(
            r#"
tenant = "9900357000004"
mcp_api_key = "k"
[mcp_servers]
[providers.anthropic]
backend = "anthropic"
api_key = "k"
[journal]
backend = "postgres"
url = "postgres://localhost/agentd"
"#,
        )
        .expect("parses");
        match cfg.journal {
            JournalConfig::Postgres { url } => assert!(url.contains("agentd")),
            JournalConfig::Redb { .. } => panic!("named backend was ignored"),
        }
    }

    /// **The documented example config parses.**
    ///
    /// TOML binds a bare key to the most recent table header, so a top-level
    /// key written below `[policy]` is read as `policy.<key>` — and every type
    /// here is `deny_unknown_fields`, so the deployment refuses to start. The
    /// README's example had seven keys past a table header, including
    /// `mcp_api_key`; copying it produced a daemon that would not boot, and
    /// nothing checked the block because a fenced snippet is prose.
    ///
    /// It is read out of the README rather than restated, which is the whole
    /// point: a second copy would be the one that stays correct while the
    /// documented one rots.
    #[test]
    fn the_readme_example_config_parses() {
        let readme = include_str!("../README.md");
        let block = readme
            .split_once("```toml\n# agentd.toml")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("\n```"))
            .map(|(block, _)| block)
            .expect("the README documents an agentd.toml");

        let cfg: AgentdConfig = toml::from_str(block)
            .expect("the documented example config must be one an operator can copy");
        cfg.validate()
            .expect("the documented production config must pass startup security validation");

        // Spot-check the keys most likely to be swallowed by a table above
        // them — the ones that were.
        assert_eq!(cfg.tenant, "9900357000004");
        assert!(
            cfg.quota.max_concurrent_runs.is_none(),
            "the documented example states no ceiling, and absent means unbounded"
        );
        assert!(cfg.inbound_hmac_secret.is_some(), "inbound_hmac_secret");
        assert!(cfg.audit_hmac_secret.is_some(), "audit_hmac_secret");
        assert!(
            cfg.mcp_servers.contains_key("makod"),
            "the MCP table survived: {:?}",
            cfg.mcp_servers.keys().collect::<Vec<_>>()
        );
        let missing: Vec<_> = crate::plane::tools::servers_named_in_grants()
            .into_iter()
            .filter(|server| !cfg.mcp_servers.contains_key(server))
            .collect();
        assert!(
            missing.is_empty(),
            "enable_all requires every granted MCP server in the example: {missing:?}"
        );
        assert!(
            cfg.policy.path.is_none(),
            "[policy] took a key that is not its own"
        );
    }

    /// A key ring declared required with no Vault parses — and is refused later,
    /// at startup, where the message can explain the consequence.
    #[test]
    fn a_keyring_block_parses() {
        let cfg: AgentdConfig = toml::from_str(
            r#"
tenant = "9900357000004"
mcp_api_key = "k"
[mcp_servers]
[providers.anthropic]
backend = "anthropic"
api_key = "k"
[keyring]
required = true
[keyring.vault]
address = "https://vault.internal:8200"
token = "s.dev"
"#,
        )
        .expect("parses");
        let keyring = cfg.keyring.expect("declared");
        assert!(keyring.required);
        assert_eq!(keyring.vault.expect("vault").mount, "transit");
    }
}
