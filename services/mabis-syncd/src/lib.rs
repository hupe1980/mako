//! `mabis-syncd` — MaBiS UTILTS synchronisation daemon.
//!
//! ## Purpose
//!
//! `mabis-syncd` aggregates per-MaLo Lastgang time series from `edmd` and submits
//! **Summenzeitreihen** to the BIKO (Bilanzkoordinator) via UTILTS messages.
//!
//! It is the production implementation of the aggregation pipeline described in
//! `mako-mabis::utilts_aggregation` architecture note:
//!
//! 1. Query `edmd` for per-MaLo Lastgang (`GET /api/v1/summenzeitreihe/{malo_id}`)
//! 2. Aggregate using `mako-mabis::SummenzeitreiheBuilder`
//! 3. Serialise via `edi-energy` UTILTS encoder (via makod)
//! 4. Submit via AS4 through `makod`
//! 5. Track status in PostgreSQL (`submission_runs` table)
//!
//! ## Schedule
//!
//! | Version | Trigger | Deadline (BK6-22-024 Anlage 3 MaBiS) |
//! |---|---|---|
//! | vorlaeufig | 3rd of month at 06:00 CET | ≤ day 3 after period end |
//! | endgueltig | 8th of month at 06:00 CET | ≤ day 8 after period end |
//!
//! ## Regulatory basis
//!
//! - **BK6-22-024 Anlage 3 MaBiS** — UTILTS submission deadlines and format
//! - **§4 StromNZV** — legal mandate for balance group accounting
//! - **UTILTS AHB S1.0 / S2.0** — EDIFACT message format
//!
//! ## Configuration (`mabis-syncd.toml`)
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8880"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! tenant        = "9900357000004"
//! sender_mp_id  = "9900357000004"   # ÜNB / NB BDEW code
//! receiver_mp_id = "9900077000006"  # BIKO BDEW code (Transnet BW etc.)
//!
//! [edmd]
//! url     = "http://edmd:8380"
//! api_key = "env:MABIS_EDMD_API_KEY"
//!
//! [makod]
//! url     = "http://makod:8080"
//! api_key = "env:MABIS_MAKOD_API_KEY"
//!
//! [schedule]
//! preliminary_day = 3   # day of month to submit vorlaeufig (default: 3)
//! final_day       = 8   # day of month to submit endgueltig (default: 8)
//! run_hour_utc    = 5   # 05:00 UTC = 06:00 CET / 07:00 CEST
//! ```

#![deny(unsafe_code)]

pub mod config;
pub mod pg;
pub mod server;
pub mod sync_engine;
