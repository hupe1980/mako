//! `agentd` — the multi-agent plane for mako.
//!
//! 28 declarative specialists run on the [agentplane] durable runtime. What
//! agentd owns is the bridge from mako to that runtime: routing a CloudEvent to
//! the specialists that subscribe to it, labelling the payload at the trust
//! boundary, wiring the stores, the policy set, the key ring and the calendar,
//! and mounting the surface a human approves things on.
//!
//! ```text
//! CloudEvent  →  Router          one run per subscribing specialist
//!             →  plane::label    re-validated identifiers trusted, the rest not
//!             →  admission key   (source, id, specialist) — at most once, in the store
//!             →  202 Accepted    returned here: admission has committed
//!             →  Runtime         journaled effects, strict replay, Cedar gate
//!                  ├─ tools      MCP calls to mako's own services
//!                  ├─ tasks      a mutating call waits for a named approver
//!                  └─ deadlines  Werktage, from mako's BDEW holiday table
//!             →  de.agent.decision.made → marktd audit log
//! ```
//!
//! | Route | What it is for |
//! |---|---|
//! | `POST /webhook` | CloudEvent ingest (Standard Webhooks-verified) |
//! | `POST /api/v1/run` | Run a specialist by hand (OIDC + `api:run.start`; honours `Idempotency-Key`) |
//! | `POST /api/v1/erasure` | Destroy a case key and/or forget a memory subject (OIDC + `api:erasure.execute`) |
//! | `GET /api/v1/agents` | What this deployment activated (OIDC + `api:agent.list`) |
//! | `GET /api/v1/agents/catalog` | Everything compiled in (OIDC + `api:agent.list`) |
//! | `GET /.well-known/agents/{name}` | A2A Agent Card, derived from the manifest |
//! | `/api/v1/oversight/*` | agentplane's operator surface: worklist, runs, cases, breached obligations |
//!
//! Port: 9580
//!
//! The daemon is [`agentd::Agentd`]; `mako_service::run` owns its lifecycle.
//!
//! [agentplane]: https://hupe1980.github.io/agentplane/

#![deny(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<agentd::Agentd>().await
}
