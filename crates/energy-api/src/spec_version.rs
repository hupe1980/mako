//! Specification versions implemented by this crate.
//!
//! Each constant identifies the OpenAPI / AsyncAPI document version that the
//! corresponding module implements.  All four EDI-Energy API-Webdienste are
//! currently at version **1.0.0**.
//!
//! Use these constants in server implementations to populate
//! `ApiRecord::major_version` when self-registering at the
//! Verzeichnisdienst (the directory expects an `i32` `majorVersion` field
//! whose value is `1` for all current specs).
//!
//! ```rust
//! use energy_api::spec_version;
//!
//! assert_eq!(spec_version::DIRECTORY_SERVICE, "1.0.0");
//! assert_eq!(spec_version::DIRECTORY_WEBSOCKET, "1.0.0");
//! assert_eq!(spec_version::CONTROL_MEASURES, "1.0.0");
//! assert_eq!(spec_version::MALO_IDENT, "1.0.0");
//! ```

/// `directoryServiceV1.yaml` — EDI-Energy Verzeichnisdienst REST API.
pub const DIRECTORY_SERVICE: &str = "1.0.0";

/// `webSocketV1.yaml` — EDI-Energy Verzeichnisdienst WebSocket subscription API.
pub const DIRECTORY_WEBSOCKET: &str = "1.0.0";

/// `controlMeasuresV1.yaml` — EDI-Energy Control Measures API.
///
/// **Note:** the Control Measures spec currently omits a `/v1` URL prefix
/// (unlike the other APIs); the path layout is `/[Post]/steuerbefehl/<action>/`.
pub const CONTROL_MEASURES: &str = "1.0.0";

/// `maloIdentV1.yaml` — EDI-Energy MaLo Identification API.
pub const MALO_IDENT: &str = "1.0.0";

/// The `majorVersion` field value for all current specs as expected by the
/// Verzeichnisdienst (`ApiRecord::major_version: i32`).
pub const MAJOR: i32 = 1;
