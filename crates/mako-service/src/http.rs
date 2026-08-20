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
    default_client_with(std::time::Duration::from_secs(30))
}

/// Like [`default_client`] but with a caller-chosen **request** timeout — for the
/// occasional call that legitimately needs longer (a slow bulk export) or
/// shorter than the 30 s default. Keeps the 5 s connect timeout and the
/// no-redirect SSRF guard, so callers never re-specify (and mis-specify) those.
///
/// # Panics
///
/// Panics only if the TLS stack fails to initialise, which cannot happen with
/// the default `reqwest` feature set on any supported platform.
#[must_use]
pub fn default_client_with(request_timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(request_timeout)
        .connect_timeout(std::time::Duration::from_secs(5))
        .pool_max_idle_per_host(4)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest default_client: TLS initialisation is infallible on supported platforms")
}

/// A peer service this daemon calls: base URL, optional service credential, and
/// the status conventions every mako service shares.
///
/// The transport primitive only — where the credential goes, what a `404`
/// means, how a non-2xx is reported. Typed domain clients own their request
/// shapes and response types and use this underneath; a gateway that must relay
/// an upstream's raw body and status builds that on top (`portald` does).
///
/// The credential is sent as `Authorization: Bearer`. It is a **service**
/// credential and never stands in for an end user's token: a call made on
/// behalf of somebody must carry their identity in a header this type does not
/// set.
#[derive(Debug, Clone)]
pub struct Upstream {
    name: &'static str,
    base_url: String,
    api_key: Option<secrecy::SecretString>,
    client: reqwest::Client,
}

impl Upstream {
    /// Address `name` at `base_url`, sharing `client`.
    ///
    /// Sharing the daemon's `ServiceContext::http` client is the point: a
    /// per-upstream `reqwest::Client` gets its own connection pool and its own
    /// (often mis-specified) timeouts.
    #[must_use]
    pub fn new(
        name: &'static str,
        base_url: &str,
        api_key: Option<secrecy::SecretString>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            name,
            // Trailing slashes are a config-file accident; `{base}{path}` would
            // otherwise produce `//api/v1/…`, which some routers 404.
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client,
        }
    }

    /// The service's name, for logs and errors.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The normalised base URL, without a trailing slash.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// A `GET` to `path`, credential attached.
    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.authed(self.client.get(self.url(path)))
    }

    /// A `POST` to `path`, credential attached.
    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.authed(self.client.post(self.url(path)))
    }

    /// A `PUT` to `path`, credential attached.
    pub fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.authed(self.client.put(self.url(path)))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => {
                use secrecy::ExposeSecret as _;
                req.bearer_auth(key.expose_secret())
            }
            None => req,
        }
    }

    /// Send `req` and deserialize the body, mapping `404` to `None`.
    ///
    /// `404` is absence, not failure: a MaLo with no billing period and a MaLo
    /// that does not exist are the same answer to a reader, and both differ from
    /// an upstream that is broken.
    ///
    /// # Errors
    ///
    /// Transport failure, a non-404 error status, or a body that does not
    /// deserialize. Every message names the upstream, so a failure in a handler
    /// that calls three of them says which one.
    pub async fn json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<Option<T>, UpstreamError> {
        let Some(resp) = self.send(req).await? else {
            return Ok(None);
        };
        resp.json()
            .await
            .map(Some)
            .map_err(|e| UpstreamError::Body {
                service: self.name,
                source: e,
            })
    }

    /// Send `req` and read the body as text — an XML rendering, a CSV export.
    /// `404` maps to `None`.
    ///
    /// # Errors
    ///
    /// Transport failure or a non-404 error status.
    pub async fn text(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<Option<String>, UpstreamError> {
        let Some(resp) = self.send(req).await? else {
            return Ok(None);
        };
        resp.text()
            .await
            .map(Some)
            .map_err(|e| UpstreamError::Body {
                service: self.name,
                source: e,
            })
    }

    /// Send `req`, returning the response for a 2xx, `None` for a `404`.
    ///
    /// Use this when the body needs handling the helpers above do not cover —
    /// streaming, or a header the caller reads.
    ///
    /// # Errors
    ///
    /// Transport failure or a non-404 error status.
    pub async fn send(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<Option<reqwest::Response>, UpstreamError> {
        let resp = req.send().await.map_err(|e| UpstreamError::Transport {
            service: self.name,
            source: e,
        })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = resp.status();
        if !status.is_success() {
            // The body carries the upstream's own error code and message;
            // dropping it leaves an operator with a bare number.
            let body = resp.text().await.unwrap_or_default();
            return Err(UpstreamError::Status {
                service: self.name,
                status,
                body: body.chars().take(512).collect(),
            });
        }
        Ok(Some(resp))
    }
}

/// A call to a peer service that did not produce a usable answer.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    /// The service could not be reached, or the request timed out.
    #[error("{service} unreachable: {source}")]
    Transport {
        service: &'static str,
        source: reqwest::Error,
    },
    /// The service answered with an error status.
    #[error("{service} returned {status}: {body}")]
    Status {
        service: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },
    /// The service answered 2xx with a body the caller cannot use.
    #[error("{service} returned an unusable body: {source}")]
    Body {
        service: &'static str,
        source: reqwest::Error,
    },
}

impl UpstreamError {
    /// `true` when the upstream answered, but with an error.
    ///
    /// Separates "the peer is down" from "the peer refused this request", which
    /// a caller deciding between `503` and `502` needs.
    #[must_use]
    pub fn is_status(&self) -> bool {
        matches!(self, Self::Status { .. })
    }

    /// The status the upstream answered with, if it answered at all.
    #[must_use]
    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

#[cfg(all(test, feature = "oidc", feature = "cedar"))]
mod tests {
    use super::{Upstream, default_client};

    fn upstream(base: &str) -> Upstream {
        Upstream::new("edmd", base, None, default_client())
    }

    /// A trailing slash in the configured URL must not produce `//api/v1/…`,
    /// which some routers answer with a 404 that reads as missing data.
    #[test]
    fn a_trailing_slash_in_the_base_url_is_normalised() {
        assert_eq!(upstream("http://edmd:8380/").base_url(), "http://edmd:8380");
        assert_eq!(upstream("http://edmd:8380").base_url(), "http://edmd:8380");
        assert_eq!(
            upstream("http://edmd:8380///").base_url(),
            "http://edmd:8380"
        );
    }

    /// An error names the upstream that produced it. A handler that calls three
    /// services otherwise reports "returned 500" with no way to tell which.
    #[test]
    fn every_error_names_the_service() {
        let e = super::UpstreamError::Status {
            service: "marktd",
            status: reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            body: "no price sheet".to_owned(),
        };
        let msg = e.to_string();
        assert!(msg.contains("marktd"), "{msg}");
        assert!(msg.contains("422"), "{msg}");
        assert!(
            msg.contains("no price sheet"),
            "the upstream's reason survives: {msg}"
        );
    }

    /// "The peer is down" and "the peer refused this request" are different
    /// answers — a caller choosing between 503 and 502 needs to tell them apart.
    #[test]
    fn a_refusal_is_distinguishable_from_an_outage() {
        let refused = super::UpstreamError::Status {
            service: "marktd",
            status: reqwest::StatusCode::FORBIDDEN,
            body: String::new(),
        };
        assert!(refused.is_status());
        assert_eq!(refused.status(), Some(reqwest::StatusCode::FORBIDDEN));
    }

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
