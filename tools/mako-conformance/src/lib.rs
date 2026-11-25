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
mod profile;
pub mod report;
pub mod rest;
pub mod suite;
pub mod suites;
mod transport;

/// The documents every service commits, embedded at build time.
pub mod documents {
    include!(concat!(env!("OUT_DIR"), "/documents.rs"));

    /// Only the schemas published by the selected service are reference evidence.
    pub fn event_schemas(service: &str) -> impl Iterator<Item = (&'static str, &'static str)> {
        EVENT_SCHEMAS
            .iter()
            .filter_map(move |&(owner, ty, schema)| (owner == service).then_some((ty, schema)))
    }
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
    let snapshot = find(documents::MCP_SNAPSHOTS)
        .ok_or_else(|| anyhow::anyhow!("missing MCP reference snapshot"))?;
    let snapshot_tools: Vec<serde_json::Value> = serde_json::from_str(snapshot)?;
    let mcp_tools = snapshot_tools
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("invalid MCP reference tool"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let document = document::Document::parse(openapi)?;
    let schemas: Vec<_> = documents::event_schemas(suite.service()).collect();
    let profile = profile::Profile::new(
        suite, only, &document, openapi, snapshot, &mcp_tools, &schemas,
    )?;
    let event_schemas = schemas
        .into_iter()
        .map(|(ty, text)| {
            let schema: serde_json::Value = serde_json::from_str(text)?;
            let validator = jsonschema::draft202012::new(&schema)
                .map_err(|_| anyhow::anyhow!("invalid reference event schema"))?;
            Ok((ty, validator))
        })
        .collect::<anyhow::Result<_>>()?;
    let http = transport::client(&target.base)?;
    let ctx = Ctx {
        target,
        document,
        mcp_tools,
        event_schemas,
        http,
        report: Report::new(profile),
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
