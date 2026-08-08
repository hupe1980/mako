//! `agentd.toml` — multi-agent configuration.

use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentdConfig {
    /// Where the agentplane journal lives.
    ///
    /// Every model call, tool call and human decision is recorded here, so this
    /// is the § 147 AO / GoBD record for the agent layer — it belongs on durable
    /// storage, not a container's ephemeral filesystem.
    #[serde(default = "default_journal_path")]
    pub journal_path: String,
    /// HTTP listen port (default: 9580).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Operator tenant identifier.
    pub tenant: String,
    /// Maximum concurrent agent sessions (default: 20).
    #[serde(default = "default_max_sessions")]
    pub max_sessions: u32,

    /// Named LLM provider configurations.
    ///
    /// The key is the name a manifest's `spec.models` refers to, so a manifest
    /// declaring `provider: anthropic` needs an `[providers.anthropic]` block.
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

    /// CloudEvent types that trigger agent sessions.
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

    /// OIDC configuration for authenticating `POST /api/v1/run`.
    /// When absent, all manual run requests are accepted (dev mode — logs a WARNING).
    pub oidc: Option<mako_service::oidc::OidcConfig>,
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
    /// Backend: "openai" | "anthropic" | "bedrock"
    pub backend: String,
    /// API base URL (optional override).
    pub api_base: Option<String>,
    /// API key / secret (never logged).
    /// Use `"env:OPENAI_API_KEY"` form in TOML to read from environment.
    #[serde(default)]
    pub api_key: SecretString,
    /// AWS region (Bedrock only).
    pub aws_region: Option<String>,
    /// AWS access key ID (Bedrock only; prefer IAM roles in production).
    pub aws_access_key_id: Option<String>,
    /// AWS secret access key (Bedrock only; never logged).
    pub aws_secret_access_key: Option<SecretString>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("backend", &self.backend)
            .field("api_base", &self.api_base)
            .field("api_key", &"[REDACTED]")
            .field("aws_region", &self.aws_region)
            .field("aws_access_key_id", &self.aws_access_key_id)
            .field("aws_secret_access_key", &"[REDACTED]")
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
fn default_max_sessions() -> u32 {
    20
}
fn default_session_timeout_secs() -> u64 {
    300
}
fn default_triggers() -> Vec<String> {
    vec![
        mako_events::mako::PROCESS_FAILED.into(),
        mako_events::invoic::RECEIPT_DISPUTED.into(),
        mako_events::accounting::MAHNUNG_ISSUED.into(),
        mako_events::eeg::ANLAGE_FOERDERUNG_AUSLAUFEND.into(),
    ]
}

impl AgentdConfig {
    /// Resolve every `env:VAR` indirection in secret-bearing fields.
    ///
    /// Config values like `api_key = "env:OPENAI_API_KEY"` are placeholders,
    /// not credentials — a config that ships them unresolved sends the
    /// literal string as the bearer token and fails as a 401 against the
    /// provider. Call once right after loading, before anything clones a
    /// provider config.
    ///
    /// # Errors
    ///
    /// Returns an error naming the missing environment variable.
    pub fn resolve_env_indirection(&mut self) -> anyhow::Result<()> {
        use mako_service::config::{resolve_env, resolve_env_secret};
        use secrecy::ExposeSecret as _;

        for (name, p) in &mut self.providers {
            p.api_key = resolve_env_secret(p.api_key.expose_secret())
                .map_err(|e| anyhow::anyhow!("providers.{name}.api_key: {e}"))?;
            if let Some(sk) = &p.aws_secret_access_key {
                p.aws_secret_access_key =
                    Some(resolve_env_secret(sk.expose_secret()).map_err(|e| {
                        anyhow::anyhow!("providers.{name}.aws_secret_access_key: {e}")
                    })?);
            }
            if let Some(ak) = &p.aws_access_key_id {
                p.aws_access_key_id = Some(
                    resolve_env(ak)
                        .map_err(|e| anyhow::anyhow!("providers.{name}.aws_access_key_id: {e}"))?,
                );
            }
        }
        self.mcp_api_key = resolve_env_secret(self.mcp_api_key.expose_secret())
            .map_err(|e| anyhow::anyhow!("mcp_api_key: {e}"))?;
        if let Some(s) = &self.audit_hmac_secret {
            self.audit_hmac_secret = Some(
                resolve_env_secret(s.expose_secret())
                    .map_err(|e| anyhow::anyhow!("audit_hmac_secret: {e}"))?,
            );
        }
        if let Some(s) = &self.inbound_hmac_secret {
            self.inbound_hmac_secret = Some(
                resolve_env_secret(s.expose_secret())
                    .map_err(|e| anyhow::anyhow!("inbound_hmac_secret: {e}"))?,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod env_resolution_tests {
    use secrecy::ExposeSecret as _;

    fn cfg_with_key(key: &str) -> super::AgentdConfig {
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
        let mut cfg = cfg_with_key("sk-plain");
        cfg.resolve_env_indirection().expect("resolve");
        assert_eq!(cfg.providers["openai"].api_key.expose_secret(), "sk-plain");

        let mut cfg2 = cfg_with_key("env:AGENTD_TEST_KEY_DOES_NOT_EXIST");
        let err = cfg2.resolve_env_indirection().unwrap_err().to_string();
        assert!(err.contains("AGENTD_TEST_KEY_DOES_NOT_EXIST"), "{err}");
    }
}
