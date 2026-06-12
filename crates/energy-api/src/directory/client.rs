//! HTTP REST client for the EDI-Energy Directory Service v1.
//!
//! Requires feature `client`.

use reqwest::{Client, StatusCode};
use url::Url;

use crate::error::Error;
use crate::models::directory::{ApiRecord, ServiceInfo};

/// HTTP REST client for the [Directory Service v1 API][spec].
///
/// All communication must use mutual TLS with an EMT.API certificate from
/// SM-PKI. In production, build the underlying [`reqwest::Client`] with the
/// appropriate client identity certificate and the SM-PKI trust anchor, then
/// pass it to [`Self::new`].
///
/// For local testing an insecure client can be created with
/// [`Self::new_insecure`].
///
/// [spec]: https://github.com/EDI-Energy/api-directory-service/blob/main/api/directoryServiceV1.yaml
#[derive(Clone, Debug)]
pub struct DirectoryServiceClient {
    inner: Client,
    base_url: Url,
}

impl DirectoryServiceClient {
    /// Create a client using the provided `reqwest::Client`.
    ///
    /// The `base_url` must include the trailing slash, e.g.
    /// `https://verzeichnisdienst.example.de/`.
    pub fn new(base_url: Url, client: Client) -> Self {
        Self {
            inner: client,
            base_url,
        }
    }

    /// Create a client with a plain (no mTLS) reqwest client.
    ///
    /// **For testing only.** Production usage requires mTLS authentication
    /// with an EMT.API certificate.
    ///
    /// # Errors
    /// Returns [`Error::Transport`] if the reqwest client cannot be built.
    pub fn new_insecure(base_url: Url) -> Result<Self, Error> {
        let client = Client::builder()
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::new(base_url, client))
    }

    // ── Service info ─────────────────────────────────────────────────────────

    /// `GET /info/service/v1` — Retrieve information about this directory
    /// service instance (version, contact, revision).
    ///
    /// # Errors
    /// - [`Error::Http`] for 4xx/5xx responses.
    /// - [`Error::Json`] if the response body cannot be deserialized.
    pub async fn get_service_info(&self) -> Result<ServiceInfo, Error> {
        let url = self.url("info/service/v1")?;
        let resp = self.inner.get(url).send().await?;
        check_ok(resp).await?.json().await.map_err(Into::into)
    }

    // ── Records ───────────────────────────────────────────────────────────────

    /// `GET /record/{providerId}/{apiId}/{majorVersion}/v1` — Look up a
    /// directory entry.
    ///
    /// On success returns `(record, signing_cert, signature)` where:
    /// - `signing_cert` is the RFC 9440-encoded `X-BDEW-CERT` header value.
    /// - `signature` is the base64url-encoded `X-BDEW-SIGNATURE` header value.
    ///
    /// Use [`crate::jws`] (feature `crypto`) to verify the signature.
    ///
    /// # Errors
    /// - [`Error::NotFound`] if the entry does not exist (404).
    /// - [`Error::Redirect`] if a redirect is configured for this entry (307).
    ///   The embedded URL is the redirect target — re-issue the GET there.
    pub async fn get_record(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> Result<(ApiRecord, String, String), Error> {
        let url = self.record_url(provider_id, api_id, major_version)?;
        let resp = self.inner.get(url).send().await?;
        match resp.status() {
            StatusCode::OK => {
                let cert = header_value(&resp, "X-BDEW-CERT");
                let sig = header_value(&resp, "X-BDEW-SIGNATURE");
                let record: ApiRecord = resp.json().await?;
                Ok((record, cert, sig))
            }
            StatusCode::TEMPORARY_REDIRECT => {
                let location = header_value(&resp, "Location");
                Err(Error::Redirect { url: location })
            }
            StatusCode::NOT_FOUND => Err(Error::NotFound),
            s => Err(Error::Http {
                status: s.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }),
        }
    }

    /// `PUT /record/{providerId}/{apiId}/{majorVersion}/v1` — Create or update
    /// a directory entry (optional selfservice endpoint).
    ///
    /// The caller must supply the `signing_cert` (RFC 9440) and `signature`
    /// (base64url JWS signature from [`crate::jws::sign`]) that were produced
    /// for `record`.
    ///
    /// Returns `true` if a new record was **created** (201) or `false` if an
    /// existing record was **updated** (204).
    ///
    /// # Errors
    /// - [`Error::Http`] with status `400` on revision constraint violations;
    ///   the `X-BDEW-EXPECTED-REVISION` response header carries the next valid
    ///   revision number.
    /// - [`Error::Http`] with status `405` if the server does not support
    ///   selfservice.
    pub async fn put_record(
        &self,
        record: &ApiRecord,
        signing_cert: &str,
        signature: &str,
    ) -> Result<bool, Error> {
        let url = self.record_url(&record.provider_id, &record.api_id, record.major_version)?;
        let resp = self
            .inner
            .put(url)
            .header("X-BDEW-CERT", signing_cert)
            .header("X-BDEW-SIGNATURE", signature)
            .json(record)
            .send()
            .await?;
        match resp.status() {
            StatusCode::CREATED => Ok(true),
            StatusCode::NO_CONTENT => Ok(false),
            s => Err(Error::Http {
                status: s.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }),
        }
    }

    /// `DELETE /record/{providerId}/{apiId}/{majorVersion}/v1` — Delete a
    /// directory entry (optional selfservice endpoint).
    ///
    /// Returns `Ok(())` if the record was deleted or did not exist (204).
    ///
    /// # Errors
    /// - [`Error::Http`] with status `405` if selfservice is not supported.
    pub async fn delete_record(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> Result<(), Error> {
        let url = self.record_url(provider_id, api_id, major_version)?;
        let resp = self.inner.delete(url).send().await?;
        match resp.status() {
            StatusCode::NO_CONTENT => Ok(()),
            s => Err(Error::Http {
                status: s.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }),
        }
    }

    // ── Redirects ─────────────────────────────────────────────────────────────

    /// `PUT /redirect/{providerId}/{apiId}/{majorVersion}/v1?url=…` —
    /// Configure or replace a redirect for a directory entry (optional
    /// selfservice endpoint).
    ///
    /// # Errors
    /// - [`Error::Http`] with status `405` if selfservice is not supported.
    pub async fn put_redirect(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
        target_url: &str,
    ) -> Result<(), Error> {
        let url = self.redirect_url(provider_id, api_id, major_version)?;
        let resp = self
            .inner
            .put(url)
            .query(&[("url", target_url)])
            .send()
            .await?;
        match resp.status() {
            StatusCode::CREATED => Ok(()),
            s => Err(Error::Http {
                status: s.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }),
        }
    }

    /// `DELETE /redirect/{providerId}/{apiId}/{majorVersion}/v1` —
    /// Remove a redirect for a directory entry (optional selfservice endpoint).
    ///
    /// Returns `Ok(())` whether the redirect existed or not (200 either way).
    ///
    /// # Errors
    /// - [`Error::Http`] with status `405` if selfservice is not supported.
    pub async fn delete_redirect(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> Result<(), Error> {
        let url = self.redirect_url(provider_id, api_id, major_version)?;
        let resp = self.inner.delete(url).send().await?;
        match resp.status() {
            StatusCode::OK => Ok(()),
            s => Err(Error::Http {
                status: s.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            }),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base_url.join(path).map_err(Error::Url)
    }

    fn record_url(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> Result<Url, Error> {
        self.url(&format!(
            "record/{}/{}/{}/v1",
            encode_path_segment(provider_id),
            encode_path_segment(api_id),
            major_version,
        ))
    }

    fn redirect_url(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
    ) -> Result<Url, Error> {
        self.url(&format!(
            "redirect/{}/{}/{}/v1",
            encode_path_segment(provider_id),
            encode_path_segment(api_id),
            major_version,
        ))
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

async fn check_ok(resp: reqwest::Response) -> Result<reqwest::Response, Error> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        Err(Error::Http {
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        })
    }
}

fn header_value(resp: &reqwest::Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

/// Percent-encode a string for use as a URL path segment.
fn encode_path_segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
