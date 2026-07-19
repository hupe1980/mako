//! RAG (Retrieval-Augmented Generation) engine backed by LanceDB.
//!
//! Embeddings are stored in LanceDB, which supports local storage OR
//! cloud object storage (S3, GCS, Azure Blob) via the `storage_uri` config.
//!
//! ## Architecture
//!
//! ```text
//! Documents (Markdown, plain text)
//!   → chunk() → embed() → LanceDB table (cloud or local)
//!
//! Agent query (event context + current task)
//!   → embed(query) → LanceDB ANN search → top-K chunks
//!   → injected into agent system prompt as "Background Knowledge"
//! ```
//!
//! ## Storage URIs
//!
//! | URI | Backend |
//! |---|---|
//! | `/var/lib/agentd/rag` | local filesystem |
//! | `s3://my-bucket/agentd/rag` | AWS S3 (uses env `AWS_*` or role) |
//! | `gs://my-bucket/agentd/rag` | Google Cloud Storage |
//! | `az://my-container/agentd/rag` | Azure Blob Storage |

pub mod embed;
pub mod store;

pub use store::{RagChunk, RagStore};

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::config::{RagConfig, RagSource};
use crate::llm::LlmProvider;

/// Central RAG engine — wraps the store and embedding logic.
#[derive(Clone)]
pub struct RagEngine {
    store: Arc<RagStore>,
    embedder: Arc<dyn LlmProvider>,
    embed_model: String,
    top_k: usize,
    /// Minimum cosine similarity score to include a result (0.0-1.0).
    /// Internally converted to `max_distance = 1.0 - score_threshold`.
    score_threshold: f32,
    /// Tenant for all queries/inserts — data-isolation boundary.
    tenant: String,
}

impl RagEngine {
    /// Build and initialise the RAG engine.
    pub async fn new(
        cfg: &RagConfig,
        embedder: Arc<dyn LlmProvider>,
        tenant: &str,
    ) -> Result<Self> {
        let store = Arc::new(RagStore::open(&cfg.storage_uri, cfg.embedding_dim).await?);
        let engine = Self {
            store,
            embedder,
            embed_model: cfg.embedding_model.clone(),
            top_k: cfg.top_k,
            score_threshold: cfg.score_threshold,
            tenant: tenant.to_owned(),
        };

        // Index all configured sources at startup (tenant-scoped)
        for src in &cfg.sources {
            if let Err(e) = engine
                .index_source(src, cfg.chunk_size, cfg.chunk_overlap)
                .await
            {
                warn!(source = %src.name, error = %e, "RAG: indexing failed — skipping");
            }
        }

        info!(
            uri = %cfg.storage_uri,
            sources = cfg.sources.len(),
            top_k = cfg.top_k,
            score_threshold = cfg.score_threshold,
            tenant = %tenant,
            "RAG engine ready"
        );
        Ok(engine)
    }

    /// Query the knowledge base for chunks relevant to `query` (tenant-scoped).
    /// Returns formatted context string ready for injection into the system prompt.
    pub async fn query(&self, query: &str) -> String {
        match self.query_chunks(query).await {
            Ok(chunks) if !chunks.is_empty() => {
                let formatted: Vec<String> = chunks
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let score_info = c
                            .distance
                            .map(|d| format!(" (sim={:.2})", 1.0 - d))
                            .unwrap_or_default();
                        format!("[{}] **{}**{}\n{}", i + 1, c.source, score_info, c.text)
                    })
                    .collect();
                format!(
                    "## Background Knowledge (from RAG knowledge base)\n\n{}\n",
                    formatted.join("\n\n---\n\n")
                )
            }
            Ok(_) => String::new(),
            Err(e) => {
                warn!(error = %e, "RAG query failed — proceeding without context");
                String::new()
            }
        }
    }

    async fn query_chunks(&self, query: &str) -> Result<Vec<RagChunk>> {
        let vecs: Vec<Vec<f32>> = self.embedder.embed(&self.embed_model, &[query]).await?;
        let query_vec = vecs.into_iter().next().unwrap_or_default();
        if query_vec.is_empty() {
            // No embedding available (e.g. Anthropic provider) — BM25 text search fallback.
            return self
                .store
                .bm25_search(query, self.top_k, &self.tenant)
                .await;
        }
        // Convert similarity threshold to cosine distance threshold:
        // cosine_similarity = 1 - cosine_distance  →  max_distance = 1 - score_threshold
        let max_distance = 1.0_f32 - self.score_threshold;
        self.store
            .vector_search(&query_vec, self.top_k, &self.tenant, max_distance)
            .await
    }

    async fn index_source(&self, src: &RagSource, chunk_size: usize, overlap: usize) -> Result<()> {
        if self.store.source_indexed(&src.name, &self.tenant).await? {
            tracing::debug!(source = %src.name, "RAG: already indexed — skipping");
            return Ok(());
        }
        info!(source = %src.name, path = %src.path, "RAG: indexing");
        let text = std::fs::read_to_string(&src.path)
            .map_err(|e| anyhow::anyhow!("RAG read {}: {e}", src.path))?;
        let chunks = embed::chunk_text(&text, chunk_size, overlap);
        if chunks.is_empty() {
            return Ok(());
        }

        let texts: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let vecs = self.embedder.embed(&self.embed_model, &texts).await?;
        let rag_chunks: Vec<RagChunk> = chunks
            .into_iter()
            .zip(vecs)
            .map(|(text, vec)| RagChunk {
                id: uuid::Uuid::new_v4().to_string(),
                tenant: self.tenant.clone(),
                source: src.name.clone(),
                text,
                embedding: vec,
                distance: None,
            })
            .collect();

        self.store.insert(&rag_chunks).await?;
        info!(source = %src.name, count = rag_chunks.len(), "RAG: indexed");
        Ok(())
    }

    /// Index a live text document (tenant-scoped).
    pub async fn index_live_text(
        &self,
        source: &str,
        text: &str,
        chunk_size: usize,
        chunk_overlap: usize,
    ) -> Result<usize> {
        if text.trim().is_empty() {
            return Ok(0);
        }

        let chunks = embed::chunk_text(text, chunk_size, chunk_overlap);
        if chunks.is_empty() {
            return Ok(0);
        }

        let texts: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let vecs = self.embedder.embed(&self.embed_model, &texts).await?;

        let rag_chunks: Vec<RagChunk> = chunks
            .into_iter()
            .zip(vecs)
            .map(|(chunk_text, vec)| RagChunk {
                id: uuid::Uuid::new_v4().to_string(),
                tenant: self.tenant.clone(),
                source: source.to_owned(),
                text: chunk_text,
                embedding: vec,
                distance: None,
            })
            .collect();

        let count = rag_chunks.len();
        self.store.insert(&rag_chunks).await?;
        tracing::info!(source, count, "RAG: live text indexed");
        Ok(count)
    }

    /// Query the knowledge base — public for use by the RAG ingest handler.
    pub async fn search(&self, query: &str) -> String {
        self.query(query).await
    }
}
