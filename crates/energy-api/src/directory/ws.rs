//! WebSocket subscription client for the Directory Service v1.
//!
//! Requires feature `websocket`.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::error::Error;
use crate::models::directory::{DirectoryNotification, SubscriptionRequest};

/// Handle for sending [`SubscriptionRequest`] messages to the server.
#[derive(Clone, Debug)]
pub struct SubscriptionSender {
    tx: mpsc::Sender<SubscriptionRequest>,
}

impl SubscriptionSender {
    /// Send a subscription management request to the server.
    ///
    /// # Errors
    /// Returns [`Error::Transport`] if the WebSocket connection is closed.
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
/// A pair of background Tokio tasks drive the WebSocket I/O; they exit when
/// either the connection is closed or `SubscriptionSender` is dropped.
///
/// # Example
///
/// ```no_run
/// # #[cfg(feature = "websocket")]
/// # async fn example() -> Result<(), energy_api::Error> {
/// use energy_api::directory::{
///     DirectoryWsClient, SubscriptionRequest, ApiRecordRef, SubscriptionItem,
/// };
/// use url::Url;
///
/// let ws = Url::parse("wss://verzeichnisdienst.example.de/ws/subscriptions/v1")?;
/// let (sender, mut rx) = DirectoryWsClient::connect(ws).await?;
///
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
/// while let Some(result) = rx.recv().await {
///     let n = result?;
///     println!("notification at {}", n.timestamp);
/// }
/// # Ok(())
/// # }
/// ```
///
/// [spec]: https://github.com/EDI-Energy/api-directory-service/blob/main/api/webSocketV1.yaml
pub struct DirectoryWsClient;

impl DirectoryWsClient {
    /// Connect to the directory service WebSocket endpoint.
    ///
    /// Returns `(sender, receiver)`:
    /// - `sender` — send [`SubscriptionRequest`] messages.
    /// - `receiver` — receive [`DirectoryNotification`] messages.
    ///
    /// # Errors
    /// Returns [`Error::Transport`] if the WebSocket handshake fails.
    pub async fn connect(
        url: Url,
    ) -> Result<
        (
            SubscriptionSender,
            mpsc::Receiver<Result<DirectoryNotification, Error>>,
        ),
        Error,
    > {
        let (ws_stream, _) = connect_async(url.as_str())
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        let (mut ws_sink, mut ws_source) = ws_stream.split();
        let (sub_tx, mut sub_rx) = mpsc::channel::<SubscriptionRequest>(32);
        let (notif_tx, notif_rx) = mpsc::channel::<Result<DirectoryNotification, Error>>(64);
        let notif_tx = Arc::new(notif_tx);

        // Outbound: forward SubscriptionRequests → WebSocket.
        tokio::spawn(async move {
            while let Some(req) = sub_rx.recv().await {
                match serde_json::to_string(&req) {
                    Ok(json) => {
                        if ws_sink.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
            let _ = ws_sink.close().await;
        });

        // Inbound: forward WebSocket messages → DirectoryNotifications.
        let notif_tx2 = Arc::clone(&notif_tx);
        tokio::spawn(async move {
            while let Some(msg) = ws_source.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        let r = serde_json::from_str::<DirectoryNotification>(&text)
                            .map_err(Error::Json);
                        if notif_tx2.send(r).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {} // Ping/pong handled internally by tungstenite.
                }
            }
        });

        Ok((SubscriptionSender { tx: sub_tx }, notif_rx))
    }
}
