//! OpenAI-compatible LLM provider (GPT-4o, Azure OpenAI, Ollama, LMStudio).

use anyhow::{Context, Result};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use super::{CompletionConfig, CompletionResult, LlmProvider, Message, ToolCall, ToolDef};
use crate::config::ProviderConfig;

pub struct OpenAiProvider {
    name: String,
    base: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(name: &str, cfg: &ProviderConfig) -> Self {
        Self {
            name: name.to_owned(),
            base: cfg
                .api_base
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            api_key: cfg.api_key.clone(),
            client: reqwest::Client::new(),
        }
    }
}

impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        cfg: &CompletionConfig,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionResult>> + Send + '_>> {
        let msgs: Vec<Value> = messages
            .iter()
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
                    serde_json::json!({ "role": "tool", "tool_call_id": id, "content": content })
                } else {
                    serde_json::json!({ "role": m.role, "content": m.content })
                }
            })
            .collect();

        let oai_tools: Vec<Value> = tools.iter().map(|t| serde_json::json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.input_schema }
        })).collect();

        let body = serde_json::json!({
            "model": cfg.model,
            "max_tokens": cfg.max_tokens,
            "messages": msgs,
            "tools": oai_tools,
        });

        let url = format!("{}/chat/completions", self.base);
        let req = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body);
        Box::pin(async move {
            let resp = req.send().await.context("openai request")?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("OpenAI {status}: {text}");
            }
            let data: Value = serde_json::from_str(&text).context("openai parse")?;
            let choice = &data["choices"][0]["message"];

            if let Some(tool_calls) = choice["tool_calls"].as_array() {
                let calls: Vec<ToolCall> = tool_calls
                    .iter()
                    .filter_map(|tc| {
                        let id = tc["id"].as_str()?.to_owned();
                        let name = tc["function"]["name"].as_str()?.to_owned();
                        let args: Value = serde_json::from_str(
                            tc["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or_default();
                        Some(ToolCall {
                            id,
                            name,
                            arguments: args,
                        })
                    })
                    .collect();
                return Ok(CompletionResult::ToolCalls(calls));
            }
            Ok(CompletionResult::Text(
                choice["content"].as_str().unwrap_or("").to_owned(),
            ))
        })
    }

    fn embed(
        &self,
        model: &str,
        texts: &[&str],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<f32>>>> + Send + '_>> {
        let url = format!("{}/embeddings", self.base);
        let body = serde_json::json!({ "model": model, "input": texts });
        let req = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body);
        Box::pin(async move {
            let resp = req.send().await.context("openai embed")?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("OpenAI embed {status}: {text}");
            }
            let data: Value = serde_json::from_str(&text)?;
            let vecs: Vec<Vec<f32>> = data["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|e| {
                    e["embedding"].as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect()
                    })
                })
                .collect();
            Ok(vecs)
        })
    }
}
