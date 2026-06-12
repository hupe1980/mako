//! WebSocket subscription client for the EDI-Energy Directory Service v1.
//!
//! Requires feature `websocket`.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::error::Error;
use crate::types::directory::{DirectoryNotification, SubscriptionRequest};

/// Handle for sending [`SubscriptionRequest`] messages to the server.
///
/// Obtained from [`DirectoryWsClient::connect`].
#[derive(Clone, Debug)]
pub struct SubscriptionSender {
    tx: mpsc::Sender<SubscriptionRequest>,
}

impl SubscriptionSender {
    /// Send a subscription management request to the server.
    ///
    /// # Errors
    /// Returns [`Error::Transport`] if the WebSocket connection has been closed.
    pub async fn send(&self, request: SubscriptionRequest) -> Result<(), Error> {
        self.tx
            .send(request)
            .await
            .map_err(|_| Error::Transport("WebSocket connection closed".into()))
    }
}

/// WebSocket client for the [Directory Service WebSocket API v1][spec].
///
/// Connects to `/ws/subscriptions/v1`, sends [`SubscriptionRequest`] messages,
/// and yields [`DirectoryNotification`] messages from the server.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "websocket")]
/// # async fn example() -> Result<(), energy_api::Error> {
/// use energy_api::directory::{DirectoryWsClient, SubscriptionRequest, ApiRecordRef, SubscriptionItem};
/// use futures_util::StreamExt;
/// use url::Url;
///
/// let ws_url = Url::parse("wss://verzeichnisdienst.example.de/ws/subscriptions/v1")?;
/// let (sender, mut notifications) = DirectoryWsClient::connect(ws_url).await?;
///
/// // Subscribe to one entry
/// sender.send(SubscriptionRequest {
///     id: "req-1".into(),
///     requested: Some(vec![SubscriptionItem {
///         record_ref: ApiRecordRef {
///             provider_id: "1234567890123".into(),
///             api_id: "controlMeasuresV1".into(),
///             major_version: 1,
///         },
///         known_revision: None,
///     }]),
///     canceled: None,
/// }).await?;
///
/// // Consume notifications
/// while let Some(notif) = notifications.recv().await {
///     let n = notif?;
///     println!("notification at {}", n.timestamp);
/// }
/// # Ok(())
/// # }
/// ```
///
/// [spec]: https://github.com/EDI-Energy/api-directory-service/blob/main/api/webSocketV1.yaml
pub struct DirectoryWsClient;

impl DirectoryWsClient {
    /// Connect to the directory service WebSocket endpoint and return a
    /// `(sender, receiver)` pair.
    ///
    /// - `sender` — use to manage subscriptions (see [`SubscriptionSender`]).
    /// - `receiver` — async channel of incoming [`DirectoryNotification`] messages.
    ///
    /// A background Tokio task drives the WebSocket I/O; it exits when either
    /// the connection is closed or `sender` is dropped.
    ///
    /// # Errors
    /// Returns [`Error::Transport`] if the WebSocket handshake fails.
    pub async fn connect(
        url: Url,
    ) -> Result<(SubscriptionSender, mpsc::Receiver<Result<DirectoryNotification, Error>>), Error>
    {
        let (ws_stream, _http_resp) = connect_async(url.as_str())
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        let (mut ws_sink, mut ws_source) = ws_stream.split();

        // Channel: application → WebSocket sink
        let (sub_tx, mut sub_rx) = mpsc::channel::<SubscriptionRequest>(32);
        // Channel: WebSocket source → application
        let (notif_tx, notif_rx) = mpsc::channel::<Result<DirectoryNotification, Error>>(64);

        let notif_tx = Arc::new(notif_tx);

        // Outbound task: forwards SubscriptionRequests to the WebSocket.
        tokio::spawn(async move {
            while let Some(req) = sub_rx.recv().await {
                let json = match serde_json::to_string(&req) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing_log(format!("energy-api: JSON encode error: {e}"));
                        continue;
                    }
                };
                if ws_sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            // Gracefully close when the sender is dropped.
            let _ = ws_sink.close().await;
        });

        // Inbound task: forwards DirectoryNotifications to the application.
        let notif_tx_clone = Arc::clone(&notif_tx);
        tokio::spawn(async move {
            while let Some(msg) = ws_source.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        let result = serde_json::from_str::<DirectoryNotification>(&text)
                            .map_err(Error::Json);
                        if notif_tx_clone.send(result).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    // Ping/pong handled by tungstenite internally.
                    _ => {}
                }
            }
        });

        Ok((SubscriptionSender { tx: sub_tx }, notif_rx))
    }
}

// Simple log shim that avoids a mandatory `tracing` dependency.
#[inline]
fn tracing_log(msg: String) {
    // Intentionally a no-op in library code; callers use their own tracing setup.
    let _ = msg;
}
