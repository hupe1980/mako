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
//! - The [`Claims`] extractor requires the `mako_tenant` custom claim — tokens
//!   without it are rejected with 401.  Services that call
//!   [`OidcVerifier::verify`] directly (e.g. `makod`, which authorizes via
//!   Cedar on the `sub` claim alone) accept tenant-less tokens.
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
//! | `mako_tenant` | `string` | **yes** (for the [`Claims`] extractor) | Operator GLN or tenant slug — data-isolation boundary |
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
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use subtle::ConstantTimeEq as _;
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
    /// tenant slug).  `None` when the IDP does not emit the claim; the
    /// [`Claims`] Axum extractor rejects such tokens with 401, while services
    /// that authorize on `sub` alone (e.g. `makod`'s Cedar layer) accept them.
    pub mako_tenant: Option<String>,
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

    /// The `mako_tenant` claim, when present.
    #[must_use]
    pub fn tenant(&self) -> Option<&str> {
        self.mako_tenant.as_deref()
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

    #[error("JWT is missing the required `mako_tenant` claim")]
    MissingTenant,

    #[error(
        "JWT `mako_tenant` {actual:?} does not match this deployment's tenant {expected:?} — \
         a validly signed token from another operator must not read this tenant's data"
    )]
    TenantMismatch {
        /// The tenant this deployment serves.
        expected: String,
        /// The tenant the token carries.
        actual: String,
    },
}

// ── ExpectedTenant ────────────────────────────────────────────────────────────

/// The tenant a single-tenant deployment serves, enforced at token extraction.
///
/// # Why this is an Extension and not a per-handler check
///
/// A service that pins one tenant in configuration still has to *reject* tokens
/// carrying a different one. Doing that per handler is the "auth by omission"
/// shape: it works until someone adds a route and forgets, and nothing fails
/// loudly when they do — the endpoint simply serves another operator's data to
/// anyone holding a validly signed token from the same OIDC realm.
///
/// Layering this Extension moves the check into [`Claims`] extraction, which
/// every authenticated handler already performs. A new route cannot opt out of
/// it without also opting out of authentication.
///
/// Services that derive the tenant *from* the token instead (`claims.tenant()`
/// used as the query key) do not need this — there is no second value to
/// disagree with.
#[derive(Debug, Clone)]
pub struct ExpectedTenant(pub String);

// ── OidcVerifier ─────────────────────────────────────────────────────────────

/// OIDC JWT verifier with background JWKS refresh.  Cheap to clone.
#[derive(Clone)]
pub struct OidcVerifier {
    inner: Arc<Inner>,
    /// Opt-in service-to-service keys. Empty unless configured. A non-JWT Bearer
    /// that matches one authenticates as a synthetic **service** principal —
    /// the mechanism internal callers (einsd→edmd, billingd→edmd, …) use, since
    /// no service mints real OIDC JWTs. Mirrors [`crate::mcp_auth`]'s key branch.
    service_keys: Arc<Vec<ServiceKey>>,
}

/// A shared machine-to-machine key mapping to a fixed service principal.
///
/// Comparison is constant-time. The principal carries a `sub`, the deployment
/// `tenant` (so Cedar's `principal_tenant == resource_tenant` holds) and the
/// market roles the calling service needs.
pub struct ServiceKey {
    secret: SecretString,
    sub: String,
    tenant: String,
    roles: Vec<String>,
    sparte: Vec<String>,
}

impl ServiceKey {
    /// Build a service key. `roles`/`sparte` default to all when empty.
    #[must_use]
    pub fn new(
        secret: SecretString,
        sub: impl Into<String>,
        tenant: impl Into<String>,
        roles: Vec<String>,
        sparte: Vec<String>,
    ) -> Self {
        Self {
            secret,
            sub: sub.into(),
            tenant: tenant.into(),
            roles: if roles.is_empty() {
                vec!["NB".to_owned(), "LF".to_owned(), "MSB".to_owned()]
            } else {
                roles
            },
            sparte: if sparte.is_empty() {
                vec!["STROM".to_owned(), "GAS".to_owned()]
            } else {
                sparte
            },
        }
    }

    fn matches(&self, token: &str) -> bool {
        token
            .as_bytes()
            .ct_eq(self.secret.expose_secret().as_bytes())
            .into()
    }

    fn claims(&self) -> JwtClaims {
        JwtClaims {
            sub: self.sub.clone(),
            mako_tenant: Some(self.tenant.clone()),
            mako_roles: self.roles.clone(),
            mako_sparte: self.sparte.clone(),
        }
    }
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
            service_keys: Arc::new(Vec::new()),
        }
    }

    /// Attach service-to-service keys (constant-time matched, opt-in).
    ///
    /// A non-JWT Bearer that matches one authenticates as that key's service
    /// principal. Empty by default; populated by [`OidcConfig::build_verifier`]
    /// from the `[[oidc.service_keys]]` config.
    #[must_use]
    pub fn with_service_keys(mut self, keys: Vec<ServiceKey>) -> Self {
        self.service_keys = Arc::new(keys);
        self
    }

    /// Authenticate an opaque (non-JWT) Bearer against the configured service
    /// keys, returning the matched service principal's claims.
    #[must_use]
    pub fn service_claims(&self, token: &str) -> Option<JwtClaims> {
        self.service_keys
            .iter()
            .find(|k| k.matches(token))
            .map(ServiceKey::claims)
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
            mako_tenant: Some(self.inner.disabled_tenant.clone()),
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
            .and_then(reqwest::Response::error_for_status)
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
            service_keys: Arc::new(Vec::new()),
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
            service_keys: Arc::new(Vec::new()),
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
            let cache = self
                .inner
                .cache
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            #[serde(default)]
            mako_tenant: Option<String>,
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
        *self
            .inner
            .cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = jwks;
        tracing::debug!(jwks_uri = %self.inner.jwks_uri, "OIDC: JWKS cache refreshed");
        Ok(())
    }

    /// Spawn a background Tokio task to refresh JWKS every `interval_secs` seconds.
    ///
    /// The task exits cleanly when `shutdown` is cancelled.  The returned
    /// [`tokio::task::JoinHandle`] can be awaited after cancellation to confirm
    /// the task has stopped; dropping it detaches the task.
    #[must_use]
    pub fn spawn_refresh_task(
        &self,
        http: Client,
        interval_secs: u64,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            ticker.tick().await; // skip the first (immediate) tick
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(e) = this.refresh(&http).await {
                            tracing::warn!(error = %e, "OIDC: JWKS refresh failed (will retry)");
                        }
                    }
                    () = shutdown.cancelled() => {
                        tracing::debug!("OIDC: JWKS refresh task shutting down");
                        return;
                    }
                }
            }
        })
    }

    async fn fetch_jwks_from(http: &Client, url: &str) -> Result<JwkSet, OidcError> {
        let jwks: JwkSet = http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
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
    ///
    /// Invariant: the [`FromRequestParts`] extractor rejects tokens without
    /// `mako_tenant` (401), so `Claims` obtained from a handler argument always
    /// carries a tenant.  If a `Claims` is constructed by hand from tenant-less
    /// [`JwtClaims`], this returns `""` — which fails every tenant-equality
    /// check (deny by default).
    #[must_use]
    pub fn tenant(&self) -> &str {
        self.0.mako_tenant.as_deref().unwrap_or_default()
    }

    /// Build a [`CedarPrincipal`] for use with [`crate::cedar::CedarEnforcer::check`].
    #[must_use]
    pub fn principal(&self) -> CedarPrincipal {
        CedarPrincipal {
            sub: self.0.sub.clone(),
            tenant: self.0.mako_tenant.clone().unwrap_or_default(),
            roles: self.0.mako_roles.clone(),
        }
    }
}

// ── AuthError ─────────────────────────────────────────────────────────────────

/// Rejection returned when bearer auth fails.  Renders RFC 7807 Problem Details.
#[derive(Debug)]
pub struct AuthError(pub OidcError);

impl OidcError {
    /// The detail safe to put on the wire.
    ///
    /// Most variants describe the *caller's* token and say nothing about this
    /// deployment. [`OidcError::TenantMismatch`] is the exception: its message
    /// names the tenant we serve, and the caller reaching it already holds a
    /// validly signed token — so echoing it would hand a foreign operator our
    /// tenant identifier. The full message still reaches the server log.
    fn client_detail(&self) -> String {
        match self {
            Self::TenantMismatch { .. } => "token is not valid for this deployment".to_owned(),
            other => other.to_string(),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        if let OidcError::TenantMismatch {
            ref expected,
            ref actual,
        } = self.0
        {
            tracing::warn!(
                expected_tenant = %expected,
                token_tenant = %actual,
                "rejected a validly signed token issued for another tenant"
            );
        }
        let body = serde_json::json!({
            "type":   "https://docs.mako.energy/problems/unauthorized",
            "title":  "Unauthorized",
            "status": 401u16,
            "detail": self.0.client_detail(),
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

        // Opaque (non-JWT) Bearer → service-to-service key. A service principal
        // always carries a tenant, so it satisfies the invariant below directly.
        if !OidcVerifier::looks_like_jwt(token) {
            return verifier.service_claims(token).map(Claims).ok_or(AuthError(
                OidcError::TokenInvalid(jsonwebtoken::errors::ErrorKind::InvalidToken.into()),
            ));
        }

        let claims = verifier.verify(token).map_err(AuthError)?;
        // Enforce the tenant invariant at extraction time: every `Claims`
        // handed to a handler carries `mako_tenant` (data-isolation boundary).
        let Some(ref tenant) = claims.mako_tenant else {
            return Err(AuthError(OidcError::MissingTenant));
        };
        // …and, where the deployment pins a tenant, that it is *this* one. A
        // token signed by the same realm for a different operator is otherwise
        // indistinguishable from a local one.
        if let Ok(Extension(ExpectedTenant(expected))) =
            Extension::<ExpectedTenant>::from_request_parts(parts, state).await
            && *tenant != expected
        {
            return Err(AuthError(OidcError::TenantMismatch {
                expected,
                actual: tenant.clone(),
            }));
        }
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
    /// Shared service-to-service keys accepted alongside OIDC JWTs.
    ///
    /// Each entry lets an internal caller authenticate with an opaque Bearer key
    /// (not a JWT) — the mechanism edmd/marktd use for calls from
    /// einsd/billingd/vertragd/portald, none of which mint real OIDC tokens.
    #[serde(default)]
    pub service_keys: Vec<ServiceKeyConfig>,
}

/// One `[[oidc.service_keys]]` entry — a shared key → service principal.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ServiceKeyConfig {
    /// Principal `sub` for the calling service (e.g. `"einsd"`).
    pub name: String,
    /// The shared secret. Supports `env:VAR` indirection.
    pub key: String,
    /// Market roles granted to the service (defaults to NB/LF/MSB).
    #[serde(default)]
    pub roles: Vec<String>,
    /// Sparten granted (defaults to STROM/GAS).
    #[serde(default)]
    pub sparte: Vec<String>,
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
            let _refresh = v.spawn_refresh_task(http.clone(), c.jwks_refresh_secs, shutdown);
            // Resolve any configured service-to-service keys (env: indirection).
            let mut keys = Vec::with_capacity(c.service_keys.len());
            for sk in &c.service_keys {
                let secret = crate::config::resolve_env_secret(&sk.key)
                    .with_context(|| format!("service_key {:?}", sk.name))?;
                keys.push(ServiceKey::new(
                    secret,
                    sk.name.clone(),
                    tenant_id,
                    sk.roles.clone(),
                    sk.sparte.clone(),
                ));
            }
            if !keys.is_empty() {
                tracing::info!(count = keys.len(), "OIDC: service-to-service keys enabled");
            }
            Ok(v.with_service_keys(keys))
        } else {
            tracing::warn!(
                "OIDC disabled — all requests accepted without authentication. \
                 Configure [oidc] in production."
            );
            Ok(OidcVerifier::disabled(tenant_id))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header};

    // ── Test RSA-2048 key pair (test-only, never used in production) ──────────
    //
    // Private key in PKCS#8 PEM format; JWK n/e derived from it.
    // Generated with `openssl genrsa 2048`.

    pub(super) const TEST_RSA_PRIVATE_KEY_PEM: &str = concat!(
        "-----BEGIN PRIVATE KEY-----\n",
        "MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCv6YP9yEHHvG3o\n",
        "gIPI2GVw16HoDxXnD2TnnRiQCH/ChaYOA580amRfdmnazjlpdiE+DpMtlAMEOIF9\n",
        "E/I5n9ivRdBZG0G0BurdiJ7KiYJ0aS7jfZOXknUHesPiqHxxGT4Sr3EZfuIRNq8h\n",
        "DoihfuXXJmS1oJK94FNcVyRYc8N2kwv+n++Tcu0rgLH6Ax4OWYGXR58VzmcK4zmJ\n",
        "IV37zV50rBVl3SQNZk01ZPhxdyLaIvgNrjvx7gyshob2RPJZ+xCU3vKcW90IEhAN\n",
        "cvDAoTTQylVPo/KyViwptEEi10GS127GD3U5Qz9w+YZY1FdaR0jEx+yOWUx7NOcS\n",
        "Bnu/sBt3AgMBAAECggEAUKRIpWEVwrY/Xkv33e1Rx4KajtLHlCaK9+Cc/35d7zMs\n",
        "dhUz+Sfivp5+lVdfm1iTkarFzqmhHmC2/7tSmhcMkwD6q7aijqBzL75vKOMT4kDL\n",
        "xW7uZ5g0vQqK3Q+nCIPtYEx8GReBFCoQ66MJgJs3S0Om/FpRmujI3jZ2i3P6QZMP\n",
        "rdXQRmZ8vYdqc6X/RwLOYw4JJPoCLiCMTqXoUgxWot3Mysoin6sQwPss9hV2Yz97\n",
        "V/eBIungHV+/n3AZ0XLOg8Dna2rM6+y1k/JCAXlxfAZygPvzhcFrCH/fLDEsn3SU\n",
        "qnKsCt8nIAo1LHEeTE3/2KGVIQ2ggvQPFNabrqEp1QKBgQD2xsWmeaC8NygSsD2n\n",
        "RAcwQVBiaZIP/8JA9x3Gy826CKy/cQxGgJm9hlLKexX2AKvwfaF5IQaJq6LU9qBf\n",
        "8uKs+ZL94aeTS1Fdx8JuoMz3RrsR172LQO425PVbSuglijm8zBOqOzKBjM9nrVpi\n",
        "Apxdw9w+LJUMi/VA46Cf2k+LMwKBgQC2fLGXufMFd6/sj2NfBEfRBQNL98TDiApF\n",
        "iv7Dgn47jFXGhZ0M03hvLzkNf+IdaFzlGZwLbIo3HibRwBzgnLIG4pV6TkGB8JSY\n",
        "lZvwZp4V7gc/04OBWoCQb63wioFUwJ/xmAg0LeVwLI5Q8CT8MahERKvph3PkYVb0\n",
        "J0Bd0mTOrQKBgBfu5zRiD2ixoL1PQmt6eYgAjZ89xeCvWVObo9On6Gfmd3qJqDse\n",
        "NcrfwB/LGDInloVYadSpk0y+zKgC00L692j3O35L6EiswVNrEDxSdA53WaU9WzCq\n",
        "N3AzfGhCN4mMglUBJdcYrqlJ0sOnWGCxCCE/4ZhWEo6I9Fw6t1VJgvVpAoGAFgXE\n",
        "VOAu8Nj51R2Uy3GzzQjC1hcnmsU/IBdfGW8VFtCfxV54joSywxA63WMygYQHueo2\n",
        "R7aok3BDFQsPMRgX7/bGPUVWaH0FIcjkUcXAjDr2iwBWnXSzkTq5Dg9Y/kZkxv4m\n",
        "900WpEvsPN5OSFUhzmNPL9aV6NjKapqWDPyIB90CgYBv4T8eGAgWHe88TuhbF5g6\n",
        "RUGAhxSOKQIqKqwxnTcUyn++6Tzdv5VSi+9MFHFv7LLf22SJIwbzeuNY2b+r9BqO\n",
        "1XJ8n4YQsvhchT9f1FYhg0cSsADCpoNU09Ofb1dLisWarF1OOj5HrjmR/4O/LiWC\n",
        "nwgjtyDMWSb/tW+M8+qBew==\n",
        "-----END PRIVATE KEY-----\n",
    );

    // JWK `n` (base64url-encoded modulus, no padding) derived from the key above.
    const TEST_JWK_N: &str = "r-mD_chBx7xt6ICDyNhlcNeh6A8V5w9k550YkAh_woWmDgOfNGpkX3Zp2s45aXYhPg6TL\
         ZQDBDiBfRPyOZ_Yr0XQWRtBtAbq3YieyomCdGku432Tl5J1B3rD4qh8cRk-Eq9xGX7iET\
         avIQ6IoX7l1yZktaCSveBTXFckWHPDdpML_p_vk3LtK4Cx-gMeDlmBl0efFc5nCuM5iSFd\
         -81edKwVZd0kDWZNNWT4cXci2iL4Da478e4MrIaG9kTyWfsQlN7ynFvdCBIQDXLwwKE00M\
         pVT6PyslYsKbRBItdBktduxg91OUM_cPmGWNRXWkdIxMfsjllMezTnEgZ7v7Abdw";

    // e = 65537 → AQAB
    const TEST_JWK_E: &str = "AQAB";
    pub(super) const TEST_KID: &str = "test-key-1";

    pub(super) fn test_jwks() -> JwkSet {
        serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "kid": TEST_KID,
                "alg": "RS256",
                "n": TEST_JWK_N,
                "e": TEST_JWK_E
            }]
        }))
        .expect("valid test JWK")
    }

    #[derive(serde::Serialize)]
    struct TestClaims<'a> {
        sub: &'a str,
        iss: &'a str,
        aud: &'a str,
        exp: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        nbf: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mako_tenant: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mako_roles: Option<Vec<&'a str>>,
    }

    fn encode_rs256(claims: &TestClaims<'_>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        jsonwebtoken::encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn make_rs256_token(sub: &str, iss: &str, aud: &str, exp: u64) -> String {
        encode_rs256(&TestClaims {
            sub,
            iss,
            aud,
            exp,
            nbf: None,
            mako_tenant: None,
            mako_roles: None,
        })
    }

    fn verifier() -> OidcVerifier {
        OidcVerifier::from_jwks_for_testing("https://idp.example.com", "makod", test_jwks())
    }

    // ── verify — happy path ───────────────────────────────────────────────────

    #[test]
    fn rs256_valid_sub_only_token_accepted() {
        // makod-style token: `sub` only, no mako_tenant custom claim.
        let token = make_rs256_token(
            "user-123",
            "https://idp.example.com",
            "makod",
            9_999_999_999,
        );
        let claims = verifier()
            .verify(&token)
            .expect("valid RS256 token must verify");
        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.tenant(), None);
        assert!(claims.mako_roles.is_empty());
    }

    #[test]
    fn rs256_token_with_mako_claims_accepted() {
        let token = encode_rs256(&TestClaims {
            sub: "user-456",
            iss: "https://idp.example.com",
            aud: "makod",
            exp: 9_999_999_999,
            nbf: None,
            mako_tenant: Some("9900357000004"),
            mako_roles: Some(vec!["NB", "LF"]),
        });
        let claims = verifier().verify(&token).expect("must verify");
        assert_eq!(claims.tenant(), Some("9900357000004"));
        assert!(claims.has_role("nb"));
        assert!(!claims.has_role("MSB"));
    }

    // ── verify — algorithm rejection ──────────────────────────────────────────

    #[test]
    fn hs256_token_is_rejected() {
        let header = Header::new(Algorithm::HS256);
        #[derive(serde::Serialize)]
        struct Hs256Claims {
            sub: String,
            iss: String,
            aud: Vec<String>,
            exp: u64,
        }
        let token = jsonwebtoken::encode(
            &header,
            &Hs256Claims {
                sub: "user1".to_owned(),
                iss: "https://idp.example.com".to_owned(),
                aud: vec!["makod".to_owned()],
                exp: 9_999_999_999,
            },
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap();

        assert!(matches!(
            verifier().verify(&token),
            Err(OidcError::AlgorithmDenied(_))
        ));
    }

    // ── verify — wrong audience ───────────────────────────────────────────────

    #[test]
    fn wrong_audience_rejected() {
        // Verifier expects "makod", token carries "other-service".
        let token = make_rs256_token(
            "user-123",
            "https://idp.example.com",
            "other-service",
            9_999_999_999,
        );
        assert!(matches!(
            verifier().verify(&token),
            Err(OidcError::TokenInvalid(_))
        ));
    }

    // ── verify — expired token ────────────────────────────────────────────────

    #[test]
    fn expired_token_rejected() {
        let token = make_rs256_token("user-123", "https://idp.example.com", "makod", 1); // exp=1 is in the past
        assert!(matches!(
            verifier().verify(&token),
            Err(OidcError::TokenInvalid(_))
        ));
    }

    // ── service-to-service keys ──────────────────────────────────────────────

    #[test]
    fn service_key_authenticates_an_opaque_bearer() {
        let v = verifier().with_service_keys(vec![ServiceKey::new(
            SecretString::from("s3cr3t-einsd-key"),
            "einsd",
            "9900357000004",
            vec![],
            vec![],
        )]);
        // A matching opaque key yields the service principal with the tenant.
        let claims = v.service_claims("s3cr3t-einsd-key").expect("key matches");
        assert_eq!(claims.sub, "einsd");
        assert_eq!(claims.mako_tenant.as_deref(), Some("9900357000004"));
        assert!(claims.has_role("MSB"));
        // A wrong key does not.
        assert!(v.service_claims("wrong-key").is_none());
        // An opaque token is not mistaken for a JWT.
        assert!(!OidcVerifier::looks_like_jwt("s3cr3t-einsd-key"));
        // With no keys configured, nothing authenticates by key.
        assert!(verifier().service_claims("s3cr3t-einsd-key").is_none());
    }

    // ── verify — future nbf rejected ─────────────────────────────────────────

    #[test]
    fn future_nbf_rejected() {
        let token = encode_rs256(&TestClaims {
            sub: "user-123",
            iss: "https://idp.example.com",
            aud: "makod",
            exp: 9_999_999_999,
            nbf: Some(4_000_000_000), // ~year 2096 — not-yet-valid
            mako_tenant: None,
            mako_roles: None,
        });
        assert!(matches!(
            verifier().verify(&token),
            Err(OidcError::TokenInvalid(_))
        ));
    }

    // ── verify — unknown kid ──────────────────────────────────────────────────

    #[test]
    fn unknown_kid_returns_error() {
        let token = make_rs256_token(
            "user-123",
            "https://idp.example.com",
            "makod",
            9_999_999_999,
        );
        // Empty JWKS — kid "test-key-1" will not be found.
        let verifier = OidcVerifier::from_jwks_for_testing(
            "https://idp.example.com",
            "makod",
            JwkSet { keys: vec![] },
        );
        assert!(matches!(
            verifier.verify(&token),
            Err(OidcError::UnknownKid(_))
        ));
    }

    // ── Claims tenant invariant ───────────────────────────────────────────────

    #[test]
    fn claims_tenant_returns_str_when_present() {
        let claims = Claims(JwtClaims {
            sub: "u".to_owned(),
            mako_tenant: Some("9900357000004".to_owned()),
            mako_roles: vec![],
            mako_sparte: vec![],
        });
        assert_eq!(claims.tenant(), "9900357000004");
    }

    #[test]
    fn claims_tenant_defaults_to_empty_for_tenantless_token() {
        // Hand-constructed Claims without a tenant deny by default ("" never
        // equals a real tenant GLN).  The Axum extractor never produces this.
        let claims = Claims(JwtClaims {
            sub: "u".to_owned(),
            mako_tenant: None,
            mako_roles: vec![],
            mako_sparte: vec![],
        });
        assert_eq!(claims.tenant(), "");
        assert_eq!(claims.principal().tenant, "");
    }
}

#[cfg(test)]
mod expected_tenant_tests {
    use super::*;

    /// A tenant mismatch must not echo this deployment's tenant back.
    ///
    /// The caller reaching this branch already holds a validly signed token —
    /// they proved they belong to the realm, just not to us. Returning the
    /// expected value would hand a foreign operator our tenant identifier for
    /// free; the full detail belongs in the server log instead.
    #[test]
    fn the_mismatch_detail_does_not_leak_the_expected_tenant() {
        let err = OidcError::TenantMismatch {
            expected: "9900357000004".to_owned(),
            actual: "9900987654321".to_owned(),
        };
        let detail = err.client_detail();
        assert!(
            !detail.contains("9900357000004"),
            "the response must not name our tenant: {detail}"
        );
        assert!(
            !detail.contains("9900987654321"),
            "nor echo the caller's: {detail}"
        );
        // …while the full message, which reaches the log, names both.
        let full = err.to_string();
        assert!(
            full.contains("9900357000004") && full.contains("9900987654321"),
            "{full}"
        );
    }

    /// Every other variant keeps its detail — those describe the caller's own
    /// token and disclose nothing about the deployment.
    #[test]
    fn other_variants_keep_their_detail() {
        let err = OidcError::MissingTenant;
        assert_eq!(err.client_detail(), err.to_string());
    }
}

#[cfg(test)]
mod expected_tenant_extraction_tests {
    use super::tests::{TEST_KID, TEST_RSA_PRIVATE_KEY_PEM, test_jwks};
    use super::*;
    use axum::http::Request;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};

    /// Mint a real RS256 token carrying `mako_tenant`, signed by the test key.
    fn token_for_tenant(tenant: &str) -> String {
        #[derive(serde::Serialize)]
        struct C<'a> {
            sub: &'a str,
            iss: &'a str,
            aud: &'a str,
            exp: u64,
            mako_tenant: &'a str,
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        jsonwebtoken::encode(
            &header,
            &C {
                sub: "user-1",
                iss: "https://idp.example.com",
                aud: "makod",
                exp: 9_999_999_999,
                mako_tenant: tenant,
            },
            &EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    async fn extract(token: &str, expected: Option<&str>) -> Result<Claims, AuthError> {
        let verifier =
            OidcVerifier::from_jwks_for_testing("https://idp.example.com", "makod", test_jwks());
        let mut builder = Request::builder()
            .uri("/")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .extension(verifier);
        if let Some(t) = expected {
            builder = builder.extension(ExpectedTenant(t.to_owned()));
        }
        let (mut parts, ()) = builder.body(()).unwrap().into_parts();
        Claims::from_request_parts(&mut parts, &()).await
    }

    /// The defect this closes: a token signed by the same realm for a different
    /// operator is cryptographically valid and, without the gate, would be
    /// served this deployment's customer data.
    #[tokio::test]
    async fn a_validly_signed_token_for_another_tenant_is_rejected() {
        let err = extract(&token_for_tenant("9900987654321"), Some("9900357000004"))
            .await
            .expect_err("a foreign tenant must not authenticate");
        match err.0 {
            OidcError::TenantMismatch { expected, actual } => {
                assert_eq!(expected, "9900357000004");
                assert_eq!(actual, "9900987654321");
            }
            other => panic!("expected TenantMismatch, got {other:?}"),
        }
    }

    /// The matching tenant passes — the gate rejects the foreign token, not
    /// every token.
    #[tokio::test]
    async fn the_deployments_own_tenant_is_accepted() {
        let claims = extract(&token_for_tenant("9900357000004"), Some("9900357000004"))
            .await
            .expect("the local tenant must authenticate");
        assert_eq!(claims.tenant(), "9900357000004");
    }

    /// Without the Extension the gate stands down, so services that derive the
    /// tenant from the token are unaffected.
    #[tokio::test]
    async fn without_the_extension_any_tenant_passes() {
        let claims = extract(&token_for_tenant("9900987654321"), None)
            .await
            .expect("no ExpectedTenant layered → no comparison");
        assert_eq!(claims.tenant(), "9900987654321");
    }

    /// A `Claims` built by hand without a tenant fails every equality check —
    /// deny by default, as the type's own docs promise.
    #[test]
    fn a_tenantless_claims_matches_no_tenant() {
        let c = Claims(JwtClaims {
            sub: "x".to_owned(),
            mako_tenant: None,
            mako_roles: vec![],
            mako_sparte: vec![],
        });
        assert_eq!(c.tenant(), "");
        assert_ne!(c.tenant(), "9900357000004");
    }
}
