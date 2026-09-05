#![deny(unsafe_code)]
#![allow(clippy::doc_markdown)]
//! Business-process observability library for the German energy market (MaKo).
//!
//! Provides a per-process event projection read-model (`ProcessProjection`)
//! that answers questions the native `makod` Prometheus metrics cannot:
//!
//! - *Which MaLo-ID is stuck in `AperakTimeout` for > 3 days?*
//! - *Which NB GLN has the highest rejection rate this month?*
//! - *What is our APERAK Frist compliance rate per PID for BNetzA Q4 reporting?*
//!
//! ## Event mapping
//!
//! | `ce_type`                    | Resulting `ProcessState` |
//! |------------------------------|--------------------------|
//! | `de.mako.process.initiated`  | `Initiated`              |
//! | `de.mako.aperak.accepted`    | `Running`                |
//! | `de.mako.aperak.rejected`    | `Rejected` + ERC code    |
//! | `de.mako.aperak.timeout`     | `AperakTimeout`          |
//! | `de.mako.process.completed`  | `Completed`              |
//! | `de.mako.process.failed`     | `Failed`                 |
//!
//! ## Deadline risk classification
//!
//! Windows come from `mako_fristen::antwort`, never from a literal beside a
//! call site:
//!
//! | Process family | Regulatory deadline | Source |
//! |---|---|---|
//! | GPKE Strom | a clock time on the 1. WT nach dem ÜT (11:00 / 06:00 / 05:00 / 09:00) | BK6-24-174 GPKE Teil 2 |
//! | WiM, beide Sparten | 3 / 5 / 7 / 1 Werktage je PID | BK6-22-024 Anlage 2a · AWH WiM Gas 2.0 |
//! | GeLi Gas | Ablauf des 4. / 3. / 2. Werktags | BK7-24-01-009 |
//!
//! | Module | Content |
//! |---|---|
//! | [`domain`] | Core types: [`domain::ProcessProjection`], [`domain::ProcessState`], [`domain::KpiReport`] |
//! | [`repository`] | Async repository traits |
//! | [`error`] | [`error::ObsError`] error type |
//! | [`testing`] | In-memory impls (behind `testing` feature) |

pub mod domain;
pub mod error;
pub mod repository;

#[cfg(feature = "testing")]
pub mod testing;
