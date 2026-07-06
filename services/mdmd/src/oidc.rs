//! OIDC/JWT token verification for `mdmd`.
//!
//! Adapted from `makod/src/oidc_verifier.rs` — same security properties,
//! but also extracts `mako_roles: Vec<String>` and `mako_sparte: Vec<String>`
//! custom claims for RBAC.

use std::sync::{Arc, RwLock};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

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

// ── Public types ──────────────────────────────────────────────────────────────

/// Verified claims extracted from a valid JWT.
#[derive(Debug, Clone)]
pub struct JwtClaims {
    /// `sub` — used as the Cedar principal entity ID.
    pub sub: String,
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
}

#[derive(Deserialize)]
struct OidcDiscovery {
    issuer: String,
    jwks_uri: String,
}

impl OidcVerifier {
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
            #[serde(default)]
            mako_roles: Vec<String>,
            #[serde(default)]
            mako_sparte: Vec<String>,
        }

        let data = jsonwebtoken::decode::<RawClaims>(token, &decoding_key, &validation)?;
        Ok(JwtClaims {
            sub: data.claims.sub,
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
