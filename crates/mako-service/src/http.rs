//! Shared HTTP client construction for inter-service calls.
//!
//! All mako daemons that call peer services (e.g. `processd` → `makod`, `edmd` → `marktd`)
//! should use [`default_client`] rather than `reqwest::Client::new()`.
//!
//! `reqwest::Client::new()` has no connection timeout — a SYN to an unreachable
//! host can block for several minutes, stalling pod startup and preventing
//! the liveness probe from responding.  [`default_client`] sets conservative
//! timeouts suitable for cluster-internal traffic, and refuses to follow
//! redirects so an operator- or partner-supplied URL cannot redirect an
//! outbound call onto internal infrastructure.

/// Build the default inter-service `reqwest::Client`.
///
/// Settings:
/// - **Request timeout**: 30 s (including response-body read)
/// - **Connect timeout**: 5 s (TCP handshake deadline)
/// - **Pool max idle per host**: 4 (sufficient for low-concurrency service calls)
/// - **Redirects**: **not followed**
///
/// # Why redirects are disabled
///
/// `reqwest` follows up to 10 redirects by default. Several of the URLs these
/// clients call are operator- or partner-supplied — ERP webhooks, the ERP
/// adapter, partner endpoints discovered through the Verzeichnisdienst — so an
/// endpoint that answers `302 → http://169.254.169.254/` or an in-cluster
/// address turns an allow-listed outbound call into a request against
/// infrastructure the caller never named. Refusing to follow keeps the target of
/// a request the one that was configured.
///
/// A caller that legitimately needs to act on a redirect reads the `Location`
/// header itself and re-issues deliberately — `verzeichnisdienst_worker` does
/// exactly that for the API-Webdienste `307`, where the redirect target is
/// meaningful data rather than a transport detail.
///
/// # Panics
///
/// Panics only if the underlying TLS/native-TLS stack fails to initialise,
/// which cannot happen with the default `reqwest` feature set on any supported
/// platform.
#[must_use]
pub fn default_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .pool_max_idle_per_host(4)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest default_client: TLS initialisation is infallible on supported platforms")
}

#[cfg(all(test, feature = "oidc", feature = "cedar"))]
mod tests {
    use super::default_client;

    /// An operator- or partner-supplied URL must not be able to redirect an
    /// outbound call onto infrastructure the caller never named — the
    /// redirect-based SSRF bypass.
    #[tokio::test]
    async fn the_default_client_does_not_follow_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt as _;
                // Redirect to a link-local metadata address.
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 302 Found\r\n\
                          Location: http://169.254.169.254/latest/meta-data/\r\n\
                          Content-Length: 0\r\n\r\n",
                    )
                    .await;
            }
        });

        let resp = default_client()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request completes");

        assert_eq!(
            resp.status().as_u16(),
            302,
            "the redirect must surface to the caller, not be followed"
        );
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("http://169.254.169.254/latest/meta-data/"),
            "the Location header stays available for callers that handle it deliberately"
        );
    }
}
