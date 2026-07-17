//! Iceberg V2 schema and partition spec for `meter_reads_archive`.

use iceberg::spec::{NestedField, PrimitiveType, Schema, Transform, Type, UnboundPartitionSpec};
use std::sync::Arc;

/// Build the Iceberg V2 schema for `meter_reads_archive`.
///
/// ## Field types
///
/// | # | Name | Type | Notes |
/// |---|------|------|-------|
/// | 1 | malo_id | String | Marktlokations-ID |
/// | 2 | melo_id | String? | Messlokations-ID |
/// | 3 | dtm_from | Timestamptz | Interval start (UTC) |
/// | 4 | dtm_to | Timestamptz | Interval end (UTC) |
/// | 5 | quantity_kwh | Decimal(18,5) | §22 MessZV 5-decimal precision |
/// | 6 | quality | String | MEASURED/ESTIMATED/… |
/// | 7 | pid | Int | Source MSCONS PID |
/// | 8 | sparte | String | STROM/GAS |
/// | 9 | obis_code | String? | OBIS-Kennzahl |
/// | 10 | tenant | String | Mandatory data-isolation key |
/// | 11 | allocation_version | String? | INITIAL/CORRECTION/FINAL (BK6-22-024 §6.4) |
pub fn meter_reads_schema() -> anyhow::Result<Arc<Schema>> {
    let schema = Schema::builder()
        .with_schema_id(0)
        .with_fields([
            NestedField::required(1, "malo_id", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(2, "melo_id", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(3, "dtm_from", Type::Primitive(PrimitiveType::Timestamptz))
                .into(),
            NestedField::required(4, "dtm_to", Type::Primitive(PrimitiveType::Timestamptz)).into(),
            // Decimal(18,5) — preserves §22 MessZV 5-decimal-place precision.
            // Avoids the TRY_CAST overhead that STRING would require in DataFusion.
            NestedField::required(
                5,
                "quantity_kwh",
                Type::Primitive(PrimitiveType::Decimal {
                    precision: 18,
                    scale: 5,
                }),
            )
            .into(),
            NestedField::required(6, "quality", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(7, "pid", Type::Primitive(PrimitiveType::Int)).into(),
            NestedField::required(8, "sparte", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(9, "obis_code", Type::Primitive(PrimitiveType::String)).into(),
            // "tenant" — mandatory data-isolation key (renamed from legacy "tenant_id").
            NestedField::required(10, "tenant", Type::Primitive(PrimitiveType::String)).into(),
            // "allocation_version" — INITIAL / CORRECTION / FINAL per BK6-22-024 §6.4.
            // Allows mabis-syncd to distinguish day-3 INITIAL from day-8 FINAL data.
            NestedField::optional(
                11,
                "allocation_version",
                Type::Primitive(PrimitiveType::String),
            )
            .into(),
        ])
        .build()
        .map_err(|e| anyhow::anyhow!("iceberg schema error: {e}"))?;
    Ok(Arc::new(schema))
}

/// Build the Iceberg V2 partition spec for `meter_reads_archive`.
///
/// Partition by `identity(sparte)`, `month(dtm_from)`.
///
/// `month` subsumes year — Iceberg 0.9.1 does not allow two time-based transforms
/// on the same source field (year + month from field 3 would conflict).
/// Querying by year still works via predicate pushdown: `WHERE dtm_from >= '2025-01-01'`.
pub fn meter_reads_partition_spec() -> UnboundPartitionSpec {
    UnboundPartitionSpec::builder()
        .with_spec_id(0)
        .add_partition_field(8, "sparte", Transform::Identity)
        .expect("sparte partition field")
        .add_partition_field(3, "dtm_from_month", Transform::Month)
        .expect("month partition field")
        .build()
}

/// Logical table name in the SQL catalog.
pub const ICEBERG_TABLE_NAME: &str = "meter_reads_archive";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_builds() {
        let schema = meter_reads_schema().unwrap();
        // Fields: malo_id melo_id dtm_from dtm_to quantity_kwh quality pid sparte obis_code tenant allocation_version
        assert_eq!(schema.as_ref().highest_field_id(), 11);
        assert!(schema.field_by_name("malo_id").is_some());
        assert!(schema.field_by_name("tenant").is_some());
        assert!(schema.field_by_name("allocation_version").is_some());
        // quantity_kwh must be Decimal, not String
        let qty_field = schema.field_by_name("quantity_kwh").unwrap();
        assert!(
            matches!(
                qty_field.field_type.as_ref(),
                iceberg::spec::Type::Primitive(iceberg::spec::PrimitiveType::Decimal { .. })
            ),
            "quantity_kwh must be Decimal(18,5), not String"
        );
    }

    #[test]
    fn partition_spec_builds() {
        let spec = meter_reads_partition_spec();
        // sparte (identity) + dtm_from_month (month) = 2 fields
        assert_eq!(spec.fields().len(), 2);
    }
}
