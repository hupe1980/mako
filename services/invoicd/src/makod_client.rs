//! HTTP client for the `makod` command API.
//!
//! Sends `POST /api/v1/commands` to approve or dispute a received INVOIC.

use reqwest::Client;
use serde_json::json;
use uuid::Uuid;

/// `makod` command API client.
#[derive(Debug, Clone)]
pub struct MakodClient {
    http: Client,
    base_url: String,
}

impl MakodClient {
    /// Create a new client targeting `base_url` (e.g. `"http://localhost:8180"`).
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Submit a `gpke.abrechnung.annehmen` (settle / accept) command to `makod`.
    ///
    /// `process_id` is the UUID from the MdmEvent `subject` field.
    /// `invoice_ref` is the INVOIC message-reference used by `dispatch_to_process`
    /// to route to the correct billing process.
    ///
    /// An `Idempotency-Key` derived from `process_id` is added so retries are
    /// safe.
    #[tracing::instrument(skip(self))]
    pub async fn settle_invoice(
        &self,
        process_id: Uuid,
        invoice_ref: &str,
    ) -> Result<(), MakodClientError> {
        let idempotency_key = Uuid::new_v5(&process_id, b"settle");
        let body = json!({
            "command": "gpke.abrechnung.annehmen",
            "payload": { "invoice_ref": invoice_ref }
        });

        let resp = self
            .http
            .post(format!("{}/api/v1/commands", self.base_url))
            .header("Idempotency-Key", idempotency_key.to_string())
            .json(&body)
            .send()
            .await
            .map_err(MakodClientError::Http)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(MakodClientError::Api { status, body })
        }
    }

    /// Submit a `gpke.abrechnung.ablehnen` (dispute) command to `makod`.
    ///
    /// `invoice_ref` is the INVOIC message-reference used for routing.
    /// `reason` is a human-readable dispute reason surfaced from the check findings.
    #[tracing::instrument(skip(self))]
    pub async fn dispute_invoice(
        &self,
        process_id: Uuid,
        invoice_ref: &str,
        reason: &str,
    ) -> Result<(), MakodClientError> {
        let idempotency_key = Uuid::new_v5(&process_id, b"dispute");
        let body = json!({
            "command": "gpke.abrechnung.ablehnen",
            "payload": { "invoice_ref": invoice_ref, "ablehnungsgrund": reason }
        });

        let resp = self
            .http
            .post(format!("{}/api/v1/commands", self.base_url))
            .header("Idempotency-Key", idempotency_key.to_string())
            .json(&body)
            .send()
            .await
            .map_err(MakodClientError::Http)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(MakodClientError::Api { status, body })
        }
    }
}

/// Errors from [`MakodClient`].
#[derive(Debug, thiserror::Error)]
pub enum MakodClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("makod returned {status}: {body}")]
    Api { status: u16, body: String },
}
