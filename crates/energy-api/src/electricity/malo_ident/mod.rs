//! MaLo-ID retrieval — 24h Lieferantenwechsel (GPKE part 2).
//!
//! Implements the `maloIdentV1` REST API defined in the BNetzA API-Webdienste
//! Strom specification. This API is **mandatory** for the 24h
//! Lieferantenwechsel process (BNetzA decision BK6-22-024), valid from
//! 2026-01-29.
//!
//! ## Process context
//!
//! In the GPKE 24h Lieferantenwechsel track a new supplier (Lieferant, LF)
//! uses this API to request the MaLo ID from the grid operator (Netzbetreiber,
//! NB) **before** sending the UTILMD. Without the MaLo ID, the UTILMD cannot
//! be routed to the correct metering point.
//!
//! ```text
//! LF → POST /maloident            (new supplier requests MaLo ID)
//! NB → POST /maloident/response   (grid operator responds with MaLo ID)
//! ```
//!
//! ## Deadline
//!
//! Under BK6-22-024, the NB must respond within **24 wall-clock hours**.
//! Use [`mako_engine::fristen::add_hours`] to compute the deadline:
//!
//! ```rust,ignore
//! use mako_engine::fristen;
//! let due = fristen::add_hours(received_at, 24);
//! ```
//!
//! ## Status
//!
//! **Not yet implemented.** The OpenAPI types and client/server stubs are
//! pending. See [`control_measures`](super::control_measures) for the
//! implemented reference implementation pattern.
//!
//! The module is declared and documented so the `[malo_ident]` doc link in the
//! parent module resolves correctly and the API surface is discoverable.

// ── Request / response types ──────────────────────────────────────────────────

/// A request from a new supplier (LF) to the grid operator (NB) to retrieve
/// the MaLo ID for a specific metering point address or OBIS reference.
///
/// POST /maloident
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaloIdentRequest {
    /// The transaction identifier assigned by the LF for this lookup.
    pub transaction_id: String,

    /// Street address or OBIS reference of the metering point.
    pub location: MaloIdentLocation,

    /// GLN of the requesting supplier.
    pub requestor_gln: String,
}

/// Location variants for a MaLo-ID lookup request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum MaloIdentLocation {
    /// Street address (Straßenadresse).
    Address(AddressLocation),
    /// OBIS key reference.
    Obis(String),
}

/// A street-address based location for MaLo-ID lookup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AddressLocation {
    /// Street name.
    pub street: String,
    /// House number.
    pub house_number: String,
    /// Postal code.
    pub postal_code: String,
    /// City.
    pub city: String,
}

/// A positive response from the NB containing the resolved MaLo ID.
///
/// POST /maloident/response (positive)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaloIdentResponsePositive {
    /// Echo of the LF's transaction ID.
    pub transaction_id: String,
    /// The resolved MaLo ID.
    pub malo_id: String,
    /// Metering point operator (MSB) GLN for the MaLo.
    pub msb_mp_id: Option<String>,
}

/// A negative response from the NB.
///
/// POST /maloident/response (negative)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaloIdentResponseNegative {
    /// Echo of the LF's transaction ID.
    pub transaction_id: String,
    /// Machine-readable reason code per BDEW Codeliste.
    pub reason_code: String,
    /// Human-readable description (optional).
    pub reason_text: Option<String>,
}

/// Combined response type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome")]
pub enum MaloIdentResponse {
    /// Grid operator found the MaLo ID.
    Positive(MaloIdentResponsePositive),
    /// Grid operator could not resolve the location.
    Negative(MaloIdentResponseNegative),
}

// ── Client / server stubs ─────────────────────────────────────────────────────

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::MaloIdentClient;

#[cfg(feature = "server")]
pub mod server;
