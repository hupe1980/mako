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
//! The reusable mechanics — named-key registry, JWT/API-key bearer routing,
//! schema parsing, strict policy validation, and request evaluation — live in
//! [`mako_service::cedar_schema`].  This module contributes the **MaKo domain
//! layer**: the `MaKo::` Cedar namespace (embedded schema + default policy),
//! the typed [`MakoAction`] enum, the resource descriptors, and the
//! `authorize_*` methods that build the domain entities.
//!
//! The [`CedarAuthorizer`] is constructed once at startup and shared via
//! `Arc` across all API states.  It holds:
//!
//! - A **named-key registry** — maps `Authorization: Bearer <token>` to a
//!   Cedar principal (`MaKo::Principal::"<name>"`).
//! - A compiled policy set — operator-supplied policies from
//!   `--cedar-policy-dir`, over the embedded default policy unless
//!   `--cedar-no-default-policy` omits it.
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
//! authenticated principal to perform any action.
//!
//! A Cedar request is allowed when **any** `permit` matches and no `forbid`
//! does, so while that baseline is in the policy set an operator `permit`
//! grants nothing that is not already granted — only `forbid` narrows it. To
//! build up from nothing instead, start `makod` with
//! `--cedar-no-default-policy` ([`DefaultPolicy::Deny`]); the baseline is then
//! omitted and `--cedar-policy-dir` becomes the only source of access. That is
//! the mode `cedar/conservative.cedar` and §9 EnWG role separation require.
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
//! Tokens do **not** need the `mako_tenant` custom claim other mako services
//! require: `makod` authorizes on `sub` alone via Cedar.
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
//! (The principal ID above would be whatever `sub` the IdP puts in the JWT —
//! typically a UUID for Azure, or a stable identifier for Kubernetes.)
//!
//! [`OidcVerifier`]: mako_service::oidc::OidcVerifier

use std::sync::Arc;

use cedar_policy::{Entity, EntityUid};
use mako_service::cedar_schema::{
    BearerAuthenticator, SchemaPolicySet, cedar_long, cedar_str, entity_uid,
};
use mako_service::oidc::OidcVerifier;

// Re-exports so call sites and tests keep using `crate::cedar_authz::{…}`.
pub use mako_service::cedar_schema::{AuthzBuildError, CallerIdentity, NamedKey};

// ── Embedded files ────────────────────────────────────────────────────────────────────────

const DEFAULT_POLICIES: &str = include_str!("../cedar/default.cedar");
const SCHEMA_SRC: &str = include_str!("../cedar/mako.cedarschema");

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
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
}

/// Resource descriptor for `IngestEdifact` checks.
pub struct IngestResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
}

/// Resource descriptor for MaLo admin checks.
pub struct MaloResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
    /// 11-digit MaLo ID, present for single-record operations; `None` for stats.
    pub malo_id: Option<&'a str>,
}

/// Resource descriptor for partner admin checks.
pub struct PartnerResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
    /// Partner MP-ID, present for single-record operations; `None` for list/import.
    pub mp_id: Option<&'a str>,
}

/// Resource descriptor for metrics endpoint checks.
pub struct MetricsResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
}

/// Resource descriptor for API-Webdienste (`:8090`) checks.
pub struct WebdiensteResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
}

/// Resource descriptor for process-state reads (§9 EnWG unbundling scope).
pub struct ProcessResource<'a> {
    /// Operator tenant (MP-ID).
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
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
}

/// Resource descriptor for migration-trigger checks.
pub struct MigrationResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
}

/// Resource descriptor for MCP endpoint checks.
pub struct McpResource<'a> {
    /// Operator tenant (MP-ID).
    pub tenant: &'a str,
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
    engine: SchemaPolicySet,
    auth: BearerAuthenticator,
}

/// Whether the compiled-in `default.cedar` baseline is part of the policy set.
///
/// Cedar is deny-by-default, but `default.cedar` contains a catch-all
/// `permit(principal is MaKo::Principal, action, resource)`. Because a Cedar
/// decision is *allow if any permit matches and no forbid matches*, an
/// operator's own `permit` statements can never narrow that catch-all — only a
/// `forbid` can. A least-privilege policy set therefore has to omit the
/// baseline entirely, which is what [`DefaultPolicy::Deny`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultPolicy {
    /// Include the baseline: every authenticated principal may perform every
    /// action unless an operator `forbid` removes it. Suitable for development
    /// and single-tenant deployments where all keys are trusted equally.
    PermitAll,
    /// Omit the baseline. Access comes only from operator-supplied `permit`
    /// statements, so anything not granted is denied. Required for a
    /// least-privilege deployment and for §9 EnWG role separation.
    Deny,
}

impl CedarAuthorizer {
    /// Build an authorizer from named keys, an optional extra policy string,
    /// and an optional OIDC verifier.
    ///
    /// `extra_policies` is typically the content of `.cedar` files loaded from
    /// `--cedar-policy-dir`. `default_policy` decides whether the compiled-in
    /// permit-all baseline sits underneath them — see [`DefaultPolicy`], and
    /// note that with [`DefaultPolicy::PermitAll`] an operator `permit` grants
    /// nothing new and only a `forbid` restricts.
    ///
    /// When `oidc` is `Some`, bearer tokens shaped like JWTs (three
    /// dot-separated parts) are validated against the OIDC issuer's cached
    /// JWKS.  API-key and OIDC authentication coexist — the token shape
    /// determines which path is taken.
    /// `expected_tenant` pins the operator this deployment serves: a JWT whose
    /// `mako_tenant` differs — or is absent — is rejected at authentication.
    /// `OidcVerifier` checks issuer, audience and expiry but not the tenant, so
    /// without this a validly signed token from another operator in the same
    /// OIDC realm would authenticate here.
    pub fn new(
        keys: Vec<NamedKey>,
        extra_policies: Option<String>,
        oidc: Option<OidcVerifier>,
        expected_tenant: Option<String>,
        default_policy: DefaultPolicy,
    ) -> Result<Self, AuthzBuildError> {
        // Omitting the baseline *and* supplying no policies of your own denies
        // every request, including the operator's own. That is a misconfigured
        // service rather than a locked-down one, so refuse at startup instead
        // of serving 403 to everything.
        if default_policy == DefaultPolicy::Deny && extra_policies.is_none() {
            return Err(AuthzBuildError::PolicyParse(
                "--cedar-no-default-policy omits the permit-all baseline, so every request \
                 would be denied. Supply your own grants with --cedar-policy-dir (see the \
                 shipped conservative.cedar for a least-privilege starting point)."
                    .to_owned(),
            ));
        }
        let baseline = match default_policy {
            DefaultPolicy::PermitAll => DEFAULT_POLICIES,
            DefaultPolicy::Deny => "",
        };
        let engine = SchemaPolicySet::new(SCHEMA_SRC, baseline, extra_policies)?;
        Ok(Self {
            inner: Arc::new(Inner {
                engine,
                auth: match expected_tenant {
                    Some(t) => BearerAuthenticator::new(keys, oidc).with_expected_tenant(t),
                    None => BearerAuthenticator::new(keys, oidc),
                },
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
        // `Deny` rather than `PermitAll`: the anonymous grant above is the whole
        // intended policy, so the baseline would only add a second, broader way
        // to reach the same allow.
        Self::new(
            vec![],
            Some(anonymous_policy.to_owned()),
            None,
            None,
            DefaultPolicy::Deny,
        )
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
    /// to prevent timing attacks.  With no keys and no OIDC configured the
    /// authorizer runs in open-access mode and resolves every call to the
    /// fixed `"anonymous"` identity.
    pub fn authenticate(&self, headers: &axum::http::HeaderMap) -> Option<CallerIdentity> {
        self.inner.auth.authenticate(headers)
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
        let principal_uid = entity_uid("MaKo::Principal", identity.name.as_ref());
        let action_uid = entity_uid("MaKo::Action", action.cedar_id());
        self.inner.engine.eval(
            principal_uid,
            action_uid,
            resource_uid,
            entities,
            context_json,
        )
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build a no-attrs [`Entity`] for the caller principal.
fn principal_entity(identity: &CallerIdentity) -> Entity {
    Entity::with_uid(entity_uid("MaKo::Principal", identity.name.as_ref()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Generic mechanics (NamedKey parsing, bearer authentication routing,
// constant-time key matching) are tested in `mako_service::cedar_schema`.
// The tests here cover the MaKo **policy semantics**: default policy,
// operator ABAC conditions (Marktrolle / PID / tenant gates), action groups.

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    fn authz(name: &str, token: &str) -> CedarAuthorizer {
        CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from(name),
                token: SecretString::new(token.to_owned().into()),
            }],
            None,
            None,
            None,
            DefaultPolicy::PermitAll,
        )
        .expect("authorizer construction failed")
    }

    fn id(name: &str) -> CallerIdentity {
        CallerIdentity {
            name: Arc::from(name),
        }
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
                malo_id: Some("10001234558")
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
            None,
            DefaultPolicy::PermitAll,
        )
        .unwrap();
        // write denied
        assert!(!a.authorize_malo(
            &id("readonly"),
            MakoAction::AdminMaloWrite,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: Some("10001234558")
            },
        ));
        // read still permitted by default policy
        assert!(a.authorize_malo(
            &id("readonly"),
            MakoAction::AdminMaloRead,
            &MaloResource {
                tenant: "9900357000004",
                malo_id: Some("10001234558")
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
            None,
            DefaultPolicy::PermitAll,
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
            None,
            DefaultPolicy::PermitAll,
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
            None,
            DefaultPolicy::PermitAll,
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
        // but allow AdminMaloStats via an unless-condition.
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
            None,
            DefaultPolicy::PermitAll,
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
                        malo_id: Some("10001234558"),
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
            None,
            DefaultPolicy::PermitAll,
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

    // ── Default-policy baseline (§9 EnWG / least privilege) ───────────────────

    /// A least-privilege policy set is only least-privilege when the built-in
    /// catch-all is gone.
    ///
    /// This is the trap `conservative.cedar` used to fall into: it grants four
    /// named principals a narrow set of actions, but the baseline
    /// `permit(principal is MaKo::Principal, action, resource)` was compiled in
    /// unconditionally, and a Cedar request allows on *any* matching permit. So
    /// every named grant was redundant and every unlisted principal kept full
    /// access — the file documented a restriction it could not impose.
    #[test]
    fn permit_all_baseline_swallows_a_least_privilege_policy_set() {
        // "erp-operator" may submit commands; nothing grants "stray" anything.
        let extra = r#"
permit(
  principal == MaKo::Principal::"erp-operator",
  action    == MaKo::Action::"SubmitCommand",
  resource  is MaKo::Command
);
        "#;
        let with_baseline = CedarAuthorizer::new(
            vec![],
            Some(extra.to_owned()),
            None,
            None,
            DefaultPolicy::PermitAll,
        )
        .unwrap();
        assert!(
            with_baseline.authorize_malo(
                &id("stray"),
                MakoAction::AdminMaloDelete,
                &MaloResource {
                    tenant: "9900357000004",
                    malo_id: Some("10001234558"),
                },
            ),
            "with the baseline present an unlisted principal still gets everything — \
             this is why DefaultPolicy::Deny exists"
        );

        let without_baseline = CedarAuthorizer::new(
            vec![],
            Some(extra.to_owned()),
            None,
            None,
            DefaultPolicy::Deny,
        )
        .unwrap();
        assert!(
            !without_baseline.authorize_malo(
                &id("stray"),
                MakoAction::AdminMaloDelete,
                &MaloResource {
                    tenant: "9900357000004",
                    malo_id: Some("10001234558"),
                },
            ),
            "an unlisted principal must be denied once the baseline is dropped"
        );
        assert!(
            without_baseline.authorize_command(
                &id("erp-operator"),
                &CommandResource {
                    tenant: "9900357000004",
                    name: "gpke.lieferbeginn.anmelden",
                    marktrolle: "LF",
                    pid: 55001,
                },
            ),
            "the grant the operator did write must still work"
        );
    }

    /// Omitting the baseline with no replacement denies everything, including
    /// the operator's own traffic. Fail at startup rather than at request time.
    #[test]
    fn deny_baseline_without_policies_is_refused_at_construction() {
        let Err(err) = CedarAuthorizer::new(vec![], None, None, None, DefaultPolicy::Deny) else {
            panic!("a policy set that denies everything must not build");
        };
        assert!(
            err.to_string().contains("--cedar-policy-dir"),
            "the error must name the flag that supplies the grants, got: {err}"
        );
    }

    /// §9 EnWG informatorisches Unbundling: in a combined-role (VIU) deployment
    /// an NB-scoped principal must not read supply-side process state.
    ///
    /// This pins the mechanism both `get_process` and `list_overdue_deadlines`
    /// rely on — the workflow name reaches Cedar as a context attribute, so a
    /// site policy can discriminate on it.
    #[test]
    fn viu_policy_separates_grid_and_supply_process_reads() {
        let extra = r#"
permit(
  principal == MaKo::Principal::"nb-operator",
  action    == MaKo::Action::"ReadProcess",
  resource  is MaKo::ProcessRecord
)
when { context.workflow like "gpke-sperrung*" };
        "#;
        let a = CedarAuthorizer::new(
            vec![],
            Some(extra.to_owned()),
            None,
            None,
            DefaultPolicy::Deny,
        )
        .unwrap();

        assert!(
            a.authorize_process_read(
                &id("nb-operator"),
                &ProcessResource {
                    tenant: "9900357000004",
                    workflow: "gpke-sperrung",
                },
            ),
            "the NB arm must still read its own Sperrung processes"
        );
        assert!(
            !a.authorize_process_read(
                &id("nb-operator"),
                &ProcessResource {
                    tenant: "9900357000004",
                    workflow: "gpke-lieferbeginn",
                },
            ),
            "§9 EnWG: the grid arm must not see supply-side process state"
        );
    }
}
