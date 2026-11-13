//! The MCP round trips every mako server is held to, against makod's `/mcp`:
//! the declared revisions on `server/discover`, requests on 2026-07-28 and on
//! 2025-11-25 without a session, `Mcp-Name` read, and a revision outside the
//! declared set refused (`mako_service::mcp::conformance::round_trips`).
//!
//! makod also serves resources, so it carries the one revision-dependent error
//! code: a resource that is not there answers `-32602` on 2026-07-28 and
//! `-32002` on 2025-11-25.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use mako_service::mcp::conformance::{Endpoint, round_trips};
use makod::commands_api::CommandsApiState;
use makod::mcp_server::{self, MakodMcpState};
use serde_json::json;

const TENANT: &str = "9900357000004";

async fn app() -> axum::Router {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let cedar =
        Arc::new(makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"));
    let malo_cache = Arc::new(makod::malo_cache::SlateDbMaloCache::new(
        (*store).clone(),
        TenantId::from_party_id(TENANT),
    ));
    let commands = Arc::new(CommandsApiState {
        tenant_id: TenantId::from_party_id(TENANT),
        sender_party_id: TENANT.to_owned(),
        configured_marktrollen: vec!["LF".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::clone(&cedar),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::clone(&malo_cache),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store: Arc::clone(&store),
        marktd_client: None,
    });
    let state = Arc::new(MakodMcpState {
        tenant: TENANT.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        cedar: Arc::clone(&cedar),
        commands,
        malo_cache,
        partner_store: Arc::new(store.as_partner_store()),
        process_store: Arc::clone(&store),
        deadline_store: store.as_deadline_store(),
        policy: Arc::new(mako_service::policy::PolicyReport::default()),
        einrichtung: Arc::new(makod::einrichtung_api::EinrichtungState {
            einrichtung: Arc::new(mako_service::einrichtung::Einrichtung {
                dienst: "makod".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                bereit: true,
                befunde: Vec::new(),
                policy: mako_service::policy::PolicyReport::default(),
                parteien: Vec::new(),
            }),
            dienste: Vec::new(),
            http: mako_service::http::default_client(),
            cedar: Arc::clone(&cedar),
            tenant: TENANT.to_owned(),
            zertifikate: None,
        }),
    });
    mcp_server::router(state, tokio_util::sync::CancellationToken::new())
}

#[tokio::test]
async fn the_mcp_server_passes_the_round_trips() {
    round_trips(app().await, "/mcp", "get_policy").await;
}

#[tokio::test]
async fn a_missing_resource_answers_the_code_of_the_revision() {
    let mcp = Endpoint::serve(app().await, "/mcp").await;
    let uri = "malo://51238696012";
    let new = mcp
        .request_2026("resources/read", Some(uri), json!({ "uri": uri }))
        .await;
    assert_eq!(new.error_code(), Some(-32602), "{new:?}");
    let old = mcp
        .request_2025("resources/read", json!({ "uri": uri }))
        .await;
    assert_eq!(old.error_code(), Some(-32002), "{old:?}");
}
