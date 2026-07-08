//! PostgreSQL persistence for `invoicd`.

pub mod receipts;

pub use receipts::{ReceiptRow, upsert_receipt};
