//! Client implementations for the EDI-Energy electricity market APIs.
//!
//! These are the **Lieferant / NB** side — callers that consume APIs offered
//! by MSB or NB endpoints.
//!
//! | Module             | API                              | Spec              |
//! |--------------------|----------------------------------|-------------------|
//! | [`control_measures`] | Grid control commands (NB/LF → MSB and responses) | `controlMeasuresV1.yaml` |
//! | [`malo_ident`]     | MaLo-ID retrieval (LF → NB)      | `maloIdentV1.yaml`|

pub mod control_measures;
pub mod malo_ident;

pub use control_measures::ControlMeasuresClient;
pub use malo_ident::MaloIdentClient;
