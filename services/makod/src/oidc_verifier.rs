//! OIDC/JWT token verification for `makod`.
//!
//! Validates `Authorization: Bearer <jwt>` tokens issued by any standards-compliant
//! OIDC identity provider — Azure AD/Entra ID, Keycloak, Okta, Google Workspace,
//! AWS Cognito, Kubernetes workload-identity, and others.
//!
//! ## How it works
//!
//! 1. At startup, `OidcVerifier::new()` fetches the issuer's OIDC discovery
//!    document (`/.well-known/openid-configuration`) to locate the JWKS endpoint.
//! 2. The JWKS is fetched and cached in memory.
//! 3. Token verification is **synchronous** — it reads the cached JWKS, selects
//!    the matching key by `kid`, and performs local asymmetric signature
//!    verification.  No per-request network round-trips.
//! 4. A background task (started with [`OidcVerifier::spawn_refresh_task`])
//!    refreshes the JWKS every `refresh_interval_secs` seconds to handle key
//!    rotation without restarting the daemon.
//!
//! ## Security properties
//!
//! - Only **asymmetric algorithms** (RS256/RS384/RS512, ES256/ES384, PS256/PS384/PS512)
//!   are accepted.  HMAC algorithms (`HS256`, `HS384`, `HS512`) are explicitly
//!   rejected: they require a shared secret and cannot be independently verified.
//! - `iss` and `aud` claims are validated on every token.
//! - `exp` (expiry) is validated by the `jsonwebtoken` crate.
//! - The JWKS cache is atomically swapped on refresh using `std::sync::RwLock`.
//!   No lock is held across `.await` points.
//!
//! ## Integration with Cedar
//!
//! The JWT `sub` claim becomes the Cedar principal entity ID
//! (`MaKo::Principal::"<sub>"`).  All Cedar policies — whether written for
//! API-key principals or OIDC principals — reference the same entity type.
//! API-key authentication and OIDC can run simultaneously on the same port;
//! the token shape (JWT vs opaque hex) determines which path is taken.
//!
//! ## Example: restrict an OIDC service account
//!
//! ```cedar
//! // Allow the Azure Managed Identity "prod-erp-app" to submit commands
//! // but not perform admin operations.
//! forbid(
//!   principal == MaKo::Principal::"api://prod-erp-app",
//!   action in [MaKo::Action::"AdminMalo", MaKo::Action::"AdminPartner"],
//!   resource
//! );
//! ```
//!
//! (The principal ID above would be whatever `sub` the IdP puts in the JWT —
//! typically a UUID for Azure, or a stable identifier for Kubernetes.)

use std::sync::{Arc, RwLock};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

// ── Allowed algorithms ────────────────────────────────────────────────────────

/// Asymmetric JWT algorithms accepted by [`OidcVerifier`].
///
/// HMAC algorithms (`HS*`) are not in this list and will be rejected with
/// [`OidcError::AlgorithmDenied`].
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
#[derive(Debug)]
pub struct JwtClaims {
    /// JWT `sub` claim — used as the Cedar principal entity ID.
    pub sub: String,
}

/// Errors produced by [`OidcVerifier`].
#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    /// OIDC discovery document could not be fetched or parsed.
    #[error("OIDC discovery failed (url={url}): {reason}")]
    Discovery { url: String, reason: String },

    /// JWKS endpoint could not be fetched or parsed.
    #[error("JWKS fetch failed (url={url}): {reason}")]
    JwksFetch { url: String, reason: String },

    /// JWT was structurally or cryptographically invalid.
    #[error("JWT invalid: {0}")]
    TokenInvalid(#[from] jsonwebtoken::errors::Error),

    /// The `issuer` claim in the OIDC discovery document does not match the
    /// configured issuer URL.
    ///
    /// Per OIDC Discovery 1.0 §4.3, the `issuer` value in the discovery
    /// document MUST be identical to the Issuer URL used to fetch it.  This
    /// mismatch is a security error — do not proceed.
    #[error(
        "OIDC issuer mismatch: configured {expected:?}, discovery document returned {actual:?}"
    )]
    IssuerMismatch { expected: String, actual: String },

    /// JWT header has no `kid` field.
    ///
    /// OIDC providers are required to include `kid` so the correct JWKS entry
    /// can be selected.  Tokens without `kid` are rejected to avoid
    /// brute-force key matching.
    #[error("JWT `kid` is missing; OIDC tokens must carry a `kid` header")]
    MissingKid,

    /// The JWT `kid` is not present in the cached JWKS.
    ///
    /// This is usually a transient condition during key rotation: the
    /// background refresh task will pick up the new key within
    /// `refresh_interval_secs` seconds.
    #[error("JWT `kid` {0:?} is not in the current JWKS (key rotation in progress?)")]
    UnknownKid(String),

    /// The JWT uses a symmetric or otherwise disallowed algorithm.
    #[error(
        "JWT algorithm {0:?} is not permitted; only asymmetric algorithms (RS*, ES*, PS*) are accepted"
    )]
    AlgorithmDenied(String),
}

// ── OidcVerifier ─────────────────────────────────────────────────────────────

/// OIDC JWT verifier with background JWKS refresh.
///
/// Cheap to clone — all state is `Arc`-wrapped.  Construct once at startup
/// and share by cloning.
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

/// Raw OIDC discovery document (only the fields we need).
#[derive(Deserialize)]
struct OidcDiscovery {
    /// MUST equal the issuer URL used to fetch this document (OIDC §4.3).
    issuer: String,
    jwks_uri: String,
}

impl OidcVerifier {
    /// Build an [`OidcVerifier`] for the given `issuer` and `audience`.
    ///
    /// Performs OIDC discovery and fetches the initial JWKS synchronously
    /// (blocking on network I/O once at startup).  The `http` client is reused
    /// by the background refresh task — pass the same `reqwest::Client` used
    /// elsewhere in `makod` to benefit from its connection pool.
    ///
    /// # Errors
    ///
    /// Returns [`OidcError::Discovery`] if the discovery document is
    /// unreachable or malformed, or [`OidcError::JwksFetch`] if the JWKS
    /// endpoint fails.
    pub async fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        http: &Client,
    ) -> Result<Self, OidcError> {
        let issuer = issuer.into();
        let audience = audience.into();

        // Fetch the OIDC discovery document to locate the JWKS URI.
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

        tracing::info!(
            issuer = %issuer,
            jwks_uri = %disc.jwks_uri,
            "OIDC: discovery succeeded"
        );

        // OIDC Discovery §4.3: the `issuer` field in the discovery document
        // MUST exactly match the URL used to fetch it.  A mismatch means a
        // rogue discovery server substituted its own issuer — reject.
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

    /// Build an [`OidcVerifier`] directly from a [`JwkSet`] without performing
    /// OIDC discovery.
    ///
    /// Intended **only for tests** where a real issuer URL is not available.
    /// The `jwks_uri` parameter is stored for logging; it does not need to be
    /// reachable.
    #[cfg(test)]
    pub(crate) fn from_jwks_for_testing(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks: JwkSet,
    ) -> Self {
        let issuer = issuer.into();
        Self {
            inner: Arc::new(Inner {
                issuer: issuer.clone(),
                audience: audience.into(),
                jwks_uri: format!("{issuer}/.well-known/jwks.json"),
                cache: RwLock::new(jwks),
            }),
        }
    }

    /// Verify a JWT and return its claims.
    ///
    /// Uses the in-memory JWKS cache — **synchronous and non-blocking**.
    /// Call [`spawn_refresh_task`][Self::spawn_refresh_task] to keep the cache
    /// current.
    ///
    /// # Errors
    ///
    /// - [`OidcError::MissingKid`] — no `kid` in JWT header
    /// - [`OidcError::AlgorithmDenied`] — symmetric or unknown algorithm
    /// - [`OidcError::UnknownKid`] — `kid` not in cached JWKS
    /// - [`OidcError::TokenInvalid`] — signature, expiry, iss, or aud mismatch
    pub fn verify(&self, token: &str) -> Result<JwtClaims, OidcError> {
        let header = jsonwebtoken::decode_header(token)?;

        // Reject symmetric / unrecognised algorithms unconditionally.
        if !ALLOWED_ALGORITHMS.contains(&header.alg) {
            return Err(OidcError::AlgorithmDenied(format!("{:?}", header.alg)));
        }

        let kid = header.kid.ok_or(OidcError::MissingKid)?;

        // Lock the JWKS cache, find the key, and drop the lock before the
        // (CPU-bound) signature verification step.
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
        // Enforce not-before (nbf) — off by default in jsonwebtoken.
        validation.validate_nbf = true;
        // Require `sub` explicitly (OIDC Core mandates it).
        validation.required_spec_claims.insert("sub".to_owned());

        #[derive(Deserialize)]
        struct Claims {
            sub: String,
        }

        let data = jsonwebtoken::decode::<Claims>(token, &decoding_key, &validation)?;
        Ok(JwtClaims {
            sub: data.claims.sub,
        })
    }

    /// Refresh the JWKS cache from the issuer's JWKS endpoint.
    ///
    /// Called automatically by the background task; call manually if you
    /// receive an [`OidcError::UnknownKid`] and want to handle key rotation
    /// immediately.
    pub async fn refresh(&self, http: &Client) -> Result<(), OidcError> {
        let jwks = Self::fetch_jwks_from(http, &self.inner.jwks_uri).await?;
        *self.inner.cache.write().unwrap_or_else(|p| p.into_inner()) = jwks;
        tracing::debug!(jwks_uri = %self.inner.jwks_uri, "OIDC: JWKS cache refreshed");
        Ok(())
    }

    /// Spawn a background Tokio task that refreshes the JWKS every
    /// `interval_secs` seconds.
    ///
    /// The task respects `shutdown` — it exits cleanly when the token is
    /// cancelled, which avoids log noise during graceful daemon shutdown.
    /// Pass `shutdown_token.clone()` from `makod`'s main `CancellationToken`.
    ///
    /// The returned [`tokio::task::JoinHandle`] can be awaited after cancellation
    /// to confirm the task has stopped.
    pub fn spawn_refresh_task(
        &self,
        http: Client,
        interval_secs: u64,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
            ticker.tick().await; // skip the first (immediate) tick
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        match this.refresh(&http).await {
                            Ok(()) => {}
                            Err(e) => tracing::warn!(
                                jwks_uri = %this.inner.jwks_uri,
                                "OIDC: JWKS refresh failed: {e}"
                            ),
                        }
                    }
                    _ = shutdown.cancelled() => {
                        tracing::debug!("OIDC: JWKS refresh task stopping (shutdown)");
                        return;
                    }
                }
            }
        })
    }

    /// Return `true` if `token` has the structure of a JWT
    /// (`<base64url>.<base64url>.<base64url>`).
    ///
    /// Used by [`CedarAuthorizer::authenticate`] to route tokens to the
    /// correct verification path without expensive full parsing.  Opaque API
    /// keys (random hex) do not contain `.` and are routed to the key table.
    pub fn looks_like_jwt(token: &str) -> bool {
        // A JWT has exactly 3 non-empty dot-separated parts.
        let mut parts = token.splitn(4, '.');
        parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_none() // no 4th part
    }

    async fn fetch_jwks_from(http: &Client, url: &str) -> Result<JwkSet, OidcError> {
        http.get(url)
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
            })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, jwk::JwkSet};

    // ── Test RSA-2048 key pair (test-only, never used in production) ──────────
    //
    // Private key in PKCS#8 PEM format; JWK n/e derived from it.
    // Generated with `openssl genrsa 2048`.

    const TEST_RSA_PRIVATE_KEY_PEM: &str = concat!(
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
    const TEST_KID: &str = "test-key-1";

    fn test_jwks() -> JwkSet {
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

    fn make_rs256_token(sub: &str, iss: &str, aud: &str, exp: u64) -> String {
        #[derive(serde::Serialize)]
        struct Claims<'a> {
            sub: &'a str,
            iss: &'a str,
            aud: &'a str,
            exp: u64,
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        jsonwebtoken::encode(
            &header,
            &Claims { sub, iss, aud, exp },
            &EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn make_rs256_token_with_nbf(sub: &str, iss: &str, aud: &str, exp: u64, nbf: u64) -> String {
        #[derive(serde::Serialize)]
        struct Claims<'a> {
            sub: &'a str,
            iss: &'a str,
            aud: &'a str,
            exp: u64,
            nbf: u64,
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        jsonwebtoken::encode(
            &header,
            &Claims {
                sub,
                iss,
                aud,
                exp,
                nbf,
            },
            &EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    // ── looks_like_jwt ────────────────────────────────────────────────────────

    #[test]
    fn looks_like_jwt_three_parts() {
        assert!(OidcVerifier::looks_like_jwt("aaa.bbb.ccc"));
    }

    #[test]
    fn looks_like_jwt_real_shaped() {
        assert!(OidcVerifier::looks_like_jwt(
            "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.signature"
        ));
    }

    #[test]
    fn not_jwt_no_dots() {
        assert!(!OidcVerifier::looks_like_jwt("randomhextoken"));
    }

    #[test]
    fn not_jwt_one_dot() {
        assert!(!OidcVerifier::looks_like_jwt("part1.part2"));
    }

    #[test]
    fn not_jwt_four_parts() {
        assert!(!OidcVerifier::looks_like_jwt("a.b.c.d"));
    }

    #[test]
    fn not_jwt_empty_part() {
        assert!(!OidcVerifier::looks_like_jwt("aaa..ccc"));
    }

    // ── verify — happy path ───────────────────────────────────────────────────

    #[test]
    fn rs256_valid_token_accepted() {
        let token = make_rs256_token(
            "user-123",
            "https://idp.example.com",
            "makod",
            9_999_999_999,
        );
        let verifier =
            OidcVerifier::from_jwks_for_testing("https://idp.example.com", "makod", test_jwks());
        let claims = verifier
            .verify(&token)
            .expect("valid RS256 token must verify");
        assert_eq!(claims.sub, "user-123");
    }

    // ── verify — algorithm rejection ──────────────────────────────────────────

    #[test]
    fn hs256_token_is_rejected() {
        let header = Header::new(Algorithm::HS256);
        #[derive(serde::Serialize)]
        struct Claims {
            sub: String,
            iss: String,
            aud: Vec<String>,
            exp: u64,
        }
        let token = jsonwebtoken::encode(
            &header,
            &Claims {
                sub: "user1".to_owned(),
                iss: "https://idp.example.com".to_owned(),
                aud: vec!["makod".to_owned()],
                exp: 9_999_999_999,
            },
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap();

        let verifier = OidcVerifier::from_jwks_for_testing(
            "https://idp.example.com",
            "makod",
            JwkSet { keys: vec![] },
        );
        assert!(matches!(
            verifier.verify(&token),
            Err(OidcError::AlgorithmDenied(_))
        ));
    }

    // ── verify — wrong audience ───────────────────────────────────────────────

    #[test]
    fn wrong_audience_rejected() {
        let token = make_rs256_token(
            "user-123",
            "https://idp.example.com",
            "other-service",
            9_999_999_999,
        );
        let verifier = OidcVerifier::from_jwks_for_testing(
            "https://idp.example.com",
            "makod", // expects "makod", token has "other-service"
            test_jwks(),
        );
        assert!(matches!(
            verifier.verify(&token),
            Err(OidcError::TokenInvalid(_))
        ));
    }

    // ── verify — expired token ────────────────────────────────────────────────

    #[test]
    fn expired_token_rejected() {
        let token = make_rs256_token("user-123", "https://idp.example.com", "makod", 1); // exp=1 is in the past
        let verifier =
            OidcVerifier::from_jwks_for_testing("https://idp.example.com", "makod", test_jwks());
        assert!(matches!(
            verifier.verify(&token),
            Err(OidcError::TokenInvalid(_))
        ));
    }

    // ── verify — future nbf rejected ─────────────────────────────────────────

    #[test]
    fn future_nbf_rejected() {
        // nbf = year 2099 — token is not-yet-valid
        let token = make_rs256_token_with_nbf(
            "user-123",
            "https://idp.example.com",
            "makod",
            9_999_999_999, // exp
            4_000_000_000, // nbf: ~year 2096
        );
        let verifier =
            OidcVerifier::from_jwks_for_testing("https://idp.example.com", "makod", test_jwks());
        assert!(matches!(
            verifier.verify(&token),
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
        // Provide an empty JWKS — kid "test-key-1" will not be found.
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
}
