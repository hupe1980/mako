//! Apache Iceberg V2 archival tier for `edmd`.
//!
//! ## Catalog
//!
//! `iceberg-catalog-sql` stores Iceberg table metadata (namespace, schema,
//! partition spec, snapshots, manifests) in the same PostgreSQL database that
//! `edmd` uses for its hot tier.  No Nessie/Polaris/AWS Glue required.
//!
//! ## Storage
//!
//! Data files are written via `iceberg-storage-opendal` (opendal 0.55).
//! Supported schemes: `s3://` / `s3a://` (S3 + S3-compatible), `gs://` (GCS),
//! `file://` (local, dev/test).  Azure: add `iceberg-storage-opendal`
//! `opendal-azdls` feature and handle the `abfss://` scheme separately.

pub mod query;
pub mod schema;
pub mod worker;
pub mod writer;

use std::sync::Arc;

use iceberg::io::{
    FileIO, FileIOBuilder, S3_ACCESS_KEY_ID, S3_ENDPOINT, S3_REGION, S3_SECRET_ACCESS_KEY,
};
use iceberg_storage_opendal::OpenDalStorageFactory;
use mako_edm::archive::ArchiveConfig;

/// Build an [`iceberg::io::FileIO`] from the archive configuration.
pub fn build_file_io(cfg: &ArchiveConfig) -> anyhow::Result<FileIO> {
    let scheme = cfg
        .storage_uri
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_else(|| "file".to_owned());

    let factory: Arc<dyn iceberg::io::StorageFactory> = match scheme.as_str() {
        "s3" | "s3a" => Arc::new(OpenDalStorageFactory::S3 {
            configured_scheme: scheme.clone(),
            customized_credential_load: None,
        }),
        "gs" | "gcs" => Arc::new(OpenDalStorageFactory::Gcs),
        "file" => Arc::new(OpenDalStorageFactory::Fs),
        other => anyhow::bail!(
            "edmd archive: unsupported scheme '{other}' in '{}'. \
             Supported: s3://, s3a://, gs://, file://",
            cfg.storage_uri
        ),
    };

    let mut b = FileIOBuilder::new(factory);

    if matches!(scheme.as_str(), "s3" | "s3a") {
        if let Some(k) = &cfg.access_key_id {
            b = b.with_prop(S3_ACCESS_KEY_ID, k);
        }
        if let Some(s) = &cfg.secret_access_key {
            b = b.with_prop(S3_SECRET_ACCESS_KEY, s);
        }
        b = b.with_prop(S3_REGION, &cfg.region);
        if let Some(ep) = &cfg.endpoint_url {
            b = b.with_prop(S3_ENDPOINT, ep);
        }
    }

    Ok(b.build())
}
