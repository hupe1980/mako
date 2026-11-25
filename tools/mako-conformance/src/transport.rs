//! HTTP limits shared by every probe that talks to a replacement service.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, ensure};
use reqwest::{Client, RequestBuilder, StatusCode, Url, header::HeaderMap};

const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[must_use = "the target and transport configuration must be validated"]
pub(crate) fn client(base: &str) -> anyhow::Result<Client> {
    let url = Url::parse(base).context("invalid test target URL")?;
    let host = url.host_str().context("test target needs a host")?;
    let loopback = host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "use HTTPS or a loopback HTTP test target"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "the test target URL cannot contain credentials, a query or a fragment"
    );
    Ok(Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()?)
}

pub(crate) struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[must_use = "an unsuccessful response must fail the probe"]
pub(crate) async fn send(request: RequestBuilder) -> anyhow::Result<Response> {
    let mut response = request.send().await?;
    ensure!(
        !response.status().is_redirection(),
        "test target returned a redirect"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= MAX_BODY_BYTES as u64),
        "probe response exceeds 16 MiB"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= MAX_BODY_BYTES - body.len(),
            "probe response exceeds 16 MiB"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(Response {
        status: response.status(),
        headers: response.headers().clone(),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use tokio::net::TcpListener;

    #[test]
    fn credentials_need_https_or_loopback() {
        for base in [
            "https://example.test/root",
            "http://127.0.0.1:9000",
            "http://[::1]:9000",
        ] {
            assert!(client(base).is_ok(), "{base}");
        }
        for base in [
            "http://example.test",
            "https://name:secret@example.test",
            "https://example.test/?token=x",
            "file:///tmp",
            "http://localhost.example.test",
        ] {
            assert!(client(base).is_err(), "{base}");
        }
    }

    #[tokio::test]
    async fn redirect_does_not_deliver_credentials_to_a_second_origin() {
        let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!("http://{}/", other.local_addr().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/",
            get(move || async move { (StatusCode::TEMPORARY_REDIRECT, [("location", location)]) }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = send(client(&base).unwrap().get(&base).bearer_auth("fixture")).await;
        assert!(result.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), other.accept())
                .await
                .is_err()
        );
        server.abort();
    }

    #[tokio::test]
    async fn oversized_chunked_body_and_stalled_body_fail_the_probe() {
        use axum::body::{Body, Bytes};
        use std::convert::Infallible;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/large",
                get(|| async {
                    let chunk = Bytes::from(vec![0; 1024 * 1024]);
                    Body::from_stream(tokio_stream::iter(
                        (0..17).map(move |_| Ok::<_, Infallible>(chunk.clone())),
                    ))
                }),
            )
            .route(
                "/stall",
                get(|| async {
                    Body::from_stream(tokio_stream::pending::<Result<Bytes, Infallible>>())
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = client(&base).unwrap();
        let error = send(client.get(format!("{base}/large")))
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("exceeds 16 MiB"));
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
        assert!(
            send(
                client
                    .get(format!("{base}/stall"))
                    .timeout(Duration::from_millis(100))
            )
            .await
            .is_err()
        );
        server.abort();
    }
}
