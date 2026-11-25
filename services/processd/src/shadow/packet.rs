//! Decode and validate the pinned local review file before installing a generation.
use super::{Settings, digest};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read};
use utoipa::ToSchema;

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub pid: u32,
    pub tree: String,
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Deadline {
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub due_at: time::OffsetDateTime,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub expected: Answer,
    pub actual: Answer,
    pub expected_fingerprint: String,
    pub facts_fingerprint: String,
    pub answer_sha256: String,
    pub release: String,
    pub ebd_revision: String,
    pub pruefschritt: u32,
    pub timing: String,
    pub observation_fingerprint: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub incumbent_received_at: Option<time::OffsetDateTime>,
    pub deadline: Deadline,
    pub producer: String,
}

impl Assessment {
    pub(super) fn differs(&self) -> bool {
        self.expected != self.actual || self.timing == "late"
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Occurrence {
    pub id: String,
    pub batch_revision: String,
    pub request_sha256: String,
    pub assessment: Option<Assessment>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub journal: String,
    pub inventory: String,
    pub observed: String,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Cohort {
    pub version: u32,
    pub nb: String,
    pub role: String,
    pub scope: String,
    pub coverage: Coverage,
    pub directions: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub start: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub end: time::OffsetDateTime,
    pub complete_werktage: u32,
    pub rows: Vec<Occurrence>,
}

impl Cohort {
    pub(super) fn load(settings: &Settings, nb: &str) -> anyhow::Result<Self> {
        const LIMIT: u64 = 16 * 1024 * 1024;
        ensure!(
            std::fs::symlink_metadata(&settings.file)?.is_file(),
            "expected a regular local review file"
        );
        let file = std::fs::File::open(&settings.file).context("open private review file")?;
        ensure!(file.metadata()?.is_file(), "expected regular review file");
        let mut bytes = Vec::new();
        file.take(LIMIT + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= LIMIT
                && format!("{:x}", Sha256::digest(&bytes)) == settings.sha256,
            "review file exceeds limit or differs from configured digest"
        );
        let cohort: Self = serde_json::from_slice(&bytes).context("invalid review file")?;
        ensure!(
            cohort.version == 1
                && cohort.role == "NB"
                && cohort.nb == nb
                && cohort.scope == settings.scope,
            "review scope mismatch"
        );
        ensure!(
            [
                &cohort.coverage.journal,
                &cohort.coverage.inventory,
                &cohort.coverage.observed
            ]
            .iter()
            .all(|v| digest(v)),
            "missing coverage fingerprints"
        );
        let (start, end) = (cohort.start, cohort.end);
        ensure!(
            start < end && (end - start).whole_days() <= 3660,
            "invalid review interval"
        );
        let mut day = mako_fristen::berlin_date(start);
        let end_day = mako_fristen::berlin_date(end);
        let mut covered = 0;
        while day < end_day {
            if mako_fristen::berlin_midnight(day) >= start
                && mako_fristen::is_werktag(day, mako_fristen::HolidayCalendar::BdewMaKo)
            {
                covered += 1;
            }
            day = day.next_day().context("review interval exceeds calendar")?;
        }
        ensure!(
            covered == cohort.complete_werktage,
            "complete Werktage differ from the covered interval"
        );
        ensure!(
            matches!(cohort.directions.as_slice(), [d] if d == "inbound" || d == "outbound")
                || cohort.directions == ["inbound", "outbound"],
            "invalid review directions"
        );
        ensure!(
            cohort.rows.is_empty() || cohort.directions.iter().any(|d| d == "inbound"),
            "outbound-only review contains inbound rows"
        );
        let mut ids = BTreeSet::new();
        ensure!(cohort.rows.len() <= 10_000, "review exceeds row limit");
        for row in &cohort.rows {
            ensure!(
                digest(&row.id)
                    && ids.insert(&row.id)
                    && digest(&row.request_sha256)
                    && digest(&row.batch_revision),
                "invalid occurrence"
            );
            if let Some(a) = &row.assessment {
                ensure!(
                    a.producer == "makod"
                        && match a.timing.as_str() {
                            "unassessed" =>
                                a.observation_fingerprint.is_none()
                                    && a.incumbent_received_at.is_none(),
                            "on_time" | "late" =>
                                a.observation_fingerprint.as_deref().is_some_and(digest)
                                    && a.incumbent_received_at
                                        .is_some_and(
                                            |t| (t > a.deadline.due_at) == (a.timing == "late")
                                        ),
                            _ => false,
                        }
                        && [
                            &a.facts_fingerprint,
                            &a.expected_fingerprint,
                            &a.answer_sha256
                        ]
                        .iter()
                        .all(|v| digest(v)),
                    "expected an isolated makod assessment"
                );
            }
        }
        Ok(cohort)
    }
}
