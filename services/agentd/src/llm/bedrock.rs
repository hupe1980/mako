//! AWS Bedrock provider — Claude, Titan, Llama via SigV4.
//!
//! Implements manual AWS Signature Version 4 to avoid pulling in the
//! full AWS SDK (~100 crates). Uses `hmac` + `sha2` (already in workspace).

use anyhow::{Context, Result};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use std::{future::Future, pin::Pin};

use super::{CompletionConfig, CompletionResult, LlmProvider, Message, ToolCall, ToolDef};
use crate::config::ProviderConfig;

pub struct BedrockProvider {
    name: String,
    region: String,
    access_key: String,
    secret_key: SecretString,
    client: reqwest::Client,
}

impl BedrockProvider {
    pub fn new(name: &str, cfg: &ProviderConfig) -> Self {
        Self {
            name: name.to_owned(),
            region: cfg.aws_region.clone().unwrap_or_else(|| "us-east-1".into()),
            access_key: cfg.aws_access_key_id.clone().unwrap_or_default(),
            secret_key: cfg
                .aws_secret_access_key
                .clone()
                .unwrap_or_else(|| SecretString::new(String::new().into())),
            client: mako_service::http::default_client(),
        }
    }

    /// Build a signed Bedrock `InvokeModel` request.
    fn invoke_url(&self, model_id: &str) -> String {
        let model_enc = model_id.replace(':', "%3A").replace('/', "%2F");
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke",
            self.region, model_enc
        )
    }

    /// AWS SigV4 request signing (HMAC-SHA256).
    fn sign_request(
        &self,
        method: &str,
        url: &str,
        body: &[u8],
        now: &time::OffsetDateTime,
    ) -> Result<Vec<(String, String)>> {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};
        type HmacSha256 = Hmac<Sha256>;

        let date_str = format!("{:04}{:02}{:02}", now.year(), now.month() as u8, now.day());
        let datetime_str = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );

        let body_hash = hex::encode(Sha256::digest(body));
        // Parse host and path from URL manually (no `url` dep needed for well-formed AWS URLs)
        let after_scheme = url.splitn(3, '/').nth(2).unwrap_or(url);
        let (host, path) = after_scheme.split_once('/').unwrap_or((after_scheme, ""));
        let path = format!("/{path}");
        let service = "bedrock-runtime";

        // Canonical request
        let canonical = format!(
            "{method}\n{path}\n\nhost:{host}\nx-amz-content-sha256:{body_hash}\nx-amz-date:{datetime_str}\n\nhost;x-amz-content-sha256;x-amz-date\n{body_hash}"
        );
        let canonical_hash = hex::encode(Sha256::digest(canonical.as_bytes()));

        // String to sign
        let scope = format!("{date_str}/{}/{service}/aws4_request", self.region);
        let string_to_sign = format!("AWS4-HMAC-SHA256\n{datetime_str}\n{scope}\n{canonical_hash}");

        // Signing key
        let sign = |key: &[u8], msg: &[u8]| -> Vec<u8> {
            let mut mac = HmacSha256::new_from_slice(key).unwrap();
            mac.update(msg);
            mac.finalize().into_bytes().to_vec()
        };
        let k_date = sign(
            format!("AWS4{}", self.secret_key.expose_secret()).as_bytes(),
            date_str.as_bytes(),
        );
        let k_region = sign(&k_date, self.region.as_bytes());
        let k_service = sign(&k_region, service.as_bytes());
        let k_signing = sign(&k_service, b"aws4_request");

        let mut mac = HmacSha256::new_from_slice(&k_signing).unwrap();
        mac.update(string_to_sign.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders=host;x-amz-content-sha256;x-amz-date,Signature={signature}",
            self.access_key
        );

        Ok(vec![
            ("Authorization".into(), auth),
            ("x-amz-date".into(), datetime_str),
            ("x-amz-content-sha256".into(), body_hash),
        ])
    }
}

impl LlmProvider for BedrockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        cfg: &CompletionConfig,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionResult>> + Send + '_>> {
        // Bedrock uses model-specific request formats.
        // Anthropic Claude on Bedrock uses the same schema as direct Anthropic API
        // but wrapped in Bedrock's InvokeModel endpoint.
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

        let mut bedrock_body = serde_json::json!({
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": cfg.max_tokens,
            "system": system,
            "messages": ant_msgs,
        });
        // Anthropic models reject an empty tools array with a 400 — omit it,
        // matching the direct Anthropic adapter.
        if !ant_tools.is_empty() {
            bedrock_body["tools"] = serde_json::json!(ant_tools);
        }

        let url = self.invoke_url(&cfg.model);
        let body_bytes = serde_json::to_vec(&bedrock_body).unwrap_or_default();
        let now = time::OffsetDateTime::now_utc();
        let sign_result = self.sign_request("POST", &url, &body_bytes, &now);
        let client = self.client.clone();

        Box::pin(async move {
            let headers = sign_result?;
            let mut req = client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body_bytes);
            for (k, v) in headers {
                req = req.header(k, v);
            }
            let resp = req.send().await.context("bedrock request")?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("Bedrock {status}: {text}");
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
            let t = content
                .iter()
                .filter(|c| c["type"].as_str() == Some("text"))
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(CompletionResult::Text(t))
        })
    }

    fn embed(
        &self,
        model: &str,
        texts: &[&str],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<f32>>>> + Send + '_>> {
        // Amazon Titan Embeddings v2
        let client = self.client.clone();
        let futures: Vec<_> = texts
            .iter()
            .map(|t| {
                let body = serde_json::json!({ "inputText": t });
                let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
                let url = self.invoke_url(model);
                let now = time::OffsetDateTime::now_utc();
                let headers = self.sign_request("POST", &url, &body_bytes, &now);
                (url, body_bytes, headers)
            })
            .collect();

        Box::pin(async move {
            let mut results = Vec::new();
            for (url, body_bytes, headers_result) in futures {
                let headers = headers_result?;
                let mut req = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .body(body_bytes);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                let resp = req.send().await.context("bedrock embed")?;
                let data: Value = resp.json().await.context("bedrock embed parse")?;
                let vec: Vec<f32> = data["embedding"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                results.push(vec);
            }
            Ok(results)
        })
    }
}
