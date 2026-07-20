//! `mabis-syncd` — MaBiS Summenzeitreihe synchronisation daemon.
//!
//! ## Purpose
//!
//! `mabis-syncd` aggregates per-MaLo Lastgang time series from `edmd` and submits
//! **Summenzeitreihen** to the BIKO (Bilanzkoordinator) as MSCONS PID 13003.
//!
//! It is the production implementation of the aggregation pipeline described in
//! `mako-mabis::summenzeitreihe` architecture note:
//!
//! 1. Query `edmd` for per-MaLo Lastgang (`GET /api/v1/summenzeitreihe/{malo_id}`)
//! 2. Aggregate using `mako-mabis::SummenzeitreiheBuilder`
//! 3. Serialise as MSCONS 13003 (via makod's `mabis.summenzeitreihe.uebermitteln`)
//! 4. Submit via AS4 through `makod`
//! 5. Track status in PostgreSQL (`submission_runs` table)
//!
//! ## Schedule (BK6-24-174 Anlage 3 §3.10, Werktage after month end)
//!
//! One scheduled submission per Bilanzierungsmonat: the background scheduler
//! fires at `run_hour_utc` (default 05:00 UTC) on the configured Werktag after
//! period end (`erstaufschlag_werktag`, default 10 — the last Werktag of the
//! Erstaufschlag window). The window a run lands in decides its Datenstatus:
//!
//! | Window | Werktage after period end | Datenstatus of a new version |
//! |---|---|---|
//! | Erstaufschlag (BKA) | ≤ 10 WT | Abrechnungsdaten directly |
//! | Clearing (BKA) | ≤ 30 WT | Prüfdaten, promoted by positive Prüfmitteilung |
//! | KBKA | after 30 WT | Prüfdaten (korrigierte Bilanzkreisabrechnung) |
//!
//! ## Regulatory basis
//!
//! - **BK6-24-174 Anlage 3 MaBiS** — Versionierung (§3.8.2), Datenstatus (§3.8.3), Fristen (§3.10)
//! - **MaBiS (Anlage 3 zur Festlegung BK6-24-174)** — Marktregeln für die
//!   Bilanzkreisabrechnung Strom
//! - **MSCONS AHB 3.2 §8.3.1** — EDIFACT message format for PID 13003
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
//! erstaufschlag_werktag = 10  # Werktag after the Bilanzierungsmonat (default: 10)
//! run_hour_utc          = 5   # 05:00 UTC = 06:00 CET / 07:00 CEST
//! ```

#![deny(unsafe_code)]

pub mod config;
pub mod pg;
pub mod server;
pub mod sync_engine;
