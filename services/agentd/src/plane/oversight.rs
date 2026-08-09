//! The human half: a worklist an approver can actually reach.
//!
//! A manifest that can act declares `oversight` — an approval mode, the roles
//! eligible to give it, a deadline, and what happens when the deadline passes —
//! and marks its mutating grants `requires_approval: true`. Reaching one
//! suspends the run and opens a **task**. Without a surface to answer it, the
//! declaration is decoration: the run waits for a decision nobody can make,
//! until its deadline expires and `on_expiry: deny` fails it.
//!
//! This module mounts agentplane's operator surface — the worklist, run status,
//! case history, cancellation, event delivery — authenticated by mako's own
//! OIDC verifier and authorized by the Cedar policy set in
//! [`policy`](super::policy).
//!
//! ## Identity comes from the token, never from the body
//!
//! agentplane's `DecisionRequest` has no `actor` field and no `roles` field, on
//! purpose: a reviewer who can name themselves can name the person who proposed
//! the action, which inverts four-eyes rather than enforcing it. The `Caller`
//! this module builds comes from the verified JWT and nothing else.
//!
//! ## No identity, no surface
//!
//! When OIDC is disabled — mako's dev mode — the routes are **not mounted**.
//! Every other dev-mode relaxation in this codebase accepts an unauthenticated
//! request and warns; an approval is the one place where that is not a
//! relaxation but a forged signature on a regulated dispatch.

use std::sync::Arc;

use agentplane::api::{Api, AuthError, Authenticator, Caller, Planes};
use agentplane::core::TenantId;
use agentplane::runtime::Runtime;
use axum::http::HeaderMap;
use mako_service::oidc::OidcVerifier;

/// Where the operator surface is mounted.
///
/// Under `/api/v1` like every other mako route, and namespaced so agentplane's
/// `/runs` and `/tasks` cannot collide with a service route added later.
pub const MOUNT: &str = "/api/v1/oversight";

/// mako's OIDC verifier, as agentplane's [`Authenticator`].
///
/// The whole header map arrives rather than a parsed token, because a
/// deployment may authenticate by bearer token, mutual TLS or a gateway header.
/// mako uses the first, and the service keys the other daemons present are
/// verified by the same code path — so `POST /events` from `makod` and a
/// decision from a person are authenticated identically and separated by role.
pub struct OidcAuthenticator {
    verifier: Arc<OidcVerifier>,
}

/// Hand-written because [`OidcVerifier`] holds keys and secrets and derives no
/// `Debug` of its own — and the trait requires one. Printing the verifier would
/// be how a service key reaches a log.
impl std::fmt::Debug for OidcAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthenticator").finish_non_exhaustive()
    }
}

impl OidcAuthenticator {
    #[must_use]
    pub fn new(verifier: Arc<OidcVerifier>) -> Self {
        Self { verifier }
    }

    /// The bearer token, if one was presented.
    fn bearer(headers: &HeaderMap) -> Option<&str> {
        headers
            .get(axum::http::header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")
            .map(str::trim)
    }
}

#[async_trait::async_trait]
impl Authenticator for OidcAuthenticator {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Caller, AuthError> {
        let token = Self::bearer(headers).ok_or(AuthError::Missing)?;

        // Service keys first: mako's daemons authenticate with a shared secret
        // rather than a JWT, and `service_claims` is what recognises one. It
        // returns the roles the operator granted that key, so a service cannot
        // widen its own authority by presenting a different header.
        let claims = self
            .verifier
            .service_claims(token)
            .map_or_else(|| self.verifier.verify(token), Ok)
            .map_err(|_| AuthError::Rejected)?;

        // A decision is recorded permanently under this name. An absent `sub`
        // would put an unnamed actor on an approval, so it is a refusal.
        if claims.sub.is_empty() {
            return Err(AuthError::Rejected);
        }

        // The tenant decides *which store* answers, so it comes from the
        // credential like everything else here. A token with no `mako_tenant`
        // cannot be resolved to a plane, and defaulting would serve one
        // operator's worklist to another.
        let tenant = claims
            .tenant()
            .ok_or(AuthError::Rejected)
            .and_then(|t| TenantId::new(t).map_err(|_| AuthError::Rejected))?;

        Ok(Caller::new(claims.sub.clone(), claims.mako_roles.clone()).in_tenant(tenant))
    }
}

/// Build the operator surface over one plane.
///
/// # Errors
///
/// When the runtime has no policy engine — agentplane refuses to open an
/// ungoverned port, and so does this — or when no plane was registered.
pub fn router(runtime: Arc<Runtime>, verifier: Arc<OidcVerifier>) -> Result<axum::Router, String> {
    let auth = Arc::new(OidcAuthenticator::new(verifier)) as Arc<dyn Authenticator>;
    let api = Api::new(Planes::one(runtime), auth)
        .map_err(|e| format!("build the oversight surface: {e}"))?;
    Ok(api.router())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            value.parse().expect("header value"),
        );
        h
    }

    fn authenticator() -> OidcAuthenticator {
        OidcAuthenticator::new(Arc::new(OidcVerifier::disabled("9900357000004")))
    }

    /// No credential is refused, and refused as *missing* rather than rejected.
    ///
    /// The distinction is what lets a client tell "you did not authenticate"
    /// from "your token is not accepted" without the surface saying which of
    /// expiry, audience or signature failed.
    #[tokio::test]
    async fn a_request_with_no_bearer_is_refused() {
        let err = authenticator()
            .authenticate(&HeaderMap::new())
            .await
            .expect_err("no credential");
        assert!(matches!(err, AuthError::Missing));
    }

    /// A malformed bearer is rejected rather than treated as anonymous.
    #[tokio::test]
    async fn a_garbage_token_is_rejected() {
        let err = authenticator()
            .authenticate(&headers("Bearer not-a-jwt"))
            .await
            .expect_err("bad token");
        assert!(matches!(err, AuthError::Rejected));
    }

    /// A scheme other than Bearer carries no identity here.
    #[tokio::test]
    async fn basic_auth_is_not_a_credential_on_this_surface() {
        let err = authenticator()
            .authenticate(&headers("Basic dXNlcjpwYXNz"))
            .await
            .expect_err("wrong scheme");
        assert!(matches!(err, AuthError::Missing));
    }
}
