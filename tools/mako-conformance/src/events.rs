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
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::profile::Probe;
use crate::report::{Reason, Report};
use crate::suite::{Ctx, Suite};

/// One delivery as it arrived.
struct Delivery {
    headers: HeaderMap,
    body: Bytes,
}

// Reserve the whole run's capture budget before retaining a delivery. Serial
// admission also bounds memory while a body of unknown length is being read.
const MAX_DELIVERIES: usize = 128;
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

struct Capture {
    tx: mpsc::Sender<Delivery>,
    budget: tokio::sync::Mutex<(usize, usize)>,
    overflow: std::sync::atomic::AtomicBool,
}

async fn receive(State(capture): State<Arc<Capture>>, request: Request) -> StatusCode {
    let (parts, body) = request.into_parts();
    let mut budget = capture.budget.lock().await;
    if budget.0 < MAX_DELIVERIES {
        let read = axum::body::to_bytes(body, MAX_CAPTURE_BYTES - budget.1);
        if let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_secs(30), read).await {
            budget.0 += 1;
            budget.1 += bytes.len();
            return match capture.tx.try_send(Delivery {
                headers: parts.headers,
                body: bytes,
            }) {
                Ok(()) => StatusCode::NO_CONTENT,
                Err(_) => StatusCode::SERVICE_UNAVAILABLE,
            };
        }
    }
    capture
        .overflow
        .store(true, std::sync::atomic::Ordering::Relaxed);
    StatusCode::PAYLOAD_TOO_LARGE
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
                Probe::Listener,
                format!(
                    "cannot listen on {}: {e} — --listen picks another",
                    ctx.target.listen
                ),
            );
            return;
        }
    };
    let callback = match (&ctx.target.callback, listener.local_addr()) {
        (Some(callback), _) => callback.clone(),
        (None, Ok(bound)) => format!("http://{bound}/"),
        (None, Err(error)) => {
            ctx.report.broken(Probe::Listener, error.to_string());
            return;
        }
    };
    ctx.report.held(Probe::Listener);
    let (tx, mut rx) = mpsc::channel(MAX_DELIVERIES);
    let capture = Arc::new(Capture {
        tx,
        budget: tokio::sync::Mutex::new((0, 0)),
        overflow: std::sync::atomic::AtomicBool::new(false),
    });
    let app = axum::Router::new()
        .fallback(axum::routing::post(receive))
        .with_state(capture.clone());
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
                (Some(id), Some(secret))
                    if (1..=128).contains(&id.len())
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                        && !secret.is_empty() =>
                {
                    (id.to_owned(), secret.to_owned())
                }
                _ => {
                    ctx.report.broken(
                        Probe::Register,
                        "missing or invalid endpoint identifier or signing secret",
                    );
                    server.abort();
                    return;
                }
            }
        }
        Ok(a) => {
            ctx.report.broken(
                Probe::Register,
                format!("endpoint registration returned {}", a.status),
            );
            server.abort();
            return;
        }
        Err(e) => {
            ctx.report.broken(Probe::Register, e.to_string());
            server.abort();
            return;
        }
    };
    ctx.report.held(Probe::Register);
    let key = mako_service::webhook::signing_key(&secret);
    let endpoint = format!("/api/v1/webhooks/endpoints/{id}");

    let seed_window_start = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let test = ctx
        .operator("POST", &format!("{endpoint}/test"), Some(&json!({})))
        .await;
    record_submission(ctx, Probe::TestRequest, test);
    let declared = suite.emits();
    match observe_test_delivery(&mut rx, ctx.target.wait, &key, &id, &declared).await {
        Ok(true) => ctx.report.held(Probe::TestDelivery),
        Ok(false) => ctx
            .report
            .skipped(Probe::TestDelivery, Reason::DeliveryUnobserved),
        Err(why) => ctx.report.broken(Probe::TestDelivery, why),
    }

    match suite.seed(ctx).await {
        Ok(()) => ctx.report.held(Probe::Seed),
        Err(e) => ctx.report.broken(Probe::Seed, e.to_string()),
    }
    let now = time::OffsetDateTime::now_utc();
    let window = json!({
        "from": seed_window_start.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "to": now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
    });
    let replay = ctx
        .operator("POST", &format!("{endpoint}/replay"), Some(&window))
        .await;
    record_submission(ctx, Probe::ReplayRequest, replay);

    // A later delivery of an already-observed type can still invalidate its
    // schema verdict. Keep the callback open for the whole advertised window.
    let deliveries = collect_replay(&mut rx, ctx.target.wait).await;
    judge_replay(
        &ctx.report,
        &declared,
        &ctx.event_schemas,
        &key,
        &deliveries,
        capture.overflow.load(std::sync::atomic::Ordering::Relaxed),
    );

    let cleanup = async {
        let answer = ctx.operator("GET", &endpoint, None).await?;
        anyhow::ensure!(
            (200..300).contains(&answer.status),
            "cannot read the created endpoint"
        );
        let etag = answer
            .etag
            .ok_or_else(|| anyhow::anyhow!("missing endpoint ETag"))?;
        let request = ctx
            .http
            .delete(format!("{}{endpoint}", ctx.target.base))
            .bearer_auth(&ctx.target.token)
            .header(reqwest::header::IF_MATCH, etag);
        let answer = crate::transport::send(request).await?;
        anyhow::ensure!(answer.status.is_success(), "endpoint cleanup failed");
        anyhow::Ok(())
    }
    .await;
    match cleanup {
        Ok(()) => ctx.report.held(Probe::Cleanup),
        Err(error) => ctx.report.broken(Probe::Cleanup, error.to_string()),
    }
    server.abort();
}

/// A live subscription can deliver ordinary events before its direct test response.
#[must_use = "the caller must record the delivery verdict"]
async fn observe_test_delivery(
    rx: &mut mpsc::Receiver<Delivery>,
    wait: Duration,
    key: &[u8],
    endpoint_id: &str,
    declared: &[&str],
) -> Result<bool, String> {
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(Some(delivery)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        let event = judge_envelope(key, &delivery)?;
        let ty = event["type"].as_str().unwrap_or_default();
        if ty == mako_events::mako::WEBHOOK_TEST {
            if event["data"]["endpoint_id"].as_str() != Some(endpoint_id) {
                return Err("test event names a different or missing endpoint".into());
            }
            return Ok(true);
        }
        if !declared.contains(&ty) {
            return Err("undeclared event type at the test endpoint".into());
        }
    }
    Ok(false)
}

async fn collect_replay(rx: &mut mpsc::Receiver<Delivery>, wait: Duration) -> Vec<Delivery> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut deliveries = Vec::new();
    while let Ok(Some(delivery)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        deliveries.push(delivery);
    }
    deliveries
}

fn record_submission(ctx: &Ctx, probe: Probe, result: anyhow::Result<crate::suite::Answer>) {
    match result {
        Ok(answer) if (200..300).contains(&answer.status) => ctx.report.held(probe),
        Ok(answer) => ctx
            .report
            .broken(probe, format!("request returned {}", answer.status)),
        Err(error) => ctx.report.broken(probe, error.to_string()),
    }
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

fn judge_replay(
    report: &Report,
    declared: &[&str],
    schemas: &BTreeMap<&str, jsonschema::Validator>,
    key: &[u8],
    deliveries: &[Delivery],
    overflow: bool,
) {
    let mut seen = BTreeMap::<String, bool>::new();
    let mut invalid = overflow;
    for delivery in deliveries {
        let Ok(event) = judge_envelope(key, delivery) else {
            invalid = true;
            continue;
        };
        let ty = event["type"].as_str().unwrap_or_default();
        if ty == mako_events::mako::WEBHOOK_TEST {
            continue;
        }
        if !declared.contains(&ty) {
            invalid = true;
            continue;
        }
        let valid = schemas
            .get(ty)
            .is_none_or(|schema| schema.is_valid(&event["data"]));
        *seen.entry(ty.into()).or_insert(true) &= valid;
    }
    if invalid {
        report.broken(
            Probe::ReplayEnvelope,
            "invalid replay envelope, undeclared event type or capture limit exceeded",
        );
    } else if seen.is_empty() {
        report.skipped(Probe::ReplayEnvelope, Reason::DeliveryUnobserved);
    } else {
        report.held(Probe::ReplayEnvelope);
    }
    for &ty in declared {
        let probe = Probe::Event(ty.into());
        match (seen.get(ty), schemas.contains_key(ty)) {
            (None, _) => report.skipped(probe, Reason::DeliveryUnobserved),
            (Some(_), false) => report.skipped(probe, Reason::SchemaUnavailable),
            (Some(true), true) => report.held(probe),
            (Some(false), true) => {
                report.broken(probe, "event data does not satisfy the published schema")
            }
        }
    }
}

/// The service's inbound routes accept a signed event, again under the same
/// id, and refuse it unsigned.
async fn inbound(ctx: &Ctx, suite: &dyn Suite) {
    for route in suite.inbound() {
        let name = Probe::Inbound(route.path.into());
        let Some(secret) = ctx.target.inbound_secret.as_deref() else {
            ctx.report.skipped(name, Reason::MissingInboundSecret);
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
            crate::transport::send(req)
        };
        let first = post(true).await.map(|r| r.status.as_u16());
        let again = post(true).await.map(|r| r.status.as_u16());
        let unsigned = post(false).await.map(|r| r.status.as_u16());
        match (first, again, unsigned) {
            (Ok(a), Ok(b), Ok(401)) if (200..300).contains(&a) && (200..300).contains(&b) => {
                ctx.report.held(name);
            }
            (Ok(a), Ok(b), Ok(u)) => ctx.report.broken(
                name,
                format!(
                    "signed {a}, redelivered {b}, unsigned {u} — expected 2xx, 2xx and 401: \
                     a redelivery must be accepted and an unsigned event refused"
                ),
            ),
            (a, b, u) => ctx.report.broken(
                name,
                format!(
                    "unreachable: {:?}",
                    [a.err(), b.err(), u.err()].map(|e| e.map(|e| e.to_string()))
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;
    use crate::report::Verdict;

    fn signed(id: &str, ty: &str, data: Value) -> Delivery {
        let body = Bytes::from(
            json!({
                "specversion": "1.0", "type": ty, "source": "urn:mako:test",
                "id": id, "data": data
            })
            .to_string(),
        );
        let mut headers = HeaderMap::new();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        for (name, value) in mako_service::webhook::headers(b"secret", id, now, &body) {
            headers.insert(name, value.parse().unwrap());
        }
        Delivery { headers, body }
    }

    #[tokio::test]
    async fn only_the_signed_test_for_this_endpoint_satisfies_the_probe() {
        let domain = "de.mako.test";
        let test = mako_events::mako::WEBHOOK_TEST;
        for (types, endpoint, expected) in [
            (vec![domain, test], "here", Some(true)),
            (vec![test, domain], "here", Some(true)),
            (vec![domain], "here", Some(false)),
            (vec![test], "elsewhere", None),
        ] {
            let (tx, mut rx) = mpsc::channel(2);
            for ty in types {
                tx.send(signed(ty, ty, json!({"endpoint_id": endpoint})))
                    .await
                    .unwrap();
            }
            drop(tx);
            let observed = observe_test_delivery(
                &mut rx,
                Duration::from_secs(1),
                b"secret",
                "here",
                &[domain],
            )
            .await;
            assert_eq!(observed.ok(), expected);
        }
    }

    #[tokio::test]
    async fn a_late_invalid_occurrence_fails_its_previously_held_schema() {
        let (tx, mut rx) = mpsc::channel(2);
        let send = async move {
            tx.send(signed("one", "de.mako.test", json!(12)))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(3200)).await;
            tx.send(signed("two", "de.mako.test", json!("wrong type")))
                .await
                .unwrap();
        };
        let (_, deliveries) = tokio::join!(send, collect_replay(&mut rx, Duration::from_secs(30)));
        let report = Report::new(Profile {
            service: "fixture".into(),
            only: None,
            fingerprint: "fixture".into(),
            expected: [
                (Probe::Event("de.mako.test".into()), true),
                (Probe::ReplayEnvelope, true),
            ]
            .into(),
            materials: BTreeMap::new(),
        });
        let schemas = [(
            "de.mako.test",
            jsonschema::draft202012::new(&json!({"type":"integer"})).unwrap(),
        )]
        .into();
        judge_replay(
            &report,
            &["de.mako.test"],
            &schemas,
            b"secret",
            &deliveries,
            false,
        );
        let evidence = report.acceptance("fixture").unwrap();
        assert_eq!(evidence.verdict, Verdict::Failed);
        assert_eq!(evidence.counts.broken, 1);
        assert_eq!(evidence.counts.held, 1);
    }
}
