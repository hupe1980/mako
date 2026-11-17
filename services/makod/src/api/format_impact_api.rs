//! `GET /api/v1/format-candidates/{fv}/impact` on the REST port: the
//! [`crate::format_impact`] report behind the Cedar action `ReadFormatImpact`.
//!
//! The handler and its document entry are `format_impact::impact_route`; this
//! module mounts it through `OpenApiRouter` and gates it with a route layer,
//! so the route is documented whether or not `[format_impact]` is configured.
//! Unconfigured, an authorized caller is answered `503
//! format_impact_not_configured` — the report needs the candidates tree and
//! the interchange archive, and there is no empty report to give instead.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse as _, Response};
use mako_service::ApiError;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::core::cedar_authz::CedarAuthorizer;
use crate::format_impact::{FormatImpactConfig, FormatImpactState};

/// What the gate and the route need.
pub struct FormatImpactApiState {
    /// `[format_impact]` and the operator's own MP-IDs; `None` unconfigured.
    pub impact: Option<Arc<FormatImpactState>>,
    /// Cedar authorizer.
    pub cedar: Arc<CedarAuthorizer>,
    /// Operator tenant (MP-ID) — the Cedar resource scope.
    pub tenant: String,
}

/// The documented route, gated.
pub fn routes<S: Clone + Send + Sync + 'static>(
    state: Arc<FormatImpactApiState>,
) -> OpenApiRouter<S> {
    // Unconfigured, the gate refuses before the handler runs, so the handler
    // never reads this placeholder.
    let impact = state.impact.clone().unwrap_or_else(|| {
        Arc::new(FormatImpactState {
            config: FormatImpactConfig {
                candidates_dir: PathBuf::new(),
                archive_dir: PathBuf::new(),
                window_days: 0,
            },
            own_mp_ids: BTreeSet::new(),
        })
    });
    OpenApiRouter::new()
        .routes(routes!(crate::format_impact::impact_route))
        .route_layer(middleware::from_fn_with_state(state, gate))
        .with_state(impact)
}

async fn gate(
    State(state): State<Arc<FormatImpactApiState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(identity) = state.cedar.authenticate(request.headers()) else {
        return ApiError::Unauthorized.into_response();
    };
    if !state
        .cedar
        .authorize_format_impact(&identity, &state.tenant)
    {
        return ApiError::forbidden("ReadFormatImpact permission denied").into_response();
    }
    if state.impact.is_none() {
        return ApiError::unavailable(
            "no [format_impact] is configured: the report reads the candidates tree and the \
             interchange archive it names",
        )
        .with_code("format_impact_not_configured")
        .into_response();
    }
    next.run(request).await
}
