//! The REST runner: refusals, statuses and bodies against the committed
//! OpenAPI document.

use crate::document::{api_error, truncate};
use crate::report::Runner;
use crate::suite::{Ctx, Suite};

/// A path parameter no service holds a record for.
const UNKNOWN_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Run every REST check.
pub async fn run(ctx: &Ctx, suite: &dyn Suite) {
    unauthorised(ctx).await;
    reads(ctx).await;
    refused(ctx, suite).await;
}

/// Every protected operation, called without a token, answers `401` as an
/// `ApiError`. One that reads its body or path before the token (`400`,
/// `404`, `405`, `415`, `422`) is not checked — a skip, not a pass.
async fn unauthorised(ctx: &Ctx) {
    for op in ctx
        .document
        .operations()
        .into_iter()
        .filter(|o| o.protected())
    {
        let name = format!("401 without a token: {}", op.name());
        let path = concrete(&op.path);
        let body = (op.method != "GET" && op.method != "DELETE").then(|| serde_json::json!({}));
        match ctx.call(None, &op.method, &path, body.as_ref()).await {
            Ok(a) if a.status == 401 => match api_error(&a.body) {
                Ok(()) => ctx.report.held(Runner::Rest, name),
                Err(gap) => ctx.report.broken(Runner::Rest, name, gap),
            },
            Ok(a) if matches!(a.status, 400 | 404 | 405 | 415 | 422) => ctx.report.skipped(
                Runner::Rest,
                name,
                format!("answered {} before the token was read", a.status),
            ),
            Ok(a) => ctx.report.broken(
                Runner::Rest,
                name,
                format!(
                    "answered {} without a token, not 401: the operation is not protected as \
                     documented",
                    a.status
                ),
            ),
            Err(e) => ctx
                .report
                .broken(Runner::Rest, name, format!("unreachable: {e}")),
        }
    }
}

/// Every `GET` without a path parameter or a required query parameter,
/// called with the operator token, answers a documented status with a body
/// valid against the document.
async fn reads(ctx: &Ctx) {
    for op in ctx
        .document
        .operations()
        .into_iter()
        .filter(|o| o.method == "GET" && !o.has_path_params() && !o.requires_query())
    {
        let name = format!("documented answer: {}", op.name());
        let a = match ctx.operator("GET", &op.path, None).await {
            Ok(a) => a,
            Err(e) => {
                ctx.report
                    .broken(Runner::Rest, name, format!("unreachable: {e}"));
                continue;
            }
        };
        let Some(response) = op.response(a.status) else {
            ctx.report.broken(
                Runner::Rest,
                name,
                format!(
                    "answered {}, a status the document does not declare: {}",
                    a.status,
                    truncate(&a.body.to_string())
                ),
            );
            continue;
        };
        if a.status >= 400 {
            match api_error(&a.body) {
                Ok(()) => ctx.report.held(Runner::Rest, name),
                Err(gap) => ctx.report.broken(Runner::Rest, name, gap),
            }
            continue;
        }
        match ctx.document.violations(response, &a.body) {
            None => ctx
                .report
                .skipped(Runner::Rest, name, "the answer declares no JSON body"),
            Some(v) if v.is_empty() => ctx.report.held(Runner::Rest, name),
            Some(v) => ctx.report.broken(
                Runner::Rest,
                name,
                format!(
                    "the body is not what the document declares: {}",
                    v.join("; ")
                ),
            ),
        }
    }
}

/// The refused principal gets `403` on the suite's refusal route.
async fn refused(ctx: &Ctx, suite: &dyn Suite) {
    let refusal = suite.refusal();
    let name = format!("403 for the refused principal: GET {}", refusal.path);
    let Some(token) = ctx.target.refused_token.as_deref() else {
        ctx.report
            .skipped(Runner::Rest, name, "no --refused-token given");
        return;
    };
    match ctx.call(Some(token), "GET", refusal.path, None).await {
        Ok(a) if a.status == 403 => match api_error(&a.body) {
            Ok(()) => ctx.report.held(Runner::Rest, name),
            Err(gap) => ctx.report.broken(Runner::Rest, name, gap),
        },
        Ok(a) => ctx.report.broken(
            Runner::Rest,
            name,
            format!(
                "answered {} — a principal the policy refuses was admitted",
                a.status
            ),
        ),
        Err(e) => ctx
            .report
            .broken(Runner::Rest, name, format!("unreachable: {e}")),
    }
}

/// `path` with every parameter replaced by an id no service holds.
fn concrete(path: &str) -> String {
    path.split('/')
        .map(|s| if s.starts_with('{') { UNKNOWN_ID } else { s })
        .collect::<Vec<_>>()
        .join("/")
}
