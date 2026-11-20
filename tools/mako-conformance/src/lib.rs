//! The conformance kit: a service — mako's own or a replacement — checked
//! against the contract its reference service publishes. Three runners read
//! the committed documents: `rest` the OpenAPI document, `events` the event
//! schemas, `mcp` the tool snapshot. One suite per service declares what the
//! documents do not: the types it emits, where it receives events, a refusal
//! pair and the calls that make it emit.
//!
//! Run it against a test deployment, never production: the seeds create
//! records and the events runner registers (and removes) a webhook endpoint.

pub mod document;
pub mod events;
pub mod mcp;
pub mod report;
pub mod rest;
pub mod suite;
pub mod suites;

/// The documents every service commits, embedded at build time.
pub mod documents {
    include!(concat!(env!("OUT_DIR"), "/documents.rs"));
}

use report::{Report, Runner};
use suite::{Ctx, Target};

/// Run `suite` against `target`; `only` limits it to one runner.
///
/// # Errors
///
/// When the service has no committed document to check against.
pub async fn run(
    suite: &dyn suite::Suite,
    target: Target,
    only: Option<Runner>,
) -> anyhow::Result<Report> {
    let find = |list: &[(&str, &'static str)]| {
        list.iter()
            .find(|(n, _)| *n == suite.service())
            .map(|(_, d)| *d)
    };
    let openapi = find(documents::OPENAPI)
        .ok_or_else(|| anyhow::anyhow!("{} commits no openapi.json", suite.service()))?;
    let mcp_tools = find(documents::MCP_SNAPSHOTS)
        .map(|s| {
            serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let ctx = Ctx {
        target,
        document: document::Document::parse(openapi)?,
        mcp_tools,
        http: reqwest::Client::new(),
        report: Report::default(),
    };
    let runs = |r: Runner| only.is_none_or(|o| o == r);
    if runs(Runner::Rest) {
        rest::run(&ctx, suite).await;
    }
    if runs(Runner::Events) {
        events::run(&ctx, suite).await;
    }
    if runs(Runner::Mcp) {
        mcp::run(&ctx, suite).await;
    }
    Ok(ctx.report)
}
