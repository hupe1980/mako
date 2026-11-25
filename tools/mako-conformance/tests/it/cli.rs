//! A release job consumes the binary's streams, exit status, and evidence file together.

use std::path::Path;
use std::process::Output;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde_json::{Value, json};
use tokio::process::Command;

const CLI: &str = env!("CARGO_BIN_EXE_mako-conformance");
const MARKER: &str = "private-test-credential-3751";

async fn acceptance(url: &str, label: &str, path: &Path) -> Output {
    Command::new(CLI)
        .args([
            "vertragd",
            url,
            "--token",
            MARKER,
            "--only",
            "mcp",
            "--acceptance",
            "--target-build",
            label,
            "--report",
        ])
        .arg(path)
        .output()
        .await
        .unwrap()
}

#[tokio::test]
async fn explicit_help_and_invalid_input_have_distinct_streams_and_status() {
    let help = Command::new(CLI).arg("--help").output().await.unwrap();
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("--target-build")
    );
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("evidence.json");
    let invalid = acceptance("https://example.invalid", "invalid/label", &path).await;
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("--target-build")
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn machine_and_human_results_agree_without_exposing_probe_input() {
    let snapshot: Value = serde_json::from_str(include_str!(
        "../../../../services/vertragd/mcp.snapshot.json"
    ))
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("evidence.json");
    for (healthy, expected_code, expected_verdict) in
        [(true, 3, "incomplete"), (false, 1, "failed")]
    {
        let data = snapshot.clone();
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move |headers: HeaderMap| {
                let data = data.clone();
                async move {
                    if !headers.contains_key("authorization") {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    if !healthy {
                        return (StatusCode::INTERNAL_SERVER_ERROR, MARKER).into_response();
                    }
                    axum::Json(json!({"jsonrpc":"2.0", "id":1, "result":{"tools":data}}))
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let output = acceptance(&url, "fixture-7", &path).await;
        server.abort();
        let human = String::from_utf8(output.stdout).unwrap();
        let errors = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "{human} {errors}"
        );
        let report = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["verdict"], expected_verdict);
        assert!(human.contains(&expected_verdict.to_ascii_uppercase()));
        assert!(!human.contains('\x1b'));
        for stream in [&report, &human, &errors] {
            assert!(!stream.contains(MARKER));
            assert!(!stream.contains(&url));
        }
        assert_eq!(temp.path().read_dir().unwrap().count(), 1);
    }
}
