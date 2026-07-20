//! Cedar-based authentication and authorization for all `makod` HTTP endpoints.
//!
//! ## Architecture
//!
//! ```text
//! HTTP request
//!   → authenticate(headers) → CallerIdentity   (bearer token → named principal)
//!   → authorize*(identity, resource)           (Cedar policy evaluation)
//!   → 200 OK  |  401 Unauthorized  |  403 Forbidden
//! ```
//!
//! The [`CedarAuthorizer`] is constructed once at startup and shared via
//! `Arc` across all API states.  It holds:
//!
//! - A **named-key registry** — maps `Authorization: Bearer <token>` to a
//!   Cedar principal (`MaKo::Principal::"<name>"`).
//! - A compiled **[`PolicySet`]** — the embedded default policy plus any
//!   operator-supplied extras from `--cedar-policy-dir`.
//! - The compiled **schema** for validation.
//!
//! ## Identity model
//!
//! Each API key is named (e.g. `"erp-sap-prod"`, `"ci-pipeline"`).  The name
//! is the Cedar entity ID and appears verbatim in every audit log entry,
//! making it trivially identifiable which system issued each request.
//!
//! Keys are configured with `--auth-key NAME=TOKEN` (repeatable).
//!
//! ## Default policy
//!
//! The embedded default policy (`cedar/default.cedar`) permits any
//! authenticated principal to perform any action — identical to the previous
//! single-token model.  Operators layer more specific policies on top using
//! `permit when {…}` conditions or `forbid` rules.
//!
//! ## Operator ABAC policies
//!
//! Drop `.cedar` files into the directory named by `--cedar-policy-dir` to
//! add or restrict permissions per principal, tenant, Marktrolle, or PID.
//!
//! ```cedar
//! // Restrict "ci-readonly" to read-only MaLo access only.
//! forbid(
//!   principal == MaKo::Principal::"ci-readonly",
//!   action in [
//!     MaKo::Action::"AdminMaloWrite",
//!     MaKo::Action::"AdminMaloDelete",
//!     MaKo::Action::"SubmitCommand",
//!     MaKo::Action::"IngestEdifact"
//!   ],
//!   resource
//! );
//!
//! // Restrict "erp-gas" to gas supplier commands only.
//! forbid(
//!   principal == MaKo::Principal::"erp-gas",
//!   action    == MaKo::Action::"SubmitCommand",
//!   resource  is MaKo::Command
//! )
//! unless {
//!   resource.marktrolle == "LFG" || resource.marktrolle == "GNB"
//! };
//!
//! // Scope "erp-tenant-a" to its own tenant only.
//! forbid(
//!   principal == MaKo::Principal::"erp-tenant-a",
//!   action,
//!   resource
//! )
//! unless {
//!   resource has tenant && resource.tenant == "9900357000001"
//! };
//! ```
//!
//! ## Cedar entity model
//!
//! ```text
//! MaKo::Principal  — caller identity (name from key registry)
//! MaKo::Command    — ERP command (name, marktrolle, pid, tenant)
//! MaKo::EdifactIngest   — EDIFACT ingest endpoint (tenant)
//! MaKo::AdminMaloRecord   — MaLo admin resource (tenant, malo_id?)
//! MaKo::AdminPartnerRecord — partner admin resource (tenant, gln?)
//! ```
//!
//! ## Action groups
//!
//! The schema defines two action groups usable in Cedar policies:
//!
//! | Group | Members |
//! |---|---|
//! | `AdminMalo` | `AdminMaloRead`, `AdminMaloWrite`, `AdminMaloDelete`, `AdminMaloStats` |
//! | `AdminPartner` | `AdminPartnerRead`, `AdminPartnerWrite`, `AdminPartnerDelete`, `AdminPartnerImport` |
//!
//! ```cedar
//! // Grant a monitoring principal read-only access to both admin sections.
//! permit(
//!   principal == MaKo::Principal::"grafana-ro",
//!   action in [MaKo::Action::"AdminMalo", MaKo::Action::"AdminPartner"],
//!   resource
//! ) when { action == MaKo::Action::"AdminMaloStats"
//!       || action == MaKo::Action::"AdminPartnerRead" };
//! ```
//!
//! ## OIDC / JWT authentication
//!
//! When an [`OidcVerifier`] is supplied to [`CedarAuthorizer::new`], bearer
//! tokens shaped like JWTs (three dot-separated Base64url parts) are validated
//! against the issuer's cached JWKS.  The JWT `sub` claim becomes the Cedar
//! principal entity ID — identical to how API-key names are used — so all
//! Cedar policies work unchanged regardless of the authentication method.
//!
//! API-key authentication and OIDC coexist on the same port: the token shape
//! (JWT vs opaque hex) determines which path is taken.  This allows gradual
//! migration: add `--oidc-issuer` without removing existing `--auth-key` entries.
//!
//! ```cedar
//! // Restrict an Azure Managed Identity (identified by its `sub`) to read-only.
//! forbid(
//!   principal == MaKo::Principal::"<azure-managed-identity-object-id>",
//!   action in [
//!     MaKo::Action::"AdminMaloWrite",
//!     MaKo::Action::"AdminMaloDelete",
//!     MaKo::Action::"SubmitCommand",
//!     MaKo::Action::"IngestEdifact"
//!   ],
//!   resource
//! );
//! ```
//!
//! [`OidcVerifier`]: crate::oidc_verifier::OidcVerifier

use std::str::FromStr as _;
use std::sync::Arc;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use secrecy::{ExposeSecret as _, SecretString};
use subtle::ConstantTimeEq as _;

use crate::oidc_verifier::OidcVerifier;

// ── Embedded files ────────────────────────────────────────────────────────────────────────

const DEFAULT_POLICIES: &str = include_str!("cedar/default.cedar");
const SCHEMA_SRC: &str = include_str!("cedar/mako.cedarschema");

// ── Named API keys ───────────────────────────────────────────────────────────

/// A named API key.
///
/// Maps a bearer token to a Cedar principal (`MaKo::Principal::"<name>"`).
/// The name is immutable after construction and appears in all audit logs.
pub struct NamedKey {
    /// Principal name — Cedar entity ID for this key.
    pub name: Arc<str>,
    /// Raw bearer token (never logged).
    pub token: SecretString,
}

impl NamedKey {
    /// Parse a `NAME=TOKEN` argument into a [`NamedKey`].
    ///
    /// The first `=` separates name from token; leading/trailing whitespace
    /// on both sides is stripped.
    pub fn from_arg(s: &str) -> Result<Self, AuthzBuildError> {
        let eq = s
            .find('=')
            .ok_or_else(|| AuthzBuildError::InvalidKeyArg(s.to_owned()))?;
        let name = s[..eq].trim();
        let token = s[eq + 1..].trim();
        if name.is_empty() || token.is_empty() {
            return Err(AuthzBuildError::InvalidKeyArg(s.to_owned()));
        }
        Ok(Self {
            name: Arc::from(name),
            token: SecretString::new(token.to_owned().into()),
        })
    }
}

// ── CallerIdentity ───────────────────────────────────────────────────────────

/// Resolved, authenticated caller identity.
///
/// Produced by [`CedarAuthorizer::authenticate`] when the bearer token matches
/// a registered [`NamedKey`].  The `name` is the Cedar principal entity ID and
/// appears verbatim in tracing spans and audit logs.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// Principal name (e.g. `"erp-sap-prod"`, `"ci-pipeline"`).
    pub name: Arc<str>,
}

// ── MaKo Cedar actions ───────────────────────────────────────────────────────

/// All actions in the `MaKo` Cedar namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakoAction {
    /// Submit an ERP command — `POST /api/v1/commands`.
    SubmitCommand,
    /// Ingest a raw EDIFACT interchange — `POST /edifact`.
    IngestEdifact,
    /// Read a MaLo record — `GET /admin/malo/{malo_id}`.
    AdminMaloRead,
    /// Write (upsert) a MaLo record — `PUT /admin/malo/{malo_id}`.
    AdminMaloWrite,
    /// Delete a MaLo record — `DELETE /admin/malo/{malo_id}`.
    AdminMaloDelete,
    /// Read per-tenant MaLo statistics — `GET /admin/malo/stats`.
    AdminMaloStats,
    /// List or read trading-partner records.
    AdminPartnerRead,
    /// Create or update a trading-partner record.
    AdminPartnerWrite,
    /// Delete a trading-partner record.
    AdminPartnerDelete,
    /// Import partners from a PARTIN interchange.
    AdminPartnerImport,
    /// Read Prometheus operational metrics — `GET /metrics`.
    ReadMetrics,
    /// Use the MCP server at `/mcp` — covers all MCP tool invocations.
    UseMcp,
    /// Read a stored BO4E Rechnung — `GET /api/v1/invoic/{process_id}/rechnung`.
    ReadRechnung,
    /// Use the `:8090` API-Webdienste Strom endpoints.
    UseWebdienste,
    /// Read process state (MCP `get_process` / `list_active_processes`).
    ReadProcess,
    /// Trigger a process migration — `POST /admin/migrations`.
    AdminMigrations,
}

impl MakoAction {
    fn cedar_id(self) -> &'static str {
        match self {
            Self::SubmitCommand => "SubmitCommand",
            Self::IngestEdifact => "IngestEdifact",
            Self::AdminMaloRead => "AdminMaloRead",
            Self::AdminMaloWrite => "AdminMaloWrite",
            Self::AdminMaloDelete => "AdminMaloDelete",
            Self::AdminMaloStats => "AdminMaloStats",
            Self::AdminPartnerRead => "AdminPartnerRead",
            Self::AdminPartnerWrite => "AdminPartnerWrite",
            Self::AdminPartnerDelete => "AdminPartnerDelete",
            Self::AdminPartnerImport => "AdminPartnerImport",
            Self::ReadMetrics => "ReadMetrics",
            Self::UseMcp => "UseMcp",
            Self::ReadRechnung => "ReadRechnung",
            Self::UseWebdienste => "UseWebdienste",
            Self::ReadProcess => "ReadProcess",
            Self::AdminMigrations => "AdminMigrations",
        }
    }
}

// ── Resource descriptors ─────────────────────────────────────────────────────

/// Resource descriptor for `SubmitCommand` checks.
pub struct CommandResource<'a> {
    /// Dotted command name (e.g. `"gpke.lieferbeginn.anmelden"`).
    pub name: &'a str,
    /// Effective Marktrolle resolved from the command registry (e.g. `"LF"`).
    pub marktrolle: &'a str,
    /// Prüfidentifikator associated with this command (e.g. `55001`).
    pub pid: u32,
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

/// Resource descriptor for `IngestEdifact` checks.
pub struct IngestResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

/// Resource descriptor for MaLo admin checks.
pub struct MaloResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
    /// 11-digit MaLo ID, present for single-record operations; `None` for stats.
    pub malo_id: Option<&'a str>,
}

/// Resource descriptor for partner admin checks.
pub struct PartnerResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
    /// Partner GLN, present for single-record operations; `None` for list/import.
    pub mp_id: Option<&'a str>,
}

/// Resource descriptor for metrics endpoint checks.
pub struct MetricsResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

/// Resource descriptor for API-Webdienste (`:8090`) checks.
pub struct WebdiensteResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

/// Resource descriptor for process-state reads (§9 EnWG unbundling scope).
pub struct ProcessResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
    /// Workflow name of the process being read (e.g. `"gpke-lf-anmeldung"`).
    ///
    /// The workflow name encodes the Marktrolle side of the process, so a
    /// VIU deployment can write Cedar policies that keep an NB-scoped
    /// principal out of LF process state (§9 EnWG Informatorisches
    /// Unbundling) by matching on `context.workflow`.
    pub workflow: &'a str,
}

/// Resource descriptor for Rechnung read checks.
pub struct RechnungResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

/// Resource descriptor for migration-trigger checks.
pub struct MigrationResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

/// Resource descriptor for MCP endpoint checks.
pub struct McpResource<'a> {
    /// Operator tenant (GLN).
    pub tenant: &'a str,
}

// ── Error types ──────────────────────────────────────────────────────────────

/// Errors constructing a [`CedarAuthorizer`].
#[derive(Debug, thiserror::Error)]
pub enum AuthzBuildError {
    /// The `--auth-key` argument was not in `NAME=TOKEN` format.
    #[error("invalid --auth-key argument {0:?}: expected NAME=TOKEN")]
    InvalidKeyArg(String),
    /// Cedar policy text could not be parsed.
    #[error("Cedar policy parse error: {0}")]
    PolicyParse(String),
    /// Cedar schema could not be parsed.
    #[error("Cedar schema error: {0}")]
    SchemaError(String),
}

// ── CedarAuthorizer ──────────────────────────────────────────────────────────

/// Cedar-based authorization engine for all `makod` HTTP endpoints.
///
/// Thread-safe; cheap to clone (inner state is `Arc`-wrapped).
#[derive(Clone)]
pub struct CedarAuthorizer {
    inner: Arc<Inner>,
}

struct Inner {
    authorizer: Authorizer,
    policy_set: PolicySet,
    schema: Schema,
    keys: Vec<NamedKey>,
    oidc: Option<OidcVerifier>,
}

impl CedarAuthorizer {
    /// Build an authorizer from named keys, an optional extra policy string,
    /// and an optional OIDC verifier.
    ///
    /// The embedded default policy is always included.  `extra_policies` is
    /// concatenated on top and is typically the content of `.cedar` files
    /// loaded from `--cedar-policy-dir`.
    ///
    /// When `oidc` is `Some`, bearer tokens shaped like JWTs (three
    /// dot-separated parts) are validated against the OIDC issuer's cached
    /// JWKS.  API-key and OIDC authentication coexist — the token shape
    /// determines which path is taken.
    pub fn new(
        keys: Vec<NamedKey>,
        extra_policies: Option<String>,
        oidc: Option<OidcVerifier>,
    ) -> Result<Self, AuthzBuildError> {
        // Parse schema from Cedar schema syntax (human-readable, with warnings).
        let (schema, schema_warnings) = Schema::from_cedarschema_str(SCHEMA_SRC)
            .map_err(|e| AuthzBuildError::SchemaError(e.to_string()))?;
        for w in schema_warnings {
            tracing::warn!("cedar schema warning: {w}");
        }

        let mut combined = DEFAULT_POLICIES.to_owned();
        if let Some(extra) = extra_policies {
            combined.push('\n');
            combined.push_str(&extra);
        }

        let policy_set = PolicySet::from_str(&combined)
            .map_err(|e| AuthzBuildError::PolicyParse(e.to_string()))?;

        // Validate policies against schema at startup — catches operator typos
        // (unknown action names, wrong attribute types, etc.) before any request.
        let validation =
            Validator::new(schema.clone()).validate(&policy_set, ValidationMode::Strict);
        for w in validation.validation_warnings() {
            tracing::warn!("cedar policy warning: {w}");
        }
        if !validation.validation_passed() {
            let errors: String = validation
                .validation_errors()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(AuthzBuildError::PolicyParse(errors));
        }

        Ok(Self {
            inner: Arc::new(Inner {
                authorizer: Authorizer::new(),
                policy_set,
                schema,
                keys,
                oidc,
            }),
        })
    }

    /// Build an open-access authorizer for internal / loopback use only.
    ///
    /// Every call to [`authenticate`][Self::authenticate] returns a fixed
    /// `"anonymous"` identity, and the default policy permits it to perform
    /// any action.  **Never expose this on a public port.**
    ///
    /// Used for the AS4 in-process ingest and loopback delivery paths where
    /// the calling code is trusted infrastructure, not an external ERP.
    pub fn unauthenticated() -> Result<Self, AuthzBuildError> {
        let anonymous_policy = concat!(
            "permit(\n",
            "  principal == MaKo::Principal::\"anonymous\",\n",
            "  action,\n",
            "  resource\n",
            ");\n",
        );
        Self::new(vec![], Some(anonymous_policy.to_owned()), None)
    }

    // ── Authentication ────────────────────────────────────────────────────────

    /// Resolve the `Authorization: Bearer <token>` header to a [`CallerIdentity`].
    ///
    /// Returns `None` if the header is absent, the token does not match any
    /// registered key, or JWT validation fails.  The caller **must** return
    /// `401 Unauthorized` in that case.
    ///
    /// **Routing:** tokens shaped like JWTs (three dot-separated Base64url
    /// parts) are verified by the OIDC verifier when one is configured.  All
    /// other tokens are compared against the API-key registry in constant time
    /// to prevent timing attacks.
    pub fn authenticate(&self, headers: &axum::http::HeaderMap) -> Option<CallerIdentity> {
        // Open-access (unauthenticated) mode — no keys and no OIDC.
        // Used only for internal/loopback paths; never expose on a public port.
        if self.inner.keys.is_empty() && self.inner.oidc.is_none() {
            return Some(CallerIdentity {
                name: Arc::from("anonymous"),
            });
        }

        // HeaderMap::get is case-insensitive per HTTP spec; use the typed
        // constant to avoid redundant lookups.
        let provided = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))?;

        // Route by token shape: 3 dot-separated non-empty parts → JWT → OIDC.
        if OidcVerifier::looks_like_jwt(provided)
            && let Some(oidc) = &self.inner.oidc
        {
            return match oidc.verify(provided) {
                Ok(claims) => {
                    tracing::debug!(sub = %claims.sub, "OIDC: JWT authenticated");
                    Some(CallerIdentity {
                        name: Arc::from(claims.sub.as_str()),
                    })
                }
                Err(e) => {
                    tracing::info!("OIDC: JWT rejected: {e}");
                    None
                }
            };
        }

        // API-key lookup (constant-time comparison).
        for key in &self.inner.keys {
            let ok: bool = provided
                .as_bytes()
                .ct_eq(key.token.expose_secret().as_bytes())
                .into();
            if ok {
                return Some(CallerIdentity {
                    name: Arc::clone(&key.name),
                });
            }
        }
        None
    }

    // ── Authorization ─────────────────────────────────────────────────────────

    /// Evaluate a Cedar authorization request for an ERP command submission.
    ///
    /// The resource attributes (`name`, `marktrolle`, `pid`, `tenant`) are
    /// populated from the resolved command so that operator policies can
    /// restrict specific principals to specific commands, Marktrollen, or PIDs.
    pub fn authorize_command(&self, identity: &CallerIdentity, res: &CommandResource<'_>) -> bool {
        let resource_uid = entity_uid("MaKo::Command", res.name);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([
                ("name".to_owned(), cedar_str(res.name)),
                ("marktrolle".to_owned(), cedar_str(res.marktrolle)),
                ("pid".to_owned(), cedar_long(res.pid as i64)),
                ("tenant".to_owned(), cedar_str(res.tenant)),
            ]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build Command entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::SubmitCommand,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({
                "tenant":     res.tenant,
                "marktrolle": res.marktrolle,
                "pid":        res.pid as i64
            }),
        )
    }

    /// Evaluate authorization for an EDIFACT ingest request.
    pub fn authorize_ingest(&self, identity: &CallerIdentity, res: &IngestResource<'_>) -> bool {
        let resource_uid = entity_uid("MaKo::EdifactIngest", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build EdifactIngest entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::IngestEdifact,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for a MaLo admin operation.
    pub fn authorize_malo(
        &self,
        identity: &CallerIdentity,
        action: MakoAction,
        res: &MaloResource<'_>,
    ) -> bool {
        debug_assert!(matches!(
            action,
            MakoAction::AdminMaloRead
                | MakoAction::AdminMaloWrite
                | MakoAction::AdminMaloDelete
                | MakoAction::AdminMaloStats
        ));
        let resource_id = res.malo_id.unwrap_or(res.tenant);
        let resource_uid = entity_uid("MaKo::AdminMaloRecord", resource_id);
        let mut attrs =
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]);
        if let Some(malo_id) = res.malo_id {
            attrs.insert("malo_id".to_owned(), cedar_str(malo_id));
        }
        let resource = match Entity::new(
            resource_uid.clone(),
            attrs,
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build AdminMaloRecord entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            action,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for a partner admin operation.
    pub fn authorize_partner(
        &self,
        identity: &CallerIdentity,
        action: MakoAction,
        res: &PartnerResource<'_>,
    ) -> bool {
        debug_assert!(matches!(
            action,
            MakoAction::AdminPartnerRead
                | MakoAction::AdminPartnerWrite
                | MakoAction::AdminPartnerDelete
                | MakoAction::AdminPartnerImport
        ));
        let resource_id = res.mp_id.unwrap_or(res.tenant);
        let resource_uid = entity_uid("MaKo::AdminPartnerRecord", resource_id);
        let mut attrs =
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]);
        if let Some(mp_id) = res.mp_id {
            attrs.insert("mp_id".to_owned(), cedar_str(mp_id));
        }
        let resource = match Entity::new(
            resource_uid.clone(),
            attrs,
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build AdminPartnerRecord entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            action,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for a metrics scrape request.
    ///
    /// The caller must hold the `MaKo::Action::"ReadMetrics"` permission.
    /// In the default open-access policy all authenticated principals are
    /// permitted.  Operators can restrict metrics access to specific scrape
    /// principals (e.g. a Prometheus service account) by adding a `forbid`
    /// policy for other principals.
    pub fn authorize_metrics(&self, identity: &CallerIdentity, res: &MetricsResource<'_>) -> bool {
        let resource_uid = entity_uid("MaKo::MetricsEndpoint", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build MetricsEndpoint entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::ReadMetrics,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for the API-Webdienste (`:8090`) endpoints.
    pub fn authorize_webdienste(
        &self,
        identity: &CallerIdentity,
        res: &WebdiensteResource<'_>,
    ) -> bool {
        let resource_uid = entity_uid("MaKo::WebdiensteEndpoint", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build WebdiensteEndpoint entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::UseWebdienste,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for a process-state read.
    ///
    /// The `workflow` context lets §9 EnWG VIU deployments deny an NB-scoped
    /// principal access to LF process state and vice versa.
    pub fn authorize_process_read(
        &self,
        identity: &CallerIdentity,
        res: &ProcessResource<'_>,
    ) -> bool {
        let resource_uid = entity_uid("MaKo::ProcessRecord", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([
                ("tenant".to_owned(), cedar_str(res.tenant)),
                ("workflow".to_owned(), cedar_str(res.workflow)),
            ]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build ProcessRecord entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::ReadProcess,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant, "workflow": res.workflow }),
        )
    }

    /// Evaluate authorization for a Rechnung read — the stored BO4E invoice
    /// carries customer billing data and must never be an unauthenticated read.
    pub fn authorize_rechnung(
        &self,
        identity: &CallerIdentity,
        res: &RechnungResource<'_>,
    ) -> bool {
        let resource_uid = entity_uid("MaKo::RechnungEndpoint", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build RechnungEndpoint entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::ReadRechnung,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for triggering a process migration — a mutation
    /// over every in-flight process, so authentication alone is not enough.
    pub fn authorize_migrations(
        &self,
        identity: &CallerIdentity,
        res: &MigrationResource<'_>,
    ) -> bool {
        let resource_uid = entity_uid("MaKo::MigrationEndpoint", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build MigrationEndpoint entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::AdminMigrations,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    /// Evaluate authorization for an MCP session request.
    ///
    /// Called by the MCP auth middleware for every `/mcp` HTTP request.
    /// The caller must hold the `MaKo::Action::"UseMcp"` permission.
    pub fn authorize_mcp(&self, identity: &CallerIdentity, res: &McpResource<'_>) -> bool {
        let resource_uid = entity_uid("MaKo::McpEndpoint", res.tenant);
        let resource = match Entity::new(
            resource_uid.clone(),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(res.tenant))]),
            std::collections::HashSet::new(),
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    "cedar: failed to build McpEndpoint entity: {e}"
                );
                return false;
            }
        };
        self.eval(
            identity,
            MakoAction::UseMcp,
            resource_uid,
            vec![principal_entity(identity), resource],
            serde_json::json!({ "tenant": res.tenant }),
        )
    }

    // ── Internal evaluation ───────────────────────────────────────────────────

    fn eval(
        &self,
        identity: &CallerIdentity,
        action: MakoAction,
        resource_uid: EntityUid,
        entities: Vec<Entity>,
        context_json: serde_json::Value,
    ) -> bool {
        let entities = match Entities::from_entities(entities, Some(&self.inner.schema)) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    action = action.cedar_id(),
                    "cedar: failed to build entities: {e}"
                );
                return false;
            }
        };

        // Build UIDs before context so we can pass the action UID to
        // Context::from_json_value — this validates the context record
        // against the schema's declared context type for this action.
        let principal_uid = entity_uid("MaKo::Principal", identity.name.as_ref());
        let action_uid = entity_uid("MaKo::Action", action.cedar_id());

        let context =
            match Context::from_json_value(context_json, Some((&self.inner.schema, &action_uid))) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        principal = %identity.name,
                        action = action.cedar_id(),
                        "cedar: failed to build context: {e}"
                    );
                    return false;
                }
            };

        let request = match Request::new(
            principal_uid,
            action_uid,
            resource_uid,
            context,
            Some(&self.inner.schema),
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    principal = %identity.name,
                    action = action.cedar_id(),
                    "cedar: failed to build request: {e}"
                );
                return false;
            }
        };

        let response =
            self.inner
                .authorizer
                .is_authorized(&request, &self.inner.policy_set, &entities);

        for err in response.diagnostics().errors() {
            tracing::warn!(
                principal = %identity.name,
                action = action.cedar_id(),
                "cedar: evaluation error: {err}"
            );
        }

        let allowed = response.decision() == Decision::Allow;
        if !allowed {
            tracing::info!(
                principal = %identity.name,
                action    = action.cedar_id(),
                "cedar: request denied"
            );
        }
        allowed
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build an [`EntityUid`] from a fully-qualified type name and an ID string.
fn entity_uid(type_name: &str, id: &str) -> EntityUid {
    let ty = EntityTypeName::from_str(type_name).expect("cedar: invalid entity type name");
    let eid = EntityId::from_str(id).expect("cedar: invalid entity id");
    EntityUid::from_type_name_and_id(ty, eid)
}

/// Build a Cedar `String` [`RestrictedExpression`] from a Rust `&str`.
fn cedar_str(s: &str) -> RestrictedExpression {
    // Cedar string literals use `\"` and `\\` as escape sequences.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    RestrictedExpression::from_str(&format!("\"{escaped}\""))
        .expect("cedar: string RestrictedExpression")
}

/// Build a Cedar `Long` [`RestrictedExpression`] from an `i64`.
fn cedar_long(n: i64) -> RestrictedExpression {
    RestrictedExpression::from_str(&n.to_string()).expect("cedar: long RestrictedExpression")
}

/// Build a no-attrs [`Entity`] for the caller principal.
fn principal_entity(identity: &CallerIdentity) -> Entity {
    Entity::with_uid(entity_uid("MaKo::Principal", identity.name.as_ref()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn authz(name: &str, token: &str) -> CedarAuthorizer {
        CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from(name),
                token: SecretString::new(token.to_owned().into()),
            }],
            None,
            None,
        )
        .expect("authorizer construction failed")
    }

    fn id(name: &str) -> CallerIdentity {
        CallerIdentity {
            name: Arc::from(name),
        }
    }

    fn bearer(token: &str) -> axum::http::HeaderMap {
        let mut m = axum::http::HeaderMap::new();
        m.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        m
    }

    // ── NamedKey::from_arg ─────────────────────────────────────────────────

    #[test]
    fn named_key_from_arg_valid() {
        let key = NamedKey::from_arg("erp-sap=secret123").unwrap();
        assert_eq!(key.name.as_ref(), "erp-sap");
        assert_eq!(key.token.expose_secret(), "secret123");
    }

    #[test]
    fn named_key_from_arg_token_may_contain_equals() {
        let key = NamedKey::from_arg("erp=tok=with=equals").unwrap();
        assert_eq!(key.name.as_ref(), "erp");
        assert_eq!(key.token.expose_secret(), "tok=with=equals");
    }

    #[test]
    fn named_key_from_arg_missing_separator() {
        assert!(NamedKey::from_arg("no-separator").is_err());
    }

    #[test]
    fn named_key_from_arg_empty_name() {
        assert!(NamedKey::from_arg("=token").is_err());
    }

    #[test]
    fn named_key_from_arg_empty_token() {
        assert!(NamedKey::from_arg("name=").is_err());
    }

    // ── Authentication ────────────────────────────────────────────────────────

    #[test]
    fn authenticate_matching_key() {
        let authz = authz("erp", "tok123");
        let id = authz
            .authenticate(&bearer("tok123"))
            .expect("must authenticate");
        assert_eq!(id.name.as_ref(), "erp");
    }

    #[test]
    fn authenticate_wrong_token_rejected() {
        let authz = authz("erp", "tok123");
        assert!(authz.authenticate(&bearer("wrong")).is_none());
    }

    #[test]
    fn authenticate_missing_header_rejected() {
        let authz = authz("erp", "tok123");
        assert!(authz.authenticate(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn authenticate_resolves_correct_principal_for_multiple_keys() {
        let authz = CedarAuthorizer::new(
            vec![
                NamedKey {
                    name: Arc::from("erp-a"),
                    token: SecretString::new("tok-a".into()),
                },
                NamedKey {
                    name: Arc::from("erp-b"),
                    token: SecretString::new("tok-b".into()),
                },
            ],
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            authz.authenticate(&bearer("tok-a")).unwrap().name.as_ref(),
            "erp-a"
        );
        assert_eq!(
            authz.authenticate(&bearer("tok-b")).unwrap().name.as_ref(),
            "erp-b"
        );
        assert!(authz.authenticate(&bearer("tok-c")).is_none());
    }

    // ── Authorization — default policy ────────────────────────────────────────

    #[test]
    fn default_policy_permits_submit_command() {
        let a = authz("erp", "tok");
        assert!(a.authorize_command(
            &id("erp"),
            &CommandResource {
                name: "gpke.lieferbeginn.anmelden",
                marktrolle: "LF",
                pid: 55001,
                tenant: "9900357000004",
            },
        ));
    }

    #[test]
    fn default_policy_permits_ingest() {
        let a = authz("erp", "tok");
        assert!(a.authorize_ingest(
            &id("erp"),
            &IngestResource {
                tenant: "9900357000004"
            }
        ));
    }

    #[test]
    fn default_policy_permits_malo_read() {
        let a = authz("erp", "tok");
        assert!(a.authorize_malo(
            &id("erp"),
            MakoAction::AdminMaloRead,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: Some("10001234567")
            },
        ));
    }

    #[test]
    fn default_policy_permits_partner_write() {
        let a = authz("erp", "tok");
        assert!(a.authorize_partner(
            &id("erp"),
            MakoAction::AdminPartnerWrite,
            &PartnerResource {
                tenant: "9900357000004",
                mp_id: Some("9900000000001")
            },
        ));
    }

    // ── Authorization — operator ABAC policies ────────────────────────────────

    #[test]
    fn forbid_write_denies_malo_write_for_readonly_key() {
        let extra = r#"
forbid(
  principal == MaKo::Principal::"readonly",
  action in [MaKo::Action::"AdminMaloWrite", MaKo::Action::"AdminMaloDelete"],
  resource
);
        "#;
        let a = CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("readonly"),
                token: SecretString::new("tok".into()),
            }],
            Some(extra.to_owned()),
            None,
        )
        .unwrap();
        // write denied
        assert!(!a.authorize_malo(
            &id("readonly"),
            MakoAction::AdminMaloWrite,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: Some("10001234567")
            },
        ));
        // read still permitted by default policy
        assert!(a.authorize_malo(
            &id("readonly"),
            MakoAction::AdminMaloRead,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: Some("10001234567")
            },
        ));
    }

    #[test]
    fn marktrolle_condition_blocks_wrong_role() {
        let extra = r#"
forbid(
  principal == MaKo::Principal::"gas-only",
  action    == MaKo::Action::"SubmitCommand",
  resource  is MaKo::Command
)
unless {
  resource.marktrolle == "LFG" || resource.marktrolle == "GNB"
};
        "#;
        let a = CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("gas-only"),
                token: SecretString::new("tok".into()),
            }],
            Some(extra.to_owned()),
            None,
        )
        .unwrap();
        // Strom command blocked
        assert!(!a.authorize_command(
            &id("gas-only"),
            &CommandResource {
                name: "gpke.lieferbeginn.anmelden",
                marktrolle: "LF",
                pid: 55001,
                tenant: "9900357000004",
            },
        ));
        // Gas command allowed
        assert!(a.authorize_command(
            &id("gas-only"),
            &CommandResource {
                name: "geli.lieferbeginn.anmelden",
                marktrolle: "LFG",
                pid: 44001,
                tenant: "9900357000004",
            },
        ));
    }

    #[test]
    fn pid_condition_restricts_to_specific_pid() {
        let extra = r#"
forbid(
  principal == MaKo::Principal::"gpke-only",
  action    == MaKo::Action::"SubmitCommand",
  resource  is MaKo::Command
)
unless {
  resource.pid == 55001
};
        "#;
        let a = CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("gpke-only"),
                token: SecretString::new("tok".into()),
            }],
            Some(extra.to_owned()),
            None,
        )
        .unwrap();
        // PID 55001 allowed
        assert!(a.authorize_command(
            &id("gpke-only"),
            &CommandResource {
                name: "gpke.lieferbeginn.anmelden",
                marktrolle: "LF",
                pid: 55001,
                tenant: "9900357000004",
            },
        ));
        // PID 55002 blocked
        assert!(!a.authorize_command(
            &id("gpke-only"),
            &CommandResource {
                name: "gpke.lieferende.anmelden",
                marktrolle: "LF",
                pid: 55002,
                tenant: "9900357000004",
            },
        ));
    }

    #[test]
    fn tenant_condition_blocks_cross_tenant_access() {
        let extra = r#"
forbid(
  principal == MaKo::Principal::"tenant-a-only",
  action,
  resource
)
unless {
  resource has tenant && resource.tenant == "9900357000001"
};
        "#;
        let a = CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("tenant-a-only"),
                token: SecretString::new("tok".into()),
            }],
            Some(extra.to_owned()),
            None,
        )
        .unwrap();
        assert!(!a.authorize_malo(
            &id("tenant-a-only"),
            MakoAction::AdminMaloRead,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: None
            },
        ));
        assert!(a.authorize_malo(
            &id("tenant-a-only"),
            MakoAction::AdminMaloRead,
            &MaloResource {
                tenant: "9900357000001",
                malo_id: None
            },
        ));
    }

    // ── Authorization — action groups ─────────────────────────────────────────

    #[test]
    fn action_group_admin_malo_covers_all_malo_actions() {
        // Forbid the principal from the AdminMalo group (all 4 malo actions)
        // but allow AdminMaloRead via a permit.
        let extra = r#"
forbid(
  principal == MaKo::Principal::"stats-only",
  action in [MaKo::Action::"AdminMalo"],
  resource
)
unless {
  action == MaKo::Action::"AdminMaloStats"
};
        "#;
        let a = CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("stats-only"),
                token: SecretString::new("tok".into()),
            }],
            Some(extra.to_owned()),
            None,
        )
        .unwrap();
        // Stats permitted
        assert!(a.authorize_malo(
            &id("stats-only"),
            MakoAction::AdminMaloStats,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: None,
            },
        ));
        // All other Malo actions denied
        for action in [
            MakoAction::AdminMaloRead,
            MakoAction::AdminMaloWrite,
            MakoAction::AdminMaloDelete,
        ] {
            assert!(
                !a.authorize_malo(
                    &id("stats-only"),
                    action,
                    &MaloResource {
                        tenant: "9900357000004",
                        malo_id: Some("10001234567"),
                    },
                ),
                "expected {action:?} to be denied",
            );
        }
    }

    #[test]
    fn action_group_admin_partner_covers_all_partner_actions() {
        let extra = r#"
forbid(
  principal == MaKo::Principal::"partner-readonly",
  action in [MaKo::Action::"AdminPartner"],
  resource
)
unless {
  action == MaKo::Action::"AdminPartnerRead"
};
        "#;
        let a = CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("partner-readonly"),
                token: SecretString::new("tok".into()),
            }],
            Some(extra.to_owned()),
            None,
        )
        .unwrap();
        // Read permitted
        assert!(a.authorize_partner(
            &id("partner-readonly"),
            MakoAction::AdminPartnerRead,
            &PartnerResource {
                tenant: "9900357000004",
                mp_id: None,
            },
        ));
        // Write / delete / import denied
        for action in [
            MakoAction::AdminPartnerWrite,
            MakoAction::AdminPartnerDelete,
            MakoAction::AdminPartnerImport,
        ] {
            assert!(
                !a.authorize_partner(
                    &id("partner-readonly"),
                    action,
                    &PartnerResource {
                        tenant: "9900357000004",
                        mp_id: Some("9900000000001"),
                    },
                ),
                "expected {action:?} to be denied",
            );
        }
    }
}
