//! `mako-seed listen` — every `CloudEvent` the running estate emits, verified
//! and printed as it arrives.
//!
//! ```text
//! mako-seed listen [--port 9199] [--host <name>] [--marktd <url>] [--once]
//! ```
//!
//! marktd is the estate's hub: makod, edmd, processd and the rest publish to
//! it, and it fans every event out to its subscribers, signed with each
//! subscriber's own secret (Standard Webhooks). `listen` registers one
//! subscription for every event type with a fresh secret, verifies each
//! delivery with `mako_service::webhook::verify_request` — the check every
//! service runs — and removes the subscription on Ctrl-C.
//!
//! `--host` is how marktd reaches this machine: `127.0.0.1` by default, which
//! `just dev`'s marktd admits (`[webhook] allow_loopback_subscribers`), and
//! `host.docker.internal` when marktd runs in `just demo`'s network. A
//! `TOKEN` in the environment is sent as the bearer (`just demo`'s edge
//! needs none; it answers as the admin persona).

use std::sync::Arc;

use anyhow::Context as _;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

struct Args {
    port: u16,
    host: String,
    marktd: String,
    once: bool,
}

fn parse(args: &[String]) -> anyhow::Result<Args> {
    let mut out = Args {
        port: 9199,
        host: "127.0.0.1".to_owned(),
        marktd: "http://127.0.0.1:8180".to_owned(),
        once: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--port" => out.port = value()?.parse().context("--port is a port number")?,
            "--host" => out.host = value()?,
            "--marktd" => value()?.trim_end_matches('/').clone_into(&mut out.marktd),
            "--once" => out.once = true,
            "--compose" => {}
            other => anyhow::bail!(
                "unknown argument {other:?} — listen [--port N] [--host NAME] [--marktd URL] [--once]"
            ),
        }
    }
    Ok(out)
}

/// The event in `body`, if it carries a valid signature under `secret`; else
/// why not.
fn verified(secret: &[u8], headers: &HeaderMap, body: &[u8]) -> Result<Value, String> {
    mako_service::webhook::verify_request(Some(secret), headers, body)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(body).map_err(|e| format!("not a JSON CloudEvent: {e}"))
}

struct Receiver {
    secret: String,
    once: Option<tokio::sync::mpsc::Sender<()>>,
}

async fn receive(State(r): State<Arc<Receiver>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    match verified(r.secret.as_bytes(), &headers, &body) {
        Ok(event) => {
            let field = |k: &str| event.get(k).and_then(Value::as_str).unwrap_or("");
            println!(
                "✓ {} · {} · {}",
                field("type"),
                field("subject"),
                field("time")
            );
            let data = event.get("data").cloned().unwrap_or(Value::Null);
            println!(
                "{}",
                serde_json::to_string_pretty(&data).unwrap_or_default()
            );
            if let Some(tx) = &r.once {
                let _ = tx.try_send(());
            }
            StatusCode::NO_CONTENT
        }
        Err(why) => {
            println!("✗ refused: {why}");
            StatusCode::UNAUTHORIZED
        }
    }
}

/// Subscribe, print each verified delivery, and remove the subscription on
/// Ctrl-C (or after the first delivery with `--once`).
pub async fn run(args: &[String]) -> anyhow::Result<()> {
    let args = parse(args)?;
    let id = format!("listen-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let url = format!("http://{}:{}/", args.host, args.port);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", args.port))
        .await
        .with_context(|| format!("port {} is taken — `--port` picks another", args.port))?;

    let http = reqwest::Client::new();
    let token = std::env::var("TOKEN").ok().filter(|t| !t.is_empty());
    let subscription = format!("{}/api/v1/subscriptions/{id}", args.marktd);
    let mut put = http.put(&subscription).json(&json!({
        "webhook_url": url,
        "webhook_secret": secret,
        "event_types": [],
        "active": true,
    }));
    if let Some(t) = &token {
        put = put.bearer_auth(t);
    }
    let answer = put.send().await.with_context(|| {
        format!(
            "marktd is not reachable at {} — is the estate running?",
            args.marktd
        )
    })?;
    if !answer.status().is_success() {
        let status = answer.status();
        let body = answer.text().await.unwrap_or_default();
        anyhow::bail!("marktd refused the subscription ({status}): {body}");
    }
    println!("👂 every event of the estate, at {url} (subscription {id}); Ctrl-C to stop");

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let receiver = Arc::new(Receiver {
        secret,
        once: args.once.then_some(tx),
    });
    let app = axum::Router::new()
        .fallback(axum::routing::post(receive))
        .with_state(receiver);
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = rx.recv() => {}
            }
        })
        .await;

    let mut delete = http.delete(&subscription);
    if let Some(t) = &token {
        delete = delete.bearer_auth(t);
    }
    match delete.send().await {
        Ok(a) if a.status().is_success() => println!("\n👋 subscription {id} removed"),
        Ok(a) => eprintln!(
            "\nmarktd kept subscription {id} ({}) — remove it with DELETE {subscription}",
            a.status()
        ),
        Err(e) => eprintln!("\nsubscription {id} not removed: {e} — DELETE {subscription}"),
    }
    served.context("the receiver stopped")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn signed(secret: &[u8], body: &[u8]) -> HeaderMap {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut h = HeaderMap::new();
        for (name, value) in mako_service::webhook::headers(secret, "evt-1", now, body) {
            h.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(&value).expect("header value"),
            );
        }
        h
    }

    #[test]
    fn a_delivery_signed_with_the_subscription_secret_is_printed() {
        let body = br#"{"type":"de.markt.malo.updated","subject":"51238696012","data":{}}"#;
        let event = verified(b"s3cret", &signed(b"s3cret", body), body).expect("verifies");
        assert_eq!(event["type"], "de.markt.malo.updated");
    }

    #[test]
    fn a_tampered_body_or_another_secret_is_refused() {
        let body = br#"{"type":"de.markt.malo.updated","data":{}}"#;
        let headers = signed(b"s3cret", body);
        assert!(verified(b"s3cret", &headers, br#"{"type":"forged"}"#).is_err());
        assert!(verified(b"other", &headers, body).is_err());
        assert!(verified(b"s3cret", &HeaderMap::new(), body).is_err());
    }

    #[test]
    fn the_arguments_parse_and_an_unknown_one_is_named() {
        let a = parse(&[
            "--port".into(),
            "9200".into(),
            "--host".into(),
            "h".into(),
            "--once".into(),
        ])
        .expect("parses");
        assert_eq!((a.port, a.host.as_str(), a.once), (9200, "h", true));
        let e = parse(&["--prot".into()])
            .err()
            .expect("refused")
            .to_string();
        assert!(e.contains("--prot"), "{e}");
    }
}
