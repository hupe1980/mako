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

use energy_api::models::electricity::LocationId as ApiLocationId;
use mako_engine::types::MarktpartnerCode;
use mako_wim::steuerungsauftrag::LocationId as DomainLocationId;

/// Convert an `energy-api` [`ApiLocationId`] into the validated domain
/// [`DomainLocationId`].
///
/// - `NetworkLocation(NeloId)` → `DomainLocationId::Nelo(mako_markt::domain::NeloId)`
/// - `ControllableResource(SrId)` → `DomainLocationId::Sr(mako_markt::domain::SrId)`
///
/// Returns `Err(String)` with the raw ID value when rubo4e validation fails,
/// so callers can surface a `400 Bad Request` to the NB/LF.
///
/// # Errors
///
/// The API and domain layers now share the same validated identifier types
/// (`rubo4e::identifiers`), so this is a variant remap with no re-parsing: the
/// check digit was already enforced when the request was deserialized.
#[must_use]
pub fn location_id_to_domain(id: &ApiLocationId) -> DomainLocationId {
    match id {
        ApiLocationId::NetworkLocation(nelo) => DomainLocationId::Nelo(nelo.clone()),
        ApiLocationId::ControllableResource(sr) => DomainLocationId::Sr(sr.clone()),
    }
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
    fn location_id_nelo_to_domain() {
        // NeloId v0.6: Codetyp 'E' + 9 [A-Z0-9] + ASCII-Verfahren check digit.
        // "E0000000001" is NeloId::from_base("E000000000").
        let id = LocationId::NetworkLocation(NeloId::new("E0000000001").expect("valid NeloId"));
        assert!(matches!(
            location_id_to_domain(&id),
            DomainLocationId::Nelo(_)
        ));
    }

    #[test]
    fn location_id_sr_to_domain() {
        // SrId v0.6: Codetyp 'C' + 9 [A-Z0-9] + ASCII-Verfahren check digit.
        // "C0000000003" is SrId::from_base("C000000000").
        let id = LocationId::ControllableResource(SrId::new("C0000000003").expect("valid SrId"));
        assert!(matches!(
            location_id_to_domain(&id),
            DomainLocationId::Sr(_)
        ));
    }

    #[test]
    fn party_id_roundtrip() {
        let mc = party_id_to_marktpartner("9900000000002");
        assert_eq!(mc.as_str(), "9900000000002");
    }
}
