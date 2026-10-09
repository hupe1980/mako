#![deny(unsafe_code)]
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
//! | `client`    | MaLo-Ident and directory HTTP clients (reqwest + rustls) |
//! | `server`    | Axum router factories for server implementations         |
//! | `websocket` | WebSocket subscription client (tokio-tungstenite)        |
//!
//! Content signing (`transport::content_security`) and the directory JWS
//! (`transport::jws`) are always compiled.
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
//!     .get_record("1234567890723", "controlMeasuresV1", 1)
//!     .await?;
//! println!("{}", record.url);
//! # Ok(())
//! # }
//! ```

#![expect(
    clippy::large_enum_variant,
    reason = "a pedantic style lint this crate does not adopt"
)]

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
