//! Anthropic Claude provider.

use anyhow::{Context, Result};
use secrecy::ExposeSecret;
use serde_json::Value;
use std::{future::Future, pin::Pin};

use super::{CompletionConfig, CompletionResult, LlmProvider, Message, ToolCall, ToolDef};
use crate::config::ProviderConfig;

pub struct AnthropicProvider {
    name: String,
    base: String,
    api_key: secrecy::SecretString,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(name: &str, cfg: &ProviderConfig) -> Self {
        Self {
            name: name.to_owned(),
            base: cfg
                .api_base
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".into()),
            api_key: cfg.api_key.clone(),
            client: mako_service::http::default_client(),
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        cfg: &CompletionConfig,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionResult>> + Send + '_>> {
        let system: String = messages
            .iter()
            .filter(|m| m.role == "system")
            .filter_map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let ant_msgs: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                if m.role == "tool" {
                    let id = m
                        .content
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_owned();
                    let content = m
                        .content
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_owned();
                    serde_json::json!({
                        "role": "user",
                        "content": [{"type":"tool_result","tool_use_id":id,"content":content}]
                    })
                } else {
                    serde_json::json!({ "role": m.role, "content": m.content })
                }
            })
            .collect();

        let ant_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name, "description": t.description, "input_schema": t.input_schema
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": cfg.model, "max_tokens": cfg.max_tokens,
            "system": system, "messages": ant_msgs,
        });
        // Omit `tools` when empty — Anthropic returns 400 on `"tools": []`.
        if !ant_tools.is_empty() {
            body["tools"] = Value::Array(ant_tools);
        }

        let url = format!("{}/v1/messages", self.base);
        let req = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .json(&body);

        Box::pin(async move {
            let resp = req.send().await.context("anthropic request")?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("Anthropic {status}: {text}");
            }
            let data: Value = serde_json::from_str(&text)?;
            let content = data["content"].as_array().cloned().unwrap_or_default();

            let tool_uses: Vec<ToolCall> = content
                .iter()
                .filter(|c| c["type"].as_str() == Some("tool_use"))
                .filter_map(|c| {
                    Some(ToolCall {
                        id: c["id"].as_str()?.to_owned(),
                        name: c["name"].as_str()?.to_owned(),
                        arguments: c["input"].clone(),
                    })
                })
                .collect();

            if !tool_uses.is_empty() {
                return Ok(CompletionResult::ToolCalls(tool_uses));
            }

            let text = content
                .iter()
                .filter(|c| c["type"].as_str() == Some("text"))
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(CompletionResult::Text(text))
        })
    }

    fn embed(
        &self,
        _model: &str,
        _texts: &[&str],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<f32>>>> + Send + '_>> {
        // Anthropic does not provide a public embedding API (2026-07).
        // Fallback: return empty vecs (RAG will use BM25 keyword search instead).
        Box::pin(async { Ok(vec![]) })
    }
}
