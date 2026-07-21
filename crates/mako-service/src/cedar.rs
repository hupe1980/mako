//! Cedar ABAC enforcement for mako services.
//!
//! Provides a [`CedarEnforcer`] that evaluates AWS Cedar policies loaded at
//! startup.  Inject via `Extension<Arc<CedarEnforcer>>` into Axum routers.
//!
//! ## Quick-start
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use mako_service::cedar::{CedarEnforcer, CedarPrincipal};
//!
//! // Load policy at startup — typically via `include_str!("../policies/marktd.cedar")`
//! let policy_str = r#"permit(principal, action == Action::"read-malo", resource)
//!     when { context.principal_tenant == context.resource_tenant };"#;
//!
//! let enforcer = Arc::new(
//!     CedarEnforcer::from_policy_str(policy_str)
//!         .expect("Cedar policy must be valid at startup"),
//! );
//!
//! // In a handler:
//! let principal = CedarPrincipal {
//!     sub: "user-sub-123".to_owned(),
//!     tenant: "9900357000004".to_owned(),
//!     roles: vec!["NB".to_owned()],
//! };
//! enforcer.check(&principal, "read-malo", "9900357000004").unwrap();
//! ```
//!
//! ## Policy pattern
//!
//! Policies are evaluated with three context fields:
//!
//! - `context.principal_tenant` — `mako_tenant` custom JWT claim
//! - `context.principal_roles` — `mako_roles` custom JWT claim (e.g. `["NB"]`)
//! - `context.resource_tenant` — owning tenant passed by the handler
//!
//! Example policy:
//! ```cedar
//! permit(principal, action == Action::"read-malo", resource) when {
//!     context.principal_tenant == context.resource_tenant
//! };
//! ```

use std::str::FromStr;
use std::sync::Arc;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityTypeName, EntityUid, PolicySet, Request,
};

// ── Errors ────────────────────────────────────────────────────────────────────

/// Errors produced by [`CedarEnforcer`].
#[derive(Debug, thiserror::Error)]
pub enum CedarError {
    /// Cedar policy text could not be parsed.
    #[error("Cedar policy parse error: {0}")]
    PolicyParse(String),

    /// A request could not be constructed (bad entity UID or context).
    #[error("Cedar request build error: {0}")]
    RequestBuild(String),

    /// The policy evaluated to `Deny`.
    #[error(
        "authorization denied: principal={principal} action={action} resource_tenant={resource_tenant}"
    )]
    Denied {
        principal: String,
        action: String,
        resource_tenant: String,
    },
}

// ── Principal ─────────────────────────────────────────────────────────────────

/// Minimal principal data derived from JWT claims for Cedar evaluation.
///
/// Build this from the handler's `Claims` extractor:
/// ```rust,ignore
/// let principal = claims.principal();
/// enforcer.check(&principal, "read-malo", &state.tenant_gln)?;
/// ```
#[derive(Debug, Clone)]
pub struct CedarPrincipal {
    /// `sub` claim — unique user identifier, used as Cedar entity ID.
    pub sub: String,
    /// `mako_tenant` custom JWT claim — data isolation boundary (typically the
    /// operator's MP-ID).
    pub tenant: String,
    /// `mako_roles` custom JWT claim — energy-market roles (e.g. `["NB", "LF"]`).
    pub roles: Vec<String>,
}

// ── CedarEnforcer ─────────────────────────────────────────────────────────────

/// Loaded Cedar policy set ready for authorization evaluation.
///
/// Cheap to clone; share via `Arc<CedarEnforcer>`.
#[derive(Clone)]
pub struct CedarEnforcer {
    policy_set: Arc<PolicySet>,
}

impl CedarEnforcer {
    /// Parse Cedar policies from a string literal.
    ///
    /// Call this once at service startup (e.g. via `include_str!`).
    /// Fails fast with a descriptive error if the policy text is malformed.
    pub fn from_policy_str(policies: &str) -> Result<Self, CedarError> {
        let policy_set =
            PolicySet::from_str(policies).map_err(|e| CedarError::PolicyParse(e.to_string()))?;
        Ok(Self {
            policy_set: Arc::new(policy_set),
        })
    }

    /// Evaluate whether `principal` may perform `action` on the given resource tenant.
    ///
    /// # Arguments
    ///
    /// - `principal` — caller identity extracted from JWT.
    /// - `action`    — coarse capability string, e.g. `"read-malo"`.
    /// - `resource_tenant` — owning tenant of the target entity.  Pass
    ///   `principal.tenant` for list-style endpoints where there is no single
    ///   resource (the policy can then treat list as a same-tenant operation).
    ///
    /// # Errors
    ///
    /// Returns [`CedarError::Denied`] when the policy evaluates to `Deny`.
    /// Returns [`CedarError::RequestBuild`] if entity UIDs cannot be
    /// constructed (should never happen with well-formed inputs).
    pub fn check(
        &self,
        principal: &CedarPrincipal,
        action: &str,
        resource_tenant: &str,
    ) -> Result<(), CedarError> {
        // Escape double-quotes inside IDs so Cedar does not confuse them with
        // string delimiters.  In practice sub/tenant MP-IDs never contain `"`.
        let p_uid = build_uid("User", &principal.sub)?;
        let a_uid = build_uid("Action", action)?;
        let r_uid = build_uid("Resource", resource_tenant)?;

        let ctx_json = serde_json::json!({
            "principal_tenant": principal.tenant,
            "principal_roles": principal.roles,
            "resource_tenant": resource_tenant,
        });
        let ctx = Context::from_json_value(ctx_json, None)
            .map_err(|e| CedarError::RequestBuild(e.to_string()))?;

        let request = Request::new(p_uid, a_uid, r_uid, ctx, None)
            .map_err(|e| CedarError::RequestBuild(e.to_string()))?;

        let response =
            Authorizer::new().is_authorized(&request, &self.policy_set, &Entities::empty());

        match response.decision() {
            Decision::Allow => Ok(()),
            Decision::Deny => Err(CedarError::Denied {
                principal: principal.sub.clone(),
                action: action.to_owned(),
                resource_tenant: resource_tenant.to_owned(),
            }),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_uid(type_name: &str, id: &str) -> Result<EntityUid, CedarError> {
    // Cedar canonical UID syntax: Type::"id"
    // Escape any embedded double-quotes in `id` with `\"`
    let escaped = id.replace('\\', r"\\").replace('"', r#"\""#);
    let raw = format!(r#"{type_name}::"{escaped}""#);
    EntityUid::from_str(&raw)
        .or_else(|_| {
            // Fallback: construct from parts if parsing fails (e.g. unusual chars)
            let etype = EntityTypeName::from_str(type_name)
                .map_err(|e| CedarError::RequestBuild(e.to_string()))?;
            EntityUid::from_str(&format!(r#"{etype}::"{escaped}""#))
                .map_err(|e| CedarError::RequestBuild(e.to_string()))
        })
        .map_err(|_: CedarError| {
            CedarError::RequestBuild(format!("cannot build EntityUid for {type_name}::{id:?}"))
        })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: &str = r#"
        // Same-tenant read — any role
        permit(
            principal,
            action in [
                Action::"read-malo",
                Action::"read-melo",
                Action::"read-contract",
                Action::"read-partner",
                Action::"read-preisblatt"
            ],
            resource
        ) when {
            context.principal_tenant == context.resource_tenant
        };

        // Same-tenant write — any role
        permit(
            principal,
            action in [
                Action::"write-malo",
                Action::"write-melo",
                Action::"write-contract",
                Action::"write-partner"
            ],
            resource
        ) when {
            context.principal_tenant == context.resource_tenant
        };

        // Price-sheet write — NB role only
        permit(
            principal,
            action == Action::"write-preisblatt",
            resource
        ) when {
            context.principal_tenant == context.resource_tenant &&
            context.principal_roles.contains("NB")
        };
    "#;

    fn enforcer() -> CedarEnforcer {
        CedarEnforcer::from_policy_str(POLICY).expect("valid test policy")
    }

    fn principal(tenant: &str, roles: &[&str]) -> CedarPrincipal {
        CedarPrincipal {
            sub: "user-alice".to_owned(),
            tenant: tenant.to_owned(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[test]
    fn same_tenant_read_is_allowed() {
        let e = enforcer();
        let p = principal("9900357000004", &["LF"]);
        assert!(e.check(&p, "read-malo", "9900357000004").is_ok());
        assert!(e.check(&p, "read-melo", "9900357000004").is_ok());
        assert!(e.check(&p, "read-contract", "9900357000004").is_ok());
        assert!(e.check(&p, "read-partner", "9900357000004").is_ok());
        assert!(e.check(&p, "read-preisblatt", "9900357000004").is_ok());
    }

    #[test]
    fn cross_tenant_read_is_denied() {
        let e = enforcer();
        let p = principal("9900357000004", &["LF"]);
        assert!(e.check(&p, "read-malo", "9900357000099").is_err());
    }

    #[test]
    fn same_tenant_write_is_allowed() {
        let e = enforcer();
        let p = principal("9900357000004", &["LF"]);
        assert!(e.check(&p, "write-malo", "9900357000004").is_ok());
        assert!(e.check(&p, "write-melo", "9900357000004").is_ok());
    }

    #[test]
    fn preisblatt_write_requires_nb_role() {
        let e = enforcer();
        let nb = principal("9900357000004", &["NB"]);
        let lf = principal("9900357000004", &["LF"]);
        assert!(e.check(&nb, "write-preisblatt", "9900357000004").is_ok());
        assert!(e.check(&lf, "write-preisblatt", "9900357000004").is_err());
    }

    #[test]
    fn preisblatt_read_is_open_to_any_role() {
        let e = enforcer();
        let lf = principal("9900357000004", &["LF"]);
        assert!(e.check(&lf, "read-preisblatt", "9900357000004").is_ok());
    }

    #[test]
    fn cross_tenant_write_is_denied() {
        let e = enforcer();
        let p = principal("9900357000004", &["NB"]);
        assert!(e.check(&p, "write-malo", "9900357000099").is_err());
        assert!(e.check(&p, "write-preisblatt", "9900357000099").is_err());
    }

    #[test]
    fn malformed_policy_is_rejected() {
        let result = CedarEnforcer::from_policy_str("this is not valid cedar policy !!!!");
        assert!(result.is_err());
    }
}
