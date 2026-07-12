//! Iceberg V2 schema and partition spec for `meter_reads_archive`.

use iceberg::spec::{NestedField, PrimitiveType, Schema, Transform, Type, UnboundPartitionSpec};
use std::sync::Arc;

/// Build the Iceberg V2 schema for `meter_reads_archive`.
pub fn meter_reads_schema() -> anyhow::Result<Arc<Schema>> {
    let schema = Schema::builder()
        .with_schema_id(0)
        .with_fields([
            NestedField::required(1, "malo_id", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(2, "melo_id", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(3, "dtm_from", Type::Primitive(PrimitiveType::Timestamptz))
                .into(),
            NestedField::required(4, "dtm_to", Type::Primitive(PrimitiveType::Timestamptz)).into(),
            NestedField::required(5, "quantity_kwh", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(6, "quality", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(7, "pid", Type::Primitive(PrimitiveType::Int)).into(),
            NestedField::required(8, "sparte", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(9, "obis_code", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::optional(10, "tenant_id", Type::Primitive(PrimitiveType::String)).into(),
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
        assert_eq!(schema.as_ref().highest_field_id(), 10);
        assert!(schema.field_by_name("malo_id").is_some());
    }

    #[test]
    fn partition_spec_builds() {
        let spec = meter_reads_partition_spec();
        // sparte (identity) + dtm_from_month (month) = 2 fields
        assert_eq!(spec.fields().len(), 2);
    }
}
