#![deny(missing_docs)]
//! # energy-api
//!
//! Rust client and server bindings for the German energy market API-Webdienste
//! (MaKo — Marktkommunikation).
//!
//! ## Module layout
//!
//! ```text
//! energy_api
//! ├── models/       OpenAPI/AsyncAPI types shared by all APIs
//! ├── transport/    HTTP + mTLS builder, JWS sign/verify
//! ├── directory/    Verzeichnisdienst — REST client, WebSocket client, server
//! ├── client/       Electricity API clients  (feature = "client")
//! │   ├── control_measures   NB/LF and MSB send calls
//! │   └── malo_ident         LF and NB callback calls
//! └── server/       Electricity API servers  (feature = "server")
//!     ├── control_measures   MSB and NB/LF receive handlers + axum router
//!     ├── malo_ident         NB and LF receive handlers + axum router
//!     └── wim_order          MSB receive handler (iMS Anmeldung) + NB callbacks
//! ```
//!
//! ## Feature flags
//!
//! | Feature     | Enables                                                  |
//! |-------------|----------------------------------------------------------|
//! | `client`    | HTTP clients for all APIs (reqwest + rustls)             |
//! | `server`    | Axum router factories for server implementations         |
//! | `websocket` | WebSocket subscription client (tokio-tungstenite)        |
//! | `crypto`    | JWS ECDSA-SHA256 sign/verify for directory records (p256)|
//!
//! ## Quick start
//!
//! ### Look up an endpoint via the directory
//!
//! ```no_run
//! # #[cfg(feature = "client")]
//! # async fn example() -> Result<(), energy_api::Error> {
//! use energy_api::directory::DirectoryServiceClient;
//! use url::Url;
//!
//! let base = Url::parse("https://verzeichnisdienst.example.de/")?;
//! let client = DirectoryServiceClient::new_insecure(base)?;
//! let (record, _cert, _sig) = client
//!     .get_record("1234567890123", "controlMeasuresV1", 1)
//!     .await?;
//! println!("{}", record.url);
//! # Ok(())
//! # }
//! ```
//!
//! ### Send a grid control command
//!
//! ```no_run
//! # #[cfg(feature = "client")]
//! # async fn example() -> Result<(), energy_api::Error> {
//! use energy_api::client::ControlMeasuresClient;
//! use energy_api::models::electricity::{
//!     CommandControl, LocationId, NeloId, MaximumPowerValue,
//! };
//! use url::Url;
//! use uuid::Uuid;
//!
//! let client = ControlMeasuresClient::new(
//!     Url::parse("https://msb.example.de/")?,
//!     reqwest::Client::new(),
//! );
//! client.send_konfiguration(
//!     Uuid::new_v4(),
//!     "2025-06-01T10:00:00.000Z",
//!     &LocationId::NetworkLocation(NeloId("E1234848431".into())),
//!     &CommandControl {
//!         maximum_power_value: MaximumPowerValue("10.5".into()),
//!         execution_time_from: "2025-06-01T10:00:00Z".into(),
//!         execution_time_until: None,
//!     },
//!     None,
//! ).await?;
//! # Ok(())
//! # }
//! ```

#![allow(clippy::too_many_arguments)]
#![allow(clippy::large_enum_variant)]

pub mod directory;
pub mod error;
pub mod models;
pub mod spec_version;
pub mod transport;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;

pub use error::Error;
