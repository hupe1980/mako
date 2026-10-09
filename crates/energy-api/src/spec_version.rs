//! Specification versions implemented by this crate.
//!
//! Each constant identifies the OpenAPI / AsyncAPI document version that the
//! corresponding module implements. All four EDI-Energy API-Webdienste are
//! at version **1.0.0**.
//!
//! The electricity specs live in `github.com/EDI-Energy/api-electricity`; the
//! Verzeichnisdienst is a **separate** repository,
//! `github.com/EDI-Energy/api-directory-service`.
//!
//! 1.0.0 is the version in force, and BNetzA Mitteilung Nr. 58 (01.10.2026)
//! keeps it so: no API-Webdienste document is published for 01.04.2027, „Die
//! API-Guideline verbleibt daher zunächst auf dem aktuell produktiven Stand",
//! and „Die im Rahmen der Konsultation neu vorgesehenen APIs werden hingegen
//! zurückgezogen" — the API-Webdienste stay limited to MaLo-Ident and Steuern,
//! „Die UTILTS bleibt damit erhalten". The `2.0.0` tag in
//! `EDI-Energy/api-electricity` adds new process APIs, among them
//! `locationBundle` and `calculationFormula` — the Lokationsbündel and
//! Berechnungsformel APIs Nr. 58 names as withdrawn. No Mitteilung makes 2.0.0
//! binding, so nothing here implements it.
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
