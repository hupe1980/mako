//! Transport layer — everything that touches the wire.
//!
//! AS4 ingest/egress (EDIFACT and Redispatch XML legs), CONTRL/APERAK
//! acknowledgement handling, the outbox senders, the Verzeichnisdienst
//! (peer-directory) worker, and the BDEW API-Webdienste surface.

pub mod api_bridge;
pub mod as4_ingest;
pub mod as4_sender;
pub mod contrl_ack;
pub mod malo_ident_sender;
pub mod redispatch_xml_ingest;
pub mod verzeichnisdienst_worker;
pub mod webdienste;
