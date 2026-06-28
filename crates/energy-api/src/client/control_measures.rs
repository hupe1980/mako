//! HTTP client for the Control Measures API v1 (`controlMeasuresV1.yaml`).
//!
//! Covers all eight endpoints across the four workflow roles:
//!
//! | Role  | Direction | Endpoints |
//! |-------|-----------|-----------|
//! | NB/LF | → MSB | `konfiguration`, `initialZustand` |
//! | MSB   | → NB/LF | `vorlaeufigePositiveAntwort`, `vorlaeufigeNegativeAntwort`, `positiveAntwort`, `negativeAntwort`, `informationAnweisung` |
//! | NB/LF/MSB/ÜNB | → MSB | `information` |
//!
//! ## Content-layer security (TR-03116-3)
//!
//! Enable automatic `DIGEST` + `SIGNATURE` signing via [`ControlMeasuresClient::with_signing`]
//! (requires feature `crypto`).

use reqwest::{Client, StatusCode};
use url::Url;
use uuid::Uuid;

use crate::error::Error;
use crate::models::electricity::{
    CommandControl, CommandRegular, LocationId, PreliminaryStatePositive, ReasonNegative,
    ReferenceId, StateNegative, StatePositive, StateUnknown,
};

#[cfg(feature = "crypto")]
use p256::ecdsa::SigningKey;

/// HTTP client for the [EDI-Energy Control Measures API v1][spec].
///
/// Build the underlying [`reqwest::Client`] via [`crate::transport::http::build_client`]
/// with the appropriate mTLS configuration, then pass it to [`Self::new`].
///
/// All requests are answered synchronously with HTTP 202 Accepted; the
/// business-logic response arrives via a separate asynchronous callback.
///
/// [spec]: https://github.com/EDI-Energy/api-electricity/blob/main/api/controlMeasures/controlMeasuresV1.yaml
#[derive(Clone, Debug)]
pub struct ControlMeasuresClient {
    inner: Client,
    base_url: Url,
    #[cfg(feature = "crypto")]
    signing_key: Option<SigningKey>,
}

impl ControlMeasuresClient {
    /// Create a client with the provided `reqwest::Client`.
    pub fn new(base_url: Url, client: Client) -> Self {
        Self {
            inner: client,
            base_url,
            #[cfg(feature = "crypto")]
            signing_key: None,
        }
    }

    /// Enable TR-03116-3 content-layer signing on every outgoing request.
    ///
    /// The `key` must belong to an EMT.API certificate from the BSI SM-PKI.
    /// Every request will carry `DIGEST` and `SIGNATURE` HTTP headers.
    #[cfg(feature = "crypto")]
    pub fn with_signing(mut self, key: SigningKey) -> Self {
        self.signing_key = Some(key);
        self
    }

    // ── Anweisung Steuerbefehl (NB/LF → MSB) ─────────────────────────────────

    /// `POST /[Post]/steuerbefehl/konfiguration/`
    ///
    /// Instruct the MSB to regulate a location to a specific maximum power
    /// value starting at `command.execution_time_from`.
    pub async fn send_konfiguration(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        location_id: &LocationId,
        command: &CommandControl,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let command_json = serde_json::to_string(command)?;
        let mut url = self.url("[Post]/steuerbefehl/konfiguration/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("commandControl", &command_json);
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    /// `POST /[Post]/steuerbefehl/initialZustand/`
    ///
    /// Instruct the MSB to reset a location to its initial / uncontrolled
    /// state starting at `command.execution_time_from`.
    pub async fn send_initial_zustand(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        location_id: &LocationId,
        command: &CommandRegular,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let command_json = serde_json::to_string(command)?;
        let mut url = self.url("[Post]/steuerbefehl/initialZustand/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("commandRegular", &command_json);
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Mitteilung zum weiteren Vorgehen (MSB → NB/LF) ───────────────────────

    /// `POST /[Post]/steuerbefehl/vorlaeufigePositiveAntwort/`
    ///
    /// Preliminary positive response: the command can in principle be executed.
    pub async fn send_vorlaeufigepositiveantwort(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: PreliminaryStatePositive,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let payload = serde_json::json!({ "preliminaryStatePositive": state });
        let mut url = self.url("[Post]/steuerbefehl/vorlaeufigePositiveAntwort/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("referenceId", &reference_id.to_string());
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("preliminaryResultPositive", &payload.to_string());
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    /// `POST /[Post]/steuerbefehl/vorlaeufigeNegativeAntwort/`
    ///
    /// Preliminary negative response: the command cannot be executed.
    pub async fn send_vorlaeufige_negative_antwort(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: StateNegative,
        reason: ReasonNegative,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let payload = serde_json::json!({ "stateNegative": state, "reasonNegative": reason });
        let mut url = self.url("[Post]/steuerbefehl/vorlaeufigeNegativeAntwort/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("referenceId", &reference_id.to_string());
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("resultNegative", &payload.to_string());
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Antwort Steuerbefehl (MSB → NB/LF) ───────────────────────────────────

    /// `POST /[Post]/steuerbefehl/positiveAntwort/`
    ///
    /// Final positive response: the command was successfully executed.
    pub async fn send_positive_antwort(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: StatePositive,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let payload = serde_json::json!({ "statePositive": state });
        let mut url = self.url("[Post]/steuerbefehl/positiveAntwort/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("referenceId", &reference_id.to_string());
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("resultPositive", &payload.to_string());
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    /// `POST /[Post]/steuerbefehl/negativeAntwort/`
    ///
    /// Final negative response: the command could not be executed.
    pub async fn send_negative_antwort(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: StateNegative,
        reason: ReasonNegative,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let payload = serde_json::json!({ "stateNegative": state, "reasonNegative": reason });
        let mut url = self.url("[Post]/steuerbefehl/negativeAntwort/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("referenceId", &reference_id.to_string());
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("resultNegative", &payload.to_string());
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Information Steuerbefehl Anweisender (MSB → NB/LF) ──────────────────

    /// `POST /[Post]/steuerbefehl/informationAnweisung/`
    ///
    /// Inform the commanding party that the final status is not yet known.
    pub async fn send_information_anweisung(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: StateUnknown,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let payload = serde_json::json!({ "stateUnknown": state });
        let mut url = self.url("[Post]/steuerbefehl/informationAnweisung/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("referenceId", &reference_id.to_string());
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("stateUnknown", &payload.to_string());
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Information Steuerbefehl Berechtigte (NB/LF/MSB/ÜNB → MSB) ─────────

    /// `POST /[Post]/steuerbefehl/information/`
    ///
    /// Broadcast the confirmed control command to all authorized parties.
    pub async fn send_information(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        location_id: &LocationId,
        partner_id: i64,
        command_control: Option<&CommandControl>,
        command_regular: Option<&CommandRegular>,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let mut url = self.url("[Post]/steuerbefehl/information/")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("locationId", &location_id.to_string());
            qp.append_pair("partnerId", &partner_id.to_string());
            if let Some(cc) = command_control {
                qp.append_pair("commandControl", &serde_json::to_string(cc)?);
            }
            if let Some(cr) = command_regular {
                qp.append_pair("commandRegular", &serde_json::to_string(cr)?);
            }
        }
        let mut req = self
            .inner
            .post(url.clone())
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time);
        req = self.sign_if_enabled(
            req,
            url.as_str(),
            &[],
            creation_date_time,
            &transaction_id.to_string(),
        )?;
        if let Some(id) = initial_transaction_id {
            req = req.header("initialTransactionId", id.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base_url.join(path).map_err(Error::Url)
    }

    /// Attach `DIGEST` and `SIGNATURE` headers when a signing key is configured.
    #[inline]
    fn sign_if_enabled(
        &self,
        req: reqwest::RequestBuilder,
        _uri: &str,
        _canonical_payload: &[u8],
        _creation_dt: &str,
        _tx_id: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        #[cfg(feature = "crypto")]
        if let Some(key) = &self.signing_key {
            use crate::transport::content_security::{self, HEADER_DIGEST, HEADER_SIGNATURE};
            let (digest, sig) = content_security::sign_request(
                _uri,
                _canonical_payload,
                _creation_dt,
                _tx_id,
                key,
            )?;
            return Ok(req
                .header(HEADER_DIGEST, digest)
                .header(HEADER_SIGNATURE, sig));
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
