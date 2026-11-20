//! The events runner: the service delivers its events signed, as valid
//! CloudEvents whose `data` meets the published schema, and refuses an
//! unsigned inbound one.
//!
//! The kit registers its own endpoint through the service's subscription API
//! (`POST /api/v1/webhooks/endpoints`), so the service under test needs no
//! configuration for it; it asks for a test delivery, runs the suite's seeds,
//! replays a window and judges every delivery.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::document::{truncate, validate};
use crate::report::Runner;
use crate::suite::{Ctx, Suite};

/// One delivery as it arrived.
struct Delivery {
    headers: HeaderMap,
    body: Bytes,
}

async fn receive(
    State(tx): State<Arc<mpsc::UnboundedSender<Delivery>>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let _ = tx.send(Delivery { headers, body });
    StatusCode::NO_CONTENT
}

/// Run every events check.
pub async fn run(ctx: &Ctx, suite: &dyn Suite) {
    outbound(ctx, suite).await;
    inbound(ctx, suite).await;
}

async fn outbound(ctx: &Ctx, suite: &dyn Suite) {
    let listener = match tokio::net::TcpListener::bind(&ctx.target.listen).await {
        Ok(l) => l,
        Err(e) => {
            ctx.report.broken(
                Runner::Events,
                "listen for deliveries",
                format!(
                    "cannot listen on {}: {e} — --listen picks another",
                    ctx.target.listen
                ),
            );
            return;
        }
    };
    let callback = ctx
        .target
        .callback
        .clone()
        .unwrap_or_else(|| format!("http://{}/", ctx.target.listen));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let app = axum::Router::new()
        .fallback(axum::routing::post(receive))
        .with_state(Arc::new(tx));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let name = format!(
        "conformance-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let created = ctx
        .operator(
            "POST",
            "/api/v1/webhooks/endpoints",
            Some(&json!({ "name": name, "url": callback, "event_types": [] })),
        )
        .await;
    let (id, secret) = match created {
        Ok(a) if a.status == 201 || a.status == 200 => {
            match (a.body["endpoint"]["id"].as_str(), a.body["secret"].as_str()) {
                (Some(id), Some(secret)) => (id.to_owned(), secret.to_owned()),
                _ => {
                    ctx.report.broken(
                        Runner::Events,
                        "register an endpoint",
                        format!(
                            "the answer names no endpoint id or secret: {}",
                            truncate(&a.body.to_string())
                        ),
                    );
                    server.abort();
                    return;
                }
            }
        }
        Ok(a) => {
            ctx.report.broken(
                Runner::Events,
                "register an endpoint",
                format!(
                    "answered {}: {} (a local run needs [subscriptions] allow_loopback)",
                    a.status,
                    truncate(&a.body.to_string())
                ),
            );
            server.abort();
            return;
        }
        Err(e) => {
            ctx.report.broken(
                Runner::Events,
                "register an endpoint",
                format!("unreachable: {e}"),
            );
            server.abort();
            return;
        }
    };
    ctx.report.held(Runner::Events, "register an endpoint");
    let key = mako_service::webhook::signing_key(&secret);
    let endpoint = format!("/api/v1/webhooks/endpoints/{id}");

    // The test delivery: signed under the issued secret, a CloudEvent.
    let _ = ctx
        .operator("POST", &format!("{endpoint}/test"), Some(&json!({})))
        .await;
    match tokio::time::timeout(ctx.target.wait, rx.recv()).await {
        Ok(Some(d)) => match judge_envelope(&key, &d) {
            Ok(_) => ctx.report.held(
                Runner::Events,
                "the test delivery is signed and a CloudEvent",
            ),
            Err(why) => ctx.report.broken(
                Runner::Events,
                "the test delivery is signed and a CloudEvent",
                why,
            ),
        },
        _ => ctx.report.broken(
            Runner::Events,
            "the test delivery arrives",
            format!("nothing arrived within {}s", ctx.target.wait.as_secs()),
        ),
    }

    if let Err(e) = suite.seed(ctx).await {
        ctx.report
            .broken(Runner::Events, "seed the service", e.to_string());
    }
    let now = time::OffsetDateTime::now_utc();
    let window = json!({
        "from": (now - time::Duration::days(30)).format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "to": (now + time::Duration::hours(1)).format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
    });
    let _ = ctx
        .operator("POST", &format!("{endpoint}/replay"), Some(&window))
        .await;

    // Collect until the stream is quiet.
    let mut deliveries = Vec::new();
    let deadline = tokio::time::Instant::now() + ctx.target.wait;
    loop {
        let quiet = if deliveries.is_empty() {
            ctx.target.wait
        } else {
            Duration::from_secs(3)
        };
        match tokio::time::timeout_at(deadline.min(tokio::time::Instant::now() + quiet), rx.recv())
            .await
        {
            Ok(Some(d)) => deliveries.push(d),
            _ => break,
        }
    }
    judge_replay(ctx, suite, &key, &deliveries);

    if let Ok(a) = ctx.operator("GET", &endpoint, None).await
        && let Some(etag) = a.etag
    {
        let _ = ctx
            .http
            .delete(format!("{}{endpoint}", ctx.target.base))
            .bearer_auth(&ctx.target.token)
            .header(reqwest::header::IF_MATCH, etag)
            .send()
            .await;
    }
    server.abort();
}

/// The CloudEvent in `d`, signed under `key`.
fn judge_envelope(key: &[u8], d: &Delivery) -> Result<Value, String> {
    mako_service::webhook::verify_request(Some(key), &d.headers, &d.body)
        .map_err(|e| format!("the signature does not verify (Standard Webhooks): {e}"))?;
    let ce: Value = serde_json::from_slice(&d.body).map_err(|e| format!("not JSON: {e}"))?;
    if ce["specversion"] != "1.0" {
        return Err("specversion is not \"1.0\"".to_owned());
    }
    for attr in ["id", "source", "type"] {
        if ce[attr].as_str().is_none_or(str::is_empty) {
            return Err(format!("the CloudEvent has no `{attr}`"));
        }
    }
    Ok(ce)
}

fn judge_replay(ctx: &Ctx, suite: &dyn Suite, key: &[u8], deliveries: &[Delivery]) {
    let declared = suite.emits();
    let schemas: BTreeMap<&str, &str> = crate::documents::EVENT_SCHEMAS.iter().copied().collect();
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in deliveries {
        let ce = match judge_envelope(key, d) {
            Ok(ce) => ce,
            Err(why) => {
                ctx.report.broken(
                    Runner::Events,
                    "a replayed delivery is signed and a CloudEvent",
                    why,
                );
                continue;
            }
        };
        let ty = ce["type"].as_str().unwrap_or_default().to_owned();
        if ty == mako_events::mako::WEBHOOK_TEST {
            continue;
        }
        if !declared.contains(&ty.as_str()) {
            ctx.report.broken(
                Runner::Events,
                format!("emits only declared types: {ty}"),
                "a type the suite does not declare — undocumented for a subscriber",
            );
            continue;
        }
        let gaps = match schemas.get(ty.as_str()) {
            Some(schema) => match serde_json::from_str::<Value>(schema) {
                Ok(schema) => validate(&schema, &ce["data"]),
                Err(e) => vec![format!("the published schema is not JSON: {e}")],
            },
            None => Vec::new(),
        };
        seen.entry(ty).or_default().extend(gaps);
    }
    for ty in declared {
        let name = format!("event {ty}");
        match (seen.get(ty), schemas.contains_key(ty)) {
            (None, _) => {
                ctx.report
                    .skipped(Runner::Events, name, "not observed in the replay window")
            }
            (Some(_), false) => ctx.report.skipped(
                Runner::Events,
                name,
                "no published schema — signature and envelope checked, `data` not",
            ),
            (Some(gaps), true) if gaps.is_empty() => ctx.report.held(Runner::Events, name),
            (Some(gaps), true) => ctx.report.broken(
                Runner::Events,
                name,
                format!(
                    "`data` does not meet the published schema: {}",
                    gaps.join("; ")
                ),
            ),
        }
    }
}

/// The service's inbound routes accept a signed event, again under the same
/// id, and refuse it unsigned.
async fn inbound(ctx: &Ctx, suite: &dyn Suite) {
    for route in suite.inbound() {
        let name = format!("inbound {}", route.path);
        let Some(secret) = ctx.target.inbound_secret.as_deref() else {
            ctx.report
                .skipped(Runner::Events, name, "no --inbound-secret given");
            continue;
        };
        let body = route.event.to_string();
        let id = route.event["id"]
            .as_str()
            .unwrap_or("conformance")
            .to_owned();
        let post = |signed: bool| {
            let mut req = ctx
                .http
                .post(format!("{}{}", ctx.target.base, route.path))
                .header("content-type", "application/json")
                .body(body.clone());
            if signed {
                let now = time::OffsetDateTime::now_utc().unix_timestamp();
                let key = mako_service::webhook::signing_key(secret);
                for (h, v) in mako_service::webhook::headers(&key, &id, now, body.as_bytes()) {
                    req = req.header(h, v);
                }
            }
            req.send()
        };
        let first = post(true).await.map(|r| r.status().as_u16());
        let again = post(true).await.map(|r| r.status().as_u16());
        let unsigned = post(false).await.map(|r| r.status().as_u16());
        match (first, again, unsigned) {
            (Ok(a), Ok(b), Ok(401)) if (200..300).contains(&a) && (200..300).contains(&b) => {
                ctx.report.held(Runner::Events, name);
            }
            (Ok(a), Ok(b), Ok(u)) => ctx.report.broken(
                Runner::Events,
                name,
                format!(
                    "signed {a}, redelivered {b}, unsigned {u} — expected 2xx, 2xx and 401: \
                     a redelivery must be accepted and an unsigned event refused"
                ),
            ),
            (a, b, u) => ctx.report.broken(
                Runner::Events,
                name,
                format!(
                    "unreachable: {:?}",
                    [a.err(), b.err(), u.err()].map(|e| e.map(|e| e.to_string()))
                ),
            ),
        }
    }
}
