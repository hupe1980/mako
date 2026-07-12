//! LLM provider abstraction.
//!
//! Supports OpenAI-compatible, Anthropic, and AWS Bedrock backends.

pub mod anthropic;
pub mod bedrock;
pub mod openai;

use std::{future::Future, pin::Pin, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ProviderConfig;

// ── Shared types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,   // "system" | "user" | "assistant" | "tool"
    pub content: Value, // String or array of content blocks
}

impl Message {
    pub fn system(s: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Value::String(s.into()),
        }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Value::String(s.into()),
        }
    }
    pub fn assistant_text(s: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: Value::String(s.into()),
        }
    }
    pub fn tool_result(tool_id: &str, result: Value) -> Self {
        Self {
            role: "tool".into(),
            content: serde_json::json!({ "tool_use_id": tool_id, "content": result.to_string() }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug)]
pub enum CompletionResult {
    ToolCalls(Vec<ToolCall>),
    Text(String),
    /// Agent requests handoff to another named agent.
    Handoff {
        to_agent: String,
        reason: String,
        context: Value,
    },
}

#[derive(Debug, Clone)]
pub struct CompletionConfig {
    pub model: String,
    pub max_tokens: u32,
}

// ── Provider trait ─────────────────────────────────────────────────────────

/// Boxed future returned by [`LlmProvider`] methods.
pub type LlmFut<'a, T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'a>>;

pub trait LlmProvider: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Complete a conversation turn.
    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        cfg: &CompletionConfig,
    ) -> LlmFut<'_, CompletionResult>;

    /// Generate embedding vectors for a list of texts.
    /// Returns one vector per input string.
    fn embed(&self, model: &str, texts: &[&str]) -> LlmFut<'_, Vec<Vec<f32>>>;
}

// ── Factory ────────────────────────────────────────────────────────────────

/// Build an `LlmProvider` from a `ProviderConfig`.
pub fn build_provider(name: &str, cfg: &ProviderConfig) -> Arc<dyn LlmProvider> {
    match cfg.backend.as_str() {
        "anthropic" => Arc::new(anthropic::AnthropicProvider::new(name, cfg)),
        "bedrock" => Arc::new(bedrock::BedrockProvider::new(name, cfg)),
        _ => Arc::new(openai::OpenAiProvider::new(name, cfg)), // "openai" + compatible
    }
}

// ── Handoff tool helper ────────────────────────────────────────────────────

/// Build handoff tool definitions for a list of available specialist agents.
pub fn handoff_tools(agent_names: &[String]) -> Vec<ToolDef> {
    agent_names
        .iter()
        .map(|n| ToolDef {
            name: format!("transfer_to_{}", n.replace('-', "_")),
            description: format!(
                "Transfer this conversation to the `{n}` specialist. \
                 Use when the current task requires expertise this agent doesn't have."
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Why you are transferring to this agent." },
                    "context": { "type": "object", "description": "Relevant context to carry over." }
                },
                "required": ["reason"]
            }),
        })
        .collect()
}

/// Parse a tool call as a potential handoff request.
pub fn parse_handoff(call: &ToolCall, agent_names: &[String]) -> Option<(String, String, Value)> {
    for name in agent_names {
        let tool = format!("transfer_to_{}", name.replace('-', "_"));
        if call.name == tool {
            let reason = call
                .arguments
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("delegating")
                .to_owned();
            let ctx = call.arguments.get("context").cloned().unwrap_or_default();
            return Some((name.clone(), reason, ctx));
        }
    }
    None
}
