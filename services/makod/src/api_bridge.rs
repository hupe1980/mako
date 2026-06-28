//! `api_bridge` — thin conversion layer between `energy-api` types and
//! `mako-engine` domain types.
//!
//! # Motivation
//!
//! `energy-api` is a standalone crate with no dependency on `mako-engine`; it
//! exposes its own identifier types (`LocationId`, `IdentificationParameter`,
//! etc.). Every value crossing the `energy-api` → `mako-engine` boundary inside
//! `makod` passes through this module, so conversions are auditable in one place
//! and protected by compile-time type checks.
//!
//! # What this module does NOT do
//!
//! - It does not carry business logic.
//! - It does not perform I/O.
//! - It does not validate domain invariants beyond what the types already
//!   guarantee (e.g. it does not check GLN check digits).
//!
//! # Adding new bridges
//!
//! When a new `energy-api` type needs to enter the engine, add a conversion here
//! and import it from the relevant `makod` module.

use energy_api::models::electricity::LocationId;
use mako_engine::types::MarktpartnerCode;

/// Convert an `energy-api` [`LocationId`] to the `String` representation
/// expected by domain commands.
///
/// Both `NeloId` and `SrId` implement `Display` — the string representation is
/// their raw identifier value (e.g. `"DE000..." for a NELO ID or an SR ID).
#[must_use]
pub fn location_id_to_string(id: &LocationId) -> String {
    id.to_string()
}

/// Convert a raw party-ID string (GLN, BDEW code, or EIC) from a request
/// context into a [`MarktpartnerCode`] domain value.
///
/// `MarktpartnerCode::new` accepts any `&str`; this wrapper documents the
/// conversion intent and centralises the call site.
#[must_use]
pub fn party_id_to_marktpartner(party_id: impl Into<String>) -> MarktpartnerCode {
    MarktpartnerCode::new(party_id.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use energy_api::models::electricity::{LocationId, NeloId, SrId};

    #[test]
    fn location_id_nelo_to_string() {
        let id = LocationId::NetworkLocation(NeloId("DE000123456789".into()));
        assert_eq!(location_id_to_string(&id), "DE000123456789");
    }

    #[test]
    fn location_id_sr_to_string() {
        let id = LocationId::ControllableResource(SrId("SR-12345".into()));
        assert_eq!(location_id_to_string(&id), "SR-12345");
    }

    #[test]
    fn party_id_roundtrip() {
        let mc = party_id_to_marktpartner("9900000000002");
        assert_eq!(mc.as_str(), "9900000000002");
    }
}
