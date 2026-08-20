//! PostgreSQL persistence for `invoicd`.

pub mod receipts;

pub use receipts::{ReceiptRow, bo4e_version, upsert_receipt};
