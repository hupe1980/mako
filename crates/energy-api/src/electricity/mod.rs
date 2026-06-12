//! EDI-Energy electricity market APIs:
//!
//! - [`control_measures`] — Grid control commands (Steuerungshandlungen) between
//!   NB/LF and MSB, per BNetzA decision BK6-22-128 and `controlMeasuresV1.yaml`.
//! - [`malo_ident`] — MaLo-ID retrieval for the 24h supplier-switch process
//!   (GPKE part 2), per BNetzA decision BK6-22-024 and `maloIdentV1.yaml`.

pub mod control_measures;
pub mod malo_ident;
