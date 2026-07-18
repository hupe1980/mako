//! RAG vector store — LanceDB implementation.
//!
//! Provides persistent ANN vector search over embedded knowledge chunks.
//! Storage backend is determined by `storage_uri` in `RagConfig`:
//!
//! | URI scheme | Backend |
//! |---|---|
//! | `/path/to/dir` or `./dir` | Local filesystem (Lance format) |
//! | `s3://bucket/prefix` | AWS S3 (requires `AWS_*` env vars or instance role) |
//! | `gs://bucket/prefix` | Google Cloud Storage |
//! | `az://container/prefix` | Azure Blob Storage |
//!
//! The table schema is fixed: `id`, `source`, `text`, `vector`.
//! Vector dimension is set at `RagStore::open` time and must match the embedding provider.
//!
//! ## Search strategy
//!
//! - **Vector search** (`vector_search`): ANN index when built, flat scan otherwise.
//! - **BM25 keyword search** (`bm25_search`): full-table scan for providers without
//!   embedding APIs (e.g. Anthropic). Correct for corpora up to ~50 000 chunks.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, RecordBatchReader,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};

const TABLE: &str = "rag_chunks";

// ── Schema ────────────────────────────────────────────────────────────────────

fn rag_schema(dim: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
            false,
        ),
    ]))
}

// ── Public types ─────────────────────────────────────────────────────────────

/// A single chunk in the RAG store.
#[derive(Debug, Clone)]
pub struct RagChunk {
    #[allow(dead_code)]
    pub id: String,
    /// Tenant this chunk belongs to (data-isolation key).
    pub tenant: String,
    pub source: String,
    pub text: String,
    /// Embedding vector.  Empty when indexed without an embedding provider.
    pub embedding: Vec<f32>,
    /// Cosine distance from query vector (0.0 = identical, 1.0 = orthogonal).
    /// `None` for BM25 search results (no geometric distance).
    pub distance: Option<f32>,
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// LanceDB-backed RAG vector store.
pub struct RagStore {
    conn: Connection,
    schema: Arc<Schema>,
    dim: i32,
}

impl RagStore {
    /// Open (or create) a LanceDB store at the given URI.
    pub async fn open(uri: &str, dim: i32) -> Result<Self> {
        let conn = lancedb::connect(uri)
            .execute()
            .await
            .with_context(|| format!("lancedb::connect({uri})"))?;
        let schema = rag_schema(dim);

        let names = conn
            .table_names()
            .execute()
            .await
            .context("list lancedb tables")?;
        if !names.contains(&TABLE.to_owned()) {
            let empty: Box<dyn RecordBatchReader + Send> = Box::new(RecordBatchIterator::new(
                std::iter::empty::<Result<RecordBatch, arrow_schema::ArrowError>>(),
                schema.clone(),
            ));
            conn.create_table(TABLE, empty)
                .execute()
                .await
                .context("create lancedb rag_chunks table")?;
            tracing::info!(uri, dim, table = TABLE, "lancedb: RAG table created");
        } else {
            tracing::debug!(uri, dim, table = TABLE, "lancedb: RAG table opened");
        }

        Ok(Self { conn, schema, dim })
    }

    /// Insert `chunks` into the store.
    pub async fn insert(&self, chunks: &[RagChunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let batch = to_batch(chunks, &self.schema, self.dim)?;
        let table = self
            .conn
            .open_table(TABLE)
            .execute()
            .await
            .context("open lancedb table for insert")?;
        let data: Box<dyn RecordBatchReader + Send> = Box::new(RecordBatchIterator::new(
            vec![Ok(batch)],
            self.schema.clone(),
        ));
        table.add(data).execute().await.context("lancedb insert")?;
        tracing::debug!(n = chunks.len(), "lancedb: inserted chunks");
        Ok(())
    }

    /// ANN vector search scoped to `tenant`, using cosine distance.
    ///
    /// Returns chunks filtered to `max_distance` (cosine distance: 0 = perfect,
    /// 1 = orthogonal).  Derive from `score_threshold` as `1.0 - score_threshold`.
    pub async fn vector_search(
        &self,
        q: &[f32],
        top_k: usize,
        tenant: &str,
        max_distance: f32,
    ) -> Result<Vec<RagChunk>> {
        let table = self.conn.open_table(TABLE).execute().await?;
        if table.count_rows(None).await? == 0 {
            return Ok(Vec::new());
        }

        // SQL-escape tenant for safe filter (tenant GLNs are numeric so safe, but escape anyway)
        let tenant_sql = tenant.replace('\'', "''");

        let batches: Vec<RecordBatch> = table
            .query()
            .nearest_to(q)?
            .distance_type(lancedb::DistanceType::Cosine)
            .limit(top_k * 2) // fetch extra; filter will reduce count
            .only_if(format!("tenant = '{tenant_sql}'"))
            .execute()
            .await?
            .try_collect()
            .await?;

        let chunks = batches_to_chunks_with_distance(batches)?;
        Ok(chunks
            .into_iter()
            .filter(|c| c.distance.is_none_or(|d| d <= max_distance))
            .take(top_k)
            .collect())
    }

    /// Check if chunks from `source_name` scoped to `tenant` are already in the store.
    pub async fn source_indexed(&self, source_name: &str, tenant: &str) -> Result<bool> {
        let table = self.conn.open_table(TABLE).execute().await?;
        let source_sql = source_name.replace('\'', "''");
        let tenant_sql = tenant.replace('\'', "''");
        let filter = format!("source = '{source_sql}' AND tenant = '{tenant_sql}'");
        let count = table.count_rows(Some(filter)).await?;
        Ok(count > 0)
    }

    /// Delete all chunks for a given `tenant` (for RAG store purge / tenant offboarding).
    pub async fn delete_by_tenant(&self, tenant: &str) -> Result<usize> {
        let table = self.conn.open_table(TABLE).execute().await?;
        let tenant_sql = tenant.replace('\'', "''");
        table
            .delete(&format!("tenant = '{tenant_sql}'"))
            .await
            .context("lancedb delete_by_tenant")?;
        // LanceDB delete doesn't return a count; return 0 as placeholder
        Ok(0)
    }
    /// BM25 keyword scan scoped to `tenant` (O(n) — fallback for providers without embeddings).
    pub async fn bm25_search(&self, query: &str, top_k: usize, tenant: &str) -> Result<Vec<RagChunk>> {
        let table = self.conn.open_table(TABLE).execute().await?;
        if table.count_rows(None).await? == 0 {
            return Ok(Vec::new());
        }

        let query_lower = query.to_lowercase();
        let keywords: Vec<&str> = query_lower.split_whitespace().collect();
        let tenant_sql = tenant.replace('\'', "''");

        // Pre-filter by tenant before fetching all rows
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(format!("tenant = '{tenant_sql}'"))
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut scored: Vec<(usize, RagChunk)> = Vec::new();
        for b in &batches {
            // Schema: id=0, tenant=1, source=2, text=3, vector=4
            if let (Some(ids), Some(tenants), Some(sources), Some(texts)) = (
                b.column(0).as_any().downcast_ref::<StringArray>(),
                b.column(1).as_any().downcast_ref::<StringArray>(),
                b.column(2).as_any().downcast_ref::<StringArray>(),
                b.column(3).as_any().downcast_ref::<StringArray>(),
            ) {
                let vectors = b.column(4).as_any().downcast_ref::<FixedSizeListArray>();
                for i in 0..b.num_rows() {
                    let tl = texts.value(i).to_lowercase();
                    let score = keywords.iter().filter(|k| tl.contains(**k)).count();
                    if score > 0 {
                        scored.push((
                            score,
                            RagChunk {
                                id: ids.value(i).to_owned(),
                                tenant: tenants.value(i).to_owned(),
                                source: sources.value(i).to_owned(),
                                text: texts.value(i).to_owned(),
                                embedding: extract_vec(vectors, i),
                                distance: None,
                            },
                        ));
                    }
                }
            }
        }

        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        Ok(scored.into_iter().take(top_k).map(|(_, c)| c).collect())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn to_batch(chunks: &[RagChunk], schema: &Arc<Schema>, dim: i32) -> Result<RecordBatch> {
    let ids: StringArray = chunks.iter().map(|c| Some(c.id.as_str())).collect();
    let tenants: StringArray = chunks.iter().map(|c| Some(c.tenant.as_str())).collect();
    let sources: StringArray = chunks.iter().map(|c| Some(c.source.as_str())).collect();
    let texts: StringArray = chunks.iter().map(|c| Some(c.text.as_str())).collect();

    let flat_values: Vec<f32> = chunks
        .iter()
        .flat_map(|c| {
            let mut v = c.embedding.clone();
            v.resize(dim as usize, 0.0);
            v
        })
        .collect();
    let flat = Arc::new(Float32Array::from(flat_values)) as Arc<dyn Array>;
    let vfield = Arc::new(Field::new("item", DataType::Float32, true));
    let vectors = FixedSizeListArray::new(vfield, dim, flat, None);

    Ok(RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(tenants),
            Arc::new(sources),
            Arc::new(texts),
            Arc::new(vectors),
        ],
    )?)
}

/// Convert ANN result batches (schema: id, tenant, source, text, vector, _distance) to chunks.
fn batches_to_chunks_with_distance(batches: Vec<RecordBatch>) -> Result<Vec<RagChunk>> {
    let mut out = Vec::new();
    for b in batches {
        // ANN result schema: id=0, tenant=1, source=2, text=3, vector=4, _distance=5
        let ids = b.column(0).as_any().downcast_ref::<StringArray>();
        let tenants = b.column(1).as_any().downcast_ref::<StringArray>();
        let sources = b.column(2).as_any().downcast_ref::<StringArray>();
        let texts = b.column(3).as_any().downcast_ref::<StringArray>();
        let vectors = b.column(4).as_any().downcast_ref::<FixedSizeListArray>();
        // _distance column appended by LanceDB ANN search
        let distances = b
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

        if let (Some(ids), Some(tenants), Some(sources), Some(texts)) =
            (ids, tenants, sources, texts)
        {
            for i in 0..b.num_rows() {
                let distance = distances.map(|d| d.value(i));
                out.push(RagChunk {
                    id: ids.value(i).to_owned(),
                    tenant: tenants.value(i).to_owned(),
                    source: sources.value(i).to_owned(),
                    text: texts.value(i).to_owned(),
                    embedding: extract_vec(vectors, i),
                    distance,
                });
            }
        }
    }
    Ok(out)
}

fn extract_vec(vectors: Option<&FixedSizeListArray>, i: usize) -> Vec<f32> {
    vectors
        .map(|v| {
            v.value(i)
                .as_any()
                .downcast_ref::<Float32Array>()
                .map(|a| a.values().to_vec())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}
