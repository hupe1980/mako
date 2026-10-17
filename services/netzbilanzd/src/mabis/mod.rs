//! MaBiS Summenzeitreihen — the Netzbetreiber's filing with the BIKO.
//!
//! The module aggregates per-MaLo Lastgang time series from `edmd` and submits
//! **Summenzeitreihen** to the BIKO (Bilanzkoordinator) as MSCONS PID 13003:
//!
//! 1. Query `edmd` for the MaLos of the period (`GET /api/v1/billing-periods`)
//!    and each one's projected series (`GET /api/v1/energy/{malo_id}`)
//! 2. Aggregate using `mako-mabis::SummenzeitreiheBuilder`, one series per
//!    Bilanzierungsgebiet
//! 3. Serialise as MSCONS 13003 (via makod's `mabis.summenzeitreihe.uebermitteln`)
//! 4. Submit via AS4 through `makod`
//! 5. Track status in PostgreSQL (`submission_runs`, `submission_series`)
//!
//! It is mounted only when `netzbilanzd.toml` carries a `[mabis]` section
//! ([`config::MabisConfig`]).
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
//! - **MSCONS AHB 3.2 §8.3.1** — EDIFACT message format for PID 13003
//!
//! ## Configuration (`[mabis]` in `netzbilanzd.toml`)
//!
//! ```toml
//! edmd_url     = "http://edmd:8380"   # required when [mabis] is present
//! edmd_api_key = "env:NETZBILANZD_EDMD_API_KEY"
//!
//! [mabis]
//! sender_mp_id           = "9900357000004"     # NB / ÜNB, NAD+MS
//! receiver_mp_id         = "9900077000006"     # BIKO, NAD+MR
//! bilanzierungsgebiet_id = "11YMAKO-TEST-01U"  # Y-type (Area) EIC
//!
//! [mabis.schedule]
//! erstaufschlag_werktag = 10  # Werktag after the Bilanzierungsmonat (default: 10)
//! run_hour_utc          = 5   # 05:00 UTC = 06:00 CET / 07:00 CEST
//! ```

pub mod config;
pub mod pg;
pub mod server;
pub mod submission;
pub mod sync_engine;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use self::config::Settings;
use self::sync_engine::SyncEngine;

/// Start the scheduler — one scheduled submission per Bilanzierungsmonat.
///
/// BK6-24-174 Anlage 3 §3.10, Tabelle 2: the Erstaufschlag window for a BG-SZR
/// runs to the 10. Werktag after the Bilanzierungsmonat. Submitting on that last
/// Werktag gives the aggregate the most complete input while the BIKO still
/// assigns 'Abrechnungsdaten' automatically — a later version starts as
/// 'Prüfdaten' and needs a positive Prüfmitteilung to settle.
pub fn spawn_scheduler(
    engine: Arc<SyncEngine>,
    settings: Arc<Settings>,
    pool: sqlx::PgPool,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let submit_wt = settings.mabis.schedule.erstaufschlag_werktag;
        let run_hour = settings.mabis.schedule.run_hour_utc;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(tokio::time::Duration::from_secs(300)) => {}
            }
            let now = time::OffsetDateTime::now_utc();
            if u32::from(now.hour()) != u32::from(run_hour) {
                continue;
            }

            // The Bilanzierungsmonat and its Werktag calendar are civil, so the
            // due date is compared against the Berlin date — a UTC date is a day
            // behind for the first hour of every local day.
            let today = mako_fristen::berlin_date(now);
            let (from, to) = sync_engine::previous_month_period(today);
            let due =
                mako_fristen::add_werktage(to, submit_wt, mako_fristen::HolidayCalendar::BdewMaKo);
            if today != due {
                continue;
            }

            // The loop wakes every 5 minutes and the hour test stays true for
            // the whole `run_hour`, so without this guard the same binding
            // Summenzeitreihe would go out a dozen times. A failed run is not a
            // filing, so it does not block the retry.
            match pg::has_live_run_for_period(
                &pool,
                &settings.tenant,
                &settings.mabis.bilanzierungsgebiet_id,
                from,
                to,
            )
            .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "netzbilanzd mabis: cannot check for an existing run — skipping this \
                         tick rather than risking a duplicate filing"
                    );
                    continue;
                }
            }

            match engine.run_aggregation(from, to, None, None).await {
                Ok(id) => tracing::info!(
                    run_id = %id, werktag = submit_wt,
                    "netzbilanzd mabis: scheduled Summenzeitreihe submission completed"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "netzbilanzd mabis: scheduled Summenzeitreihe submission failed"
                ),
            }
        }
    });
}
