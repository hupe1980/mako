//! Issued documents and how they reach the customer.
//!
//! # Why this is the daemon's problem and not the renderer's
//!
//! [`crate::document`] turns a caller's view into bytes; that is where the work
//! stops. Enough to *make* a document, not to communicate one — nothing
//! reproduces the bytes that were sent, and nothing says whether they were
//! sent. Both are regulated facts:
//!
//! * **§ 14 Abs. 1 Satz 2 UStG, § 147 Abs. 1 Nr. 2–3 AO** — the
//!   Rechnungsdoppel is kept eight years *in the form in which it was issued*.
//!   A pinned template hash makes the layout resolvable; it does not make the
//!   document resolvable, because the data behind it moves.
//! * **§ 126b BGB, § 41f Abs. 1 und Abs. 5 EnWG** — the disconnection sequence
//!   rests on notices that must have *reached* the customer in Textform, four
//!   weeks and eight Werktage before the act.
//!
//! # The shape
//!
//! One [`store`] write per document (append-only, bytes included), one
//! delivery row per channel, and a [`worker`] that drains the pending ones with
//! backoff:
//!
//! | Channel | How it is delivered | Evidence |
//! |---|---|---|
//! | `PORTAL` | the document is in the store and `portald` serves it | published, then `read_at` when the customer opens it |
//! | `EMAIL` | POST to a configured mail relay | the relay's message id |
//! | `POST` | a spool a print service pulls (`GET /api/v1/spool`) | the batch reference it reports back |
//! | `ERP` | POST to the operator's own webhook | its response |
//!
//! **No SMTP client, no print driver.** Both are adapters an operator already
//! runs, and embedding them turns a document daemon into a mail server. The
//! relay contract is the one `accountingd` uses for its bank adapter: a URL, a
//! bearer token, JSON. A deployment that configures none still has the portal
//! channel, which is what § 41 Abs. 5 EnWG and § 126b BGB actually ask for —
//! Textform on a durable medium, not registered post.

pub mod channel;
pub mod store;
pub mod worker;

pub use channel::{Channel, DeliveryOutcome};
pub use store::{DeliveryRow, DocumentRow, IssuedDocument, NewDocument};
