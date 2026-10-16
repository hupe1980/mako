//! HTTP client for the MaLo Identification API v1 (`maloIdentV1.yaml`).
//!
//! Implements the GPKE part 2 — "Ermittlung der MaLo-ID der Marktlokation"
//! process for the 24 h supplier-switch (LFW24), per BNetzA decision BK6-22-024.
//!
//! ## Process flow
//!
//! ```text
//!  Lieferant (LF)                   Netzbetreiber (NB)
//!       │                                  │
//!       │─ POST /maloId/request/v1 ───────>│  request MaLo-ID
//!       │                                  │
//!       │<─ POST /maloId/dataForMarket...  │  positive: MaLo data
//!       │      Positive/v1 ────────────────│
//!       │   OR                             │
//!       │<─ POST /maloId/dataForMarket...  │  negative: reason code
//!       │      Negative/v1 ────────────────│
//! ```
//!
//! ## Content-layer security (TR-03116-3)
//!
//! Enable content-layer signing via [`MaloIdentClient::with_signing`]: every
//! request then carries `X-BDEW-DIGEST`, `X-BDEW-SIGNATURE` and `X-BDEW-CERT`
//! (Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3). The
//! digest covers the URI, the RFC 8785 canonical JSON body, `creationDateTime`,
//! `transactionId` and — when the request carries one — `initialTransactionId`.

use reqwest::{Client, StatusCode};
use url::Url;
use uuid::Uuid;

use crate::error::Error;
use crate::models::electricity::{
    IdentificationParameter, MaloIdentResultNegative, MaloIdentResultPositive, ReferenceId,
};

use p256::ecdsa::SigningKey;

use crate::transport::content_security::SignedInputs;

/// HTTP client for the [MaLo Identification API v1][spec].
///
/// The **Lieferant (LF)** uses [`Self::request_malo_id`] to ask the NB for
/// the MaLo-ID.  The **Netzbetreiber (NB)** uses
/// [`Self::send_positive_response`] / [`Self::send_negative_response`] to
/// deliver the result to the LF's callback endpoint.
///
/// Build the underlying `reqwest::Client` via
/// [`crate::transport::http::build_client`] with mTLS configuration.
///
/// [spec]: https://github.com/EDI-Energy/api-electricity/blob/main/api/identificationId/maloIdentV1.yaml
#[derive(Clone, Debug)]
pub struct MaloIdentClient {
    inner: Client,
    base_url: Url,
    signing: Option<Signing>,
}

/// The key a request is signed with and the certificate that names it.
#[derive(Clone, Debug)]
struct Signing {
    key: SigningKey,
    /// The [`HEADER_CERT`](crate::transport::content_security::HEADER_CERT)
    /// value, encoded once.
    cert_header: String,
}

impl MaloIdentClient {
    /// Create a client with the provided `reqwest::Client`.
    pub fn new(base_url: Url, client: Client) -> Self {
        Self {
            inner: client,
            base_url,
            signing: None,
        }
    }

    /// Enable content-layer signing on every outgoing request.
    ///
    /// `key` must belong to the EMT.API certificate `cert_der` from the BSI
    /// SM-PKI. Every request then carries `X-BDEW-DIGEST`, `X-BDEW-SIGNATURE`
    /// and `X-BDEW-CERT` (Regelungen zum Übertragungsweg für API-Webdienste 1.2,
    /// Kap. 3.3) — the receiver verifies the signature against the key of the
    /// certificate the request names, so the three travel together.
    #[must_use]
    pub fn with_signing(mut self, key: SigningKey, cert_der: &[u8]) -> Self {
        self.signing = Some(Signing {
            key,
            cert_header: crate::transport::content_security::cert_header_value(cert_der),
        });
        self
    }

    // ── LF → NB ───────────────────────────────────────────────────────────────

    /// `POST /maloId/request/v1` — Request the MaLo-ID for a market location.
    ///
    /// Sent by the **Lieferant** to the **Netzbetreiber**.
    ///
    /// The response is asynchronous: the NB will call
    /// [`Self::send_positive_response`] or [`Self::send_negative_response`] on
    /// the LF's callback endpoint.
    pub async fn request_malo_id(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        params: &IdentificationParameter,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let url = self.url("maloId/request/v1")?;
        let canonical = self.canonical_body(params)?;
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .header("Content-Type", "application/json")
            .body(canonical.clone());
        let initial = initial_transaction_id.map(|id| id.to_string());
        if let Some(id) = &initial {
            req = req.header("initialTransactionId", id);
        }
        req = self.sign_if_enabled(
            req,
            &SignedInputs {
                uri: url.as_str(),
                payload: &canonical,
                creation_date_time,
                transaction_id: &transaction_id.to_string(),
                initial_transaction_id: initial.as_deref(),
            },
        )?;
        check_accepted(req.send().await?).await
    }

    // ── NB → LF (callbacks) ───────────────────────────────────────────────────

    /// `POST /maloId/dataForMarketLocationPositive/v1` — Deliver a positive
    /// MaLo-ID identification result to the requester.
    ///
    /// Sent by the **Netzbetreiber** to the **Lieferant's** callback endpoint.
    pub async fn send_positive_response(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        result: &MaloIdentResultPositive,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let mut url = self.url("maloId/dataForMarketLocationPositive/v1")?;
        url.query_pairs_mut()
            .append_pair("referenceId", &reference_id.to_string());
        let canonical = self.canonical_body(result)?;
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .header("Content-Type", "application/json")
            .body(canonical.clone());
        let initial = initial_transaction_id.map(|id| id.to_string());
        if let Some(id) = &initial {
            req = req.header("initialTransactionId", id);
        }
        req = self.sign_if_enabled(
            req,
            &SignedInputs {
                uri: url.as_str(),
                payload: &canonical,
                creation_date_time,
                transaction_id: &transaction_id.to_string(),
                initial_transaction_id: initial.as_deref(),
            },
        )?;
        check_accepted(req.send().await?).await
    }

    /// `POST /maloId/dataForMarketLocationNegative/v1` — Deliver a negative
    /// MaLo-ID identification result to the requester.
    ///
    /// Sent by the **Netzbetreiber** to the **Lieferant's** callback endpoint.
    pub async fn send_negative_response(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        result: &MaloIdentResultNegative,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let mut url = self.url("maloId/dataForMarketLocationNegative/v1")?;
        url.query_pairs_mut()
            .append_pair("referenceId", &reference_id.to_string());
        let canonical = self.canonical_body(result)?;
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .header("Content-Type", "application/json")
            .body(canonical.clone());
        let initial = initial_transaction_id.map(|id| id.to_string());
        if let Some(id) = &initial {
            req = req.header("initialTransactionId", id);
        }
        req = self.sign_if_enabled(
            req,
            &SignedInputs {
                uri: url.as_str(),
                payload: &canonical,
                creation_date_time,
                transaction_id: &transaction_id.to_string(),
                initial_transaction_id: initial.as_deref(),
            },
        )?;
        check_accepted(req.send().await?).await
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base_url.join(path).map_err(Error::Url)
    }

    /// Serialize `value` to RFC 8785 canonical JSON bytes for use as the
    /// request body and as the signing payload.
    fn canonical_body<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, Error> {
        if self.signing.is_some() {
            return crate::transport::content_security::canonical_json(value);
        }
        // When signing is disabled fall back to compact serde_json so the
        // body is still well-formed JSON (no pretty-printing overhead).
        serde_json::to_vec(value).map_err(Error::Json)
    }

    /// Attach the three content-signature headers when signing is configured.
    #[inline]
    fn sign_if_enabled(
        &self,
        req: reqwest::RequestBuilder,
        inputs: &SignedInputs<'_>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        if let Some(signing) = &self.signing {
            use crate::transport::content_security::{
                self, HEADER_CERT, HEADER_DIGEST, HEADER_SIGNATURE,
            };
            let (digest, sig) = content_security::sign_request(inputs, &signing.key)?;
            return Ok(req
                .header(HEADER_DIGEST, digest)
                .header(HEADER_SIGNATURE, sig)
                .header(HEADER_CERT, &signing.cert_header));
        }
        Ok(req)
    }
}

async fn check_accepted(resp: reqwest::Response) -> Result<(), Error> {
    match resp.status() {
        StatusCode::ACCEPTED => Ok(()),
        s => Err(Error::Http {
            status: s.as_u16(),
            body: resp.text().await.unwrap_or_default(),
        }),
    }
}
