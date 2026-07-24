//! Schema-validated Cedar authorization engine with named-key / OIDC
//! bearer authentication.
//!
//! This module contains the **reusable mechanics** behind a full-blown Cedar
//! deployment (as used by `makod`):
//!
//! - [`NamedKey`](crate::cedar_schema::NamedKey) — a `NAME=TOKEN` API key
//!   whose name becomes the Cedar principal entity ID.  Tokens are stored as
//!   `SecretString` and matched in constant time.
//! - [`BearerAuthenticator`](crate::cedar_schema::BearerAuthenticator) —
//!   resolves `Authorization: Bearer <token>` to a
//!   [`CallerIdentity`](crate::cedar_schema::CallerIdentity), routing
//!   JWT-shaped tokens to an optional [`OidcVerifier`](crate::oidc::OidcVerifier)
//!   and everything else to the named-key table.  With no keys and no OIDC it
//!   runs in **anonymous mode** (internal/loopback use only — never expose on
//!   a public port).
//! - [`SchemaPolicySet`](crate::cedar_schema::SchemaPolicySet) — an embedded
//!   default policy plus operator-supplied extras, parsed against a Cedar
//!   schema and validated with `ValidationMode::Strict` at construction, with
//!   a generic [`eval`](crate::cedar_schema::SchemaPolicySet::eval) helper.
//!
//! The **domain layer** — entity types, action enums, resource descriptors,
//! and the embedded schema/policy text — stays in the consuming service
//! (see `makod`'s `cedar_authz` module).  Services that only need simple
//! context-based checks should keep using [`crate::cedar::CedarEnforcer`].

use std::str::FromStr as _;
use std::sync::Arc;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use secrecy::{ExposeSecret as _, SecretString};
use subtle::ConstantTimeEq as _;

use crate::oidc::OidcVerifier;

// ── Named API keys ───────────────────────────────────────────────────────────

/// A named API key.
///
/// Maps a bearer token to a Cedar principal entity ID.  The name is immutable
/// after construction and appears in all audit logs.
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
/// Produced by [`BearerAuthenticator::authenticate`] when the bearer token
/// matches a registered [`NamedKey`] or verifies as an OIDC JWT.  The `name`
/// is the Cedar principal entity ID and appears verbatim in tracing spans and
/// audit logs.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// Principal name (e.g. `"erp-sap-prod"`, `"ci-pipeline"`, a JWT `sub`).
    pub name: Arc<str>,
}

// ── Error types ──────────────────────────────────────────────────────────────

/// Errors constructing an authenticator or schema-validated policy set.
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

// ── BearerAuthenticator ──────────────────────────────────────────────────────

/// Resolves bearer tokens to caller identities.
///
/// Combines a named-key registry with an optional OIDC verifier:
///
/// - tokens shaped like JWTs (three dot-separated Base64url parts) are
///   verified against the OIDC issuer's cached JWKS when a verifier is
///   configured; the JWT `sub` claim becomes the identity name,
/// - all other tokens are compared against the key table in constant time,
/// - **no keys + no OIDC** ⇒ anonymous mode: every call resolves to the fixed
///   identity `"anonymous"` without reading any header.  This is intended for
///   trusted internal/loopback paths only — never expose it on a public port.
pub struct BearerAuthenticator {
    keys: Vec<NamedKey>,
    oidc: Option<OidcVerifier>,
}

impl BearerAuthenticator {
    /// Build an authenticator from named keys and an optional OIDC verifier.
    #[must_use]
    pub fn new(keys: Vec<NamedKey>, oidc: Option<OidcVerifier>) -> Self {
        Self { keys, oidc }
    }

    /// Resolve the `Authorization: Bearer <token>` header to a [`CallerIdentity`].
    ///
    /// Returns `None` if the header is absent, the token does not match any
    /// registered key, or JWT validation fails.  The caller **must** return
    /// `401 Unauthorized` in that case.
    pub fn authenticate(&self, headers: &axum::http::HeaderMap) -> Option<CallerIdentity> {
        // Open-access (unauthenticated) mode — no keys and no OIDC.
        // Used only for internal/loopback paths; never expose on a public port.
        if self.keys.is_empty() && self.oidc.is_none() {
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
            && let Some(oidc) = &self.oidc
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
        for key in &self.keys {
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
}

// ── SchemaPolicySet ──────────────────────────────────────────────────────────

/// A Cedar policy set validated against a schema, ready for evaluation.
///
/// Constructed once at startup from an embedded default policy plus optional
/// operator-supplied extras (typically the concatenated content of `.cedar`
/// files from a `--cedar-policy-dir`).  Construction fails fast when:
///
/// - the schema does not parse,
/// - the combined policy text does not parse,
/// - the policies do not validate against the schema under
///   [`ValidationMode::Strict`] — catching operator typos (unknown action
///   names, wrong attribute types, …) before any request is served.
pub struct SchemaPolicySet {
    authorizer: Authorizer,
    policy_set: PolicySet,
    schema: Schema,
}

impl SchemaPolicySet {
    /// Parse `schema_src` (Cedar schema syntax) and the concatenation of
    /// `default_policies` + `extra_policies`, then strictly validate the
    /// policies against the schema.
    pub fn new(
        schema_src: &str,
        default_policies: &str,
        extra_policies: Option<String>,
    ) -> Result<Self, AuthzBuildError> {
        // Parse schema from Cedar schema syntax (human-readable, with warnings).
        let (schema, schema_warnings) = Schema::from_cedarschema_str(schema_src)
            .map_err(|e| AuthzBuildError::SchemaError(e.to_string()))?;
        for w in schema_warnings {
            tracing::warn!("cedar schema warning: {w}");
        }

        let mut combined = default_policies.to_owned();
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
            authorizer: Authorizer::new(),
            policy_set,
            schema,
        })
    }

    /// Evaluate an authorization request.
    ///
    /// `entities` are validated against the schema, and `context_json` is
    /// validated against the schema's declared context type for `action_uid`.
    /// Any construction or evaluation error is logged and treated as **deny**.
    pub fn eval(
        &self,
        principal_uid: EntityUid,
        action_uid: EntityUid,
        resource_uid: EntityUid,
        entities: Vec<Entity>,
        context_json: serde_json::Value,
    ) -> bool {
        let entities = match Entities::from_entities(entities, Some(&self.schema)) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    principal = %principal_uid,
                    action = %action_uid,
                    "cedar: failed to build entities: {e}"
                );
                return false;
            }
        };

        // Passing the action UID to Context::from_json_value validates the
        // context record against the schema's declared context type.
        let context =
            match Context::from_json_value(context_json, Some((&self.schema, &action_uid))) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        principal = %principal_uid,
                        action = %action_uid,
                        "cedar: failed to build context: {e}"
                    );
                    return false;
                }
            };

        let request = match Request::new(
            principal_uid.clone(),
            action_uid.clone(),
            resource_uid,
            context,
            Some(&self.schema),
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    principal = %principal_uid,
                    action = %action_uid,
                    "cedar: failed to build request: {e}"
                );
                return false;
            }
        };

        let response = self
            .authorizer
            .is_authorized(&request, &self.policy_set, &entities);

        for err in response.diagnostics().errors() {
            tracing::warn!(
                principal = %principal_uid,
                action = %action_uid,
                "cedar: evaluation error: {err}"
            );
        }

        let allowed = response.decision() == Decision::Allow;
        if !allowed {
            tracing::info!(
                principal = %principal_uid,
                action    = %action_uid,
                "cedar: request denied"
            );
        }
        allowed
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build an [`EntityUid`] from a fully-qualified type name and an ID string.
///
/// # Panics
///
/// Panics when `type_name` is not a valid Cedar entity type name — callers
/// pass compile-time constants, so this is a programmer error.
#[must_use]
pub fn entity_uid(type_name: &str, id: &str) -> EntityUid {
    let ty = EntityTypeName::from_str(type_name).expect("cedar: invalid entity type name");
    let eid = EntityId::from_str(id).expect("cedar: invalid entity id");
    EntityUid::from_type_name_and_id(ty, eid)
}

/// Build a Cedar `String` [`RestrictedExpression`] from a Rust `&str`.
#[must_use]
pub fn cedar_str(s: &str) -> RestrictedExpression {
    // Cedar string literals use `\"` and `\\` as escape sequences.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    RestrictedExpression::from_str(&format!("\"{escaped}\""))
        .expect("cedar: string RestrictedExpression")
}

/// Build a Cedar `Long` [`RestrictedExpression`] from an `i64`.
#[must_use]
pub fn cedar_long(n: i64) -> RestrictedExpression {
    RestrictedExpression::from_str(&n.to_string()).expect("cedar: long RestrictedExpression")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(token: &str) -> axum::http::HeaderMap {
        let mut m = axum::http::HeaderMap::new();
        m.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        m
    }

    fn single_key(name: &str, token: &str) -> BearerAuthenticator {
        BearerAuthenticator::new(
            vec![NamedKey {
                name: Arc::from(name),
                token: SecretString::new(token.to_owned().into()),
            }],
            None,
        )
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
        let auth = single_key("erp", "tok123");
        let id = auth
            .authenticate(&bearer("tok123"))
            .expect("must authenticate");
        assert_eq!(id.name.as_ref(), "erp");
    }

    #[test]
    fn authenticate_wrong_token_rejected() {
        let auth = single_key("erp", "tok123");
        assert!(auth.authenticate(&bearer("wrong")).is_none());
    }

    #[test]
    fn authenticate_missing_header_rejected() {
        let auth = single_key("erp", "tok123");
        assert!(auth.authenticate(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn authenticate_resolves_correct_principal_for_multiple_keys() {
        let auth = BearerAuthenticator::new(
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
        );
        assert_eq!(
            auth.authenticate(&bearer("tok-a")).unwrap().name.as_ref(),
            "erp-a"
        );
        assert_eq!(
            auth.authenticate(&bearer("tok-b")).unwrap().name.as_ref(),
            "erp-b"
        );
        assert!(auth.authenticate(&bearer("tok-c")).is_none());
    }

    #[test]
    fn no_keys_no_oidc_is_anonymous_mode() {
        let auth = BearerAuthenticator::new(vec![], None);
        // No header required — every call resolves to "anonymous".
        let id = auth.authenticate(&axum::http::HeaderMap::new()).unwrap();
        assert_eq!(id.name.as_ref(), "anonymous");
        // Even with a (bogus) bearer header the identity stays "anonymous".
        let id = auth.authenticate(&bearer("whatever")).unwrap();
        assert_eq!(id.name.as_ref(), "anonymous");
    }

    // ── SchemaPolicySet ───────────────────────────────────────────────────────

    const TEST_SCHEMA: &str = r#"
namespace Test {
  entity Principal;
  entity Doc = { "tenant": String };
  action "Read" appliesTo {
    principal: [Principal],
    resource: [Doc],
    context: { "tenant": String }
  };
}
"#;

    const TEST_DEFAULT_POLICY: &str = r#"
permit(principal, action == Test::Action::"Read", resource);
"#;

    fn doc_entity(id: &str, tenant: &str) -> Entity {
        Entity::new(
            entity_uid("Test::Doc", id),
            std::collections::HashMap::from([("tenant".to_owned(), cedar_str(tenant))]),
            std::collections::HashSet::new(),
        )
        .unwrap()
    }

    #[test]
    fn schema_policy_set_permits_and_forbids() {
        let extra = r#"
forbid(principal == Test::Principal::"blocked", action, resource);
"#;
        let set = SchemaPolicySet::new(TEST_SCHEMA, TEST_DEFAULT_POLICY, Some(extra.to_owned()))
            .expect("valid schema + policies");
        let ctx = serde_json::json!({ "tenant": "t1" });
        assert!(set.eval(
            entity_uid("Test::Principal", "ok"),
            entity_uid("Test::Action", "Read"),
            entity_uid("Test::Doc", "d1"),
            vec![
                Entity::with_uid(entity_uid("Test::Principal", "ok")),
                doc_entity("d1", "t1")
            ],
            ctx.clone(),
        ));
        assert!(!set.eval(
            entity_uid("Test::Principal", "blocked"),
            entity_uid("Test::Action", "Read"),
            entity_uid("Test::Doc", "d1"),
            vec![
                Entity::with_uid(entity_uid("Test::Principal", "blocked")),
                doc_entity("d1", "t1")
            ],
            ctx,
        ));
    }

    #[test]
    fn strict_validation_rejects_unknown_action() {
        // Operator typo: action "Reed" does not exist in the schema.
        let extra = r#"
forbid(principal, action == Test::Action::"Reed", resource);
"#;
        let Err(err) =
            SchemaPolicySet::new(TEST_SCHEMA, TEST_DEFAULT_POLICY, Some(extra.to_owned()))
        else {
            panic!("unknown action must fail strict validation");
        };
        assert!(matches!(err, AuthzBuildError::PolicyParse(_)));
    }

    #[test]
    fn invalid_schema_rejected() {
        let Err(err) = SchemaPolicySet::new("this is not a schema", TEST_DEFAULT_POLICY, None)
        else {
            panic!("garbage schema must fail");
        };
        assert!(matches!(err, AuthzBuildError::SchemaError(_)));
    }
}
