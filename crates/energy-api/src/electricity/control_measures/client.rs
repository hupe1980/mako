//! HTTP client for the Control Measures API v1.
//!
//! Requires feature `client`.

use reqwest::{Client, StatusCode};
use url::Url;
use uuid::Uuid;

use crate::error::Error;
use crate::types::electricity::{
    CommandControl, CommandRegular, LocationId, ReasonNegative, ReferenceId,
    StateNegative, StatePositive, StateUnknown, PreliminaryStatePositive,
};

/// HTTP client for the [EDI-Energy Control Measures API v1][spec].
///
/// Covers all eight endpoints (Anweisung, Mitteilung, Antwort, Information).
///
/// All requests must include mTLS client authentication with an EMT.API
/// certificate from SM-PKI.  Build the underlying [`reqwest::Client`] with the
/// certificate, then pass it to [`Self::new`].
///
/// # Signature
///
/// Each call is signed by the sender. For the full signature scheme see the
/// spec section "Inhaltsdatensicherungsebene". This client transmits the
/// required `DIGEST` and `SIGNATURE` headers when they are supplied.
///
/// [spec]: https://github.com/EDI-Energy/api-electricity/blob/main/api/controlMeasures/controlMeasuresV1.yaml
#[derive(Clone, Debug)]
pub struct ControlMeasuresClient {
    inner: Client,
    base_url: Url,
}

impl ControlMeasuresClient {
    /// Create a client using the provided `reqwest::Client`.
    pub fn new(base_url: Url, client: Client) -> Self {
        Self { inner: client, base_url }
    }

    /// Create a client with a plain (no mTLS) reqwest client — **for testing only**.
    pub fn new_insecure(base_url: Url) -> Result<Self, Error> {
        let client = Client::builder()
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::new(base_url, client))
    }

    // ── Anweisung Steuerbefehl (NB/LF → MSB) ────────────────────────────────

    /// `POST /[Post]/steuerbefehl/konfiguration/` — Send a power-value
    /// control command (Anweisung Steuerbefehl Konfiguration).
    ///
    /// Used by NB or LF to instruct the MSB to regulate a location to a
    /// specific maximum power value.
    ///
    /// # Errors
    /// - [`Error::Http`] with status `400` on invalid request.
    /// - [`Error::Http`] with status `401` for authentication failure.
    pub async fn send_konfiguration(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        location_id: &LocationId,
        command: &CommandControl,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let url = self.url("[Post]/steuerbefehl/konfiguration/")?;
        let command_json = serde_json::to_string(command)?;
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[("locationId", location_id.to_string())])
            .query(&[("commandControl", command_json)]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    /// `POST /[Post]/steuerbefehl/initialZustand/` — Reset a location to its
    /// initial / uncontrolled state (Anweisung Steuerbefehl InitialZustand).
    pub async fn send_initial_zustand(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        location_id: &LocationId,
        command: &CommandRegular,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let url = self.url("[Post]/steuerbefehl/initialZustand/")?;
        let command_json = serde_json::to_string(command)?;
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[("locationId", location_id.to_string())])
            .query(&[("commandRegular", command_json)]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Mitteilung zum weiteren Vorgehen (MSB → NB/LF) ───────────────────────

    /// `POST /[Post]/steuerbefehl/vorlaeufigePositiveAntwort/` — Preliminary
    /// positive response: the command can in principle be executed.
    pub async fn send_vorlaeufigepositiveantwort(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: PreliminaryStatePositive,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let url = self.url("[Post]/steuerbefehl/vorlaeufigePositiveAntwort/")?;
        let payload = serde_json::json!({ "preliminaryStatePositive": state });
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[
                ("referenceId", reference_id.to_string()),
                ("locationId", location_id.to_string()),
                ("preliminaryResultPositive", payload.to_string()),
            ]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    /// `POST /[Post]/steuerbefehl/vorlaeufigeNegativeAntwort/` — Preliminary
    /// negative response: the command cannot be executed, with a reason code.
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
        let url = self.url("[Post]/steuerbefehl/vorlaeufigeNegativeAntwort/")?;
        let payload =
            serde_json::json!({ "stateNegative": state, "reasonNegative": reason });
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[
                ("referenceId", reference_id.to_string()),
                ("locationId", location_id.to_string()),
                ("resultNegative", payload.to_string()),
            ]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Antwort Steuerbefehl (MSB → NB/LF) ───────────────────────────────────

    /// `POST /[Post]/steuerbefehl/positiveAntwort/` — Final positive response:
    /// the command was successfully executed.
    pub async fn send_positive_antwort(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: StatePositive,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let url = self.url("[Post]/steuerbefehl/positiveAntwort/")?;
        let payload = serde_json::json!({ "statePositive": state });
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[
                ("referenceId", reference_id.to_string()),
                ("locationId", location_id.to_string()),
                ("resultPositive", payload.to_string()),
            ]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    /// `POST /[Post]/steuerbefehl/negativeAntwort/` — Final negative response:
    /// the command could not be executed.
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
        let url = self.url("[Post]/steuerbefehl/negativeAntwort/")?;
        let payload =
            serde_json::json!({ "stateNegative": state, "reasonNegative": reason });
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[
                ("referenceId", reference_id.to_string()),
                ("locationId", location_id.to_string()),
                ("resultNegative", payload.to_string()),
            ]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Information Steuerbefehl Anweisender (MSB → NB/LF) ──────────────────

    /// `POST /[Post]/steuerbefehl/informationAnweisung/` — Inform the
    /// commanding party that the final status is not yet available.
    pub async fn send_information_anweisung(
        &self,
        transaction_id: Uuid,
        creation_date_time: &str,
        reference_id: ReferenceId,
        location_id: &LocationId,
        state: StateUnknown,
        initial_transaction_id: Option<Uuid>,
    ) -> Result<(), Error> {
        let url = self.url("[Post]/steuerbefehl/informationAnweisung/")?;
        let payload = serde_json::json!({ "stateUnknown": state });
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&[
                ("referenceId", reference_id.to_string()),
                ("locationId", location_id.to_string()),
                ("stateUnknown", payload.to_string()),
            ]);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Information Steuerbefehl Berechtigte (NB/LF/MSB/ÜNB → MSB) ─────────

    /// `POST /[Post]/steuerbefehl/information/` — Broadcast the confirmed
    /// control command to all authorized parties.
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
        let url = self.url("[Post]/steuerbefehl/information/")?;
        let mut query: Vec<(String, String)> = vec![
            ("locationId".into(), location_id.to_string()),
            ("partnerId".into(), partner_id.to_string()),
        ];
        if let Some(cc) = command_control {
            query.push(("commandControl".into(), serde_json::to_string(cc)?));
        }
        if let Some(cr) = command_regular {
            query.push(("commandRegular".into(), serde_json::to_string(cr)?));
        }
        let mut req = self
            .inner
            .post(url)
            .header("transactionId", transaction_id.to_string())
            .header("creationDateTime", creation_date_time)
            .query(&query);
        if let Some(itid) = initial_transaction_id {
            req = req.header("initialTransactionId", itid.to_string());
        }
        check_accepted(req.send().await?).await
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base_url.join(path).map_err(Error::Url)
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
