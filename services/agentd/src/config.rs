//! `agentd.toml` — multi-agent configuration.
//!
//! What is *not* here is the point: no prompts, no models, no tool grants, no
//! ceilings. Those live in each specialist's manifest, where they are covered by
//! the digest a reviewer approves. This file is deployment wiring — where the
//! journal lives, which providers exist, which MCP servers to reach, who may
//! approve, and what the plane is allowed to do.

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
    /// How this plane is reached from outside, for the A2A Agent Cards.
    ///
    /// A card states where an agent is; that is deployment wiring and not a
    /// property of the agent, which is why it is here and not in a manifest.
    #[serde(default = "default_public_base_url")]
    pub public_base_url: String,
    /// Maximum concurrent agent runs (default: 20).
    #[serde(default = "default_max_sessions")]
    pub max_sessions: u32,

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

    /// CloudEvent types that trigger agent runs.
    #[serde(default = "default_triggers")]
    pub trigger_event_types: Vec<String>,

    /// Audit CloudEvent webhook (marktd event_log).
    pub audit_webhook_url: Option<String>,
    /// HMAC-SHA256 secret for signing outbound audit webhook events ("sha256=" prefix).
    /// When set, every `de.agent.decision.made` POST carries an `X-Mako-Signature` header.
    pub audit_hmac_secret: Option<SecretString>,

    /// HMAC-SHA256 secret for verifying **inbound** CloudEvent webhook signatures.
    /// When set, `POST /webhook` rejects requests where the `X-Mako-Signature` header
    /// does not match `sha256=HMAC(secret, body)`.
    /// When absent, all inbound webhooks are accepted (dev mode only — log a WARNING).
    pub inbound_hmac_secret: Option<SecretString>,

    /// Wall-clock ceiling in seconds for one event's whole fan-out (default: 300).
    ///
    /// A single run is already bounded by its manifest's `budgets`; this bounds
    /// the *set* of runs one event triggers, so a slow specialist cannot hold a
    /// concurrency permit indefinitely. Exceeding it abandons the wait, not the
    /// work — each run's effects stay journaled and resumable.
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
/// One backend holds all five. The journal is the § 147 AO / GoBD record for the
/// agent layer — every model call, tool call and human decision is written here
/// before it happens — so it belongs on durable storage, not a container's
/// ephemeral filesystem.
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
    /// Marktrolle's agents (§ 9 EnWG).
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
fn default_max_sessions() -> u32 {
    20
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
fn default_triggers() -> Vec<String> {
    vec![
        mako_events::mako::PROCESS_FAILED.into(),
        mako_events::invoic::RECEIPT_DISPUTED.into(),
        mako_events::accounting::MAHNUNG_ISSUED.into(),
        mako_events::eeg::ANLAGE_FOERDERUNG_AUSLAUFEND.into(),
    ]
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
    pub audit_hmac_secret: Option<SecretString>,
    pub inbound_hmac_secret: Option<SecretString>,
    /// The Vault token, when a key ring is configured.
    pub vault_token: Option<SecretString>,
    /// The journal DSN, when the backend is Postgres.
    pub journal_url: Option<String>,
}

impl AgentdConfig {
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
    /// reference for the bind address, so an in-place `&mut` resolution cannot
    /// work — an earlier version tried to unwrap the `Arc` and would have
    /// panicked on every start.
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
