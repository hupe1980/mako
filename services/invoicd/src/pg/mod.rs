//! PostgreSQL persistence for `invoicd`.

pub mod receipts;

pub use receipts::{ReceiptRow, bo4e_version, receipt_outcome, upsert_receipt};
