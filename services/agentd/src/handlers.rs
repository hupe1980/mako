//! HTTP handlers — CloudEvent webhook + manual run.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::Value;

use crate::AppState;

pub async fn webhook(
    State(state): State<Arc<AppState>>,
    Json(event): Json<Value>,
) -> impl IntoResponse {
    let event_type = event["type"].as_str().unwrap_or("unknown").to_owned();
    let event_id = event["id"].as_str().unwrap_or("unknown").to_owned();
    let data = event["data"].clone();

    if !state
        .cfg
        .trigger_event_types
        .iter()
        .any(|t| t == &event_type)
    {
        tracing::debug!(event_type, "agentd: ignoring non-trigger event");
        return StatusCode::NO_CONTENT.into_response();
    }
    tracing::info!(event_type, event_id, "agentd: trigger received");
    let state2 = Arc::clone(&state);
    tokio::spawn(async move {
        let decision = state2
            .orchestrator
            .dispatch(
                event_id,
                event_type,
                data,
                &state2.registry,
                &state2.mcp,
                state2.rag.as_ref(),
                &state2.cfg.tenant,
            )
            .await;
        emit_audit(&state2, &decision).await;
    });
    StatusCode::ACCEPTED.into_response()
}

pub async fn manual_run(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    let event_type = req["event_type"].as_str().unwrap_or("manual").to_owned();
    let event_id = uuid::Uuid::new_v4().to_string();
    let data = req["context"].clone();
    tracing::info!(event_type, event_id, "agentd: manual run");
    let decision = state
        .orchestrator
        .dispatch(
            event_id,
            event_type,
            data,
            &state.registry,
            &state.mcp,
            state.rag.as_ref(),
            &state.cfg.tenant,
        )
        .await;
    emit_audit(&state, &decision).await;
    (StatusCode::OK, Json(decision)).into_response()
}

async fn emit_audit(state: &AppState, decision: &crate::agent::AgentDecision) {
    let Some(ref url) = state.cfg.audit_webhook_url else {
        return;
    };
    let ce = decision.to_cloud_event(&state.cfg.tenant);
    let mut req = reqwest::Client::new()
        .post(url)
        .header("Content-Type", "application/cloudevents+json")
        .json(&ce);
    if let Some(ref secret) = state.cfg.audit_hmac_secret {
        req = req.bearer_auth(secret);
    }
    if let Err(e) = req.send().await {
        tracing::warn!(error = %e, "audit webhook failed");
    }
}

// ── M9: RAG ingest endpoint ────────────────────────────────────────────────

/// Request body for `POST /api/v1/rag/ingest`.
///
/// Accepts pre-formatted text (e.g. from `edmd.get_device_history`) for live
/// LanceDB RAG indexing.  This is the write-through path for M9 MSB device
/// history RAG.
#[derive(Debug, serde::Deserialize)]
pub struct RagIngestRequest {
    /// Source identifier for this document in search results.
    /// Convention for MSB history: `"msb-{malo_id}"`.
    pub source: String,
    /// The document text to chunk and index.
    pub text: String,
    /// Optional metadata (stored alongside the chunk; not searched).
    #[allow(dead_code)]
    pub metadata: Option<serde_json::Value>,
}

/// `POST /api/v1/rag/ingest`
///
/// Index a live text document into the LanceDB RAG store.
///
/// ## Intended callers
///
/// - `agentd` `msb-history-agent` after calling `edmd.get_device_history`
/// - ERP integrations that want to index operator runbooks or device notes
/// - Direct API clients for one-off document ingestion
///
/// ## M9 workflow
///
/// ```
/// 1. agentd receives de.edmd.reading.quality.warning or de.mako.process.initiated (WiM PID)
/// 2. msb-history-agent calls edmd MCP get_device_history { malo_id }
/// 3. msb-history-agent calls POST /api/v1/rag/ingest { source: "msb-{malo_id}", text: <document> }
/// 4. LanceDB stores chunks → available for natural-language queries
/// 5. Field operator: POST /api/v1/run { event_type: "msb.query", context: { query: "..." } }
/// ```
pub async fn rag_ingest(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(req): Json<RagIngestRequest>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;

    let Some(ref rag) = state.rag else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "RAG is disabled — enable [rag] in agentd.toml",
        )
            .into_response();
    };

    if req.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "text must not be empty").into_response();
    }
    if req.source.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "source must not be empty").into_response();
    }

    let chunk_size = state.cfg.rag.chunk_size;
    let chunk_overlap = state.cfg.rag.chunk_overlap;

    match rag
        .index_live_text(&req.source, &req.text, chunk_size, chunk_overlap)
        .await
    {
        Ok(count) => {
            tracing::info!(source = %req.source, chunks = count, "RAG: live ingest complete");
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "source": req.source,
                    "chunks_indexed": count,
                    "note": "Document indexed into LanceDB RAG store. Available for natural-language queries immediately.",
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(source = %req.source, error = %e, "RAG: live ingest failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}
