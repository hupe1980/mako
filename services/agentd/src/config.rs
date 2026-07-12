//! `agentd.toml` — multi-agent configuration.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentdConfig {
    /// HTTP listen port (default: 9580).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Operator tenant identifier.
    pub tenant: String,
    /// Maximum concurrent agent sessions (default: 20).
    #[serde(default = "default_max_sessions")]
    pub max_sessions: u32,

    /// Named LLM provider configurations.
    /// Reference these by name in `[[agents]]`.
    pub providers: HashMap<String, ProviderConfig>,

    /// Orchestrator configuration.
    pub orchestrator: OrchestratorConfig,

    /// Specialized agent definitions.
    #[serde(default)]
    pub agents: Vec<AgentConfig>,

    /// MCP server endpoints (name → base URL).
    pub mcp_servers: HashMap<String, String>,
    /// Bearer token for MCP authentication.
    pub mcp_api_key: String,

    /// RAG knowledge base.
    #[serde(default)]
    pub rag: RagConfig,

    /// CloudEvent types that trigger agent sessions.
    #[serde(default = "default_triggers")]
    pub trigger_event_types: Vec<String>,

    /// Audit CloudEvent webhook (marktd event_log).
    pub audit_webhook_url: Option<String>,
    pub audit_hmac_secret: Option<String>,
}

// ── Provider config ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Backend: "openai" | "anthropic" | "bedrock"
    pub backend: String,
    /// API base URL (optional override).
    pub api_base: Option<String>,
    /// API key / secret (set via env override).
    #[serde(default)]
    pub api_key: String,
    /// AWS region (Bedrock only).
    pub aws_region: Option<String>,
    /// AWS access key ID (Bedrock only; prefer IAM roles in production).
    pub aws_access_key_id: Option<String>,
    /// AWS secret access key (Bedrock only).
    pub aws_secret_access_key: Option<String>,
}

// ── Orchestrator config ────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorConfig {
    /// Named provider to use for the orchestrator.
    pub provider: String,
    /// LLM model identifier.
    pub model: String,
    /// Maximum orchestrator turns before forcing specialist delegation.
    #[serde(default = "default_orch_turns")]
    pub max_turns: u32,
    /// Custom system prompt prefix for the orchestrator.
    pub system_prompt: Option<String>,
}

// ── Specialist agent config ────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Unique agent name (used for routing and handoff tool names).
    pub name: String,
    /// One-line specialty description (shown in orchestrator + handoff tools).
    pub specialty: String,
    /// Named provider for this agent.
    pub provider: String,
    /// LLM model identifier.
    pub model: String,
    /// Maximum ReAct turns per session.
    #[serde(default = "default_agent_turns")]
    pub max_turns: u32,
    /// MCP server names this agent can access (subset of `[mcp_servers]`).
    /// Empty = access to all servers.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    /// CloudEvent type glob patterns for direct routing (bypasses orchestrator).
    /// Example: `["de.eeg.*", "de.invoic.receipt.disputed"]`
    #[serde(default)]
    pub trigger_patterns: Vec<String>,
    /// Custom system prompt prefix.
    pub system_prompt: Option<String>,
    /// Enable RAG context injection for this agent.
    #[serde(default = "default_true")]
    pub use_rag: bool,
}

// ── RAG config ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RagConfig {
    /// Enable RAG (default: false — requires sources to be configured).
    #[serde(default)]
    pub enabled: bool,
    /// LanceDB storage URI.
    /// - `/var/lib/agentd/rag` — local filesystem
    /// - `s3://my-bucket/agentd/rag` — AWS S3 (env AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY)
    /// - `gs://bucket/prefix` — Google Cloud Storage
    /// - `az://container/prefix` — Azure Blob Storage
    #[serde(default = "default_rag_db")]
    pub storage_uri: String,
    /// Embedding vector dimension (default: 1536 for text-embedding-3-small).
    #[serde(default = "default_embed_dim")]
    pub embedding_dim: i32,
    /// Named provider to use for embeddings (must support `embed()`).
    /// Defaults to orchestrator provider.
    pub embedding_provider: Option<String>,
    /// Embedding model (e.g. `text-embedding-3-small`, `amazon.titan-embed-text-v2:0`).
    #[serde(default = "default_embed_model")]
    pub embedding_model: String,
    /// Number of chunks to retrieve per query (default: 5).
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Text chunk size in characters (default: 512).
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    /// Chunk overlap in characters (default: 64).
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    /// Document sources to index at startup.
    #[serde(default)]
    pub sources: Vec<RagSource>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RagSource {
    /// Logical name for this source.
    pub name: String,
    /// Path to a file (Markdown, plain text, or PDF — PDF requires `pdfium`).
    pub path: String,
}

// ── Defaults ──────────────────────────────────────────────────────────────

fn default_port() -> u16 {
    9580
}
fn default_max_sessions() -> u32 {
    20
}
fn default_orch_turns() -> u32 {
    5
}
fn default_agent_turns() -> u32 {
    20
}
fn default_true() -> bool {
    true
}
fn default_rag_db() -> String {
    "/var/lib/agentd/rag".into()
}
fn default_embed_dim() -> i32 {
    1536
}
fn default_embed_model() -> String {
    "text-embedding-3-small".into()
}
fn default_top_k() -> usize {
    5
}
fn default_chunk_size() -> usize {
    512
}
fn default_chunk_overlap() -> usize {
    64
}
fn default_triggers() -> Vec<String> {
    vec![
        "de.mako.process.escalated".into(),
        "de.invoic.receipt.disputed".into(),
        "de.accounting.mahnung.issued".into(),
        "de.eeg.anlage.foerderung_auslaufend".into(),
    ]
}
