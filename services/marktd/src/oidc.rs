//! OIDC/JWT verification for `marktd`.
//!
//! Re-exports from `mako-service` — the authoritative implementation lives there
//! so all mako services share the same verifier without code duplication.
pub use mako_service::oidc::{JwtClaims, OidcError, OidcVerifier};
