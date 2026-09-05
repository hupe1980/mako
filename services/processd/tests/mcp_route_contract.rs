//! The MCP tools' loopback URLs against the router's own registrations.
//!
//! `approve_queue_entry` and `reject_queue_entry` do not call a Rust function —
//! they issue an HTTP request back into this service's own router, so that the
//! REST handler's Cedar check runs rather than being bypassed. Nothing in the
//! type system connects a `format!`-built path to a `.route()` string, and the
//! two drifted: the tools built `PUT /api/v1/approval-queue/{id}/…` while the
//! router registers `POST /api/v1/queue/{id}/…`. Both the method and the path
//! were wrong, so every call to either tool returned 404 and the operator
//! override they exist for had never once worked.
//!
//! Pinning the tools' path with a second string literal would have been no
//! guard at all — the wrong path written twice still agrees with itself. So the
//! method and path now live in one `QueueRoute` constant that the router
//! registers and the tools build their request from, and this suite checks the
//! two ends against each other:
//!
//! - the constants produce the paths and method they are supposed to
//!   (`the_queue_routes_are_the_documented_endpoints`), and
//! - the router really registers *those constants* rather than literals of its
//!   own that could drift again (`the_router_registers_the_shared_constants`,
//!   `no_source_file_spells_a_queue_action_path_by_hand`).

use std::path::{Path, PathBuf};

use processd::server::{QUEUE_APPROVE, QUEUE_REJECT, QueueMethod};

fn src(file: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

/// The `.route(<path expr>, <method>(rest::<handler>))` registration for one
/// handler, as the router source spells it.
///
/// Source text, deliberately: the router is the authority on what is reachable,
/// so the guard has to read what it actually registers rather than ask the same
/// constant twice.
fn registration(handler: &str) -> (String, String) {
    let server = src("server.rs");
    let needle = format!("rest::{handler})");
    let idx = server
        .find(&needle)
        .unwrap_or_else(|| panic!("no route registers rest::{handler}"));
    let head = &server[..idx];
    let route_at = head
        .rfind(".route(")
        .unwrap_or_else(|| panic!("rest::{handler} appears outside a .route(…) call"));
    let call = &server[route_at + ".route(".len()..idx];
    let (path_expr, method) = call
        .rsplit_once(',')
        .unwrap_or_else(|| panic!("malformed .route(…) call for rest::{handler}"));
    (
        path_expr.trim().trim_end_matches(',').to_owned(),
        method.trim().trim_end_matches('(').to_owned(),
    )
}

#[test]
fn the_queue_routes_are_the_documented_endpoints() {
    let id: uuid::Uuid = "6f1d5a2c-0c3a-4f1e-9a44-1b0f7d8e5c21"
        .parse()
        .expect("uuid");
    assert_eq!(
        QUEUE_APPROVE.path_for(id),
        "/api/v1/queue/6f1d5a2c-0c3a-4f1e-9a44-1b0f7d8e5c21/approve"
    );
    assert_eq!(
        QUEUE_REJECT.path_for(id),
        "/api/v1/queue/6f1d5a2c-0c3a-4f1e-9a44-1b0f7d8e5c21/reject"
    );
    // Both are state-changing and non-idempotent (each dispatches a market
    // message to makod), which is what makes them POST and not PUT.
    assert_eq!(QUEUE_APPROVE.method, QueueMethod::Post);
    assert_eq!(QUEUE_REJECT.method, QueueMethod::Post);
}

#[test]
fn the_router_registers_the_shared_constants() {
    for (handler, konst, route) in [
        ("approve_queue_entry", "QUEUE_APPROVE", QUEUE_APPROVE),
        ("reject_queue_entry", "QUEUE_REJECT", QUEUE_REJECT),
    ] {
        let (path_expr, method) = registration(handler);
        assert_eq!(
            path_expr,
            format!("{konst}.path_template"),
            "the router must register the same constant the MCP tool builds its \
             loopback URL from — a literal here can drift from the tool again"
        );
        assert_eq!(
            method,
            route.method.as_str(),
            "the router registers rest::{handler} under `{method}` but the MCP \
             tool issues `{}` — the tool would get a 405",
            route.method.as_str()
        );
    }
}

#[test]
fn no_source_file_spells_a_queue_action_path_by_hand() {
    // `/api/v1/approval-queue/…` is the path that never existed. It is still
    // fine as prose *about* the queue ("approval-queue entries"), so only the
    // routed form is barred.
    for file in ["mcp_server.rs", "server.rs", "config.rs", "handler.rs"] {
        let text = src(file);
        for (n, line) in text.lines().enumerate() {
            assert!(
                !line.contains("/api/v1/approval-queue"),
                "src/{file}:{} names /api/v1/approval-queue, which no route \
                 serves; the endpoint is POST /api/v1/queue/{{id}}/approve",
                n + 1
            );
        }
    }
}
