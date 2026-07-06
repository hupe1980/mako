//! `invoicd` — INVOIC plausibility-check daemon for the Lieferant (LF) role.
//!
//! ## Architecture
//!
//! ```text
//! mdmd ──(POST /webhook)──► invoicd handler
//!                               │
//!                     parse MdmEvent JSON
//!                               │
//!                     ┌─────────▼──────────┐
//!                     │  ce_type routing    │
//!                     └─────────┬──────────┘
//!                               │
//!          ┌────────────────────┼───────────────────────────┐
//!          ▼                    ▼                           ▼
//!  "de.mako.process.   "de.mako.process.       all others
//!   initiated"         completed" + pid==27003  → 204 No Content
//!  + pid in INVOIC set → seed TariffStore
//!          │
//!  run InvoicCheckEngine::check()
//!          │
//!   ┌──────┴──────┐
//!   │             │
//!  Ok           Dispute
//!   │             │
//!  POST         POST
//!  /api/v1/     /api/v1/
//!  commands     commands
//!  (annehmen)   (ablehnen)
//! ```
//!
//! ## Configuration
//!
//! All settings can be provided as CLI flags or environment variables
//! (env takes precedence, as per `clap`'s `env` attribute):
//!
//! | Flag                       | Env var                        | Default                    |
//! |----------------------------|--------------------------------|----------------------------|
//! | `--listen`                 | `INVOICD_LISTEN`               | `0.0.0.0:8280`             |
//! | `--makod-url`              | `INVOICD_MAKOD_URL`            | `http://localhost:8180`    |
//! | `--mdmd-url`               | `INVOICD_MDMD_URL`             | `http://localhost:9180`    |
//! | `--subscriber-id`          | `INVOICD_SUBSCRIBER_ID`        | `invoicd`                  |
//! | `--webhook-url`            | `INVOICD_WEBHOOK_URL`          | *(required)*               |
//! | `--webhook-secret`         | `INVOICD_WEBHOOK_SECRET`       | *(optional)*               |
//! | `--inbound-secret`         | `INVOICD_INBOUND_SECRET`       | *(optional)*               |
//! | `--arithmetic-tolerance`   | `INVOICD_ARITHMETIC_TOLERANCE` | `0.01`                     |
//! | `--total-tolerance`        | `INVOICD_TOTAL_TOLERANCE`      | `0.01`                     |
//! | `--tariff-tolerance`       | `INVOICD_TARIFF_TOLERANCE`     | `0.03`                     |
//! | `--require-tariff`         | `INVOICD_REQUIRE_TARIFF`       | `false`                    |
//! | `--auto-dispute-threshold` | `INVOICD_AUTO_DISPUTE_THRESHOLD`| `0.0` (always approve Warn)|
//!
//! ## Subscription registration
//!
//! At startup `invoicd` calls `PUT /api/v1/subscriptions/invoicd` on `mdmd`
//! to ensure it receives `de.mako.process.initiated` events.  The idempotent
//! `PUT` is safe to call on every restart.

pub mod config;
pub mod handler;
pub mod makod_client;
pub mod server;
pub mod tariff_store;
