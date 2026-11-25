//! The REST runner: refusals, statuses and bodies against the committed
//! OpenAPI document.

use crate::document::api_error;
use crate::profile::Probe;
use crate::report::Reason;
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
        .filter(|op| ctx.report.required(&Probe::RestAuth(op.name())))
    {
        let name = Probe::RestAuth(op.name());
        let path = concrete(&op.path);
        let body = (op.method != "GET" && op.method != "DELETE").then(|| serde_json::json!({}));
        match ctx.call(None, &op.method, &path, body.as_ref()).await {
            Ok(a) if a.status == 401 => match api_error(&a.body) {
                Ok(()) => ctx.report.held(name),
                Err(gap) => ctx.report.broken(name, gap),
            },
            Ok(a) if matches!(a.status, 400 | 404 | 405 | 415 | 422) => {
                ctx.report.skipped(name, Reason::AuthNotReached)
            }
            Ok(a) => ctx.report.broken(
                name,
                format!(
                    "answered {} without a token, not 401: the operation is not protected as \
                     documented",
                    a.status
                ),
            ),
            Err(e) => ctx.report.broken(name, format!("unreachable: {e}")),
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
        .filter(|op| ctx.report.required(&Probe::RestStatus(op.name())))
    {
        let status = Probe::RestStatus(op.name());
        let body = Probe::RestBody(op.name());
        let required_body = ctx.report.required(&body);
        let answer = match ctx.operator("GET", &op.path, None).await {
            Ok(answer) => answer,
            Err(error) => {
                ctx.report.broken(status, error.to_string());
                continue;
            }
        };
        let Some(response) = op.response(answer.status) else {
            ctx.report
                .broken(status, format!("undocumented status {}", answer.status));
            continue;
        };
        if answer.status >= 400 {
            match api_error(&answer.body) {
                Ok(()) => ctx.report.held(status),
                Err(gap) => ctx.report.broken(status, gap),
            }
            if required_body {
                ctx.report.skipped(body, Reason::UnobservedBody);
            }
            continue;
        }
        ctx.report.held(status);
        if required_body {
            match ctx.document.violations(response, &answer.body) {
                None => ctx.report.skipped(body, Reason::UnobservedBody),
                Some(_) if !answer.json_body => ctx.report.broken(body, "expected a JSON response"),
                Some(violations) if violations.is_empty() => ctx.report.held(body),
                Some(violations) => ctx.report.broken(body, violations.join("; ")),
            }
        }
    }
}

/// The refused principal gets `403` on the suite's refusal route.
async fn refused(ctx: &Ctx, suite: &dyn Suite) {
    let refusal = suite.refusal();
    let name = Probe::RestRefusal(refusal.path.into());
    let Some(token) = ctx.target.refused_token.as_deref() else {
        ctx.report.skipped(name, Reason::MissingRefusedCredential);
        return;
    };
    match ctx.call(Some(token), "GET", refusal.path, None).await {
        Ok(a) if a.status == 403 => match api_error(&a.body) {
            Ok(()) => ctx.report.held(name),
            Err(gap) => ctx.report.broken(name, gap),
        },
        Ok(a) => ctx.report.broken(
            name,
            format!(
                "answered {} — a principal the policy refuses was admitted",
                a.status
            ),
        ),
        Err(e) => ctx.report.broken(name, format!("unreachable: {e}")),
    }
}

/// `path` with every parameter replaced by an id no service holds.
fn concrete(path: &str) -> String {
    path.split('/')
        .map(|s| if s.starts_with('{') { UNKNOWN_ID } else { s })
        .collect::<Vec<_>>()
        .join("/")
}
