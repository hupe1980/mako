//! OIDC/JWT token verification for mako services.
//!
//! Provides [`OidcVerifier`] for validating Bearer tokens from any OIDC issuer,
//! and the [`Claims`] Axum extractor that services use in handlers.
//!
//! ## Security properties
//!
//! - Accepts only asymmetric algorithms: RS256/384/512, ES256/384, PS256/384/512
//! - HS* (symmetric HMAC) algorithms are rejected unconditionally
//! - JWKS is cached in-process; a background task refreshes it on a configurable interval
//! - The `mako_tenant` custom claim is required — tokens without it are rejected
//!
//! ## Quick-start
//!
//! ```rust,no_run
//! use mako_service::oidc::{Claims, OidcVerifier};
//! use axum::{Router, routing::get};
//!
//! async fn my_handler(claims: Claims) -> String {
//!     format!("Hello, tenant {}", claims.tenant())
//! }
//!
//! // At startup (no OIDC in dev):
//! let verifier = OidcVerifier::disabled("my-tenant-gln");
//! // In production: OidcVerifier::new(issuer, audience, &http).await?
//!
//! let app: Router = Router::new()
//!     .route("/api/resource", get(my_handler))
//!     .layer(axum::Extension(verifier));
//! ```
//!
//! ## Custom JWT claims (required in IDP configuration)
//!
//! | Claim | Type | Required | Description |
//! |---|---|---|---|
//! | `mako_tenant` | `string` | **yes** | Operator GLN or tenant slug — data-isolation boundary |
//! | `mako_roles`  | `string[]` | no | Energy-market roles: `"NB"`, `"LF"`, `"MSB"`, … |
//! | `mako_sparte` | `string[]` | no | Grid commodity: `"STROM"`, `"GAS"` |

use std::sync::{Arc, RwLock};

use axum::{
    Extension,
    extract::FromRequestParts,
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::cedar::CedarPrincipal;

// ── Allowed algorithms ────────────────────────────────────────────────────────

const ALLOWED_ALGORITHMS: &[Algorithm] = &[
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
];

// ── JwtClaims ─────────────────────────────────────────────────────────────────

/// Verified claims extracted from a valid JWT.
#[derive(Debug, Clone)]
pub struct JwtClaims {
    /// `sub` — unique user identifier; used as the Cedar principal entity ID.
    pub sub: String,
    /// Custom claim `mako_tenant` — data-isolation boundary (operator GLN or
    /// tenant slug).  The IDP must include this claim in every token.
    pub mako_tenant: String,
    /// Custom claim `mako_roles: ["NB", "LF", ...]` — energy-market roles.
    pub mako_roles: Vec<String>,
    /// Custom claim `mako_sparte: ["STROM", "GAS"]`.
    pub mako_sparte: Vec<String>,
}

impl JwtClaims {
    /// Returns `true` if the caller holds `role` (case-insensitive).
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.mako_roles.iter().any(|r| r.eq_ignore_ascii_case(role))
    }
}

// ── OidcError ─────────────────────────────────────────────────────────────────

/// Errors produced by [`OidcVerifier`].
#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("OIDC discovery failed (url={url}): {reason}")]
    Discovery { url: String, reason: String },

    #[error("JWKS fetch failed (url={url}): {reason}")]
    JwksFetch { url: String, reason: String },

    #[error("JWT invalid: {0}")]
    TokenInvalid(#[from] jsonwebtoken::errors::Error),

    #[error("OIDC issuer mismatch: configured {expected:?}, discovery returned {actual:?}")]
    IssuerMismatch { expected: String, actual: String },

    #[error("JWT `kid` is missing")]
    MissingKid,

    #[error("JWT `kid` {0:?} is not in the current JWKS (key rotation in progress?)")]
    UnknownKid(String),

    #[error("JWT algorithm {0:?} is not permitted; only asymmetric algorithms are accepted")]
    AlgorithmDenied(String),
}

// ── OidcVerifier ─────────────────────────────────────────────────────────────

/// OIDC JWT verifier with background JWKS refresh.  Cheap to clone.
#[derive(Clone)]
pub struct OidcVerifier {
    inner: Arc<Inner>,
}

struct Inner {
    issuer: String,
    audience: String,
    jwks_uri: String,
    cache: RwLock<JwkSet>,
    /// When `true`, all requests are accepted with synthetic dev-admin claims.
    /// Never use in production.
    disabled: bool,
    /// Tenant GLN for synthetic claims (only used when `disabled = true`).
    disabled_tenant: String,
}

#[derive(Deserialize)]
struct OidcDiscovery {
    issuer: String,
    jwks_uri: String,
}

impl OidcVerifier {
    /// Build a disabled (dev-only) [`OidcVerifier`] that accepts all requests
    /// without a token and returns synthetic dev-admin claims scoped to `tenant_id`.
    ///
    /// **Never use in production.**
    pub fn disabled(tenant_id: impl Into<String>) -> Self {
        let tenant_id = tenant_id.into();
        Self {
            inner: Arc::new(Inner {
                issuer: String::new(),
                audience: String::new(),
                jwks_uri: String::new(),
                cache: RwLock::new(JwkSet { keys: vec![] }),
                disabled: true,
                disabled_tenant: tenant_id,
            }),
        }
    }

    /// Returns `true` when this verifier was created with [`OidcVerifier::disabled`].
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.inner.disabled
    }

    /// Returns `true` if `token` looks like a JWT (three non-empty dot-separated parts).
    ///
    /// Used by [`crate::mcp_auth::McpAuth`] to route incoming Bearer tokens without
    /// attempting to parse opaque API keys as JWTs.  A JWT always has exactly three
    /// base64url-encoded parts: `header.payload.signature`.  API keys are typically
    /// random hex or base64 strings without dots.
    ///
    /// This is a cheap structural check — it does NOT verify the token.
    #[must_use]
    pub fn looks_like_jwt(token: &str) -> bool {
        let mut parts = token.splitn(4, '.');
        parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_none() // exactly 3 parts, not 4
    }

    /// Returns synthetic dev-admin claims for use when auth is disabled.
    #[must_use]
    pub fn disabled_claims(&self) -> JwtClaims {
        JwtClaims {
            sub: "dev-admin".to_owned(),
            mako_tenant: self.inner.disabled_tenant.clone(),
            mako_roles: vec!["NB".to_owned(), "LF".to_owned(), "MSB".to_owned()],
            mako_sparte: vec!["STROM".to_owned(), "GAS".to_owned()],
        }
    }

    /// Build an [`OidcVerifier`] via OIDC discovery.
    pub async fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        http: &Client,
    ) -> Result<Self, OidcError> {
        let issuer = issuer.into();
        let audience = audience.into();

        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let disc: OidcDiscovery = http
            .get(&discovery_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::Discovery {
                url: discovery_url.clone(),
                reason: e.to_string(),
            })?
            .json()
            .await
            .map_err(|e| OidcError::Discovery {
                url: discovery_url,
                reason: e.to_string(),
            })?;

        tracing::info!(issuer = %issuer, jwks_uri = %disc.jwks_uri, "OIDC: discovery succeeded");

        if disc.issuer != issuer {
            return Err(OidcError::IssuerMismatch {
                expected: issuer,
                actual: disc.issuer,
            });
        }

        let jwks = Self::fetch_jwks_from(http, &disc.jwks_uri).await?;

        Ok(Self {
            inner: Arc::new(Inner {
                issuer,
                audience,
                jwks_uri: disc.jwks_uri,
                cache: RwLock::new(jwks),
                disabled: false,
                disabled_tenant: String::new(),
            }),
        })
    }

    #[cfg(test)]
    pub fn from_jwks_for_testing(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks: JwkSet,
    ) -> Self {
        let issuer = issuer.into();
        Self {
            inner: Arc::new(Inner {
                jwks_uri: format!("{issuer}/.well-known/jwks.json"),
                issuer,
                audience: audience.into(),
                cache: RwLock::new(jwks),
                disabled: false,
                disabled_tenant: String::new(),
            }),
        }
    }

    /// Verify a JWT and return its claims.  Non-blocking — uses cached JWKS.
    pub fn verify(&self, token: &str) -> Result<JwtClaims, OidcError> {
        let header = jsonwebtoken::decode_header(token)?;

        if !ALLOWED_ALGORITHMS.contains(&header.alg) {
            return Err(OidcError::AlgorithmDenied(format!("{:?}", header.alg)));
        }

        let kid = header.kid.ok_or(OidcError::MissingKid)?;

        let decoding_key = {
            let cache = self.inner.cache.read().unwrap_or_else(|p| p.into_inner());
            let jwk = cache
                .find(&kid)
                .ok_or_else(|| OidcError::UnknownKid(kid.clone()))?;
            DecodingKey::from_jwk(jwk)?
        };

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.inner.issuer]);
        validation.set_audience(&[&self.inner.audience]);
        validation.validate_nbf = true;
        validation.required_spec_claims.insert("sub".to_owned());

        #[derive(Deserialize)]
        struct RawClaims {
            sub: String,
            mako_tenant: String,
            #[serde(default)]
            mako_roles: Vec<String>,
            #[serde(default)]
            mako_sparte: Vec<String>,
        }

        let data = jsonwebtoken::decode::<RawClaims>(token, &decoding_key, &validation)?;
        Ok(JwtClaims {
            sub: data.claims.sub,
            mako_tenant: data.claims.mako_tenant,
            mako_roles: data.claims.mako_roles,
            mako_sparte: data.claims.mako_sparte,
        })
    }

    /// Refresh the JWKS cache.
    pub async fn refresh(&self, http: &Client) -> Result<(), OidcError> {
        let jwks = Self::fetch_jwks_from(http, &self.inner.jwks_uri).await?;
        *self.inner.cache.write().unwrap_or_else(|p| p.into_inner()) = jwks;
        tracing::debug!(jwks_uri = %self.inner.jwks_uri, "OIDC: JWKS cache refreshed");
        Ok(())
    }

    /// Spawn a background Tokio task to refresh JWKS every `interval_secs` seconds.
    pub fn spawn_refresh_task(self, http: Client, interval_secs: u64, shutdown: CancellationToken) {
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(interval_secs);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {
                        if let Err(e) = self.refresh(&http).await {
                            tracing::warn!(error = %e, "OIDC: JWKS refresh failed (will retry)");
                        }
                    }
                    _ = shutdown.cancelled() => {
                        tracing::debug!("OIDC: JWKS refresh task shutting down");
                        break;
                    }
                }
            }
        });
    }

    async fn fetch_jwks_from(http: &Client, url: &str) -> Result<JwkSet, OidcError> {
        let jwks: JwkSet = http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::JwksFetch {
                url: url.to_owned(),
                reason: e.to_string(),
            })?
            .json()
            .await
            .map_err(|e| OidcError::JwksFetch {
                url: url.to_owned(),
                reason: e.to_string(),
            })?;
        Ok(jwks)
    }
}

// ── Claims Axum extractor ─────────────────────────────────────────────────────

/// JWT claims extracted from `Authorization: Bearer <token>`.
///
/// The `OidcVerifier` must be injected via `Extension<OidcVerifier>` at the
/// router level.  Handlers declare `claims: Claims` to require authentication.
///
/// **Dev bypass:** When the `OidcVerifier` was created with
/// [`OidcVerifier::disabled`], all requests pass with synthetic dev-admin claims.
/// Never configure `disabled()` in production.
#[derive(Debug, Clone)]
pub struct Claims(pub JwtClaims);

impl Claims {
    /// Returns `true` if the caller holds `role` (case-insensitive).
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.0.has_role(role)
    }

    /// Subject claim (`sub`).
    #[must_use]
    pub fn sub(&self) -> &str {
        &self.0.sub
    }

    /// Returns the caller's tenant (data-isolation boundary from `mako_tenant` claim).
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.0.mako_tenant
    }

    /// Build a [`CedarPrincipal`] for use with [`crate::cedar::CedarEnforcer::check`].
    #[must_use]
    pub fn principal(&self) -> CedarPrincipal {
        CedarPrincipal {
            sub: self.0.sub.clone(),
            tenant: self.0.mako_tenant.clone(),
            roles: self.0.mako_roles.clone(),
        }
    }
}

// ── AuthError ─────────────────────────────────────────────────────────────────

/// Rejection returned when bearer auth fails.  Renders RFC 7807 Problem Details.
#[derive(Debug)]
pub struct AuthError(pub OidcError);

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "type":   "https://docs.mako.energy/problems/unauthorized",
            "title":  "Unauthorized",
            "status": 401u16,
            "detail": self.0.to_string(),
        });
        let mut resp = (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/problem+json"
                .parse()
                .expect("valid header value"),
        );
        resp
    }
}

impl<S> FromRequestParts<S> for Claims
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Extension(verifier): Extension<OidcVerifier> =
            Extension::from_request_parts(parts, state)
                .await
                .map_err(|_| AuthError(OidcError::MissingKid))?;

        if verifier.is_disabled() {
            return Ok(Claims(verifier.disabled_claims()));
        }

        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        let token = bearer.ok_or(AuthError(OidcError::MissingKid))?;

        let claims = verifier.verify(token).map_err(AuthError)?;
        Ok(Claims(claims))
    }
}

// ── OidcConfig ────────────────────────────────────────────────────────────────

/// Standard OIDC configuration block, shared across **all** mako services.
///
/// Add to your service config struct as an optional field:
///
/// ```rust
/// # use mako_service::oidc::OidcConfig;
/// #[derive(serde::Deserialize)]
/// struct MyConfig {
///     pub tenant: String,
///     pub oidc: Option<OidcConfig>,
/// }
/// ```
///
/// The corresponding TOML section is optional — when absent,
/// [`OidcConfig::build_verifier`] returns a disabled verifier (dev mode):
///
/// ```toml
/// # Production:
/// [oidc]
/// issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
/// audience = "api://mako-myservice"
///
/// # Dev mode: omit the [oidc] section entirely.
/// ```
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OidcConfig {
    /// OIDC issuer URL (without trailing slash).
    pub issuer: String,
    /// JWT `aud` claim expected value.
    pub audience: String,
    /// JWKS background refresh interval in seconds.  Default: 300 (5 min).
    #[serde(default = "OidcConfig::default_jwks_refresh_secs")]
    pub jwks_refresh_secs: u64,
}

impl OidcConfig {
    fn default_jwks_refresh_secs() -> u64 {
        300
    }

    /// Build an [`OidcVerifier`] from this config.
    ///
    /// - **Present config** → performs OIDC discovery, loads JWKS, spawns a
    ///   background refresh task that cancels with `shutdown`.
    /// - **`None` config** → returns [`OidcVerifier::disabled`] scoped to
    ///   `tenant_id` (dev mode — all requests accepted without a token).
    ///
    /// This replaces the identical 8-line boilerplate that every OIDC service
    /// copied into its startup code:
    ///
    /// ```rust,no_run
    /// # use mako_service::oidc::{OidcConfig, OidcVerifier};
    /// # use tokio_util::sync::CancellationToken;
    /// # use reqwest::Client;
    /// # async fn run(oidc: Option<OidcConfig>, http: Client, ct: CancellationToken) -> anyhow::Result<()> {
    /// let verifier = OidcConfig::build_verifier(oidc.as_ref(), &http, "my-tenant", ct).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Err` when OIDC discovery fails (network unreachable, TLS error,
    /// issuer mismatch).
    pub async fn build_verifier(
        cfg: Option<&OidcConfig>,
        http: &Client,
        tenant_id: &str,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<OidcVerifier> {
        use anyhow::Context as _;
        if let Some(c) = cfg {
            let v = OidcVerifier::new(&c.issuer, &c.audience, http)
                .await
                .context("OIDC discovery")?;
            v.clone()
                .spawn_refresh_task(http.clone(), c.jwks_refresh_secs, shutdown);
            Ok(v)
        } else {
            tracing::warn!(
                "OIDC disabled — all requests accepted without authentication. \
                 Configure [oidc] in production."
            );
            Ok(OidcVerifier::disabled(tenant_id))
        }
    }
}
