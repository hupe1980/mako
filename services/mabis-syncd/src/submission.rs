//! Where a Summenzeitreihe is filed — bilateral BIKO today, MaBiS-Hub later.
//!
//! # Why this seam exists before the Hub does
//!
//! BK6-24-210 will replace bilateral BIKO submission with a central **MaBiS-Hub**
//! that routes **exclusively by MaLo-ID**. Today mako files a Summenzeitreihe
//! under its MaBiS-Zählpunkt (`LOC+172`) for a Bilanzierungsgebiet (`LOC+107`),
//! and the aggregation step groups MaLos *by Bilanzierungsgebiet* before
//! building one series per territory.
//!
//! That grouping is the part the cutover invalidates: under the Hub there is no
//! territory-level series to build, and neither identifier is the routing key.
//! Putting the target behind this enum now means the cutover changes the
//! submission target and the aggregation key — not every call site that touches
//! a series.
//!
//! # Why the Hub arm refuses instead of guessing
//!
//! There is **no Beschluss**. The H1-2026 target slipped, the -1 consultation
//! closed 17.11.2025, and go-live is planned for H2 2028. No wire format, no
//! endpoint and no payload shape has been published, so an implementation would
//! be invention rather than compliance — and an invented format that reaches a
//! real Hub is indistinguishable, at the point of failure, from a correct one
//! that was rejected. The arm therefore fails loudly at configuration time.
//!
//! # What the cutover will need
//!
//! Recorded here so the audit does not have to be redone:
//!
//! - **Aggregation key.** `sync_engine::resolve_bilanzierungsgebiete` groups
//!   MaLos by Bilanzierungsgebiet. The Hub routes by MaLo-ID, so this becomes a
//!   per-MaLo submission, not a per-territory one.
//! - **`mabis_zp_id`.** Resolved from `marktd` master data and carried as the
//!   `LOC+172` Meldepunkt. The Hub does not use it for routing; whether it stays
//!   as payload content is a format question for the Beschluss.
//! - **Tranchen.** `marktd.tranche` keys on `tranche_id` with a parent
//!   `malo_id`. A Tranche is not a MaLo, so any series built per Tranche needs a
//!   resolution rule before it can be routed by MaLo-ID.

use serde::Deserialize;

/// Where `mabis-syncd` files its Summenzeitreihen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmissionTarget {
    /// Bilateral submission to the Bilanzkoordinator — the process in force.
    ///
    /// MSCONS 13003 per Bilanzierungsgebiet, dispatched through `makod`.
    #[default]
    BikoBilateral,
    /// Central MaBiS-Hub per BK6-24-210 — **not yet specified**.
    MabisHub,
}

impl SubmissionTarget {
    /// Reject a target that cannot be honoured, at startup rather than mid-run.
    ///
    /// A run that reaches its first submission before discovering the target is
    /// unimplemented has already aggregated a month of metering data and
    /// consumed the run's version number.
    ///
    /// # Errors
    ///
    /// Returns an error for [`SubmissionTarget::MabisHub`] until BK6-24-210 is
    /// published.
    pub fn ensure_supported(self) -> Result<(), UnsupportedTarget> {
        match self {
            Self::BikoBilateral => Ok(()),
            Self::MabisHub => Err(UnsupportedTarget),
        }
    }

    /// Stable label for logs and run records.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::BikoBilateral => "biko-bilateral",
            Self::MabisHub => "mabis-hub",
        }
    }
}

/// The configured submission target has no implementation yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "submission target `mabis-hub` is not implemented: BK6-24-210 has no Beschluss \
     (H1-2026 target slipped; -1 consultation closed 17.11.2025; go-live planned \
     H2 2028), so no wire format, endpoint or payload shape is published. Use \
     `biko-bilateral` until it is — filing against an invented Hub format cannot \
     be distinguished from a correct submission that was rejected."
)]
pub struct UnsupportedTarget;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilateral_is_the_default_and_is_supported() {
        assert_eq!(SubmissionTarget::default(), SubmissionTarget::BikoBilateral);
        assert!(SubmissionTarget::BikoBilateral.ensure_supported().is_ok());
    }

    #[test]
    fn the_hub_refuses_rather_than_inventing_a_format() {
        let err = SubmissionTarget::MabisHub
            .ensure_supported()
            .expect_err("must refuse");
        let msg = err.to_string();
        assert!(
            msg.contains("BK6-24-210"),
            "the refusal must cite why: {msg}"
        );
        assert!(msg.contains("biko-bilateral"), "and name the way forward");
    }

    #[test]
    fn targets_parse_from_kebab_case_config() {
        let t: SubmissionTarget = serde_json::from_str("\"biko-bilateral\"").unwrap();
        assert_eq!(t, SubmissionTarget::BikoBilateral);
        let t: SubmissionTarget = serde_json::from_str("\"mabis-hub\"").unwrap();
        assert_eq!(t, SubmissionTarget::MabisHub);
        assert!(serde_json::from_str::<SubmissionTarget>("\"biko\"").is_err());
    }

    #[test]
    fn labels_are_stable_for_run_records() {
        assert_eq!(SubmissionTarget::BikoBilateral.label(), "biko-bilateral");
        assert_eq!(SubmissionTarget::MabisHub.label(), "mabis-hub");
    }
}
