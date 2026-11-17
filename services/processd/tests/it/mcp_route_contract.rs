//! The MCP tools' loopback URLs against the router's own registrations.
//!
//! `approve_queue_entry` and `reject_queue_entry` do not call a Rust function —
//! they issue an HTTP request back into this service's own router, so that the
//! REST handler's Cedar check runs rather than being bypassed. Nothing in the
//! type system connects a `format!`-built path to a route's path, so a
//! tool whose method or path differs from the router's answers every call with
//! 404 and the operator override it exists for never works.
//!
//! Pinning the tools' path with a second string literal would be no guard at
//! all — the wrong path written twice still agrees with itself. So the
//! method and path live in one `QueueRoute` constant the tools build their
//! request from, and this suite checks the two ends against each other:
//!
//! - the constants produce the paths and method they are supposed to
//!   (`the_queue_routes_are_the_documented_endpoints`), and
//! - the `#[utoipa::path]` the router mounts each handler under names the
//!   constant's path and method (`the_router_registers_the_shared_constants`,
//!   `no_source_file_spells_a_queue_action_path_by_hand`; the served document
//!   is held to every constant by `openapi::tests`).

use std::path::{Path, PathBuf};

use processd::server::{QUEUE_APPROVE, QUEUE_REJECT, QueueMethod};

fn src(file: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

/// The `#[utoipa::path(<method>, path = "<path>", …)]` a handler is mounted
/// under, as the router source spells it.
///
/// Source text, deliberately: the attribute is what `routes!` mounts, so the
/// guard has to read what is actually registered rather than ask the same
/// constant twice.
fn registration(handler: &str) -> (String, String) {
    let server = src("server.rs");
    assert!(
        src("openapi.rs").contains(&format!("routes!(rest::{handler})")),
        "no route mounts rest::{handler}"
    );
    let idx = server
        .find(&format!("pub async fn {handler}("))
        .unwrap_or_else(|| panic!("rest::{handler} is not defined"));
    let attr_at = server[..idx]
        .rfind("#[utoipa::path(")
        .unwrap_or_else(|| panic!("rest::{handler} carries no #[utoipa::path]"));
    let attr = &server[attr_at + "#[utoipa::path(".len()..idx];
    let method = attr.split(',').next().expect("a method").trim().to_owned();
    let path = attr
        .split("path = \"")
        .nth(1)
        .and_then(|p| p.split('"').next())
        .unwrap_or_else(|| panic!("rest::{handler}'s attribute names no path"))
        .to_owned();
    (path, method)
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
    for (handler, route) in [
        ("approve_queue_entry", QUEUE_APPROVE),
        ("reject_queue_entry", QUEUE_REJECT),
    ] {
        let (path, method) = registration(handler);
        assert_eq!(
            path, route.path_template,
            "the router must mount rest::{handler} under the path the MCP tool \
             builds its loopback URL from"
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
