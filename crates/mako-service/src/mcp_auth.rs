//! Unified MCP server authentication for mako daemons.
//!
//! Provides [`McpAuth`][crate::mcp_auth::McpAuth] — a single configurable auth strategy that covers every
//! deployment scenario:
//!
//! | Mode | Configuration | Used by |
//! |---|---|---|
//! | **OIDC + Cedar** | [`from_auth_config_oidc`][crate::mcp_auth::McpAuth::from_auth_config_oidc] with `Some(cedar)` | every MCP service but `portald` — 13 of them |
//! | **API key only** | [`from_auth_config`][crate::mcp_auth::McpAuth::from_auth_config] | `portald`, which verifies no tokens |
//! | **OIDC only** | `from_auth_config_oidc` with `None` | nothing today; the type still supports it |
//! | **Dev mode** | `McpAuth::dev()`, no keys | local development only |
//!
//! Named API keys are accepted alongside OIDC in any of these — a key is a
//! Cedar principal like a token, so the first row covers key callers too.
//! `makod` is not in the table: it authenticates through its own
//! `cedar_schema::BearerAuthenticator` rather than through `McpAuth`.
//!
//! ## Security properties
//!
//! - API keys are stored as [`secrecy::SecretString`] — never appear in `Debug` output or logs.
//! - All API-key comparisons use constant-time equality (`subtle::ConstantTimeEq`) to
//!   prevent timing attacks.
//! - JWT tokens are routed by shape (`looks_like_jwt`) before OIDC verification, so
//!   API-key requests are never fed to the JWT parser and OIDC verification is never
//!   run unnecessarily.
//! - Multiple named keys are supported — each key has an `identity` string written to
//!   the [`McpIdentity`][crate::mcp_auth::McpIdentity] extension injected into the request for downstream audit logging.
//! - A named key is a Cedar principal like any token caller: `User::"<key name>"`
//!   carrying the roles its configuration declares. A key holder therefore
//!   reaches only what the policy grants those roles, and a role-less key is
//!   refused by every role-testing `permit`.
//!
//! ## Identity propagation
//!
//! After a successful authentication, [`McpAuth::authenticate`][crate::mcp_auth::McpAuth::authenticate] injects an
//! [`McpIdentity`][crate::mcp_auth::McpIdentity] as an Axum request extension:
//!
//! ```rust,no_run
//! use mako_service::mcp_auth::McpIdentity;
//! // From within a regular Axum handler:
//! async fn my_handler(
//!     axum::Extension(identity): axum::Extension<McpIdentity>,
//! ) {
//!     tracing::info!(caller = %identity.name, method = ?identity.method, "MCP call");
//! }
//! ```
//!
//! Note: rmcp `#[tool]` handlers do not have direct access to request extensions;
//! use `McpIdentity` from regular Axum middleware or handler layers instead.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use mako_service::mcp_auth::McpAuth;
//!
//! #[derive(Clone)]
//! pub struct MyMcpState {
//!     pub pool: Vec<u8>, // replace with sqlx::PgPool in real code
//!     pub tenant: String,
//!     pub auth: McpAuth,
//! }
//!
//! async fn mcp_auth_middleware(
//!     axum::extract::State(state): axum::extract::State<Arc<MyMcpState>>,
//!     req: axum::extract::Request,
//!     next: axum::middleware::Next,
//! ) -> axum::response::Response {
//!     state.auth.authenticate(req, next).await
//! }
//! ```
//!
//! ```rust
//! # use std::sync::Arc;
//! # use mako_service::mcp_auth::McpAuth;
//! # let tenant = "9910000000002";
//! // API key only (services without IdP):
//! let auth = McpAuth::dev(tenant)
//!     .with_named_key_roles("agentd", "agent-secret-change-me", ["LF"]);
//!
//! // Dev mode (allow all):
//! let auth2 = McpAuth::dev(tenant);
//! ```

use std::sync::Arc;

use axum::{http::StatusCode, middleware::Next, response::IntoResponse};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use subtle::ConstantTimeEq;

// ── McpAuthConfig ─────────────────────────────────────────────────────────────

/// Standard MCP server auth configuration, shared across **all** mako services.
///
/// Add as `pub mcp: McpAuthConfig` (or re-export as `McpConfig`) to your
/// service config struct and use `#[serde(default)]` so the section is optional.
///
/// ## TOML example
///
/// ```toml
/// # Minimal — dev mode, no auth required:
/// # (omit the [mcp] section entirely)
///
/// # API-key only (for agentd / LLM clients):
/// [mcp]
/// api_key       = "env:SERVICE_MCP_API_KEY"   # use env: prefix or literal
/// api_key_roles = ["LF"]                      # roles the key is evaluated under
///
/// # Multiple named keys (for per-agent audit trails):
/// [mcp]
/// api_key = "env:AGENTD_KEY"   # identity = "agentd"
///
/// [[mcp.named_keys]]
/// name    = "billing-bot"
/// api_key = "env:BILLING_BOT_KEY"
/// roles   = ["LF"]              # what Cedar evaluates this key as
/// ```
///
/// When an OIDC verifier is passed to [`McpAuth::from_auth_config_oidc`], JWT
/// tokens are verified against it; API keys remain accepted as a fallback for
/// LLM clients that cannot perform a full OIDC flow.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpAuthConfig {
    /// Primary API key — accepted as Bearer token for `/mcp` requests.
    ///
    /// Stored as `SecretString` internally; use `"env:VAR_NAME"` in TOML to
    /// defer the value to an environment variable.
    ///
    /// When absent and no OIDC verifier is supplied, the server runs in
    /// **dev mode** (all requests accepted — never use in production).
    pub api_key: Option<String>,

    /// Additional named keys for per-agent auditing.
    ///
    /// Each key has its own `name` that appears in [`McpIdentity`] so you can
    /// distinguish `"agentd"` from `"billing-bot"` in logs.
    #[serde(default)]
    pub named_keys: Vec<McpAuthNamedKey>,

    /// Market roles carried by [`Self::api_key`] (identity `"agentd"`).
    ///
    /// Same vocabulary and Cedar meaning as [`McpAuthNamedKey::roles`].
    #[serde(default)]
    pub api_key_roles: Vec<String>,
}

/// A single named API key entry inside [`McpAuthConfig`].
#[derive(Debug, Clone, Deserialize)]
pub struct McpAuthNamedKey {
    /// Identity name used in audit logs (e.g. `"agentd"`, `"billing-bot"`).
    pub name: String,
    /// The secret key value (supports `"env:VAR_NAME"` syntax).
    pub api_key: String,
    /// Market roles this key carries, in the same vocabulary as the
    /// `mako_roles` JWT claim (`"NB"`, `"LF"`, `"MSB"`, `"ESA"`, `"ADMIN"`).
    ///
    /// Cedar sees them as `context.principal_roles`, so a key is authorised by
    /// the same policy text as a token. An empty list is a caller with no
    /// roles: every role-testing `permit` denies it, which leaves only the
    /// rules written for the key itself.
    #[serde(default)]
    pub roles: Vec<String>,
}

use crate::{cedar::CedarEnforcer, oidc::OidcVerifier};

// ── McpApiKey ─────────────────────────────────────────────────────────────────

/// A named API key for MCP endpoint authentication.
///
/// The `name` is used as the caller identity in [`McpIdentity`], in audit logs,
/// and as the Cedar principal (`User::"<name>"`). The `secret` is stored as a
/// [`SecretString`] and never appears in `Debug` output.
///
/// Use [`McpAuth::with_named_key`] to register keys.
#[derive(Clone)]
pub struct McpApiKey {
    /// Human-readable name for audit logs (e.g. `"agentd"`, `"billing-agent"`).
    pub name: String,
    secret: SecretString,
    /// Roles this key carries, seen by Cedar as `context.principal_roles`.
    pub roles: Vec<String>,
}

impl std::fmt::Debug for McpApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpApiKey")
            .field("name", &self.name)
            .field("secret", &"[REDACTED]")
            .field("roles", &self.roles)
            .finish()
    }
}

impl McpApiKey {
    /// A key carrying no roles. Add them with [`Self::with_roles`].
    pub fn new(name: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            secret: SecretString::new(secret.into().into()),
            roles: Vec::new(),
        }
    }

    /// Declare the roles this key carries.
    #[must_use]
    pub fn with_roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    fn matches(&self, provided: &str) -> bool {
        provided
            .as_bytes()
            .ct_eq(self.secret.expose_secret().as_bytes())
            .into()
    }
}

// ── McpAuthMethod / McpIdentity ───────────────────────────────────────────────

/// How the caller authenticated — included in [`McpIdentity`] for audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthMethod {
    /// Bearer token verified via OIDC JWKS.
    Oidc,
    /// Static named API key (constant-time matched).
    ApiKey,
    /// Dev mode — no authentication configured.
    DevMode,
}

/// Authenticated caller identity, injected as an Axum request extension.
///
/// Downstream handlers and middleware can extract this to log or enforce
/// per-caller rate limits or audit trails.
///
/// ```rust,no_run
/// async fn handler(axum::Extension(id): axum::Extension<mako_service::mcp_auth::McpIdentity>) {
///     tracing::info!(caller = %id.name, method = ?id.method, "MCP call");
/// }
/// ```
#[derive(Debug, Clone)]
pub struct McpIdentity {
    /// Caller name: OIDC `sub` claim, API key name, or `"dev-mode"`.
    pub name: String,
    /// Authentication method used for this request.
    pub method: McpAuthMethod,
}

// ── McpAuth ───────────────────────────────────────────────────────────────────

/// Configurable MCP authentication strategy.
///
/// Cheap to clone — all inner state is `Arc`-wrapped.
/// Embed in your MCP state struct and call [`McpAuth::authenticate`] from
/// your `mcp_auth_middleware`.
#[derive(Clone)]
pub struct McpAuth {
    oidc: OidcVerifier,
    /// Optional Cedar enforcer for policy-checked access control.
    /// `None` = OIDC token is sufficient (no Cedar policy check).
    cedar: Option<Arc<CedarEnforcer>>,
    /// Tenant used as the Cedar resource when `cedar` is `Some`.
    tenant: String,
    /// Named API keys.  Empty = no API-key auth (OIDC or dev mode only).
    api_keys: Vec<McpApiKey>,
}

impl McpAuth {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// OIDC-only auth.  Call [`McpAuth::with_cedar`] for Cedar policy checks or
    /// [`McpAuth::with_named_key`] to add an API-key fallback for agent clients.
    pub fn new(oidc: OidcVerifier, tenant: impl Into<String>) -> Self {
        Self {
            oidc,
            cedar: None,
            tenant: tenant.into(),
            api_keys: Vec::new(),
        }
    }

    /// Dev / API-key mode: `OidcVerifier::disabled()` with optional named keys.
    ///
    /// - No keys added → accept all requests (dev mode, no Bearer required).
    /// - Keys added → require a matching Bearer token (API-key only mode).
    pub fn dev(tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        Self {
            oidc: OidcVerifier::disabled(&tenant),
            cedar: None,
            tenant,
            api_keys: Vec::new(),
        }
    }

    // ── Builder methods ───────────────────────────────────────────────────────

    /// Add a Cedar policy enforcer.
    ///
    /// Requests that pass OIDC verification are also checked against the Cedar
    /// policy under action `"use-mcp"`.
    #[must_use]
    pub fn with_cedar(mut self, cedar: Arc<CedarEnforcer>) -> Self {
        self.cedar = Some(cedar);
        self
    }

    /// Add a named API key carrying no roles.
    ///
    /// When set, a Bearer token matching this key is accepted **in addition to**
    /// (or instead of, when OIDC is disabled) a valid OIDC token.  The `name` is
    /// recorded in [`McpIdentity`] for audit logging and is the Cedar principal.
    ///
    /// Multiple keys may be registered by chaining calls.  Keys are checked in
    /// registration order; first match wins.
    ///
    /// A role-less key is denied by every role-testing `permit`, so under Cedar
    /// it reaches only what a policy grants it by name. Use
    /// [`Self::with_named_key_roles`] to give it the roles its work needs.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use mako_service::mcp_auth::McpAuth;
    /// let auth = McpAuth::dev("9910000000002")
    ///     .with_named_key("agentd",       "agentd-prod-secret-change-me")
    ///     .with_named_key("billing-bot",  "billing-bot-secret-change-me");
    /// ```
    #[must_use]
    pub fn with_named_key(mut self, name: impl Into<String>, key: impl Into<String>) -> Self {
        self.api_keys.push(McpApiKey::new(name, key));
        self
    }

    /// Add a named API key carrying `roles`.
    ///
    /// The roles use the `mako_roles` vocabulary and reach Cedar as
    /// `context.principal_roles`, so one policy covers key and token callers
    /// alike.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use mako_service::mcp_auth::McpAuth;
    /// let auth = McpAuth::dev("9910000000002")
    ///     .with_named_key_roles("billing-bot", "billing-bot-secret", ["LF"]);
    /// ```
    #[must_use]
    pub fn with_named_key_roles(
        mut self,
        name: impl Into<String>,
        key: impl Into<String>,
        roles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.api_keys
            .push(McpApiKey::new(name, key).with_roles(roles));
        self
    }

    /// Convenience wrapper: add a single key with the identity name `"api-key"`.
    ///
    /// For deployments where auditing the specific caller is not required.
    /// Prefer [`McpAuth::with_named_key`] when you know the caller's name.
    #[must_use]
    pub fn with_api_key(self, key: impl Into<String>) -> Self {
        self.with_named_key("api-key", key)
    }

    // ── Standard factory constructors from McpAuthConfig ─────────────────────

    /// Build `McpAuth` from a [`McpAuthConfig`] for services **without OIDC**.
    ///
    /// - `cfg.api_key` present → API-key only (identity = `"agentd"`)
    /// - `cfg.named_keys` → additional named keys
    /// - both absent → dev mode (allow all; **never use in production**)
    ///
    /// This is the standard construction for `productd`, `einsd`, `vertragd`,
    /// `sperrd`, `billingd`, `accountingd`, `portald`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use mako_service::mcp_auth::{McpAuth, McpAuthConfig};
    /// # let cfg = McpAuthConfig::default();
    /// let auth = McpAuth::from_auth_config(&cfg, "9910000000002");
    /// ```
    #[must_use]
    pub fn from_auth_config(cfg: &McpAuthConfig, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        let mut auth = Self::dev(&tenant);
        if let Some(k) = &cfg.api_key
            && !k.is_empty()
        {
            auth = auth.with_named_key_roles("agentd", k, cfg.api_key_roles.clone());
        }
        for nk in &cfg.named_keys {
            if !nk.api_key.is_empty() {
                auth = auth.with_named_key_roles(&nk.name, &nk.api_key, nk.roles.clone());
            }
        }
        auth
    }

    /// Build `McpAuth` from a [`McpAuthConfig`] for services **with OIDC**.
    ///
    /// - JWT Bearer tokens → verified against `oidc` → optional Cedar check
    /// - `cfg.api_key` → named key `"agentd"` accepted alongside OIDC
    /// - `cfg.named_keys` → additional named keys
    ///
    /// This is the standard construction for `marktd`, `invoicd`, `processd`,
    /// `edmd`, `obsd`, `accountingd`, `billingd`, `sperrd` when OIDC is active.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use mako_service::mcp_auth::{McpAuth, McpAuthConfig};
    /// # use mako_service::oidc::OidcVerifier;
    /// # let cfg = McpAuthConfig::default();
    /// # let oidc = OidcVerifier::disabled("tenant");
    /// let auth = McpAuth::from_auth_config_oidc(&cfg, oidc, None, "tenant");
    /// ```
    #[must_use]
    pub fn from_auth_config_oidc(
        cfg: &McpAuthConfig,
        oidc: OidcVerifier,
        cedar: Option<Arc<CedarEnforcer>>,
        tenant: impl Into<String>,
    ) -> Self {
        let tenant = tenant.into();
        let mut auth = Self::new(oidc, &tenant);
        if let Some(c) = cedar {
            auth = auth.with_cedar(c);
        }
        if let Some(k) = &cfg.api_key
            && !k.is_empty()
        {
            auth = auth.with_named_key_roles("agentd", k, cfg.api_key_roles.clone());
        }
        for nk in &cfg.named_keys {
            if !nk.api_key.is_empty() {
                auth = auth.with_named_key_roles(&nk.name, &nk.api_key, nk.roles.clone());
            }
        }
        auth.warn_role_less_keys();
        auth
    }

    // ── Auth check ────────────────────────────────────────────────────────────

    /// Run the configured auth check for an incoming MCP request.
    ///
    /// On success, injects [`McpIdentity`] as an Axum request extension before
    /// forwarding to `next`.
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use mako_service::mcp_auth::McpAuth;
    /// # #[derive(Clone)] struct S { auth: McpAuth }
    /// async fn mcp_auth_middleware(
    ///     axum::extract::State(s): axum::extract::State<Arc<S>>,
    ///     req: axum::extract::Request,
    ///     next: axum::middleware::Next,
    /// ) -> axum::response::Response {
    ///     s.auth.authenticate(req, next).await
    /// }
    /// ```
    pub async fn authenticate(
        &self,
        mut request: axum::extract::Request,
        next: Next,
    ) -> axum::response::Response {
        // ── Dev mode: OIDC disabled and no API keys → allow all ────────────
        if self.oidc.is_disabled() && self.api_keys.is_empty() {
            request.extensions_mut().insert(McpIdentity {
                name: "dev-mode".to_owned(),
                method: McpAuthMethod::DevMode,
            });
            return next.run(request).await;
        }

        // ── Extract Bearer token (required for all other modes) ─────────────
        let token = match request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(ToOwned::to_owned)
        {
            Some(t) => t,
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    "Authorization: Bearer <token> required for /mcp",
                )
                    .into_response();
            }
        };

        // ── OIDC disabled: API keys only ────────────────────────────────────
        if self.oidc.is_disabled() {
            return match self.try_api_key(&token) {
                Some(key) => match self.check_key(key, "use-mcp") {
                    Ok(()) => {
                        request.extensions_mut().insert(key.identity());
                        next.run(request).await
                    }
                    Err(resp) => resp,
                },
                None => (StatusCode::UNAUTHORIZED, "invalid API key").into_response(),
            };
        }

        // ── OIDC active: route by token shape ───────────────────────────────
        //
        // Use JWT-shape detection to immediately route:
        //   - 3-part dot-separated token → OIDC JWT verification
        //   - any other token → API-key lookup (if keys configured)
        //
        // This avoids feeding opaque API keys into the JWT parser and avoids
        // triggering an expensive failed OIDC parse for every agent API-key call.

        if OidcVerifier::looks_like_jwt(&token) {
            // ── OIDC path ──────────────────────────────────────────────────
            match self.oidc.verify(&token) {
                Ok(claims) => {
                    // MCP callers must carry the `mako_tenant` claim — the
                    // data-isolation boundary all downstream checks key on.
                    // (Tenant-less tokens are only meaningful for makod's
                    // sub-based Cedar layer, which does not use McpAuth.)
                    if claims.mako_tenant.is_none() {
                        return (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token")
                            .into_response();
                    }
                    // Optional Cedar policy check.
                    if let Some(ref cedar) = self.cedar {
                        let principal = crate::oidc::Claims(claims.clone()).principal();
                        if let Err(e) = cedar.check(&principal, "use-mcp", &self.tenant) {
                            return (StatusCode::FORBIDDEN, format!("403 Forbidden: {e}"))
                                .into_response();
                        }
                    }
                    request.extensions_mut().insert(McpIdentity {
                        name: claims.sub.clone(),
                        method: McpAuthMethod::Oidc,
                    });
                    next.run(request).await
                }
                Err(_) => {
                    (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response()
                }
            }
        } else {
            // ── API-key path ───────────────────────────────────────────────
            match self.try_api_key(&token) {
                Some(key) => match self.check_key(key, "use-mcp") {
                    Ok(()) => {
                        request.extensions_mut().insert(key.identity());
                        next.run(request).await
                    }
                    Err(resp) => resp,
                },
                None => {
                    (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response()
                }
            }
        }
    }

    /// Authorize a specific Cedar `action` for the caller identified by the
    /// request's bearer token, **without** running the request.
    ///
    /// The [`authenticate`](Self::authenticate) middleware applies one blanket
    /// gate (`use-mcp`) to the whole MCP surface, which cannot distinguish a
    /// read tool from a destructive one. A server that exposes destructive tools
    /// (Ersatzwert generation, campaign dispatch) calls this from its own
    /// middleware for exactly those tools, so the MCP path enforces the same role
    /// its REST twin does rather than accepting any `use-mcp` caller.
    ///
    /// Both caller kinds are evaluated: a JWT by its `mako_roles` claim, a named
    /// API key by the roles its configuration declares. One policy therefore
    /// governs both, and a key reaches a destructive tool only if the policy
    /// says so.
    ///
    /// Returns `Ok(())` when allowed — including dev mode and when no Cedar
    /// enforcer is configured. Otherwise returns the `401`/`403` response to
    /// short-circuit with.
    // The `Err` is a fully-formed HTTP response the caller returns as-is; boxing
    // it would add an allocation on every auth failure for no benefit.
    #[allow(clippy::result_large_err)]
    pub fn authorize(
        &self,
        headers: &axum::http::HeaderMap,
        action: &str,
    ) -> Result<(), axum::response::Response> {
        // Same posture as `authenticate`: nothing to enforce without Cedar, and
        // dev mode (no OIDC and no keys) carries no caller to evaluate.
        if self.cedar.is_none() || self.is_dev_mode() {
            return Ok(());
        }
        let token = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    "Authorization: Bearer <token> required",
                )
                    .into_response()
            })?;
        // A non-JWT token is a named key: evaluate it under its declared roles
        // rather than waving it through.
        if !OidcVerifier::looks_like_jwt(&token) {
            let key = self.try_api_key(&token).ok_or_else(|| {
                (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response()
            })?;
            return self.check_key(key, action);
        }
        let claims = self.oidc.verify(&token).map_err(|_| {
            (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response()
        })?;
        let principal = crate::oidc::Claims(claims).principal();
        self.check_principal(&principal, action)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Check `token` against all registered API keys.
    ///
    /// Returns the first matching key, or `None`. Comparisons are constant-time
    /// to prevent timing attacks.
    fn try_api_key(&self, token: &str) -> Option<&McpApiKey> {
        self.api_keys.iter().find(|key| key.matches(token))
    }

    /// Say so at startup when a key carries no roles and a policy is loaded.
    ///
    /// Such a key is denied by every role-testing rule — including the blanket
    /// `use-mcp` gate where a service writes one that way. The failure is a 403
    /// on the caller's first request and says nothing about the configuration
    /// that caused it, so name it here instead.
    fn warn_role_less_keys(&self) {
        if self.cedar.is_none() {
            return;
        }
        for key in self.api_keys.iter().filter(|k| k.roles.is_empty()) {
            tracing::warn!(
                key = %key.name,
                "MCP key declares no roles: Cedar sees it with empty \
                 `principal_roles`, so every role-testing rule denies it — set \
                 `roles` on the key if it is meant to reach more than the \
                 rules written for its name"
            );
        }
    }

    /// No OIDC and no keys: the endpoint is open by construction.
    fn is_dev_mode(&self) -> bool {
        self.oidc.is_disabled() && self.api_keys.is_empty()
    }

    /// Evaluate `action` for a named key under the roles it declares.
    ///
    /// A key is issued by the operator running this deployment, so its principal
    /// tenant is this service's tenant — the same-tenant clause every policy
    /// carries holds, and what remains for the policy to decide is the roles.
    #[allow(clippy::result_large_err)]
    fn check_key(&self, key: &McpApiKey, action: &str) -> Result<(), axum::response::Response> {
        self.check_principal(
            &crate::cedar::CedarPrincipal {
                sub: key.name.clone(),
                tenant: self.tenant.clone(),
                roles: key.roles.clone(),
            },
            action,
        )
    }

    /// Run the Cedar check if one is configured.
    #[allow(clippy::result_large_err)]
    fn check_principal(
        &self,
        principal: &crate::cedar::CedarPrincipal,
        action: &str,
    ) -> Result<(), axum::response::Response> {
        let Some(cedar) = self.cedar.as_ref() else {
            return Ok(());
        };
        cedar
            .check(principal, action, &self.tenant)
            .map_err(|e| (StatusCode::FORBIDDEN, format!("403 Forbidden: {e}")).into_response())
    }
}

impl McpApiKey {
    /// The audit identity this key authenticates as.
    fn identity(&self) -> McpIdentity {
        McpIdentity {
            name: self.name.clone(),
            method: McpAuthMethod::ApiKey,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── McpApiKey ─────────────────────────────────────────────────────────────

    #[test]
    fn api_key_matches_correct_secret() {
        let key = McpApiKey::new("agentd", "secret-abc-123");
        assert!(key.matches("secret-abc-123"));
    }

    #[test]
    fn api_key_rejects_wrong_secret() {
        let key = McpApiKey::new("agentd", "secret-abc-123");
        assert!(!key.matches("wrong-secret"));
        assert!(!key.matches(""));
        assert!(!key.matches("secret-abc-123 ")); // trailing space
        assert!(!key.matches(" secret-abc-123")); // leading space
    }

    #[test]
    fn api_key_name_accessible_secret_not_debug() {
        let key = McpApiKey::new("billing-bot", "my-secret");
        assert_eq!(key.name, "billing-bot");
        // SecretString does not expose the secret in Debug output
        let debug = format!("{key:?}");
        assert!(
            !debug.contains("my-secret"),
            "secret must not appear in Debug: {debug}"
        );
    }

    // ── McpAuth::dev (no keys) → dev mode ─────────────────────────────────────

    #[test]
    fn dev_mode_has_no_keys() {
        let auth = McpAuth::dev("tenant-gln");
        assert!(auth.api_keys.is_empty());
        assert!(auth.oidc.is_disabled());
    }

    // ── McpAuth::with_api_key / with_named_key ────────────────────────────────

    #[test]
    fn with_api_key_uses_default_name() {
        let auth = McpAuth::dev("t").with_api_key("k");
        assert_eq!(auth.api_keys.len(), 1);
        assert_eq!(auth.api_keys[0].name, "api-key");
        assert!(auth.api_keys[0].matches("k"));
    }

    #[test]
    fn with_named_key_uses_given_name() {
        let auth = McpAuth::dev("t").with_named_key("agentd", "agent-secret");
        assert_eq!(auth.api_keys[0].name, "agentd");
        assert!(auth.api_keys[0].matches("agent-secret"));
    }

    #[test]
    fn multiple_named_keys_registered() {
        let auth = McpAuth::dev("t")
            .with_named_key("agentd", "key-a")
            .with_named_key("billing-bot", "key-b")
            .with_named_key("admin-ui", "key-c");
        assert_eq!(auth.api_keys.len(), 3);
        assert_eq!(auth.api_keys[0].name, "agentd");
        assert_eq!(auth.api_keys[1].name, "billing-bot");
        assert_eq!(auth.api_keys[2].name, "admin-ui");
    }

    // ── try_api_key ────────────────────────────────────────────────────────────

    #[test]
    fn try_api_key_matches_first_key() {
        let auth = McpAuth::dev("t")
            .with_named_key("a", "key-a")
            .with_named_key("b", "key-b");
        let key = auth.try_api_key("key-a").unwrap();
        assert_eq!(key.name, "a");
        assert_eq!(key.identity().method, McpAuthMethod::ApiKey);
    }

    #[test]
    fn try_api_key_matches_second_key() {
        let auth = McpAuth::dev("t")
            .with_named_key("a", "key-a")
            .with_named_key("b", "key-b");
        let id = auth.try_api_key("key-b").unwrap();
        assert_eq!(id.name, "b");
    }

    #[test]
    fn try_api_key_no_match_returns_none() {
        let auth = McpAuth::dev("t").with_named_key("a", "key-a");
        assert!(auth.try_api_key("wrong").is_none());
        assert!(auth.try_api_key("").is_none());
    }

    // ── looks_like_jwt routing ────────────────────────────────────────────────

    #[test]
    fn jwt_shaped_tokens_detected() {
        // Real-shaped JWTs have base64url header.payload.signature
        assert!(OidcVerifier::looks_like_jwt("eyJ.eyJ.sig"));
        assert!(OidcVerifier::looks_like_jwt("header.payload.signature"));
        // Any 3-non-empty-dot-separated parts qualifies
        assert!(OidcVerifier::looks_like_jwt("a.b.c"));
    }

    #[test]
    fn non_jwt_tokens_rejected() {
        assert!(!OidcVerifier::looks_like_jwt("abc123-no-dots")); // API key
        assert!(!OidcVerifier::looks_like_jwt("a.b")); // 2 parts
        assert!(!OidcVerifier::looks_like_jwt("a.b.c.d")); // 4 parts
        assert!(!OidcVerifier::looks_like_jwt(".b.c")); // empty first
        assert!(!OidcVerifier::looks_like_jwt("a..c")); // empty middle
        assert!(!OidcVerifier::looks_like_jwt("")); // empty
        assert!(!OidcVerifier::looks_like_jwt("demo-secret-change-me")); // typical demo key
    }

    // ── McpIdentity ───────────────────────────────────────────────────────────

    // ── Cedar covers named keys ───────────────────────────────────────────────

    /// A policy that grants the destructive action to `LF` only.
    const LF_ONLY: &str = r#"
        permit(principal, action, resource)
        when { context.principal_roles.contains("LF") };
    "#;

    fn lf_only_auth() -> McpAuth {
        McpAuth::new(OidcVerifier::disabled("t"), "t")
            .with_cedar(Arc::new(CedarEnforcer::from_policy_str(LF_ONLY).unwrap()))
            .with_named_key_roles("billing-bot", "key-lf", ["LF"])
            .with_named_key("reader-bot", "key-none")
    }

    fn bearer(token: &str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            "Authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        h
    }

    /// The roles a key declares are what the policy sees.
    #[test]
    fn key_roles_reach_cedar() {
        let auth = lf_only_auth();
        let key = auth.try_api_key("key-lf").expect("registered");
        assert!(auth.check_key(key, "generate-ersatzwert").is_ok());
    }

    /// A key with no roles is refused by a role-testing permit — it does not
    /// fall through to an allow the way the old trust-boundary path did.
    #[test]
    fn role_less_key_is_denied_not_waved_through() {
        let auth = lf_only_auth();
        let key = auth.try_api_key("key-none").expect("registered");
        let err = auth
            .check_key(key, "generate-ersatzwert")
            .expect_err("no roles, and the policy grants only LF");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    /// `authorize` is the destructive-tool gate: it must evaluate a key caller,
    /// which is the whole point of giving keys roles.
    #[test]
    fn authorize_evaluates_a_key_caller() {
        let auth = lf_only_auth();
        assert!(
            auth.authorize(&bearer("key-lf"), "generate-ersatzwert")
                .is_ok()
        );
        assert_eq!(
            auth.authorize(&bearer("key-none"), "generate-ersatzwert")
                .expect_err("role-less")
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    /// An unregistered opaque token is a 401 at the gate, not a silent pass.
    #[test]
    fn authorize_refuses_an_unknown_key() {
        let auth = lf_only_auth();
        assert_eq!(
            auth.authorize(&bearer("not-a-key"), "generate-ersatzwert")
                .expect_err("unknown")
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    /// Without Cedar there is nothing to evaluate, and dev mode carries no
    /// caller at all — both stay open, as `authenticate` does.
    #[test]
    fn authorize_is_open_without_cedar_and_in_dev_mode() {
        let no_cedar = McpAuth::dev("t").with_named_key("agentd", "key-a");
        assert!(no_cedar.authorize(&bearer("key-a"), "anything").is_ok());
        assert!(
            no_cedar
                .authorize(&axum::http::HeaderMap::new(), "anything")
                .is_ok()
        );

        let dev = McpAuth::dev("t")
            .with_cedar(Arc::new(CedarEnforcer::from_policy_str(LF_ONLY).unwrap()));
        assert!(dev.is_dev_mode());
        assert!(
            dev.authorize(&axum::http::HeaderMap::new(), "anything")
                .is_ok()
        );
    }

    /// Config roles survive the trip into the registered key.
    #[test]
    fn config_roles_reach_the_registered_key() {
        let cfg = McpAuthConfig {
            api_key: Some("primary".to_owned()),
            api_key_roles: vec!["ADMIN".to_owned()],
            named_keys: vec![McpAuthNamedKey {
                name: "billing-bot".to_owned(),
                api_key: "secondary".to_owned(),
                roles: vec!["LF".to_owned()],
            }],
        };
        let auth = McpAuth::from_auth_config(&cfg, "t");
        assert_eq!(auth.try_api_key("primary").unwrap().roles, ["ADMIN"]);
        assert_eq!(auth.try_api_key("secondary").unwrap().roles, ["LF"]);
    }

    #[test]
    fn mcp_identity_debug_does_not_expose_secrets() {
        let id = McpIdentity {
            name: "agentd".to_owned(),
            method: McpAuthMethod::ApiKey,
        };
        let debug = format!("{id:?}");
        assert!(debug.contains("agentd"));
        assert!(debug.contains("ApiKey"));
    }
}
