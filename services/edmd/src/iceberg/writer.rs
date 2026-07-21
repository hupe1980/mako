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

use arrow::array::{ArrayRef, Decimal128Array, Int32Array, StringArray, TimestampMicrosecondArray};
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
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::file::properties::WriterProperties;
use parquet::schema::types::ColumnPath;
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
    use iceberg::spec::{Literal, PartitionKey, Struct};
    use std::collections::BTreeMap;

    if reads.is_empty() {
        return Ok(Vec::new());
    }

    let schema = table.metadata().current_schema().clone();

    let location_gen = DefaultLocationGenerator::new(table.metadata().clone())
        .map_err(|e| anyhow::anyhow!("location generator: {e}"))?;

    // Use a per-sink UUID prefix to avoid duplicate Parquet paths on restart.
    // A fixed prefix ("edmd-archive-") would regenerate the same paths after restart
    // because the internal counter resets to 0, causing Iceberg catalog commit failures
    // or silent data corruption on re-committed files.
    let sink_id = Uuid::new_v4();

    // The table is partitioned by (tenant, sparte, month(dtm_from)); every
    // data file must carry the partition tuple its rows belong to, so the
    // batch is grouped and each group written by its own DataFileWriter.
    // Committing an unpartitioned file into a partitioned table fails with
    // "Partition value is not compatible with partition type" — caught by
    // the `iceberg_archive` integration tests.
    let mut groups: BTreeMap<(String, &'static str, i32), Vec<MeterRead>> = BTreeMap::new();
    for r in reads {
        let key = (
            r.tenant.clone(),
            sparte_str(r.sparte),
            months_since_epoch(r.dtm_from),
        );
        groups.entry(key).or_default().push(r.clone());
    }

    let spec = table.metadata().default_partition_spec();
    let mut all_files: Vec<DataFile> = Vec::new();

    for (group_idx, ((tenant, sparte, month), group_rows)) in groups.into_iter().enumerate() {
        let record_batch = rows_to_record_batch(&group_rows, table)?;
        let file_name_gen = DefaultFileNameGenerator::new(
            format!("edmd-{sink_id}-p{group_idx}-"),
            None,
            DataFileFormat::Parquet,
        );
        let partition_key = PartitionKey::new(
            (**spec).clone(),
            schema.clone(),
            Struct::from_iter([
                Some(Literal::string(&tenant)),
                Some(Literal::string(sparte)),
                Some(Literal::int(month)),
            ]),
        );

        let props = WriterProperties::builder()
            // ZSTD level 3 — best ratio for archival data; decompression is rare
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap_or_default()))
            // Bloom filter on malo_id: probabilistic MaLo presence test (1% FPR)
            // eliminates ~99% of Parquet file reads for single-MaLo cold-tier queries
            .set_column_bloom_filter_enabled(ColumnPath::from("malo_id"), true)
            .set_column_bloom_filter_ndv(ColumnPath::from("malo_id"), 100_000)
            .set_column_bloom_filter_fpp(ColumnPath::from("malo_id"), 0.01)
            // DELTA_BINARY_PACKED on timestamps: regular 15-min intervals have
            // delta-of-delta = 0 → ~1 bit per timestamp (Gorilla §5.2 principle)
            .set_column_encoding(ColumnPath::from("dtm_from"), Encoding::DELTA_BINARY_PACKED)
            .set_column_encoding(ColumnPath::from("dtm_to"), Encoding::DELTA_BINARY_PACKED)
            // Low-cardinality string columns (quality: 8 values, sparte: 2,
            // obis_code: typically <20 per tenant) use dictionary encoding — the
            // parquet default, controlled by the dictionary flag. Setting
            // `RLE_DICTIONARY` via `set_column_encoding` instead panics at write
            // time ("Dictionary encoding can not be used as fallback encoding") —
            // caught by the `iceberg_archive` integration tests.
            .set_column_dictionary_enabled(ColumnPath::from("quality"), true)
            .set_column_dictionary_enabled(ColumnPath::from("sparte"), true)
            .set_column_dictionary_enabled(ColumnPath::from("malo_id"), true)
            .set_column_dictionary_enabled(ColumnPath::from("obis_code"), true)
            .set_column_dictionary_enabled(ColumnPath::from("allocation_version"), true)
            .build();

        // ParquetWriterBuilder takes (properties, schema) — FileIO/location are
        // in the rolling writer.
        let parquet_builder = ParquetWriterBuilder::new(props, schema.clone());

        let rolling_builder = RollingFileWriterBuilder::new_with_default_file_size(
            parquet_builder,
            table.file_io().clone(),
            location_gen.clone(),
            file_name_gen,
        );

        // DataFileWriterBuilder tracks statistics (record count, file size, min/max).
        let mut writer = DataFileWriterBuilder::new(rolling_builder)
            .build(Some(partition_key))
            .await
            .map_err(|e| anyhow::anyhow!("DataFileWriter build: {e}"))?;

        writer
            .write(record_batch)
            .await
            .map_err(|e| anyhow::anyhow!("iceberg write: {e}"))?;

        all_files.extend(
            writer
                .close()
                .await
                .map_err(|e| anyhow::anyhow!("iceberg close: {e}"))?,
        );
    }

    debug!(
        rows = reads.len(),
        files = all_files.len(),
        "iceberg: data files written"
    );
    Ok(all_files)
}

/// The Iceberg `month` transform value: whole months since 1970-01.
fn months_since_epoch(ts: OffsetDateTime) -> i32 {
    (ts.year() - 1970) * 12 + (u8::from(ts.month()) as i32 - 1)
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
        .with_timezone("+00:00"),
    );
    let dtm_to: ArrayRef = Arc::new(
        TimestampMicrosecondArray::from(
            rows.iter()
                .map(|r| odt_to_micros(r.dtm_to))
                .collect::<Vec<_>>(),
        )
        .with_timezone("+00:00"),
    );
    let qty: ArrayRef = {
        // Convert NUMERIC(18,5) Decimal to i128 scaled by 10^5 for Arrow Decimal128.
        // Arrow Decimal128 stores the value as (significand * 10^-scale) where scale=5.
        let values: Vec<i128> = rows
            .iter()
            .map(|r| {
                use rust_decimal::prelude::ToPrimitive;
                // Multiply by 10^5 to get the integer representation
                let scaled = r.quantity_kwh * rust_decimal::Decimal::from(100_000u32);
                scaled.to_i128().unwrap_or(0)
            })
            .collect();
        Arc::new(
            Decimal128Array::from(values)
                .with_precision_and_scale(18, 5)
                .expect("Decimal128 precision=18 scale=5"),
        )
    };
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
            .map(|r| Some(r.tenant.as_str()))
            .collect::<Vec<_>>(),
    ));
    let allocation_version: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| Some(r.allocation_version.as_str()))
            .collect::<Vec<_>>(),
    ));

    Ok(RecordBatch::try_new(
        arrow_schema,
        vec![
            malo_id,
            melo_id,
            dtm_from,
            dtm_to,
            qty,
            quality,
            pid,
            sparte,
            obis,
            tenant,
            allocation_version,
        ],
    )?)
}

fn sparte_str(s: mako_edm::domain::Sparte) -> &'static str {
    match s {
        mako_edm::domain::Sparte::Gas => "GAS",
        mako_edm::domain::Sparte::Strom => "STROM",
        mako_edm::domain::Sparte::Waerme => "WAERME",
        mako_edm::domain::Sparte::Wasser => "WASSER",
    }
}

#[inline]
fn odt_to_micros(dt: OffsetDateTime) -> i64 {
    dt.unix_timestamp() * 1_000_000 + dt.nanosecond() as i64 / 1_000
}
