//! `dvgw-edi` — DVGW EDIFACT for the German gas transport and balancing market.
//!
//! Parses, validates and writes the DVGW-governed formats used in gas balancing
//! (`GaBi` Gas 2.1, `BNetzA` BK7-24-01-008). The DVGW counterpart to `edi-energy`,
//! which covers the BDEW EDI@Energy retail layer.
//!
//! # The one thing to know first
//!
//! **A DVGW message does not name itself in `UNH`.** Every format is a subset of
//! a UN/EDIFACT D.07A message, so `UNH` carries the *carrier* — `ORDERS` or
//! `ORDRSP` — and `BGM` C002 DE 1001 carries the message:
//!
//! ```text
//! UNH+1+ORDERS:D:07A:UN:DVGW18'      ← the carrier
//! BGM+01G::332+NOMINT00052'          ← *this* says NOMINT
//! DTM+Z05:0:805'                     ← timestamps below are UTC
//! DTM+137:201801042056:203'          ← message date/time
//! DTM+Z01:201801050400201801060400:719'  ← Gültigkeitszeitraum = the gas day
//! RFF+Z13:70030'                     ← Prüfidentifikator
//! ```
//!
//! Matching `UNH` against `"NOMINT"` therefore rejects every conformant message.
//! Identity here is resolved from [`DvgwDocument`], with the carrier as a
//! cross-check.
//!
//! # Supported formats
//!
//! | Message | Carrier | Document codes (`BGM` DE 1001) | Prüfidentifikatoren |
//! |---|---|---|---|
//! | **ALOCAT** — Allokationsnachricht | `ORDRSP` | `X1G X2G X3G X4G X5G X6G X7G XBG` | 70001–70023 |
//! | **NOMINT** — Nominierung | `ORDERS` | `01G 55G Y1G Y6G Y7G` | 70030–70034 |
//! | **NOMRES** — Nominierungsantwort | `ORDRSP` | `07G 08G 19G 20G Y2G` | 70035–70039 |
//! | **SSQNOT** — Mehr-/Mindermengenmeldung | `ORDRSP` | `BAG` | 70095–70096 |
//!
//! `CONTRL` and `APERAK` acknowledge DVGW interchanges but are BDEW formats;
//! they live in `edi-energy` and are not reimplemented here.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use dvgw_edi::{DvgwMessageType, DvgwPlatform};
//!
//! # let raw: &[u8] = b"";
//! let platform = DvgwPlatform::default();
//! for result in platform.parse_interchange(raw) {
//!     let msg = result?;
//!     println!("{} ({})", msg.message_type, msg.document.description());
//!
//!     // The gas day is DTM+Z01, decoded through its own format code.
//!     if let Some(period) = msg.validity_period {
//!         println!("  Gastag {period}");
//!     }
//!     // A LOC group carries a time series, not one value.
//!     for qty in msg.quantities() {
//!         println!("  {:?} {:?}", qty.value, qty.period);
//!     }
//!     if msg.message_type == DvgwMessageType::Nomint {
//!         // RFF+AGO — the nomination a re-nomination corrects. A NOMRES has no
//!         // such reference and is paired on the business key instead.
//!         println!("  korrigiert {:?}", msg.original_nomination_ref());
//!     }
//!
//!     let report = DvgwPlatform::validate_message(&msg);
//!     for issue in report.errors() {
//!         eprintln!("  {issue}");
//!     }
//! }
//! # Ok::<(), dvgw_edi::Error>(())
//! ```
//!
//! # Market roles
//!
//! | Role | Abbreviation |
//! |---|---|
//! | Fernleitungsnetzbetreiber | FNB |
//! | Verteilnetzbetreiber | VNB |
//! | Bilanzkreisverantwortlicher | BKV |
//! | Marktgebietsverantwortlicher | MGV |
//!
//! # Regulatory references
//!
//! - **§ 20 Abs. 3 `EnWG`** — Festlegungskompetenz for gas network access and balancing
//! - **`BNetzA` BK7-24-01-008** — `GaBi` Gas 2.1
//! - **Kooperationsvereinbarung Gas (`KoV`)** — nomination and allocation deadlines
//! - DVGW-Nachrichtenbeschreibungen: <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas>
//!
//! # Relationship to other crates
//!
//! | Crate | Layer |
//! |---|---|
//! | `dvgw-edi` | EDIFACT parsing / validation / writing — **this crate** |
//! | `mako-gabi-gas` | `GaBi` Gas process engine (workflows, deadlines) |
//! | `edi-energy` | BDEW EDI@Energy formats (UTILMD, MSCONS, APERAK, …) |

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::return_self_not_must_use)]

mod builder;
mod datetime;
mod document;
mod error;
mod message;
mod platform;
mod pruefidentifikator;
mod report;
mod validate;
mod version;
mod zuordnung;

/// The typed message model: positions, locations, quantities and parties.
pub mod model;
/// SSQNOT read as one Mehr-/Mindermengen record.
pub mod ssqnot;

pub use builder::{MessageBuilder, Position};
pub use datetime::{DtmFormat, DtmValue, DvgwPeriod};
pub use document::{Carrier, DVGW_AGENCY_CODE, DvgwDocument, DvgwMessageType};
pub use error::Error;
pub use message::DvgwMessage;
pub use model::{
    EnergyByQualifier, ItemDescription, LineItem, LocationGroup, Party, Quantity, Reference,
};
pub use platform::{DvgwPlatform, sniff};
pub use pruefidentifikator::{
    PID_MAX, PID_MIN, PidInfo, Pruefidentifikator, SSQNOT_RLM_CUTOFF, catalogue, catalogue_for,
};
pub use report::{DvgwIssue, DvgwReport, Severity};
pub use version::DvgwVersion;
pub use zuordnung::{CorrelationKey, Zuordnung, assigned_pids};
