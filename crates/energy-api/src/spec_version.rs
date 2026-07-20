//! Specification versions implemented by this crate.
//!
//! Each constant identifies the OpenAPI / AsyncAPI document version that the
//! corresponding module implements. All four EDI-Energy API-Webdienste are
//! currently at version **1.0.0** — the only tag in either spec repository.
//!
//! The electricity specs live in `github.com/EDI-Energy/api-electricity`; the
//! Verzeichnisdienst is a **separate** repository,
//! `github.com/EDI-Energy/api-directory-service`.
//!
//! See [`RELEASE_2_0_0_STATUS`] for why 2.0.0 is tracked but not implemented.
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

// ── Release 2.0.0 — tracked, deliberately not implemented ────────────────────

/// Why this crate targets 1.0.0 and not the announced 2.0.0.
///
/// Mitteilung Nr. 55 (02.02.2026) put **API-Webdienste Strom Release 2.0.0** out
/// for consultation with a target date of 01.10.2026. **Mitteilung Nr. 56**
/// (01.04.2026) then excluded it, verbatim:
///
/// > Die im Release 2.0.0 zur Konsultation gestellten Anpassungen an den
/// > API-Webdiensten sind nicht Bestandteil dieser Veröffentlichung.
///
/// Only **API Guideline 1.0b** binds on 01.10.2026. In the spec repository
/// (`github.com/EDI-Energy/api-electricity`) the only tag is **`1.0.0`**; the
/// 2.0.0 material lives solely on the `2026-07-31-consultation` branch, which
/// is still moving. BNetzA's own link to a `2.0.0` release tag returns 404.
///
/// Implementing against it now would be rework against an unfrozen contract.
pub const RELEASE_2_0_0_STATUS: &str =
    "deferred by Mitteilung Nr. 56; consultation branch only, no 2.0.0 tag";

/// What 2.0.0 actually changes, from the consultation branch.
///
/// It is **not** a decomposition of MaLo-Ident: `maloIdentV2.yaml` keeps all
/// three operations. The changes are:
///
/// 1. **Modularisation** — a shared `schema/` library referenced by relative
///    `$ref`, replacing the self-contained single-file specs of 1.x.
/// 2. **Six new process APIs** alongside maloIdent — `locationBundle`,
///    `calculationFormula`, `countingTime`, `powerCurve`, `switchingTime`,
///    `complaintDefinition`.
/// 3. **Property renames** in `identificationParameterId`:
///    `maloId` → `marketLocationId`, `tranchenIds` → `marketTranchesIds`,
///    `meloIds` → `meterLocationsIds`. The *component* stays `maloId`.
///    [`crate::models::electricity`] implements the **1.x** names, pinned by a
///    wire-contract test.
/// 4. English operation paths (`/controlMeasure/configuration/v2`) replacing the
///    German ones (`/[Post]/steuerbefehl/konfiguration/`).
/// 5. An expanded error set (415/422/429/503/504) and `testFlag` / `referenceId`
///    headers per the API Guideline.
pub const RELEASE_2_0_0_SCOPE: &str =
    "modularised schema/ library + 6 new process APIs + identificationParameterId renames";
