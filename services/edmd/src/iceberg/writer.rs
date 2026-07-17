//! Iceberg V2 data-file writer for `edmd` archival.
//!
//! ## iceberg-rust 0.9.1 writer pipeline
//!
//! ```text
//! rows_to_record_batch()
//!       │
//!       ▼
//! ParquetWriterBuilder(props, schema)   ← file format only
//!       │
//!       ▼
//! RollingFileWriterBuilder              ← wraps Parquet + adds FileIO + location
//!   (parquet_builder, file_io, location_gen, file_name_gen)
//!       │
//!       ▼
//! DataFileWriterBuilder(rolling)
//!       │  .build(None).await?
//!       ▼
//! DataFileWriter  →  .write(batch)  →  .close()  →  Vec<DataFile>
//! ```

use std::sync::Arc;

use arrow::array::{ArrayRef, Int32Array, StringArray, TimestampMicrosecondArray};
use arrow::record_batch::RecordBatch;
use iceberg::io::FileIO;
use iceberg::spec::DataFile;
use iceberg::spec::DataFileFormat;
use iceberg::table::Table;
use iceberg::writer::IcebergWriter;
use iceberg::writer::IcebergWriterBuilder;
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use mako_edm::domain::MeterRead;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use time::OffsetDateTime;
use tracing::debug;
use uuid::Uuid;

/// Write a batch of `MeterRead` rows as Iceberg V2 Parquet data files.
///
/// Returns `Vec<DataFile>` ready to be committed via `Transaction::fast_append`.
pub async fn write_data_files(
    table: &Table,
    reads: &[MeterRead],
    _file_io: &FileIO, // FileIO is taken from table.file_io() — kept for API clarity
) -> anyhow::Result<Vec<DataFile>> {
    if reads.is_empty() {
        return Ok(Vec::new());
    }

    let schema = table.metadata().current_schema().clone();

    let record_batch = rows_to_record_batch(reads, table)?;

    let location_gen = DefaultLocationGenerator::new(table.metadata().clone())
        .map_err(|e| anyhow::anyhow!("location generator: {e}"))?;

    // F-05: Use a per-sink UUID prefix to avoid duplicate Parquet paths on restart.
    // A fixed prefix ("edmd-archive-") would regenerate the same paths after restart
    // because the internal counter resets to 0, causing Iceberg catalog commit failures
    // or silent data corruption on re-committed files.
    let sink_id = Uuid::new_v4();
    let file_name_gen =
        DefaultFileNameGenerator::new(format!("edmd-{}-", sink_id), None, DataFileFormat::Parquet);

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();

    // ParquetWriterBuilder takes (properties, schema) — FileIO/location are in rolling writer.
    let parquet_builder = ParquetWriterBuilder::new(props, schema);

    // RollingFileWriterBuilder wraps the parquet builder with FileIO and path generators.
    let rolling_builder = RollingFileWriterBuilder::new_with_default_file_size(
        parquet_builder,
        table.file_io().clone(),
        location_gen,
        file_name_gen,
    );

    // DataFileWriterBuilder tracks statistics (record count, file size, min/max).
    let mut writer = DataFileWriterBuilder::new(rolling_builder)
        .build(None)
        .await
        .map_err(|e| anyhow::anyhow!("DataFileWriter build: {e}"))?;

    writer
        .write(record_batch)
        .await
        .map_err(|e| anyhow::anyhow!("iceberg write: {e}"))?;

    let data_files = writer
        .close()
        .await
        .map_err(|e| anyhow::anyhow!("iceberg close: {e}"))?;

    debug!(
        rows = reads.len(),
        files = data_files.len(),
        "iceberg: data files written"
    );
    Ok(data_files)
}

// ── Arrow record batch ────────────────────────────────────────────────────────

fn rows_to_record_batch(rows: &[MeterRead], table: &Table) -> anyhow::Result<RecordBatch> {
    use iceberg::arrow::schema_to_arrow_schema;

    let iceberg_schema = table.metadata().current_schema();

    let arrow_schema = Arc::new(
        schema_to_arrow_schema(iceberg_schema)
            .map_err(|e| anyhow::anyhow!("schema_to_arrow_schema: {e}"))?,
    );

    let malo_id: ArrayRef = Arc::new(StringArray::from_iter_values(
        rows.iter().map(|r| r.malo_id.as_str()),
    ));
    let melo_id: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.melo_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let dtm_from: ArrayRef = Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| odt_to_micros(r.dtm_from))
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    );
    let dtm_to: ArrayRef = Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| odt_to_micros(r.dtm_to))
                .collect::<Vec<_>>(),
        )
        .with_timezone("UTC"),
    );
    let qty: ArrayRef = Arc::new(StringArray::from_iter_values(
        rows.iter().map(|r| r.quantity_kwh.to_string()),
    ));
    let quality: ArrayRef = Arc::new(StringArray::from_iter_values(
        rows.iter()
            .map(|r| format!("{:?}", r.quality).to_uppercase()),
    ));
    let pid: ArrayRef = Arc::new(Int32Array::from_iter_values(
        rows.iter().map(|r| r.pid as i32),
    ));
    let sparte: ArrayRef = Arc::new(StringArray::from_iter_values(
        rows.iter().map(|r| sparte_str(r.sparte)),
    ));
    let obis: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.obis_code.as_deref())
            .collect::<Vec<_>>(),
    ));
    let tenant: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.tenant_id.map(|u| u.to_string()))
            .collect::<Vec<_>>(),
    ));

    Ok(RecordBatch::try_new(
        arrow_schema,
        vec![
            malo_id, melo_id, dtm_from, dtm_to, qty, quality, pid, sparte, obis, tenant,
        ],
    )?)
}

fn sparte_str(s: mako_edm::domain::Sparte) -> &'static str {
    match s {
        mako_edm::domain::Sparte::Gas => "GAS",
        mako_edm::domain::Sparte::Strom => "STROM",
    }
}

#[inline]
fn odt_to_micros(dt: OffsetDateTime) -> i64 {
    dt.unix_timestamp() * 1_000_000 + dt.nanosecond() as i64 / 1_000
}
