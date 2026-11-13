//! HTTP REST client for the EDI-Energy Directory Service v1.
//!
//! Requires feature `client`.

use reqwest::{Client, StatusCode};
use url::Url;

use crate::error::{Error, RecordRefusal};
use crate::models::directory::{ApiRecord, ApiRecordRef, ServiceInfo};
use crate::transport::jws::RecordVerifyingKey;

/// How many `307` redirects [`DirectoryServiceClient::get_record`] follows.
///
/// Regelungen zum Verzeichnisdienst 1.1, Kap. 2.2 (S. 6) obliges the provider
/// to keep its redirects acyclic but sets no length; a provider moving
/// directories (Kap. 5.1) needs one hop, so a handful bounds a misconfigured
/// cycle without refusing any chain the document describes.
pub const MAX_REDIRECTS: usize = 5;

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
        // Conservative timeouts + no redirects even on this test helper: a bare
        // `Client::builder().build()` has no connect timeout (a SYN to a dead host
        // blocks for minutes) and follows redirects (an SSRF vector).
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
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
    /// directory entry and return it only once its signature verifies.
    ///
    /// The record is the provider's statement, not the directory's:
    /// Regelungen zum Verzeichnisdienst 1.1, Kap. 2.2 (S. 6) requires that „die
    /// aktuelle Signatur eines Eintrags MUSS jederzeit anhand eines gültigen
    /// Signaturzertifikats der Klasse EMT.API des entsprechenden Marktpartners
    /// überprüft werden können", and Kap. 5.2 (S. 13) rests the security of a
    /// followed redirect on „die TLS-Zertifikate des Clients und des Servers
    /// sowie die Signatur des Verzeichniseintrags". The directory distributes no
    /// certificates — „Zertifikate können ausschließlich über die
    /// LDAP-Verzeichnisdienste der ausstellenden CA abgerufen werden" (Kap. 1.6,
    /// S. 4) — so `signers` are the provider's EMT.API signature keys, obtained
    /// by the caller from the issuing Sub-CA, several during a certificate
    /// rollover. The `X-BDEW-CERT` the response carries is not trusted: a key
    /// the response names itself proves nothing about who signed.
    ///
    /// The JWS is the scheme of [`crate::transport::jws`] — the Regelungen
    /// leave its algorithm and encoding to the „Verzeichnisdienst API"
    /// specification (Kap. 7 \[3\], S. 14) — computed over the record as it
    /// arrived. A record naming another entry than the one requested is
    /// refused as well: a provider's genuine signature over its record for a
    /// different API would otherwise answer for this one.
    ///
    /// A `307` is followed (Kap. 5.2: „Ein Client MUSS dem Redirect eines
    /// Verzeichnisdienst-Servers folgen"), up to [`MAX_REDIRECTS`] times, and
    /// the record it leads to is held to the same check.
    ///
    /// # Errors
    /// - [`Error::UnverifiedRecord`] if `signers` is empty, the signature is
    ///   missing or verifies against none of them, the record names another
    ///   entry, or the redirect chain is longer than [`MAX_REDIRECTS`].
    /// - [`Error::NotFound`] if the entry does not exist (404).
    pub async fn get_record(
        &self,
        provider_id: &str,
        api_id: &str,
        major_version: i32,
        signers: &[RecordVerifyingKey],
    ) -> Result<ApiRecord, Error> {
        if signers.is_empty() {
            return Err(Error::UnverifiedRecord(RecordRefusal::NoTrustedKey {
                provider_id: provider_id.to_owned(),
            }));
        }
        let mut url = self.record_url(provider_id, api_id, major_version)?;
        let mut redirects = 0;
        let resp = loop {
            let resp = self.inner.get(url.clone()).send().await?;
            if resp.status() != StatusCode::TEMPORARY_REDIRECT {
                break resp;
            }
            let location = header_value(&resp, "Location");
            let next = url.join(&location).map_err(Error::Url)?;
            redirects += 1;
            if redirects > MAX_REDIRECTS {
                return Err(Error::UnverifiedRecord(RecordRefusal::TooManyRedirects {
                    max: MAX_REDIRECTS,
                    url: next.to_string(),
                }));
            }
            url = next;
        };
        match resp.status() {
            StatusCode::OK => {
                let mut signatures = resp.headers().get_all("X-BDEW-SIGNATURE").iter();
                let signature = match (signatures.next().map(|v| v.to_str()), signatures.next()) {
                    (Some(Ok(sig)), None) if !sig.trim().is_empty() => sig.trim().to_owned(),
                    _ => return Err(Error::UnverifiedRecord(RecordRefusal::MissingSignature)),
                };
                let raw: serde_json::Value = resp.json().await?;
                super::verify_record(
                    raw,
                    &signature,
                    signers,
                    &ApiRecordRef {
                        provider_id: provider_id.to_owned(),
                        api_id: api_id.to_owned(),
                        major_version,
                    },
                )
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
    /// (base64url JWS signature from `crate::jws::sign`) that were produced
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::jws;
    use p256::SecretKey;
    use p256::ecdsa::SigningKey;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const PROVIDER: &str = "9900357000004";

    fn signing_key() -> SigningKey {
        SigningKey::from(&SecretKey::from_slice(&[0x11; 32]).expect("a valid P-256 scalar"))
    }

    fn record(url: &str) -> serde_json::Value {
        serde_json::json!({
            "providerId": PROVIDER,
            "apiId": "maloIdV1",
            "majorVersion": 1,
            "url": url,
            "lastUpdated": "2026-10-01T00:00:00Z",
            "revision": 2,
            "status": "Online",
        })
    }

    /// The provider's JWS over `value` exactly as it is sent.
    fn signature_over(value: &serde_json::Value) -> String {
        use base64ct::{Base64UrlUnpadded, Encoding as _};
        use p256::ecdsa::signature::Signer as _;
        let payload = serde_jcs::to_string(value).expect("JCS");
        let input = format!(
            "{}.{}",
            jws::JWS_PROTECTED_HEADER,
            Base64UrlUnpadded::encode_string(payload.as_bytes())
        );
        let sig: p256::ecdsa::Signature = signing_key().sign(input.as_bytes());
        Base64UrlUnpadded::encode_string(&sig.to_bytes())
    }

    /// A directory answering every request with `response` (a raw HTTP/1.1
    /// response).
    async fn directory(response: String) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let response = response.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Url::parse(&format!("http://{addr}/")).expect("url")
    }

    fn ok_response(body: &serde_json::Value, signature: &str) -> String {
        let body = body.to_string();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             x-bdew-signature: {signature}\r\nx-bdew-cert: :AAAA:\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn verifying_key() -> RecordVerifyingKey {
        RecordVerifyingKey::from(&signing_key())
    }

    async fn lookup(base: Url, signers: &[RecordVerifyingKey]) -> Result<ApiRecord, Error> {
        DirectoryServiceClient::new_insecure(base)
            .expect("client")
            .get_record(PROVIDER, "maloIdV1", 1, signers)
            .await
    }

    /// A record whose endpoint was changed after the provider signed it is
    /// refused: the signature is what binds the endpoint to the provider.
    #[tokio::test]
    async fn a_tampered_record_is_refused() {
        let signed = record("https://lf.example/maloId/");
        let signature = signature_over(&signed);
        let tampered = record("https://attacker.example/maloId/");
        let base = directory(ok_response(&tampered, &signature)).await;
        let got = lookup(base, &[verifying_key()]).await;
        assert!(
            matches!(
                got,
                Err(Error::UnverifiedRecord(RecordRefusal::BadSignature { .. }))
            ),
            "a tampered record was accepted: {got:?}"
        );
    }

    /// The acceptance twin: the record the provider signed is returned. Its
    /// timestamp carries fractional seconds and it has a member this crate
    /// does not model, so a check over a re-serialised [`ApiRecord`] — which
    /// spells both differently — would refuse it.
    #[tokio::test]
    async fn a_correctly_signed_record_is_returned() {
        let mut signed = record("https://lf.example/maloId/");
        signed["lastUpdated"] = "2026-10-01T00:00:00.000Z".into();
        signed["comment"] = "not modelled".into();
        let base = directory(ok_response(&signed, &signature_over(&signed))).await;
        let got = lookup(base, &[verifying_key()])
            .await
            .expect("a signed record");
        assert_eq!(got.url.as_str(), "https://lf.example/maloId/");
    }

    /// A key the response names itself proves nothing: a record signed by
    /// anybody but the provider's trusted keys is refused, and with no trusted
    /// key at all nothing is accepted.
    #[tokio::test]
    async fn a_record_signed_by_an_untrusted_key_or_with_no_trusted_key_is_refused() {
        let signed = record("https://lf.example/maloId/");
        let base = directory(ok_response(&signed, &signature_over(&signed))).await;
        let other = RecordVerifyingKey::from(&SigningKey::from(
            &SecretKey::from_slice(&[0x22; 32]).expect("a valid P-256 scalar"),
        ));
        let got = lookup(base.clone(), &[other]).await;
        assert!(
            matches!(
                got,
                Err(Error::UnverifiedRecord(RecordRefusal::BadSignature { .. }))
            ),
            "{got:?}"
        );
        let got = lookup(base, &[]).await;
        assert!(
            matches!(
                got,
                Err(Error::UnverifiedRecord(RecordRefusal::NoTrustedKey { .. }))
            ),
            "{got:?}"
        );
    }

    /// The provider's genuine signature over its record for another API does
    /// not answer for the one requested.
    #[tokio::test]
    async fn a_signed_record_for_another_entry_is_refused() {
        let mut other = record("https://lf.example/controlMeasures/");
        other["apiId"] = "controlMeasuresV1".into();
        let base = directory(ok_response(&other, &signature_over(&other))).await;
        let got = lookup(base, &[verifying_key()]).await;
        assert!(
            matches!(
                got,
                Err(Error::UnverifiedRecord(RecordRefusal::WrongEntry { .. }))
            ),
            "{got:?}"
        );
    }

    /// A directory redirecting every request to `location`.
    async fn redirecting_to(location: &str) -> Url {
        directory(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: {location}\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n"
        ))
        .await
    }

    /// A redirect is followed, and the record it leads to is verified like any
    /// other — a redirect is not a way around the signature.
    #[tokio::test]
    async fn a_redirect_is_followed_and_its_record_verified() {
        let signed = record("https://lf.example/maloId/");
        let signature = signature_over(&signed);
        let path = format!("record/{PROVIDER}/maloIdV1/1/v1");

        let target = directory(ok_response(&signed, &signature)).await;
        let base = redirecting_to(target.join(&path).expect("location").as_str()).await;
        let got = lookup(base, &[verifying_key()]).await.expect("followed");
        assert_eq!(got.url.as_str(), "https://lf.example/maloId/");

        let tampered = record("https://attacker.example/maloId/");
        let target = directory(ok_response(&tampered, &signature)).await;
        let base = redirecting_to(target.join(&path).expect("location").as_str()).await;
        let got = lookup(base, &[verifying_key()]).await;
        assert!(
            matches!(
                got,
                Err(Error::UnverifiedRecord(RecordRefusal::BadSignature { .. }))
            ),
            "a tampered record behind a redirect was accepted: {got:?}"
        );
    }

    /// A redirect cycle ends in a refusal rather than a hang.
    #[tokio::test]
    async fn a_redirect_cycle_is_refused() {
        let base = redirecting_to("/again").await;
        let got = lookup(base, &[verifying_key()]).await;
        assert!(
            matches!(
                got,
                Err(Error::UnverifiedRecord(
                    RecordRefusal::TooManyRedirects { .. }
                ))
            ),
            "{got:?}"
        );
    }
}
