//! The `[mabis]` section of `netzbilanzd.toml`.
//!
//! The module files under the host's tenant and through the host's `edmd`,
//! `marktd` and `makod` upstreams; this section adds only what a MaBiS filing
//! needs and the billing surface does not: who files, who receives, for which
//! Bilanzierungsgebiet, where, and when.

use serde::Deserialize;

use super::submission::SubmissionTarget;

/// `[mabis]` — present when this Netzbetreiber files Summenzeitreihen.
///
/// Absent, the MaBiS routes are not mounted and no scheduler runs: a Gas-only
/// Netzbetreiber bills from this daemon and files no MSCONS 13003.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MabisConfig {
    /// MP-ID of the sender (ÜNB / NB). Used in MSCONS `NAD+MS`.
    pub sender_mp_id: String,
    /// MP-ID of the BIKO receiver. Used in MSCONS `NAD+MR`.
    pub receiver_mp_id: String,
    /// Fallback Bilanzierungsgebiet for MaLos whose master data does not name
    /// one.
    ///
    /// The authoritative value is `marktd`'s per-MaLo `bilanzierungsgebiet`;
    /// this is only used when that lookup returns nothing, and such MaLos are
    /// logged rather than silently folded into the fallback zone.
    pub bilanzierungsgebiet_id: String,
    /// Where Summenzeitreihen are filed. See [`SubmissionTarget`]; an
    /// unimplemented target is refused at startup rather than mid-run.
    #[serde(default)]
    pub submission_target: SubmissionTarget,
    /// When the scheduled submission runs.
    #[serde(default)]
    pub schedule: ScheduleConfig,
}

/// `[mabis.schedule]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleConfig {
    /// Werktag after the Bilanzierungsmonat on which to submit.
    ///
    /// BK6-24-174 Anlage 3 §3.10, Tabelle 2: the Erstaufschlag window for a
    /// BG-SZR is the 1.–10. Werktag. Submitting on the last of them maximises
    /// the input data while the BIKO still assigns `Abrechnungsdaten` directly;
    /// a version sent later starts as `Prüfdaten`.
    #[serde(default = "default_erstaufschlag_werktag")]
    pub erstaufschlag_werktag: u32,
    /// UTC hour (0–23) to run submissions. Default: 5 (= 06:00 CET).
    #[serde(default = "default_run_hour")]
    pub run_hour_utc: u8,
}

fn default_erstaufschlag_werktag() -> u32 {
    10
}
fn default_run_hour() -> u8 {
    5
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            erstaufschlag_werktag: default_erstaufschlag_werktag(),
            run_hour_utc: default_run_hour(),
        }
    }
}

impl MabisConfig {
    /// Refuse a section that cannot produce a valid submission.
    ///
    /// Called from the daemon's `build`, which is the only start-up path there
    /// is. Both checks exist to fail here rather than at 05:00 on the
    /// Erstaufschlag-Werktag, by which point a run has aggregated a month of
    /// metering data and consumed its version number.
    ///
    /// # Errors
    ///
    /// - The configured [`SubmissionTarget`] has no implementation.
    /// - `bilanzierungsgebiet_id` is not a Y-type (Area) EIC. A Bilanzkreis is
    ///   type `X` (Party) and the same length, so only the object type
    ///   separates them — and `LOC+107` carries the value as free text, which
    ///   means the BIKO would accept either.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.submission_target.ensure_supported()?;
        rubo4e::identifiers::BilanzierungsgebietId::new(&self.bilanzierungsgebiet_id).map_err(
            |e| {
                anyhow::anyhow!(
                    "[mabis] bilanzierungsgebiet_id `{}` is not a Bilanzierungsgebiet-EIC: {e}. \
                     A Bilanzierungsgebiet is a 16-character EIC of ENTSO-E object type `Y` \
                     (Area); a Bilanzkreis is type `X` (Party) and belongs in a different field.",
                    self.bilanzierungsgebiet_id
                )
            },
        )?;
        Ok(())
    }
}

/// What the module runs on: the host's tenant and the `[mabis]` section.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique
    /// string is valid.
    pub tenant: String,
    /// The `[mabis]` section.
    pub mabis: MabisConfig,
}
