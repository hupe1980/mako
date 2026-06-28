//! Server (handler trait + axum router) implementations for the electricity APIs.
//!
//! These are the **Netzbetreiber / MSB** side — services that implement the
//! electricity market API endpoints.
//!
//! | Module               | Implements                               |
//! |----------------------|------------------------------------------|
//! | [`control_measures`] | MSB endpoint (konfiguration / initialZustand) + NB/LF endpoint (all responses) |
//! | [`malo_ident`]       | NB endpoint (MaLo-ID request handler)    |
//! | [`wim_order`]        | MSB endpoint (iMS Anmeldung) + NB endpoint (Bestätigung / Ablehnung) |

pub mod control_measures;
pub mod malo_ident;
pub mod wim_order;
